//! Typed LD-Q4 calls above the generic correlated worker client.

use std::time::Duration;

use latentdeck_control::{
    Ack, Command, CommandName, EmptyPayload, Q4CaptureStart, Q4CaptureStatus,
    Q4CaptureStatusRequest, Q4CaptureStop, Q4ControlsSet, Q4ControlsSetAck, Q4Load, Q4ProcessSlot,
    Q4ProcessSlotAck, Q4Reset, Q4ResetAck, Q4Restart, Q4RestartAck, Q4RolesSet, Q4RolesSetAck,
    Q4SeedSet, Q4SeedSetAck, Q4Status, Q4TransportSet, Q4TransportSetAck,
};

use crate::worker_client::{WorkerClient, WorkerClientError};

impl WorkerClient {
    /// Load four validated LD-Q4 sources, their explicit role permutation, and
    /// the selected trusted operator.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch. Calls remain one-at-a-time through [`WorkerClient::call`].
    pub async fn deck_q4_load(
        &mut self,
        request: Q4Load,
        timeout: Duration,
    ) -> Result<Q4Status, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4Load(request),
            timeout,
            CommandName::DeckQ4Load,
            |ack| match ack {
                Ack::DeckQ4Load(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Process and decode exactly one post-operator LD-Q4 slot.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_process_slot(
        &mut self,
        request: Q4ProcessSlot,
        timeout: Duration,
    ) -> Result<Q4ProcessSlotAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4ProcessSlot(request),
            timeout,
            CommandName::DeckQ4ProcessSlot,
            |ack| match ack {
                Ack::DeckQ4ProcessSlot(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Apply a previously reported Q4 causal reset barrier with a strictly
    /// newer stream generation.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_reset(
        &mut self,
        request: Q4Reset,
        timeout: Duration,
    ) -> Result<Q4ResetAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4Reset(request),
            timeout,
            CommandName::DeckQ4Reset,
            |ack| match ack {
                Ack::DeckQ4Reset(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Request an explicit Q4 restart barrier without resetting state
    /// implicitly.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_restart(
        &mut self,
        request: Q4Restart,
        timeout: Duration,
    ) -> Result<Q4RestartAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4Restart(request),
            timeout,
            CommandName::DeckQ4Restart,
            |ack| match ack {
                Ack::DeckQ4Restart(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace the complete closed LD-Q4 control block atomically.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_controls_set(
        &mut self,
        request: Q4ControlsSet,
        timeout: Duration,
    ) -> Result<Q4ControlsSetAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4ControlsSet(request),
            timeout,
            CommandName::DeckQ4ControlsSet,
            |ack| match ack {
                Ack::DeckQ4ControlsSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace the explicit Carrier and ordered Donor B/C/D role permutation
    /// atomically between complete slots.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_roles_set(
        &mut self,
        request: Q4RolesSet,
        timeout: Duration,
    ) -> Result<Q4RolesSetAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4RolesSet(request),
            timeout,
            CommandName::DeckQ4RolesSet,
            |ack| match ack {
                Ack::DeckQ4RolesSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace all four independent play/pause and loop pairs atomically.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_transport_set(
        &mut self,
        request: Q4TransportSet,
        timeout: Duration,
    ) -> Result<Q4TransportSetAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4TransportSet(request),
            timeout,
            CommandName::DeckQ4TransportSet,
            |ack| match ack {
                Ack::DeckQ4TransportSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Replace the deterministic LD-Q4 seed within the exact u53 range.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_seed_set(
        &mut self,
        request: Q4SeedSet,
        timeout: Duration,
    ) -> Result<Q4SeedSetAck, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4SeedSet(request),
            timeout,
            CommandName::DeckQ4SeedSet,
            |ack| match ack {
                Ack::DeckQ4SeedSet(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Read the current worker-owned LD-Q4 state without advancing its clock.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_status(
        &mut self,
        timeout: Duration,
    ) -> Result<Q4Status, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4Status(EmptyPayload {}),
            timeout,
            CommandName::DeckQ4Status,
            |ack| match ack {
                Ack::DeckQ4Status(payload) => Some(payload),
                _ => None,
            },
        )
        .await
    }

    /// Arm Q4 Snapshot or Live Capture at the worker's next causal reset
    /// boundary.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_capture_start(
        &mut self,
        request: Q4CaptureStart,
        timeout: Duration,
    ) -> Result<Q4CaptureStatus, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4CaptureStart(request),
            timeout,
            CommandName::DeckQ4CaptureStart,
            |ack| match ack {
                Ack::DeckQ4CaptureStart(payload) => Some(*payload),
                _ => None,
            },
        )
        .await
    }

    /// Stop Q4 Live Capture at its next codec-valid boundary.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_capture_stop(
        &mut self,
        request: Q4CaptureStop,
        timeout: Duration,
    ) -> Result<Q4CaptureStatus, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4CaptureStop(request),
            timeout,
            CommandName::DeckQ4CaptureStop,
            |ack| match ack {
                Ack::DeckQ4CaptureStop(payload) => Some(*payload),
                _ => None,
            },
        )
        .await
    }

    /// Read one Q4 capture's bounded worker-owned state without advancing it.
    ///
    /// # Errors
    ///
    /// Returns the underlying worker failure or a typed acknowledgement
    /// mismatch.
    pub async fn deck_q4_capture_status(
        &mut self,
        request: Q4CaptureStatusRequest,
        timeout: Duration,
    ) -> Result<Q4CaptureStatus, WorkerClientError> {
        self.call_q4(
            Command::DeckQ4CaptureStatus(request),
            timeout,
            CommandName::DeckQ4CaptureStatus,
            |ack| match ack {
                Ack::DeckQ4CaptureStatus(payload) => Some(*payload),
                _ => None,
            },
        )
        .await
    }

    async fn call_q4<T>(
        &mut self,
        command: Command,
        timeout: Duration,
        expected: CommandName,
        extract: impl FnOnce(Ack) -> Option<T>,
    ) -> Result<T, WorkerClientError> {
        let ack = self.call(command, timeout).await?;
        extract_q4_ack(ack, expected, extract)
    }
}

fn extract_q4_ack<T>(
    ack: Ack,
    expected: CommandName,
    extract: impl FnOnce(Ack) -> Option<T>,
) -> Result<T, WorkerClientError> {
    let actual = ack.name();
    extract(ack).ok_or(WorkerClientError::UnexpectedAck { expected, actual })
}

#[cfg(test)]
mod tests {
    use latentdeck_control::{
        BoundedVec, Q4CaptureMode, Q4CaptureState, Q4Controls, Q4ResetAppliedKind,
        Q4ResetBarrierKind, Q4ResetReason, Q4Roles, Q4SeedSetAck, Q4Slot, Q4SourceStatus,
        Q4Transport, WireUuid,
    };

    use super::*;

    #[test]
    fn command_names_cover_the_complete_q4_client_surface() {
        let identity = ("main-q4".to_owned(), 1);
        let commands = [
            Command::DeckQ4Load(load_request()),
            Command::DeckQ4ProcessSlot(Q4ProcessSlot {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                stream_generation: 1,
            }),
            Command::DeckQ4Reset(Q4Reset {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                new_stream_generation: 2,
            }),
            Command::DeckQ4Restart(Q4Restart {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
            }),
            Command::DeckQ4ControlsSet(Q4ControlsSet {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                controls: Q4Controls::default(),
            }),
            Command::DeckQ4RolesSet(Q4RolesSet {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                roles: Q4Roles::default(),
            }),
            Command::DeckQ4TransportSet(Q4TransportSet {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                transport: Q4Transport::default(),
            }),
            Command::DeckQ4SeedSet(Q4SeedSet {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                seed: 42,
            }),
            Command::DeckQ4Status(EmptyPayload {}),
            Command::DeckQ4CaptureStart(Q4CaptureStart {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                capture_id: WireUuid::new_v4(),
                mode: Q4CaptureMode::Snapshot,
                temporary_root: "capture".to_owned(),
                max_latent_slots: 128,
                max_visual_bytes: 1024,
            }),
            Command::DeckQ4CaptureStop(Q4CaptureStop {
                deck_id: identity.0.clone(),
                deck_revision: identity.1,
                capture_id: WireUuid::new_v4(),
            }),
            Command::DeckQ4CaptureStatus(Q4CaptureStatusRequest {
                deck_id: identity.0,
                deck_revision: identity.1,
                capture_id: WireUuid::new_v4(),
            }),
        ];
        let expected = [
            CommandName::DeckQ4Load,
            CommandName::DeckQ4ProcessSlot,
            CommandName::DeckQ4Reset,
            CommandName::DeckQ4Restart,
            CommandName::DeckQ4ControlsSet,
            CommandName::DeckQ4RolesSet,
            CommandName::DeckQ4TransportSet,
            CommandName::DeckQ4SeedSet,
            CommandName::DeckQ4Status,
            CommandName::DeckQ4CaptureStart,
            CommandName::DeckQ4CaptureStop,
            CommandName::DeckQ4CaptureStatus,
        ];

        for (command, expected) in commands.into_iter().zip(expected) {
            assert_eq!(command.name(), expected);
        }
    }

    #[test]
    fn ack_names_cover_load_process_reset_updates_status_and_capture() {
        let status = status();
        let capture = capture_status();
        let acks = [
            Ack::DeckQ4Load(status.clone()),
            Ack::DeckQ4ProcessSlot(Q4ProcessSlotAck::Paused {
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                stream_generation: 1,
                playhead_a: 0,
                playhead_b: 0,
                playhead_c: 0,
                playhead_d: 0,
                roles: Q4Roles::default(),
                transport: Q4Transport::default(),
            }),
            Ack::DeckQ4Reset(Q4ResetAck {
                kind: Q4ResetAppliedKind::ResetApplied,
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                stream_generation: 2,
                playhead_a: 0,
                playhead_b: 0,
                playhead_c: 0,
                playhead_d: 0,
                reasons: BoundedVec::try_from_vec(vec![Q4ResetReason::TransportRestart]).unwrap(),
                causal_state_cleared: true,
            }),
            Ack::DeckQ4Restart(Q4RestartAck {
                kind: Q4ResetBarrierKind::ResetBarrier,
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                current_generation: 1,
                minimum_new_generation: 2,
                reasons: BoundedVec::try_from_vec(vec![Q4ResetReason::TransportRestart]).unwrap(),
            }),
            Ack::DeckQ4ControlsSet(Q4ControlsSetAck {
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                controls: Q4Controls::default(),
                requires_causal_reset: false,
            }),
            Ack::DeckQ4RolesSet(Q4RolesSetAck {
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                roles: Q4Roles::default(),
                requires_causal_reset: false,
            }),
            Ack::DeckQ4TransportSet(Q4TransportSetAck {
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                transport: Q4Transport::default(),
                requires_causal_reset: false,
            }),
            Ack::DeckQ4SeedSet(Q4SeedSetAck {
                deck_id: "main-q4".to_owned(),
                deck_revision: 1,
                seed: 42,
                requires_causal_reset: false,
            }),
            Ack::DeckQ4Status(status),
            Ack::DeckQ4CaptureStart(Box::new(capture.clone())),
            Ack::DeckQ4CaptureStop(Box::new(capture.clone())),
            Ack::DeckQ4CaptureStatus(Box::new(capture)),
        ];
        let expected = [
            CommandName::DeckQ4Load,
            CommandName::DeckQ4ProcessSlot,
            CommandName::DeckQ4Reset,
            CommandName::DeckQ4Restart,
            CommandName::DeckQ4ControlsSet,
            CommandName::DeckQ4RolesSet,
            CommandName::DeckQ4TransportSet,
            CommandName::DeckQ4SeedSet,
            CommandName::DeckQ4Status,
            CommandName::DeckQ4CaptureStart,
            CommandName::DeckQ4CaptureStop,
            CommandName::DeckQ4CaptureStatus,
        ];

        for (ack, expected) in acks.into_iter().zip(expected) {
            assert_eq!(ack.name(), expected);
        }
    }

    #[test]
    fn unexpected_ack_preserves_expected_and_actual_q4_names() {
        let error = extract_q4_ack(
            Ack::DeckQ4Status(status()),
            CommandName::DeckQ4Load,
            |ack| match ack {
                Ack::DeckQ4Load(payload) => Some(payload),
                _ => None,
            },
        )
        .expect_err("wrong typed Q4 acknowledgement must fail");

        assert!(matches!(
            error,
            WorkerClientError::UnexpectedAck {
                expected: CommandName::DeckQ4Load,
                actual: CommandName::DeckQ4Status,
            }
        ));
    }

    #[test]
    fn boxed_capture_status_and_realtime_role_seed_acks_extract_exactly() {
        let capture = capture_status();
        let extracted = extract_q4_ack(
            Ack::DeckQ4CaptureStatus(Box::new(capture.clone())),
            CommandName::DeckQ4CaptureStatus,
            |ack| match ack {
                Ack::DeckQ4CaptureStatus(payload) => Some(*payload),
                _ => None,
            },
        )
        .expect("matching capture ack");
        assert_eq!(extracted, capture);

        let roles = Q4Roles {
            carrier: Q4Slot::C,
            donor_b: Q4Slot::A,
            donor_c: Q4Slot::B,
            donor_d: Q4Slot::D,
        };
        let Ack::DeckQ4RolesSet(role_ack) = Ack::DeckQ4RolesSet(Q4RolesSetAck {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            roles,
            requires_causal_reset: false,
        }) else {
            unreachable!()
        };
        assert_eq!(role_ack.roles, roles);
        assert!(!role_ack.requires_causal_reset);

        let Ack::DeckQ4SeedSet(seed_ack) = Ack::DeckQ4SeedSet(Q4SeedSetAck {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            seed: 42,
            requires_causal_reset: false,
        }) else {
            unreachable!()
        };
        assert_eq!(seed_ack.seed, 42);
        assert!(!seed_ack.requires_causal_reset);
    }

    fn load_request() -> Q4Load {
        let source = |digest: char| latentdeck_control::Q4SourceBinding {
            cartridge_path: format!("{digest}.lc"),
            cartridge_id: WireUuid::new_v4(),
            expected_archive_sha256: digest.to_string().repeat(64),
        };
        Q4Load {
            deck_id: "main-q4".to_owned(),
            operator_id: "org.latentdeck.builtin.ld_q4".to_owned(),
            operator_version: "0.1.0".to_owned(),
            source_a: source('a'),
            source_b: source('b'),
            source_c: source('c'),
            source_d: source('d'),
            roles: Q4Roles::default(),
            controls: Q4Controls::default(),
            transport: Q4Transport::default(),
            seed: 42,
            stream_generation: 1,
        }
    }

    fn status() -> Q4Status {
        let source = |digest: char| Q4SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: digest.to_string().repeat(64),
            latent_slot_count: 7,
        };
        Q4Status {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            operator_id: "org.latentdeck.builtin.ld_q4".to_owned(),
            operator_version: "0.1.0".to_owned(),
            stream_generation: 1,
            stream_sequence: 0,
            playhead_a: 0,
            playhead_b: 0,
            playhead_c: 0,
            playhead_d: 0,
            roles: Q4Roles::default(),
            transport: Q4Transport::default(),
            controls: Q4Controls::default(),
            seed: 42,
            pending_reset: false,
            pending_reset_reasons: BoundedVec::default(),
            decoded_start_frame: 0,
            source_a: source('a'),
            source_b: source('b'),
            source_c: source('c'),
            source_d: source('d'),
        }
    }

    fn capture_status() -> Q4CaptureStatus {
        Q4CaptureStatus {
            capture_id: WireUuid::new_v4(),
            mode: Q4CaptureMode::LiveCapture,
            state: Q4CaptureState::AwaitingReset,
            structural_carrier: Q4Slot::A,
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
