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
    MAX_CODEC_EXTRACTED_BYTES, MAX_CODEC_FILES, MAX_DECK_FILE_BYTES, MAX_JSON_BYTES,
    max_archive_bytes, max_extracted_bytes, max_files, parse_integrity_catalog, parse_manifest,
    validate_deck_file_extension, validate_portable_relative_path, validate_sha256,
    validate_strict_json_value,
};

const MAX_ARCHIVE_ENTRIES: usize = MAX_CODEC_FILES;
const MAX_TREE_DEPTH: usize = 256;
const COPY_BUFFER_BYTES: usize = 1024 * 1024;
const ZIP32_EOCD_MIN_BYTES: usize = 22;
const ZIP32_EOCD_SCAN_BYTES: u64 = 22 + 65_535;
const ZIP64_LOCATOR_BYTES: u64 = 20;
const ZIP64_EOCD_MIN_BYTES: usize = 56;
const ZIP64_EOCD_SCAN_BYTES: u64 = 1024 * 1024;

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

struct ExpectedDirectoryLayout {
    kind: PackageKind,
    files: BTreeSet<String>,
    directories: BTreeSet<String>,
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
    preflight_zip_metadata(&mut file, archive_byte_length)?;
    let archive_sha256 = hash_open_file(&mut file)?;
    if expected_sha256.is_some_and(|expected| expected != archive_sha256) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("archive SHA-256 mismatch; measured {archive_sha256}"),
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "rewind archive", &error))?;
    let mut archive = open_archive_with_unique_central_directory(file)?;
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

fn preflight_zip_metadata(file: &mut File, archive_byte_length: u64) -> Result<()> {
    if archive_byte_length < ZIP32_EOCD_MIN_BYTES as u64 {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "ZIP end-of-central-directory record is missing",
        ));
    }
    let tail_length = archive_byte_length.min(ZIP32_EOCD_SCAN_BYTES);
    let tail_start = archive_byte_length - tail_length;
    let tail_capacity = usize::try_from(tail_length).map_err(|_| {
        ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "ZIP metadata scan length cannot be represented safely",
        )
    })?;
    let mut tail = vec![0_u8; tail_capacity];
    file.seek(SeekFrom::Start(tail_start)).map_err(|error| {
        ExtensionError::io(ErrorCode::ArchiveInvalid, "seek ZIP tail metadata", &error)
    })?;
    file.read_exact(&mut tail).map_err(|error| {
        ExtensionError::io(ErrorCode::ArchiveInvalid, "read ZIP tail metadata", &error)
    })?;

    let mut selected = None;
    for offset in (0..=tail.len() - ZIP32_EOCD_MIN_BYTES).rev() {
        if tail[offset..].starts_with(b"PK\x05\x06") {
            let comment_length =
                usize::from(u16::from_le_bytes([tail[offset + 20], tail[offset + 21]]));
            if offset
                .checked_add(ZIP32_EOCD_MIN_BYTES)
                .and_then(|end| end.checked_add(comment_length))
                == Some(tail.len())
            {
                selected = Some(offset);
                break;
            }
        }
    }
    let eocd_tail_offset = selected.ok_or_else(|| {
        ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "ZIP end-of-central-directory record is absent from its bounded tail",
        )
    })?;
    let eocd_offset = tail_start
        .checked_add(eocd_tail_offset as u64)
        .ok_or_else(|| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "ZIP EOCD offset overflowed")
        })?;
    let eocd = &tail[eocd_tail_offset..eocd_tail_offset + ZIP32_EOCD_MIN_BYTES];
    let count = declared_zip_entry_count(file, eocd, eocd_offset)?;
    if count > MAX_ARCHIVE_ENTRIES as u64 {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            format!(
                "ZIP declares more than {MAX_ARCHIVE_ENTRIES} entries before metadata allocation"
            ),
        ));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "rewind ZIP metadata", &error))?;
    Ok(())
}

