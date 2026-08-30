//! Tauri command boundary for the backend-owned LD-Q4 runtime.

use std::sync::{Arc, Mutex, MutexGuard};

use latentdeck_core::diagnostics::{LogLevel, record_global};
use latentdeck_library::{CartridgeKey, DeckSourceIdentity, ResolvedDeckSource};
use latentdeck_native_output::NativeSpoutStatus;
use serde::Deserialize;
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::{
    diagnostic_state::DeckDiagnosticLifecycle,
    library_state::{AppState as LibraryAppState, CommandError, DeckKind},
    q4_capture_host::Q4CaptureView,
    q4_runtime::{
        Q4BackendController, Q4BackendView, Q4CaptureHostServices, Q4ControlsAckView,
        Q4ControlsInput, Q4LaunchConfig, Q4RolesAckView, Q4RolesInput, Q4Runtime,
        Q4RuntimeDiagnostics, Q4RuntimeError, Q4SeedAckView, Q4StatusView, Q4TransportAckView,
        Q4TransportInput, validate_selected_decoder,
    },
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Q4SourceIdentityInput {
    cartridge_id: String,
    archive_sha256: String,
}

pub(crate) struct Q4AppState {
    backend: Arc<Mutex<Q4BackendController>>,
    runtime: AsyncMutex<Option<Arc<Q4Runtime>>>,
    lifecycle: AsyncMutex<()>,
    status: Arc<Mutex<Q4StatusView>>,
    capture_status: Arc<Mutex<Q4CaptureView>>,
    resize_sender: watch::Sender<(u32, u32)>,
    resize_receiver: Mutex<Option<watch::Receiver<(u32, u32)>>>,
}

impl Q4AppState {
    pub(crate) fn discover() -> Self {
        let (resize_sender, resize_receiver) = watch::channel((0, 0));
        Self {
            backend: Arc::new(Mutex::new(Q4BackendController::discover_default())),
            runtime: AsyncMutex::new(None),
            lifecycle: AsyncMutex::new(()),
            status: Arc::new(Mutex::new(Q4StatusView::default())),
            capture_status: Arc::new(Mutex::new(Q4CaptureView::default())),
            resize_sender,
            resize_receiver: Mutex::new(Some(resize_receiver)),
        }
    }

    pub(crate) fn start_resize_forwarder(&self, app: AppHandle) {
        let receiver = self
            .resize_receiver
            .lock()
            .ok()
            .and_then(|mut receiver| receiver.take());
        let Some(mut receiver) = receiver else {
            return;
        };
        tauri::async_runtime::spawn(async move {
            while receiver.changed().await.is_ok() {
                let (width, height) = *receiver.borrow_and_update();
                if width > 0 && height > 0 {
                    app.state::<Q4AppState>().resize(width, height).await;
                }
            }
        });
    }

    pub(crate) fn queue_resize(&self, width: u32, height: u32) {
        self.resize_sender.send_replace((width, height));
    }

    pub(crate) async fn shutdown_runtime(&self) -> Result<(), Q4RuntimeError> {
        let _lifecycle = self.lifecycle.lock().await;
        shutdown_runtime_slot(&self.runtime).await
    }

    pub(crate) async fn resize(&self, width: u32, height: u32) {
        if let Some(runtime) = clone_slot(&self.runtime).await {
            let _ = runtime.resize(width, height).await;
        }
    }

    pub(crate) async fn runtime_diagnostics(
        &self,
    ) -> Result<Option<Q4RuntimeDiagnostics>, Q4RuntimeError> {
        match clone_diagnostic_slot(&self.lifecycle, &self.runtime).await {
            Some(runtime) => runtime.diagnostics().await,
            None => Ok(None),
        }
    }

    fn emit_error(app: &AppHandle, error: &Q4RuntimeError) {
        if let Some(lifecycle) = app.try_state::<DeckDiagnosticLifecycle>() {
            lifecycle.record_error(&error.code);
        }
        let _ = app.emit("deck-q4-error", error.event());
    }

    fn backend_view(&self) -> Result<Q4BackendView, Q4RuntimeError> {
        Ok(lock_backend(&self.backend)?.view())
    }

    fn shared_status(&self) -> Result<Q4StatusView, Q4RuntimeError> {
        self.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| Q4RuntimeError::state_poisoned())
    }

    fn shared_capture_status(&self) -> Result<Q4CaptureView, Q4RuntimeError> {
        self.capture_status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| Q4RuntimeError::state_poisoned())
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn deck_q4_backend_status_get(
    state: State<'_, Q4AppState>,
) -> Result<Q4BackendView, CommandError> {
    state.backend_view().map_err(command_error)
}

/// Re-run bounded physical Codec Pack discovery so an installation performed
/// after app startup becomes available without restarting `LatentDeck`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_backend_rediscover(
    state: State<'_, Q4AppState>,
) -> Result<Q4BackendView, CommandError> {
    let discovered = tauri::async_runtime::spawn_blocking(Q4BackendController::discover_default)
        .await
        .map_err(|_| {
            CommandError::new(
                "codec.discovery_failed",
                "Codec Pack discovery stopped unexpectedly.",
            )
        })?;
    let mut backend = lock_backend(&state.backend).map_err(command_error)?;
    Ok(backend.accept_rediscovery(discovered))
}

