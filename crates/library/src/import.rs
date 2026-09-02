use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use latentdeck_cartridge::{
    manifest::AudioDisposition,
    reader::{ValidationOptions, open_integrity_validated},
    signal::validate_codec_neutral_signal_geometry,
    writer::canonical_json_bytes,
};
use rusqlite::{OptionalExtension as _, params};

use crate::{
    CartridgeKey, ErrorCode, FolderImportOptions, FolderImportReport, ImportDisposition,
    ImportResult, Library, LibraryError, PathState, ReindexDisposition, ReindexResult,
    RejectedImport, Result,
    db::{next_sequence, normalize, now_ms, u64_to_i64},
};

const MAX_PATH_TEXT_BYTES: usize = 32_768;
const MAX_FOLDER_CANDIDATES: usize = 100_000;

struct PreparedImport {
    canonical_path: String,
    file_name_normalized: String,
    key: CartridgeKey,
    cartridge_id: String,
    archive_bytes: u64,
    manifest_json: String,
    codec_family: String,
    codec_profile: String,
    codec_profile_version: String,
    timing_contract: String,
    timing_contract_version: String,
    decoded_width: u32,
    decoded_height: u32,
    decoded_frame_count: u64,
    frame_rate_numerator: u64,
    frame_rate_denominator: u64,
    duration_numerator: u64,
    duration_denominator: u64,
    audio_policy: &'static str,
    has_preview: bool,
    file_size: u64,
    modified_ns: i64,
}

impl Library {
    /// Explicitly imports one selected `.lc` after full LC validation and
    /// archive hashing. Re-importing a changed registered path is the only API
    /// that accepts the replacement identity.
    ///
    /// # Errors
    ///
    /// Rejects non-files, links, non-LC paths, invalid cartridges, and DB
    /// failures without interpolating the machine path into the error.
    pub fn import_file(&mut self, path: impl AsRef<Path>) -> Result<ImportResult> {
        let prepared = prepare_file(path.as_ref())?;
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        insert_cartridge_if_new(&transaction, &prepared)?;
        let (disposition, previous_key) = upsert_path(&transaction, &prepared)?;
        transaction.commit().map_err(LibraryError::database)?;
        Ok(ImportResult {
            disposition,
            key: prepared.key,
            previous_key,
            path: PathBuf::from(prepared.canonical_path),
        })
    }

    /// Imports `.lc` candidates only from a user-selected folder. Recursion is
    /// opt-in, traversal never follows symbolic-link directories, and the
    /// candidate walk is bounded.
    ///
    /// # Errors
    ///
    /// Rejects an invalid root or candidate ceiling. Individual invalid `.lc`
    /// files are returned in the report instead of aborting valid neighbors.
    pub fn import_folder(
        &mut self,
        folder: impl AsRef<Path>,
        options: &FolderImportOptions,
    ) -> Result<FolderImportReport> {
        if options.max_candidates == 0 || options.max_candidates > MAX_FOLDER_CANDIDATES {
            return Err(LibraryError::new(
                ErrorCode::ImportLimit,
                "folder candidate ceiling is outside the allowed range",
            ));
        }
        let root_metadata = fs::symlink_metadata(folder.as_ref()).map_err(filesystem_error)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(LibraryError::new(
                ErrorCode::InvalidInput,
                "selected import root is not a regular directory",
            ));
        }

        let mut queue = VecDeque::from([folder.as_ref().to_path_buf()]);
        let mut candidates = Vec::new();
        let mut ignored_non_cartridges = 0_usize;
        let mut visited_entries = 0_usize;
        while let Some(directory) = queue.pop_front() {
            let mut entries = fs::read_dir(directory)
                .map_err(filesystem_error)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(filesystem_error)?;
            entries.sort_by_key(fs::DirEntry::file_name);
            for entry in entries {
                visited_entries = visited_entries.checked_add(1).ok_or_else(|| {
                    LibraryError::new(ErrorCode::ImportLimit, "folder candidate count overflowed")
                })?;
                if visited_entries > options.max_candidates {
                    return Err(LibraryError::new(
                        ErrorCode::ImportLimit,
                        "selected folder exceeds the explicit candidate ceiling",
                    ));
                }
                let metadata = fs::symlink_metadata(entry.path()).map_err(filesystem_error)?;
                if metadata.file_type().is_symlink() {
                    if has_lc_extension(&entry.path()) {
                        candidates.push(entry.path());
                    }
                } else if metadata.is_dir() {
                    if options.recursive {
                        queue.push_back(entry.path());
                    }
                } else if metadata.is_file() && has_lc_extension(&entry.path()) {
                    candidates.push(entry.path());
                } else if metadata.is_file() {
                    ignored_non_cartridges = ignored_non_cartridges.saturating_add(1);
                }
            }
        }

