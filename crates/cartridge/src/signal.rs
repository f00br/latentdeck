//! Closed codec-neutral visual signal geometry derived from a validated LC manifest.

use crate::{
    error::{CartridgeError, ErrorCode, Result},
    limits::ValidationLimits,
    manifest::{DType, ManifestV0_1, Rational, TensorStream},
};

/// Exact visual and decoded geometry available without applying codec semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecNeutralSignalGeometry {
    pub runtime_dtype: DType,
    pub batch: u64,
    pub latent_channels: u64,
    pub latent_slots: u64,
    pub latent_height: u64,
    pub latent_width: u64,
    pub decoded_frame_count: u64,
    pub decoded_height: u32,
    pub decoded_width: u32,
    pub frame_rate: Rational,
}

/// Validate the closed signal geometry required by Library and runtime selection.
///
/// This deliberately applies no codec-specific temporal mapping or spatial scaling.
/// Those remain the selected trusted adapter's responsibility.
///
/// # Errors
///
/// Rejects manifests without exactly one visual `[1,C,T,H,W]` tensor, non-positive
/// extents, contradictory decoded duration, or a payload reference mismatch.
pub fn validate_codec_neutral_signal_geometry(
    manifest: &ManifestV0_1,
) -> Result<CodecNeutralSignalGeometry> {
    manifest.validate_common(&ValidationLimits::default())?;
    let mut visuals = manifest
        .tensors
        .iter()
        .enumerate()
        .filter(|(_, tensor)| tensor.stream == TensorStream::Visual);
    let (visual_index, visual) = visuals.next().ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::TensorMissing,
            "LC signal geometry requires exactly one visual tensor",
        )
        .at_json("/tensors")
    })?;
    if visuals.next().is_some() {
        return Err(CartridgeError::new(
            ErrorCode::TensorUnexpected,
            "LC signal geometry requires exactly one visual tensor",
        )
        .at_json("/tensors"));
    }
    if visual.shape.len() != 5
        || visual.shape[0] != 1
        || visual.shape.contains(&0)
        || !visual.runtime_dtype.is_supported()
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "visual signal geometry must be finite-runtime [1,C,T,H,W] with positive extents",
        )
        .at_json(format!("/tensors/{visual_index}/shape")));
    }
    let payload = manifest.payloads.first().ok_or_else(|| {
        CartridgeError::new(ErrorCode::ManifestInvalid, "LC signal payload is missing")
            .at_json("/payloads")
    })?;
    if visual.payload != payload.path {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "visual signal tensor does not reference the exact LC payload",
        )
        .at_json(format!("/tensors/{visual_index}/payload")));
    }

    let decoded = &manifest.timing.decoded_video;
    if decoded.width == 0 || decoded.height == 0 || decoded.frame_count == 0 {
        return Err(CartridgeError::new(
            ErrorCode::DecodedGeometryMismatch,
            "decoded signal geometry must have positive width, height, and frame count",
        )
        .at_json("/timing/decoded_video"));
    }
    let duration_numerator = decoded
        .frame_count
        .checked_mul(decoded.frame_rate.denominator)
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TimingMismatch,
                "decoded signal duration arithmetic overflowed",
            )
            .at_json("/timing/decoded_video/duration")
        })?;
    let exact_duration = Rational::reduced(duration_numerator, decoded.frame_rate.numerator)
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TimingMismatch,
                "decoded signal timing is invalid",
            )
            .at_json("/timing/decoded_video/duration")
        })?;
    if decoded.duration != exact_duration {
        return Err(CartridgeError::new(
            ErrorCode::TimingMismatch,
            "decoded duration must exactly equal frame_count divided by frame_rate",
        )
        .at_json("/timing/decoded_video/duration"));
    }

    Ok(CodecNeutralSignalGeometry {
        runtime_dtype: visual.runtime_dtype,
        batch: visual.shape[0],
        latent_channels: visual.shape[1],
        latent_slots: visual.shape[2],
        latent_height: visual.shape[3],
        latent_width: visual.shape[4],
        decoded_frame_count: decoded.frame_count,
        decoded_height: decoded.height,
        decoded_width: decoded.width,
        frame_rate: decoded.frame_rate,
    })
}
