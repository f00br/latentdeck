use std::io::Cursor;

use latentdeck_control::{
    Ack, AckReply, AuthToken, BoundedVec, CodecInspection, CodecState, Command, EmptyPayload,
    Envelope, ErrorCode, ErrorPayload, ErrorReply, Event, EventMessage, FramingError,
    InboundPolicy, MAX_CONTROL_FRAME_BYTES, MAX_MESSAGES_PER_SESSION, Message, ProtocolMarker,
    RingBind, RingBound, RingState, SessionConfigure, SessionConfigured, SessionValidator,
    SlotState, ValidationError, WORKER_PROTOCOL_VERSION, WireUuid, WorkerHeartbeat, WorkerHello,
    WorkerState, decode_envelope, read_envelope, write_envelope,
};
use serde::Serialize;
use uuid::Uuid;

fn id(value: u128) -> WireUuid {
    WireUuid::from_uuid(Uuid::from_u128(value))
}

fn configure() -> SessionConfigure {
    SessionConfigure {
        selected_protocol_version: 1,
        app_version: "0.1.0".to_owned(),
        heartbeat_interval_ms: 1_000,
        heartbeat_hard_timeout_ms: 10_000,
        max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
        max_inflight_decode_batches: 1,
    }
}

fn configure_ack() -> SessionConfigured {
    SessionConfigured {
        selected_protocol_version: 1,
        heartbeat_interval_ms: 1_000,
        heartbeat_hard_timeout_ms: 10_000,
        max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
        max_inflight_decode_batches: 1,
    }
}

fn command_envelope(
    session_id: WireUuid,
    sequence: u64,
    message_id: WireUuid,
    command: Command,
) -> Envelope {
    Envelope::new(
        session_id,
        sequence,
        message_id,
        123,
        Message::Command(command),
    )
}

fn ack_envelope(
    session_id: WireUuid,
    sequence: u64,
    message_id: WireUuid,
    reply_to: WireUuid,
    ack: Ack,
) -> Envelope {
    Envelope::new(
        session_id,
        sequence,
        message_id,
        456,
        Message::Ack(AckReply { reply_to, ack }),
    )
}

#[test]
fn framed_round_trip_preserves_a_typed_command() {
    let envelope = command_envelope(id(1), 1, id(2), Command::SessionConfigure(configure()));
    let mut bytes = Vec::new();
    write_envelope(&mut bytes, &envelope).expect("valid envelope must encode");

    let mut reader = Cursor::new(bytes);
    let decoded = read_envelope(&mut reader)
        .expect("valid frame must decode")
        .expect("one frame was written");
    assert_eq!(decoded, envelope);
    assert!(
        read_envelope(&mut reader)
            .expect("clean EOF is not an error")
            .is_none()
    );
}

#[test]
fn worker_hello_uses_a_fixed_redacted_auth_token() {
    let token = AuthToken::new([7; 32]);
    assert_eq!(format!("{token:?}"), "AuthToken([REDACTED])");
    assert!(token.constant_time_eq(&AuthToken::new([7; 32])));
    assert!(!token.constant_time_eq(&AuthToken::new([8; 32])));

    let hello = Envelope::new(
        id(1),
        1,
        id(2),
        0,
        Message::Event(EventMessage {
            caused_by: None,
            event: Event::WorkerHello(WorkerHello {
                auth_token: token,
                worker_version: "0.1.0".to_owned(),
                protocol_min: 1,
                protocol_max: 1,
                pid: 42,
                os: "windows".to_owned(),
                arch: "x86_64".to_owned(),
                python_version: "3.13".to_owned(),
                available_adapters: BoundedVec::try_from_vec(vec!["minimax_h3".to_owned()])
                    .unwrap(),
            }),
        }),
    );

    let encoded = latentdeck_control::encode_envelope(&hello).unwrap();
    assert_eq!(decode_envelope(&encoded).unwrap(), hello);
}

