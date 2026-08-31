//! Safe, Windows-only Spout2 sender state machine.
//!
//! The real backend is opt-in through the `spout-sdk` feature and accepts only
//! application-owned D3D12 textures. It never exposes a pixel upload or encoded
//! fallback. The sole native path is the official `SpoutDX12` `D3D11On12` wrapper.

#![deny(unsafe_code)]

use std::cell::Cell;
use std::ffi::c_void;
use std::marker::PhantomData;

use thiserror::Error;

#[cfg(all(windows, feature = "spout-sdk"))]
mod ffi;

#[cfg(windows)]
use windows::Win32::Graphics::Direct3D12::{ID3D12CommandQueue, ID3D12Device, ID3D12Resource};
#[cfg(windows)]
use windows::core::Interface;

/// Spout output is an explicit 0.1 release requirement.
pub const SPOUT_REQUIRED_FOR_RELEASE: bool = true;

/// Maximum requested sender-name length.
///
/// Spout uses a 256-byte name buffer and may append a collision suffix. The
/// smaller public bound leaves room for that suffix without invoking the
/// upstream invalid-parameter handler.
pub const MAX_SENDER_NAME_BYTES: usize = 240;

/// D3D12's maximum width or height for a two-dimensional texture.
pub const MAX_TEXTURE_DIMENSION: u32 = 16_384;

/// Texture formats supported without conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum SpoutFormat {
    /// `DXGI_FORMAT_R8G8B8A8_UNORM`.
    Rgba8Unorm = 28,
    /// `DXGI_FORMAT_B8G8R8A8_UNORM`.
    Bgra8Unorm = 87,
}

/// D3D12 state in which `D3D11On12` acquires the application texture.
///
/// The pinned official `SpoutDX12::WrapDX12Resource` hardcodes an output state
/// of `PRESENT`, which would desynchronize a reused wgpu texture tracked as
/// wgpu's combined pixel/non-pixel shader-resource state. The bridge therefore
/// uses the official
/// `ID3D11On12Device` with identical input/output states, then calls official
/// `SendDX11Resource`. The application must synchronize the direct queue and
/// provide the actual state; the bridge returns the resource to that same state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Dx12ResourceState {
    /// `D3D12_RESOURCE_STATE_COMMON` / `PRESENT`.
    CommonOrPresent = 0,
    /// `D3D12_RESOURCE_STATE_RENDER_TARGET`.
    RenderTarget = 0x4,
    /// `D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE`.
    PixelShaderResource = 0x80,
    /// `D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE |
    /// D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE`.
    ///
    /// This is the exact state used by pinned wgpu-hal 30 for a sampled
    /// `TextureUses::RESOURCE` texture.
    ShaderResource = 0xc0,
    /// `D3D12_RESOURCE_STATE_COPY_DEST`.
    CopyDestination = 0x400,
    /// `D3D12_RESOURCE_STATE_COPY_SOURCE`.
    CopySource = 0x800,
}

/// Validated immutable sender geometry and requested name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderConfig {
    name: String,
    width: u32,
    height: u32,
    format: SpoutFormat,
}

impl SenderConfig {
    /// Validate a sender configuration before native allocation.
    ///
    /// # Errors
    ///
    /// Returns [`SpoutError::InvalidName`] or [`SpoutError::InvalidDimensions`]
    /// when an input is outside the public contract.
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        format: SpoutFormat,
    ) -> Result<Self, SpoutError> {
        let name = name.into();
        validate_requested_name(&name)?;
        if width == 0
            || height == 0
            || width > MAX_TEXTURE_DIMENSION
            || height > MAX_TEXTURE_DIMENSION
        {
            return Err(SpoutError::InvalidDimensions { width, height });
        }
        Ok(Self {
            name,
            width,
            height,
            format,
        })
    }

    /// Requested sender name. The active name may gain a Spout collision suffix.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Exact texture width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Exact texture height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Exact texture format; no conversion is performed.
    #[must_use]
    pub const fn format(&self) -> SpoutFormat {
        self.format
    }
}

