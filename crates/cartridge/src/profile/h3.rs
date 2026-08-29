use crate::{
    error::{CartridgeError, ErrorCode, Result},
    limits::ValidationLimits,
    manifest::{DType, ManifestV0_1, Rational, TensorDescriptor, TensorStream},
};

pub const CODEC_FAMILY: &str = "minimax_h3";
pub const PROFILE: &str = "h3_av_latent";
pub const PROFILE_VERSION: &str = "0.1.0";
pub const TIMING_CONTRACT: &str = "minimax_h3_causal";
pub const TIMING_CONTRACT_VERSION: &str = "0.1.0";
pub const PAYLOAD_PATH: &str = "payloads/h3.safetensors";
pub const PAYLOAD_MEDIA_TYPE: &str = "application/vnd.safetensors";
pub const STREAMING_NEW_SLOTS: u64 = 5;
pub const STREAMING_USABLE_FRAMES: u64 = 17;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedVisual {
    pub latent_slots: u64,
    pub latent_height: u64,
    pub latent_width: u64,
    pub decoded_frame_count: u64,
    pub decoded_height: u32,
    pub decoded_width: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedAudio {
    pub latent_slots: u64,
    pub storage_dtype: DType,
    pub runtime_dtype: DType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H3CompatibilityKey {
    pub codec_family: &'static str,
    pub profile: &'static str,
    pub profile_version: &'static str,
    pub runtime_dtype: DType,
    pub batch: u64,
    pub latent_channels: u64,
    pub latent_height: u64,
    pub latent_width: u64,
    pub timing_contract: &'static str,
    pub timing_contract_version: &'static str,
    pub frame_rate: Rational,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedH3Profile {
    pub visual: ValidatedVisual,
    pub audio: Option<ValidatedAudio>,
    pub compatibility_key: H3CompatibilityKey,
}

/// Derive the exact decoded frame count for a complete H3 clip.
///
/// # Errors
///
/// Returns `tensor_shape_invalid` when `T` is not `2 + 5n`, or
/// `tensor_size_overflow` when checked cadence arithmetic overflows.
pub fn decoded_frame_count(latent_slots: u64) -> Result<u64> {
    let tail = latent_slots.checked_sub(2).ok_or_else(|| {
        CartridgeError::new(ErrorCode::TensorShapeInvalid, "H3 visual T must be 2 + 5n")
            .at_tensor("video")
    })?;
    if tail % 5 != 0 {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "H3 visual T must be 2 + 5n",
        )
        .at_tensor("video"));
    }
    let cycles = tail / 5;
    cycles
        .checked_mul(17)
        .and_then(|frames| frames.checked_add(5))
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "H3 frame-count arithmetic overflow",
            )
            .at_tensor("video")
        })
}

/// Derive the H3 audio latent length from the decoded video frame count.
///
/// H3 rounds `decoded_frames * 5 / 3` to the nearest integer. Because the
/// denominator is three, the value can never land exactly on a half slot.
///
/// # Errors
///
/// Returns `tensor_size_overflow` when checked cadence arithmetic overflows.
pub fn audio_latent_slot_count(decoded_frames: u64) -> Result<u64> {
    decoded_frames
        .checked_mul(5)
        .and_then(|slots| slots.checked_add(1))
        .map(|slots| slots / 3)
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "H3 audio cadence arithmetic overflow",
            )
            .at_tensor("audio")
        })
}

/// Return the incremental streaming cadence. This is not full-clip `T=5`.
#[must_use]
pub const fn streaming_contract() -> (u64, u64) {
    (STREAMING_NEW_SLOTS, STREAMING_USABLE_FRAMES)
}

/// Convert complete incremental streaming blocks to newly usable frames.
///
/// # Errors
///
/// Returns `timing_mismatch` for zero or partial five-slot blocks and
/// `tensor_size_overflow` if checked frame arithmetic overflows.
pub fn streaming_usable_frames(new_slots: u64) -> Result<u64> {
    if new_slots == 0 || !new_slots.is_multiple_of(STREAMING_NEW_SLOTS) {
        return Err(CartridgeError::new(
            ErrorCode::TimingMismatch,
            "H3 streaming input must contain complete five-slot blocks",
        ));
    }
    (new_slots / STREAMING_NEW_SLOTS)
        .checked_mul(STREAMING_USABLE_FRAMES)
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "H3 streaming cadence arithmetic overflow",
            )
        })
}

