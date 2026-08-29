//! Audited Windows process and Named Pipe boundary.
//!
//! All unsafe code in the supervisor is isolated here. Raw pointers are used
//! only while calling documented Win32 APIs; owned kernel handles immediately
//! become `OwnedHandle`, and LocalAlloc-owned security buffers are released by
//! `LocalFree`.

#![allow(unsafe_code)]

use std::{
    env,
    ffi::c_void,
    io,
    mem::{size_of, size_of_val},
    os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle, RawHandle},
    process::{ExitStatus, Stdio},
    ptr::{self, NonNull},
    time::Duration,
};

use latentdeck_control::{
    Ack, AuthToken, Command, Envelope, Event, InboundPolicy, MAX_CONTROL_FRAME_BYTES, Message,
    SessionValidator, ShutdownReason, WireUuid, WorkerShutdown, decode_envelope, encode_envelope,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions},
    process::{Child, Command as ProcessCommand},
    time::{Instant, timeout_at},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, LocalFree},
    Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    },
    Security::{
        GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Pipes::GetNamedPipeClientProcessId,
        Threading::{CREATE_NO_WINDOW, GetCurrentProcess, OpenProcessToken},
    },
};

#[cfg(test)]
use windows_sys::Win32::{
    Foundation::GENERIC_ALL,
    Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation, GetAce,
        GetAclInformation, GetSecurityDescriptorDacl,
    },
    System::JobObjects::QueryInformationJobObject,
};

use super::{ValidatedWorkerLaunch, WorkerExit, WorkerSupervisorError, encode_bootstrap};

const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const FORCED_TERMINATION_EXIT_CODE: u32 = 1;
const WORKER_ENVIRONMENT_KEYS: [&str; 4] = ["SystemRoot", "SystemDrive", "TEMP", "TMP"];

/// A spawned worker that has not completed its authenticated hello yet.
pub struct PendingWorker {
    session_id: WireUuid,
    expected_token: AuthToken,
    pipe: NamedPipeServer,
    child: Child,
    job: JobObject,
    worker_pid: u32,
    connect_timeout: Duration,
}

/// One authenticated worker session. A dropped session closes its Job Object,
/// terminating the worker and descendants if they are still running.
pub struct WorkerSession {
    session_id: WireUuid,
    pipe: NamedPipeServer,
    child: Child,
    job: JobObject,
    validator: SessionValidator,
    started_at: Instant,
    worker_pid: u32,
}

/// Create the secure single-client pipe, spawn the exact validated executable,
/// contain it in a kill-on-close Job Object, and deliver its single-use secret
/// over stdin.
///
/// # Errors
///
/// Returns an error without leaving the child running when pipe security,
/// spawn, Job Object assignment, randomness, or bootstrap delivery fails.
pub async fn spawn_worker(
    launch: ValidatedWorkerLaunch,
) -> Result<PendingWorker, WorkerSupervisorError> {
    let session_id = WireUuid::new_v4();
    let pipe_name = format!(r"\\.\pipe\latentdeck-worker-{session_id}");
    let mut token_bytes = [0_u8; 32];
    getrandom::fill(&mut token_bytes).map_err(|_| WorkerSupervisorError::Random)?;
    let expected_token = AuthToken::new(token_bytes);
    token_bytes.fill(0);

    let pipe = create_secure_pipe(&pipe_name)?;
    let job = JobObject::new()?;

    let mut command = ProcessCommand::new(&launch.executable);
    command
        .args(&launch.arguments)
        .current_dir(&launch.working_directory)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .creation_flags(CREATE_NO_WINDOW);
    configure_worker_environment(&mut command)?;
    let mut child = command.spawn().map_err(WorkerSupervisorError::Spawn)?;
    let worker_pid = child.id().ok_or_else(|| {
        WorkerSupervisorError::Spawn(io::Error::other("spawned worker has no process identifier"))
    })?;

    if let Err(error) = job.assign(&child) {
        terminate_uncontained_child(&mut child).await;
        return Err(error);
    }

    let mut bootstrap = encode_bootstrap(session_id, &pipe_name, &expected_token)?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = job.terminate();
        let _ = child.wait().await;
        bootstrap.fill(0);
        return Err(WorkerSupervisorError::BootstrapWrite(io::Error::other(
            "worker stdin was not piped",
        )));
    };
    let write_result = stdin.write_all(&bootstrap).await;
    bootstrap.fill(0);
    drop(stdin);
    if let Err(error) = write_result {
        let _ = job.terminate();
        let _ = child.wait().await;
        return Err(WorkerSupervisorError::BootstrapWrite(error));
    }

    Ok(PendingWorker {
        session_id,
        expected_token,
        pipe,
        child,
        job,
        worker_pid,
        connect_timeout: launch.connect_timeout,
    })
}

