//! Tauri command boundary for the backend-owned LD-D2 runtime.

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU8, Ordering},
};

use latentdeck_core::diagnostics::{LogLevel, record_global};
use latentdeck_library::{CartridgeKey, DeckSourceIdentity, ResolvedDeckSource};
use latentdeck_native_output::NativeSpoutStatus;
use serde::Deserialize;
use tauri::{AppHandle, Emitter as _, Manager as _, State};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::{
    d2_capture_host::D2CaptureView,
    d2_runtime::{
        D2BackendController, D2BackendView, D2CaptureHostServices, D2ControlsAckView,
        D2ControlsInput, D2LaunchConfig, D2Runtime, D2RuntimeDiagnostics, D2RuntimeError,
        D2SeedAckView, D2StatusView, D2TransportAckView, D2TransportInput,
        validate_selected_decoder,
    },
    diagnostic_state::DeckDiagnosticLifecycle,
    library_state::{AppState as LibraryAppState, CommandError, DeckKind},
};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct D2SourceIdentityInput {
    cartridge_id: String,
    archive_sha256: String,
}

pub(crate) struct D2AppState {
    backend: Arc<Mutex<D2BackendController>>,
    runtime: AsyncMutex<Option<Arc<D2Runtime>>>,
    lifecycle: AsyncMutex<()>,
    status: Arc<Mutex<D2StatusView>>,
    capture_status: Arc<Mutex<D2CaptureView>>,
    exit_gate: ExitGate,
    resize_sender: watch::Sender<(u32, u32)>,
    resize_receiver: Mutex<Option<watch::Receiver<(u32, u32)>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitRequest {
    BeginShutdown,
    WaitForShutdown,
    AllowExit,
}

const EXIT_IDLE: u8 = 0;
const EXIT_SHUTTING_DOWN: u8 = 1;
const EXIT_READY: u8 = 2;

struct ExitGate {
    phase: AtomicU8,
}

impl ExitGate {
    const fn new() -> Self {
        Self {
            phase: AtomicU8::new(EXIT_IDLE),
        }
    }

    fn request(&self) -> ExitRequest {
        loop {
            match self.phase.load(Ordering::Acquire) {
                EXIT_IDLE => {
                    if self
                        .phase
                        .compare_exchange(
                            EXIT_IDLE,
                            EXIT_SHUTTING_DOWN,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return ExitRequest::BeginShutdown;
                    }
                }
                EXIT_READY => return ExitRequest::AllowExit,
                _ => return ExitRequest::WaitForShutdown,
            }
        }
    }

    fn mark_ready(&self) {
        self.phase.store(EXIT_READY, Ordering::Release);
    }
}

impl D2AppState {
    pub(crate) fn discover() -> Self {
        let (resize_sender, resize_receiver) = watch::channel((0, 0));
        Self {
            backend: Arc::new(Mutex::new(D2BackendController::discover_default())),
            runtime: AsyncMutex::new(None),
            lifecycle: AsyncMutex::new(()),
            status: Arc::new(Mutex::new(D2StatusView::default())),
            capture_status: Arc::new(Mutex::new(D2CaptureView::default())),
            exit_gate: ExitGate::new(),
            resize_sender,
            resize_receiver: Mutex::new(Some(resize_receiver)),
        }
    }

    pub(crate) fn request_exit(&self) -> ExitRequest {
        self.exit_gate.request()
    }

