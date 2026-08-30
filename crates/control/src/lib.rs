//! Versioned commands and events independent from any UI or operating-system
//! transport.
//!
//! The crate deliberately owns only the bounded `MessagePack` wire contract. A
//! Windows Named Pipe implementation and worker process supervision belong in
//! the runtime crate.

mod d2;
mod d2_capture;
mod framing;
mod preset;
mod protocol;
mod q4;

pub use d2::*;
pub use d2_capture::*;
pub use framing::{FramingError, decode_envelope, encode_envelope, read_envelope, write_envelope};
pub use preset::*;
pub use protocol::*;
pub use q4::*;

/// The first worker protocol understood by the 0.1 applications and worker.
pub const CONTROL_SCHEMA_VERSION: u16 = WORKER_PROTOCOL_VERSION;
