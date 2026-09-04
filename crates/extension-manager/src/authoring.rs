use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use semver::Version;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::Builder;

use crate::archive::{PackRequest, open_directory_pin, open_regular_no_follow, pack};
use crate::deck_ui_contract::validate_deck_ui_contract;
use crate::error::{ErrorCode, ExtensionError, Result};
use crate::json_schema_contract::{PublicSchema, validate_public_schema};
use crate::model::{
    Architecture, CodecAdapterDescriptor, CodecCapability, CodecCompatibility, CodecPackManifest,
    CodecWorkerDescriptor, DeckCompatibility, DeckPackManifest, DeckRoleDescriptor,
    DeckRuntimeDescriptor, DeckRuntimeKind, DeckSignalDescriptor, IntegrityCatalog,
    IntegrityDescriptor, IntegrityFile, LicenseDescriptor, OperatingSystem, PackReceipt,
    PackageKind, PackageManifest, PackageReference, PlatformDescriptor, ProfileKey,
    PublisherDescriptor, PublisherIdentityClaim, PythonConstraint, PythonImplementation,
    RuntimeLockDescriptor, SignalGeometry, TensorDevice, TensorDtype, TimingDescriptor,
};
use crate::schema::{
    MAX_DECK_FILE_BYTES, canonical_json, is_reserved_package_id, max_extracted_bytes, max_files,
    parse_manifest, validate_deck_file_extension, validate_package_reference,
    validate_portable_relative_path,
};

const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const MAX_AUTHORING_DEPTH: usize = 32;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
enum PlatformIdentity {
    #[cfg(windows)]
    Windows {
        volume_serial_number: u64,
        file_id: [u8; 16],
    },
    #[cfg(unix)]
    Unix { device: u64, inode: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectStamp {
    byte_length: u64,
    created: Option<SystemTime>,
    modified: Option<SystemTime>,
    identity: Option<PlatformIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PlannedSourceFile {
    path: String,
    filesystem_path: PathBuf,
    stamp: ObjectStamp,
}

#[derive(Debug, PartialEq, Eq)]
struct SourcePlan {
    files: BTreeMap<String, PlannedSourceFile>,
    directories: BTreeMap<String, ObjectStamp>,
    extracted_byte_length: u64,
}

/// Request for a safe, no-clobber package source scaffold.
#[derive(Debug, Clone)]
pub struct ScaffoldRequest {
    pub kind: PackageKind,
    pub package_id: String,
    pub package_version: String,
    pub output_directory: PathBuf,
}

/// Stable description of a newly-created source scaffold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScaffoldReceipt {
    pub output_directory: PathBuf,
    pub package: PackageReference,
    /// Deck scaffolds are immediately buildable. Codec authors must supply the
    /// declared isolated runtime executable before `build` can succeed.
    pub ready_to_build: bool,
    pub required_author_action: Option<String>,
}

/// Request for cataloguing and packaging an author-owned source tree.
#[derive(Debug, Clone)]
pub struct BuildRequest {
    pub source_directory: PathBuf,
    pub output_path: PathBuf,
}

/// Create a source tree without choosing a community author's license or
/// replacing an existing path.
///
/// # Errors
///
/// Returns a stable error for invalid or reserved identities, unsafe paths,
/// existing output, or an atomic publication failure.
pub fn scaffold(request: &ScaffoldRequest) -> Result<ScaffoldReceipt> {
    let package = PackageReference {
        kind: request.kind,
        package_id: request.package_id.clone(),
        package_version: request.package_version.clone(),
    };
    validate_package_reference(&package)?;
    if is_reserved_package_id(&request.package_id) {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "scaffolds must use a publisher-owned ID outside org.latentdeck.*",
        ));
    }
    if Version::parse(&request.package_version)
        .map(|version| version.to_string() != request.package_version)
        .unwrap_or(true)
    {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "scaffold version must be canonical SemVer",
        ));
    }
    refuse_existing(&request.output_directory, "scaffold output")?;
    let parent = request
        .output_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::archive::ensure_existing_tree_safe(parent)?;
    if !parent.is_dir() {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "scaffold output parent does not exist",
        ));
    }
    let staging = Builder::new()
        .prefix(".latentdeck-scaffold-")
        .tempdir_in(parent)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "create scaffold staging", &error))?;
    let (ready_to_build, required_author_action) = match request.kind {
        PackageKind::DeckPack => {
            write_deck_scaffold(staging.path(), &package)?;
            (true, None)
        }
        PackageKind::CodecPack => {
            write_codec_scaffold(staging.path(), &package)?;
            (
                false,
                Some(
                    "Supply runtime/python.exe and runtime/python313.dll matching codec-pack.json and runtime/runtime.lock."
                        .to_owned(),
                ),
            )
        }
    };
    fs::rename(staging.path(), &request.output_directory).map_err(|error| {
        let code = if error.kind() == std::io::ErrorKind::AlreadyExists {
            ErrorCode::PackageExists
        } else {
            ErrorCode::Io
        };
        ExtensionError::io(code, "atomically publish scaffold", &error)
    })?;
    let _ = staging.keep();
    Ok(ScaffoldReceipt {
        output_directory: request.output_directory.clone(),
        package,
        ready_to_build,
        required_author_action,
    })
}

