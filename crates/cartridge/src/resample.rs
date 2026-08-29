//! Build validated LC manifests around trusted post-operator H3 spools.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    LC_SPEC_VERSION,
    error::{CartridgeError, ErrorCode, Result},
    hash::{Sha256Hash, hash_path},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, CartridgeId, CodecDescriptor, DType, DecodedVideoDescriptor, Identifier,
        ManifestV0_1, OperationRecord, ParentCartridge, PayloadDescriptor, ProducerDescriptor,
        Provenance, ProvenanceSource, Rational, Sha256Digest, SpecVersion, TensorDescriptor,
        TensorStream, TimingDescriptor,
    },
    profile::h3,
    safetensor::{
        EntryRange, H3SafetensorsPreflight, SafetensorDType, preflight_h3_safetensors,
        scan_h3_safetensors_finite,
    },
    writer::{PackRequest, WriteOptions, WriteReceipt, pack_atomic},
};

const RESAMPLE_PRODUCER: &str = "latentdeck-resample";
const RESAMPLE_PRODUCER_VERSION: &str = "0.1.0";
const CAPTURE_MODE_CONTROL: &str = "capture_mode";

/// The two required v0.1 post-operator capture modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Snapshot,
    LiveCapture,
}

impl CaptureMode {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::LiveCapture => "live_capture",
        }
    }
}

/// Worker-reported identity used to bind a temporary payload against swaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadExpectation {
    pub byte_length: u64,
    pub sha256: Sha256Digest,
}

/// Complete non-path metadata needed to construct one resampled LC manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct ResampleManifestRequest {
    pub cartridge_id: CartridgeId,
    pub expected_payload: PayloadExpectation,
    pub capture_mode: CaptureMode,
    pub audio: AudioDisposition,
    pub parent_cartridges: Vec<ParentCartridge>,
    pub operator_id: Identifier,
    pub operator_version: String,
    pub seed: u64,
    pub controls: BTreeMap<String, Value>,
}

/// Atomic LC write plus cleanup state for the consumed temporary spool.
#[derive(Debug)]
pub struct ResampleWriteReceipt {
    pub output_path: PathBuf,
    pub validation: crate::reader::ValidationReceipt,
    pub spool_removed: bool,
}

impl From<(WriteReceipt, bool)> for ResampleWriteReceipt {
    fn from((receipt, spool_removed): (WriteReceipt, bool)) -> Self {
        Self {
            output_path: receipt.output_path,
            validation: receipt.validation,
            spool_removed,
        }
    }
}

/// Validate a bounded worker spool, construct its genealogy, write the LC
/// archive atomically, then consume the exact spool path on success.
///
/// # Errors
///
/// Returns a stable cartridge error for a swapped/invalid spool, forbidden
/// post-operator dtype, contradictory audio policy, invalid provenance, or LC
/// writer failure. A failed write leaves the spool in place for explicit
/// recovery; a successful write reports whether cleanup succeeded.
pub fn pack_resample_atomic(
    request: &ResampleManifestRequest,
    payload_path: impl AsRef<Path>,
    output: impl AsRef<Path>,
    options: &WriteOptions,
) -> Result<ResampleWriteReceipt> {
    let payload_path = payload_path.as_ref();
    let manifest = build_resample_manifest(request, payload_path)?;
    let receipt = pack_atomic(
        &PackRequest::new(manifest, payload_path),
        output.as_ref(),
        options,
    )?;
    let spool_removed = fs::remove_file(payload_path).is_ok();
    Ok((receipt, spool_removed).into())
}

