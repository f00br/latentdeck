//! Strict discovery and integrity checks for separately installed codec packs.

use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{Read, Take};
use std::path::{Component, Path, PathBuf};

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const CODEC_PACK_MANIFEST_VERSION: &str = "1.0.0";
const INTEGRITY_CATALOG_VERSION: &str = "1.0.0";
const WORKER_PROTOCOL_VERSION: u16 = 1;
const LC_SPEC_VERSION: &str = "0.1.0";
const H3_CODEC_FAMILY: &str = "minimax_h3";
const H3_PROFILE: &str = "h3_av_latent";
const H3_PROFILE_VERSION: &str = "0.1.0";
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_PACK_IDS: usize = 64;
const MAX_VERSIONS_PER_PACK: usize = 16;
// Keep the launch-time validator identical to the pack builder, curator, and
// installer contract. The self-contained Windows PyTorch runtime legitimately
// contains more than 4,096 individually catalogued files.
const MAX_CATALOG_FILES: usize = 32_768;
const MAX_PACK_DIRECTORIES: usize = 131_072;
const PACK_CONTROL_FILES: usize = 2;
const MAX_ARGUMENTS: usize = 64;
const MAX_EXTERNAL_ASSETS: usize = 16;
const MAX_VARIANTS_PER_ASSET: usize = 32;

/// Stable machine-readable reasons a codec pack is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecPackErrorCode {
    RootInvalid,
    TooManyPacks,
    TooManyVersions,
    PackConflict,
    ManifestMissing,
    ManifestTooLarge,
    ManifestInvalid,
    PackIdentityMismatch,
    PackIncompatibleApp,
    PackIncompatibleProtocol,
    PackIncompatiblePlatform,
    PackIncompatibleProfile,
    PathUnsafe,
    ReparsePointForbidden,
    IntegrityCatalogInvalid,
    IntegrityFailed,
    ExternalAssetMissing,
    ExternalAssetIncompatible,
}

impl CodecPackErrorCode {
    /// Return the stable dotted state/error token used by the UI and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RootInvalid => "codec.root_invalid",
            Self::TooManyPacks => "codec.pack_limit_exceeded",
            Self::TooManyVersions => "codec.version_limit_exceeded",
            Self::PackConflict => "codec.pack_conflict",
            Self::ManifestMissing => "codec.pack_missing",
            Self::ManifestTooLarge => "codec.pack_manifest_too_large",
            Self::ManifestInvalid => "codec.pack_invalid",
            Self::PackIdentityMismatch => "codec.pack_identity_mismatch",
            Self::PackIncompatibleApp => "codec.pack_incompatible_app",
            Self::PackIncompatibleProtocol => "codec.pack_incompatible_protocol",
            Self::PackIncompatiblePlatform => "codec.pack_incompatible_platform",
            Self::PackIncompatibleProfile => "codec.pack_incompatible_profile",
            Self::PathUnsafe => "codec.pack_path_unsafe",
            Self::ReparsePointForbidden => "codec.pack_reparse_forbidden",
            Self::IntegrityCatalogInvalid => "codec.pack_catalog_invalid",
            Self::IntegrityFailed => "codec.pack_integrity_failed",
            Self::ExternalAssetMissing => "codec.asset_missing",
            Self::ExternalAssetIncompatible => "codec.asset_incompatible",
        }
    }
}

/// Codec pack validation failure without exposing machine-local paths.
#[derive(Debug, Error)]
#[error("{code}: {detail}")]
pub struct CodecPackError {
    pub code: &'static str,
    pub detail: String,
}

impl CodecPackError {
    fn new(code: CodecPackErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code: code.as_str(),
            detail: detail.into(),
        }
    }
}

/// Publisher identity recorded by the installed pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublisherDescriptor {
    pub name: String,
    pub url: Option<String>,
}

/// License and notice metadata for the installed pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LicenseDescriptor {
    pub spdx_or_label: String,
    pub notice_path: String,
}

/// Supported host platform.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlatformDescriptor {
    pub os: String,
    pub arch: String,
}

/// One LC codec/profile version range supported by a pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProfileCompatibility {
    pub codec_family: String,
    pub profile: String,
    pub profile_versions: Vec<String>,
}