/// Sanitized native-backend failure categories.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BackendFault {
    /// The application device, queue, or source resource was rejected.
    #[error("the DX12 device, direct queue, or resource is incompatible")]
    IncompatibleDx12,
    /// The official `D3D11On12` context could not be opened.
    #[error("the SpoutDX12 D3D11On12 context could not be opened")]
    OpenFailed,
    /// The source texture does not exactly match configured geometry/format/state.
    #[error("the DX12 texture does not match the configured Spout surface")]
    ResourceMismatch,
    /// The bounded wrapped-resource cache is full.
    #[error("the bounded Spout wrapped-resource cache is full")]
    ResourceLimit,
    /// The official SDK could not wrap the D3D12 resource.
    #[error("SpoutDX12 could not wrap the DX12 resource")]
    WrapFailed,
    /// The official SDK rejected the wrapped-resource send.
    #[error("SpoutDX12 could not send the wrapped resource")]
    SendFailed,
    /// Native status violated the bridge contract.
    #[error("the Spout bridge returned inconsistent status")]
    StatusMismatch,
    /// A bounded internal native operation failed.
    #[error("the Spout bridge failed internally")]
    Internal,
}

/// Safe sender API failures. Messages intentionally contain no paths or handles.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SpoutError {
    /// The name is empty, non-ASCII, padded, or too long for safe collision suffixing.
    #[error(
        "sender name must be 1..={MAX_SENDER_NAME_BYTES} printable ASCII bytes without surrounding spaces"
    )]
    InvalidName,
    /// Width or height is outside the exact D3D12 `Texture2D` bound.
    #[error("invalid Spout surface dimensions {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    /// The source build is deliberately absent from the default build.
    #[error(
        "Spout2 SDK support is not built; prepare the pinned source and enable feature `spout-sdk`"
    )]
    SdkNotBuilt,
    /// Frames are rejected until the sender is explicitly enabled.
    #[error("Spout sender is disabled")]
    Disabled,
    /// Stop is terminal for a sender instance.
    #[error("Spout sender is stopped")]
    Stopped,
    /// App frame IDs must strictly increase; jumps are allowed for dropped frames.
    #[error("frame sequence {submitted} is not newer than {previous}")]
    NonMonotonicSequence { previous: u64, submitted: u64 },
    /// Sanitized backend category.
    #[error(transparent)]
    Backend(#[from] BackendFault),
}

/// Snapshot used by UI/control integrations without exposing COM handles.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SenderStatus {
    /// Whether the application has enabled frame submission.
    pub enabled: bool,
    /// Whether Spout registered the sender after a successful frame.
    pub published: bool,
    /// Whether this sender instance has been terminally stopped.
    pub stopped: bool,
    /// User-requested name.
    pub requested_name: String,
    /// Collision-resolved name reported by Spout.
    pub active_name: String,
    /// Exact output width.
    pub width: u32,
    /// Exact output height.
    pub height: u32,
    /// Exact output texture format.
    pub format: SpoutFormat,
    /// Number of successful submissions through this instance.
    pub submitted_frames: u64,
    /// Last successful application frame sequence.
    pub last_sequence: Option<u64>,
    /// Frame counter reported by Spout after publication.
    pub spout_frame: Option<i64>,
    /// Last sanitized native failure, cleared on successful backend work.
    pub last_fault: Option<BackendFault>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BackendStatus {
    active_name: String,
    published: bool,
    width: u32,
    height: u32,
    format: u32,
    spout_frame: Option<i64>,
}

trait Backend: Send {
    fn set_name(&mut self, name: &str) -> Result<BackendStatus, BackendFault>;
    fn release_sender(&mut self) -> Result<BackendStatus, BackendFault>;
    fn send(
        &mut self,
        resource: *mut c_void,
        initial_state: Dx12ResourceState,
    ) -> Result<BackendStatus, BackendFault>;
    fn close(&mut self);
}

