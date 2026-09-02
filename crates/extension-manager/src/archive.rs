use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::error::{ErrorCode, ExtensionError, Result};
use crate::model::{InspectedPackage, PackReceipt, PackageKind, PackageManifest};
use crate::schema::{
    MAX_CODEC_EXTRACTED_BYTES, MAX_DECK_FILE_BYTES, MAX_JSON_BYTES, max_archive_bytes,
    max_extracted_bytes, max_files, parse_integrity_catalog, parse_manifest,
    validate_deck_file_extension, validate_portable_relative_path, validate_sha256,
    validate_strict_json_value,
};

const MAX_ARCHIVE_ENTRIES: usize = 65_536;
const MAX_TREE_DEPTH: usize = 256;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PackRequest {
    pub source_directory: PathBuf,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileMeasurement {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Debug)]
struct ArchiveEntryPlan {
    index: usize,
    path: String,
    relative: PathBuf,
    normalized: String,
    directory: bool,
    byte_length: u64,
}

#[derive(Debug)]
pub(crate) struct PreparedPackage {
    pub archive: ZipArchive<File>,
    plan: Vec<ArchiveEntryPlan>,
    pub inspection: InspectedPackage,
    pub manifest: PackageManifest,
    pub files: BTreeMap<String, FileMeasurement>,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidatedDirectory {
    pub manifest: PackageManifest,
    pub files: BTreeMap<String, FileMeasurement>,
    pub manifest_sha256: String,
    pub integrity_catalog_sha256: String,
    pub extracted_byte_length: u64,
}

/// Fully inspect an archive without installing it.
///
/// # Errors
///
/// Returns a stable error when the path, expected hash, ZIP structure, closed
/// manifest schema, integrity catalog, or bounded file content is invalid.
pub fn inspect(path: impl AsRef<Path>, expected_sha256: Option<&str>) -> Result<InspectedPackage> {
    let kind = kind_from_archive_extension(path.as_ref())?;
    prepare_archive(path.as_ref(), expected_sha256, Some(kind)).map(|prepared| prepared.inspection)
}

/// Deterministically pack a fully catalogued source tree. Existing outputs are
/// never replaced.
///
/// # Errors
///
/// Returns a stable error when the source tree is unsafe or invalid, changes
/// while being copied, the output exists, or atomic publication fails.
pub fn pack(request: &PackRequest) -> Result<PackReceipt> {
    let source = validate_directory(&request.source_directory, None)?;
    let kind = source.manifest.kind();
    validate_output(&request.output_path, kind)?;
    let output_parent = request
        .output_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_existing_tree_safe(output_parent)?;
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

    let partial = output_parent.join(format!(
        ".latentdeck-pack-{}.partial",
        Uuid::new_v4().simple()
    ));
    let partial_guard = TemporaryFileGuard(Some(partial.clone()));
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&partial)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "create package partial", &error))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o644);
    for measurement in source.files.values() {
        let mut source_file = open_regular_no_follow(
            &request
                .source_directory
                .join(path_from_archive(&measurement.path)),
            false,
        )?;
        writer
            .start_file(
                &measurement.path,
                options.large_file(measurement.byte_length > u64::from(u32::MAX)),
            )
            .map_err(|error| {
                ExtensionError::new(ErrorCode::Io, format!("start ZIP entry: {error}"))
            })?;
        let copied = copy_and_hash(&mut source_file, &mut writer, measurement.byte_length)?;
        if copied.byte_length != measurement.byte_length || copied.sha256 != measurement.sha256 {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("source changed while packing {}", measurement.path),
            ));
        }
    }
    let completed = writer.finish().map_err(|error| {
        ExtensionError::new(ErrorCode::Io, format!("finalize package ZIP: {error}"))
    })?;
    completed
        .sync_all()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "synchronize package ZIP", &error))?;
    drop(completed);

    let inspection = prepare_archive(&partial, None, Some(kind))?.inspection;
    if inspection.file_count != source.files.len()
        || inspection.extracted_byte_length != source.extracted_byte_length
        || inspection.manifest_sha256 != source.manifest_sha256
        || inspection.integrity_catalog_sha256 != source.integrity_catalog_sha256
    {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "post-pack inspection differs from the validated source tree",
        ));
    }
    fs::hard_link(&partial, &request.output_path).map_err(|error| {
        let code = if error.kind() == io::ErrorKind::AlreadyExists {
            ErrorCode::PackageExists
        } else {
            ErrorCode::Io
        };
        ExtensionError::io(code, "atomically publish package", &error)
    })?;
    fs::remove_file(&partial).map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "remove package publication link", &error)
    })?;
    partial_guard.disarm();
    Ok(PackReceipt {
        output_path: request.output_path.clone(),
        inspection,
    })
}

