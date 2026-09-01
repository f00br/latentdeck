//! Native, version-scoped H3 Codec Pack lifecycle operations.

use std::collections::HashSet;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use latentdeck_core::codec_pack::validate_codec_pack_directory;
use semver::Version;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive};

const PACK_ID: &str = "org.latentdeck.h3";
const APPLICATION_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_ARCHIVE_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 32_770;
const MAX_UNCOMPRESSED_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const MAX_VERSIONS: usize = 16;
const MAX_STALE_STAGING_ENTRIES: usize = 64;
const MAX_PROBED_TREE_ENTRIES: usize = 131_072;
const MAX_PROBED_TREE_DEPTH: usize = 256;
const FREE_SPACE_OVERHEAD_BYTES: u64 = 64 * 1024 * 1024;

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
/// Stable process exit code for a quarantined version that remains in use.
pub const EXIT_IN_USE: u8 = 50;

/// Explicit install, staging, trash, and conflict roots.
#[derive(Debug, Clone)]
pub struct LifecycleRoots {
    pub install_root: PathBuf,
    pub other_scope_root: Option<PathBuf>,
    pub staging_root: PathBuf,
    pub trash_root: PathBuf,
    pub lock_path: PathBuf,
}

impl LifecycleRoots {
    /// Derive sibling work roots for an explicit discovery root.
    #[must_use]
    pub fn for_install_root(
        install_root: impl Into<PathBuf>,
        other_scope_root: Option<PathBuf>,
    ) -> Self {
        let install_root = install_root.into();
        let parent = install_root.parent().unwrap_or_else(|| Path::new("."));
        Self {
            staging_root: parent.join("CodecPackStaging"),
            trash_root: parent.join("CodecPackTrash"),
            lock_path: parent.join("CodecPackLifecycle.lock"),
            install_root,
            other_scope_root,
        }
    }

    /// Build fixed current-user and all-users discovery roots from explicit
    /// Windows known-folder paths supplied by the trusted installer wrapper.
    #[must_use]
    pub fn from_known_folders(
        local_app_data: impl Into<PathBuf>,
        program_data: impl Into<PathBuf>,
    ) -> Self {
        let install_root = local_app_data.into().join("LatentDeck/CodecPacks");
        let other_scope_root = program_data.into().join("LatentDeck/CodecPacks");
        Self::for_install_root(install_root, Some(other_scope_root))
    }
}

/// Immutable archive identity required by the installer wrapper.
#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub archive_path: PathBuf,
    pub expected_sha256: String,
    pub expected_length: u64,
    pub expected_version: String,
}

/// Successful install identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReceipt {
    pub destination: PathBuf,
    pub archive_sha256: String,
    pub archive_length: u64,
    pub extracted_files: usize,
    pub extracted_bytes: u64,
}

/// Successful uninstall identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallReceipt {
    pub removed_version: String,
    pub cleaned_quarantine: bool,
}

/// Lifecycle failure with a stable CLI exit class.
#[derive(Debug, thiserror::Error)]
pub enum LifecycleError {
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("archive is invalid: {0}")]
    ArchiveInvalid(String),
    #[error("Codec Pack is invalid: {0}")]
    PackInvalid(String),
    #[error("Codec Pack {0} is already installed")]
    AlreadyInstalled(String),
    #[error("Codec Pack {0} is not installed")]
    NotInstalled(String),
    #[error("lifecycle conflict: {0}")]
    Conflict(String),
    #[error("lifecycle operation is busy: {0}")]
    Busy(String),
    #[error("Codec Pack is in use: {0}")]
    InUse(String),
    #[error("Codec Pack was quarantined but cleanup failed at {path}: {detail}")]
    Quarantined { path: PathBuf, detail: String },
}

impl LifecycleError {
    /// Stable process exit code for NSIS integration.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidArguments(_) => EXIT_INVALID_ARGUMENTS,
            Self::ArchiveInvalid(_) | Self::PackInvalid(_) => EXIT_INVALID_PACK,
            Self::AlreadyInstalled(_) => EXIT_ALREADY_INSTALLED,
            Self::NotInstalled(_) => EXIT_NOT_INSTALLED,
            Self::Conflict(_) | Self::Busy(_) => EXIT_CONFLICT,
            Self::InUse(_) | Self::Quarantined { .. } => EXIT_IN_USE,
        }
    }
}

#[derive(Debug)]
struct ArchiveEntryPlan {
    index: usize,
    relative: PathBuf,
    normalized: String,
    is_directory: bool,
    uncompressed_size: u64,
}

#[derive(Debug)]
struct ArchivePlan {
    entries: Vec<ArchiveEntryPlan>,
    file_count: usize,
    uncompressed_bytes: u64,
}

struct PreparedArchive {
    archive: ZipArchive<File>,
    plan: ArchivePlan,
    measured_sha256: String,
    archive_length: u64,
}

struct LifecycleLock {
    file: File,
}

impl Drop for LifecycleLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

struct StagingGuard {
    path: Option<PathBuf>,
    parent: PathBuf,
}

impl StagingGuard {
    fn publish(&mut self) {
        self.path = None;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = remove_temporary_tree(&self.parent, &path, ".install-");
        }
        remove_directory_if_empty(&self.parent);
    }
}

/// Install one immutable H3 Codec Pack version.
///
/// # Errors
///
/// Returns [`LifecycleError`] when the request, roots, archive, extracted pack,
/// free-space preflight, lifecycle serialization, or atomic publication fails.
pub fn install(
    roots: &LifecycleRoots,
    request: &InstallRequest,
) -> Result<InstallReceipt, LifecycleError> {
    validate_install_request(request)?;
    validate_roots(roots)?;
    prepare_lifecycle_parent(roots)?;
    let _lock = acquire_lock(roots)?;
    reject_other_scope_conflict(roots, &request.expected_version)?;
    let mut prepared = prepare_archive(request)?;
    cleanup_stale_staging(roots)?;
    let destination = preflight_install_destination(roots, request, &mut prepared)?;
    install_archive(roots, request, destination, prepared)
}

