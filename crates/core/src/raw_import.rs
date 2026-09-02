//! Codec-neutral Core finalization for optional Codec Pack v2 raw import.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use latentdeck_cartridge::{
    LC_SPEC_VERSION,
    error::CartridgeError,
    hash::{Sha256Hash, hash_path},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, CartridgeId, CodecDescriptor, DType, DecodedVideoDescriptor, Identifier,
        ManifestV0_1, PayloadDescriptor, ProducerDescriptor, Provenance, ProvenanceSource,
        Rational, Sha256Digest, SpecVersion, TensorDescriptor, TensorStream, TimingDescriptor,
    },
    writer::{IntegrityWriteReceipt, PackRequest, WriteOptions, pack_integrity_atomic},
};
use latentdeck_control::v2::{
    MAX_RAW_IMPORT_SOURCE_BYTES, ProfileKey, RawImportArtifact, RawImportAudioPolicy,
    RawImportPreflight, RawImportStorageDtype, RawImportTensorStream, ValidationError,
};
use serde_json::Value;
use tempfile::{Builder, TempDir};
use thiserror::Error;
use uuid::Uuid;

/// Core-retained directory into which one trusted adapter may stage a payload.
///
/// The root is never supplied by the adapter. Dropping this value recursively
/// removes its own random directory, while adapter abort is limited to the one
/// file it created inside that directory.
#[derive(Debug)]
pub struct RawImportStagingRoot {
    // Keep this field before `directory`: fields are dropped in declaration
    // order, so the no-delete directory pin is released before TempDir removes
    // its owned tree.
    _directory_pin: File,
    directory: TempDir,
    canonical_path: PathBuf,
}

impl RawImportStagingRoot {
    /// Create a unique retained staging directory under a host-selected parent.
    ///
    /// # Errors
    ///
    /// Returns a stable staging error if the parent is unavailable or the
    /// unique directory cannot be created.
    pub fn create_in(parent: impl AsRef<Path>) -> Result<Self, RawImportError> {
        let parent = canonical_directory_without_reparse(parent.as_ref())?;
        let directory = Builder::new()
            .prefix(".latentdeck-raw-import-")
            .tempdir_in(&parent)
            .map_err(RawImportError::StagingCreate)?;
        let canonical_path = canonical_directory_without_reparse(directory.path())?;
        let directory_pin = open_pinned_staging_directory(&canonical_path)?;
        if canonical_directory_without_reparse(directory.path())? != canonical_path {
            return Err(RawImportError::StagingRootUnavailable);
        }
        Ok(Self {
            _directory_pin: directory_pin,
            directory,
            canonical_path,
        })
    }

    /// Absolute directory passed to `raw_import.stage`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.canonical_path
    }

    fn admit(&self, artifact: &RawImportArtifact) -> Result<PathBuf, RawImportError> {
        artifact.validate()?;
        let declared = Path::new(&artifact.staged_payload_path);
        if !declared.is_absolute() || declared.parent() != Some(self.canonical_path.as_path()) {
            return Err(RawImportError::StagedPathUntrusted);
        }
        if canonical_directory_without_reparse(self.directory.path())
            .map_err(|_| RawImportError::StagedPathUntrusted)?
            != self.canonical_path
        {
            return Err(RawImportError::StagedPathUntrusted);
        }
        let metadata =
            fs::symlink_metadata(declared).map_err(|_| RawImportError::StagedPathUntrusted)?;
        if !metadata.file_type().is_file() || metadata_is_reparse(&metadata) {
            return Err(RawImportError::StagedPathUntrusted);
        }
        let staged = declared
            .canonicalize()
            .map_err(|_| RawImportError::StagedPathUntrusted)?;
        if staged.parent() != Some(self.canonical_path.as_path()) {
            return Err(RawImportError::StagedPathUntrusted);
        }
        Ok(staged)
    }
}

/// Host-owned selection and source measurement expected from the adapter receipt.
///
/// These fields must come from trusted package selection and a host-side read of
/// the raw source. They are intentionally separate from adapter-authored data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImportExpectedAuthority {
    pack_id: String,
    pack_version: String,
    adapter_id: String,
    adapter_version: String,
    source_sha256: String,
    source_byte_length: u64,
    selected_profile: ProfileKey,
}

