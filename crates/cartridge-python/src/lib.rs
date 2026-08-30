//! Native Python adapter for the single Rust Cartridge SDK implementation.

use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use latentdeck_cartridge::{
    error::{CartridgeError as CoreError, ErrorCode, Result as CoreResult},
    hash::{MeasuredHash, hash_path, hash_reader},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, CartridgeId, CodecDescriptor, DType, DecodedVideoDescriptor, Identifier,
        ManifestV0_1, PayloadDescriptor, PreviewDescriptor, ProducerDescriptor, Provenance,
        ProvenanceSource, Rational, Sha256Digest, SpecVersion, TensorDescriptor, TensorStream,
        TimingDescriptor, parse_manifest_json,
    },
    preview::inspect_webp,
    profile::h3,
    reader::{InspectOptions, ValidationOptions, inspect_path, open_validated},
    safetensor::{
        EntryRange, H3SafetensorsPreflight, SafetensorDType, SafetensorTensorDescriptor,
        preflight_h3_safetensors, scan_h3_safetensors_finite,
    },
    writer::{OverwritePolicy, PackRequest, WriteOptions, pack_atomic},
};
use pyo3::{exceptions::PyException, prelude::*, types::PyBytes};
use serde::Deserialize;
use serde_json::{Value, json};

const BINDING_ABI_VERSION: &str = "0.1";

/// One native exception with stable Rust error metadata.
#[pyclass(extends=PyException, module = "latentdeck_cartridge._native")]
struct CartridgeError {
    #[pyo3(get)]
    code: String,
    #[pyo3(get)]
    detail: String,
    #[pyo3(get)]
    entry: Option<String>,
    #[pyo3(get)]
    tensor: Option<String>,
    #[pyo3(get)]
    json_pointer: Option<String>,
}

#[pymethods]
impl CartridgeError {
    #[new]
    #[pyo3(signature = (code, detail, entry=None, tensor=None, json_pointer=None))]
    fn new(
        code: String,
        detail: String,
        entry: Option<String>,
        tensor: Option<String>,
        json_pointer: Option<String>,
    ) -> Self {
        Self {
            code,
            detail,
            entry,
            tensor,
            json_pointer,
        }
    }

