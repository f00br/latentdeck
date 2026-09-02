use latentdeck_control::v2::{
    Ack, AckReply, Capability, CaptureArtifact, CaptureIdentity, CaptureMode, CaptureStart,
    CaptureState, CaptureStatusSnapshot, CodecDescriptor, CodecError, CodecState, Command,
    CommandName, DeckIdentity, DeckLoad, DeckRuntimeBinding, DeckState, DeckTransportSet,
    DecodedAbi, DeviceKind, EmptyPayload, Envelope, ErrorCode, InboundPolicy, LimitedVec, Message,
    PROTOCOL_VERSION, PlayerReset, PlayerState, ProfileKey, ProfileReceipt, RawImportAbort,
    RawImportAborted, RawImportArtifact, RawImportAudioPolicy, RawImportMetadata,
    RawImportPreflight, RawImportPreflightRequest, RawImportStage, RawImportStorageDtype,
    RawImportTensor, RawImportTensorStream, RingConfigure, RingKind, RoleBinding, SessionConfigure,
    SessionState, SessionValidator, SignalGeometry, SourceBinding, SourceOpen,
    SourceTransportBinding, StatusSnapshot, TensorAbi, TensorDtype, ValidationError, WorkerHello,
    WorkerHelloAuthToken, decode_json, decode_messagepack, encode_json, encode_messagepack,
};
use serde::Serialize;
use uuid::Uuid;

fn id(value: u128) -> Uuid {
    Uuid::from_u128(value)
}

fn configure() -> SessionConfigure {
    SessionConfigure {
        selected_protocol_version: PROTOCOL_VERSION,
        app_version: "0.2.0".to_owned(),
        heartbeat_interval_ms: 1_000,
        heartbeat_hard_timeout_ms: 10_000,
        max_frame_bytes: 262_144,
        max_inflight_batches: 4,
        requested_capabilities: LimitedVec::try_from_vec(vec![
            Capability::Player,
            Capability::Realtime,
            Capability::Resample,
            Capability::SnapshotCapture,
            Capability::LiveCapture,
        ])
        .unwrap(),
    }
}

fn envelope(command: Command) -> Envelope {
    Envelope::new(id(1), 1, id(2), 5, Message::Command(command))
}

fn status() -> StatusSnapshot {
    StatusSnapshot {
        session: SessionState::Ready,
        codec: CodecState::Ready,
        player: PlayerState::Ready,
        deck: DeckState::Ready,
        capture: CaptureState::Idle,
        open_session_count: 1,
        foreground_output_session: Some(id(20)),
        output_lease_pinned: false,
    }
}

fn profile_receipt() -> ProfileReceipt {
    ProfileReceipt {
        receipt_id: id(10),
        cartridge_id: id(11),
        archive_sha256: "a".repeat(64),
        payload_sha256: "b".repeat(64),
        pack_id: "org.example.synthetic".to_owned(),
        pack_version: "0.2.0".to_owned(),
        adapter_id: "org.example.synthetic.adapter".to_owned(),
        adapter_version: "0.2.0".to_owned(),
        profile_key: ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "test_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
        signal_geometry: SignalGeometry {
            channels: 4,
            latent_height: 8,
            latent_width: 8,
            decoded_height: 64,
            decoded_width: 64,
            frame_rate_numerator: 24,
            frame_rate_denominator: 1,
            timing_contract: "synthetic_causal".to_owned(),
            timing_contract_version: "0.1.0".to_owned(),
        },
        tensor_abi: TensorAbi {
            python_major: 3,
            python_minor: 13,
            torch_version: "2.13.0+cu130".to_owned(),
            dtype: TensorDtype::Float16,
            shape: [1, 4, 1, 8, 8],
            contiguous: true,
            device: DeviceKind::Cuda,
        },
        decoded_abi: DecodedAbi {
            pixel_format: "rgba8".to_owned(),
            maximum_batch: 24,
        },
        capabilities: LimitedVec::try_from_vec(vec![Capability::Player, Capability::Realtime])
            .unwrap(),
        estimated_host_bytes: 4_096,
        estimated_device_bytes: 8_192,
    }
}

