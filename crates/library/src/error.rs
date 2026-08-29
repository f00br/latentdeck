use std::{error::Error as StdError, fmt};

use serde::Serialize;

/// Stable machine-readable failures exposed by the library boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Database,
    UnsupportedSchema,
    Filesystem,
    CartridgeRejected,
    InvalidInput,
    ImportLimit,
    NotFound,
    Conflict,
    VirtualCollection,
}

impl ErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Database => "database",
            Self::UnsupportedSchema => "unsupported_schema",
            Self::Filesystem => "filesystem",
            Self::CartridgeRejected => "cartridge_rejected",
            Self::InvalidInput => "invalid_input",
            Self::ImportLimit => "import_limit",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::VirtualCollection => "virtual_collection",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Path-free public error. Raw I/O and database sources are deliberately not
/// retained because their messages can embed machine-local paths.
#[derive(Debug)]
pub struct LibraryError {
    pub code: ErrorCode,
    pub detail: &'static str,
    pub cartridge_code: Option<String>,
}

impl LibraryError {
    #[must_use]
    pub const fn new(code: ErrorCode, detail: &'static str) -> Self {
        Self {
            code,
            detail,
            cartridge_code: None,
        }
    }

    #[must_use]
    pub(crate) fn database(_source: rusqlite::Error) -> Self {
        Self::new(ErrorCode::Database, "library database operation failed")
    }

    #[must_use]
    pub(crate) fn cartridge(source: &latentdeck_cartridge::error::CartridgeError) -> Self {
        let cartridge_code = source.code().to_owned();
        let mut error = Self::new(ErrorCode::CartridgeRejected, "cartridge validation failed");
        error.cartridge_code = Some(cartridge_code);
        error
    }
}

impl fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.detail)
    }
}

impl StdError for LibraryError {}

pub type Result<T> = std::result::Result<T, LibraryError>;
