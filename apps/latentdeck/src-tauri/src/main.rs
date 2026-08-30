use std::fs;

use latentdeck_library::Library;
use tauri::{Emitter as _, Manager as _, RunEvent, Window, WindowEvent};

mod d2_capture_host;
mod d2_runtime;
mod d2_state;
mod library_state;
mod q4_capture_host;
mod q4_runtime;
mod q4_state;

use d2_runtime::D2_OUTPUT_WINDOW_LABEL;
use d2_state::{
    D2AppState, ExitRequest, deck_d2_backend_status_get, deck_d2_capture_live_start,
    deck_d2_capture_live_stop, deck_d2_capture_snapshot, deck_d2_capture_status_get,
    deck_d2_controls_set, deck_d2_fullscreen, deck_d2_open, deck_d2_restart, deck_d2_seed_set,
    deck_d2_select_decoder, deck_d2_spout_configure, deck_d2_spout_status_get, deck_d2_status_get,
    deck_d2_transport_set,
};
use library_state::{
    AppState, database_path, library_add_membership, library_create_collection,
    library_delete_collection, library_import_files, library_import_folder, library_mark_recent,
    library_reindex, library_remove_membership, library_rename_collection,
    library_reorder_collections, library_reorder_members, library_set_active_collection,
    library_set_favorite, library_set_tags, library_snapshot,
};
use q4_runtime::Q4_OUTPUT_WINDOW_LABEL;
use q4_state::{
    Q4AppState, deck_q4_backend_status_get, deck_q4_capture_live_start, deck_q4_capture_live_stop,
    deck_q4_capture_snapshot, deck_q4_capture_status_get, deck_q4_controls_set, deck_q4_fullscreen,
    deck_q4_open, deck_q4_restart, deck_q4_roles_set, deck_q4_seed_set, deck_q4_select_decoder,
    deck_q4_spout_configure, deck_q4_spout_status_get, deck_q4_status_get, deck_q4_transport_set,
};

#[tauri::command]
const fn product_version() -> &'static str {
    latentdeck_core::product_version()
}

fn handle_d2_output_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::Resized(size) => {
            window
                .app_handle()
                .state::<D2AppState>()
                .queue_resize(size.width, size.height);
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let app = window.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = app.state::<D2AppState>().shutdown_runtime().await {
                    let _ = app.emit("deck-d2-error", error.event());
                }
                // Runtime shutdown normally owns destruction. If its output
                // teardown failed after taking the slot, remove a surviving
                // window so repeated CloseRequested cannot wedge.
                if let Some(window) = app.get_window(D2_OUTPUT_WINDOW_LABEL) {
                    let _ = window.destroy();
                }
            });
        }
        _ => {}
    }
}

fn handle_q4_output_event(window: &Window, event: &WindowEvent) {
    match event {
        WindowEvent::Resized(size) => {
            window
                .app_handle()
                .state::<Q4AppState>()
                .queue_resize(size.width, size.height);
        }
        WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let app = window.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = app.state::<Q4AppState>().shutdown_runtime().await {
                    let _ = app.emit("deck-q4-error", error.event());
                }
                if let Some(window) = app.get_window(Q4_OUTPUT_WINDOW_LABEL) {
                    let _ = window.destroy();
                }
            });
        }
        _ => {}
    }
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(D2AppState::discover())
        .manage(Q4AppState::discover())
        .setup(|app| {
            let app_data_dir = app.path().app_local_data_dir()?;
            fs::create_dir_all(&app_data_dir)?;
            let library = Library::open(database_path(&app_data_dir))?;
            app.manage(AppState::new(library));
            app.state::<D2AppState>()
                .start_resize_forwarder(app.handle().clone());
            app.state::<Q4AppState>()
                .start_resize_forwarder(app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == D2_OUTPUT_WINDOW_LABEL {
                handle_d2_output_event(window, event);
            } else if window.label() == Q4_OUTPUT_WINDOW_LABEL {
                handle_q4_output_event(window, event);
            }
        })
        .invoke_handler(tauri::generate_handler![
            product_version,
            deck_d2_backend_status_get,
            deck_d2_select_decoder,
            deck_d2_open,
            deck_d2_controls_set,
            deck_d2_transport_set,
            deck_d2_seed_set,
            deck_d2_restart,
            deck_d2_capture_snapshot,
            deck_d2_capture_live_start,
            deck_d2_capture_live_stop,
            deck_d2_capture_status_get,
            deck_d2_status_get,
            deck_d2_fullscreen,
            deck_d2_spout_status_get,
            deck_d2_spout_configure,
            deck_q4_backend_status_get,
            deck_q4_select_decoder,
            deck_q4_open,
            deck_q4_controls_set,
            deck_q4_roles_set,
            deck_q4_transport_set,
            deck_q4_seed_set,
            deck_q4_restart,
            deck_q4_capture_snapshot,
            deck_q4_capture_live_start,
            deck_q4_capture_live_stop,
            deck_q4_capture_status_get,
            deck_q4_status_get,
            deck_q4_fullscreen,
            deck_q4_spout_status_get,
            deck_q4_spout_configure,
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
        .build(tauri::generate_context!())
        .expect("LatentDeck application runtime failed");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            let state = app_handle.state::<D2AppState>();
            match state.request_exit() {
                ExitRequest::BeginShutdown => {
                    api.prevent_exit();
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = app_handle.state::<D2AppState>().shutdown_runtime().await;
                        let _ = app_handle.state::<Q4AppState>().shutdown_runtime().await;
                        app_handle.state::<D2AppState>().mark_exit_ready();
                        app_handle.exit(code.unwrap_or_default());
                    });
                }
                ExitRequest::WaitForShutdown => api.prevent_exit(),
                ExitRequest::AllowExit => {}
            }
        }
    });
}