fn raw_import_metadata() -> RawImportMetadata {
    RawImportMetadata {
        profile_key: ProfileKey {
            codec_family: "minimax_h3".to_owned(),
            profile: "h3_av_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
        payload_entry: "payloads/h3.safetensors".to_owned(),
        payload_media_type: "application/vnd.safetensors".to_owned(),
        tensors: LimitedVec::try_from_vec(vec![RawImportTensor {
            stream: RawImportTensorStream::Visual,
            name: "video".to_owned(),
            storage_dtype: RawImportStorageDtype::F16,
            runtime_dtype: RawImportStorageDtype::F16,
            shape: LimitedVec::try_from_vec(vec![1, 24, 7, 8, 8]).unwrap(),
        }])
        .unwrap(),
        timing_contract: "minimax_h3_causal".to_owned(),
        timing_contract_version: "0.1.0".to_owned(),
        decoded_width: 128,
        decoded_height: 128,
        decoded_frame_count: 22,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        duration_numerator: 11,
        duration_denominator: 12,
        audio_policy: RawImportAudioPolicy::SourceAbsent,
    }
}

#[test]
fn worker_hello_round_trips_a_redacted_exact_lowercase_auth_token() {
    let hello = Envelope::new(
        id(1),
        1,
        id(90),
        1,
        Message::Event(latentdeck_control::v2::EventMessage {
            caused_by: None,
            event: latentdeck_control::v2::Event::WorkerHello(WorkerHello {
                auth_token: WorkerHelloAuthToken::new([0xab; 32]),
                worker_pid: 1234,
                worker_identity: "org.latentdeck.codec-host".to_owned(),
                runtime_identity: "cpython-3.13".to_owned(),
                protocol_min: PROTOCOL_VERSION,
                protocol_max: PROTOCOL_VERSION,
            }),
        }),
    );

    let json = encode_json(&hello).expect("encode hello");
    let text = String::from_utf8(json.clone()).expect("hello JSON");
    assert!(text.contains(&"ab".repeat(32)));
    assert!(!format!("{hello:?}").contains(&"ab".repeat(32)));
    assert_eq!(decode_json(&json).expect("decode hello"), hello);
    assert_eq!(
        decode_messagepack(&encode_messagepack(&hello).expect("encode msgpack"))
            .expect("decode msgpack"),
        hello
    );
}

#[test]
fn session_validator_requires_worker_hello_first_and_correlates_ordered_replies() {
    let session_id = id(1);
    let mut validator = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    let hello = Envelope::new(
        session_id,
        1,
        id(91),
        1,
        Message::Event(latentdeck_control::v2::EventMessage {
            caused_by: None,
            event: latentdeck_control::v2::Event::WorkerHello(WorkerHello {
                auth_token: WorkerHelloAuthToken::new([0x5a; 32]),
                worker_pid: 1234,
                worker_identity: "org.latentdeck.codec-host".to_owned(),
                runtime_identity: "cpython-3.13".to_owned(),
                protocol_min: PROTOCOL_VERSION,
                protocol_max: PROTOCOL_VERSION,
            }),
        }),
    );
    validator
        .validate_inbound(&hello)
        .expect("strict first hello");

    let command = envelope(Command::SessionConfigure(configure()));
    validator
        .track_outbound_command(&command)
        .expect("track first command");
    let reply = Envelope::new(
        session_id,
        2,
        id(92),
        2,
        Message::Ack(AckReply {
            reply_to: command.message_id,
            ack: Ack::SessionConfigure(latentdeck_control::v2::SessionConfigured {
                selected_protocol_version: PROTOCOL_VERSION,
                maximum_frame_bytes: 262_144,
                accepted_capabilities: LimitedVec::try_from_vec(vec![Capability::Player]).unwrap(),
            }),
            status: status(),
        }),
    );
    validator
        .validate_inbound(&reply)
        .expect("ordered correlated reply");
    assert!(!validator.has_pending_reply(command.message_id));
}

#[test]
fn json_and_named_messagepack_round_trip_the_same_closed_command() {
    let expected = envelope(Command::SessionConfigure(configure()));
    assert_eq!(
        decode_json(&encode_json(&expected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
        expected
    );
}

#[test]
fn dynamic_deck_runtime_binding_round_trips_with_exact_identity_and_hashes() {
    let python_root = std::env::current_dir()
        .unwrap()
        .join("synthetic-deck-python")
        .to_string_lossy()
        .into_owned();
    let expected = envelope(Command::DeckLoad(Box::new(DeckLoad {
        deck_session_id: id(30),
        deck_id: "com.example.deck".to_owned(),
        deck_version: "0.2.0".to_owned(),
        operator_id: "com.example.operator".to_owned(),
        operator_version: "0.2.0".to_owned(),
        runtime: Some(DeckRuntimeBinding {
            deck_id: "com.example.deck".to_owned(),
            deck_version: "0.2.0".to_owned(),
            operator_id: "com.example.operator".to_owned(),
            operator_version: "0.2.0".to_owned(),
            python_root,
            entrypoint: "external_deck.operator:process_sources".to_owned(),
            package_manifest_sha256: "a".repeat(64),
            integrity_catalog_sha256: "b".repeat(64),
        }),
        sources: LimitedVec::try_from_vec(vec![SourceBinding {
            physical_slot: 1,
            source_id: id(31),
            cartridge_id: id(32),
            archive_sha256: "c".repeat(64),
            profile_receipt_id: id(33),
            loop_enabled: true,
        }])
        .unwrap(),
        roles: LimitedVec::try_from_vec(vec![RoleBinding {
            role: "carrier".to_owned(),
            physical_slot: 1,
        }])
        .unwrap(),
        controls: LimitedVec::default(),
        seed: 7,
        stream_generation: 1,
    })));

    assert_eq!(
        decode_json(&encode_json(&expected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
        expected
    );

    let mut mismatched = serde_json::to_value(&expected).unwrap();
    mismatched["message"]["body"]["payload"]["runtime"]["operator_version"] =
        serde_json::json!("0.3.0");
    assert!(matches!(
        decode_json(&serde_json::to_vec(&mismatched).unwrap()),
        Err(CodecError::Validation(ValidationError::InvalidField(
            "deck.runtime.identity"
        )))
    ));
}

#[test]
fn deck_transport_round_trips_independent_physical_slot_play_and_loop_state() {
    let expected = envelope(Command::DeckTransportSet(DeckTransportSet {
        deck_session_id: id(30),
        deck_revision: 1,
        sources: LimitedVec::try_from_vec(vec![
            SourceTransportBinding {
                physical_slot: 1,
                playing: false,
                loop_enabled: true,
            },
            SourceTransportBinding {
                physical_slot: 2,
                playing: true,
                loop_enabled: false,
            },
        ])
        .unwrap(),
    }));

    assert_eq!(
        decode_json(&encode_json(&expected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
        expected
    );

    let mut duplicate = serde_json::to_value(&expected).unwrap();
    duplicate["message"]["body"]["payload"]["sources"][1]["physical_slot"] = serde_json::json!(1);
    assert!(matches!(
        decode_json(&serde_json::to_vec(&duplicate).unwrap()),
        Err(CodecError::Validation(ValidationError::DuplicateValue(
            "deck.transport.sources"
        )))
    ));
}

#[test]
fn deck_reset_round_trips_explicit_playhead_preservation_policy() {
    let expected = envelope(Command::DeckReset(latentdeck_control::v2::DeckReset {
        deck_session_id: id(30),
        deck_revision: 1,
        new_stream_generation: 2,
        preserve_playheads: true,
    }));

    assert_eq!(
        decode_json(&encode_json(&expected).unwrap()).unwrap(),
        expected
    );
    assert_eq!(
        decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
        expected
    );

    let mut missing_policy = serde_json::to_value(&expected).unwrap();
    missing_policy["message"]["body"]["payload"]
        .as_object_mut()
        .unwrap()
        .remove("preserve_playheads");
    assert!(matches!(
        decode_json(&serde_json::to_vec(&missing_policy).unwrap()),
        Err(CodecError::Json(_))
    ));
}

#[test]
fn capture_round_trips_host_owned_staging_and_decoded_timing_evidence() {
    let staging_root = std::env::temp_dir()
        .join("latentdeck-capture-staging")
        .to_string_lossy()
        .into_owned();
    let staged_payload_path = std::path::Path::new(&staging_root)
        .join("capture.safetensors")
        .to_string_lossy()
        .into_owned();
    let command = envelope(Command::CaptureStart(CaptureStart {
        deck_session_id: id(30),
        deck_revision: 1,
        capture_id: id(40),
        mode: CaptureMode::Snapshot,
        staging_root,
        maximum_latent_slots: 128,
        maximum_visual_bytes: 64 * 1024 * 1024,
        maximum_reset_events: 32,
    }));

    assert_eq!(
        decode_json(&encode_json(&command).unwrap()).unwrap(),
        command
    );
    assert_eq!(
        decode_messagepack(&encode_messagepack(&command).unwrap()).unwrap(),
        command
    );

    let reply = Envelope::new(
        id(1),
        2,
        id(41),
        10,
        Message::Ack(AckReply {
            reply_to: command.message_id,
            ack: Ack::CaptureStatus(Box::new(CaptureStatusSnapshot {
                deck_session_id: id(30),
                deck_revision: 1,
                capture_id: id(40),
                state: CaptureState::Completed,
                mode: CaptureMode::Snapshot,
                latent_slots: 2,
                reset_events: 0,
                artifact: Some(CaptureArtifact {
                    staged_payload_path,
                    payload_sha256: "a".repeat(64),
                    payload_byte_length: 4_096,
                    latent_slots: 2,
                    decoded_frame_count: 5,
                }),
            })),
            status: status(),
        }),
    );
    assert_eq!(decode_json(&encode_json(&reply).unwrap()).unwrap(), reply);
    assert_eq!(
        decode_messagepack(&encode_messagepack(&reply).unwrap()).unwrap(),
        reply
    );
}

#[test]
fn capture_rejects_relative_staging_and_unknown_staging_aliases() {
    let invalid = envelope(Command::CaptureStart(CaptureStart {
        deck_session_id: id(30),
        deck_revision: 1,
        capture_id: id(40),
        mode: CaptureMode::LiveCapture,
        staging_root: "relative/capture".to_owned(),
        maximum_latent_slots: 128,
        maximum_visual_bytes: 64 * 1024 * 1024,
        maximum_reset_events: 32,
    }));
    assert_eq!(
        invalid.validate(),
        Err(ValidationError::InvalidField("capture.staging_root"))
    );

    let mut unknown = serde_json::to_value(envelope(Command::CaptureStart(CaptureStart {
        deck_session_id: id(30),
        deck_revision: 1,
        capture_id: id(40),
        mode: CaptureMode::LiveCapture,
        staging_root: std::env::temp_dir()
            .join("latentdeck-capture-staging")
            .to_string_lossy()
            .into_owned(),
        maximum_latent_slots: 128,
        maximum_visual_bytes: 64 * 1024 * 1024,
        maximum_reset_events: 32,
    })))
    .unwrap();
    unknown["message"]["body"]["payload"]["worker_path"] = serde_json::json!("trusted");
    assert!(matches!(
        decode_json(&serde_json::to_vec(&unknown).unwrap()),
        Err(CodecError::Json(_))
    ));
}

#[test]
fn per_capture_status_requires_artifact_exactly_when_completed() {
    let capture_id = id(40);
    let status_without_artifact = |state| CaptureStatusSnapshot {
        deck_session_id: id(30),
        deck_revision: 1,
        capture_id,
        state,
        mode: CaptureMode::Snapshot,
        latent_slots: 2,
        reset_events: 0,
        artifact: None,
    };
    let envelope_for = |snapshot| {
        Envelope::new(
            id(1),
            2,
            id(41),
            10,
            Message::Ack(AckReply {
                reply_to: id(2),
                ack: Ack::CaptureStatus(Box::new(snapshot)),
                status: status(),
            }),
        )
    };

    assert_eq!(
        envelope_for(status_without_artifact(CaptureState::Completed)).validate(),
        Err(ValidationError::InvalidField("capture.artifact"))
    );
    assert_eq!(
        envelope_for(status_without_artifact(CaptureState::Idle)).validate(),
        Err(ValidationError::InvalidField("capture.state"))
    );

    let mut finalizing_with_artifact = status_without_artifact(CaptureState::Finalizing);
    finalizing_with_artifact.artifact = Some(CaptureArtifact {
        staged_payload_path: std::env::temp_dir()
            .join("capture.safetensors")
            .to_string_lossy()
            .into_owned(),
        payload_sha256: "a".repeat(64),
        payload_byte_length: 4_096,
        latent_slots: 2,
        decoded_frame_count: 5,
    });
    assert_eq!(
        envelope_for(finalizing_with_artifact).validate(),
        Err(ValidationError::InvalidField("capture.artifact"))
    );
}

#[derive(Serialize)]
struct EnvelopeWithUnknown<'a> {
    protocol: &'static str,
    protocol_version: u16,
    session_id: Uuid,
    sequence: u64,
    message_id: Uuid,
    sender_uptime_ns: u64,
    message: &'a Message,
    hidden_fallback: bool,
}

#[test]
fn unknown_json_and_messagepack_fields_are_rejected() {
    let message = Message::Command(Command::SessionConfigure(configure()));
    let value = EnvelopeWithUnknown {
        protocol: "latentdeck.worker",
        protocol_version: PROTOCOL_VERSION,
        session_id: id(1),
        sequence: 1,
        message_id: id(2),
        sender_uptime_ns: 5,
        message: &message,
        hidden_fallback: true,
    };
    let json = serde_json::to_vec(&value).unwrap();
    assert!(matches!(decode_json(&json), Err(CodecError::Json(_))));
    let messagepack = rmp_serde::to_vec_named(&value).unwrap();
    assert!(matches!(
        decode_messagepack(&messagepack),
        Err(CodecError::MessagePack(_))
    ));
}

#[test]
fn collection_bounds_are_enforced_during_deserialization() {
    let mut value = serde_json::to_value(envelope(Command::SessionConfigure(configure()))).unwrap();
    value["message"]["body"]["payload"]["requested_capabilities"] =
        serde_json::to_value(vec!["player"; 17]).unwrap();
    assert!(matches!(
        decode_json(&serde_json::to_vec(&value).unwrap()),
        Err(CodecError::Json(_))
    ));
}

#[test]
fn each_command_domain_has_a_closed_typed_name() {
    let commands = [
        Command::SessionStatus(EmptyPayload {}),
        Command::CodecDescriptor(latentdeck_control::v2::CodecDescriptorRequest {
            pack_id: "org.example.codec".to_owned(),
            pack_version: "0.2.0".to_owned(),
            adapter_id: "org.example.codec.adapter".to_owned(),
        }),
        Command::ProfileInspect(latentdeck_control::v2::ProfileInspect {
            source_id: id(3),
            cartridge_id: id(4),
            archive_sha256: "a".repeat(64),
        }),
        Command::PlayerReset(PlayerReset {
            player_session_id: id(5),
            new_stream_generation: 2,
        }),
        Command::DeckRestart(DeckIdentity {
            deck_session_id: id(6),
            deck_revision: 1,
        }),
        Command::CaptureStatus(CaptureIdentity {
            deck_session_id: id(6),
            deck_revision: 1,
            capture_id: id(7),
        }),
    ];
    let expected = [
        CommandName::SessionStatus,
        CommandName::CodecDescriptor,
        CommandName::ProfileInspect,
        CommandName::PlayerReset,
        CommandName::DeckRestart,
        CommandName::CaptureStatus,
    ];
    for (command, expected_name) in commands.into_iter().zip(expected) {
        assert_eq!(command.name(), expected_name);
        envelope(command).validate().unwrap();
    }
}

#[test]
fn retained_source_ring_and_metrics_lifecycle_are_explicit_non_byte_commands() {
    let commands = [
        Command::SourceOpen(SourceOpen {
            source_id: id(30),
            cartridge_id: id(31),
            archive_sha256: "a".repeat(64),
            archive_bytes: 4_096,
            retained_native_handle: 123,
            integrity_access_receipt: "{\"access_abi_version\":1}".to_owned(),
        }),
        Command::RingConfigure(RingConfigure {
            ring_id: id(32),
            kind: RingKind::DecodedRgba,
            mapping_handle: 124,
            ready_event_handle: 125,
            consumed_event_handle: 126,
            slot_count: 4,
            slot_bytes: 64 * 64 * 4,
        }),
        Command::MetricsGet(EmptyPayload {}),
    ];
    let expected = [
        CommandName::SourceOpen,
        CommandName::RingConfigure,
        CommandName::MetricsGet,
    ];
    for (command, name) in commands.into_iter().zip(expected) {
        assert_eq!(command.name(), name);
        let encoded = encode_json(&envelope(command)).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(!text.contains("tensor_bytes"));
        assert!(!text.contains("rgba_bytes"));
        assert!(!text.contains("pixels"));
    }
}

#[test]
fn typed_ack_payloads_round_trip_descriptor_and_profile_receipt() {
    let descriptor = CodecDescriptor {
        pack_id: "org.example.synthetic".to_owned(),
        pack_version: "0.2.0".to_owned(),
        adapter_id: "org.example.synthetic.adapter".to_owned(),
        adapter_version: "0.2.0".to_owned(),
        host_api_version: "2.0".to_owned(),
        capabilities: LimitedVec::try_from_vec(Capability::REQUIRED_CODEC_V2.to_vec()).unwrap(),
        profiles: LimitedVec::try_from_vec(vec![profile_receipt().profile_key]).unwrap(),
    };
    let descriptor_reply = Envelope::new(
        id(1),
        2,
        id(40),
        10,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::CodecDescriptor(descriptor),
            status: status(),
        }),
    );
    let profile_reply = Envelope::new(
        id(1),
        3,
        id(41),
        11,
        Message::Ack(AckReply {
            reply_to: id(3),
            ack: Ack::ProfileValidate(Box::new(profile_receipt())),
            status: status(),
        }),
    );

    for expected in [descriptor_reply, profile_reply] {
        assert_eq!(
            decode_json(&encode_json(&expected).unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn duplicate_capabilities_and_non_finite_controls_fail_before_encoding() {
    let duplicate = envelope(Command::SessionConfigure(SessionConfigure {
        requested_capabilities: LimitedVec::try_from_vec(vec![
            Capability::Player,
            Capability::Player,
        ])
        .unwrap(),
        ..configure()
    }));
    assert_eq!(
        duplicate.validate(),
        Err(ValidationError::DuplicateValue("capabilities"))
    );

    let invalid = envelope(Command::DeckControlsSet(
        latentdeck_control::v2::DeckControlsSet {
            deck_session_id: id(6),
            deck_revision: 1,
            controls: LimitedVec::try_from_vec(vec![latentdeck_control::v2::ControlBinding {
                name: "mix".to_owned(),
                value: latentdeck_control::v2::ControlValue::Number(f64::NAN),
            }])
            .unwrap(),
        },
    ));
    assert_eq!(
        invalid.validate().unwrap_err().stable_code(),
        ErrorCode::ProtocolInvalidMessage
    );
}

#[test]
fn protocol_2_never_accepts_protocol_1_or_a_trailing_object() {
    let mut wrong = envelope(Command::SessionConfigure(configure()));
    wrong.protocol_version = 1;
    assert_eq!(
        wrong.validate(),
        Err(ValidationError::UnsupportedVersion(1))
    );

    let mut encoded =
        encode_messagepack(&envelope(Command::SessionConfigure(configure()))).unwrap();
    encoded.push(0xc0);
    assert!(matches!(
        decode_messagepack(&encoded),
        Err(CodecError::TrailingData)
    ));
}

#[test]
fn profile_receipt_binds_exact_codec_cartridge_signal_and_tensor_identities() {
    let receipt = profile_receipt();
    receipt.validate().unwrap();

    let mut missing_capability = receipt;
    missing_capability.capabilities = LimitedVec::default();
    assert_eq!(
        missing_capability.validate(),
        Err(ValidationError::InvalidField(
            "profile_receipt.capabilities"
        ))
    );
}

#[test]
fn raw_import_commands_and_acks_are_closed_typed_and_path_bounded() {
    let source_path = std::env::current_dir()
        .unwrap()
        .join("raw-input.safetensors")
        .to_string_lossy()
        .into_owned();
    let staging_root = std::env::current_dir()
        .unwrap()
        .join("raw-import-staging")
        .to_string_lossy()
        .into_owned();
    let staged_payload_path = std::path::Path::new(&staging_root)
        .join("staged.safetensors")
        .to_string_lossy()
        .into_owned();
    let import_id = id(70);
    let receipt_id = id(71);
    let preflight_command = Command::RawImportPreflight(RawImportPreflightRequest {
        import_id,
        source_path,
        maximum_source_bytes: 64 * 1024 * 1024,
    });
    let stage_command = Command::RawImportStage(RawImportStage {
        import_id,
        receipt_id,
        staging_root,
    });
    let abort_command = Command::RawImportAbort(RawImportAbort {
        import_id,
        receipt_id,
    });
    for (command, expected_name) in [
        (preflight_command, CommandName::RawImportPreflight),
        (stage_command, CommandName::RawImportStage),
        (abort_command, CommandName::RawImportAbort),
    ] {
        assert_eq!(command.name(), expected_name);
        let expected = envelope(command);
        assert_eq!(
            decode_json(&encode_json(&expected).unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
            expected
        );
    }

    let preflight = RawImportPreflight {
        receipt_id,
        import_id,
        pack_id: "org.latentdeck.h3".to_owned(),
        pack_version: "0.2.0".to_owned(),
        adapter_id: "org.latentdeck.h3".to_owned(),
        adapter_version: "0.2.0".to_owned(),
        source_sha256: "a".repeat(64),
        source_byte_length: 4_096,
        metadata: raw_import_metadata(),
    };
    let artifact = RawImportArtifact {
        receipt_id,
        import_id,
        staged_payload_path,
        payload_sha256: "b".repeat(64),
        payload_byte_length: 4_096,
    };
    let acks = [
        Ack::RawImportPreflight(Box::new(preflight)),
        Ack::RawImportStage(artifact),
        Ack::RawImportAbort(RawImportAborted {
            import_id,
            receipt_id,
        }),
    ];
    for (index, ack) in acks.into_iter().enumerate() {
        let expected = Envelope::new(
            id(1),
            u64::try_from(index + 2).unwrap(),
            id(80 + index as u128),
            10,
            Message::Ack(AckReply {
                reply_to: id(2 + index as u128),
                ack,
                status: status(),
            }),
        );
        assert_eq!(
            decode_json(&encode_json(&expected).unwrap()).unwrap(),
            expected
        );
        assert_eq!(
            decode_messagepack(&encode_messagepack(&expected).unwrap()).unwrap(),
            expected
        );
    }
}

#[test]
fn raw_import_rejects_relative_or_escaping_paths_and_duplicate_tensor_names() {
    let invalid_source = envelope(Command::RawImportPreflight(RawImportPreflightRequest {
        import_id: id(70),
        source_path: "relative.safetensors".to_owned(),
        maximum_source_bytes: 1,
    }));
    assert_eq!(
        invalid_source.validate(),
        Err(ValidationError::InvalidField("raw_import.source_path"))
    );

    let mut metadata = raw_import_metadata();
    metadata.payload_entry = "../outside.safetensors".to_owned();
    let invalid_preflight = RawImportPreflight {
        receipt_id: id(71),
        import_id: id(70),
        pack_id: "org.latentdeck.h3".to_owned(),
        pack_version: "0.2.0".to_owned(),
        adapter_id: "org.latentdeck.h3".to_owned(),
        adapter_version: "0.2.0".to_owned(),
        source_sha256: "a".repeat(64),
        source_byte_length: 4_096,
        metadata,
    };
    let reply = Envelope::new(
        id(1),
        2,
        id(72),
        10,
        Message::Ack(AckReply {
            reply_to: id(2),
            ack: Ack::RawImportPreflight(Box::new(invalid_preflight)),
            status: status(),
        }),
    );
    assert_eq!(
        reply.validate(),
        Err(ValidationError::InvalidField("raw_import.payload_entry"))
    );

    let mut metadata = raw_import_metadata();
    let duplicate = metadata.tensors.as_slice()[0].clone();
    metadata.tensors = LimitedVec::try_from_vec(vec![duplicate.clone(), duplicate]).unwrap();
    assert_eq!(
        metadata.validate(),
        Err(ValidationError::DuplicateValue("raw_import.tensors"))
    );
}
