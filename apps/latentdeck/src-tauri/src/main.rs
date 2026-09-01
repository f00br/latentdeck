#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{fs, path::PathBuf, time::SystemTime};

use latentdeck_core::{
    diagnostics::{LogLevel, initialize_global_json_log, record_global},
    realtime_diagnostics::RealtimeDiagnosticError,
};
use latentdeck_library::Library;
use latentdeck_native_output::HostFullscreenController;
use tauri::{AppHandle, Manager as _, RunEvent, State};
use tauri_plugin_dialog::DialogExt as _;

#[cfg(test)]
mod codec_pack_test_support;
mod d2_capture_host;
mod d2_runtime;
mod d2_state;
mod decoded_recording;
mod diagnostic_state;
mod embedded_viewport;
mod library_state;
mod preset_state;
mod q4_capture_host;
mod q4_runtime;
mod q4_state;
mod runtime_diagnostics;
mod runtime_replacement;

use d2_state::{
    D2AppState, ExitRequest, deck_d2_backend_rediscover, deck_d2_backend_status_get,
    deck_d2_capture_live_start, deck_d2_capture_live_stop, deck_d2_capture_snapshot,
    deck_d2_capture_status_get, deck_d2_controls_set, deck_d2_fullscreen_set,
    deck_d2_fullscreen_status_get, deck_d2_open, deck_d2_recording_start,
    deck_d2_recording_status_get, deck_d2_recording_stop, deck_d2_restart, deck_d2_seed_set,
    deck_d2_select_decoder, deck_d2_spout_configure, deck_d2_spout_status_get, deck_d2_status_get,
    deck_d2_transport_set, deck_d2_viewport_session_begin, deck_d2_viewport_set_bounds,
};
use diagnostic_state::{
    DeckDiagnosticLifecycle, DeckSnapshotError, DiagnosticSaveResult, deck_snapshot,
    write_deck_bundle,
};
use library_state::{
    AppState, database_path, library_activate_collection_snapshot, library_add_membership,
    library_create_collection, library_delete_collection, library_import_files,
    library_import_folder, library_mark_recent, library_reindex, library_remove_membership,
    library_rename_collection, library_reorder_collections, library_reorder_members,
    library_resolve_preset_sources, library_set_active_collection, library_set_favorite,
    library_set_tags, library_signal_compatibility, library_snapshot,
};
use preset_state::{deck_preset_load, deck_preset_save};
use q4_state::{
    Q4AppState, deck_q4_backend_rediscover, deck_q4_backend_status_get, deck_q4_capture_live_start,
    deck_q4_capture_live_stop, deck_q4_capture_snapshot, deck_q4_capture_status_get,
    deck_q4_controls_set, deck_q4_fullscreen_set, deck_q4_fullscreen_status_get, deck_q4_open,
    deck_q4_recording_start, deck_q4_recording_status_get, deck_q4_recording_stop, deck_q4_restart,
    deck_q4_roles_set, deck_q4_seed_set, deck_q4_select_decoder, deck_q4_spout_configure,
    deck_q4_spout_status_get, deck_q4_status_get, deck_q4_transport_set,
    deck_q4_viewport_session_begin, deck_q4_viewport_set_bounds,
};