pub(crate) fn prepare_archive(
    path: &Path,
    expected_sha256: Option<&str>,
    expected_kind: Option<PackageKind>,
) -> Result<PreparedPackage> {
    if let Some(expected) = expected_sha256 {
        validate_sha256(expected, "expected archive SHA-256")?;
    }
    let mut file = open_regular_no_follow(path, true)?;
    let archive_byte_length = file
        .metadata()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect archive metadata", &error))?
        .len();
    if archive_byte_length == 0 || archive_byte_length > max_archive_bytes(PackageKind::CodecPack) {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "archive is empty or exceeds the 32 GiB absolute archive bound",
        ));
    }
    let archive_sha256 = hash_open_file(&mut file)?;
    if expected_sha256.is_some_and(|expected| expected != archive_sha256) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("archive SHA-256 mismatch; measured {archive_sha256}"),
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "rewind archive", &error))?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        ExtensionError::new(ErrorCode::ArchiveInvalid, format!("open ZIP: {error}"))
    })?;
    let plan = plan_archive(&mut archive)?;
    let kind = detect_kind(&plan)?;
    if expected_kind.is_some_and(|expected| expected != kind) {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "archive package kind does not match the requested package kind",
        ));
    }
    if archive_byte_length > max_archive_bytes(kind) {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            format!(
                "{} archive exceeds its {} byte bound",
                kind.archive_extension(),
                max_archive_bytes(kind)
            ),
        ));
    }
    let files_in_plan = plan.iter().filter(|entry| !entry.directory).count();
    if files_in_plan == 0 || files_in_plan > max_files(kind) {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            format!(
                "archive file count exceeds the {} file bound",
                max_files(kind)
            ),
        ));
    }
    let extracted_byte_length = plan
        .iter()
        .filter(|entry| !entry.directory)
        .try_fold(0_u64, |total, entry| total.checked_add(entry.byte_length))
        .ok_or_else(|| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "archive size total overflowed")
        })?;
    if extracted_byte_length > max_extracted_bytes(kind) {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            format!(
                "archive expands beyond the {} byte bound",
                max_extracted_bytes(kind)
            ),
        ));
    }
    let (files, directories, contents) = measure_archive(&mut archive, &plan, kind)?;
    let validated = validate_contract(kind, &files, &directories, &contents)?;
    let inspection = InspectedPackage {
        package: validated.manifest.reference(),
        display_name: validated.manifest.display_name().to_owned(),
        publisher_name: validated.manifest.publisher().name.clone(),
        publisher_identity_claim: validated.manifest.publisher().identity_claim.clone(),
        archive_sha256,
        archive_byte_length,
        manifest_sha256: validated.manifest_sha256.clone(),
        integrity_catalog_sha256: validated.integrity_catalog_sha256.clone(),
        file_count: files.len(),
        extracted_byte_length,
        manifest: validated.manifest.clone(),
    };
    Ok(PreparedPackage {
        archive,
        plan,
        inspection,
        manifest: validated.manifest,
        files,
    })
}