fn configure_worker_environment(command: &mut ProcessCommand) -> Result<(), WorkerSupervisorError> {
    // A validated Codec Pack is self-contained. It must not depend on PATH,
    // PYTHONPATH, CUDA variables, or credentials inherited from the desktop
    // process. Windows still expects its root/drive variables in an explicit
    // environment block, while Python and native runtimes need a writable
    // temporary directory. Copy only those four values, rejecting an
    // incomplete host environment rather than silently restoring inheritance.
    for key in WORKER_ENVIRONMENT_KEYS {
        let value = env::var_os(key)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                WorkerSupervisorError::WorkerEnvironment(io::Error::other(format!(
                    "required Windows runtime variable {key} is missing"
                )))
            })?;
        command.env(key, value);
    }
    Ok(())
}

impl PendingWorker {
    /// Authenticate exactly one client and consume the pending supervisor.
    ///
    /// # Errors
    ///
    /// A timeout, early process exit, wrong client PID/token, malformed frame,
    /// or non-hello first message terminates this session. It is never resumed
    /// automatically.
    pub async fn connect(mut self) -> Result<WorkerSession, WorkerSupervisorError> {
        let deadline = Instant::now() + self.connect_timeout;
        let connected = timeout_at(deadline, async {
            tokio::select! {
                biased;
                result = self.pipe.connect() => result.map_err(WorkerSupervisorError::PipeIo),
                status = self.child.wait() => Err(process_exit_error(status)),
            }
        })
        .await
        .map_err(|_| WorkerSupervisorError::ConnectTimeout)?;
        connected?;

        let client_pid = named_pipe_client_pid(&self.pipe)?;
        if client_pid != self.worker_pid {
            let _ = self.job.terminate();
            let _ = self.child.wait().await;
            return Err(WorkerSupervisorError::PeerProcessMismatch);
        }

        let first = timeout_at(deadline, async {
            tokio::select! {
                biased;
                result = read_envelope(&mut self.pipe) => result,
                status = self.child.wait() => Err(process_exit_error(status)),
            }
        })
        .await
        .map_err(|_| WorkerSupervisorError::HandshakeTimeout)??;

        let mut validator =
            SessionValidator::new(self.session_id, InboundPolicy::ResponsesAndEvents);
        validator.validate_inbound(&first)?;
        let Message::Event(event_message) = &first.message else {
            return Err(WorkerSupervisorError::UnexpectedHandshake);
        };
        if event_message.caused_by.is_some() {
            return Err(WorkerSupervisorError::UnexpectedHandshake);
        }
        let Event::WorkerHello(hello) = &event_message.event else {
            return Err(WorkerSupervisorError::UnexpectedHandshake);
        };
        if hello.pid != self.worker_pid || !hello.auth_token.constant_time_eq(&self.expected_token)
        {
            let _ = self.job.terminate();
            let _ = self.child.wait().await;
            return Err(WorkerSupervisorError::AuthenticationFailed);
        }

        Ok(WorkerSession {
            session_id: self.session_id,
            pipe: self.pipe,
            child: self.child,
            job: self.job,
            validator,
            started_at: Instant::now(),
            worker_pid: self.worker_pid,
        })
    }

    #[must_use]
    pub const fn session_id(&self) -> WireUuid {
        self.session_id
    }

    #[must_use]
    pub const fn worker_pid(&self) -> u32 {
        self.worker_pid
    }
}

impl WorkerSession {
    #[must_use]
    pub const fn session_id(&self) -> WireUuid {
        self.session_id
    }

    #[must_use]
    pub const fn worker_pid(&self) -> u32 {
        self.worker_pid
    }

    /// Number of additional worker replies/events this session can validate.
    ///
    /// The authenticated `worker.hello` has already consumed one inbound
    /// message when a `WorkerSession` becomes available.
    #[must_use]
    pub fn remaining_inbound_message_budget(&self) -> usize {
        self.validator.remaining_inbound_message_budget()
    }

    /// Number of additional Core commands this session can register.
    /// Completed command IDs remain retained for duplicate/correlation checks.
    #[must_use]
    pub fn remaining_outbound_message_budget(&self) -> usize {
        self.validator.remaining_outbound_message_budget()
    }

