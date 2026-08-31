//! Raw latent authoring helpers shared by desktop and language bindings.

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::{
    error::{CartridgeError, ErrorCode, Result},
    hash::{MeasuredHash, Sha256Hash, hash_path},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, CartridgeId, CodecDescriptor, DType, DecodedVideoDescriptor, Identifier,
        ManifestV0_1, PayloadDescriptor, PreviewDescriptor, ProducerDescriptor, Provenance,
        ProvenanceSource, Rational, Sha256Digest, SpecVersion, TensorDescriptor, TensorStream,
        TimingDescriptor,
    },
    preview::inspect_webp,
    profile::h3::{self, ValidatedH3Profile},
    safetensor::{
        EntryRange, H3SafetensorsPreflight, SafetensorDType, preflight_h3_safetensors,
        scan_h3_safetensors_finite,
    },
    writer::{OverwritePolicy, PackRequest, WriteOptions, WriteReceipt, pack_atomic},
};

/// User-visible producer identity recorded in a raw-H3 conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawH3AuthoringOptions {
    pub producer_name: String,
    pub producer_version: String,
    pub cartridge_id: Option<String>,
    pub created_at: Option<String>,
    pub source_kind: String,
    pub source_metadata: Option<BTreeMap<String, Value>>,
    pub preview_path: Option<PathBuf>,
    pub overwrite: OverwritePolicy,
    pub expected_payload_sha256: Option<Sha256Hash>,
}

impl RawH3AuthoringOptions {
    #[must_use]
    pub fn new(producer_name: impl Into<String>, producer_version: impl Into<String>) -> Self {
        Self {
            producer_name: producer_name.into(),
            producer_version: producer_version.into(),
            cartridge_id: None,
            created_at: None,
            source_kind: "raw_h3_safetensors".to_owned(),
            source_metadata: None,
            preview_path: None,
            overwrite: OverwritePolicy::Forbid,
            expected_payload_sha256: None,
        }
    }

    #[must_use]
    pub fn with_cartridge_id(mut self, cartridge_id: impl Into<String>) -> Self {
        self.cartridge_id = Some(cartridge_id.into());
        self
    }

    #[must_use]
    pub fn with_created_at(mut self, created_at: impl Into<String>) -> Self {
        self.created_at = Some(created_at.into());
        self
    }

    #[must_use]
    pub fn with_source_kind(mut self, source_kind: impl Into<String>) -> Self {
        self.source_kind = source_kind.into();
        self
    }

    #[must_use]
    pub fn with_source_metadata(mut self, metadata: BTreeMap<String, Value>) -> Self {
        self.source_metadata = Some(metadata);
        self
    }

    #[must_use]
    pub fn with_preview(mut self, preview_path: impl Into<PathBuf>) -> Self {
        self.preview_path = Some(preview_path.into());
        self
    }

    #[must_use]
    pub fn with_overwrite(mut self, overwrite: bool) -> Self {
        self.overwrite = if overwrite {
            OverwritePolicy::Replace
        } else {
            OverwritePolicy::Forbid
        };
        self
    }

    /// Binds conversion to the exact raw payload approved during preflight.
    #[must_use]
    pub fn with_expected_payload_sha256(mut self, expected: Sha256Hash) -> Self {
        self.expected_payload_sha256 = Some(expected);
        self
    }
}

/// Fully validated raw-H3 metadata that is safe to expose before conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawH3Inspection {
    pub payload_bytes: u64,
    pub payload_sha256: Sha256Hash,
    pub safetensors: H3SafetensorsPreflight,
    pub profile: ValidatedH3Profile,
}

/// Fully validates one raw H3 Safetensors payload without writing an LC.
///
/// # Errors
///
/// Returns a stable cartridge error for unsafe structure, invalid H3 timing or
/// geometry, non-finite tensor data, or an I/O failure.
pub fn inspect_raw_h3(payload_path: impl AsRef<Path>) -> Result<RawH3Inspection> {
    let payload_path = payload_path.as_ref();
    let limits = ValidationLimits::default();
    let (safetensors, measured) = inspect_raw_h3_payload(payload_path, &limits, true)?;
    let manifest = build_h3_manifest(
        &safetensors,
        &measured,
        &RawH3AuthoringOptions::new("latentdeck-inspect", env!("CARGO_PKG_VERSION")),
        None,
    )?;
    let profile = h3::validate(&manifest, &limits)?;
    Ok(RawH3Inspection {
        payload_bytes: measured.byte_length,
        payload_sha256: measured.sha256,
        safetensors,
        profile,
    })
}