fn preflight_install_destination(
    roots: &LifecycleRoots,
    request: &InstallRequest,
    prepared: &mut PreparedArchive,
) -> Result<PathBuf, LifecycleError> {
    let pack_parent = roots.install_root.join(PACK_ID);
    ensure_safe_directory(&roots.install_root, true)?;
    ensure_safe_directory(&pack_parent, true)?;
    let destination = pack_parent.join(&request.expected_version);
    ensure_direct_child(&pack_parent, &destination)?;

    if exact_path_exists_fail_closed(&destination, "installed Codec Pack destination")? {
        let existing =
            validate_codec_pack_directory(&destination, APPLICATION_VERSION).map_err(|error| {
                LifecycleError::PackInvalid(format!(
                    "installed {} is corrupt; remove it explicitly: {}",
                    request.expected_version, error.code
                ))
            })?;
        if existing.manifest.pack_id == PACK_ID
            && existing.manifest.pack_version == request.expected_version
        {
            if archive_tree_matches_directory(&mut prepared.archive, &prepared.plan, &destination)?
            {
                return Err(LifecycleError::AlreadyInstalled(
                    request.expected_version.clone(),
                ));
            }
            return Err(LifecycleError::Conflict(format!(
                "installed {} has different files from the bound archive",
                request.expected_version
            )));
        }
        return Err(LifecycleError::PackInvalid(
            "installed directory identity does not match its path".to_owned(),
        ));
    }

    cleanup_matching_quarantines(roots, &request.expected_version)?;
    enforce_version_limit(&pack_parent)?;
    Ok(destination)
}

fn prepare_archive(request: &InstallRequest) -> Result<PreparedArchive, LifecycleError> {
    let mut archive_file = open_archive(&request.archive_path)?;
    let archive_length = archive_file
        .metadata()
        .map_err(|error| archive_io_error("archive metadata", &error))?
        .len();
    if archive_length != request.expected_length {
        return Err(LifecycleError::ArchiveInvalid(format!(
            "byte length mismatch: expected {}, found {archive_length}",
            request.expected_length
        )));
    }
    let measured_sha256 = hash_open_file(&mut archive_file)?;
    if measured_sha256 != request.expected_sha256 {
        return Err(LifecycleError::ArchiveInvalid(format!(
            "SHA-256 mismatch: found {measured_sha256}"
        )));
    }
    archive_file
        .seek(SeekFrom::Start(0))
        .map_err(|error| archive_io_error("rewind archive", &error))?;
    let mut archive = ZipArchive::new(archive_file)
        .map_err(|error| LifecycleError::ArchiveInvalid(error.to_string()))?;
    let plan = inspect_archive(&mut archive)?;
    Ok(PreparedArchive {
        archive,
        plan,
        measured_sha256,
        archive_length,
    })
}

fn install_archive(
    roots: &LifecycleRoots,
    request: &InstallRequest,
    destination: PathBuf,
    mut prepared: PreparedArchive,
) -> Result<InstallReceipt, LifecycleError> {
    ensure_safe_directory(&roots.staging_root, true)?;
    let required_space = prepared
        .plan
        .uncompressed_bytes
        .checked_add(FREE_SPACE_OVERHEAD_BYTES)
        .ok_or_else(|| LifecycleError::ArchiveInvalid("size total overflowed".to_owned()))?;
    let available_space = fs2::available_space(&roots.staging_root).map_err(|error| {
        LifecycleError::Conflict(format!("could not measure destination free space: {error}"))
    })?;
    if available_space < required_space {
        return Err(LifecycleError::Conflict(format!(
            "insufficient free space: need {required_space} bytes, found {available_space}"
        )));
    }

    let staging_path = roots
        .staging_root
        .join(format!(".install-{}", Uuid::new_v4().simple()));
    ensure_direct_child(&roots.staging_root, &staging_path)?;
    fs::create_dir(&staging_path).map_err(|error| {
        LifecycleError::Conflict(format!("could not create extraction staging: {error}"))
    })?;
    let mut staging_guard = StagingGuard {
        path: Some(staging_path.clone()),
        parent: roots.staging_root.clone(),
    };

    extract_archive(&mut prepared.archive, &prepared.plan, &staging_path)?;
    let validated = validate_codec_pack_directory(&staging_path, APPLICATION_VERSION)
        .map_err(|error| LifecycleError::PackInvalid(error.code.to_owned()))?;
    if validated.manifest.pack_id != PACK_ID
        || validated.manifest.pack_version != request.expected_version
    {
        return Err(LifecycleError::PackInvalid(
            "manifest identity does not match the requested H3 version".to_owned(),
        ));
    }

    if exact_path_exists_fail_closed(&destination, "publication destination")? {
        return Err(LifecycleError::Conflict(
            "destination appeared during installation".to_owned(),
        ));
    }
    fs::rename(&staging_path, &destination)
        .map_err(|error| LifecycleError::Conflict(format!("atomic publication failed: {error}")))?;
    if let Err(error) = validate_codec_pack_directory(&destination, APPLICATION_VERSION) {
        fs::rename(&destination, &staging_path).map_err(|rollback_error| {
            LifecycleError::Conflict(format!(
                "post-publication validation failed ({}) and rollback failed: {rollback_error}",
                error.code
            ))
        })?;
        return Err(LifecycleError::PackInvalid(format!(
            "post-publication validation failed: {}",
            error.code
        )));
    }
    staging_guard.publish();
    remove_directory_if_empty(&roots.staging_root);

    Ok(InstallReceipt {
        destination,
        archive_sha256: prepared.measured_sha256,
        archive_length: prepared.archive_length,
        extracted_files: prepared.plan.file_count,
        extracted_bytes: prepared.plan.uncompressed_bytes,
    })
}

