//! Lifecycle boundary for one isolated codec worker process.

use std::{ffi::OsString, fmt, io, path::PathBuf, process::ExitStatus, time::Duration};

use latentdeck_control::{AuthToken, FramingError, ValidationError, WireUuid};
use serde::Serialize;
use thiserror::Error;

use crate::codec_pack::ValidatedCodecPack;

const BOOTSTRAP_VERSION: u16 = 1;
const MAX_BOOTSTRAP_BYTES: usize = 4_096;

#[cfg(windows)]
#[path = "worker_supervisor/windows.rs"]
mod platform;
#[cfg(not(windows))]
#[path = "worker_supervisor/unsupported.rs"]
mod platform;

pub use platform::{PendingWorker, WorkerSession, spawn_worker};

/// A direct process launch derived from an integrity-checked codec pack.
///
/// The fields are intentionally private: runtime code cannot turn an
/// unvalidated path or a shell command into a worker launch descriptor.
#[derive(Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct ValidatedWorkerLaunch {
    executable: PathBuf,
    arguments: Vec<OsString>,
    working_directory: PathBuf,
    connect_timeout: Duration,
}

impl ValidatedWorkerLaunch {
    /// Derive the only accepted launch descriptor from a validated codec pack.
    #[must_use]
    pub fn from_codec_pack(pack: &ValidatedCodecPack) -> Self {
        Self::from_arguments(pack, &pack.manifest.worker.arguments)
    }

    /// Derive the D2 worker launch only when the validated pack declares the
    /// dedicated trusted entrypoint.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerSupervisorError::WorkerEntrypointMissing`] for a valid
    /// Player-only pack. Runtime code must surface that state and must not
    /// silently reuse the Player worker command.
    pub fn from_codec_pack_d2(pack: &ValidatedCodecPack) -> Result<Self, WorkerSupervisorError> {
        let arguments = pack
            .manifest
            .worker
            .d2_arguments
            .as_ref()
            .ok_or(WorkerSupervisorError::WorkerEntrypointMissing("d2"))?;
        Ok(Self::from_arguments(pack, arguments))
    }

    fn from_arguments(pack: &ValidatedCodecPack, arguments: &[String]) -> Self {
        Self {
            executable: pack.worker_executable.clone(),
            arguments: arguments.iter().map(OsString::from).collect(),
            working_directory: pack.worker_working_directory.clone(),
            connect_timeout: Duration::from_millis(u64::from(
                pack.manifest.worker.probe_timeout_ms,
            )),
        }
    }
}

/// Failures that terminate a worker session instead of triggering auto-resume.
#[derive(Debug, Error)]
pub enum WorkerSupervisorError {
    #[error("codec workers are supported only on Windows in LatentDeck 0.1")]
    UnsupportedPlatform,
    #[error("validated codec pack does not declare the {0} worker entrypoint")]
    WorkerEntrypointMissing(&'static str),
    #[error("secure random generation failed")]
    Random,
    #[error("worker bootstrap could not be encoded")]
    BootstrapEncode,
    #[error("worker bootstrap exceeded its bounded size")]
    BootstrapTooLarge,
    #[error("current-user pipe security setup failed")]
    PipeSecurity(#[source] io::Error),
    #[error("worker control pipe creation failed")]
    PipeCreate(#[source] io::Error),
    #[error("worker process could not be spawned")]
    Spawn(#[source] io::Error),
    #[error("minimal worker runtime environment could not be constructed")]
    WorkerEnvironment(#[source] io::Error),
    #[error("worker process could not be contained in its Job Object")]
    Job(#[source] io::Error),
    #[error("single-use worker bootstrap delivery failed")]
    BootstrapWrite(#[source] io::Error),
    #[error("worker did not connect before the validated pack timeout")]
    ConnectTimeout,
    #[error("worker did not complete its authenticated hello before timeout")]
    HandshakeTimeout,
    #[error("worker did not produce the next control message before timeout")]
    ReceiveTimeout,
    #[error("the connected pipe client is not the spawned worker process")]
    PeerProcessMismatch,
    #[error("the authenticated worker process handle is no longer available")]
    ProcessHandleUnavailable,
    #[error("worker authentication failed")]
    AuthenticationFailed,
    #[error("worker did not send worker.hello as its first message")]
    UnexpectedHandshake,
    #[error("worker control pipe I/O failed")]
    PipeIo(#[source] io::Error),
    #[error("worker protocol frame failed validation")]
    Framing(#[from] FramingError),
    #[error("worker session ordering or correlation failed validation")]
    Session(#[from] ValidationError),
    #[error("worker exited: {0}")]
    WorkerExited(WorkerExit),
    #[error("worker rejected or malformed the shutdown handshake")]
    ShutdownRejected,
    #[error("worker did not exit after acknowledging shutdown")]
    ShutdownTimeout,
    #[error("worker Job Object termination failed")]
    Terminate(#[source] io::Error),
}

/// Sanitized process result used by Player state transitions and diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerExit {
    pub success: bool,
    pub code: Option<i32>,
}

impl From<ExitStatus> for WorkerExit {
    fn from(status: ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
        }
    }
}

impl fmt::Display for WorkerExit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code {
            Some(code) => write!(formatter, "exit code {code}"),
            None => formatter.write_str("terminated without an exit code"),
        }
    }
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRecord<'a> {
    bootstrap_version: u16,
    session_id: WireUuid,
    pipe_name: &'a str,
    auth_token: &'a AuthToken,
}

fn encode_bootstrap(
    session_id: WireUuid,
    pipe_name: &str,
    auth_token: &AuthToken,
) -> Result<Vec<u8>, WorkerSupervisorError> {
    let payload = rmp_serde::to_vec_named(&BootstrapRecord {
        bootstrap_version: BOOTSTRAP_VERSION,
        session_id,
        pipe_name,
        auth_token,
    })
    .map_err(|_| WorkerSupervisorError::BootstrapEncode)?;
    if payload.is_empty() || payload.len() > MAX_BOOTSTRAP_BYTES {
        return Err(WorkerSupervisorError::BootstrapTooLarge);
    }
    let length =
        u32::try_from(payload.len()).map_err(|_| WorkerSupervisorError::BootstrapTooLarge)?;
    let mut framed = Vec::with_capacity(payload.len() + 4);
    framed.extend_from_slice(&length.to_le_bytes());
    framed.extend_from_slice(&payload);
    Ok(framed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DecodedBootstrap {
        bootstrap_version: u16,
        session_id: WireUuid,
        pipe_name: String,
        auth_token: AuthToken,
    }

    #[test]
    fn bootstrap_matches_the_bounded_python_stdin_contract() {
        let session_id = WireUuid::new_v4();
        let pipe_name = format!(r"\\.\pipe\latentdeck-worker-{session_id}");
        let token = AuthToken::new([0x5a; 32]);

        let encoded = encode_bootstrap(session_id, &pipe_name, &token).expect("bootstrap");
        assert!(encoded.len() <= MAX_BOOTSTRAP_BYTES + 4);

        let payload_len = u32::from_le_bytes(encoded[..4].try_into().expect("prefix")) as usize;
        assert_eq!(payload_len, encoded.len() - 4);
        let decoded: DecodedBootstrap =
            rmp_serde::from_slice(&encoded[4..]).expect("named MessagePack map");
        assert_eq!(decoded.bootstrap_version, BOOTSTRAP_VERSION);
        assert_eq!(decoded.session_id, session_id);
        assert_eq!(decoded.pipe_name, pipe_name);
        assert!(decoded.auth_token.constant_time_eq(&token));
    }
}
