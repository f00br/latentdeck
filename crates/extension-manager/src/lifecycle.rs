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
    prepare_archive, validate_contract, validate_directory, validate_directory_snapshot,
};
use crate::error::{ErrorCode, ExtensionError, Result};
use crate::model::{
    ActiveInstalledPackage, BundledPackageIndex, CodecCapability, CompatibilityPair,
    CompatibilityReason, ExtensionInventory, InstallReceipt, InstalledPackageSummary,
    PackageHealth, PackageKind, PackageManifest, PackageReference, TrustReceipt,
    ValidatedInstalledPackage,
};
use crate::schema::{
    MAX_JSON_BYTES, TRUST_RECEIPT_VERSION, canonical_json, is_reserved_package_id,
    parse_integrity_catalog, parse_manifest, parse_strict_json, validate_bundled_index,
    validate_package_reference, validate_sha256,
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
    resolve_active_impl(roots, package, None)
}

pub(crate) fn resolve_active_counted(
    roots: &ExtensionRoots,
    package: &PackageReference,
    full_hash_attempts: &std::sync::atomic::AtomicU64,
) -> Result<ActiveInstalledPackage> {
    resolve_active_impl(roots, package, Some(full_hash_attempts))
}

pub(crate) fn enable_active_counted(
    roots: &ExtensionRoots,
    package: &PackageReference,
    full_hash_attempts: &std::sync::atomic::AtomicU64,
) -> Result<ActiveInstalledPackage> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    let destination = package_destination(roots, package);
    if !path_exists(&destination, "installed package")? {
        return Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact package version is not installed",
        ));
    }
    let receipt_path = trust_receipt_path(roots, package);
    let mut receipt = read_trust_receipt(&receipt_path)?;
    validate_exact_receipt(&receipt, package)?;
    ensure_no_other_enabled_version(roots, package, &receipt_path)?;

    let usage_lock = open_usage_lock(roots, package)?;
    FileExt::try_lock_shared(&usage_lock).map_err(|error| {
        ExtensionError::io(
            ErrorCode::LifecycleConflict,
            "acquire package usage lease",
            &error,
        )
    })?;
    full_hash_attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let (validated, retained_tree_handles) =
        pin_and_validate_package_tree(roots, &destination, package.kind)?;
    validate_hash_bound_receipt(&validated, &receipt, package)?;

    receipt.enabled = true;
    write_receipt_replace(&receipt_path, &receipt)?;
    Ok(ActiveInstalledPackage::new(
        ValidatedInstalledPackage::new(destination, validated.manifest, receipt),
        validated.files,
        1,
        usage_lock,
        retained_tree_handles,
    ))
}

fn resolve_active_impl(
    roots: &ExtensionRoots,
    package: &PackageReference,
    full_hash_attempts: Option<&std::sync::atomic::AtomicU64>,
) -> Result<ActiveInstalledPackage> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    prepare_locked_roots(roots)?;
    let destination = package_destination(roots, package);
    if !path_exists(&destination, "installed package")? {
        return Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact package version is not installed",
        ));
    }
    let receipt = read_trust_receipt(&trust_receipt_path(roots, package))?;
    validate_exact_receipt(&receipt, package)?;
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
    if let Some(full_hash_attempts) = full_hash_attempts {
        full_hash_attempts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    let (validated, retained_tree_handles) =
        pin_and_validate_package_tree(roots, &destination, package.kind)?;
    validate_hash_bound_receipt(&validated, &receipt, package)?;
    Ok(ActiveInstalledPackage::new(
        ValidatedInstalledPackage::new(destination, validated.manifest, receipt),
        validated.files,
        1,
        usage_lock,
        retained_tree_handles,
    ))
}

