//! Bounded LC inspection and full validation.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Take};
use std::path::Path;

use serde::Serialize;

use crate::archive::{ArchiveEntry, ArchiveIndex, inspect_canonical, verify_entry};
use crate::error::{CartridgeError, ErrorCode, Result};
use crate::hash::{MeasuredHash, Sha256Hash, hash_reader};
use crate::limits::{MAX_ARCHIVE_BYTES, ValidationLimits};
use crate::manifest::{DType, ManifestV0_1, TensorDescriptor, TensorStream, parse_manifest_json};
use crate::preview::inspect_webp;
use crate::profile::h3::{self, ValidatedH3Profile};
use crate::safetensor::{
    EntryRange, H3SafetensorsPreflight, SafetensorDType, SafetensorTensorDescriptor,
    preflight_h3_safetensors, scan_h3_safetensors_finite,
};
use crate::writer::canonical_json_bytes;

const MANIFEST_ENTRY: &str = "manifest.json";
const H3_PAYLOAD_ENTRY: &str = "payloads/h3.safetensors";
const PREVIEW_ENTRY: &str = "preview.webp";

/// How far validation progressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationLevel {
    Structure,
    Full,
}

/// Options for bounded structural inspection.
#[derive(Debug, Clone, Copy, Default)]
pub struct InspectOptions {
    pub limits: ValidationLimits,
}

/// Options for full validation.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidationOptions {
    pub limits: ValidationLimits,
}

/// Metadata produced without granting tensor access.
#[derive(Debug)]
pub struct CartridgeInspection {
    pub validation_level: ValidationLevel,
    pub archive_size: u64,
    pub manifest: ManifestV0_1,
    pub h3_profile: ValidatedH3Profile,
    pub safetensors: H3SafetensorsPreflight,
}

/// Evidence produced by full streaming validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReceipt {
    pub validation_level: ValidationLevel,
    pub archive_bytes: u64,
    pub archive_sha256: Sha256Hash,
    pub payload_bytes: u64,
    pub payload_sha256: Sha256Hash,
    pub visual_runtime_bytes: u64,
}

/// A cartridge whose retained handle passed full validation.
#[derive(Debug)]
pub struct ValidatedCartridge {
    file: File,
    archive: ArchiveIndex,
    manifest: ManifestV0_1,
    h3_profile: ValidatedH3Profile,
    safetensors: H3SafetensorsPreflight,
    receipt: ValidationReceipt,
}

impl ValidatedCartridge {
    /// Returns the strictly parsed manifest.
    #[must_use]
    pub const fn manifest(&self) -> &ManifestV0_1 {
        &self.manifest
    }

    /// Returns the selected and validated H3 profile.
    #[must_use]
    pub const fn h3_profile(&self) -> &ValidatedH3Profile {
        &self.h3_profile
    }

    /// Returns the validation evidence for this retained file handle.
    #[must_use]
    pub const fn receipt(&self) -> &ValidationReceipt {
        &self.receipt
    }

    /// Opens a bounded tensor stream from the already validated handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the tensor name is not part of the validated H3
    /// payload or the retained handle cannot seek to its data range.
    pub fn tensor_reader(&mut self, name: &str) -> Result<TensorReader<'_>> {
        let descriptor = match name {
            "video" => &self.safetensors.video,
            "audio" => self.safetensors.audio.as_ref().ok_or_else(|| {
                CartridgeError::new(ErrorCode::TensorMissing, "cartridge has no audio tensor")
                    .at_tensor("audio")
            })?,
            _ => {
                return Err(CartridgeError::new(
                    ErrorCode::TensorUnexpected,
                    "tensor is not part of the H3 profile",
                )
                .at_tensor(name));
            }
        };
        let payload = find_entry(&self.archive, H3_PAYLOAD_ENTRY)?;
        let data_area = payload
            .data_offset
            .checked_add(self.safetensors.data_offset)
            .ok_or_else(|| {
                CartridgeError::new(
                    ErrorCode::TensorSizeOverflow,
                    "tensor data area offset overflows u64",
                )
                .at_tensor(name)
            })?;
        let offset = data_area
            .checked_add(descriptor.data_offsets[0])
            .ok_or_else(|| {
                CartridgeError::new(
                    ErrorCode::TensorSizeOverflow,
                    "tensor data offset overflows u64",
                )
                .at_tensor(name)
            })?;
        self.file.seek(SeekFrom::Start(offset)).map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot seek to validated tensor")
                .at_tensor(name)
                .with_source(error)
        })?;
        Ok(TensorReader {
            inner: (&mut self.file).take(descriptor.byte_length),
        })
    }
}

