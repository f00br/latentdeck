#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::SystemTime,
};

use latentdeck_core::{
    diagnostics::{LogLevel, initialize_global_json_log, record_global},
    realtime_diagnostics::{RealtimeDiagnosticError, SanitizedToken},
};
use latentdeck_extension_manager::ExtensionRoots;
use latentdeck_library::Library;
use latentdeck_native_output::HostFullscreenController;
use tauri::{AppHandle, Manager as _, RunEvent, State};
use tauri_plugin_dialog::DialogExt as _;

mod bundled_decks;
mod capture_finalizer_v2;
#[cfg(test)]
mod codec_pack_test_support;
mod decoded_recording;
mod diagnostic_state;
mod embedded_viewport;
mod extension_commands;
mod generic_deck_runtime;
mod generic_deck_state;
mod library_state;
mod preset_state;
mod runtime_diagnostics;
use diagnostic_state::{DiagnosticSaveResult, deck_snapshot, write_deck_bundle};
use extension_commands::{
    ExtensionManagerState, extensions_deck_catalog, extensions_disable, extensions_enable,
    extensions_inspect, extensions_install, extensions_remove, extensions_repair,
    extensions_snapshot, extensions_verify,
};
use generic_deck_state::{
    GenericDeckAppState, deck_generic_capture_start, deck_generic_capture_status_get,
    deck_generic_capture_stop, deck_generic_close, deck_generic_controls_set,
    deck_generic_diagnostics_get, deck_generic_external_asset_clear,
    deck_generic_external_asset_select, deck_generic_foreground_clear, deck_generic_foreground_set,
    deck_generic_fullscreen_set, deck_generic_fullscreen_status_get, deck_generic_open,
    deck_generic_preset_load, deck_generic_preset_save, deck_generic_process_once,
    deck_generic_recording_start, deck_generic_recording_status_get, deck_generic_recording_stop,
    deck_generic_reset, deck_generic_roles_set, deck_generic_runtime_options,
    deck_generic_seed_set, deck_generic_sessions_get, deck_generic_spout_configure,
    deck_generic_spout_status_get, deck_generic_status_get, deck_generic_transport_set,
    deck_generic_viewport_session_begin, deck_generic_viewport_set_bounds,
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

#[derive(Default)]
struct ExitLifecycle {
    shutdown_started: AtomicBool,
    ready: AtomicBool,
}

const APP_LOCAL_DATA_DIRECTORY: &str = "studio.latentdeck.deck";

#[derive(Clone)]
struct AppLocalDataDir(PathBuf);

fn app_local_data_dir(local_app_data: &Path) -> PathBuf {
    local_app_data.join(APP_LOCAL_DATA_DIRECTORY)
}

#[tauri::command]
const fn product_version() -> &'static str {
    latentdeck_core::product_version()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn deck_save_diagnostics(
    app: AppHandle,
    generic: State<'_, GenericDeckAppState>,
    app_local_data: State<'_, AppLocalDataDir>,
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

    let (diagnostics, last_error) = generic.foreground_diagnostics().await?;
    let captured_at_unix_ms = current_unix_ms()?;
    let last_error = last_error
        .map(SanitizedToken::parse)
        .transpose()
        .map_err(|error| diagnostic_command_error(&error))?;
    let snapshot = deck_snapshot(captured_at_unix_ms, diagnostics, last_error)
        .map_err(|error| diagnostic_command_error(&error))?;
    let deck_log_root = app_local_data.0.join("logs");
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
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .expect("LOCALAPPDATA is required for the LatentDeck extension roots");
    assert!(
        local_app_data.is_absolute(),
        "LOCALAPPDATA must be an absolute path"
    );
    let app_local_data = app_local_data_dir(&local_app_data);
    let extension_roots = ExtensionRoots::from_local_app_data(local_app_data);
    let generic_app_data = extension_roots.base_root.clone();
    let bundled_deck_report = bundled_decks::provision_bundled_decks(&extension_roots)
        .expect("LatentDeck bundled Deck provisioning root is unavailable");
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ExitLifecycle::default())
        .manage(HostFullscreenController::new())
        .manage(ExtensionManagerState::new(extension_roots))
        .manage(GenericDeckAppState::new(generic_app_data))
        .manage(AppLocalDataDir(app_local_data.clone()))
        .setup(move |app| {
            let app_data_dir = app_local_data.clone();
            fs::create_dir_all(&app_data_dir)?;
            if initialize_global_json_log(&app_data_dir.join("logs"), "latentdeck").is_ok() {
                record_global(LogLevel::Info, "app.started", None);
                for issue in &bundled_deck_report.issues {
                    let event = match issue.package.package_id.as_str() {
                        "org.latentdeck.deck.d2" => "extensions.bundled_d2_issue",
                        "org.latentdeck.deck.q4" => "extensions.bundled_q4_issue",
                        _ => "extensions.bundled_deck_issue",
                    };
                    record_global(LogLevel::Warn, event, Some(issue.code.as_str()));
                }
            }
            let library = Library::open(database_path(&app_data_dir))?;
            app.manage(AppState::new(library));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            product_version,
            deck_generic_runtime_options,
            deck_generic_external_asset_select,
            deck_generic_external_asset_clear,
            deck_generic_open,
            deck_generic_sessions_get,
            deck_generic_status_get,
            deck_generic_process_once,
            deck_generic_controls_set,
            deck_generic_roles_set,
            deck_generic_transport_set,
            deck_generic_seed_set,
            deck_generic_reset,
            deck_generic_foreground_set,
            deck_generic_foreground_clear,
            deck_generic_close,
            deck_generic_viewport_session_begin,
            deck_generic_viewport_set_bounds,
            deck_generic_fullscreen_status_get,
            deck_generic_fullscreen_set,
            deck_generic_spout_status_get,
            deck_generic_spout_configure,
            deck_generic_capture_start,
            deck_generic_capture_stop,
            deck_generic_capture_status_get,
            deck_generic_recording_start,
            deck_generic_recording_stop,
            deck_generic_recording_status_get,
            deck_generic_diagnostics_get,
            deck_generic_preset_save,
            deck_generic_preset_load,
            extensions_deck_catalog,
            extensions_snapshot,
            extensions_inspect,
            extensions_install,
            extensions_verify,
            extensions_enable,
            extensions_disable,
            extensions_remove,
            extensions_repair,
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
            let lifecycle = app_handle.state::<ExitLifecycle>();
            if !lifecycle.ready.load(Ordering::Acquire) {
                api.prevent_exit();
                if !lifecycle.shutdown_started.swap(true, Ordering::AcqRel) {
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        app_handle
                            .state::<GenericDeckAppState>()
                            .shutdown_all()
                            .await;
                        app_handle
                            .state::<ExitLifecycle>()
                            .ready
                            .store(true, Ordering::Release);
                        record_global(LogLevel::Info, "app.exit_ready", None);
                        app_handle.exit(code.unwrap_or_default());
                    });
                }
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

    #[test]
    fn app_data_dir_uses_the_same_explicit_local_root_as_extensions() {
        let local_app_data = PathBuf::from("explicit-local-app-data-root");
        assert_eq!(
            app_local_data_dir(&local_app_data),
            local_app_data.join(APP_LOCAL_DATA_DIRECTORY)
        );
    }

    #[test]
    fn app_data_directory_matches_the_tauri_identifier() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("Tauri config JSON");
        assert_eq!(
            config.get("identifier").and_then(serde_json::Value::as_str),
            Some(APP_LOCAL_DATA_DIRECTORY)
        );
    }
}
