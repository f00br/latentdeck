use std::io::Cursor;

use latentdeck_control::{
    Ack, AckReply, BoundedVec, Command, CommandName, D2Algorithm, D2CaptureAudioDescriptor,
    D2CaptureAudioDtype, D2CaptureAudioPolicy, D2CaptureAudioPolicyReason, D2CaptureControlEvent,
    D2CaptureMode, D2CaptureParent, D2CaptureReceipt, D2CaptureStart, D2CaptureState,
    D2CaptureStatus, D2CaptureStatusRequest, D2CaptureStop, D2CaptureVisualDtype, D2Controls,
    D2ControlsSet, D2ControlsSetAck, D2Load, D2Mode, D2ProcessSlot, D2ProcessSlotAck, D2Reset,
    D2ResetAck, D2ResetAppliedKind, D2ResetBarrierKind, D2ResetReason, D2Restart, D2RestartAck,
    D2Routing, D2SeedSet, D2SeedSetAck, D2SourceBinding, D2SourceStatus, D2Status, D2Transport,
    D2TransportSet, D2TransportSetAck, D2Xs5Routing, EmptyPayload, Envelope, ErrorCode, FiniteF64,
    FramingError, InboundPolicy, MAX_D2_SAFE_INTEGER, Message, SessionValidator, ValidationError,
    WireUuid, decode_envelope, read_envelope, write_envelope,
};
use serde::Serialize;
use uuid::Uuid;

fn id(value: u128) -> WireUuid {
    WireUuid::from_uuid(Uuid::from_u128(value))
}

fn source(cartridge_id: WireUuid, digest: char) -> D2SourceBinding {
    D2SourceBinding {
        cartridge_path: format!("{digest}.lc"),
        cartridge_id,
        expected_archive_sha256: digest.to_string().repeat(64),
    }
}

fn load() -> D2Load {
    D2Load {
        deck_id: "main-d2".to_owned(),
        operator_id: "org.latentdeck.builtin.ld_d2".to_owned(),
        operator_version: "0.1.0".to_owned(),
        source_a: source(id(11), 'a'),
        source_b: source(id(12), 'b'),
        controls: D2Controls::default(),
        transport: D2Transport::default(),
        seed: 42,
        stream_generation: 1,
    }
}

fn source_status(cartridge_id: WireUuid, digest: char) -> D2SourceStatus {
    D2SourceStatus {
        cartridge_id,
        archive_sha256: digest.to_string().repeat(64),
        latent_slot_count: 7,
    }
}

fn status() -> D2Status {
    D2Status {
        deck_id: "main-d2".to_owned(),
        deck_revision: 1,
        operator_id: "org.latentdeck.builtin.ld_d2".to_owned(),
        operator_version: "0.1.0".to_owned(),
        stream_generation: 1,
        stream_sequence: 0,
        playhead_a: 0,
        playhead_b: 0,
        transport: D2Transport::default(),
        controls: D2Controls::default(),
        seed: 42,
        pending_reset: false,
        pending_reset_reasons: BoundedVec::default(),
        decoded_start_frame: 0,
        source_a: source_status(id(11), 'a'),
        source_b: source_status(id(12), 'b'),
    }
}

fn command_envelope(command: Command) -> Envelope {
    Envelope::new(id(1), 1, id(2), 10, Message::Command(command))
}

fn decoded_slot_ack(provenance_json: impl Into<String>) -> Envelope {
    Envelope::new(
        id(1),
        1,
        id(3),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2ProcessSlot(D2ProcessSlotAck::DecodedSlot {
                deck_id: "main-d2".to_owned(),
                deck_revision: 1,
                stream_generation: 1,
                stream_sequence: 0,
                playhead_a: 0,
                playhead_b: 0,
                transport: D2Transport::default(),
                decoded_start_frame: 0,
                decoded_frame_count: 1,
                ring_first_sequence: 1,
                ring_last_sequence_exclusive: 2,
                provenance_json: provenance_json.into(),
            }),
        }),
    )
}

fn awaiting_capture_status(mode: D2CaptureMode) -> D2CaptureStatus {
    D2CaptureStatus {
        capture_id: id(30),
        mode,
        state: D2CaptureState::AwaitingReset,
        structural_carrier: D2Routing::A,
        latent_slots: 0,
        current_generation: Some(1),
        minimum_new_generation: Some(2),
        target_latent_slots: Some(if mode == D2CaptureMode::Snapshot {
            7
        } else {
            0
        }),
        stream_generation: None,
        finalize_after_latent_slots: None,
        reason: None,
        receipt: None,
    }
}

