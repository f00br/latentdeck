//! Reusable raw Tauri window and DX12 presentation surface for decoded frames.
//!
//! This boundary deliberately contains no `WebView`, audio, seek state, codec
//! logic, application runtime, or backend fallback. An application actor owns
//! one [`NativeOutput`] and feeds it validated RGB Ring frames sequentially.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};

use latentdeck_gpu::{
    renderer::{Dx12Device, RgbaFrameRenderer, RgbaUpload, create_dx12_instance},
    ring::RingLayout,
};
use serde::Serialize;
use tauri::{AppHandle, WebviewWindow, Window, window::WindowBuilder};
use thiserror::Error;

mod spout;

use spout::SpoutSurface;

/// Explicit window identity and decoded-program dimensions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOutputConfig {
    /// Decoded RGBA frame width, independent of the resizable swapchain.
    pub frame_width: u32,
    /// Decoded RGBA frame height, independent of the resizable swapchain.
    pub frame_height: u32,
    window_label: String,
    window_title: String,
    spout_sender_name: String,
}

/// Physical client-area bounds for an embedded native output child window.
///
/// Coordinates are relative to the owning Tauri window's client area. The
/// decoded program geometry remains independent from these presentation
/// bounds, so resizing this rectangle never resamples the source texture or
/// changes the Spout sender extent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeOutputBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

impl NativeOutputBounds {
    /// Construct non-negative, non-zero physical child-window bounds.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOutputError::WindowPlacement`] for an invalid origin or
    /// zero extent.
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Result<Self, NativeOutputError> {
        if x < 0 || y < 0 || width == 0 || height == 0 {
            return Err(NativeOutputError::WindowPlacement);
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    /// Physical left edge relative to the parent client area.
    #[must_use]
    pub const fn x(self) -> i32 {
        self.x
    }

    /// Physical top edge relative to the parent client area.
    #[must_use]
    pub const fn y(self) -> i32 {
        self.y
    }

    /// Physical child-window width.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Physical child-window height.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

impl NativeOutputConfig {
    /// Construct an explicit native-output configuration.
    #[must_use]
    pub fn new(
        frame_width: u32,
        frame_height: u32,
        window_label: impl Into<String>,
        window_title: impl Into<String>,
    ) -> Self {
        let window_label = window_label.into();
        let window_title = window_title.into();
        Self {
            frame_width,
            frame_height,
            window_label,
            spout_sender_name: window_title.clone(),
            window_title,
        }
    }

    /// Tauri label supplied by the owning application.
    #[must_use]
    pub fn window_label(&self) -> &str {
        &self.window_label
    }

    /// Native window title supplied by the owning application.
    #[must_use]
    pub fn window_title(&self) -> &str {
        &self.window_title
    }

    /// Initial Spout sender name. It defaults to the native window title.
    #[must_use]
    pub fn spout_sender_name(&self) -> &str {
        &self.spout_sender_name
    }

    /// Override the initial Spout sender name before native allocation.
    #[must_use]
    pub fn with_spout_sender_name(mut self, name: impl Into<String>) -> Self {
        self.spout_sender_name = name.into();
        self
    }
}

/// Sanitized Spout sender state exposed to app commands and UI.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Wire view mirrors independent native facts.
pub struct NativeSpoutStatus {
    /// Whether this binary was compiled against the separately prepared SDK.
    pub sdk_built: bool,
    /// Whether the SDK opened on this exact native DX12 device/queue.
    pub ready: bool,
    /// Whether frame publication was requested.
    pub enabled: bool,
    /// Whether Spout has registered the sender after a successful frame.
    pub published: bool,
    /// Requested sender name.
    pub requested_name: String,
    /// Collision-resolved active sender name.
    pub active_name: String,
    /// Exact shared texture width.
    pub width: u32,
    /// Exact shared texture height.
    pub height: u32,
    /// Stable exact texture format token.
    pub format: &'static str,
    /// Number of successful GPU texture submissions.
    pub submitted_frames: u64,
    /// Last successful monotonically increasing output sequence.
    pub last_sequence: Option<u64>,
    /// Spout's own sender frame counter after publication.
    pub spout_frame: Option<i64>,
    /// Stable sanitized failure code; no path, handle, or driver text.
    pub last_error_code: Option<&'static str>,
}

/// Bounded path-free GPU identity suitable for a local support bundle.
///
/// The raw backend strings are normalized to a small printable allowlist so a
/// driver cannot inject a machine path or arbitrary diagnostic text into a
/// shareable report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeDeviceIdentity {
    /// Human-readable adapter name, normalized and capped at 96 bytes.
    pub adapter_name: String,
    /// Exact renderer backend selected by the v0.1 contract.
    pub backend: &'static str,
    /// Driver family, normalized and capped at 96 bytes.
    pub driver: String,
    /// Driver version/detail, normalized and capped at 96 bytes.
    pub driver_info: String,
    /// PCI vendor identity reported by wgpu.
    pub vendor_id: u32,
    /// PCI device identity reported by wgpu.
    pub device_id: u32,
    /// Stable coarse adapter class.
    pub device_type: &'static str,
}

/// Result of applying a physical-window resize event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResizeOutcome {
    /// A non-zero surface extent was configured with FIFO presentation.
    Configured,
    /// The requested extent already matched the configured surface.
    Unchanged,
    /// A zero-sized/minimized window suspended surface acquisition.
    Suspended,
}

/// Explicit local-surface result after one decoded frame was consumed.
///
/// Every successful result means the intrinsic texture was uploaded and, when
/// enabled, submitted to Spout exactly once. The `Skipped*` variants describe
/// only the local swapchain; callers must still advance their input stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentOutcome {
    /// The frame was submitted and presented normally.
    Presented,
    /// The frame was presented, then a suboptimal surface was reconfigured.
    PresentedAndReconfigured,
    /// The window had a zero physical extent, so no local swapchain image was used.
    SkippedZeroSized,
    /// Local surface acquisition timed out after the frame was consumed.
    SkippedTimeout,
    /// The local window was occluded, while the frame was still consumed.
    SkippedOccluded,
    /// An outdated local surface was reconfigured without displaying the frame.
    SkippedOutdated,
    /// A lost local surface was safely recreated without displaying the frame.
    SkippedSurfaceRecreated,
}

impl PresentOutcome {
    /// Whether the consumed frame reached the local swapchain.
    #[must_use]
    pub const fn locally_presented(self) -> bool {
        matches!(self, Self::Presented | Self::PresentedAndReconfigured)
    }
}