/// Read-only bounded access to one fully validated tensor.
pub struct TensorReader<'a> {
    inner: Take<&'a mut File>,
}

impl Read for TensorReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buffer)
    }
}

/// Performs bounded archive, manifest, profile, and tensor-layout inspection.
///
/// # Errors
///
/// Returns a stable error when the path cannot be opened or any structural
/// contract is invalid.
pub fn inspect_path(
    path: impl AsRef<Path>,
    options: &InspectOptions,
) -> Result<CartridgeInspection> {
    let mut file = open_readonly(path.as_ref())?;
    inspect_file(&mut file, &options.limits).map(|state| state.inspection)
}

/// Fully validates a cartridge and retains the validated file handle.
///
/// # Errors
///
/// Returns a stable error for any structural, profile, checksum, hash, finite
/// value, or I/O failure.
pub fn open_validated(
    path: impl AsRef<Path>,
    options: &ValidationOptions,
) -> Result<ValidatedCartridge> {
    let mut file = open_readonly(path.as_ref())?;
    let state = inspect_file(&mut file, &options.limits)?;

    for entry in &state.archive.entries {
        verify_entry(&mut file, entry)?;
    }
    let payload_entry = find_entry(&state.archive, H3_PAYLOAD_ENTRY)?;
    scan_h3_safetensors_finite(
        &mut file,
        EntryRange::new(payload_entry.data_offset, payload_entry.size),
        &state.inspection.safetensors,
    )?;
    let measured_payload = hash_entry(&mut file, payload_entry)?;
    let declared_payload = &state.inspection.manifest.payloads[0];
    let expected_payload_hash = Sha256Hash::parse(&declared_payload.sha256.0)
        .map_err(|error| error.at_json("/payloads/0/sha256"))?;
    if measured_payload.byte_length != declared_payload.byte_length
        || measured_payload.sha256 != expected_payload_hash
    {
        return Err(CartridgeError::new(
            ErrorCode::PayloadHashMismatch,
            "H3 payload bytes do not match the manifest",
        )
        .at_entry(H3_PAYLOAD_ENTRY));
    }

    if let Some(preview) = &state.inspection.manifest.preview {
        let preview_entry = find_entry(&state.archive, PREVIEW_ENTRY)?;
        let measured_preview = hash_entry(&mut file, preview_entry)?;
        let expected_preview_hash = Sha256Hash::parse(&preview.sha256.0)
            .map_err(|error| error.at_json("/preview/sha256"))?;
        if measured_preview.byte_length != preview.byte_length
            || measured_preview.sha256 != expected_preview_hash
        {
            return Err(CartridgeError::new(
                ErrorCode::PayloadHashMismatch,
                "preview bytes do not match the manifest",
            )
            .at_entry(PREVIEW_ENTRY));
        }
    }

    file.seek(SeekFrom::Start(0)).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot rewind cartridge for hashing")
            .with_source(error)
    })?;
    let archive_hash = hash_reader(&mut file)?;
    let runtime_elements = state
        .inspection
        .safetensors
        .video
        .shape
        .iter()
        .try_fold(1_u64, |product, axis| product.checked_mul(*axis))
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "visual runtime element count overflows u64",
            )
            .at_tensor("video")
        })?;
    let visual_runtime_bytes = runtime_elements.checked_mul(2).ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::TensorSizeOverflow,
            "visual runtime byte estimate overflows u64",
        )
        .at_tensor("video")
    })?;
    let receipt = ValidationReceipt {
        validation_level: ValidationLevel::Full,
        archive_bytes: archive_hash.byte_length,
        archive_sha256: archive_hash.sha256,
        payload_bytes: measured_payload.byte_length,
        payload_sha256: measured_payload.sha256,
        visual_runtime_bytes,
    };

    Ok(ValidatedCartridge {
        file,
        archive: state.archive,
        manifest: state.inspection.manifest,
        h3_profile: state.inspection.h3_profile,
        safetensors: state.inspection.safetensors,
        receipt,
    })
}

