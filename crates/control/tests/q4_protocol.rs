use std::io::Cursor;

use latentdeck_control::{
    Ack, AckReply, BoundedVec, Command, CommandName, EmptyPayload, Envelope, ErrorCode,
    ErrorPayload, ErrorReply, FiniteF64, FramingError, InboundPolicy, Message, Q4Algorithm,
    Q4CaptureAudioPolicy, Q4CaptureControlEvent, Q4CaptureMode, Q4CaptureParent, Q4CaptureReceipt,
    Q4CaptureStart, Q4CaptureState, Q4CaptureStatus, Q4CaptureStatusRequest, Q4CaptureStop,
    Q4CaptureVisualDtype, Q4Controls, Q4ControlsSet, Q4ControlsSetAck, Q4InfluenceMode, Q4Load,
    Q4Mode, Q4ProcessSlot, Q4ProcessSlotAck, Q4Reset, Q4ResetAck, Q4ResetAppliedKind,
    Q4ResetBarrierKind, Q4ResetReason, Q4Restart, Q4RestartAck, Q4Roles, Q4RolesSet, Q4RolesSetAck,
    Q4SeedSet, Q4SeedSetAck, Q4Slot, Q4SourceBinding, Q4SourceStatus, Q4Status, Q4Transport,
    Q4TransportSet, Q4TransportSetAck, Q4Xs5Routing, SessionValidator, ValidationError, WireUuid,
    WorkerState, decode_envelope, encode_envelope, read_envelope, write_envelope,
};
use serde::Serialize;
use uuid::Uuid;

fn id(value: u128) -> WireUuid {
    WireUuid::from_uuid(Uuid::from_u128(value))
}

fn source(cartridge_id: u128, digest: char) -> Q4SourceBinding {
    Q4SourceBinding {
        cartridge_path: format!("{digest}.lc"),
        cartridge_id: id(cartridge_id),
        expected_archive_sha256: digest.to_string().repeat(64),
    }
}

fn source_status(cartridge_id: u128, digest: char) -> Q4SourceStatus {
    Q4SourceStatus {
        cartridge_id: id(cartridge_id),
        archive_sha256: digest.to_string().repeat(64),
        latent_slot_count: 7,
    }
}

fn load() -> Q4Load {
    Q4Load {
        deck_id: "main-q4".to_owned(),
        operator_id: "org.latentdeck.builtin.ld_q4".to_owned(),
        operator_version: "0.1.0".to_owned(),
        source_a: source(11, 'a'),
        source_b: source(12, 'b'),
        source_c: source(13, 'c'),
        source_d: source(14, 'd'),
        roles: Q4Roles::default(),
        controls: Q4Controls::default(),
        transport: Q4Transport::default(),
        seed: 42,
        stream_generation: 1,
    }
}

fn status() -> Q4Status {
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
        source_a: source_status(11, 'a'),
        source_b: source_status(12, 'b'),
        source_c: source_status(13, 'c'),
        source_d: source_status(14, 'd'),
    }
}

fn command_envelope(command: Command) -> Envelope {
    Envelope::new(id(1), 1, id(2), 10, Message::Command(command))
}

fn round_trip(command: Command) {
    let expected = command_envelope(command);
    let mut framed = Vec::new();
    write_envelope(&mut framed, &expected).expect("valid Q4 command");
    let actual = read_envelope(&mut Cursor::new(framed))
        .expect("valid frame")
        .expect("one frame");
    assert_eq!(actual, expected);
}

fn process_ack(provenance_json: impl Into<String>) -> Q4ProcessSlotAck {
    Q4ProcessSlotAck::DecodedSlot {
        deck_id: "main-q4".to_owned(),
        deck_revision: 1,
        stream_generation: 1,
        stream_sequence: 0,
        playhead_a: 0,
        playhead_b: 1,
        playhead_c: 2,
        playhead_d: 3,
        roles: Q4Roles::default(),
        transport: Q4Transport::default(),
        decoded_start_frame: 0,
        decoded_frame_count: 1,
        ring_first_sequence: 1,
        ring_last_sequence_exclusive: 2,
        provenance_json: provenance_json.into(),
    }
}

fn capture_parent(slot: Q4Slot, cartridge_id: u128, digest: char) -> Q4CaptureParent {
    Q4CaptureParent {
        slot,
        cartridge_id: id(cartridge_id),
        archive_sha256: digest.to_string().repeat(64),
    }
}

