use std::fs;

use latentdeck_library::Library;
use tauri::Manager as _;

mod library_state;

use library_state::{
    AppState, database_path, library_add_membership, library_create_collection,
    library_delete_collection, library_import_files, library_import_folder, library_mark_recent,
    library_reindex, library_remove_membership, library_rename_collection,
    library_reorder_collections, library_reorder_members, library_set_active_collection,
    library_set_favorite, library_set_tags, library_snapshot,
};

#[tauri::command]
const fn product_version() -> &'static str {
    latentdeck_core::product_version()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_local_data_dir()?;
            fs::create_dir_all(&app_data_dir)?;
            let library = Library::open(database_path(&app_data_dir))?;
            app.manage(AppState::new(library));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            product_version,
            library_snapshot,
            library_set_active_collection,
            library_import_files,
            library_import_folder,
            library_reindex,
            library_create_collection,
            library_rename_collection,
            library_delete_collection,
            library_reorder_collections,
            library_add_membership,
            library_remove_membership,
            library_reorder_members,
            library_set_favorite,
            library_set_tags,
            library_mark_recent,
        ])
        .run(tauri::generate_context!())
        .expect("LatentDeck application runtime failed");
}