struct InspectionState {
    archive: ArchiveIndex,
    inspection: CartridgeInspection,
}

fn inspect_file(file: &mut File, limits: &ValidationLimits) -> Result<InspectionState> {
    let archive = inspect_canonical(file, MAX_ARCHIVE_BYTES)?;
    let manifest_entry = find_entry(&archive, MANIFEST_ENTRY)?;
    let manifest_bytes = read_entry_bytes(file, manifest_entry)?;
    let manifest = parse_manifest_json(&manifest_bytes, limits)?;
    if canonical_json_bytes(&manifest)? != manifest_bytes {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "manifest.json is not RFC 8785 canonical JSON",
        )
        .at_entry(MANIFEST_ENTRY));
    }
    validate_archive_descriptors(&archive, &manifest)?;
    let h3_profile = h3::validate(&manifest, limits)?;
    let payload_entry = find_entry(&archive, H3_PAYLOAD_ENTRY)?;
    let safetensors = preflight_h3_safetensors(
        file,
        EntryRange::new(payload_entry.data_offset, payload_entry.size),
        limits,
    )?;
    crosscheck_tensor_descriptors(&manifest.tensors, &safetensors)?;

    if let Some(preview) = &manifest.preview {
        let preview_entry = find_entry(&archive, PREVIEW_ENTRY)?;
        let preview_bytes = read_entry_bytes(file, preview_entry)?;
        let info = inspect_webp(&preview_bytes, limits)?;
        if info.width != preview.width || info.height != preview.height {
            return Err(CartridgeError::new(
                ErrorCode::ManifestInvalid,
                "preview dimensions do not match its manifest descriptor",
            )
            .at_entry(PREVIEW_ENTRY)
            .at_json("/preview"));
        }
    }

    Ok(InspectionState {
        inspection: CartridgeInspection {
            validation_level: ValidationLevel::Structure,
            archive_size: archive.archive_size,
            manifest,
            h3_profile,
            safetensors,
        },
        archive,
    })
}

fn validate_archive_descriptors(archive: &ArchiveIndex, manifest: &ManifestV0_1) -> Result<()> {
    if manifest.payloads.len() != 1 {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "LC 0.1 requires exactly one payload descriptor",
        )
        .at_json("/payloads"));
    }
    let payload = &manifest.payloads[0];
    if payload.path != H3_PAYLOAD_ENTRY
        || payload.media_type != "application/vnd.safetensors"
        || find_entry(archive, H3_PAYLOAD_ENTRY)?.size != payload.byte_length
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "H3 payload descriptor does not match the archive entry",
        )
        .at_json("/payloads/0"));
    }

    match (
        &manifest.preview,
        archive
            .entries
            .iter()
            .find(|entry| entry.name == PREVIEW_ENTRY),
    ) {
        (None, None) => {}
        (Some(preview), Some(entry))
            if preview.path == PREVIEW_ENTRY
                && preview.media_type == "image/webp"
                && preview.byte_length == entry.size => {}
        (Some(_), None) => {
            return Err(CartridgeError::new(
                ErrorCode::EntryMissing,
                "manifest declares a preview but the entry is missing",
            )
            .at_entry(PREVIEW_ENTRY));
        }
        (None, Some(_)) => {
            return Err(CartridgeError::new(
                ErrorCode::EntryUnexpected,
                "preview entry has no manifest descriptor",
            )
            .at_entry(PREVIEW_ENTRY));
        }
        (Some(_), Some(_)) => {
            return Err(CartridgeError::new(
                ErrorCode::ManifestInvalid,
                "preview descriptor does not match its archive entry",
            )
            .at_json("/preview"));
        }
    }
    Ok(())
}

