//! Versioned commands and events independent from any UI or operating-system
//! transport.
//!
//! The crate deliberately owns only the bounded `MessagePack` wire contract. A
//! Windows Named Pipe implementation and worker process supervision belong in
//! the runtime crate.

mod framing;
mod protocol;

pub use framing::{FramingError, decode_envelope, encode_envelope, read_envelope, write_envelope};
pub use protocol::*;

/// The first worker protocol understood by the 0.1 applications and worker.
pub const CONTROL_SCHEMA_VERSION: u16 = WORKER_PROTOCOL_VERSION;