    /// Borrow the authenticated child process handle for a synchronous,
    /// non-retaining operation such as duplicating anonymous ring handles.
    ///
    /// The handle never crosses this callback boundary and is unavailable
    /// after process exit.
    ///
    /// # Errors
    ///
    /// Returns an error when the child was already reaped.
    pub fn with_process_handle<T>(
        &self,
        operation: impl FnOnce(BorrowedHandle<'_>) -> T,
    ) -> Result<T, WorkerSupervisorError> {
        let raw_handle = self
            .child
            .raw_handle()
            .ok_or(WorkerSupervisorError::ProcessHandleUnavailable)?;
        // SAFETY: Tokio owns this live child process handle for `self`; the
        // borrow cannot escape `operation` and no ownership is transferred.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(raw_handle) };
        Ok(operation(borrowed))
    }

    /// Send one typed command using the next validated Core sequence.
    ///
    /// # Errors
    ///
    /// Returns a protocol/session error or a fatal pipe I/O error.
    pub async fn send_command(
        &mut self,
        command: Command,
    ) -> Result<WireUuid, WorkerSupervisorError> {
        let message_id = WireUuid::new_v4();
        let envelope = Envelope::new(
            self.session_id,
            self.validator.next_outbound_sequence(),
            message_id,
            elapsed_ns(self.started_at),
            Message::Command(command),
        );
        self.validator.track_outbound_command(&envelope)?;
        write_envelope(&mut self.pipe, &envelope).await?;
        Ok(message_id)
    }

    /// Receive and validate the next worker reply/event while also observing
    /// process exit.
    ///
    /// # Errors
    ///
    /// Returns on timeout, pipe/protocol failure, or process exit. None of
    /// those errors restarts the worker.
    pub async fn receive(&mut self, timeout: Duration) -> Result<Envelope, WorkerSupervisorError> {
        let deadline = Instant::now() + timeout;
        let envelope = timeout_at(deadline, async {
            tokio::select! {
                biased;
                result = read_envelope(&mut self.pipe) => result,
                status = self.child.wait() => Err(process_exit_error(status)),
            }
        })
        .await
        .map_err(|_| WorkerSupervisorError::ReceiveTimeout)??;
        self.validator.validate_inbound(&envelope)?;
        Ok(envelope)
    }

    /// Observe an already-completed process without blocking.
    ///
    /// # Errors
    ///
    /// Returns an operating-system observation failure.
    pub fn try_wait(&mut self) -> Result<Option<WorkerExit>, WorkerSupervisorError> {
        self.child
            .try_wait()
            .map(|status| status.map(WorkerExit::from))
            .map_err(WorkerSupervisorError::PipeIo)
    }

    /// Wait for the process to exit without restarting it.
    ///
    /// # Errors
    ///
    /// Returns an operating-system wait failure.
    pub async fn wait_for_exit(&mut self) -> Result<WorkerExit, WorkerSupervisorError> {
        self.child
            .wait()
            .await
            .map(WorkerExit::from)
            .map_err(WorkerSupervisorError::PipeIo)
    }

    /// Request the typed orderly shutdown, require its acknowledgement, then
    /// require the process to exit within the same bounded deadline.
    ///
    /// # Errors
    ///
    /// Returns when the worker rejects/malforms the handshake, exits early, or
    /// misses the deadline. A deadline miss does not pretend shutdown worked;
    /// the caller can then invoke [`Self::force_kill`].
    pub async fn request_shutdown(
        &mut self,
        reason: ShutdownReason,
        timeout: Duration,
    ) -> Result<WorkerExit, WorkerSupervisorError> {
        let command_id = self
            .send_command(Command::WorkerShutdown(WorkerShutdown { reason }))
            .await?;
        let deadline = Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(WorkerSupervisorError::ShutdownTimeout);
            }
            let envelope = self.receive(remaining).await.map_err(|error| {
                if matches!(error, WorkerSupervisorError::ReceiveTimeout) {
                    WorkerSupervisorError::ShutdownTimeout
                } else {
                    error
                }
            })?;
            match envelope.message {
                Message::Ack(reply) if reply.reply_to == command_id => match reply.ack {
                    Ack::WorkerShutdown(ack) if ack.accepted => break,
                    _ => return Err(WorkerSupervisorError::ShutdownRejected),
                },
                Message::Error(reply) if reply.reply_to == command_id => {
                    return Err(WorkerSupervisorError::ShutdownRejected);
                }
                _ => {}
            }
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(WorkerSupervisorError::ShutdownTimeout);
        }
        timeout_at(deadline, self.child.wait())
            .await
            .map_err(|_| WorkerSupervisorError::ShutdownTimeout)?
            .map(WorkerExit::from)
            .map_err(WorkerSupervisorError::PipeIo)
    }

    /// Terminate the entire worker Job Object and wait for the main process.
    ///
    /// # Errors
    ///
    /// Returns only if Job Object termination or process wait fails.
    pub async fn force_kill(&mut self) -> Result<WorkerExit, WorkerSupervisorError> {
        if let Some(exit) = self.try_wait()? {
            return Ok(exit);
        }
        self.job.terminate()?;
        self.wait_for_exit().await
    }
}

