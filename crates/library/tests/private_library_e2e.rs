//! Private opt-in proof over owner-controlled real cartridges.
//!
//! Set `LATENTDECK_PRIVATE_LIBRARY_CARTRIDGES` to an OS path-list containing
//! at least three compatible `.lc` files. No local path or payload is embedded
//! in the public test.

use std::{env, path::PathBuf};

use latentdeck_library::{CollectionId, Library, QueryOptions};

#[test]
#[ignore = "requires at least three owner-controlled real LC cartridges"]
fn real_cartridges_keep_many_to_many_membership_and_virtual_views_consistent() {
    let paths = private_paths();
    assert!(
        paths.len() >= 3,
        "at least three private cartridges are required"
    );

    let mut library = Library::in_memory().expect("private library database");
    let keys = paths
        .iter()
        .map(|path| {
            library
                .import_file(path)
                .expect("real cartridge passes full LC validation")
                .key
        })
        .collect::<Vec<_>>();

    let alpha = library
        .create_collection("Private Alpha")
        .expect("create first collection");
    let beta = library
        .create_collection("Private Beta")
        .expect("create second collection");
    library
        .add_to_collection(&alpha.id, &keys[0])
        .expect("first cartridge joins alpha");
    library
        .add_to_collection(&beta.id, &keys[0])
        .expect("same cartridge joins beta");
    library
        .add_to_collection(&alpha.id, &keys[1])
        .expect("second cartridge joins alpha");
    library
        .add_to_collection(&beta.id, &keys[2])
        .expect("third cartridge joins beta");

    library
        .reorder_collection(&alpha.id, &[keys[1].clone(), keys[0].clone()])
        .expect("manual member order is accepted atomically");
    assert_eq!(
        keys_for(&library, &alpha.id),
        vec![keys[1].as_str().to_owned(), keys[0].as_str().to_owned()]
    );
    assert_eq!(
        keys_for(&library, &beta.id),
        vec![keys[0].as_str().to_owned(), keys[2].as_str().to_owned()]
    );

    library
        .set_favorite(&keys[0], true)
        .expect("favorite real cartridge");
    library
        .set_tags(
            &keys[0],
            &["private-proof".to_owned(), "many-to-many".to_owned()],
        )
        .expect("tag real cartridge");
    library
        .mark_recent(&keys[0])
        .expect("record recent real cartridge");

    library
        .delete_collection(&alpha.id)
        .expect("delete collection only");
    assert_eq!(
        keys_for(&library, &CollectionId::all_cartridges()).len(),
        keys.len(),
        "deleting a collection never deletes cartridge identities"
    );
    assert_eq!(
        keys_for(&library, &beta.id),
        vec![keys[0].as_str().to_owned(), keys[2].as_str().to_owned()],
        "other memberships survive collection deletion"
    );
    assert_eq!(
        keys_for(&library, &CollectionId::unassigned()),
        vec![keys[1].as_str().to_owned()],
        "a cartridge with no remaining membership becomes unassigned"
    );
}

fn private_paths() -> Vec<PathBuf> {
    let value = env::var_os("LATENTDECK_PRIVATE_LIBRARY_CARTRIDGES")
        .expect("LATENTDECK_PRIVATE_LIBRARY_CARTRIDGES is required");
    let paths = env::split_paths(&value).collect::<Vec<_>>();
    assert!(
        paths.iter().all(|path| path.is_file()),
        "every private path must be a file"
    );
    paths
}

fn keys_for(library: &Library, collection: &CollectionId) -> Vec<String> {
    library
        .query_collection(collection, &QueryOptions::default())
        .expect("query collection")
        .iter()
        .map(|record| record.key.as_str().to_owned())
        .collect()
}
