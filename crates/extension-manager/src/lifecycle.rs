use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use atomicwrites::replace_atomic;
use fs2::FileExt;
use semver::Version;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::archive::{
    PreparedPackage, ensure_existing_tree_safe, extract_prepared, is_reparse_or_symlink,
    prepare_archive, validate_directory,
};
use crate::error::{ErrorCode, ExtensionError, Result};
use crate::model::{
    ActiveInstalledPackage, BundledPackageIndex, CodecCapability, CompatibilityPair,
    CompatibilityReason, InstallReceipt, InstalledPackageSummary, PackageHealth, PackageKind,
    PackageManifest, PackageReference, TrustReceipt, ValidatedInstalledPackage,
};
use crate::schema::{
    MAX_JSON_BYTES, TRUST_RECEIPT_VERSION, canonical_json, is_reserved_package_id,
    parse_strict_json, validate_bundled_index, validate_package_reference, validate_sha256,
};

const MAX_PACKAGES_PER_KIND: usize = 256;
const MAX_VERSIONS_PER_PACKAGE: usize = 16;
const MAX_STALE_STAGING_ENTRIES: usize = 64;
const MAX_STALE_TRASH_ENTRIES: usize = 64;
const MAX_REMOVE_TREE_ENTRIES: usize = 131_072;
const MAX_REMOVE_TREE_DEPTH: usize = 256;
const FREE_SPACE_OVERHEAD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ExtensionRoots {
    pub base_root: PathBuf,
    pub decks_root: PathBuf,
    pub codec_packs_root: PathBuf,
    pub trust_root: PathBuf,
    pub usage_root: PathBuf,
    pub staging_root: PathBuf,
    pub trash_root: PathBuf,
    pub lock_path: PathBuf,
}

impl ExtensionRoots {
    #[must_use]
    pub fn from_local_app_data(local_app_data: impl Into<PathBuf>) -> Self {
        let base_root = local_app_data.into().join("LatentDeck");
        Self::for_base_root(base_root)
    }

    #[must_use]
    pub fn for_base_root(base_root: impl Into<PathBuf>) -> Self {
        let base_root = base_root.into();
        Self {
            decks_root: base_root.join("Decks"),
            codec_packs_root: base_root.join("CodecPacks"),
            trust_root: base_root.join("PackageTrust"),
            usage_root: base_root.join("PackageUsage"),
            staging_root: base_root.join("ExtensionStaging"),
            trash_root: base_root.join("ExtensionTrash"),
            lock_path: base_root.join("ExtensionLifecycle.lock"),
            base_root,
        }
    }

    #[must_use]
    pub fn package_root(&self, kind: PackageKind) -> &Path {
        match kind {
            PackageKind::DeckPack => &self.decks_root,
            PackageKind::CodecPack => &self.codec_packs_root,
        }
    }

    #[must_use]
    pub fn receipt_root(&self, kind: PackageKind) -> PathBuf {
        self.trust_root.join(kind.receipt_root_name())
    }
}

#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub archive_path: PathBuf,
    pub expected_sha256: String,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RemoveOptions {
    pub allow_corrupt: bool,
}

struct LifecycleLock(File);

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

struct TemporaryDirectoryGuard(Option<PathBuf>);

impl TemporaryDirectoryGuard {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TemporaryDirectoryGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let mut seen = 0;
            let _ = remove_tree_no_follow(&path, 0, &mut seen);
        }
    }
}

/// Install one immutable, initially disabled package version.
///
/// # Errors
///
/// Returns a stable error when roots, hash-bound preflight, extraction,
/// validation, atomic publication, or trust-receipt creation fails.
pub fn install(roots: &ExtensionRoots, request: &InstallRequest) -> Result<InstallReceipt> {
    validate_roots(roots)?;
    let mut prepared = prepare_authorized(request, None)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    publish_new(roots, &mut prepared, false)
}

/// Install one exact reserved-namespace package authorized by a
/// build-generated exact-hash index.
///
/// # Errors
///
/// Returns a stable error when the index is invalid, the package identity or
/// archive hash is absent from it, or the common lifecycle rejects the package.
pub fn install_from_bundled_index(
    roots: &ExtensionRoots,
    request: &InstallRequest,
    index: &BundledPackageIndex,
) -> Result<InstallReceipt> {
    validate_roots(roots)?;
    let mut prepared = prepare_authorized(request, Some(index))?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    publish_new(roots, &mut prepared, false)
}

/// Restore an exact disabled version from a hash-bound archive.
///
/// # Errors
///
/// Returns a stable error when the version is active or any preflight,
/// quarantine, extraction, validation, or atomic receipt operation fails.
pub fn repair(roots: &ExtensionRoots, request: &InstallRequest) -> Result<InstallReceipt> {
    validate_roots(roots)?;
    let prepared = prepare_authorized(request, None)?;
    repair_prepared(roots, prepared)
}

/// Repair one exact reserved-namespace package authorized by a
/// build-generated exact-hash index.
///
/// # Errors
///
/// Returns a stable error when authorization, active-version gating, repair,
/// or receipt rebinding fails.
pub fn repair_from_bundled_index(
    roots: &ExtensionRoots,
    request: &InstallRequest,
    index: &BundledPackageIndex,
) -> Result<InstallReceipt> {
    validate_roots(roots)?;
    let prepared = prepare_authorized(request, Some(index))?;
    repair_prepared(roots, prepared)
}

fn repair_prepared(
    roots: &ExtensionRoots,
    mut prepared: PreparedPackage,
) -> Result<InstallReceipt> {
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    let package = prepared.manifest.reference();
    let destination = package_destination(roots, &package);
    let receipt_path = trust_receipt_path(roots, &package);

    if !path_exists(&destination, "repair destination")? {
        return publish_new(roots, &mut prepared, true);
    }
    ensure_not_in_use_locked(roots, &package)?;
    if let Ok(receipt) = read_trust_receipt(&receipt_path)
        && receipt.enabled
        && trust_receipt_matches_prepared(&receipt, &prepared)
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageActive,
            "disable the exact package version before repair",
        ));
    }
    if let Ok(validated) = validate_directory(&destination, Some(package.kind))
        && validated.files == prepared.files
        && validated.manifest == prepared.manifest
    {
        let receipt = trust_from_prepared(&prepared, false)?;
        write_receipt_replace(&receipt_path, &receipt)?;
        return Ok(InstallReceipt {
            destination,
            trust_receipt_path: receipt_path,
            inspection: prepared.inspection.clone(),
        });
    }

    ensure_safe_directory(&roots.trash_root, true)?;
    let quarantine = roots
        .trash_root
        .join(format!(".repair-{}", Uuid::new_v4().simple()));
    fs::rename(&destination, &quarantine).map_err(|error| {
        let code = if is_in_use_error(&error) {
            ErrorCode::PackageActive
        } else {
            ErrorCode::LifecycleConflict
        };
        ExtensionError::io(code, "quarantine corrupt package for repair", &error)
    })?;
    match publish_new(roots, &mut prepared, true) {
        Ok(receipt) => {
            let mut seen = 0;
            remove_tree_no_follow(&quarantine, 0, &mut seen).map_err(|error| {
                ExtensionError::io(
                    ErrorCode::LifecycleConflict,
                    "remove repaired package quarantine",
                    &error,
                )
            })?;
            remove_if_empty(&roots.trash_root);
            Ok(receipt)
        }
        Err(error) => {
            if !destination.exists() {
                let _ = fs::rename(&quarantine, &destination);
            }
            Err(error)
        }
    }
}

