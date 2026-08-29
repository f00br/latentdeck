//! Typed LD-D2 calls above the generic correlated worker client.

use std::time::Duration;

use latentdeck_control::{
    Ack, Command, CommandName, D2CaptureStart, D2CaptureStatus, D2CaptureStatusRequest,
    D2CaptureStop, D2ControlsSet, D2ControlsSetAck, D2Load, D2ProcessSlot, D2ProcessSlotAck,
    D2Reset, D2ResetAck, D2Restart, D2RestartAck, D2SeedSet, D2SeedSetAck, D2Status,
    D2TransportSet, D2TransportSetAck, EmptyPayload,
};

use crate::worker_client::{WorkerClient, WorkerClientError};

impl WorkerClient {
    /// Load and compatibility-check both LD-D2 sources and the explicitly
    /// selected trusted operator.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch. Calls remain one-at-a-time through [`WorkerClient::call`].
    pub async fn deck_d2_load(
        &mut self,
        request: D2Load,
        timeout: Duration,
    ) -> Result<D2Status, WorkerClientError> {
        self.call_d2(
            Command::DeckD2Load(request),
            timeout,
            CommandName::DeckD2Load,
            |ack| match ack {
                Ack::DeckD2Load(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Process and decode exactly one post-operator LD-D2 slot.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_process_slot(
        &mut self,
        request: D2ProcessSlot,
        timeout: Duration,
    ) -> Result<D2ProcessSlotAck, WorkerClientError> {
        self.call_d2(
            Command::DeckD2ProcessSlot(request),
            timeout,
            CommandName::DeckD2ProcessSlot,
            |ack| match ack {
                Ack::DeckD2ProcessSlot(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Apply a previously reported causal reset barrier with a strictly newer
    /// stream generation.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_reset(
        &mut self,
        request: D2Reset,
        timeout: Duration,
    ) -> Result<D2ResetAck, WorkerClientError> {
        self.call_d2(
            Command::DeckD2Reset(request),
            timeout,
            CommandName::DeckD2Reset,
            |ack| match ack {
                Ack::DeckD2Reset(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Request an explicit restart barrier without resetting state implicitly.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_restart(
        &mut self,
        request: D2Restart,
        timeout: Duration,
    ) -> Result<D2RestartAck, WorkerClientError> {
        self.call_d2(
            Command::DeckD2Restart(request),
            timeout,
            CommandName::DeckD2Restart,
            |ack| match ack {
                Ack::DeckD2Restart(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace the complete closed LD-D2 control block atomically.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_controls_set(
        &mut self,
        request: D2ControlsSet,
        timeout: Duration,
    ) -> Result<D2ControlsSetAck, WorkerClientError> {
        self.call_d2(
            Command::DeckD2ControlsSet(request),
            timeout,
            CommandName::DeckD2ControlsSet,
            |ack| match ack {
                Ack::DeckD2ControlsSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace both independent play/pause and loop flags atomically.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_transport_set(
        &mut self,
        request: D2TransportSet,
        timeout: Duration,
    ) -> Result<D2TransportSetAck, WorkerClientError> {
        self.call_d2(
            Command::DeckD2TransportSet(request),
            timeout,
            CommandName::DeckD2TransportSet,
            |ack| match ack {
                Ack::DeckD2TransportSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace the deterministic LD-D2 seed within the exact u53 range.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_seed_set(
        &mut self,
        request: D2SeedSet,
        timeout: Duration,
    ) -> Result<D2SeedSetAck, WorkerClientError> {
        self.call_d2(
            Command::DeckD2SeedSet(request),
            timeout,
            CommandName::DeckD2SeedSet,
            |ack| match ack {
                Ack::DeckD2SeedSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Read the current worker-owned LD-D2 state without advancing its clock.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_status(
        &mut self,
        timeout: Duration,
    ) -> Result<D2Status, WorkerClientError> {
        self.call_d2(
            Command::DeckD2Status(EmptyPayload {}),
            timeout,
            CommandName::DeckD2Status,
            |ack| match ack {
                Ack::DeckD2Status(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Arm Snapshot or Live Capture at the worker's next causal reset boundary.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_capture_start(
        &mut self,
        request: D2CaptureStart,
        timeout: Duration,
    ) -> Result<D2CaptureStatus, WorkerClientError> {
        self.call_d2(
            Command::DeckD2CaptureStart(request),
            timeout,
            CommandName::DeckD2CaptureStart,
            |ack| match ack {
                Ack::DeckD2CaptureStart(payload) => Some(*payload),
                _ => None,
            },
        )
        .await
    }

    /// Stop Live Capture at its next codec-valid boundary.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_capture_stop(
        &mut self,
        request: D2CaptureStop,
        timeout: Duration,
    ) -> Result<D2CaptureStatus, WorkerClientError> {
        self.call_d2(
            Command::DeckD2CaptureStop(request),
            timeout,
            CommandName::DeckD2CaptureStop,
            |ack| match ack {
                Ack::DeckD2CaptureStop(payload) => Some(*payload),
                _ => None,
            },
        )
        .await
    }

    /// Read one capture's bounded worker-owned state without advancing it.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_d2_capture_status(
        &mut self,
        request: D2CaptureStatusRequest,
        timeout: Duration,
    ) -> Result<D2CaptureStatus, WorkerClientError> {
        self.call_d2(
            Command::DeckD2CaptureStatus(request),
            timeout,
            CommandName::DeckD2CaptureStatus,
            |ack| match ack {
                Ack::DeckD2CaptureStatus(payload) => Some(*payload),
                _ => None,
            },
        )
        .await
    }

    async fn call_d2<T>(
        &mut self,
        command: Command,
        timeout: Duration,
        expected: CommandName,
        extract: impl FnOnce(Ack) -> Option<T>,
    ) -> Result<T, WorkerClientError> {
        let ack = self.call(command, timeout).await?;
        let actual = ack.name();
        extract(ack).ok_or(WorkerClientError::UnexpectedAck { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use latentdeck_control::{D2SeedSetAck, WireUuid};

    use super::*;

    #[test]
    fn command_names_remain_distinct_for_same_shape_status_replies() {
        let load = Ack::DeckD2Load(status());
        let status = Ack::DeckD2Status(status());

        assert_eq!(load.name(), CommandName::DeckD2Load);
        assert_eq!(status.name(), CommandName::DeckD2Status);
    }

    #[test]
    fn capture_status_ack_names_remain_distinct() {
        let status = capture_status();
        let acks = [
            Ack::DeckD2CaptureStart(Box::new(status.clone())),
            Ack::DeckD2CaptureStop(Box::new(status.clone())),
            Ack::DeckD2CaptureStatus(Box::new(status)),
        ];
        let expected = [
            CommandName::DeckD2CaptureStart,
            CommandName::DeckD2CaptureStop,
            CommandName::DeckD2CaptureStatus,
        ];

        for (ack, expected) in acks.into_iter().zip(expected) {
            assert_eq!(ack.name(), expected);
        }
    }

    #[test]
    fn realtime_seed_ack_is_a_closed_payload() {
        let ack = Ack::DeckD2SeedSet(D2SeedSetAck {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            seed: 42,
            requires_causal_reset: false,
        });

        let Ack::DeckD2SeedSet(payload) = ack else {
            panic!("closed D2 seed acknowledgement");
        };
        assert_eq!(payload.seed, 42);
        assert!(!payload.requires_causal_reset);
    }

    fn status() -> D2Status {
        let source = latentdeck_control::D2SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: "a".repeat(64),
            latent_slot_count: 7,
        };
        D2Status {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            operator_id: "org.latentdeck.builtin.ld_d2".to_owned(),
            operator_version: "0.1.0".to_owned(),
            stream_generation: 1,
            stream_sequence: 0,
            playhead_a: 0,
            playhead_b: 0,
            transport: latentdeck_control::D2Transport::default(),
            controls: latentdeck_control::D2Controls::default(),
            seed: 42,
            pending_reset: false,
            pending_reset_reasons: latentdeck_control::BoundedVec::default(),
            decoded_start_frame: 0,
            source_a: source.clone(),
            source_b: source,
        }
    }

    fn capture_status() -> D2CaptureStatus {
        D2CaptureStatus {
            capture_id: WireUuid::new_v4(),
            mode: latentdeck_control::D2CaptureMode::LiveCapture,
            state: latentdeck_control::D2CaptureState::AwaitingReset,
            structural_carrier: latentdeck_control::D2Routing::A,
            latent_slots: 0,
            current_generation: Some(1),
            minimum_new_generation: Some(2),
            target_latent_slots: Some(0),
            stream_generation: None,
            finalize_after_latent_slots: None,
            reason: None,
            receipt: None,
        }
    }
}