/// Builds a canonical H3 manifest and atomically commits a fully validated LC.
///
/// # Errors
///
/// Returns a stable cartridge error for malformed raw data, invalid profile
/// geometry, source mutation, output collisions, or write/validation failure.
pub fn pack_raw_h3_atomic(
    payload_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    options: &RawH3AuthoringOptions,
) -> Result<WriteReceipt> {
    let payload_path = payload_path.as_ref();
    let limits = ValidationLimits::default();
    let preview = options
        .preview_path
        .as_deref()
        .map(|path| preview_descriptor(path, &limits))
        .transpose()?;
    let (preflight, measured) = inspect_raw_h3_payload(payload_path, &limits, false)?;
    if options
        .expected_payload_sha256
        .is_some_and(|expected| expected != measured.sha256)
    {
        return Err(CartridgeError::new(
            ErrorCode::PayloadHashMismatch,
            "raw H3 source changed after approved preflight",
        )
        .at_entry(h3::PAYLOAD_PATH));
    }
    let manifest = build_h3_manifest(&preflight, &measured, options, preview)?;
    let mut request = PackRequest::new(manifest, payload_path);
    if let Some(preview_path) = &options.preview_path {
        request = request.with_preview(preview_path);
    }
    pack_atomic(
        &request,
        output_path,
        &WriteOptions {
            overwrite: options.overwrite,
        },
    )
}

fn inspect_raw_h3_payload(
    payload_path: &Path,
    limits: &ValidationLimits,
    verify_finite: bool,
) -> Result<(H3SafetensorsPreflight, MeasuredHash)> {
    let mut payload_file = File::open(payload_path).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open raw H3 Safetensors payload")
            .at_entry(h3::PAYLOAD_PATH)
            .with_source(error)
    })?;
    let payload_bytes = payload_file
        .metadata()
        .map_err(|error| {
            CartridgeError::new(
                ErrorCode::IoRead,
                "cannot inspect raw H3 Safetensors payload",
            )
            .at_entry(h3::PAYLOAD_PATH)
            .with_source(error)
        })?
        .len();
    let range = EntryRange::new(0, payload_bytes);
    let preflight = preflight_h3_safetensors(&mut payload_file, range, limits)?;
    if verify_finite {
        scan_h3_safetensors_finite(&mut payload_file, range, &preflight)?;
    }
    let measured = hash_path(payload_path)?;
    Ok((preflight, measured))
}