    pub(crate) fn mark_exit_ready(&self) {
        self.exit_gate.mark_ready();
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
                    app.state::<D2AppState>().resize(width, height).await;
                }
            }
        });
    }

    pub(crate) fn queue_resize(&self, width: u32, height: u32) {
        self.resize_sender.send_replace((width, height));
    }

    pub(crate) async fn shutdown_runtime(&self) -> Result<(), D2RuntimeError> {
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
    ) -> Result<Option<D2RuntimeDiagnostics>, D2RuntimeError> {
        match clone_diagnostic_slot(&self.lifecycle, &self.runtime).await {
            Some(runtime) => runtime.diagnostics().await,
            None => Ok(None),
        }
    }

    fn emit_error(app: &AppHandle, error: &D2RuntimeError) {
        if let Some(lifecycle) = app.try_state::<DeckDiagnosticLifecycle>() {
            lifecycle.record_error(&error.code);
        }
        let _ = app.emit("deck-d2-error", error.event());
    }

    fn backend_view(&self) -> Result<D2BackendView, D2RuntimeError> {
        Ok(lock_backend(&self.backend)?.view())
    }

    fn shared_status(&self) -> Result<D2StatusView, D2RuntimeError> {
        self.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| D2RuntimeError::state_poisoned())
    }

    fn shared_capture_status(&self) -> Result<D2CaptureView, D2RuntimeError> {
        self.capture_status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| D2RuntimeError::state_poisoned())
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn deck_d2_backend_status_get(
    state: State<'_, D2AppState>,
) -> Result<D2BackendView, CommandError> {
    state.backend_view().map_err(command_error)
}

