//! Deterministic and atomic LC writer.

use std::fs::{File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crc32fast::Hasher as Crc32;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempPath};

use crate::archive::{EntryWrite, write_canonical};
use crate::error::{CartridgeError, ErrorCode, Result};
use crate::hash::Sha256Hash;
use crate::limits::ValidationLimits;
use crate::manifest::ManifestV0_1;
use crate::preview::inspect_webp;
use crate::profile::h3;
use crate::reader::{
    IntegrityValidationReceipt, ValidationOptions, ValidationReceipt,
    crosscheck_integrity_tensor_descriptors, open_integrity_validated, open_validated,
};
use crate::safetensor::{EntryRange, preflight_safetensors, scan_safetensors_finite};

const PREVIEW_ENTRY: &str = "preview.webp";
const SOURCE_BUFFER_BYTES: usize = 64 * 1024;

/// Serializes a manifest using the RFC 8785 JSON Canonicalization Scheme.
///
/// # Errors
///
/// Returns a manifest error if a value cannot be represented by canonical JSON.
pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_jcs::to_vec(value).map_err(|error| {
        CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "manifest cannot be represented as RFC 8785 canonical JSON",
        )
        .with_source(error)
    })
}

/// Finalized logical inputs for one cartridge.
#[derive(Debug, Clone)]
pub struct PackRequest {
    pub manifest: ManifestV0_1,
    pub payload_path: PathBuf,
    pub preview_path: Option<PathBuf>,
}

impl PackRequest {
    /// Creates a visual or AV request without a preview.
    pub fn new(manifest: ManifestV0_1, payload_path: impl Into<PathBuf>) -> Self {
        Self {
            manifest,
            payload_path: payload_path.into(),
            preview_path: None,
        }
    }

    /// Attaches the optional preview source described by the manifest.
    #[must_use]
    pub fn with_preview(mut self, preview_path: impl Into<PathBuf>) -> Self {
        self.preview_path = Some(preview_path.into());
        self
    }
}

/// Whether an existing output may be replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    Forbid,
    Replace,
}

/// Atomic writer behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    pub overwrite: OverwritePolicy,
}

impl Default for WriteOptions {
    fn default() -> Self {
        Self {
            overwrite: OverwritePolicy::Forbid,
        }
    }
}

/// Result of a committed and post-write-validated cartridge.
#[derive(Debug, Clone)]
pub struct WriteReceipt {
    pub output_path: PathBuf,
    pub validation: ValidationReceipt,
}

/// Result of a committed cartridge validated only at the codec-neutral layer.
#[derive(Debug, Clone)]
pub struct IntegrityWriteReceipt {
    pub output_path: PathBuf,
    pub validation: IntegrityValidationReceipt,
}

/// Writes, validates, and atomically commits one deterministic cartridge.
///
/// # Errors
///
/// Returns a stable error for invalid inputs, source mutation, output I/O,
/// failed post-write validation, or a forbidden existing target.
pub fn pack_atomic(
    request: &PackRequest,
    output: impl AsRef<Path>,
    options: &WriteOptions,
) -> Result<WriteReceipt> {
    let output = output.as_ref();
    validate_output_target(output, options.overwrite)?;
    h3::validate(&request.manifest, &ValidationLimits::default())?;
    let mut sources = prepare_sources(request)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let partial = write_partial(parent, &mut sources)?;
    let validation = validate_and_commit(partial, output, options.overwrite)?;
    Ok(WriteReceipt {
        output_path: output.to_path_buf(),
        validation,
    })
}

/// Writes and atomically commits one codec-neutral LC envelope.
///
/// This is the Core finalization boundary for a trusted codec adapter's staged
/// Safetensors payload. It applies no codec/profile semantics; the caller must
/// already hold a cross-checked profile receipt for those staged bytes.
///
/// # Errors
///
/// Returns a stable error for generic LC/schema/Safetensors/hash failures,
/// source mutation, failed post-write validation, or a forbidden target.
pub fn pack_integrity_atomic(
    request: &PackRequest,
    output: impl AsRef<Path>,
    options: &WriteOptions,
) -> Result<IntegrityWriteReceipt> {
    let output = output.as_ref();
    validate_output_target(output, options.overwrite)?;
    let mut sources = prepare_sources(request)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let partial = write_partial(parent, &mut sources)?;
    let validation = validate_and_commit_integrity(partial, output, options.overwrite)?;
    Ok(IntegrityWriteReceipt {
        output_path: output.to_path_buf(),
        validation,
    })
}