fn capture_parent(slot: D2Routing, cartridge_id: u128, digest: char) -> D2CaptureParent {
    D2CaptureParent {
        slot,
        cartridge_id: id(cartridge_id),
        archive_sha256: digest.to_string().repeat(64),
    }
}

fn snapshot_receipt() -> D2CaptureReceipt {
    D2CaptureReceipt {
        capture_id: id(30),
        mode: D2CaptureMode::Snapshot,
        payload_path: format!(r"W:\latentdeck-capture\{}.safetensors.partial", id(30)),
        payload_sha256: "c".repeat(64),
        payload_bytes: 4_200,
        storage_dtype: D2CaptureVisualDtype::F16,
        visual_shape: [1, 24, 7, 3, 4],
        decoded_frame_count: 22,
        audio_policy: D2CaptureAudioPolicy::SourceAbsent,
        audio_policy_reason: None,
        audio_descriptor: None,
        structural_carrier: D2Routing::A,
        parents: [
            capture_parent(D2Routing::A, 11, 'a'),
            capture_parent(D2Routing::B, 12, 'b'),
        ],
        frozen_seed: Some(42),
        frozen_controls: Some(D2Controls::default()),
        control_events: None,
    }
}

fn finished_capture_status(receipt: D2CaptureReceipt) -> D2CaptureStatus {
    D2CaptureStatus {
        capture_id: receipt.capture_id,
        mode: receipt.mode,
        state: D2CaptureState::Finished,
        structural_carrier: receipt.structural_carrier,
        latent_slots: receipt.visual_shape[2],
        current_generation: None,
        minimum_new_generation: None,
        target_latent_slots: None,
        stream_generation: Some(2),
        finalize_after_latent_slots: None,
        reason: None,
        receipt: Some(receipt),
    }
}

fn capture_status_envelope(status: D2CaptureStatus) -> Envelope {
    Envelope::new(
        id(1),
        1,
        id(59),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2CaptureStatus(Box::new(status)),
        }),
    )
}

fn round_trip(command: Command) {
    let expected = command_envelope(command);
    let mut framed = Vec::new();
    write_envelope(&mut framed, &expected).expect("valid D2 command");
    let actual = read_envelope(&mut Cursor::new(framed))
        .expect("valid frame")
        .expect("one frame");
    assert_eq!(actual, expected);
}

#[test]
fn all_d2_commands_round_trip_as_closed_typed_payloads() {
    let controls = D2Controls::default();
    let transport = D2Transport::default();
    let commands = [
        Command::DeckD2Load(load()),
        Command::DeckD2ProcessSlot(D2ProcessSlot {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            stream_generation: 1,
        }),
        Command::DeckD2Reset(D2Reset {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            new_stream_generation: 2,
        }),
        Command::DeckD2Restart(D2Restart {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
        }),
        Command::DeckD2ControlsSet(D2ControlsSet {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            controls,
        }),
        Command::DeckD2TransportSet(D2TransportSet {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            transport,
        }),
        Command::DeckD2SeedSet(D2SeedSet {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            seed: MAX_D2_SAFE_INTEGER,
        }),
        Command::DeckD2Status(EmptyPayload {}),
    ];

    for command in commands {
        round_trip(command);
    }
}

#[test]
fn capture_commands_and_status_acks_round_trip_with_corrected_modes() {
    let identity = ("main-d2".to_owned(), 1, id(30));
    let commands = [
        Command::DeckD2CaptureStart(D2CaptureStart {
            deck_id: identity.0.clone(),
            deck_revision: identity.1,
            capture_id: identity.2,
            mode: D2CaptureMode::Snapshot,
            temporary_root: r"W:\latentdeck-capture".to_owned(),
            max_latent_slots: 128,
            max_visual_bytes: 16 * 1024 * 1024,
        }),
        Command::DeckD2CaptureStop(D2CaptureStop {
            deck_id: identity.0.clone(),
            deck_revision: identity.1,
            capture_id: identity.2,
        }),
        Command::DeckD2CaptureStatus(D2CaptureStatusRequest {
            deck_id: identity.0,
            deck_revision: identity.1,
            capture_id: identity.2,
        }),
    ];
    for command in commands {
        round_trip(command);
    }

    let statuses = [
        Ack::DeckD2CaptureStart(Box::new(awaiting_capture_status(D2CaptureMode::Snapshot))),
        Ack::DeckD2CaptureStart(Box::new(awaiting_capture_status(
            D2CaptureMode::LiveCapture,
        ))),
        Ack::DeckD2CaptureStop(Box::new(awaiting_capture_status(
            D2CaptureMode::LiveCapture,
        ))),
        Ack::DeckD2CaptureStatus(Box::new(awaiting_capture_status(D2CaptureMode::Snapshot))),
    ];
    for (index, ack) in statuses.into_iter().enumerate() {
        let envelope = Envelope::new(
            id(1),
            1,
            id(40 + index as u128),
            20,
            Message::Ack(AckReply {
                reply_to: id(2),
                ack,
            }),
        );
        let encoded = latentdeck_control::encode_envelope(&envelope).expect("capture ack");
        assert_eq!(
            decode_envelope(&encoded).expect("capture ack decode"),
            envelope
        );
    }
}