fn prepare_authorized(
    request: &InstallRequest,
    bundled_index: Option<&BundledPackageIndex>,
) -> Result<PreparedPackage> {
    validate_sha256(&request.expected_sha256, "expected archive SHA-256")?;
    if let Some(index) = bundled_index {
        validate_bundled_index(index)?;
    }
    let kind = kind_from_path(&request.archive_path)?;
    let prepared = prepare_archive(
        &request.archive_path,
        Some(&request.expected_sha256),
        Some(kind),
    )?;
    let package = prepared.manifest.reference();
    if is_reserved_package_id(&package.package_id) {
        let Some(index) = bundled_index else {
            return Err(ExtensionError::new(
                ErrorCode::PackageUntrusted,
                "org.latentdeck.* is reserved and requires an exact build-generated index",
            ));
        };
        let authorized = index.packages.iter().any(|entry| {
            entry.package == package
                && entry.archive_sha256 == prepared.inspection.archive_sha256
                && entry.archive_sha256 == request.expected_sha256
        });
        if !authorized {
            return Err(ExtensionError::new(
                ErrorCode::PackageUntrusted,
                "reserved package identity and archive hash are absent from the bundled index",
            ));
        }
    } else if bundled_index.is_some() {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "bundled index authorization cannot be used for an external package identity",
        ));
    }
    Ok(prepared)
}

/// Verify the complete installed tree against its trust receipt.
///
/// # Errors
///
/// Returns a stable error when the package is absent, untrusted, unsafe, or
/// differs from its manifest, catalog, or receipt.
pub fn verify(
    roots: &ExtensionRoots,
    package: &PackageReference,
) -> Result<InstalledPackageSummary> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    verify_locked(roots, package)
}

/// Revalidate and resolve one exact installed package for a runtime boundary.
///
/// The returned type can only be constructed after the complete installed
/// tree, closed manifest, integrity catalog, and atomic trust receipt agree.
/// Enabled state is preserved in the receipt; runtime code must require it
/// before executing package code.
///
/// # Errors
///
/// Returns a stable error when the exact version is absent, unsafe,
/// untrusted, corrupt, or differs from its receipt.
pub fn resolve_installed(
    roots: &ExtensionRoots,
    package: &PackageReference,
) -> Result<ValidatedInstalledPackage> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    let destination = package_destination(roots, package);
    let (receipt, validated) = verify_installed_tree_locked(roots, package, &destination)?;
    Ok(ValidatedInstalledPackage::new(
        destination,
        validated.manifest,
        receipt,
    ))
}

/// Resolve one exact enabled package and hold its cross-process usage lease and
/// validated-tree handles.
///
/// The lease remains held until the returned value is dropped. Disabling an
/// active version therefore affects only future launches, while remove and
/// repair cannot tear files out from under an existing runtime session. The
/// exact validated file handles are remeasured before return; on Windows they
/// also deny write/delete sharing while normal runtime reads remain available.
/// This protects the already validated paths, not a same-user security
/// sandbox: Windows directory handles do not prevent creation of a new child
/// name. A future resolution rejects such additions through closed-tree
/// validation.
///
/// # Errors
///
/// Returns a stable error when validation fails, the exact version is
/// disabled, or its usage lease cannot be acquired.
pub fn resolve_active(
    roots: &ExtensionRoots,
    package: &PackageReference,
) -> Result<ActiveInstalledPackage> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    let destination = package_destination(roots, package);
    let (receipt, validated) = verify_installed_tree_locked(roots, package, &destination)?;
    if !receipt.enabled {
        return Err(ExtensionError::new(
            ErrorCode::PackageDisabled,
            "enable the exact package version before runtime use",
        ));
    }
    let usage_lock = open_usage_lock(roots, package)?;
    FileExt::try_lock_shared(&usage_lock).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "acquire package usage lease",
            &error,
        )
    })?;
    let retained_tree_handles = pin_validated_package_tree(roots, &destination, &validated.files)?;
    Ok(ActiveInstalledPackage::new(
        ValidatedInstalledPackage::new(destination, validated.manifest, receipt),
        usage_lock,
        retained_tree_handles,
    ))
}

fn pin_validated_package_tree(
    roots: &ExtensionRoots,
    destination: &Path,
    files: &BTreeMap<String, crate::archive::FileMeasurement>,
) -> Result<Vec<File>> {
    let mut directory_paths = BTreeSet::new();
    directory_paths.insert(roots.base_root.clone());
    directory_paths.insert(
        destination
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| {
                ExtensionError::new(
                    ErrorCode::LifecycleConflict,
                    "installed package has no kind root",
                )
            })?
            .to_path_buf(),
    );
    directory_paths.insert(
        destination
            .parent()
            .ok_or_else(|| {
                ExtensionError::new(
                    ErrorCode::LifecycleConflict,
                    "installed package has no identity root",
                )
            })?
            .to_path_buf(),
    );
    directory_paths.insert(destination.to_path_buf());
    for relative in files.keys() {
        let mut parent = PathBuf::new();
        let component_count = relative.split('/').count();
        for component in relative.split('/').take(component_count.saturating_sub(1)) {
            parent.push(component);
            directory_paths.insert(destination.join(&parent));
        }
    }
    let mut directory_paths: Vec<_> = directory_paths.into_iter().collect();
    directory_paths.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });

    let mut retained = Vec::with_capacity(directory_paths.len().saturating_add(files.len()));
    for directory in directory_paths {
        retained.push(open_pinned_directory(&directory)?);
    }
    for measurement in files.values() {
        let path = measurement
            .path
            .split('/')
            .fold(destination.to_path_buf(), |path, component| {
                path.join(component)
            });
        let mut file = open_pinned_file(&path)?;
        verify_pinned_file(&mut file, measurement)?;
        retained.push(file);
    }
    Ok(retained)
}