fn snapshot_receipt() -> Q4CaptureReceipt {
    Q4CaptureReceipt {
        capture_id: id(30),
        mode: Q4CaptureMode::Snapshot,
        payload_path: format!(r"W:\latentdeck-capture\{}.safetensors.partial", id(30)),
        payload_sha256: "e".repeat(64),
        payload_bytes: 4_200,
        storage_dtype: Q4CaptureVisualDtype::F16,
        visual_shape: [1, 24, 7, 3, 4],
        decoded_frame_count: 22,
        audio_policy: Q4CaptureAudioPolicy::SourceAbsent,
        audio_policy_reason: None,
        audio_descriptor: None,
        structural_carrier: Q4Slot::A,
        parents: [
            capture_parent(Q4Slot::A, 11, 'a'),
            capture_parent(Q4Slot::B, 12, 'b'),
            capture_parent(Q4Slot::C, 13, 'c'),
            capture_parent(Q4Slot::D, 14, 'd'),
        ],
        frozen_seed: Some(42),
        frozen_roles: Some(Q4Roles::default()),
        frozen_controls: Some(Q4Controls::default()),
        control_events: None,
    }
}

fn finished_status(receipt: Q4CaptureReceipt) -> Q4CaptureStatus {
    Q4CaptureStatus {
        capture_id: receipt.capture_id,
        mode: receipt.mode,
        state: Q4CaptureState::Finished,
        structural_carrier: receipt.structural_carrier,
        latent_slots: receipt.visual_shape[2],
        current_generation: None,
        minimum_new_generation: None,
        target_latent_slots: None,
        stream_generation: Some(2),
        finalize_after_latent_slots: None,
        reason: None,
        receipt: Some(Box::new(receipt)),
    }
}

fn capture_status_ack(status: Q4CaptureStatus) -> Envelope {
    Envelope::new(
        id(1),
        1,
        id(50),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckQ4CaptureStatus(Box::new(status)),
        }),
    )
}

#[test]
fn all_q4_commands_round_trip_as_closed_typed_payloads() {
    let identity = ("main-q4".to_owned(), 1);
    let commands = [
        Command::DeckQ4Load(load()),
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
            roles: Q4Roles {
                carrier: Q4Slot::C,
                donor_b: Q4Slot::A,
                donor_c: Q4Slot::B,
                donor_d: Q4Slot::D,
            },
        }),
        Command::DeckQ4TransportSet(Q4TransportSet {
            deck_id: identity.0.clone(),
            deck_revision: identity.1,
            transport: Q4Transport::default(),
        }),
        Command::DeckQ4SeedSet(Q4SeedSet {
            deck_id: identity.0,
            deck_revision: identity.1,
            seed: 9_007_199_254_740_991,
        }),
        Command::DeckQ4Status(EmptyPayload {}),
    ];

    for command in commands {
        round_trip(command);
    }
}

#[test]
fn q4_capture_commands_and_snapshot_receipt_round_trip() {
    for command in [
        Command::DeckQ4CaptureStart(Q4CaptureStart {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            capture_id: id(30),
            mode: Q4CaptureMode::Snapshot,
            temporary_root: r"W:\latentdeck-capture".to_owned(),
            max_latent_slots: 128,
            max_visual_bytes: 16 * 1024 * 1024,
        }),
        Command::DeckQ4CaptureStop(Q4CaptureStop {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            capture_id: id(30),
        }),
        Command::DeckQ4CaptureStatus(Q4CaptureStatusRequest {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            capture_id: id(30),
        }),
    ] {
        round_trip(command);
    }

    let envelope = capture_status_ack(finished_status(snapshot_receipt()));
    let encoded = encode_envelope(&envelope).expect("valid Q4 snapshot receipt");
    assert_eq!(decode_envelope(&encoded).expect("decode receipt"), envelope);
}

#[test]
fn q4_live_receipt_records_ordered_roles_controls_and_seed_events() {
    let roles = Q4Roles {
        carrier: Q4Slot::C,
        donor_b: Q4Slot::A,
        donor_c: Q4Slot::B,
        donor_d: Q4Slot::D,
    };
    let mut receipt = snapshot_receipt();
    receipt.mode = Q4CaptureMode::LiveCapture;
    receipt.structural_carrier = Q4Slot::C;
    receipt.frozen_seed = None;
    receipt.frozen_roles = None;
    receipt.frozen_controls = None;
    receipt.control_events = Some(
        BoundedVec::try_from_vec(vec![
            Q4CaptureControlEvent {
                slot_offset: 0,
                roles,
                controls: Q4Controls::default(),
                seed: 42,
            },
            Q4CaptureControlEvent {
                slot_offset: 2,
                roles,
                controls: Q4Controls {
                    algorithm: Q4Algorithm::Xs5,
                    interaction: FiniteF64::new(0.75).unwrap(),
                    ..Q4Controls::default()
                },
                seed: 77,
            },
        ])
        .expect("bounded events"),
    );

    let envelope = capture_status_ack(finished_status(receipt));
    encode_envelope(&envelope).expect("valid Q4 live provenance");
}