/// Validate codec-specific H3 tensor, geometry, and timing semantics.
///
/// # Errors
///
/// Returns a stable profile error when the manifest is unsupported,
/// inconsistent, exceeds a ceiling, or violates H3 cadence.
pub fn validate(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<ValidatedH3Profile> {
    manifest.validate_common(limits)?;
    validate_profile_identity(manifest, limits)?;
    validate_tensor_set(&manifest.tensors)?;
    validate_declared_tensor_bytes(manifest)?;

    let visual_descriptor = exactly_one_tensor(&manifest.tensors, TensorStream::Visual, "video")?;
    let visual = validate_visual(visual_descriptor, manifest, limits)?;
    let frame_rate = validate_timing(manifest, visual.decoded_frame_count)?;

    let audio_descriptor = manifest
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Audio);
    if manifest.audio.requires_audio_tensor() != audio_descriptor.is_some() {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "audio policy contradicts audio tensor presence",
        )
        .at_json("/audio/policy"));
    }
    let audio = audio_descriptor
        .map(|descriptor| validate_audio(descriptor, visual.decoded_frame_count, limits))
        .transpose()?;

    Ok(ValidatedH3Profile {
        compatibility_key: H3CompatibilityKey {
            codec_family: CODEC_FAMILY,
            profile: PROFILE,
            profile_version: PROFILE_VERSION,
            runtime_dtype: visual_descriptor.runtime_dtype,
            batch: visual_descriptor.shape[0],
            latent_channels: visual_descriptor.shape[1],
            latent_height: visual.latent_height,
            latent_width: visual.latent_width,
            timing_contract: TIMING_CONTRACT,
            timing_contract_version: TIMING_CONTRACT_VERSION,
            frame_rate,
        },
        visual,
        audio,
    })
}

fn validate_profile_identity(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<()> {
    if manifest.codec.family.0 != CODEC_FAMILY {
        return Err(
            CartridgeError::new(ErrorCode::UnsupportedCodec, "unsupported codec family")
                .at_json("/codec/family"),
        );
    }
    if manifest.codec.profile.0 != PROFILE || manifest.codec.profile_version.0 != PROFILE_VERSION {
        return Err(CartridgeError::new(
            ErrorCode::UnsupportedProfileVersion,
            "unsupported H3 profile or profile version",
        )
        .at_json("/codec/profile_version"));
    }
    if manifest.timing.contract.0 != TIMING_CONTRACT
        || manifest.timing.contract_version.0 != TIMING_CONTRACT_VERSION
    {
        return Err(CartridgeError::new(
            ErrorCode::UnsupportedProfileVersion,
            "unsupported H3 timing contract",
        )
        .at_json("/timing/contract_version"));
    }
    let payload = &manifest.payloads[0];
    if payload.path != PAYLOAD_PATH {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "H3 payload path must be payloads/h3.safetensors",
        )
        .at_json("/payloads/0/path"));
    }
    if payload.media_type != PAYLOAD_MEDIA_TYPE {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "H3 payload media type must be application/vnd.safetensors",
        )
        .at_json("/payloads/0/media_type"));
    }
    if payload.byte_length == 0 || payload.byte_length > limits.max_h3_payload_bytes() {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "H3 payload byte length is outside the profile limit",
        )
        .at_json("/payloads/0/byte_length"));
    }
    Ok(())
}

