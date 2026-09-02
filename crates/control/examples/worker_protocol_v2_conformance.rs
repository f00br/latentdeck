//! Runtime-generated Rust/Python Protocol 2 conformance fixture.

use std::{
    env,
    error::Error,
    io::{self, Read, Write},
};

use latentdeck_control::v2::{
    Ack, AckReply, Capability, CaptureState, CodecDescriptor, CodecState, Command, CommandName,
    DeckState, Envelope, ErrorCode, ErrorDetail, ErrorPayload, ErrorReply, LimitedVec, Message,
    PROTOCOL_VERSION, PlayerState, ProfileKey, RawImportAudioPolicy, RawImportMetadata,
    RawImportPreflight, RawImportPreflightRequest, RawImportStorageDtype, RawImportTensor,
    RawImportTensorStream, SessionConfigure, SessionState, StatusSnapshot, decode_json,
    decode_messagepack, encode_json, encode_messagepack,
};
use uuid::Uuid;

fn fixture() -> Envelope {
    Envelope::new(
        Uuid::parse_str("9ca8c228-04c7-4b59-909f-6fbef591a43e").unwrap(),
        1,
        Uuid::parse_str("10000000-0000-4000-8000-000000000002").unwrap(),
        123_456,
        Message::Command(Command::SessionConfigure(SessionConfigure {
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
        })),
    )
}

fn error_fixture() -> Envelope {
    Envelope::new(
        Uuid::parse_str("9ca8c228-04c7-4b59-909f-6fbef591a43e").unwrap(),
        2,
        Uuid::parse_str("10000000-0000-4000-8000-000000000003").unwrap(),
        223_456,
        Message::Error(ErrorReply {
            reply_to: Uuid::parse_str("10000000-0000-4000-8000-000000000002").unwrap(),
            name: CommandName::CaptureStart,
            error: ErrorPayload {
                code: ErrorCode::SessionOutputLeasePinned,
                message: "foreground output is pinned by capture".to_owned(),
                retryable: true,
                fatal: false,
                status: StatusSnapshot {
                    session: SessionState::Busy,
                    codec: CodecState::Ready,
                    player: PlayerState::Paused,
                    deck: DeckState::Capturing,
                    capture: CaptureState::Finalizing,
                    open_session_count: 4,
                    foreground_output_session: Some(
                        Uuid::parse_str("10000000-0000-4000-8000-000000000004").unwrap(),
                    ),
                    output_lease_pinned: true,
                },
                diagnostic_id: Uuid::parse_str("10000000-0000-4000-8000-000000000005").unwrap(),
                details: LimitedVec::try_from_vec(vec![ErrorDetail {
                    key: "capture_id".to_owned(),
                    value: "10000000-0000-4000-8000-000000000006".to_owned(),
                }])
                .unwrap(),
            },
        }),
    )
}

fn ack_fixture() -> Envelope {
    Envelope::new(
        Uuid::parse_str("9ca8c228-04c7-4b59-909f-6fbef591a43e").unwrap(),
        3,
        Uuid::parse_str("10000000-0000-4000-8000-000000000007").unwrap(),
        323_456,
        Message::Ack(AckReply {
            reply_to: Uuid::parse_str("10000000-0000-4000-8000-000000000002").unwrap(),
            ack: Ack::CodecDescriptor(CodecDescriptor {
                pack_id: "org.example.synthetic".to_owned(),
                pack_version: "0.2.0".to_owned(),
                adapter_id: "org.example.synthetic.adapter".to_owned(),
                adapter_version: "0.2.0".to_owned(),
                host_api_version: "2.0".to_owned(),
                capabilities: LimitedVec::try_from_vec(vec![
                    Capability::Player,
                    Capability::Realtime,
                    Capability::Resample,
                    Capability::SnapshotCapture,
                    Capability::LiveCapture,
                ])
                .unwrap(),
                profiles: LimitedVec::try_from_vec(vec![ProfileKey {
                    codec_family: "synthetic".to_owned(),
                    profile: "test_latent".to_owned(),
                    profile_version: "0.1.0".to_owned(),
                }])
                .unwrap(),
            }),
            status: StatusSnapshot {
                session: SessionState::Ready,
                codec: CodecState::Ready,
                player: PlayerState::Empty,
                deck: DeckState::Empty,
                capture: CaptureState::Idle,
                open_session_count: 0,
                foreground_output_session: None,
                output_lease_pinned: false,
            },
        }),
    )
}

