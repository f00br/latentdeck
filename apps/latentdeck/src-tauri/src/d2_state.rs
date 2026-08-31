//! Tauri command boundary for the backend-owned LD-D2 runtime.

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU8, Ordering},
};

use latentdeck_core::diagnostics::{LogLevel, record_global};
use latentdeck_library::{CartridgeKey, DeckSourceIdentity, ResolvedDeckSource};
use latentdeck_native_output::{HostFullscreenController, NativeSpoutStatus};
use latentdeck_output_mp4::RecorderStatus;
use serde::Deserialize;
use tauri::{AppHandle, Emitter as _, Manager as _, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::Mutex as AsyncMutex;

use crate::{
    d2_capture_host::D2CaptureView,
    d2_runtime::{
        D2BackendController, D2BackendView, D2CaptureHostServices, D2ControlsAckView,
        D2ControlsInput, D2LaunchConfig, D2Runtime, D2RuntimeDiagnostics, D2RuntimeError,
        D2SeedAckView, D2StatusView, D2TransportAckView, D2TransportInput,
        validate_selected_decoder,
    },
    decoded_recording::{
        DecodedRecordingController, DecodedRecordingError, ensure_latent_capture_idle,
        normalize_mp4_destination,
    },
    diagnostic_state::DeckDiagnosticLifecycle,
    embedded_viewport::{
        EmbeddedViewportStore, ViewportBoundsRequest, ViewportSessionAck, validate_viewport_bounds,
        viewport_error,
    },
    library_state::{AppState as LibraryAppState, CommandError, DeckKind},
    runtime_replacement::preflight_before_shutdown,
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
    recording: DecodedRecordingController,
    capture_gate: AsyncMutex<()>,
    exit_gate: ExitGate,
    viewport: EmbeddedViewportStore,
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
        Self {
            backend: Arc::new(Mutex::new(D2BackendController::discover_default())),
            runtime: AsyncMutex::new(None),
            lifecycle: AsyncMutex::new(()),
            status: Arc::new(Mutex::new(D2StatusView::default())),
            capture_status: Arc::new(Mutex::new(D2CaptureView::default())),
            recording: DecodedRecordingController::new(),
            capture_gate: AsyncMutex::new(()),
            exit_gate: ExitGate::new(),
            viewport: EmbeddedViewportStore::new(),
        }
    }

    pub(crate) fn request_exit(&self) -> ExitRequest {
        self.exit_gate.request()
    }

    pub(crate) fn mark_exit_ready(&self) {
        self.exit_gate.mark_ready();
    }

    pub(crate) async fn shutdown_runtime(&self) -> Result<(), D2RuntimeError> {
        let _lifecycle = self.lifecycle.lock().await;
        // Serialize exit with the native save dialog and recorder arm. Either
        // an in-flight start is stopped here, or a later start observes that
        // the runtime has already been removed.
        let _capture_gate = self.capture_gate.lock().await;
        let runtime_result = shutdown_runtime_slot(&self.runtime).await;
        let recording = self.recording.clone();
        match tauri::async_runtime::spawn_blocking(move || recording.stop()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => record_global(
                LogLevel::Error,
                "deck.d2.recording_shutdown_failed",
                Some(error.code()),
            ),
            Err(_) => record_global(
                LogLevel::Error,
                "deck.d2.recording_shutdown_failed",
                Some("recording.worker_stopped"),
            ),
        }
        runtime_result
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
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_pass_by_value
)]
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
    let parent = main_window(&app)?;
    let viewport = state.viewport.current_visible()?;
    let controls = controls.into_wire().map_err(command_error)?;
    let transport = transport.into();
    if seed > latentdeck_control::MAX_D2_SAFE_INTEGER {
        return Err(command_error(D2RuntimeError::invalid_seed()));
    }
    let identity_a = source_identity(source_a)?;
    let identity_b = source_identity(source_b)?;
    let _lifecycle = state.lifecycle.lock().await;
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
    // Resolving identities and building the launch config are the complete
    // candidate preflight. Keep the current good runtime alive until all of
    // that succeeds, then transfer runtime ownership at one explicit boundary.
    let config = preflight_before_shutdown(async { Ok::<_, CommandError>(config) }, async {
        shutdown_runtime_slot(&state.runtime)
            .await
            .map_err(|error| {
                D2AppState::emit_error(&app, &error);
                command_error(error)
            })
    })
    .await?;
    let deck_session = library.begin_deck_session(DeckKind::D2)?;
    let started = D2Runtime::start(
        app.clone(),
        parent,
        Arc::clone(&state.status),
        Arc::clone(&state.capture_status),
        state.recording.clone(),
        config,
        deck_session.clone(),
        viewport,
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
    replace_slot(&state.runtime, Arc::clone(&started)).await;
    // Viewport updates can arrive while the worker and renderer are starting,
    // before the runtime is published into the state slot. Re-apply the
    // authoritative latest revision after publication so none are lost.
    let latest_viewport = match state.viewport.current() {
        Ok(viewport) => viewport,
        Err(error) => {
            let _ = shutdown_runtime_slot(&state.runtime).await;
            return Err(error);
        }
    };
    if let Err(error) = started.set_viewport(latest_viewport).await {
        let _ = shutdown_runtime_slot(&state.runtime).await;
        return Err(command_error(error));
    }
    if let Err(error) = deck_session.publish(slot_bindings) {
        let _ = shutdown_runtime_slot(&state.runtime).await;
        return Err(error);
    }
    Ok(view)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_viewport_set_bounds(
    app: AppHandle,
    state: State<'_, D2AppState>,
    bounds: ViewportBoundsRequest,
) -> Result<(), CommandError> {
    let window = main_window(&app)?;
    let scale_factor = window.scale_factor().map_err(|_| {
        CommandError::new(
            "output.viewport_scale_unavailable",
            "LatentDeck could not read the main-window display scale.",
        )
    })?;
    let client = window.inner_size().map_err(|_| {
        CommandError::new(
            "output.viewport_client_unavailable",
            "LatentDeck could not read the main-window client size.",
        )
    })?;
    let request = validate_viewport_bounds(bounds, scale_factor, client.width, client.height)
        .map_err(viewport_error)?;
    let viewport = state.viewport.apply(request)?;
    if let Some(runtime) = clone_slot(&state.runtime).await {
        runtime
            .set_viewport(viewport)
            .await
            .map_err(command_error)?;
    }
    state.viewport.confirm_applied(request, viewport)?;
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_viewport_session_begin(
    app: AppHandle,
    state: State<'_, D2AppState>,
) -> Result<ViewportSessionAck, CommandError> {
    // Resolve the authoritative parent before mutating the epoch. A future
    // auxiliary WebView must never select the child-output parent by invoking
    // this command itself.
    let _parent = main_window(&app)?;
    let (session, hidden) = state.viewport.begin_session()?;
    if let Some(runtime) = clone_slot(&state.runtime).await {
        runtime.set_viewport(hidden).await.map_err(command_error)?;
    }
    state.viewport.confirm_session(session, hidden)?;
    Ok(session)
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
    let _capture_gate = state.capture_gate.lock().await;
    ensure_recording_idle_for_capture(&state.recording)?;
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
    let _capture_gate = state.capture_gate.lock().await;
    ensure_recording_idle_for_capture(&state.recording)?;
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

/// Select a no-clobber destination and arm video-only H.264 MP4 recording.
/// Geometry is fixed by the next successfully presented decoded frame.
#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_recording_start(
    app: AppHandle,
    state: State<'_, D2AppState>,
) -> Result<RecorderStatus, CommandError> {
    let _capture_gate = state.capture_gate.lock().await;
    if clone_slot(&state.runtime).await.is_none() {
        return Err(runtime_inactive());
    }
    let capture = state.shared_capture_status().map_err(command_error)?;
    ensure_latent_capture_idle(&capture.state).map_err(recording_command_error)?;
    if state.recording.is_active() {
        return Err(recording_command_error(DecodedRecordingError::Active));
    }
    let selected = app
        .dialog()
        .file()
        .add_filter("H.264 MP4 Video", &["mp4"])
        .set_file_name("LatentDeck D2 Output.mp4")
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(state.recording.status());
    };
    let destination = selected.into_path().map_err(|_| {
        recording_command_error(DecodedRecordingError::Recorder(
            latentdeck_output_mp4::RecorderError::InvalidDestination,
        ))
    })?;
    let destination = normalize_mp4_destination(destination).map_err(recording_command_error)?;
    if clone_slot(&state.runtime).await.is_none() {
        return Err(runtime_inactive());
    }
    state
        .recording
        .arm(destination)
        .map_err(recording_command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_recording_stop(
    state: State<'_, D2AppState>,
) -> Result<RecorderStatus, CommandError> {
    let recording = state.recording.clone();
    tauri::async_runtime::spawn_blocking(move || recording.stop())
        .await
        .map_err(|_| {
            recording_command_error(DecodedRecordingError::Recorder(
                latentdeck_output_mp4::RecorderError::WorkerStopped,
            ))
        })?
        .map_err(recording_command_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn deck_d2_recording_status_get(state: State<'_, D2AppState>) -> RecorderStatus {
    state.recording.status()
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
    app: AppHandle,
    fullscreen: State<'_, HostFullscreenController>,
) -> Result<Option<bool>, CommandError> {
    fullscreen
        .status(&main_window(&app)?)
        .await
        .map(Some)
        .map_err(|_| fullscreen_error())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_d2_fullscreen_set(
    app: AppHandle,
    fullscreen: State<'_, HostFullscreenController>,
    enabled: bool,
) -> Result<bool, CommandError> {
    let window = main_window(&app)?;
    fullscreen
        .set(&window, enabled)
        .await
        .map_err(|_| fullscreen_error())
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

fn main_window(app: &AppHandle) -> Result<WebviewWindow, CommandError> {
    app.get_webview_window("main").ok_or_else(|| {
        CommandError::new(
            "output.main_window_unavailable",
            "The LatentDeck main window is unavailable.",
        )
    })
}

fn fullscreen_error() -> CommandError {
    CommandError::new(
        "output.window_fullscreen_failed",
        "LatentDeck could not change or confirm the main-window fullscreen state.",
    )
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

fn ensure_recording_idle_for_capture(
    recording: &DecodedRecordingController,
) -> Result<(), CommandError> {
    if recording.is_active() {
        Err(CommandError::new(
            "capture.recording_conflict",
            "Stop decoded MP4 recording before starting latent Snapshot or Live Capture.",
        ))
    } else {
        Ok(())
    }
}

fn recording_command_error(error: DecodedRecordingError) -> CommandError {
    record_global(
        LogLevel::Error,
        "deck.d2.recording_failed",
        Some(error.code()),
    );
    CommandError::new(error.code(), error.message())
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
    fn fullscreen_error_uses_the_shared_host_code() {
        let value = serde_json::to_value(fullscreen_error()).expect("serialize error");
        assert_eq!(value["code"], "output.window_fullscreen_failed");
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