fn build_h3_manifest(
    preflight: &H3SafetensorsPreflight,
    measured: &MeasuredHash,
    options: &RawH3AuthoringOptions,
    preview: Option<PreviewDescriptor>,
) -> Result<ManifestV0_1> {
    let decoded_frames = h3::decoded_frame_count(preflight.video.shape[2])?;
    if let Some(audio) = &preflight.audio {
        let expected_audio_slots = h3::audio_latent_slot_count(decoded_frames)?;
        if audio.shape[3] != expected_audio_slots {
            return Err(CartridgeError::new(
                ErrorCode::TimingMismatch,
                format!(
                    "H3 audio T={} does not match {} decoded video frames (expected T={expected_audio_slots})",
                    audio.shape[3], decoded_frames
                ),
            )
            .at_tensor("audio"));
        }
    }
    let duration = Rational::reduced(decoded_frames, 24).ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::TimingMismatch,
            "raw H3 payload has zero duration",
        )
        .at_tensor("video")
    })?;
    let mut tensors = vec![TensorDescriptor {
        stream: TensorStream::Visual,
        name: Identifier("video".to_owned()),
        payload: h3::PAYLOAD_PATH.to_owned(),
        storage_dtype: manifest_dtype(preflight.video.dtype),
        runtime_dtype: DType::F16,
        shape: preflight.video.shape.clone(),
    }];
    if let Some(audio) = &preflight.audio {
        let dtype = manifest_dtype(audio.dtype);
        tensors.push(TensorDescriptor {
            stream: TensorStream::Audio,
            name: Identifier("audio".to_owned()),
            payload: h3::PAYLOAD_PATH.to_owned(),
            storage_dtype: dtype,
            runtime_dtype: dtype,
            shape: audio.shape.clone(),
        });
    }
    let payload_sha256 = measured.sha256.to_string();
    Ok(ManifestV0_1 {
        spec_version: SpecVersion(crate::LC_SPEC_VERSION.to_owned()),
        cartridge_id: CartridgeId(
            options
                .cartridge_id
                .clone()
                .unwrap_or_else(|| deterministic_cartridge_id(&measured.sha256)),
        ),
        codec: CodecDescriptor {
            family: Identifier(h3::CODEC_FAMILY.to_owned()),
            profile: Identifier(h3::PROFILE.to_owned()),
            profile_version: SpecVersion(h3::PROFILE_VERSION.to_owned()),
        },
        payloads: vec![PayloadDescriptor {
            path: h3::PAYLOAD_PATH.to_owned(),
            media_type: h3::PAYLOAD_MEDIA_TYPE.to_owned(),
            byte_length: measured.byte_length,
            sha256: Sha256Digest(payload_sha256.clone()),
        }],
        tensors,
        timing: TimingDescriptor {
            contract: Identifier(h3::TIMING_CONTRACT.to_owned()),
            contract_version: SpecVersion(h3::TIMING_CONTRACT_VERSION.to_owned()),
            decoded_video: DecodedVideoDescriptor {
                width: decoded_axis(preflight.video.shape[4], "width")?,
                height: decoded_axis(preflight.video.shape[3], "height")?,
                frame_count: decoded_frames,
                frame_rate: Rational {
                    numerator: 24,
                    denominator: 1,
                },
                duration,
            },
        },
        audio: if preflight.audio.is_some() {
            AudioDisposition::PreservedSource
        } else {
            AudioDisposition::SourceAbsent
        },
        preview,
        provenance: Provenance {
            created_by: ProducerDescriptor {
                name: Identifier(options.producer_name.clone()),
                version: options.producer_version.clone(),
            },
            created_at: options.created_at.clone(),
            sources: vec![ProvenanceSource {
                kind: Identifier(options.source_kind.clone()),
                sha256: Some(Sha256Digest(payload_sha256)),
                uri: None,
                license: None,
                metadata: options.source_metadata.clone(),
            }],
        },
        parent_cartridges: Vec::new(),
        operation_history: Vec::new(),
    })
}

fn preview_descriptor(path: &Path, limits: &ValidationLimits) -> Result<PreviewDescriptor> {
    let mut file = File::open(path).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open preview source")
            .at_entry("preview.webp")
            .with_source(error)
    })?;
    let maximum = limits.max_preview_bytes();
    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot read preview source")
                .at_entry("preview.webp")
                .with_source(error)
        })?;
    let byte_length = u64::try_from(bytes.len()).map_err(|error| {
        CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "preview byte length does not fit u64",
        )
        .at_entry("preview.webp")
        .with_source(error)
    })?;
    if byte_length > maximum {
        return Err(CartridgeError::new(
            ErrorCode::EntryTooLarge,
            "preview exceeds the LC byte ceiling",
        )
        .at_entry("preview.webp"));
    }
    let info = inspect_webp(&bytes, limits)?;
    let measured = hash_path(path)?;
    Ok(PreviewDescriptor {
        path: "preview.webp".to_owned(),
        media_type: "image/webp".to_owned(),
        byte_length: measured.byte_length,
        sha256: Sha256Digest(measured.sha256.to_string()),
        width: info.width,
        height: info.height,
    })
}

fn deterministic_cartridge_id(payload_sha256: &Sha256Hash) -> String {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&payload_sha256.as_bytes()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn decoded_axis(latent_axis: u64, axis_name: &str) -> Result<u32> {
    let decoded = latent_axis.checked_mul(16).ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::TensorSizeOverflow,
            format!("H3 decoded {axis_name} arithmetic overflow"),
        )
        .at_tensor("video")
    })?;
    u32::try_from(decoded).map_err(|error| {
        CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            format!("H3 decoded {axis_name} does not fit u32"),
        )
        .at_tensor("video")
        .with_source(error)
    })
}

const fn manifest_dtype(dtype: SafetensorDType) -> DType {
    match dtype {
        SafetensorDType::F16 => DType::F16,
        SafetensorDType::F32 => DType::F32,
    }
}