fn raw_import_metadata() -> RawImportMetadata {
    RawImportMetadata {
        profile_key: ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "test_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
        payload_entry: "payloads/synthetic.safetensors".to_owned(),
        payload_media_type: "application/vnd.safetensors".to_owned(),
        tensors: LimitedVec::try_from_vec(vec![RawImportTensor {
            stream: RawImportTensorStream::Visual,
            name: "video".to_owned(),
            storage_dtype: RawImportStorageDtype::F16,
            runtime_dtype: RawImportStorageDtype::F16,
            shape: LimitedVec::try_from_vec(vec![1, 4, 2, 1, 1]).unwrap(),
        }])
        .unwrap(),
        timing_contract: "synthetic_ticks".to_owned(),
        timing_contract_version: "0.1.0".to_owned(),
        decoded_width: 8,
        decoded_height: 8,
        decoded_frame_count: 2,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        duration_numerator: 1,
        duration_denominator: 12,
        audio_policy: RawImportAudioPolicy::SourceAbsent,
    }
}

fn raw_import_fixture() -> Envelope {
    Envelope::new(
        Uuid::parse_str("9ca8c228-04c7-4b59-909f-6fbef591a43e").unwrap(),
        4,
        Uuid::parse_str("10000000-0000-4000-8000-000000000008").unwrap(),
        423_456,
        Message::Command(Command::RawImportPreflight(RawImportPreflightRequest {
            import_id: Uuid::parse_str("10000000-0000-4000-8000-000000000009").unwrap(),
            source_path: r"C:\latentdeck-conformance\raw.safetensors".to_owned(),
            maximum_source_bytes: 1_024,
        })),
    )
}

fn raw_import_ack_fixture() -> Envelope {
    Envelope::new(
        Uuid::parse_str("9ca8c228-04c7-4b59-909f-6fbef591a43e").unwrap(),
        5,
        Uuid::parse_str("10000000-0000-4000-8000-00000000000a").unwrap(),
        523_456,
        Message::Ack(AckReply {
            reply_to: Uuid::parse_str("10000000-0000-4000-8000-000000000008").unwrap(),
            ack: Ack::RawImportPreflight(Box::new(RawImportPreflight {
                receipt_id: Uuid::parse_str("10000000-0000-4000-8000-00000000000b").unwrap(),
                import_id: Uuid::parse_str("10000000-0000-4000-8000-000000000009").unwrap(),
                pack_id: "org.example.synthetic".to_owned(),
                pack_version: "0.2.0".to_owned(),
                adapter_id: "org.example.synthetic.adapter".to_owned(),
                adapter_version: "0.2.0".to_owned(),
                source_sha256: "11".repeat(32),
                source_byte_length: 512,
                metadata: raw_import_metadata(),
            })),
            status: StatusSnapshot {
                session: SessionState::Ready,
                codec: CodecState::Unloaded,
                player: PlayerState::Empty,
                deck: DeckState::Empty,
                capture: CaptureState::Idle,
                open_session_count: 0,
                foreground_output_session: None,
                output_lease_pinned: false,
            },
        }),
    )
}

fn read_stdin() -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    io::stdin().lock().read_to_end(&mut payload)?;
    Ok(payload)
}

