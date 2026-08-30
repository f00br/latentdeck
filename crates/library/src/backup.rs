use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use rusqlite::{Connection, MAIN_DB, OptionalExtension as _};

use crate::{ErrorCode, Library, LibraryError, Result};

const MAX_MIGRATION_BACKUP_CANDIDATES: u32 = 100;

impl Library {
    /// Writes a consistent `SQLite` snapshot without replacing an existing file.
    ///
    /// The snapshot is first written and validated at a sibling `.partial`
    /// path, then atomically renamed into place. This API never overwrites an
    /// existing backup or accumulates an incomplete destination in RAM.
    ///
    /// # Errors
    ///
    /// Returns `conflict` when the destination or its partial sibling already
    /// exists, and a path-free filesystem/database error for other failures.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<()> {
        backup_connection_atomic(
            &self.connection,
            destination.as_ref(),
            Some(crate::migrations::SCHEMA_VERSION),
        )
    }
}

pub(crate) fn backup_before_migration(
    connection: &Connection,
    database_path: &Path,
    current_version: u32,
) -> Result<()> {
    let destination = migration_backup_destination(database_path, current_version)?;
    backup_connection_atomic(connection, &destination, Some(current_version))
}

fn migration_backup_destination(database_path: &Path, version: u32) -> Result<PathBuf> {
    let file_name = database_path.file_name().ok_or_else(|| {
        LibraryError::new(
            ErrorCode::Filesystem,
            "library database backup destination is unavailable",
        )
    })?;
    let parent = database_path.parent().unwrap_or_else(|| Path::new("."));

    for attempt in 1..=MAX_MIGRATION_BACKUP_CANDIDATES {
        let mut candidate_name = OsString::from(file_name);
        candidate_name.push(format!(".pre-migration-v{version}-{attempt}.sqlite3"));
        let candidate = parent.join(candidate_name);
        if !candidate.exists() && !partial_path(&candidate).exists() {
            return Ok(candidate);
        }
    }

    Err(LibraryError::new(
        ErrorCode::Conflict,
        "library migration backup destination limit was reached",
    ))
}

fn backup_connection_atomic(
    connection: &Connection,
    destination: &Path,
    expected_version: Option<u32>,
) -> Result<()> {
    let partial = partial_path(destination);
    if destination.exists() || partial.exists() {
        return Err(LibraryError::new(
            ErrorCode::Conflict,
            "library backup destination already exists",
        ));
    }
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| {
        LibraryError::new(
            ErrorCode::Filesystem,
            "library backup directory could not be created",
        )
    })?;

    let result = (|| {
        connection
            .backup(MAIN_DB, &partial, None)
            .map_err(LibraryError::database)?;
        validate_backup(&partial, expected_version)?;
        fs::rename(&partial, destination).map_err(|_| {
            LibraryError::new(
                ErrorCode::Filesystem,
                "library backup could not be finalized",
            )
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

fn validate_backup(path: &Path, expected_version: Option<u32>) -> Result<()> {
    let connection = Connection::open(path).map_err(LibraryError::database)?;
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(LibraryError::database)?;
    if integrity != "ok" {
        return Err(LibraryError::new(
            ErrorCode::Database,
            "library backup integrity check failed",
        ));
    }
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(LibraryError::database)?;
    if foreign_key_violation.is_some() {
        return Err(LibraryError::new(
            ErrorCode::Database,
            "library backup foreign-key check failed",
        ));
    }
    if let Some(expected) = expected_version {
        let actual = crate::migrations::current_version(&connection)?;
        if actual != expected {
            return Err(LibraryError::new(
                ErrorCode::Database,
                "library backup schema version changed",
            ));
        }
    }
    Ok(())
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".partial");
    PathBuf::from(name)
}