async fn terminate_uncontained_child(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

fn create_secure_pipe(pipe_name: &str) -> Result<NamedPipeServer, WorkerSupervisorError> {
    let mut descriptor =
        CurrentUserSecurityDescriptor::new().map_err(WorkerSupervisorError::PipeSecurity)?;
    let mut attributes = descriptor.attributes();
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .max_instances(1)
        .pipe_mode(PipeMode::Byte)
        .in_buffer_size(PIPE_BUFFER_BYTES)
        .out_buffer_size(PIPE_BUFFER_BYTES);

    // SAFETY: `attributes` points to a valid SECURITY_ATTRIBUTES whose
    // self-relative descriptor remains alive until CreateNamedPipeW returns.
    unsafe {
        options
            .create_with_security_attributes_raw(pipe_name, (&raw mut attributes).cast::<c_void>())
    }
    .map_err(WorkerSupervisorError::PipeCreate)
}

async fn read_envelope<R>(pipe: &mut R) -> Result<Envelope, WorkerSupervisorError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    pipe.read_exact(&mut prefix)
        .await
        .map_err(WorkerSupervisorError::PipeIo)?;
    let length = u32::from_le_bytes(prefix);
    if !(1..=MAX_CONTROL_FRAME_BYTES).contains(&length) {
        return Err(latentdeck_control::FramingError::InvalidLength {
            actual: length,
            maximum: MAX_CONTROL_FRAME_BYTES,
        }
        .into());
    }
    let mut payload = vec![0_u8; length as usize];
    pipe.read_exact(&mut payload)
        .await
        .map_err(WorkerSupervisorError::PipeIo)?;
    decode_envelope(&payload).map_err(WorkerSupervisorError::from)
}

async fn write_envelope<W>(pipe: &mut W, envelope: &Envelope) -> Result<(), WorkerSupervisorError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let payload = encode_envelope(envelope)?;
    let length = u32::try_from(payload.len()).map_err(|_| {
        latentdeck_control::FramingError::InvalidLength {
            actual: u32::MAX,
            maximum: MAX_CONTROL_FRAME_BYTES,
        }
    })?;
    pipe.write_all(&length.to_le_bytes())
        .await
        .map_err(WorkerSupervisorError::PipeIo)?;
    pipe.write_all(&payload)
        .await
        .map_err(WorkerSupervisorError::PipeIo)?;
    pipe.flush().await.map_err(WorkerSupervisorError::PipeIo)
}

fn process_exit_error(result: io::Result<ExitStatus>) -> WorkerSupervisorError {
    match result {
        Ok(status) => WorkerSupervisorError::WorkerExited(status.into()),
        Err(error) => WorkerSupervisorError::PipeIo(error),
    }
}

fn elapsed_ns(started_at: Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn named_pipe_client_pid(pipe: &NamedPipeServer) -> Result<u32, WorkerSupervisorError> {
    let mut pid = 0_u32;
    // SAFETY: the borrowed raw handle belongs to the live connected pipe and
    // `pid` is a valid writable u32 for the duration of the call.
    let succeeded =
        unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle().cast::<c_void>(), &raw mut pid) };
    if succeeded == 0 || pid == 0 {
        return Err(WorkerSupervisorError::PipeIo(io::Error::last_os_error()));
    }
    Ok(pid)
}

struct JobObject {
    handle: OwnedHandle,
}

impl JobObject {
    fn new() -> Result<Self, WorkerSupervisorError> {
        // SAFETY: null security/name pointers request an unnamed, non-inherited
        // Job Object. A successful raw handle is immediately owned.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        if raw.is_null() {
            return Err(WorkerSupervisorError::Job(io::Error::last_os_error()));
        }
        // SAFETY: CreateJobObjectW returned a unique owned kernel handle.
        let handle = unsafe { OwnedHandle::from_raw_handle(raw as RawHandle) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = u32::try_from(size_of_val(&limits)).expect("Win32 structure size fits u32");
        // SAFETY: `limits` is the exact structure required by the selected
        // information class and remains valid for the synchronous call.
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle().cast::<c_void>(),
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast::<c_void>(),
                size,
            )
        };
        if configured == 0 {
            return Err(WorkerSupervisorError::Job(io::Error::last_os_error()));
        }
        Ok(Self { handle })
    }

    fn assign(&self, child: &Child) -> Result<(), WorkerSupervisorError> {
        let process = child.raw_handle().ok_or_else(|| {
            WorkerSupervisorError::Job(io::Error::other(
                "worker exited before Job Object assignment",
            ))
        })?;
        // SAFETY: both handles are live borrowed handles. Assignment does not
        // transfer ownership of either handle.
        let assigned = unsafe {
            AssignProcessToJobObject(
                self.handle.as_raw_handle().cast::<c_void>(),
                process.cast::<c_void>(),
            )
        };
        if assigned == 0 {
            return Err(WorkerSupervisorError::Job(io::Error::last_os_error()));
        }
        Ok(())
    }

    fn terminate(&self) -> Result<(), WorkerSupervisorError> {
        // SAFETY: the Job Object handle is live and remains owned by `self`.
        let terminated = unsafe {
            TerminateJobObject(
                self.handle.as_raw_handle().cast::<c_void>(),
                FORCED_TERMINATION_EXIT_CODE,
            )
        };
        if terminated == 0 {
            return Err(WorkerSupervisorError::Terminate(io::Error::last_os_error()));
        }
        Ok(())
    }

    #[cfg(test)]
    fn has_kill_on_close(&self) -> io::Result<bool> {
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: `limits` is the exact writable structure selected by the
        // information class and the Job Object handle is live.
        let queried = unsafe {
            QueryInformationJobObject(
                self.handle.as_raw_handle().cast::<c_void>(),
                JobObjectExtendedLimitInformation,
                (&raw mut limits).cast::<c_void>(),
                u32::try_from(size_of_val(&limits)).expect("Win32 structure size fits u32"),
                ptr::null_mut(),
            )
        };
        if queried == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(limits.BasicLimitInformation.LimitFlags & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0)
    }
}

