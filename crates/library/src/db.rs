use std::path::Path;

use rusqlite::{Connection, OptionalExtension as _, params, types::Type};

use crate::{
    Availability, CartridgeKey, CartridgeMetadata, CartridgeRecord, CollectionId, ErrorCode,
    Library, LibraryError, PathRecord, PathState, QueryOptions, Result, migrations,
};

const MAX_QUERY_BYTES: usize = 256;
const MAX_QUERY_LIMIT: usize = 1_000;
const MAX_TAGS: usize = 64;
const MAX_TAG_BYTES: usize = 64;

impl Library {
    /// Opens or creates a local `SQLite` index and applies all supported
    /// migrations transactionally.
    ///
    /// # Errors
    ///
    /// Returns a path-free filesystem/database error or rejects a newer schema.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut connection = Connection::open(path).map_err(LibraryError::database)?;
        migrations::migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Creates an ephemeral migrated database, primarily for tests and
    /// temporary application sessions.
    ///
    /// # Errors
    ///
    /// Returns a stable database error when `SQLite` initialization fails.
    pub fn in_memory() -> Result<Self> {
        let mut connection = Connection::open_in_memory().map_err(LibraryError::database)?;
        migrations::migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Returns the on-disk schema version.
    ///
    /// # Errors
    ///
    /// Returns a stable database error if the pragma cannot be read.
    pub fn schema_version(&self) -> Result<u32> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(LibraryError::database)
    }

    /// Loads one immutable cartridge identity and its current local state.
    ///
    /// # Errors
    ///
    /// Returns a stable database error if the record cannot be read.
    pub fn get_cartridge(&self, key: &CartridgeKey) -> Result<Option<CartridgeRecord>> {
        validate_cartridge_key(key)?;
        load_record(&self.connection, key)
    }

    /// Queries a real or virtual collection in its deterministic order.
    /// Search filters preserve that collection order.
    ///
    /// # Errors
    ///
    /// Rejects unknown collections and bounded-input violations.
    pub fn query_collection(
        &self,
        collection_id: &CollectionId,
        options: &QueryOptions,
    ) -> Result<Vec<CartridgeRecord>> {
        let search = validate_query_options(options)?;
        if !collection_id.is_virtual() {
            ensure_collection(&self.connection, collection_id)?;
        }

        let all_sql = format!(
            "SELECT c.archive_sha256 FROM cartridges c WHERE {SEARCH_CLAUSE} \
             ORDER BY c.import_sequence, c.archive_sha256 LIMIT ?2"
        );
        let unassigned_sql = format!(
            "SELECT c.archive_sha256 FROM cartridges c WHERE \
             NOT EXISTS (SELECT 1 FROM collection_members cm \
             WHERE cm.archive_sha256 = c.archive_sha256) AND {SEARCH_CLAUSE} \
             ORDER BY c.import_sequence, c.archive_sha256 LIMIT ?2"
        );
        let collection_sql = format!(
            "SELECT c.archive_sha256 FROM collection_members cm \
             JOIN cartridges c ON c.archive_sha256 = cm.archive_sha256 \
             WHERE cm.collection_id = ?1 AND {SEARCH_CLAUSE_COLLECTION} \
             ORDER BY cm.position, c.archive_sha256 LIMIT ?3"
        );

        let keys = if collection_id.as_str() == crate::ALL_CARTRIDGES_ID {
            query_keys(
                &self.connection,
                &all_sql,
                params![search, usize_to_i64(options.limit)?],
            )?
        } else if collection_id.as_str() == crate::UNASSIGNED_ID {
            query_keys(
                &self.connection,
                &unassigned_sql,
                params![search, usize_to_i64(options.limit)?],
            )?
        } else {
            query_keys(
                &self.connection,
                &collection_sql,
                params![collection_id.as_str(), search, usize_to_i64(options.limit)?],
            )?
        };
        keys.into_iter()
            .map(|key| {
                load_record(&self.connection, &key)?.ok_or_else(|| {
                    LibraryError::new(ErrorCode::Database, "indexed cartridge record is missing")
                })
            })
            .collect()
    }

