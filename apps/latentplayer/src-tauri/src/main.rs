#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU8, Ordering},
};
use std::{path::PathBuf, time::SystemTime};

mod diagnostic_state;
mod native_output;
mod playback_runtime;

use latentdeck_core::{
    codec_pack::default_codec_pack_roots,
    diagnostics::{LogLevel, initialize_global_json_log, record_global},
    player::{PlayerCoordinator, PlayerCoordinatorError, PlayerView},
    realtime_diagnostics::RealtimeDiagnosticError,
};
use latentdeck_native_output::{HostFullscreenController, NativeSpoutStatus};
use playback_runtime::{PlaybackLaunchConfig, PlaybackRuntime, PlaybackRuntimeError};
use serde::Serialize;
use tauri::{AppHandle, Manager, RunEvent, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::Mutex as AsyncMutex;

use crate::diagnostic_state::{
    DiagnosticSaveResult, active_snapshot, inactive_snapshot, write_player_bundle,
};
use crate::native_output::{
    PlayerViewportStore, ViewportBoundsError, ViewportBoundsRequest, ViewportSessionAck,
    ViewportStoreError, validate_viewport_bounds,
};

const MAIN_WINDOW_LABEL: &str = "main";

struct AppState {
    player: Arc<Mutex<PlayerCoordinator>>,
    runtime: Arc<AsyncMutex<Option<PlaybackRuntime>>>,
    viewport: PlayerViewportStore,
    fullscreen: HostFullscreenController,
    exit_gate: ExitGate,
}

impl AppState {
    fn discover() -> Self {
        let player = PlayerCoordinator::discover_visible(
            &default_codec_pack_roots(),
            latentdeck_core::product_version(),
        );
        Self {
            player: Arc::new(Mutex::new(player)),
            runtime: Arc::new(AsyncMutex::new(None)),
            viewport: PlayerViewportStore::new(),
            fullscreen: HostFullscreenController::new(),
            exit_gate: ExitGate::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitRequest {
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandError {
    code: String,
    message: String,
    recoverable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FullscreenStatus {
    active: bool,
}

impl From<bool> for FullscreenStatus {
    fn from(active: bool) -> Self {
        Self { active }
    }
}

impl CommandError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::with_recoverability(code, message, true)
    }

    fn with_recoverability(
        code: impl Into<String>,
        message: impl Into<String>,
        recoverable: bool,
    ) -> Self {
        let code = code.into();
        record_global(LogLevel::Error, "player.command_failed", Some(&code));
        Self {
            code,
            message: message.into(),
            recoverable,
        }
    }

    fn runtime_inactive() -> Self {
        Self::new(
            "state.invalid_transition",
            "Start playback before controlling the native output.",
        )
    }
}

impl From<PlayerCoordinatorError> for CommandError {
    fn from(error: PlayerCoordinatorError) -> Self {
        Self::new(error.code, error.message)
    }
}

impl From<PlaybackRuntimeError> for CommandError {
    fn from(error: PlaybackRuntimeError) -> Self {
        Self::with_recoverability(error.code, error.message, error.recoverable)
    }
}

impl From<RealtimeDiagnosticError> for CommandError {
    fn from(error: RealtimeDiagnosticError) -> Self {
        match error {
            RealtimeDiagnosticError::OutputExists => Self::new(
                "diagnostics.output_exists",
                "The selected diagnostic archive already exists and was not overwritten. Choose a new file name.",
            ),
            RealtimeDiagnosticError::InvalidDestination => Self::new(
                "diagnostics.destination_invalid",
                "Choose a writable local folder and a .zip file name for the diagnostic archive.",
            ),
            RealtimeDiagnosticError::LimitExceeded(_) => Self::new(
                "diagnostics.limit_exceeded",
                "The bounded diagnostic evidence exceeded its safety limit; older logs can be removed before retrying.",
            ),
            RealtimeDiagnosticError::Io { .. } => Self::new(
                "diagnostics.write_failed",
                "The diagnostic archive could not be written. Check folder permissions and try another file name.",
            ),
            _ => Self::new(
                "diagnostics.contract_invalid",
                "LatentPlayer could not create a safe diagnostic snapshot from the current state.",
            ),
        }
    }
}

fn lock_player(
    player: &Arc<Mutex<PlayerCoordinator>>,
) -> Result<MutexGuard<'_, PlayerCoordinator>, CommandError> {
    player.lock().map_err(|_| {
        CommandError::new(
            "player.state_poisoned",
            "Player state is unavailable; restart LatentPlayer.",
        )
    })
}

fn launch_config(
    player: &Arc<Mutex<PlayerCoordinator>>,
) -> Result<PlaybackLaunchConfig, CommandError> {
    let player = lock_player(player)?;
    PlaybackLaunchConfig::from_player(&player).map_err(Into::into)
}

fn trusted_snapshot(player: &Arc<Mutex<PlayerCoordinator>>) -> Result<PlayerView, CommandError> {
    Ok(lock_player(player)?.view())
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow, CommandError> {
    app.get_webview_window(MAIN_WINDOW_LABEL).ok_or_else(|| {
        CommandError::new(
            "output.parent_window_unavailable",
            "LatentPlayer could not attach the native video surface to its main window.",
        )
    })
}

fn viewport_error(error: ViewportBoundsError) -> CommandError {
    let code = match error {
        ViewportBoundsError::NonFinite => "output.viewport_non_finite",
        ViewportBoundsError::InvalidScaleFactor => "output.viewport_scale_invalid",
        ViewportBoundsError::StaleScaleFactor => "output.viewport_scale_stale",
        ViewportBoundsError::InvalidExtent => "output.viewport_extent_invalid",
        ViewportBoundsError::OutsideClient => "output.viewport_outside_client",
        ViewportBoundsError::Overflow => "output.viewport_overflow",
    };
    CommandError::new(
        code,
        "LatentPlayer rejected an invalid embedded video-area measurement.",
    )
}

fn viewport_store_error(error: ViewportStoreError) -> CommandError {
    CommandError::new(error.code(), error.message())
}

async fn shutdown_runtime(runtime: &mut Option<PlaybackRuntime>) -> Result<(), CommandError> {
    let Some(runtime) = runtime.take() else {
        return Ok(());
    };
    runtime.shutdown().await.map_err(Into::into)
}

async fn start_runtime(
    app: &AppHandle,
    player: &Arc<Mutex<PlayerCoordinator>>,
    viewport: &PlayerViewportStore,
) -> Result<PlaybackRuntime, CommandError> {
    let config = launch_config(player)?;
    let viewport = viewport.current_visible().map_err(viewport_store_error)?;
    let parent = main_window(app)?;
    PlaybackRuntime::start(app.clone(), parent, Arc::clone(player), config, viewport)
        .await
        .map_err(Into::into)
}

async fn start_and_restart(
    app: &AppHandle,
    player: &Arc<Mutex<PlayerCoordinator>>,
    viewport: &PlayerViewportStore,
) -> Result<(PlaybackRuntime, PlayerView), CommandError> {
    let runtime = start_runtime(app, player, viewport).await?;
    match runtime.restart().await {
        Ok(view) => Ok((runtime, view)),
        Err(error) => {
            let _ = runtime.shutdown().await;
            Err(error.into())
        }
    }
}

#[tauri::command]
const fn product_version() -> &'static str {
    latentdeck_core::product_version()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
fn player_snapshot(state: State<'_, AppState>) -> Result<PlayerView, CommandError> {
    trusted_snapshot(&state.player)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractors own their values.
async fn player_viewport_set_bounds(
    app: AppHandle,
    state: State<'_, AppState>,
    bounds: ViewportBoundsRequest,
) -> Result<(), CommandError> {
    let window = main_window(&app)?;
    let scale_factor = window.scale_factor().map_err(|_| {
        CommandError::new(
            "output.viewport_scale_unavailable",
            "LatentPlayer could not read the current display scale.",
        )
    })?;
    let client = window.inner_size().map_err(|_| {
        CommandError::new(
            "output.viewport_client_unavailable",
            "LatentPlayer could not read the main window size.",
        )
    })?;
    let request = validate_viewport_bounds(bounds, scale_factor, client.width, client.height)
        .map_err(viewport_error)?;
    let viewport = state
        .viewport
        .apply(request)
        .map_err(viewport_store_error)?;

    let runtime = state.runtime.lock().await;
    if let Some(runtime) = runtime.as_ref() {
        let _ = runtime.set_viewport(viewport).await?;
    }
    state
        .viewport
        .confirm_applied(request, viewport)
        .map_err(viewport_store_error)?;
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn player_viewport_session_begin(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ViewportSessionAck, CommandError> {
    // Resolve the authoritative parent before mutating the epoch. An auxiliary
    // WebView must never select itself as the native child parent.
    let _parent = main_window(&app)?;
    let (session, hidden) = state
        .viewport
        .begin_session()
        .map_err(viewport_store_error)?;
    let runtime = state.runtime.lock().await;
    if let Some(runtime) = runtime.as_ref() {
        let _ = runtime.set_viewport(hidden).await?;
    }
    state
        .viewport
        .confirm_session(session, hidden)
        .map_err(viewport_store_error)?;
    Ok(session)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_open(state: State<'_, AppState>, path: String) -> Result<PlayerView, CommandError> {
    let mut runtime = state.runtime.lock().await;
    shutdown_runtime(&mut runtime).await?;
    let player = Arc::clone(&state.player);
    tauri::async_runtime::spawn_blocking(move || {
        lock_player(&player)?
            .open_cartridge(path)
            .map_err(Into::into)
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "player.validation_failed",
            "Cartridge validation task stopped unexpectedly.",
        )
    })?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_select_decoder(
    state: State<'_, AppState>,
    path: String,
) -> Result<PlayerView, CommandError> {
    let mut runtime = state.runtime.lock().await;
    shutdown_runtime(&mut runtime).await?;
    let player = Arc::clone(&state.player);
    tauri::async_runtime::spawn_blocking(move || {
        lock_player(&player)?
            .select_decoder_asset(path)
            .map_err(Into::into)
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "codec.asset_validation_failed",
            "Decoder asset validation task stopped unexpectedly.",
        )
    })?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_set_loop(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<PlayerView, CommandError> {
    let runtime = state.runtime.lock().await;
    if let Some(runtime) = runtime.as_ref() {
        match runtime.set_loop(enabled) {
            Ok(view) => return Ok(view),
            Err(error) if error.code == "player.runtime_unavailable" => {}
            Err(error) => return Err(error.into()),
        }
    }
    lock_player(&state.player)?
        .set_loop_enabled(enabled)
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractors own their values.
async fn player_play(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PlayerView, CommandError> {
    let mut runtime_slot = state.runtime.lock().await;
    if let Some(runtime) = runtime_slot.as_ref() {
        return runtime.play().await.map_err(Into::into);
    }

    let runtime = start_runtime(&app, &state.player, &state.viewport).await?;
    match runtime.play().await {
        Ok(view) => {
            *runtime_slot = Some(runtime);
            Ok(view)
        }
        Err(error) => {
            let _ = runtime.shutdown().await;
            Err(error.into())
        }
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_pause(state: State<'_, AppState>) -> Result<PlayerView, CommandError> {
    let runtime = state.runtime.lock().await;
    runtime
        .as_ref()
        .ok_or_else(CommandError::runtime_inactive)?
        .pause()
        .await
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractors own their values.
async fn player_restart(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<PlayerView, CommandError> {
    let mut runtime_slot = state.runtime.lock().await;
    if let Some(runtime) = runtime_slot.as_ref() {
        match runtime.restart().await {
            Ok(view) => return Ok(view),
            Err(error) if error.code == "player.runtime_unavailable" => {}
            Err(error) => return Err(error.into()),
        }
    }

    shutdown_runtime(&mut runtime_slot).await?;
    let (runtime, view) = start_and_restart(&app, &state.player, &state.viewport).await?;
    *runtime_slot = Some(runtime);
    Ok(view)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_fullscreen_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<FullscreenStatus>, CommandError> {
    let window = main_window(&app)?;
    state
        .fullscreen
        .status(&window)
        .await
        .map(FullscreenStatus::from)
        .map(Some)
        .map_err(|_| {
            CommandError::new(
                "output.window_fullscreen_failed",
                "LatentPlayer could not read the main window fullscreen state.",
            )
        })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_set_fullscreen(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<FullscreenStatus, CommandError> {
    let window = main_window(&app)?;
    state
        .fullscreen
        .set(&window, enabled)
        .await
        .map(FullscreenStatus::from)
        .map_err(|_| {
            CommandError::new(
                "output.window_fullscreen_failed",
                "LatentPlayer could not change or confirm the main window fullscreen state.",
            )
        })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_spout_status(
    state: State<'_, AppState>,
) -> Result<Option<NativeSpoutStatus>, CommandError> {
    let runtime = state.runtime.lock().await;
    let Some(runtime) = runtime.as_ref() else {
        return Ok(None);
    };
    runtime.spout_status().await.map(Some).map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_spout_configure(
    state: State<'_, AppState>,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<NativeSpoutStatus, CommandError> {
    let runtime = state.runtime.lock().await;
    runtime
        .as_ref()
        .ok_or_else(CommandError::runtime_inactive)?
        .configure_spout(name, enabled)
        .await
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractors own their values.
async fn player_save_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DiagnosticSaveResult, CommandError> {
    let suggested_name = format!(
        "latentplayer-diagnostics-{}.zip",
        current_unix_ms()? / 1_000
    );
    let selected = app
        .dialog()
        .file()
        .add_filter("LatentPlayer Diagnostic Bundle", &["zip"])
        .set_file_name(suggested_name)
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(DiagnosticSaveResult::Cancelled);
    };
    let destination = validate_diagnostic_destination(selected.into_path().map_err(|_| {
        CommandError::new(
            "diagnostics.destination_invalid",
            "The native save dialog did not return a usable diagnostic archive path.",
        )
    })?)?;

    let active = {
        let runtime = state.runtime.lock().await;
        match runtime.as_ref() {
            Some(runtime) => runtime.diagnostics().await?,
            None => None,
        }
    };
    let captured_at_unix_ms = current_unix_ms()?;
    let snapshot = match active {
        Some(diagnostics) => active_snapshot(captured_at_unix_ms, diagnostics)?,
        None => inactive_snapshot(captured_at_unix_ms, &trusted_snapshot(&state.player)?)?,
    };
    let player_log_root = app
        .path()
        .app_local_data_dir()
        .map_err(|_| {
            CommandError::new(
                "diagnostics.log_root_unavailable",
                "LatentPlayer could not resolve its installed diagnostic log folder.",
            )
        })?
        .join("logs");
    let worker_log_root = std::env::temp_dir()
        .join("LatentDeck")
        .join("worker-diagnostics");

    let receipt = tauri::async_runtime::spawn_blocking(move || {
        write_player_bundle(&destination, &snapshot, &player_log_root, &worker_log_root)
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "diagnostics.task_failed",
            "The diagnostic archive task stopped unexpectedly; retry with a new file name.",
        )
    })??;
    record_global(LogLevel::Info, "diagnostics.bundle_saved", None);
    Ok(receipt.into())
}

fn current_unix_ms() -> Result<u64, CommandError> {
    let milliseconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| {
            CommandError::new(
                "diagnostics.clock_invalid",
                "The system clock cannot represent a diagnostic timestamp.",
            )
        })?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| {
        CommandError::new(
            "diagnostics.clock_invalid",
            "The system clock cannot represent a diagnostic timestamp.",
        )
    })
}

fn validate_diagnostic_destination(mut path: PathBuf) -> Result<PathBuf, CommandError> {
    if !path.is_absolute() {
        return Err(CommandError::new(
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
            return Err(CommandError::new(
                "diagnostics.destination_invalid",
                "The diagnostic archive file name must use the .zip extension.",
            ));
        }
    }
    Ok(path)
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::discover())
        .setup(|app| {
            let app_data_dir = app.path().app_local_data_dir()?;
            if initialize_global_json_log(&app_data_dir.join("logs"), "latentplayer").is_ok() {
                record_global(LogLevel::Info, "app.started", None);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            product_version,
            player_snapshot,
            player_viewport_session_begin,
            player_viewport_set_bounds,
            player_open,
            player_select_decoder,
            player_set_loop,
            player_play,
            player_pause,
            player_restart,
            player_fullscreen_status,
            player_set_fullscreen,
            player_spout_status,
            player_spout_configure,
            player_save_diagnostics,
        ])
        .build(tauri::generate_context!())
        .expect("LatentPlayer application runtime failed");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            record_global(LogLevel::Info, "app.exit_requested", None);
            let state = app_handle.state::<AppState>();
            match state.exit_gate.request() {
                ExitRequest::BeginShutdown => {
                    api.prevent_exit();
                    let runtime = Arc::clone(&state.runtime);
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut runtime = runtime.lock().await;
                        let _ = shutdown_runtime(&mut runtime).await;
                        app_handle.state::<AppState>().exit_gate.mark_ready();
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
    fn diagnostic_destination_is_absolute_and_gets_a_zip_extension() {
        let destination = std::env::temp_dir().join("latentplayer-support");
        let validated = validate_diagnostic_destination(destination).expect("destination");

        assert!(validated.is_absolute());
        assert_eq!(
            validated.extension().and_then(|value| value.to_str()),
            Some("zip")
        );
    }

    #[test]
    fn diagnostic_destination_rejects_a_different_extension() {
        let destination = std::env::temp_dir().join("latentplayer-support.txt");
        let error = validate_diagnostic_destination(destination).expect_err("must reject");

        assert_eq!(error.code, "diagnostics.destination_invalid");
        assert!(error.recoverable);
    }

    #[test]
    fn output_collision_error_is_recoverable_and_path_free() {
        let error = CommandError::from(RealtimeDiagnosticError::OutputExists);
        let value = serde_json::to_value(error).expect("serialize");

        assert_eq!(value["code"], "diagnostics.output_exists");
        assert_eq!(value["recoverable"], true);
        let json = value.to_string();
        assert!(!json.contains("C:\\"));
        assert!(!json.contains("W:\\"));
    }

    #[test]
    fn fullscreen_status_serializes_the_confirmed_native_state() {
        let active = serde_json::to_value(FullscreenStatus::from(true)).expect("serialize");
        let inactive = serde_json::to_value(FullscreenStatus::from(false)).expect("serialize");

        assert_eq!(active, serde_json::json!({ "active": true }));
        assert_eq!(inactive, serde_json::json!({ "active": false }));
    }

    #[test]
    fn exit_gate_prevents_repeated_close_until_shutdown_is_ready() {
        let gate = ExitGate::new();
        assert_eq!(gate.request(), ExitRequest::BeginShutdown);
        assert_eq!(gate.request(), ExitRequest::WaitForShutdown);
        gate.mark_ready();
        assert_eq!(gate.request(), ExitRequest::AllowExit);
    }
}