#[test]
fn bounded_arrays_reject_an_oversized_declared_sequence() {
    let values = vec!["adapter".to_owned(); 17];
    let encoded = rmp_serde::to_vec_named(&values).unwrap();
    let result = rmp_serde::from_slice::<BoundedVec<String, 16>>(&encoded);
    assert!(result.is_err());
}

#[test]
fn length_prefix_is_bounded_before_payload_allocation() {
    let mut zero = Cursor::new(0_u32.to_le_bytes());
    assert!(matches!(
        read_envelope(&mut zero),
        Err(FramingError::InvalidLength { actual: 0, .. })
    ));

    let mut oversized = Cursor::new((MAX_CONTROL_FRAME_BYTES + 1).to_le_bytes());
    assert!(matches!(
        read_envelope(&mut oversized),
        Err(FramingError::InvalidLength { actual, .. })
            if actual == MAX_CONTROL_FRAME_BYTES + 1
    ));
}

#[test]
fn truncated_prefix_and_payload_are_distinct_failures() {
    let mut prefix = Cursor::new(vec![1, 0]);
    assert!(matches!(
        read_envelope(&mut prefix),
        Err(FramingError::TruncatedLengthPrefix)
    ));

    let mut payload = Vec::from(12_u32.to_le_bytes());
    payload.extend_from_slice(&[0x80, 0x80]);
    assert!(matches!(
        read_envelope(&mut Cursor::new(payload)),
        Err(FramingError::TruncatedPayload)
    ));
}

#[test]
fn trailing_messagepack_object_is_rejected() {
    let envelope = command_envelope(id(1), 1, id(2), Command::SessionConfigure(configure()));
    let mut encoded = latentdeck_control::encode_envelope(&envelope).unwrap();
    encoded.push(0xc0);
    assert!(matches!(
        decode_envelope(&encoded),
        Err(FramingError::TrailingBytes)
    ));
}

#[derive(Serialize)]
struct EnvelopeWithUnknown<'a> {
    protocol: ProtocolMarker,
    protocol_version: u16,
    session_id: WireUuid,
    sequence: u64,
    message_id: WireUuid,
    sender_uptime_ns: u64,
    message: &'a Message,
    unexpected: bool,
}

#[test]
fn unknown_envelope_field_is_rejected() {
    let message = Message::Command(Command::SessionConfigure(configure()));
    let encoded = rmp_serde::to_vec_named(&EnvelopeWithUnknown {
        protocol: ProtocolMarker::LatentDeckWorker,
        protocol_version: 1,
        session_id: id(1),
        sequence: 1,
        message_id: id(2),
        sender_uptime_ns: 0,
        message: &message,
        unexpected: true,
    })
    .unwrap();

    assert!(matches!(
        decode_envelope(&encoded),
        Err(FramingError::Decode(_))
    ));
}

#[derive(Serialize)]
struct ConfigureWithUnknown {
    selected_protocol_version: u16,
    app_version: String,
    heartbeat_interval_ms: u32,
    heartbeat_hard_timeout_ms: u32,
    max_frame_bytes: u32,
    max_inflight_decode_batches: u16,
    hidden_conversion: bool,
}

#[derive(Serialize)]
#[serde(tag = "name", content = "payload")]
enum CommandWithUnknown {
    #[serde(rename = "session.configure")]
    SessionConfigure(ConfigureWithUnknown),
}

#[derive(Serialize)]
#[serde(tag = "kind", content = "body", rename_all = "snake_case")]
enum MessageWithUnknown {
    Command(CommandWithUnknown),
}

#[derive(Serialize)]
struct EnvelopeWithUnknownPayload {
    protocol: ProtocolMarker,
    protocol_version: u16,
    session_id: WireUuid,
    sequence: u64,
    message_id: WireUuid,
    sender_uptime_ns: u64,
    message: MessageWithUnknown,
}