#[test]
fn finished_snapshot_status_round_trips_a_strict_receipt() {
    let receipt = snapshot_receipt();
    let status = D2CaptureStatus {
        capture_id: id(30),
        mode: D2CaptureMode::Snapshot,
        state: D2CaptureState::Finished,
        structural_carrier: D2Routing::A,
        latent_slots: 7,
        current_generation: None,
        minimum_new_generation: None,
        target_latent_slots: None,
        stream_generation: Some(2),
        finalize_after_latent_slots: None,
        reason: None,
        receipt: Some(receipt),
    };
    let envelope = Envelope::new(
        id(1),
        1,
        id(50),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2CaptureStatus(Box::new(status)),
        }),
    );

    let encoded = latentdeck_control::encode_envelope(&envelope).expect("snapshot receipt");
    assert_eq!(decode_envelope(&encoded).expect("decode receipt"), envelope);
}

#[test]
fn finished_live_status_round_trips_bounded_control_events() {
    let changed_controls = D2Controls {
        algorithm: D2Algorithm::Xs1,
        ..D2Controls::default()
    };
    let events = BoundedVec::try_from_vec(vec![
        D2CaptureControlEvent {
            slot_offset: 0,
            controls: D2Controls::default(),
            seed: 42,
        },
        D2CaptureControlEvent {
            slot_offset: 2,
            controls: changed_controls,
            seed: 77,
        },
    ])
    .expect("bounded events");
    let mut receipt = snapshot_receipt();
    receipt.mode = D2CaptureMode::LiveCapture;
    receipt.frozen_seed = None;
    receipt.frozen_controls = None;
    receipt.control_events = Some(events);
    let status = D2CaptureStatus {
        capture_id: id(30),
        mode: D2CaptureMode::LiveCapture,
        state: D2CaptureState::Finished,
        structural_carrier: D2Routing::A,
        latent_slots: 7,
        current_generation: None,
        minimum_new_generation: None,
        target_latent_slots: None,
        stream_generation: Some(2),
        finalize_after_latent_slots: None,
        reason: None,
        receipt: Some(receipt),
    };
    let envelope = Envelope::new(
        id(1),
        1,
        id(51),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2CaptureStop(Box::new(status)),
        }),
    );

    let encoded = latentdeck_control::encode_envelope(&envelope).expect("live receipt");
    assert_eq!(decode_envelope(&encoded).expect("decode receipt"), envelope);
}

#[test]
fn active_and_aborted_capture_statuses_round_trip_exact_optional_fields() {
    let statuses = [
        D2CaptureStatus {
            capture_id: id(30),
            mode: D2CaptureMode::Snapshot,
            state: D2CaptureState::Capturing,
            structural_carrier: D2Routing::A,
            latent_slots: 1,
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: None,
            stream_generation: Some(2),
            finalize_after_latent_slots: None,
            reason: None,
            receipt: None,
        },
        D2CaptureStatus {
            capture_id: id(30),
            mode: D2CaptureMode::LiveCapture,
            state: D2CaptureState::StopArmed,
            structural_carrier: D2Routing::A,
            latent_slots: 3,
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: None,
            stream_generation: Some(2),
            finalize_after_latent_slots: Some(7),
            reason: None,
            receipt: None,
        },
        D2CaptureStatus {
            capture_id: id(30),
            mode: D2CaptureMode::LiveCapture,
            state: D2CaptureState::Aborted,
            structural_carrier: D2Routing::A,
            latent_slots: 0,
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: None,
            stream_generation: None,
            finalize_after_latent_slots: None,
            reason: Some("stopped_before_start".to_owned()),
            receipt: None,
        },
        D2CaptureStatus {
            capture_id: id(30),
            mode: D2CaptureMode::Snapshot,
            state: D2CaptureState::Aborted,
            structural_carrier: D2Routing::A,
            latent_slots: 2,
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: None,
            stream_generation: Some(2),
            finalize_after_latent_slots: None,
            reason: Some("process_or_decode_error".to_owned()),
            receipt: None,
        },
    ];

    for (index, status) in statuses.into_iter().enumerate() {
        let envelope = Envelope::new(
            id(1),
            1,
            id(60 + index as u128),
            20,
            Message::Ack(AckReply {
                reply_to: id(2),
                ack: Ack::DeckD2CaptureStatus(Box::new(status)),
            }),
        );
        let encoded = latentdeck_control::encode_envelope(&envelope)
            .expect("valid capture state must encode");
        assert_eq!(
            decode_envelope(&encoded).expect("valid capture state must decode"),
            envelope
        );
    }
}