#[tauri::command]
const fn product_version() -> &'static str {
    latentdeck_core::product_version()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn deck_save_diagnostics(
    app: AppHandle,
    d2: State<'_, D2AppState>,
    q4: State<'_, Q4AppState>,
    lifecycle: State<'_, DeckDiagnosticLifecycle>,
) -> Result<DiagnosticSaveResult, library_state::CommandError> {
    let suggested_name = format!("latentdeck-diagnostics-{}.zip", current_unix_ms()? / 1_000);
    let selected = app
        .dialog()
        .file()
        .add_filter("LatentDeck Diagnostic Bundle", &["zip"])
        .set_file_name(suggested_name)
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(DiagnosticSaveResult::Cancelled);
    };
    let destination = validate_diagnostic_destination(selected.into_path().map_err(|_| {
        library_state::CommandError::new(
            "diagnostics.destination_invalid",
            "The native save dialog did not return a usable diagnostic archive path.",
        )
    })?)?;

    let (d2, q4) = tokio::join!(d2.runtime_diagnostics(), q4.runtime_diagnostics());
    let d2 = d2.map_err(|error| library_state::CommandError::new(error.code, error.message))?;
    let q4 = q4.map_err(|error| library_state::CommandError::new(error.code, error.message))?;
    let captured_at_unix_ms = current_unix_ms()?;
    let last_error = lifecycle
        .last_error()
        .map_err(|error| diagnostic_command_error(&error))?;
    let snapshot = deck_snapshot(captured_at_unix_ms, d2, q4, last_error)
        .map_err(deck_snapshot_command_error)?;
    let deck_log_root = app
        .path()
        .app_local_data_dir()
        .map_err(|_| {
            library_state::CommandError::new(
                "diagnostics.log_root_unavailable",
                "LatentDeck could not resolve its installed diagnostic log folder.",
            )
        })?
        .join("logs");
    let worker_log_root = std::env::temp_dir()
        .join("LatentDeck")
        .join("worker-diagnostics");
    let receipt = tauri::async_runtime::spawn_blocking(move || {
        write_deck_bundle(&destination, &snapshot, &deck_log_root, &worker_log_root)
    })
    .await
    .map_err(|_| {
        library_state::CommandError::new(
            "diagnostics.task_failed",
            "The diagnostic archive task stopped unexpectedly; retry with a new file name.",
        )
    })?
    .map_err(|error| diagnostic_command_error(&error))?;
    record_global(LogLevel::Info, "diagnostics.bundle_saved", None);
    Ok(receipt.into())
}

fn current_unix_ms() -> Result<u64, library_state::CommandError> {
    let milliseconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| {
            library_state::CommandError::new(
                "diagnostics.clock_invalid",
                "The system clock cannot represent a diagnostic timestamp.",
            )
        })?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| {
        library_state::CommandError::new(
            "diagnostics.clock_invalid",
            "The system clock cannot represent a diagnostic timestamp.",
        )
    })
}

fn validate_diagnostic_destination(
    mut path: PathBuf,
) -> Result<PathBuf, library_state::CommandError> {
    if !path.is_absolute() {
        return Err(library_state::CommandError::new(
            "diagnostics.destination_invalid",
            "Choose an absolute local destination for the diagnostic archive.",
        ));
    }
    match path.extension().and_then(|extension| extension.to_str()) {
        None => {
            let _ = path.set_extension("zip");
        }
        Some(extension) if extension.eq_ignore_ascii_case("zip") => {}
        Some(_) => {
            return Err(library_state::CommandError::new(
                "diagnostics.destination_invalid",
                "The diagnostic archive file name must use the .zip extension.",
            ));
        }
    }
    Ok(path)
}

fn deck_snapshot_command_error(error: DeckSnapshotError) -> library_state::CommandError {
    match error {
        DeckSnapshotError::IdentityConflict => library_state::CommandError::new(
            "diagnostics.session_identity_conflict",
            "Active D2 and Q4 sessions use different GPU or codec identities and cannot be merged safely.",
        ),
        DeckSnapshotError::Contract(error) => diagnostic_command_error(&error),
    }
}

fn diagnostic_command_error(error: &RealtimeDiagnosticError) -> library_state::CommandError {
    match error {
        RealtimeDiagnosticError::OutputExists => library_state::CommandError::new(
            "diagnostics.output_exists",
            "The selected diagnostic archive already exists and was not overwritten. Choose a new file name.",
        ),
        RealtimeDiagnosticError::InvalidDestination => library_state::CommandError::new(
            "diagnostics.destination_invalid",
            "Choose a writable local folder and a .zip file name for the diagnostic archive.",
        ),
        RealtimeDiagnosticError::LimitExceeded(_) => library_state::CommandError::new(
            "diagnostics.limit_exceeded",
            "The bounded diagnostic evidence exceeded its safety limit; older logs can be removed before retrying.",
        ),
        RealtimeDiagnosticError::Io { .. } => library_state::CommandError::new(
            "diagnostics.write_failed",
            "The diagnostic archive could not be written. Check folder permissions and try another file name.",
        ),
        _ => library_state::CommandError::new(
            "diagnostics.contract_invalid",
            "LatentDeck could not create a safe diagnostic snapshot from the current state.",
        ),
    }
}