fn open_pinned_directory(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ExtensionError::io(
            ErrorCode::IntegrityFailed,
            "inspect package directory for pin",
            &error,
        )
    })?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package directory changed before it could be pinned",
        ));
    }
    ensure_existing_tree_safe(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    }
    let file = options.open(path).map_err(|error| {
        ExtensionError::io(
            ErrorCode::IntegrityFailed,
            "open package directory pin",
            &error,
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        ExtensionError::io(
            ErrorCode::IntegrityFailed,
            "inspect pinned package directory",
            &error,
        )
    })?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "opened package directory pin is not a regular directory",
        ));
    }
    Ok(file)
}

fn open_pinned_file(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ExtensionError::io(
            ErrorCode::IntegrityFailed,
            "inspect package file for pin",
            &error,
        )
    })?;
    if !metadata.is_file() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "catalogued package file changed before it could be pinned",
        ));
    }
    ensure_existing_tree_safe(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        ExtensionError::io(
            ErrorCode::IntegrityFailed,
            "open catalogued package file pin",
            &error,
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        ExtensionError::io(
            ErrorCode::IntegrityFailed,
            "inspect pinned package file",
            &error,
        )
    })?;
    if !metadata.is_file() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "opened package file pin is not a regular file",
        ));
    }
    Ok(file)
}

fn verify_pinned_file(file: &mut File, expected: &crate::archive::FileMeasurement) -> Result<()> {
    let observed_length = file
        .metadata()
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::IntegrityFailed,
                "inspect pinned package file",
                &error,
            )
        })?
        .len();
    if observed_length != expected.byte_length {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!(
                "catalogued file length changed before pin: {}",
                expected.path
            ),
        ));
    }
    let mut hasher = Sha256::new();
    let mut observed_length = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            ExtensionError::io(
                ErrorCode::IntegrityFailed,
                "hash pinned package file",
                &error,
            )
        })?;
        if read == 0 {
            break;
        }
        observed_length = observed_length.checked_add(read as u64).ok_or_else(|| {
            ExtensionError::new(ErrorCode::IntegrityFailed, "pinned file length overflowed")
        })?;
        if observed_length > expected.byte_length {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("catalogued file grew before pin: {}", expected.path),
            ));
        }
        hasher.update(&buffer[..read]);
    }
    let observed_sha256 = hex::encode(hasher.finalize());
    if observed_length != expected.byte_length || observed_sha256 != expected.sha256 {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!(
                "catalogued file bytes changed before pin: {}",
                expected.path
            ),
        ));
    }
    Ok(())
}

/// Explicitly enable one exact package version.
///
/// # Errors
///
/// Returns a stable error when verification fails or another version of the
/// same package is already enabled.
pub fn enable(roots: &ExtensionRoots, package: &PackageReference) -> Result<TrustReceipt> {
    set_enabled(roots, package, true)
}

/// Enable one exact healthy package only when no other installed version of
/// the same kind and ID exists at the locked decision point.
///
/// This is the atomic first-install default-selection primitive used by
/// build-authorized bundled packages. It never replaces explicit version
/// selection when any alternate version is present, even when that alternate
/// version is disabled or has a damaged receipt.
///
/// # Errors
///
/// Returns a stable conflict without mutating the trust receipt when another
/// installed version is present, or the normal verification/lifecycle error
/// when the exact version cannot be enabled safely.
pub fn enable_if_only_installed_version(
    roots: &ExtensionRoots,
    package: &PackageReference,
) -> Result<TrustReceipt> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    verify_locked(roots, package)?;

    let id_root = roots.package_root(package.kind).join(&package.package_id);
    ensure_safe_directory(&id_root, false)?;
    for entry in fs::read_dir(&id_root).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "inspect installed package versions",
            &error,
        )
    })? {
        let entry = entry.map_err(|error| {
            ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "inspect installed package version",
                &error,
            )
        })?;
        if entry.file_name() != std::ffi::OsStr::new(&package.package_version) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "another version of this package is installed; select a version explicitly",
            ));
        }
    }

    let receipt_path = trust_receipt_path(roots, package);
    let mut receipt = read_trust_receipt(&receipt_path)?;
    if receipt.enabled {
        return Ok(receipt);
    }
    receipt.enabled = true;
    write_receipt_replace(&receipt_path, &receipt)?;
    Ok(receipt)
}

/// Explicitly disable one exact package version.
///
/// # Errors
///
/// Returns a stable error when the package or its trust receipt cannot be
/// verified or the atomic receipt update fails.
pub fn disable(roots: &ExtensionRoots, package: &PackageReference) -> Result<TrustReceipt> {
    set_enabled(roots, package, false)
}

/// Remove one exact disabled package version without following links.
///
/// # Errors
///
/// Returns a stable error when the package is active, missing, corrupt without
/// explicit authorization, in use, or cannot be quarantined and removed.
pub fn remove(
    roots: &ExtensionRoots,
    package: &PackageReference,
    options: RemoveOptions,
) -> Result<PackageReference> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    let destination = package_destination(roots, package);
    if !path_exists(&destination, "package removal destination")? {
        return Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact package version is not installed",
        ));
    }
    let receipt_path = trust_receipt_path(roots, package);
    ensure_not_in_use_locked(roots, package)?;
    match verify_installed_tree_locked(roots, package, &destination) {
        Ok((receipt, _)) if receipt.enabled => {
            return Err(ExtensionError::new(
                ErrorCode::PackageActive,
                "disable the exact package version before removal",
            ));
        }
        Err(error) if !options.allow_corrupt => return Err(error),
        _ => {}
    }
    ensure_safe_directory(&roots.trash_root, true)?;
    let quarantine = roots
        .trash_root
        .join(format!(".remove-{}", Uuid::new_v4().simple()));
    fs::rename(&destination, &quarantine).map_err(|error| {
        let code = if is_in_use_error(&error) {
            ErrorCode::PackageActive
        } else {
            ErrorCode::LifecycleConflict
        };
        ExtensionError::io(code, "quarantine package for removal", &error)
    })?;
    if path_exists(&receipt_path, "trust receipt")?
        && let Err(error) = fs::remove_file(&receipt_path)
    {
        let _ = fs::rename(&quarantine, &destination);
        return Err(ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "remove trust receipt",
            &error,
        ));
    }
    let mut seen = 0;
    if let Err(error) = remove_tree_no_follow(&quarantine, 0, &mut seen) {
        return Err(ExtensionError::io(
            ErrorCode::PackageActive,
            "remove quarantined package",
            &error,
        ));
    }
    cleanup_package_parents(roots, package);
    remove_usage_lock(roots, package);
    remove_if_empty(&roots.trash_root);
    Ok(package.clone())
}