/// Generate a sorted integrity catalog, bind it into the manifest, and use the
/// existing deterministic packer and post-pack inspection path.
///
/// The input tree is never modified. An existing `integrity.json` is ignored
/// and regenerated in an isolated staging directory. Existing outputs are
/// never replaced.
///
/// # Errors
///
/// Returns a stable error when the source or output is unsafe, the source does
/// not match the closed package contract, changes while copied, or packaging
/// and post-pack inspection fail.
#[allow(clippy::too_many_lines)] // One closed staging transaction is easier to audit in order.
pub fn build(request: &BuildRequest) -> Result<PackReceipt> {
    refuse_existing(&request.output_path, "package output")?;
    crate::archive::ensure_existing_tree_safe(&request.source_directory)?;
    let source_metadata = fs::symlink_metadata(&request.source_directory).map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "inspect package source directory", &error)
    })?;
    if !source_metadata.is_dir() || crate::archive::is_reparse_or_symlink(&source_metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package source must be a regular non-reparse directory",
        ));
    }
    let kind = discover_source_kind(&request.source_directory)?;
    if request
        .output_path
        .extension()
        .and_then(|value| value.to_str())
        != Some(kind.archive_extension())
    {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            format!(
                "output must use the .{} extension",
                kind.archive_extension()
            ),
        ));
    }
    let output_parent = request
        .output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::archive::ensure_existing_tree_safe(output_parent)?;
    if !output_parent.is_dir() {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "package output parent does not exist",
        ));
    }
    let canonical_source = fs::canonicalize(&request.source_directory).map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "resolve package source directory", &error)
    })?;
    let canonical_output_parent = fs::canonicalize(output_parent).map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "resolve package output parent", &error)
    })?;
    if canonical_output_parent.starts_with(&canonical_source) {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "package output must not be written inside its source tree",
        ));
    }

    // Inventory and bound the complete source before staging or copying any
    // byte. Directory pins and per-file identities are retained/compared so a
    // path replacement cannot silently become a different build input.
    let (source_plan, _source_directory_pins) =
        preflight_source_tree(&request.source_directory, kind)?;
    let manifest_file = source_plan.files.get(kind.manifest_name()).ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "source root must contain exactly one package manifest",
        )
    })?;
    let (manifest_bytes, manifest_hash) = read_planned_file(manifest_file, 1024 * 1024)?;
    let mut source_hashes = BTreeMap::from([(kind.manifest_name().to_owned(), manifest_hash)]);
    validate_public_schema(
        &manifest_bytes,
        PublicSchema::for_manifest(kind),
        kind.manifest_name(),
    )?;
    let mut manifest = parse_manifest(kind, &manifest_bytes)?;

    let staging = Builder::new()
        .prefix(".latentdeck-build-")
        .tempdir_in(output_parent)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "create build staging", &error))?;
    let mut entries = Vec::new();
    for planned in source_plan.files.values() {
        if planned.path == kind.manifest_name() {
            continue;
        }
        if planned.path == "integrity.json" {
            source_hashes.insert(planned.path.clone(), hash_planned_file(planned)?);
            continue;
        }
        let target = planned
            .path
            .split('/')
            .fold(staging.path().to_path_buf(), |base, part| base.join(part));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                ExtensionError::io(ErrorCode::Io, "create build staging directory", &error)
            })?;
        }
        let measurement = copy_stable_file(planned, &target)?;
        source_hashes.insert(planned.path.clone(), measurement.1.clone());
        entries.push(IntegrityFile {
            path: planned.path.clone(),
            byte_length: measurement.0,
            sha256: measurement.1,
        });
    }
    if entries.is_empty() || entries.len().saturating_add(2) > max_files(kind) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!(
                "package payload must leave room for two control files within {} files",
                max_files(kind)
            ),
        ));
    }
    let payload_bytes = entries.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.byte_length).ok_or_else(|| {
            ExtensionError::new(ErrorCode::IntegrityFailed, "package size total overflowed")
        })
    })?;
    if payload_bytes > max_extracted_bytes(kind).saturating_sub(2 * 1024 * 1024) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package payload exceeds its extracted-size bound",
        ));
    }
    if let PackageManifest::Deck(deck) = &manifest {
        let operator = fs::read(staging.path().join("operator.json")).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "read staged operator.json", &error)
        })?;
        let faceplate = fs::read(staging.path().join("faceplate.json")).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "read staged faceplate.json", &error)
        })?;
        validate_public_schema(&operator, PublicSchema::Operator, "operator.json")?;
        validate_public_schema(&faceplate, PublicSchema::Faceplate, "faceplate.json")?;
        validate_deck_ui_contract(deck, &operator, &faceplate)?;
    }

    // A successful build must describe one stable source snapshot. This exact
    // rescan catches additions, removals, path replacements, metadata changes,
    // and same-length content changes made after the preflight or copy.
    validate_final_source_snapshot(
        &request.source_directory,
        kind,
        &source_plan,
        &source_hashes,
    )?;

    let catalog = IntegrityCatalog {
        manifest_version: "1.0.0".to_owned(),
        files: entries,
    };
    let catalog_bytes = canonical_json(&catalog, "integrity.json")?;
    validate_public_schema(&catalog_bytes, PublicSchema::Integrity, "integrity.json")?;
    let catalog_sha256 = hash_bytes(&catalog_bytes);
    match &mut manifest {
        PackageManifest::Deck(deck) => deck.integrity.catalog_sha256 = catalog_sha256,
        PackageManifest::Codec(codec) => codec.integrity.catalog_sha256 = catalog_sha256,
    }
    let final_manifest = canonical_json(&manifest, kind.manifest_name())?;
    validate_public_schema(
        &final_manifest,
        PublicSchema::for_manifest(kind),
        kind.manifest_name(),
    )?;
    write_new_file(&staging.path().join("integrity.json"), &catalog_bytes)?;
    write_new_file(&staging.path().join(kind.manifest_name()), &final_manifest)?;
    pack(&PackRequest {
        source_directory: staging.path().to_path_buf(),
        output_path: request.output_path.clone(),
    })
}