    /// Sets the favorite flag without changing collection order.
    ///
    /// # Errors
    ///
    /// Returns `not_found` for an unknown cartridge.
    pub fn set_favorite(&mut self, key: &CartridgeKey, favorite: bool) -> Result<()> {
        let changed = self
            .connection
            .execute(
                "UPDATE cartridges SET favorite = ?2 WHERE archive_sha256 = ?1",
                params![key.as_str(), i64::from(favorite)],
            )
            .map_err(LibraryError::database)?;
        if changed == 0 {
            return Err(not_found("cartridge is not indexed"));
        }
        Ok(())
    }

    /// Replaces a cartridge's tag set atomically. Tags are case-insensitively
    /// unique while preserving the submitted display spelling.
    ///
    /// # Errors
    ///
    /// Rejects unknown cartridges, duplicates, controls, and bounded inputs.
    pub fn set_tags(&mut self, key: &CartridgeKey, tags: &[String]) -> Result<()> {
        if tags.len() > MAX_TAGS {
            return Err(invalid("tag count exceeds the library limit"));
        }
        let mut validated = Vec::with_capacity(tags.len());
        let mut normalized_seen = std::collections::BTreeSet::new();
        for tag in tags {
            let display = tag.trim();
            if display.is_empty()
                || display.len() > MAX_TAG_BYTES
                || display.chars().any(char::is_control)
            {
                return Err(invalid("tag is empty, too long, or contains controls"));
            }
            let normalized = normalize(display);
            if !normalized_seen.insert(normalized.clone()) {
                return Err(invalid("tag set contains a case-insensitive duplicate"));
            }
            validated.push((display.to_owned(), normalized));
        }

        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_cartridge(&transaction, key)?;
        transaction
            .execute(
                "DELETE FROM cartridge_tags WHERE archive_sha256 = ?1",
                [key.as_str()],
            )
            .map_err(LibraryError::database)?;
        for (tag, normalized) in validated {
            transaction
                .execute(
                    "INSERT INTO cartridge_tags(archive_sha256, tag, normalized_tag) \
                     VALUES (?1, ?2, ?3)",
                    params![key.as_str(), tag, normalized],
                )
                .map_err(LibraryError::database)?;
        }
        transaction.commit().map_err(LibraryError::database)
    }

    /// Moves a cartridge to the front of the deterministic recent list.
    ///
    /// # Errors
    ///
    /// Returns `not_found` for an unknown cartridge.
    pub fn mark_recent(&mut self, key: &CartridgeKey) -> Result<()> {
        let now = now_ms();
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_cartridge(&transaction, key)?;
        let sequence = next_sequence(&transaction, "recent_cartridges", "recent_sequence")?;
        transaction
            .execute(
                "INSERT INTO recent_cartridges(archive_sha256, recent_sequence, opened_at_ms) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT(archive_sha256) DO UPDATE SET \
                 recent_sequence = excluded.recent_sequence, opened_at_ms = excluded.opened_at_ms",
                params![key.as_str(), sequence, now],
            )
            .map_err(LibraryError::database)?;
        transaction.commit().map_err(LibraryError::database)
    }

    /// Returns most-recently-used cartridges, newest first.
    ///
    /// # Errors
    ///
    /// Rejects limits above the UI/API ceiling.
    pub fn recent(&self, limit: usize) -> Result<Vec<CartridgeRecord>> {
        if limit > MAX_QUERY_LIMIT {
            return Err(invalid("query limit exceeds the library ceiling"));
        }
        let keys = query_keys(
            &self.connection,
            "SELECT archive_sha256 FROM recent_cartridges \
             ORDER BY recent_sequence DESC, archive_sha256 LIMIT ?1",
            [usize_to_i64(limit)?],
        )?;
        keys.into_iter()
            .map(|key| {
                load_record(&self.connection, &key)?.ok_or_else(|| {
                    LibraryError::new(ErrorCode::Database, "recent cartridge record is missing")
                })
            })
            .collect()
    }
}

const SEARCH_CLAUSE: &str = "(?1 = '' OR instr(lower(c.cartridge_id), ?1) > 0 \
    OR instr(c.codec_family, ?1) > 0 OR instr(c.codec_profile, ?1) > 0 \
    OR EXISTS (SELECT 1 FROM cartridge_tags ct WHERE ct.archive_sha256 = c.archive_sha256 \
        AND instr(ct.normalized_tag, ?1) > 0) \
    OR EXISTS (SELECT 1 FROM cartridge_paths cp WHERE cp.archive_sha256 = c.archive_sha256 \
        AND instr(cp.file_name_normalized, ?1) > 0))";