/// Uninstall one exact H3 Codec Pack version.
///
/// # Errors
///
/// Returns [`LifecycleError`] when the version or roots are invalid, another
/// lifecycle operation conflicts, the healthy-pack validation fails, or exact
/// quarantine/removal cannot complete.
pub fn uninstall(
    roots: &LifecycleRoots,
    version: &str,
    remove_corrupt: bool,
) -> Result<UninstallReceipt, LifecycleError> {
    validate_semver(version, "version")?;
    validate_roots(roots)?;
    prepare_lifecycle_parent(roots)?;
    let _lock = acquire_lock(roots)?;

    let pack_parent = roots.install_root.join(PACK_ID);
    let destination = pack_parent.join(version);
    ensure_direct_child(&pack_parent, &destination)?;
    let destination_exists =
        exact_path_exists_fail_closed(&destination, "installed Codec Pack destination")?;
    let quarantines = matching_quarantines(roots, version)?;

    if !destination_exists {
        if quarantines.is_empty() {
            return Err(LifecycleError::NotInstalled(version.to_owned()));
        }
        cleanup_quarantines(roots, version, quarantines)?;
        return Ok(UninstallReceipt {
            removed_version: version.to_owned(),
            cleaned_quarantine: true,
        });
    }
    if !quarantines.is_empty() {
        return Err(LifecycleError::Conflict(
            "an older quarantine exists for the same version".to_owned(),
        ));
    }

    ensure_safe_directory(&roots.install_root, false)?;
    ensure_safe_directory(&pack_parent, false)?;
    ensure_safe_directory(&destination, false)?;
    if !remove_corrupt {
        let validated =
            validate_codec_pack_directory(&destination, APPLICATION_VERSION).map_err(|error| {
                if tree_contains_in_use_file(&destination) {
                    LifecycleError::InUse("stop Codec Pack workers and retry uninstall".to_owned())
                } else {
                    LifecycleError::PackInvalid(error.code.to_owned())
                }
            })?;
        if validated.manifest.pack_id != PACK_ID || validated.manifest.pack_version != version {
            return Err(LifecycleError::PackInvalid(
                "installed directory identity does not match the requested version".to_owned(),
            ));
        }
    }

    ensure_safe_directory(&roots.trash_root, true)?;
    let prefix = quarantine_prefix(version);
    let quarantine = roots
        .trash_root
        .join(format!("{prefix}{}", Uuid::new_v4().simple()));
    ensure_direct_child(&roots.trash_root, &quarantine)?;
    fs::rename(&destination, &quarantine).map_err(|error| {
        if is_in_use_error(&error) {
            LifecycleError::InUse("stop Codec Pack workers and retry uninstall".to_owned())
        } else {
            LifecycleError::Conflict(format!("could not quarantine exact version: {error}"))
        }
    })?;
    if let Err(error) = remove_temporary_tree(&roots.trash_root, &quarantine, &prefix) {
        return Err(LifecycleError::Quarantined {
            path: quarantine,
            detail: error.to_string(),
        });
    }

    remove_directory_if_empty(&pack_parent);
    remove_directory_if_empty(&roots.trash_root);
    Ok(UninstallReceipt {
        removed_version: version.to_owned(),
        cleaned_quarantine: false,
    })
}

fn validate_install_request(request: &InstallRequest) -> Result<(), LifecycleError> {
    validate_semver(&request.expected_version, "expected-version")?;
    if request.expected_length == 0 || request.expected_length > MAX_ARCHIVE_BYTES {
        return Err(LifecycleError::InvalidArguments(
            "expected-length is outside the supported archive bound".to_owned(),
        ));
    }
    if request.expected_sha256.len() != 64
        || !request
            .expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LifecycleError::InvalidArguments(
            "expected-sha256 must be canonical lowercase hexadecimal".to_owned(),
        ));
    }
    Ok(())
}

fn validate_semver(value: &str, name: &str) -> Result<(), LifecycleError> {
    let version = Version::parse(value).map_err(|_| {
        LifecycleError::InvalidArguments(format!("{name} must be canonical SemVer"))
    })?;
    if version.to_string() != value {
        return Err(LifecycleError::InvalidArguments(format!(
            "{name} must be canonical SemVer"
        )));
    }
    Ok(())
}

