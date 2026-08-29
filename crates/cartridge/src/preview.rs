//! Bounded WebP preview envelope validation.

use crate::error::{CartridgeError, ErrorCode, Result};
use crate::limits::{MAX_PREVIEW_AXIS, MAX_PREVIEW_BYTES, MAX_PREVIEW_PIXELS, ValidationLimits};

const RIFF_HEADER_BYTES: usize = 12;
const CHUNK_HEADER_BYTES: usize = 8;

/// Canvas geometry measured from a bounded WebP envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebpInfo {
    pub width: u32,
    pub height: u32,
}

/// Validates a WebP RIFF envelope and reads its canvas dimensions.
///
/// # Errors
///
/// Returns an error for malformed, truncated, oversized, or ambiguous WebP
/// envelopes.
pub fn inspect_webp(bytes: &[u8], _limits: &ValidationLimits) -> Result<WebpInfo> {
    let byte_length = u64::try_from(bytes.len()).map_err(|error| {
        CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "preview byte length does not fit u64",
        )
        .at_entry("preview.webp")
        .with_source(error)
    })?;
    if byte_length > MAX_PREVIEW_BYTES {
        return Err(preview_error(
            ErrorCode::EntryTooLarge,
            "preview exceeds the LC byte ceiling",
        ));
    }
    if bytes.len() < RIFF_HEADER_BYTES || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        return Err(preview_error(
            ErrorCode::ManifestInvalid,
            "preview is not a WebP RIFF envelope",
        ));
    }
    let declared_riff_size = usize::try_from(read_u32(bytes, 4)?).map_err(|error| {
        preview_error(
            ErrorCode::ManifestInvalid,
            "WebP RIFF length does not fit memory",
        )
        .with_source(error)
    })?;
    if declared_riff_size.checked_add(8) != Some(bytes.len()) {
        return Err(preview_error(
            ErrorCode::ManifestInvalid,
            "WebP RIFF length does not match the entry",
        ));
    }

    let mut cursor = RIFF_HEADER_BYTES;
    let mut dimensions = None;
    while cursor < bytes.len() {
        let header_end = cursor.checked_add(CHUNK_HEADER_BYTES).ok_or_else(|| {
            preview_error(
                ErrorCode::ManifestInvalid,
                "WebP chunk header offset overflow",
            )
        })?;
        if header_end > bytes.len() {
            return Err(preview_error(
                ErrorCode::ManifestInvalid,
                "WebP chunk header is truncated",
            ));
        }
        let fourcc = &bytes[cursor..cursor + 4];
        let payload_length = usize::try_from(read_u32(bytes, cursor + 4)?).map_err(|error| {
            preview_error(
                ErrorCode::ManifestInvalid,
                "WebP chunk length does not fit memory",
            )
            .with_source(error)
        })?;
        let payload_end = header_end.checked_add(payload_length).ok_or_else(|| {
            preview_error(
                ErrorCode::ManifestInvalid,
                "WebP chunk payload offset overflow",
            )
        })?;
        let padded_end = payload_end.checked_add(payload_length & 1).ok_or_else(|| {
            preview_error(
                ErrorCode::ManifestInvalid,
                "WebP chunk padding offset overflow",
            )
        })?;
        if padded_end > bytes.len() {
            return Err(preview_error(
                ErrorCode::ManifestInvalid,
                "WebP chunk extends beyond the RIFF envelope",
            ));
        }
        if let Some(found) = dimensions_for_chunk(fourcc, &bytes[header_end..payload_end])?
            && dimensions.replace(found).is_some()
        {
            return Err(preview_error(
                ErrorCode::ManifestInvalid,
                "WebP contains more than one authoritative image geometry",
            ));
        }
        cursor = padded_end;
    }
    let info = dimensions.ok_or_else(|| {
        preview_error(
            ErrorCode::ManifestInvalid,
            "WebP does not contain a supported image geometry chunk",
        )
    })?;
    validate_geometry(info)?;
    Ok(info)
}

fn dimensions_for_chunk(fourcc: &[u8], payload: &[u8]) -> Result<Option<WebpInfo>> {
    match fourcc {
        b"VP8X" => {
            if payload.len() != 10 {
                return Err(preview_error(
                    ErrorCode::ManifestInvalid,
                    "VP8X chunk must contain exactly ten bytes",
                ));
            }
            Ok(Some(WebpInfo {
                width: read_u24(payload, 4)?.checked_add(1).ok_or_else(|| {
                    preview_error(ErrorCode::ManifestInvalid, "VP8X width overflows")
                })?,
                height: read_u24(payload, 7)?.checked_add(1).ok_or_else(|| {
                    preview_error(ErrorCode::ManifestInvalid, "VP8X height overflows")
                })?,
            }))
        }
        b"VP8 " => {
            if payload.len() < 10 || payload[3..6] != [0x9d, 0x01, 0x2a] {
                return Err(preview_error(
                    ErrorCode::ManifestInvalid,
                    "VP8 frame header is invalid",
                ));
            }
            Ok(Some(WebpInfo {
                width: u32::from(u16::from_le_bytes([payload[6], payload[7]]) & 0x3fff),
                height: u32::from(u16::from_le_bytes([payload[8], payload[9]]) & 0x3fff),
            }))
        }
        b"VP8L" => {
            if payload.len() < 5 || payload[0] != 0x2f {
                return Err(preview_error(
                    ErrorCode::ManifestInvalid,
                    "VP8L frame header is invalid",
                ));
            }
            let width = 1 + (u32::from(payload[1]) | (u32::from(payload[2] & 0x3f) << 8));
            let height = 1
                + (u32::from(payload[2] >> 6)
                    | (u32::from(payload[3]) << 2)
                    | (u32::from(payload[4] & 0x0f) << 10));
            Ok(Some(WebpInfo { width, height }))
        }
        _ => Ok(None),
    }
}

fn validate_geometry(info: WebpInfo) -> Result<()> {
    let pixels = u64::from(info.width)
        .checked_mul(u64::from(info.height))
        .ok_or_else(|| {
            preview_error(
                ErrorCode::RuntimeLimitExceeded,
                "preview pixel count overflows",
            )
        })?;
    if info.width == 0
        || info.height == 0
        || info.width > MAX_PREVIEW_AXIS
        || info.height > MAX_PREVIEW_AXIS
        || pixels > MAX_PREVIEW_PIXELS
    {
        return Err(preview_error(
            ErrorCode::RuntimeLimitExceeded,
            "preview geometry exceeds the LC limits",
        ));
    }
    Ok(())
}

fn read_u24(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(3)
        .ok_or_else(|| preview_error(ErrorCode::ManifestInvalid, "WebP u24 offset overflow"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| preview_error(ErrorCode::ManifestInvalid, "WebP u24 field is truncated"))?;
    Ok(u32::from(slice[0]) | (u32::from(slice[1]) << 8) | (u32::from(slice[2]) << 16))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| preview_error(ErrorCode::ManifestInvalid, "WebP u32 offset overflow"))?;
    let slice = bytes
        .get(offset..end)
        .ok_or_else(|| preview_error(ErrorCode::ManifestInvalid, "WebP u32 field is truncated"))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn preview_error(code: ErrorCode, detail: impl Into<String>) -> CartridgeError {
    CartridgeError::new(code, detail).at_entry("preview.webp")
}