pub(crate) fn validate_directory(
    root: &Path,
    expected_kind: Option<PackageKind>,
) -> Result<ValidatedDirectory> {
    ensure_existing_tree_safe(root)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package directory", &error))?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package root is not a regular non-reparse directory",
        ));
    }
    let mut file_paths = Vec::new();
    let mut directories = BTreeSet::new();
    scan_directory(root, root, 0, &mut file_paths, &mut directories)?;
    file_paths.sort_by(|left, right| left.0.cmp(&right.0));
    let kind = detect_kind_from_paths(file_paths.iter().map(|(path, _)| path.as_str()))?;
    if expected_kind.is_some_and(|expected| expected != kind) {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "installed package kind differs from its root",
        ));
    }
    if file_paths.is_empty() || file_paths.len() > max_files(kind) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("package tree exceeds the {} file bound", max_files(kind)),
        ));
    }
    let mut files = BTreeMap::new();
    let mut contents = BTreeMap::new();
    let mut extracted = 0_u64;
    for (relative, full_path) in file_paths {
        if kind == PackageKind::DeckPack {
            validate_deck_file_extension(&relative)?;
        }
        let mut file = open_regular_no_follow(&full_path, false)?;
        let byte_length = file
            .metadata()
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package file", &error))?
            .len();
        if kind == PackageKind::DeckPack && byte_length > MAX_DECK_FILE_BYTES {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("{relative} exceeds the 1 MiB .ld file bound"),
            ));
        }
        extracted = extracted.checked_add(byte_length).ok_or_else(|| {
            ExtensionError::new(ErrorCode::IntegrityFailed, "package size total overflowed")
        })?;
        if extracted > max_extracted_bytes(kind) {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "package tree exceeds its extracted-size bound",
            ));
        }
        let keep_bytes = kind == PackageKind::DeckPack
            || relative == kind.manifest_name()
            || relative == "integrity.json";
        if keep_bytes && byte_length > MAX_JSON_BYTES as u64 && kind == PackageKind::CodecPack {
            return Err(ExtensionError::new(
                ErrorCode::ManifestInvalid,
                "Codec control JSON exceeds the 1 MiB bound",
            ));
        }
        let measured = read_and_hash(&mut file, byte_length, keep_bytes)?;
        if let Some(bytes) = measured.bytes {
            contents.insert(relative.clone(), bytes);
        }
        files.insert(
            relative.clone(),
            FileMeasurement {
                path: relative,
                byte_length: measured.byte_length,
                sha256: measured.sha256,
            },
        );
    }
    let mut validated = validate_contract(kind, &files, &directories, &contents)?;
    validated.extracted_byte_length = extracted;
    Ok(validated)
}

pub(crate) fn extract_prepared(prepared: &mut PreparedPackage, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(destination).map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "inspect extraction destination", &error)
    })?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "extraction destination is not a regular directory",
        ));
    }
    for planned in &prepared.plan {
        let output = destination.join(&planned.relative);
        if !output.starts_with(destination) {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "archive entry escaped extraction root",
            ));
        }
        if planned.directory {
            fs::create_dir_all(&output).map_err(|error| {
                ExtensionError::io(ErrorCode::Io, "create extraction directory", &error)
            })?;
            ensure_descendant_safe(destination, &output)?;
            continue;
        }
        let parent = output.parent().ok_or_else(|| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "archive file has no safe parent")
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "create extraction parent", &error)
        })?;
        ensure_descendant_safe(destination, parent)?;
        let mut output_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output)
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "create extracted file", &error))?;
        let mut entry = prepared.archive.by_index(planned.index).map_err(|error| {
            ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                format!("open ZIP entry: {error}"),
            )
        })?;
        let copied = copy_and_hash(&mut entry, &mut output_file, planned.byte_length)?;
        let expected = prepared.files.get(&planned.path).ok_or_else(|| {
            ExtensionError::new(ErrorCode::IntegrityFailed, "planned ZIP file is unmeasured")
        })?;
        if copied.byte_length != expected.byte_length || copied.sha256 != expected.sha256 {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("ZIP entry changed while extracting {}", planned.path),
            ));
        }
        output_file.sync_all().map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "synchronize extracted file", &error)
        })?;
    }
    let validated = validate_directory(destination, Some(prepared.manifest.kind()))?;
    if validated.files != prepared.files
        || validated.manifest != prepared.manifest
        || validated.manifest_sha256 != prepared.inspection.manifest_sha256
        || validated.integrity_catalog_sha256 != prepared.inspection.integrity_catalog_sha256
    {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "extracted tree differs from the preflighted archive",
        ));
    }
    Ok(())
}