    fn __str__(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

#[pyfunction]
fn inspect_json(py: Python<'_>, path: &str) -> PyResult<String> {
    inspect_value(path)
        .map(|value| value.to_string())
        .map_err(|error| into_py_error(py, &error))
}

#[pyfunction]
fn validate_json(py: Python<'_>, path: &str) -> PyResult<String> {
    validate_value(path)
        .map(|value| value.to_string())
        .map_err(|error| into_py_error(py, &error))
}

#[pyfunction]
fn hash_json(py: Python<'_>, path: &str) -> PyResult<String> {
    hash_value(path)
        .map(|value| value.to_string())
        .map_err(|error| into_py_error(py, &error))
}

#[pyfunction]
fn inspect_raw_h3_json(py: Python<'_>, path: &str) -> PyResult<String> {
    inspect_raw_h3_value(path)
        .map(|value| value.to_string())
        .map_err(|error| into_py_error(py, &error))
}

type ReadH3Result = (String, Vec<u8>, Option<Vec<u8>>);

#[derive(Clone, Copy)]
struct TensorReadLimits {
    max_visual_values: Option<u64>,
    max_tensor_bytes: Option<u64>,
}

#[pyfunction]
#[pyo3(signature = (path, max_visual_values=None, max_tensor_bytes=None))]
fn read_h3(
    py: Python<'_>,
    path: &str,
    max_visual_values: Option<u64>,
    max_tensor_bytes: Option<u64>,
) -> PyResult<(String, Py<PyBytes>, Option<Py<PyBytes>>)> {
    read_h3_value(
        path,
        TensorReadLimits {
            max_visual_values,
            max_tensor_bytes,
        },
    )
    .map(|(metadata, video, audio)| {
        (
            metadata,
            PyBytes::new(py, &video).unbind(),
            audio.map(|bytes| PyBytes::new(py, &bytes).unbind()),
        )
    })
    .map_err(|error| into_py_error(py, &error))
}

#[pyfunction]
#[pyo3(signature = (path, max_visual_values=None, max_tensor_bytes=None))]
fn read_raw_h3(
    py: Python<'_>,
    path: &str,
    max_visual_values: Option<u64>,
    max_tensor_bytes: Option<u64>,
) -> PyResult<(String, Py<PyBytes>, Option<Py<PyBytes>>)> {
    read_raw_h3_value(
        path,
        TensorReadLimits {
            max_visual_values,
            max_tensor_bytes,
        },
    )
    .map(|(metadata, video, audio)| {
        (
            metadata,
            PyBytes::new(py, &video).unbind(),
            audio.map(|bytes| PyBytes::new(py, &bytes).unbind()),
        )
    })
    .map_err(|error| into_py_error(py, &error))
}

#[pyfunction]
#[pyo3(signature = (manifest_json, payload_path, output_path, preview_path=None, overwrite=false))]
fn pack_json(
    py: Python<'_>,
    manifest_json: &str,
    payload_path: &str,
    output_path: &str,
    preview_path: Option<&str>,
    overwrite: bool,
) -> PyResult<String> {
    pack_value(
        manifest_json,
        payload_path,
        output_path,
        preview_path,
        overwrite,
    )
    .map(|value| value.to_string())
    .map_err(|error| into_py_error(py, &error))
}

#[pyfunction]
#[pyo3(signature = (
    payload_path,
    output_path,
    preview_path=None,
    cartridge_id=None,
    provenance_json=None,
    overwrite=false
))]
fn pack_raw_h3_json(
    py: Python<'_>,
    payload_path: &str,
    output_path: &str,
    preview_path: Option<&str>,
    cartridge_id: Option<&str>,
    provenance_json: Option<&str>,
    overwrite: bool,
) -> PyResult<String> {
    pack_raw_h3_value(
        payload_path,
        output_path,
        preview_path,
        cartridge_id,
        provenance_json,
        overwrite,
    )
    .map(|value| value.to_string())
    .map_err(|error| into_py_error(py, &error))
}

fn inspect_value(path: &str) -> CoreResult<Value> {
    let inspection = inspect_path(path, &InspectOptions::default())?;
    Ok(json!({
        "status": "ok",
        "command": "inspect",
        "validation_level": inspection.validation_level,
        "archive_bytes": inspection.archive_size,
        "manifest": inspection.manifest,
        "profile": {
            "visual": {
                "latent_slots": inspection.h3_profile.visual.latent_slots,
                "latent_height": inspection.h3_profile.visual.latent_height,
                "latent_width": inspection.h3_profile.visual.latent_width,
                "decoded_frames": inspection.h3_profile.visual.decoded_frame_count,
                "decoded_height": inspection.h3_profile.visual.decoded_height,
                "decoded_width": inspection.h3_profile.visual.decoded_width,
            },
            "audio_latent_slots": inspection
                .h3_profile
                .audio
                .as_ref()
                .map(|audio| audio.latent_slots),
        },
        "safetensors": preflight_value(&inspection.safetensors),
    }))
}

fn validate_value(path: &str) -> CoreResult<Value> {
    let validated = open_validated(path, &ValidationOptions::default())?;
    Ok(json!({
        "status": "ok",
        "command": "validate",
        "cartridge_id": validated.manifest().cartridge_id.0,
        "validation": validated.receipt(),
    }))
}

fn hash_value(path: &str) -> CoreResult<Value> {
    let measured = hash_path(path)?;
    Ok(json!({
        "status": "ok",
        "command": "hash",
        "byte_length": measured.byte_length,
        "sha256": measured.sha256,
    }))
}

