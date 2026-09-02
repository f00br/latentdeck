#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicU8, Ordering},
};
use std::{path::PathBuf, time::SystemTime};

mod conversion;
mod diagnostic_state;
mod native_output;
mod playback_runtime;
mod playback_runtime_v2;
mod player_protocol2;
mod player_selection_v2;
mod raw_import_runtime;

use latentdeck_control::v2::DeviceKind;
use latentdeck_core::{
    diagnostics::{LogLevel, initialize_global_json_log, record_global},
    player::{PlayerCoordinator, PlayerCoordinatorError, PlayerView},
    realtime_diagnostics::RealtimeDiagnosticError,
};
use latentdeck_extension_manager::{
    CompatibilityPair, CompatibilityReason, ErrorCode as ExtensionErrorCode, ExtensionError,
    ExtensionRoots, InstallRequest, InstalledPackageSummary, PackageHealth, PackageKind,
    PackageReference, PublisherIdentityClaim, RemoveOptions, disable, enable,
    inspect as inspect_extension_archive, install, inventory as extension_inventory, remove,
    repair, verify,
};
use latentdeck_native_output::{HostFullscreenController, NativeSpoutStatus};
use playback_runtime_v2::{PlaybackLaunchConfig, PlaybackRuntime, PlaybackRuntimeError};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, RunEvent, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt as _;
use tokio::sync::Mutex as AsyncMutex;

use crate::conversion::{
    ConversionCoordinator, ConversionError, ConversionPhase, ConversionPlanRequest,
    ConversionSnapshot,
};
use crate::diagnostic_state::{
    DiagnosticSaveResult, active_snapshot, inactive_snapshot, write_player_bundle,
};
use crate::native_output::{
    PlayerViewportStore, ViewportBoundsError, ViewportBoundsRequest, ViewportSessionAck,
    ViewportStoreError, validate_viewport_bounds,
};
use crate::player_selection_v2::{
    PlayerCodecSelectionV2, PlayerSelectionV2Error, prepare_exact_launch, validate_exact_selection,
};
use crate::raw_import_runtime::{
    RawImportCodecOptions, RawImportRuntimeError, RawImportSelectionRequest,
    preflight_conversion_plan, prepare_exact_raw_import, raw_import_options_for,
    run_conversion_batch,
};

const MAIN_WINDOW_LABEL: &str = "main";
type ConversionSlot = Arc<Mutex<Option<Arc<ConversionCoordinator>>>>;

struct AppState {
    player: Arc<Mutex<PlayerCoordinator>>,
    runtime: Arc<AsyncMutex<Option<PlaybackRuntime>>>,
    viewport: PlayerViewportStore,
    fullscreen: HostFullscreenController,
    conversion: ConversionSlot,
    extension_roots: Option<ExtensionRoots>,
    codec_selection: Arc<Mutex<Option<PlayerCodecSelectionV2>>>,
    raw_import_staging_parent: Option<PathBuf>,
    exit_gate: ExitGate,
}