fn validate_tensor_set(tensors: &[TensorDescriptor]) -> Result<()> {
    for tensor in tensors {
        if tensor.payload != PAYLOAD_PATH {
            return Err(CartridgeError::new(
                ErrorCode::TensorDescriptorMismatch,
                "H3 tensors must reference payloads/h3.safetensors",
            )
            .at_tensor(&tensor.name.0));
        }
        let known = matches!(
            (tensor.stream, tensor.name.0.as_str()),
            (TensorStream::Visual, "video") | (TensorStream::Audio, "audio")
        );
        if !known {
            return Err(CartridgeError::new(
                ErrorCode::TensorUnexpected,
                format!("unexpected H3 tensor descriptor {}", tensor.name.0),
            )
            .at_tensor(&tensor.name.0));
        }
    }
    Ok(())
}

fn validate_declared_tensor_bytes(manifest: &ManifestV0_1) -> Result<()> {
    let mut tensor_bytes = 0_u64;
    for descriptor in &manifest.tensors {
        if descriptor.shape.is_empty() || descriptor.shape.contains(&0) {
            return Err(CartridgeError::new(
                ErrorCode::TensorShapeInvalid,
                "tensor axes must be positive",
            )
            .at_tensor(&descriptor.name.0));
        }
        let elements = descriptor.shape.iter().try_fold(1_u64, |count, axis| {
            count.checked_mul(*axis).ok_or_else(|| {
                CartridgeError::new(
                    ErrorCode::TensorSizeOverflow,
                    "tensor element-count arithmetic overflow",
                )
                .at_tensor(&descriptor.name.0)
            })
        })?;
        let width = descriptor.storage_dtype.byte_width().ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorDtypeForbidden,
                "tensor storage dtype has no LC 0.1 byte width",
            )
            .at_tensor(&descriptor.name.0)
        })?;
        let bytes = elements.checked_mul(width).ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "tensor byte-count arithmetic overflow",
            )
            .at_tensor(&descriptor.name.0)
        })?;
        tensor_bytes = tensor_bytes.checked_add(bytes).ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "combined tensor byte-count arithmetic overflow",
            )
            .at_tensor(&descriptor.name.0)
        })?;
    }
    let minimum_payload_bytes = tensor_bytes.checked_add(8).ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::TensorSizeOverflow,
            "Safetensors envelope size arithmetic overflow",
        )
    })?;
    if manifest.payloads[0].byte_length < minimum_payload_bytes {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "payload is shorter than its declared tensor data",
        )
        .at_json("/payloads/0/byte_length"));
    }
    Ok(())
}

fn validate_visual(
    descriptor: &TensorDescriptor,
    manifest: &ManifestV0_1,
    limits: &ValidationLimits,
) -> Result<ValidatedVisual> {
    if descriptor.shape.len() != 5 || descriptor.shape[0] != 1 || descriptor.shape[1] != 24 {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "H3 video shape must be [1,24,T,H,W]",
        )
        .at_tensor("video"));
    }
    if descriptor.runtime_dtype != DType::F16 {
        return Err(CartridgeError::new(
            ErrorCode::TensorDtypeForbidden,
            "H3 video runtime dtype must be F16",
        )
        .at_tensor("video"));
    }

    let latent_slots = descriptor.shape[2];
    if latent_slots > limits.max_h3_temporal_axis() {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "H3 visual temporal axis exceeds the profile limit",
        )
        .at_tensor("video"));
    }
    let expected_frames = decoded_frame_count(latent_slots)?;
    if manifest.timing.decoded_video.frame_count != expected_frames {
        return Err(CartridgeError::new(
            ErrorCode::TimingMismatch,
            format!("H3 T={latent_slots} requires {expected_frames} decoded frames"),
        )
        .at_json("/timing/decoded_video/frame_count"));
    }

    let latent_height = descriptor.shape[3];
    let latent_width = descriptor.shape[4];
    let decoded_height = checked_decoded_axis(latent_height, limits, "height")?;
    let decoded_width = checked_decoded_axis(latent_width, limits, "width")?;
    if manifest.timing.decoded_video.height != decoded_height
        || manifest.timing.decoded_video.width != decoded_width
    {
        return Err(CartridgeError::new(
            ErrorCode::DecodedGeometryMismatch,
            "decoded geometry must equal the H3 latent grid multiplied by 16",
        )
        .at_json("/timing/decoded_video"));
    }

    Ok(ValidatedVisual {
        latent_slots,
        latent_height,
        latent_width,
        decoded_frame_count: expected_frames,
        decoded_height,
        decoded_width,
    })
}