fn inspect_raw_h3_value(path: &str) -> CoreResult<Value> {
    let limits = ValidationLimits::default();
    let payload_path = PathBuf::from(path);
    let (preflight, measured) = inspect_raw_h3_payload(&payload_path, &limits, true)?;
    let payload_sha256 = measured.sha256.to_string();
    let manifest = build_h3_manifest(
        &preflight,
        measured.byte_length,
        &payload_sha256,
        None,
        AuthoringProvenance::default(),
        None,
        &limits,
    )?;
    let profile = h3::validate(&manifest, &limits)?;

    Ok(json!({
        "status": "ok",
        "command": "inspect_raw_h3",
        "byte_length": measured.byte_length,
        "sha256": measured.sha256,
        "profile": {
            "codec_family": h3::CODEC_FAMILY,
            "profile": h3::PROFILE,
            "profile_version": h3::PROFILE_VERSION,
            "visual": {
                "latent_slots": profile.visual.latent_slots,
                "latent_height": profile.visual.latent_height,
                "latent_width": profile.visual.latent_width,
                "decoded_frames": profile.visual.decoded_frame_count,
                "decoded_height": profile.visual.decoded_height,
                "decoded_width": profile.visual.decoded_width,
            },
            "audio_latent_slots": profile.audio.as_ref().map(|audio| audio.latent_slots),
        },
        "safetensors": preflight_value(&preflight),
    }))
}