#[test]
fn q4_roles_are_an_exact_permutation_at_load_update_status_and_capture() {
    let duplicate = Q4Roles {
        carrier: Q4Slot::A,
        donor_b: Q4Slot::A,
        donor_c: Q4Slot::C,
        donor_d: Q4Slot::D,
    };

    let mut invalid_load = load();
    invalid_load.roles = duplicate;
    assert!(encode_envelope(&command_envelope(Command::DeckQ4Load(invalid_load))).is_err());

    let invalid_update = Q4RolesSet {
        deck_id: "main-q4".to_owned(),
        deck_revision: 1,
        roles: duplicate,
    };
    assert!(encode_envelope(&command_envelope(Command::DeckQ4RolesSet(invalid_update))).is_err());

    let mut invalid_status = status();
    invalid_status.roles = duplicate;
    let envelope = Envelope::new(
        id(1),
        1,
        id(3),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckQ4Status(invalid_status),
        }),
    );
    assert!(encode_envelope(&envelope).is_err());

    let mut invalid_receipt = snapshot_receipt();
    invalid_receipt.frozen_roles = Some(duplicate);
    assert!(encode_envelope(&capture_status_ack(finished_status(invalid_receipt))).is_err());
}

#[test]
fn q4_controls_reject_hidden_behavior_invalid_distribution_and_outside_triangle() {
    let mut zero_manual = load();
    zero_manual.controls.donor_weight_b = FiniteF64::new(0.0).unwrap();
    zero_manual.controls.donor_weight_c = FiniteF64::new(0.0).unwrap();
    zero_manual.controls.donor_weight_d = FiniteF64::new(0.0).unwrap();
    assert!(encode_envelope(&command_envelope(Command::DeckQ4Load(zero_manual))).is_err());

    let mut outside_triangle = load();
    outside_triangle.controls.influence_mode = Q4InfluenceMode::Triangle;
    outside_triangle.controls.triangle_x = FiniteF64::new(0.1).unwrap();
    outside_triangle.controls.triangle_y = FiniteF64::new(0.9).unwrap();
    assert!(encode_envelope(&command_envelope(Command::DeckQ4Load(outside_triangle))).is_err());

    let mut raw = serde_json::to_value(command_envelope(Command::DeckQ4Load(load()))).unwrap();
    raw["message"]["body"]["payload"]["controls"]["hidden_downscale"] = serde_json::json!(true);
    let wire = rmp_serde::to_vec_named(&raw).unwrap();
    assert!(decode_envelope(&wire).is_err());
}

#[test]
fn q4_process_ack_is_closed_bounded_and_requires_object_provenance() {
    let valid = Envelope::new(
        id(1),
        1,
        id(3),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckQ4ProcessSlot(process_ack(r#"{"algorithm":"XS5"}"#)),
        }),
    );
    encode_envelope(&valid).expect("valid decoded slot");

    for provenance in ["[]", "null", "{not-json"] {
        let envelope = Envelope::new(
            id(1),
            1,
            id(4),
            20,
            Message::Ack(AckReply {
                reply_to: id(2),
                ack: Ack::DeckQ4ProcessSlot(process_ack(provenance)),
            }),
        );
        assert!(matches!(
            encode_envelope(&envelope),
            Err(FramingError::Validation(ValidationError::InvalidField {
                field: "q4.provenance_json",
                ..
            }))
        ));
    }

    let mut invalid_range = process_ack("{}");
    let Q4ProcessSlotAck::DecodedSlot {
        ring_last_sequence_exclusive,
        ..
    } = &mut invalid_range
    else {
        unreachable!("helper always returns a decoded slot")
    };
    *ring_last_sequence_exclusive = 1;
    let envelope = Envelope::new(
        id(1),
        1,
        id(5),
        20,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::DeckQ4ProcessSlot(invalid_range),
        }),
    );
    assert!(encode_envelope(&envelope).is_err());
}