#[test]
fn capture_status_rejects_cross_state_fields_and_invalid_boundaries() {
    let mut live_awaiting = awaiting_capture_status(D2CaptureMode::LiveCapture);
    live_awaiting.target_latent_slots = None;

    let capturing_without_generation = D2CaptureStatus {
        capture_id: id(30),
        mode: D2CaptureMode::Snapshot,
        state: D2CaptureState::Capturing,
        structural_carrier: D2Routing::A,
        latent_slots: 1,
        current_generation: None,
        minimum_new_generation: None,
        target_latent_slots: None,
        stream_generation: None,
        finalize_after_latent_slots: None,
        reason: None,
        receipt: None,
    };
    let mut snapshot_stop_armed = capturing_without_generation.clone();
    snapshot_stop_armed.state = D2CaptureState::StopArmed;
    snapshot_stop_armed.stream_generation = Some(2);
    snapshot_stop_armed.finalize_after_latent_slots = Some(7);

    let mut invalid_stop_boundary = snapshot_stop_armed.clone();
    invalid_stop_boundary.mode = D2CaptureMode::LiveCapture;
    invalid_stop_boundary.latent_slots = 3;
    invalid_stop_boundary.finalize_after_latent_slots = Some(4);

    let mut aborted_without_reason = capturing_without_generation.clone();
    aborted_without_reason.state = D2CaptureState::Aborted;

    let mut aborted_with_awaiting_field = aborted_without_reason.clone();
    aborted_with_awaiting_field.reason = Some("stopped_before_start".to_owned());
    aborted_with_awaiting_field.current_generation = Some(1);

    let invalid = [
        live_awaiting,
        capturing_without_generation,
        snapshot_stop_armed,
        invalid_stop_boundary,
        aborted_without_reason,
        aborted_with_awaiting_field,
    ];
    for status in invalid {
        assert!(
            latentdeck_control::encode_envelope(&capture_status_envelope(status)).is_err(),
            "invalid state-specific fields must fail"
        );
    }
}

#[test]
fn capture_commands_reject_legacy_modes_nil_ids_and_limit_overflow() {
    let valid = D2CaptureStart {
        deck_id: "main-d2".to_owned(),
        deck_revision: 1,
        capture_id: id(30),
        mode: D2CaptureMode::Snapshot,
        temporary_root: r"W:\latentdeck-capture".to_owned(),
        max_latent_slots: 2,
        max_visual_bytes: 1,
    };

    let mut nil_id = valid.clone();
    nil_id.capture_id = id(0);
    let mut too_few_slots = valid.clone();
    too_few_slots.max_latent_slots = 1;
    let mut too_many_slots = valid.clone();
    too_many_slots.max_latent_slots = 1_048_577;
    let mut zero_bytes = valid.clone();
    zero_bytes.max_visual_bytes = 0;
    let mut too_many_bytes = valid.clone();
    too_many_bytes.max_visual_bytes = 15 * 1024 * 1024 * 1024 + 1;

    for request in [
        nil_id,
        too_few_slots,
        too_many_slots,
        zero_bytes,
        too_many_bytes,
    ] {
        assert!(
            latentdeck_control::encode_envelope(&command_envelope(Command::DeckD2CaptureStart(
                request
            )))
            .is_err()
        );
    }

    for legacy in ["SNAPSHOT", "LIVE"] {
        let mut raw =
            serde_json::to_value(command_envelope(Command::DeckD2CaptureStart(valid.clone())))
                .expect("serialize typed command");
        raw["message"]["body"]["payload"]["mode"] = serde_json::json!(legacy);
        let encoded = rmp_serde::to_vec_named(&raw).expect("encode legacy wire value");
        assert!(
            decode_envelope(&encoded).is_err(),
            "legacy capture mode {legacy} must remain rejected"
        );
    }
}

