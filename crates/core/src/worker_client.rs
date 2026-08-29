//! Sequential typed client above the isolated worker supervisor.

use std::time::Duration;

#[cfg(windows)]
use std::os::windows::io::BorrowedHandle;

use latentdeck_control::{
    Ack, Command, CommandName, ErrorCode, ErrorPayload, Event, Message, MetricsSnapshot,
    StatusSnapshot, WireUuid, WorkerState,
};
use thiserror::Error;
use tokio::time::Instant;

use crate::worker_supervisor::{WorkerExit, WorkerSession, WorkerSupervisorError};

/// Stable remote worker failure without stack traces or machine-local paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWorkerError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub fatal: bool,
    pub worker_state: WorkerState,
    pub diagnostic_id: WireUuid,
}

impl From<ErrorPayload> for RemoteWorkerError {
    fn from(error: ErrorPayload) -> Self {
        Self {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
            fatal: error.fatal,
            worker_state: error.worker_state,
            diagnostic_id: error.diagnostic_id,
        }
    }
}

/// Failures observed by Core while executing one sequential command.
#[derive(Debug, Error)]
pub enum WorkerClientError {
    #[error(transparent)]
    Supervisor(#[from] WorkerSupervisorError),
    #[error("worker rejected the command: {0:?}")]
    Remote(RemoteWorkerError),
    #[error("worker command {0:?} exceeded its deadline")]
    CommandTimeout(CommandName),
    #[error("worker heartbeat stopped while command {0:?} was pending")]
    HeartbeatTimeout(CommandName),
    #[error("worker returned a reply for a different sequential command")]
    UnexpectedReply,
}

/// One-command-at-a-time client that consumes interleaved worker events.
pub struct WorkerClient {
    session: WorkerSession,
    heartbeat_hard_timeout: Option<Duration>,
    last_heartbeat: Instant,
    last_status: Option<StatusSnapshot>,
    last_metrics: Option<MetricsSnapshot>,
}

impl WorkerClient {
    #[must_use]
    pub fn new(session: WorkerSession) -> Self {
        Self {
            session,
            heartbeat_hard_timeout: None,
            last_heartbeat: Instant::now(),
            last_status: None,
            last_metrics: None,
        }
    }

    #[must_use]
    pub const fn worker_pid(&self) -> u32 {
        self.session.worker_pid()
    }

    #[must_use]
    pub const fn session_id(&self) -> WireUuid {
        self.session.session_id()
    }