fn plan_archive(archive: &mut ZipArchive<File>) -> Result<Vec<ArchiveEntryPlan>> {
    if archive.is_empty() || archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            format!("ZIP entry count must be between 1 and {MAX_ARCHIVE_ENTRIES}"),
        ));
    }
    let mut plan = Vec::with_capacity(archive.len());
    let mut normalized_paths = HashSet::with_capacity(archive.len());
    let mut general_extracted = 0_u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                format!("read ZIP header: {error}"),
            )
        })?;
        if entry.encrypted() {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "encrypted ZIP entries are forbidden",
            ));
        }
        if !matches!(
            entry.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP entry uses an unsupported compression method",
            ));
        }
        let raw_name = std::str::from_utf8(entry.name_raw()).map_err(|_| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "ZIP path is not strict UTF-8")
        })?;
        let directory = entry.is_dir();
        let name = if directory {
            raw_name.strip_suffix('/').ok_or_else(|| {
                ExtensionError::new(
                    ErrorCode::ArchiveInvalid,
                    "ZIP directory path lacks a trailing slash",
                )
            })?
        } else {
            raw_name
        };
        let relative = validate_portable_relative_path(name, directory).map_err(|error| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, error.detail().to_owned())
        })?;
        let normalized = name.to_ascii_lowercase();
        if !normalized_paths.insert(normalized.clone()) {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP contains a case-insensitive duplicate path",
            ));
        }
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170_000;
            let expected = if directory { 0o040_000 } else { 0o100_000 };
            if kind != 0 && kind != expected {
                return Err(ExtensionError::new(
                    ErrorCode::ArchiveInvalid,
                    "ZIP contains a symlink or special filesystem entry",
                ));
            }
        }
        if directory && entry.size() != 0 {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP directory entry has a non-zero payload",
            ));
        }
        general_extracted = general_extracted.checked_add(entry.size()).ok_or_else(|| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "ZIP size total overflowed")
        })?;
        if general_extracted > MAX_CODEC_EXTRACTED_BYTES {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP expands beyond the 64 GiB absolute bound",
            ));
        }
        plan.push(ArchiveEntryPlan {
            index,
            path: name.to_owned(),
            relative,
            normalized,
            directory,
            byte_length: entry.size(),
        });
    }
    validate_archive_hierarchy(&plan)?;
    Ok(plan)
}

fn validate_archive_hierarchy(plan: &[ArchiveEntryPlan]) -> Result<()> {
    let mut spelling = HashMap::<String, String>::new();
    for entry in plan {
        let mut canonical_prefix = String::new();
        let mut normalized_prefix = String::new();
        for component in entry.path.split('/') {
            if !canonical_prefix.is_empty() {
                canonical_prefix.push('/');
                normalized_prefix.push('/');
            }
            canonical_prefix.push_str(component);
            normalized_prefix.push_str(&component.to_ascii_lowercase());
            if let Some(existing) =
                spelling.insert(normalized_prefix.clone(), canonical_prefix.clone())
                && existing != canonical_prefix
            {
                return Err(ExtensionError::new(
                    ErrorCode::ArchiveInvalid,
                    "ZIP path hierarchy contains inconsistent case aliases",
                ));
            }
        }
    }
    let file_paths: BTreeSet<&str> = plan
        .iter()
        .filter(|entry| !entry.directory)
        .map(|entry| entry.normalized.as_str())
        .collect();
    for path in &file_paths {
        let mut parent = *path;
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if file_paths.contains(prefix) {
                return Err(ExtensionError::new(
                    ErrorCode::ArchiveInvalid,
                    "ZIP path is both a file and a directory parent",
                ));
            }
            parent = prefix;
        }
    }
    Ok(())
}