        let mut report = FolderImportReport {
            accepted: Vec::with_capacity(candidates.len()),
            rejected: Vec::new(),
            ignored_non_cartridges,
        };
        for path in candidates {
            match self.import_file(&path) {
                Ok(result) => report.accepted.push(result),
                Err(error) => report.rejected.push(RejectedImport {
                    path,
                    code: error.code.as_str().to_owned(),
                    cartridge_code: error.cartridge_code,
                }),
            }
        }
        Ok(report)
    }

    /// Incrementally checks only already registered paths. Unchanged
    /// size/mtime pairs are skipped. Missing and invalid paths remain indexed;
    /// a new valid archive hash is marked `content_changed` and is never
    /// accepted by this method.
    ///
    /// # Errors
    ///
    /// Returns a stable database error if the atomic state update fails.
    pub fn reindex_registered(&mut self) -> Result<Vec<ReindexResult>> {
        let paths = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT path_id, path_text, archive_sha256, file_size, modified_ns, state, \
                     observed_archive_sha256 FROM cartridge_paths ORDER BY path_id",
                )
                .map_err(LibraryError::database)?;
            statement
                .query_map([], |row| {
                    Ok(RegisteredPath {
                        path_id: row.get(0)?,
                        path: PathBuf::from(row.get::<_, String>(1)?),
                        expected_key: CartridgeKey::new_unchecked(row.get::<_, String>(2)?),
                        file_size: row.get(3)?,
                        modified_ns: row.get(4)?,
                        state: row.get(5)?,
                        observed_key: row
                            .get::<_, Option<String>>(6)?
                            .map(CartridgeKey::new_unchecked),
                    })
                })
                .map_err(LibraryError::database)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(LibraryError::database)?
        };

        let mut plans = Vec::with_capacity(paths.len());
        for registered in paths {
            plans.push(plan_reindex(registered));
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        for plan in &plans {
            if plan.disposition == ReindexDisposition::Unchanged {
                continue;
            }
            transaction
                .execute(
                    "UPDATE cartridge_paths SET file_size = ?2, modified_ns = ?3, state = ?4, \
                     warning_code = ?5, observed_archive_sha256 = ?6, last_checked_ms = ?7 \
                     WHERE path_id = ?1",
                    params![
                        plan.path_id,
                        plan.file_size,
                        plan.modified_ns,
                        plan.path_state.as_str(),
                        plan.warning_code,
                        plan.observed_key.as_ref().map(CartridgeKey::as_str),
                        now_ms(),
                    ],
                )
                .map_err(LibraryError::database)?;
        }
        transaction.commit().map_err(LibraryError::database)?;
        Ok(plans
            .into_iter()
            .map(|plan| ReindexResult {
                path_id: plan.path_id,
                expected_key: plan.expected_key,
                observed_key: plan.observed_key,
                disposition: plan.disposition,
            })
            .collect())
    }
}