fn validate_roots(roots: &LifecycleRoots) -> Result<(), LifecycleError> {
    for (name, path) in [
        ("install root", &roots.install_root),
        ("staging root", &roots.staging_root),
        ("trash root", &roots.trash_root),
        ("lock path", &roots.lock_path),
    ] {
        validate_absolute_normal_path(path, name)?;
    }
    if let Some(other) = &roots.other_scope_root {
        validate_absolute_normal_path(other, "other-scope root")?;
        if paths_equal(other, &roots.install_root) {
            return Err(LifecycleError::InvalidArguments(
                "other-scope root overlaps the install root".to_owned(),
            ));
        }
    }
    let parent = roots.install_root.parent().ok_or_else(|| {
        LifecycleError::InvalidArguments("install root has no safe parent".to_owned())
    })?;
    for auxiliary in [&roots.staging_root, &roots.trash_root, &roots.lock_path] {
        if auxiliary.parent() != Some(parent) || paths_equal(auxiliary, &roots.install_root) {
            return Err(LifecycleError::InvalidArguments(
                "lifecycle work roots must be distinct install-root siblings".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_absolute_normal_path(path: &Path, name: &str) -> Result<(), LifecycleError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(LifecycleError::InvalidArguments(format!(
            "{name} must be an absolute normal path"
        )));
    }
    Ok(())
}

fn prepare_lifecycle_parent(roots: &LifecycleRoots) -> Result<(), LifecycleError> {
    let parent = roots.install_root.parent().ok_or_else(|| {
        LifecycleError::InvalidArguments("install root has no safe parent".to_owned())
    })?;
    ensure_existing_components_safe(parent)?;
    fs::create_dir_all(parent).map_err(|error| {
        LifecycleError::Conflict(format!("could not create lifecycle parent: {error}"))
    })?;
    ensure_existing_components_safe(parent)
}

fn cleanup_stale_staging(roots: &LifecycleRoots) -> Result<(), LifecycleError> {
    match fs::symlink_metadata(&roots.staging_root) {
        Ok(_) => ensure_safe_directory(&roots.staging_root, false)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(LifecycleError::Conflict(format!(
                "could not inspect extraction staging: {error}"
            )));
        }
    }
    let mut entry_count = 0usize;
    for entry in fs::read_dir(&roots.staging_root).map_err(|error| {
        LifecycleError::Conflict(format!("could not inspect extraction staging: {error}"))
    })? {
        entry_count = entry_count
            .checked_add(1)
            .ok_or_else(|| LifecycleError::Conflict("staging entry count overflowed".to_owned()))?;
        if entry_count > MAX_STALE_STAGING_ENTRIES {
            return Err(LifecycleError::Conflict(format!(
                "staging contains more than {MAX_STALE_STAGING_ENTRIES} entries"
            )));
        }
        let entry = entry.map_err(|error| {
            LifecycleError::Conflict(format!("could not inspect extraction staging: {error}"))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_exact_staging_name(&name) {
            continue;
        }
        let candidate = entry.path();
        let metadata = fs::symlink_metadata(&candidate).map_err(|error| {
            LifecycleError::Conflict(format!("could not inspect stale staging: {error}"))
        })?;
        if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
            return Err(LifecycleError::Conflict(
                "stale staging target is not a regular directory".to_owned(),
            ));
        }
        remove_temporary_tree(&roots.staging_root, &candidate, ".install-").map_err(|error| {
            LifecycleError::Conflict(format!("could not clean stale staging: {error}"))
        })?;
    }
    remove_directory_if_empty(&roots.staging_root);
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

fn acquire_lock(roots: &LifecycleRoots) -> Result<LifecycleLock, LifecycleError> {
    match fs::symlink_metadata(&roots.lock_path) {
        Ok(metadata) if !metadata.is_file() || is_reparse_or_symlink(&metadata) => {
            return Err(LifecycleError::Conflict(
                "lifecycle lock path is unsafe".to_owned(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(LifecycleError::Conflict(format!(
                "could not inspect lifecycle lock: {error}"
            )));
        }
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(3);
    }
    let file = options.open(&roots.lock_path).map_err(|error| {
        LifecycleError::Conflict(format!("could not open lifecycle lock: {error}"))
    })?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(LifecycleLock { file }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(LifecycleError::Busy(
            "another Codec Pack lifecycle operation is active".to_owned(),
        )),
        Err(error) => Err(LifecycleError::Conflict(format!(
            "could not acquire lifecycle lock: {error}"
        ))),
    }
}

fn ensure_safe_directory(path: &Path, create: bool) -> Result<(), LifecycleError> {
    ensure_existing_components_safe(path)?;
    if create {
        fs::create_dir_all(path).map_err(|error| {
            LifecycleError::Conflict(format!("could not create directory: {error}"))
        })?;
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        LifecycleError::Conflict(format!("required directory is unavailable: {error}"))
    })?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(LifecycleError::Conflict(
            "lifecycle directory is not a regular directory".to_owned(),
        ));
    }
    ensure_existing_components_safe(path)
}

fn ensure_existing_components_safe(path: &Path) -> Result<(), LifecycleError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if is_reparse_or_symlink(&metadata) {
                    return Err(LifecycleError::Conflict(
                        "path contains a reparse-point component".to_owned(),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(LifecycleError::Conflict(format!(
                    "could not inspect path component: {error}"
                )));
            }
        }
    }
    Ok(())
}

fn ensure_direct_child(parent: &Path, child: &Path) -> Result<(), LifecycleError> {
    if child.parent() != Some(parent) {
        return Err(LifecycleError::InvalidArguments(
            "lifecycle target escaped its exact parent".to_owned(),
        ));
    }
    Ok(())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

fn reject_other_scope_conflict(
    roots: &LifecycleRoots,
    version: &str,
) -> Result<(), LifecycleError> {
    if let Some(other_root) = &roots.other_scope_root {
        let candidate = other_root.join(PACK_ID).join(version);
        if exact_path_exists_fail_closed(&candidate, "all-users Codec Pack candidate")? {
            return Err(LifecycleError::Conflict(
                "the same Codec Pack version exists in the all-users scope".to_owned(),
            ));
        }
    }
    Ok(())
}

fn exact_path_exists_fail_closed(path: &Path, context: &str) -> Result<bool, LifecycleError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) => classify_path_probe_error(&error, context),
    }
}

fn classify_path_probe_error(error: &io::Error, context: &str) -> Result<bool, LifecycleError> {
    if error.kind() == io::ErrorKind::NotFound {
        Ok(false)
    } else {
        Err(LifecycleError::Conflict(format!(
            "could not inspect {context}: {error}"
        )))
    }
}

fn enforce_version_limit(pack_parent: &Path) -> Result<(), LifecycleError> {
    let mut versions = 0usize;
    for entry in fs::read_dir(pack_parent).map_err(|error| {
        LifecycleError::Conflict(format!("could not inspect installed versions: {error}"))
    })? {
        let entry = entry.map_err(|error| {
            LifecycleError::Conflict(format!("could not inspect installed version: {error}"))
        })?;
        let metadata = entry.metadata().map_err(|error| {
            LifecycleError::Conflict(format!("could not inspect installed version: {error}"))
        })?;
        if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
            return Err(LifecycleError::Conflict(
                "installed pack root contains an unsafe entry".to_owned(),
            ));
        }
        versions = versions.checked_add(1).ok_or_else(|| {
            LifecycleError::Conflict("installed version count overflowed".to_owned())
        })?;
        if versions >= MAX_VERSIONS {
            return Err(LifecycleError::Conflict(format!(
                "the maximum of {MAX_VERSIONS} installed versions is already present"
            )));
        }
    }
    Ok(())
}

fn open_archive(path: &Path) -> Result<File, LifecycleError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| archive_io_error("open archive metadata", &error))?;
    if !metadata.is_file() || is_reparse_or_symlink(&metadata) {
        return Err(LifecycleError::ArchiveInvalid(
            "archive must be a regular non-reparse file".to_owned(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(LifecycleError::ArchiveInvalid(
            "archive is empty or exceeds 20 GiB".to_owned(),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    options
        .open(path)
        .map_err(|error| archive_io_error("open archive exclusively", &error))
}

fn archive_io_error(context: &str, error: &io::Error) -> LifecycleError {
    LifecycleError::ArchiveInvalid(format!("{context}: {error}"))
}

fn hash_open_file(file: &mut File) -> Result<String, LifecycleError> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024].into_boxed_slice();
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| archive_io_error("hash archive", &error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn inspect_archive(file: &mut ZipArchive<File>) -> Result<ArchivePlan, LifecycleError> {
    let entry_count = file.len();
    if entry_count == 0 || entry_count > MAX_ARCHIVE_ENTRIES {
        return Err(LifecycleError::ArchiveInvalid(format!(
            "entry count must be between 1 and {MAX_ARCHIVE_ENTRIES}"
        )));
    }
    let mut entries = Vec::with_capacity(entry_count);
    let mut normalized_paths = HashSet::with_capacity(entry_count);
    let mut uncompressed_bytes = 0u64;
    let mut file_count = 0usize;
    for index in 0..entry_count {
        let entry = file
            .by_index(index)
            .map_err(|error| LifecycleError::ArchiveInvalid(error.to_string()))?;
        let raw_name = entry.name_raw();
        let name = std::str::from_utf8(raw_name).map_err(|_| {
            LifecycleError::ArchiveInvalid("entry name is not strict UTF-8".to_owned())
        })?;
        let is_directory = entry.is_dir();
        let relative = validate_archive_path(name, is_directory)?;
        let normalized = relative.to_string_lossy().replace('\\', "/").to_lowercase();
        if !normalized_paths.insert(normalized.clone()) {
            return Err(LifecycleError::ArchiveInvalid(
                "archive contains a case-insensitive duplicate path".to_owned(),
            ));
        }
        let compression = entry.compression();
        if compression != CompressionMethod::Stored && compression != CompressionMethod::Deflated {
            return Err(LifecycleError::ArchiveInvalid(
                "archive uses an unsupported compression method".to_owned(),
            ));
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170_000;
            let expected_kind = if is_directory { 0o040_000 } else { 0o100_000 };
            if kind != 0 && kind != expected_kind {
                return Err(LifecycleError::ArchiveInvalid(
                    "archive contains a symlink or special filesystem entry".to_owned(),
                ));
            }
        }
        let size = entry.size();
        uncompressed_bytes = uncompressed_bytes.checked_add(size).ok_or_else(|| {
            LifecycleError::ArchiveInvalid("uncompressed size overflowed".to_owned())
        })?;
        if uncompressed_bytes > MAX_UNCOMPRESSED_BYTES {
            return Err(LifecycleError::ArchiveInvalid(
                "uncompressed archive exceeds 20 GiB".to_owned(),
            ));
        }
        if !is_directory {
            file_count = file_count.checked_add(1).ok_or_else(|| {
                LifecycleError::ArchiveInvalid("file count overflowed".to_owned())
            })?;
        }
        entries.push(ArchiveEntryPlan {
            index,
            relative,
            normalized,
            is_directory,
            uncompressed_size: size,
        });
    }
    entries.sort_by(|left, right| left.normalized.cmp(&right.normalized));
    for adjacent in entries.windows(2) {
        let left = &adjacent[0];
        let right = &adjacent[1];
        if !left.is_directory
            && right
                .normalized
                .starts_with(&(left.normalized.clone() + "/"))
        {
            return Err(LifecycleError::ArchiveInvalid(
                "archive path is both a file and a directory parent".to_owned(),
            ));
        }
    }
    entries.sort_by_key(|entry| entry.index);
    Ok(ArchivePlan {
        entries,
        file_count,
        uncompressed_bytes,
    })
}

fn archive_tree_matches_directory(
    archive: &mut ZipArchive<File>,
    plan: &ArchivePlan,
    installed_root: &Path,
) -> Result<bool, LifecycleError> {
    if count_regular_files_no_follow(installed_root)? != plan.file_count {
        return Ok(false);
    }
    let mut archive_buffer = vec![0u8; 1024 * 1024].into_boxed_slice();
    let mut installed_buffer = vec![0u8; 1024 * 1024].into_boxed_slice();
    for planned in plan.entries.iter().filter(|entry| !entry.is_directory) {
        let installed_path = installed_root.join(&planned.relative);
        if !installed_path.starts_with(installed_root) {
            return Err(LifecycleError::ArchiveInvalid(
                "archive entry escaped the installed comparison root".to_owned(),
            ));
        }
        let metadata = match fs::symlink_metadata(&installed_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(LifecycleError::Conflict(format!(
                    "could not inspect installed comparison file: {error}"
                )));
            }
        };
        if !metadata.is_file()
            || is_reparse_or_symlink(&metadata)
            || metadata.len() != planned.uncompressed_size
        {
            return Ok(false);
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(7);
        }
        let mut installed = options.open(&installed_path).map_err(|error| {
            LifecycleError::Conflict(format!("could not read installed comparison file: {error}"))
        })?;
        let mut entry = archive
            .by_index(planned.index)
            .map_err(|error| LifecycleError::ArchiveInvalid(error.to_string()))?;
        let mut archive_total = 0u64;
        loop {
            let read = entry
                .read(&mut archive_buffer)
                .map_err(|error| LifecycleError::ArchiveInvalid(error.to_string()))?;
            if read == 0 {
                break;
            }
            archive_total = archive_total.checked_add(read as u64).ok_or_else(|| {
                LifecycleError::ArchiveInvalid("archive entry length overflowed".to_owned())
            })?;
            installed
                .read_exact(&mut installed_buffer[..read])
                .map_err(|error| {
                    LifecycleError::Conflict(format!(
                        "could not compare installed file content: {error}"
                    ))
                })?;
            if archive_buffer[..read] != installed_buffer[..read] {
                return Ok(false);
            }
        }
        if archive_total != planned.uncompressed_size {
            return Err(LifecycleError::ArchiveInvalid(
                "archive entry length differs from its central directory".to_owned(),
            ));
        }
    }
    Ok(true)
}

fn count_regular_files_no_follow(root: &Path) -> Result<usize, LifecycleError> {
    let mut directories = vec![root.to_path_buf()];
    let mut seen = 0usize;
    let mut files = 0usize;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory).map_err(|error| {
            LifecycleError::Conflict(format!("could not inspect installed tree: {error}"))
        })? {
            seen = seen.checked_add(1).ok_or_else(|| {
                LifecycleError::PackInvalid("installed tree entry count overflowed".to_owned())
            })?;
            if seen > MAX_PROBED_TREE_ENTRIES {
                return Err(LifecycleError::PackInvalid(
                    "installed tree exceeds the comparison bound".to_owned(),
                ));
            }
            let entry = entry.map_err(|error| {
                LifecycleError::Conflict(format!("could not inspect installed tree: {error}"))
            })?;
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                LifecycleError::Conflict(format!("could not inspect installed tree: {error}"))
            })?;
            if is_reparse_or_symlink(&metadata) {
                return Err(LifecycleError::PackInvalid(
                    "installed tree contains a reparse point".to_owned(),
                ));
            }
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() {
                files = files.checked_add(1).ok_or_else(|| {
                    LifecycleError::PackInvalid("installed file count overflowed".to_owned())
                })?;
            } else {
                return Err(LifecycleError::PackInvalid(
                    "installed tree contains a special filesystem entry".to_owned(),
                ));
            }
        }
    }
    Ok(files)
}