fn read_h3_value(path: &str, read_limits: TensorReadLimits) -> CoreResult<ReadH3Result> {
    let mut validated = open_validated(path, &ValidationOptions::default())?;
    let manifest = validated.manifest().clone();
    let validation = validated.receipt().clone();
    for descriptor in &manifest.tensors {
        let values = checked_tensor_values(&descriptor.name.0, &descriptor.shape)?;
        let byte_width = descriptor.storage_dtype.byte_width().ok_or_else(|| {
            CoreError::new(
                ErrorCode::RuntimeLimitExceeded,
                "validated tensor has no supported storage byte width",
            )
            .at_tensor(&descriptor.name.0)
        })?;
        let byte_length = values.checked_mul(byte_width).ok_or_else(|| {
            CoreError::new(
                ErrorCode::RuntimeLimitExceeded,
                "validated tensor byte length overflows u64",
            )
            .at_tensor(&descriptor.name.0)
        })?;
        enforce_tensor_read_limits(&descriptor.name.0, values, byte_length, read_limits)?;
    }
    let tensor_metadata = manifest
        .tensors
        .iter()
        .map(|descriptor| {
            (
                descriptor.name.0.clone(),
                json!({
                    "dtype": descriptor.storage_dtype,
                    "shape": descriptor.shape,
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut video = Vec::new();
    validated
        .tensor_reader("video")?
        .read_to_end(&mut video)
        .map_err(|error| {
            CoreError::new(ErrorCode::IoRead, "cannot read validated H3 visual tensor")
                .at_tensor("video")
                .with_source(error)
        })?;
    let audio = if tensor_metadata.contains_key("audio") {
        let mut bytes = Vec::new();
        validated
            .tensor_reader("audio")?
            .read_to_end(&mut bytes)
            .map_err(|error| {
                CoreError::new(ErrorCode::IoRead, "cannot read validated H3 audio tensor")
                    .at_tensor("audio")
                    .with_source(error)
            })?;
        Some(bytes)
    } else {
        None
    };
    let metadata = json!({
        "status": "ok",
        "command": "read_h3",
        "manifest": manifest,
        "validation": validation,
        "tensors": tensor_metadata,
    })
    .to_string();
    Ok((metadata, video, audio))
}

fn read_raw_h3_value(path: &str, read_limits: TensorReadLimits) -> CoreResult<ReadH3Result> {
    let limits = ValidationLimits::default();
    let mut file = File::open(path).map_err(|error| {
        CoreError::new(ErrorCode::IoOpen, "cannot open raw H3 Safetensors payload")
            .at_entry(h3::PAYLOAD_PATH)
            .with_source(error)
    })?;
    let payload_bytes = file
        .metadata()
        .map_err(|error| {
            CoreError::new(
                ErrorCode::IoRead,
                "cannot inspect raw H3 Safetensors payload",
            )
            .at_entry(h3::PAYLOAD_PATH)
            .with_source(error)
        })?
        .len();
    let range = EntryRange::new(0, payload_bytes);
    let preflight = preflight_h3_safetensors(&mut file, range, &limits)?;
    enforce_tensor_read_limits(
        "video",
        checked_tensor_values("video", &preflight.video.shape)?,
        preflight.video.byte_length,
        read_limits,
    )?;
    if let Some(descriptor) = &preflight.audio {
        enforce_tensor_read_limits(
            "audio",
            checked_tensor_values("audio", &descriptor.shape)?,
            descriptor.byte_length,
            read_limits,
        )?;
    }
    scan_h3_safetensors_finite(&mut file, range, &preflight)?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        CoreError::new(
            ErrorCode::IoRead,
            "cannot rewind raw H3 payload for hashing",
        )
        .at_entry(h3::PAYLOAD_PATH)
        .with_source(error)
    })?;
    let measured = hash_reader(&mut file)?;
    let payload_sha256 = measured.sha256.to_string();
    let manifest = build_h3_manifest(
        &preflight,
        measured.byte_length,
        &payload_sha256,
        None,
        AuthoringProvenance::default(),
        None,
        &limits,
    )?;
    let profile = h3::validate(&manifest, &limits)?;
    let video = read_raw_tensor(&mut file, &preflight, &preflight.video)?;
    let audio = preflight
        .audio
        .as_ref()
        .map(|descriptor| read_raw_tensor(&mut file, &preflight, descriptor))
        .transpose()?;

    let tensors = raw_h3_tensor_metadata(&preflight);
    let metadata = json!({
        "status": "ok",
        "command": "read_raw_h3",
        "byte_length": measured.byte_length,
        "sha256": measured.sha256,
        "profile": {
            "codec_family": h3::CODEC_FAMILY,
            "profile": h3::PROFILE,
            "profile_version": h3::PROFILE_VERSION,
            "visual": {
                "latent_slots": profile.visual.latent_slots,
                "latent_height": profile.visual.latent_height,
                "latent_width": profile.visual.latent_width,
                "decoded_frames": profile.visual.decoded_frame_count,
                "decoded_height": profile.visual.decoded_height,
                "decoded_width": profile.visual.decoded_width,
            },
            "audio_latent_slots": profile.audio.as_ref().map(|audio| audio.latent_slots),
        },
        "safetensors": preflight_value(&preflight),
        "tensors": tensors,
    })
    .to_string();
    Ok((metadata, video, audio))
}

fn raw_h3_tensor_metadata(preflight: &H3SafetensorsPreflight) -> serde_json::Map<String, Value> {
    let mut tensors = serde_json::Map::new();
    tensors.insert(
        "video".to_owned(),
        json!({
            "dtype": safetensor_dtype_name(preflight.video.dtype),
            "shape": preflight.video.shape,
        }),
    );
    if let Some(descriptor) = &preflight.audio {
        tensors.insert(
            "audio".to_owned(),
            json!({
                "dtype": safetensor_dtype_name(descriptor.dtype),
                "shape": descriptor.shape,
            }),
        );
    }
    tensors
}

fn checked_tensor_values(name: &str, shape: &[u64]) -> CoreResult<u64> {
    shape.iter().try_fold(1_u64, |values, axis| {
        values.checked_mul(*axis).ok_or_else(|| {
            CoreError::new(
                ErrorCode::RuntimeLimitExceeded,
                "tensor value count overflows u64",
            )
            .at_tensor(name)
        })
    })
}

fn enforce_tensor_read_limits(
    name: &str,
    values: u64,
    byte_length: u64,
    limits: TensorReadLimits,
) -> CoreResult<()> {
    if name == "video"
        && limits
            .max_visual_values
            .is_some_and(|maximum| values > maximum)
    {
        return Err(CoreError::new(
            ErrorCode::RuntimeLimitExceeded,
            "visual tensor exceeds the caller's value admission bound",
        )
        .at_tensor(name));
    }
    if limits
        .max_tensor_bytes
        .is_some_and(|maximum| byte_length > maximum)
    {
        return Err(CoreError::new(
            ErrorCode::RuntimeLimitExceeded,
            "tensor exceeds the caller's byte admission bound",
        )
        .at_tensor(name));
    }
    Ok(())
}

fn read_raw_tensor(
    file: &mut File,
    preflight: &H3SafetensorsPreflight,
    descriptor: &SafetensorTensorDescriptor,
) -> CoreResult<Vec<u8>> {
    let offset = preflight
        .data_offset
        .checked_add(descriptor.data_offsets[0])
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::TensorSizeOverflow,
                "raw H3 tensor data offset overflows u64",
            )
            .at_tensor(descriptor.name.clone())
        })?;
    file.seek(SeekFrom::Start(offset)).map_err(|error| {
        CoreError::new(ErrorCode::IoRead, "cannot seek to validated raw H3 tensor")
            .at_tensor(descriptor.name.clone())
            .with_source(error)
    })?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(descriptor.byte_length)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CoreError::new(ErrorCode::IoRead, "cannot read validated raw H3 tensor")
                .at_tensor(descriptor.name.clone())
                .with_source(error)
        })?;
    if u64::try_from(bytes.len()).ok() != Some(descriptor.byte_length) {
        return Err(CoreError::new(
            ErrorCode::IoRead,
            "validated raw H3 tensor was truncated while reading",
        )
        .at_tensor(descriptor.name.clone()));
    }
    Ok(bytes)
}