/// Sanitized native-output failure suitable for an application actor/API.
///
/// Backend strings are intentionally discarded at this boundary: they may
/// contain adapter details, local paths, or driver-specific diagnostics. The
/// stable code is safe to expose; structured private diagnostics can be added
/// separately without changing this contract.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum NativeOutputError {
    /// This v0.1 output path is deliberately Windows/DX12-only.
    #[error("native output requires Windows and DirectX 12")]
    UnsupportedPlatform,
    /// Decoded dimensions violate the bounded RGB Ring ABI.
    #[error("native output frame dimensions are invalid")]
    InvalidFrameDimensions,
    /// The separate raw Tauri window could not be created.
    #[error("native output window could not be created")]
    WindowCreate,
    /// Tauri could not report a usable initial physical extent.
    #[error("native output window size is unavailable")]
    WindowSize,
    /// The requested window visibility operation failed.
    #[error("native output window visibility could not be changed")]
    WindowVisibility,
    /// The owned native window could not be destroyed explicitly.
    #[error("native output window could not be destroyed")]
    WindowDestroy,
    /// The embedded child parent or physical client bounds were unavailable.
    #[error("native output child window could not be placed")]
    WindowPlacement,
    /// The requested fullscreen operation failed.
    #[error("native output fullscreen state could not be changed")]
    WindowFullscreen,
    /// A safe owned wgpu surface could not be created for the raw window.
    #[error("native output surface could not be created")]
    SurfaceCreate,
    /// DX12 adapter, device, surface policy, or renderer creation failed.
    #[error("native DX12 renderer could not be initialized")]
    RendererInitialization,
    /// A frame was not the exact padded RGBA layout promised by RGB Ring ABI 1.
    #[error("native output rejected an invalid padded RGBA frame")]
    FrameRejected,
    /// Surface acquisition reported a validation failure.
    #[error("native output surface validation failed")]
    SurfaceValidation,
    /// wgpu reported a device loss; no fallback device is selected.
    #[error("native DX12 device was lost")]
    DeviceLost,
    /// wgpu reported exhausted GPU memory.
    #[error("native DX12 output ran out of GPU memory")]
    GpuOutOfMemory,
    /// wgpu reported an internal backend failure.
    #[error("native DX12 output encountered an internal GPU failure")]
    GpuInternal,
    /// wgpu reported an uncaptured validation failure.
    #[error("native DX12 output encountered a GPU validation failure")]
    GpuValidation,
    /// The binary does not contain a usable prepared Spout2 SDK bridge.
    #[error("Spout2 output is unavailable in this build or on this device")]
    SpoutUnavailable,
    /// A Spout sender name or enable/disable request was rejected.
    #[error("Spout2 output control failed")]
    SpoutControl,
}

impl NativeOutputError {
    /// Stable machine-readable error code for a typed application event.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "output.platform_unsupported",
            Self::InvalidFrameDimensions => "output.frame_dimensions_invalid",
            Self::WindowCreate => "output.window_create_failed",
            Self::WindowSize => "output.window_size_unavailable",
            Self::WindowVisibility => "output.window_visibility_failed",
            Self::WindowDestroy => "output.window_destroy_failed",
            Self::WindowPlacement => "output.window_placement_failed",
            Self::WindowFullscreen => "output.window_fullscreen_failed",
            Self::SurfaceCreate => "output.surface_create_failed",
            Self::RendererInitialization => "output.renderer_initialization_failed",
            Self::FrameRejected => "output.frame_rejected",
            Self::SurfaceValidation => "output.surface_validation_failed",
            Self::DeviceLost => "output.device_lost",
            Self::GpuOutOfMemory => "output.gpu_out_of_memory",
            Self::GpuInternal => "output.gpu_internal",
            Self::GpuValidation => "output.gpu_validation",
            Self::SpoutUnavailable => "output.spout_unavailable",
            Self::SpoutControl => "output.spout_control_failed",
        }
    }
}

/// Shared owner for borderless fullscreen transitions of the Tauri host.
///
/// Embedded outputs remain child windows throughout the transition. The host
/// controller owns the one piece of window-manager state that a Deck
/// faceplate must not reimplement: capture and restoration of the real
/// top-level HWND style and placement. All mutations run on Tauri's main
/// thread and are verified against the monitor rectangle before an active
/// state is acknowledged.
#[derive(Clone, Default)]
pub struct HostFullscreenController {
    state: Arc<Mutex<HostFullscreenState>>,
}

#[derive(Default)]
struct HostFullscreenState {
    #[cfg(target_os = "windows")]
    restore: Option<windows_host::RestoreState>,
    #[cfg(not(target_os = "windows"))]
    active: bool,
}

impl HostFullscreenController {
    /// Create an inactive host-fullscreen controller.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read fullscreen from the verified top-level host, not from a cached UI
    /// flag.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOutputError::WindowFullscreen`] when the main-thread
    /// dispatch, HWND inspection, or controller lock fails.
    pub async fn status(&self, window: &WebviewWindow) -> Result<bool, NativeOutputError> {
        self.dispatch(window, HostFullscreenRequest::Status).await
    }

    /// Enter or leave borderless monitor fullscreen and return the verified
    /// resulting state.
    ///
    /// The original style and `WINDOWPLACEMENT` are captured once and restored
    /// exactly on exit. Repeating the same request is idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`NativeOutputError::WindowFullscreen`] if the transition or
    /// any native postcondition fails.
    pub async fn set(
        &self,
        window: &WebviewWindow,
        enabled: bool,
    ) -> Result<bool, NativeOutputError> {
        self.dispatch(window, HostFullscreenRequest::Set(enabled))
            .await
    }