#[test]
fn capture_receipt_rejects_wrong_payload_identity_and_mode_provenance() {
    let mut wrong_basename = snapshot_receipt();
    wrong_basename.payload_path = r"W:\latentdeck-capture\other.safetensors.partial".to_owned();

    let mut uppercase_digest = snapshot_receipt();
    uppercase_digest.payload_sha256 = "C".repeat(64);

    let mut wrong_frames = snapshot_receipt();
    wrong_frames.decoded_frame_count = 23;

    let mut wrong_parent_order = snapshot_receipt();
    wrong_parent_order.parents.swap(0, 1);

    let mut live_with_frozen_snapshot = snapshot_receipt();
    live_with_frozen_snapshot.mode = D2CaptureMode::LiveCapture;

    let mut snapshot_with_live_events = snapshot_receipt();
    snapshot_with_live_events.control_events = Some(
        BoundedVec::try_from_vec(vec![D2CaptureControlEvent {
            slot_offset: 0,
            controls: D2Controls::default(),
            seed: 42,
        }])
        .unwrap(),
    );

    for receipt in [
        wrong_basename,
        uppercase_digest,
        wrong_frames,
        wrong_parent_order,
        live_with_frozen_snapshot,
        snapshot_with_live_events,
    ] {
        assert!(
            latentdeck_control::encode_envelope(&capture_status_envelope(finished_capture_status(
                receipt
            )))
            .is_err(),
            "invalid receipt identity/provenance must fail"
        );
    }
}

#[test]
fn capture_receipt_audio_policy_matrix_is_closed_and_cadence_checked() {
    let mut copied = snapshot_receipt();
    copied.payload_bytes = 9_000;
    copied.audio_policy = D2CaptureAudioPolicy::CopiedFromCarrierExact;
    copied.audio_descriptor = Some(D2CaptureAudioDescriptor {
        storage_dtype: D2CaptureAudioDtype::F16,
        shape: [1, 32, 2, 37],
        byte_length: 4_736,
    });
    latentdeck_control::encode_envelope(&capture_status_envelope(finished_capture_status(
        copied.clone(),
    )))
    .expect("exact snapshot carrier audio must be valid");

    let mut copied_with_reason = copied.clone();
    copied_with_reason.audio_policy_reason = Some(D2CaptureAudioPolicyReason::DurationMismatch);

    let mut copied_wrong_cadence = copied;
    copied_wrong_cadence
        .audio_descriptor
        .as_mut()
        .unwrap()
        .shape[3] = 38;
    copied_wrong_cadence
        .audio_descriptor
        .as_mut()
        .unwrap()
        .byte_length = 4_864;

    let mut source_absent_with_descriptor = snapshot_receipt();
    source_absent_with_descriptor.audio_descriptor = Some(D2CaptureAudioDescriptor {
        storage_dtype: D2CaptureAudioDtype::F32,
        shape: [1, 32, 2, 37],
        byte_length: 9_472,
    });

    let mut snapshot_omitted = snapshot_receipt();
    snapshot_omitted.audio_policy = D2CaptureAudioPolicy::OmittedTimingMismatch;
    snapshot_omitted.audio_policy_reason = Some(D2CaptureAudioPolicyReason::DurationMismatch);

    for receipt in [
        copied_with_reason,
        copied_wrong_cadence,
        source_absent_with_descriptor,
        snapshot_omitted,
    ] {
        assert!(
            latentdeck_control::encode_envelope(&capture_status_envelope(finished_capture_status(
                receipt
            )))
            .is_err(),
            "invalid audio policy combination must fail"
        );
    }
}

#[test]
fn d2_process_ack_is_exactly_one_closed_result_variant() {
    let variants = [
        D2ProcessSlotAck::DecodedSlot {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            stream_generation: 1,
            stream_sequence: 0,
            playhead_a: 0,
            playhead_b: 0,
            transport: D2Transport::default(),
            decoded_start_frame: 0,
            decoded_frame_count: 1,
            ring_first_sequence: 1,
            ring_last_sequence_exclusive: 2,
            provenance_json: "{}".to_owned(),
        },
        D2ProcessSlotAck::ResetBarrier {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            current_generation: 1,
            minimum_new_generation: 2,
            reasons: BoundedVec::try_from_vec(vec![D2ResetReason::SlotALoop]).unwrap(),
        },
        D2ProcessSlotAck::Paused {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            stream_generation: 1,
            playhead_a: 0,
            playhead_b: 0,
            transport: D2Transport {
                playing_a: false,
                playing_b: false,
                loop_a: true,
                loop_b: true,
            },
        },
    ];

    for (index, variant) in variants.into_iter().enumerate() {
        let envelope = Envelope::new(
            id(1),
            1,
            id(100 + index as u128),
            20,
            Message::Ack(AckReply {
                reply_to: id(2),
                ack: Ack::DeckD2ProcessSlot(variant),
            }),
        );
        let encoded = latentdeck_control::encode_envelope(&envelope).expect("valid D2 ack");
        assert_eq!(decode_envelope(&encoded).expect("round trip"), envelope);
    }
}