pub(crate) fn crosscheck_tensor_descriptors(
    manifest: &[TensorDescriptor],
    payload: &H3SafetensorsPreflight,
) -> Result<()> {
    let visual = exactly_one_manifest_tensor(manifest, TensorStream::Visual, "video")?;
    crosscheck_tensor(visual, &payload.video)?;
    let manifest_audio = manifest
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Audio);
    match (manifest_audio, payload.audio.as_ref()) {
        (None, None) => Ok(()),
        (Some(descriptor), Some(header)) => crosscheck_tensor(descriptor, header),
        _ => Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "manifest and Safetensors audio presence differ",
        )
        .at_tensor("audio")),
    }
}

fn exactly_one_manifest_tensor<'a>(
    tensors: &'a [TensorDescriptor],
    stream: TensorStream,
    name: &str,
) -> Result<&'a TensorDescriptor> {
    let mut found = tensors
        .iter()
        .filter(|tensor| tensor.stream == stream && tensor.name.0 == name);
    let descriptor = found.next().ok_or_else(|| {
        CartridgeError::new(ErrorCode::TensorMissing, "manifest tensor is missing").at_tensor(name)
    })?;
    if found.next().is_some() {
        return Err(CartridgeError::new(
            ErrorCode::TensorUnexpected,
            "manifest tensor is duplicated",
        )
        .at_tensor(name));
    }
    Ok(descriptor)
}

fn crosscheck_tensor(
    manifest: &TensorDescriptor,
    header: &SafetensorTensorDescriptor,
) -> Result<()> {
    let dtype = match header.dtype {
        SafetensorDType::F16 => DType::F16,
        SafetensorDType::F32 => DType::F32,
    };
    if manifest.name.0 != header.name
        || manifest.payload != H3_PAYLOAD_ENTRY
        || manifest.storage_dtype != dtype
        || manifest.shape != header.shape
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "manifest tensor does not match the Safetensors header",
        )
        .at_tensor(&header.name));
    }
    Ok(())
}

fn find_entry<'a>(archive: &'a ArchiveIndex, name: &str) -> Result<&'a ArchiveEntry> {
    archive
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            CartridgeError::new(ErrorCode::EntryMissing, "required archive entry is missing")
                .at_entry(name)
        })
}

fn read_entry_bytes(file: &mut File, entry: &ArchiveEntry) -> Result<Vec<u8>> {
    let allocation = usize::try_from(entry.size).map_err(|error| {
        CartridgeError::new(ErrorCode::EntryTooLarge, "entry does not fit memory")
            .at_entry(&entry.name)
            .with_source(error)
    })?;
    file.seek(SeekFrom::Start(entry.data_offset))
        .map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot seek to archive entry")
                .at_entry(&entry.name)
                .with_source(error)
        })?;
    let mut bytes = vec![0_u8; allocation];
    file.read_exact(&mut bytes).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot read complete archive entry")
            .at_entry(&entry.name)
            .with_source(error)
    })?;
    Ok(bytes)
}

fn hash_entry(file: &mut File, entry: &ArchiveEntry) -> Result<MeasuredHash> {
    file.seek(SeekFrom::Start(entry.data_offset))
        .map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot seek to entry for SHA-256")
                .at_entry(&entry.name)
                .with_source(error)
        })?;
    let mut bounded = (&mut *file).take(entry.size);
    let measured = hash_reader(&mut bounded)?;
    if measured.byte_length != entry.size {
        return Err(CartridgeError::new(
            ErrorCode::EntrySizeMismatch,
            "entry ended before its indexed size",
        )
        .at_entry(&entry.name));
    }
    Ok(measured)
}

fn open_readonly(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open cartridge").with_source(error)
    })
}