#[test]
fn q4_ack_names_correlate_with_command_ids_and_statuses_expose_no_paths() {
    let controls = Q4Controls::default();
    let roles = Q4Roles::default();
    let transport = Q4Transport::default();
    let acks = [
        Ack::DeckQ4Load(status()),
        Ack::DeckQ4ProcessSlot(Q4ProcessSlotAck::Paused {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            stream_generation: 1,
            playhead_a: 0,
            playhead_b: 0,
            playhead_c: 0,
            playhead_d: 0,
            roles,
            transport,
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
            controls,
            requires_causal_reset: false,
        }),
        Ack::DeckQ4RolesSet(Q4RolesSetAck {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            roles,
            requires_causal_reset: false,
        }),
        Ack::DeckQ4TransportSet(Q4TransportSetAck {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            transport,
            requires_causal_reset: false,
        }),
        Ack::DeckQ4SeedSet(Q4SeedSetAck {
            deck_id: "main-q4".to_owned(),
            deck_revision: 1,
            seed: 42,
            requires_causal_reset: false,
        }),
        Ack::DeckQ4Status(status()),
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
    ];
    for (ack, expected) in acks.into_iter().zip(expected) {
        assert_eq!(ack.name(), expected);
    }

    let command_id = id(2);
    let command = Envelope::new(
        id(1),
        1,
        command_id,
        10,
        Message::Command(Command::DeckQ4Load(load())),
    );
    let matching = Envelope::new(
        id(1),
        1,
        id(3),
        20,
        Message::Ack(AckReply {
            reply_to: command_id,
            ack: Ack::DeckQ4Load(status()),
        }),
    );
    let mut validator = SessionValidator::new(id(1), InboundPolicy::ResponsesAndEvents);
    validator.track_outbound_command(&command).unwrap();
    validator.validate_inbound(&matching).unwrap();

    let status_wire = serde_json::to_string(&status()).unwrap();
    assert!(!status_wire.contains("cartridge_path"));
    assert!(!status_wire.contains(".lc"));
}

#[test]
fn q4_typed_error_uses_the_same_command_id_and_closed_name() {
    let command_id = id(2);
    let command = Envelope::new(
        id(1),
        1,
        command_id,
        10,
        Message::Command(Command::DeckQ4Load(load())),
    );
    let error = Envelope::new(
        id(1),
        1,
        id(3),
        20,
        Message::Error(ErrorReply {
            reply_to: command_id,
            name: CommandName::DeckQ4Load,
            error: ErrorPayload {
                code: ErrorCode::DeckSourceIncompatible,
                message: "four Library cartridges are not Q4-compatible".to_owned(),
                retryable: false,
                fatal: false,
                worker_state: WorkerState::Ready,
                diagnostic_id: id(4),
                details: BoundedVec::default(),
            },
        }),
    );

    let encoded = encode_envelope(&error).expect("typed Q4 error");
    assert_eq!(decode_envelope(&encoded).unwrap(), error);
    let mut validator = SessionValidator::new(id(1), InboundPolicy::ResponsesAndEvents);
    validator.track_outbound_command(&command).unwrap();
    validator.validate_inbound(&error).unwrap();
    assert!(!validator.has_pending_reply(command_id));
}

#[derive(Serialize)]
struct RolesWithUnknown {
    carrier: Q4Slot,
    donor_b: Q4Slot,
    donor_c: Q4Slot,
    donor_d: Q4Slot,
    hidden_carrier_override: Q4Slot,
}

#[test]
fn q4_nested_role_schema_rejects_unknown_fields() {
    let encoded = rmp_serde::to_vec_named(&RolesWithUnknown {
        carrier: Q4Slot::A,
        donor_b: Q4Slot::B,
        donor_c: Q4Slot::C,
        donor_d: Q4Slot::D,
        hidden_carrier_override: Q4Slot::B,
    })
    .unwrap();
    assert!(rmp_serde::from_slice::<Q4Roles>(&encoded).is_err());
}

#[test]
fn q4_capture_rejects_parent_order_and_structural_carrier_drift() {
    let mut wrong_order = snapshot_receipt();
    wrong_order.parents.swap(0, 1);
    assert!(encode_envelope(&capture_status_ack(finished_status(wrong_order))).is_err());

    let mut carrier_drift = snapshot_receipt();
    carrier_drift.structural_carrier = Q4Slot::C;
    assert!(encode_envelope(&capture_status_ack(finished_status(carrier_drift))).is_err());
}

#[test]
fn q4_control_enums_are_exact_wire_values() {
    let controls = Q4Controls {
        algorithm: Q4Algorithm::Xs5,
        mode: Q4Mode::Interact,
        xs5_routing: Q4Xs5Routing::Sinkhorn,
        ..Q4Controls::default()
    };
    controls.validate().expect("closed enum controls");
    let value = serde_json::to_value(controls).unwrap();
    assert_eq!(value["algorithm"], "XS5");
    assert_eq!(value["mode"], "INTERACT");
    assert_eq!(value["xs5_routing"], "SINKHORN");
}