fn pin_and_validate_package_tree(
    roots: &ExtensionRoots,
    destination: &Path,
    kind: PackageKind,
) -> Result<(crate::archive::ValidatedDirectory, Vec<File>)> {
    let mut retained = pin_package_root_directories(roots, destination)?;
    let (manifest_measurement, manifest_bytes, manifest_file) =
        open_and_measure_control_file(destination, kind.manifest_name())?;
    let manifest = parse_manifest(kind, &manifest_bytes)?;
    let (catalog_measurement, catalog_bytes, catalog_file) =
        open_and_measure_control_file(destination, "integrity.json")?;
    if catalog_measurement.sha256 != manifest.integrity().catalog_sha256 {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "integrity.json hash does not match the control manifest",
        ));
    }
    let catalog = parse_integrity_catalog(&catalog_bytes, kind)?;

    let mut expected_files = BTreeMap::from([
        (
            kind.manifest_name().to_owned(),
            manifest_measurement.clone(),
        ),
        ("integrity.json".to_owned(), catalog_measurement.clone()),
    ]);
    for described in &catalog.files {
        expected_files.insert(
            described.path.clone(),
            crate::archive::FileMeasurement {
                path: described.path.clone(),
                byte_length: described.byte_length,
                sha256: described.sha256.clone(),
            },
        );
    }
    validate_directory_snapshot(destination, kind, &expected_files)?;
    retained.extend(pin_package_subdirectories(
        destination,
        expected_files.keys(),
    )?);

    let mut jobs = Vec::with_capacity(catalog.files.len());
    for (index, described) in catalog.files.into_iter().enumerate() {
        let path = path_from_portable(destination, &described.path);
        let file = open_pinned_file_under_pinned_tree(&path)?;
        jobs.push(PinnedHashJob {
            index,
            expected: crate::archive::FileMeasurement {
                path: described.path,
                byte_length: described.byte_length,
                sha256: described.sha256,
            },
            keep_bytes: kind == PackageKind::DeckPack,
            file,
        });
    }
    let measured_payloads = hash_pinned_jobs(jobs)?;
    let mut files = BTreeMap::from([
        (kind.manifest_name().to_owned(), manifest_measurement),
        ("integrity.json".to_owned(), catalog_measurement),
    ]);
    let mut contents = BTreeMap::from([
        (kind.manifest_name().to_owned(), manifest_bytes),
        ("integrity.json".to_owned(), catalog_bytes),
    ]);
    for payload in measured_payloads {
        if let Some(bytes) = payload.bytes {
            contents.insert(payload.measurement.path.clone(), bytes);
        }
        files.insert(payload.measurement.path.clone(), payload.measurement);
        retained.push(payload.file);
    }
    retained.push(manifest_file);
    retained.push(catalog_file);

    let explicit_directories = implied_directories(files.keys());
    let validated = validate_contract(kind, &files, &explicit_directories, &contents)?;
    if validated.manifest != manifest {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "measured package manifest changed during active validation",
        ));
    }
    validate_directory_snapshot(destination, kind, &validated.files)?;
    Ok((validated, retained))
}

fn pin_package_root_directories(roots: &ExtensionRoots, destination: &Path) -> Result<Vec<File>> {
    let kind_root = destination.parent().and_then(Path::parent).ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "installed package has no kind root",
        )
    })?;
    let identity_root = destination.parent().ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "installed package has no identity root",
        )
    })?;
    [
        roots.base_root.as_path(),
        kind_root,
        identity_root,
        destination,
    ]
    .into_iter()
    .map(open_pinned_directory)
    .collect()
}

fn pin_package_subdirectories<'a>(
    destination: &Path,
    paths: impl Iterator<Item = &'a String>,
) -> Result<Vec<File>> {
    let directories = implied_directories(paths);
    let mut directories: Vec<_> = directories
        .into_iter()
        .map(|relative| path_from_portable(destination, &relative))
        .collect();
    directories.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    directories
        .into_iter()
        .map(|path| open_pinned_directory_under_pinned_tree(&path))
        .collect()
}

fn implied_directories<'a>(paths: impl Iterator<Item = &'a String>) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for path in paths {
        let mut current = path.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            directories.insert(parent.to_owned());
            current = parent;
        }
    }
    directories
}

fn path_from_portable(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component))
}

fn open_and_measure_control_file(
    destination: &Path,
    relative: &str,
) -> Result<(crate::archive::FileMeasurement, Vec<u8>, File)> {
    let path = path_from_portable(destination, relative);
    let mut file = open_pinned_file(&path)?;
    let length = file
        .metadata()
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::IntegrityFailed,
                "inspect package control file",
                &error,
            )
        })?
        .len();
    if length > MAX_JSON_BYTES as u64 {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "package control JSON exceeds the 1 MiB bound",
        ));
    }
    let mut buffer = vec![0_u8; 1024 * 1024].into_boxed_slice();
    let (measurement, bytes) = measure_pinned_file(&mut file, relative, length, true, &mut buffer)?;
    let bytes = bytes.ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "bounded package control bytes are unavailable",
        )
    })?;
    Ok((measurement, bytes, file))
}