fn discover_source_kind(root: &Path) -> Result<PackageKind> {
    let deck = root.join("deck-pack.json");
    let codec = root.join("codec-pack.json");
    let deck_exists = regular_source_control_exists(&deck)?;
    let codec_exists = regular_source_control_exists(&codec)?;
    match (deck_exists, codec_exists) {
        (true, false) => Ok(PackageKind::DeckPack),
        (false, true) => Ok(PackageKind::CodecPack),
        _ => Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "source root must contain exactly one deck-pack.json or codec-pack.json",
        )),
    }
}

fn regular_source_control_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !crate::archive::is_reparse_or_symlink(&metadata) => {
            Ok(true)
        }
        Ok(_) => Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package manifest path is not a regular non-reparse file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ExtensionError::io(
            ErrorCode::Io,
            "inspect source package manifest",
            &error,
        )),
    }
}

fn preflight_source_tree(root: &Path, kind: PackageKind) -> Result<(SourcePlan, Vec<File>)> {
    crate::archive::ensure_existing_tree_safe(root)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package source", &error))?;
    if !metadata.is_dir() || crate::archive::is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package source must be a regular non-reparse directory",
        ));
    }
    let root_pin = open_directory_pin(root, true)?;
    let root_opened = root_pin
        .metadata()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect source root pin", &error))?;
    ensure_path_metadata_matches_opened(&metadata, &root_opened)?;
    let mut pins = vec![root_pin];
    let mut plan = SourcePlan {
        files: BTreeMap::new(),
        directories: BTreeMap::from([(String::new(), object_stamp(&pins[0], &root_opened))]),
        extracted_byte_length: 0,
    };
    scan_source_tree(root, root, kind, 0, &mut plan, &mut pins)?;
    let has_deck = plan.files.contains_key("deck-pack.json");
    let has_codec = plan.files.contains_key("codec-pack.json");
    if (kind == PackageKind::DeckPack && (!has_deck || has_codec))
        || (kind == PackageKind::CodecPack && (!has_codec || has_deck))
    {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "source root changed package manifest identity during preflight",
        ));
    }
    Ok((plan, pins))
}

fn scan_source_tree(
    root: &Path,
    current: &Path,
    kind: PackageKind,
    depth: usize,
    plan: &mut SourcePlan,
    directory_pins: &mut Vec<File>,
) -> Result<()> {
    if depth > MAX_AUTHORING_DEPTH {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package source depth exceeds its bound",
        ));
    }
    let children = fs::read_dir(current)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read package source", &error))?;
    let mut child_count = 0_usize;
    let mut casefold = BTreeSet::new();
    for entry in children {
        let entry = entry.map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "read package source entry", &error)
        })?;
        child_count = child_count.saturating_add(1);
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "inspect package source entry", &error)
        })?;
        if crate::archive::is_reparse_or_symlink(&metadata) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package source contains a symlink or reparse point",
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package source escaped its root",
            )
        })?;
        let portable = relative.to_string_lossy().replace('\\', "/");
        validate_portable_relative_path(&portable, metadata.is_dir())?;
        reject_sensitive_source_path(&portable, metadata.is_dir())?;
        if !casefold.insert(entry.file_name().to_string_lossy().to_ascii_lowercase()) {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "package source contains case-colliding siblings",
            ));
        }
        if metadata.is_dir() {
            let directory_limit = max_files(kind);
            if plan.directories.len() >= directory_limit {
                return Err(ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    "package source exceeds its directory-count bound",
                ));
            }
            let pin = open_directory_pin(&path, false)?;
            let opened = pin.metadata().map_err(|error| {
                ExtensionError::io(ErrorCode::Io, "inspect source directory pin", &error)
            })?;
            ensure_path_metadata_matches_opened(&metadata, &opened)?;
            if plan
                .directories
                .insert(portable.clone(), object_stamp(&pin, &opened))
                .is_some()
            {
                return Err(ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    "package source contains a duplicate directory path",
                ));
            }
            directory_pins.push(pin);
            scan_source_tree(root, &path, kind, depth + 1, plan, directory_pins)?;
        } else if metadata.is_file() {
            record_source_file(path, portable, &metadata, kind, plan)?;
        } else {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package source contains a special filesystem entry",
            ));
        }
    }
    if current != root && child_count == 0 {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package source contains an empty directory",
        ));
    }
    Ok(())
}

fn record_source_file(
    path: PathBuf,
    portable: String,
    metadata: &Metadata,
    kind: PackageKind,
    plan: &mut SourcePlan,
) -> Result<()> {
    if kind == PackageKind::DeckPack {
        validate_deck_file_extension(&portable)?;
        if metadata.len() > MAX_DECK_FILE_BYTES {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("{portable} exceeds the 1 MiB .ld file bound"),
            ));
        }
    }
    let next_count = plan.files.len().saturating_add(1);
    plan.extracted_byte_length =
        checked_preflight_total(kind, next_count, plan.extracted_byte_length, metadata.len())?;
    if kind == PackageKind::CodecPack
        && matches!(portable.as_str(), "codec-pack.json" | "integrity.json")
        && metadata.len() > 1024 * 1024
    {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "Codec control JSON exceeds the 1 MiB bound",
        ));
    }
    let file = open_regular_no_follow(&path, false)?;
    let opened = file
        .metadata()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect opened source file", &error))?;
    ensure_path_metadata_matches_opened(metadata, &opened)?;
    let planned = PlannedSourceFile {
        path: portable.clone(),
        filesystem_path: path,
        stamp: object_stamp(&file, &opened),
    };
    if plan.files.insert(portable, planned).is_some() {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package source contains a duplicate file path",
        ));
    }
    Ok(())
}

