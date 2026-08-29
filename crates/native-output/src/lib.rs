//! Reusable raw Tauri window and DX12 presentation surface for decoded frames.
//!
//! This boundary deliberately contains no `WebView`, audio, seek state, codec
//! logic, application runtime, or backend fallback. An application actor owns
//! one [`NativeOutput`] and feeds it validated RGB Ring frames sequentially.

use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use latentdeck_gpu::{
    renderer::{Dx12Device, RgbaFrameRenderer, RgbaUpload, create_dx12_instance},
    ring::RingLayout,
};
use tauri::{AppHandle, Window, window::WindowBuilder};
use thiserror::Error;

/// Explicit window identity and decoded-program dimensions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOutputConfig {
    /// Decoded RGBA frame width, independent of the resizable swapchain.
    pub frame_width: u32,
    /// Decoded RGBA frame height, independent of the resizable swapchain.
    pub frame_height: u32,
    window_label: String,
    window_title: String,
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
        Self {
            frame_width,
            frame_height,
            window_label: window_label.into(),
            window_title: window_title.into(),
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

/// Explicit result of one decoded-frame presentation attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentOutcome {
    /// The frame was submitted and presented normally.
    Presented,
    /// The frame was presented, then a suboptimal surface was reconfigured.
    PresentedAndReconfigured,
    /// The window had a zero physical extent, so no swapchain image was used.
    SkippedZeroSized,
    /// Surface acquisition timed out; the caller may submit a later frame.
    SkippedTimeout,
    /// The window was occluded; the caller may submit after it is visible.
    SkippedOccluded,
    /// An outdated surface was reconfigured without presenting this frame.
    SkippedOutdated,
    /// A lost surface was safely recreated without presenting this frame.
    SkippedSurfaceRecreated,
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
            Self::WindowFullscreen => "output.window_fullscreen_failed",
            Self::SurfaceCreate => "output.surface_create_failed",
            Self::RendererInitialization => "output.renderer_initialization_failed",
            Self::FrameRejected => "output.frame_rejected",
            Self::SurfaceValidation => "output.surface_validation_failed",
            Self::DeviceLost => "output.device_lost",
            Self::GpuOutOfMemory => "output.gpu_out_of_memory",
            Self::GpuInternal => "output.gpu_internal",
            Self::GpuValidation => "output.gpu_validation",
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
    surface_configuration: wgpu::SurfaceConfiguration,
    surface_extent: Option<(u32, u32)>,
    gpu_health: GpuHealth,
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

        let output = Self {
            window,
            instance,
            surface,
            dx12,
            renderer,
            surface_configuration,
            surface_extent: Some((physical_size.width, physical_size.height)),
            gpu_health,
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

    /// Toggle fullscreen and return the newly requested state.
    ///
    /// # Errors
    ///
    /// Returns a sanitized Tauri window-operation failure.
    pub fn toggle_fullscreen(&self) -> Result<bool, NativeOutputError> {
        let fullscreen = self
            .window
            .is_fullscreen()
            .map_err(|_| NativeOutputError::WindowFullscreen)?;
        let next = !fullscreen;
        self.set_fullscreen(next)?;
        Ok(next)
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
        if self.surface_extent.is_none() {
            return Ok(PresentOutcome::SkippedZeroSized);
        }

        self.renderer
            .upload(self.dx12.queue(), upload)
            .map_err(|_| NativeOutputError::FrameRejected)?;
        self.poll_gpu_health()?;

        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => {
                self.render_and_present(texture)?;
                Ok(PresentOutcome::Presented)
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                self.render_and_present(texture)?;
                let (surface_width, surface_height) =
                    self.surface_extent.ok_or(NativeOutputError::WindowSize)?;
                self.configure_surface(surface_width, surface_height)?;
                Ok(PresentOutcome::PresentedAndReconfigured)
            }
            wgpu::CurrentSurfaceTexture::Timeout => Ok(PresentOutcome::SkippedTimeout),
            wgpu::CurrentSurfaceTexture::Occluded => Ok(PresentOutcome::SkippedOccluded),
            wgpu::CurrentSurfaceTexture::Outdated => {
                let (surface_width, surface_height) =
                    self.surface_extent.ok_or(NativeOutputError::WindowSize)?;
                self.configure_surface(surface_width, surface_height)?;
                Ok(PresentOutcome::SkippedOutdated)
            }
            wgpu::CurrentSurfaceTexture::Lost => {
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
        self.renderer.encode(&mut encoder, &view);
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
            Some(
                RgbaFrameRenderer::new(
                    self.dx12.device(),
                    next.format,
                    frame.width(),
                    frame.height(),
                )
                .map_err(|_| NativeOutputError::RendererInitialization)?,
            )
        };

        self.surface.configure(self.dx12.device(), &next);
        if let Some(renderer) = replacement_renderer {
            self.renderer = renderer;
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
        let frame = self.renderer.frame_layout();
        let renderer = RgbaFrameRenderer::new(
            self.dx12.device(),
            next.format,
            frame.width(),
            frame.height(),
        )
        .map_err(|_| NativeOutputError::RendererInitialization)?;
        replacement.configure(self.dx12.device(), &next);
        self.surface = replacement;
        self.surface_configuration = next;
        self.renderer = renderer;
        self.poll_gpu_health()
    }

    fn poll_gpu_health(&self) -> Result<(), NativeOutputError> {
        let _ = self.instance.poll_all(false);
        self.gpu_health.check()
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
        assert_send::<NativeOutput>();
        assert_eq!(
            latentdeck_gpu::renderer::dx12_instance_descriptor().backends,
            wgpu::Backends::DX12
        );
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