fn detect_kind(plan: &[ArchiveEntryPlan]) -> Result<PackageKind> {
    detect_kind_from_paths(
        plan.iter()
            .filter(|entry| !entry.directory)
            .map(|entry| entry.path.as_str()),
    )
}

fn detect_kind_from_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Result<PackageKind> {
    let mut deck = false;
    let mut codec = false;
    for path in paths {
        deck |= path == PackageKind::DeckPack.manifest_name();
        codec |= path == PackageKind::CodecPack.manifest_name();
    }
    match (deck, codec) {
        (true, false) => Ok(PackageKind::DeckPack),
        (false, true) => Ok(PackageKind::CodecPack),
        (false, false) => Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "package is missing its root control manifest",
        )),
        (true, true) => Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "package contains both Deck and Codec control manifests",
        )),
    }
}

type MeasuredArchive = (
    BTreeMap<String, FileMeasurement>,
    BTreeSet<String>,
    BTreeMap<String, Vec<u8>>,
);

fn measure_archive(
    archive: &mut ZipArchive<File>,
    plan: &[ArchiveEntryPlan],
    kind: PackageKind,
) -> Result<MeasuredArchive> {
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut contents = BTreeMap::new();
    for planned in plan {
        if planned.directory {
            directories.insert(planned.path.clone());
            continue;
        }
        if kind == PackageKind::DeckPack {
            validate_deck_file_extension(&planned.path)?;
            if planned.byte_length > MAX_DECK_FILE_BYTES {
                return Err(ExtensionError::new(
                    ErrorCode::ArchiveInvalid,
                    format!("{} exceeds the 1 MiB .ld file bound", planned.path),
                ));
            }
        }
        let mut entry = archive.by_index(planned.index).map_err(|error| {
            ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                format!("open ZIP entry: {error}"),
            )
        })?;
        let keep_bytes = kind == PackageKind::DeckPack
            || planned.path == kind.manifest_name()
            || planned.path == "integrity.json";
        if keep_bytes
            && planned.byte_length > MAX_JSON_BYTES as u64
            && kind == PackageKind::CodecPack
        {
            return Err(ExtensionError::new(
                ErrorCode::ManifestInvalid,
                "Codec control JSON exceeds the 1 MiB bound",
            ));
        }
        let measured = read_and_hash(&mut entry, planned.byte_length, keep_bytes)?;
        if let Some(bytes) = measured.bytes {
            contents.insert(planned.path.clone(), bytes);
        }
        files.insert(
            planned.path.clone(),
            FileMeasurement {
                path: planned.path.clone(),
                byte_length: measured.byte_length,
                sha256: measured.sha256,
            },
        );
    }
    Ok((files, directories, contents))
}