// Tauri's composition root deliberately lists every exposed command and
// lifecycle hook in one auditable place.
#[allow(clippy::too_many_lines)]
fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DeckDiagnosticLifecycle::default())
        .manage(HostFullscreenController::new())
        .manage(D2AppState::discover())
        .manage(Q4AppState::discover())
        .setup(|app| {
            let app_data_dir = app.path().app_local_data_dir()?;
            fs::create_dir_all(&app_data_dir)?;
            if initialize_global_json_log(&app_data_dir.join("logs"), "latentdeck").is_ok() {
                record_global(LogLevel::Info, "app.started", None);
            }
            let library = Library::open(database_path(&app_data_dir))?;
            app.manage(AppState::new(library));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            product_version,
            deck_d2_backend_status_get,
            deck_d2_backend_rediscover,
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
            deck_d2_recording_start,
            deck_d2_recording_stop,
            deck_d2_recording_status_get,
            deck_d2_status_get,
            deck_d2_fullscreen_status_get,
            deck_d2_fullscreen_set,
            deck_d2_spout_status_get,
            deck_d2_spout_configure,
            deck_d2_viewport_session_begin,
            deck_d2_viewport_set_bounds,
            deck_q4_backend_status_get,
            deck_q4_backend_rediscover,
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
            deck_q4_recording_start,
            deck_q4_recording_stop,
            deck_q4_recording_status_get,
            deck_q4_status_get,
            deck_q4_fullscreen_status_get,
            deck_q4_fullscreen_set,
            deck_q4_spout_status_get,
            deck_q4_spout_configure,
            deck_q4_viewport_session_begin,
            deck_q4_viewport_set_bounds,
            library_snapshot,
            library_activate_collection_snapshot,
            library_resolve_preset_sources,
            library_signal_compatibility,
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
            deck_preset_save,
            deck_preset_load,
            deck_save_diagnostics,
        ])
        .build(tauri::generate_context!())
        .expect("LatentDeck application runtime failed");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            record_global(LogLevel::Info, "app.exit_requested", None);
            let state = app_handle.state::<D2AppState>();
            match state.request_exit() {
                ExitRequest::BeginShutdown => {
                    api.prevent_exit();
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = app_handle.state::<D2AppState>().shutdown_runtime().await;
                        let _ = app_handle.state::<Q4AppState>().shutdown_runtime().await;
                        app_handle.state::<D2AppState>().mark_exit_ready();
                        record_global(LogLevel::Info, "app.exit_ready", None);
                        app_handle.exit(code.unwrap_or_default());
                    });
                }
                ExitRequest::WaitForShutdown => api.prevent_exit(),
                ExitRequest::AllowExit => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_destination_adds_zip_only_when_extension_is_missing() {
        let destination = std::env::temp_dir().join("latentdeck-diagnostics-test");
        let validated = validate_diagnostic_destination(destination).expect("destination");
        assert_eq!(
            validated.extension().and_then(|value| value.to_str()),
            Some("zip")
        );
    }

    #[test]
    fn diagnostic_destination_rejects_non_zip_extension() {
        let destination = std::env::temp_dir().join("latentdeck-diagnostics-test.txt");
        let error = validate_diagnostic_destination(destination).expect_err("must reject");
        let value = serde_json::to_value(error).expect("serialize");
        assert_eq!(value["code"], "diagnostics.destination_invalid");
    }
}