/// Application and wire compatibility declared by a pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityDescriptor {
    pub app_min_inclusive: String,
    pub app_max_exclusive: String,
    pub worker_protocol_min: u16,
    pub worker_protocol_max: u16,
    pub lc_spec_versions: Vec<String>,
    pub profiles: Vec<ProfileCompatibility>,
}

/// Direct worker launch descriptor. Arguments are passed without a shell.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkerDescriptor {
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub probe_timeout_ms: u32,
}

/// Trusted adapter identity bundled by the pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterDescriptor {
    pub adapter_id: String,
    pub adapter_version: String,
}

/// Integrity catalog binding.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IntegrityDescriptor {
    pub catalog_path: String,
    pub catalog_sha256: String,
}

/// One accepted external weight variant. The bytes are never part of the pack.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalAssetVariant {
    pub variant_id: String,
    pub sha256: String,
    pub byte_length: u64,
    pub source_url: String,
    pub license_label: String,
    pub license_url: String,
}

/// Explicitly selected external asset requirement.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalAssetDescriptor {
    pub asset_id: String,
    pub display_name: String,
    pub kind: String,
    pub required: bool,
    pub selection: String,
    pub format: String,
    pub accepted_variants: Vec<ExternalAssetVariant>,
}

/// Strict `codec-pack.json` contract.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CodecPackManifest {
    pub manifest_version: String,
    pub pack_id: String,
    pub pack_version: String,
    pub display_name: String,
    pub publisher: PublisherDescriptor,
    pub license: LicenseDescriptor,
    pub platform: PlatformDescriptor,
    pub compatibility: CompatibilityDescriptor,
    pub worker: WorkerDescriptor,
    pub adapter: AdapterDescriptor,
    pub integrity: IntegrityDescriptor,
    pub external_assets: Vec<ExternalAssetDescriptor>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntegrityCatalog {
    manifest_version: String,
    files: Vec<IntegrityFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntegrityFile {
    path: String,
    byte_length: u64,
    sha256: String,
}

/// A pack whose manifest, compatibility, paths, and file catalog are verified.
#[derive(Debug, Clone)]
pub struct ValidatedCodecPack {
    pub root: PathBuf,
    pub manifest: CodecPackManifest,
    pub worker_executable: PathBuf,
    pub worker_working_directory: PathBuf,
}

/// An explicitly selected external model asset matched to a manifest variant.
#[derive(Debug, Clone)]
pub struct ValidatedExternalAsset {
    pub asset_id: String,
    pub variant_id: String,
    pub path: PathBuf,
    pub sha256: String,
    pub byte_length: u64,
}

/// Resolve the only two installation roots used by public builds.
#[must_use]
pub fn default_codec_pack_roots() -> Vec<PathBuf> {
    let mut roots = Vec::with_capacity(2);
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("LatentDeck").join("CodecPacks"));
    }
    if let Some(program_data) = env::var_os("PROGRAMDATA") {
        roots.push(
            PathBuf::from(program_data)
                .join("LatentDeck")
                .join("CodecPacks"),
        );
    }
    roots
}