struct PinnedHashJob {
    index: usize,
    expected: crate::archive::FileMeasurement,
    keep_bytes: bool,
    file: File,
}

#[derive(Debug)]
struct PinnedHashResult {
    index: usize,
    measurement: crate::archive::FileMeasurement,
    bytes: Option<Vec<u8>>,
    file: File,
}

fn hash_pinned_jobs(mut jobs: Vec<PinnedHashJob>) -> Result<Vec<PinnedHashResult>> {
    const MAX_WORKERS: usize = 4;
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAX_WORKERS)
        .min(jobs.len());
    jobs.sort_by(|left, right| {
        right
            .expected
            .byte_length
            .cmp(&left.expected.byte_length)
            .then_with(|| left.expected.path.cmp(&right.expected.path))
    });
    let mut buckets: Vec<Vec<PinnedHashJob>> = (0..worker_count).map(|_| Vec::new()).collect();
    let mut bucket_bytes = vec![0_u64; worker_count];
    for job in jobs {
        let bucket = bucket_bytes
            .iter()
            .enumerate()
            .min_by_key(|(index, bytes)| (**bytes, *index))
            .map_or(0, |(index, _)| index);
        bucket_bytes[bucket] = bucket_bytes[bucket].saturating_add(job.expected.byte_length);
        buckets[bucket].push(job);
    }

    let workers: Vec<_> = buckets
        .into_iter()
        .map(|bucket| {
            std::thread::spawn(move || -> Result<Vec<PinnedHashResult>> {
                let mut buffer = vec![0_u8; 1024 * 1024].into_boxed_slice();
                bucket
                    .into_iter()
                    .map(|mut job| {
                        let (measurement, bytes) = measure_pinned_file(
                            &mut job.file,
                            &job.expected.path,
                            job.expected.byte_length,
                            job.keep_bytes,
                            &mut buffer,
                        )?;
                        if measurement.sha256 != job.expected.sha256 {
                            return Err(ExtensionError::new(
                                ErrorCode::IntegrityFailed,
                                format!(
                                    "catalogued file bytes changed before pin: {}",
                                    job.expected.path
                                ),
                            ));
                        }
                        Ok(PinnedHashResult {
                            index: job.index,
                            measurement,
                            bytes,
                            file: job.file,
                        })
                    })
                    .collect()
            })
        })
        .collect();
    let mut results = Vec::new();
    let mut first_error = None;
    for worker in workers {
        let outcome = worker.join().unwrap_or_else(|_| {
            Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package hash worker terminated unexpectedly",
            ))
        });
        match outcome {
            Ok(mut measured) => results.append(&mut measured),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    results.sort_by_key(|result| result.index);
    Ok(results)
}

fn measure_pinned_file(
    file: &mut File,
    relative: &str,
    expected_length: u64,
    keep_bytes: bool,
    buffer: &mut [u8],
) -> Result<(crate::archive::FileMeasurement, Option<Vec<u8>>)> {
    let initial_length = file
        .metadata()
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::IntegrityFailed,
                "inspect pinned package file",
                &error,
            )
        })?
        .len();
    if initial_length != expected_length {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("catalogued file length changed before pin: {relative}"),
        ));
    }
    let mut stored = if keep_bytes {
        Some(Vec::with_capacity(
            usize::try_from(expected_length).map_err(|_| {
                ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    "pinned file cannot fit in bounded memory",
                )
            })?,
        ))
    } else {
        None
    };
    let mut hasher = Sha256::new();
    let mut observed_length = 0_u64;
    loop {
        let read = file.read(buffer).map_err(|error| {
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
        if observed_length > expected_length {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("catalogued file grew before pin: {relative}"),
            ));
        }
        hasher.update(&buffer[..read]);
        if let Some(bytes) = stored.as_mut() {
            bytes.extend_from_slice(&buffer[..read]);
        }
    }
    let final_length = file
        .metadata()
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::IntegrityFailed,
                "reinspect pinned package file",
                &error,
            )
        })?
        .len();
    if observed_length != expected_length || final_length != expected_length {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("catalogued file length changed while pinned: {relative}"),
        ));
    }
    Ok((
        crate::archive::FileMeasurement {
            path: relative.to_owned(),
            byte_length: observed_length,
            sha256: hex::encode(hasher.finalize()),
        },
        stored,
    ))
}