fn validate_archive_path(name: &str, is_directory: bool) -> Result<PathBuf, LifecycleError> {
    if name.is_empty()
        || name.len() > 4096
        || name.contains('\0')
        || name.contains('\\')
        || name.starts_with('/')
        || (!is_directory && name.ends_with('/'))
    {
        return Err(LifecycleError::ArchiveInvalid(
            "entry path is empty, oversized, or non-portable".to_owned(),
        ));
    }
    let normalized_name = if is_directory {
        name.strip_suffix('/').ok_or_else(|| {
            LifecycleError::ArchiveInvalid("directory entry lacks a trailing slash".to_owned())
        })?
    } else {
        name
    };
    if normalized_name.is_empty() {
        return Err(LifecycleError::ArchiveInvalid(
            "archive contains an empty directory name".to_owned(),
        ));
    }
    for component in normalized_name.split('/') {
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.encode_utf16().count() > 255
            || component.contains(':')
            || component.chars().any(is_forbidden_windows_character)
            || component.ends_with('.')
            || component.ends_with(' ')
            || is_reserved_windows_name(component)
        {
            return Err(LifecycleError::ArchiveInvalid(
                "archive entry has an unsafe Windows path component".to_owned(),
            ));
        }
    }
    let relative = PathBuf::from(normalized_name);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(LifecycleError::ArchiveInvalid(
            "archive entry escaped its extraction root".to_owned(),
        ));
    }
    Ok(relative)
}