/// Thread-affine safe wrapper around the `SpoutDX12` sender.
///
/// Construction opens `D3D11On12` on the application's D3D12 device and direct
/// queue. Publication is explicit: call [`Self::enable`] and submit a frame.
/// The type is `Send` so a single-owner async actor may migrate between worker
/// threads, but deliberately not `Sync`. The native backend enables
/// `ID3D11Multithread` protection and `&mut self` prevents concurrent calls.
///
/// ```compile_fail
/// fn require_sync<T: Sync>() {}
/// require_sync::<latentdeck_output_spout::SpoutSender>();
/// ```
pub struct SpoutSender {
    config: SenderConfig,
    backend: Box<dyn Backend>,
    status: SenderStatus,
    _not_sync: PhantomData<Cell<()>>,
}

impl SpoutSender {
    /// Open the real pinned `SpoutDX12` backend on application-owned objects.
    ///
    /// With the default feature set this returns [`SpoutError::SdkNotBuilt`]
    /// without touching the supplied COM objects.
    ///
    /// # Errors
    ///
    /// Returns [`SpoutError::SdkNotBuilt`] when the feature is absent or a
    /// sanitized [`BackendFault`] when the device/direct queue cannot be opened.
    #[cfg(windows)]
    pub fn open_dx12(
        config: SenderConfig,
        device: &ID3D12Device,
        direct_queue: &ID3D12CommandQueue,
    ) -> Result<Self, SpoutError> {
        #[cfg(feature = "spout-sdk")]
        {
            let backend = ffi::NativeBackend::open(&config, device, direct_queue)?;
            Ok(Self::from_backend(config, Box::new(backend)))
        }
        #[cfg(not(feature = "spout-sdk"))]
        {
            let _ = (config, device, direct_queue);
            Err(SpoutError::SdkNotBuilt)
        }
    }

    #[cfg(any(test, feature = "spout-sdk"))]
    fn from_backend(config: SenderConfig, backend: Box<dyn Backend>) -> Self {
        let status = SenderStatus {
            enabled: false,
            published: false,
            stopped: false,
            requested_name: config.name.clone(),
            active_name: config.name.clone(),
            width: config.width,
            height: config.height,
            format: config.format,
            submitted_frames: 0,
            last_sequence: None,
            spout_frame: None,
            last_fault: None,
        };
        Self {
            config,
            backend,
            status,
            _not_sync: PhantomData,
        }
    }

    /// Enable publication. Spout registers the sender on the first successful frame.
    ///
    /// # Errors
    ///
    /// Returns [`SpoutError::Stopped`] after terminal stop or a sanitized
    /// backend error when the sender name cannot be established.
    pub fn enable(&mut self) -> Result<SenderStatus, SpoutError> {
        if self.status.stopped {
            return Err(SpoutError::Stopped);
        }
        if self.status.enabled {
            return Ok(self.status());
        }
        match self.backend.set_name(&self.config.name) {
            Ok(native) => {
                self.apply_backend_status(native)?;
                self.status.enabled = true;
                self.status.last_fault = None;
                Ok(self.status())
            }
            Err(fault) => self.fail(fault),
        }
    }

    /// Toggle publication without destroying the `D3D11On12` context.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::enable`] or [`Self::disable`].
    pub fn set_enabled(&mut self, enabled: bool) -> Result<SenderStatus, SpoutError> {
        if enabled {
            self.enable()
        } else {
            self.disable()
        }
    }

    /// Unregister the sender while retaining device/queue and wrapped resources.
    ///
    /// # Errors
    ///
    /// Returns [`SpoutError::Stopped`] after terminal stop or a sanitized
    /// backend error when Spout cannot unregister the sender.
    pub fn disable(&mut self) -> Result<SenderStatus, SpoutError> {
        if self.status.stopped {
            return Err(SpoutError::Stopped);
        }
        if !self.status.enabled {
            return Ok(self.status());
        }
        match self.backend.release_sender() {
            Ok(native) => {
                self.apply_backend_status(native)?;
                self.status.enabled = false;
                self.status.published = false;
                self.status.spout_frame = None;
                self.status.last_fault = None;
                Ok(self.status())
            }
            Err(fault) => self.fail(fault),
        }
    }

