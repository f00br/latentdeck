use rusqlite::{Connection, TransactionBehavior};

use crate::error::{ErrorCode, LibraryError, Result};

pub const SCHEMA_VERSION: u32 = 1;

const MIGRATION_1: &str = r"
CREATE TABLE cartridges (
    archive_sha256 TEXT PRIMARY KEY NOT NULL CHECK(length(archive_sha256) = 64),
    cartridge_id TEXT NOT NULL,
    archive_bytes INTEGER NOT NULL CHECK(archive_bytes >= 0),
    manifest_json TEXT NOT NULL,
    codec_family TEXT NOT NULL,
    codec_profile TEXT NOT NULL,
    codec_profile_version TEXT NOT NULL,
    timing_contract TEXT NOT NULL,
    timing_contract_version TEXT NOT NULL,
    decoded_width INTEGER NOT NULL CHECK(decoded_width > 0),
    decoded_height INTEGER NOT NULL CHECK(decoded_height > 0),
    decoded_frame_count INTEGER NOT NULL CHECK(decoded_frame_count > 0),
    frame_rate_numerator INTEGER NOT NULL CHECK(frame_rate_numerator > 0),
    frame_rate_denominator INTEGER NOT NULL CHECK(frame_rate_denominator > 0),
    duration_numerator INTEGER NOT NULL CHECK(duration_numerator > 0),
    duration_denominator INTEGER NOT NULL CHECK(duration_denominator > 0),
    audio_policy TEXT NOT NULL,
    has_preview INTEGER NOT NULL CHECK(has_preview IN (0, 1)),
    favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0, 1)),
    import_sequence INTEGER NOT NULL UNIQUE CHECK(import_sequence >= 0),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0)
) STRICT;

CREATE TABLE cartridge_paths (
    path_id INTEGER PRIMARY KEY,
    path_text TEXT NOT NULL UNIQUE,
    file_name_normalized TEXT NOT NULL,
    archive_sha256 TEXT NOT NULL REFERENCES cartridges(archive_sha256) ON DELETE RESTRICT,
    file_size INTEGER NOT NULL CHECK(file_size >= 0),
    modified_ns INTEGER NOT NULL CHECK(modified_ns >= 0),
    state TEXT NOT NULL CHECK(state IN ('present', 'missing', 'invalid', 'content_changed')),
    warning_code TEXT,
    observed_archive_sha256 TEXT,
    last_checked_ms INTEGER NOT NULL CHECK(last_checked_ms >= 0)
) STRICT;

CREATE INDEX cartridge_paths_cartridge_idx
    ON cartridge_paths(archive_sha256, path_id);

CREATE TABLE collections (
    collection_id TEXT PRIMARY KEY NOT NULL
        CHECK(collection_id NOT IN ('latentdeck.virtual.all', 'latentdeck.virtual.unassigned')),
    name TEXT NOT NULL,
    normalized_name TEXT NOT NULL UNIQUE,
    position INTEGER NOT NULL UNIQUE CHECK(position >= 0),
    created_at_ms INTEGER NOT NULL CHECK(created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK(updated_at_ms >= 0)
) STRICT;

CREATE TABLE collection_members (
    collection_id TEXT NOT NULL REFERENCES collections(collection_id) ON DELETE CASCADE,
    archive_sha256 TEXT NOT NULL REFERENCES cartridges(archive_sha256) ON DELETE RESTRICT,
    position INTEGER NOT NULL CHECK(position >= 0),
    PRIMARY KEY(collection_id, archive_sha256),
    UNIQUE(collection_id, position)
) STRICT;

CREATE INDEX collection_members_cartridge_idx
    ON collection_members(archive_sha256, collection_id);

CREATE TABLE cartridge_tags (
    archive_sha256 TEXT NOT NULL REFERENCES cartridges(archive_sha256) ON DELETE CASCADE,
    tag TEXT NOT NULL,
    normalized_tag TEXT NOT NULL,
    PRIMARY KEY(archive_sha256, normalized_tag)
) STRICT;

CREATE INDEX cartridge_tags_search_idx ON cartridge_tags(normalized_tag);

CREATE TABLE recent_cartridges (
    archive_sha256 TEXT PRIMARY KEY NOT NULL
        REFERENCES cartridges(archive_sha256) ON DELETE CASCADE,
    recent_sequence INTEGER NOT NULL UNIQUE CHECK(recent_sequence >= 0),
    opened_at_ms INTEGER NOT NULL CHECK(opened_at_ms >= 0)
) STRICT;
";

pub fn migrate(connection: &mut Connection) -> Result<()> {
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(LibraryError::database)?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(LibraryError::database)?;

    let current = connection
        .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
        .map_err(LibraryError::database)?;
    if current > SCHEMA_VERSION {
        return Err(LibraryError::new(
            ErrorCode::UnsupportedSchema,
            "library database was created by a newer schema",
        ));
    }
    if current == 0 {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(LibraryError::database)?;
        transaction
            .execute_batch(MIGRATION_1)
            .map_err(LibraryError::database)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .map_err(LibraryError::database)?;
        transaction.commit().map_err(LibraryError::database)?;
    }

    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(LibraryError::database)?;
    if violation.is_some() {
        return Err(LibraryError::new(
            ErrorCode::Database,
            "library database foreign-key check failed",
        ));
    }
    Ok(())
}

use rusqlite::OptionalExtension as _;
