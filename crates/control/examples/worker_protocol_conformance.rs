//! Cross-language Worker Protocol fixture generator and response validator.
//!
//! The repository Python tests invoke this helper at runtime. Rust produces
//! every bootstrap and command byte, then validates the Python worker's typed
//! response stream with `latentdeck-control`.

use std::{
    env,
    error::Error,
    io::{self, Read, Write},
};

use latentdeck_control::{
    Ack, AuthToken, Command, CommandName, EmptyPayload, Envelope, ErrorCode, Event, InboundPolicy,
    MAX_CONTROL_FRAME_BYTES, Message, SessionConfigure, SessionValidator, ShutdownReason, WireUuid,
    WorkerShutdown, read_envelope, write_envelope,
};
use serde::Serialize;
use uuid::Uuid;

const SESSION_ID: &str = "9ca8c228-04c7-4b59-909f-6fbef591a43e";
const PRECONFIGURE_STATUS_ID: &str = "10000000-0000-4000-8000-000000000001";
const CONFIGURE_ID: &str = "10000000-0000-4000-8000-000000000002";
const STATUS_ID: &str = "10000000-0000-4000-8000-000000000003";
const SHUTDOWN_ID: &str = "10000000-0000-4000-8000-000000000004";
const GAP_ID: &str = "10000000-0000-4000-8000-000000000005";
const AUTH_BYTES: [u8; 32] = [b'a'; 32];

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct BootstrapRecord<'a> {
    bootstrap_version: u16,
    session_id: WireUuid,
    pipe_name: &'a str,
    auth_token: &'a AuthToken,
}

fn id(value: &str) -> WireUuid {
    WireUuid::from_uuid(Uuid::parse_str(value).expect("fixture UUID is valid"))
}

fn configure() -> SessionConfigure {
    SessionConfigure {
        selected_protocol_version: 1,
        app_version: "rust-python-conformance".to_owned(),
        heartbeat_interval_ms: 1_000,
        heartbeat_hard_timeout_ms: 10_000,
        max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
        max_inflight_decode_batches: 1,
    }
}

fn command(sequence: u64, message_id: &str, command: Command) -> Envelope {
    Envelope::new(
        id(SESSION_ID),
        sequence,
        id(message_id),
        sequence,
        Message::Command(command),
    )
}

fn session_commands() -> Vec<Envelope> {
    vec![
        command(
            1,
            PRECONFIGURE_STATUS_ID,
            Command::WorkerStatus(EmptyPayload {}),
        ),
        command(2, CONFIGURE_ID, Command::SessionConfigure(configure())),
        command(3, STATUS_ID, Command::WorkerStatus(EmptyPayload {})),
        command(
            4,
            SHUTDOWN_ID,
            Command::WorkerShutdown(WorkerShutdown {
                reason: ShutdownReason::UserRequest,
            }),
        ),
    ]
}

fn sequence_gap_command() -> Envelope {
    command(2, GAP_ID, Command::SessionConfigure(configure()))
}

fn write_bootstrap(output: &mut impl Write) -> Result<(), Box<dyn Error>> {
    let session_id = id(SESSION_ID);
    let auth_token = AuthToken::new(AUTH_BYTES);
    let pipe_name = format!(r"\\.\pipe\LatentDeck.Conformance.{session_id}");
    let payload = rmp_serde::to_vec_named(&BootstrapRecord {
        bootstrap_version: 1,
        session_id,
        pipe_name: &pipe_name,
        auth_token: &auth_token,
    })?;
    let length = u32::try_from(payload.len())?;
    output.write_all(&length.to_le_bytes())?;
    output.write_all(&payload)?;
    Ok(())
}

fn emit(commands: &[Envelope]) -> Result<(), Box<dyn Error>> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    write_bootstrap(&mut output)?;
    for envelope in commands {
        write_envelope(&mut output, envelope)?;
    }
    output.flush()?;
    Ok(())
}

fn read_python_frames() -> Result<Vec<Envelope>, Box<dyn Error>> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut raw = Vec::new();
    input.read_to_end(&mut raw)?;
    let mut cursor = io::Cursor::new(raw);
    let mut frames = Vec::new();
    while let Some(frame) = read_envelope(&mut cursor)? {
        frames.push(frame);
    }
    Ok(frames)
}