    /// Set a validated name. A published sender is first unregistered and will
    /// reappear under the new collision-resolved name on the next frame.
    ///
    /// # Errors
    ///
    /// Returns [`SpoutError::InvalidName`], [`SpoutError::Stopped`], or a
    /// sanitized backend error.
    pub fn set_name(&mut self, name: impl Into<String>) -> Result<SenderStatus, SpoutError> {
        if self.status.stopped {
            return Err(SpoutError::Stopped);
        }
        let name = name.into();
        validate_requested_name(&name)?;
        match self.backend.set_name(&name) {
            Ok(native) => {
                self.config.name.clone_from(&name);
                self.status.requested_name = name;
                self.apply_backend_status(native)?;
                self.status.published = false;
                self.status.spout_frame = None;
                self.status.last_fault = None;
                Ok(self.status())
            }
            Err(fault) => self.fail(fault),
        }
    }

    /// Send one application-owned D3D12 texture through `D3D11On12`.
    ///
    /// `sequence` is application provenance, not Spout's internal frame count.
    /// It must strictly increase, but gaps are allowed when upstream drops a frame.
    ///
    /// # Errors
    ///
    /// Returns disabled/stopped/sequence errors before native work, or a
    /// sanitized backend error for incompatible/wrap/send failures.
    #[cfg(windows)]
    pub fn send_frame(
        &mut self,
        sequence: u64,
        resource: &ID3D12Resource,
        initial_state: Dx12ResourceState,
    ) -> Result<SenderStatus, SpoutError> {
        self.submit(sequence, resource.as_raw(), initial_state)
    }

    fn submit(
        &mut self,
        sequence: u64,
        resource: *mut c_void,
        initial_state: Dx12ResourceState,
    ) -> Result<SenderStatus, SpoutError> {
        if self.status.stopped {
            return Err(SpoutError::Stopped);
        }
        if !self.status.enabled {
            return Err(SpoutError::Disabled);
        }
        if let Some(previous) = self.status.last_sequence
            && sequence <= previous
        {
            return Err(SpoutError::NonMonotonicSequence {
                previous,
                submitted: sequence,
            });
        }
        match self.backend.send(resource, initial_state) {
            Ok(native) => {
                self.apply_backend_status(native)?;
                self.status.submitted_frames += 1;
                self.status.last_sequence = Some(sequence);
                self.status.last_fault = None;
                Ok(self.status())
            }
            Err(fault) => self.fail(fault),
        }
    }

    /// Return the last validated status snapshot without a native call.
    #[must_use]
    pub fn status(&self) -> SenderStatus {
        self.status.clone()
    }

    /// Terminally close the sender and retained wrapped resources. Idempotent.
    pub fn stop(&mut self) -> SenderStatus {
        if !self.status.stopped {
            self.backend.close();
            self.status.enabled = false;
            self.status.published = false;
            self.status.stopped = true;
            self.status.spout_frame = None;
        }
        self.status()
    }

    fn apply_backend_status(&mut self, native: BackendStatus) -> Result<(), SpoutError> {
        if native.active_name.is_empty()
            || native.active_name.len() > 255
            || !native
                .active_name
                .bytes()
                .all(|byte| (0x20..=0x7e).contains(&byte))
            || native.width != self.config.width
            || native.height != self.config.height
            || native.format != self.config.format as u32
        {
            return self.fail(BackendFault::StatusMismatch);
        }
        self.status.active_name = native.active_name;
        self.status.published = native.published;
        self.status.spout_frame = native.spout_frame;
        Ok(())
    }

    fn fail<T>(&mut self, fault: BackendFault) -> Result<T, SpoutError> {
        self.status.last_fault = Some(fault);
        Err(SpoutError::Backend(fault))
    }
}

