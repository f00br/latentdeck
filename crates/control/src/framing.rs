use std::io::{self, Cursor, Read, Write};

use serde::Deserialize;
use thiserror::Error;

use crate::{Envelope, ErrorCode, MAX_CONTROL_FRAME_BYTES, ValidationError};

/// Errors produced before a message reaches the operating-system transport.
#[derive(Debug, Error)]
pub enum FramingError {
    #[error("control frame length prefix ended early")]
    TruncatedLengthPrefix,
    #[error("control frame payload ended early")]
    TruncatedPayload,
    #[error("control frame length {actual} is outside 1..={maximum}")]
    InvalidLength { actual: u32, maximum: u32 },
    #[error("MessagePack encode failed: {0}")]
    Encode(String),
    #[error("MessagePack decode failed: {0}")]
    Decode(String),
    #[error("MessagePack frame contains trailing bytes")]
    TrailingBytes,
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error("control I/O failed: {0}")]
    Io(#[from] io::Error),
}

impl FramingError {
    /// Return the stable wire code corresponding to this local failure.
    #[must_use]
    pub fn stable_code(&self) -> ErrorCode {
        match self {
            Self::TruncatedLengthPrefix | Self::TruncatedPayload => {
                ErrorCode::ProtocolTruncatedFrame
            }
            Self::InvalidLength { actual, maximum } if actual > maximum => {
                ErrorCode::ProtocolFrameTooLarge
            }
            Self::InvalidLength { .. } => ErrorCode::ProtocolBadLength,
            Self::Decode(_) | Self::TrailingBytes => ErrorCode::ProtocolInvalidMessagePack,
            Self::Validation(error) => error.stable_code(),
            Self::Encode(_) | Self::Io(_) => ErrorCode::WorkerInternal,
        }
    }
}

/// Encode one validated envelope without its four-byte length prefix.
///
/// # Errors
///
/// Returns an error when the envelope is invalid, serialization fails, or the
/// encoded payload exceeds the protocol frame limit.
pub fn encode_envelope(envelope: &Envelope) -> Result<Vec<u8>, FramingError> {
    envelope.validate_static()?;
    let payload = rmp_serde::to_vec_named(envelope)
        .map_err(|error| FramingError::Encode(error.to_string()))?;
    let length = u32::try_from(payload.len()).map_err(|_| FramingError::InvalidLength {
        actual: u32::MAX,
        maximum: MAX_CONTROL_FRAME_BYTES,
    })?;
    if !(1..=MAX_CONTROL_FRAME_BYTES).contains(&length) {
        return Err(FramingError::InvalidLength {
            actual: length,
            maximum: MAX_CONTROL_FRAME_BYTES,
        });
    }
    Ok(payload)
}

/// Decode exactly one `MessagePack` envelope and reject trailing data.
///
/// # Errors
///
/// Returns an error for an invalid length, malformed or trailing data, an
/// unsupported version, or a statically invalid envelope.
pub fn decode_envelope(payload: &[u8]) -> Result<Envelope, FramingError> {
    let length = u32::try_from(payload.len()).map_err(|_| FramingError::InvalidLength {
        actual: u32::MAX,
        maximum: MAX_CONTROL_FRAME_BYTES,
    })?;
    if !(1..=MAX_CONTROL_FRAME_BYTES).contains(&length) {
        return Err(FramingError::InvalidLength {
            actual: length,
            maximum: MAX_CONTROL_FRAME_BYTES,
        });
    }

    let mut cursor = Cursor::new(payload);
    let mut deserializer = rmp_serde::Deserializer::new(&mut cursor);
    let envelope = Envelope::deserialize(&mut deserializer)
        .map_err(|error| FramingError::Decode(error.to_string()))?;
    drop(deserializer);
    if cursor.position() != payload.len() as u64 {
        return Err(FramingError::TrailingBytes);
    }
    envelope.validate_static()?;
    Ok(envelope)
}

/// Write one `u32` little-endian length-prefixed envelope.
///
/// # Errors
///
/// Returns an encode/validation error or the underlying writer error.
pub fn write_envelope<W: Write>(writer: &mut W, envelope: &Envelope) -> Result<(), FramingError> {
    let payload = encode_envelope(envelope)?;
    let length = u32::try_from(payload.len()).map_err(|_| FramingError::InvalidLength {
        actual: u32::MAX,
        maximum: MAX_CONTROL_FRAME_BYTES,
    })?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&payload)?;
    Ok(())
}

/// Read one `u32` little-endian length-prefixed envelope.
///
/// Clean EOF before the first prefix byte returns `Ok(None)`. EOF after any
/// part of a prefix or payload is a framing error.
///
/// # Errors
///
/// Returns an I/O error, a truncated/invalid length error, or any decoding and
/// envelope-validation error.
pub fn read_envelope<R: Read>(reader: &mut R) -> Result<Option<Envelope>, FramingError> {
    let mut prefix = [0_u8; 4];
    loop {
        match reader.read(&mut prefix[..1]) {
            Ok(0) => return Ok(None),
            Ok(1) => break,
            Ok(_) => unreachable!("the input slice contains exactly one byte"),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FramingError::Io(error)),
        }
    }
    reader
        .read_exact(&mut prefix[1..])
        .map_err(map_prefix_error)?;

    let length = u32::from_le_bytes(prefix);
    if !(1..=MAX_CONTROL_FRAME_BYTES).contains(&length) {
        return Err(FramingError::InvalidLength {
            actual: length,
            maximum: MAX_CONTROL_FRAME_BYTES,
        });
    }

    let mut payload = vec![0_u8; length as usize];
    reader.read_exact(&mut payload).map_err(map_payload_error)?;
    decode_envelope(&payload).map(Some)
}

fn map_prefix_error(error: io::Error) -> FramingError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        FramingError::TruncatedLengthPrefix
    } else {
        FramingError::Io(error)
    }
}

fn map_payload_error(error: io::Error) -> FramingError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        FramingError::TruncatedPayload
    } else {
        FramingError::Io(error)
    }
}
