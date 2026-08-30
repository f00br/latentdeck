use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};

mod native_output;
mod playback_runtime;

use latentdeck_core::{
    codec_pack::default_codec_pack_roots,
    player::{PlayerCoordinator, PlayerCoordinatorError, PlayerView},
};
use latentdeck_native_output::NativeSpoutStatus;
use playback_runtime::{PlaybackLaunchConfig, PlaybackRuntime, PlaybackRuntimeError};
use serde::Serialize;
use tauri::{AppHandle, Manager, RunEvent, State, WindowEvent};
use tokio::sync::Mutex as AsyncMutex;

use crate::native_output::NATIVE_OUTPUT_WINDOW_LABEL;

struct AppState {
    player: Arc<Mutex<PlayerCoordinator>>,
    runtime: Arc<AsyncMutex<Option<PlaybackRuntime>>>,
    exit_started: AtomicBool,
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
            exit_started: AtomicBool::new(false),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandError {
    code: String,
    message: String,
}

impl CommandError {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
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
        Self::new(error.code, error.message)
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

async fn shutdown_runtime(runtime: &mut Option<PlaybackRuntime>) -> Result<(), CommandError> {
    let Some(runtime) = runtime.take() else {
        return Ok(());
    };
    runtime.shutdown().await.map_err(Into::into)
}

async fn start_runtime(
    app: &AppHandle,
    player: &Arc<Mutex<PlayerCoordinator>>,
) -> Result<PlaybackRuntime, CommandError> {
    let config = launch_config(player)?;
    PlaybackRuntime::start(app.clone(), Arc::clone(player), config)
        .await
        .map_err(Into::into)
}

async fn start_and_restart(
    app: &AppHandle,
    player: &Arc<Mutex<PlayerCoordinator>>,
) -> Result<(PlaybackRuntime, PlayerView), CommandError> {
    let runtime = start_runtime(app, player).await?;
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

    let runtime = start_runtime(&app, &state.player).await?;
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
    let (runtime, view) = start_and_restart(&app, &state.player).await?;
    *runtime_slot = Some(runtime);
    Ok(view)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command extractor owns `State`.
async fn player_fullscreen(state: State<'_, AppState>) -> Result<PlayerView, CommandError> {
    let runtime = state.runtime.lock().await;
    let runtime = runtime
        .as_ref()
        .ok_or_else(CommandError::runtime_inactive)?;
    let _fullscreen = runtime.toggle_fullscreen().await?;
    trusted_snapshot(&state.player)
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

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::discover())
        .on_window_event(|window, event| {
            if window.label() != NATIVE_OUTPUT_WINDOW_LABEL {
                return;
            }

            match event {
                WindowEvent::Resized(size) => {
                    let width = size.width;
                    let height = size.height;
                    let runtime = Arc::clone(&window.state::<AppState>().runtime);
                    tauri::async_runtime::spawn(async move {
                        let runtime = runtime.lock().await;
                        if let Some(runtime) = runtime.as_ref() {
                            let _ = runtime.resize(width, height).await;
                        }
                    });
                }
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let runtime = Arc::clone(&window.state::<AppState>().runtime);
                    let native_window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut runtime = runtime.lock().await;
                        let _ = shutdown_runtime(&mut runtime).await;
                        let _ = native_window.destroy();
                    });
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            product_version,
            player_snapshot,
            player_open,
            player_select_decoder,
            player_set_loop,
            player_play,
            player_pause,
            player_restart,
            player_fullscreen,
            player_spout_status,
            player_spout_configure,
        ])
        .build(tauri::generate_context!())
        .expect("LatentPlayer application runtime failed");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            let state = app_handle.state::<AppState>();
            if state.exit_started.swap(true, Ordering::AcqRel) {
                return;
            }

            api.prevent_exit();
            let runtime = Arc::clone(&state.runtime);
            let app_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let mut runtime = runtime.lock().await;
                let _ = shutdown_runtime(&mut runtime).await;
                app_handle.exit(code.unwrap_or_default());
            });
        }
    });
}
