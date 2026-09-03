//! Production actor for one exact installed Deck + Codec Protocol 2 session.
//!
//! Package roots, operator entrypoints, cartridge paths, and external asset
//! paths are resolved and retained before this boundary. The actor owns the
//! authenticated worker, RGB Ring ABI 2 consumer, and one hidden native output
//! window. Visibility is controlled only by the app-level foreground lease.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant as StdInstant},
};

use latentdeck_control::v2::{
    Ack, CaptureArtifact, CaptureIdentity, CaptureMode, CaptureStart, CaptureState,
    CaptureStatusSnapshot, Command, ControlBinding, DeckControlsSet, DeckProcess, DeckReset,
    DeckRolesSet, DeckSeedSet, DeckState, DeckStatusSnapshot, DeckTransportSet, EmptyPayload,
    ErrorCode, LimitedVec, MAX_CONTROLS, MAX_ROLES, MAX_SOURCES, RoleBinding, ShutdownReason,
    SourceBinding, SourceTransportBinding,
};
use latentdeck_core::{
    deck_runtime_v2::DeckLoadRequest,
    deck_selection_v2::PreparedDeckSelectionV2,
    deck_session_v2::{
        DeckSessionV2, DeckSessionV2Error, DeckSessionV2LoadRequest,
        start_deck_session_v2_with_retained_assets,
    },
    realtime_diagnostics::{
        DiagnosticCodecIdentity, DiagnosticGpuIdentity, Protocol2DeckSessionIdentity,
        RealtimeSessionMetrics, SanitizedToken, Sha256Token,
    },
    worker_client_v2::WorkerClientV2Error,
    worker_source_v2::WorkerSourceV2Error,
    worker_supervisor::WorkerSupervisorError,
};
use latentdeck_gpu::{
    ring::RingLayout,
    ring_v2::{ReadV2Status, RgbaBatchV2},
};
use latentdeck_native_output::{
    NativeOutput, NativeOutputConfig, NativeSpoutStatus, ResizeOutcome,
};
use latentdeck_output_mp4::RecorderStatus;
use serde::Serialize;
use tauri::{AppHandle, WebviewWindow, async_runtime::JoinHandle as TauriJoinHandle};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{Instant, sleep_until, timeout},
};
use uuid::Uuid;

use crate::{
    capture_finalizer_v2::{
        CaptureArtifactEvidence, CaptureFinalizationContext, CaptureFinalizerError,
        CaptureSourceEvidence, CaptureStagingRoot, finalize_capture_with_carrier,
    },
    decoded_recording::{DecodedRecordingController, DecodedRecordingError},
    embedded_viewport::EmbeddedViewport,
    library_state::LibraryImporter,
    runtime_diagnostics::{
        PreparedProtocol2DeckDiagnosticIdentity, PresentationDiagnosticState,
        Protocol2DeckDiagnosticIdentity, Protocol2DeckDiagnosticSelection, diagnostic_gpu_identity,
        protocol2_metrics_from_ack, realtime_metrics_v2,
    },
};

const CHANNEL_CAPACITY: usize = 16;
const ACTOR_DEADLINE: Duration = Duration::from_secs(130);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);
const MAX_DECODE_BATCH: u32 = 24;
const MAX_CAPTURE_LATENT_SLOTS: u64 = 16_382;
const MAX_CAPTURE_VISUAL_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_JSON_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GenericDeckRuntimeError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    fatal: bool,
}

impl GenericDeckRuntimeError {
    const fn new(code: &'static str, message: &'static str, fatal: bool) -> Self {
        Self {
            code,
            message,
            fatal,
        }
    }

    const fn input() -> Self {
        Self::new(
            "deck.input_invalid",
            "The exact Deck controls, roles, transport, or seed are invalid.",
            false,
        )
    }

    const fn protocol() -> Self {
        Self::new(
            "deck.protocol_fault",
            "The Protocol 2 worker returned a response outside the negotiated Deck contract.",
            true,
        )
    }

    const fn worker() -> Self {
        Self::new(
            "deck.worker_fault",
            "The isolated Protocol 2 worker stopped unexpectedly.",
            true,
        )
    }

    const fn timeout() -> Self {
        Self::new(
            "deck.worker_timeout",
            "The isolated Protocol 2 worker did not answer within the bounded deadline.",
            true,
        )
    }

    const fn output() -> Self {
        Self::new(
            "output.unavailable",
            "The native decoded output is unavailable for this session.",
            true,
        )
    }

    const fn ring() -> Self {
        Self::new(
            "output.ring_fault",
            "The decoded RGB Ring ABI 2 contract was violated.",
            true,
        )
    }

    const fn diagnostics() -> Self {
        Self::new(
            "diagnostics.contract_invalid",
            "The bounded Protocol 2 runtime diagnostics violated the negotiated contract.",
            true,
        )
    }

    const fn closed() -> Self {
        Self::new(
            "session.not_found",
            "The requested generic Deck session is no longer running.",
            false,
        )
    }

    const fn capture(code: &'static str, message: &'static str) -> Self {
        Self::new(code, message, false)
    }

    const fn remote(code: &'static str, message: &'static str, fatal: bool) -> Self {
        Self::new(code, message, fatal)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericCaptureView {
    pub(crate) capture_id: Option<String>,
    pub(crate) mode: Option<CaptureMode>,
    pub(crate) state: String,
    pub(crate) latent_slots: String,
    pub(crate) reset_events: u32,
    pub(crate) cartridge_id: Option<String>,
    pub(crate) archive_sha256: Option<String>,
    pub(crate) detail: Option<String>,
}

impl Default for GenericCaptureView {
    fn default() -> Self {
        Self {
            capture_id: None,
            mode: None,
            state: "idle".to_owned(),
            latent_slots: "0".to_owned(),
            reset_events: 0,
            cartridge_id: None,
            archive_sha256: None,
            detail: None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericDeckRuntimeView {
    pub(crate) status: DeckStatusSnapshot,
    pub(crate) output_visible: bool,
    pub(crate) fault_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericDeckRuntimeDiagnostics {
    pub(crate) gpu: DiagnosticGpuIdentity,
    pub(crate) codec: DiagnosticCodecIdentity,
    pub(crate) session: Protocol2DeckSessionIdentity,
    pub(crate) operator: SanitizedToken,
    pub(crate) source_archive_sha256: Vec<Sha256Token>,
    pub(crate) metrics: RealtimeSessionMetrics,
}

pub(crate) struct GenericDeckRuntime {
    sender: mpsc::Sender<RuntimeCommand>,
    worker_pid: u32,
    closed: Arc<AtomicBool>,
    output_visible: Arc<AtomicBool>,
    status: Arc<Mutex<DeckStatusSnapshot>>,
    fault_code: Arc<Mutex<Option<String>>>,
    capture_status: Arc<Mutex<GenericCaptureView>>,
    recording: DecodedRecordingController,
    cleanup_complete: watch::Receiver<bool>,
    _task: TauriJoinHandle<()>,
}

impl GenericDeckRuntime {
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn start(
        app: AppHandle,
        parent: WebviewWindow,
        viewport: EmbeddedViewport,
        prepared: PreparedDeckSelectionV2,
        load: DeckSessionV2LoadRequest,
        app_local_data: PathBuf,
        library_importer: LibraryImporter,
    ) -> Result<Self, GenericDeckRuntimeError> {
        prevalidate_load(&prepared, &load)?;
        let deck_session_id = prepared.host.deck_session_id;
        let ring_id = prepared.host.ring_id;
        let command_timeout = prepared.host.command_timeout;
        let dimensions = (
            prepared.host.signal_geometry.decoded_width,
            prepared.host.signal_geometry.decoded_height,
        );
        let frame_clock = FrameClock::new(
            prepared.host.signal_geometry.frame_rate_numerator,
            prepared.host.signal_geometry.frame_rate_denominator,
        )?;
        let slot_counts = prepared
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                u8::try_from(index + 1)
                    .map(|slot| (slot, source.latent_slot_count))
                    .map_err(|_| GenericDeckRuntimeError::input())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let capture_sources = prepared
            .cartridges
            .iter()
            .zip(&prepared.sources)
            .enumerate()
            .map(|(index, (cartridge, source))| {
                Ok(CaptureSourceEvidence {
                    physical_slot: u8::try_from(index + 1)
                        .map_err(|_| GenericDeckRuntimeError::input())?,
                    archive_sha256: source.archive_sha256.clone(),
                    manifest: cartridge.manifest().clone(),
                })
            })
            .collect::<Result<Vec<_>, GenericDeckRuntimeError>>()?;
        let operator_id = prepared
            .deck_runtime
            .operator_descriptor()
            .operator_id
            .clone();
        let operator_version = prepared
            .deck_runtime
            .operator_descriptor()
            .operator_version
            .clone();
        let structural_carrier_role = match prepared.deck_runtime.active_package().manifest() {
            latentdeck_extension_manager::PackageManifest::Deck(manifest) => {
                manifest.signal.structural_carrier_role.clone()
            }
            latentdeck_extension_manager::PackageManifest::Codec(_) => {
                return Err(GenericDeckRuntimeError::input());
            }
        };
        let codec_manifest = match prepared.codec_package.manifest() {
            latentdeck_extension_manager::PackageManifest::Codec(manifest) => manifest,
            latentdeck_extension_manager::PackageManifest::Deck(_) => {
                return Err(GenericDeckRuntimeError::input());
            }
        };
        let diagnostic_identity =
            PreparedProtocol2DeckDiagnosticIdentity::new(Protocol2DeckDiagnosticSelection {
                profile: &prepared.host.profile_key,
                codec_pack: &codec_manifest.pack_id,
                codec_pack_version: &codec_manifest.pack_version,
                adapter: &codec_manifest.adapter.adapter_id,
                adapter_version: &codec_manifest.adapter.adapter_version,
                compute_device: prepared.host.tensor_abi.device,
                device_ordinal: prepared.host.device_ordinal,
                external_assets: &prepared.external_assets,
                deck_session_id,
                deck_package: prepared
                    .deck_runtime
                    .active_package()
                    .manifest()
                    .package_id(),
                deck_package_version: prepared
                    .deck_runtime
                    .active_package()
                    .manifest()
                    .package_version(),
                operator: &operator_id,
                operator_version: &operator_version,
                frame_rate_numerator: prepared.host.signal_geometry.frame_rate_numerator,
                frame_rate_denominator: prepared.host.signal_geometry.frame_rate_denominator,
            })
            .map_err(|_| GenericDeckRuntimeError::diagnostics())?;
        let diagnostic_source_sha256 = prepared
            .sources
            .iter()
            .map(|source| Sha256Token::parse(&source.archive_sha256))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| GenericDeckRuntimeError::diagnostics())?;

        let mut session = start_deck_session_v2_with_retained_assets(
            prepared.codec_package,
            prepared.deck_runtime,
            prepared.cartridges,
            prepared.host,
            prepared.external_assets,
            prepared.retained_external_assets,
            load,
        )
        .await
        .map_err(map_startup_error)?;
        let worker_pid = session.client().worker_pid();
        let initial_status = session.initial_status().clone();
        validate_loaded_status(&initial_status, deck_session_id, &slot_counts)?;
        let diagnostic_identity = diagnostic_identity
            .complete(session.client_mut().session_id().as_uuid())
            .map_err(|_| GenericDeckRuntimeError::diagnostics())?;

        let output_label = format!("latentdeck-generic-output-{}", deck_session_id.simple());
        let output_title = "LatentDeck Generic Output";
        let bounds = viewport
            .bounds()
            .ok_or_else(GenericDeckRuntimeError::output)?;
        let Ok(output) = NativeOutput::new_embedded(
            &app,
            &parent,
            NativeOutputConfig::new(dimensions.0, dimensions.1, output_label, output_title)
                .with_spout_sender_name(format!("LatentDeck Generic {}", deck_session_id.simple())),
            bounds,
        )
        .await
        else {
            let _ = session
                .client_mut()
                .request_shutdown(ShutdownReason::ProtocolFault, SHUTDOWN_DEADLINE)
                .await;
            return Err(GenericDeckRuntimeError::output());
        };
        if output.hide().is_err() {
            let _ = output.destroy();
            let _ = session
                .client_mut()
                .request_shutdown(ShutdownReason::ProtocolFault, SHUTDOWN_DEADLINE)
                .await;
            return Err(GenericDeckRuntimeError::output());
        }
        if output.frame_dimensions() != dimensions {
            let _ = output.destroy();
            let _ = session
                .client_mut()
                .request_shutdown(ShutdownReason::ProtocolFault, SHUTDOWN_DEADLINE)
                .await;
            return Err(GenericDeckRuntimeError::output());
        }
        let initial_spout = output.spout_status();
        let presentation_diagnostics = PresentationDiagnosticState::new(&initial_spout);

        let status = Arc::new(Mutex::new(initial_status.clone()));
        let fault_code = Arc::new(Mutex::new(None));
        let capture_status = Arc::new(Mutex::new(GenericCaptureView::default()));
        let recording = DecodedRecordingController::new();
        let output_visible = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
        let (cleanup_sender, cleanup_complete) = watch::channel(false);
        let actor_closed = Arc::clone(&closed);
        let actor_fault = Arc::clone(&fault_code);
        let actor = RuntimeActor {
            session,
            output,
            status: initial_status,
            status_view: Arc::clone(&status),
            fault_code: actor_fault,
            output_visible: Arc::clone(&output_visible),
            ring_id,
            dimensions,
            slot_counts,
            frame_clock,
            command_timeout,
            capture_status: Arc::clone(&capture_status),
            app_local_data,
            library_importer,
            capture_sources,
            operator_id,
            operator_version,
            structural_carrier_role,
            diagnostic_identity,
            diagnostic_source_sha256,
            started_at: StdInstant::now(),
            presentation_diagnostics,
            active_capture: None,
            recording: recording.clone(),
            pending_frames: VecDeque::new(),
            viewport,
            foreground: false,
            spout_requested_enabled: initial_spout.enabled,
            worker_stopped: false,
        };
        let task = tauri::async_runtime::spawn(async move {
            actor.run(receiver).await;
            actor_closed.store(true, Ordering::Release);
            cleanup_sender.send_replace(true);
        });

        Ok(Self {
            sender,
            worker_pid,
            closed,
            output_visible,
            status,
            fault_code,
            capture_status,
            recording,
            cleanup_complete,
            _task: task,
        })
    }

    #[must_use]
    pub(crate) const fn worker_pid(&self) -> u32 {
        self.worker_pid
    }

    #[must_use]
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub(crate) fn view(&self) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        let status = self
            .status
            .lock()
            .map_err(|_| GenericDeckRuntimeError::worker())?
            .clone();
        let fault_code = self
            .fault_code
            .lock()
            .map_err(|_| GenericDeckRuntimeError::worker())?
            .clone();
        Ok(GenericDeckRuntimeView {
            status,
            output_visible: self.output_visible.load(Ordering::Acquire),
            fault_code,
        })
    }

    pub(crate) async fn process_once(
        &self,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::ProcessOnce { reply })
            .await
    }

    pub(crate) async fn controls_set(
        &self,
        controls: Vec<ControlBinding>,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::ControlsSet { controls, reply })
            .await
    }

    pub(crate) async fn roles_set(
        &self,
        roles: Vec<RoleBinding>,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::RolesSet { roles, reply })
            .await
    }

    pub(crate) async fn transport_set(
        &self,
        sources: Vec<SourceTransportBinding>,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::TransportSet { sources, reply })
            .await
    }