    async fn dispatch(
        &self,
        window: &WebviewWindow,
        request: HostFullscreenRequest,
    ) -> Result<bool, NativeOutputError> {
        let state = Arc::clone(&self.state);
        let dispatched_window = window.clone();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        window
            .run_on_main_thread(move || {
                let result = state
                    .lock()
                    .map_err(|_| NativeOutputError::WindowFullscreen)
                    .and_then(|mut state| {
                        apply_host_fullscreen_request(&dispatched_window, &mut state, request)
                    });
                let _ = sender.send(result);
            })
            .map_err(|_| NativeOutputError::WindowFullscreen)?;
        receiver
            .await
            .map_err(|_| NativeOutputError::WindowFullscreen)?
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostFullscreenRequest {
    Status,
    Set(bool),
}

#[cfg(target_os = "windows")]
fn apply_host_fullscreen_request(
    window: &WebviewWindow,
    state: &mut HostFullscreenState,
    request: HostFullscreenRequest,
) -> Result<bool, NativeOutputError> {
    windows_host::apply(window, state, request)
}

#[cfg(not(target_os = "windows"))]
fn apply_host_fullscreen_request(
    window: &WebviewWindow,
    state: &mut HostFullscreenState,
    request: HostFullscreenRequest,
) -> Result<bool, NativeOutputError> {
    match request {
        HostFullscreenRequest::Status => window
            .is_fullscreen()
            .map_err(|_| NativeOutputError::WindowFullscreen),
        HostFullscreenRequest::Set(enabled) => {
            window
                .set_fullscreen(enabled)
                .map_err(|_| NativeOutputError::WindowFullscreen)?;
            let active = window
                .is_fullscreen()
                .map_err(|_| NativeOutputError::WindowFullscreen)?;
            state.active = active;
            Ok(active)
        }
    }
}

/// One raw Tauri window with a safe owned DX12 wgpu surface.
///
/// Mutating methods take `&mut self`, which makes resize/reconfigure/present
/// sequencing explicit for a later single-owner actor.
pub struct NativeOutput {
    window: Window,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    dx12: Dx12Device,
    renderer: RgbaFrameRenderer,
    offscreen_target: OffscreenTarget,
    surface_configuration: wgpu::SurfaceConfiguration,
    surface_extent: Option<(u32, u32)>,
    gpu_health: GpuHealth,
    spout: SpoutSurface,
}

impl NativeOutput {
    /// Create a hidden raw Tauri window, a safely owned surface, and the
    /// DX12-only presentation pipeline.
    ///
    /// The window starts hidden so the caller can show it only after it has a
    /// valid decoded frame/status. Call this from an async Tauri setup or
    /// command context; raw-window creation may dispatch to the UI thread.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error for unsupported platforms, invalid decoded
    /// dimensions, raw-window/surface failures, or unavailable DX12 hardware.
    pub async fn new(
        app: &AppHandle,
        config: NativeOutputConfig,
    ) -> Result<Self, NativeOutputError> {
        if !cfg!(target_os = "windows") {
            return Err(NativeOutputError::UnsupportedPlatform);
        }

        let frame_layout = RingLayout::new(config.frame_width, config.frame_height)
            .map_err(|_| NativeOutputError::InvalidFrameDimensions)?;
        let spout_sender_name = config.spout_sender_name.clone();
        let window = WindowBuilder::new(app, config.window_label)
            .title(config.window_title)
            .inner_size(
                f64::from(frame_layout.width()),
                f64::from(frame_layout.height()),
            )
            .resizable(true)
            .visible(false)
            .build()
            .map_err(|_| NativeOutputError::WindowCreate)?;
        Self::initialize(window, frame_layout, spout_sender_name).await
    }

    /// Create a hidden borderless DX12 child surface inside one Tauri
    /// `WebviewWindow`.
    ///
    /// Tauri's Windows `parent_raw` path creates a real `WS_CHILD`, not an
    /// owned top-level popup. The `WebView` remains responsible only for controls
    /// and an empty layout anchor; decoded RGBA never crosses into browser
    /// rendering. The initial physical bounds are applied before wgpu creates
    /// the surface, preventing a detached-window flash.
    ///
    /// # Errors
    ///
    /// Returns a sanitized error when the parent HWND, child window, physical
    /// bounds, or native DX12 renderer cannot be created.
    #[cfg(target_os = "windows")]
    pub async fn new_embedded(
        app: &AppHandle,
        parent: &WebviewWindow,
        config: NativeOutputConfig,
        bounds: NativeOutputBounds,
    ) -> Result<Self, NativeOutputError> {
        let frame_layout = RingLayout::new(config.frame_width, config.frame_height)
            .map_err(|_| NativeOutputError::InvalidFrameDimensions)?;
        let spout_sender_name = config.spout_sender_name.clone();
        let parent_hwnd = parent
            .hwnd()
            .map_err(|_| NativeOutputError::WindowPlacement)?;
        let window = WindowBuilder::new(app, config.window_label)
            .title(config.window_title)
            .inner_size(1.0, 1.0)
            .decorations(false)
            .resizable(false)
            .minimizable(false)
            .maximizable(false)
            .closable(false)
            .shadow(false)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .visible(false)
            .parent_raw(parent_hwnd)
            .build()
            .map_err(|_| NativeOutputError::WindowCreate)?;
        let pending_window = PendingWindow::new(window.clone());
        apply_window_bounds(&window, bounds)?;
        pending_window.disarm();
        Self::initialize(window, frame_layout, spout_sender_name).await
    }

    async fn initialize(
        window: Window,
        frame_layout: RingLayout,
        spout_sender_name: String,
    ) -> Result<Self, NativeOutputError> {
        let pending_window = PendingWindow::new(window.clone());

        let physical_size = window
            .inner_size()
            .map_err(|_| NativeOutputError::WindowSize)?;
        if physical_size.width == 0 || physical_size.height == 0 {
            return Err(NativeOutputError::WindowSize);
        }

        let instance =
            create_dx12_instance().map_err(|_| NativeOutputError::RendererInitialization)?;
        // Moving an owned Tauri Window clone into `create_surface` makes wgpu
        // retain its own handle source. The resulting surface is therefore
        // safely `'static`; this module contains no raw-handle unsafe code.
        let surface: wgpu::Surface<'static> = instance
            .create_surface(window.clone())
            .map_err(|_| NativeOutputError::SurfaceCreate)?;
        let dx12 = Dx12Device::request(&instance, Some(&surface))
            .await
            .map_err(|_| NativeOutputError::RendererInitialization)?;
        let gpu_health = GpuHealth::new();
        install_gpu_callbacks(dx12.device(), &gpu_health);

        let surface_configuration = dx12
            .surface_configuration(&surface, physical_size.width, physical_size.height)
            .map_err(|_| NativeOutputError::RendererInitialization)?;
        debug_assert_eq!(surface_configuration.present_mode, wgpu::PresentMode::Fifo);
        surface.configure(dx12.device(), &surface_configuration);
        let renderer = RgbaFrameRenderer::new(
            dx12.device(),
            surface_configuration.format,
            frame_layout.width(),
            frame_layout.height(),
        )
        .map_err(|_| NativeOutputError::RendererInitialization)?;
        let offscreen_target = OffscreenTarget::new(dx12.device(), surface_configuration.format);
        let spout = SpoutSurface::open(
            &spout_sender_name,
            frame_layout.width(),
            frame_layout.height(),
            dx12.device(),
        );

        let output = Self {
            window,
            instance,
            surface,
            dx12,
            renderer,
            offscreen_target,
            surface_configuration,
            surface_extent: Some((physical_size.width, physical_size.height)),
            gpu_health,
            spout,
        };
        output.poll_gpu_health()?;
        pending_window.disarm();
        Ok(output)
    }

    /// Borrow the raw output window for later event-listener registration.
    #[must_use]
    pub const fn window(&self) -> &Window {
        &self.window
    }

    /// Fixed decoded-program dimensions, independent of swapchain size.
    #[must_use]
    pub fn frame_dimensions(&self) -> (u32, u32) {
        let layout = self.renderer.frame_layout();
        (layout.width(), layout.height())
    }

    /// Return a bounded, path-free identity for support diagnostics.
    #[must_use]
    pub fn device_identity(&self) -> NativeDeviceIdentity {
        let info = self.dx12.adapter().get_info();
        NativeDeviceIdentity {
            adapter_name: sanitize_device_identity(&info.name),
            backend: "dx12",
            driver: sanitize_device_identity(&info.driver),
            driver_info: sanitize_device_identity(&info.driver_info),
            vendor_id: info.vendor,
            device_id: info.device,
            device_type: match info.device_type {
                wgpu::DeviceType::DiscreteGpu => "discrete_gpu",
                wgpu::DeviceType::IntegratedGpu => "integrated_gpu",
                wgpu::DeviceType::VirtualGpu => "virtual_gpu",
                wgpu::DeviceType::Cpu => "cpu",
                wgpu::DeviceType::Other => "other",
            },
        }
    }

    /// Return the latest sanitized Spout sender state without native I/O.
    #[must_use]
    pub fn spout_status(&self) -> NativeSpoutStatus {
        self.spout.status()
    }

    /// Enable or disable GPU texture publication through Spout2.
    ///
    /// # Errors
    ///
    /// Returns a stable control/unavailable error while preserving native
    /// window playback and the detailed sanitized status snapshot.
    pub fn set_spout_enabled(
        &mut self,
        enabled: bool,
    ) -> Result<NativeSpoutStatus, NativeOutputError> {
        self.spout.set_enabled(enabled)
    }

    /// Change the requested Spout sender name.
    ///
    /// # Errors
    ///
    /// Rejects invalid names or an unavailable SDK with stable output errors.
    pub fn set_spout_name(
        &mut self,
        name: impl Into<String>,
    ) -> Result<NativeSpoutStatus, NativeOutputError> {
        let name = name.into();
        self.spout.set_name(&name)
    }

    /// The enforced swapchain presentation mode.
    #[must_use]
    pub const fn present_mode(&self) -> wgpu::PresentMode {
        self.surface_configuration.present_mode
    }

    /// Show the raw native output window.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-operation failure.
    pub fn show(&self) -> Result<(), NativeOutputError> {
        self.window
            .show()
            .map_err(|_| NativeOutputError::WindowVisibility)
    }

    /// Hide the raw native output window without destroying GPU resources.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-operation failure.
    pub fn hide(&self) -> Result<(), NativeOutputError> {
        self.window
            .hide()
            .map_err(|_| NativeOutputError::WindowVisibility)
    }

    /// Destroy the owned native window and unregister its Tauri label.
    ///
    /// `Drop` repeats this operation best-effort, making every constructor,
    /// actor-fault, and channel-close path safe for an explicit restart.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-destruction failure.
    pub fn destroy(&self) -> Result<(), NativeOutputError> {
        self.window
            .destroy()
            .map_err(|_| NativeOutputError::WindowDestroy)
    }

    /// Set the window fullscreen state explicitly.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-operation failure.
    pub fn set_fullscreen(&self, fullscreen: bool) -> Result<(), NativeOutputError> {
        self.window
            .set_fullscreen(fullscreen)
            .map_err(|_| NativeOutputError::WindowFullscreen)
    }

    /// Return the fullscreen state reported by the native output window.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-operation failure.
    pub fn fullscreen(&self) -> Result<bool, NativeOutputError> {
        self.window
            .is_fullscreen()
            .map_err(|_| NativeOutputError::WindowFullscreen)
    }

    /// Toggle fullscreen and return the newly requested state.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-operation failure.
    pub fn toggle_fullscreen(&self) -> Result<bool, NativeOutputError> {
        let fullscreen = self.fullscreen()?;
        let next = !fullscreen;
        self.set_fullscreen(next)?;
        self.fullscreen()
    }

    /// Apply a physical resize event. Zero dimensions suspend acquisition and
    /// never reach `Surface::configure`.
    ///
    /// # Errors
    ///
    /// Returns on device loss or if the current surface cannot be configured
    /// for the requested non-zero extent.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<ResizeOutcome, NativeOutputError> {
        self.poll_gpu_health()?;
        match resize_decision(self.surface_extent, width, height) {
            ResizeDecision::Suspend => {
                self.surface_extent = None;
                Ok(ResizeOutcome::Suspended)
            }
            ResizeDecision::Unchanged => Ok(ResizeOutcome::Unchanged),
            ResizeDecision::Configure => {
                self.configure_surface(width, height)?;
                Ok(ResizeOutcome::Configured)
            }
        }
    }

    /// Move and resize an embedded child output in physical parent-client
    /// coordinates, then reconfigure its swapchain exactly once.
    ///
    /// The fixed decoded texture and Spout dimensions are not changed.
    ///
    /// # Errors
    ///
    /// Returns a sanitized placement, surface, or GPU error.
    pub fn set_embedded_bounds(
        &mut self,
        bounds: NativeOutputBounds,
    ) -> Result<ResizeOutcome, NativeOutputError> {
        let (width, height) = apply_window_bounds(&self.window, bounds)?;
        self.resize(width, height)
    }

    /// Validate, upload, render, and present one exact ABI-padded RGBA frame.
    ///
    /// Surface timeout/occlusion/outdated/lost states are returned explicitly;
    /// none silently switches backend or device. A lost DX12 device is fatal to
    /// this output instance and must be recreated by the owner.
    ///
    /// # Errors
    ///
    /// Returns a sanitized frame, surface-validation, or GPU health failure.
    pub fn present_padded_rgba(
        &mut self,
        width: u32,
        height: u32,
        row_stride: u32,
        padded_rgba: &[u8],
    ) -> Result<PresentOutcome, NativeOutputError> {
        self.poll_gpu_health()?;
        let upload = validated_upload(width, height, row_stride, padded_rgba)?;
        self.renderer
            .upload(self.dx12.queue(), upload)
            .map_err(|_| NativeOutputError::FrameRejected)?;
        self.poll_gpu_health()?;

        if self.surface_extent.is_none() {
            self.consume_without_local_surface()?;
            return Ok(PresentOutcome::SkippedZeroSized);
        }

        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => {
                self.render_and_present(texture)?;
                self.spout.submit(self.renderer.frame_texture());
                Ok(PresentOutcome::Presented)
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                self.render_and_present(texture)?;
                self.spout.submit(self.renderer.frame_texture());
                let (surface_width, surface_height) =
                    self.surface_extent.ok_or(NativeOutputError::WindowSize)?;
                self.configure_surface(surface_width, surface_height)?;
                Ok(PresentOutcome::PresentedAndReconfigured)
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                self.consume_without_local_surface()?;
                Ok(PresentOutcome::SkippedTimeout)
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                self.consume_without_local_surface()?;
                Ok(PresentOutcome::SkippedOccluded)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.consume_without_local_surface()?;
                let (surface_width, surface_height) =
                    self.surface_extent.ok_or(NativeOutputError::WindowSize)?;
                self.configure_surface(surface_width, surface_height)?;
                Ok(PresentOutcome::SkippedOutdated)
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.consume_without_local_surface()?;
                self.poll_gpu_health()?;
                self.recreate_surface()?;
                Ok(PresentOutcome::SkippedSurfaceRecreated)
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                self.poll_gpu_health()?;
                Err(NativeOutputError::SurfaceValidation)
            }
        }
    }