impl RawImportExpectedAuthority {
    /// Measure the raw source independently of the adapter and bind it to the
    /// exact trusted package selection made by the host.
    ///
    /// # Errors
    ///
    /// Rejects missing, empty, oversized, linked/reparse, or unstable source
    /// files before an adapter receipt can be accepted.
    pub fn measure_source(
        pack_id: impl Into<String>,
        pack_version: impl Into<String>,
        adapter_id: impl Into<String>,
        adapter_version: impl Into<String>,
        source: impl AsRef<Path>,
        selected_profile: ProfileKey,
    ) -> Result<Self, RawImportError> {
        let source = source.as_ref();
        let before =
            fs::symlink_metadata(source).map_err(|_| RawImportError::RawSourceUntrusted)?;
        if !before.is_file()
            || metadata_is_reparse(&before)
            || !(1..=MAX_RAW_IMPORT_SOURCE_BYTES).contains(&before.len())
        {
            return Err(RawImportError::RawSourceUntrusted);
        }
        let measured = hash_path(source).map_err(|_| RawImportError::RawSourceUntrusted)?;
        let after = fs::symlink_metadata(source).map_err(|_| RawImportError::RawSourceUntrusted)?;
        if !after.is_file()
            || metadata_is_reparse(&after)
            || before.len() != after.len()
            || before.modified().ok() != after.modified().ok()
            || measured.byte_length != after.len()
        {
            return Err(RawImportError::RawSourceUntrusted);
        }
        Ok(Self {
            pack_id: pack_id.into(),
            pack_version: pack_version.into(),
            adapter_id: adapter_id.into(),
            adapter_version: adapter_version.into(),
            source_sha256: measured.sha256.to_string(),
            source_byte_length: measured.byte_length,
            selected_profile,
        })
    }

    /// Cross-check one adapter preflight against the exact host-selected
    /// package, adapter, exact profile, and independently measured source.
    ///
    /// # Errors
    ///
    /// Returns [`RawImportError::AuthorityMismatch`] when any adapter-authored
    /// identity differs from host authority. Callers may use this before
    /// displaying preflight metadata; finalization repeats the same check.
    pub fn validate_preflight(&self, preflight: &RawImportPreflight) -> Result<(), RawImportError> {
        preflight.validate()?;
        if self.matches(preflight) {
            Ok(())
        } else {
            Err(RawImportError::AuthorityMismatch)
        }
    }

    /// Remeasure the source after adapter staging and require the exact same
    /// host-owned bytes and package/profile authority observed before preflight.
    ///
    /// # Errors
    ///
    /// Rejects a source that was replaced, rewritten, linked, resized, or made
    /// unavailable while the adapter was producing its staged payload.
    pub fn validate_source_unchanged(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<(), RawImportError> {
        let current = Self::measure_source(
            self.pack_id.clone(),
            self.pack_version.clone(),
            self.adapter_id.clone(),
            self.adapter_version.clone(),
            source,
            self.selected_profile.clone(),
        )?;
        if current == *self {
            Ok(())
        } else {
            Err(RawImportError::RawSourceUntrusted)
        }
    }

    fn matches(&self, preflight: &RawImportPreflight) -> bool {
        self.pack_id == preflight.pack_id
            && self.pack_version == preflight.pack_version
            && self.adapter_id == preflight.adapter_id
            && self.adapter_version == preflight.adapter_version
            && self.source_sha256 == preflight.source_sha256
            && self.source_byte_length == preflight.source_byte_length
            && self.selected_profile == preflight.metadata.profile_key
    }
}

/// Host-owned identity and provenance added around adapter-staged bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct RawImportAuthoring {
    pub producer_name: String,
    pub producer_version: String,
    pub cartridge_id: Option<Uuid>,
    pub created_at: Option<String>,
    pub source_kind: String,
    pub source_metadata: Option<BTreeMap<String, Value>>,
}

impl RawImportAuthoring {
    #[must_use]
    pub fn new(producer_name: impl Into<String>, producer_version: impl Into<String>) -> Self {
        Self {
            producer_name: producer_name.into(),
            producer_version: producer_version.into(),
            cartridge_id: None,
            created_at: None,
            source_kind: "raw_codec_source".to_owned(),
            source_metadata: None,
        }
    }
}

/// Exact typed adapter receipts plus host-owned authoring fields.
#[derive(Debug, Clone, PartialEq)]
pub struct RawImportFinalizeRequest {
    pub expected: RawImportExpectedAuthority,
    pub preflight: RawImportPreflight,
    pub artifact: RawImportArtifact,
    pub authoring: RawImportAuthoring,
}