fn checked_preflight_total(
    kind: PackageKind,
    file_count: usize,
    current_bytes: u64,
    next_bytes: u64,
) -> Result<u64> {
    if file_count > max_files(kind) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package source exceeds its file-count bound during preflight",
        ));
    }
    let total = current_bytes.checked_add(next_bytes).ok_or_else(|| {
        ExtensionError::new(ErrorCode::IntegrityFailed, "package source size overflowed")
    })?;
    if total > max_extracted_bytes(kind) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package source exceeds its extracted-size bound during preflight",
        ));
    }
    Ok(total)
}

fn reject_sensitive_source_path(portable: &str, is_directory: bool) -> Result<()> {
    let name = portable
        .rsplit('/')
        .next()
        .unwrap_or(portable)
        .to_ascii_lowercase();
    let forbidden_directory = is_directory
        && matches!(
            name.as_str(),
            ".git" | ".hg" | ".svn" | ".bzr" | "_darcs" | "cvs"
        );
    let credential_like = !is_directory
        && (name == ".env"
            || name.starts_with(".env.")
            || matches!(
                name.as_str(),
                ".npmrc"
                    | ".pypirc"
                    | ".netrc"
                    | "_netrc"
                    | "credentials"
                    | "credentials.json"
                    | "id_rsa"
                    | "id_dsa"
                    | "id_ecdsa"
                    | "id_ed25519"
            )
            || name.starts_with("credentials.")
            || name.starts_with("secret.")
            || name.starts_with("secrets.")
            || ["pem", "key", "pfx", "p12", "jks", "kdbx"]
                .iter()
                .any(|extension| name.ends_with(&format!(".{extension}"))));
    if forbidden_directory || credential_like {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!(
                "package source contains forbidden repository metadata or credential-like path: {portable}"
            ),
        ));
    }
    Ok(())
}

fn read_planned_file(planned: &PlannedSourceFile, maximum: usize) -> Result<(Vec<u8>, String)> {
    if planned.stamp.byte_length == 0
        || planned.stamp.byte_length > u64::try_from(maximum).unwrap_or(u64::MAX)
    {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("{} is empty or exceeds its byte bound", planned.path),
        ));
    }
    let mut file = open_planned_file(planned)?;
    let mut bytes = Vec::with_capacity(usize::try_from(planned.stamp.byte_length).unwrap_or(0));
    file.read_to_end(&mut bytes)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read source control file", &error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != planned.stamp.byte_length {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("source changed while reading {}", planned.path),
        ));
    }
    let hash = hash_bytes(&bytes);
    Ok((bytes, hash))
}

fn copy_stable_file(planned: &PlannedSourceFile, target: &Path) -> Result<(u64, String)> {
    let mut input = open_planned_file(planned)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "create staged payload", &error))?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "read source payload", &error))?;
        if read == 0 {
            break;
        }
        copied = copied.checked_add(read as u64).ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "source payload length overflowed",
            )
        })?;
        if copied > planned.stamp.byte_length {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "source payload grew while being copied",
            ));
        }
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "write staged payload", &error))?;
    }
    if copied != planned.stamp.byte_length {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "source payload length changed while being copied",
        ));
    }
    output
        .sync_all()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "synchronize staged payload", &error))?;
    Ok((copied, hex::encode(hasher.finalize())))
}

fn hash_planned_file(planned: &PlannedSourceFile) -> Result<String> {
    let mut file = open_planned_file(planned)?;
    let mut hasher = Sha256::new();
    let mut read_total = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "read source snapshot", &error))?;
        if read == 0 {
            break;
        }
        read_total = read_total.checked_add(read as u64).ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "source snapshot size overflowed",
            )
        })?;
        if read_total > planned.stamp.byte_length {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("source changed while hashing {}", planned.path),
            ));
        }
        hasher.update(&buffer[..read]);
    }
    if read_total != planned.stamp.byte_length {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("source changed while hashing {}", planned.path),
        ));
    }
    Ok(hex::encode(hasher.finalize()))
}

fn open_planned_file(planned: &PlannedSourceFile) -> Result<File> {
    let file = open_regular_no_follow(&planned.filesystem_path, false)?;
    let metadata = file.metadata().map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "inspect opened planned source file", &error)
    })?;
    if object_stamp(&file, &metadata) != planned.stamp {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            format!("source file changed identity or metadata: {}", planned.path),
        ));
    }
    Ok(file)
}

fn validate_final_source_snapshot(
    root: &Path,
    kind: PackageKind,
    expected: &SourcePlan,
    expected_hashes: &BTreeMap<String, String>,
) -> Result<()> {
    let (observed, _pins) = preflight_source_tree(root, kind)?;
    if &observed != expected || expected_hashes.len() != expected.files.len() {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package source tree changed after its bounded preflight",
        ));
    }
    for (path, planned) in &observed.files {
        let expected_hash = expected_hashes.get(path).ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package source snapshot is missing an initial file hash",
            )
        })?;
        if hash_planned_file(planned)? != *expected_hash {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                format!("package source content changed after copy: {path}"),
            ));
        }
    }
    Ok(())
}