fn declared_zip_entry_count(file: &mut File, eocd: &[u8], eocd_offset: u64) -> Result<u64> {
    let entries_on_disk = u16::from_le_bytes([eocd[8], eocd[9]]);
    let total_entries = u16::from_le_bytes([eocd[10], eocd[11]]);
    let central_size = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]);
    let central_offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]);
    let needs_zip64 = entries_on_disk == u16::MAX
        || total_entries == u16::MAX
        || central_size == u32::MAX
        || central_offset == u32::MAX;
    if !needs_zip64 {
        return Ok(u64::from(total_entries));
    }
    read_zip64_entry_count(file, eocd_offset)
}

fn read_zip64_entry_count(file: &mut File, eocd_offset: u64) -> Result<u64> {
    let locator_offset = eocd_offset
        .checked_sub(ZIP64_LOCATOR_BYTES)
        .ok_or_else(invalid_zip64_metadata)?;
    let mut locator = [0_u8; 20];
    file.seek(SeekFrom::Start(locator_offset))
        .and_then(|_| file.read_exact(&mut locator))
        .map_err(|_| invalid_zip64_metadata())?;
    if !locator.starts_with(b"PK\x06\x07") {
        return Err(invalid_zip64_metadata());
    }
    let relative_offset = u64::from_le_bytes(
        locator[8..16]
            .try_into()
            .expect("ZIP64 locator offset has fixed width"),
    );
    if let Some(count) = read_zip64_count_at(file, relative_offset, locator_offset)? {
        return Ok(count);
    }

    let scan_start = locator_offset.saturating_sub(ZIP64_EOCD_SCAN_BYTES);
    let scan_length = usize::try_from(locator_offset - scan_start).map_err(|_| {
        ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "ZIP64 metadata scan length cannot be represented safely",
        )
    })?;
    let mut bytes = vec![0_u8; scan_length];
    file.seek(SeekFrom::Start(scan_start))
        .and_then(|_| file.read_exact(&mut bytes))
        .map_err(|_| invalid_zip64_metadata())?;
    let Some(last_record_offset) = bytes.len().checked_sub(ZIP64_EOCD_MIN_BYTES) else {
        return Err(invalid_zip64_metadata());
    };
    for offset in (0..=last_record_offset).rev() {
        if !bytes[offset..].starts_with(b"PK\x06\x06") {
            continue;
        }
        let record_size = u64::from_le_bytes(
            bytes[offset + 4..offset + 12]
                .try_into()
                .expect("ZIP64 record size has fixed width"),
        );
        let absolute_offset = scan_start + offset as u64;
        if record_size >= 44
            && absolute_offset
                .checked_add(12)
                .and_then(|start| start.checked_add(record_size))
                == Some(locator_offset)
        {
            return Ok(u64::from_le_bytes(
                bytes[offset + 32..offset + 40]
                    .try_into()
                    .expect("ZIP64 entry count has fixed width"),
            ));
        }
    }
    Err(invalid_zip64_metadata())
}

fn read_zip64_count_at(file: &mut File, offset: u64, locator_offset: u64) -> Result<Option<u64>> {
    if offset
        .checked_add(ZIP64_EOCD_MIN_BYTES as u64)
        .is_none_or(|end| end > locator_offset)
    {
        return Ok(None);
    }
    let mut header = [0_u8; ZIP64_EOCD_MIN_BYTES];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut header))
        .map_err(|_| invalid_zip64_metadata())?;
    if !header.starts_with(b"PK\x06\x06") {
        return Ok(None);
    }
    let record_size = u64::from_le_bytes(
        header[4..12]
            .try_into()
            .expect("ZIP64 record size has fixed width"),
    );
    if record_size < 44
        || offset
            .checked_add(12)
            .and_then(|start| start.checked_add(record_size))
            != Some(locator_offset)
    {
        return Ok(None);
    }
    Ok(Some(u64::from_le_bytes(
        header[32..40]
            .try_into()
            .expect("ZIP64 entry count has fixed width"),
    )))
}

fn invalid_zip64_metadata() -> ExtensionError {
    ExtensionError::new(
        ErrorCode::ArchiveInvalid,
        "ZIP64 end metadata is missing, malformed, or exceeds its bounded scan",
    )
}