fn validate_output_target(output: &Path, overwrite: OverwritePolicy) -> Result<()> {
    if output.extension().and_then(|extension| extension.to_str()) != Some("lc") {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "cartridge output must use the .lc extension",
        ));
    }
    if overwrite == OverwritePolicy::Forbid && output.exists() {
        return Err(CartridgeError::new(
            ErrorCode::TargetExists,
            "output cartridge already exists",
        ));
    }
    Ok(())
}

struct PreparedSources {
    payload_file: File,
    payload_measurement: SourceMeasurement,
    preview_file: Option<File>,
    preview_measurement: Option<SourceMeasurement>,
    payload_entry: String,
    manifest_bytes: Vec<u8>,
}

fn prepare_sources(request: &PackRequest) -> Result<PreparedSources> {
    let limits = ValidationLimits::default();
    request.manifest.validate_common(&limits)?;
    let payload_descriptor = request.manifest.payloads.first().ok_or_else(|| {
        CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "manifest has no payload descriptor",
        )
        .at_json("/payloads")
    })?;
    if request.manifest.payloads.len() != 1
        || payload_descriptor.media_type != "application/vnd.safetensors"
    {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "manifest must describe exactly one Safetensors payload",
        )
        .at_json("/payloads"));
    }
    let payload_entry = payload_descriptor.path.clone();

    let mut payload_file = open_source(&request.payload_path)?;
    let payload_length = payload_file
        .metadata()
        .map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot inspect payload source")
                .at_entry(&payload_entry)
                .with_source(error)
        })?
        .len();
    let payload_range = EntryRange::new(0, payload_length);
    let payload_preflight =
        preflight_safetensors(&mut payload_file, payload_range, &payload_entry, &limits)?;
    scan_safetensors_finite(
        &mut payload_file,
        payload_range,
        &payload_entry,
        &payload_preflight,
    )?;
    crosscheck_integrity_tensor_descriptors(
        &request.manifest.tensors,
        &payload_entry,
        &payload_preflight,
    )?;
    let payload_measurement = measure_source(&mut payload_file, &payload_entry)?;
    verify_declared_source(
        payload_descriptor.byte_length,
        &payload_descriptor.sha256.0,
        payload_measurement,
        &payload_entry,
        "/payloads/0",
    )?;
    let (preview_file, preview_measurement) = prepare_preview(request, &limits)?;
    Ok(PreparedSources {
        payload_file,
        payload_measurement,
        preview_file,
        preview_measurement,
        payload_entry,
        manifest_bytes: canonical_json_bytes(&request.manifest)?,
    })
}

fn prepare_preview(
    request: &PackRequest,
    limits: &ValidationLimits,
) -> Result<(Option<File>, Option<SourceMeasurement>)> {
    let (descriptor, path) = match (&request.manifest.preview, &request.preview_path) {
        (None, None) => return Ok((None, None)),
        (Some(descriptor), Some(path)) => (descriptor, path),
        (Some(_), None) => {
            return Err(CartridgeError::new(
                ErrorCode::EntryMissing,
                "manifest preview has no source file",
            )
            .at_entry(PREVIEW_ENTRY));
        }
        (None, Some(_)) => {
            return Err(CartridgeError::new(
                ErrorCode::EntryUnexpected,
                "preview source has no manifest descriptor",
            )
            .at_entry(PREVIEW_ENTRY));
        }
    };
    let mut file = open_source(path)?;
    let measurement = measure_source(&mut file, PREVIEW_ENTRY)?;
    verify_declared_source(
        descriptor.byte_length,
        &descriptor.sha256.0,
        measurement,
        PREVIEW_ENTRY,
        "/preview",
    )?;
    if descriptor.path != PREVIEW_ENTRY || descriptor.media_type != "image/webp" {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "preview descriptor path or media type is invalid",
        )
        .at_json("/preview"));
    }
    let preview_bytes = read_source(&mut file, measurement.byte_length, PREVIEW_ENTRY)?;
    let info = inspect_webp(&preview_bytes, limits)?;
    if info.width != descriptor.width || info.height != descriptor.height {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "preview source dimensions differ from the manifest",
        )
        .at_json("/preview"));
    }
    Ok((Some(file), Some(measurement)))
}

