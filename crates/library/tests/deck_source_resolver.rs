mod support;

use latentdeck_library::{
    CartridgeKey, DeckSourceIdentity, ErrorCode, Library, MAX_DECK_SOURCE_PATH_CANDIDATES,
};
use tempfile::tempdir;

use support::{ID_A, ID_B, ID_C, write_synthetic_lc, write_synthetic_non_h3_lc};

#[test]
fn resolves_a_present_registered_source_by_immutable_identity() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("source.lc");
    write_synthetic_lc(&path, ID_A);

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&path).expect("import");
    let identity = DeckSourceIdentity::new(ID_A, imported.key.clone()).expect("identity");

    let resolved = library
        .resolve_deck_source(&identity)
        .expect("registered source");

    assert_eq!(resolved.identity(), &identity);
    assert_eq!(
        resolved.path(),
        path.canonicalize().expect("canonical path")
    );
}

#[test]
fn imports_and_resolves_a_codec_neutral_non_h3_source() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("synthetic-non-h3.lc");
    write_synthetic_non_h3_lc(&path, ID_C);

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&path).expect("generic import");
    let identity = DeckSourceIdentity::new(ID_C, imported.key).expect("identity");
    let resolved = library
        .resolve_deck_source(&identity)
        .expect("generic registered source");

    assert_eq!(resolved.identity(), &identity);
    assert_eq!(
        resolved.path(),
        path.canonicalize().expect("canonical path")
    );
}

#[test]
fn rejects_wrong_cartridge_id_and_archive_hash_without_exposing_a_path() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("private-deck-source.lc");
    write_synthetic_lc(&path, ID_A);

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&path).expect("import");

    let wrong_id =
        DeckSourceIdentity::new("550e8400-e29b-41d4-a716-446655440099", imported.key.clone())
            .expect("canonical identity");
    let id_error = library
        .resolve_deck_source(&wrong_id)
        .expect_err("wrong cartridge id");
    assert_eq!(id_error.code, ErrorCode::Conflict);

    let wrong_hash = DeckSourceIdentity::new(ID_A, CartridgeKey::new_unchecked("0".repeat(64)))
        .expect("canonical identity");
    let hash_error = library
        .resolve_deck_source(&wrong_hash)
        .expect_err("wrong archive hash");
    assert_eq!(hash_error.code, ErrorCode::NotFound);

    for error in [id_error, hash_error] {
        let rendered = error.to_string();
        assert!(!rendered.contains("private-deck-source"));
        assert!(!rendered.contains(&temp.path().to_string_lossy().to_string()));
    }
}

#[test]
fn rejects_missing_and_changed_registered_present_paths() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("registered-private-source.lc");
    let original = write_synthetic_lc(&path, ID_A);

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&path).expect("import");
    let identity = DeckSourceIdentity::new(ID_A, imported.key).expect("identity");

    std::fs::remove_file(&path).expect("remove registered source");
    let missing = library
        .resolve_deck_source(&identity)
        .expect_err("stale present path must fail closed");
    assert_eq!(missing.code, ErrorCode::NotFound);

    std::fs::write(&path, original).expect("restore source");
    write_synthetic_lc(&path, ID_B);
    let changed = library
        .resolve_deck_source(&identity)
        .expect_err("changed registered source must fail closed");
    assert_eq!(changed.code, ErrorCode::Conflict);

    for error in [missing, changed] {
        let rendered = error.to_string();
        assert!(!rendered.contains("registered-private-source"));
        assert!(!rendered.contains(&temp.path().to_string_lossy().to_string()));
    }
}

#[test]
fn ignores_paths_already_marked_non_present_by_the_index() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("marked-missing.lc");
    write_synthetic_lc(&path, ID_A);

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&path).expect("import");
    let identity = DeckSourceIdentity::new(ID_A, imported.key).expect("identity");
    std::fs::remove_file(path).expect("remove source");
    library.reindex_registered().expect("mark missing");

    let error = library
        .resolve_deck_source(&identity)
        .expect_err("non-present path must not be selected");
    assert_eq!(error.code, ErrorCode::NotFound);
}