#[test]
fn decoded_slot_rejects_malformed_provenance_json() {
    let malformed = decoded_slot_ack("{not-json");
    let error = latentdeck_control::encode_envelope(&malformed)
        .expect_err("malformed provenance must fail validation");
    assert!(matches!(
        error,
        FramingError::Validation(ValidationError::InvalidField {
            field: "d2.provenance_json",
            ..
        })
    ));

    let untrusted_wire = rmp_serde::to_vec_named(&malformed).expect("raw malformed wire");
    assert!(matches!(
        decode_envelope(&untrusted_wire),
        Err(FramingError::Validation(ValidationError::InvalidField {
            field: "d2.provenance_json",
            ..
        }))
    ));
}

#[test]
fn decoded_slot_rejects_non_object_provenance_json() {
    for non_object in ["null", "true", "42", r#""text""#, "[]"] {
        let error = latentdeck_control::encode_envelope(&decoded_slot_ack(non_object))
            .expect_err("provenance root must be an object");
        assert!(matches!(
            error,
            FramingError::Validation(ValidationError::InvalidField {
                field: "d2.provenance_json",
                ..
            })
        ));
    }
}

#[test]
fn decoded_slot_rejects_non_finite_json_numbers() {
    for non_finite in [
        r#"{"value":NaN}"#,
        r#"{"value":Infinity}"#,
        r#"{"value":1e400}"#,
    ] {
        let error = latentdeck_control::encode_envelope(&decoded_slot_ack(non_finite))
            .expect_err("non-finite provenance number must fail validation");
        assert!(matches!(
            error,
            FramingError::Validation(ValidationError::InvalidField {
                field: "d2.provenance_json",
                ..
            })
        ));
    }
}

#[test]
fn decoded_slot_accepts_current_python_shaped_provenance_object() {
    let provenance = serde_json::json!({
        "operation": {
            "operator_id": "org.latentdeck.builtin.ld_d2",
            "operator_version": "0.1.0",
            "seed": 42,
            "controls": {"algorithm": "XS5", "xs5_routing": "TOPK"}
        },
        "profile": {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
            "frame_rate": {"numerator": 24, "denominator": 1}
        },
        "playheads": {"a": 0, "b": 1},
        "structural_carrier": "A",
        "grid": {"height": 56, "width": 100, "tokens": 5600, "full": true},
        "history": {"previous_a_supplied": false, "previous_b_supplied": false},
        "stream": {
            "generation": 1,
            "sequence": 0,
            "sources": {
                "a": {
                    "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
                    "archive_sha256": "a".repeat(64),
                    "playhead": 0
                },
                "b": {
                    "cartridge_id": "550e8400-e29b-41d4-a716-446655440001",
                    "archive_sha256": "b".repeat(64),
                    "playhead": 1
                }
            }
        }
    })
    .to_string();
    let envelope = decoded_slot_ack(provenance);

    let encoded = latentdeck_control::encode_envelope(&envelope)
        .expect("current Python provenance object must stay compatible");
    assert_eq!(
        decode_envelope(&encoded).expect("decode valid provenance"),
        envelope
    );
}

#[test]
fn every_d2_ack_has_the_correlated_command_name() {
    let controls = D2Controls::default();
    let transport = D2Transport::default();
    let acks = [
        Ack::DeckD2Load(status()),
        Ack::DeckD2ProcessSlot(D2ProcessSlotAck::Paused {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            stream_generation: 1,
            playhead_a: 0,
            playhead_b: 0,
            transport,
        }),
        Ack::DeckD2Reset(D2ResetAck {
            kind: D2ResetAppliedKind::ResetApplied,
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            stream_generation: 2,
            playhead_a: 0,
            playhead_b: 0,
            reasons: BoundedVec::try_from_vec(vec![D2ResetReason::TransportRestart]).unwrap(),
            causal_state_cleared: true,
        }),
        Ack::DeckD2Restart(D2RestartAck {
            kind: D2ResetBarrierKind::ResetBarrier,
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            current_generation: 1,
            minimum_new_generation: 2,
            reasons: BoundedVec::try_from_vec(vec![D2ResetReason::TransportRestart]).unwrap(),
        }),
        Ack::DeckD2ControlsSet(D2ControlsSetAck {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            controls,
            requires_causal_reset: false,
        }),
        Ack::DeckD2TransportSet(D2TransportSetAck {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            transport,
            requires_causal_reset: false,
        }),
        Ack::DeckD2SeedSet(D2SeedSetAck {
            deck_id: "main-d2".to_owned(),
            deck_revision: 1,
            seed: 42,
            requires_causal_reset: false,
        }),
        Ack::DeckD2Status(status()),
    ];
    let expected = [
        CommandName::DeckD2Load,
        CommandName::DeckD2ProcessSlot,
        CommandName::DeckD2Reset,
        CommandName::DeckD2Restart,
        CommandName::DeckD2ControlsSet,
        CommandName::DeckD2TransportSet,
        CommandName::DeckD2SeedSet,
        CommandName::DeckD2Status,
    ];

    for (ack, expected_name) in acks.into_iter().zip(expected) {
        assert_eq!(ack.name(), expected_name);
    }
}