/// Re-run bounded physical Codec Pack discovery so an installation performed
/// after app startup becomes available without restarting `LatentDeck`.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_backend_rediscover(
    state: State<'_, D2AppState>,
) -> Result<D2BackendView, CommandError> {
    let discovered = tauri::async_runtime::spawn_blocking(D2BackendController::discover_default)
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

/// Open a native file picker and validate the chosen external TAEH3 asset.
/// There is intentionally no path argument on this command.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_select_decoder(
    app: AppHandle,
    state: State<'_, D2AppState>,
) -> Result<D2BackendView, CommandError> {
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
        D2AppState::emit_error(&app, &error);
        return Err(command_error(error));
    }
    let pack = match state
        .backend_view()
        .and_then(|_| lock_backend(&state.backend).and_then(|backend| backend.pack_for_selection()))
    {
        Ok(pack) => pack,
        Err(error) => {
            D2AppState::emit_error(&app, &error);
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
            D2AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let mut backend = lock_backend(&state.backend).map_err(command_error)?;
    Ok(backend.accept_decoder(validated))
}

#[tauri::command]
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_open(
    app: AppHandle,
    state: State<'_, D2AppState>,
    library: State<'_, LibraryAppState>,
    source_a: D2SourceIdentityInput,
    source_b: D2SourceIdentityInput,
    controls: D2ControlsInput,
    transport: D2TransportInput,
    seed: u64,
) -> Result<D2StatusView, CommandError> {
    let controls = controls.into_wire().map_err(command_error)?;
    let transport = transport.into();
    if seed > latentdeck_control::MAX_D2_SAFE_INTEGER {
        return Err(command_error(D2RuntimeError::invalid_seed()));
    }
    let identity_a = source_identity(source_a)?;
    let identity_b = source_identity(source_b)?;
    let _lifecycle = state.lifecycle.lock().await;
    if let Err(error) = shutdown_runtime_slot(&state.runtime).await {
        D2AppState::emit_error(&app, &error);
        return Err(command_error(error));
    }
    let backend = match lock_backend(&state.backend).and_then(|backend| backend.launch_backend()) {
        Ok(value) => value,
        Err(error) => {
            D2AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let source_a = library.resolve_deck_source(identity_a).await.map_err(|_| {
        CommandError::new(
            "deck.source_invalid",
            "LD-D2 source A is not a present, unchanged Library cartridge.",
        )
    })?;
    let source_b = library.resolve_deck_source(identity_b).await.map_err(|_| {
        CommandError::new(
            "deck.source_invalid",
            "LD-D2 source B is not a present, unchanged Library cartridge.",
        )
    })?;
    let slot_bindings = d2_slot_bindings(&source_a, &source_b);
    let app_local_data = app.path().app_local_data_dir().map_err(|_| {
        CommandError::new(
            "capture.spool_root_invalid",
            "The app-local capture storage root is unavailable.",
        )
    })?;
    let library_importer = library.importer();
    let capture_host = D2CaptureHostServices::new(app_local_data, library_importer);
    let config = tauri::async_runtime::spawn_blocking(move || {
        D2LaunchConfig::build(
            backend,
            &source_a,
            &source_b,
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
            "LD-D2 source validation task stopped unexpectedly.",
        )
    })?;
    let config = match config {
        Ok(value) => value,
        Err(error) => {
            D2AppState::emit_error(&app, &error);
            return Err(command_error(error));
        }
    };
    let deck_session = library.begin_deck_session(DeckKind::D2)?;
    let started = D2Runtime::start(
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
            D2AppState::emit_error(&app, &error);
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
pub(crate) async fn deck_d2_controls_set(
    state: State<'_, D2AppState>,
    controls: D2ControlsInput,
) -> Result<D2ControlsAckView, CommandError> {
    let controls = controls.into_wire().map_err(command_error)?;
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.controls_set(controls).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_transport_set(
    state: State<'_, D2AppState>,
    transport: D2TransportInput,
) -> Result<D2TransportAckView, CommandError> {
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
pub(crate) async fn deck_d2_seed_set(
    state: State<'_, D2AppState>,
    seed: u64,
) -> Result<D2SeedAckView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.seed_set(seed).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_restart(
    state: State<'_, D2AppState>,
) -> Result<D2StatusView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.restart().await.map_err(command_error)
}

/// Run one full-carrier Snapshot after a native save selection. No path is
/// accepted from or returned to the webview.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_capture_snapshot(
    app: AppHandle,
    state: State<'_, D2AppState>,
) -> Result<Option<D2CaptureView>, CommandError> {
    let Some(output) = capture_output_path(&app, "LatentDeck Snapshot.lc")? else {
        return Ok(None);
    };
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime
        .capture_start(latentdeck_control::D2CaptureMode::Snapshot, output)
        .await
        .map(Some)
        .map_err(command_error)
}

/// Start a bounded Live Capture after a native save selection. Completion is
/// actor-owned and reported through path-free status/events.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_capture_live_start(
    app: AppHandle,
    state: State<'_, D2AppState>,
) -> Result<Option<D2CaptureView>, CommandError> {
    let Some(output) = capture_output_path(&app, "LatentDeck Live Capture.lc")? else {
        return Ok(None);
    };
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime
        .capture_start(latentdeck_control::D2CaptureMode::LiveCapture, output)
        .await
        .map(Some)
        .map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_capture_live_stop(
    state: State<'_, D2AppState>,
) -> Result<D2CaptureView, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.capture_stop().await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_capture_status_get(
    state: State<'_, D2AppState>,
) -> Result<D2CaptureView, CommandError> {
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
pub(crate) async fn deck_d2_status_get(
    state: State<'_, D2AppState>,
) -> Result<D2StatusView, CommandError> {
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
pub(crate) async fn deck_d2_fullscreen_status_get(
    state: State<'_, D2AppState>,
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
pub(crate) async fn deck_d2_fullscreen_set(
    state: State<'_, D2AppState>,
    enabled: bool,
) -> Result<bool, CommandError> {
    let runtime = clone_slot(&state.runtime)
        .await
        .ok_or_else(runtime_inactive)?;
    runtime.set_fullscreen(enabled).await.map_err(command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_spout_status_get(
    state: State<'_, D2AppState>,
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
pub(crate) async fn deck_d2_spout_configure(
    state: State<'_, D2AppState>,
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

async fn shutdown_runtime_slot(
    runtime: &AsyncMutex<Option<Arc<D2Runtime>>>,
) -> Result<(), D2RuntimeError> {
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

fn source_identity(input: D2SourceIdentityInput) -> Result<DeckSourceIdentity, CommandError> {
    DeckSourceIdentity::new(
        input.cartridge_id,
        CartridgeKey::new_unchecked(input.archive_sha256),
    )
    .map_err(|_| {
        CommandError::new(
            "deck.source_identity_invalid",
            "LD-D2 source identity must contain a canonical UUID and lowercase SHA-256.",
        )
    })
}

fn d2_slot_bindings(
    source_a: &ResolvedDeckSource,
    source_b: &ResolvedDeckSource,
) -> [(&'static str, CartridgeKey); 2] {
    [
        ("A", source_a.identity().archive_sha256().clone()),
        ("B", source_b.identity().archive_sha256().clone()),
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
                    "The native save dialog did not return a usable output path.",
                )
            })
        })
        .transpose()
}

fn lock_backend(
    backend: &Arc<Mutex<D2BackendController>>,
) -> Result<MutexGuard<'_, D2BackendController>, D2RuntimeError> {
    backend.lock().map_err(|_| D2RuntimeError::state_poisoned())
}

fn runtime_inactive() -> CommandError {
    CommandError::new(
        "deck.runtime_unavailable",
        "Open two validated cartridges before controlling LD-D2.",
    )
}

fn command_error(error: D2RuntimeError) -> CommandError {
    record_global(LogLevel::Error, "deck.d2.command_failed", Some(&error.code));
    CommandError::new(error.code, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_command_has_no_path_field_and_rejects_bad_identity() {
        let decoded: D2SourceIdentityInput = serde_json::from_value(serde_json::json!({
            "cartridgeId": "11111111-1111-4111-8111-111111111111",
            "archiveSha256": "a".repeat(64)
        }))
        .expect("identity only");
        source_identity(decoded).expect("canonical identity");

        let with_path = serde_json::from_value::<D2SourceIdentityInput>(serde_json::json!({
            "cartridgeId": "11111111-1111-4111-8111-111111111111",
            "archiveSha256": "a".repeat(64),
            "path": "forbidden.lc"
        }));
        assert!(with_path.is_err());
    }

    #[test]
    fn command_error_never_exposes_a_machine_path() {
        let mapped = command_error(D2RuntimeError::source_invalid());
        let json = serde_json::to_string(&mapped).expect("serialize error");
        assert!(!json.contains("C:\\\\"));
        assert!(!json.contains("W:\\\\"));
    }

    #[test]
    fn exit_gate_prevents_every_request_until_cleanup_is_ready() {
        let gate = ExitGate::new();
        assert_eq!(gate.request(), ExitRequest::BeginShutdown);
        assert_eq!(gate.request(), ExitRequest::WaitForShutdown);
        gate.mark_ready();
        assert_eq!(gate.request(), ExitRequest::AllowExit);
    }

    #[tokio::test]
    async fn resize_channel_coalesces_to_the_latest_dimensions() {
        let (sender, mut receiver) = watch::channel((0, 0));
        sender.send_replace((640, 360));
        sender.send_replace((1280, 720));
        receiver.changed().await.expect("sender remains live");
        assert_eq!(*receiver.borrow_and_update(), (1280, 720));
    }

    #[tokio::test]
    async fn runtime_slot_clone_and_take_release_the_mutex_before_owner_awaits() {
        let original = Arc::new(7_u8);
        let slot = AsyncMutex::new(Some(Arc::clone(&original)));

        let cloned = clone_slot(&slot).await.expect("runtime clone");
        assert!(Arc::ptr_eq(&cloned, &original));
        let guard = tokio::time::timeout(std::time::Duration::from_millis(10), slot.lock())
            .await
            .expect("clone must not retain the runtime mutex");
        drop(guard);

        let taken = take_slot(&slot).await.expect("runtime owner");
        assert!(Arc::ptr_eq(&taken, &original));
        let guard = tokio::time::timeout(std::time::Duration::from_millis(10), slot.lock())
            .await
            .expect("take must release the runtime mutex before shutdown awaits");
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