/// List bounded installed candidates while isolating individual corruption.
///
/// # Errors
///
/// Returns a stable error only when a lifecycle root itself is unsafe or its
/// bounded directory inventory cannot be inspected.
pub fn list(roots: &ExtensionRoots) -> Result<Vec<InstalledPackageSummary>> {
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    let mut packages = Vec::new();
    for kind in [PackageKind::DeckPack, PackageKind::CodecPack] {
        list_kind_locked(roots, kind, &mut packages)?;
    }
    packages.sort_by(|left, right| {
        (
            left.package.kind.archive_extension(),
            &left.package.package_id,
            &left.package.package_version,
        )
            .cmp(&(
                right.package.kind.archive_extension(),
                &right.package.package_id,
                &right.package.package_version,
            ))
    });
    Ok(packages)
}

/// Resolve every installed Deck-version by Codec-version pair.
///
/// # Errors
///
/// Returns a stable error when the installed roots cannot be safely listed or
/// inspected. Individual invalid packages become `package_invalid` pairs.
pub fn compatibility_matrix(roots: &ExtensionRoots) -> Result<Vec<CompatibilityPair>> {
    let summaries = list(roots)?;
    let decks: Vec<_> = summaries
        .iter()
        .filter(|item| item.package.kind == PackageKind::DeckPack)
        .collect();
    let codecs: Vec<_> = summaries
        .iter()
        .filter(|item| item.package.kind == PackageKind::CodecPack)
        .collect();
    let mut manifests = BTreeMap::new();
    for summary in &summaries {
        if summary.health != PackageHealth::Corrupt
            && let Ok(validated) = validate_directory(
                &package_destination(roots, &summary.package),
                Some(summary.package.kind),
            )
        {
            manifests.insert(summary.package.clone(), validated.manifest);
        }
    }
    let mut pairs = Vec::with_capacity(decks.len().saturating_mul(codecs.len()));
    for deck in decks {
        for codec in &codecs {
            let (reason, compatible_profile) = if deck.health == PackageHealth::Corrupt
                || codec.health == PackageHealth::Corrupt
            {
                (CompatibilityReason::PackageInvalid, None)
            } else if deck.health == PackageHealth::Untrusted
                || codec.health == PackageHealth::Untrusted
            {
                (CompatibilityReason::Untrusted, None)
            } else {
                match (manifests.get(&deck.package), manifests.get(&codec.package)) {
                    (
                        Some(PackageManifest::Deck(deck_manifest)),
                        Some(PackageManifest::Codec(codec_manifest)),
                    ) => resolve_pair(deck_manifest, codec_manifest),
                    _ => (CompatibilityReason::PackageInvalid, None),
                }
            };
            pairs.push(CompatibilityPair {
                deck: deck.package.clone(),
                codec: codec.package.clone(),
                reason,
                compatible_profile,
            });
        }
    }
    Ok(pairs)
}

fn publish_new(
    roots: &ExtensionRoots,
    prepared: &mut PreparedPackage,
    replace_receipt: bool,
) -> Result<InstallReceipt> {
    let package = prepared.manifest.reference();
    validate_package_reference(&package)?;
    let destination = package_destination(roots, &package);
    let package_parent = destination
        .parent()
        .expect("package destination has parent");
    ensure_safe_directory(roots.package_root(package.kind), true)?;
    if !path_exists(package_parent, "package identity root")? {
        enforce_package_id_limit(roots.package_root(package.kind))?;
    }
    ensure_safe_directory(package_parent, true)?;
    if path_exists(&destination, "package installation destination")? {
        verify_installed_tree_locked(roots, &package, &destination)?;
        return Err(ExtensionError::new(
            ErrorCode::PackageExists,
            "immutable package version is already installed",
        ));
    }
    enforce_version_limit(package_parent)?;
    ensure_safe_directory(&roots.staging_root, true)?;
    let required_space = prepared
        .inspection
        .extracted_byte_length
        .checked_add(FREE_SPACE_OVERHEAD_BYTES)
        .ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "required installation space overflowed",
            )
        })?;
    let available_space = fs2::available_space(&roots.staging_root).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "measure installation free space",
            &error,
        )
    })?;
    if available_space < required_space {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            format!(
                "insufficient free space: need {required_space} bytes, found {available_space}"
            ),
        ));
    }
    let staging = roots
        .staging_root
        .join(format!(".install-{}", Uuid::new_v4().simple()));
    fs::create_dir(&staging).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "create install staging",
            &error,
        )
    })?;
    let mut staging_guard = TemporaryDirectoryGuard(Some(staging.clone()));
    extract_prepared(prepared, &staging)?;
    fs::rename(&staging, &destination).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "atomically publish package directory",
            &error,
        )
    })?;
    staging_guard.disarm();

    let receipt_path = trust_receipt_path(roots, &package);
    ensure_safe_directory(receipt_path.parent().expect("receipt parent"), true)?;
    let trust = trust_from_prepared(prepared, false)?;
    let receipt_result = if replace_receipt {
        write_receipt_replace(&receipt_path, &trust)
    } else {
        write_receipt_new(&receipt_path, &trust)
    };
    if let Err(error) = receipt_result {
        let rollback = roots
            .staging_root
            .join(format!(".install-{}", Uuid::new_v4().simple()));
        if fs::rename(&destination, &rollback).is_ok() {
            let mut seen = 0;
            let _ = remove_tree_no_follow(&rollback, 0, &mut seen);
        }
        return Err(error);
    }
    remove_if_empty(&roots.staging_root);
    Ok(InstallReceipt {
        destination,
        trust_receipt_path: receipt_path,
        inspection: prepared.inspection.clone(),
    })
}

fn trust_from_prepared(prepared: &PreparedPackage, enabled: bool) -> Result<TrustReceipt> {
    let installed_at_utc = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|error| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                format!("format trust receipt time: {error}"),
            )
        })?;
    Ok(TrustReceipt {
        receipt_version: TRUST_RECEIPT_VERSION.to_owned(),
        package: prepared.manifest.reference(),
        archive_sha256: prepared.inspection.archive_sha256.clone(),
        archive_byte_length: prepared.inspection.archive_byte_length,
        manifest_sha256: prepared.inspection.manifest_sha256.clone(),
        integrity_catalog_sha256: prepared.inspection.integrity_catalog_sha256.clone(),
        publisher_name: prepared.manifest.publisher().name.clone(),
        publisher_identity_claim: prepared.manifest.publisher().identity_claim.clone(),
        installed_at_utc,
        enabled,
    })
}