    fn consume_without_local_surface(&mut self) -> Result<(), NativeOutputError> {
        // `Queue::write_texture` leaves the fixed frame texture in COPY_DST
        // until a tracked GPU use is submitted. Draw it into a persistent 1x1
        // target so every path reaches wgpu's same combined shader-resource
        // state expected by the cached D3D11On12 Spout wrapper. This also
        // flushes the pending upload while remaining independent from swapchain
        // visibility.
        let mut encoder =
            self.dx12
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("LatentDeck offscreen frame consumption"),
                });
        self.renderer
            .encode(&mut encoder, &self.offscreen_target.view);
        self.dx12.queue().submit([encoder.finish()]);
        self.poll_gpu_health()?;
        self.spout.submit(self.renderer.frame_texture());
        Ok(())
    }

    fn render_and_present(
        &self,
        surface_texture: wgpu::SurfaceTexture,
    ) -> Result<(), NativeOutputError> {
        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            self.dx12
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("LatentDeck native output present"),
                });
        let (surface_width, surface_height) =
            self.surface_extent.ok_or(NativeOutputError::WindowSize)?;
        self.renderer
            .encode_aspect_fit(&mut encoder, &view, surface_width, surface_height)
            .map_err(|_| NativeOutputError::RendererInitialization)?;
        self.dx12.queue().submit([encoder.finish()]);
        self.dx12.queue().present(surface_texture);
        self.poll_gpu_health()
    }

    fn configure_surface(&mut self, width: u32, height: u32) -> Result<(), NativeOutputError> {
        let next = self
            .dx12
            .surface_configuration(&self.surface, width, height)
            .map_err(|_| NativeOutputError::RendererInitialization)?;
        debug_assert_eq!(next.present_mode, wgpu::PresentMode::Fifo);
        let replacement_renderer = if next.format == self.surface_configuration.format {
            None
        } else {
            let frame = self.renderer.frame_layout();
            let renderer = RgbaFrameRenderer::new(
                self.dx12.device(),
                next.format,
                frame.width(),
                frame.height(),
            )
            .map_err(|_| NativeOutputError::RendererInitialization)?;
            let offscreen_target = OffscreenTarget::new(self.dx12.device(), next.format);
            Some((renderer, offscreen_target))
        };

        self.surface.configure(self.dx12.device(), &next);
        if let Some((renderer, offscreen_target)) = replacement_renderer {
            self.renderer = renderer;
            self.offscreen_target = offscreen_target;
        }
        self.surface_configuration = next;
        self.surface_extent = Some((width, height));
        self.poll_gpu_health()
    }

    fn recreate_surface(&mut self) -> Result<(), NativeOutputError> {
        let (width, height) = self.surface_extent.ok_or(NativeOutputError::WindowSize)?;
        let replacement: wgpu::Surface<'static> = self
            .instance
            .create_surface(self.window.clone())
            .map_err(|_| NativeOutputError::SurfaceCreate)?;
        let next = self
            .dx12
            .surface_configuration(&replacement, width, height)
            .map_err(|_| NativeOutputError::RendererInitialization)?;
        let replacement_renderer = if next.format == self.surface_configuration.format {
            None
        } else {
            let frame = self.renderer.frame_layout();
            let renderer = RgbaFrameRenderer::new(
                self.dx12.device(),
                next.format,
                frame.width(),
                frame.height(),
            )
            .map_err(|_| NativeOutputError::RendererInitialization)?;
            let offscreen_target = OffscreenTarget::new(self.dx12.device(), next.format);
            Some((renderer, offscreen_target))
        };
        replacement.configure(self.dx12.device(), &next);
        self.surface = replacement;
        self.surface_configuration = next;
        if let Some((renderer, offscreen_target)) = replacement_renderer {
            self.renderer = renderer;
            self.offscreen_target = offscreen_target;
        }
        self.poll_gpu_health()
    }

    fn poll_gpu_health(&self) -> Result<(), NativeOutputError> {
        let _ = self.instance.poll_all(false);
        self.gpu_health.check()
    }
}