fn open_archive_with_unique_central_directory(file: File) -> Result<ZipArchive<File>> {
    let archive = ZipArchive::new(file).map_err(|error| {
        ExtensionError::new(ErrorCode::ArchiveInvalid, format!("open ZIP: {error}"))
    })?;
    let unique_entry_count = archive.len();
    let central_directory_start = archive.central_directory_start();
    let mut file = archive.into_inner();
    validate_raw_central_directory(&mut file, central_directory_start, unique_entry_count)?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "rewind archive", &error))?;
    ZipArchive::new(file).map_err(|error| {
        ExtensionError::new(ErrorCode::ArchiveInvalid, format!("reopen ZIP: {error}"))
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
    let mut retained_directories = vec![open_directory_pin(root, true)?];
    let expected = expected_directory_layout(root, expected_kind)?;
    let mut file_paths = Vec::new();
    let mut directories = BTreeSet::new();
    scan_directory(&expected, root, root, 0, &mut file_paths, &mut directories)?;
    file_paths.sort_by(|left, right| left.0.cmp(&right.0));
    let mut directory_paths: Vec<_> = directories
        .iter()
        .map(|relative| root.join(path_from_archive(relative)))
        .collect();
    directory_paths.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    for directory in directory_paths {
        retained_directories.push(open_directory_pin(&directory, false)?);
    }
    let kind = expected.kind;
    if file_paths.is_empty() || file_paths.len() > max_files(kind) {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            format!("package tree exceeds the {} file bound", max_files(kind)),
        ));
    }
    let mut jobs = Vec::with_capacity(file_paths.len());
    let mut extracted = 0_u64;
    for (index, (relative, full_path)) in file_paths.into_iter().enumerate() {
        if kind == PackageKind::DeckPack {
            validate_deck_file_extension(&relative)?;
        }
        let file = open_regular_under_pinned_tree(&full_path)?;
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
        jobs.push(DirectoryHashJob {
            index,
            relative,
            byte_length,
            keep_bytes,
            file,
        });
    }
    let measured = hash_directory_jobs(jobs)?;
    let mut files = BTreeMap::new();
    let mut contents = BTreeMap::new();
    let mut retained_files = Vec::with_capacity(measured.len());
    for item in measured {
        if let Some(bytes) = item.bytes {
            contents.insert(item.measurement.path.clone(), bytes);
        }
        files.insert(item.measurement.path.clone(), item.measurement);
        retained_files.push(item.file);
    }
    let mut validated = validate_contract(kind, &files, &directories, &contents)?;
    validated.extracted_byte_length = extracted;
    validate_directory_snapshot(root, kind, &validated.files)?;
    drop(retained_files);
    drop(retained_directories);
    Ok(validated)
}

pub(crate) fn validate_directory_snapshot(
    root: &Path,
    kind: PackageKind,
    expected_files: &BTreeMap<String, FileMeasurement>,
) -> Result<()> {
    ensure_existing_tree_safe(root)?;
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package directory", &error))?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package root is not a regular non-reparse directory",
        ));
    }
    let expected = ExpectedDirectoryLayout {
        kind,
        files: expected_files.keys().cloned().collect(),
        directories: implied_directories(expected_files.keys().map(String::as_str)),
    };
    let mut observed_files = Vec::new();
    let mut observed_directories = BTreeSet::new();
    scan_directory(
        &expected,
        root,
        root,
        0,
        &mut observed_files,
        &mut observed_directories,
    )?;
    if observed_directories != expected.directories || observed_files.len() != expected_files.len()
    {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "package tree no longer matches its validated closed layout",
        ));
    }
    for (relative, path) in observed_files {
        let expected_file = expected_files.get(&relative).ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("uncatalogued package file: {relative}"),
            )
        })?;
        let observed_length = fs::symlink_metadata(&path)
            .map_err(|error| {
                ExtensionError::io(ErrorCode::IntegrityFailed, "reinspect package file", &error)
            })?
            .len();
        if observed_length != expected_file.byte_length {
            return Err(ExtensionError::new(
                ErrorCode::IntegrityFailed,
                format!("catalogued file length changed: {relative}"),
            ));
        }
    }
    Ok(())
}

struct DirectoryHashJob {
    index: usize,
    relative: String,
    byte_length: u64,
    keep_bytes: bool,
    file: File,
}

struct DirectoryHashResult {
    index: usize,
    measurement: FileMeasurement,
    bytes: Option<Vec<u8>>,
    file: File,
}

