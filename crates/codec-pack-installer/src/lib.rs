//! H3-specific setup adapter over the shared extension lifecycle.
//!
//! This crate intentionally owns no ZIP parsing, extraction, trust receipt,
//! staging, quarantine, or removal implementation. Those operations are
//! delegated to `latentdeck-extension-manager`; this wrapper only binds the
//! installer payload to the official H3 package identity and asset contract.

use std::fs;
use std::path::{Path, PathBuf};

use latentdeck_extension_manager::{
    BundledPackageEntry, BundledPackageIndex, ErrorCode, ExtensionError, ExtensionRoots,
    PackageKind, PackageManifest, PackageReference, RemoveOptions,
};

const PACK_ID: &str = "org.latentdeck.h3";
const ADAPTER_ID: &str = "org.latentdeck.h3";
const ADAPTER_VERSION: &str = "0.2.0";
const CODEC_FAMILY: &str = "minimax_h3";
const PROFILE: &str = "h3_av_latent";
const PROFILE_VERSION: &str = "0.1.0";
const TAEH3_ASSET_ID: &str = "taeh3";
const TAEH3_DISPLAY_NAME: &str = "TAEH3 decoder weight";
const TAEH3_SHA256: &str = "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13";
const TAEH3_BYTE_LENGTH: u64 = 22_709_752;
const TAEH3_SOURCE_URL: &str =
    "https://huggingface.co/madebyollin/taehv/resolve/main/taeh3.safetensors";
const TAEH3_LICENSE_LABEL: &str = "MIT";
const TAEH3_LICENSE_URL: &str = "https://github.com/madebyollin/taehv/blob/e743234f/LICENSE";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EmbeddedH3Authorization {
    pack_version: &'static str,
    archive_sha256: &'static str,
    archive_byte_length: u64,
}

include!(concat!(env!("OUT_DIR"), "/h3_authorization.rs"));

/// Stable process exit code for an invalid command-line contract.
pub const EXIT_INVALID_ARGUMENTS: u8 = 10;
/// Stable process exit code for an invalid archive or Codec Pack.
pub const EXIT_INVALID_PACK: u8 = 20;
/// Stable process exit code when the exact healthy version already exists.
pub const EXIT_ALREADY_INSTALLED: u8 = 30;
/// Stable process exit code when the exact version is absent.
pub const EXIT_NOT_INSTALLED: u8 = 31;
/// Stable process exit code for lifecycle conflicts, limits, or serialization.
pub const EXIT_CONFLICT: u8 = 40;
/// Stable process exit code when the exact package is active or in use.
pub const EXIT_IN_USE: u8 = 50;

/// The shared lifecycle error is preserved verbatim, including its stable
/// machine-readable code and CLI exit class.
pub type LifecycleError = ExtensionError;

/// Current-user roots consumed by the shared extension lifecycle.
#[derive(Debug, Clone)]
pub struct LifecycleRoots {
    extensions: ExtensionRoots,
}

impl LifecycleRoots {
    /// Adapt an explicit legacy `CodecPacks` discovery root for tests and
    /// embedding callers. The shared lifecycle derives all sibling roots from
    /// the containing `LatentDeck` directory.
    #[must_use]
    pub fn for_install_root(
        install_root: impl Into<PathBuf>,
        _other_scope_root: Option<PathBuf>,
    ) -> Self {
        let install_root = install_root.into();
        let base_root = install_root
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        Self {
            extensions: ExtensionRoots::for_base_root(base_root),
        }
    }

    /// Build the canonical current-user root. `ProgramData` remains accepted
    /// only so existing Setup command lines stay parse-compatible; Protocol 2
    /// packages are never installed into or trusted from the all-users scope.
    #[must_use]
    pub fn from_known_folders(
        local_app_data: impl Into<PathBuf>,
        _program_data: impl Into<PathBuf>,
    ) -> Self {
        Self {
            extensions: ExtensionRoots::from_local_app_data(local_app_data),
        }
    }

    /// Borrow the authoritative common roots for future Tauri integration.
    #[must_use]
    pub const fn extension_roots(&self) -> &ExtensionRoots {
        &self.extensions
    }
}

/// Archive selected by the H3 Setup wrapper.
///
/// Authorization is deliberately absent: exact allowed identity is embedded
/// into the helper at build time and cannot be supplied at runtime.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub archive_path: PathBuf,
}

/// Successful H3 install identity projected from the common receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReceipt {
    pub pack_version: String,
    pub destination: PathBuf,
    pub trust_receipt_path: PathBuf,
    pub archive_sha256: String,
    pub archive_length: u64,
    pub extracted_files: usize,
    pub extracted_bytes: u64,
}

/// Successful exact-version removal identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReceipt {
    pub removed_version: String,
    pub cleaned_quarantine: bool,
}

