mod support;

use latentdeck_library::{CartridgeKey, CollectionId, ErrorCode, Library, QueryOptions};
use tempfile::tempdir;

use support::{ID_A, ID_B, ID_C, write_synthetic_lc};

#[test]
fn favorites_tags_recent_and_search_have_deterministic_results() {
    let temp = tempdir().expect("tempdir");
    let mut library = Library::in_memory().expect("library");
    let alpha_path = temp.path().join("Alpha Signal.lc");
    let beta_path = temp.path().join("Beta Signal.lc");
    let gamma_path = temp.path().join("Gamma.lc");
    write_synthetic_lc(&alpha_path, ID_A);
    write_synthetic_lc(&beta_path, ID_B);
    write_synthetic_lc(&gamma_path, ID_C);
    let alpha = library.import_file(alpha_path).expect("alpha").key;
    let beta = library.import_file(beta_path).expect("beta").key;
    let gamma = library.import_file(gamma_path).expect("gamma").key;

    library.set_favorite(&beta, true).expect("favorite beta");
    library
        .set_tags(&alpha, &["Ambient".to_owned(), "Warm".to_owned()])
        .expect("alpha tags");
    library
        .set_tags(&beta, &["ambient".to_owned(), "Cold".to_owned()])
        .expect("beta tags");

    let ambient = library
        .query_collection(
            &CollectionId::all_cartridges(),
            &QueryOptions {
                search: Some("AMBIENT".to_owned()),
                limit: 10,
            },
        )
        .expect("ambient search");
    assert_eq!(
        ambient
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>(),
        vec![alpha.clone(), beta.clone()]
    );
    assert!(!ambient[0].favorite);
    assert!(ambient[1].favorite);

    let by_file_name = library
        .query_collection(
            &CollectionId::all_cartridges(),
            &QueryOptions {
                search: Some("beta signal".to_owned()),
                limit: 10,
            },
        )
        .expect("filename search");
    assert_eq!(by_file_name.len(), 1);
    assert_eq!(by_file_name[0].key, beta);

    library.mark_recent(&alpha).expect("recent alpha");
    library.mark_recent(&gamma).expect("recent gamma");
    library.mark_recent(&alpha).expect("alpha again");
    let recent = library.recent(10).expect("recent list");
    assert_eq!(recent[0].key, alpha);
    assert_eq!(recent[1].key, gamma);
}

#[test]
fn bounded_names_tags_search_and_limits_are_rejected() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("a.lc");
    write_synthetic_lc(&path, ID_A);
    let mut library = Library::in_memory().expect("library");
    let key = library.import_file(path).expect("import").key;

    let long_name = "x".repeat(129);
    assert_eq!(
        library
            .create_collection(&long_name)
            .expect_err("long collection")
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        library
            .get_cartridge(&CartridgeKey::new_unchecked("not-a-hash"))
            .expect_err("malformed key")
            .code,
        ErrorCode::InvalidInput
    );
    let too_many_tags = (0..65)
        .map(|index| format!("tag-{index}"))
        .collect::<Vec<_>>();
    assert_eq!(
        library
            .set_tags(&key, &too_many_tags)
            .expect_err("too many tags")
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        library
            .query_collection(
                &CollectionId::all_cartridges(),
                &QueryOptions {
                    search: Some("x".repeat(257)),
                    limit: 10,
                }
            )
            .expect_err("long search")
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(
        library.recent(1_001).expect_err("large recent limit").code,
        ErrorCode::InvalidInput
    );
}

#[test]
fn future_database_schema_is_refused_without_mutation() {
    let temp = tempdir().expect("tempdir");
    let db = temp.path().join("future.sqlite3");
    let connection = rusqlite::Connection::open(&db).expect("open raw db");
    connection
        .pragma_update(None, "user_version", 999_u32)
        .expect("set future schema");
    drop(connection);
    let error = Library::open(&db).expect_err("future schema rejected");
    assert_eq!(error.code, ErrorCode::UnsupportedSchema);
    let connection = rusqlite::Connection::open(db).expect("reopen raw db");
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read version");
    assert_eq!(version, 999);
}

#[test]
fn failed_initial_migration_rolls_back_schema_version() {
    let temp = tempdir().expect("tempdir");
    let db = temp.path().join("conflict.sqlite3");
    let connection = rusqlite::Connection::open(&db).expect("open raw db");
    connection
        .execute("CREATE TABLE cartridges(dummy INTEGER)", [])
        .expect("create conflicting table");
    drop(connection);

    let error = Library::open(&db).expect_err("migration must fail");
    assert_eq!(error.code, ErrorCode::Database);
    let connection = rusqlite::Connection::open(db).expect("reopen raw db");
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read version");
    assert_eq!(version, 0);
    let paths_table: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
             AND name = 'cartridge_paths'",
            [],
            |row| row.get(0),
        )
        .expect("inspect rollback");
    assert_eq!(paths_table, 0);
}