const SEARCH_CLAUSE_COLLECTION: &str = "(?2 = '' OR instr(lower(c.cartridge_id), ?2) > 0 \
    OR instr(c.codec_family, ?2) > 0 OR instr(c.codec_profile, ?2) > 0 \
    OR EXISTS (SELECT 1 FROM cartridge_tags ct WHERE ct.archive_sha256 = c.archive_sha256 \
        AND instr(ct.normalized_tag, ?2) > 0) \
    OR EXISTS (SELECT 1 FROM cartridge_paths cp WHERE cp.archive_sha256 = c.archive_sha256 \
        AND instr(cp.file_name_normalized, ?2) > 0))";

fn load_record(connection: &Connection, key: &CartridgeKey) -> Result<Option<CartridgeRecord>> {
    let metadata_row = connection
        .query_row(
            "SELECT cartridge_id, archive_sha256, archive_bytes, manifest_json, \
             codec_family, codec_profile, codec_profile_version, timing_contract, \
             timing_contract_version, decoded_width, decoded_height, decoded_frame_count, \
             frame_rate_numerator, frame_rate_denominator, duration_numerator, \
             duration_denominator, audio_policy, has_preview, favorite, import_sequence \
             FROM cartridges WHERE archive_sha256 = ?1",
            [key.as_str()],
            |row| {
                Ok((
                    CartridgeMetadata {
                        cartridge_id: row.get(0)?,
                        archive_sha256: row.get(1)?,
                        archive_bytes: nonnegative_u64(row.get(2)?, 2)?,
                        manifest_json: row.get(3)?,
                        codec_family: row.get(4)?,
                        codec_profile: row.get(5)?,
                        codec_profile_version: row.get(6)?,
                        timing_contract: row.get(7)?,
                        timing_contract_version: row.get(8)?,
                        decoded_width: nonnegative_u32(row.get(9)?, 9)?,
                        decoded_height: nonnegative_u32(row.get(10)?, 10)?,
                        decoded_frame_count: nonnegative_u64(row.get(11)?, 11)?,
                        frame_rate_numerator: nonnegative_u64(row.get(12)?, 12)?,
                        frame_rate_denominator: nonnegative_u64(row.get(13)?, 13)?,
                        duration_numerator: nonnegative_u64(row.get(14)?, 14)?,
                        duration_denominator: nonnegative_u64(row.get(15)?, 15)?,
                        audio_policy: row.get(16)?,
                        has_preview: row.get::<_, i64>(17)? != 0,
                    },
                    row.get::<_, i64>(18)? != 0,
                    nonnegative_u64(row.get(19)?, 19)?,
                ))
            },
        )
        .optional()
        .map_err(LibraryError::database)?;
    let Some((metadata, favorite, import_sequence)) = metadata_row else {
        return Ok(None);
    };

    let mut tag_statement = connection
        .prepare(
            "SELECT tag FROM cartridge_tags WHERE archive_sha256 = ?1 \
             ORDER BY normalized_tag, tag",
        )
        .map_err(LibraryError::database)?;
    let tags = tag_statement
        .query_map([key.as_str()], |row| row.get::<_, String>(0))
        .map_err(LibraryError::database)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::database)?;

    let mut path_statement = connection
        .prepare(
            "SELECT path_id, path_text, state, warning_code, observed_archive_sha256, \
             last_checked_ms FROM cartridge_paths WHERE archive_sha256 = ?1 \
             ORDER BY path_id",
        )
        .map_err(LibraryError::database)?;
    let paths = path_statement
        .query_map([key.as_str()], |row| {
            let state_text: String = row.get(2)?;
            let state = PathState::parse(&state_text).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid path state",
                    )),
                )
            })?;
            Ok(PathRecord {
                path_id: row.get(0)?,
                path: std::path::PathBuf::from(row.get::<_, String>(1)?),
                state,
                warning_code: row.get(3)?,
                observed_archive_sha256: row.get(4)?,
                last_checked_ms: row.get(5)?,
            })
        })
        .map_err(LibraryError::database)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::database)?;
    let availability = availability(&paths);

    Ok(Some(CartridgeRecord {
        key: key.clone(),
        metadata,
        favorite,
        tags,
        paths,
        availability,
        import_sequence,
    }))
}