/// Open a native picker and validate the selected external TAEH3 asset.
/// There is intentionally no path argument on this command.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_select_decoder(
    app: AppHandle,
    state: State<'_, Q4AppState>,
) -> Result<Q4BackendView, CommandError> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Safetensors", &["safetensors"])
        .blocking_pick_file();
    let Some(selected) = selected else {
        return state.backend_view().map_err(command_error);
    };
    let path = selected.into_path().map_err(|_| {
        CommandError::new(
            "codec.asset_incompatible",
            "The selected decoder file path is unavailable.",
        )
    })?;

    let _lifecycle = state.lifecycle.lock().await;
    if let Err(error) = shutdown_runtime_slot(&state.runtime).await {
        Q4AppState::emit_error(&app, &error);
        return Err(command_error(error));
    }
    let pack = match state
        .backend_view()
        .and_then(|_| lock_backend(&state.backend).and_then(|backend| backend.pack_for_selection()))
    {
        Ok(pack) => pack,
        Err(error) => {
            Q4AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let validated =
        tauri::async_runtime::spawn_blocking(move || validate_selected_decoder(&pack, &path))
            .await
            .map_err(|_| {
                CommandError::new(
                    "codec.asset_validation_failed",
                    "Decoder asset validation task stopped unexpectedly.",
                )
            })?;
    let validated = match validated {
        Ok(value) => value,
        Err(error) => {
            Q4AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let mut backend = lock_backend(&state.backend).map_err(command_error)?;
    Ok(backend.accept_decoder(validated))
}

#[tauri::command]
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_open(
    app: AppHandle,
    state: State<'_, Q4AppState>,
    library: State<'_, LibraryAppState>,
    source_a: Q4SourceIdentityInput,
    source_b: Q4SourceIdentityInput,
    source_c: Q4SourceIdentityInput,
    source_d: Q4SourceIdentityInput,
    roles: Q4RolesInput,
    controls: Q4ControlsInput,
    transport: Q4TransportInput,
    seed: u64,
) -> Result<Q4StatusView, CommandError> {
    let roles = roles.into_wire().map_err(command_error)?;
    let controls = controls.into_wire().map_err(command_error)?;
    let transport = transport.into();
    if seed > latentdeck_control::MAX_Q4_SAFE_INTEGER {
        return Err(command_error(Q4RuntimeError::invalid_seed()));
    }
    let identity_a = source_identity(source_a)?;
    let identity_b = source_identity(source_b)?;
    let identity_c = source_identity(source_c)?;
    let identity_d = source_identity(source_d)?;
    let _lifecycle = state.lifecycle.lock().await;
    if let Err(error) = shutdown_runtime_slot(&state.runtime).await {
        Q4AppState::emit_error(&app, &error);
        return Err(command_error(error));
    }
    let backend = match lock_backend(&state.backend).and_then(|backend| backend.launch_backend()) {
        Ok(value) => value,
        Err(error) => {
            Q4AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let source_a = resolve_source(&library, identity_a, "A").await?;
    let source_b = resolve_source(&library, identity_b, "B").await?;
    let source_c = resolve_source(&library, identity_c, "C").await?;
    let source_d = resolve_source(&library, identity_d, "D").await?;
    let slot_bindings = q4_slot_bindings(&source_a, &source_b, &source_c, &source_d);
    let app_local_data = app.path().app_local_data_dir().map_err(|_| {
        CommandError::new(
            "capture.spool_root_invalid",
            "The app-local Q4 capture storage root is unavailable.",
        )
    })?;
    let library_importer = library.importer();
    let capture_host = Q4CaptureHostServices::new(app_local_data, library_importer);
    let config = tauri::async_runtime::spawn_blocking(move || {
        Q4LaunchConfig::build(
            backend,
            &source_a,
            &source_b,
            &source_c,
            &source_d,
            roles,
            controls,
            transport,
            seed,
            capture_host,
        )
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "deck.source_validation_failed",
            "LD-Q4 source validation task stopped unexpectedly.",
        )
    })?;
    let config = match config {
        Ok(value) => value,
        Err(error) => {
            Q4AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let deck_session = library.begin_deck_session(DeckKind::Q4)?;
    let started = Q4Runtime::start(
        app.clone(),
        Arc::clone(&state.status),
        Arc::clone(&state.capture_status),
        config,
        deck_session.clone(),
    )
    .await;
    let started = match started {
        Ok(value) => value,
        Err(error) => {
            deck_session.close();
            Q4AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let started = Arc::new(started);
    let view = match started.status().await {
        Ok(view) => view,
        Err(error) => {
            let _ = started.shutdown().await;
            return Err(command_error(error));
        }
    };
    replace_slot(&state.runtime, started).await;
    if let Err(error) = deck_session.publish(slot_bindings) {
        let _ = shutdown_runtime_slot(&state.runtime).await;
        return Err(error);
    }
    Ok(view)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_controls_set(
    state: State<'_, Q4AppState>,
    controls: Q4ControlsInput,
) -> Result<Q4ControlsAckView, CommandError> {
    let controls = controls.into_wire().map_err(command_error)?;
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.controls_set(controls).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_roles_set(
    state: State<'_, Q4AppState>,
    roles: Q4RolesInput,
) -> Result<Q4RolesAckView, CommandError> {
    let roles = roles.into_wire().map_err(command_error)?;
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.roles_set(roles).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_transport_set(
    state: State<'_, Q4AppState>,
    transport: Q4TransportInput,
) -> Result<Q4TransportAckView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime
        .transport_set(transport.into())
        .await
        .map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_seed_set(
    state: State<'_, Q4AppState>,
    seed: u64,
) -> Result<Q4SeedAckView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.seed_set(seed).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_restart(
    state: State<'_, Q4AppState>,
) -> Result<Q4StatusView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.restart().await.map_err(command_error)
}

/// Run one full-carrier Q4 Snapshot after a native save selection. No path is
/// accepted from or returned to the webview.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_capture_snapshot(
    app: AppHandle,
    state: State<'_, Q4AppState>,
) -> Result<Option<Q4CaptureView>, CommandError> {
    let Some(output) = capture_output_path(&app, "LatentDeck Q4 Snapshot.lc")? else {
        return Ok(None);
    };
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime
        .capture_start(latentdeck_control::Q4CaptureMode::Snapshot, output)
        .await
        .map(Some)
        .map_err(command_error)
}

/// Start a bounded Q4 Live Capture after a native save selection. Completion
/// remains actor-owned and is reported through path-free status and events.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_capture_live_start(
    app: AppHandle,
    state: State<'_, Q4AppState>,
) -> Result<Option<Q4CaptureView>, CommandError> {
    let Some(output) = capture_output_path(&app, "LatentDeck Q4 Live Capture.lc")? else {
        return Ok(None);
    };
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime
        .capture_start(latentdeck_control::Q4CaptureMode::LiveCapture, output)
        .await
        .map(Some)
        .map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_capture_live_stop(
    state: State<'_, Q4AppState>,
) -> Result<Q4CaptureView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.capture_stop().await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_capture_status_get(
    state: State<'_, Q4AppState>,
) -> Result<Q4CaptureView, CommandError> {
    let Some(runtime) = clone_slot(&state.runtime).await else {
        return state.shared_capture_status().map_err(command_error);
    };
    match runtime.capture_status().await {
        Ok(view) => Ok(view),
        Err(error) if error.code == "deck.runtime_unavailable" => {
            state.shared_capture_status().map_err(command_error)
        }
        Err(error) => Err(command_error(error)),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_status_get(
    state: State<'_, Q4AppState>,
) -> Result<Q4StatusView, CommandError> {
    let Some(runtime) = clone_slot(&state.runtime).await else {
        return state.shared_status().map_err(command_error);
    };
    match runtime.status().await {
        Ok(view) => Ok(view),
        Err(error) if error.code == "deck.runtime_unavailable" => {
            state.shared_status().map_err(command_error)
        }
        Err(error) => Err(command_error(error)),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_fullscreen_status_get(
    state: State<'_, Q4AppState>,
) -> Result<Option<bool>, CommandError> {
    let Some(runtime) = clone_slot(&state.runtime).await else {
        return Ok(None);
    };
    match runtime.fullscreen_status().await {
        Ok(active) => Ok(Some(active)),
        Err(error) if error.code == "deck.runtime_unavailable" => Ok(None),
        Err(error) => Err(command_error(error)),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_fullscreen_set(
    state: State<'_, Q4AppState>,
    enabled: bool,
) -> Result<bool, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.set_fullscreen(enabled).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_spout_status_get(
    state: State<'_, Q4AppState>,
) -> Result<Option<NativeSpoutStatus>, CommandError> {
    let Some(runtime) = clone_slot(&state.runtime).await else {
        return Ok(None);
    };
    runtime
        .spout_status()
        .await
        .map(Some)
        .map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_q4_spout_configure(
    state: State<'_, Q4AppState>,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<NativeSpoutStatus, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime
        .configure_spout(name, enabled)
        .await
        .map_err(command_error)
}

async fn resolve_source(
    library: &State<'_, LibraryAppState>,
    identity: DeckSourceIdentity,
    slot: &'static str,
) -> Result<latentdeck_library::ResolvedDeckSource, CommandError> {
    library.resolve_deck_source(identity).await.map_err(|_| {
        CommandError::new(
            "deck.source_invalid",
            format!("LD-Q4 source {slot} is not a present, unchanged Library cartridge."),
        )
    })
}

async fn shutdown_runtime_slot(
    runtime: &AsyncMutex<Option<Arc<Q4Runtime>>>,
) -> Result<(), Q4RuntimeError> {
    let Some(active) = take_slot(runtime).await else {
        return Ok(());
    };
    active.shutdown().await
}

async fn clone_slot<T>(slot: &AsyncMutex<Option<Arc<T>>>) -> Option<Arc<T>> {
    slot.lock().await.as_ref().cloned()
}

async fn clone_diagnostic_slot<T>(
    lifecycle: &AsyncMutex<()>,
    slot: &AsyncMutex<Option<Arc<T>>>,
) -> Option<Arc<T>> {
    let lifecycle_guard = lifecycle.lock().await;
    let runtime = clone_slot(slot).await;
    drop(lifecycle_guard);
    runtime
}

async fn take_slot<T>(slot: &AsyncMutex<Option<Arc<T>>>) -> Option<Arc<T>> {
    slot.lock().await.take()
}

async fn replace_slot<T>(slot: &AsyncMutex<Option<Arc<T>>>, value: Arc<T>) {
    *slot.lock().await = Some(value);
}

fn source_identity(input: Q4SourceIdentityInput) -> Result<DeckSourceIdentity, CommandError> {
    DeckSourceIdentity::new(
        input.cartridge_id,
        CartridgeKey::new_unchecked(input.archive_sha256),
    )
    .map_err(|_| {
        CommandError::new(
            "deck.source_identity_invalid",
            "LD-Q4 source identity must contain a canonical UUID and lowercase SHA-256.",
        )
    })
}

fn q4_slot_bindings(
    source_a: &ResolvedDeckSource,
    source_b: &ResolvedDeckSource,
    source_c: &ResolvedDeckSource,
    source_d: &ResolvedDeckSource,
) -> [(&'static str, CartridgeKey); 4] {
    [
        ("A", source_a.identity().archive_sha256().clone()),
        ("B", source_b.identity().archive_sha256().clone()),
        ("C", source_c.identity().archive_sha256().clone()),
        ("D", source_d.identity().archive_sha256().clone()),
    ]
}

fn capture_output_path(
    app: &AppHandle,
    suggested_name: &str,
) -> Result<Option<std::path::PathBuf>, CommandError> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Latent Cartridge", &["lc"])
        .set_file_name(suggested_name)
        .blocking_save_file();
    selected
        .map(|path| {
            path.into_path().map_err(|_| {
                CommandError::new(
                    "capture.output_path_invalid",
                    "The native save dialog did not return a usable Q4 output path.",
                )
            })
        })
        .transpose()
}

fn lock_backend(
    backend: &Arc<Mutex<Q4BackendController>>,
) -> Result<MutexGuard<'_, Q4BackendController>, Q4RuntimeError> {
    backend.lock().map_err(|_| Q4RuntimeError::state_poisoned())
}

fn runtime_inactive() -> CommandError {
    CommandError::new(
        "deck.runtime_unavailable",
        "Open four validated Library cartridges before controlling LD-Q4.",
    )
}

fn command_error(error: Q4RuntimeError) -> CommandError {
    record_global(LogLevel::Error, "deck.q4.command_failed", Some(&error.code));
    CommandError::new(error.code, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_command_has_no_path_field_and_rejects_bad_identity() {
        let decoded: Q4SourceIdentityInput = serde_json::from_value(serde_json::json!({
            "cartridgeId": "11111111-1111-4111-8111-111111111111",
            "archiveSha256": "a".repeat(64)
        }))
        .expect("identity only");
        source_identity(decoded).expect("canonical identity");

        let with_path = serde_json::from_value::<Q4SourceIdentityInput>(serde_json::json!({
            "cartridgeId": "11111111-1111-4111-8111-111111111111",
            "archiveSha256": "a".repeat(64),
            "path": "forbidden.lc"
        }));
        assert!(with_path.is_err());
    }

    #[test]
    fn command_error_never_exposes_a_machine_path() {
        let mapped = command_error(Q4RuntimeError::source_invalid());
        let json = serde_json::to_string(&mapped).expect("serialize error");
        assert!(!json.contains("C:\\\\"));
        assert!(!json.contains("W:\\\\"));
    }

    #[tokio::test]
    async fn resize_channel_coalesces_to_latest_dimensions() {
        let (sender, mut receiver) = watch::channel((0, 0));
        sender.send_replace((640, 360));
        sender.send_replace((1280, 720));
        receiver.changed().await.expect("sender remains live");
        assert_eq!(*receiver.borrow_and_update(), (1280, 720));
    }

    #[tokio::test]
    async fn runtime_slot_clone_and_take_release_mutex_before_owner_awaits() {
        let original = Arc::new(7_u8);
        let slot = AsyncMutex::new(Some(Arc::clone(&original)));

        let cloned = clone_slot(&slot).await.expect("runtime clone");
        assert!(Arc::ptr_eq(&cloned, &original));
        let guard = tokio::time::timeout(std::time::Duration::from_millis(10), slot.lock())
            .await
            .expect("clone must not retain runtime mutex");
        drop(guard);

        let taken = take_slot(&slot).await.expect("runtime owner");
        assert!(Arc::ptr_eq(&taken, &original));
        let guard = tokio::time::timeout(std::time::Duration::from_millis(10), slot.lock())
            .await
            .expect("take must release runtime mutex before shutdown awaits");
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn diagnostic_clone_releases_lifecycle_gate_before_actor_await() {
        let lifecycle = AsyncMutex::new(());
        let original = Arc::new(9_u8);
        let slot = AsyncMutex::new(Some(Arc::clone(&original)));

        let cloned = clone_diagnostic_slot(&lifecycle, &slot)
            .await
            .expect("runtime clone");
        assert!(Arc::ptr_eq(&cloned, &original));
        let guard = tokio::time::timeout(std::time::Duration::from_millis(10), lifecycle.lock())
            .await
            .expect("diagnostic actor await must not retain lifecycle gate");
        drop(guard);
    }
}