#[test]
fn never_scans_for_an_unregistered_matching_file() {
    let temp = tempdir().expect("tempdir");
    let registered = temp.path().join("registered.lc");
    let unregistered = temp.path().join("matching-but-unregistered.lc");
    let bytes = write_synthetic_lc(&registered, ID_A);

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&registered).expect("import");
    let identity = DeckSourceIdentity::new(ID_A, imported.key).expect("identity");
    std::fs::write(&unregistered, bytes).expect("unregistered matching source");
    std::fs::remove_file(registered).expect("remove registered source");

    let error = library
        .resolve_deck_source(&identity)
        .expect_err("resolver must not discover nearby files");
    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(
        unregistered.is_file(),
        "matching file exists but is not indexed"
    );
}

#[test]
fn chooses_registered_present_paths_by_path_id_with_bounded_fallback() {
    let temp = tempdir().expect("tempdir");
    let first = temp.path().join("first.lc");
    let second = temp.path().join("second.lc");
    let bytes = write_synthetic_lc(&first, ID_A);
    std::fs::write(&second, bytes).expect("duplicate registered source");

    let mut library = Library::in_memory().expect("library");
    let imported = library.import_file(&first).expect("import first");
    library.import_file(&second).expect("import second");
    let identity = DeckSourceIdentity::new(ID_A, imported.key).expect("identity");

    let selected_first = library
        .resolve_deck_source(&identity)
        .expect("lowest path id");
    assert_eq!(
        selected_first.path(),
        first.canonicalize().expect("canonical first")
    );

    std::fs::remove_file(first).expect("make first registered path stale");
    let selected_second = library
        .resolve_deck_source(&identity)
        .expect("next registered present candidate");
    assert_eq!(
        selected_second.path(),
        second.canonicalize().expect("canonical second")
    );
}

#[test]
fn candidate_fallback_never_reads_beyond_the_documented_ceiling() {
    let temp = tempdir().expect("tempdir");
    let source = temp.path().join("source-00.lc");
    let bytes = write_synthetic_lc(&source, ID_A);
    let mut paths = vec![source];
    for index in 1..=MAX_DECK_SOURCE_PATH_CANDIDATES {
        let path = temp.path().join(format!("source-{index:02}.lc"));
        std::fs::write(&path, &bytes).expect("duplicate source");
        paths.push(path);
    }

    let mut library = Library::in_memory().expect("library");
    let first = library.import_file(&paths[0]).expect("first import");
    for path in &paths[1..] {
        library.import_file(path).expect("duplicate import");
    }
    let identity = DeckSourceIdentity::new(ID_A, first.key).expect("identity");
    for path in &paths[..MAX_DECK_SOURCE_PATH_CANDIDATES] {
        std::fs::remove_file(path).expect("stale bounded candidate");
    }

    let error = library
        .resolve_deck_source(&identity)
        .expect_err("candidate beyond ceiling must not be inspected");
    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(
        paths[MAX_DECK_SOURCE_PATH_CANDIDATES].is_file(),
        "the next registered candidate is valid but beyond the fixed ceiling"
    );
}

#[test]
fn source_identity_requires_the_same_canonical_forms_as_lc() {
    let hash = CartridgeKey::new_unchecked("a".repeat(64));
    let nil_error = DeckSourceIdentity::new("00000000-0000-0000-0000-000000000000", hash.clone())
        .expect_err("nil cartridge id");
    assert_eq!(nil_error.code, ErrorCode::InvalidInput);

    let uppercase_error = DeckSourceIdentity::new("550E8400-E29B-41D4-A716-446655440000", hash)
        .expect_err("non-canonical cartridge id");
    assert_eq!(uppercase_error.code, ErrorCode::InvalidInput);
}