fn is_forbidden_windows_character(character: char) -> bool {
    matches!(character, '<' | '>' | '"' | '|' | '?' | '*')
        || ('\u{1}'..='\u{1f}').contains(&character)
}

fn is_reserved_windows_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_lowercase();
    matches!(stem.as_str(), "con" | "prn" | "aux" | "nul" | "clock$")
        || stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                    || matches!(suffix, "¹" | "²" | "³")
            })
}

fn extract_archive(
    archive: &mut ZipArchive<File>,
    plan: &ArchivePlan,
    staging_root: &Path,
) -> Result<(), LifecycleError> {
    for planned in &plan.entries {
        let destination = staging_root.join(&planned.relative);
        if !destination.starts_with(staging_root) {
            return Err(LifecycleError::ArchiveInvalid(
                "archive entry escaped extraction staging".to_owned(),
            ));
        }
        if planned.is_directory {
            fs::create_dir_all(&destination).map_err(|error| {
                LifecycleError::ArchiveInvalid(format!("create archive directory: {error}"))
            })?;
            continue;
        }
        let parent = destination.parent().ok_or_else(|| {
            LifecycleError::ArchiveInvalid("archive file has no parent".to_owned())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            LifecycleError::ArchiveInvalid(format!("create archive parent: {error}"))
        })?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|error| {
                LifecycleError::ArchiveInvalid(format!("create archive file: {error}"))
            })?;
        let entry = archive
            .by_index(planned.index)
            .map_err(|error| LifecycleError::ArchiveInvalid(error.to_string()))?;
        let mut bounded = entry.take(planned.uncompressed_size.saturating_add(1));
        let copied = io::copy(&mut bounded, &mut output).map_err(|error| {
            LifecycleError::ArchiveInvalid(format!("extract archive file: {error}"))
        })?;
        output.flush().map_err(|error| {
            LifecycleError::ArchiveInvalid(format!("flush archive file: {error}"))
        })?;
        if copied != planned.uncompressed_size {
            return Err(LifecycleError::ArchiveInvalid(
                "archive entry length differs from its central directory".to_owned(),
            ));
        }
    }
    Ok(())
}