    pub(crate) async fn seed_set(
        &self,
        seed: u64,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::SeedSet { seed, reply })
            .await
    }

    pub(crate) async fn reset(
        &self,
        preserve_playheads: bool,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::Reset {
            preserve_playheads,
            reply,
        })
        .await
    }

    pub(crate) async fn set_foreground(
        &self,
        foreground: bool,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::SetForeground { foreground, reply })
            .await
    }

    pub(crate) async fn set_viewport(
        &self,
        viewport: EmbeddedViewport,
    ) -> Result<ResizeOutcome, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::SetViewport { viewport, reply })
            .await
    }

    pub(crate) async fn spout_status(&self) -> Result<NativeSpoutStatus, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::SpoutStatus { reply })
            .await
    }

    pub(crate) async fn configure_spout(
        &self,
        name: Option<String>,
        enabled: Option<bool>,
    ) -> Result<NativeSpoutStatus, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::ConfigureSpout {
            name,
            enabled,
            reply,
        })
        .await
    }

    pub(crate) async fn capture_start(
        &self,
        mode: CaptureMode,
        output: PathBuf,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::CaptureStart {
            mode,
            output,
            reply,
        })
        .await
    }

    pub(crate) async fn capture_stop(&self) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::CaptureStop { reply })
            .await
    }

    pub(crate) async fn capture_status(
        &self,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::CaptureStatus { reply })
            .await
    }

    pub(crate) fn cached_capture_status(
        &self,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        self.capture_status
            .lock()
            .map_err(|_| GenericDeckRuntimeError::worker())
            .map(|view| view.clone())
    }

    pub(crate) async fn recording_start(
        &self,
        output: PathBuf,
    ) -> Result<RecorderStatus, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::RecordingStart { output, reply })
            .await
    }

    pub(crate) async fn recording_stop(&self) -> Result<RecorderStatus, GenericDeckRuntimeError> {
        let recording = self.recording.clone();
        tauri::async_runtime::spawn_blocking(move || recording.stop())
            .await
            .map_err(|_| GenericDeckRuntimeError::worker())?
            .map_err(map_recording_error)
    }

    pub(crate) fn recording_status(&self) -> RecorderStatus {
        self.recording.status()
    }

    pub(crate) async fn diagnostics(
        &self,
    ) -> Result<GenericDeckRuntimeDiagnostics, GenericDeckRuntimeError> {
        self.request(|reply| RuntimeCommand::Diagnostics { reply })
            .await
    }

    pub(crate) async fn shutdown(&self) -> Result<(), GenericDeckRuntimeError> {
        if self.is_closed() {
            return Ok(());
        }
        let result = self
            .request(|reply| RuntimeCommand::Shutdown { reply })
            .await;
        let mut cleanup = self.cleanup_complete.clone();
        if !*cleanup.borrow() {
            timeout(SHUTDOWN_DEADLINE, cleanup.changed())
                .await
                .map_err(|_| GenericDeckRuntimeError::timeout())?
                .map_err(|_| GenericDeckRuntimeError::worker())?;
        }
        result
    }

    async fn request<T>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, GenericDeckRuntimeError>>) -> RuntimeCommand,
    ) -> Result<T, GenericDeckRuntimeError> {
        if self.is_closed() {
            return Err(GenericDeckRuntimeError::closed());
        }
        let (reply, receiver) = oneshot::channel();
        timeout(ACTOR_DEADLINE, self.sender.send(command(reply)))
            .await
            .map_err(|_| GenericDeckRuntimeError::timeout())?
            .map_err(|_| GenericDeckRuntimeError::closed())?;
        timeout(ACTOR_DEADLINE, receiver)
            .await
            .map_err(|_| GenericDeckRuntimeError::timeout())?
            .map_err(|_| GenericDeckRuntimeError::closed())?
    }
}