pub(crate) fn revalidate_cached_active(
    roots: &ExtensionRoots,
    package: &PackageReference,
    active: &ActiveInstalledPackage,
) -> Result<()> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    let destination = package_destination(roots, package);
    if active.root() != destination || active.package().trust_receipt().package != *package {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "cached active package identity does not match the exact checkout",
        ));
    }
    let receipt = read_trust_receipt(&trust_receipt_path(roots, package))?;
    if !receipt.enabled {
        return Err(ExtensionError::new(
            ErrorCode::PackageDisabled,
            "enable the exact package version before runtime use",
        ));
    }
    if receipt != *active.trust_receipt() {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "cached active package receipt no longer matches its exact trusted receipt",
        ));
    }
    validate_directory_snapshot(active.root(), package.kind, active.expected_files())?;
    Ok(())
}

fn open_pinned_directory(path: &Path) -> Result<File> {
    open_pinned_directory_impl(path, true)
}

fn open_pinned_directory_under_pinned_tree(path: &Path) -> Result<File> {
    open_pinned_directory_impl(path, false)
}

fn open_pinned_directory_impl(path: &Path, check_ancestors: bool) -> Result<File> {
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
    if check_ancestors {
        ensure_existing_tree_safe(path)?;
    }
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
    open_pinned_file_impl(path, true)
}

fn open_pinned_file_under_pinned_tree(path: &Path) -> Result<File> {
    open_pinned_file_impl(path, false)
}