fn validate_contract(
    kind: PackageKind,
    files: &BTreeMap<String, FileMeasurement>,
    explicit_directories: &BTreeSet<String>,
    contents: &BTreeMap<String, Vec<u8>>,
) -> Result<ValidatedDirectory> {
    let manifest_bytes = contents.get(kind.manifest_name()).ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "control manifest bytes are unavailable",
        )
    })?;
    let catalog_bytes = contents.get("integrity.json").ok_or_else(|| {
        ExtensionError::new(ErrorCode::ManifestInvalid, "integrity.json is missing")
    })?;
    let manifest = parse_manifest(kind, manifest_bytes)?;
    let catalog_hash = hash_bytes(catalog_bytes);
    if manifest.integrity().catalog_sha256 != catalog_hash {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "integrity.json hash does not match the control manifest",
        ));
    }
    let catalog = parse_integrity_catalog(catalog_bytes, kind)?;
    if catalog.files.len().saturating_add(2) != files.len() {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "integrity catalog does not close the complete package file tree",
        ));
    }
    for described in &catalog.files {
        let measured = files.get(&described.path).ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("catalogued file is missing: {}", described.path),
            )
        })?;
        if measured.byte_length != described.byte_length || measured.sha256 != described.sha256 {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("catalogued file differs: {}", described.path),
            ));
        }
    }
    for path in files.keys() {
        if path != kind.manifest_name()
            && path != "integrity.json"
            && catalog
                .files
                .binary_search_by_key(&path.as_str(), |file| file.path.as_str())
                .is_err()
        {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("uncatalogued package file: {path}"),
            ));
        }
    }
    let implied_directories = implied_directories(files.keys().map(String::as_str));
    if !explicit_directories.is_subset(&implied_directories) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package contains an empty or uncatalogued directory",
        ));
    }
    require_catalogued(files, &manifest.license().notice_path)?;
    validate_manifest_files(&manifest, files, contents)?;
    let extracted_byte_length = sum_file_lengths(files)?;
    Ok(ValidatedDirectory {
        manifest,
        files: files.clone(),
        manifest_sha256: hash_bytes(manifest_bytes),
        integrity_catalog_sha256: catalog_hash,
        extracted_byte_length,
    })
}

fn validate_manifest_files(
    manifest: &PackageManifest,
    files: &BTreeMap<String, FileMeasurement>,
    contents: &BTreeMap<String, Vec<u8>>,
) -> Result<()> {
    match &manifest {
        PackageManifest::Deck(deck) => {
            require_catalogued(files, &deck.runtime.operator_descriptor_path)?;
            require_catalogued(files, &deck.faceplate_path)?;
            validate_deck_contents(contents)?;
            let module = deck
                .runtime
                .entrypoint
                .split_once(':')
                .expect("schema validated")
                .0;
            require_python_entrypoint(files, &deck.runtime.python_root, module)?;
        }
        PackageManifest::Codec(codec) => {
            require_catalogued(files, &codec.worker.executable)?;
            let runtime_lock = files.get(&codec.runtime_lock.path).ok_or_else(|| {
                ExtensionError::new(ErrorCode::IntegrityFailed, "runtime lock is not catalogued")
            })?;
            if runtime_lock.sha256 != codec.runtime_lock.sha256 {
                return Err(ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    "runtime lock hash differs from codec-pack.json",
                ));
            }
            let module = codec
                .adapter
                .entrypoint
                .split_once(':')
                .expect("schema validated")
                .0;
            require_codec_python_entrypoint(files, &codec.worker.working_directory, module)?;
        }
    }
    Ok(())
}

fn sum_file_lengths(files: &BTreeMap<String, FileMeasurement>) -> Result<u64> {
    files
        .values()
        .try_fold(0_u64, |total, file| total.checked_add(file.byte_length))
        .ok_or_else(|| ExtensionError::new(ErrorCode::IntegrityFailed, "package length overflowed"))
}

fn validate_deck_contents(contents: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    for (path, bytes) in contents {
        let extension = Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "json" => validate_strict_json_value(bytes, path)?,
            "py" | "txt" | "md" => {
                std::str::from_utf8(bytes).map_err(|_| {
                    ExtensionError::new(
                        ErrorCode::ManifestInvalid,
                        format!("{path} is not strict UTF-8"),
                    )
                })?;
                if bytes.contains(&0) {
                    return Err(ExtensionError::new(
                        ErrorCode::ManifestInvalid,
                        format!("{path} contains a NUL byte"),
                    ));
                }
            }
            "png" if !bytes.starts_with(PNG_SIGNATURE) => {
                return Err(ExtensionError::new(
                    ErrorCode::ManifestInvalid,
                    format!("{path} is not a PNG file"),
                ));
            }
            "png" => {}
            _ => unreachable!("Deck extension checked before content validation"),
        }
    }
    Ok(())
}