    /// Borrow the authenticated worker process handle for one synchronous
    /// handle-duplication operation.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker process has already exited.
    #[cfg(windows)]
    pub fn with_process_handle<T>(
        &self,
        operation: impl FnOnce(BorrowedHandle<'_>) -> T,
    ) -> Result<T, WorkerClientError> {
        self.session
            .with_process_handle(operation)
            .map_err(Into::into)
    }

    #[must_use]
    pub const fn last_status(&self) -> Option<&StatusSnapshot> {
        self.last_status.as_ref()
    }

    #[must_use]
    pub const fn last_metrics(&self) -> Option<&MetricsSnapshot> {
        self.last_metrics.as_ref()
    }

    /// Send one typed command and wait for its correlated acknowledgement.
    /// Heartbeats and state/metrics events are consumed while the command is
    /// pending; a worker fault terminates the call.
    ///
    /// # Errors
    ///
    /// Returns a supervisor, remote, command-deadline, heartbeat, or
    /// correlation failure. Calls are deliberately sequential.
    pub async fn call(
        &mut self,
        command: Command,
        command_timeout: Duration,
    ) -> Result<Ack, WorkerClientError> {
        let command_name = command.name();
        let command_id = self.session.send_command(command).await?;
        let deadline = Instant::now() + command_timeout;

        loop {
            let now = Instant::now();
            let remaining = deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                return Err(WorkerClientError::CommandTimeout(command_name));
            }
            let heartbeat_remaining = self
                .heartbeat_hard_timeout
                .map(|heartbeat| (self.last_heartbeat + heartbeat).saturating_duration_since(now));
            if heartbeat_remaining == Some(Duration::ZERO) {
                return Err(WorkerClientError::HeartbeatTimeout(command_name));
            }
            let receive_budget =
                heartbeat_remaining.map_or(remaining, |heartbeat| remaining.min(heartbeat));
            let envelope = match self.session.receive(receive_budget).await {
                Ok(envelope) => envelope,
                Err(WorkerSupervisorError::ReceiveTimeout)
                    if heartbeat_remaining.is_some_and(|heartbeat| heartbeat <= remaining) =>
                {
                    return Err(WorkerClientError::HeartbeatTimeout(command_name));
                }
                Err(WorkerSupervisorError::ReceiveTimeout) => {
                    return Err(WorkerClientError::CommandTimeout(command_name));
                }
                Err(error) => return Err(error.into()),
            };

            match envelope.message {
                Message::Ack(reply) if reply.reply_to == command_id => {
                    if let Ack::SessionConfigure(configured) = &reply.ack {
                        self.heartbeat_hard_timeout = Some(Duration::from_millis(u64::from(
                            configured.heartbeat_hard_timeout_ms,
                        )));
                        self.last_heartbeat = Instant::now();
                    }
                    return Ok(reply.ack);
                }
                Message::Error(reply) if reply.reply_to == command_id => {
                    return Err(WorkerClientError::Remote(reply.error.into()));
                }
                Message::Event(event) => match event.event {
                    Event::WorkerHeartbeat(_) => {
                        self.last_heartbeat = Instant::now();
                    }
                    Event::WorkerStateChanged(changed) => {
                        self.last_status = Some(changed.status);
                    }
                    Event::MetricsSnapshot(metrics) => {
                        self.last_metrics = Some(metrics);
                    }
                    Event::WorkerFault(error) => {
                        return Err(WorkerClientError::Remote(error.into()));
                    }
                    Event::WorkerHello(_) => return Err(WorkerClientError::UnexpectedReply),
                },
                Message::Ack(_) | Message::Error(_) | Message::Command(_) => {
                    return Err(WorkerClientError::UnexpectedReply);
                }
            }
        }
    }

    /// Force termination of the contained worker job.
    ///
    /// # Errors
    ///
    /// Returns a supervisor error if termination or process observation fails.
    pub async fn force_kill(&mut self) -> Result<WorkerExit, WorkerClientError> {
        self.session.force_kill().await.map_err(Into::into)
    }

    /// Request a typed orderly shutdown and require the worker process to
    /// actually exit before the deadline.
    ///
    /// # Errors
    ///
    /// Returns a supervisor error for a rejected/malformed acknowledgement,
    /// timeout, pipe failure, or failed process observation. Callers may then
    /// use [`Self::force_kill`] as the explicit recovery path.
    pub async fn request_shutdown(
        &mut self,
        reason: latentdeck_control::ShutdownReason,
        timeout: Duration,
    ) -> Result<WorkerExit, WorkerClientError> {
        self.session
            .request_shutdown(reason, timeout)
            .await
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use latentdeck_control::BoundedVec;

    use super::*;

    #[test]
    fn remote_error_retains_only_stable_path_free_fields() {
        let diagnostic_id = WireUuid::new_v4();
        let remote = RemoteWorkerError::from(ErrorPayload {
            code: ErrorCode::CodecCudaUnavailable,
            message: "CUDA device is unavailable".to_owned(),
            retryable: false,
            fatal: false,
            worker_state: WorkerState::Ready,
            diagnostic_id,
            details: BoundedVec::default(),
        });

        assert_eq!(remote.code, ErrorCode::CodecCudaUnavailable);
        assert_eq!(remote.diagnostic_id, diagnostic_id);
        assert!(!remote.fatal);
    }
}