fn open_pinned_file_impl(path: &Path, check_ancestors: bool) -> Result<File> {
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
    if check_ancestors {
        ensure_existing_tree_safe(path)?;
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
/// Returns a stable error when the exact package root or trust receipt is
/// invalid, or the atomic receipt update fails. Disabling only narrows runtime
/// authority and therefore does not reread the package payload.
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
    list_with_manifests(roots).map(|(packages, _)| packages)
}

/// List only one installed package kind without inspecting the other kind's
/// root or payloads.
///
/// # Errors
///
/// Returns a stable error only when the shared lifecycle root or the requested
/// kind root is unsafe or cannot be inspected.
pub fn list_kind(
    roots: &ExtensionRoots,
    kind: PackageKind,
) -> Result<Vec<InstalledPackageSummary>> {
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    let mut packages = Vec::new();
    let mut manifests = BTreeMap::new();
    list_kind_locked(roots, kind, &mut packages, &mut manifests)?;
    packages.sort_by(|left, right| {
        (&left.package.package_id, &left.package.package_version)
            .cmp(&(&right.package.package_id, &right.package.package_version))
    });
    Ok(packages)
}

/// Read installed package summaries and their compatibility matrix from one
/// inventory pass. Every healthy package tree is validated once.
///
/// # Errors
///
/// Returns the same bounded root-level errors as [`list`]. Individual package
/// failures remain isolated in the package summaries and matrix reasons.
pub fn inventory(roots: &ExtensionRoots) -> Result<ExtensionInventory> {
    let (packages, manifests) = list_with_manifests(roots)?;
    let matrix = compatibility_matrix_from_inventory(&packages, &manifests);
    Ok(ExtensionInventory { packages, matrix })
}

fn list_with_manifests(
    roots: &ExtensionRoots,
) -> Result<(
    Vec<InstalledPackageSummary>,
    BTreeMap<PackageReference, PackageManifest>,
)> {
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    let mut packages = Vec::new();
    let mut manifests = BTreeMap::new();
    for kind in [PackageKind::DeckPack, PackageKind::CodecPack] {
        list_kind_locked(roots, kind, &mut packages, &mut manifests)?;
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
    Ok((packages, manifests))
}

/// Resolve every installed Deck-version by Codec-version pair.
///
/// # Errors
///
/// Returns a stable error when the installed roots cannot be safely listed or
/// inspected. Individual invalid packages become `package_invalid` pairs.
pub fn compatibility_matrix(roots: &ExtensionRoots) -> Result<Vec<CompatibilityPair>> {
    let (summaries, manifests) = list_with_manifests(roots)?;
    Ok(compatibility_matrix_from_inventory(&summaries, &manifests))
}

pub(crate) fn compatibility_matrix_from_inventory(
    summaries: &[InstalledPackageSummary],
    manifests: &BTreeMap<PackageReference, PackageManifest>,
) -> Vec<CompatibilityPair> {
    let decks: Vec<_> = summaries
        .iter()
        .filter(|item| item.package.kind == PackageKind::DeckPack)
        .collect();
    let codecs: Vec<_> = summaries
        .iter()
        .filter(|item| item.package.kind == PackageKind::CodecPack)
        .collect();
    let mut pairs = Vec::with_capacity(decks.len().saturating_mul(codecs.len()));
    for deck in decks {
        for codec in &codecs {
            let (reason, compatible_profile) = if deck.health == PackageHealth::Corrupt
                || codec.health == PackageHealth::Corrupt
            {
                (CompatibilityReason::PackageInvalid, None)
            } else if !deck.enabled
                || !codec.enabled
                || deck.health == PackageHealth::Untrusted
                || codec.health == PackageHealth::Untrusted
                || deck.health == PackageHealth::VerificationRequired
                || codec.health == PackageHealth::VerificationRequired
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
    pairs
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
    validate_exact_receipt(&receipt, package)?;
    let validated = validate_directory(destination, Some(package.kind))?;
    validate_hash_bound_receipt(&validated, &receipt, package)?;
    Ok((receipt, validated))
}

fn validate_exact_receipt(receipt: &TrustReceipt, package: &PackageReference) -> Result<()> {
    if receipt.receipt_version != TRUST_RECEIPT_VERSION || receipt.package != *package {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt identity or version is invalid",
        ));
    }
    Ok(())
}

fn validate_hash_bound_receipt(
    validated: &crate::archive::ValidatedDirectory,
    receipt: &TrustReceipt,
    package: &PackageReference,
) -> Result<()> {
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
    Ok(())
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
    prepare_locked_roots(roots)?;
    if !enabled {
        let destination = package_destination(roots, package);
        ensure_existing_tree_safe(&destination)?;
        let metadata = fs::symlink_metadata(&destination).map_err(|error| {
            let code = if error.kind() == io::ErrorKind::NotFound {
                ErrorCode::PackageMissing
            } else {
                ErrorCode::Io
            };
            ExtensionError::io(code, "inspect package before disabling", &error)
        })?;
        if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "installed package root is unsafe",
            ));
        }
    }
    let path = trust_receipt_path(roots, package);
    let mut receipt = read_trust_receipt(&path)?;
    validate_exact_receipt(&receipt, package)?;
    if enabled {
        ensure_no_other_enabled_version(roots, package, &path)?;
        verify_locked(roots, package)?;
    }
    receipt.enabled = enabled;
    write_receipt_replace(&path, &receipt)?;
    Ok(receipt)
}

fn ensure_no_other_enabled_version(
    roots: &ExtensionRoots,
    package: &PackageReference,
    exact_receipt_path: &Path,
) -> Result<()> {
    let id_root = roots.receipt_root(package.kind).join(&package.package_id);
    if !id_root.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&id_root).map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "inspect active package versions", &error)
    })? {
        let entry = entry.map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "inspect active trust receipt", &error)
        })?;
        if entry.path() == exact_receipt_path {
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
    Ok(())
}

#[derive(Debug)]
pub(crate) enum InventoryCandidate {
    Exact {
        package: PackageReference,
        destination: PathBuf,
    },
    Isolated(InstalledPackageSummary),
}

impl InventoryCandidate {
    pub(crate) fn package(&self) -> &PackageReference {
        match self {
            Self::Exact { package, .. } => package,
            Self::Isolated(summary) => &summary.package,
        }
    }
}

pub(crate) fn discover_inventory_candidates(
    roots: &ExtensionRoots,
    kinds: &[PackageKind],
) -> Result<Vec<InventoryCandidate>> {
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    let mut candidates = Vec::new();
    for kind in kinds {
        discover_kind_locked(roots, *kind, &mut candidates)?;
    }
    Ok(candidates)
}