struct CurrentUserSecurityDescriptor {
    allocation: LocalAllocation,
    #[cfg(test)]
    current_user_sid: String,
}

impl CurrentUserSecurityDescriptor {
    fn new() -> io::Result<Self> {
        let current_user_sid = current_user_sid_string()?;
        let sddl = format!("D:P(A;;GA;;;{current_user_sid})");
        let encoded: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `encoded` is a live NUL-terminated UTF-16 SDDL string;
        // `descriptor` is an out-pointer populated with LocalAlloc memory.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                encoded.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = LocalAllocation::new(descriptor)?;
        Ok(Self {
            allocation,
            #[cfg(test)]
            current_user_sid,
        })
    }

    fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("Win32 structure size fits u32"),
            lpSecurityDescriptor: self.allocation.as_ptr(),
            bInheritHandle: 0,
        }
    }

    #[cfg(test)]
    fn is_exact_current_user_dacl(&self) -> io::Result<bool> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl: *mut ACL = ptr::null_mut();
        // SAFETY: allocation is a valid self-relative security descriptor and
        // all out-pointers live through this synchronous call.
        let got_dacl = unsafe {
            GetSecurityDescriptorDacl(
                self.allocation.as_ptr(),
                &raw mut present,
                &raw mut acl,
                &raw mut defaulted,
            )
        };
        if got_dacl == 0 {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || defaulted != 0 || acl.is_null() {
            return Ok(false);
        }

        let mut info = ACL_SIZE_INFORMATION::default();
        // SAFETY: `acl` points inside the live descriptor and `info` is the
        // exact output structure for AclSizeInformation.
        let got_info = unsafe {
            GetAclInformation(
                acl,
                (&raw mut info).cast::<c_void>(),
                u32::try_from(size_of_val(&info)).expect("ACL info size fits u32"),
                AclSizeInformation,
            )
        };
        if got_info == 0 {
            return Err(io::Error::last_os_error());
        }
        if info.AceCount != 1 {
            return Ok(false);
        }

        let mut raw_ace: *mut c_void = ptr::null_mut();
        // SAFETY: the ACL reports one ACE, so index zero is valid.
        if unsafe { GetAce(acl, 0, &raw mut raw_ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
        // SAFETY: SDDL generated an ACCESS_ALLOWED_ACE; `SidStart` is the
        // documented start of its variable-length SID.
        let (mask, sid) = unsafe {
            (
                (*ace).Mask,
                (&raw const (*ace).SidStart).cast_mut().cast::<c_void>(),
            )
        };
        if mask != GENERIC_ALL {
            return Ok(false);
        }
        Ok(sid_to_string(sid)? == self.current_user_sid)
    }
}

struct LocalAllocation(NonNull<c_void>);

impl LocalAllocation {
    fn new(pointer: *mut c_void) -> io::Result<Self> {
        NonNull::new(pointer)
            .map(Self)
            .ok_or_else(|| io::Error::other("Win32 returned a null local allocation"))
    }

    fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }
}

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: this pointer came from a Win32 API documented to allocate
        // with LocalAlloc and is freed exactly once here.
        let _ = unsafe { LocalFree(self.0.as_ptr()) };
    }
}