fn hash_directory_jobs(mut jobs: Vec<DirectoryHashJob>) -> Result<Vec<DirectoryHashResult>> {
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
            .byte_length
            .cmp(&left.byte_length)
            .then_with(|| left.relative.cmp(&right.relative))
    });
    let mut buckets: Vec<Vec<DirectoryHashJob>> = (0..worker_count).map(|_| Vec::new()).collect();
    let mut bucket_bytes = vec![0_u64; worker_count];
    for job in jobs {
        let bucket = bucket_bytes
            .iter()
            .enumerate()
            .min_by_key(|(index, bytes)| (**bytes, *index))
            .map_or(0, |(index, _)| index);
        bucket_bytes[bucket] = bucket_bytes[bucket].saturating_add(job.byte_length);
        buckets[bucket].push(job);
    }
    let workers: Vec<_> = buckets
        .into_iter()
        .map(|bucket| {
            std::thread::spawn(move || -> Result<Vec<DirectoryHashResult>> {
                let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
                bucket
                    .into_iter()
                    .map(|mut job| {
                        let measured = read_and_hash_with_buffer(
                            &mut job.file,
                            job.byte_length,
                            job.keep_bytes,
                            &mut buffer,
                        )?;
                        Ok(DirectoryHashResult {
                            index: job.index,
                            measurement: FileMeasurement {
                                path: job.relative,
                                byte_length: measured.byte_length,
                                sha256: measured.sha256,
                            },
                            bytes: measured.bytes,
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
                "directory hash worker terminated unexpectedly",
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

fn expected_directory_layout(
    root: &Path,
    expected_kind: Option<PackageKind>,
) -> Result<ExpectedDirectoryLayout> {
    let mut controls = Vec::with_capacity(2);
    for kind in [PackageKind::DeckPack, PackageKind::CodecPack] {
        let path = root.join(kind.manifest_name());
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !is_reparse_or_symlink(&metadata) => {
                controls.push(kind.manifest_name());
            }
            Ok(_) => {
                return Err(ExtensionError::new(
                    ErrorCode::LifecycleConflict,
                    "package root control manifest is not a regular non-reparse file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ExtensionError::io(
                    ErrorCode::Io,
                    "inspect package root control manifest",
                    &error,
                ));
            }
        }
    }
    let kind = detect_kind_from_paths(controls.iter().copied())?;
    if expected_kind.is_some_and(|expected| expected != kind) {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "installed package kind differs from its root",
        ));
    }

    let manifest_bytes = read_bounded_control_file(&root.join(kind.manifest_name()))?;
    let manifest = parse_manifest(kind, &manifest_bytes)?;
    let catalog_bytes = read_bounded_control_file(&root.join("integrity.json"))?;
    if hash_bytes(&catalog_bytes) != manifest.integrity().catalog_sha256 {
        return Err(ExtensionError::new(
            ErrorCode::IntegrityFailed,
            "integrity.json hash does not match the control manifest",
        ));
    }
    let catalog = parse_integrity_catalog(&catalog_bytes, kind)?;
    let mut files = BTreeSet::from([kind.manifest_name().to_owned(), "integrity.json".to_owned()]);
    files.extend(catalog.files.into_iter().map(|file| file.path));
    let directories = implied_directories(files.iter().map(String::as_str));
    Ok(ExpectedDirectoryLayout {
        kind,
        files,
        directories,
    })
}

fn read_bounded_control_file(path: &Path) -> Result<Vec<u8>> {
    let mut file = open_regular_no_follow(path, false)?;
    let byte_length = file
        .metadata()
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect control file", &error))?
        .len();
    if byte_length > MAX_JSON_BYTES as u64 {
        return Err(ExtensionError::new(
            ErrorCode::ManifestInvalid,
            "package control JSON exceeds the 1 MiB bound",
        ));
    }
    read_and_hash(&mut file, byte_length, true).and_then(|measured| {
        measured.bytes.ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::IntegrityFailed,
                "bounded package control bytes are unavailable",
            )
        })
    })
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

fn validate_raw_central_directory(
    file: &mut File,
    central_directory_start: u64,
    unique_entry_count: usize,
) -> Result<()> {
    const CENTRAL_ENTRY_SIGNATURE: [u8; 4] = *b"PK\x01\x02";
    const END_SIGNATURES: [[u8; 4]; 4] = [
        *b"PK\x05\x05",
        *b"PK\x06\x06",
        *b"PK\x06\x07",
        *b"PK\x05\x06",
    ];
    file.seek(SeekFrom::Start(central_directory_start))
        .map_err(|error| {
            ExtensionError::io(
                ErrorCode::ArchiveInvalid,
                "seek ZIP central directory",
                &error,
            )
        })?;
    let mut paths = HashSet::new();
    let mut entry_count = 0_usize;
    loop {
        let mut signature = [0_u8; 4];
        file.read_exact(&mut signature).map_err(|error| {
            ExtensionError::io(
                ErrorCode::ArchiveInvalid,
                "read ZIP central-directory signature",
                &error,
            )
        })?;
        if END_SIGNATURES.contains(&signature) {
            break;
        }
        if signature != CENTRAL_ENTRY_SIGNATURE {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP central directory contains an unexpected record",
            ));
        }
        entry_count = entry_count.checked_add(1).ok_or_else(|| {
            ExtensionError::new(ErrorCode::ArchiveInvalid, "ZIP entry count overflowed")
        })?;
        if entry_count > MAX_ARCHIVE_ENTRIES {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                format!("ZIP entry count exceeds {MAX_ARCHIVE_ENTRIES}"),
            ));
        }
        let mut fixed = [0_u8; 42];
        file.read_exact(&mut fixed).map_err(|error| {
            ExtensionError::io(
                ErrorCode::ArchiveInvalid,
                "read ZIP central-directory entry",
                &error,
            )
        })?;
        let name_length = usize::from(u16::from_le_bytes([fixed[24], fixed[25]]));
        let extra_length = u64::from(u16::from_le_bytes([fixed[26], fixed[27]]));
        let comment_length = u64::from(u16::from_le_bytes([fixed[28], fixed[29]]));
        if name_length == 0 || name_length > 241 {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP path is outside the portable path bound",
            ));
        }
        let mut name = vec![0_u8; name_length];
        file.read_exact(&mut name).map_err(|error| {
            ExtensionError::io(ErrorCode::ArchiveInvalid, "read ZIP central path", &error)
        })?;
        if !paths.insert(name) {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP contains a duplicate path",
            ));
        }
        let trailing_length = extra_length.checked_add(comment_length).ok_or_else(|| {
            ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP central-directory length overflowed",
            )
        })?;
        let trailing_length = i64::try_from(trailing_length).map_err(|_| {
            ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                "ZIP central-directory length is unsupported",
            )
        })?;
        file.seek(SeekFrom::Current(trailing_length))
            .map_err(|error| {
                ExtensionError::io(
                    ErrorCode::ArchiveInvalid,
                    "skip ZIP central-directory metadata",
                    &error,
                )
            })?;
    }
    if entry_count != unique_entry_count {
        return Err(ExtensionError::new(
            ErrorCode::ArchiveInvalid,
            "ZIP central-directory identity count is inconsistent",
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
        let entry = archive.by_index_raw(index).map_err(|error| {
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
    let implied = implied_directories(
        plan.iter()
            .filter(|entry| !entry.directory)
            .map(|entry| entry.path.as_str()),
    );
    for directory in plan.iter().filter(|entry| entry.directory) {
        if !implied.contains(&directory.path) {
            return Err(ExtensionError::new(
                ErrorCode::ArchiveInvalid,
                format!(
                    "ZIP contains unexpected or empty directory: {}",
                    directory.path
                ),
            ));
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

pub(crate) fn validate_contract(
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
    expected: &ExpectedDirectoryLayout,
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
            if !expected.directories.contains(&portable) {
                return Err(ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    format!("package tree contains unexpected or empty directory: {portable}"),
                ));
            }
            directories.insert(portable);
            scan_directory(expected, root, &path, depth + 1, files, directories)?;
        } else if metadata.is_file() {
            if expected.kind == PackageKind::DeckPack {
                validate_deck_file_extension(&portable)?;
            }
            if !expected.files.contains(&portable) {
                return Err(ExtensionError::new(
                    ErrorCode::IntegrityFailed,
                    format!("uncatalogued package file: {portable}"),
                ));
            }
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
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    read_and_hash_with_buffer(reader, expected, keep_bytes, &mut buffer)
}

fn read_and_hash_with_buffer<R: Read>(
    reader: &mut R,
    expected: u64,
    keep_bytes: bool,
    buffer: &mut [u8],
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
    loop {
        let read = reader.read(buffer).map_err(|error| {
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

fn open_directory_pin(path: &Path, check_ancestors: bool) -> Result<File> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package directory", &error))?;
    if !metadata.is_dir() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package directory is not a regular non-reparse directory",
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
    let file = options
        .open(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "open package directory pin", &error))?;
    let opened = file.metadata().map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "inspect package directory pin", &error)
    })?;
    if !opened.is_dir() || is_reparse_or_symlink(&opened) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "opened package directory pin is unsafe",
        ));
    }
    Ok(file)
}

fn open_regular_under_pinned_tree(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "inspect package file", &error))?;
    if !metadata.is_file() || is_reparse_or_symlink(&metadata) {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "package input is not a regular non-reparse file",
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
    let file = options
        .open(path)
        .map_err(|error| ExtensionError::io(ErrorCode::Io, "open package file", &error))?;
    let opened = file.metadata().map_err(|error| {
        ExtensionError::io(ErrorCode::Io, "inspect opened package file", &error)
    })?;
    if !opened.is_file() || is_reparse_or_symlink(&opened) || opened.len() != metadata.len() {
        return Err(ExtensionError::new(
            ErrorCode::LifecycleConflict,
            "opened package file changed identity or length",
        ));
    }
    Ok(file)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_stays_bound_to_the_preflighted_handle_during_path_swap_attempt() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("source");
        copy_catalogued_deck_fixture(&source);
        let archive_path = temp.path().join("preflight.ld");
        let receipt = pack(&PackRequest {
            source_directory: source,
            output_path: archive_path.clone(),
        })
        .expect("pack fixture");
        let replacement_path = temp.path().join("replacement.ld");
        let replacement_bytes = b"replacement archive bytes";
        fs::write(&replacement_path, replacement_bytes).expect("write replacement");

        let mut prepared = prepare_archive(
            &archive_path,
            Some(&receipt.inspection.archive_sha256),
            Some(PackageKind::DeckPack),
        )
        .expect("preflight fixture");
        let preflight_files = prepared.files.clone();
        let moved_path = temp.path().join("preflight-moved.ld");
        let path_was_swapped = fs::rename(&archive_path, &moved_path).is_ok();
        if path_was_swapped {
            fs::rename(&replacement_path, &archive_path).expect("swap archive path");
        } else {
            let mutation = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&archive_path);
            assert!(
                mutation.is_err(),
                "an exclusive retained handle must reject in-place mutation"
            );
        }

        let destination = temp.path().join("extracted");
        fs::create_dir(&destination).expect("create destination");
        extract_prepared(&mut prepared, &destination).expect("extract retained archive handle");

        assert_eq!(prepared.files, preflight_files);
        assert_eq!(
            prepared.inspection.archive_sha256,
            receipt.inspection.archive_sha256
        );
        if path_was_swapped {
            assert_eq!(
                fs::read(&archive_path).expect("read replacement path"),
                replacement_bytes
            );
        }
    }

    fn copy_catalogued_deck_fixture(destination: &Path) {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../operators/builtin/d2/package");
        let integrity_bytes =
            fs::read(source.join("integrity.json")).expect("read fixture integrity catalog");
        let integrity: serde_json::Value =
            serde_json::from_slice(&integrity_bytes).expect("parse fixture integrity catalog");
        let mut paths = vec!["deck-pack.json".to_owned(), "integrity.json".to_owned()];
        paths.extend(
            integrity["files"]
                .as_array()
                .expect("integrity files")
                .iter()
                .map(|entry| {
                    entry["path"]
                        .as_str()
                        .expect("integrity file path")
                        .to_owned()
                }),
        );
        for relative in paths {
            let relative_path = path_from_archive(&relative);
            let output = destination.join(&relative_path);
            fs::create_dir_all(output.parent().expect("fixture parent"))
                .expect("create fixture parent");
            fs::copy(source.join(relative_path), output).expect("copy fixture file");
        }
    }
}