pub(crate) fn summarize_inventory_candidate(
    roots: &ExtensionRoots,
    package: PackageReference,
    destination: &Path,
) -> Result<(InstalledPackageSummary, Option<PackageManifest>)> {
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    Ok(summarize_version_locked(roots, package, destination))
}

pub(crate) fn summarize_disabled_codec_candidate(
    roots: &ExtensionRoots,
    package: PackageReference,
    destination: &Path,
) -> Result<(InstalledPackageSummary, Option<PackageManifest>)> {
    validate_package_reference(&package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    match validate_disabled_codec_metadata_locked(roots, &package, destination) {
        Ok((receipt, validated)) => Ok((
            InstalledPackageSummary {
                package: package.clone(),
                display_name: Some(validated.manifest.display_name().to_owned()),
                publisher_name: Some(validated.manifest.publisher().name.clone()),
                enabled: receipt.enabled,
                health: PackageHealth::VerificationRequired,
                error_code: None,
                error_detail: None,
            },
            Some(validated.manifest),
        )),
        Err(error) => Ok((isolated_summary_from_error(package, &error), None)),
    }
}

fn validate_disabled_codec_metadata_locked(
    roots: &ExtensionRoots,
    package: &PackageReference,
    destination: &Path,
) -> Result<(TrustReceipt, crate::archive::ValidatedDirectory)> {
    if package.kind != PackageKind::CodecPack
        || destination != package_destination(roots, package)
        || !path_exists(destination, "installed Codec package")?
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact installed Codec package candidate is unavailable",
        ));
    }
    let receipt = read_trust_receipt(&trust_receipt_path(roots, package))?;
    if receipt.receipt_version != TRUST_RECEIPT_VERSION || receipt.package != *package {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt identity or version is invalid",
        ));
    }

    let mut retained = pin_package_root_directories(roots, destination)?;
    let (manifest_measurement, manifest_bytes, manifest_file) =
        open_and_measure_control_file(destination, PackageKind::CodecPack.manifest_name())?;
    let manifest = parse_manifest(PackageKind::CodecPack, &manifest_bytes)?;
    let (catalog_measurement, catalog_bytes, catalog_file) =
        open_and_measure_control_file(destination, "integrity.json")?;
    if catalog_measurement.sha256 != manifest.integrity().catalog_sha256 {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "integrity.json hash does not match the control manifest",
        ));
    }
    let catalog = parse_integrity_catalog(&catalog_bytes, PackageKind::CodecPack)?;
    let mut expected_files = BTreeMap::from([
        (
            PackageKind::CodecPack.manifest_name().to_owned(),
            manifest_measurement,
        ),
        ("integrity.json".to_owned(), catalog_measurement),
    ]);
    for described in catalog.files {
        expected_files.insert(
            described.path.clone(),
            crate::archive::FileMeasurement {
                path: described.path,
                byte_length: described.byte_length,
                sha256: described.sha256,
            },
        );
    }
    retained.extend(pin_package_subdirectories(
        destination,
        expected_files.keys(),
    )?);
    retained.push(manifest_file);
    retained.push(catalog_file);
    let explicit_directories = implied_directories(expected_files.keys());
    let contents = BTreeMap::from([
        (
            PackageKind::CodecPack.manifest_name().to_owned(),
            manifest_bytes,
        ),
        ("integrity.json".to_owned(), catalog_bytes),
    ]);
    let validated = validate_contract(
        PackageKind::CodecPack,
        &expected_files,
        &explicit_directories,
        &contents,
    )?;
    if validated.manifest != manifest
        || validated.manifest.reference() != *package
        || validated.manifest_sha256 != receipt.manifest_sha256
        || validated.integrity_catalog_sha256 != receipt.integrity_catalog_sha256
        || validated.manifest.publisher().name != receipt.publisher_name
        || validated.manifest.publisher().identity_claim != receipt.publisher_identity_claim
    {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "installed Codec metadata differs from its hash-bound trust receipt",
        ));
    }
    validate_directory_snapshot(destination, PackageKind::CodecPack, &validated.files)?;
    drop(retained);
    Ok((receipt, validated))
}