/// Successful exact-version verification against the helper's embedded build
/// authorization and the common lifecycle trust receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReceipt {
    pub pack_version: String,
    pub destination: PathBuf,
    pub archive_sha256: String,
    pub archive_length: u64,
    pub enabled: bool,
}

struct AuthorizedArchive {
    authorization: &'static EmbeddedH3Authorization,
    request: latentdeck_extension_manager::InstallRequest,
    index: BundledPackageIndex,
}

/// Install one exact immutable H3 `.ldcodec` through the common lifecycle.
///
/// # Errors
///
/// Returns a shared stable lifecycle error when byte identity, H3 metadata,
/// package validation, trust publication, or immutable installation fails.
pub fn install(
    roots: &LifecycleRoots,
    request: &InstallRequest,
) -> Result<InstallReceipt, LifecycleError> {
    let authorized = authorize_archive(request)?;
    let result = latentdeck_extension_manager::install_from_bundled_index(
        &roots.extensions,
        &authorized.request,
        &authorized.index,
    );
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(error) if error.code() == ErrorCode::PackageExists => {
            verify(roots, authorized.authorization.pack_version)?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    project_install_receipt(receipt, authorized.authorization)
}

/// Explicitly repair one exact H3 archive authorized by the helper build.
///
/// This is deliberately separate from `install`: an immutable existing
/// version is never replaced implicitly merely because Setup was rerun.
///
/// # Errors
///
/// Returns a shared stable lifecycle error when build authorization, active
/// usage, quarantine, replacement, or receipt rebinding fails.
pub fn repair(
    roots: &LifecycleRoots,
    request: &InstallRequest,
) -> Result<InstallReceipt, LifecycleError> {
    let authorized = authorize_archive(request)?;
    let receipt = latentdeck_extension_manager::repair_from_bundled_index(
        &roots.extensions,
        &authorized.request,
        &authorized.index,
    )?;
    project_install_receipt(receipt, authorized.authorization)
}

/// Verify one exact installed H3 version against the common closed tree and
/// the helper's immutable build authorization.
///
/// # Errors
///
/// Returns a shared stable lifecycle error for an absent, corrupt, untrusted,
/// or non-build-authorized exact version.
pub fn verify(roots: &LifecycleRoots, version: &str) -> Result<VerifyReceipt, LifecycleError> {
    let authorization = embedded_authorization_for_version(version)?;
    let package = h3_package_reference(version);
    let resolved = latentdeck_extension_manager::resolve_installed(&roots.extensions, &package)?;
    validate_h3_inspection(resolved.manifest(), version)?;
    let trust = resolved.trust_receipt();
    if trust.archive_sha256 != authorization.archive_sha256
        || trust.archive_byte_length != authorization.archive_byte_length
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "installed H3 receipt does not match the helper's embedded build authorization",
        ));
    }
    Ok(VerifyReceipt {
        pack_version: version.to_owned(),
        destination: resolved.root().to_path_buf(),
        archive_sha256: trust.archive_sha256.clone(),
        archive_length: trust.archive_byte_length,
        enabled: trust.enabled,
    })
}

fn authorize_archive(request: &InstallRequest) -> Result<AuthorizedArchive, LifecycleError> {
    let metadata = fs::metadata(&request.archive_path).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            format!("could not inspect H3 archive metadata: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(invalid_arguments("H3 archive path is not a regular file"));
    }
    let inspection = latentdeck_extension_manager::inspect(&request.archive_path, None)?;
    let authorization = embedded_authorization_for(&inspection)?;
    validate_h3_inspection(&inspection.manifest, authorization.pack_version)?;

    let package = h3_package_reference(authorization.pack_version);
    let bundled_index = BundledPackageIndex {
        index_version: "1.0.0".to_owned(),
        packages: vec![BundledPackageEntry {
            package,
            archive_sha256: authorization.archive_sha256.to_owned(),
        }],
    };
    Ok(AuthorizedArchive {
        authorization,
        request: latentdeck_extension_manager::InstallRequest {
            archive_path: request.archive_path.clone(),
            expected_sha256: authorization.archive_sha256.to_owned(),
        },
        index: bundled_index,
    })
}

fn project_install_receipt(
    receipt: latentdeck_extension_manager::InstallReceipt,
    authorization: &EmbeddedH3Authorization,
) -> Result<InstallReceipt, LifecycleError> {
    if receipt.inspection.archive_byte_length != authorization.archive_byte_length {
        return Err(invalid_pack(
            "installed H3 archive length does not match embedded authorization",
        ));
    }
    validate_h3_inspection(&receipt.inspection.manifest, authorization.pack_version)?;
    Ok(InstallReceipt {
        pack_version: authorization.pack_version.to_owned(),
        destination: receipt.destination,
        trust_receipt_path: receipt.trust_receipt_path,
        archive_sha256: receipt.inspection.archive_sha256,
        archive_length: receipt.inspection.archive_byte_length,
        extracted_files: receipt.inspection.file_count,
        extracted_bytes: receipt.inspection.extracted_byte_length,
    })
}