fn ensure_path_metadata_matches_opened(path: &Metadata, opened: &Metadata) -> Result<()> {
    let matches = path.is_file() == opened.is_file()
        && path.is_dir() == opened.is_dir()
        && path.len() == opened.len()
        && path.created().ok() == opened.created().ok()
        && path.modified().ok() == opened.modified().ok();
    #[cfg(windows)]
    let matches = {
        use std::os::windows::fs::MetadataExt;
        matches
            && path.file_attributes() == opened.file_attributes()
            && path.creation_time() == opened.creation_time()
            && path.last_write_time() == opened.last_write_time()
    };
    #[cfg(unix)]
    let matches = {
        use std::os::unix::fs::MetadataExt;
        matches
            && path.dev() == opened.dev()
            && path.ino() == opened.ino()
            && path.mode() == opened.mode()
            && path.mtime() == opened.mtime()
            && path.mtime_nsec() == opened.mtime_nsec()
    };
    if !matches {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package source path changed while it was being opened",
        ));
    }
    Ok(())
}

fn object_stamp(file: &File, metadata: &Metadata) -> ObjectStamp {
    ObjectStamp {
        byte_length: metadata.len(),
        created: metadata.created().ok(),
        modified: metadata.modified().ok(),
        identity: platform_identity(file, metadata),
    }
}

#[cfg(unix)]
fn platform_identity(_file: &File, metadata: &Metadata) -> Option<PlatformIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(PlatformIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn platform_identity(file: &File, _metadata: &Metadata) -> Option<PlatformIdentity> {
    windows_identity::file_identity(file)
}

#[cfg(not(any(unix, windows)))]
fn platform_identity(_file: &File, _metadata: &Metadata) -> Option<PlatformIdentity> {
    None
}

#[cfg(windows)]
mod windows_identity {
    #![allow(unsafe_code)]

    use std::ffi::c_void;
    use std::fs::File;
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };

    use super::PlatformIdentity;

    pub(super) fn file_identity(file: &File) -> Option<PlatformIdentity> {
        let mut information: FILE_ID_INFO = unsafe { zeroed() };
        let succeeded = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle().cast::<c_void>(),
                FileIdInfo,
                (&raw mut information).cast::<c_void>(),
                u32::try_from(size_of::<FILE_ID_INFO>()).ok()?,
            )
        };
        if succeeded == 0 {
            return None;
        }
        Some(PlatformIdentity::Windows {
            volume_serial_number: information.VolumeSerialNumber,
            file_id: information.FileId.Identifier,
        })
    }
}

#[allow(clippy::too_many_lines)] // Keep the mutually-bound starter files visible together.
fn write_deck_scaffold(root: &Path, package: &PackageReference) -> Result<()> {
    let module = python_module(&package.package_id, "deck");
    let entrypoint = format!("{module}.operator:process_sources_host");
    let manifest = DeckPackManifest {
        manifest_version: "1.0.0".to_owned(),
        kind: PackageKind::DeckPack,
        deck_id: package.package_id.clone(),
        deck_version: package.package_version.clone(),
        display_name: "Starter Deck".to_owned(),
        summary: "One-source identity Deck starter; replace its signal contract and operator."
            .to_owned(),
        publisher: publisher_placeholder(),
        license: license_placeholder(),
        compatibility: DeckCompatibility {
            app_min_inclusive: "0.1.0".to_owned(),
            app_max_exclusive: "1.0.0".to_owned(),
            deck_host_api: 1,
            worker_protocol: 2,
            deck_operator_api: 1,
            tensor_abi: "latentdeck.tensor.v1".to_owned(),
            python: python_constraint(),
            torch_exact_build: "2.13.0+cu130".to_owned(),
        },
        runtime: DeckRuntimeDescriptor {
            kind: DeckRuntimeKind::PythonOperatorStreamV1,
            operator_descriptor_path: "operator.json".to_owned(),
            python_root: "python".to_owned(),
            entrypoint: entrypoint.clone(),
        },
        signal: DeckSignalDescriptor {
            slots: 1,
            roles: vec![DeckRoleDescriptor {
                role_id: "source".to_owned(),
                display_name: "Source".to_owned(),
            }],
            default_permutation: vec!["source".to_owned()],
            structural_carrier_role: "source".to_owned(),
            geometry_allowlist: vec![SignalGeometry {
                dtype: TensorDtype::Fp32,
                device: TensorDevice::Cpu,
                batch: 1,
                channels: 4,
                temporal: 1,
                height: 2,
                width: 3,
            }],
            timing: TimingDescriptor {
                frames_per_second_numerator: 24,
                frames_per_second_denominator: 1,
                samples_per_slot: 24,
            },
            required_capabilities: mandatory_capabilities(),
            profile_allowlist: Some(vec![ProfileKey {
                codec_family: "synthetic".to_owned(),
                profile: "example_latent".to_owned(),
                profile_version: "0.1.0".to_owned(),
            }]),
        },
        faceplate_path: "faceplate.json".to_owned(),
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: ZERO_SHA256.to_owned(),
        },
    };
    let operator = json!({
        "schema_version": "0.2.0",
        "deck_operator_api": "0.2.0",
        "deck_id": package.package_id,
        "deck_version": package.package_version,
        "operator_id": package.package_id,
        "operator_version": package.package_version,
        "entrypoint": entrypoint,
        "source_count": 1,
        "role_ids": ["source"],
        "controls": [{
            "control_id": "mode",
            "value_type": "enum",
            "default": "identity",
            "options": ["identity"]
        }]
    });
    let faceplate = json!({
        "schema_version": 2,
        "title": "Starter Deck",
        "sections": [
            {"section_id":"source","title":"Source","region":"controls","columns":1,
             "widgets":[{"id":"source","kind":"source_picker","label":"Source","slot_index":0}]},
            {"section_id":"transport","title":"Transport","region":"controls","columns":1,
             "widgets":[{"id":"transport","kind":"transport","label":"Transport","slot_indices":[0]},
                        {"id":"seed","kind":"seed","label":"Seed"}]},
            {"section_id":"roles","title":"Roles","region":"controls","columns":1,
             "widgets":[{"id":"roles","kind":"role_editor","label":"Source role","role_ids":["source"]}]},
            {"section_id":"operator","title":"Operator","region":"controls","columns":1,
             "widgets":[{"id":"mode","kind":"select","label":"Mode","control_id":"mode",
                         "options":[{"value":"identity","label":"Identity"}]}]},
            {"section_id":"capture","title":"Capture","region":"actions","columns":1,
             "widgets":[{"id":"capture","kind":"capture","label":"Latent capture",
                         "modes":["snapshot","live_capture"]}]},
            {"section_id":"output","title":"Output","region":"output","columns":1,
             "widgets":[{"id":"monitor","kind":"monitor","label":"Monitor"}]}
        ]
    });
    let operator_py = format!(
        "\"\"\"Identity operator generated for {id}.\"\"\"\n\nfrom latentdeck_deck_sdk import DeckContractError, DeckOperatorResult\n\ndef process_sources_host(sources, controls, context):\n    del context\n    if controls != {{\"mode\": \"identity\"}}:\n        raise DeckContractError(\"control.mode\", \"mode must be identity\")\n    return DeckOperatorResult(\n        output=sources[0].clone().contiguous(),\n        provenance={{\"operator_id\": \"{id}\", \"operator_version\": \"{version}\"}},\n    )\n",
        id = package.package_id,
        version = package.package_version,
    );
    write_scaffold_file(root, "deck-pack.json", &pretty_json(&manifest)?)?;
    write_scaffold_file(root, "operator.json", &pretty_json(&operator)?)?;
    write_scaffold_file(root, "faceplate.json", &pretty_json(&faceplate)?)?;
    write_scaffold_file(
        root,
        &format!("python/{module}/__init__.py"),
        b"\"\"\"Starter Deck package.\"\"\"\n",
    )?;
    write_scaffold_file(
        root,
        &format!("python/{module}/operator.py"),
        operator_py.as_bytes(),
    )?;
    write_scaffold_file(root, "NOTICE.txt", notice_placeholder().as_bytes())
}