fn trust_receipt_matches_prepared(receipt: &TrustReceipt, prepared: &PreparedPackage) -> bool {
    receipt.receipt_version == TRUST_RECEIPT_VERSION
        && receipt.package == prepared.manifest.reference()
        && receipt.archive_sha256 == prepared.inspection.archive_sha256
        && receipt.archive_byte_length == prepared.inspection.archive_byte_length
        && receipt.manifest_sha256 == prepared.inspection.manifest_sha256
        && receipt.integrity_catalog_sha256 == prepared.inspection.integrity_catalog_sha256
        && receipt.publisher_name == prepared.manifest.publisher().name
        && receipt.publisher_identity_claim == prepared.manifest.publisher().identity_claim
}

fn verify_locked(
    roots: &ExtensionRoots,
    package: &PackageReference,
) -> Result<InstalledPackageSummary> {
    let destination = package_destination(roots, package);
    let (receipt, validated) = verify_installed_tree_locked(roots, package, &destination)?;
    Ok(InstalledPackageSummary {
        package: package.clone(),
        display_name: Some(validated.manifest.display_name().to_owned()),
        publisher_name: Some(validated.manifest.publisher().name.clone()),
        enabled: receipt.enabled,
        health: PackageHealth::Healthy,
        error_code: None,
        error_detail: None,
    })
}

fn verify_installed_tree_locked(
    roots: &ExtensionRoots,
    package: &PackageReference,
    destination: &Path,
) -> Result<(TrustReceipt, crate::archive::ValidatedDirectory)> {
    if !path_exists(destination, "installed package")? {
        return Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact package version is not installed",
        ));
    }
    let receipt = read_trust_receipt(&trust_receipt_path(roots, package))?;
    if receipt.receipt_version != TRUST_RECEIPT_VERSION || receipt.package != *package {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt identity or version is invalid",
        ));
    }
    let validated = validate_directory(destination, Some(package.kind))?;
    if validated.manifest.reference() != *package
        || validated.manifest_sha256 != receipt.manifest_sha256
        || validated.integrity_catalog_sha256 != receipt.integrity_catalog_sha256
        || validated.manifest.publisher().name != receipt.publisher_name
        || validated.manifest.publisher().identity_claim != receipt.publisher_identity_claim
    {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "installed package differs from its hash-bound trust receipt",
        ));
    }
    Ok((receipt, validated))
}

fn set_enabled(
    roots: &ExtensionRoots,
    package: &PackageReference,
    enabled: bool,
) -> Result<TrustReceipt> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    verify_locked(roots, package)?;
    let path = trust_receipt_path(roots, package);
    let mut receipt = read_trust_receipt(&path)?;
    if enabled {
        let id_root = roots.receipt_root(package.kind).join(&package.package_id);
        if id_root.is_dir() {
            for entry in fs::read_dir(&id_root).map_err(|error| {
                ExtensionError::io(ErrorCode::Io, "inspect active package versions", &error)
            })? {
                let entry = entry.map_err(|error| {
                    ExtensionError::io(ErrorCode::Io, "inspect active trust receipt", &error)
                })?;
                if entry.path() == path {
                    continue;
                }
                if let Ok(other) = read_trust_receipt(&entry.path())
                    && other.enabled
                {
                    return Err(ExtensionError::new(
                        ErrorCode::LifecycleConflict,
                        "another version of this package is enabled; disable it explicitly first",
                    ));
                }
            }
        }
    }
    receipt.enabled = enabled;
    write_receipt_replace(&path, &receipt)?;
    Ok(receipt)
}

fn list_kind_locked(
    roots: &ExtensionRoots,
    kind: PackageKind,
    output: &mut Vec<InstalledPackageSummary>,
) -> Result<()> {
    let root = roots.package_root(kind);
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ExtensionError::io(
                ErrorCode::Io,
                "inspect package root",
                &error,
            ));
        }
        Ok(metadata) if !metadata.is_dir() || is_reparse_or_symlink(&metadata) => {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package root is not a regular directory",
            ));
        }
        Ok(_) => {}
    }
    let entries = fs::read_dir(root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read package root", &error))?;
    let mut package_count = 0;
    for id_entry in entries {
        let Ok(id_entry) = id_entry else {
            output.push(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: "org.invalid.unreadable".to_owned(),
                    package_version: "unknown".to_owned(),
                },
                ErrorCode::Io.as_str(),
                "an installed package entry could not be read",
            ));
            continue;
        };
        package_count += 1;
        if package_count > MAX_PACKAGES_PER_KIND {
            output.push(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: id_entry.file_name().to_string_lossy().into_owned(),
                    package_version: "unknown".to_owned(),
                },
                ErrorCode::LifecycleConflict.as_str(),
                "additional installed package entries exceed the bounded inventory",
            ));
            break;
        }
        list_identity_locked(roots, kind, &id_entry, output);
    }
    Ok(())
}

fn list_identity_locked(
    roots: &ExtensionRoots,
    kind: PackageKind,
    id_entry: &fs::DirEntry,
    output: &mut Vec<InstalledPackageSummary>,
) {
    let package_id = id_entry.file_name().to_string_lossy().into_owned();
    let Ok(id_metadata) = fs::symlink_metadata(id_entry.path()) else {
        output.push(corrupt_summary(
            PackageReference {
                kind,
                package_id,
                package_version: "unknown".to_owned(),
            },
            ErrorCode::Io.as_str(),
            "package identity root metadata is unavailable",
        ));
        return;
    };
    if !id_metadata.is_dir() || is_reparse_or_symlink(&id_metadata) {
        output.push(corrupt_summary(
            PackageReference {
                kind,
                package_id,
                package_version: "unknown".to_owned(),
            },
            ErrorCode::LifecycleConflict.as_str(),
            "package identity root is unsafe",
        ));
        return;
    }
    let Ok(versions) = fs::read_dir(id_entry.path()) else {
        output.push(corrupt_summary(
            PackageReference {
                kind,
                package_id,
                package_version: "unknown".to_owned(),
            },
            ErrorCode::Io.as_str(),
            "installed version inventory is unavailable",
        ));
        return;
    };
    let mut version_count = 0;
    for version_entry in versions {
        let Ok(version_entry) = version_entry else {
            output.push(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: package_id.clone(),
                    package_version: "unknown".to_owned(),
                },
                ErrorCode::Io.as_str(),
                "an installed version entry could not be read",
            ));
            continue;
        };
        version_count += 1;
        if version_count > MAX_VERSIONS_PER_PACKAGE {
            output.push(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: package_id.clone(),
                    package_version: version_entry.file_name().to_string_lossy().into_owned(),
                },
                ErrorCode::LifecycleConflict.as_str(),
                "additional installed versions exceed the bounded inventory",
            ));
            break;
        }
        list_version_locked(roots, kind, &package_id, &version_entry, output);
    }
}

