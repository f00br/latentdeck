use std::io;

/// Stable error classes shared by the library, CLI, and future Tauri commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArguments,
    ArchiveInvalid,
    ManifestInvalid,
    IntegrityFailed,
    PackageExists,
    PackageMissing,
    PackageActive,
    PackageDisabled,
    PackageUntrusted,
    LifecycleBusy,
    LifecycleConflict,
    Io,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArguments => "extension.invalid_arguments",
            Self::ArchiveInvalid => "extension.archive_invalid",
            Self::ManifestInvalid => "extension.manifest_invalid",
            Self::IntegrityFailed => "extension.integrity_failed",
            Self::PackageExists => "extension.package_exists",
            Self::PackageMissing => "extension.package_missing",
            Self::PackageActive => "extension.package_active",
            Self::PackageDisabled => "extension.package_disabled",
            Self::PackageUntrusted => "extension.package_untrusted",
            Self::LifecycleBusy => "extension.lifecycle_busy",
            Self::LifecycleConflict => "extension.lifecycle_conflict",
            Self::Io => "extension.io",
        }
    }

    #[must_use]
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::InvalidArguments => 10,
            Self::ArchiveInvalid | Self::ManifestInvalid | Self::IntegrityFailed => 20,
            Self::PackageExists => 30,
            Self::PackageMissing => 31,
            Self::PackageActive | Self::PackageDisabled => 50,
            Self::PackageUntrusted | Self::LifecycleBusy | Self::LifecycleConflict | Self::Io => 40,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{code}: {detail}", code = .code.as_str())]
pub struct ExtensionError {
    code: ErrorCode,
    detail: String,
}

impl ExtensionError {
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }

    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        self.code.exit_code()
    }

    pub(crate) fn io(code: ErrorCode, context: &str, error: &io::Error) -> Self {
        Self::new(code, format!("{context}: {error}"))
    }
}

pub type Result<T> = std::result::Result<T, ExtensionError>;
