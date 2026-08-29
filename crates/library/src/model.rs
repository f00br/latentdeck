use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const ALL_CARTRIDGES_ID: &str = "latentdeck.virtual.all";
pub const UNASSIGNED_ID: &str = "latentdeck.virtual.unassigned";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CartridgeKey(String);

impl CartridgeKey {
    #[must_use]
    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CollectionId(String);

impl CollectionId {
    #[must_use]
    pub fn new_unchecked(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn all_cartridges() -> Self {
        Self(ALL_CARTRIDGES_ID.to_owned())
    }

    #[must_use]
    pub fn unassigned() -> Self {
        Self(UNASSIGNED_ID.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn is_virtual(&self) -> bool {
        matches!(self.0.as_str(), ALL_CARTRIDGES_ID | UNASSIGNED_ID)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathState {
    Present,
    Missing,
    Invalid,
    ContentChanged,
}

impl PathState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Invalid => "invalid",
            Self::ContentChanged => "content_changed",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "present" => Some(Self::Present),
            "missing" => Some(Self::Missing),
            "invalid" => Some(Self::Invalid),
            "content_changed" => Some(Self::ContentChanged),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Present,
    Warning,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRecord {
    pub path_id: i64,
    pub path: PathBuf,
    pub state: PathState,
    pub warning_code: Option<String>,
    pub observed_archive_sha256: Option<String>,
    pub last_checked_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartridgeMetadata {
    pub cartridge_id: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub codec_family: String,
    pub codec_profile: String,
    pub codec_profile_version: String,
    pub timing_contract: String,
    pub timing_contract_version: String,
    pub decoded_width: u32,
    pub decoded_height: u32,
    pub decoded_frame_count: u64,
    pub frame_rate_numerator: u64,
    pub frame_rate_denominator: u64,
    pub duration_numerator: u64,
    pub duration_denominator: u64,
    pub audio_policy: String,
    pub has_preview: bool,
    pub manifest_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CartridgeRecord {
    pub key: CartridgeKey,
    pub metadata: CartridgeMetadata,
    pub favorite: bool,
    pub tags: Vec<String>,
    pub paths: Vec<PathRecord>,
    pub availability: Availability,
    pub import_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionRecord {
    pub id: CollectionId,
    pub name: String,
    pub position: Option<u32>,
    pub is_virtual: bool,
    pub member_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportDisposition {
    Added,
    AlreadyIndexed,
    AcceptedReplacement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    pub disposition: ImportDisposition,
    pub key: CartridgeKey,
    pub previous_key: Option<CartridgeKey>,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FolderImportOptions {
    pub recursive: bool,
    pub max_candidates: usize,
}

impl Default for FolderImportOptions {
    fn default() -> Self {
        Self {
            recursive: false,
            max_candidates: 4_096,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedImport {
    pub path: PathBuf,
    pub code: String,
    pub cartridge_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderImportReport {
    pub accepted: Vec<ImportResult>,
    pub rejected: Vec<RejectedImport>,
    pub ignored_non_cartridges: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReindexDisposition {
    Unchanged,
    Present,
    Missing,
    Invalid,
    ContentChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReindexResult {
    pub path_id: i64,
    pub expected_key: CartridgeKey,
    pub observed_key: Option<CartridgeKey>,
    pub disposition: ReindexDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryOptions {
    pub search: Option<String>,
    pub limit: usize,
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            search: None,
            limit: 1_000,
        }
    }
}