fn assert_authenticated_hello(envelope: &Envelope) {
    let Message::Event(event) = &envelope.message else {
        panic!("first Python frame was not an event");
    };
    assert!(event.caused_by.is_none());
    let Event::WorkerHello(hello) = &event.event else {
        panic!("first Python event was not worker.hello");
    };
    assert!(
        hello
            .auth_token
            .constant_time_eq(&AuthToken::new(AUTH_BYTES))
    );
    assert_eq!(hello.protocol_min, 1);
    assert_eq!(hello.protocol_max, 1);
    assert!(
        hello
            .available_adapters
            .iter()
            .any(|adapter| adapter == "org.latentdeck.h3")
    );
}

fn validate_session() -> Result<(), Box<dyn Error>> {
    let commands = session_commands();
    let mut validator = SessionValidator::new(id(SESSION_ID), InboundPolicy::ResponsesAndEvents);
    for envelope in &commands {
        validator.track_outbound_command(envelope)?;
    }

    let frames = read_python_frames()?;
    assert!(!frames.is_empty());
    assert_authenticated_hello(&frames[0]);

    let mut saw_expected_error = false;
    let mut saw_configure_ack = false;
    let mut saw_status_ack = false;
    let mut saw_shutdown_ack = false;
    let mut state_events = 0;
    for envelope in &frames {
        validator.validate_inbound(envelope)?;
        match &envelope.message {
            Message::Error(reply) => {
                assert_eq!(reply.reply_to, id(PRECONFIGURE_STATUS_ID));
                assert_eq!(reply.name, CommandName::WorkerStatus);
                assert_eq!(reply.error.code, ErrorCode::StateInvalidTransition);
                assert!(!reply.error.fatal);
                saw_expected_error = true;
            }
            Message::Ack(reply) => match &reply.ack {
                Ack::SessionConfigure(_) => {
                    assert_eq!(reply.reply_to, id(CONFIGURE_ID));
                    saw_configure_ack = true;
                }
                Ack::WorkerStatus(status) => {
                    assert_eq!(reply.reply_to, id(STATUS_ID));
                    assert_eq!(status.worker_version, "0.1.0");
                    saw_status_ack = true;
                }
                Ack::WorkerShutdown(shutdown) => {
                    assert_eq!(reply.reply_to, id(SHUTDOWN_ID));
                    assert!(shutdown.accepted);
                    saw_shutdown_ack = true;
                }
                other => panic!("unexpected Python acknowledgement: {other:?}"),
            },
            Message::Event(event) => {
                if matches!(event.event, Event::WorkerStateChanged(_)) {
                    state_events += 1;
                }
            }
            Message::Command(_) => panic!("Python worker emitted a command"),
        }
    }

    assert!(saw_expected_error);
    assert!(saw_configure_ack);
    assert!(saw_status_ack);
    assert!(saw_shutdown_ack);
    assert_eq!(state_events, 2);
    for envelope in &commands {
        assert!(!validator.has_pending_reply(envelope.message_id));
    }
    Ok(())
}

fn validate_sequence_gap() -> Result<(), Box<dyn Error>> {
    let frames = read_python_frames()?;
    assert_eq!(frames.len(), 2);
    assert_authenticated_hello(&frames[0]);

    let mut validator = SessionValidator::new(id(SESSION_ID), InboundPolicy::ResponsesAndEvents);
    for envelope in &frames {
        validator.validate_inbound(envelope)?;
    }
    let Message::Event(event) = &frames[1].message else {
        panic!("sequence gap did not produce an event");
    };
    let Event::WorkerFault(fault) = &event.event else {
        panic!("sequence gap did not produce worker.fault");
    };
    assert_eq!(fault.code, ErrorCode::ProtocolSequenceInvalid);
    assert!(fault.fatal);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    match env::args().nth(1).as_deref() {
        Some("emit-session") => emit(&session_commands()),
        Some("emit-sequence-gap") => emit(&[sequence_gap_command()]),
        Some("validate-session") => validate_session(),
        Some("validate-sequence-gap") => validate_sequence_gap(),
        _ => Err(io::Error::other("unknown conformance helper mode").into()),
    }
}