/// Discover and fully validate installed codec packs in the supplied exact roots.
///
/// Missing roots are ignored. Any malformed or conflicting candidate blocks the
/// result so a damaged pack can never be silently skipped in favor of another.
///
/// # Errors
///
/// Returns a stable [`CodecPackError`] when a discovery root, manifest,
/// compatibility declaration, path, or integrity receipt is invalid.
pub fn discover_codec_packs(
    roots: &[PathBuf],
    app_version: &str,
) -> Result<Vec<ValidatedCodecPack>, CodecPackError> {
    let app_version = Version::parse(app_version).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::PackIncompatibleApp,
            "application version is not canonical SemVer",
        )
    })?;
    let mut identities = HashSet::new();
    let mut packs = Vec::new();
    for root in roots {
        if !root.exists() {
            continue;
        }
        reject_reparse(root)?;
        let pack_directories =
            bounded_directories(root, MAX_PACK_IDS, CodecPackErrorCode::TooManyPacks)?;
        for pack_directory in pack_directories {
            let directory_id = file_name_utf8(&pack_directory)?;
            validate_token(&directory_id, "pack directory")?;
            let version_directories = bounded_directories(
                &pack_directory,
                MAX_VERSIONS_PER_PACK,
                CodecPackErrorCode::TooManyVersions,
            )?;
            for version_directory in version_directories {
                let directory_version = file_name_utf8(&version_directory)?;
                Version::parse(&directory_version).map_err(|_| {
                    CodecPackError::new(
                        CodecPackErrorCode::PackIdentityMismatch,
                        "codec version directory is not canonical SemVer",
                    )
                })?;
                let pack = validate_codec_pack(&version_directory, &app_version)?;
                if pack.manifest.pack_id != directory_id
                    || pack.manifest.pack_version != directory_version
                {
                    return Err(CodecPackError::new(
                        CodecPackErrorCode::PackIdentityMismatch,
                        "manifest identity does not match its installation directories",
                    ));
                }
                let identity = (
                    pack.manifest.pack_id.clone(),
                    pack.manifest.pack_version.clone(),
                );
                if !identities.insert(identity) {
                    return Err(CodecPackError::new(
                        CodecPackErrorCode::PackConflict,
                        "the same codec pack identity exists in more than one discovery root",
                    ));
                }
                packs.push(pack);
            }
        }
    }
    packs.sort_by(|left, right| {
        (&left.manifest.pack_id, &left.manifest.pack_version)
            .cmp(&(&right.manifest.pack_id, &right.manifest.pack_version))
    });
    Ok(packs)
}

/// Validate one already isolated Codec Pack directory.
///
/// This is the narrow installer boundary: callers are responsible for placing
/// the candidate outside discovery and for checking its directory identity
/// before publication. The same manifest, compatibility, reparse-point,
/// physical-inventory, byte-length, and SHA-256 checks used by discovery are
/// applied here.
///
/// # Errors
///
/// Returns a stable [`CodecPackError`] when the application version is not
/// canonical `SemVer` or the candidate violates any Codec Pack contract.
pub fn validate_codec_pack_directory(
    root: &Path,
    app_version: &str,
) -> Result<ValidatedCodecPack, CodecPackError> {
    let app_version = Version::parse(app_version).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::PackIncompatibleApp,
            "application version is not canonical SemVer",
        )
    })?;
    validate_codec_pack(root, &app_version)
}

/// Validate one user-selected external asset against an installed pack.
///
/// The path is never inferred or scanned. The codec worker repeats the same
/// size and digest check immediately before GPU allocation.
///
/// # Errors
///
/// Returns a stable error if the asset is absent, linked through a reparse
/// point, not a regular file, unreadable, or not one of the accepted variants.
pub fn validate_external_asset(
    pack: &ValidatedCodecPack,
    asset_id: &str,
    path: impl AsRef<Path>,
) -> Result<ValidatedExternalAsset, CodecPackError> {
    let descriptor = pack
        .manifest
        .external_assets
        .iter()
        .find(|asset| asset.asset_id == asset_id)
        .ok_or_else(|| {
            CodecPackError::new(
                CodecPackErrorCode::ExternalAssetMissing,
                "codec pack does not declare the selected external asset",
            )
        })?;
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ExternalAssetMissing,
            "selected external asset is missing",
        )
    })?;
    reject_reparse_metadata(&metadata)?;
    if !metadata.is_file() {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ExternalAssetIncompatible,
            "selected external asset is not a regular file",
        ));
    }
    let measured = measure_path(path).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ExternalAssetIncompatible,
            "selected external asset cannot be measured",
        )
    })?;
    let variant = descriptor
        .accepted_variants
        .iter()
        .find(|variant| {
            variant.byte_length == measured.byte_length && variant.sha256 == measured.sha256
        })
        .ok_or_else(|| {
            CodecPackError::new(
                CodecPackErrorCode::ExternalAssetIncompatible,
                "selected external asset does not match an accepted variant",
            )
        })?;
    let canonical = fs::canonicalize(path).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ExternalAssetIncompatible,
            "selected external asset cannot be canonicalized",
        )
    })?;
    Ok(ValidatedExternalAsset {
        asset_id: descriptor.asset_id.clone(),
        variant_id: variant.variant_id.clone(),
        path: canonical,
        sha256: measured.sha256,
        byte_length: measured.byte_length,
    })
}