impl AppState {
    fn discover() -> Self {
        let player = PlayerCoordinator::without_codec();
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let extension_roots = local_app_data
            .as_ref()
            .map(ExtensionRoots::from_local_app_data);
        let raw_import_staging_parent = local_app_data
            .as_ref()
            .map(|root| root.join("LatentDeck").join("RawImportStaging"));
        Self {
            player: Arc::new(Mutex::new(player)),
            runtime: Arc::new(AsyncMutex::new(None)),
            viewport: PlayerViewportStore::new(),
            fullscreen: HostFullscreenController::new(),
            conversion: Arc::new(Mutex::new(None)),
            extension_roots,
            codec_selection: Arc::new(Mutex::new(None)),
            raw_import_staging_parent,
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

impl From<PlayerSelectionV2Error> for CommandError {
    fn from(error: PlayerSelectionV2Error) -> Self {
        Self::new(error.code(), error.to_string())
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

impl From<ConversionError> for CommandError {
    fn from(error: ConversionError) -> Self {
        Self::with_recoverability(error.code, error.message, error.recoverable)
    }
}

impl From<RawImportRuntimeError> for CommandError {
    fn from(error: RawImportRuntimeError) -> Self {
        Self::new(error.code(), error.user_message())
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

fn lock_conversion_slot(
    conversion: &ConversionSlot,
) -> Result<MutexGuard<'_, Option<Arc<ConversionCoordinator>>>, CommandError> {
    conversion.lock().map_err(|_| {
        CommandError::new(
            "conversion.state_unavailable",
            "Conversion state is unavailable; restart LatentPlayer.",
        )
    })
}

fn current_conversion(
    conversion: &ConversionSlot,
) -> Result<Arc<ConversionCoordinator>, CommandError> {
    lock_conversion_slot(conversion)?.clone().ok_or_else(|| {
        CommandError::new(
            "conversion.not_prepared",
            "Select a codec profile, add raw files, and prepare an import first.",
        )
    })
}

fn selected_codec_reference(
    selection: &Arc<Mutex<Option<PlayerCodecSelectionV2>>>,
) -> Result<Option<PackageReference>, CommandError> {
    selection
        .lock()
        .map_err(|_| {
            CommandError::new(
                "codec.selection_unavailable",
                "Codec selection state is unavailable.",
            )
        })
        .map(|selected| selected.as_ref().map(|value| value.package().clone()))
}

fn launch_config(
    player: &Arc<Mutex<PlayerCoordinator>>,
    roots: &ExtensionRoots,
    selection: &Arc<Mutex<Option<PlayerCodecSelectionV2>>>,
) -> Result<PlaybackLaunchConfig, CommandError> {
    let player = lock_player(player)?;
    let source = player.protocol2_source_inputs()?;
    let selection = selection.lock().map_err(|_| {
        CommandError::new(
            "codec.selection_unavailable",
            "Codec selection state is unavailable.",
        )
    })?;
    prepare_exact_launch(
        roots,
        selection.as_ref(),
        &source,
        latentdeck_core::product_version(),
        player.view().loop_enabled,
    )
    .map_err(Into::into)
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

fn extension_roots(state: &AppState) -> Result<&ExtensionRoots, CommandError> {
    state.extension_roots.as_ref().ok_or_else(|| {
        CommandError::new(
            "extension.root_unavailable",
            "LatentPlayer could not resolve the current-user Extensions root.",
        )
    })
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExtensionPackageRequest {
    kind: PackageKind,
    package_id: String,
    package_version: String,
}

impl ExtensionPackageRequest {
    fn into_reference(self) -> PackageReference {
        PackageReference {
            kind: self.kind,
            package_id: self.package_id,
            package_version: self.package_version,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionPackageReferenceView {
    kind: PackageKind,
    package_id: String,
    package_version: String,
}

impl From<&PackageReference> for ExtensionPackageReferenceView {
    fn from(value: &PackageReference) -> Self {
        Self {
            kind: value.kind,
            package_id: value.package_id.clone(),
            package_version: value.package_version.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionPackageSummaryView {
    package: ExtensionPackageReferenceView,
    display_name: Option<String>,
    publisher_name: Option<String>,
    enabled: bool,
    health: PackageHealth,
    error_code: Option<String>,
    error_detail: Option<String>,
}

impl From<InstalledPackageSummary> for ExtensionPackageSummaryView {
    fn from(value: InstalledPackageSummary) -> Self {
        let error_detail = value.error_detail.as_ref().map(|_| match value.health {
            PackageHealth::Healthy => "The exact package version is healthy.".to_owned(),
            PackageHealth::Corrupt => {
                "The installed package tree is corrupt; verify or repair this exact version."
                    .to_owned()
            }
            PackageHealth::Untrusted => {
                "The exact package version has no valid hash-bound trust receipt.".to_owned()
            }
        });
        Self {
            package: (&value.package).into(),
            display_name: value.display_name,
            publisher_name: value.publisher_name,
            enabled: value.enabled,
            health: value.health,
            error_code: value.error_code,
            error_detail,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionProfileView {
    codec_family: String,
    profile: String,
    profile_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionCompatibilityPairView {
    deck: ExtensionPackageReferenceView,
    codec: ExtensionPackageReferenceView,
    reason: CompatibilityReason,
    compatible_profile: Option<ExtensionProfileView>,
}

impl From<CompatibilityPair> for ExtensionCompatibilityPairView {
    fn from(value: CompatibilityPair) -> Self {
        Self {
            deck: (&value.deck).into(),
            codec: (&value.codec).into(),
            reason: value.reason,
            compatible_profile: value
                .compatible_profile
                .map(|profile| ExtensionProfileView {
                    codec_family: profile.codec_family,
                    profile: profile.profile,
                    profile_version: profile.profile_version,
                }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionManagerSnapshot {
    packages: Vec<ExtensionPackageSummaryView>,
    matrix: Vec<ExtensionCompatibilityPairView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct InspectedExtensionView {
    package: ExtensionPackageReferenceView,
    display_name: String,
    publisher_name: String,
    publisher_identity_claim: PublisherIdentityClaim,
    archive_sha256: String,
    archive_byte_length: u64,
    file_count: usize,
    extracted_byte_length: u64,
}

fn extension_snapshot_for(
    roots: &ExtensionRoots,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let inventory = extension_inventory(roots).map_err(extension_command_error)?;
    let packages = inventory.packages.into_iter().map(Into::into).collect();
    let matrix = inventory.matrix.into_iter().map(Into::into).collect();
    Ok(ExtensionManagerSnapshot { packages, matrix })
}

#[allow(clippy::needless_pass_by_value)] // `Result::map_err` transfers the owned lifecycle error.
fn extension_command_error(error: ExtensionError) -> CommandError {
    let message = match error.code() {
        ExtensionErrorCode::InvalidArguments => {
            "The extension request or exact package identity is invalid."
        }
        ExtensionErrorCode::ArchiveInvalid => {
            "The local package archive is malformed or violates its bounded format."
        }
        ExtensionErrorCode::ManifestInvalid => {
            "The package manifest or declared compatibility contract is invalid."
        }
        ExtensionErrorCode::IntegrityFailed => {
            "The package bytes no longer match their integrity catalog or trust receipt."
        }
        ExtensionErrorCode::PackageExists => {
            "That exact immutable package version is already installed."
        }
        ExtensionErrorCode::PackageMissing => "That exact package version is not installed.",
        ExtensionErrorCode::PackageActive => {
            "Disable and close every session using this exact package version first."
        }
        ExtensionErrorCode::PackageDisabled => "Enable that exact package version before using it.",
        ExtensionErrorCode::PackageUntrusted => {
            "The package is not authorized by an exact local hash-bound trust receipt."
        }
        ExtensionErrorCode::LifecycleBusy => {
            "Another extension lifecycle operation is still in progress."
        }
        ExtensionErrorCode::LifecycleConflict => {
            "The extension tree changed during the operation; refresh and try again."
        }
        ExtensionErrorCode::Io => {
            "The local extension operation could not access its bounded storage."
        }
    };
    CommandError::new(error.code().as_str(), message)
}

fn extension_task_failed() -> CommandError {
    CommandError::new(
        "extension.task_failed",
        "The local extension operation stopped unexpectedly; refresh its state before retrying.",
    )
}

fn inspected_extension_view(
    value: latentdeck_extension_manager::InspectedPackage,
) -> InspectedExtensionView {
    InspectedExtensionView {
        package: (&value.package).into(),
        display_name: value.display_name,
        publisher_name: value.publisher_name,
        publisher_identity_claim: value.publisher_identity_claim,
        archive_sha256: value.archive_sha256,
        archive_byte_length: value.archive_byte_length,
        file_count: value.file_count,
        extracted_byte_length: value.extracted_byte_length,
    }
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
    roots: &ExtensionRoots,
    selection: &Arc<Mutex<Option<PlayerCodecSelectionV2>>>,
) -> Result<PlaybackRuntime, CommandError> {
    let config = launch_config(player, roots, selection)?;
    let viewport = viewport.current_visible().map_err(viewport_store_error)?;
    let parent = main_window(app)?;
    Box::pin(PlaybackRuntime::start_protocol2(
        app.clone(),
        parent,
        Arc::clone(player),
        config,
        viewport,
    ))
    .await
    .map_err(Into::into)
}

async fn start_and_restart(
    app: &AppHandle,
    player: &Arc<Mutex<PlayerCoordinator>>,
    viewport: &PlayerViewportStore,
    roots: &ExtensionRoots,
    selection: &Arc<Mutex<Option<PlayerCodecSelectionV2>>>,
) -> Result<(PlaybackRuntime, PlayerView), CommandError> {
    let runtime = Box::pin(start_runtime(app, player, viewport, roots, selection)).await?;
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
#[allow(clippy::needless_pass_by_value)]
async fn extensions_snapshot(
    state: State<'_, AppState>,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = extension_roots(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move || extension_snapshot_for(&roots))
        .await
        .map_err(|_| extension_task_failed())?
}

#[tauri::command]
async fn extensions_inspect(path: String) -> Result<InspectedExtensionView, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_extension_archive(PathBuf::from(path), None)
            .map(inspected_extension_view)
            .map_err(extension_command_error)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn extensions_install(
    state: State<'_, AppState>,
    path: String,
    expected_sha256: String,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = extension_roots(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        install(
            &roots,
            &InstallRequest {
                archive_path: PathBuf::from(path),
                expected_sha256,
            },
        )
        .map_err(extension_command_error)?;
        extension_snapshot_for(&roots)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn extensions_repair(
    state: State<'_, AppState>,
    path: String,
    expected_sha256: String,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = extension_roots(&state)?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        repair(
            &roots,
            &InstallRequest {
                archive_path: PathBuf::from(path),
                expected_sha256,
            },
        )
        .map_err(extension_command_error)?;
        extension_snapshot_for(&roots)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn extensions_verify(
    state: State<'_, AppState>,
    package: ExtensionPackageRequest,
) -> Result<ExtensionPackageSummaryView, CommandError> {
    let roots = extension_roots(&state)?.clone();
    let package = package.into_reference();
    tauri::async_runtime::spawn_blocking(move || {
        verify(&roots, &package)
            .map(Into::into)
            .map_err(extension_command_error)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn extensions_enable(
    state: State<'_, AppState>,
    package: ExtensionPackageRequest,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = extension_roots(&state)?.clone();
    let package = package.into_reference();
    tauri::async_runtime::spawn_blocking(move || {
        enable(&roots, &package).map_err(extension_command_error)?;
        extension_snapshot_for(&roots)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn extensions_disable(
    state: State<'_, AppState>,
    package: ExtensionPackageRequest,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = extension_roots(&state)?.clone();
    let package = package.into_reference();
    tauri::async_runtime::spawn_blocking(move || {
        disable(&roots, &package).map_err(extension_command_error)?;
        extension_snapshot_for(&roots)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn extensions_remove(
    state: State<'_, AppState>,
    package: ExtensionPackageRequest,
    allow_corrupt: bool,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = extension_roots(&state)?.clone();
    let package = package.into_reference();
    tauri::async_runtime::spawn_blocking(move || {
        remove(&roots, &package, RemoveOptions { allow_corrupt })
            .map_err(extension_command_error)?;
        extension_snapshot_for(&roots)
    })
    .await
    .map_err(|_| extension_task_failed())?
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
#[allow(clippy::needless_pass_by_value)]
async fn player_raw_import_options(
    state: State<'_, AppState>,
) -> Result<RawImportCodecOptions, CommandError> {
    let roots = extension_roots(&state)?.clone();
    let selected = selected_codec_reference(&state.codec_selection)?;
    tauri::async_runtime::spawn_blocking(move || {
        raw_import_options_for(
            &roots,
            selected.as_ref(),
            latentdeck_core::product_version(),
        )
        .map_err(Into::into)
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "raw_import.options_failed",
            "Raw import codec discovery stopped unexpectedly.",
        )
    })?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn player_conversion_plan(
    state: State<'_, AppState>,
    inputs: Vec<String>,
    output_directory: String,
    recursive: bool,
    selection: RawImportSelectionRequest,
) -> Result<ConversionSnapshot, CommandError> {
    {
        let slot = lock_conversion_slot(&state.conversion)?;
        if let Some(current) = slot.as_ref() {
            let phase = current.snapshot()?.phase;
            if matches!(phase, ConversionPhase::Running | ConversionPhase::Stopping) {
                return Err(CommandError::new(
                    "conversion.busy",
                    "Stop the active import after its current file before preparing another batch.",
                ));
            }
        }
    }
    let request = ConversionPlanRequest {
        inputs: inputs.into_iter().map(PathBuf::from).collect(),
        output_directory: PathBuf::from(output_directory),
        recursive,
    };
    let roots = extension_roots(&state)?.clone();
    let selected = selected_codec_reference(&state.codec_selection)?;
    let prepared_selection = selection.clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_exact_raw_import(
            &roots,
            prepared_selection,
            selected.as_ref(),
            latentdeck_core::product_version(),
        )
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "conversion.preflight_failed",
            "Exact raw import selection validation stopped unexpectedly.",
        )
    })??;
    let plan = preflight_conversion_plan(request, selection, prepared).await?;
    let coordinator = Arc::new(ConversionCoordinator::from_plan(plan));
    let snapshot = coordinator.snapshot()?;
    let mut slot = lock_conversion_slot(&state.conversion)?;
    if let Some(current) = slot.as_ref() {
        let phase = current.snapshot()?.phase;
        if matches!(phase, ConversionPhase::Running | ConversionPhase::Stopping) {
            return Err(CommandError::new(
                "conversion.busy",
                "Stop the active import after its current file before preparing another batch.",
            ));
        }
    }
    *slot = Some(coordinator);
    Ok(snapshot)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn player_conversion_snapshot(
    state: State<'_, AppState>,
) -> Result<Option<ConversionSnapshot>, CommandError> {
    lock_conversion_slot(&state.conversion)?
        .as_ref()
        .map(|coordinator| coordinator.snapshot().map_err(Into::into))
        .transpose()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn player_conversion_start(
    state: State<'_, AppState>,
) -> Result<ConversionSnapshot, CommandError> {
    let coordinator = {
        let slot = lock_conversion_slot(&state.conversion)?;
        slot.as_ref().cloned().ok_or_else(|| {
            CommandError::new(
                "conversion.not_prepared",
                "Select a codec profile, add raw files, and prepare an import first.",
            )
        })?
    };
    let roots = extension_roots(&state)?.clone();
    let selected = selected_codec_reference(&state.codec_selection)?;
    let selection = coordinator.selection().clone();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_exact_raw_import(
            &roots,
            selection,
            selected.as_ref(),
            latentdeck_core::product_version(),
        )
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "conversion.preflight_failed",
            "Exact raw import selection validation stopped unexpectedly.",
        )
    })??;
    let staging_parent = state.raw_import_staging_parent.clone().ok_or_else(|| {
        CommandError::new(
            "raw_import.staging_root_unavailable",
            "LatentPlayer could not resolve its current-user raw import staging root.",
        )
    })?;
    coordinator.begin()?;
    run_conversion_batch(coordinator, prepared, staging_parent)
        .await
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
fn player_conversion_stop(state: State<'_, AppState>) -> Result<ConversionSnapshot, CommandError> {
    current_conversion(&state.conversion)?
        .request_stop()
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
async fn player_open_converted(
    state: State<'_, AppState>,
    index: usize,
) -> Result<PlayerView, CommandError> {
    let output = current_conversion(&state.conversion)?.completed_output(index)?;
    let mut runtime = state.runtime.lock().await;
    shutdown_runtime(&mut runtime).await?;
    let player = Arc::clone(&state.player);
    tauri::async_runtime::spawn_blocking(move || {
        lock_player(&player)?
            .open_cartridge(output)
            .map_err(Into::into)
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "player.validation_failed",
            "Converted cartridge validation task stopped unexpectedly.",
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
    drop(runtime);
    let asset_id = lock_player(&state.player)?
        .view()
        .codec
        .decoder_asset_id
        .ok_or_else(|| {
            CommandError::new(
                "codec.asset_ambiguous",
                "Select an exact Codec Pack and asset identity before choosing a file.",
            )
        })?;
    let roots = extension_roots(&state)?.clone();
    let selection = Arc::clone(&state.codec_selection);
    let player = Arc::clone(&state.player);
    tauri::async_runtime::spawn_blocking(move || {
        let summary = {
            let mut selected = selection.lock().map_err(|_| {
                CommandError::new(
                    "codec.selection_unavailable",
                    "Codec selection state is unavailable.",
                )
            })?;
            let selected = selected
                .as_mut()
                .ok_or(PlayerSelectionV2Error::MissingSelection)?;
            selected.bind_external_asset(asset_id, PathBuf::from(path));
            validate_exact_selection(&roots, selected)?
        };
        lock_player(&player)?
            .set_protocol2_codec_summary(summary)
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
#[allow(clippy::needless_pass_by_value)]
async fn player_select_codec_exact(
    state: State<'_, AppState>,
    package_id: String,
    package_version: String,
    device: String,
) -> Result<PlayerView, CommandError> {
    let mut runtime = state.runtime.lock().await;
    shutdown_runtime(&mut runtime).await?;
    drop(runtime);
    let device = match device.as_str() {
        "cpu" => DeviceKind::Cpu,
        "cuda" => DeviceKind::Cuda,
        _ => {
            return Err(CommandError::new(
                "codec.device_invalid",
                "Codec device must be exactly cpu or cuda.",
            ));
        }
    };
    let roots = extension_roots(&state)?.clone();
    let selection_slot = Arc::clone(&state.codec_selection);
    let player = Arc::clone(&state.player);
    tauri::async_runtime::spawn_blocking(move || {
        let selection = PlayerCodecSelectionV2::new(package_id, package_version, device);
        let summary = validate_exact_selection(&roots, &selection)?;
        *selection_slot.lock().map_err(|_| {
            CommandError::new(
                "codec.selection_unavailable",
                "Codec selection state is unavailable.",
            )
        })? = Some(selection);
        lock_player(&player)?
            .set_protocol2_codec_summary(summary)
            .map_err(Into::into)
    })
    .await
    .map_err(|_| {
        CommandError::new(
            "codec.selection_failed",
            "Exact Codec Pack selection stopped unexpectedly.",
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

    let runtime = Box::pin(start_runtime(
        &app,
        &state.player,
        &state.viewport,
        extension_roots(&state)?,
        &state.codec_selection,
    ))
    .await?;
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
    let (runtime, view) = Box::pin(start_and_restart(
        &app,
        &state.player,
        &state.viewport,
        extension_roots(&state)?,
        &state.codec_selection,
    ))
    .await?;
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
            extensions_snapshot,
            extensions_inspect,
            extensions_install,
            extensions_verify,
            extensions_enable,
            extensions_disable,
            extensions_remove,
            extensions_repair,
            player_snapshot,
            player_viewport_session_begin,
            player_viewport_set_bounds,
            player_open,
            player_raw_import_options,
            player_conversion_plan,
            player_conversion_snapshot,
            player_conversion_start,
            player_conversion_stop,
            player_open_converted,
            player_select_decoder,
            player_select_codec_exact,
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
                    let conversion = lock_conversion_slot(&state.conversion)
                        .ok()
                        .and_then(|slot| slot.clone());
                    if let Some(coordinator) = conversion.as_ref() {
                        let _ = coordinator.request_stop();
                    }
                    let app_handle = app_handle.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut runtime = runtime.lock().await;
                        let _ = shutdown_runtime(&mut runtime).await;
                        drop(runtime);
                        if let Some(coordinator) = conversion {
                            let _ = tauri::async_runtime::spawn_blocking(move || {
                                coordinator.wait_until_idle()
                            })
                            .await;
                        }
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

    #[test]
    fn extension_snapshot_is_empty_and_path_free_for_a_fresh_root() {
        let directory = tempfile::tempdir().expect("extension root");
        let roots = ExtensionRoots::for_base_root(directory.path().join("LatentDeck"));

        let snapshot = extension_snapshot_for(&roots).expect("empty extension snapshot");
        let value = serde_json::to_value(snapshot).expect("serialize snapshot");

        assert_eq!(value, serde_json::json!({ "packages": [], "matrix": [] }));
        assert!(
            !value
                .to_string()
                .contains(&directory.path().to_string_lossy().to_string())
        );
    }

    #[test]
    fn extension_errors_keep_the_stable_lifecycle_code_without_a_local_path() {
        let source = latentdeck_extension_manager::ExtensionError::new(
            latentdeck_extension_manager::ErrorCode::PackageUntrusted,
            r"package at W:\private\codec failed",
        );

        let error = extension_command_error(source);

        assert_eq!(error.code, "extension.package_untrusted");
        assert!(error.recoverable);
        assert!(!error.message.contains("W:\\private"));
    }

    #[test]
    fn extension_package_request_preserves_one_exact_version() {
        let request = ExtensionPackageRequest {
            kind: latentdeck_extension_manager::PackageKind::DeckPack,
            package_id: "org.example.deck".to_owned(),
            package_version: "1.2.3".to_owned(),
        };

        let package = request.into_reference();

        assert_eq!(
            package.kind,
            latentdeck_extension_manager::PackageKind::DeckPack
        );
        assert_eq!(package.package_id, "org.example.deck");
        assert_eq!(package.package_version, "1.2.3");
    }

    #[test]
    fn corrupt_extension_summary_does_not_expose_its_installed_path() {
        let summary = InstalledPackageSummary {
            package: PackageReference {
                kind: PackageKind::CodecPack,
                package_id: "org.example.codec".to_owned(),
                package_version: "1.0.0".to_owned(),
            },
            display_name: None,
            publisher_name: None,
            enabled: false,
            health: PackageHealth::Corrupt,
            error_code: Some("extension.integrity_failed".to_owned()),
            error_detail: Some(r"catalog mismatch at W:\private\runtime.exe".to_owned()),
        };

        let view = ExtensionPackageSummaryView::from(summary);
        assert!(
            !view
                .error_detail
                .as_deref()
                .unwrap_or_default()
                .contains("W:\\")
        );
        let value = serde_json::to_value(view).expect("serialize summary");

        assert_eq!(value["errorCode"], "extension.integrity_failed");
    }
}