fn write_codec_scaffold(root: &Path, package: &PackageReference) -> Result<()> {
    let lock_bytes =
        b"# Replace with the exact isolated runtime lock.\npython==3.13\ntorch==2.13.0+cu130\n";
    let manifest = CodecPackManifest {
        manifest_version: "2.0.0".to_owned(),
        kind: PackageKind::CodecPack,
        pack_id: package.package_id.clone(),
        pack_version: package.package_version.clone(),
        display_name: "Synthetic Codec Starter".to_owned(),
        summary: "CPU-oriented adapter starter; supply a matching isolated Windows runtime."
            .to_owned(),
        publisher: publisher_placeholder(),
        license: license_placeholder(),
        platform: PlatformDescriptor {
            os: OperatingSystem::Windows,
            arch: Architecture::X86_64,
        },
        compatibility: CodecCompatibility {
            app_min_inclusive: "0.1.0".to_owned(),
            app_max_exclusive: "1.0.0".to_owned(),
            worker_protocol: 2,
            codec_adapter_api: 1,
            tensor_abi: "latentdeck.tensor.v1".to_owned(),
            python: python_constraint(),
            torch_exact_build: "2.13.0+cu130".to_owned(),
            lc_spec_versions: vec!["0.1.0".to_owned()],
            profiles: vec![ProfileKey {
                codec_family: "synthetic".to_owned(),
                profile: "example_latent".to_owned(),
                profile_version: "0.1.0".to_owned(),
            }],
        },
        adapter: CodecAdapterDescriptor {
            adapter_id: package.package_id.clone(),
            adapter_version: package.package_version.clone(),
            entrypoint: "adapter:make_adapter".to_owned(),
        },
        worker: CodecWorkerDescriptor {
            executable: "runtime/python.exe".to_owned(),
            arguments: vec!["-m".to_owned(), "latentdeck_codec_host".to_owned()],
            working_directory: "runtime".to_owned(),
            start_timeout_ms: 30_000,
            heartbeat_timeout_ms: 5_000,
        },
        capabilities: mandatory_capabilities(),
        external_assets: Vec::new(),
        runtime_lock: RuntimeLockDescriptor {
            path: "runtime/runtime.lock".to_owned(),
            sha256: hash_bytes(lock_bytes),
        },
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: ZERO_SHA256.to_owned(),
        },
    };
    let adapter_py = b"\"\"\"Synthetic Codec adapter authoring placeholder.\"\"\"\n\ndef make_adapter():\n    raise NotImplementedError(\"implement the Codec SDK adapter contract\")\n";
    write_scaffold_file(root, "codec-pack.json", &pretty_json(&manifest)?)?;
    write_scaffold_file(root, "runtime/adapter.py", adapter_py)?;
    write_scaffold_file(root, "runtime/runtime.lock", lock_bytes)?;
    write_scaffold_file(
        root,
        "runtime/README.md",
        b"# Isolated runtime\n\nSupply `python.exe`, its matching versioned runtime DLL (`python313.dll` for the declared CPython 3.13 runtime), the Codec Host, Codec SDK, adapter dependencies, and licenses that exactly match `runtime.lock`. The source repository must not vendor an unrelated local environment.\n",
    )?;
    write_scaffold_file(root, "NOTICE.txt", notice_placeholder().as_bytes())
}

fn python_module(package_id: &str, suffix: &str) -> String {
    let normalized: String = package_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    format!("{normalized}_{suffix}")
}