fn validate_codec_pack(
    root: &Path,
    app_version: &Version,
) -> Result<ValidatedCodecPack, CodecPackError> {
    reject_reparse(root)?;
    let root = fs::canonicalize(root).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::RootInvalid,
            "codec pack root cannot be canonicalized",
        )
    })?;
    let manifest_path = root.join("codec-pack.json");
    let manifest_bytes = read_bounded_json(&manifest_path, CodecPackErrorCode::ManifestMissing)?;
    let manifest: CodecPackManifest = serde_json::from_slice(&manifest_bytes).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "codec-pack.json is not a strict manifest object",
        )
    })?;
    validate_manifest(&manifest, app_version)?;

    let catalog_path = resolve_pack_path(&root, &manifest.integrity.catalog_path, true)?;
    let measured_catalog = measure_path(&catalog_path)?;
    if measured_catalog.sha256 != manifest.integrity.catalog_sha256 {
        return Err(CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "integrity catalog hash does not match codec-pack.json",
        ));
    }
    let catalog_bytes =
        read_bounded_json(&catalog_path, CodecPackErrorCode::IntegrityCatalogInvalid)?;
    let catalog: IntegrityCatalog = serde_json::from_slice(&catalog_bytes).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::IntegrityCatalogInvalid,
            "integrity catalog is not a strict JSON object",
        )
    })?;
    validate_catalog(&root, &catalog_path, &catalog)?;

    let worker_executable = resolve_pack_path(&root, &manifest.worker.executable, true)?;
    let worker_working_directory =
        resolve_pack_path(&root, &manifest.worker.working_directory, false)?;
    let notice_path = resolve_pack_path(&root, &manifest.license.notice_path, true)?;
    require_catalog_path(&catalog, &root, &worker_executable)?;
    require_catalog_path(&catalog, &root, &notice_path)?;

    Ok(ValidatedCodecPack {
        root,
        manifest,
        worker_executable,
        worker_working_directory,
    })
}

fn validate_manifest(
    manifest: &CodecPackManifest,
    app_version: &Version,
) -> Result<(), CodecPackError> {
    if manifest.manifest_version != CODEC_PACK_MANIFEST_VERSION {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "unsupported codec-pack manifest version",
        ));
    }
    validate_token(&manifest.pack_id, "pack_id")?;
    validate_token(&manifest.adapter.adapter_id, "adapter_id")?;
    let pack_version = Version::parse(&manifest.pack_version).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "pack_version is not canonical SemVer",
        )
    })?;
    if pack_version.to_string() != manifest.pack_version
        || Version::parse(&manifest.adapter.adapter_version).is_err()
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "pack or adapter version is not canonical SemVer",
        ));
    }
    let app_min = Version::parse(&manifest.compatibility.app_min_inclusive).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "app_min_inclusive is not canonical SemVer",
        )
    })?;
    let app_max = Version::parse(&manifest.compatibility.app_max_exclusive).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "app_max_exclusive is not canonical SemVer",
        )
    })?;
    if app_min >= app_max || app_version < &app_min || app_version >= &app_max {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PackIncompatibleApp,
            "codec pack does not support this application version",
        ));
    }
    if manifest.compatibility.worker_protocol_min > WORKER_PROTOCOL_VERSION
        || manifest.compatibility.worker_protocol_max < WORKER_PROTOCOL_VERSION
        || manifest.compatibility.worker_protocol_min > manifest.compatibility.worker_protocol_max
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PackIncompatibleProtocol,
            "codec pack has no compatible worker protocol",
        ));
    }
    if manifest.platform.os != "windows" || manifest.platform.arch != "x86_64" {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PackIncompatiblePlatform,
            "codec pack is not for Windows x86_64",
        ));
    }
    let supports_profile = manifest
        .compatibility
        .lc_spec_versions
        .iter()
        .any(|version| version == LC_SPEC_VERSION)
        && manifest.compatibility.profiles.iter().any(|profile| {
            profile.codec_family == H3_CODEC_FAMILY
                && profile.profile == H3_PROFILE
                && profile
                    .profile_versions
                    .iter()
                    .any(|version| version == H3_PROFILE_VERSION)
        });
    if !supports_profile {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PackIncompatibleProfile,
            "codec pack does not support the LC 0.1 H3 profile",
        ));
    }
    validate_launch_and_assets(manifest)
}