#[test]
fn session_correlation_accepts_matching_d2_ack_and_rejects_wrong_name() {
    let session_id = id(1);
    let command_id = id(2);
    let command = Envelope::new(
        session_id,
        1,
        command_id,
        10,
        Message::Command(Command::DeckD2Load(load())),
    );
    let matching = Envelope::new(
        session_id,
        1,
        id(3),
        20,
        Message::Ack(AckReply {
            reply_to: command_id,
            ack: Ack::DeckD2Load(status()),
        }),
    );
    let wrong = Envelope::new(
        session_id,
        1,
        id(4),
        20,
        Message::Ack(AckReply {
            reply_to: command_id,
            ack: Ack::DeckD2Status(status()),
        }),
    );

    let mut accepted = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    accepted.track_outbound_command(&command).unwrap();
    accepted.validate_inbound(&matching).unwrap();

    let mut rejected = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    rejected.track_outbound_command(&command).unwrap();
    assert!(matches!(
        rejected.validate_inbound(&wrong),
        Err(ValidationError::ReplyNameMismatch {
            expected: CommandName::DeckD2Load,
            actual: CommandName::DeckD2Status,
        })
    ));
}

#[test]
fn d2_controls_reject_non_finite_out_of_range_and_conflicting_channels() {
    let nan = rmp_serde::to_vec_named(&f64::NAN).unwrap();
    assert!(rmp_serde::from_slice::<FiniteF64>(&nan).is_err());

    let mut outside = load();
    outside.controls.mix = FiniteF64::new(1.01).unwrap();
    assert!(matches!(
        latentdeck_control::encode_envelope(&command_envelope(Command::DeckD2Load(outside))),
        Err(FramingError::Validation(ValidationError::InvalidField {
            field: "d2.controls.mix",
            ..
        }))
    ));

    let mut conflict = load();
    conflict.controls.xs1_channel_b = conflict.controls.xs1_channel_a;
    assert!(matches!(
        latentdeck_control::encode_envelope(&command_envelope(Command::DeckD2Load(conflict))),
        Err(FramingError::Validation(ValidationError::InvalidField {
            field: "d2.controls.xs1_channels",
            ..
        }))
    ));
}

#[test]
fn d2_integer_and_ack_invariants_are_bounded_before_transport() {
    let invalid_seed = Command::DeckD2SeedSet(D2SeedSet {
        deck_id: "main-d2".to_owned(),
        deck_revision: 1,
        seed: MAX_D2_SAFE_INTEGER + 1,
    });
    assert!(latentdeck_control::encode_envelope(&command_envelope(invalid_seed)).is_err());

    let invalid_frames = Envelope::new(
        id(1),
        1,
        id(3),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2ProcessSlot(D2ProcessSlotAck::DecodedSlot {
                deck_id: "main-d2".to_owned(),
                deck_revision: 1,
                stream_generation: 1,
                stream_sequence: 0,
                playhead_a: 0,
                playhead_b: 0,
                transport: D2Transport::default(),
                decoded_start_frame: 0,
                decoded_frame_count: 5,
                ring_first_sequence: 1,
                ring_last_sequence_exclusive: 6,
                provenance_json: "{}".to_owned(),
            }),
        }),
    );
    assert!(latentdeck_control::encode_envelope(&invalid_frames).is_err());

    let invalid_ring_range = Envelope::new(
        id(1),
        1,
        id(33),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2ProcessSlot(D2ProcessSlotAck::DecodedSlot {
                deck_id: "main-d2".to_owned(),
                deck_revision: 1,
                stream_generation: 1,
                stream_sequence: 0,
                playhead_a: 0,
                playhead_b: 0,
                transport: D2Transport::default(),
                decoded_start_frame: 0,
                decoded_frame_count: 2,
                ring_first_sequence: 1,
                ring_last_sequence_exclusive: 2,
                provenance_json: "{}".to_owned(),
            }),
        }),
    );
    assert!(latentdeck_control::encode_envelope(&invalid_ring_range).is_err());

    let mut pending = status();
    pending.pending_reset = true;
    let invalid_status = Envelope::new(
        id(1),
        1,
        id(4),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckD2Status(pending),
        }),
    );
    assert!(latentdeck_control::encode_envelope(&invalid_status).is_err());
}