impl Drop for SpoutSender {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_requested_name(name: &str) -> Result<(), SpoutError> {
    if name.is_empty()
        || name.len() > MAX_SENDER_NAME_BYTES
        || name.trim() != name
        || !name.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
    {
        return Err(SpoutError::InvalidName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    struct MockTrace {
        names: Vec<String>,
        releases: u32,
        sends: u32,
        closes: u32,
        fail_next_send: Option<BackendFault>,
    }

    struct MockBackend {
        trace: Arc<Mutex<MockTrace>>,
        name: String,
        width: u32,
        height: u32,
        format: u32,
        published: bool,
        frame: i64,
    }

    impl MockBackend {
        fn new(config: &SenderConfig, trace: Arc<Mutex<MockTrace>>) -> Self {
            Self {
                trace,
                name: config.name.clone(),
                width: config.width,
                height: config.height,
                format: config.format as u32,
                published: false,
                frame: 0,
            }
        }

        fn status(&self) -> BackendStatus {
            BackendStatus {
                active_name: self.name.clone(),
                published: self.published,
                width: self.width,
                height: self.height,
                format: self.format,
                spout_frame: self.published.then_some(self.frame),
            }
        }
    }

    impl Backend for MockBackend {
        fn set_name(&mut self, name: &str) -> Result<BackendStatus, BackendFault> {
            self.published = false;
            self.name = name.to_owned();
            self.trace
                .lock()
                .expect("trace lock")
                .names
                .push(name.to_owned());
            Ok(self.status())
        }

        fn release_sender(&mut self) -> Result<BackendStatus, BackendFault> {
            self.published = false;
            self.trace.lock().expect("trace lock").releases += 1;
            Ok(self.status())
        }

        fn send(
            &mut self,
            resource: *mut c_void,
            initial_state: Dx12ResourceState,
        ) -> Result<BackendStatus, BackendFault> {
            let _ = (resource, initial_state);
            let mut trace = self.trace.lock().expect("trace lock");
            if let Some(fault) = trace.fail_next_send.take() {
                return Err(fault);
            }
            trace.sends += 1;
            drop(trace);
            self.published = true;
            self.frame += 1;
            Ok(self.status())
        }

        fn close(&mut self) {
            self.published = false;
            self.trace.lock().expect("trace lock").closes += 1;
        }
    }

    fn sender(trace: Arc<Mutex<MockTrace>>) -> SpoutSender {
        let config = SenderConfig::new("LatentDeck", 800, 448, SpoutFormat::Bgra8Unorm)
            .expect("valid config");
        let backend = MockBackend::new(&config, trace);
        SpoutSender::from_backend(config, Box::new(backend))
    }

    #[test]
    fn config_rejects_names_and_dimensions_before_native_allocation() {
        for name in ["", " padded", "padded ", "line\nbreak", "Дека"] {
            assert_eq!(
                SenderConfig::new(name, 800, 448, SpoutFormat::Bgra8Unorm),
                Err(SpoutError::InvalidName)
            );
        }
        assert_eq!(
            SenderConfig::new(
                "x".repeat(MAX_SENDER_NAME_BYTES + 1),
                800,
                448,
                SpoutFormat::Bgra8Unorm
            ),
            Err(SpoutError::InvalidName)
        );
        assert_eq!(
            SenderConfig::new("ok", 0, 448, SpoutFormat::Bgra8Unorm),
            Err(SpoutError::InvalidDimensions {
                width: 0,
                height: 448
            })
        );
    }

    #[test]
    fn enable_send_sequence_status_and_stop_are_deterministic() {
        let trace = Arc::new(Mutex::new(MockTrace::default()));
        let mut sender = sender(Arc::clone(&trace));
        assert!(!sender.status().enabled);

        sender.enable().expect("enable");
        let first = sender
            .submit(41, ptr::null_mut(), Dx12ResourceState::RenderTarget)
            .expect("first frame");
        assert!(first.enabled);
        assert!(first.published);
        assert_eq!(first.submitted_frames, 1);
        assert_eq!(first.last_sequence, Some(41));
        assert_eq!(first.spout_frame, Some(1));

        assert_eq!(
            sender.submit(41, ptr::null_mut(), Dx12ResourceState::RenderTarget),
            Err(SpoutError::NonMonotonicSequence {
                previous: 41,
                submitted: 41
            })
        );
        let skipped = sender
            .submit(44, ptr::null_mut(), Dx12ResourceState::RenderTarget)
            .expect("sequence gap");
        assert_eq!(skipped.submitted_frames, 2);
        assert_eq!(skipped.last_sequence, Some(44));

        let stopped = sender.stop();
        assert!(stopped.stopped);
        assert!(!stopped.enabled);
        assert!(!stopped.published);
        assert_eq!(trace.lock().expect("trace lock").closes, 1);
        sender.stop();
        assert_eq!(trace.lock().expect("trace lock").closes, 1);
        assert_eq!(sender.enable(), Err(SpoutError::Stopped));
    }

    #[test]
    fn rename_and_disable_unregister_without_losing_sequence_history() {
        let trace = Arc::new(Mutex::new(MockTrace::default()));
        let mut sender = sender(Arc::clone(&trace));
        sender.enable().expect("enable");
        sender
            .submit(7, ptr::null_mut(), Dx12ResourceState::RenderTarget)
            .expect("publish");

        let renamed = sender.set_name("Deck Output").expect("rename");
        assert!(renamed.enabled);
        assert!(!renamed.published);
        assert_eq!(renamed.requested_name, "Deck Output");
        assert_eq!(renamed.active_name, "Deck Output");
        assert_eq!(renamed.last_sequence, Some(7));

        let disabled = sender.disable().expect("disable");
        assert!(!disabled.enabled);
        assert_eq!(
            sender.submit(8, ptr::null_mut(), Dx12ResourceState::RenderTarget),
            Err(SpoutError::Disabled)
        );
        sender.enable().expect("re-enable");
        sender
            .submit(8, ptr::null_mut(), Dx12ResourceState::RenderTarget)
            .expect("resume sequence");
        assert_eq!(trace.lock().expect("trace lock").sends, 2);
    }

    #[test]
    fn backend_failure_is_sanitized_and_does_not_commit_sequence() {
        let trace = Arc::new(Mutex::new(MockTrace::default()));
        let mut sender = sender(Arc::clone(&trace));
        sender.enable().expect("enable");
        trace.lock().expect("trace lock").fail_next_send = Some(BackendFault::WrapFailed);
        assert_eq!(
            sender.submit(5, ptr::null_mut(), Dx12ResourceState::RenderTarget),
            Err(SpoutError::Backend(BackendFault::WrapFailed))
        );
        assert_eq!(sender.status().last_sequence, None);
        assert_eq!(sender.status().submitted_frames, 0);
        assert_eq!(sender.status().last_fault, Some(BackendFault::WrapFailed));
        sender
            .submit(5, ptr::null_mut(), Dx12ResourceState::RenderTarget)
            .expect("retry same sequence");
    }

    #[test]
    fn drop_closes_backend_once() {
        let trace = Arc::new(Mutex::new(MockTrace::default()));
        {
            let _sender = sender(Arc::clone(&trace));
        }
        assert_eq!(trace.lock().expect("trace lock").closes, 1);
    }

    #[test]
    fn sender_is_send_for_single_owner_async_actor_migration() {
        fn assert_send<T: Send>() {}
        assert_send::<SpoutSender>();
    }

    fn native_bridge_source() -> String {
        include_str!("../native/spout_bridge.cpp").replace("\r\n", "\n")
    }

    #[test]
    fn native_bridge_has_only_the_wrapped_texture_send_path() {
        let source = native_bridge_source();
        assert!(source.contains("SendDX11Resource"));
        assert!(source.contains("GetD3D11On12device"));
        assert!(source.contains("CreateWrappedResource"));
        assert!(source.contains("SetMultithreadProtected(TRUE)"));
        assert!(source.contains("exact_state,\n        exact_state"));
        assert!(source.contains("GetD3D11context"));
        assert!(!source.contains("spout.WrapDX12Resource"));
        assert!(!source.contains("SendImage"));
    }

    #[test]
    fn shader_resource_state_matches_the_pinned_wgpu_dx12_mapping() {
        assert_eq!(Dx12ResourceState::ShaderResource as u32, 0xc0);
        let source = native_bridge_source();
        assert!(source.contains(
            "D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE |\n    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE"
        ));
        assert!(source.contains("state == kShaderResourceState"));
    }
}