fn validate_launch_and_assets(manifest: &CodecPackManifest) -> Result<(), CodecPackError> {
    if manifest.worker.arguments.len() > MAX_ARGUMENTS
        || manifest.external_assets.len() > MAX_EXTERNAL_ASSETS
        || manifest.worker.probe_timeout_ms == 0
        || manifest.worker.probe_timeout_ms > 120_000
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "codec pack collection or timeout limit is invalid",
        ));
    }
    safe_relative_path(&manifest.worker.executable)?;
    safe_relative_path(&manifest.worker.working_directory)?;
    safe_relative_path(&manifest.license.notice_path)?;
    safe_relative_path(&manifest.integrity.catalog_path)?;
    validate_sha256(&manifest.integrity.catalog_sha256)?;
    for argument in &manifest.worker.arguments {
        if argument.is_empty() || argument.len() > 4096 || argument.contains('\0') {
            return Err(CodecPackError::new(
                CodecPackErrorCode::ManifestInvalid,
                "worker argument is empty or exceeds its bounded text contract",
            ));
        }
    }
    for asset in &manifest.external_assets {
        validate_token(&asset.asset_id, "asset_id")?;
        if asset.kind != "decoder_weight"
            || asset.selection != "explicit_file"
            || asset.format != "safetensors"
            || asset.accepted_variants.is_empty()
            || asset.accepted_variants.len() > MAX_VARIANTS_PER_ASSET
        {
            return Err(CodecPackError::new(
                CodecPackErrorCode::ManifestInvalid,
                "external asset contract is unsupported",
            ));
        }
        let mut variants = HashSet::new();
        for variant in &asset.accepted_variants {
            validate_token(&variant.variant_id, "variant_id")?;
            validate_sha256(&variant.sha256)?;
            if variant.byte_length == 0 || !variants.insert(&variant.variant_id) {
                return Err(CodecPackError::new(
                    CodecPackErrorCode::ManifestInvalid,
                    "external asset variant is empty or duplicated",
                ));
            }
        }
    }
    Ok(())
}

fn validate_catalog(
    root: &Path,
    catalog_path: &Path,
    catalog: &IntegrityCatalog,
) -> Result<(), CodecPackError> {
    if catalog.manifest_version != INTEGRITY_CATALOG_VERSION
        || !valid_catalog_file_count(catalog.files.len())
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::IntegrityCatalogInvalid,
            "integrity catalog version or file count is invalid",
        ));
    }
    let mut paths = HashSet::new();
    paths.insert("codec-pack.json".to_owned());
    paths.insert(portable_pack_path(root, catalog_path)?.to_ascii_lowercase());
    for entry in &catalog.files {
        if !paths.insert(entry.path.to_ascii_lowercase()) {
            return Err(CodecPackError::new(
                CodecPackErrorCode::IntegrityCatalogInvalid,
                "integrity catalog contains a duplicate or control-file path",
            ));
        }
        validate_sha256(&entry.sha256)?;
        let path = resolve_pack_path(root, &entry.path, true)?;
        let measured = measure_path(&path)?;
        if measured.byte_length != entry.byte_length || measured.sha256 != entry.sha256 {
            return Err(CodecPackError::new(
                CodecPackErrorCode::IntegrityFailed,
                "codec pack file does not match its integrity catalog",
            ));
        }
    }
    validate_physical_pack_inventory(root, &paths)?;
    Ok(())
}