fn validate_timing(manifest: &ManifestV0_1, expected_frames: u64) -> Result<Rational> {
    let frame_rate = manifest.timing.decoded_video.frame_rate;
    if frame_rate
        != (Rational {
            numerator: 24,
            denominator: 1,
        })
    {
        return Err(
            CartridgeError::new(ErrorCode::TimingMismatch, "H3 frame rate must be 24/1")
                .at_json("/timing/decoded_video/frame_rate"),
        );
    }
    let expected_duration = Rational::reduced(expected_frames, 24).ok_or_else(|| {
        CartridgeError::new(ErrorCode::TimingMismatch, "invalid zero duration")
            .at_json("/timing/decoded_video/duration")
    })?;
    if manifest.timing.decoded_video.duration != expected_duration {
        return Err(CartridgeError::new(
            ErrorCode::TimingMismatch,
            "duration must exactly equal frame_count / frame_rate",
        )
        .at_json("/timing/decoded_video/duration"));
    }
    Ok(frame_rate)
}

fn validate_audio(
    descriptor: &TensorDescriptor,
    expected_frames: u64,
    limits: &ValidationLimits,
) -> Result<ValidatedAudio> {
    if descriptor.name.0 != "audio"
        || descriptor.shape.len() != 4
        || descriptor.shape[0] != 1
        || descriptor.shape[1] != 32
        || descriptor.shape[2] != 2
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "H3 audio shape must be [1,32,2,T_audio]",
        )
        .at_tensor("audio"));
    }
    if descriptor.runtime_dtype != descriptor.storage_dtype {
        return Err(CartridgeError::new(
            ErrorCode::TensorDtypeForbidden,
            "H3 audio runtime dtype must preserve its storage dtype",
        )
        .at_tensor("audio"));
    }
    let expected_audio_slots = audio_latent_slot_count(expected_frames)?;
    if descriptor.shape[3] != expected_audio_slots
        || descriptor.shape[3] == 0
        || descriptor.shape[3] > limits.max_h3_temporal_axis()
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            format!("H3 audio T must be {expected_audio_slots}"),
        )
        .at_tensor("audio"));
    }
    Ok(ValidatedAudio {
        latent_slots: descriptor.shape[3],
        storage_dtype: descriptor.storage_dtype,
        runtime_dtype: descriptor.runtime_dtype,
    })
}

fn exactly_one_tensor<'a>(
    tensors: &'a [TensorDescriptor],
    stream: TensorStream,
    name: &str,
) -> Result<&'a TensorDescriptor> {
    let mut matches = tensors
        .iter()
        .filter(|tensor| tensor.stream == stream && tensor.name.0 == name);
    let tensor = matches.next().ok_or_else(|| {
        CartridgeError::new(ErrorCode::TensorMissing, format!("missing {name} tensor"))
            .at_tensor(name)
    })?;
    if matches.next().is_some() {
        return Err(CartridgeError::new(
            ErrorCode::TensorUnexpected,
            format!("duplicate {name} tensor descriptor"),
        )
        .at_tensor(name));
    }
    Ok(tensor)
}

fn checked_decoded_axis(axis: u64, limits: &ValidationLimits, label: &str) -> Result<u32> {
    let decoded = axis.checked_mul(16).ok_or_else(|| {
        CartridgeError::new(ErrorCode::TensorSizeOverflow, "decoded geometry overflow")
            .at_tensor("video")
    })?;
    let decoded = u32::try_from(decoded).map_err(|error| {
        CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "decoded geometry exceeds u32",
        )
        .at_tensor("video")
        .with_source(error)
    })?;
    if decoded == 0 || decoded > limits.max_h3_decoded_axis() {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            format!("decoded {label} exceeds the H3 profile limit"),
        )
        .at_tensor("video"));
    }
    Ok(decoded)
}
