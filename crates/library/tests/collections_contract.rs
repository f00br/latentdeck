mod support;

use latentdeck_library::{
    ALL_CARTRIDGES_ID, CollectionId, ErrorCode, Library, QueryOptions, UNASSIGNED_ID,
};
use tempfile::tempdir;

use support::{ID_A, ID_B, ID_C, write_synthetic_lc};

fn seeded_library() -> (
    tempfile::TempDir,
    Library,
    latentdeck_library::CartridgeKey,
    latentdeck_library::CartridgeKey,
    latentdeck_library::CartridgeKey,
) {
    let temp = tempdir().expect("tempdir");
    let mut library = Library::in_memory().expect("library");
    let a = temp.path().join("alpha.lc");
    let b = temp.path().join("beta.lc");
    let c = temp.path().join("gamma.lc");
    write_synthetic_lc(&a, ID_A);
    write_synthetic_lc(&b, ID_B);
    write_synthetic_lc(&c, ID_C);
    let a = library.import_file(a).expect("import A").key;
    let b = library.import_file(b).expect("import B").key;
    let c = library.import_file(c).expect("import C").key;
    (temp, library, a, b, c)
}

#[test]
fn virtual_views_and_many_to_many_manual_order_are_stable() {
    let (_temp, mut library, a, b, c) = seeded_library();
    let warm = library.create_collection("Warm").expect("create warm");
    let live = library.create_collection("Live").expect("create live");
    library.add_to_collection(&warm.id, &a).expect("A to warm");
    library.add_to_collection(&warm.id, &b).expect("B to warm");
    library.add_to_collection(&live.id, &a).expect("A to live");
    library
        .reorder_collection(&warm.id, &[b.clone(), a.clone()])
        .expect("reorder warm");

    let warm_members = library
        .query_collection(&warm.id, &QueryOptions::default())
        .expect("warm query");
    assert_eq!(
        warm_members
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>(),
        vec![b.clone(), a.clone()]
    );
    let all = library
        .query_collection(&CollectionId::all_cartridges(), &QueryOptions::default())
        .expect("all query");
    assert_eq!(all.len(), 3);
    let unassigned = library
        .query_collection(&CollectionId::unassigned(), &QueryOptions::default())
        .expect("unassigned query");
    assert_eq!(unassigned.len(), 1);
    assert_eq!(unassigned[0].key, c);

    let collections = library.list_collections().expect("collections");
    assert_eq!(collections[0].id.as_str(), ALL_CARTRIDGES_ID);
    assert_eq!(collections[1].id.as_str(), UNASSIGNED_ID);
    assert_eq!(collections[2].id, warm.id);
    assert_eq!(collections[3].id, live.id);

    library
        .reorder_collections(&[live.id.clone(), warm.id.clone()])
        .expect("reorder collections");
    let collections = library.list_collections().expect("collections reordered");
    assert_eq!(collections[2].id, live.id);
    assert_eq!(collections[3].id, warm.id);
}

#[test]
fn deleting_collection_only_deletes_membership_and_never_cartridge_rows() {
    let (temp, mut library, a, _b, _c) = seeded_library();
    let first = library.create_collection("First").expect("first");
    let second = library.create_collection("Second").expect("second");
    library
        .add_to_collection(&first.id, &a)
        .expect("first member");
    library
        .add_to_collection(&second.id, &a)
        .expect("second member");

    library.delete_collection(&first.id).expect("delete first");
    assert!(library.get_cartridge(&a).expect("get A").is_some());
    let second_members = library
        .query_collection(&second.id, &QueryOptions::default())
        .expect("second members");
    assert_eq!(second_members.len(), 1);
    assert_eq!(second_members[0].key, a);
    assert!(temp.path().join("alpha.lc").is_file());
}

#[test]
fn rename_remove_and_duplicate_add_preserve_compact_order() {
    let (_temp, mut library, a, b, c) = seeded_library();
    let collection = library.create_collection("Draft").expect("collection");
    library
        .rename_collection(&collection.id, "Final")
        .expect("rename");
    library.add_to_collection(&collection.id, &a).expect("A");
    library.add_to_collection(&collection.id, &b).expect("B");
    library.add_to_collection(&collection.id, &c).expect("C");
    library
        .add_to_collection(&collection.id, &b)
        .expect("duplicate add is idempotent");
    library
        .remove_from_collection(&collection.id, &b)
        .expect("remove B");
    let members = library
        .query_collection(&collection.id, &QueryOptions::default())
        .expect("members");
    assert_eq!(
        members
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>(),
        vec![a, c]
    );
    let listed = library.list_collections().expect("collections");
    assert_eq!(listed[2].name, "Final");
}

#[test]
fn virtual_collections_cannot_be_mutated_or_persisted() {
    let (_temp, mut library, a, _b, _c) = seeded_library();
    for id in [CollectionId::all_cartridges(), CollectionId::unassigned()] {
        let add = library
            .add_to_collection(&id, &a)
            .expect_err("virtual add rejected");
        assert_eq!(add.code, ErrorCode::VirtualCollection);
        let rename = library
            .rename_collection(&id, "Nope")
            .expect_err("virtual rename rejected");
        assert_eq!(rename.code, ErrorCode::VirtualCollection);
        let delete = library
            .delete_collection(&id)
            .expect_err("virtual delete rejected");
        assert_eq!(delete.code, ErrorCode::VirtualCollection);
    }
    for reserved in ["All Cartridges", "Unassigned"] {
        let error = library
            .create_collection(reserved)
            .expect_err("reserved name");
        assert_eq!(error.code, ErrorCode::VirtualCollection);
    }
}

#[test]
fn failed_reorder_is_atomic() {
    let (_temp, mut library, a, b, _c) = seeded_library();
    let collection = library.create_collection("Atomic").expect("collection");
    library.add_to_collection(&collection.id, &a).expect("A");
    library.add_to_collection(&collection.id, &b).expect("B");
    let error = library
        .reorder_collection(&collection.id, &[a.clone(), a.clone()])
        .expect_err("duplicate reorder");
    assert_eq!(error.code, ErrorCode::InvalidInput);
    let members = library
        .query_collection(&collection.id, &QueryOptions::default())
        .expect("members");
    assert_eq!(
        members
            .iter()
            .map(|record| record.key.clone())
            .collect::<Vec<_>>(),
        vec![a, b]
    );
}
