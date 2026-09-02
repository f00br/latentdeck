//! Sequential typed client above the authenticated Protocol 2 supervisor.

use std::time::Duration;

#[cfg(windows)]
use std::os::windows::io::BorrowedHandle;

use latentdeck_control::{
    WireUuid,
    v2::{
        Ack, Command, CommandName, ErrorCode, ErrorPayload, Event, Message, MetricsSnapshot,
        StatusSnapshot,
    },
};
use thiserror::Error;
use tokio::time::Instant;

use crate::worker_supervisor::{WorkerExit, WorkerSessionV2, WorkerSupervisorError};

/// Stable path-free Protocol 2 failure returned by the isolated worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteWorkerErrorV2 {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub fatal: bool,
    pub status: StatusSnapshot,
    pub diagnostic_id: WireUuid,
}

impl From<ErrorPayload> for RemoteWorkerErrorV2 {
    fn from(error: ErrorPayload) -> Self {
        Self {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
            fatal: error.fatal,
            status: error.status,
            diagnostic_id: WireUuid::from_uuid(error.diagnostic_id),
        }
    }
}

/// Failures observed while one sequential Protocol 2 command is pending.
#[derive(Debug, Error)]
pub enum WorkerClientV2Error {
    #[error(transparent)]
    Supervisor(#[from] WorkerSupervisorError),
    #[error("Protocol 2 worker rejected the command: {0:?}")]
    Remote(RemoteWorkerErrorV2),
    #[error("Protocol 2 worker command {0:?} exceeded its deadline")]
    CommandTimeout(CommandName),
    #[error("Protocol 2 worker heartbeat stopped while command {0:?} was pending")]
    HeartbeatTimeout(CommandName),
    #[error("Protocol 2 worker returned a reply for a different sequential command")]
    UnexpectedReply,
    #[error("Protocol 2 acknowledgement mismatch: expected {expected:?}, received {actual:?}")]
    UnexpectedAck {
        expected: CommandName,
        actual: CommandName,
    },
}

/// One-command-at-a-time Protocol 2 client that consumes interleaved status,
/// heartbeat, and fault events.
pub struct WorkerClientV2 {
    session: WorkerSessionV2,
    heartbeat_hard_timeout: Option<Duration>,
    last_heartbeat: Instant,
    last_status: Option<StatusSnapshot>,
    last_metrics: Option<MetricsSnapshot>,
}

impl WorkerClientV2 {
    #[must_use]
    pub fn new(session: WorkerSessionV2) -> Self {
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

    #[must_use]
    pub fn remaining_inbound_message_budget(&self) -> usize {
        self.session.remaining_inbound_message_budget()
    }

    #[must_use]
    pub fn remaining_outbound_message_budget(&self) -> usize {
        self.session.remaining_outbound_message_budget()
    }

    /// Borrow the authenticated worker process handle for one synchronous
    /// duplication operation.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker process has already exited.
    #[cfg(windows)]
    pub fn with_process_handle<T>(
        &self,
        operation: impl FnOnce(BorrowedHandle<'_>) -> T,
    ) -> Result<T, WorkerClientV2Error> {
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

    /// Send one Protocol 2 command and wait for its correlated typed reply.
    ///
    /// # Errors
    ///
    /// Returns a supervisor, remote, deadline, heartbeat, or correlation
    /// failure. The client never changes protocol or retries through P1.
    pub async fn call(
        &mut self,
        command: Command,
        command_timeout: Duration,
    ) -> Result<Ack, WorkerClientV2Error> {
        let command_name = command.name();
        let configured_heartbeat = match &command {
            Command::SessionConfigure(configure) => Some(Duration::from_millis(u64::from(
                configure.heartbeat_hard_timeout_ms,
            ))),
            _ => None,
        };
        let command_id = self.session.send_command(command).await?;
        let command_started_at = Instant::now();
        let deadline = Instant::now() + command_timeout;

        loop {
            let now = Instant::now();
            let remaining = deadline.saturating_duration_since(now);
            if remaining.is_zero() {
                return Err(WorkerClientV2Error::CommandTimeout(command_name));
            }
            let heartbeat_remaining = heartbeat_remaining(
                self.last_heartbeat,
                command_started_at,
                self.heartbeat_hard_timeout,
                now,
            );
            if heartbeat_remaining == Some(Duration::ZERO) {
                return Err(WorkerClientV2Error::HeartbeatTimeout(command_name));
            }
            let receive_budget =
                heartbeat_remaining.map_or(remaining, |heartbeat| remaining.min(heartbeat));
            let envelope = match self.session.receive(receive_budget).await {
                Ok(envelope) => envelope,
                Err(WorkerSupervisorError::ReceiveTimeout)
                    if heartbeat_remaining.is_some_and(|heartbeat| heartbeat <= remaining) =>
                {
                    return Err(WorkerClientV2Error::HeartbeatTimeout(command_name));
                }
                Err(WorkerSupervisorError::ReceiveTimeout) => {
                    return Err(WorkerClientV2Error::CommandTimeout(command_name));
                }
                Err(error) => return Err(error.into()),
            };

            match envelope.message {
                Message::Ack(reply) if reply.reply_to == command_id.as_uuid() => {
                    let actual = reply.ack.name();
                    if actual != command_name {
                        return Err(WorkerClientV2Error::UnexpectedAck {
                            expected: command_name,
                            actual,
                        });
                    }
                    self.last_status = Some(reply.status);
                    if let Ack::MetricsGet(metrics) = &reply.ack {
                        self.last_metrics = Some(metrics.clone());
                    }
                    if matches!(reply.ack, Ack::SessionConfigure(_)) {
                        self.heartbeat_hard_timeout = configured_heartbeat;
                        self.last_heartbeat = Instant::now();
                    }
                    return Ok(reply.ack);
                }
                Message::Error(reply) if reply.reply_to == command_id.as_uuid() => {
                    return Err(WorkerClientV2Error::Remote(reply.error.into()));
                }
                Message::Event(event) => match event.event {
                    Event::WorkerHeartbeat(status) => {
                        self.last_heartbeat = Instant::now();
                        self.last_status = Some(status);
                    }
                    Event::StatusChanged(status) => {
                        self.last_status = Some(status);
                    }
                    Event::WorkerFault(error) => {
                        return Err(WorkerClientV2Error::Remote(error.into()));
                    }
                    Event::WorkerHello(_) => return Err(WorkerClientV2Error::UnexpectedReply),
                },
                Message::Ack(_) | Message::Error(_) | Message::Command(_) => {
                    return Err(WorkerClientV2Error::UnexpectedReply);
                }
            }
        }
    }

    /// Force termination of the contained Protocol 2 worker job.
    ///
    /// # Errors
    ///
    /// Returns a supervisor error if termination or process observation fails.
    pub async fn force_kill(&mut self) -> Result<WorkerExit, WorkerClientV2Error> {
        self.session.force_kill().await.map_err(Into::into)
    }

    /// Wait for the authenticated Protocol 2 process to exit.
    ///
    /// # Errors
    ///
    /// Returns a supervisor error if process observation fails.
    pub async fn wait_for_exit(&mut self) -> Result<WorkerExit, WorkerClientV2Error> {
        self.session.wait_for_exit().await.map_err(Into::into)
    }

    /// Request exact Protocol 2 orderly shutdown and require process exit.
    ///
    /// # Errors
    ///
    /// Returns a supervisor error for a rejected acknowledgement, timeout,
    /// pipe failure, or failed process observation.
    pub async fn request_shutdown(
        &mut self,
        reason: latentdeck_control::v2::ShutdownReason,
        timeout: Duration,
    ) -> Result<WorkerExit, WorkerClientV2Error> {
        self.session
            .request_shutdown(reason, timeout)
            .await
            .map_err(Into::into)
    }
}

fn heartbeat_remaining(
    last_heartbeat: Instant,
    command_started_at: Instant,
    hard_timeout: Option<Duration>,
    now: Instant,
) -> Option<Duration> {
    hard_timeout.map(|timeout| {
        let pending_baseline = last_heartbeat.max(command_started_at);
        (pending_baseline + timeout).saturating_duration_since(now)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_time_before_a_command_does_not_exhaust_the_p2_heartbeat_window() {
        let command_started_at = Instant::now();
        let stale_heartbeat = command_started_at
            .checked_sub(Duration::from_secs(30))
            .expect("test instant supports subtraction");
        let hard_timeout = Duration::from_secs(5);

        assert_eq!(
            heartbeat_remaining(
                stale_heartbeat,
                command_started_at,
                Some(hard_timeout),
                command_started_at,
            ),
            Some(hard_timeout)
        );
    }
}