/// Inspect one exact post-operator spool and construct its strict LC manifest.
///
/// # Errors
///
/// Returns a stable error before any archive output is created when the spool
/// or supplied genealogy violates the v0.1 resample contract.
pub fn build_resample_manifest(
    request: &ResampleManifestRequest,
    payload_path: impl AsRef<Path>,
) -> Result<ManifestV0_1> {
    validate_request(request)?;
    let payload_path = payload_path.as_ref();
    let measured = hash_path(payload_path)?;
    bind_payload_expectation(
        &request.expected_payload,
        measured.byte_length,
        measured.sha256,
    )?;
    let preflight = inspect_payload(payload_path, measured.byte_length)?;
    if preflight.video.dtype != SafetensorDType::F16 {
        return Err(CartridgeError::new(
            ErrorCode::TensorDtypeForbidden,
            "resampled post-operator visual storage dtype must be F16",
        )
        .at_tensor("video"));
    }
    validate_audio_policy(request, &preflight)?;

    let manifest =
        manifest_from_preflight(request, &preflight, measured.byte_length, measured.sha256)?;
    let limits = ValidationLimits::default();
    manifest.validate_common(&limits)?;
    h3::validate(&manifest, &limits)?;
    Ok(manifest)
}

fn validate_request(request: &ResampleManifestRequest) -> Result<()> {
    if request.controls.contains_key(CAPTURE_MODE_CONTROL) {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "capture_mode is reserved resample provenance",
        )
        .at_json("/operation_history/0/controls/capture_mode"));
    }
    if request.parent_cartridges.is_empty() {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "resampled cartridge requires at least one parent",
        )
        .at_json("/parent_cartridges"));
    }
    if matches!(request.audio, AudioDisposition::PreservedSource) {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "resampling must record copied, omitted, or absent audio explicitly",
        )
        .at_json("/audio/policy"));
    }
    if request.capture_mode == CaptureMode::Snapshot
        && matches!(
            request.audio,
            AudioDisposition::OmittedTimingMismatch { .. }
        )
    {
        return Err(CartridgeError::new(
            ErrorCode::TimingMismatch,
            "snapshot audio cannot be omitted for a timing mismatch",
        )
        .at_json("/audio/policy"));
    }
    if let Some(source) = audio_source(&request.audio) {
        let source_is_parent = request.parent_cartridges.iter().any(|parent| {
            parent.cartridge_id == source.cartridge_id
                && parent.archive_sha256 == source.archive_sha256
        });
        if !source_is_parent {
            return Err(CartridgeError::new(
                ErrorCode::ManifestInvalid,
                "audio source cartridge must be one of the declared parents",
            )
            .at_json("/audio/source_cartridge"));
        }
    }
    Ok(())
}

fn bind_payload_expectation(
    expected: &PayloadExpectation,
    actual_bytes: u64,
    actual_sha256: Sha256Hash,
) -> Result<()> {
    Sha256Hash::parse(&expected.sha256.0)?;
    if expected.byte_length != actual_bytes || expected.sha256.0 != actual_sha256.to_string() {
        return Err(CartridgeError::new(
            ErrorCode::PayloadHashMismatch,
            "resample spool changed after worker finalization",
        )
        .at_entry(h3::PAYLOAD_PATH));
    }
    Ok(())
}

fn inspect_payload(path: &Path, byte_length: u64) -> Result<H3SafetensorsPreflight> {
    let mut file = File::open(path).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open resample spool")
            .at_entry(h3::PAYLOAD_PATH)
            .with_source(error)
    })?;
    let range = EntryRange::new(0, byte_length);
    let limits = ValidationLimits::default();
    let preflight = preflight_h3_safetensors(&mut file, range, &limits)?;
    scan_h3_safetensors_finite(&mut file, range, &preflight)?;
    Ok(preflight)
}

fn validate_audio_policy(
    request: &ResampleManifestRequest,
    preflight: &H3SafetensorsPreflight,
) -> Result<()> {
    if request.audio.requires_audio_tensor() != preflight.audio.is_some() {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "resample audio policy contradicts the spool tensor set",
        )
        .at_json("/audio/policy"));
    }
    Ok(())
}