enum RuntimeCommand {
    ProcessOnce {
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    ControlsSet {
        controls: Vec<ControlBinding>,
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    RolesSet {
        roles: Vec<RoleBinding>,
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    TransportSet {
        sources: Vec<SourceTransportBinding>,
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    SeedSet {
        seed: u64,
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    Reset {
        preserve_playheads: bool,
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    SetForeground {
        foreground: bool,
        reply: oneshot::Sender<Result<GenericDeckRuntimeView, GenericDeckRuntimeError>>,
    },
    SetViewport {
        viewport: EmbeddedViewport,
        reply: oneshot::Sender<Result<ResizeOutcome, GenericDeckRuntimeError>>,
    },
    SpoutStatus {
        reply: oneshot::Sender<Result<NativeSpoutStatus, GenericDeckRuntimeError>>,
    },
    ConfigureSpout {
        name: Option<String>,
        enabled: Option<bool>,
        reply: oneshot::Sender<Result<NativeSpoutStatus, GenericDeckRuntimeError>>,
    },
    CaptureStart {
        mode: CaptureMode,
        output: PathBuf,
        reply: oneshot::Sender<Result<GenericCaptureView, GenericDeckRuntimeError>>,
    },
    CaptureStop {
        reply: oneshot::Sender<Result<GenericCaptureView, GenericDeckRuntimeError>>,
    },
    CaptureStatus {
        reply: oneshot::Sender<Result<GenericCaptureView, GenericDeckRuntimeError>>,
    },
    RecordingStart {
        output: PathBuf,
        reply: oneshot::Sender<Result<RecorderStatus, GenericDeckRuntimeError>>,
    },
    Diagnostics {
        reply: oneshot::Sender<Result<GenericDeckRuntimeDiagnostics, GenericDeckRuntimeError>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), GenericDeckRuntimeError>>,
    },
}

struct RuntimeActor {
    session: DeckSessionV2,
    output: NativeOutput,
    status: DeckStatusSnapshot,
    status_view: Arc<Mutex<DeckStatusSnapshot>>,
    fault_code: Arc<Mutex<Option<String>>>,
    output_visible: Arc<AtomicBool>,
    ring_id: Uuid,
    dimensions: (u32, u32),
    slot_counts: Vec<(u8, u64)>,
    frame_clock: FrameClock,
    command_timeout: Duration,
    capture_status: Arc<Mutex<GenericCaptureView>>,
    app_local_data: PathBuf,
    library_importer: LibraryImporter,
    capture_sources: Vec<CaptureSourceEvidence>,
    operator_id: String,
    operator_version: String,
    structural_carrier_role: String,
    diagnostic_identity: Protocol2DeckDiagnosticIdentity,
    diagnostic_source_sha256: Vec<Sha256Token>,
    started_at: StdInstant,
    presentation_diagnostics: PresentationDiagnosticState,
    active_capture: Option<ActiveCapture>,
    recording: DecodedRecordingController,
    pending_frames: VecDeque<Vec<u8>>,
    viewport: EmbeddedViewport,
    foreground: bool,
    spout_requested_enabled: bool,
    worker_stopped: bool,
}

struct ActiveCapture {
    capture_id: Uuid,
    mode: CaptureMode,
    state: CaptureState,
    binding: Option<CaptureStagingRoot>,
    output: PathBuf,
}

impl RuntimeActor {
    async fn run(mut self, mut receiver: mpsc::Receiver<RuntimeCommand>) {
        loop {
            if self.processing_active() && self.pending_frames.is_empty() {
                // Give one already-queued mutation priority before a comparatively
                // long worker/GPU call.  Once the command lane is clear, decode
                // the next bounded batch immediately instead of waiting until the
                // presentation deadline and making decode time visible as a hitch.
                match receiver.try_recv() {
                    Ok(command) => {
                        let was_processing = self.processing_active();
                        if self.handle(command).await {
                            break;
                        }
                        self.restart_clock_on_resume(was_processing);
                        continue;
                    }
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        let _ = self.stop(ShutdownReason::HostExit).await;
                        break;
                    }
                    Err(mpsc::error::TryRecvError::Empty) => {}
                }
                if let Err(error) = self.process_once().await {
                    self.fail(error);
                    break;
                }
                continue;
            }

            let command = if self.processing_active() {
                let frame_deadline = match self.frame_clock.next_deadline() {
                    Ok(deadline) => deadline,
                    Err(error) => {
                        self.fail(error);
                        break;
                    }
                };
                tokio::select! {
                    biased;
                    () = sleep_until(frame_deadline) => {
                        if let Err(error) = self.frame_clock.advance_past(Instant::now()) {
                            self.fail(error);
                            break;
                        }
                        if let Err(error) = self.present_once() {
                            self.fail(error);
                            break;
                        }
                        continue;
                    },
                    command = receiver.recv() => command,
                }
            } else {
                receiver.recv().await
            };
            let Some(command) = command else {
                let _ = self.stop(ShutdownReason::HostExit).await;
                break;
            };
            let was_processing = self.processing_active();
            if self.handle(command).await {
                break;
            }
            self.restart_clock_on_resume(was_processing);
        }
        if !self.worker_stopped {
            let _ = self.stop(ShutdownReason::ProtocolFault).await;
        }
        self.settle_capture_on_shutdown();
        let _ = self.output.hide();
        let _ = self.output.destroy();
        self.output_visible.store(false, Ordering::Release);
    }

    fn processing_active(&self) -> bool {
        transport_active(&self.status) || self.capture_drives_processing()
    }

    fn restart_clock_on_resume(&mut self, was_processing: bool) {
        if !was_processing && self.processing_active() {
            self.frame_clock.restart();
            self.presentation_diagnostics.cut_interval();
        }
    }

    async fn handle(&mut self, command: RuntimeCommand) -> bool {
        match command {
            RuntimeCommand::ProcessOnce { reply } => {
                let result = self.tick().await.and_then(|()| self.view());
                finish(reply, result)
            }
            RuntimeCommand::ControlsSet { controls, reply } => {
                let result = self.controls_set(controls).await;
                finish(reply, result)
            }
            RuntimeCommand::RolesSet { roles, reply } => {
                let result = self.roles_set(roles).await;
                finish(reply, result)
            }
            RuntimeCommand::TransportSet { sources, reply } => {
                let result = self.transport_set(sources).await;
                finish(reply, result)
            }
            RuntimeCommand::SeedSet { seed, reply } => {
                let result = self.seed_set(seed).await;
                finish(reply, result)
            }
            RuntimeCommand::Reset {
                preserve_playheads,
                reply,
            } => {
                let result = self.reset(preserve_playheads).await;
                finish(reply, result)
            }
            RuntimeCommand::SetForeground { foreground, reply } => {
                let result = self.set_foreground(foreground).and_then(|()| self.view());
                finish(reply, result)
            }
            RuntimeCommand::SetViewport { viewport, reply } => {
                let result = self.set_viewport(viewport);
                finish(reply, result)
            }
            RuntimeCommand::SpoutStatus { reply } => finish(reply, Ok(self.output.spout_status())),
            RuntimeCommand::ConfigureSpout {
                name,
                enabled,
                reply,
            } => {
                let result = self.configure_spout(name, enabled);
                finish(reply, result)
            }
            RuntimeCommand::CaptureStart {
                mode,
                output,
                reply,
            } => {
                let result = self.capture_start(mode, output).await;
                finish(reply, result)
            }
            RuntimeCommand::CaptureStop { reply } => {
                let result = self.capture_stop().await;
                finish(reply, result)
            }
            RuntimeCommand::CaptureStatus { reply } => {
                let result = self.capture_status().await;
                finish(reply, result)
            }
            RuntimeCommand::RecordingStart { output, reply } => {
                let result = self.recording_start(output);
                finish(reply, result)
            }
            RuntimeCommand::Diagnostics { reply } => {
                let result = self.diagnostics().await;
                finish(reply, result)
            }
            RuntimeCommand::Shutdown { reply } => {
                let result = self.stop(ShutdownReason::HostExit).await;
                let _ = reply.send(result);
                true
            }
        }
    }

    async fn tick(&mut self) -> Result<(), GenericDeckRuntimeError> {
        if self.pending_frames.is_empty() {
            self.process_once().await?;
        }
        self.present_once()
    }

    fn present_once(&mut self) -> Result<(), GenericDeckRuntimeError> {
        let frame = self
            .pending_frames
            .pop_front()
            .ok_or_else(GenericDeckRuntimeError::ring)?;
        let layout = RingLayout::new(self.dimensions.0, self.dimensions.1)
            .map_err(|_| GenericDeckRuntimeError::ring())?;
        let padded = pad_tight_rgba(
            &frame,
            self.dimensions.0,
            self.dimensions.1,
            layout.row_stride(),
        )?;
        let outcome = self
            .output
            .present_padded_rgba(
                self.dimensions.0,
                self.dimensions.1,
                layout.row_stride(),
                &padded,
            )
            .map_err(|_| GenericDeckRuntimeError::output())?;
        self.presentation_diagnostics
            .observe_runtime_outcome(outcome, StdInstant::now());
        let _ = self.recording.submit_if_active(
            self.dimensions.0,
            self.dimensions.1,
            layout.row_stride(),
            &padded,
        );
        Ok(())
    }

    async fn process_once(&mut self) -> Result<(), GenericDeckRuntimeError> {
        let previous = self.status.clone();
        let ack = self
            .session
            .client_mut()
            .call(
                Command::DeckProcess(DeckProcess {
                    deck_session_id: previous.deck_session_id,
                    deck_revision: previous.deck_revision,
                    stream_generation: previous.stream_generation,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_client_error)?;
        let Ack::DeckProcess(processed) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_process_status(&previous, &processed.status, &self.slot_counts)?;
        if processed.output_ring_id != self.ring_id || processed.output_slot_sequence == 0 {
            return Err(GenericDeckRuntimeError::protocol());
        }
        let batch = match self
            .session
            .ring_consumer_mut()
            .try_read()
            .map_err(|_| GenericDeckRuntimeError::ring())?
        {
            ReadV2Status::Batch(batch) => batch,
            ReadV2Status::Empty => return Err(GenericDeckRuntimeError::ring()),
        };
        validate_batch(
            &batch,
            &processed.status,
            processed.output_slot_sequence,
            self.dimensions,
        )?;
        self.pending_frames = split_batch(&batch)?;
        let looped = looped_physical_slots(&previous, &processed.status, &self.slot_counts)?;
        self.status = processed.status;
        if self.active_capture.is_some() {
            self.refresh_capture_status().await?;
        }
        if !looped.is_empty() {
            self.causal_loop_reset().await?;
        }
        self.publish_status()
    }

    async fn causal_loop_reset(&mut self) -> Result<(), GenericDeckRuntimeError> {
        let before = self.status.clone();
        let (generation, reset) = causal_loop_reset_request(&before)?;
        let ack = self
            .session
            .client_mut()
            .call(Command::DeckReset(reset), self.command_timeout)
            .await
            .map_err(map_client_error)?;
        let Ack::DeckReset(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_reset_status(&before, &status, generation, true, &self.slot_counts)?;
        self.session
            .adopt_ring_generation(generation)
            .map_err(|_| GenericDeckRuntimeError::ring())?;
        self.frame_clock.restart();
        self.presentation_diagnostics.cut_interval();
        self.status = *status;
        Ok(())
    }

    async fn controls_set(
        &mut self,
        controls: Vec<ControlBinding>,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        prevalidate_dynamic_state(&self.session, self.status.roles.as_slice(), &controls)?;
        let expected = controls;
        let controls = LimitedVec::<_, MAX_CONTROLS>::try_from_vec(expected.clone())
            .map_err(|_| GenericDeckRuntimeError::input())?;
        let ack = self
            .session
            .client_mut()
            .call(
                Command::DeckControlsSet(DeckControlsSet {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    controls,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_client_error)?;
        let Ack::DeckControlsSet(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_mutation_status(&self.status, &status, &self.slot_counts)?;
        if status.controls.as_slice() != expected {
            return Err(GenericDeckRuntimeError::protocol());
        }
        self.pending_frames.clear();
        self.status = *status;
        self.publish_status()?;
        self.view()
    }

    async fn roles_set(
        &mut self,
        roles: Vec<RoleBinding>,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        prevalidate_dynamic_state(&self.session, &roles, self.status.controls.as_slice())?;
        let expected = roles;
        let roles = LimitedVec::<_, MAX_ROLES>::try_from_vec(expected.clone())
            .map_err(|_| GenericDeckRuntimeError::input())?;
        let ack = self
            .session
            .client_mut()
            .call(
                Command::DeckRolesSet(DeckRolesSet {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    roles,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_client_error)?;
        let Ack::DeckRolesSet(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_mutation_status(&self.status, &status, &self.slot_counts)?;
        if status.roles.as_slice() != expected {
            return Err(GenericDeckRuntimeError::protocol());
        }
        self.pending_frames.clear();
        self.status = *status;
        self.publish_status()?;
        self.view()
    }

    async fn transport_set(
        &mut self,
        sources: Vec<SourceTransportBinding>,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        validate_transport(&sources, self.status.playheads.len())?;
        let expected = sources;
        let sources = LimitedVec::<_, MAX_SOURCES>::try_from_vec(expected.clone())
            .map_err(|_| GenericDeckRuntimeError::input())?;
        let ack = self
            .session
            .client_mut()
            .call(
                Command::DeckTransportSet(DeckTransportSet {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    sources,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_client_error)?;
        let Ack::DeckTransportSet(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_mutation_status(&self.status, &status, &self.slot_counts)?;
        if status.source_transport.as_slice() != expected {
            return Err(GenericDeckRuntimeError::protocol());
        }
        self.pending_frames.clear();
        self.status = *status;
        self.publish_status()?;
        self.view()
    }

    async fn seed_set(
        &mut self,
        seed: u64,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        if seed > 9_007_199_254_740_991 {
            return Err(GenericDeckRuntimeError::input());
        }
        let ack = self
            .session
            .client_mut()
            .call(
                Command::DeckSeedSet(DeckSeedSet {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    seed,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_client_error)?;
        let Ack::DeckSeedSet(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_mutation_status(&self.status, &status, &self.slot_counts)?;
        if status.seed != seed {
            return Err(GenericDeckRuntimeError::protocol());
        }
        self.pending_frames.clear();
        self.status = *status;
        self.publish_status()?;
        self.view()
    }

    async fn reset(
        &mut self,
        preserve_playheads: bool,
    ) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        if !manual_reset_allowed(self.status.capture_state) {
            return Err(GenericDeckRuntimeError::input());
        }
        let before = self.status.clone();
        let generation = before
            .stream_generation
            .checked_add(1)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let ack = self
            .session
            .client_mut()
            .call(
                Command::DeckReset(DeckReset {
                    deck_session_id: before.deck_session_id,
                    deck_revision: before.deck_revision,
                    new_stream_generation: generation,
                    preserve_playheads,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_client_error)?;
        let Ack::DeckReset(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_reset_status(
            &before,
            &status,
            generation,
            preserve_playheads,
            &self.slot_counts,
        )?;
        self.session
            .adopt_ring_generation(generation)
            .map_err(|_| GenericDeckRuntimeError::ring())?;
        self.pending_frames.clear();
        self.frame_clock.restart();
        self.presentation_diagnostics.cut_interval();
        self.status = *status;
        self.publish_status()?;
        self.view()
    }

    async fn capture_start(
        &mut self,
        mode: CaptureMode,
        output: PathBuf,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        validate_capture_start_availability(self.active_capture.is_some(), &self.recording)?;
        let capture_id = Uuid::new_v4();
        let binding = CaptureStagingRoot::create(&self.app_local_data, capture_id)
            .map_err(map_capture_finalizer_error)?;
        let staging_root = binding
            .root()
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                GenericDeckRuntimeError::capture(
                    "capture.staging_root_invalid",
                    "The host-owned capture staging root cannot be represented safely.",
                )
            })?
            .to_owned();
        let ack = self
            .session
            .client_mut()
            .call(
                Command::CaptureStart(CaptureStart {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    capture_id,
                    mode,
                    staging_root,
                    maximum_latent_slots: MAX_CAPTURE_LATENT_SLOTS,
                    maximum_visual_bytes: MAX_CAPTURE_VISUAL_BYTES,
                    maximum_reset_events: 32,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_capture_client_error)?;
        let Ack::CaptureStart(status) = ack else {
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_capture_status(
            &status,
            &self.status,
            capture_id,
            mode,
            &[CaptureState::Capturing],
        )?;
        if status.latent_slots != 0 || status.reset_events != 0 {
            return Err(GenericDeckRuntimeError::protocol());
        }
        self.active_capture = Some(ActiveCapture {
            capture_id,
            mode,
            state: status.state,
            binding: Some(binding),
            output,
        });
        self.adopt_capture_deck_state(status.state);
        let view = capture_view(&status, None, None, None);
        self.publish_capture(view.clone())?;
        self.publish_status()?;
        Ok(view)
    }

    async fn capture_stop(&mut self) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        let (capture_id, mode) = self
            .active_capture
            .as_ref()
            .map(|active| (active.capture_id, active.mode))
            .ok_or_else(|| {
                GenericDeckRuntimeError::capture(
                    "capture.not_active",
                    "No latent capture is active in this Deck session.",
                )
            })?;
        if mode != CaptureMode::LiveCapture {
            return Err(GenericDeckRuntimeError::capture(
                "capture.stop_invalid",
                "Snapshot capture finishes automatically at its first codec-valid boundary.",
            ));
        }
        let ack = self
            .session
            .client_mut()
            .call(
                Command::CaptureStop(CaptureIdentity {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    capture_id,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_capture_client_error)?;
        let Ack::CaptureStop(status) = ack else {
            self.fail_active_capture("The worker returned an invalid capture acknowledgement.");
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_capture_status(
            &status,
            &self.status,
            capture_id,
            mode,
            &[CaptureState::Finalizing, CaptureState::Completed],
        )?;
        self.apply_capture_status(*status).await
    }

    async fn capture_status(&mut self) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        if self.active_capture.is_some() {
            self.refresh_capture_status().await
        } else {
            self.capture_status
                .lock()
                .map_err(|_| GenericDeckRuntimeError::worker())
                .map(|view| view.clone())
        }
    }

    async fn refresh_capture_status(
        &mut self,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        let (capture_id, mode) = self
            .active_capture
            .as_ref()
            .map(|active| (active.capture_id, active.mode))
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let ack = self
            .session
            .client_mut()
            .call(
                Command::CaptureStatus(CaptureIdentity {
                    deck_session_id: self.status.deck_session_id,
                    deck_revision: self.status.deck_revision,
                    capture_id,
                }),
                self.command_timeout,
            )
            .await
            .map_err(map_capture_client_error)?;
        let Ack::CaptureStatus(status) = ack else {
            self.fail_active_capture("The worker returned an invalid capture status.");
            return Err(GenericDeckRuntimeError::protocol());
        };
        validate_capture_status(
            &status,
            &self.status,
            capture_id,
            mode,
            &[
                CaptureState::Capturing,
                CaptureState::Finalizing,
                CaptureState::Completed,
                CaptureState::Aborted,
                CaptureState::Faulted,
            ],
        )?;
        self.apply_capture_status(*status).await
    }

    async fn apply_capture_status(
        &mut self,
        status: CaptureStatusSnapshot,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        let previous = self
            .active_capture
            .as_ref()
            .map(|active| active.state)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let mode = self
            .active_capture
            .as_ref()
            .map(|active| active.mode)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        if !valid_capture_status_transition(previous, status.state, mode) {
            self.fail_active_capture("The worker returned an invalid capture state transition.");
            return Err(GenericDeckRuntimeError::protocol());
        }
        if let Some(active) = self.active_capture.as_mut() {
            active.state = status.state;
        }
        self.adopt_capture_deck_state(status.state);
        match status.state {
            CaptureState::Completed => self.finalize_completed_capture(status).await,
            CaptureState::Aborted | CaptureState::Faulted => {
                self.active_capture = None;
                let detail = if status.state == CaptureState::Aborted {
                    "The worker aborted latent capture safely."
                } else {
                    "The worker capture writer failed and its staging was cleaned."
                };
                let state = if status.state == CaptureState::Aborted {
                    "aborted"
                } else {
                    "error"
                };
                let view = capture_view(&status, Some(state), None, Some(detail));
                self.publish_capture(view.clone())?;
                self.publish_status()?;
                Ok(view)
            }
            CaptureState::Capturing | CaptureState::Finalizing => {
                let view = capture_view(&status, None, None, None);
                self.publish_capture(view.clone())?;
                self.publish_status()?;
                Ok(view)
            }
            CaptureState::Idle | CaptureState::Starting => Err(GenericDeckRuntimeError::protocol()),
        }
    }

    async fn finalize_completed_capture(
        &mut self,
        status: CaptureStatusSnapshot,
    ) -> Result<GenericCaptureView, GenericDeckRuntimeError> {
        let artifact = status
            .artifact
            .as_ref()
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        validate_capture_artifact_bounds(artifact)?;
        if artifact.latent_slots != status.latent_slots {
            return Err(GenericDeckRuntimeError::protocol());
        }
        let mut active = self
            .active_capture
            .take()
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let binding = active
            .binding
            .take()
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let pending = capture_view(
            &status,
            Some("finalizing"),
            None,
            Some("Validating and importing the codec-staged cartridge."),
        );
        self.publish_capture(pending)?;
        let context = CaptureFinalizationContext {
            sources: self.capture_sources.clone(),
            roles: self.status.roles.as_slice().to_vec(),
            controls: self.status.controls.as_slice().to_vec(),
            operator_id: self.operator_id.clone(),
            operator_version: self.operator_version.clone(),
            seed: self.status.seed,
        };
        let result = finalize_capture_with_carrier(
            binding,
            capture_artifact_evidence(active.capture_id, artifact),
            active.mode,
            status.reset_events,
            context,
            &self.structural_carrier_role,
            MAX_CAPTURE_LATENT_SLOTS,
            MAX_CAPTURE_VISUAL_BYTES,
            u64::from(MAX_DECODE_BATCH),
            active.output,
            self.library_importer.clone(),
        )
        .await;
        match result {
            Ok(finalized) => {
                let view = capture_view(
                    &status,
                    Some("finished"),
                    Some((finalized.cartridge_id, finalized.archive_sha256)),
                    Some("Validated cartridge saved and imported into the Library."),
                );
                self.publish_capture(view.clone())?;
                self.publish_status()?;
                Ok(view)
            }
            Err(error) => {
                let view = capture_view(&status, Some("error"), None, Some(error.message));
                self.publish_capture(view)?;
                Err(map_capture_finalizer_error(error))
            }
        }
    }

    fn capture_drives_processing(&self) -> bool {
        self.active_capture.as_ref().is_some_and(|capture| {
            (capture.mode == CaptureMode::Snapshot && capture.state == CaptureState::Capturing)
                || capture.state == CaptureState::Finalizing
        })
    }

    fn adopt_capture_deck_state(&mut self, state: CaptureState) {
        self.status.capture_state = state;
        self.status.state = if matches!(state, CaptureState::Capturing | CaptureState::Finalizing) {
            DeckState::Capturing
        } else if transport_active(&self.status) {
            DeckState::Playing
        } else {
            DeckState::Paused
        };
    }

    fn publish_capture(&self, view: GenericCaptureView) -> Result<(), GenericDeckRuntimeError> {
        *self
            .capture_status
            .lock()
            .map_err(|_| GenericDeckRuntimeError::worker())? = view;
        Ok(())
    }

    fn fail_active_capture(&mut self, detail: &'static str) {
        let Some(active) = self.active_capture.take() else {
            return;
        };
        let status = CaptureStatusSnapshot {
            deck_session_id: self.status.deck_session_id,
            deck_revision: self.status.deck_revision,
            capture_id: active.capture_id,
            state: CaptureState::Faulted,
            mode: active.mode,
            latent_slots: 0,
            reset_events: 0,
            artifact: None,
        };
        let _ = self.publish_capture(capture_view(&status, Some("error"), None, Some(detail)));
        self.adopt_capture_deck_state(CaptureState::Faulted);
        let _ = self.publish_status();
    }

    fn settle_capture_on_shutdown(&mut self) {
        let Some(active) = self.active_capture.take() else {
            return;
        };
        let status = CaptureStatusSnapshot {
            deck_session_id: self.status.deck_session_id,
            deck_revision: self.status.deck_revision,
            capture_id: active.capture_id,
            state: CaptureState::Aborted,
            mode: active.mode,
            latent_slots: 0,
            reset_events: 0,
            artifact: None,
        };
        let _ = self.publish_capture(capture_view(
            &status,
            Some("aborted"),
            None,
            Some("Capture ended because the Deck runtime stopped."),
        ));
        self.adopt_capture_deck_state(CaptureState::Aborted);
        let _ = self.publish_status();
    }

    fn recording_start(
        &mut self,
        output: PathBuf,
    ) -> Result<RecorderStatus, GenericDeckRuntimeError> {
        if self.active_capture.is_some() {
            return Err(GenericDeckRuntimeError::capture(
                "recording.capture_conflict",
                "Finish latent Snapshot or Live Capture before recording decoded MP4.",
            ));
        }
        self.recording.arm(output).map_err(map_recording_error)
    }

    async fn diagnostics(
        &mut self,
    ) -> Result<GenericDeckRuntimeDiagnostics, GenericDeckRuntimeError> {
        let ack = self
            .session
            .client_mut()
            .call(Command::MetricsGet(EmptyPayload {}), self.command_timeout)
            .await
            .map_err(map_client_error)?;
        let worker =
            protocol2_metrics_from_ack(ack).map_err(|_| GenericDeckRuntimeError::diagnostics())?;
        let presentation = self
            .presentation_diagnostics
            .snapshot(&self.output.spout_status())
            .map_err(|_| GenericDeckRuntimeError::diagnostics())?;
        let duration_ms = u64::try_from(self.started_at.elapsed().as_millis())
            .map_err(|_| GenericDeckRuntimeError::diagnostics())?;
        let metrics = realtime_metrics_v2(
            duration_ms,
            self.diagnostic_identity.target_fps,
            &worker,
            presentation,
        )
        .map_err(|_| GenericDeckRuntimeError::diagnostics())?;
        Ok(GenericDeckRuntimeDiagnostics {
            gpu: diagnostic_gpu_identity(&self.output.device_identity())
                .map_err(|_| GenericDeckRuntimeError::diagnostics())?,
            codec: self.diagnostic_identity.codec.clone(),
            session: self.diagnostic_identity.session.clone(),
            operator: self.diagnostic_identity.operator.clone(),
            source_archive_sha256: self.diagnostic_source_sha256.clone(),
            metrics,
        })
    }

    fn set_foreground(&mut self, foreground: bool) -> Result<(), GenericDeckRuntimeError> {
        self.output
            .set_spout_enabled(effective_spout_enabled(
                foreground,
                self.spout_requested_enabled,
            ))
            .map_err(|_| GenericDeckRuntimeError::output())?;
        let visible = foreground && self.viewport.visible();
        if visible {
            self.output.show()
        } else {
            self.output.hide()
        }
        .map_err(|_| GenericDeckRuntimeError::output())?;
        self.foreground = foreground;
        self.output_visible.store(visible, Ordering::Release);
        Ok(())
    }

    fn set_viewport(
        &mut self,
        viewport: EmbeddedViewport,
    ) -> Result<ResizeOutcome, GenericDeckRuntimeError> {
        if viewport.revision() <= self.viewport.revision() {
            return Ok(ResizeOutcome::Unchanged);
        }
        let outcome = match viewport.bounds() {
            Some(bounds) => self.output.set_embedded_bounds(bounds),
            None => self.output.resize(0, 0),
        }
        .map_err(|_| GenericDeckRuntimeError::output())?;
        self.viewport = viewport;
        let visible = self.foreground && viewport.visible();
        if visible {
            self.output.show()
        } else {
            self.output.hide()
        }
        .map_err(|_| GenericDeckRuntimeError::output())?;
        self.output_visible.store(visible, Ordering::Release);
        Ok(outcome)
    }

    fn configure_spout(
        &mut self,
        name: Option<String>,
        enabled: Option<bool>,
    ) -> Result<NativeSpoutStatus, GenericDeckRuntimeError> {
        if let Some(name) = name {
            self.output
                .set_spout_name(name)
                .map_err(|_| GenericDeckRuntimeError::output())?;
        }
        if let Some(enabled) = enabled {
            if enabled && !self.foreground {
                return Err(GenericDeckRuntimeError::remote(
                    "session.output_lease_not_owned",
                    "Only the exact foreground Deck session may enable Spout publication.",
                    false,
                ));
            }
            self.output
                .set_spout_enabled(enabled)
                .map_err(|_| GenericDeckRuntimeError::output())?;
            self.spout_requested_enabled = enabled;
        }
        let status = self.output.spout_status();
        self.presentation_diagnostics.observe_spout(&status);
        Ok(status)
    }

    async fn stop(&mut self, reason: ShutdownReason) -> Result<(), GenericDeckRuntimeError> {
        if self.worker_stopped {
            return Ok(());
        }
        self.worker_stopped = true;
        let _ = self.recording.stop();
        self.session
            .client_mut()
            .request_shutdown(reason, SHUTDOWN_DEADLINE)
            .await
            .map(|_| ())
            .map_err(map_client_error)
    }

    fn publish_status(&self) -> Result<(), GenericDeckRuntimeError> {
        *self
            .status_view
            .lock()
            .map_err(|_| GenericDeckRuntimeError::worker())? = self.status.clone();
        Ok(())
    }

    fn view(&self) -> Result<GenericDeckRuntimeView, GenericDeckRuntimeError> {
        Ok(GenericDeckRuntimeView {
            status: self.status.clone(),
            output_visible: self.output_visible.load(Ordering::Acquire),
            fault_code: self
                .fault_code
                .lock()
                .map_err(|_| GenericDeckRuntimeError::worker())?
                .clone(),
        })
    }

    fn fail(&self, error: GenericDeckRuntimeError) {
        if let Ok(mut fault) = self.fault_code.lock() {
            *fault = Some(error.code.to_owned());
        }
    }
}

const fn effective_spout_enabled(foreground: bool, requested: bool) -> bool {
    foreground && requested
}

fn prevalidate_load(
    prepared: &PreparedDeckSelectionV2,
    load: &DeckSessionV2LoadRequest,
) -> Result<(), GenericDeckRuntimeError> {
    if load.seed > 9_007_199_254_740_991
        || load.controls.len() != prepared.deck_runtime.operator_descriptor().controls.len()
    {
        return Err(GenericDeckRuntimeError::input());
    }
    validate_transport(&load.source_transport, prepared.sources.len())?;
    let sources = dummy_sources(&load.source_transport, &prepared.sources)?;
    prepared
        .deck_runtime
        .build_load_command(DeckLoadRequest {
            deck_session_id: prepared.host.deck_session_id,
            sources,
            roles: load.roles.clone(),
            controls: load.controls.clone(),
            seed: load.seed,
            stream_generation: prepared.host.stream_generation,
        })
        .map(|_| ())
        .map_err(|_| GenericDeckRuntimeError::input())
}

fn prevalidate_dynamic_state(
    session: &DeckSessionV2,
    roles: &[RoleBinding],
    controls: &[ControlBinding],
) -> Result<(), GenericDeckRuntimeError> {
    if controls.len() != session.deck_runtime().operator_descriptor().controls.len() {
        return Err(GenericDeckRuntimeError::input());
    }
    let sources = session
        .cartridges()
        .iter()
        .enumerate()
        .map(|(index, cartridge)| {
            Ok(SourceBinding {
                physical_slot: u8::try_from(index + 1)
                    .map_err(|_| GenericDeckRuntimeError::input())?,
                source_id: Uuid::new_v4(),
                cartridge_id: Uuid::parse_str(&cartridge.manifest().cartridge_id.0)
                    .map_err(|_| GenericDeckRuntimeError::input())?,
                archive_sha256: cartridge.receipt().archive_sha256.to_string(),
                profile_receipt_id: Uuid::new_v4(),
                loop_enabled: false,
            })
        })
        .collect::<Result<Vec<_>, GenericDeckRuntimeError>>()?;
    session
        .deck_runtime()
        .build_load_command(DeckLoadRequest {
            deck_session_id: Uuid::new_v4(),
            sources,
            roles: roles.to_vec(),
            controls: controls.to_vec(),
            seed: 0,
            stream_generation: 1,
        })
        .map(|_| ())
        .map_err(|_| GenericDeckRuntimeError::input())
}

fn dummy_sources(
    transport: &[SourceTransportBinding],
    facts: &[latentdeck_core::deck_selection_v2::DeckSourceFactsV2],
) -> Result<Vec<SourceBinding>, GenericDeckRuntimeError> {
    facts
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let physical_slot =
                u8::try_from(index + 1).map_err(|_| GenericDeckRuntimeError::input())?;
            let loop_enabled = transport
                .iter()
                .find(|binding| binding.physical_slot == physical_slot)
                .ok_or_else(GenericDeckRuntimeError::input)?
                .loop_enabled;
            Ok(SourceBinding {
                physical_slot,
                source_id: Uuid::new_v4(),
                cartridge_id: Uuid::parse_str(&source.cartridge_id)
                    .map_err(|_| GenericDeckRuntimeError::input())?,
                archive_sha256: source.archive_sha256.clone(),
                profile_receipt_id: Uuid::new_v4(),
                loop_enabled,
            })
        })
        .collect()
}

fn validate_transport(
    transport: &[SourceTransportBinding],
    source_count: usize,
) -> Result<(), GenericDeckRuntimeError> {
    if source_count == 0 || source_count > MAX_SOURCES || transport.len() != source_count {
        return Err(GenericDeckRuntimeError::input());
    }
    let mut slots = transport
        .iter()
        .map(|source| source.physical_slot)
        .collect::<Vec<_>>();
    slots.sort_unstable();
    let maximum = u8::try_from(source_count).map_err(|_| GenericDeckRuntimeError::input())?;
    if slots != (1..=maximum).collect::<Vec<_>>() {
        return Err(GenericDeckRuntimeError::input());
    }
    Ok(())
}

fn validate_loaded_status(
    status: &DeckStatusSnapshot,
    session_id: Uuid,
    slot_counts: &[(u8, u64)],
) -> Result<(), GenericDeckRuntimeError> {
    if status.deck_session_id != session_id
        || status.deck_revision != 1
        || status.stream_generation != 1
        || status.stream_sequence != 0
        || status.playheads.is_empty()
        || status.playheads.len() > MAX_SOURCES
        || matches!(
            status.state,
            DeckState::Empty | DeckState::Loading | DeckState::Faulted
        )
        || status.capture_state != CaptureState::Idle
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    validate_playhead_bounds(status, slot_counts)
}

fn validate_process_status(
    previous: &DeckStatusSnapshot,
    current: &DeckStatusSnapshot,
    slot_counts: &[(u8, u64)],
) -> Result<(), GenericDeckRuntimeError> {
    if current.deck_session_id != previous.deck_session_id
        || current.deck_revision != previous.deck_revision
        || current.stream_generation != previous.stream_generation
        || previous
            .stream_sequence
            .checked_add(1)
            .is_none_or(|expected| expected != current.stream_sequence)
        || current.roles != previous.roles
        || current.controls != previous.controls
        || current.source_transport != previous.source_transport
        || current.seed != previous.seed
        || !valid_process_capture_transition(previous.capture_state, current.capture_state)
        || matches!(
            current.state,
            DeckState::Empty | DeckState::Loading | DeckState::Faulted
        )
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    validate_playhead_bounds(current, slot_counts)
}

const fn valid_process_capture_transition(previous: CaptureState, current: CaptureState) -> bool {
    matches!(
        (previous, current),
        (CaptureState::Idle, CaptureState::Idle)
            | (
                CaptureState::Capturing,
                CaptureState::Capturing | CaptureState::Finalizing | CaptureState::Completed
            )
            | (
                CaptureState::Finalizing,
                CaptureState::Finalizing | CaptureState::Completed
            )
            | (CaptureState::Completed, CaptureState::Completed)
            | (CaptureState::Aborted, CaptureState::Aborted)
            | (CaptureState::Faulted, CaptureState::Faulted)
    )
}

fn validate_mutation_status(
    previous: &DeckStatusSnapshot,
    current: &DeckStatusSnapshot,
    slot_counts: &[(u8, u64)],
) -> Result<(), GenericDeckRuntimeError> {
    if current.deck_session_id != previous.deck_session_id
        || current.deck_revision != previous.deck_revision
        || current.stream_generation != previous.stream_generation
        || current.stream_sequence != previous.stream_sequence
        || current.playheads != previous.playheads
        || current.capture_state != previous.capture_state
        || current.state != previous.state
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    validate_playhead_bounds(current, slot_counts)
}

fn validate_reset_status(
    previous: &DeckStatusSnapshot,
    current: &DeckStatusSnapshot,
    generation: u64,
    preserve_playheads: bool,
    slot_counts: &[(u8, u64)],
) -> Result<(), GenericDeckRuntimeError> {
    let expected_capture_state = reset_capture_state(previous.capture_state)?;
    let playheads_valid = if preserve_playheads {
        current.playheads == previous.playheads
    } else {
        current
            .playheads
            .as_slice()
            .iter()
            .all(|playhead| playhead.latent_slot == 0 && !playhead.end_of_stream)
    };
    if current.deck_session_id != previous.deck_session_id
        || current.deck_revision != previous.deck_revision
        || current.stream_generation != generation
        || current.stream_sequence != 0
        || !playheads_valid
        || current.roles != previous.roles
        || current.controls != previous.controls
        || current.source_transport != previous.source_transport
        || current.seed != previous.seed
        || current.capture_state != expected_capture_state
        || matches!(
            current.state,
            DeckState::Empty | DeckState::Loading | DeckState::Faulted
        )
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    validate_playhead_bounds(current, slot_counts)
}

const fn manual_reset_allowed(capture_state: CaptureState) -> bool {
    matches!(
        capture_state,
        CaptureState::Idle
            | CaptureState::Completed
            | CaptureState::Aborted
            | CaptureState::Faulted
    )
}

fn reset_capture_state(previous: CaptureState) -> Result<CaptureState, GenericDeckRuntimeError> {
    match previous {
        CaptureState::Idle => Ok(CaptureState::Idle),
        CaptureState::Capturing => Ok(CaptureState::Capturing),
        CaptureState::Finalizing => Ok(CaptureState::Finalizing),
        CaptureState::Completed | CaptureState::Aborted | CaptureState::Faulted => {
            Ok(CaptureState::Idle)
        }
        CaptureState::Starting => Err(GenericDeckRuntimeError::protocol()),
    }
}

fn validate_playhead_bounds(
    status: &DeckStatusSnapshot,
    slot_counts: &[(u8, u64)],
) -> Result<(), GenericDeckRuntimeError> {
    if status.playheads.len() != slot_counts.len()
        || status.source_transport.len() != slot_counts.len()
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    for (physical_slot, count) in slot_counts {
        if *count == 0 || *count > MAX_JSON_SAFE_INTEGER {
            return Err(GenericDeckRuntimeError::protocol());
        }
        let playhead = status
            .playheads
            .as_slice()
            .iter()
            .find(|value| value.physical_slot == *physical_slot)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let transport = status
            .source_transport
            .as_slice()
            .iter()
            .find(|value| value.physical_slot == *physical_slot)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let exact_end_sentinel = playhead.latent_slot == *count
            && playhead.end_of_stream
            && !transport.playing
            && !transport.loop_enabled;
        if playhead.latent_slot > *count
            || (playhead.latent_slot == *count && !exact_end_sentinel)
            || playhead.latent_slot > MAX_JSON_SAFE_INTEGER
            || playhead.loop_enabled != transport.loop_enabled
            || (playhead.end_of_stream
                && (transport.playing
                    || transport.loop_enabled
                    || playhead.latent_slot.saturating_add(1) < *count))
        {
            return Err(GenericDeckRuntimeError::protocol());
        }
    }
    Ok(())
}

fn looped_physical_slots(
    previous: &DeckStatusSnapshot,
    current: &DeckStatusSnapshot,
    slot_counts: &[(u8, u64)],
) -> Result<Vec<u8>, GenericDeckRuntimeError> {
    let mut looped = Vec::new();
    for transport in previous.source_transport.as_slice() {
        if !transport.playing || !transport.loop_enabled {
            continue;
        }
        let before = previous
            .playheads
            .as_slice()
            .iter()
            .find(|playhead| playhead.physical_slot == transport.physical_slot)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let after = current
            .playheads
            .as_slice()
            .iter()
            .find(|playhead| playhead.physical_slot == transport.physical_slot)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let count = slot_counts
            .iter()
            .find_map(|(slot, count)| (*slot == transport.physical_slot).then_some(*count))
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        if count == 0 || before.latent_slot >= count || after.latent_slot >= count {
            return Err(GenericDeckRuntimeError::protocol());
        }
        if after.latent_slot < before.latent_slot {
            looped.push(transport.physical_slot);
        }
    }
    Ok(looped)
}

fn causal_loop_reset_request(
    status: &DeckStatusSnapshot,
) -> Result<(u64, DeckReset), GenericDeckRuntimeError> {
    let generation = status
        .stream_generation
        .checked_add(1)
        .ok_or_else(GenericDeckRuntimeError::protocol)?;
    Ok((
        generation,
        DeckReset {
            deck_session_id: status.deck_session_id,
            deck_revision: status.deck_revision,
            new_stream_generation: generation,
            preserve_playheads: true,
        },
    ))
}

fn validate_batch(
    batch: &RgbaBatchV2,
    status: &DeckStatusSnapshot,
    slot_sequence: u64,
    dimensions: (u32, u32),
) -> Result<(), GenericDeckRuntimeError> {
    let metadata = batch.metadata();
    if metadata.session_id() != *status.deck_session_id.as_bytes()
        || metadata.generation() != status.stream_generation
        || metadata.logical_sequence() != status.stream_sequence
        || metadata.slot_sequence() != slot_sequence
        || metadata.batch() == 0
        || metadata.batch() > MAX_DECODE_BATCH
        || (batch.width(), batch.height()) != dimensions
    {
        return Err(GenericDeckRuntimeError::ring());
    }
    Ok(())
}

fn split_batch(batch: &RgbaBatchV2) -> Result<VecDeque<Vec<u8>>, GenericDeckRuntimeError> {
    let frame_bytes = usize::try_from(u64::from(batch.width()) * u64::from(batch.height()) * 4)
        .map_err(|_| GenericDeckRuntimeError::ring())?;
    let batch_count =
        usize::try_from(batch.metadata().batch()).map_err(|_| GenericDeckRuntimeError::ring())?;
    if frame_bytes == 0 || batch.pixels().len() != frame_bytes.saturating_mul(batch_count) {
        return Err(GenericDeckRuntimeError::ring());
    }
    Ok(batch
        .pixels()
        .chunks_exact(frame_bytes)
        .map(<[u8]>::to_vec)
        .collect())
}

fn pad_tight_rgba(
    bytes: &[u8],
    width: u32,
    height: u32,
    row_stride: u32,
) -> Result<Vec<u8>, GenericDeckRuntimeError> {
    let tight_stride = width
        .checked_mul(4)
        .ok_or_else(GenericDeckRuntimeError::ring)?;
    let expected = usize::try_from(u64::from(tight_stride) * u64::from(height))
        .map_err(|_| GenericDeckRuntimeError::ring())?;
    if bytes.len() != expected || row_stride < tight_stride {
        return Err(GenericDeckRuntimeError::ring());
    }
    let output_len = usize::try_from(u64::from(row_stride) * u64::from(height))
        .map_err(|_| GenericDeckRuntimeError::ring())?;
    let tight_stride =
        usize::try_from(tight_stride).map_err(|_| GenericDeckRuntimeError::ring())?;
    let row_stride = usize::try_from(row_stride).map_err(|_| GenericDeckRuntimeError::ring())?;
    let mut output = vec![0_u8; output_len];
    for row in 0..usize::try_from(height).map_err(|_| GenericDeckRuntimeError::ring())? {
        let source = row * tight_stride;
        let target = row * row_stride;
        output[target..target + tight_stride]
            .copy_from_slice(&bytes[source..source + tight_stride]);
    }
    Ok(output)
}

struct FrameClock {
    numerator: u64,
    denominator: u64,
    epoch: Instant,
    next_tick: u64,
}

impl FrameClock {
    fn new(numerator: u32, denominator: u32) -> Result<Self, GenericDeckRuntimeError> {
        let numerator = u64::from(numerator);
        let denominator = u64::from(denominator);
        if numerator == 0 || denominator == 0 || frame_offset_ns(numerator, denominator, 1)? == 0 {
            return Err(GenericDeckRuntimeError::input());
        }
        Ok(Self {
            numerator,
            denominator,
            epoch: Instant::now(),
            next_tick: 1,
        })
    }

    fn restart(&mut self) {
        self.epoch = Instant::now();
        self.next_tick = 1;
    }

    fn next_deadline(&self) -> Result<Instant, GenericDeckRuntimeError> {
        let offset = frame_offset_ns(self.numerator, self.denominator, self.next_tick)?;
        self.epoch
            .checked_add(Duration::from_nanos(offset))
            .ok_or_else(GenericDeckRuntimeError::protocol)
    }

    fn advance_past(&mut self, now: Instant) -> Result<(), GenericDeckRuntimeError> {
        let elapsed_ns = now
            .checked_duration_since(self.epoch)
            .unwrap_or_default()
            .as_nanos();
        let period_scale = u128::from(self.denominator)
            .checked_mul(1_000_000_000)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let scaled = elapsed_ns
            .checked_add(1)
            .and_then(|value| value.checked_mul(u128::from(self.numerator)))
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        let first_future_tick = scaled
            .checked_add(period_scale - 1)
            .ok_or_else(GenericDeckRuntimeError::protocol)?
            / period_scale;
        let first_future_tick = u64::try_from(first_future_tick)
            .map_err(|_| GenericDeckRuntimeError::protocol())?
            .max(1);
        let following_tick = self
            .next_tick
            .checked_add(1)
            .ok_or_else(GenericDeckRuntimeError::protocol)?;
        self.next_tick = following_tick.max(first_future_tick);
        Ok(())
    }
}

fn frame_offset_ns(
    numerator: u64,
    denominator: u64,
    tick: u64,
) -> Result<u64, GenericDeckRuntimeError> {
    if numerator == 0 || denominator == 0 || tick == 0 {
        return Err(GenericDeckRuntimeError::input());
    }
    let value = u128::from(tick)
        .checked_mul(u128::from(denominator))
        .and_then(|value| value.checked_mul(1_000_000_000))
        .ok_or_else(GenericDeckRuntimeError::protocol)?
        / u128::from(numerator);
    u64::try_from(value).map_err(|_| GenericDeckRuntimeError::protocol())
}

fn transport_active(status: &DeckStatusSnapshot) -> bool {
    status
        .source_transport
        .as_slice()
        .iter()
        .any(|source| source.playing)
}

fn validate_capture_start_availability(
    active_capture: bool,
    recording: &DecodedRecordingController,
) -> Result<(), GenericDeckRuntimeError> {
    if active_capture {
        return Err(GenericDeckRuntimeError::capture(
            "capture.already_active",
            "Only one latent capture may be active in this Deck session.",
        ));
    }
    if recording.is_active() {
        return Err(GenericDeckRuntimeError::capture(
            "capture.recording_conflict",
            "Stop decoded MP4 recording before starting latent capture.",
        ));
    }
    Ok(())
}

fn validate_capture_status(
    status: &CaptureStatusSnapshot,
    deck: &DeckStatusSnapshot,
    capture_id: Uuid,
    mode: CaptureMode,
    allowed_states: &[CaptureState],
) -> Result<(), GenericDeckRuntimeError> {
    let artifact_presence_valid = if status.state == CaptureState::Completed {
        status.artifact.is_some()
    } else {
        status.artifact.is_none()
    };
    if status.deck_session_id != deck.deck_session_id
        || status.deck_revision != deck.deck_revision
        || status.capture_id != capture_id
        || status.mode != mode
        || !allowed_states.contains(&status.state)
        || status.reset_events > 32
        || !artifact_presence_valid
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    Ok(())
}

fn valid_capture_status_transition(
    previous: CaptureState,
    current: CaptureState,
    mode: CaptureMode,
) -> bool {
    match (previous, current) {
        (CaptureState::Capturing, CaptureState::Capturing | CaptureState::Completed)
        | (
            CaptureState::Capturing | CaptureState::Finalizing,
            CaptureState::Aborted | CaptureState::Faulted,
        )
        | (CaptureState::Finalizing, CaptureState::Finalizing | CaptureState::Completed) => true,
        (CaptureState::Capturing, CaptureState::Finalizing) => mode == CaptureMode::LiveCapture,
        _ => false,
    }
}

fn capture_artifact_evidence(
    capture_id: Uuid,
    artifact: &CaptureArtifact,
) -> CaptureArtifactEvidence {
    CaptureArtifactEvidence {
        capture_id,
        staged_payload_path: artifact.staged_payload_path.clone(),
        payload_sha256: artifact.payload_sha256.clone(),
        payload_byte_length: artifact.payload_byte_length,
        latent_slots: artifact.latent_slots,
        decoded_frame_count: artifact.decoded_frame_count,
    }
}

fn validate_capture_artifact_bounds(
    artifact: &CaptureArtifact,
) -> Result<(), GenericDeckRuntimeError> {
    let maximum_decoded_frames = artifact
        .latent_slots
        .checked_mul(u64::from(MAX_DECODE_BATCH))
        .ok_or_else(GenericDeckRuntimeError::protocol)?;
    if artifact.latent_slots == 0
        || artifact.latent_slots > MAX_CAPTURE_LATENT_SLOTS
        || artifact.payload_byte_length == 0
        || artifact.payload_byte_length > MAX_CAPTURE_VISUAL_BYTES
        || artifact.decoded_frame_count < artifact.latent_slots
        || artifact.decoded_frame_count > maximum_decoded_frames
    {
        return Err(GenericDeckRuntimeError::protocol());
    }
    Ok(())
}

fn capture_view(
    status: &CaptureStatusSnapshot,
    state_override: Option<&str>,
    finalized: Option<(String, String)>,
    detail: Option<&str>,
) -> GenericCaptureView {
    let state = state_override.unwrap_or(match status.state {
        CaptureState::Idle => "idle",
        CaptureState::Starting => "starting",
        CaptureState::Capturing => "capturing",
        CaptureState::Finalizing | CaptureState::Completed => "finalizing",
        CaptureState::Aborted => "aborted",
        CaptureState::Faulted => "error",
    });
    let (cartridge_id, archive_sha256) = finalized.unzip();
    GenericCaptureView {
        capture_id: Some(status.capture_id.hyphenated().to_string()),
        mode: Some(status.mode),
        state: state.to_owned(),
        latent_slots: status.latent_slots.to_string(),
        reset_events: status.reset_events,
        cartridge_id,
        archive_sha256,
        detail: detail.map(str::to_owned),
    }
}

fn map_capture_client_error(error: WorkerClientV2Error) -> GenericDeckRuntimeError {
    match error {
        WorkerClientV2Error::Remote(remote) => {
            let code = match remote.code {
                ErrorCode::CaptureInvalidState => "capture.invalid_state",
                ErrorCode::CaptureNotReady => "capture.not_ready",
                ErrorCode::CaptureLimitExceeded => "capture.limit_exceeded",
                ErrorCode::SessionOutputLeasePinned => "session.output_lease_pinned",
                ErrorCode::SessionOutputLeaseBusy => "session.output_lease_busy",
                _ => "capture.worker_rejected",
            };
            GenericDeckRuntimeError::remote(
                code,
                "The codec worker rejected the latent capture command.",
                remote.fatal,
            )
        }
        other => map_client_error(other),
    }
}

fn map_capture_finalizer_error(error: CaptureFinalizerError) -> GenericDeckRuntimeError {
    // The only non-trust-boundary failure is the final Library import. Every
    // known staging/context/finalization rejection and every future unknown
    // code fails closed and terminates this actor.
    let fatal = error.is_worker_trust_boundary();
    GenericDeckRuntimeError::new(error.code, error.message, fatal)
}

const fn map_recording_error(error: DecodedRecordingError) -> GenericDeckRuntimeError {
    GenericDeckRuntimeError::capture(error.code(), error.message())
}

fn map_client_error(error: WorkerClientV2Error) -> GenericDeckRuntimeError {
    match error {
        WorkerClientV2Error::CommandTimeout(_) | WorkerClientV2Error::HeartbeatTimeout(_) => {
            GenericDeckRuntimeError::timeout()
        }
        WorkerClientV2Error::Remote(remote) => GenericDeckRuntimeError::remote(
            "deck.worker_rejected",
            "The Protocol 2 worker rejected the requested Deck command.",
            remote.fatal,
        ),
        WorkerClientV2Error::Supervisor(_)
        | WorkerClientV2Error::UnexpectedReply
        | WorkerClientV2Error::UnexpectedAck { .. } => GenericDeckRuntimeError::worker(),
    }
}

fn map_startup_client_error(error: WorkerClientV2Error) -> GenericDeckRuntimeError {
    match error {
        WorkerClientV2Error::Supervisor(error) => map_supervisor_error(&error),
        WorkerClientV2Error::UnexpectedReply | WorkerClientV2Error::UnexpectedAck { .. } => {
            GenericDeckRuntimeError::protocol()
        }
        other => map_client_error(other),
    }
}

fn map_supervisor_error(error: &WorkerSupervisorError) -> GenericDeckRuntimeError {
    match error {
        WorkerSupervisorError::ConnectTimeout
        | WorkerSupervisorError::HandshakeTimeout
        | WorkerSupervisorError::ReceiveTimeout
        | WorkerSupervisorError::ShutdownTimeout => GenericDeckRuntimeError::timeout(),
        WorkerSupervisorError::PeerProcessMismatch
        | WorkerSupervisorError::AuthenticationFailed
        | WorkerSupervisorError::UnexpectedHandshake
        | WorkerSupervisorError::Framing(_)
        | WorkerSupervisorError::Session(_)
        | WorkerSupervisorError::Protocol2Codec(_)
        | WorkerSupervisorError::Protocol2Session(_)
        | WorkerSupervisorError::ShutdownRejected => GenericDeckRuntimeError::protocol(),
        WorkerSupervisorError::ExtensionPackageKind
        | WorkerSupervisorError::ExtensionPackageDisabled
        | WorkerSupervisorError::ExtensionRuntimeUnavailable => GenericDeckRuntimeError::input(),
        WorkerSupervisorError::UnsupportedPlatform
        | WorkerSupervisorError::Random
        | WorkerSupervisorError::BootstrapEncode
        | WorkerSupervisorError::BootstrapTooLarge
        | WorkerSupervisorError::PipeSecurity(_)
        | WorkerSupervisorError::PipeCreate(_)
        | WorkerSupervisorError::Spawn(_)
        | WorkerSupervisorError::WorkerEnvironment(_)
        | WorkerSupervisorError::Job(_)
        | WorkerSupervisorError::BootstrapWrite(_)
        | WorkerSupervisorError::ProcessHandleUnavailable
        | WorkerSupervisorError::PipeIo(_)
        | WorkerSupervisorError::WorkerExited(_)
        | WorkerSupervisorError::Terminate(_) => GenericDeckRuntimeError::worker(),
    }
}

fn map_source_error(error: WorkerSourceV2Error) -> GenericDeckRuntimeError {
    match error {
        WorkerSourceV2Error::Client(error) => map_startup_client_error(error),
        WorkerSourceV2Error::Supervisor(error) => map_supervisor_error(&error),
        WorkerSourceV2Error::Receipt(_)
        | WorkerSourceV2Error::CartridgeIdentity
        | WorkerSourceV2Error::ReceiptEncoding
        | WorkerSourceV2Error::Duplicate(_)
        | WorkerSourceV2Error::UnsupportedPlatform => GenericDeckRuntimeError::input(),
    }
}

fn map_startup_error(error: DeckSessionV2Error) -> GenericDeckRuntimeError {
    match error {
        DeckSessionV2Error::ProtocolMismatch => GenericDeckRuntimeError::protocol(),
        DeckSessionV2Error::Client(error) => map_startup_client_error(error),
        DeckSessionV2Error::Supervisor(error) => map_supervisor_error(&error),
        DeckSessionV2Error::Source(error) => map_source_error(error),
        DeckSessionV2Error::Ring(_) => GenericDeckRuntimeError::ring(),
        DeckSessionV2Error::InvalidHostContract(_)
        | DeckSessionV2Error::IncompatiblePackage(_)
        | DeckSessionV2Error::InvalidSource
        | DeckSessionV2Error::CapabilityMismatch
        | DeckSessionV2Error::ProfileMismatch
        | DeckSessionV2Error::InvalidNativeTransfer
        | DeckSessionV2Error::ExternalAssetInvalid
        | DeckSessionV2Error::DeckRuntime(_) => GenericDeckRuntimeError::input(),
    }
}

fn finish<T>(
    reply: oneshot::Sender<Result<T, GenericDeckRuntimeError>>,
    result: Result<T, GenericDeckRuntimeError>,
) -> bool {
    let fatal = result.as_ref().is_err_and(|error| error.fatal);
    let disconnected = reply.send(result).is_err();
    fatal || disconnected
}

#[cfg(test)]
mod tests {
    use latentdeck_control::v2::{CommandName, PlayheadSnapshot, RoleBinding};
    use latentdeck_core::worker_supervisor::WorkerExit;

    use super::*;

    #[test]
    fn startup_protocol_mismatch_is_not_reported_as_a_worker_exit() {
        let error = map_startup_error(DeckSessionV2Error::ProtocolMismatch);

        assert_eq!(error.code, "deck.protocol_fault");
        assert!(error.fatal);
    }

    #[test]
    fn startup_worker_exit_and_timeouts_keep_distinct_stable_codes() {
        let worker_exit = map_startup_error(DeckSessionV2Error::Supervisor(
            WorkerSupervisorError::WorkerExited(WorkerExit {
                success: false,
                code: Some(7),
            }),
        ));
        let connect_timeout = map_startup_error(DeckSessionV2Error::Supervisor(
            WorkerSupervisorError::ConnectTimeout,
        ));
        let command_timeout = map_startup_error(DeckSessionV2Error::Client(
            WorkerClientV2Error::CommandTimeout(CommandName::DeckLoad),
        ));

        assert_eq!(worker_exit.code, "deck.worker_fault");
        assert!(worker_exit.fatal);
        assert_eq!(connect_timeout.code, "deck.worker_timeout");
        assert!(connect_timeout.fatal);
        assert_eq!(command_timeout.code, "deck.worker_timeout");
        assert!(command_timeout.fatal);
    }

    #[test]
    fn startup_package_state_errors_remain_sanitized_input_rejections() {
        for supervisor in [
            WorkerSupervisorError::ExtensionPackageKind,
            WorkerSupervisorError::ExtensionPackageDisabled,
            WorkerSupervisorError::ExtensionRuntimeUnavailable,
        ] {
            let error = map_startup_error(DeckSessionV2Error::Supervisor(supervisor));

            assert_eq!(error.code, "deck.input_invalid");
            assert!(!error.fatal);
        }
    }

    fn status(playhead: u64, generation: u64, sequence: u64) -> DeckStatusSnapshot {
        DeckStatusSnapshot {
            deck_session_id: Uuid::new_v4(),
            state: DeckState::Playing,
            deck_revision: 1,
            stream_generation: generation,
            stream_sequence: sequence,
            playheads: LimitedVec::try_from_vec(vec![PlayheadSnapshot {
                physical_slot: 1,
                latent_slot: playhead,
                loop_enabled: true,
                end_of_stream: false,
            }])
            .expect("playheads"),
            roles: LimitedVec::try_from_vec(vec![RoleBinding {
                role: "carrier".to_owned(),
                physical_slot: 1,
            }])
            .expect("roles"),
            controls: LimitedVec::try_from_vec(Vec::new()).expect("controls"),
            source_transport: LimitedVec::try_from_vec(vec![SourceTransportBinding {
                physical_slot: 1,
                playing: true,
                loop_enabled: true,
            }])
            .expect("transport"),
            seed: 7,
            capture_state: CaptureState::Idle,
        }
    }

    fn capture_status_fixture(
        deck: &DeckStatusSnapshot,
        capture_id: Uuid,
        reset_events: u32,
    ) -> CaptureStatusSnapshot {
        CaptureStatusSnapshot {
            deck_session_id: deck.deck_session_id,
            deck_revision: deck.deck_revision,
            capture_id,
            state: CaptureState::Capturing,
            mode: CaptureMode::LiveCapture,
            latent_slots: 1,
            reset_events,
            artifact: None,
        }
    }

    #[test]
    fn loop_detection_is_physical_slot_scoped_and_requires_exact_bounds() {
        let previous = status(4, 1, 9);
        let mut current = status(0, 1, 10);
        current.deck_session_id = previous.deck_session_id;

        assert_eq!(
            looped_physical_slots(&previous, &current, &[(1, 5)]).expect("loop"),
            vec![1]
        );
        assert_eq!(
            looped_physical_slots(&previous, &current, &[(1, 0)])
                .expect_err("zero bound")
                .code,
            "deck.protocol_fault"
        );
    }

    #[test]
    fn automatic_loop_transition_builds_a_causal_reset_and_preserves_capture_state() {
        let mut previous = status(4, 1, 9);
        previous.capture_state = CaptureState::Capturing;
        previous.state = DeckState::Capturing;
        let mut wrapped = status(0, 1, 10);
        wrapped.deck_session_id = previous.deck_session_id;
        wrapped.capture_state = CaptureState::Capturing;
        wrapped.state = DeckState::Capturing;

        validate_process_status(&previous, &wrapped, &[(1, 5)])
            .expect("process status may wrap one active loop");
        assert_eq!(
            looped_physical_slots(&previous, &wrapped, &[(1, 5)]).expect("detect loop"),
            vec![1]
        );
        let (generation, reset) =
            causal_loop_reset_request(&wrapped).expect("build causal loop reset");
        assert_eq!(generation, 2);
        assert_eq!(reset.deck_session_id, previous.deck_session_id);
        assert_eq!(reset.deck_revision, previous.deck_revision);
        assert_eq!(reset.new_stream_generation, generation);
        assert!(reset.preserve_playheads);

        let mut reset_status = wrapped.clone();
        reset_status.stream_generation = generation;
        reset_status.stream_sequence = 0;
        validate_reset_status(&wrapped, &reset_status, generation, true, &[(1, 5)])
            .expect("causal reset preserves wrapped playhead and active capture");

        let capture_id = Uuid::new_v4();
        let capture = capture_status_fixture(&reset_status, capture_id, 1);
        validate_capture_status(
            &capture,
            &reset_status,
            capture_id,
            CaptureMode::LiveCapture,
            &[CaptureState::Capturing],
        )
        .expect("capture receipt records the causal reset event");
    }

    #[test]
    fn rgba_padding_preserves_rows_without_resize_or_crop() {
        let padded = pad_tight_rgba(&[1, 2, 3, 4, 5, 6, 7, 8], 1, 2, 8).expect("padded rgba");
        assert_eq!(padded, [1, 2, 3, 4, 0, 0, 0, 0, 5, 6, 7, 8, 0, 0, 0, 0]);
    }

    #[test]
    fn frame_clock_keeps_absolute_rational_cadence_and_skips_late_ticks() {
        let mut clock = FrameClock::new(24, 1).expect("24 fps clock");
        let epoch = clock.epoch;

        assert_eq!(
            clock
                .next_deadline()
                .expect("first deadline")
                .duration_since(epoch)
                .as_nanos(),
            41_666_666
        );
        // Re-reading status or handling another non-causal command does not
        // mutate the clock phase.
        assert_eq!(
            clock
                .next_deadline()
                .expect("stable deadline")
                .duration_since(epoch)
                .as_nanos(),
            41_666_666
        );

        clock
            .advance_past(epoch + Duration::from_millis(100))
            .expect("late tick skip");
        assert_eq!(
            clock
                .next_deadline()
                .expect("deadline after a late presentation")
                .duration_since(epoch)
                .as_nanos(),
            125_000_000
        );
    }

    #[test]
    fn frame_clock_rejects_invalid_rates_without_float_rounding() {
        assert!(FrameClock::new(0, 1).is_err());
        assert!(FrameClock::new(24, 0).is_err());
        assert_eq!(frame_offset_ns(30_000, 1_001, 3).unwrap(), 100_100_000);
    }

    #[test]
    fn malformed_capture_identity_is_fatal_but_user_input_is_not() {
        let deck = status(0, 1, 0);
        let capture_id = Uuid::new_v4();
        let malformed = CaptureStatusSnapshot {
            deck_session_id: Uuid::new_v4(),
            deck_revision: deck.deck_revision,
            capture_id,
            state: CaptureState::Capturing,
            mode: CaptureMode::Snapshot,
            latent_slots: 0,
            reset_events: 0,
            artifact: None,
        };
        let error = validate_capture_status(
            &malformed,
            &deck,
            capture_id,
            CaptureMode::Snapshot,
            &[CaptureState::Capturing],
        )
        .expect_err("identity mismatch");
        assert!(error.fatal);
        assert!(!GenericDeckRuntimeError::input().fatal);
    }

    #[test]
    fn fatal_reply_is_delivered_then_closes_while_capture_not_ready_continues() {
        let (reply, mut receiver) = oneshot::channel::<Result<(), GenericDeckRuntimeError>>();
        assert!(finish(reply, Err(GenericDeckRuntimeError::protocol())));
        assert!(receiver.try_recv().expect("fatal reply").is_err());

        let not_ready =
            GenericDeckRuntimeError::remote("capture.not_ready", "Capture is not ready.", false);
        let (reply, mut receiver) = oneshot::channel::<Result<(), GenericDeckRuntimeError>>();
        assert!(!finish(reply, Err(not_ready)));
        assert!(receiver.try_recv().expect("nonfatal reply").is_err());
    }

    #[test]
    fn malicious_capture_finalization_errors_terminate_but_import_failure_does_not() {
        for code in [
            "capture.staging_root_invalid",
            "capture.staging_root_escape",
            "capture.staged_path_untrusted",
            "capture.staged_payload_mismatch",
            "capture.finalization_context_invalid",
            "capture.finalize_failed",
            "capture.unexpected_finalizer_code",
        ] {
            let error = map_capture_finalizer_error(CaptureFinalizerError {
                code,
                message: "Capture evidence was rejected.",
            });
            assert!(error.fatal, "{code} must terminate the actor");
            let (reply, _) = oneshot::channel::<Result<(), GenericDeckRuntimeError>>();
            assert!(finish(reply, Err(error)), "{code} must close after reply");
        }

        let import = map_capture_finalizer_error(CaptureFinalizerError {
            code: "capture.import_failed",
            message: "Library import failed.",
        });
        assert!(!import.fatal);
        let (reply, mut receiver) = oneshot::channel::<Result<(), GenericDeckRuntimeError>>();
        assert!(!finish(reply, Err(import)));
        assert!(receiver.try_recv().expect("import reply").is_err());
    }

    #[test]
    fn spout_publication_is_suppressed_without_the_foreground_output_lease() {
        assert!(effective_spout_enabled(true, true));
        assert!(!effective_spout_enabled(false, true));
        assert!(!effective_spout_enabled(true, false));
    }

    #[cfg(windows)]
    #[test]
    fn active_mp4_blocks_latent_capture_with_the_stable_reverse_conflict() {
        let root = tempfile::tempdir().expect("temporary recording directory");
        let recording = DecodedRecordingController::new();
        recording
            .arm(root.path().join("active.mp4"))
            .expect("arm decoded recording");
        assert!(recording.is_active());

        let conflict = validate_capture_start_availability(false, &recording)
            .expect_err("active MP4 must block latent capture");
        assert_eq!(conflict.code, "capture.recording_conflict");
        assert_eq!(
            conflict.message,
            "Stop decoded MP4 recording before starting latent capture."
        );
        assert!(!conflict.fatal);

        let ordered = validate_capture_start_availability(true, &recording)
            .expect_err("existing latent capture retains first precedence");
        assert_eq!(ordered.code, "capture.already_active");
        assert_eq!(
            ordered.message,
            "Only one latent capture may be active in this Deck session."
        );
        assert!(!ordered.fatal);

        recording.stop().expect("cancel test recording");
    }

    #[test]
    fn capture_reset_event_bound_accepts_thirty_two_and_fails_closed_on_the_thirty_third() {
        let deck = status(0, 1, 0);
        let capture_id = Uuid::new_v4();
        let mut receipt = capture_status_fixture(&deck, capture_id, 32);

        validate_capture_status(
            &receipt,
            &deck,
            capture_id,
            CaptureMode::LiveCapture,
            &[CaptureState::Capturing],
        )
        .expect("the negotiated thirty-two-event boundary remains valid");

        receipt.reset_events = 33;
        let error = validate_capture_status(
            &receipt,
            &deck,
            capture_id,
            CaptureMode::LiveCapture,
            &[CaptureState::Capturing],
        )
        .expect_err("the thirty-third reset event must fail closed");
        assert_eq!(error.code, "deck.protocol_fault");
        assert!(error.fatal);
    }

    #[test]
    fn oversized_capture_receipt_is_fatal_before_staged_path_access() {
        let artifact = CaptureArtifact {
            staged_payload_path: "Z:\\this-path-must-never-be-opened".to_owned(),
            payload_sha256: "0".repeat(64),
            payload_byte_length: MAX_CAPTURE_VISUAL_BYTES + 1,
            latent_slots: 1,
            decoded_frame_count: 1,
        };

        let error = validate_capture_artifact_bounds(&artifact).expect_err("oversized receipt");
        assert_eq!(error.code, "deck.protocol_fault");
        assert!(error.fatal);
    }

    #[test]
    fn paused_source_cannot_publish_an_out_of_range_playhead() {
        let mut malicious = status(0, 1, 0);
        malicious.state = DeckState::Paused;
        malicious.playheads = LimitedVec::try_from_vec(vec![PlayheadSnapshot {
            physical_slot: 1,
            latent_slot: u64::MAX,
            loop_enabled: false,
            end_of_stream: false,
        }])
        .expect("playheads");
        malicious.source_transport = LimitedVec::try_from_vec(vec![SourceTransportBinding {
            physical_slot: 1,
            playing: false,
            loop_enabled: false,
        }])
        .expect("transport");

        let error = validate_playhead_bounds(&malicious, &[(1, 3)])
            .expect_err("out-of-range worker playhead");
        assert_eq!(error.code, "deck.protocol_fault");
        assert!(error.fatal);
    }

    #[test]
    fn deck_process_accepts_owned_capture_progress_without_requiring_idle() {
        for state in [CaptureState::Capturing, CaptureState::Finalizing] {
            let mut previous = status(0, 1, 0);
            previous.state = DeckState::Capturing;
            previous.capture_state = state;
            let mut current = previous.clone();
            current.stream_sequence = 1;

            validate_process_status(&previous, &current, &[(1, 3)])
                .expect("active capture process status");
        }

        let mut previous = status(0, 1, 0);
        previous.state = DeckState::Capturing;
        previous.capture_state = CaptureState::Capturing;
        let mut completed = previous.clone();
        completed.stream_sequence = 1;
        completed.capture_state = CaptureState::Completed;
        validate_process_status(&previous, &completed, &[(1, 3)])
            .expect("snapshot may complete on process boundary");
    }

    #[test]
    fn reset_capture_state_is_closed_for_active_and_terminal_capture() {
        assert!(manual_reset_allowed(CaptureState::Idle));
        for state in [
            CaptureState::Completed,
            CaptureState::Aborted,
            CaptureState::Faulted,
        ] {
            assert!(manual_reset_allowed(state));
            let mut previous = status(0, 1, 0);
            previous.capture_state = state;
            let mut current = previous.clone();
            current.stream_generation = 2;
            current.capture_state = CaptureState::Idle;
            validate_reset_status(&previous, &current, 2, true, &[(1, 3)])
                .expect("terminal capture reset must clear to idle");
        }

        for state in [CaptureState::Capturing, CaptureState::Finalizing] {
            assert!(!manual_reset_allowed(state));
            let mut previous = status(0, 1, 0);
            previous.capture_state = state;
            let mut current = previous.clone();
            current.stream_generation = 2;
            validate_reset_status(&previous, &current, 2, true, &[(1, 3)])
                .expect("causal loop reset must keep active capture state");
        }
        assert!(!manual_reset_allowed(CaptureState::Starting));
    }
}