fn publisher_placeholder() -> PublisherDescriptor {
    PublisherDescriptor {
        name: "Replace with publisher name".to_owned(),
        url: None,
        identity_claim: PublisherIdentityClaim::SelfDeclared,
    }
}

fn license_placeholder() -> LicenseDescriptor {
    LicenseDescriptor {
        spdx_or_label: "UNLICENSED".to_owned(),
        notice_path: "NOTICE.txt".to_owned(),
    }
}

fn notice_placeholder() -> String {
    "Choose a license and replace the license metadata before distribution.\n".to_owned()
}

fn python_constraint() -> PythonConstraint {
    PythonConstraint {
        implementation: PythonImplementation::Cpython,
        version: "3.13".to_owned(),
        platform_tag: "win_amd64".to_owned(),
    }
}

fn mandatory_capabilities() -> Vec<CodecCapability> {
    vec![
        CodecCapability::Player,
        CodecCapability::Realtime,
        CodecCapability::Resample,
        CodecCapability::SnapshotCapture,
        CodecCapability::LiveCapture,
    ]
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            format!("serialize scaffold JSON: {error}"),
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_scaffold_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    validate_portable_relative_path(relative, false)?;
    let target = relative
        .split('/')
        .fold(root.to_path_buf(), |base, component| base.join(component));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "create scaffold directory", &error)
        })?;
    }
    write_new_file(&target, bytes)
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "create authoring file", &error))?;
    file.write_all(bytes)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "write authoring file", &error))?;
    file.sync_all()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "synchronize authoring file", &error))
}