fn validate_physical_pack_inventory(
    root: &Path,
    expected: &HashSet<String>,
) -> Result<(), CodecPackError> {
    let maximum_files = MAX_CATALOG_FILES + PACK_CONTROL_FILES;
    let mut actual = HashSet::new();
    let mut pending = vec![root.to_path_buf()];
    let mut directory_count = 0_usize;

    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|_| {
            CodecPackError::new(
                CodecPackErrorCode::IntegrityFailed,
                "codec pack directory cannot be read",
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|_| {
                CodecPackError::new(
                    CodecPackErrorCode::IntegrityFailed,
                    "codec pack contains an unreadable filesystem entry",
                )
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|_| {
                CodecPackError::new(
                    CodecPackErrorCode::IntegrityFailed,
                    "codec pack entry metadata cannot be read",
                )
            })?;
            reject_reparse_metadata(&metadata)?;
            if metadata.is_dir() {
                directory_count = directory_count.checked_add(1).ok_or_else(|| {
                    CodecPackError::new(
                        CodecPackErrorCode::IntegrityFailed,
                        "codec pack directory count overflowed",
                    )
                })?;
                if directory_count > MAX_PACK_DIRECTORIES {
                    return Err(CodecPackError::new(
                        CodecPackErrorCode::IntegrityFailed,
                        "codec pack directory count exceeds its bounded limit",
                    ));
                }
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(CodecPackError::new(
                    CodecPackErrorCode::IntegrityFailed,
                    "codec pack contains a non-file filesystem entry",
                ));
            }
            let portable = portable_pack_path(root, &path)?.to_ascii_lowercase();
            if !actual.insert(portable) || actual.len() > maximum_files {
                return Err(CodecPackError::new(
                    CodecPackErrorCode::IntegrityFailed,
                    "codec pack physical file inventory is duplicated or oversized",
                ));
            }
        }
    }

    if actual != *expected {
        return Err(CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "codec pack physical file inventory differs from its integrity catalog",
        ));
    }
    Ok(())
}

fn portable_pack_path(root: &Path, path: &Path) -> Result<String, CodecPackError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::PathUnsafe,
            "codec pack path escaped its installation root",
        )
    })?;
    let portable = relative.to_str().ok_or_else(|| {
        CodecPackError::new(
            CodecPackErrorCode::PathUnsafe,
            "codec pack path is not portable UTF-8",
        )
    })?;
    Ok(portable.replace('\\', "/"))
}

fn valid_catalog_file_count(count: usize) -> bool {
    (1..=MAX_CATALOG_FILES).contains(&count)
}

fn require_catalog_path(
    catalog: &IntegrityCatalog,
    root: &Path,
    path: &Path,
) -> Result<(), CodecPackError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::PathUnsafe,
            "resolved pack path escaped its root",
        )
    })?;
    let portable = relative.to_string_lossy().replace('\\', "/");
    if !catalog.files.iter().any(|entry| entry.path == portable) {
        return Err(CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "required codec pack file is absent from the integrity catalog",
        ));
    }
    Ok(())
}

struct MeasuredFile {
    byte_length: u64,
    sha256: String,
}

fn measure_path(path: &Path) -> Result<MeasuredFile, CodecPackError> {
    let mut file = File::open(path).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "codec pack file cannot be opened",
        )
    })?;
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| {
            CodecPackError::new(
                CodecPackErrorCode::IntegrityFailed,
                "codec pack file cannot be read",
            )
        })?;
        if read == 0 {
            break;
        }
        byte_length = byte_length.checked_add(read as u64).ok_or_else(|| {
            CodecPackError::new(
                CodecPackErrorCode::IntegrityFailed,
                "codec pack file length overflowed",
            )
        })?;
        hasher.update(&buffer[..read]);
    }
    Ok(MeasuredFile {
        byte_length,
        sha256: hex::encode(hasher.finalize()),
    })
}

fn read_bounded_json(
    path: &Path,
    missing_code: CodecPackErrorCode,
) -> Result<Vec<u8>, CodecPackError> {
    let file = File::open(path).map_err(|_| {
        CodecPackError::new(missing_code, "required codec pack JSON file is missing")
    })?;
    let mut bytes = Vec::new();
    let mut bounded: Take<File> = file.take(MAX_JSON_BYTES + 1);
    bounded.read_to_end(&mut bytes).map_err(|_| {
        CodecPackError::new(missing_code, "required codec pack JSON file cannot be read")
    })?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ManifestTooLarge,
            "codec pack JSON exceeds one MiB",
        ));
    }
    Ok(bytes)
}

fn bounded_directories(
    root: &Path,
    limit: usize,
    limit_code: CodecPackErrorCode,
) -> Result<Vec<PathBuf>, CodecPackError> {
    let mut directories = Vec::new();
    let entries = fs::read_dir(root).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::RootInvalid,
            "codec discovery root cannot be read",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            CodecPackError::new(
                CodecPackErrorCode::RootInvalid,
                "codec discovery root contains an unreadable entry",
            )
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
            CodecPackError::new(
                CodecPackErrorCode::RootInvalid,
                "codec discovery entry metadata cannot be read",
            )
        })?;
        reject_reparse_metadata(&metadata)?;
        if metadata.is_dir() {
            directories.push(entry.path());
            if directories.len() > limit {
                return Err(CodecPackError::new(
                    limit_code,
                    "codec discovery collection exceeds its bounded limit",
                ));
            }
        }
    }
    directories.sort();
    Ok(directories)
}

