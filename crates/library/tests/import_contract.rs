mod support;

use std::fs;

use latentdeck_library::{
    CollectionId, FolderImportOptions, ImportDisposition, Library, PathState, QueryOptions,
    ReindexDisposition, SCHEMA_VERSION,
};
use tempfile::tempdir;

use support::{
    ID_A, ID_B, ID_C, write_synthetic_lc, write_synthetic_non_h3_lc,
    write_synthetic_non_h3_lc_with_duration,
};

#[test]
fn full_validation_import_is_persistent_and_deduplicates_archive_identity() {
    let temp = tempdir().expect("tempdir");
    let db = temp.path().join("library.sqlite3");
    let first = temp.path().join("first.lc");
    let second = temp.path().join("second.lc");
    let bytes = write_synthetic_lc(&first, ID_A);
    fs::write(&second, bytes).expect("copy cartridge");

    let mut library = Library::open(&db).expect("open library");
    assert_eq!(library.schema_version().expect("schema"), SCHEMA_VERSION);
    let imported = library.import_file(&first).expect("import first");
    assert_eq!(imported.disposition, ImportDisposition::Added);
    let duplicate = library.import_file(&second).expect("import duplicate");
    assert_eq!(duplicate.key, imported.key);
    assert_eq!(duplicate.disposition, ImportDisposition::Added);
    let again = library.import_file(&first).expect("repeat import");
    assert_eq!(again.disposition, ImportDisposition::AlreadyIndexed);
    drop(library);

    let library = Library::open(&db).expect("reopen library");
    let all = library
        .query_collection(&CollectionId::all_cartridges(), &QueryOptions::default())
        .expect("all cartridges");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].metadata.cartridge_id, ID_A);
    assert_eq!(all[0].paths.len(), 2);
    assert!(
        all[0]
            .paths
            .iter()
            .all(|path| path.state == PathState::Present)
    );
    assert_eq!(all[0].metadata.decoded_frame_count, 5);
}

#[test]
fn explicit_folder_import_is_bounded_and_recursion_is_opt_in() {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().join("selected");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).expect("create folder");
    write_synthetic_lc(&root.join("a.lc"), ID_A);
    write_synthetic_lc(&nested.join("b.LC"), ID_B);
    fs::write(root.join("notes.txt"), b"not a cartridge").expect("write ignored file");

    let mut library = Library::in_memory().expect("library");
    let shallow = library
        .import_folder(&root, &FolderImportOptions::default())
        .expect("shallow import");
    assert_eq!(shallow.accepted.len(), 1);
    assert_eq!(shallow.ignored_non_cartridges, 1);

    let recursive = library
        .import_folder(
            &root,
            &FolderImportOptions {
                recursive: true,
                max_candidates: 16,
            },
        )
        .expect("recursive import");
    assert_eq!(recursive.accepted.len(), 2);

    let error = library
        .import_folder(
            &root,
            &FolderImportOptions {
                recursive: true,
                max_candidates: 1,
            },
        )
        .expect_err("candidate ceiling");
    assert_eq!(error.code.as_str(), "import_limit");
}

#[test]
fn reindex_retains_missing_identity_and_never_accepts_changed_content() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("stable.lc");
    let original = write_synthetic_lc(&path, ID_A);
    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&path).expect("import");

    fs::remove_file(&path).expect("remove test file");
    let missing = library.reindex_registered().expect("missing reindex");
    assert_eq!(missing[0].disposition, ReindexDisposition::Missing);
    let record = library
        .get_cartridge(&imported.key)
        .expect("query missing")
        .expect("record retained");
    assert_eq!(record.paths[0].state, PathState::Missing);

    fs::write(&path, &original).expect("restore same archive");
    let recovered = library.reindex_registered().expect("recover reindex");
    assert_eq!(recovered[0].disposition, ReindexDisposition::Present);

    let replacement = write_synthetic_non_h3_lc(&path, ID_C);
    assert_ne!(
        replacement.len(),
        original.len(),
        "replacement fixture must change the cached file size"
    );
    let changed = library.reindex_registered().expect("changed reindex");
    assert_eq!(changed[0].disposition, ReindexDisposition::ContentChanged);
    assert_ne!(changed[0].observed_key.as_ref(), Some(&imported.key));
    let repeated = library
        .reindex_registered()
        .expect("incremental changed reindex");
    assert_eq!(repeated[0].disposition, ReindexDisposition::ContentChanged);
    assert_eq!(repeated[0].observed_key, changed[0].observed_key);
    let retained = library
        .get_cartridge(&imported.key)
        .expect("query retained")
        .expect("old identity");
    assert_eq!(retained.paths[0].state, PathState::ContentChanged);

    let accepted = library
        .import_file(&path)
        .expect("explicit replacement import");
    assert_eq!(accepted.disposition, ImportDisposition::AcceptedReplacement);
    assert_eq!(accepted.previous_key.as_ref(), Some(&imported.key));
    assert_ne!(accepted.key, imported.key);
}

#[test]
fn accepted_replacement_does_not_transfer_old_collection_membership() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("slot.lc");
    write_synthetic_lc(&path, ID_A);
    let mut library = Library::in_memory().expect("library");
    let original = library.import_file(&path).expect("original import").key;
    let collection = library.create_collection("Preserved").expect("collection");
    library
        .add_to_collection(&collection.id, &original)
        .expect("membership");

    write_synthetic_lc(&path, ID_B);
    let replacement = library.import_file(&path).expect("accept replacement").key;
    let members = library
        .query_collection(&collection.id, &QueryOptions::default())
        .expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].key, original);
    assert_eq!(
        members[0].availability,
        latentdeck_library::Availability::Missing
    );
    let unassigned = library
        .query_collection(&CollectionId::unassigned(), &QueryOptions::default())
        .expect("unassigned");
    assert_eq!(unassigned.len(), 1);
    assert_eq!(unassigned[0].key, replacement);
}

#[test]
fn invalid_import_errors_do_not_echo_machine_paths() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("private-name.lc");
    fs::write(&path, b"broken").expect("broken file");
    let mut library = Library::in_memory().expect("library");
    let error = library.import_file(&path).expect_err("must reject");
    let rendered = error.to_string();
    assert!(!rendered.contains("private-name"));
    assert!(!rendered.contains(&temp.path().to_string_lossy().to_string()));
    assert!(error.cartridge_code.is_some());
}

#[test]
fn malformed_codec_neutral_geometry_never_enters_the_index() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("contradictory-timing.lc");
    write_synthetic_non_h3_lc_with_duration(&path, ID_C, 2, 1);
    let mut library = Library::in_memory().expect("library");

    let error = library
        .import_file(&path)
        .expect_err("contradictory generic timing must be rejected before insert");

    assert_eq!(error.cartridge_code.as_deref(), Some("timing_mismatch"));
    let all = library
        .query_collection(&CollectionId::all_cartridges(), &QueryOptions::default())
        .expect("empty index remains queryable");
    assert!(all.is_empty());
}