#[test]
fn unknown_command_payload_field_is_rejected() {
    let encoded = rmp_serde::to_vec_named(&EnvelopeWithUnknownPayload {
        protocol: ProtocolMarker::LatentDeckWorker,
        protocol_version: 1,
        session_id: id(1),
        sequence: 1,
        message_id: id(2),
        sender_uptime_ns: 0,
        message: MessageWithUnknown::Command(CommandWithUnknown::SessionConfigure(
            ConfigureWithUnknown {
                selected_protocol_version: 1,
                app_version: "0.1.0".to_owned(),
                heartbeat_interval_ms: 1_000,
                heartbeat_hard_timeout_ms: 10_000,
                max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
                max_inflight_decode_batches: 1,
                hidden_conversion: true,
            },
        )),
    })
    .unwrap();

    assert!(matches!(
        decode_envelope(&encoded),
        Err(FramingError::Decode(_))
    ));
}

#[test]
fn unsupported_protocol_version_is_rejected_after_decode() {
    let mut envelope = command_envelope(id(1), 1, id(2), Command::SessionConfigure(configure()));
    envelope.protocol_version = WORKER_PROTOCOL_VERSION + 1;
    let encoded = rmp_serde::to_vec_named(&envelope).unwrap();

    assert!(matches!(
        decode_envelope(&encoded),
        Err(FramingError::Validation(
            ValidationError::UnsupportedProtocolVersion { actual: 2 }
        ))
    ));
}

#[test]
fn session_validator_accepts_exactly_one_matching_reply() {
    let session_id = id(1);
    let command_id = id(2);
    let command = command_envelope(
        session_id,
        1,
        command_id,
        Command::SessionConfigure(configure()),
    );
    let reply = ack_envelope(
        session_id,
        1,
        id(3),
        command_id,
        Ack::SessionConfigure(configure_ack()),
    );

    let mut validator = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    validator.track_outbound_command(&command).unwrap();
    assert!(validator.has_pending_reply(command_id));
    validator.validate_inbound(&reply).unwrap();
    assert!(!validator.has_pending_reply(command_id));
    assert_eq!(validator.next_inbound_sequence(), 2);
    assert_eq!(validator.next_outbound_sequence(), 2);
}

#[test]
fn outbound_command_sequence_also_starts_at_one() {
    let session_id = id(1);
    let command = command_envelope(session_id, 2, id(2), Command::WorkerStatus(EmptyPayload {}));
    let mut validator = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    assert_eq!(
        validator.track_outbound_command(&command),
        Err(ValidationError::SequenceMismatch {
            expected: 1,
            actual: 2,
        })
    );
}

#[test]
fn inbound_message_budget_is_exact_at_both_edges() {
    let session_id = id(1);
    let mut validator = SessionValidator::new(session_id, InboundPolicy::CommandsOnly);
    assert_eq!(
        validator.remaining_inbound_message_budget(),
        MAX_MESSAGES_PER_SESSION
    );
    assert_eq!(
        validator.remaining_outbound_message_budget(),
        MAX_MESSAGES_PER_SESSION
    );

    for accepted in 0..MAX_MESSAGES_PER_SESSION {
        if accepted == MAX_MESSAGES_PER_SESSION - 1 {
            assert_eq!(validator.remaining_inbound_message_budget(), 1);
        }
        let sequence = u64::try_from(accepted + 1).expect("bounded session sequence fits u64");
        let envelope = command_envelope(
            session_id,
            sequence,
            id(u128::from(sequence) + 1),
            Command::WorkerStatus(EmptyPayload {}),
        );
        validator
            .validate_inbound(&envelope)
            .expect("message within the session cap must be accepted");
        if accepted == 0 {
            assert_eq!(
                validator.remaining_inbound_message_budget(),
                MAX_MESSAGES_PER_SESSION - 1
            );
        }
    }

    assert_eq!(validator.remaining_inbound_message_budget(), 0);
    assert_eq!(
        validator.remaining_outbound_message_budget(),
        MAX_MESSAGES_PER_SESSION
    );
    let over_limit_sequence =
        u64::try_from(MAX_MESSAGES_PER_SESSION + 1).expect("bounded test sequence fits u64");
    let over_limit = command_envelope(
        session_id,
        over_limit_sequence,
        id(u128::from(over_limit_sequence) + 1),
        Command::WorkerStatus(EmptyPayload {}),
    );
    assert_eq!(
        validator.validate_inbound(&over_limit),
        Err(ValidationError::SessionMessageLimit)
    );
    assert_eq!(validator.remaining_inbound_message_budget(), 0);
}

