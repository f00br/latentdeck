use std::{error::Error as StdError, fmt};

use serde::Serialize;
use thiserror::Error;

/// Stable machine-readable validation and I/O failure codes.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    IoOpen,
    IoRead,
    IoWrite,
    TargetExists,
    AtomicCommitFailed,
    PostwriteValidationFailed,
    ArchiveTooLarge,
    ArchiveMalformed,
    Zip64Required,
    ArchiveNoncanonical,
    EntryCountInvalid,
    EntryMissing,
    EntryUnexpected,
    EntryDuplicate,
    EntryUnsafePath,
    EntryEncrypted,
    EntryCompressed,
    EntryTooLarge,
    EntrySizeMismatch,
    EntryOverlap,
    ArchiveTrailingData,
    EntryCrcMismatch,
    ManifestTooLarge,
    ManifestNotUtf8,
    ManifestJsonInvalid,
    ManifestDuplicateKey,
    ManifestUnknownField,
    ManifestInvalid,
    UnsupportedSpecVersion,
    PayloadHashMismatch,
    SafetensorsHeaderTooLarge,
    SafetensorsInvalid,
    TensorMissing,
    TensorUnexpected,
    TensorDescriptorMismatch,
    TensorDtypeForbidden,
    TensorShapeInvalid,
    TensorSizeOverflow,
    TensorNonFinite,
    UnsupportedCodec,
    UnsupportedProfileVersion,
    TimingMismatch,
    DecodedGeometryMismatch,
    RuntimeLimitExceeded,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IoOpen => "io_open",
            Self::IoRead => "io_read",
            Self::IoWrite => "io_write",
            Self::TargetExists => "target_exists",
            Self::AtomicCommitFailed => "atomic_commit_failed",
            Self::PostwriteValidationFailed => "postwrite_validation_failed",
            Self::ArchiveTooLarge => "archive_too_large",
            Self::ArchiveMalformed => "archive_malformed",
            Self::Zip64Required => "zip64_required",
            Self::ArchiveNoncanonical => "archive_noncanonical",
            Self::EntryCountInvalid => "entry_count_invalid",
            Self::EntryMissing => "entry_missing",
            Self::EntryUnexpected => "entry_unexpected",
            Self::EntryDuplicate => "entry_duplicate",
            Self::EntryUnsafePath => "entry_unsafe_path",
            Self::EntryEncrypted => "entry_encrypted",
            Self::EntryCompressed => "entry_compressed",
            Self::EntryTooLarge => "entry_too_large",
            Self::EntrySizeMismatch => "entry_size_mismatch",
            Self::EntryOverlap => "entry_overlap",
            Self::ArchiveTrailingData => "archive_trailing_data",
            Self::EntryCrcMismatch => "entry_crc_mismatch",
            Self::ManifestTooLarge => "manifest_too_large",
            Self::ManifestNotUtf8 => "manifest_not_utf8",
            Self::ManifestJsonInvalid => "manifest_json_invalid",
            Self::ManifestDuplicateKey => "manifest_duplicate_key",
            Self::ManifestUnknownField => "manifest_unknown_field",
            Self::ManifestInvalid => "manifest_invalid",
            Self::UnsupportedSpecVersion => "unsupported_spec_version",
            Self::PayloadHashMismatch => "payload_hash_mismatch",
            Self::SafetensorsHeaderTooLarge => "safetensors_header_too_large",
            Self::SafetensorsInvalid => "safetensors_invalid",
            Self::TensorMissing => "tensor_missing",
            Self::TensorUnexpected => "tensor_unexpected",
            Self::TensorDescriptorMismatch => "tensor_descriptor_mismatch",
            Self::TensorDtypeForbidden => "tensor_dtype_forbidden",
            Self::TensorShapeInvalid => "tensor_shape_invalid",
            Self::TensorSizeOverflow => "tensor_size_overflow",
            Self::TensorNonFinite => "tensor_non_finite",
            Self::UnsupportedCodec => "unsupported_codec",
            Self::UnsupportedProfileVersion => "unsupported_profile_version",
            Self::TimingMismatch => "timing_mismatch",
            Self::DecodedGeometryMismatch => "decoded_geometry_mismatch",
            Self::RuntimeLimitExceeded => "runtime_limit_exceeded",
        }
    }

    /// Stable process status grouping used by the CLI contract.
    #[must_use]
    pub const fn exit_status(self) -> u8 {
        match self {
            Self::UnsupportedSpecVersion
            | Self::UnsupportedCodec
            | Self::UnsupportedProfileVersion => 4,
            Self::IoOpen
            | Self::IoRead
            | Self::IoWrite
            | Self::TargetExists
            | Self::AtomicCommitFailed => 5,
            Self::PostwriteValidationFailed => 6,
            _ => 3,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Optional structured position within a cartridge.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ErrorLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tensor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_pointer: Option<String>,
}

impl fmt::Display for ErrorLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let Some(entry) = &self.entry {
            parts.push(format!("entry={entry}"));
        }
        if let Some(tensor) = &self.tensor {
            parts.push(format!("tensor={tensor}"));
        }
        if let Some(pointer) = &self.json_pointer {
            parts.push(format!("json_pointer={pointer}"));
        }
        if parts.is_empty() {
            formatter.write_str("cartridge")
        } else {
            formatter.write_str(&parts.join(","))
        }
    }
}

/// One public error type for Rust, CLI, and future Python binding parity.
#[derive(Debug, Error)]
#[error("{code} at {location}: {detail}")]
pub struct CartridgeError {
    pub code: ErrorCode,
    pub location: ErrorLocation,
    pub detail: String,
    #[source]
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl CartridgeError {
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            location: ErrorLocation::default(),
            detail: detail.into(),
            source: None,
        }
    }

    /// Stable `snake_case` machine code for CLI and binding surfaces.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code.as_str()
    }

    #[must_use]
    pub fn at_entry(mut self, entry: impl Into<String>) -> Self {
        self.location.entry = Some(entry.into());
        self
    }

    #[must_use]
    pub fn at_tensor(mut self, tensor: impl Into<String>) -> Self {
        self.location.tensor = Some(tensor.into());
        self
    }

    #[must_use]
    pub fn at_json(mut self, json_pointer: impl Into<String>) -> Self {
        self.location.json_pointer = Some(json_pointer.into());
        self
    }

    #[must_use]
    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }
}

pub type Result<T> = std::result::Result<T, CartridgeError>;