const fn safetensor_dtype_name(dtype: SafetensorDType) -> &'static str {
    match dtype {
        SafetensorDType::F16 => "F16",
        SafetensorDType::F32 => "F32",
    }
}

fn pack_value(
    manifest_json: &str,
    payload_path: &str,
    output_path: &str,
    preview_path: Option<&str>,
    overwrite: bool,
) -> CoreResult<Value> {
    let manifest = parse_manifest_json(manifest_json.as_bytes(), &ValidationLimits::default())?;
    let mut request = PackRequest::new(manifest, PathBuf::from(payload_path));
    if let Some(preview_path) = preview_path {
        request = request.with_preview(PathBuf::from(preview_path));
    }
    let options = WriteOptions {
        overwrite: if overwrite {
            OverwritePolicy::Replace
        } else {
            OverwritePolicy::Forbid
        },
    };
    let receipt = pack_atomic(&request, PathBuf::from(output_path), &options)?;
    Ok(json!({
        "status": "ok",
        "command": "pack",
        "output": receipt.output_path,
        "validation": receipt.validation,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoringProvenance {
    created_by: Option<AuthoringProducer>,
    created_at: Option<String>,
    source_kind: Option<String>,
    source_metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthoringProducer {
    name: String,
    version: String,
}

fn pack_raw_h3_value(
    payload_path: &str,
    output_path: &str,
    preview_path: Option<&str>,
    cartridge_id: Option<&str>,
    provenance_json: Option<&str>,
    overwrite: bool,
) -> CoreResult<Value> {
    let limits = ValidationLimits::default();
    let payload_path_buf = PathBuf::from(payload_path);
    let (preflight, measured_payload) = inspect_raw_h3_payload(&payload_path_buf, &limits, false)?;
    let payload_sha256 = measured_payload.sha256.to_string();
    let manifest = build_h3_manifest(
        &preflight,
        measured_payload.byte_length,
        &payload_sha256,
        cartridge_id,
        parse_authoring_provenance(provenance_json, &limits)?,
        preview_path,
        &limits,
    )?;
    let mut request = PackRequest::new(manifest, payload_path_buf);
    if let Some(preview_path) = preview_path {
        request = request.with_preview(PathBuf::from(preview_path));
    }
    let options = write_options(overwrite);
    let receipt = pack_atomic(&request, PathBuf::from(output_path), &options)?;
    Ok(pack_receipt_value(
        &receipt.output_path,
        &receipt.validation,
    ))
}

fn inspect_raw_h3_payload(
    payload_path: &Path,
    limits: &ValidationLimits,
    verify_finite: bool,
) -> CoreResult<(H3SafetensorsPreflight, MeasuredHash)> {
    let mut payload_file = File::open(payload_path).map_err(|error| {
        CoreError::new(ErrorCode::IoOpen, "cannot open raw H3 Safetensors payload")
            .at_entry(h3::PAYLOAD_PATH)
            .with_source(error)
    })?;
    let payload_bytes = payload_file
        .metadata()
        .map_err(|error| {
            CoreError::new(
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
    payload_bytes: u64,
    payload_sha256: &str,
    cartridge_id: Option<&str>,
    authoring: AuthoringProvenance,
    preview_path: Option<&str>,
    limits: &ValidationLimits,
) -> CoreResult<ManifestV0_1> {
    let decoded_frames = h3::decoded_frame_count(preflight.video.shape[2])?;
    validate_audio_cadence(preflight, decoded_frames)?;
    let payload_digest = Sha256Digest(payload_sha256.to_owned());
    let cartridge_id = CartridgeId(match cartridge_id {
        Some(value) => value.to_owned(),
        None => deterministic_cartridge_id(payload_sha256)?,
    });
    let preview = preview_path
        .map(|path| preview_descriptor(path, limits))
        .transpose()?;

    Ok(ManifestV0_1 {
        spec_version: SpecVersion(latentdeck_cartridge::LC_SPEC_VERSION.to_owned()),
        cartridge_id,
        codec: CodecDescriptor {
            family: Identifier(h3::CODEC_FAMILY.to_owned()),
            profile: Identifier(h3::PROFILE.to_owned()),
            profile_version: SpecVersion(h3::PROFILE_VERSION.to_owned()),
        },
        payloads: vec![PayloadDescriptor {
            path: h3::PAYLOAD_PATH.to_owned(),
            media_type: h3::PAYLOAD_MEDIA_TYPE.to_owned(),
            byte_length: payload_bytes,
            sha256: payload_digest.clone(),
        }],
        tensors: h3_tensor_descriptors(preflight),
        timing: h3_timing_descriptor(preflight, decoded_frames)?,
        audio: if preflight.audio.is_some() {
            AudioDisposition::PreservedSource
        } else {
            AudioDisposition::SourceAbsent
        },
        preview,
        provenance: h3_authoring_provenance(authoring, payload_digest),
        parent_cartridges: Vec::new(),
        operation_history: Vec::new(),
    })
}

fn validate_audio_cadence(
    preflight: &H3SafetensorsPreflight,
    decoded_frames: u64,
) -> CoreResult<()> {
    let Some(audio) = &preflight.audio else {
        return Ok(());
    };
    let expected_audio_slots = h3::audio_latent_slot_count(decoded_frames)?;
    if audio.shape[3] != expected_audio_slots {
        return Err(CoreError::new(
            ErrorCode::TimingMismatch,
            format!(
                "H3 audio T={} does not match {} decoded video frames (expected T={expected_audio_slots})",
                audio.shape[3], decoded_frames
            ),
        )
        .at_tensor("audio"));
    }
    Ok(())
}

fn h3_timing_descriptor(
    preflight: &H3SafetensorsPreflight,
    decoded_frames: u64,
) -> CoreResult<TimingDescriptor> {
    let duration = Rational::reduced(decoded_frames, 24).ok_or_else(|| {
        CoreError::new(
            ErrorCode::TimingMismatch,
            "raw H3 payload has zero duration",
        )
        .at_tensor("video")
    })?;
    Ok(TimingDescriptor {
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
    })
}

fn h3_tensor_descriptors(preflight: &H3SafetensorsPreflight) -> Vec<TensorDescriptor> {
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
    tensors
}

fn h3_authoring_provenance(
    authoring: AuthoringProvenance,
    payload_digest: Sha256Digest,
) -> Provenance {
    let created_by = authoring.created_by.unwrap_or_else(|| AuthoringProducer {
        name: "latentdeck-pack".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    });
    let source_kind = authoring
        .source_kind
        .unwrap_or_else(|| "raw_h3_safetensors".to_owned());
    Provenance {
        created_by: ProducerDescriptor {
            name: Identifier(created_by.name),
            version: created_by.version,
        },
        created_at: authoring.created_at,
        sources: vec![ProvenanceSource {
            kind: Identifier(source_kind),
            sha256: Some(payload_digest),
            uri: None,
            license: None,
            metadata: authoring.source_metadata,
        }],
    }
}

fn parse_authoring_provenance(
    encoded: Option<&str>,
    limits: &ValidationLimits,
) -> CoreResult<AuthoringProvenance> {
    let Some(encoded) = encoded else {
        return Ok(AuthoringProvenance::default());
    };
    if encoded.len() > limits.max_manifest_bytes() {
        return Err(CoreError::new(
            ErrorCode::ManifestTooLarge,
            "authoring provenance exceeds the LC manifest byte limit",
        )
        .at_json("/provenance"));
    }
    let mut deserializer = serde_json::Deserializer::from_str(encoded);
    let parsed =
        serde_path_to_error::deserialize(&mut deserializer).map_err(authoring_deserialize_error)?;
    deserializer.end().map_err(|error| {
        CoreError::new(
            ErrorCode::ManifestJsonInvalid,
            "authoring provenance must contain exactly one JSON object",
        )
        .at_json("/provenance")
        .with_source(error)
    })?;
    Ok(parsed)
}

fn authoring_deserialize_error(error: serde_path_to_error::Error<serde_json::Error>) -> CoreError {
    let path = error.path().to_string();
    let source = error.into_inner();
    let detail = source.to_string();
    let (code, field) = if let Some(field) = serde_error_field(&detail, "duplicate field `") {
        (ErrorCode::ManifestDuplicateKey, Some(field))
    } else if let Some(field) = serde_error_field(&detail, "unknown field `") {
        (ErrorCode::ManifestUnknownField, Some(field))
    } else {
        (ErrorCode::ManifestJsonInvalid, None)
    };
    CoreError::new(
        code,
        "authoring provenance does not match the strict options schema",
    )
    .at_json(authoring_json_pointer(&path, field.as_deref()))
    .with_source(source)
}

fn serde_error_field(detail: &str, marker: &str) -> Option<String> {
    let (_, suffix) = detail.split_once(marker)?;
    let (field, _) = suffix.split_once('`')?;
    Some(field.to_owned())
}

fn authoring_json_pointer(path: &str, field: Option<&str>) -> String {
    let mut segments: Vec<&str> = if path == "." || path.is_empty() {
        Vec::new()
    } else {
        path.split('.').collect()
    };
    if let Some(field) = field
        && segments.last().copied() != Some(field)
    {
        segments.push(field);
    }
    let mut pointer = String::from("/provenance");
    for segment in segments {
        pointer.push('/');
        pointer.push_str(&segment.replace('~', "~0").replace('/', "~1"));
    }
    pointer
}

fn deterministic_cartridge_id(payload_sha256: &str) -> CoreResult<String> {
    let digest = latentdeck_cartridge::hash::Sha256Hash::parse(payload_sha256)?;
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // RFC 9562 UUIDv8 reserves this version for application-defined payloads.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
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
    ))
}

fn decoded_axis(latent_axis: u64, axis_name: &str) -> CoreResult<u32> {
    let decoded = latent_axis.checked_mul(16).ok_or_else(|| {
        CoreError::new(
            ErrorCode::TensorSizeOverflow,
            format!("H3 decoded {axis_name} arithmetic overflow"),
        )
        .at_tensor("video")
    })?;
    u32::try_from(decoded).map_err(|error| {
        CoreError::new(
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

fn preview_descriptor(path: &str, limits: &ValidationLimits) -> CoreResult<PreviewDescriptor> {
    let mut file = File::open(path).map_err(|error| {
        CoreError::new(ErrorCode::IoOpen, "cannot open preview source")
            .at_entry("preview.webp")
            .with_source(error)
    })?;
    let maximum = limits.max_preview_bytes();
    let mut bytes = Vec::new();
    file.by_ref()
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            CoreError::new(ErrorCode::IoRead, "cannot read preview source")
                .at_entry("preview.webp")
                .with_source(error)
        })?;
    let byte_length = u64::try_from(bytes.len()).map_err(|error| {
        CoreError::new(
            ErrorCode::RuntimeLimitExceeded,
            "preview byte length does not fit u64",
        )
        .at_entry("preview.webp")
        .with_source(error)
    })?;
    if byte_length > maximum {
        return Err(CoreError::new(
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

fn write_options(overwrite: bool) -> WriteOptions {
    WriteOptions {
        overwrite: if overwrite {
            OverwritePolicy::Replace
        } else {
            OverwritePolicy::Forbid
        },
    }
}

fn pack_receipt_value(
    output_path: &Path,
    validation: &latentdeck_cartridge::reader::ValidationReceipt,
) -> Value {
    json!({
        "status": "ok",
        "command": "pack",
        "output": output_path,
        "validation": validation,
    })
}

fn preflight_value(preflight: &H3SafetensorsPreflight) -> Value {
    json!({
        "payload_bytes": preflight.payload_bytes,
        "header_bytes": preflight.header_bytes,
        "data_offset": preflight.data_offset,
        "data_bytes": preflight.data_bytes,
        "video": tensor_value(&preflight.video),
        "audio": preflight.audio.as_ref().map(tensor_value),
    })
}

fn tensor_value(descriptor: &SafetensorTensorDescriptor) -> Value {
    json!({
        "name": descriptor.name,
        "dtype": match descriptor.dtype {
            SafetensorDType::F16 => "F16",
            SafetensorDType::F32 => "F32",
        },
        "shape": descriptor.shape,
        "data_offsets": descriptor.data_offsets,
        "byte_length": descriptor.byte_length,
    })
}

fn into_py_error(py: Python<'_>, error: &CoreError) -> PyErr {
    let exception_type = py.get_type::<CartridgeError>();
    match exception_type.call1((
        error.code().to_owned(),
        error.detail.clone(),
        error.location.entry.clone(),
        error.location.tensor.clone(),
        error.location.json_pointer.clone(),
    )) {
        Ok(instance) => PyErr::from_value(instance.into_any()),
        Err(construction_error) => construction_error,
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<CartridgeError>()?;
    module.add_function(wrap_pyfunction!(inspect_json, module)?)?;
    module.add_function(wrap_pyfunction!(validate_json, module)?)?;
    module.add_function(wrap_pyfunction!(hash_json, module)?)?;
    module.add_function(wrap_pyfunction!(inspect_raw_h3_json, module)?)?;
    module.add_function(wrap_pyfunction!(read_h3, module)?)?;
    module.add_function(wrap_pyfunction!(read_raw_h3, module)?)?;
    module.add_function(wrap_pyfunction!(pack_json, module)?)?;
    module.add_function(wrap_pyfunction!(pack_raw_h3_json, module)?)?;
    module.add("BINDING_ABI_VERSION", BINDING_ABI_VERSION)?;
    Ok(())
}