struct OffscreenTarget {
    // Keep the backing texture alive explicitly; the view is the render-pass
    // target used to normalize the source texture state while occluded.
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl OffscreenTarget {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("LatentDeck offscreen presentation target"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            _texture: texture,
            view,
        }
    }
}

impl Drop for NativeOutput {
    fn drop(&mut self) {
        // Tauri's Window clone is not an ownership guard: dropping it alone
        // leaves the registered label alive. Always request destruction so a
        // failed worker/presenter can be restarted in the same app process.
        let _ = self.window.destroy();
    }
}

struct PendingWindow(Option<Window>);

impl PendingWindow {
    fn new(window: Window) -> Self {
        Self(Some(window))
    }

    fn disarm(mut self) {
        let _ = self.0.take();
    }
}

impl Drop for PendingWindow {
    fn drop(&mut self) {
        if let Some(window) = self.0.take() {
            // Constructor errors are already sanitized. Cleanup is best-effort
            // and must not replace the primary failure or leave a hidden label
            // registered, which would make an explicit retry impossible.
            let _ = window.destroy();
        }
    }
}

fn validated_upload(
    width: u32,
    height: u32,
    row_stride: u32,
    padded_rgba: &[u8],
) -> Result<RgbaUpload<'_>, NativeOutputError> {
    RgbaUpload::new(width, height, row_stride, padded_rgba)
        .map_err(|_| NativeOutputError::FrameRejected)
}

fn sanitize_device_identity(value: &str) -> String {
    const MAX_BYTES: usize = 96;
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.is_empty()
        || !trimmed.is_ascii()
        || trimmed.len() > MAX_BYTES * 4
        || trimmed.contains(['/', '\\', ':', '$', '%', '@', '\n', '\r'])
        || trimmed.contains("..")
        || lower.contains("password=")
        || lower.contains("secret=")
        || lower.contains("token=")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("bearer ")
    {
        return "unknown".to_owned();
    }

    let mut output = String::with_capacity(trimmed.len().min(MAX_BYTES));
    let mut previous_replacement = false;
    for byte in trimmed.bytes() {
        if output.len() >= MAX_BYTES {
            break;
        }
        let accepted = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b' ' | b'.' | b'_' | b'-' | b'+' | b'(' | b')' | b'[' | b']'
            );
        if accepted {
            output.push(char::from(byte));
            previous_replacement = false;
        } else if !previous_replacement {
            output.push('_');
            previous_replacement = true;
        }
    }
    let trimmed = output.trim_matches([' ', '_']);
    if trimmed.is_empty() {
        "unknown".to_owned()
    } else {
        trimmed.to_owned()
    }
}

#[cfg(target_os = "windows")]
fn apply_window_bounds(
    window: &Window,
    bounds: NativeOutputBounds,
) -> Result<(u32, u32), NativeOutputError> {
    windows_child::apply_exact_bounds(window, bounds)
}

#[cfg(not(target_os = "windows"))]
fn apply_window_bounds(
    _window: &Window,
    _bounds: NativeOutputBounds,
) -> Result<(u32, u32), NativeOutputError> {
    Err(NativeOutputError::UnsupportedPlatform)
}

#[cfg(target_os = "windows")]
mod windows_host {
    #![allow(unsafe_code)]

    use std::mem::size_of;