fn require_catalogued(files: &BTreeMap<String, FileMeasurement>, path: &str) -> Result<()> {
    if path == "integrity.json" || path.ends_with('/') || !files.contains_key(path) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("required package file is missing or uncatalogued: {path}"),
        ));
    }
    Ok(())
}

fn require_python_entrypoint(
    files: &BTreeMap<String, FileMeasurement>,
    root: &str,
    module: &str,
) -> Result<()> {
    let module_path = module.replace('.', "/");
    let prefix = if root == "." {
        String::new()
    } else {
        format!("{root}/")
    };
    let module_file = format!("{prefix}{module_path}.py");
    let package_file = format!("{prefix}{module_path}/__init__.py");
    if !files.contains_key(&module_file) && !files.contains_key(&package_file) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "declared Python entrypoint is not present in the integrity catalog",
        ));
    }
    Ok(())
}

fn require_codec_python_entrypoint(
    files: &BTreeMap<String, FileMeasurement>,
    working_directory: &str,
    module: &str,
) -> Result<()> {
    let module_path = module.replace('.', "/");
    let root = working_directory.trim_end_matches('/');
    for python_root in [root.to_owned(), format!("{root}/Lib/site-packages")] {
        let module_file = format!("{python_root}/{module_path}.py");
        let package_file = format!("{python_root}/{module_path}/__init__.py");
        if files.contains_key(&module_file) || files.contains_key(&package_file) {
            return Ok(());
        }
    }
    Err(ExtensionError::new(
        ErrorCode::IntegrityFailed,
        "declared Codec adapter entrypoint is not present in the isolated Python roots",
    ))
}

fn implied_directories<'a>(paths: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for path in paths {
        let mut current = path;
        while let Some((parent, _)) = current.rsplit_once('/') {
            directories.insert(parent.to_owned());
            current = parent;
        }
    }
    directories
}

fn scan_directory(
    root: &Path,
    current: &Path,
    depth: usize,
    files: &mut Vec<(String, PathBuf)>,
    directories: &mut BTreeSet<String>,
) -> Result<()> {
    if depth > MAX_TREE_DEPTH {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package directory depth exceeds its bound",
        ));
    }
    for entry in fs::read_dir(current)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "read package directory", &error))?
    {
        let entry = entry.map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "read package directory entry", &error)
        })?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            ExtensionError::io(ErrorCode::Io, "inspect package directory entry", &error)
        })?;
        if is_reparse_or_symlink(&metadata) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package tree contains a symlink or reparse point",
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package path escaped its root",
            )
        })?;
        let portable = relative.to_string_lossy().replace('\\', "/");
        validate_portable_relative_path(&portable, metadata.is_dir())?;
        if metadata.is_dir() {
            directories.insert(portable);
            scan_directory(root, &path, depth + 1, files, directories)?;
        } else if metadata.is_file() {
            files.push((portable, path));
            if files.len() > MAX_ARCHIVE_ENTRIES {
                return Err(ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    "package tree exceeds the absolute entry bound",
                ));
            }
        } else {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "package tree contains a special filesystem entry",
            ));
        }
    }
    Ok(())
}

struct ReadMeasurement {
    byte_length: u64,
    sha256: String,
    bytes: Option<Vec<u8>>,
}

fn read_and_hash<R: Read>(
    reader: &mut R,
    expected: u64,
    keep_bytes: bool,
) -> Result<ReadMeasurement> {
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut stored = if keep_bytes {
        Some(Vec::with_capacity(usize::try_from(expected).map_err(
            |_| ExtensionError::new(ErrorCode::ArchiveInvalid, "file cannot fit in memory"),
        )?))
    } else {
        None
    };
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            ExtensionError::io(ErrorCode::ArchiveInvalid, "read package file", &error)
        })?;
        if read == 0 {
            break;
        }
        byte_length = byte_length.checked_add(read as u64).ok_or_else(|| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "file length overflowed")
        })?;
        if byte_length > expected {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "file expanded beyond its declared ZIP length",
            ));
        }
        hasher.update(&buffer[..read]);
        if let Some(bytes) = stored.as_mut() {
            bytes.extend_from_slice(&buffer[..read]);
        }
    }
    if byte_length != expected {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "file length differs from its ZIP header",
        ));
    }
    Ok(ReadMeasurement {
        byte_length,
        sha256: hex::encode(hasher.finalize()),
        bytes: stored,
    })
}