fn list_version_locked(
    roots: &ExtensionRoots,
    kind: PackageKind,
    package_id: &str,
    version_entry: &fs::DirEntry,
    output: &mut Vec<InstalledPackageSummary>,
) {
    let package = PackageReference {
        kind,
        package_id: package_id.to_owned(),
        package_version: version_entry.file_name().to_string_lossy().into_owned(),
    };
    match verify_locked(roots, &package) {
        Ok(summary) => output.push(summary),
        Err(error) => {
            let tree = validate_directory(&version_entry.path(), Some(kind));
            let (display_name, publisher_name) = tree.ok().map_or((None, None), |validated| {
                (
                    Some(validated.manifest.display_name().to_owned()),
                    Some(validated.manifest.publisher().name.clone()),
                )
            });
            let health = if error.code() == ErrorCode::PackageUntrusted {
                PackageHealth::Untrusted
            } else {
                PackageHealth::Corrupt
            };
            output.push(InstalledPackageSummary {
                package,
                display_name,
                publisher_name,
                enabled: false,
                health,
                error_code: Some(error.code().as_str().to_owned()),
                error_detail: Some(error.detail().to_owned()),
            });
        }
    }
}

fn resolve_pair(
    deck: &crate::model::DeckPackManifest,
    codec: &crate::model::CodecPackManifest,
) -> (CompatibilityReason, Option<crate::model::ProfileKey>) {
    if deck.compatibility.worker_protocol != codec.compatibility.worker_protocol {
        return (CompatibilityReason::UnsupportedProtocol, None);
    }
    let app = Version::parse(env!("CARGO_PKG_VERSION")).expect("crate version is SemVer");
    if !version_in_range(
        &app,
        &deck.compatibility.app_min_inclusive,
        &deck.compatibility.app_max_exclusive,
    ) || !version_in_range(
        &app,
        &codec.compatibility.app_min_inclusive,
        &codec.compatibility.app_max_exclusive,
    ) {
        return (CompatibilityReason::UnsupportedHostApi, None);
    }
    if deck.compatibility.tensor_abi != codec.compatibility.tensor_abi
        || deck.compatibility.python != codec.compatibility.python
        || deck.compatibility.torch_exact_build != codec.compatibility.torch_exact_build
    {
        return (CompatibilityReason::UnsupportedTensorAbi, None);
    }
    let profile_candidates: Vec<_> = codec
        .compatibility
        .profiles
        .iter()
        .filter(|profile| {
            deck.signal
                .profile_allowlist
                .as_ref()
                .is_none_or(|allowlist| allowlist.contains(profile))
        })
        .collect();
    if profile_candidates.is_empty() {
        return (CompatibilityReason::UnsupportedProfile, None);
    }
    let provided: HashSet<CodecCapability> = codec.capabilities.iter().copied().collect();
    if deck
        .signal
        .required_capabilities
        .iter()
        .any(|required| !provided.contains(required))
    {
        return (CompatibilityReason::UnsupportedCapability, None);
    }
    (
        CompatibilityReason::Compatible,
        Some((*profile_candidates[0]).clone()),
    )
}

fn version_in_range(version: &Version, minimum: &str, maximum: &str) -> bool {
    Version::parse(minimum)
        .ok()
        .zip(Version::parse(maximum).ok())
        .is_some_and(|(minimum, maximum)| version >= &minimum && version < &maximum)
}

fn corrupt_summary(package: PackageReference, code: &str, detail: &str) -> InstalledPackageSummary {
    InstalledPackageSummary {
        package,
        display_name: None,
        publisher_name: None,
        enabled: false,
        health: PackageHealth::Corrupt,
        error_code: Some(code.to_owned()),
        error_detail: Some(detail.to_owned()),
    }
}

fn prepare_base(roots: &ExtensionRoots) -> Result<()> {
    let parent = roots.base_root.parent().ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::InvalidArguments,
            "extension root has no safe parent",
        )
    })?;
    ensure_existing_tree_safe(parent)?;
    fs::create_dir_all(&roots.base_root).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "create extension root",
            &error,
        )
    })?;
    ensure_safe_directory(&roots.base_root, false)
}

fn prepare_locked_roots(roots: &ExtensionRoots) -> Result<()> {
    cleanup_stale_staging(roots)?;
    cleanup_stale_trash(roots)?;
    ensure_safe_directory(&roots.decks_root, true)?;
    ensure_safe_directory(&roots.codec_packs_root, true)?;
    ensure_safe_directory(&roots.trust_root, true)?;
    ensure_safe_directory(&roots.usage_root, true)
}

fn acquire_lock(roots: &ExtensionRoots) -> Result<LifecycleLock> {
    match fs::symlink_metadata(&roots.lock_path) {
        Ok(metadata) if !metadata.is_file() || is_reparse_or_symlink(&metadata) => {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "extension lifecycle lock path is unsafe",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ExtensionError::io(
                ErrorCode::Io,
                "inspect extension lifecycle lock",
                &error,
            ));
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&roots.lock_path)
        .map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "open extension lifecycle lock", &error)
        })?;
    file.try_lock_exclusive().map_err(|error| {
        let code = if is_lock_contended(&error) {
            ErrorCode::LifecycleBusy
        } else {
            ErrorCode::LifecycleConflict
        };
        ExtensionError::io(code, "acquire extension lifecycle lock", &error)
    })?;
    Ok(LifecycleLock(file))
}

fn cleanup_stale_staging(roots: &ExtensionRoots) -> Result<()> {
    match fs::symlink_metadata(&roots.staging_root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ExtensionError::io(
                ErrorCode::Io,
                "inspect staging root",
                &error,
            ));
        }
        Ok(metadata) if !metadata.is_dir() || is_reparse_or_symlink(&metadata) => {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "extension staging root is unsafe",
            ));
        }
        Ok(_) => {}
    }
    let mut count = 0;
    for entry in fs::read_dir(&roots.staging_root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read staging root", &error))?
    {
        count += 1;
        if count > MAX_STALE_STAGING_ENTRIES {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "staging root exceeds its bounded recovery count",
            ));
        }
        let entry = entry
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "read staging entry", &error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_exact_staging_name(&name) {
            continue;
        }
        let mut seen = 0;
        remove_tree_no_follow(&entry.path(), 0, &mut seen).map_err(|error| {
            ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "recover stale staging",
                &error,
            )
        })?;
    }
    remove_if_empty(&roots.staging_root);
    Ok(())
}