pub(crate) fn inventory_candidate_enabled(
    roots: &ExtensionRoots,
    package: &PackageReference,
    destination: &Path,
) -> Result<bool> {
    validate_package_reference(package)?;
    validate_roots(roots)?;
    prepare_base(roots)?;
    let _lock = acquire_lock(roots)?;
    if destination != package_destination(roots, package)
        || !path_exists(destination, "installed package")?
    {
        return Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact installed package candidate is unavailable",
        ));
    }
    let receipt = read_trust_receipt(&trust_receipt_path(roots, package))?;
    if receipt.receipt_version != TRUST_RECEIPT_VERSION || receipt.package != *package {
        return Err(ExtensionError::new(
            ErrorCode::PackageUntrusted,
            "trust receipt identity or version is invalid",
        ));
    }
    Ok(receipt.enabled)
}

fn list_kind_locked(
    roots: &ExtensionRoots,
    kind: PackageKind,
    output: &mut Vec<InstalledPackageSummary>,
    manifests: &mut BTreeMap<PackageReference, PackageManifest>,
) -> Result<()> {
    let mut candidates = Vec::new();
    discover_kind_locked(roots, kind, &mut candidates)?;
    for candidate in candidates {
        match candidate {
            InventoryCandidate::Exact {
                package,
                destination,
            } => {
                let (summary, manifest) =
                    summarize_version_locked(roots, package.clone(), &destination);
                if let Some(manifest) = manifest {
                    manifests.insert(package, manifest);
                }
                output.push(summary);
            }
            InventoryCandidate::Isolated(summary) => output.push(summary),
        }
    }
    Ok(())
}

fn discover_kind_locked(
    roots: &ExtensionRoots,
    kind: PackageKind,
    output: &mut Vec<InventoryCandidate>,
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
            output.push(InventoryCandidate::Isolated(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: "org.invalid.unreadable".to_owned(),
                    package_version: "unknown".to_owned(),
                },
                ErrorCode::Io.as_str(),
                "an installed package entry could not be read",
            )));
            continue;
        };
        package_count += 1;
        if package_count > MAX_PACKAGES_PER_KIND {
            output.push(InventoryCandidate::Isolated(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: id_entry.file_name().to_string_lossy().into_owned(),
                    package_version: "unknown".to_owned(),
                },
                ErrorCode::LifecycleConflict.as_str(),
                "additional installed package entries exceed the bounded inventory",
            )));
            break;
        }
        discover_identity_locked(kind, &id_entry, output);
    }
    Ok(())
}

fn discover_identity_locked(
    kind: PackageKind,
    id_entry: &fs::DirEntry,
    output: &mut Vec<InventoryCandidate>,
) {
    let package_id = id_entry.file_name().to_string_lossy().into_owned();
    let Ok(id_metadata) = fs::symlink_metadata(id_entry.path()) else {
        output.push(InventoryCandidate::Isolated(corrupt_summary(
            PackageReference {
                kind,
                package_id,
                package_version: "unknown".to_owned(),
            },
            ErrorCode::Io.as_str(),
            "package identity root metadata is unavailable",
        )));
        return;
    };
    if !id_metadata.is_dir() || is_reparse_or_symlink(&id_metadata) {
        output.push(InventoryCandidate::Isolated(corrupt_summary(
            PackageReference {
                kind,
                package_id,
                package_version: "unknown".to_owned(),
            },
            ErrorCode::LifecycleConflict.as_str(),
            "package identity root is unsafe",
        )));
        return;
    }
    let Ok(versions) = fs::read_dir(id_entry.path()) else {
        output.push(InventoryCandidate::Isolated(corrupt_summary(
            PackageReference {
                kind,
                package_id,
                package_version: "unknown".to_owned(),
            },
            ErrorCode::Io.as_str(),
            "installed version inventory is unavailable",
        )));
        return;
    };
    let mut version_count = 0;
    for version_entry in versions {
        let Ok(version_entry) = version_entry else {
            output.push(InventoryCandidate::Isolated(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: package_id.clone(),
                    package_version: "unknown".to_owned(),
                },
                ErrorCode::Io.as_str(),
                "an installed version entry could not be read",
            )));
            continue;
        };
        version_count += 1;
        if version_count > MAX_VERSIONS_PER_PACKAGE {
            output.push(InventoryCandidate::Isolated(corrupt_summary(
                PackageReference {
                    kind,
                    package_id: package_id.clone(),
                    package_version: version_entry.file_name().to_string_lossy().into_owned(),
                },
                ErrorCode::LifecycleConflict.as_str(),
                "additional installed versions exceed the bounded inventory",
            )));
            break;
        }
        output.push(InventoryCandidate::Exact {
            package: PackageReference {
                kind,
                package_id: package_id.clone(),
                package_version: version_entry.file_name().to_string_lossy().into_owned(),
            },
            destination: version_entry.path(),
        });
    }
}