fn copy_and_hash<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    expected: u64,
) -> Result<ReadMeasurement> {
    let mut hasher = Sha256::new();
    let mut byte_length = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "read package stream", &error))?;
        if read == 0 {
            break;
        }
        byte_length = byte_length.checked_add(read as u64).ok_or_else(|| {
            ExtensionError::new(ErrorCode::IntegrityFailed, "stream length overflowed")
        })?;
        if byte_length > expected {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "package stream grew while copying",
            ));
        }
        hasher.update(&buffer[..read]);
        writer
            .write_all(&buffer[..read])
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "write package stream", &error))?;
    }
    Ok(ReadMeasurement {
        byte_length,
        sha256: hex::encode(hasher.finalize()),
        bytes: None,
    })
}

fn hash_open_file(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "rewind package", &error))?;
    let length = file
        .metadata()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package", &error))?
        .len();
    let measured = read_and_hash(file, length, false)?;
    Ok(measured.sha256)
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn open_regular_no_follow(path: &Path, exclusive: bool) -> Result<File> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package file", &error))?;
    if !metadata.is_file() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package input is not a regular non-reparse file",
        ));
    }
    ensure_existing_tree_safe(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(u32::from(!exclusive));
    }
    let _ = exclusive;
    options
        .open(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "open package file", &error))
}

pub(crate) fn ensure_existing_tree_safe(path: &Path) -> Result<()> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_reparse_or_symlink(&metadata) => {
                return Err(ExtensionError::new(
                    ErrorCode::LifecycleConflict,
                    "path contains a symlink or reparse-point component",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ExtensionError::io(
                    ErrorCode::Io,
                    "inspect path component",
                    &error,
                ));
            }
        }
    }
    Ok(())
}

fn ensure_descendant_safe(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        ExtensionError::new(ErrorCode::LifecycleConflict, "path escaped extraction root")
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect extracted path", &error))?;
        if is_reparse_or_symlink(&metadata) {
            return Err(ExtensionError::new(
                ErrorCode::LifecycleConflict,
                "extracted path contains a reparse point",
            ));
        }
    }
    Ok(())
}

fn validate_output(path: &Path, kind: PackageKind) -> Result<()> {
    let extension = path.extension().and_then(|value| value.to_str());
    if extension != Some(kind.archive_extension()) {
        return Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            format!(
                "output must use the .{} extension",
                kind.archive_extension()
            ),
        ));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Err(ExtensionError::new(
            ErrorCode::PackageExists,
            "package output already exists",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ExtensionError::io(
            ErrorCode::Io,
            "inspect package output",
            &error,
        )),
    }
}

fn kind_from_archive_extension(path: &Path) -> Result<PackageKind> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("ld") => Ok(PackageKind::DeckPack),
        Some("ldcodec") => Ok(PackageKind::CodecPack),
        _ => Err(ExtensionError::new(
            ErrorCode::InvalidArguments,
            "package archive must use canonical .ld or .ldcodec extension",
        )),
    }
}

fn path_from_archive(path: &str) -> PathBuf {
    path.split('/').collect()
}

#[cfg(windows)]
pub(crate) fn is_reparse_or_symlink(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
pub(crate) fn is_reparse_or_symlink(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

struct TemporaryFileGuard(Option<PathBuf>);

impl TemporaryFileGuard {
    fn disarm(self) {
        let mut this = self;
        this.0 = None;
    }
}

impl Drop for TemporaryFileGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}