fn insert_cartridge_if_new(
    transaction: &rusqlite::Transaction<'_>,
    prepared: &PreparedImport,
) -> Result<()> {
    let existing = transaction
        .query_row(
            "SELECT 1 FROM cartridges WHERE archive_sha256 = ?1",
            [prepared.key.as_str()],
            |_| Ok(()),
        )
        .optional()
        .map_err(LibraryError::database)?
        .is_some();
    if existing {
        return Ok(());
    }
    let sequence = next_sequence(transaction, "cartridges", "import_sequence")?;
    transaction
        .execute(
            "INSERT INTO cartridges(archive_sha256, cartridge_id, archive_bytes, \
             manifest_json, codec_family, codec_profile, codec_profile_version, \
             timing_contract, timing_contract_version, decoded_width, decoded_height, \
             decoded_frame_count, frame_rate_numerator, frame_rate_denominator, \
             duration_numerator, duration_denominator, audio_policy, has_preview, \
             import_sequence, created_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
             ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
            params![
                prepared.key.as_str(),
                prepared.cartridge_id,
                u64_to_i64(prepared.archive_bytes)?,
                prepared.manifest_json,
                prepared.codec_family,
                prepared.codec_profile,
                prepared.codec_profile_version,
                prepared.timing_contract,
                prepared.timing_contract_version,
                i64::from(prepared.decoded_width),
                i64::from(prepared.decoded_height),
                u64_to_i64(prepared.decoded_frame_count)?,
                u64_to_i64(prepared.frame_rate_numerator)?,
                u64_to_i64(prepared.frame_rate_denominator)?,
                u64_to_i64(prepared.duration_numerator)?,
                u64_to_i64(prepared.duration_denominator)?,
                prepared.audio_policy,
                i64::from(prepared.has_preview),
                sequence,
                now_ms(),
            ],
        )
        .map_err(LibraryError::database)?;
    Ok(())
}