fn manifest_from_preflight(
    request: &ResampleManifestRequest,
    preflight: &H3SafetensorsPreflight,
    payload_bytes: u64,
    payload_sha256: Sha256Hash,
) -> Result<ManifestV0_1> {
    let visual_shape = &preflight.video.shape;
    let latent_slots = visual_shape[2];
    let decoded_frames = h3::decoded_frame_count(latent_slots)?;
    let decoded_height = decoded_axis(visual_shape[3])?;
    let decoded_width = decoded_axis(visual_shape[4])?;
    let duration = Rational::reduced(decoded_frames, 24).ok_or_else(|| {
        CartridgeError::new(ErrorCode::TimingMismatch, "resample duration is invalid")
    })?;

    let mut tensors = vec![TensorDescriptor {
        stream: TensorStream::Visual,
        name: Identifier("video".to_owned()),
        payload: h3::PAYLOAD_PATH.to_owned(),
        storage_dtype: DType::F16,
        runtime_dtype: DType::F16,
        shape: visual_shape.clone(),
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

    let mut controls = request.controls.clone();
    controls.insert(
        CAPTURE_MODE_CONTROL.to_owned(),
        Value::String(request.capture_mode.as_str().to_owned()),
    );

    Ok(ManifestV0_1 {
        spec_version: SpecVersion(LC_SPEC_VERSION.to_owned()),
        cartridge_id: request.cartridge_id.clone(),
        codec: CodecDescriptor {
            family: Identifier(h3::CODEC_FAMILY.to_owned()),
            profile: Identifier(h3::PROFILE.to_owned()),
            profile_version: SpecVersion(h3::PROFILE_VERSION.to_owned()),
        },
        payloads: vec![PayloadDescriptor {
            path: h3::PAYLOAD_PATH.to_owned(),
            media_type: h3::PAYLOAD_MEDIA_TYPE.to_owned(),
            byte_length: payload_bytes,
            sha256: Sha256Digest(payload_sha256.to_string()),
        }],
        tensors,
        timing: TimingDescriptor {
            contract: Identifier(h3::TIMING_CONTRACT.to_owned()),
            contract_version: SpecVersion(h3::TIMING_CONTRACT_VERSION.to_owned()),
            decoded_video: DecodedVideoDescriptor {
                width: decoded_width,
                height: decoded_height,
                frame_count: decoded_frames,
                frame_rate: Rational {
                    numerator: 24,
                    denominator: 1,
                },
                duration,
            },
        },
        audio: request.audio.clone(),
        preview: None,
        provenance: Provenance {
            created_by: ProducerDescriptor {
                name: Identifier(RESAMPLE_PRODUCER.to_owned()),
                version: RESAMPLE_PRODUCER_VERSION.to_owned(),
            },
            created_at: None,
            sources: provenance_sources(&request.parent_cartridges),
        },
        parent_cartridges: request.parent_cartridges.clone(),
        operation_history: vec![OperationRecord {
            operator_id: request.operator_id.clone(),
            operator_version: request.operator_version.clone(),
            seed: request.seed,
            controls,
        }],
    })
}

fn provenance_sources(parents: &[ParentCartridge]) -> Vec<ProvenanceSource> {
    parents
        .iter()
        .map(|parent| {
            let metadata = Map::from_iter([
                (
                    "cartridge_id".to_owned(),
                    Value::String(parent.cartridge_id.0.clone()),
                ),
                ("role".to_owned(), Value::String(parent.role.0.clone())),
            ]);
            ProvenanceSource {
                kind: Identifier("latent_cartridge".to_owned()),
                sha256: Some(parent.archive_sha256.clone()),
                uri: None,
                license: None,
                metadata: Some(metadata.into_iter().collect()),
            }
        })
        .collect()
}

const fn manifest_dtype(dtype: SafetensorDType) -> DType {
    match dtype {
        SafetensorDType::F16 => DType::F16,
        SafetensorDType::F32 => DType::F32,
    }
}

fn decoded_axis(latent_axis: u64) -> Result<u32> {
    latent_axis
        .checked_mul(16)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::DecodedGeometryMismatch,
                "resample decoded geometry is invalid",
            )
            .at_tensor("video")
        })
}

fn audio_source(audio: &AudioDisposition) -> Option<&crate::manifest::SourceCartridgeRef> {
    match audio {
        AudioDisposition::CopiedFromCarrierExact { source_cartridge }
        | AudioDisposition::OmittedTimingMismatch {
            source_cartridge, ..
        } => Some(source_cartridge),
        AudioDisposition::SourceAbsent | AudioDisposition::PreservedSource => None,
    }
}