fn embedded_authorization_for(
    inspection: &latentdeck_extension_manager::InspectedPackage,
) -> Result<&'static EmbeddedH3Authorization, LifecycleError> {
    let PackageManifest::Codec(manifest) = &inspection.manifest else {
        return Err(package_untrusted());
    };
    EMBEDDED_H3_AUTHORIZATIONS
        .iter()
        .find(|authorization| {
            manifest.pack_id == PACK_ID
                && manifest.pack_version == authorization.pack_version
                && inspection.archive_sha256 == authorization.archive_sha256
                && inspection.archive_byte_length == authorization.archive_byte_length
        })
        .ok_or_else(package_untrusted)
}

fn embedded_authorization_for_version(
    version: &str,
) -> Result<&'static EmbeddedH3Authorization, LifecycleError> {
    EMBEDDED_H3_AUTHORIZATIONS
        .iter()
        .find(|authorization| authorization.pack_version == version)
        .ok_or_else(package_untrusted)
}

fn h3_package_reference(version: &str) -> PackageReference {
    PackageReference {
        kind: PackageKind::CodecPack,
        package_id: PACK_ID.to_owned(),
        package_version: version.to_owned(),
    }
}

/// Remove one exact disabled H3 package version through the common lifecycle.
///
/// # Errors
///
/// Returns a shared stable lifecycle error when the version is missing,
/// active, corrupt without authorization, or cannot be removed atomically.
pub fn uninstall(
    roots: &LifecycleRoots,
    version: &str,
    remove_corrupt: bool,
) -> Result<UninstallReceipt, LifecycleError> {
    let package = PackageReference {
        kind: PackageKind::CodecPack,
        package_id: PACK_ID.to_owned(),
        package_version: version.to_owned(),
    };
    let removed = latentdeck_extension_manager::remove(
        &roots.extensions,
        &package,
        RemoveOptions {
            allow_corrupt: remove_corrupt,
        },
    )?;
    Ok(UninstallReceipt {
        removed_version: removed.package_version,
        cleaned_quarantine: false,
    })
}

fn validate_h3_inspection(
    manifest: &PackageManifest,
    expected_version: &str,
) -> Result<(), LifecycleError> {
    let PackageManifest::Codec(manifest) = manifest else {
        return Err(invalid_pack("H3 Setup accepts only Codec Pack v2"));
    };
    if manifest.pack_id != PACK_ID || manifest.pack_version != expected_version {
        return Err(invalid_pack(
            "manifest identity does not match the requested H3 package version",
        ));
    }
    if manifest.adapter.adapter_id != ADAPTER_ID
        || manifest.adapter.adapter_version != ADAPTER_VERSION
    {
        return Err(invalid_pack(
            "H3 adapter identity must be org.latentdeck.h3 version 0.2.0",
        ));
    }
    if manifest.compatibility.worker_protocol != 2 || manifest.compatibility.codec_adapter_api != 1
    {
        return Err(invalid_pack(
            "H3 Codec Pack must use Worker Protocol 2 and Codec Adapter API 1",
        ));
    }
    let expected_profile = latentdeck_extension_manager::ProfileKey {
        codec_family: CODEC_FAMILY.to_owned(),
        profile: PROFILE.to_owned(),
        profile_version: PROFILE_VERSION.to_owned(),
    };
    if manifest.compatibility.profiles.as_slice() != [expected_profile] {
        return Err(invalid_pack(
            "H3 Codec Pack must declare only minimax_h3/h3_av_latent/0.1.0",
        ));
    }
    let [asset] = manifest.external_assets.as_slice() else {
        return Err(invalid_pack(
            "H3 Codec Pack must declare the one exact TAEH3 external asset",
        ));
    };
    if asset.asset_id != TAEH3_ASSET_ID
        || asset.display_name != TAEH3_DISPLAY_NAME
        || !asset.required
        || asset.byte_length != TAEH3_BYTE_LENGTH
        || asset.sha256 != TAEH3_SHA256
        || asset.source_url.as_deref() != Some(TAEH3_SOURCE_URL)
        || asset.license_label != TAEH3_LICENSE_LABEL
        || asset.license_url.as_deref() != Some(TAEH3_LICENSE_URL)
    {
        return Err(invalid_pack(
            "H3 Codec Pack TAEH3 external asset identity is not exact",
        ));
    }
    Ok(())
}

fn invalid_arguments(detail: impl Into<String>) -> LifecycleError {
    ExtensionError::new(ErrorCode::InvalidArguments, detail)
}

fn invalid_pack(detail: impl Into<String>) -> LifecycleError {
    ExtensionError::new(ErrorCode::ManifestInvalid, detail)
}

fn package_untrusted() -> LifecycleError {
    ExtensionError::new(
        ErrorCode::PackageUntrusted,
        "H3 archive identity is not present in the helper's embedded build authorization",
    )
}