/// Committed LC identity and whether the admitted staged file was consumed.
#[derive(Debug)]
pub struct RawImportWriteReceipt {
    pub output_path: PathBuf,
    pub validation: latentdeck_cartridge::reader::IntegrityValidationReceipt,
    pub staged_payload_removed: bool,
}

impl From<(IntegrityWriteReceipt, bool)> for RawImportWriteReceipt {
    fn from((receipt, staged_payload_removed): (IntegrityWriteReceipt, bool)) -> Self {
        Self {
            output_path: receipt.output_path,
            validation: receipt.validation,
            staged_payload_removed,
        }
    }
}

#[derive(Debug, Error)]
pub enum RawImportError {
    #[error("raw import staging parent is unavailable")]
    StagingRootUnavailable,
    #[error("raw import staging root could not be created")]
    StagingCreate(#[source] std::io::Error),
    #[error("adapter staged payload is outside the retained Core root")]
    StagedPathUntrusted,
    #[error("raw import source is not a stable bounded regular file")]
    RawSourceUntrusted,
    #[error("raw import receipts do not bind the same operation")]
    ReceiptMismatch,
    #[error("adapter raw import receipt does not match host authority")]
    AuthorityMismatch,
    #[error("raw import metadata is not canonical")]
    MetadataInvalid,
    #[error(transparent)]
    Protocol(#[from] ValidationError),
    #[error(transparent)]
    Cartridge(#[from] CartridgeError),
}

impl RawImportError {
    #[must_use]
    pub fn stable_code(&self) -> &'static str {
        match self {
            Self::StagingRootUnavailable | Self::StagingCreate(_) => {
                "raw_import.staging_root_unavailable"
            }
            Self::StagedPathUntrusted => "raw_import.staged_path_untrusted",
            Self::RawSourceUntrusted => "raw_import.source_untrusted",
            Self::ReceiptMismatch => "raw_import.receipt_mismatch",
            Self::AuthorityMismatch => "raw_import.authority_mismatch",
            Self::MetadataInvalid => "raw_import.metadata_invalid",
            Self::Protocol(_) => "raw_import.protocol_invalid",
            Self::Cartridge(error) => error.code(),
        }
    }
}

/// Construct, reopen-validate, and atomically publish one imported `.lc`.
///
/// Core never accepts an adapter-authored manifest. It constructs the manifest
/// from bounded typed metadata, remeasures the admitted staged file, and lets
/// the generic LC writer cross-check every tensor descriptor against the actual
/// finite Safetensors payload before a no-clobber atomic commit.
///
/// # Errors
///
/// Returns a stable error for receipt mismatch, path escape/linking, payload
/// mutation, invalid typed metadata, output collision, or LC validation/write
/// failure. Failed finalization leaves the staged file for explicit abort.
pub fn finalize_raw_import_atomic(
    staging: &RawImportStagingRoot,
    request: &RawImportFinalizeRequest,
    output: impl AsRef<Path>,
) -> Result<RawImportWriteReceipt, RawImportError> {
    request.expected.validate_preflight(&request.preflight)?;
    request.artifact.validate()?;
    if request.preflight.import_id != request.artifact.import_id
        || request.preflight.receipt_id != request.artifact.receipt_id
    {
        return Err(RawImportError::ReceiptMismatch);
    }
    let staged = staging.admit(&request.artifact)?;
    let measured = hash_path(&staged)?;
    if measured.byte_length != request.artifact.payload_byte_length
        || measured.sha256.to_string() != request.artifact.payload_sha256
    {
        return Err(CartridgeError::new(
            latentdeck_cartridge::error::ErrorCode::PayloadHashMismatch,
            "staged raw import changed after adapter validation",
        )
        .into());
    }
    let manifest = build_manifest(request, measured.sha256, measured.byte_length)?;
    let receipt = pack_integrity_atomic(
        &PackRequest::new(manifest, &staged),
        output.as_ref(),
        &WriteOptions::default(),
    )?;
    let staged_payload_removed = fs::remove_file(&staged).is_ok();
    Ok((receipt, staged_payload_removed).into())
}

fn build_manifest(
    request: &RawImportFinalizeRequest,
    payload_sha256: Sha256Hash,
    payload_byte_length: u64,
) -> Result<ManifestV0_1, RawImportError> {
    let metadata = &request.preflight.metadata;
    metadata.validate()?;
    let frame_rate = exact_rational(
        metadata.frame_rate_numerator,
        metadata.frame_rate_denominator,
    )?;
    let duration = exact_rational(metadata.duration_numerator, metadata.duration_denominator)?;
    let tensors = metadata
        .tensors
        .as_slice()
        .iter()
        .map(|tensor| TensorDescriptor {
            stream: match tensor.stream {
                RawImportTensorStream::Visual => TensorStream::Visual,
                RawImportTensorStream::Audio => TensorStream::Audio,
            },
            name: Identifier(tensor.name.clone()),
            payload: metadata.payload_entry.clone(),
            storage_dtype: manifest_dtype(tensor.storage_dtype),
            runtime_dtype: manifest_dtype(tensor.runtime_dtype),
            shape: tensor.shape.as_slice().to_vec(),
        })
        .collect();
    let payload_sha256 = payload_sha256.to_string();
    let cartridge_id = request.authoring.cartridge_id.unwrap_or_else(Uuid::new_v4);
    let manifest = ManifestV0_1 {
        spec_version: SpecVersion(LC_SPEC_VERSION.to_owned()),
        cartridge_id: CartridgeId(cartridge_id.hyphenated().to_string()),
        codec: CodecDescriptor {
            family: Identifier(metadata.profile_key.codec_family.clone()),
            profile: Identifier(metadata.profile_key.profile.clone()),
            profile_version: SpecVersion(metadata.profile_key.profile_version.clone()),
        },
        payloads: vec![PayloadDescriptor {
            path: metadata.payload_entry.clone(),
            media_type: metadata.payload_media_type.clone(),
            byte_length: payload_byte_length,
            sha256: Sha256Digest(payload_sha256.clone()),
        }],
        tensors,
        timing: TimingDescriptor {
            contract: Identifier(metadata.timing_contract.clone()),
            contract_version: SpecVersion(metadata.timing_contract_version.clone()),
            decoded_video: DecodedVideoDescriptor {
                width: metadata.decoded_width,
                height: metadata.decoded_height,
                frame_count: metadata.decoded_frame_count,
                frame_rate,
                duration,
            },
        },
        audio: match metadata.audio_policy {
            RawImportAudioPolicy::SourceAbsent => AudioDisposition::SourceAbsent,
            RawImportAudioPolicy::PreservedSource => AudioDisposition::PreservedSource,
        },
        preview: None,
        provenance: Provenance {
            created_by: ProducerDescriptor {
                name: Identifier(request.authoring.producer_name.clone()),
                version: request.authoring.producer_version.clone(),
            },
            created_at: request.authoring.created_at.clone(),
            sources: vec![ProvenanceSource {
                kind: Identifier(request.authoring.source_kind.clone()),
                sha256: Some(Sha256Digest(request.preflight.source_sha256.clone())),
                uri: None,
                license: None,
                metadata: request.authoring.source_metadata.clone(),
            }],
        },
        parent_cartridges: Vec::new(),
        operation_history: Vec::new(),
    };
    manifest.validate_common(&ValidationLimits::default())?;
    Ok(manifest)
}

fn exact_rational(numerator: u64, denominator: u64) -> Result<Rational, RawImportError> {
    let Some(reduced) = Rational::reduced(numerator, denominator) else {
        return Err(RawImportError::MetadataInvalid);
    };
    if reduced.numerator != numerator || reduced.denominator != denominator {
        return Err(RawImportError::MetadataInvalid);
    }
    Ok(reduced)
}

const fn manifest_dtype(dtype: RawImportStorageDtype) -> DType {
    match dtype {
        RawImportStorageDtype::F16 => DType::F16,
        RawImportStorageDtype::F32 => DType::F32,
    }
}

fn canonical_directory_without_reparse(path: &Path) -> Result<PathBuf, RawImportError> {
    if !path.is_absolute() {
        return Err(RawImportError::StagingRootUnavailable);
    }
    for ancestor in path.ancestors() {
        let metadata =
            fs::symlink_metadata(ancestor).map_err(|_| RawImportError::StagingRootUnavailable)?;
        if !metadata.is_dir() || metadata_is_reparse(&metadata) {
            return Err(RawImportError::StagingRootUnavailable);
        }
    }
    path.canonicalize()
        .map_err(|_| RawImportError::StagingRootUnavailable)
}

fn open_pinned_staging_directory(path: &Path) -> Result<File, RawImportError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    }
    let directory = options
        .open(path)
        .map_err(|_| RawImportError::StagingRootUnavailable)?;
    let metadata = directory
        .metadata()
        .map_err(|_| RawImportError::StagingRootUnavailable)?;
    if !metadata.is_dir() || metadata_is_reparse(&metadata) {
        return Err(RawImportError::StagingRootUnavailable);
    }
    Ok(directory)
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}