fn quarantine_prefix(version: &str) -> String {
    format!(".remove-{PACK_ID}-v{}-{version}-", version.len())
}

fn is_exact_quarantine_name(name: &str, version: &str) -> bool {
    name.strip_prefix(&quarantine_prefix(version))
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn cleanup_matching_quarantines(
    roots: &LifecycleRoots,
    version: &str,
) -> Result<(), LifecycleError> {
    let quarantines = matching_quarantines(roots, version)?;
    if !quarantines.is_empty() {
        cleanup_quarantines(roots, version, quarantines)?;
    }
    Ok(())
}

fn matching_quarantines(
    roots: &LifecycleRoots,
    version: &str,
) -> Result<Vec<PathBuf>, LifecycleError> {
    if !exact_path_exists_fail_closed(&roots.trash_root, "Codec Pack quarantine root")? {
        return Ok(Vec::new());
    }
    ensure_safe_directory(&roots.trash_root, false)?;
    let mut matches = Vec::new();
    for entry in fs::read_dir(&roots.trash_root).map_err(|error| {
        LifecycleError::Conflict(format!("could not inspect quarantine: {error}"))
    })? {
        let entry = entry.map_err(|error| {
            LifecycleError::Conflict(format!("could not inspect quarantine: {error}"))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_exact_quarantine_name(&name, version) {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                LifecycleError::Conflict(format!("could not inspect quarantine: {error}"))
            })?;
            if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
                return Err(LifecycleError::Conflict(
                    "matching quarantine path is unsafe".to_owned(),
                ));
            }
            matches.push(entry.path());
        }
    }
    matches.sort();
    Ok(matches)
}

fn cleanup_quarantines(
    roots: &LifecycleRoots,
    version: &str,
    quarantines: Vec<PathBuf>,
) -> Result<(), LifecycleError> {
    let prefix = quarantine_prefix(version);
    for quarantine in quarantines {
        let name = quarantine.file_name().and_then(|value| value.to_str());
        if !name.is_some_and(|name| is_exact_quarantine_name(name, version)) {
            return Err(LifecycleError::Conflict(
                "quarantine name is invalid".to_owned(),
            ));
        }
        if let Err(error) = remove_temporary_tree(&roots.trash_root, &quarantine, &prefix) {
            return Err(LifecycleError::Quarantined {
                path: quarantine,
                detail: error.to_string(),
            });
        }
    }
    remove_directory_if_empty(&roots.trash_root);
    Ok(())
}

fn remove_temporary_tree(parent: &Path, candidate: &Path, prefix: &str) -> io::Result<()> {
    if candidate.parent() != Some(parent)
        || !candidate
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with(prefix))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "temporary path failed containment or prefix validation",
        ));
    }
    let metadata = fs::symlink_metadata(candidate)?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "temporary root is not a regular directory",
        ));
    }
    remove_tree_no_follow(candidate)
}

fn remove_tree_no_follow(path: &Path) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child)?;
        if is_reparse_or_symlink(&metadata) {
            if metadata.is_dir() {
                fs::remove_dir(&child)?;
            } else {
                fs::remove_file(&child)?;
            }
        } else if metadata.is_dir() {
            remove_tree_no_follow(&child)?;
        } else {
            fs::remove_file(&child)?;
        }
    }
    fs::remove_dir(path)
}

fn tree_contains_in_use_file(path: &Path) -> bool {
    let mut seen = 0usize;
    tree_contains_in_use_file_bounded(path, 0, &mut seen)
}

fn tree_contains_in_use_file_bounded(path: &Path, depth: usize, seen: &mut usize) -> bool {
    if depth > MAX_PROBED_TREE_DEPTH {
        return false;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    for entry in entries {
        *seen = seen.saturating_add(1);
        if *seen > MAX_PROBED_TREE_ENTRIES {
            return false;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let child = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&child) else {
            continue;
        };
        if is_reparse_or_symlink(&metadata) {
            continue;
        }
        if metadata.is_dir() {
            if tree_contains_in_use_file_bounded(&child, depth + 1, seen) {
                return true;
            }
        } else if metadata.is_file() {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options.share_mode(7);
            }
            if options.open(&child).is_err() {
                return true;
            }
        }
    }
    false
}

fn remove_directory_if_empty(path: &Path) {
    let Ok(mut entries) = fs::read_dir(path) else {
        return;
    };
    if entries.next().is_none() {
        let _ = fs::remove_dir(path);
    }
}