    use tauri::WebviewWindow;
    use windows_sys::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::WindowsAndMessaging::{
            GA_ROOT, GWL_STYLE, GetAncestor, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect,
            HWND_TOP, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
            SWP_NOZORDER, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, WINDOWPLACEMENT,
            WS_MAXIMIZE, WS_MINIMIZE, WS_OVERLAPPEDWINDOW,
        },
    };

    use super::{HostFullscreenRequest, HostFullscreenState, NativeOutputError};

    #[derive(Clone, Copy)]
    pub(super) struct RestoreState {
        hwnd: isize,
        style: isize,
        placement: WINDOWPLACEMENT,
    }

    pub(super) fn apply(
        window: &WebviewWindow,
        state: &mut HostFullscreenState,
        request: HostFullscreenRequest,
    ) -> Result<bool, NativeOutputError> {
        let hwnd = root_hwnd(window)?;
        match request {
            HostFullscreenRequest::Status => state.restore.map_or(Ok(false), |restore| {
                if restore.hwnd != hwnd as isize {
                    return Err(NativeOutputError::WindowFullscreen);
                }
                require_verified_fullscreen(hwnd)
            }),
            HostFullscreenRequest::Set(true) => enter(hwnd, state),
            HostFullscreenRequest::Set(false) => leave(hwnd, state),
        }
    }

    fn enter(hwnd: HWND, state: &mut HostFullscreenState) -> Result<bool, NativeOutputError> {
        if let Some(restore) = state.restore {
            if restore.hwnd != hwnd as isize {
                return Err(NativeOutputError::WindowFullscreen);
            }
            apply_fullscreen(hwnd, restore.style)?;
            return require_verified_fullscreen(hwnd);
        }

        let restore = capture(hwnd)?;
        // Publish the recovery state before the first mutation. From this
        // point onward every error path must either prove rollback completed
        // or retain this snapshot so a later Set(false) can retry it.
        state.restore = Some(restore);
        if let Err(error) = apply_fullscreen(hwnd, restore.style) {
            let _ = rollback_failed_enter_with(state, restore, restore_window);
            return Err(error);
        }
        if let Ok(true) = fullscreen_postconditions(hwnd) {
            Ok(true)
        } else {
            let _ = rollback_failed_enter_with(state, restore, restore_window);
            Err(NativeOutputError::WindowFullscreen)
        }
    }

    fn rollback_failed_enter_with(
        state: &mut HostFullscreenState,
        restore: RestoreState,
        rollback: impl FnOnce(RestoreState) -> Result<(), NativeOutputError>,
    ) -> Result<(), NativeOutputError> {
        match rollback(restore) {
            Ok(()) => {
                state.restore = None;
                Ok(())
            }
            Err(error) => {
                state.restore = Some(restore);
                Err(error)
            }
        }
    }

    fn leave(hwnd: HWND, state: &mut HostFullscreenState) -> Result<bool, NativeOutputError> {
        let Some(restore) = state.restore else {
            return Ok(false);
        };
        if restore.hwnd != hwnd as isize {
            return Err(NativeOutputError::WindowFullscreen);
        }
        restore_window(restore)?;
        state.restore = None;
        Ok(false)
    }

    fn root_hwnd(window: &WebviewWindow) -> Result<HWND, NativeOutputError> {
        let window = window
            .hwnd()
            .map_err(|_| NativeOutputError::WindowFullscreen)?
            .0 as HWND;
        if window.is_null() {
            return Err(NativeOutputError::WindowFullscreen);
        }
        // SAFETY: the source handle belongs to the live Tauri WebviewWindow.
        // `GetAncestor` performs no mutation; selecting `GA_ROOT` prevents a
        // WebView implementation detail from becoming the fullscreen owner.
        let root = unsafe { GetAncestor(window, GA_ROOT) };
        if root.is_null() { Ok(window) } else { Ok(root) }
    }

    fn capture(hwnd: HWND) -> Result<RestoreState, NativeOutputError> {
        let mut placement = WINDOWPLACEMENT {
            length: u32::try_from(size_of::<WINDOWPLACEMENT>())
                .map_err(|_| NativeOutputError::WindowFullscreen)?,
            ..WINDOWPLACEMENT::default()
        };
        // SAFETY: `hwnd` is a live root window and `placement` has the exact
        // Win32 structure size required by `GetWindowPlacement`.
        let style = unsafe {
            if GetWindowPlacement(hwnd, &raw mut placement) == 0 {
                return Err(NativeOutputError::WindowFullscreen);
            }
            GetWindowLongPtrW(hwnd, GWL_STYLE)
        };
        if style == 0 {
            return Err(NativeOutputError::WindowFullscreen);
        }
        Ok(RestoreState {
            hwnd: hwnd as isize,
            style,
            placement,
        })
    }

    fn apply_fullscreen(hwnd: HWND, original_style: isize) -> Result<(), NativeOutputError> {
        let monitor_rect = monitor_rect(hwnd)?;
        let width = rect_width(monitor_rect)?;
        let height = rect_height(monitor_rect)?;
        let style = borderless_style(original_style);

        // SAFETY: all values were obtained from the live root HWND and its
        // nearest monitor. The operation changes only the host frame and
        // physical monitor rectangle; embedded WS_CHILD outputs retain their
        // parent and receive the resulting client resize normally.
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style);
            if GetWindowLongPtrW(hwnd, GWL_STYLE) != style
                || SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    monitor_rect.left,
                    monitor_rect.top,
                    width,
                    height,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                ) == 0
            {
                return Err(NativeOutputError::WindowFullscreen);
            }
        }
        Ok(())
    }

    fn restore_window(restore: RestoreState) -> Result<(), NativeOutputError> {
        let hwnd = restore.hwnd as HWND;
        // SAFETY: the controller retains the captured HWND only while the host
        // window is alive. Style and placement originated from that same HWND.
        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, restore.style);
            if GetWindowLongPtrW(hwnd, GWL_STYLE) != restore.style
                || SetWindowPlacement(hwnd, &raw const restore.placement) == 0
                || SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED
                        | SWP_NOACTIVATE
                        | SWP_NOMOVE
                        | SWP_NOOWNERZORDER
                        | SWP_NOSIZE
                        | SWP_NOZORDER,
                ) == 0
            {
                return Err(NativeOutputError::WindowFullscreen);
            }

            let mut restored = WINDOWPLACEMENT {
                length: restore.placement.length,
                ..WINDOWPLACEMENT::default()
            };
            let mut restored_window = RECT::default();
            if GetWindowPlacement(hwnd, &raw mut restored) == 0
                || GetWindowRect(hwnd, &raw mut restored_window) == 0
                || !restore_postconditions(&restore.placement, &restored, restored_window)
            {
                return Err(NativeOutputError::WindowFullscreen);
            }
        }
        Ok(())
    }

    fn restore_postconditions(
        requested: &WINDOWPLACEMENT,
        restored: &WINDOWPLACEMENT,
        restored_window: RECT,
    ) -> bool {
        // SetWindowPlacement is allowed to normalize showCmd and the normal
        // rectangle when monitor topology, work area, or per-monitor DPI has
        // changed. Require a valid observable placement and live positive
        // window extent instead of demanding byte-for-byte stale geometry.
        placement_is_usable(requested)
            && placement_is_usable(restored)
            && rect_has_positive_extent(restored_window)
    }

    fn placement_is_usable(placement: &WINDOWPLACEMENT) -> bool {
        const MAX_SHOW_WINDOW_COMMAND: u32 = 11;
        placement.length == u32::try_from(size_of::<WINDOWPLACEMENT>()).unwrap_or_default()
            && placement.showCmd <= MAX_SHOW_WINDOW_COMMAND
            && rect_has_positive_extent(placement.rcNormalPosition)
    }

    fn rect_has_positive_extent(rect: RECT) -> bool {
        rect_width(rect).is_ok() && rect_height(rect).is_ok()
    }

    fn fullscreen_postconditions(hwnd: HWND) -> Result<bool, NativeOutputError> {
        let monitor = monitor_rect(hwnd)?;
        let mut window = RECT::default();
        // SAFETY: both calls only inspect the live root window.
        let style = unsafe {
            if GetWindowRect(hwnd, &raw mut window) == 0 {
                return Err(NativeOutputError::WindowFullscreen);
            }
            GetWindowLongPtrW(hwnd, GWL_STYLE)
        };
        Ok(style == borderless_style(style) && rect_eq(window, monitor))
    }

    fn require_verified_fullscreen(hwnd: HWND) -> Result<bool, NativeOutputError> {
        retained_restore_status(fullscreen_postconditions(hwnd)?)
    }

    fn retained_restore_status(verified: bool) -> Result<bool, NativeOutputError> {
        if verified {
            Ok(true)
        } else {
            // A retained RestoreState means the host still owns a recovery
            // obligation. Never report a partially mutated HWND as safely
            // windowed; the caller must keep Exit fullscreen available.
            Err(NativeOutputError::WindowFullscreen)
        }
    }

    fn monitor_rect(hwnd: HWND) -> Result<RECT, NativeOutputError> {
        // SAFETY: the monitor is selected from a live root HWND; `info` uses
        // the exact Win32 structure size required by `GetMonitorInfoW`.
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if monitor.is_null() {
                return Err(NativeOutputError::WindowFullscreen);
            }
            let mut info = MONITORINFO {
                cbSize: u32::try_from(size_of::<MONITORINFO>())
                    .map_err(|_| NativeOutputError::WindowFullscreen)?,
                ..MONITORINFO::default()
            };
            if GetMonitorInfoW(monitor, &raw mut info) == 0 {
                return Err(NativeOutputError::WindowFullscreen);
            }
            rect_width(info.rcMonitor)?;
            rect_height(info.rcMonitor)?;
            Ok(info.rcMonitor)
        }
    }

    fn rect_width(rect: RECT) -> Result<i32, NativeOutputError> {
        rect.right
            .checked_sub(rect.left)
            .filter(|value| *value > 0)
            .ok_or(NativeOutputError::WindowFullscreen)
    }

    fn rect_height(rect: RECT) -> Result<i32, NativeOutputError> {
        rect.bottom
            .checked_sub(rect.top)
            .filter(|value| *value > 0)
            .ok_or(NativeOutputError::WindowFullscreen)
    }

    const fn rect_eq(left: RECT, right: RECT) -> bool {
        left.left == right.left
            && left.top == right.top
            && left.right == right.right
            && left.bottom == right.bottom
    }

    pub(super) fn borderless_style(style: isize) -> isize {
        let stripped = WS_OVERLAPPEDWINDOW | WS_MINIMIZE | WS_MAXIMIZE;
        style & !isize::try_from(stripped).unwrap_or_default()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn placement(show_cmd: u32, rect: RECT) -> WINDOWPLACEMENT {
            WINDOWPLACEMENT {
                length: u32::try_from(size_of::<WINDOWPLACEMENT>())
                    .expect("WINDOWPLACEMENT size fits u32"),
                showCmd: show_cmd,
                rcNormalPosition: rect,
                ..WINDOWPLACEMENT::default()
            }
        }

        fn restore_state() -> RestoreState {
            RestoreState {
                hwnd: 42,
                style: 0x00CF_0000,
                placement: placement(
                    3,
                    RECT {
                        left: 100,
                        top: 80,
                        right: 1_100,
                        bottom: 780,
                    },
                ),
            }
        }

        #[test]
        fn failed_enter_retains_recovery_snapshot_until_rollback_succeeds() {
            let restore = restore_state();
            let mut state = HostFullscreenState {
                restore: Some(restore),
            };

            let error = rollback_failed_enter_with(&mut state, restore, |_| {
                Err(NativeOutputError::WindowFullscreen)
            })
            .expect_err("failed rollback must be reported");
            assert_eq!(error, NativeOutputError::WindowFullscreen);
            assert_eq!(state.restore.map(|value| value.hwnd), Some(restore.hwnd));

            rollback_failed_enter_with(&mut state, restore, |_| Ok(()))
                .expect("successful rollback");
            assert!(state.restore.is_none());
        }

        #[test]
        fn retained_restore_never_reports_a_partially_mutated_host_as_windowed() {
            assert!(retained_restore_status(true).expect("verified fullscreen"));
            assert!(retained_restore_status(false).is_err());
        }

        #[test]
        fn restore_accepts_windows_normalized_show_state_and_geometry() {
            let requested = restore_state().placement;
            let normalized = placement(
                1,
                RECT {
                    left: 140,
                    top: 120,
                    right: 1_020,
                    bottom: 720,
                },
            );
            let actual_window = RECT {
                left: 140,
                top: 120,
                right: 1_036,
                bottom: 759,
            };

            assert!(restore_postconditions(
                &requested,
                &normalized,
                actual_window
            ));
        }

        #[test]
        fn restore_rejects_unusable_normalized_placement() {
            let requested = restore_state().placement;
            let invalid = placement(
                1,
                RECT {
                    left: 140,
                    top: 120,
                    right: 140,
                    bottom: 720,
                },
            );
            assert!(!restore_postconditions(
                &requested,
                &invalid,
                RECT {
                    left: 140,
                    top: 120,
                    right: 1_036,
                    bottom: 759,
                }
            ));
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_child {
    #![allow(unsafe_code)]

    use tauri::Window;
    use windows_sys::Win32::{
        Foundation::{HWND, POINT, RECT},
        Graphics::Gdi::ClientToScreen,
        UI::WindowsAndMessaging::{
            GWL_STYLE, GetClientRect, GetParent, GetTopWindow, GetWindowLongPtrW, HWND_TOP,
            SWP_NOACTIVATE, SWP_NOOWNERZORDER, SetWindowPos, WS_CHILD,
        },
    };

    use super::{NativeOutputBounds, NativeOutputError};

    pub(super) fn apply_exact_bounds(
        window: &Window,
        bounds: NativeOutputBounds,
    ) -> Result<(u32, u32), NativeOutputError> {
        let child = window
            .hwnd()
            .map_err(|_| NativeOutputError::WindowPlacement)?
            .0 as HWND;
        let width =
            i32::try_from(bounds.width()).map_err(|_| NativeOutputError::WindowPlacement)?;
        let height =
            i32::try_from(bounds.height()).map_err(|_| NativeOutputError::WindowPlacement)?;
        let child_style_flag =
            isize::try_from(WS_CHILD).map_err(|_| NativeOutputError::WindowPlacement)?;

        // SAFETY: `child` is obtained from a live owned Tauri Window. Every
        // queried handle and Win32 return value is validated before use. The
        // synchronous SetWindowPos call only mutates that WS_CHILD's placement
        // and sibling order inside its existing parent.
        unsafe {
            let parent = GetParent(child);
            if parent.is_null()
                || (GetWindowLongPtrW(child, GWL_STYLE) & child_style_flag) != child_style_flag
                || SetWindowPos(
                    child,
                    HWND_TOP,
                    bounds.x(),
                    bounds.y(),
                    width,
                    height,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER,
                ) == 0
            {
                return Err(NativeOutputError::WindowPlacement);
            }

            let mut client = RECT::default();
            let mut child_origin = POINT::default();
            let mut parent_origin = POINT::default();
            if GetClientRect(child, &raw mut client) == 0
                || ClientToScreen(child, &raw mut child_origin) == 0
                || ClientToScreen(parent, &raw mut parent_origin) == 0
            {
                return Err(NativeOutputError::WindowPlacement);
            }
            let actual_x = child_origin
                .x
                .checked_sub(parent_origin.x)
                .ok_or(NativeOutputError::WindowPlacement)?;
            let actual_y = child_origin
                .y
                .checked_sub(parent_origin.y)
                .ok_or(NativeOutputError::WindowPlacement)?;
            let actual_width = client
                .right
                .checked_sub(client.left)
                .ok_or(NativeOutputError::WindowPlacement)?;
            let actual_height = client
                .bottom
                .checked_sub(client.top)
                .ok_or(NativeOutputError::WindowPlacement)?;
            if actual_x != bounds.x()
                || actual_y != bounds.y()
                || actual_width != width
                || actual_height != height
                || GetTopWindow(parent) != child
            {
                return Err(NativeOutputError::WindowPlacement);
            }
        }

        Ok((bounds.width(), bounds.height()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResizeDecision {
    Suspend,
    Unchanged,
    Configure,
}

const fn resize_decision(current: Option<(u32, u32)>, width: u32, height: u32) -> ResizeDecision {
    if width == 0 || height == 0 {
        ResizeDecision::Suspend
    } else if matches!(current, Some((current_width, current_height)) if current_width == width && current_height == height)
    {
        ResizeDecision::Unchanged
    } else {
        ResizeDecision::Configure
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum GpuFault {
    Healthy = 0,
    Validation = 1,
    Internal = 2,
    OutOfMemory = 3,
    DeviceLost = 4,
}

#[derive(Clone)]
struct GpuHealth {
    fault: Arc<AtomicU8>,
}

impl GpuHealth {
    fn new() -> Self {
        Self {
            fault: Arc::new(AtomicU8::new(GpuFault::Healthy as u8)),
        }
    }

    fn record(&self, fault: GpuFault) {
        let _ = self.fault.fetch_max(fault as u8, Ordering::AcqRel);
    }

    fn check(&self) -> Result<(), NativeOutputError> {
        match self.fault.load(Ordering::Acquire) {
            value if value == GpuFault::Healthy as u8 => Ok(()),
            value if value == GpuFault::Validation as u8 => Err(NativeOutputError::GpuValidation),
            value if value == GpuFault::Internal as u8 => Err(NativeOutputError::GpuInternal),
            value if value == GpuFault::OutOfMemory as u8 => Err(NativeOutputError::GpuOutOfMemory),
            _ => Err(NativeOutputError::DeviceLost),
        }
    }
}

fn install_gpu_callbacks(device: &wgpu::Device, health: &GpuHealth) {
    let device_lost = health.clone();
    device.set_device_lost_callback(move |_reason, _message| {
        device_lost.record(GpuFault::DeviceLost);
    });

    let uncaptured = health.clone();
    device.on_uncaptured_error(Arc::new(move |error| {
        let fault = match error {
            wgpu::Error::OutOfMemory { .. } => GpuFault::OutOfMemory,
            wgpu::Error::Validation { .. } => GpuFault::Validation,
            wgpu::Error::Internal { .. } => GpuFault::Internal,
        };
        uncaptured.record(fault);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_identity_is_owned_and_application_configurable() {
        let label = String::from("test-native-output");
        let title = String::from("Test Output");
        let config = NativeOutputConfig::new(800, 448, label, title);
        assert_eq!(config.frame_width, 800);
        assert_eq!(config.frame_height, 448);
        assert_eq!(config.window_label(), "test-native-output");
        assert_eq!(config.window_title(), "Test Output");
    }

    #[test]
    fn renderer_policy_is_exactly_dx12_without_a_fallback_backend() {
        fn assert_send<T: Send>() {}
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send::<NativeOutput>();
        assert_send_sync::<HostFullscreenController>();
        assert_eq!(
            latentdeck_gpu::renderer::dx12_instance_descriptor().backends,
            wgpu::Backends::DX12
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn host_fullscreen_style_removes_frame_and_window_state_only() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            WS_CLIPCHILDREN, WS_MAXIMIZE, WS_MINIMIZE, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        let style = |bits| isize::try_from(bits).expect("Win32 styles fit isize");
        let original =
            style(WS_OVERLAPPEDWINDOW | WS_MINIMIZE | WS_MAXIMIZE | WS_CLIPCHILDREN | WS_VISIBLE);
        let fullscreen = windows_host::borderless_style(original);

        assert_eq!(
            fullscreen & style(WS_OVERLAPPEDWINDOW | WS_MINIMIZE | WS_MAXIMIZE),
            0
        );
        assert_ne!(fullscreen & style(WS_CLIPCHILDREN), 0);
        assert_ne!(fullscreen & style(WS_VISIBLE), 0);
    }

    #[test]
    fn device_identity_text_is_bounded_and_cannot_contain_a_path() {
        let value = sanitize_device_identity("NVIDIA GeForce RTX 4070 (AD104)");
        assert!(value.len() <= 96);
        assert_eq!(value, "NVIDIA GeForce RTX 4070 (AD104)");
        assert_eq!(
            sanitize_device_identity("NVIDIA C:\\Users\\owner\\driver.dll"),
            "unknown"
        );
        assert_eq!(sanitize_device_identity("token=private"), "unknown");
        assert_eq!(sanitize_device_identity("driver\nprivate"), "unknown");
        assert_eq!(sanitize_device_identity("///"), "unknown");
    }

    #[test]
    fn resize_policy_never_configures_zero_and_avoids_duplicate_configure() {
        assert_eq!(
            resize_decision(Some((800, 448)), 0, 448),
            ResizeDecision::Suspend
        );
        assert_eq!(
            resize_decision(Some((800, 448)), 800, 448),
            ResizeDecision::Unchanged
        );
        assert_eq!(resize_decision(None, 800, 448), ResizeDecision::Configure);
        assert_eq!(
            resize_decision(Some((800, 448)), 1_600, 900),
            ResizeDecision::Configure
        );
    }

    #[test]
    fn present_outcome_distinguishes_local_display_from_consumed_frames() {
        assert!(PresentOutcome::Presented.locally_presented());
        assert!(PresentOutcome::PresentedAndReconfigured.locally_presented());
        assert!(!PresentOutcome::SkippedZeroSized.locally_presented());
        assert!(!PresentOutcome::SkippedTimeout.locally_presented());
        assert!(!PresentOutcome::SkippedOccluded.locally_presented());
        assert!(!PresentOutcome::SkippedOutdated.locally_presented());
        assert!(!PresentOutcome::SkippedSurfaceRecreated.locally_presented());
    }

    #[test]
    fn upload_requires_the_exact_padded_rgb_ring_layout() {
        let layout = RingLayout::new(65, 2).expect("bounded test layout");
        let payload_bytes = usize::try_from(layout.payload_bytes()).expect("test payload fits");
        let bytes = vec![0_u8; payload_bytes];
        let upload = validated_upload(65, 2, layout.row_stride(), &bytes).expect("exact upload");
        assert_eq!(upload.bytes().len(), bytes.len());

        assert_eq!(
            validated_upload(65, 2, 65 * 4, &bytes).expect_err("tight stride must fail"),
            NativeOutputError::FrameRejected
        );
        assert_eq!(
            validated_upload(65, 2, layout.row_stride(), &bytes[..bytes.len() - 1])
                .expect_err("truncated padding must fail"),
            NativeOutputError::FrameRejected
        );
    }

    #[test]
    fn gpu_faults_are_sticky_and_device_loss_has_highest_priority() {
        let health = GpuHealth::new();
        assert_eq!(health.check(), Ok(()));
        health.record(GpuFault::Validation);
        assert_eq!(health.check(), Err(NativeOutputError::GpuValidation));
        health.record(GpuFault::DeviceLost);
        health.record(GpuFault::Internal);
        assert_eq!(health.check(), Err(NativeOutputError::DeviceLost));
    }

    #[test]
    fn public_errors_are_stable_and_do_not_embed_backend_diagnostics() {
        let cases = [
            (
                NativeOutputError::SurfaceCreate,
                "output.surface_create_failed",
                "native output surface could not be created",
            ),
            (
                NativeOutputError::DeviceLost,
                "output.device_lost",
                "native DX12 device was lost",
            ),
            (
                NativeOutputError::GpuValidation,
                "output.gpu_validation",
                "native DX12 output encountered a GPU validation failure",
            ),
            (
                NativeOutputError::WindowDestroy,
                "output.window_destroy_failed",
                "native output window could not be destroyed",
            ),
        ];

        for (error, code, message) in cases {
            assert_eq!(error.code(), code);
            assert_eq!(error.to_string(), message);
            assert!(!error.to_string().contains("adapter"));
            assert!(!error.to_string().contains("driver"));
            assert!(!error.to_string().contains('\\'));
        }
    }
}