fn current_user_sid_string() -> io::Result<String> {
    let mut raw_token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle valid for this call and
    // `raw_token` is a writable out-pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut raw_token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned a unique owned token handle.
    let token = unsafe { OwnedHandle::from_raw_handle(raw_token as RawHandle) };

    let mut byte_length = 0_u32;
    // SAFETY: the null/zero probe requests the required buffer size.
    let _ = unsafe {
        GetTokenInformation(
            token.as_raw_handle().cast::<c_void>(),
            TokenUser,
            ptr::null_mut(),
            0,
            &raw mut byte_length,
        )
    };
    if byte_length == 0 {
        return Err(io::Error::last_os_error());
    }

    let word_size = size_of::<usize>();
    let word_count = (byte_length as usize).div_ceil(word_size);
    let mut aligned = vec![0_usize; word_count];
    // SAFETY: `aligned` has at least `byte_length` writable bytes and suitable
    // alignment for TOKEN_USER; all pointers remain live during the call.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle().cast::<c_void>(),
            TokenUser,
            aligned.as_mut_ptr().cast::<c_void>(),
            byte_length,
            &raw mut byte_length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful TokenUser information begins with TOKEN_USER in the
    // aligned buffer and its SID pointer remains valid while `aligned` lives.
    let sid = unsafe { (*aligned.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    sid_to_string(sid)
}

fn sid_to_string(sid: *mut c_void) -> io::Result<String> {
    let mut encoded = ptr::null_mut();
    // SAFETY: caller supplies a SID owned by a live descriptor/token buffer;
    // `encoded` receives LocalAlloc-owned NUL-terminated UTF-16.
    if unsafe { ConvertSidToStringSidW(sid, &raw mut encoded) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let allocation = LocalAllocation::new(encoded.cast::<c_void>())?;
    let mut length = 0_usize;
    // A Windows SID string is far below this defensive 256-code-unit bound.
    while length < 256 {
        // SAFETY: `encoded` is a valid NUL-terminated string returned by Win32.
        if unsafe { *encoded.add(length) } == 0 {
            break;
        }
        length += 1;
    }
    if length == 256 {
        return Err(io::Error::other("current-user SID string is oversized"));
    }
    // SAFETY: the preceding bounded scan proved these UTF-16 code units lie
    // before the terminator in the live LocalAlloc buffer.
    let units = unsafe { std::slice::from_raw_parts(encoded, length) };
    let value = String::from_utf16(units)
        .map_err(|_| io::Error::other("current-user SID is not valid UTF-16"))?;
    drop(allocation);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsString, io::Read};

    use latentdeck_control::{
        AckReply, BoundedVec, EventMessage, ShutdownAck, WORKER_PROTOCOL_VERSION, WorkerHello,
    };
    use serde::Deserialize;
    use tokio::net::windows::named_pipe::ClientOptions;

    use super::*;
    use crate::worker_supervisor::{BOOTSTRAP_VERSION, MAX_BOOTSTRAP_BYTES};

    const GOOD_HELPER: &str = "worker_supervisor::platform::tests::worker_child_helper";
    const BAD_TOKEN_HELPER: &str = "worker_supervisor::platform::tests::worker_bad_token_helper";
    const ENVIRONMENT_HELPER: &str =
        "worker_supervisor::platform::tests::worker_minimal_environment_helper";
    const EARLY_EXIT_HELPER: &str =
        "worker_supervisor::platform::tests::worker_exit_before_connect_helper";
    const EXIT_AFTER_HELLO_HELPER: &str =
        "worker_supervisor::platform::tests::worker_exit_after_hello_helper";

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ChildBootstrap {
        bootstrap_version: u16,
        session_id: WireUuid,
        pipe_name: String,
        auth_token: AuthToken,
    }

    #[test]
    fn pipe_descriptor_has_one_current_user_full_access_ace() {
        let descriptor = CurrentUserSecurityDescriptor::new().expect("security descriptor");
        assert!(
            descriptor
                .is_exact_current_user_dacl()
                .expect("inspect descriptor")
        );
    }

    #[tokio::test]
    async fn pipe_and_job_use_the_bounded_release_settings() {
        let name = format!(r"\\.\pipe\latentdeck-worker-test-{}", WireUuid::new_v4());
        let pipe = create_secure_pipe(&name).expect("secure pipe");
        let info = pipe.info().expect("pipe info");
        assert_eq!(info.max_instances, 1);
        assert_eq!(info.in_buffer_size, PIPE_BUFFER_BYTES);
        assert_eq!(info.out_buffer_size, PIPE_BUFFER_BYTES);

        let job = JobObject::new().expect("job object");
        assert!(job.has_kill_on_close().expect("job limits"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authenticates_and_completes_an_explicit_shutdown() {
        let pending = spawn_worker(helper_launch(GOOD_HELPER))
            .await
            .expect("spawn worker");
        let session_id = pending.session_id();
        let worker_pid = pending.worker_pid();
        let session = pending.connect().await.expect("authenticated session");
        assert_eq!(session.session_id(), session_id);
        assert_eq!(session.worker_pid(), worker_pid);
        assert_eq!(
            session.remaining_inbound_message_budget(),
            latentdeck_control::MAX_MESSAGES_PER_SESSION - 1
        );
        assert_eq!(
            session.remaining_outbound_message_budget(),
            latentdeck_control::MAX_MESSAGES_PER_SESSION
        );

        let mut client = crate::worker_client::WorkerClient::new(session);
        assert_eq!(
            client.remaining_inbound_message_budget(),
            latentdeck_control::MAX_MESSAGES_PER_SESSION - 1
        );
        assert_eq!(
            client.remaining_outbound_message_budget(),
            latentdeck_control::MAX_MESSAGES_PER_SESSION
        );

        let exit = client
            .request_shutdown(ShutdownReason::ApplicationExit, Duration::from_secs(10))
            .await
            .expect("orderly shutdown");
        assert!(exit.success, "helper should exit successfully: {exit}");
        assert_eq!(
            client.remaining_inbound_message_budget(),
            latentdeck_control::MAX_MESSAGES_PER_SESSION - 2
        );
        assert_eq!(
            client.remaining_outbound_message_budget(),
            latentdeck_control::MAX_MESSAGES_PER_SESSION - 1
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_exit_wait_leaves_orderly_shutdown_usable() {
        let pending = spawn_worker(helper_launch(GOOD_HELPER))
            .await
            .expect("spawn worker");
        let session = pending.connect().await.expect("authenticated session");
        let mut client = crate::worker_client::WorkerClient::new(session);

        let timed_out =
            tokio::time::timeout(Duration::from_millis(50), client.wait_for_exit()).await;
        assert!(timed_out.is_err(), "live worker exit wait should time out");

        let exit = client
            .request_shutdown(ShutdownReason::ApplicationExit, Duration::from_secs(10))
            .await
            .expect("orderly shutdown after cancelled wait");
        assert!(exit.success, "helper should exit successfully: {exit}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authenticated_unexpected_exit_is_observed_repeatedly() {
        let pending = spawn_worker(helper_launch(EXIT_AFTER_HELLO_HELPER))
            .await
            .expect("spawn worker");
        let session = pending.connect().await.expect("authenticated session");
        let mut client = crate::worker_client::WorkerClient::new(session);

        let first = tokio::time::timeout(Duration::from_secs(10), client.wait_for_exit())
            .await
            .expect("unexpected exit observation should not hang")
            .expect("observe unexpected exit");
        let repeated = client
            .wait_for_exit()
            .await
            .expect("repeat cached exit observation");

        assert!(
            first.success,
            "test helper should exit successfully: {first}"
        );
        assert_eq!(repeated, first);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn child_receives_only_the_minimal_windows_runtime_environment() {
        assert!(std::env::var_os("PATH").is_some(), "parent test needs PATH");
        let pending = spawn_worker(helper_launch(ENVIRONMENT_HELPER))
            .await
            .expect("spawn worker");
        let mut session = pending.connect().await.expect("environment-checked hello");
        let exit = session
            .request_shutdown(ShutdownReason::ApplicationExit, Duration::from_secs(10))
            .await
            .expect("orderly shutdown");
        assert!(exit.success, "helper should exit successfully: {exit}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_a_wrong_token_and_does_not_resume() {
        let pending = spawn_worker(helper_launch(BAD_TOKEN_HELPER))
            .await
            .expect("spawn worker");
        let Err(error) = pending.connect().await else {
            panic!("bad token was accepted");
        };
        assert!(matches!(error, WorkerSupervisorError::AuthenticationFailed));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn observes_exit_before_connect_without_waiting_for_timeout() {
        let pending = spawn_worker(helper_launch(EARLY_EXIT_HELPER))
            .await
            .expect("spawn worker");
        let started = Instant::now();
        let Err(error) = pending.connect().await else {
            panic!("worker that exited early authenticated");
        };
        assert!(matches!(error, WorkerSupervisorError::WorkerExited(_)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn force_kill_terminates_the_worker_job() {
        let pending = spawn_worker(helper_launch(GOOD_HELPER))
            .await
            .expect("spawn worker");
        let mut session = pending.connect().await.expect("authenticated session");
        let exit = session.force_kill().await.expect("force kill");
        assert!(!exit.success);
        assert!(session.try_wait().expect("observe exit").is_some());
    }

    #[test]
    #[ignore = "spawned by the worker supervisor contract test"]
    fn worker_child_helper() {
        run_worker_child(false);
    }

    #[test]
    #[ignore = "spawned by the worker supervisor authentication test"]
    fn worker_bad_token_helper() {
        run_worker_child(true);
    }

    #[test]
    #[ignore = "spawned by the worker supervisor environment test"]
    fn worker_minimal_environment_helper() {
        assert_minimal_worker_environment();
        run_worker_child(false);
    }

    #[test]
    #[ignore = "spawned by the worker supervisor exit-observation test"]
    fn worker_exit_before_connect_helper() {
        let _ = read_child_bootstrap();
    }

    #[test]
    #[ignore = "spawned by the worker supervisor exit-observation test"]
    fn worker_exit_after_hello_helper() {
        run_worker_hello_then_exit();
    }

    fn helper_launch(test_name: &str) -> ValidatedWorkerLaunch {
        let executable = std::env::current_exe().expect("test executable");
        let working_directory = executable
            .parent()
            .expect("test executable directory")
            .to_path_buf();
        ValidatedWorkerLaunch {
            executable,
            arguments: ["--ignored", "--exact", test_name, "--nocapture"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            working_directory,
            connect_timeout: Duration::from_secs(10),
        }
    }

    fn run_worker_child(bad_token: bool) {
        let bootstrap = read_child_bootstrap();
        assert_eq!(bootstrap.bootstrap_version, BOOTSTRAP_VERSION);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("child runtime");
        runtime.block_on(async move {
            let mut client = ClientOptions::new()
                .open(&bootstrap.pipe_name)
                .expect("connect to supervisor pipe");
            let hello_token = if bad_token {
                AuthToken::new([0_u8; 32])
            } else {
                bootstrap.auth_token
            };
            let adapters = BoundedVec::try_from_vec(vec!["org.latentdeck.h3".to_owned()])
                .expect("adapter list");
            let hello = Envelope::new(
                bootstrap.session_id,
                1,
                WireUuid::new_v4(),
                1,
                Message::Event(EventMessage {
                    caused_by: None,
                    event: Event::WorkerHello(WorkerHello {
                        auth_token: hello_token,
                        worker_version: "test-worker-0.1.0".to_owned(),
                        protocol_min: WORKER_PROTOCOL_VERSION,
                        protocol_max: WORKER_PROTOCOL_VERSION,
                        pid: std::process::id(),
                        os: "windows".to_owned(),
                        arch: "x86_64".to_owned(),
                        python_version: "test-runtime".to_owned(),
                        available_adapters: adapters,
                    }),
                }),
            );
            write_envelope(&mut client, &hello)
                .await
                .expect("write hello");

            if bad_token {
                tokio::time::sleep(Duration::from_secs(30)).await;
                return;
            }

            let command = read_envelope(&mut client).await.expect("shutdown command");
            let Message::Command(Command::WorkerShutdown(_)) = command.message else {
                panic!("test worker expected worker.shutdown");
            };
            let ack = Envelope::new(
                bootstrap.session_id,
                2,
                WireUuid::new_v4(),
                2,
                Message::Ack(AckReply {
                    reply_to: command.message_id,
                    ack: Ack::WorkerShutdown(ShutdownAck { accepted: true }),
                }),
            );
            write_envelope(&mut client, &ack)
                .await
                .expect("write shutdown ack");
        });
    }

    fn run_worker_hello_then_exit() {
        let bootstrap = read_child_bootstrap();
        assert_eq!(bootstrap.bootstrap_version, BOOTSTRAP_VERSION);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("child runtime");
        runtime.block_on(async move {
            let mut client = ClientOptions::new()
                .open(&bootstrap.pipe_name)
                .expect("connect to supervisor pipe");
            let adapters = BoundedVec::try_from_vec(vec!["org.latentdeck.h3".to_owned()])
                .expect("adapter list");
            let hello = Envelope::new(
                bootstrap.session_id,
                1,
                WireUuid::new_v4(),
                1,
                Message::Event(EventMessage {
                    caused_by: None,
                    event: Event::WorkerHello(WorkerHello {
                        auth_token: bootstrap.auth_token,
                        worker_version: "test-worker-0.1.0".to_owned(),
                        protocol_min: WORKER_PROTOCOL_VERSION,
                        protocol_max: WORKER_PROTOCOL_VERSION,
                        pid: std::process::id(),
                        os: "windows".to_owned(),
                        arch: "x86_64".to_owned(),
                        python_version: "test-runtime".to_owned(),
                        available_adapters: adapters,
                    }),
                }),
            );
            write_envelope(&mut client, &hello)
                .await
                .expect("write hello");
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
    }

    fn assert_minimal_worker_environment() {
        for (key, value) in std::env::vars_os() {
            let key = key.to_string_lossy();
            assert!(
                WORKER_ENVIRONMENT_KEYS
                    .iter()
                    .any(|allowed| key.eq_ignore_ascii_case(allowed)),
                "unexpected inherited worker environment variable: {key}"
            );
            assert!(
                !value.is_empty(),
                "worker environment value is empty: {key}"
            );
        }
        for key in WORKER_ENVIRONMENT_KEYS {
            assert!(
                std::env::var_os(key).is_some(),
                "required worker environment variable is missing: {key}"
            );
        }
        assert!(std::env::var_os("PATH").is_none());
        assert!(std::env::var_os("PYTHONPATH").is_none());
    }

    fn read_child_bootstrap() -> ChildBootstrap {
        let stdin = std::io::stdin();
        let mut stdin = stdin.lock();
        let mut prefix = [0_u8; 4];
        stdin.read_exact(&mut prefix).expect("bootstrap prefix");
        let length = u32::from_le_bytes(prefix) as usize;
        assert!((1..=MAX_BOOTSTRAP_BYTES).contains(&length));
        let mut payload = vec![0_u8; length];
        stdin.read_exact(&mut payload).expect("bootstrap payload");
        rmp_serde::from_slice(&payload).expect("bootstrap MessagePack")
    }
}