#[test]
fn outbound_message_budget_counts_completed_commands_and_is_exact_at_the_cap() {
    let session_id = id(1);
    let mut validator = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);

    for accepted in 0..MAX_MESSAGES_PER_SESSION {
        if accepted == MAX_MESSAGES_PER_SESSION - 1 {
            assert_eq!(validator.remaining_outbound_message_budget(), 1);
            assert_eq!(validator.remaining_inbound_message_budget(), 1);
        }
        let sequence = u64::try_from(accepted + 1).expect("bounded session sequence fits u64");
        let command_id = id(100_000 + u128::from(sequence));
        let command = command_envelope(
            session_id,
            sequence,
            command_id,
            Command::RingBind(RingBind {
                layout_version: 1,
                mapping_handle: 1,
                mapping_bytes: 4_096,
                frames_ready_event_handle: 1,
                ring_id: id(42),
            }),
        );
        validator
            .track_outbound_command(&command)
            .expect("command within the session cap must be accepted");
        assert!(validator.has_pending_reply(command_id));

        let reply = ack_envelope(
            session_id,
            sequence,
            id(200_000 + u128::from(sequence)),
            command_id,
            Ack::RingBind(RingBound {
                layout_version: 1,
                ring_id: id(42),
                mapping_bytes: 4_096,
            }),
        );
        validator
            .validate_inbound(&reply)
            .expect("matching reply must complete the command");
        assert!(!validator.has_pending_reply(command_id));
        if accepted == 0 {
            assert_eq!(
                validator.remaining_outbound_message_budget(),
                MAX_MESSAGES_PER_SESSION - 1
            );
            assert_eq!(
                validator.remaining_inbound_message_budget(),
                MAX_MESSAGES_PER_SESSION - 1
            );
        }
    }

    assert_eq!(validator.remaining_outbound_message_budget(), 0);
    assert_eq!(validator.remaining_inbound_message_budget(), 0);
    let over_limit_sequence =
        u64::try_from(MAX_MESSAGES_PER_SESSION + 1).expect("bounded test sequence fits u64");
    let over_limit = command_envelope(
        session_id,
        over_limit_sequence,
        id(100_000 + u128::from(over_limit_sequence)),
        Command::WorkerStatus(EmptyPayload {}),
    );
    assert_eq!(
        validator.track_outbound_command(&over_limit),
        Err(ValidationError::SessionMessageLimit)
    );
    assert_eq!(validator.remaining_outbound_message_budget(), 0);
}

#[test]
fn typed_error_reply_completes_its_matching_command() {
    let session_id = id(1);
    let command_id = id(2);
    let command = command_envelope(
        session_id,
        1,
        command_id,
        Command::WorkerStatus(EmptyPayload {}),
    );
    let error = Envelope::new(
        session_id,
        1,
        id(3),
        500,
        Message::Error(ErrorReply {
            reply_to: command_id,
            name: latentdeck_control::CommandName::WorkerStatus,
            error: ErrorPayload {
                code: ErrorCode::StateBusy,
                message: "worker is completing another state transition".to_owned(),
                retryable: true,
                fatal: false,
                worker_state: WorkerState::Busy,
                diagnostic_id: id(4),
                details: BoundedVec::default(),
            },
        }),
    );

    let encoded = latentdeck_control::encode_envelope(&error).unwrap();
    assert_eq!(decode_envelope(&encoded).unwrap(), error);

    let mut validator = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    validator.track_outbound_command(&command).unwrap();
    validator.validate_inbound(&error).unwrap();
    assert!(!validator.has_pending_reply(command_id));
}