#[derive(Serialize)]
struct ControlsWithUnknown {
    algorithm: D2Algorithm,
    mix: f64,
    mode: D2Mode,
    routing: D2Routing,
    interaction: f64,
    preserve: f64,
    chaos: f64,
    xs1_channel_a: u8,
    xs1_channel_b: u8,
    xs1_angle_degrees: f64,
    xs2_radius: u8,
    xs3_high_gain: f64,
    xs4_epsilon: f64,
    xs5_routing: D2Xs5Routing,
    temperature: f64,
    top_k: u8,
    sinkhorn_iterations: u8,
    hidden_resize: bool,
}

#[test]
fn d2_controls_are_closed_against_hidden_behavior() {
    let encoded = rmp_serde::to_vec_named(&ControlsWithUnknown {
        algorithm: D2Algorithm::Linear,
        mix: 0.5,
        mode: D2Mode::Hybridize,
        routing: D2Routing::A,
        interaction: 0.0,
        preserve: 0.55,
        chaos: 0.0,
        xs1_channel_a: 0,
        xs1_channel_b: 1,
        xs1_angle_degrees: 30.0,
        xs2_radius: 1,
        xs3_high_gain: 0.5,
        xs4_epsilon: 0.000_001,
        xs5_routing: D2Xs5Routing::TopK,
        temperature: 0.12,
        top_k: 8,
        sinkhorn_iterations: 5,
        hidden_resize: true,
    })
    .unwrap();

    assert!(rmp_serde::from_slice::<D2Controls>(&encoded).is_err());
}

#[test]
fn d2_error_codes_use_stable_dotted_wire_values() {
    let cases = [
        (ErrorCode::OperatorNotInstalled, "operator.not_installed"),
        (
            ErrorCode::OperatorVersionMismatch,
            "operator.version_mismatch",
        ),
        (ErrorCode::OperatorNotTrusted, "operator.not_trusted"),
        (
            ErrorCode::OperatorProfileIncompatible,
            "operator.profile_incompatible",
        ),
        (
            ErrorCode::DeckSourceIncompatible,
            "deck.source_incompatible",
        ),
        (ErrorCode::DeckProcessFailed, "deck.process_failed"),
        (ErrorCode::DeckResetFailed, "deck.reset_failed"),
        (ErrorCode::DeckResetNotRequired, "deck.reset_not_required"),
        (
            ErrorCode::DeckGenerationExhausted,
            "deck.generation_exhausted",
        ),
        (ErrorCode::CaptureAlreadyActive, "capture.already_active"),
        (
            ErrorCode::CaptureBoundaryInvalid,
            "capture.boundary_invalid",
        ),
        (
            ErrorCode::CaptureBoundaryUnavailable,
            "capture.boundary_unavailable",
        ),
        (ErrorCode::CaptureCarrierPaused, "capture.carrier_paused"),
        (
            ErrorCode::CaptureControlsInvalid,
            "capture.controls_invalid",
        ),
        (ErrorCode::CaptureEventLimit, "capture.event_limit"),
        (ErrorCode::CaptureIdInvalid, "capture.id_invalid"),
        (ErrorCode::CaptureIdMismatch, "capture.id_mismatch"),
        (ErrorCode::CaptureInvalidState, "capture.invalid_state"),
        (ErrorCode::CaptureLimitExceeded, "capture.limit_exceeded"),
        (ErrorCode::CaptureMappingChanged, "capture.mapping_changed"),
        (ErrorCode::CaptureModeInvalid, "capture.mode_invalid"),
        (ErrorCode::CaptureNotFound, "capture.not_found"),
        (
            ErrorCode::CaptureProvenanceInvalid,
            "capture.provenance_invalid",
        ),
        (
            ErrorCode::CaptureReceiptTooLarge,
            "capture.receipt_too_large",
        ),
        (ErrorCode::CaptureSnapshotFrozen, "capture.snapshot_frozen"),
        (
            ErrorCode::CaptureSourceCycleIncompatible,
            "capture.source_cycle_incompatible",
        ),
        (ErrorCode::CaptureStartFailed, "capture.start_failed"),
        (
            ErrorCode::CaptureTemporaryRootInvalid,
            "capture.temporary_root_invalid",
        ),
        (
            ErrorCode::CaptureTransportLocked,
            "capture.transport_locked",
        ),
        (ErrorCode::CaptureWriteFailed, "capture.write_failed"),
    ];

    for (code, wire) in cases {
        let encoded = rmp_serde::to_vec_named(&code).unwrap();
        assert!(
            encoded
                .windows(wire.len())
                .any(|window| window == wire.as_bytes())
        );
    }
}