fn is_exact_staging_name(name: &str) -> bool {
    name.strip_prefix(".install-").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn cleanup_stale_trash(roots: &ExtensionRoots) -> Result<()> {
    match fs::symlink_metadata(&roots.trash_root) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(ExtensionError::io(
                ErrorCode::Io,
                "inspect extension trash root",
                &error,
            ));
        }
        Ok(metadata) if !metadata.is_dir() || is_reparse_or_symlink(&metadata) => {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "extension trash root is unsafe",
            ));
        }
        Ok(_) => {}
    }
    let mut count = 0;
    for entry in fs::read_dir(&roots.trash_root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read extension trash root", &error))?
    {
        count += 1;
        if count > MAX_STALE_TRASH_ENTRIES {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "extension trash root exceeds its bounded recovery count",
            ));
        }
        let entry = entry.map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "read extension trash entry", &error)
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_exact_trash_name(&name) {
            continue;
        }
        let mut seen = 0;
        remove_tree_no_follow(&entry.path(), 0, &mut seen).map_err(|error| {
            ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "recover stale extension trash",
                &error,
            )
        })?;
    }
    remove_if_empty(&roots.trash_root);
    Ok(())
}

fn is_exact_trash_name(name: &str) -> bool {
    [".remove-", ".repair-"].iter().any(|prefix| {
        name.strip_prefix(prefix).is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
    })
}

fn validate_roots(roots: &ExtensionRoots) -> Result<()> {
    validate_absolute_normal(&roots.base_root, "base root")?;
    for (actual, expected, name) in [
        (
            &roots.decks_root,
            roots.base_root.join("Decks"),
            "Deck root",
        ),
        (
            &roots.codec_packs_root,
            roots.base_root.join("CodecPacks"),
            "Codec root",
        ),
        (
            &roots.trust_root,
            roots.base_root.join("PackageTrust"),
            "trust root",
        ),
        (
            &roots.usage_root,
            roots.base_root.join("PackageUsage"),
            "usage root",
        ),
        (
            &roots.staging_root,
            roots.base_root.join("ExtensionStaging"),
            "staging root",
        ),
        (
            &roots.trash_root,
            roots.base_root.join("ExtensionTrash"),
            "trash root",
        ),
        (
            &roots.lock_path,
            roots.base_root.join("ExtensionLifecycle.lock"),
            "lock path",
        ),
    ] {
        if actual != &expected {
            return Err(ExtensionError::new(
                ErrorCode::InvalidArguments,
                format!("{name} does not match the fixed root layout"),
            ));
        }
    }
    Ok(())
}

fn validate_absolute_normal(path: &Path, name: &str) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            format!("{name} must be an absolute normal path"),
        ));
    }
    Ok(())
}

fn ensure_safe_directory(path: &Path, create: bool) -> Result<()> {
    ensure_existing_tree_safe(path)?;
    if create {
        fs::create_dir_all(path).map_err(|error| {
            ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "create lifecycle directory",
                &error,
            )
        })?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "inspect lifecycle directory",
            &error,
        )
    })?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "lifecycle directory is not a regular directory",
        ));
    }
    ensure_existing_tree_safe(path)
}

fn package_destination(roots: &ExtensionRoots, package: &PackageReference) -> PathBuf {
    roots
        .package_root(package.kind)
        .join(&package.package_id)
        .join(&package.package_version)
}

fn trust_receipt_path(roots: &ExtensionRoots, package: &PackageReference) -> PathBuf {
    roots
        .receipt_root(package.kind)
        .join(&package.package_id)
        .join(format!("{}.json", package.package_version))
}

fn usage_lock_path(roots: &ExtensionRoots, package: &PackageReference) -> PathBuf {
    roots
        .usage_root
        .join(package.kind.receipt_root_name())
        .join(&package.package_id)
        .join(format!("{}.lock", package.package_version))
}

fn open_usage_lock(roots: &ExtensionRoots, package: &PackageReference) -> Result<File> {
    let path = usage_lock_path(roots, package);
    let parent = path.parent().ok_or_else(|| {
        ExtensionError::new(ErrorCode::LifecycleConflict, "usage lock has no parent")
    })?;
    ensure_safe_directory(parent, true)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || is_reparse_or_symlink(&metadata) => {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package usage lock path is unsafe",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "inspect package usage lock",
                &error,
            ));
        }
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "open package usage lock",
                &error,
            )
        })
}

fn ensure_not_in_use_locked(roots: &ExtensionRoots, package: &PackageReference) -> Result<()> {
    let usage_lock = open_usage_lock(roots, package)?;
    FileExt::try_lock_exclusive(&usage_lock).map_err(|error| {
        let code = if is_lock_contended(&error) {
            ErrorCode::PackageActive
        } else {
            ErrorCode::LifecycleConflict
        };
        ExtensionError::io(code, "acquire package removal lease", &error)
    })?;
    FileExt::unlock(&usage_lock).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "release package removal lease",
            &error,
        )
    })
}

fn is_lock_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

fn remove_usage_lock(roots: &ExtensionRoots, package: &PackageReference) {
    let path = usage_lock_path(roots, package);
    let _ = fs::remove_file(&path);
    if let Some(version_parent) = path.parent() {
        remove_if_empty(version_parent);
        if let Some(kind_parent) = version_parent.parent() {
            remove_if_empty(kind_parent);
        }
    }
    remove_if_empty(&roots.usage_root);
}

fn read_trust_receipt(path: &Path) -> Result<TrustReceipt> {
    ensure_existing_tree_safe(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        let code = if error.kind() == io::ErrorKind::NotFound {
            ErrorCode::PackageUntrusted
        } else {
            ErrorCode::Io
        };
        ExtensionError::io(code, "inspect trust receipt", &error)
    })?;
    if !metadata.is_file()
        || is_reparse_or_symlink(&metadata)
        || metadata.len() > MAX_JSON_BYTES as u64
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt is not a bounded regular file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|error| {
        ExtensionError::io(ErrorCode::PackageUntrusted, "open trust receipt", &error)
    })?;
    let opened_metadata = file.metadata().map_err(|error| {
        ExtensionError::io(
            ErrorCode::PackageUntrusted,
            "inspect opened trust receipt",
            &error,
        )
    })?;
    ensure_existing_tree_safe(path)?;
    if !opened_metadata.is_file()
        || is_reparse_or_symlink(&opened_metadata)
        || opened_metadata.len() != metadata.len()
        || opened_metadata.len() > MAX_JSON_BYTES as u64
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "opened trust receipt changed identity or exceeded its bound",
        ));
    }
    let capacity = usize::try_from(opened_metadata.len()).map_err(|_| {
        ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt size cannot be represented safely",
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    let mut bounded = file.take(MAX_JSON_BYTES as u64 + 1);
    bounded
        .read_to_end(&mut bytes)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read trust receipt", &error))?;
    let final_length = bounded
        .get_ref()
        .metadata()
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::PackageUntrusted,
                "reinspect opened trust receipt",
                &error,
            )
        })?
        .len();
    if bytes.len() > MAX_JSON_BYTES
        || bytes.len() as u64 != opened_metadata.len()
        || final_length != opened_metadata.len()
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt changed while it was read or exceeded its bound",
        ));
    }
    let receipt: TrustReceipt = parse_strict_json(&bytes, "trust receipt")?;
    validate_package_reference(&receipt.package)?;
    validate_sha256(&receipt.archive_sha256, "receipt archive SHA-256")?;
    validate_sha256(&receipt.manifest_sha256, "receipt manifest SHA-256")?;
    validate_sha256(&receipt.integrity_catalog_sha256, "receipt catalog SHA-256")?;
    if receipt.archive_byte_length == 0 {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt archive length is zero",
        ));
    }
    OffsetDateTime::parse(&receipt.installed_at_utc, &Rfc3339).map_err(|_| {
        ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt installed_at_utc is not RFC 3339",
        )
    })?;
    Ok(receipt)
}