fn write_partial(parent: &Path, sources: &mut PreparedSources) -> Result<TempPath> {
    let manifest_length = u64::try_from(sources.manifest_bytes.len()).map_err(|error| {
        CartridgeError::new(
            ErrorCode::ManifestTooLarge,
            "manifest length does not fit u64",
        )
        .with_source(error)
    })?;
    let mut partial = Builder::new()
        .prefix(".latentdeck-")
        .suffix(".partial")
        .tempfile_in(parent)
        .map_err(|error| {
            CartridgeError::new(ErrorCode::IoWrite, "cannot create same-directory partial")
                .with_source(error)
        })?;
    sources
        .payload_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot rewind payload source")
                .at_entry(&sources.payload_entry)
                .with_source(error)
        })?;
    let mut payload_copy = MeasuringReader::new(&mut sources.payload_file);
    let mut manifest_reader = Cursor::new(&sources.manifest_bytes);
    let mut preview_copy = sources.preview_file.as_mut().map(MeasuringReader::new);
    let mut entries = Vec::with_capacity(if preview_copy.is_some() { 3 } else { 2 });
    entries.push(EntryWrite::new(
        "manifest.json",
        manifest_length,
        crc32fast::hash(&sources.manifest_bytes),
        &mut manifest_reader,
    ));
    entries.push(EntryWrite::new(
        &sources.payload_entry,
        sources.payload_measurement.byte_length,
        sources.payload_measurement.crc32,
        &mut payload_copy,
    ));
    if let (Some(measurement), Some(copy)) = (sources.preview_measurement, preview_copy.as_mut()) {
        entries.push(EntryWrite::new(
            PREVIEW_ENTRY,
            measurement.byte_length,
            measurement.crc32,
            copy,
        ));
    }
    write_canonical(partial.as_file_mut(), &mut entries)?;
    drop(entries);
    verify_copied_source(
        payload_copy.finish(),
        sources.payload_measurement,
        &sources.payload_entry,
    )?;
    if let (Some(expected), Some(copy)) = (sources.preview_measurement, preview_copy) {
        verify_copied_source(copy.finish(), expected, PREVIEW_ENTRY)?;
    }
    partial.as_file().sync_all().map_err(|error| {
        CartridgeError::new(ErrorCode::IoWrite, "cannot synchronize partial cartridge")
            .with_source(error)
    })?;
    Ok(partial.into_temp_path())
}

fn verify_copied_source(
    copied: CopiedMeasurement,
    expected: SourceMeasurement,
    entry: &str,
) -> Result<()> {
    if copied.byte_length != expected.byte_length || copied.sha256 != expected.sha256 {
        return Err(CartridgeError::new(
            ErrorCode::PayloadHashMismatch,
            "source changed while the cartridge was written",
        )
        .at_entry(entry));
    }
    Ok(())
}

fn validate_and_commit(
    partial: TempPath,
    output: &Path,
    overwrite: OverwritePolicy,
) -> Result<ValidationReceipt> {
    let validated = open_validated(&partial, &ValidationOptions::default()).map_err(|error| {
        CartridgeError::new(
            ErrorCode::PostwriteValidationFailed,
            "partial cartridge failed post-write validation",
        )
        .with_source(error)
    })?;
    let validation = validated.receipt().clone();
    drop(validated);
    persist_partial(partial, output, overwrite)?;
    Ok(validation)
}

fn validate_and_commit_integrity(
    partial: TempPath,
    output: &Path,
    overwrite: OverwritePolicy,
) -> Result<IntegrityValidationReceipt> {
    let validated =
        open_integrity_validated(&partial, &ValidationOptions::default()).map_err(|error| {
            CartridgeError::new(
                ErrorCode::PostwriteValidationFailed,
                "partial cartridge failed codec-neutral post-write validation",
            )
            .with_source(error)
        })?;
    let validation = validated.receipt().clone();
    drop(validated);
    persist_partial(partial, output, overwrite)?;
    Ok(validation)
}