fn availability(paths: &[PathRecord]) -> Availability {
    if paths.iter().any(|path| path.state == PathState::Present) {
        Availability::Present
    } else if paths
        .iter()
        .any(|path| matches!(path.state, PathState::Invalid | PathState::ContentChanged))
    {
        Availability::Warning
    } else {
        Availability::Missing
    }
}

fn validate_query_options(options: &QueryOptions) -> Result<String> {
    if options.limit > MAX_QUERY_LIMIT {
        return Err(invalid("query limit exceeds the library ceiling"));
    }
    let search = options.search.as_deref().unwrap_or_default().trim();
    if search.len() > MAX_QUERY_BYTES || search.chars().any(char::is_control) {
        return Err(invalid("search query is too long or contains controls"));
    }
    Ok(normalize(search))
}

fn query_keys<P>(connection: &Connection, sql: &str, parameters: P) -> Result<Vec<CartridgeKey>>
where
    P: rusqlite::Params,
{
    let mut statement = connection.prepare(sql).map_err(LibraryError::database)?;
    statement
        .query_map(parameters, |row| {
            row.get::<_, String>(0).map(CartridgeKey::new_unchecked)
        })
        .map_err(LibraryError::database)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::database)
}

pub(crate) fn ensure_cartridge(connection: &Connection, key: &CartridgeKey) -> Result<()> {
    validate_cartridge_key(key)?;
    let exists = connection
        .query_row(
            "SELECT 1 FROM cartridges WHERE archive_sha256 = ?1",
            [key.as_str()],
            |_| Ok(()),
        )
        .optional()
        .map_err(LibraryError::database)?
        .is_some();
    if !exists {
        return Err(not_found("cartridge is not indexed"));
    }
    Ok(())
}

pub(crate) fn ensure_collection(connection: &Connection, id: &CollectionId) -> Result<()> {
    validate_collection_id(id)?;
    let exists = connection
        .query_row(
            "SELECT 1 FROM collections WHERE collection_id = ?1",
            [id.as_str()],
            |_| Ok(()),
        )
        .optional()
        .map_err(LibraryError::database)?
        .is_some();
    if !exists {
        return Err(not_found("collection does not exist"));
    }
    Ok(())
}

pub(crate) fn next_sequence(connection: &Connection, table: &str, column: &str) -> Result<i64> {
    let sql = format!("SELECT COALESCE(MAX({column}), -1) + 1 FROM {table}");
    connection
        .query_row(&sql, [], |row| row.get(0))
        .map_err(LibraryError::database)
}

pub(crate) fn normalize(value: &str) -> String {
    value.to_lowercase()
}

pub(crate) fn validate_cartridge_key(key: &CartridgeKey) -> Result<()> {
    let value = key.as_str();
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("cartridge identity is not canonical SHA-256"));
    }
    Ok(())
}

pub(crate) fn validate_collection_id(id: &CollectionId) -> Result<()> {
    let value = id.as_str();
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| invalid("collection identity is not a canonical UUID"))?;
    if parsed.to_string() != value {
        return Err(invalid("collection identity is not a canonical UUID"));
    }
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

pub(crate) fn invalid(detail: &'static str) -> LibraryError {
    LibraryError::new(ErrorCode::InvalidInput, detail)
}

pub(crate) fn not_found(detail: &'static str) -> LibraryError {
    LibraryError::new(ErrorCode::NotFound, detail)
}

pub(crate) fn usize_to_i64(value: usize) -> Result<i64> {
    i64::try_from(value).map_err(|_| invalid("numeric input exceeds the library ceiling"))
}

pub(crate) fn u64_to_i64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| invalid("numeric metadata exceeds SQLite range"))
}

fn nonnegative_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Integer, Box::new(error))
    })
}

fn nonnegative_u32(value: i64, column: usize) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Integer, Box::new(error))
    })
}