fn resolve_pack_path(root: &Path, relative: &str, file: bool) -> Result<PathBuf, CodecPackError> {
    let relative = safe_relative_path(relative)?;
    reject_reparse_components(root, &relative)?;
    let candidate = root.join(relative);
    let metadata = fs::symlink_metadata(&candidate).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "required codec pack path is missing",
        )
    })?;
    reject_reparse_metadata(&metadata)?;
    if (file && !metadata.is_file()) || (!file && !metadata.is_dir()) {
        return Err(CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "codec pack path has the wrong filesystem type",
        ));
    }
    let root = fs::canonicalize(root).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::RootInvalid,
            "codec pack root cannot be canonicalized",
        )
    })?;
    let canonical = fs::canonicalize(&candidate).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::IntegrityFailed,
            "codec pack path cannot be canonicalized",
        )
    })?;
    if !canonical.starts_with(&root) {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PathUnsafe,
            "codec pack path escaped its installation root",
        ));
    }
    Ok(canonical)
}

fn reject_reparse_components(root: &Path, relative: &Path) -> Result<(), CodecPackError> {
    let mut current = root.to_path_buf();
    reject_reparse(&current)?;
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(CodecPackError::new(
                CodecPackErrorCode::PathUnsafe,
                "codec pack path contains a non-normal component",
            ));
        };
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|_| {
            CodecPackError::new(
                CodecPackErrorCode::IntegrityFailed,
                "required codec pack path component is missing",
            )
        })?;
        reject_reparse_metadata(&metadata)?;
    }
    Ok(())
}

fn safe_relative_path(value: &str) -> Result<PathBuf, CodecPackError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') || value.contains('\\') {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PathUnsafe,
            "codec pack path is empty, oversized, or non-portable",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::PathUnsafe,
            "codec pack path must contain only relative normal components",
        ));
    }
    Ok(path.to_path_buf())
}

fn validate_sha256(value: &str) -> Result<(), CodecPackError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            "SHA-256 must be canonical lowercase hexadecimal",
        ));
    }
    Ok(())
}

fn validate_token(value: &str, label: &str) -> Result<(), CodecPackError> {
    if value.is_empty()
        || value.len() > 128
        || !value.split('.').all(|segment| {
            !segment.is_empty()
                && segment.as_bytes()[0].is_ascii_lowercase()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ManifestInvalid,
            format!("{label} is not a canonical lowercase token"),
        ));
    }
    Ok(())
}

fn file_name_utf8(path: &Path) -> Result<String, CodecPackError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| {
            CodecPackError::new(
                CodecPackErrorCode::PackIdentityMismatch,
                "codec pack directory name is not UTF-8",
            )
        })
}

fn reject_reparse(path: &Path) -> Result<(), CodecPackError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CodecPackError::new(
            CodecPackErrorCode::RootInvalid,
            "codec pack path metadata cannot be read",
        )
    })?;
    reject_reparse_metadata(&metadata)
}

#[cfg(windows)]
fn reject_reparse_metadata(metadata: &fs::Metadata) -> Result<(), CodecPackError> {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ReparsePointForbidden,
            "codec pack discovery refuses reparse points",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_reparse_metadata(metadata: &fs::Metadata) -> Result<(), CodecPackError> {
    if metadata.file_type().is_symlink() {
        return Err(CodecPackError::new(
            CodecPackErrorCode::ReparsePointForbidden,
            "codec pack discovery refuses symbolic links",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_CATALOG_FILES, valid_catalog_file_count};

    #[test]
    fn catalog_file_bound_matches_the_physical_packaging_contract() {
        assert!(!valid_catalog_file_count(0));
        assert!(valid_catalog_file_count(4_978));
        assert!(valid_catalog_file_count(MAX_CATALOG_FILES));
        assert!(!valid_catalog_file_count(MAX_CATALOG_FILES + 1));
    }
}