fn summarize_version_locked(
    roots: &ExtensionRoots,
    package: PackageReference,
    destination: &Path,
) -> (InstalledPackageSummary, Option<PackageManifest>) {
    let receipt = match path_exists(destination, "installed package") {
        Ok(true) => read_trust_receipt(&trust_receipt_path(roots, &package)).and_then(|receipt| {
            if receipt.receipt_version == TRUST_RECEIPT_VERSION && receipt.package == package {
                Ok(receipt)
            } else {
                Err(ExtensionError::new(
                    ErrorCode::PackageUntrusted,
                    "trust receipt identity or version is invalid",
                ))
            }
        }),
        Ok(false) => Err(ExtensionError::new(
            ErrorCode::PackageMissing,
            "exact package version is not installed",
        )),
        Err(error) => Err(error),
    };
    let tree = validate_directory(destination, Some(package.kind));
    match (receipt, tree) {
        (Ok(receipt), Ok(validated))
            if validated.manifest.reference() == package
                && validated.manifest_sha256 == receipt.manifest_sha256
                && validated.integrity_catalog_sha256 == receipt.integrity_catalog_sha256
                && validated.manifest.publisher().name == receipt.publisher_name
                && validated.manifest.publisher().identity_claim
                    == receipt.publisher_identity_claim =>
        {
            let summary = InstalledPackageSummary {
                package: package.clone(),
                display_name: Some(validated.manifest.display_name().to_owned()),
                publisher_name: Some(validated.manifest.publisher().name.clone()),
                enabled: receipt.enabled,
                health: PackageHealth::Healthy,
                error_code: None,
                error_detail: None,
            };
            (summary, Some(validated.manifest))
        }
        (Ok(_), Ok(validated)) => (
            InstalledPackageSummary {
                package,
                display_name: Some(validated.manifest.display_name().to_owned()),
                publisher_name: Some(validated.manifest.publisher().name.clone()),
                enabled: false,
                health: PackageHealth::Corrupt,
                error_code: Some(ErrorCode::IntegrityFailed.as_str().to_owned()),
                error_detail: Some(
                    "installed package differs from its hash-bound trust receipt".to_owned(),
                ),
            },
            None,
        ),
        (receipt, tree) => {
            let (error, validated) = match (receipt, tree) {
                (Err(error), tree) => (error, tree.ok()),
                (Ok(_), Err(error)) => (error, None),
                (Ok(_), Ok(_)) => unreachable!("healthy and receipt-mismatch arms are exhaustive"),
            };
            let (display_name, publisher_name) = validated.map_or((None, None), |validated| {
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
            (
                InstalledPackageSummary {
                    package,
                    display_name,
                    publisher_name,
                    enabled: false,
                    health,
                    error_code: Some(error.code().as_str().to_owned()),
                    error_detail: Some(error.detail().to_owned()),
                },
                None,
            )
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

fn isolated_summary_from_error(
    package: PackageReference,
    error: &ExtensionError,
) -> InstalledPackageSummary {
    InstalledPackageSummary {
        package,
        display_name: None,
        publisher_name: None,
        enabled: false,
        health: if error.code() == ErrorCode::PackageUntrusted {
            PackageHealth::Untrusted
        } else {
            PackageHealth::Corrupt
        },
        error_code: Some(error.code().as_str().to_owned()),
        error_detail: Some(error.detail().to_owned()),
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
    fn pinned_hash_rejects_bytes_changed_from_the_expected_catalog() {
        let temp = TempDir::new().expect("temp");
        let destination = temp.path().join("com.example.deck").join("0.2.0");
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

        let file = open_pinned_file(&runtime_file).expect("open pinned fixture");
        let error = hash_pinned_jobs(vec![PinnedHashJob {
            index: 0,
            expected,
            keep_bytes: false,
            file,
        }])
        .expect_err("unexpected bytes must not produce an active package");
        assert_eq!(error.code(), ErrorCode::IntegrityFailed);
    }
}