fn upsert_path(
    transaction: &rusqlite::Transaction<'_>,
    prepared: &PreparedImport,
) -> Result<(ImportDisposition, Option<CartridgeKey>)> {
    let previous = transaction
        .query_row(
            "SELECT archive_sha256 FROM cartridge_paths WHERE path_text = ?1",
            [&prepared.canonical_path],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(LibraryError::database)?
        .map(CartridgeKey::new_unchecked);
    match previous {
        None => insert_path(transaction, prepared),
        Some(previous) if previous == prepared.key => refresh_path(transaction, prepared),
        Some(previous) => replace_path(transaction, prepared, previous),
    }
}

fn insert_path(
    transaction: &rusqlite::Transaction<'_>,
    prepared: &PreparedImport,
) -> Result<(ImportDisposition, Option<CartridgeKey>)> {
    transaction
        .execute(
            "INSERT INTO cartridge_paths(path_text, file_name_normalized, archive_sha256, \
             file_size, modified_ns, state, warning_code, observed_archive_sha256, \
             last_checked_ms) VALUES (?1, ?2, ?3, ?4, ?5, 'present', NULL, NULL, ?6)",
            params![
                prepared.canonical_path,
                prepared.file_name_normalized,
                prepared.key.as_str(),
                u64_to_i64(prepared.file_size)?,
                prepared.modified_ns,
                now_ms(),
            ],
        )
        .map_err(LibraryError::database)?;
    Ok((ImportDisposition::Added, None))
}

fn refresh_path(
    transaction: &rusqlite::Transaction<'_>,
    prepared: &PreparedImport,
) -> Result<(ImportDisposition, Option<CartridgeKey>)> {
    transaction
        .execute(
            "UPDATE cartridge_paths SET file_name_normalized = ?2, file_size = ?3, \
             modified_ns = ?4, state = 'present', warning_code = NULL, \
             observed_archive_sha256 = NULL, last_checked_ms = ?5 WHERE path_text = ?1",
            params![
                prepared.canonical_path,
                prepared.file_name_normalized,
                u64_to_i64(prepared.file_size)?,
                prepared.modified_ns,
                now_ms(),
            ],
        )
        .map_err(LibraryError::database)?;
    Ok((ImportDisposition::AlreadyIndexed, None))
}

fn replace_path(
    transaction: &rusqlite::Transaction<'_>,
    prepared: &PreparedImport,
    previous: CartridgeKey,
) -> Result<(ImportDisposition, Option<CartridgeKey>)> {
    transaction
        .execute(
            "UPDATE cartridge_paths SET file_name_normalized = ?2, archive_sha256 = ?3, \
             file_size = ?4, modified_ns = ?5, state = 'present', warning_code = NULL, \
             observed_archive_sha256 = NULL, last_checked_ms = ?6 WHERE path_text = ?1",
            params![
                prepared.canonical_path,
                prepared.file_name_normalized,
                prepared.key.as_str(),
                u64_to_i64(prepared.file_size)?,
                prepared.modified_ns,
                now_ms(),
            ],
        )
        .map_err(LibraryError::database)?;
    Ok((ImportDisposition::AcceptedReplacement, Some(previous)))
}

struct RegisteredPath {
    path_id: i64,
    path: PathBuf,
    expected_key: CartridgeKey,
    file_size: i64,
    modified_ns: i64,
    state: String,
    observed_key: Option<CartridgeKey>,
}

struct ReindexPlan {
    path_id: i64,
    expected_key: CartridgeKey,
    observed_key: Option<CartridgeKey>,
    disposition: ReindexDisposition,
    path_state: PathState,
    warning_code: Option<&'static str>,
    file_size: i64,
    modified_ns: i64,
}

fn plan_reindex(registered: RegisteredPath) -> ReindexPlan {
    let metadata = match fs::symlink_metadata(&registered.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ReindexPlan {
                path_id: registered.path_id,
                expected_key: registered.expected_key,
                observed_key: None,
                disposition: ReindexDisposition::Missing,
                path_state: PathState::Missing,
                warning_code: Some("file_missing"),
                file_size: registered.file_size,
                modified_ns: registered.modified_ns,
            };
        }
        Err(_) => {
            return invalid_reindex_plan(registered, "filesystem_unavailable");
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return invalid_reindex_plan(registered, "path_not_regular_file");
    }
    let file_size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    let modified_ns = metadata_modified_ns(&metadata);
    if registered.file_size == file_size && registered.modified_ns == modified_ns {
        match registered.state.as_str() {
            "present" => {
                return ReindexPlan {
                    path_id: registered.path_id,
                    expected_key: registered.expected_key,
                    observed_key: None,
                    disposition: ReindexDisposition::Unchanged,
                    path_state: PathState::Present,
                    warning_code: None,
                    file_size,
                    modified_ns,
                };
            }
            "content_changed" => {
                return ReindexPlan {
                    path_id: registered.path_id,
                    expected_key: registered.expected_key,
                    observed_key: registered.observed_key,
                    disposition: ReindexDisposition::ContentChanged,
                    path_state: PathState::ContentChanged,
                    warning_code: Some("content_changed"),
                    file_size,
                    modified_ns,
                };
            }
            "invalid" => {
                return ReindexPlan {
                    path_id: registered.path_id,
                    expected_key: registered.expected_key,
                    observed_key: None,
                    disposition: ReindexDisposition::Invalid,
                    path_state: PathState::Invalid,
                    warning_code: Some("cartridge_invalid"),
                    file_size,
                    modified_ns,
                };
            }
            _ => {}
        }
    }

    match prepare_file(&registered.path) {
        Ok(prepared) if prepared.key == registered.expected_key => ReindexPlan {
            path_id: registered.path_id,
            expected_key: registered.expected_key,
            observed_key: None,
            disposition: ReindexDisposition::Present,
            path_state: PathState::Present,
            warning_code: None,
            file_size,
            modified_ns,
        },
        Ok(prepared) => ReindexPlan {
            path_id: registered.path_id,
            expected_key: registered.expected_key,
            observed_key: Some(prepared.key),
            disposition: ReindexDisposition::ContentChanged,
            path_state: PathState::ContentChanged,
            warning_code: Some("content_changed"),
            file_size,
            modified_ns,
        },
        Err(_) => ReindexPlan {
            path_id: registered.path_id,
            expected_key: registered.expected_key,
            observed_key: None,
            disposition: ReindexDisposition::Invalid,
            path_state: PathState::Invalid,
            warning_code: Some("cartridge_invalid"),
            file_size,
            modified_ns,
        },
    }
}

fn invalid_reindex_plan(registered: RegisteredPath, warning: &'static str) -> ReindexPlan {
    ReindexPlan {
        path_id: registered.path_id,
        expected_key: registered.expected_key,
        observed_key: None,
        disposition: ReindexDisposition::Invalid,
        path_state: PathState::Invalid,
        warning_code: Some(warning),
        file_size: registered.file_size,
        modified_ns: registered.modified_ns,
    }
}

fn prepare_file(path: &Path) -> Result<PreparedImport> {
    if !has_lc_extension(path) {
        return Err(LibraryError::new(
            ErrorCode::InvalidInput,
            "cartridge import requires the .lc extension",
        ));
    }
    let source_metadata = fs::symlink_metadata(path).map_err(filesystem_error)?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err(LibraryError::new(
            ErrorCode::InvalidInput,
            "cartridge import requires a regular non-link file",
        ));
    }
    let canonical = fs::canonicalize(path).map_err(filesystem_error)?;
    let canonical_path = canonical.to_str().ok_or_else(|| {
        LibraryError::new(
            ErrorCode::InvalidInput,
            "cartridge path is not representable as Unicode",
        )
    })?;
    if canonical_path.len() > MAX_PATH_TEXT_BYTES {
        return Err(LibraryError::new(
            ErrorCode::InvalidInput,
            "cartridge path exceeds the local index ceiling",
        ));
    }

    let validated = open_integrity_validated(&canonical, &ValidationOptions::default())
        .map_err(|error| LibraryError::cartridge(&error))?;
    validate_codec_neutral_signal_geometry(validated.manifest())
        .map_err(|error| LibraryError::cartridge(&error))?;
    let metadata = fs::metadata(&canonical).map_err(filesystem_error)?;
    if metadata.len() != validated.receipt().archive_bytes {
        return Err(LibraryError::new(
            ErrorCode::Filesystem,
            "cartridge changed while it was being validated",
        ));
    }
    let manifest = validated.manifest();
    let manifest_json = String::from_utf8(
        canonical_json_bytes(manifest).map_err(|error| LibraryError::cartridge(&error))?,
    )
    .map_err(|_error| {
        LibraryError::new(
            ErrorCode::CartridgeRejected,
            "validated manifest is not UTF-8",
        )
    })?;
    let decoded = &manifest.timing.decoded_video;
    let file_name_normalized = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .map(normalize)
        .ok_or_else(|| {
            LibraryError::new(
                ErrorCode::InvalidInput,
                "cartridge filename is not representable as Unicode",
            )
        })?;
    Ok(PreparedImport {
        canonical_path: canonical_path.to_owned(),
        file_name_normalized,
        key: CartridgeKey::new_unchecked(validated.receipt().archive_sha256.to_string()),
        cartridge_id: manifest.cartridge_id.0.clone(),
        archive_bytes: validated.receipt().archive_bytes,
        manifest_json,
        codec_family: manifest.codec.family.0.clone(),
        codec_profile: manifest.codec.profile.0.clone(),
        codec_profile_version: manifest.codec.profile_version.0.clone(),
        timing_contract: manifest.timing.contract.0.clone(),
        timing_contract_version: manifest.timing.contract_version.0.clone(),
        decoded_width: decoded.width,
        decoded_height: decoded.height,
        decoded_frame_count: decoded.frame_count,
        frame_rate_numerator: decoded.frame_rate.numerator,
        frame_rate_denominator: decoded.frame_rate.denominator,
        duration_numerator: decoded.duration.numerator,
        duration_denominator: decoded.duration.denominator,
        audio_policy: audio_policy(&manifest.audio),
        has_preview: manifest.preview.is_some(),
        file_size: metadata.len(),
        modified_ns: metadata_modified_ns(&metadata),
    })
}

const fn audio_policy(disposition: &AudioDisposition) -> &'static str {
    match disposition {
        AudioDisposition::SourceAbsent => "source_absent",
        AudioDisposition::PreservedSource => "preserved_source",
        AudioDisposition::CopiedFromCarrierExact { .. } => "copied_from_carrier_exact",
        AudioDisposition::OmittedTimingMismatch { .. } => "omitted_timing_mismatch",
    }
}

fn metadata_modified_ns(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_nanos()).ok())
        .unwrap_or_default()
}

fn has_lc_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lc"))
}

fn filesystem_error(_source: std::io::Error) -> LibraryError {
    LibraryError::new(ErrorCode::Filesystem, "local filesystem operation failed")
}
