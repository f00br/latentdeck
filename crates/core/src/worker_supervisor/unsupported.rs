//! Non-Windows compile surface for the Windows-first 0.1 runtime.

#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "0.1 non-Windows stubs are unconstructable and always return UnsupportedPlatform"
)]

use std::time::Duration;

use latentdeck_control::{Command, Envelope, ShutdownReason, WireUuid, v2 as protocol2};

use super::{ValidatedWorkerLaunch, WorkerExit, WorkerSupervisorError};

pub struct PendingWorker;

pub struct WorkerSession;

pub struct PendingWorkerV2;

pub struct WorkerSessionV2;

pub async fn spawn_worker(
    _launch: ValidatedWorkerLaunch,
) -> Result<PendingWorker, WorkerSupervisorError> {
    Err(WorkerSupervisorError::UnsupportedPlatform)
}

pub async fn spawn_worker_v2(
    _launch: ValidatedWorkerLaunch,
) -> Result<PendingWorkerV2, WorkerSupervisorError> {
    Err(WorkerSupervisorError::UnsupportedPlatform)
}

impl PendingWorker {
    pub async fn connect(self) -> Result<WorkerSession, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    #[must_use]
    pub fn session_id(&self) -> WireUuid {
        unreachable!("unsupported platform")
    }

    #[must_use]
    pub fn worker_pid(&self) -> u32 {
        unreachable!("unsupported platform")
    }
}

impl WorkerSession {
    #[must_use]
    pub fn session_id(&self) -> WireUuid {
        unreachable!("unsupported platform")
    }

    #[must_use]
    pub fn worker_pid(&self) -> u32 {
        unreachable!("unsupported platform")
    }

    /// Number of additional worker replies/events this session can validate.
    #[must_use]
    pub fn remaining_inbound_message_budget(&self) -> usize {
        unreachable!("unsupported platform")
    }

    /// Number of additional Core commands this session can register.
    #[must_use]
    pub fn remaining_outbound_message_budget(&self) -> usize {
        unreachable!("unsupported platform")
    }

    pub async fn send_command(
        &mut self,
        _command: Command,
    ) -> Result<WireUuid, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn receive(&mut self, _timeout: Duration) -> Result<Envelope, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub fn try_wait(&mut self) -> Result<Option<WorkerExit>, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn wait_for_exit(&mut self) -> Result<WorkerExit, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn request_shutdown(
        &mut self,
        _reason: ShutdownReason,
        _timeout: Duration,
    ) -> Result<WorkerExit, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn force_kill(&mut self) -> Result<WorkerExit, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }
}

impl PendingWorkerV2 {
    pub async fn connect(self) -> Result<WorkerSessionV2, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    #[must_use]
    pub fn session_id(&self) -> WireUuid {
        unreachable!("unsupported platform")
    }

    #[must_use]
    pub fn worker_pid(&self) -> u32 {
        unreachable!("unsupported platform")
    }
}

impl WorkerSessionV2 {
    #[must_use]
    pub fn session_id(&self) -> WireUuid {
        unreachable!("unsupported platform")
    }

    #[must_use]
    pub fn worker_pid(&self) -> u32 {
        unreachable!("unsupported platform")
    }

    #[must_use]
    pub fn remaining_inbound_message_budget(&self) -> usize {
        unreachable!("unsupported platform")
    }

    #[must_use]
    pub fn remaining_outbound_message_budget(&self) -> usize {
        unreachable!("unsupported platform")
    }

    pub async fn send_command(
        &mut self,
        _command: protocol2::Command,
    ) -> Result<WireUuid, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn receive(
        &mut self,
        _timeout: Duration,
    ) -> Result<protocol2::Envelope, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub fn try_wait(&mut self) -> Result<Option<WorkerExit>, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn wait_for_exit(&mut self) -> Result<WorkerExit, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn request_shutdown(
        &mut self,
        _reason: protocol2::ShutdownReason,
        _timeout: Duration,
    ) -> Result<WorkerExit, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }

    pub async fn force_kill(&mut self) -> Result<WorkerExit, WorkerSupervisorError> {
        Err(WorkerSupervisorError::UnsupportedPlatform)
    }
}