fn write_receipt_new(path: &Path, receipt: &TrustReceipt) -> Result<()> {
    write_receipt(path, receipt, false)
}

fn write_receipt_replace(path: &Path, receipt: &TrustReceipt) -> Result<()> {
    write_receipt(path, receipt, true)
}

fn write_receipt(path: &Path, receipt: &TrustReceipt, replace: bool) -> Result<()> {
    let bytes = canonical_json(receipt, "trust receipt")?;
    let parent = path.parent().ok_or_else(|| {
        ExtensionError::new(ErrorCode::LifecycleConflict, "trust receipt has no parent")
    })?;
    ensure_safe_directory(parent, true)?;
    let partial = parent.join(format!(".receipt-{}.partial", Uuid::new_v4().simple()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::LifecycleConflict,
                "create trust receipt partial",
                &error,
            )
        })?;
    let result = (|| -> io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        if replace {
            replace_atomic(&partial, path)
        } else {
            fs::hard_link(&partial, path)?;
            fs::remove_file(&partial)
        }
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&partial);
        let code = if error.kind() == io::ErrorKind::AlreadyExists {
            ErrorCode::PackageExists
        } else {
            ErrorCode::LifecycleConflict
        };
        return Err(ExtensionError::io(
            code,
            "atomically publish trust receipt",
            &error,
        ));
    }
    Ok(())
}

fn enforce_version_limit(parent: &Path) -> Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(parent)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read installed versions", &error))?
    {
        let entry = entry
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "read installed version", &error))?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "inspect installed version", &error)
        })?;
        if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "installed version root contains an unsafe entry",
            ));
        }
        count += 1;
        if count >= MAX_VERSIONS_PER_PACKAGE {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "the maximum of 16 side-by-side versions is already installed",
            ));
        }
    }
    Ok(())
}

fn enforce_package_id_limit(root: &Path) -> Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read package identities", &error))?
    {
        let entry = entry
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "read package identity", &error))?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "inspect package identity", &error)
        })?;
        if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package root contains an unsafe identity entry",
            ));
        }
        count += 1;
        if count >= MAX_PACKAGES_PER_KIND {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "the maximum of 256 package identities is already installed",
            ));
        }
    }
    Ok(())
}

fn cleanup_package_parents(roots: &ExtensionRoots, package: &PackageReference) {
    remove_if_empty(&roots.package_root(package.kind).join(&package.package_id));
    remove_if_empty(&roots.receipt_root(package.kind).join(&package.package_id));
}

fn remove_tree_no_follow(path: &Path, depth: usize, seen: &mut usize) -> io::Result<()> {
    if depth > MAX_REMOVE_TREE_DEPTH {
        return Err(io::Error::other("remove tree depth exceeded"));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(io::Error::other("remove tree root is unsafe"));
    }
    for entry in fs::read_dir(path)? {
        *seen = seen.saturating_add(1);
        if *seen > MAX_REMOVE_TREE_ENTRIES {
            return Err(io::Error::other("remove tree entry bound exceeded"));
        }
        let entry = entry?;
        let child = entry.path();
        let child_metadata = fs::symlink_metadata(&child)?;
        if is_reparse_or_symlink(&child_metadata) {
            if child_metadata.is_dir() {
                fs::remove_dir(&child)?;
            } else {
                fs::remove_file(&child)?;
            }
        } else if child_metadata.is_dir() {
            remove_tree_no_follow(&child, depth + 1, seen)?;
        } else if child_metadata.is_file() {
            fs::remove_file(&child)?;
        } else {
            return Err(io::Error::other("remove tree contains a special entry"));
        }
    }
    fs::remove_dir(path)
}

fn remove_if_empty(path: &Path) {
    let Ok(mut entries) = fs::read_dir(path) else {
        return;
    };
    if entries.next().is_none() {
        let _ = fs::remove_dir(path);
    }
}

fn path_exists(path: &Path, context: &str) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ExtensionError::io(ErrorCode::Io, context, &error)),
    }
}

fn kind_from_path(path: &Path) -> Result<PackageKind> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("ld") => Ok(PackageKind::DeckPack),
        Some("ldcodec") => Ok(PackageKind::CodecPack),
        _ => Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "package archive must use canonical .ld or .ldcodec extension",
        )),
    }
}

fn is_in_use_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::FileMeasurement;
    use tempfile::TempDir;

    #[test]
    fn pin_rehash_rejects_bytes_changed_after_validation_snapshot() {
        let temp = TempDir::new().expect("temp");
        let roots = ExtensionRoots::for_base_root(temp.path().join("LatentDeck"));
        let destination = roots.decks_root.join("com.example.deck").join("0.2.0");
        let runtime_file = destination.join("python/deck_operator.py");
        fs::create_dir_all(runtime_file.parent().expect("runtime parent"))
            .expect("create package tree");
        let trusted = b"trusted";
        fs::write(&runtime_file, trusted).expect("write trusted snapshot");
        let expected = FileMeasurement {
            path: "python/deck_operator.py".to_owned(),
            byte_length: trusted.len() as u64,
            sha256: hex::encode(Sha256::digest(trusted)),
        };
        fs::write(&runtime_file, b"changed").expect("change after validation snapshot");

        let error = pin_validated_package_tree(
            &roots,
            &destination,
            &BTreeMap::from([(expected.path.clone(), expected)]),
        )
        .expect_err("stale validation snapshot must not produce an active package");
        assert_eq!(error.code(), ErrorCode::IntegrityFailed);
    }
}