#[test]
fn stable_error_codes_use_the_documented_dotted_wire_value() {
    let encoded = rmp_serde::to_vec_named(&ErrorCode::RingBackpressure).unwrap();
    assert!(
        encoded
            .windows(b"ring.backpressure".len())
            .any(|window| window == b"ring.backpressure")
    );
    assert_eq!(
        rmp_serde::from_slice::<ErrorCode>(&encoded).unwrap(),
        ErrorCode::RingBackpressure
    );
}

#[test]
fn session_validator_rejects_wrong_session_and_sequence() {
    let mut validator = SessionValidator::new(id(1), InboundPolicy::CommandsOnly);
    let wrong_session = command_envelope(id(99), 1, id(2), Command::WorkerStatus(EmptyPayload {}));
    assert_eq!(
        validator.validate_inbound(&wrong_session),
        Err(ValidationError::SessionMismatch)
    );

    let gap = command_envelope(id(1), 2, id(3), Command::WorkerStatus(EmptyPayload {}));
    assert_eq!(
        validator.validate_inbound(&gap),
        Err(ValidationError::SequenceMismatch {
            expected: 1,
            actual: 2,
        })
    );
}

#[test]
fn duplicate_command_message_id_is_rejected() {
    let session_id = id(1);
    let command_id = id(2);
    let first = command_envelope(
        session_id,
        1,
        command_id,
        Command::WorkerStatus(EmptyPayload {}),
    );
    let duplicate = command_envelope(
        session_id,
        2,
        command_id,
        Command::MetricsGet(EmptyPayload {}),
    );
    let mut validator = SessionValidator::new(session_id, InboundPolicy::CommandsOnly);
    validator.validate_inbound(&first).unwrap();
    assert_eq!(
        validator.validate_inbound(&duplicate),
        Err(ValidationError::DuplicateMessageId)
    );
}

#[test]
fn reply_must_reference_a_pending_command_with_the_same_name() {
    let session_id = id(1);
    let command_id = id(2);
    let command = command_envelope(
        session_id,
        1,
        command_id,
        Command::SessionConfigure(configure()),
    );
    let wrong_name = ack_envelope(
        session_id,
        1,
        id(3),
        command_id,
        Ack::CodecInspect(CodecInspection {
            torch_version: None,
            cuda_available: false,
            cuda_runtime: None,
            devices: BoundedVec::default(),
            adapters: BoundedVec::default(),
        }),
    );
    let mut validator = SessionValidator::new(session_id, InboundPolicy::ResponsesAndEvents);
    validator.track_outbound_command(&command).unwrap();
    assert!(matches!(
        validator.validate_inbound(&wrong_name),
        Err(ValidationError::ReplyNameMismatch { .. })
    ));

    let unknown_reply = ack_envelope(
        session_id,
        1,
        id(4),
        id(88),
        Ack::SessionConfigure(configure_ack()),
    );
    assert_eq!(
        validator.validate_inbound(&unknown_reply),
        Err(ValidationError::UnknownReply)
    );
}

#[test]
fn heartbeat_event_is_valid_only_in_response_direction() {
    let event = Envelope::new(
        id(1),
        1,
        id(2),
        1_000,
        Message::Event(EventMessage {
            caused_by: None,
            event: Event::WorkerHeartbeat(WorkerHeartbeat {
                worker_state: WorkerState::Ready,
                codec_state: CodecState::Unloaded,
                slot_state: SlotState::Empty,
                ring_state: RingState::Unbound,
                stream_generation: 0,
                last_completed_core_sequence: 0,
                decode_in_flight: false,
                worker_uptime_ns: 1_000,
            }),
        }),
    );
    let mut core = SessionValidator::new(id(1), InboundPolicy::ResponsesAndEvents);
    core.validate_inbound(&event).unwrap();

    let mut worker = SessionValidator::new(id(1), InboundPolicy::CommandsOnly);
    assert_eq!(
        worker.validate_inbound(&event),
        Err(ValidationError::UnexpectedMessageKind)
    );
}