fn is_in_use_error(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::PermissionDenied
        || matches!(error.raw_os_error(), Some(5 | 32 | 33))
}

#[cfg(windows)]
fn is_reparse_or_symlink(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_or_symlink(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        EXIT_ALREADY_INSTALLED, EXIT_CONFLICT, EXIT_IN_USE, EXIT_INVALID_ARGUMENTS,
        EXIT_INVALID_PACK, EXIT_NOT_INSTALLED, LifecycleError, LifecycleRoots,
        classify_path_probe_error, is_exact_quarantine_name, is_exact_staging_name,
        is_reserved_windows_name, quarantine_prefix, validate_archive_path,
    };
    use std::io;

    #[test]
    fn stable_exit_codes_cover_every_failure_class() {
        assert_eq!(
            LifecycleError::InvalidArguments("x".to_owned()).exit_code(),
            EXIT_INVALID_ARGUMENTS
        );
        assert_eq!(
            LifecycleError::ArchiveInvalid("x".to_owned()).exit_code(),
            EXIT_INVALID_PACK
        );
        assert_eq!(
            LifecycleError::PackInvalid("x".to_owned()).exit_code(),
            EXIT_INVALID_PACK
        );
        assert_eq!(
            LifecycleError::AlreadyInstalled("0.1.1".to_owned()).exit_code(),
            EXIT_ALREADY_INSTALLED
        );
        assert_eq!(
            LifecycleError::NotInstalled("0.1.1".to_owned()).exit_code(),
            EXIT_NOT_INSTALLED
        );
        assert_eq!(
            LifecycleError::Conflict("x".to_owned()).exit_code(),
            EXIT_CONFLICT
        );
        assert_eq!(
            LifecycleError::Busy("x".to_owned()).exit_code(),
            EXIT_CONFLICT
        );
        assert_eq!(
            LifecycleError::InUse("x".to_owned()).exit_code(),
            EXIT_IN_USE
        );
        assert_eq!(
            LifecycleError::Quarantined {
                path: "x".into(),
                detail: "x".to_owned(),
            }
            .exit_code(),
            EXIT_IN_USE
        );
    }

    #[test]
    fn windows_path_aliases_and_traversal_are_rejected() {
        assert!(validate_archive_path("../escape", false).is_err());
        assert!(validate_archive_path("safe\\escape", false).is_err());
        assert!(validate_archive_path("runtime/CON.txt", false).is_err());
        assert!(validate_archive_path("runtime/file. ", false).is_err());
        for forbidden in ['<', '>', '"', '|', '?', '*'] {
            assert!(
                validate_archive_path(&format!("runtime/bad{forbidden}name.bin"), false).is_err()
            );
        }
        for control in '\u{1}'..='\u{1f}' {
            assert!(
                validate_archive_path(&format!("runtime/bad{control}name.bin"), false).is_err()
            );
        }
        let oversized_component = "x".repeat(256);
        assert!(validate_archive_path(&format!("runtime/{oversized_component}"), false).is_err());
        for alias in [
            "COM¹",
            "com².log",
            "CoM³.txt",
            "LPT¹",
            "lpt².log",
            "LpT³.txt",
        ] {
            assert!(validate_archive_path(&format!("runtime/{alias}"), false).is_err());
        }
        assert!(validate_archive_path("runtime/python.exe", false).is_ok());
        assert!(validate_archive_path("runtime/COM⁴.txt", false).is_ok());
        assert!(is_reserved_windows_name("LPT9.log"));
        assert!(!is_reserved_windows_name("LPT10.log"));
    }

    #[test]
    fn lifecycle_temporary_names_are_exact_and_version_unambiguous() {
        assert!(is_exact_staging_name(
            ".install-0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_exact_staging_name(".install-owner-notes"));
        assert!(!is_exact_staging_name(
            ".install-0123456789ABCDEF0123456789ABCDEF"
        ));

        let stable = quarantine_prefix("0.1.1");
        let prerelease = quarantine_prefix("0.1.1-alpha");
        assert!(!prerelease.starts_with(&stable));
        assert!(is_exact_quarantine_name(
            &(stable + "0123456789abcdef0123456789abcdef"),
            "0.1.1"
        ));
        assert!(!is_exact_quarantine_name(
            ".remove-org.latentdeck.h3-0.1.1-alpha-deadbeef",
            "0.1.1"
        ));
    }

    #[test]
    fn cross_scope_probe_is_absent_only_for_not_found() {
        assert!(
            !classify_path_probe_error(&io::Error::from(io::ErrorKind::NotFound), "test candidate")
                .expect("NotFound is absent")
        );
        let error = classify_path_probe_error(
            &io::Error::from(io::ErrorKind::PermissionDenied),
            "test candidate",
        )
        .expect_err("AccessDenied fails closed");
        assert_eq!(error.exit_code(), EXIT_CONFLICT);
    }

    #[test]
    fn known_folder_constructor_uses_only_the_explicit_roots() {
        let roots = LifecycleRoots::from_known_folders(
            PathBuf::from(r"C:\ExplicitLocal"),
            PathBuf::from(r"C:\ExplicitProgramData"),
        );
        assert_eq!(
            roots.install_root,
            PathBuf::from(r"C:\ExplicitLocal\LatentDeck\CodecPacks")
        );
        assert_eq!(
            roots.other_scope_root,
            Some(PathBuf::from(
                r"C:\ExplicitProgramData\LatentDeck\CodecPacks"
            ))
        );
        assert_eq!(
            roots.staging_root,
            PathBuf::from(r"C:\ExplicitLocal\LatentDeck\CodecPackStaging")
        );
    }
}