fn refuse_existing(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(ExtensionError::new(
            ErrorCode::PackageExists,
            format!("{label} already exists"),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ExtensionError::io(
            ErrorCode::Io,
            &format!("inspect {label}"),
            &error,
        )),
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaffolded_deck(temp: &tempfile::TempDir, name: &str) -> PathBuf {
        let source = temp.path().join(name);
        scaffold(&ScaffoldRequest {
            kind: PackageKind::DeckPack,
            package_id: format!("com.example.{name}"),
            package_version: "0.1.0".to_owned(),
            output_directory: source.clone(),
        })
        .expect("scaffold Deck fixture");
        source
    }

    fn measured_source_hashes(plan: &SourcePlan) -> BTreeMap<String, String> {
        plan.files
            .iter()
            .map(|(path, planned)| {
                (
                    path.clone(),
                    hash_planned_file(planned).expect("hash planned source file"),
                )
            })
            .collect()
    }

    #[test]
    fn deck_scaffold_builds_without_modifying_its_source() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("starter");
        let receipt = scaffold(&ScaffoldRequest {
            kind: PackageKind::DeckPack,
            package_id: "com.example.identity".to_owned(),
            package_version: "0.1.0".to_owned(),
            output_directory: source.clone(),
        })
        .expect("scaffold Deck");
        assert!(receipt.ready_to_build);
        assert!(!source.join("integrity.json").exists());
        let original_manifest = fs::read(source.join("deck-pack.json")).expect("read manifest");
        let output = temp.path().join("identity.ld");
        let built = build(&BuildRequest {
            source_directory: source.clone(),
            output_path: output,
        })
        .expect("build Deck");
        assert_eq!(built.inspection.package.package_id, "com.example.identity");
        assert_eq!(
            fs::read(source.join("deck-pack.json")).expect("re-read manifest"),
            original_manifest
        );
        assert!(!source.join("integrity.json").exists());
    }

    #[test]
    fn scaffold_and_build_refuse_existing_outputs() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("starter");
        let request = ScaffoldRequest {
            kind: PackageKind::DeckPack,
            package_id: "com.example.no-clobber".to_owned(),
            package_version: "0.1.0".to_owned(),
            output_directory: source.clone(),
        };
        scaffold(&request).expect("initial scaffold");
        let error = scaffold(&request).expect_err("second scaffold must fail");
        assert_eq!(error.code(), ErrorCode::PackageExists);

        let output = temp.path().join("no-clobber.ld");
        let first = build(&BuildRequest {
            source_directory: source.clone(),
            output_path: output.clone(),
        })
        .expect("initial build");
        let second = build(&BuildRequest {
            source_directory: source.clone(),
            output_path: temp.path().join("deterministic.ld"),
        })
        .expect("deterministic rebuild");
        assert_eq!(
            first.inspection.archive_sha256,
            second.inspection.archive_sha256
        );
        let error = build(&BuildRequest {
            source_directory: source,
            output_path: output,
        })
        .expect_err("second build must fail");
        assert_eq!(error.code(), ErrorCode::PackageExists);
    }

    #[test]
    fn codec_scaffold_names_the_missing_runtime_action() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("codec");
        let receipt = scaffold(&ScaffoldRequest {
            kind: PackageKind::CodecPack,
            package_id: "com.example.synthetic".to_owned(),
            package_version: "0.1.0".to_owned(),
            output_directory: source.clone(),
        })
        .expect("scaffold Codec");
        assert!(!receipt.ready_to_build);
        assert!(receipt.required_author_action.is_some());
        let error = build(&BuildRequest {
            source_directory: source,
            output_path: temp.path().join("synthetic.ldcodec"),
        })
        .expect_err("runtime-less Codec must not build");
        assert_eq!(error.code(), ErrorCode::IntegrityFailed);
    }

    #[test]
    fn scaffold_rejects_reserved_project_identity() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let error = scaffold(&ScaffoldRequest {
            kind: PackageKind::DeckPack,
            package_id: "org.latentdeck.unapproved".to_owned(),
            package_version: "0.1.0".to_owned(),
            output_directory: temp.path().join("reserved"),
        })
        .expect_err("reserved namespace must fail");
        assert_eq!(error.code(), ErrorCode::InvalidArguments);
    }

    #[test]
    fn build_validates_operator_faceplate_and_cross_file_bindings() {
        let temp = tempfile::tempdir().expect("temporary directory");
        for (name, relative, mutate, expected_detail) in [
            (
                "bad-operator",
                "operator.json",
                ("deck_id", serde_json::json!("com.example.another")),
                "exact Deck package contract",
            ),
            (
                "bad-faceplate",
                "faceplate.json",
                ("title", serde_json::json!("")),
                "public JSON Schema",
            ),
        ] {
            let source = scaffolded_deck(&temp, name);
            let path = source.join(relative);
            let mut document: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).expect("read descriptor"))
                    .expect("parse descriptor");
            document[mutate.0] = mutate.1;
            fs::write(&path, pretty_json(&document).expect("serialize descriptor"))
                .expect("write invalid descriptor");

            let error = build(&BuildRequest {
                source_directory: source,
                output_path: temp.path().join(format!("{name}.ld")),
            })
            .expect_err("cross-file/UI-invalid Deck must not build");
            assert_eq!(error.code(), ErrorCode::ManifestInvalid);
            assert!(error.detail().contains(expected_detail), "{error:?}");
        }
    }

    #[test]
    fn build_receipt_exposes_the_exact_sorted_included_catalog() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = scaffolded_deck(&temp, "catalog");
        let receipt = build(&BuildRequest {
            source_directory: source,
            output_path: temp.path().join("catalog.ld"),
        })
        .expect("build Deck");

        assert!(
            receipt
                .included_files
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert_eq!(receipt.included_files.len(), receipt.inspection.file_count);
        assert!(
            receipt
                .included_files
                .contains(&"deck-pack.json".to_owned())
        );
        assert!(
            receipt
                .included_files
                .contains(&"integrity.json".to_owned())
        );
        assert!(receipt.included_files.contains(&"operator.json".to_owned()));
        assert!(
            receipt
                .included_files
                .contains(&"faceplate.json".to_owned())
        );
    }

    #[test]
    fn preflight_rejects_repository_metadata_and_credential_like_paths() {
        let temp = tempfile::tempdir().expect("temporary directory");
        for (name, relative) in [
            ("environment", ".env.local"),
            ("credential", "runtime/client-secret.pem"),
            ("repository", ".git/config"),
        ] {
            let source = scaffolded_deck(&temp, name);
            let path = source.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            fs::create_dir_all(path.parent().expect("sensitive parent"))
                .expect("create sensitive parent");
            fs::write(path, b"not-a-real-secret").expect("write sensitive fixture");
            let error = build(&BuildRequest {
                source_directory: source,
                output_path: temp.path().join(format!("{name}.ld")),
            })
            .expect_err("sensitive authoring path must fail closed");
            assert_eq!(error.code(), ErrorCode::ManifestInvalid);
            assert!(error.detail().contains("credential-like path"));
        }
    }

    #[test]
    fn final_snapshot_rejects_add_remove_change_and_identity_replacement() {
        for mutation in ["add", "remove", "change", "replace"] {
            let temp = tempfile::tempdir().expect("temporary directory");
            let source = scaffolded_deck(&temp, mutation);
            let (plan, pins) =
                preflight_source_tree(&source, PackageKind::DeckPack).expect("preflight source");
            let hashes = measured_source_hashes(&plan);
            drop(pins);
            let operator = source.join("operator.json");
            match mutation {
                "add" => fs::write(source.join("added.txt"), b"added").expect("add file"),
                "remove" => fs::remove_file(&operator).expect("remove file"),
                "change" => {
                    let mut bytes = fs::read(&operator).expect("read operator");
                    bytes[0] ^= 1;
                    fs::write(&operator, bytes).expect("change file");
                }
                "replace" => {
                    let bytes = fs::read(&operator).expect("read operator");
                    fs::remove_file(&operator).expect("remove operator identity");
                    fs::write(&operator, bytes).expect("replace operator identity");
                }
                _ => unreachable!(),
            }

            let error =
                validate_final_source_snapshot(&source, PackageKind::DeckPack, &plan, &hashes)
                    .expect_err("mutated snapshot must fail closed");
            assert_eq!(error.code(), ErrorCode::LifecycleConflict, "{mutation}");
        }
    }

    #[test]
    fn codec_preflight_limits_are_enforced_before_copy() {
        assert_eq!(
            checked_preflight_total(
                PackageKind::CodecPack,
                max_files(PackageKind::CodecPack) + 1,
                0,
                1,
            )
            .expect_err("Codec file-count overflow must fail")
            .code(),
            ErrorCode::IntegrityFailed
        );
        assert_eq!(
            checked_preflight_total(
                PackageKind::CodecPack,
                1,
                max_extracted_bytes(PackageKind::CodecPack),
                1,
            )
            .expect_err("Codec aggregate overflow must fail")
            .code(),
            ErrorCode::IntegrityFailed
        );
    }

    #[test]
    fn source_symlinks_or_reparse_points_fail_preflight() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = scaffolded_deck(&temp, "linked");
        let target = source.join("NOTICE.txt");
        let linked = source.join("linked.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &linked).expect("create test symlink");
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&target, &linked).is_err() {
            return;
        }

        let error = preflight_source_tree(&source, PackageKind::DeckPack)
            .expect_err("source link must fail closed");
        assert_eq!(error.code(), ErrorCode::LifecycleConflict);
    }
}