fn command_names() -> [CommandName; 32] {
    [
        CommandName::SessionConfigure,
        CommandName::SessionStatus,
        CommandName::SessionShutdown,
        CommandName::CodecDescriptor,
        CommandName::CodecLoad,
        CommandName::CodecUnload,
        CommandName::SourceOpen,
        CommandName::SourceClose,
        CommandName::RingConfigure,
        CommandName::RingRelease,
        CommandName::ProfileInspect,
        CommandName::ProfileValidate,
        CommandName::RawImportPreflight,
        CommandName::RawImportStage,
        CommandName::RawImportAbort,
        CommandName::PlayerOpen,
        CommandName::PlayerStep,
        CommandName::PlayerReset,
        CommandName::PlayerStatus,
        CommandName::DeckLoad,
        CommandName::DeckProcess,
        CommandName::DeckControlsSet,
        CommandName::DeckRolesSet,
        CommandName::DeckTransportSet,
        CommandName::DeckSeedSet,
        CommandName::DeckReset,
        CommandName::DeckRestart,
        CommandName::DeckStatus,
        CommandName::CaptureStart,
        CommandName::CaptureStop,
        CommandName::CaptureStatus,
        CommandName::MetricsGet,
    ]
}

fn main() -> Result<(), Box<dyn Error>> {
    let output = match env::args().nth(1).as_deref() {
        Some("emit-command-names-json") => serde_json::to_vec(&command_names())?,
        Some("emit-json") => encode_json(&fixture())?,
        Some("emit-msgpack") => encode_messagepack(&fixture())?,
        Some("emit-error-json") => encode_json(&error_fixture())?,
        Some("emit-error-msgpack") => encode_messagepack(&error_fixture())?,
        Some("emit-ack-json") => encode_json(&ack_fixture())?,
        Some("emit-ack-msgpack") => encode_messagepack(&ack_fixture())?,
        Some("emit-raw-import-json") => encode_json(&raw_import_fixture())?,
        Some("emit-raw-import-msgpack") => encode_messagepack(&raw_import_fixture())?,
        Some("emit-raw-import-ack-json") => encode_json(&raw_import_ack_fixture())?,
        Some("emit-raw-import-ack-msgpack") => encode_messagepack(&raw_import_ack_fixture())?,
        Some("validate-json") => {
            let actual = decode_json(&read_stdin()?)?;
            assert_eq!(actual, fixture());
            encode_json(&actual)?
        }
        Some("validate-msgpack") => {
            let actual = decode_messagepack(&read_stdin()?)?;
            assert_eq!(actual, fixture());
            encode_json(&actual)?
        }
        Some("validate-error-json") => {
            let actual = decode_json(&read_stdin()?)?;
            assert_eq!(actual, error_fixture());
            encode_json(&actual)?
        }
        Some("validate-error-msgpack") => {
            let actual = decode_messagepack(&read_stdin()?)?;
            assert_eq!(actual, error_fixture());
            encode_json(&actual)?
        }
        Some("validate-ack-json") => {
            let actual = decode_json(&read_stdin()?)?;
            assert_eq!(actual, ack_fixture());
            encode_json(&actual)?
        }
        Some("validate-ack-msgpack") => {
            let actual = decode_messagepack(&read_stdin()?)?;
            assert_eq!(actual, ack_fixture());
            encode_json(&actual)?
        }
        Some("validate-raw-import-json") => {
            let actual = decode_json(&read_stdin()?)?;
            assert_eq!(actual, raw_import_fixture());
            encode_json(&actual)?
        }
        Some("validate-raw-import-msgpack") => {
            let actual = decode_messagepack(&read_stdin()?)?;
            assert_eq!(actual, raw_import_fixture());
            encode_json(&actual)?
        }
        Some("validate-raw-import-ack-json") => {
            let actual = decode_json(&read_stdin()?)?;
            assert_eq!(actual, raw_import_ack_fixture());
            encode_json(&actual)?
        }
        Some("validate-raw-import-ack-msgpack") => {
            let actual = decode_messagepack(&read_stdin()?)?;
            assert_eq!(actual, raw_import_ack_fixture());
            encode_json(&actual)?
        }
        _ => return Err(io::Error::other("unknown Protocol 2 conformance mode").into()),
    };
    io::stdout().lock().write_all(&output)?;
    Ok(())
}
