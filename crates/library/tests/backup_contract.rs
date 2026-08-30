use std::fs;

use latentdeck_library::{ErrorCode, Library};
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn explicit_backup_is_atomic_valid_and_never_overwrites() {
    let temp = tempdir().expect("tempdir");
    let source = temp.path().join("library.sqlite3");
    let destination = temp.path().join("backups").join("library.sqlite3.bak");
    let mut library = Library::open(&source).expect("library");
    let collection = library.create_collection("Live set").expect("collection");

    library.backup_to(&destination).expect("backup");

    assert!(destination.is_file());
    assert!(!destination.with_extension("bak.partial").exists());
    let backup = Library::open(&destination).expect("open backup");
    assert_eq!(backup.schema_version().expect("schema"), 1);
    assert!(
        backup
            .list_collections()
            .expect("collections")
            .into_iter()
            .any(|entry| entry.id == collection.id)
    );

    let error = library
        .backup_to(&destination)
        .expect_err("existing destination must be preserved");
    assert_eq!(error.code, ErrorCode::Conflict);
}

#[test]
fn opening_an_existing_older_database_keeps_a_pre_migration_backup() {
    let temp = tempdir().expect("tempdir");
    let source = temp.path().join("legacy.sqlite3");
    let connection = Connection::open(&source).expect("legacy db");
    connection
        .execute_batch(
            "CREATE TABLE legacy_marker(value TEXT NOT NULL);\n\
             INSERT INTO legacy_marker(value) VALUES ('before-migration');",
        )
        .expect("legacy contents");
    drop(connection);

    let library = Library::open(&source).expect("migrated library");
    assert_eq!(library.schema_version().expect("schema"), 1);

    let backups = fs::read_dir(temp.path())
        .expect("read tempdir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("legacy.sqlite3.pre-migration-v0-"))
        })
        .collect::<Vec<_>>();
    assert_eq!(backups.len(), 1);

    let backup = Connection::open(&backups[0]).expect("open migration backup");
    let marker: String = backup
        .query_row("SELECT value FROM legacy_marker", [], |row| row.get(0))
        .expect("legacy marker");
    let version: u32 = backup
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("backup version");
    assert_eq!(marker, "before-migration");
    assert_eq!(version, 0);
}

#[test]
fn fresh_database_does_not_create_a_migration_backup() {
    let temp = tempdir().expect("tempdir");
    let source = temp.path().join("fresh.sqlite3");

    Library::open(&source).expect("fresh library");

    let names = fs::read_dir(temp.path())
        .expect("read tempdir")
        .map(|entry| entry.expect("entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["fresh.sqlite3"]);
}