fn persist_partial(partial: TempPath, output: &Path, overwrite: OverwritePolicy) -> Result<()> {
    let persist_result = match overwrite {
        OverwritePolicy::Forbid => partial.persist_noclobber(output),
        OverwritePolicy::Replace => partial.persist(output),
    };
    persist_result.map_err(|error| {
        let code = if error.error.kind() == std::io::ErrorKind::AlreadyExists {
            ErrorCode::TargetExists
        } else {
            ErrorCode::AtomicCommitFailed
        };
        CartridgeError::new(code, "cannot commit validated cartridge").with_source(error)
    })?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceMeasurement {
    byte_length: u64,
    sha256: Sha256Hash,
    crc32: u32,
}

fn measure_source(file: &mut File, entry: &str) -> Result<SourceMeasurement> {
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot rewind source")
            .at_entry(entry)
            .with_source(error)
    })?;
    let mut sha256 = Sha256::new();
    let mut crc32 = Crc32::new();
    let mut byte_length = 0_u64;
    let mut buffer = vec![0_u8; SOURCE_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            CartridgeError::new(ErrorCode::IoRead, "cannot measure source")
                .at_entry(entry)
                .with_source(error)
        })?;
        if read == 0 {
            break;
        }
        sha256.update(&buffer[..read]);
        crc32.update(&buffer[..read]);
        byte_length = byte_length
            .checked_add(u64::try_from(read).map_err(|error| {
                CartridgeError::new(ErrorCode::EntryTooLarge, "source read length exceeds u64")
                    .at_entry(entry)
                    .with_source(error)
            })?)
            .ok_or_else(|| {
                CartridgeError::new(ErrorCode::EntryTooLarge, "source length overflows u64")
                    .at_entry(entry)
            })?;
    }
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot rewind measured source")
            .at_entry(entry)
            .with_source(error)
    })?;
    Ok(SourceMeasurement {
        byte_length,
        sha256: Sha256Hash::from_bytes(sha256.finalize().into()),
        crc32: crc32.finalize(),
    })
}

fn verify_declared_source(
    declared_length: u64,
    declared_sha256: &str,
    measured: SourceMeasurement,
    entry: &str,
    json_pointer: &str,
) -> Result<()> {
    let expected = Sha256Hash::parse(declared_sha256)
        .map_err(|error| error.at_json(format!("{json_pointer}/sha256")))?;
    if measured.byte_length != declared_length || measured.sha256 != expected {
        return Err(CartridgeError::new(
            ErrorCode::PayloadHashMismatch,
            "source bytes do not match their manifest descriptor",
        )
        .at_entry(entry)
        .at_json(json_pointer));
    }
    Ok(())
}

fn read_source(file: &mut File, length: u64, entry: &str) -> Result<Vec<u8>> {
    let allocation = usize::try_from(length).map_err(|error| {
        CartridgeError::new(ErrorCode::EntryTooLarge, "source does not fit memory")
            .at_entry(entry)
            .with_source(error)
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot rewind source")
            .at_entry(entry)
            .with_source(error)
    })?;
    let mut bytes = vec![0_u8; allocation];
    file.read_exact(&mut bytes).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot read complete source")
            .at_entry(entry)
            .with_source(error)
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        CartridgeError::new(ErrorCode::IoRead, "cannot rewind source")
            .at_entry(entry)
            .with_source(error)
    })?;
    Ok(bytes)
}

fn open_source(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path).map_err(|error| {
        CartridgeError::new(ErrorCode::IoOpen, "cannot open cartridge source").with_source(error)
    })
}

struct MeasuringReader<'a> {
    inner: &'a mut File,
    sha256: Sha256,
    byte_length: u64,
}

impl<'a> MeasuringReader<'a> {
    fn new(inner: &'a mut File) -> Self {
        Self {
            inner,
            sha256: Sha256::new(),
            byte_length: 0,
        }
    }

    fn finish(self) -> CopiedMeasurement {
        CopiedMeasurement {
            byte_length: self.byte_length,
            sha256: Sha256Hash::from_bytes(self.sha256.finalize().into()),
        }
    }
}

impl Read for MeasuringReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.sha256.update(&buffer[..read]);
        self.byte_length = self
            .byte_length
            .checked_add(u64::try_from(read).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("source length overflows u64"))?;
        Ok(read)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CopiedMeasurement {
    byte_length: u64,
    sha256: Sha256Hash,
}
