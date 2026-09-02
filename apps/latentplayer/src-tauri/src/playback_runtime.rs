//! Single-owner runtime actors for causal H3 cartridge playback.
//!
//! The webview never owns the media clock, worker protocol, shared-memory
//! handles, decoder state, or native presentation resources. This module keeps
//! those responsibilities behind bounded typed commands.

#![allow(
    dead_code,
    reason = "Protocol 1 remains compiled as an explicit compatibility bridge while production uses Protocol 2"
)]

use std::{
    fmt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use latentdeck_core::{
    codec_pack::{ValidatedCodecPack, ValidatedExternalAsset},
    player::{CartridgeSummary, PlayerCoordinator, PlayerLaunchInputs, PlayerView},
    realtime_diagnostics::{
        DiagnosticCodecIdentity, DiagnosticGpuIdentity, PlayerDiagnosticSession, SanitizedToken,
        Sha256Token,
    },
};
use tauri::{AppHandle, WebviewWindow};

use crate::native_output::{PlayerViewport, ResizeOutcome};
use latentdeck_native_output::{NativeDeviceIdentity, NativeSpoutStatus};

/// Owned, trusted inputs copied from [`PlayerCoordinator::launch_inputs`].
///
/// Paths stay on the Rust side and are sent only to the authenticated worker.
#[derive(Clone)]
pub struct PlaybackLaunchConfig {
    codec_pack: ValidatedCodecPack,
    decoder_asset: ValidatedExternalAsset,
    cartridge_path: PathBuf,
    cartridge: CartridgeSummary,
}

impl PlaybackLaunchConfig {
    /// Clone the complete launch trust decision before an async worker start.
    #[must_use]
    pub fn from_launch_inputs(inputs: &PlayerLaunchInputs<'_>) -> Self {
        Self {
            codec_pack: inputs.codec_pack.clone(),
            decoder_asset: inputs.decoder_asset.clone(),
            cartridge_path: inputs.cartridge_path.to_path_buf(),
            cartridge: inputs.cartridge.clone(),
        }
    }

    /// Snapshot launch inputs while holding the coordinator lock only for the
    /// duration of this synchronous clone.
    ///
    /// # Errors
    ///
    /// Returns a stable state error when codec, decoder, or cartridge trust is
    /// incomplete.
    pub fn from_player(player: &PlayerCoordinator) -> Result<Self, PlaybackRuntimeError> {
        player
            .launch_inputs()
            .map(|inputs| Self::from_launch_inputs(&inputs))
            .map_err(|_| PlaybackRuntimeError::state_not_ready())
    }
}

/// Complete path-free active-session section consumed by the native bundle host.
pub(crate) struct PlaybackRuntimeDiagnostics {
    pub(crate) gpu: DiagnosticGpuIdentity,
    pub(crate) codec: DiagnosticCodecIdentity,
    pub(crate) session: PlayerDiagnosticSession,
}

#[derive(Clone)]
struct PlaybackDiagnosticIdentity {
    codec: DiagnosticCodecIdentity,
    cartridge_sha256: Sha256Token,
    target_fps: f64,
}

impl PlaybackDiagnosticIdentity {
    fn from_config(config: &PlaybackLaunchConfig) -> Result<Self, PlaybackRuntimeError> {
        let codec = DiagnosticCodecIdentity::new(
            diagnostic_token("minimax_h3")?,
            diagnostic_token("h3_av_latent")?,
            diagnostic_token(&config.codec_pack.manifest.pack_id)?,
            diagnostic_token(&config.codec_pack.manifest.pack_version)?,
            diagnostic_token(&config.decoder_asset.asset_id)?,
            Some(
                Sha256Token::parse(&config.decoder_asset.sha256)
                    .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?,
            ),
        );
        let cartridge_sha256 = Sha256Token::parse(&config.cartridge.archive_sha256)
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
        let numerator = u32::try_from(config.cartridge.frame_rate_numerator)
            .map(f64::from)
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
        let denominator = u32::try_from(config.cartridge.frame_rate_denominator)
            .map(f64::from)
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
        if numerator <= 0.0 || denominator <= 0.0 {
            return Err(PlaybackRuntimeError::diagnostics_contract());
        }
        Ok(Self {
            codec,
            cartridge_sha256,
            target_fps: numerator / denominator,
        })
    }
}

fn diagnostic_token(value: &str) -> Result<SanitizedToken, PlaybackRuntimeError> {
    SanitizedToken::parse(value).map_err(|_| PlaybackRuntimeError::diagnostics_contract())
}

fn diagnostic_gpu_identity(
    identity: &NativeDeviceIdentity,
) -> Result<DiagnosticGpuIdentity, PlaybackRuntimeError> {
    let adapter = SanitizedToken::from_hardware_label(&identity.adapter_name)
        .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
    let driver_label = format!(
        "{} {} {}",
        identity.driver, identity.driver_info, identity.backend
    );
    let driver = SanitizedToken::from_hardware_label(&driver_label)
        .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
    Ok(DiagnosticGpuIdentity::new(adapter, driver))
}

/// Path-free error returned by the desktop runtime facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaybackRuntimeError {
    pub code: &'static str,
    pub message: &'static str,
    pub recoverable: bool,
}

impl PlaybackRuntimeError {
    const fn new(code: &'static str, message: &'static str, recoverable: bool) -> Self {
        Self {
            code,
            message,
            recoverable,
        }
    }

    const fn state_not_ready() -> Self {
        Self::new(
            "player.runtime_not_ready",
            "Playback requires a validated cartridge, Codec Pack, and decoder asset.",
            true,
        )
    }

    #[cfg(not(target_os = "windows"))]
    const fn unsupported() -> Self {
        Self::new(
            "output.platform_unsupported",
            "LatentPlayer native playback requires Windows and DirectX 12.",
            false,
        )
    }

    const fn channel_closed() -> Self {
        Self::new(
            "player.runtime_unavailable",
            "The playback runtime is no longer available; restart playback.",
            true,
        )
    }

    const fn reply_timeout() -> Self {
        Self::new(
            "player.runtime_timeout",
            "The playback runtime did not answer within its bounded deadline.",
            true,
        )
    }

    const fn reset_in_progress() -> Self {
        Self::new(
            "player.reset_in_progress",
            "A causal decoder reset is already in progress.",
            true,
        )
    }

    const fn worker_start() -> Self {
        Self::new(
            "worker.start_failed",
            "The isolated H3 codec worker could not be started.",
            true,
        )
    }

    const fn worker_protocol() -> Self {
        Self::new(
            "worker.protocol_failed",
            "The isolated H3 codec worker violated the playback contract.",
            true,
        )
    }

    const fn worker_rejected() -> Self {
        Self::new(
            "worker.command_failed",
            "The isolated H3 codec worker rejected the playback request.",
            true,
        )
    }

    const fn worker_shutdown() -> Self {
        Self::new(
            "worker.shutdown_failed",
            "The isolated H3 codec worker could not be stopped safely.",
            false,
        )
    }

    const fn codec_inspection() -> Self {
        Self::new(
            "codec.runtime_incompatible",
            "The Codec Pack does not expose the required CUDA H3 adapter.",
            true,
        )
    }

    const fn input_contract() -> Self {
        Self::new(
            "player.input_contract_invalid",
            "Validated playback input could not be represented by Worker Protocol 1.",
            true,
        )
    }

    const fn ring() -> Self {
        Self::new(
            "ring.runtime_failed",
            "The bounded decoded-frame transport failed validation.",
            true,
        )
    }

    const fn schedule() -> Self {
        Self::new(
            "decode.schedule_invalid",
            "The codec decode cadence violated the validated cartridge timing.",
            true,
        )
    }

    const fn player_state() -> Self {
        Self::new(
            "player.state_unavailable",
            "Trusted Player state is unavailable; restart LatentPlayer.",
            false,
        )
    }

    const fn diagnostics_contract() -> Self {
        Self::new(
            "diagnostics.contract_invalid",
            "The active playback session could not be represented safely in a support bundle.",
            true,
        )
    }

    const fn output(code: &'static str) -> Self {
        Self::new(
            code,
            "Native DX12 output failed and playback was stopped.",
            true,
        )
    }
}

impl fmt::Display for PlaybackRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for PlaybackRuntimeError {}

#[cfg(target_os = "windows")]
mod windows {
    use std::{
        collections::VecDeque,
        sync::atomic::{AtomicBool, Ordering},
        time::SystemTime,
    };

    use latentdeck_control::{
        Ack, BoundedVec, CodecInspection, CodecLoad, Command, CyclePattern, DecodeCycleAck,
        EmptyPayload, ErrorCode, ExternalAssetBinding, MAX_CONTROL_FRAME_BYTES, MetricsSnapshot,
        ProfileRef, ResetReason, RingBind, SessionConfigure, ShutdownReason, SlotLoad, SlotLoaded,
        TimingDescriptor, WORKER_PROTOCOL_VERSION, WireUuid,
    };
    use latentdeck_core::{
        diagnostics::{LogLevel, record_global},
        playback_schedule::PlaybackSchedule,
        realtime_diagnostics::{
            MAX_STABLE_ERRORS, PresentationDiagnosticCounters, RealtimeSessionMetrics,
            StableErrorRecord, StableErrorSource, TimingDistribution, WorkerDiagnosticCounters,
        },
        worker_client::{WorkerClient, WorkerClientError},
        worker_supervisor::{ValidatedWorkerLaunch, spawn_worker},
    };
    use latentdeck_gpu::{
        ring::{ReadStatus, RingDescriptor, RingState},
        windows_ring::WindowsRgbRingConsumer,
        windows_ring::WindowsRgbRingOwner,
    };
    use serde::Deserialize;
    use tauri::async_runtime::JoinHandle;
    use tokio::{
        sync::{mpsc, oneshot, watch},
        time::{Instant, MissedTickBehavior, sleep_until, timeout},
    };

    use super::{
        AppHandle, Arc, CartridgeSummary, Duration, Mutex, NativeDeviceIdentity, NativeSpoutStatus,
        PlaybackDiagnosticIdentity, PlaybackLaunchConfig, PlaybackRuntimeDiagnostics,
        PlaybackRuntimeError, PlayerCoordinator, PlayerDiagnosticSession, PlayerView,
        PlayerViewport, ResizeOutcome, ValidatedCodecPack, WebviewWindow, diagnostic_gpu_identity,
        diagnostic_token,
    };
    use crate::native_output::{NativeOutput, native_output_config};

    const CHANNEL_CAPACITY: usize = 8;
    const ACTOR_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
    const SCHEDULER_POLL: Duration = Duration::from_millis(2);
    const DIAGNOSTIC_REPLY_TIMEOUT: Duration = Duration::from_secs(10);
    // One replacement may consume the bounded worker probe plus six bounded
    // configure/load/bind/prime round trips. User commands already admitted to
    // the WorkerActor must not time out while that replacement is in progress:
    // a timed-out Reset would remain queued and could apply later as stale UI
    // intent. Diagnostics share the same actor-ordering contract.
    const ROTATION_AWARE_REPLY_TIMEOUT: Duration = Duration::from_secs(900);
    const MAX_TIMING_SAMPLES: usize = 4_096;
    // Rotate before entering this authenticated-envelope reserve so the old
    // worker can still acknowledge an orderly shutdown.
    const SESSION_SHUTDOWN_MESSAGE_RESERVE: usize = 1_024;
    const INITIAL_GENERATION: u64 = 1;
    const SLOT_ID: &str = "player.a";
    const CODEC_FAMILY: &str = "minimax_h3";
    const PROFILE_ID: &str = "h3_av_latent";
    const PROFILE_VERSION: &str = "0.1.0";

    /// Bounded facade over the worker and presenter actors.
    pub struct PlaybackRuntime {
        worker_tx: mpsc::Sender<WorkerCommand>,
        presenter_tx: mpsc::Sender<PresenterCommand>,
        player: Arc<Mutex<PlayerCoordinator>>,
        playing: Arc<AtomicBool>,
        loop_enabled: Arc<AtomicBool>,
        at_end: Arc<AtomicBool>,
        reset_in_flight: Arc<AtomicBool>,
        session_rotating: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
        started_at: Instant,
        diagnostic_identity: PlaybackDiagnosticIdentity,
        worker_cleanup_complete: watch::Receiver<bool>,
        presenter_cleanup_complete: watch::Receiver<bool>,
        worker_task: JoinHandle<()>,
        presenter_task: JoinHandle<()>,
    }

    impl PlaybackRuntime {
        /// Start, authenticate, configure, bind, and warm one isolated playback
        /// session. The first codec-valid cycle is decoded before this returns.
        ///
        /// # Errors
        ///
        /// Returns a stable, path-free error after terminating any worker that
        /// did start successfully.
        #[allow(clippy::too_many_lines)] // Closed startup ownership and cleanup sequence.
        pub async fn start_protocol1_h3(
            app: AppHandle,
            parent: WebviewWindow,
            player: Arc<Mutex<PlayerCoordinator>>,
            config: PlaybackLaunchConfig,
            viewport: PlayerViewport,
        ) -> Result<Self, PlaybackRuntimeError> {
            let diagnostic_identity = PlaybackDiagnosticIdentity::from_config(&config)?;
            let launch = ValidatedWorkerLaunch::from_codec_pack(&config.codec_pack);
            let pending = spawn_worker(launch).await.map_err(|_| {
                let error = PlaybackRuntimeError::worker_start();
                record_runtime_error(&player, error);
                error
            })?;
            let session = pending.connect().await.map_err(|_| {
                let error = PlaybackRuntimeError::worker_start();
                record_runtime_error(&player, error);
                error
            })?;
            let mut client = WorkerClient::new(session);

            let initialized =
                initialize_session(&app, &parent, viewport, &config, &mut client).await;
            let InitializedSession {
                schedule,
                slot,
                owner,
                consumer,
                output,
            } = match initialized {
                Ok(initialized) => initialized,
                Err(error) => {
                    let _ = stop_worker(&mut client, ShutdownReason::Recovery).await;
                    record_runtime_error(&player, error);
                    return Err(error);
                }
            };

            if let Err(error) = update_output_available(&player, true) {
                let _ = stop_worker(&mut client, ShutdownReason::Recovery).await;
                record_runtime_error(&player, error);
                return Err(error);
            }

            let view = player_view(&player)?;
            let playing = Arc::new(AtomicBool::new(false));
            let loop_enabled = Arc::new(AtomicBool::new(view.loop_enabled));
            let at_end = Arc::new(AtomicBool::new(false));
            let reset_in_flight = Arc::new(AtomicBool::new(false));
            let session_rotating = Arc::new(AtomicBool::new(false));
            let closed = Arc::new(AtomicBool::new(false));
            let (worker_tx, worker_rx) = mpsc::channel(CHANNEL_CAPACITY);
            let (presenter_tx, presenter_rx) = mpsc::channel(CHANNEL_CAPACITY);
            let spout_diagnostics = SpoutDiagnosticHistory::from_status(&output.spout_status());
            // This retained, already-validated launch config is the sole
            // authority for every replacement session. It is never rebuilt
            // from UI state or a worker acknowledgement.
            let session_config = Box::new(config.clone());
            let session_identity = Box::new(PlaybackSessionIdentity::from_config(&config));

            let presenter = PresenterActor {
                output,
                consumer,
                player: Arc::clone(&player),
                worker_tx: worker_tx.clone(),
                playing: Arc::clone(&playing),
                loop_enabled: Arc::clone(&loop_enabled),
                at_end: Arc::clone(&at_end),
                reset_in_flight: Arc::clone(&reset_in_flight),
                closed: Arc::clone(&closed),
                generation: INITIAL_GENERATION,
                frame_count: config.cartridge.frame_count,
                consumed_frames: 0,
                pending_frame: None,
                clock: FrameClock::new(
                    config.cartridge.frame_rate_numerator,
                    config.cartridge.frame_rate_denominator,
                )?,
                quiesced: true,
                diagnostic_frames_presented: 0,
                last_presented_at: None,
                frame_intervals: TimingSamples::new(MAX_TIMING_SAMPLES),
                spout_diagnostics,
                viewport_revision: viewport.revision(),
            };
            let worker = WorkerActor {
                client,
                schedule,
                slot,
                owner,
                presenter_tx: presenter_tx.clone(),
                player: Arc::clone(&player),
                playing: Arc::clone(&playing),
                at_end: Arc::clone(&at_end),
                reset_in_flight: Arc::clone(&reset_in_flight),
                session_rotating: Arc::clone(&session_rotating),
                closed: Arc::clone(&closed),
                session_config,
                session_identity,
            };

            let (presenter_cleanup_sender, presenter_cleanup_complete) = watch::channel(false);
            let presenter_task = tauri::async_runtime::spawn(async move {
                presenter.run(presenter_rx).await;
                presenter_cleanup_sender.send_replace(true);
            });
            let (worker_cleanup_sender, worker_cleanup_complete) = watch::channel(false);
            let worker_task = tauri::async_runtime::spawn(async move {
                worker.run(worker_rx).await;
                worker_cleanup_sender.send_replace(true);
            });

            Ok(Self {
                worker_tx,
                presenter_tx,
                player,
                playing,
                loop_enabled,
                at_end,
                reset_in_flight,
                session_rotating,
                closed,
                started_at: Instant::now(),
                diagnostic_identity,
                worker_cleanup_complete,
                presenter_cleanup_complete,
                worker_task,
                presenter_task,
            })
        }

        /// Start or resume native presentation. Playing again after EOS performs
        /// the same explicit causal reset as Restart.
        pub async fn play(&self) -> Result<PlayerView, PlaybackRuntimeError> {
            self.ensure_open()?;
            if self.at_end.load(Ordering::Acquire) {
                return self
                    .request_reset(ResetReason::Restart, true, ROTATION_AWARE_REPLY_TIMEOUT)
                    .await;
            }

            let view = with_player(&self.player, |player| player.set_playing(true))?;
            self.playing.store(true, Ordering::Release);
            // A rotating WorkerActor owns the presenter quiesce/ring-swap
            // barrier. Preserve the new play intent and let that actor issue
            // the single Resume after the fresh ring is installed.
            if self.session_rotating.load(Ordering::Acquire) {
                return Ok(view);
            }
            if let Err(error) = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Resume,
                ACTOR_REPLY_TIMEOUT,
            )
            .await
            {
                self.playing.store(false, Ordering::Release);
                record_runtime_error(&self.player, error);
                return Err(error);
            }
            Ok(view)
        }

        /// Stop presentation immediately and wait for a presenter barrier. The
        /// worker may retain already-decoded frames and causal state.
        pub async fn pause(&self) -> Result<PlayerView, PlaybackRuntimeError> {
            self.ensure_open()?;
            self.playing.store(false, Ordering::Release);
            let view = if self.session_rotating.load(Ordering::Acquire)
                && player_view(&self.player)?.phase != latentdeck_core::player::PlayerPhase::Playing
            {
                player_view(&self.player)?
            } else {
                with_player(&self.player, |player| player.set_playing(false))?
            };
            if self.session_rotating.load(Ordering::Acquire) {
                return Ok(view);
            }
            request_presenter(
                &self.presenter_tx,
                PresenterRequest::Quiesce,
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            Ok(view)
        }

        /// Reset worker causal state and both ring endpoints to a strictly newer
        /// generation. Restart deliberately returns paused at frame zero.
        pub async fn restart(&self) -> Result<PlayerView, PlaybackRuntimeError> {
            self.ensure_open()?;
            self.playing.store(false, Ordering::Release);
            request_presenter(
                &self.presenter_tx,
                PresenterRequest::Quiesce,
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            self.request_reset(ResetReason::Restart, false, ROTATION_AWARE_REPLY_TIMEOUT)
                .await
        }

        /// Change only the loop transport policy; it never mutates decoder
        /// state until the presented stream actually reaches EOS.
        pub fn set_loop(&self, enabled: bool) -> Result<PlayerView, PlaybackRuntimeError> {
            self.ensure_open()?;
            let view = with_player(&self.player, |player| player.set_loop_enabled(enabled))?;
            self.loop_enabled.store(enabled, Ordering::Release);
            Ok(view)
        }

        /// Apply one monotonic embedded-child viewport update without changing
        /// decoded or Spout source dimensions.
        pub async fn set_viewport(
            &self,
            viewport: PlayerViewport,
        ) -> Result<ResizeOutcome, PlaybackRuntimeError> {
            self.ensure_open()?;
            match request_presenter(
                &self.presenter_tx,
                PresenterRequest::SetViewport(viewport),
                ACTOR_REPLY_TIMEOUT,
            )
            .await?
            {
                PresenterReply::Resize(value) => Ok(value),
                _ => Err(PlaybackRuntimeError::channel_closed()),
            }
        }

        /// Return the current native Spout sender state through the presenter
        /// actor without exposing GPU or SDK handles.
        pub async fn spout_status(&self) -> Result<NativeSpoutStatus, PlaybackRuntimeError> {
            self.ensure_open()?;
            match request_presenter(
                &self.presenter_tx,
                PresenterRequest::SpoutStatus,
                ACTOR_REPLY_TIMEOUT,
            )
            .await?
            {
                PresenterReply::Spout(status) => Ok(status),
                _ => Err(PlaybackRuntimeError::channel_closed()),
            }
        }

        /// Apply an optional sender-name and enable-state update. Invalid or
        /// unavailable Spout controls are reflected in the returned status and
        /// never stop native-window playback.
        pub async fn configure_spout(
            &self,
            name: Option<String>,
            enabled: Option<bool>,
        ) -> Result<NativeSpoutStatus, PlaybackRuntimeError> {
            self.ensure_open()?;
            match request_presenter(
                &self.presenter_tx,
                PresenterRequest::ConfigureSpout { name, enabled },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?
            {
                PresenterReply::Spout(status) => Ok(status),
                _ => Err(PlaybackRuntimeError::channel_closed()),
            }
        }

        /// Capture one truthful, path-free active-session diagnostic snapshot.
        ///
        /// `None` means the actor session is already closed and callers must
        /// emit the explicit lifecycle-only `NoActiveSession` form instead.
        pub async fn diagnostics(
            &self,
        ) -> Result<Option<PlaybackRuntimeDiagnostics>, PlaybackRuntimeError> {
            if self.closed.load(Ordering::Acquire) {
                return Ok(None);
            }
            let worker = request_worker_diagnostics(&self.worker_tx).await?;
            let PresenterReply::Diagnostics(presenter) = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Diagnostics,
                DIAGNOSTIC_REPLY_TIMEOUT,
            )
            .await?
            else {
                return Err(PlaybackRuntimeError::channel_closed());
            };
            let duration_ms = u64::try_from(self.started_at.elapsed().as_millis())
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let worker = WorkerDiagnosticCounters::from_metrics_snapshot(&worker)
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let presentation = PresentationDiagnosticCounters::new(
                presenter.frames_presented,
                None,
                Some(presenter.spout_frames_sent),
            )
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let metrics = RealtimeSessionMetrics::new(
                duration_ms,
                self.diagnostic_identity.target_fps,
                presenter.measured_fps,
                presenter.frame_intervals,
                presenter.control_latency,
                worker,
                presentation,
                presenter.stable_errors,
            )
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            Ok(Some(PlaybackRuntimeDiagnostics {
                gpu: diagnostic_gpu_identity(&presenter.device)?,
                codec: self.diagnostic_identity.codec.clone(),
                session: PlayerDiagnosticSession::new(
                    self.diagnostic_identity.cartridge_sha256.clone(),
                    metrics,
                ),
            }))
        }

        /// Quiesce presentation and establish an ownership barrier over both
        /// runtime actors. Any bounded send/reply failure aborts the affected
        /// actor, which drops its owned Job Object or native output instead of
        /// leaving the cross-linked channels detached from the application.
        pub async fn shutdown(&self) -> Result<(), PlaybackRuntimeError> {
            let was_closed = self.closed.swap(true, Ordering::AcqRel);
            self.playing.store(false, Ordering::Release);
            let _ = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Quiesce,
                ACTOR_REPLY_TIMEOUT,
            )
            .await;
            let worker_result = if was_closed {
                Ok(())
            } else {
                request_worker_shutdown(&self.worker_tx).await
            };
            if worker_result.is_err() {
                self.worker_task.abort();
            }

            // WorkerActor normally stops PresenterActor itself. This direct
            // request is the independent fallback for a blocked worker command
            // queue or worker IPC call.
            let presenter_result = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Stop,
                ACTOR_REPLY_TIMEOUT,
            )
            .await;
            if presenter_result.is_err() {
                self.presenter_task.abort();
            }

            let presenter_cleanup_result = ensure_task_cleanup(
                &self.presenter_task,
                self.presenter_cleanup_complete.clone(),
            )
            .await;
            // Always establish both ownership barriers. A stuck presenter
            // must not prevent the worker Job Object from being aborted (and
            // a stuck worker must not leave NativeOutput owned by presenter).
            let worker_cleanup_result =
                ensure_task_cleanup(&self.worker_task, self.worker_cleanup_complete.clone()).await;
            let pause_result = pause_if_playing(&self.player);
            let output_result = update_output_available(&self.player, false);
            presenter_cleanup_result?;
            worker_cleanup_result?;
            pause_result?;
            output_result?;
            worker_result
        }

        async fn request_reset(
            &self,
            reason: ResetReason,
            resume: bool,
            deadline: Duration,
        ) -> Result<PlayerView, PlaybackRuntimeError> {
            if self
                .reset_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(PlaybackRuntimeError::reset_in_progress());
            }
            let (reply_tx, reply_rx) = oneshot::channel();
            if let Err(error) = send_bounded(
                &self.worker_tx,
                WorkerCommand::Reset {
                    reason,
                    resume,
                    reply: Some(reply_tx),
                },
                ACTOR_REPLY_TIMEOUT,
            )
            .await
            {
                self.reset_in_flight.store(false, Ordering::Release);
                return Err(error);
            }
            match receive_bounded(reply_rx, deadline).await {
                Ok(result) => result,
                Err(error) => {
                    self.reset_in_flight.store(false, Ordering::Release);
                    Err(error)
                }
            }
        }

        fn ensure_open(&self) -> Result<(), PlaybackRuntimeError> {
            if self.closed.load(Ordering::Acquire) {
                Err(PlaybackRuntimeError::channel_closed())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for PlaybackRuntime {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Release);
            self.playing.store(false, Ordering::Release);
            // Dropping the facade is a terminal ownership transfer. Aborting
            // both tasks breaks the Worker/Presenter sender cycle; task-future
            // destruction then drops the worker Job Object and NativeOutput.
            self.worker_task.abort();
            self.presenter_task.abort();
        }
    }

    struct InitializedSession {
        schedule: PlaybackSchedule,
        slot: SlotLoaded,
        owner: WindowsRgbRingOwner,
        consumer: WindowsRgbRingConsumer,
        output: NativeOutput,
    }

    struct LoadedWorkerSession {
        schedule: PlaybackSchedule,
        slot: SlotLoaded,
        owner: WindowsRgbRingOwner,
        consumer: WindowsRgbRingConsumer,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct PlaybackSourceIdentity {
        cartridge_path: std::path::PathBuf,
        cartridge: CartridgeSummary,
        decoder_path: std::path::PathBuf,
        decoder_asset_id: String,
        decoder_variant_id: String,
        decoder_sha256: String,
        decoder_byte_length: u64,
    }

    impl PlaybackSourceIdentity {
        fn from_config(config: &PlaybackLaunchConfig) -> Self {
            Self {
                cartridge_path: config.cartridge_path.clone(),
                cartridge: config.cartridge.clone(),
                decoder_path: config.decoder_asset.path.clone(),
                decoder_asset_id: config.decoder_asset.asset_id.clone(),
                decoder_variant_id: config.decoder_asset.variant_id.clone(),
                decoder_sha256: config.decoder_asset.sha256.clone(),
                decoder_byte_length: config.decoder_asset.byte_length,
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct PlaybackSessionIdentity {
        codec_pack_root: std::path::PathBuf,
        worker_executable: std::path::PathBuf,
        worker_working_directory: std::path::PathBuf,
        codec_pack_manifest: latentdeck_core::codec_pack::CodecPackManifest,
        source: PlaybackSourceIdentity,
    }

    impl PlaybackSessionIdentity {
        fn from_config(config: &PlaybackLaunchConfig) -> Self {
            Self {
                codec_pack_root: config.codec_pack.root.clone(),
                worker_executable: config.codec_pack.worker_executable.clone(),
                worker_working_directory: config.codec_pack.worker_working_directory.clone(),
                codec_pack_manifest: config.codec_pack.manifest.clone(),
                source: PlaybackSourceIdentity::from_config(config),
            }
        }
    }

    async fn initialize_session(
        app: &AppHandle,
        parent: &WebviewWindow,
        viewport: PlayerViewport,
        config: &PlaybackLaunchConfig,
        client: &mut WorkerClient,
    ) -> Result<InitializedSession, PlaybackRuntimeError> {
        let LoadedWorkerSession {
            schedule,
            slot,
            owner,
            consumer,
        } = initialize_worker_session(config, client).await?;

        let bounds = viewport
            .bounds()
            .ok_or_else(|| PlaybackRuntimeError::output("output.viewport_not_ready"))?;
        let output = NativeOutput::new_embedded(
            app,
            parent,
            native_output_config(slot.width, slot.height),
            bounds,
        )
        .await
        .map_err(|error| PlaybackRuntimeError::output(error.code()))?;
        if output.frame_dimensions() != (slot.width, slot.height)
            || output.present_mode() != wgpu::PresentMode::Fifo
        {
            return Err(PlaybackRuntimeError::output("output.contract_invalid"));
        }
        if viewport.visible() {
            output
                .show()
                .map_err(|error| PlaybackRuntimeError::output(error.code()))?;
        }
        Ok(InitializedSession {
            schedule,
            slot,
            owner,
            consumer,
            output,
        })
    }

    async fn initialize_worker_session(
        config: &PlaybackLaunchConfig,
        client: &mut WorkerClient,
    ) -> Result<LoadedWorkerSession, PlaybackRuntimeError> {
        configure_session(client).await?;
        let profile = h3_profile();
        let inspection = inspect_codec(client).await?;
        validate_inspection(&inspection, &config.codec_pack, &profile)?;
        load_codec(client, config, &profile).await?;
        let slot = load_slot(client, config).await?;
        validate_slot(&slot, &config.cartridge, &profile)?;
        let mut schedule = PlaybackSchedule::new(slot.clone(), INITIAL_GENERATION)
            .map_err(|_| PlaybackRuntimeError::schedule())?;
        let descriptor = RingDescriptor::new(slot.width, slot.height, INITIAL_GENERATION)
            .map_err(|_| PlaybackRuntimeError::ring())?;
        let owner =
            WindowsRgbRingOwner::create(descriptor).map_err(|_| PlaybackRuntimeError::ring())?;
        let consumer = owner
            .open_consumer()
            .map_err(|_| PlaybackRuntimeError::ring())?;
        let mut owner = bind_ring(client, owner).await?;
        let decoded = decode_next(client, &mut schedule, &slot, &mut owner).await?;
        if !decoded {
            return Err(PlaybackRuntimeError::ring());
        }
        Ok(LoadedWorkerSession {
            schedule,
            slot,
            owner,
            consumer,
        })
    }

    async fn spawn_loaded_worker_session(
        config: &PlaybackLaunchConfig,
    ) -> Result<(WorkerClient, LoadedWorkerSession), PlaybackRuntimeError> {
        let launch = ValidatedWorkerLaunch::from_codec_pack(&config.codec_pack);
        let pending = spawn_worker(launch)
            .await
            .map_err(|_| PlaybackRuntimeError::worker_start())?;
        let session = pending
            .connect()
            .await
            .map_err(|_| PlaybackRuntimeError::worker_start())?;
        let mut client = WorkerClient::new(session);
        match initialize_worker_session(config, &mut client).await {
            Ok(loaded) => Ok((client, loaded)),
            Err(error) => {
                let _ = stop_worker(&mut client, ShutdownReason::Recovery).await;
                Err(error)
            }
        }
    }

    async fn configure_session(client: &mut WorkerClient) -> Result<(), PlaybackRuntimeError> {
        let request = SessionConfigure {
            selected_protocol_version: WORKER_PROTOCOL_VERSION,
            app_version: latentdeck_core::product_version().to_owned(),
            heartbeat_interval_ms: 1_000,
            heartbeat_hard_timeout_ms: 5_000,
            max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
            max_inflight_decode_batches: 1,
        };
        let ack = client
            .call(Command::SessionConfigure(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::SessionConfigure(configured) = ack else {
            return Err(PlaybackRuntimeError::worker_protocol());
        };
        if configured.selected_protocol_version != request.selected_protocol_version
            || configured.heartbeat_interval_ms != request.heartbeat_interval_ms
            || configured.heartbeat_hard_timeout_ms != request.heartbeat_hard_timeout_ms
            || configured.max_frame_bytes != request.max_frame_bytes
            || configured.max_inflight_decode_batches != request.max_inflight_decode_batches
        {
            return Err(PlaybackRuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn inspect_codec(
        client: &mut WorkerClient,
    ) -> Result<CodecInspection, PlaybackRuntimeError> {
        match client
            .call(Command::CodecInspect(EmptyPayload {}), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?
        {
            Ack::CodecInspect(inspection) => Ok(inspection),
            _ => Err(PlaybackRuntimeError::worker_protocol()),
        }
    }

    fn validate_inspection(
        inspection: &CodecInspection,
        pack: &ValidatedCodecPack,
        profile: &ProfileRef,
    ) -> Result<(), PlaybackRuntimeError> {
        if !inspection.cuda_available
            || !inspection.devices.iter().any(|device| device.ordinal == 0)
        {
            return Err(PlaybackRuntimeError::codec_inspection());
        }
        let adapter = inspection
            .adapters
            .iter()
            .find(|adapter| adapter.adapter_id == pack.manifest.adapter.adapter_id)
            .ok_or_else(PlaybackRuntimeError::codec_inspection)?;
        if adapter.adapter_version != pack.manifest.adapter.adapter_version
            || !adapter
                .profiles
                .iter()
                .any(|candidate| candidate == profile)
        {
            return Err(PlaybackRuntimeError::codec_inspection());
        }
        let declared = pack
            .manifest
            .compatibility
            .profiles
            .iter()
            .any(|candidate| {
                candidate.codec_family == profile.codec_family
                    && candidate.profile == profile.profile
                    && candidate
                        .profile_versions
                        .iter()
                        .any(|version| version == &profile.profile_version)
            });
        if !declared {
            return Err(PlaybackRuntimeError::codec_inspection());
        }
        Ok(())
    }

    async fn load_codec(
        client: &mut WorkerClient,
        config: &PlaybackLaunchConfig,
        profile: &ProfileRef,
    ) -> Result<(), PlaybackRuntimeError> {
        let asset_path = path_for_protocol(&config.decoder_asset.path)?;
        let asset = ExternalAssetBinding {
            asset_id: config.decoder_asset.asset_id.clone(),
            path: asset_path,
            sha256: config.decoder_asset.sha256.clone(),
            byte_length: config.decoder_asset.byte_length,
        };
        let request = CodecLoad {
            pack_id: config.codec_pack.manifest.pack_id.clone(),
            pack_version: config.codec_pack.manifest.pack_version.clone(),
            adapter_id: config.codec_pack.manifest.adapter.adapter_id.clone(),
            profile: profile.clone(),
            device_ordinal: 0,
            assets: BoundedVec::try_from_vec(vec![asset])
                .map_err(|_| PlaybackRuntimeError::input_contract())?,
        };
        let ack = client
            .call(Command::CodecLoad(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::CodecLoad(loaded) = ack else {
            return Err(PlaybackRuntimeError::worker_protocol());
        };
        if loaded.pack_id != request.pack_id
            || loaded.pack_version != request.pack_version
            || loaded.adapter_id != request.adapter_id
            || loaded.adapter_version != config.codec_pack.manifest.adapter.adapter_version
            || loaded.profile != request.profile
            || loaded.device.ordinal != request.device_ordinal
        {
            return Err(PlaybackRuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn load_slot(
        client: &mut WorkerClient,
        config: &PlaybackLaunchConfig,
    ) -> Result<SlotLoaded, PlaybackRuntimeError> {
        let cartridge_id = parse_wire_uuid(&config.cartridge.cartridge_id)?;
        let request = SlotLoad {
            slot_id: SLOT_ID.to_owned(),
            cartridge_path: path_for_protocol(&config.cartridge_path)?,
            cartridge_id,
            expected_archive_sha256: config.cartridge.archive_sha256.clone(),
            stream_generation: INITIAL_GENERATION,
        };
        match client
            .call(Command::SlotLoad(request), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?
        {
            Ack::SlotLoad(slot) => Ok(slot),
            _ => Err(PlaybackRuntimeError::worker_protocol()),
        }
    }

    fn validate_slot(
        slot: &SlotLoaded,
        cartridge: &CartridgeSummary,
        profile: &ProfileRef,
    ) -> Result<(), PlaybackRuntimeError> {
        let frame_rate_numerator = u32::try_from(cartridge.frame_rate_numerator)
            .map_err(|_| PlaybackRuntimeError::input_contract())?;
        let frame_rate_denominator = u32::try_from(cartridge.frame_rate_denominator)
            .map_err(|_| PlaybackRuntimeError::input_contract())?;
        if slot.slot_id != SLOT_ID
            || slot.width != cartridge.width
            || slot.height != cartridge.height
            || slot.profile != *profile
            || slot.timing.decoded_frame_count != cartridge.frame_count
            || slot.timing.frame_rate_numerator != frame_rate_numerator
            || slot.timing.frame_rate_denominator != frame_rate_denominator
            || !slot.timing.reset_required_on_wrap
            || slot.timing.arbitrary_seek
            || slot.timing.max_frames_per_cycle > latentdeck_gpu::ring::RING_SLOT_COUNT
        {
            return Err(PlaybackRuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn bind_ring(
        client: &mut WorkerClient,
        owner: WindowsRgbRingOwner,
    ) -> Result<WindowsRgbRingOwner, PlaybackRuntimeError> {
        ensure_zero_ring(owner.state().map_err(|_| PlaybackRuntimeError::ring())?)?;
        let binding = client
            .with_process_handle(|process| owner.duplicate_into(process))
            .map_err(|_| PlaybackRuntimeError::ring())?
            .map_err(|_| PlaybackRuntimeError::ring())?;
        let ring_id = WireUuid::new_v4();
        let request = RingBind {
            layout_version: 1,
            mapping_handle: binding.mapping_handle(),
            mapping_bytes: binding.mapping_bytes(),
            frames_ready_event_handle: binding.frames_ready_event_handle(),
            ring_id,
        };
        let ack = client
            .call(Command::RingBind(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::RingBind(bound) = ack else {
            return Err(PlaybackRuntimeError::worker_protocol());
        };
        if bound.layout_version != request.layout_version
            || bound.mapping_bytes != request.mapping_bytes
            || bound.ring_id != request.ring_id
        {
            return Err(PlaybackRuntimeError::worker_protocol());
        }
        ensure_zero_ring(owner.state().map_err(|_| PlaybackRuntimeError::ring())?)?;
        Ok(owner)
    }

    struct WorkerActor {
        client: WorkerClient,
        schedule: PlaybackSchedule,
        slot: SlotLoaded,
        owner: WindowsRgbRingOwner,
        presenter_tx: mpsc::Sender<PresenterCommand>,
        player: Arc<Mutex<PlayerCoordinator>>,
        playing: Arc<AtomicBool>,
        at_end: Arc<AtomicBool>,
        reset_in_flight: Arc<AtomicBool>,
        session_rotating: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
        session_config: Box<PlaybackLaunchConfig>,
        session_identity: Box<PlaybackSessionIdentity>,
    }

    impl WorkerActor {
        async fn run(mut self, mut receiver: mpsc::Receiver<WorkerCommand>) {
            let mut poll = tokio::time::interval(SCHEDULER_POLL);
            poll.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    command = receiver.recv() => {
                        let Some(command) = command else {
                            let _ = self.stop(ShutdownReason::ApplicationExit).await;
                            break;
                        };
                        if self.handle_command(command).await {
                            break;
                        }
                    }
                    _ = poll.tick() => {
                        if let Err(error) = self.schedule_once().await {
                            self.fail(error).await;
                            break;
                        }
                    }
                }
            }
        }

        async fn handle_command(&mut self, command: WorkerCommand) -> bool {
            match command {
                WorkerCommand::Reset {
                    reason,
                    resume,
                    reply,
                } => {
                    let result = self.reset(reason, resume).await;
                    let failure = result.as_ref().err().copied();
                    self.reset_in_flight.store(false, Ordering::Release);
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                    if let Some(error) = failure {
                        self.fail(error).await;
                        return true;
                    }
                    false
                }
                WorkerCommand::PresenterFault(error) => {
                    self.fail(error).await;
                    true
                }
                WorkerCommand::Diagnostics { reply } => {
                    if let Err(error) = self.ensure_worker_session_ready().await {
                        let _ = reply.send(Err(error));
                        self.fail(error).await;
                        return true;
                    }
                    let result = match self
                        .client
                        .call(
                            Command::MetricsGet(EmptyPayload {}),
                            DIAGNOSTIC_REPLY_TIMEOUT,
                        )
                        .await
                        .map_err(map_worker_error)
                    {
                        Ok(Ack::MetricsGet(metrics)) => Ok(metrics),
                        Ok(_) => Err(PlaybackRuntimeError::worker_protocol()),
                        Err(error) => Err(error),
                    };
                    let _ = reply.send(result);
                    false
                }
                WorkerCommand::Shutdown { reply } => {
                    let result = self.stop(ShutdownReason::ApplicationExit).await;
                    let _ = reply.send(result);
                    true
                }
            }
        }

        async fn schedule_once(&mut self) -> Result<(), PlaybackRuntimeError> {
            if self.schedule.end_of_stream() {
                return Ok(());
            }
            let expected =
                expected_cycle_frames(&self.slot.timing, self.schedule.next_cycle_index())?;
            let state = self
                .owner
                .state()
                .map_err(|_| PlaybackRuntimeError::ring())?;
            if !state.can_publish(expected) {
                return Ok(());
            }
            self.ensure_worker_session_ready().await?;
            let _ = decode_next(
                &mut self.client,
                &mut self.schedule,
                &self.slot,
                &mut self.owner,
            )
            .await?;
            Ok(())
        }

        async fn reset(
            &mut self,
            reason: ResetReason,
            resume: bool,
        ) -> Result<PlayerView, PlaybackRuntimeError> {
            // Rotation is admitted only at this top-level actor boundary. The
            // SlotReset -> generation adoption -> prime sequence below stays
            // on one authenticated session.
            self.ensure_worker_session_ready().await?;
            self.playing.store(false, Ordering::Release);
            request_presenter(
                &self.presenter_tx,
                PresenterRequest::Quiesce,
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            let command = self
                .schedule
                .begin_reset(reason)
                .map_err(|_| PlaybackRuntimeError::schedule())?;
            let ack = self
                .client
                .call(command, COMMAND_TIMEOUT)
                .await
                .map_err(map_worker_error)?;
            let Ack::SlotReset(reset) = ack else {
                return Err(PlaybackRuntimeError::worker_protocol());
            };
            self.schedule
                .accept_reset(&reset)
                .map_err(|_| PlaybackRuntimeError::schedule())?;
            let generation = self.schedule.generation();
            self.owner
                .adopt_generation(generation)
                .map_err(|_| PlaybackRuntimeError::ring())?;
            ensure_zero_ring(
                self.owner
                    .state()
                    .map_err(|_| PlaybackRuntimeError::ring())?,
            )?;
            request_presenter(
                &self.presenter_tx,
                PresenterRequest::AdoptGeneration(generation),
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            self.ensure_reserved_message_capacity()?;
            if !decode_next(
                &mut self.client,
                &mut self.schedule,
                &self.slot,
                &mut self.owner,
            )
            .await?
            {
                return Err(PlaybackRuntimeError::ring());
            }
            self.at_end.store(false, Ordering::Release);
            let mut view = with_player(&self.player, PlayerCoordinator::reset_to_start)?;
            if resume {
                view = with_player(&self.player, |player| player.set_playing(true))?;
                self.playing.store(true, Ordering::Release);
                request_presenter(
                    &self.presenter_tx,
                    PresenterRequest::Resume,
                    ACTOR_REPLY_TIMEOUT,
                )
                .await?;
            }
            Ok(view)
        }

        async fn ensure_worker_session_ready(&mut self) -> Result<(), PlaybackRuntimeError> {
            if !session_rotation_required(
                self.client.remaining_inbound_message_budget(),
                self.client.remaining_outbound_message_budget(),
            ) {
                return Ok(());
            }
            self.rotate_worker_session().await
        }

        fn ensure_reserved_message_capacity(&self) -> Result<(), PlaybackRuntimeError> {
            if reserved_message_capacity_available(
                self.client.remaining_inbound_message_budget(),
                self.client.remaining_outbound_message_budget(),
            ) {
                Ok(())
            } else {
                Err(PlaybackRuntimeError::worker_protocol())
            }
        }

        fn rotation_config(&self) -> Result<Box<PlaybackLaunchConfig>, PlaybackRuntimeError> {
            // The current worker acknowledgement must still describe the
            // retained cartridge. The cartridge path/hash and the complete
            // validated Codec Pack + decoder selection remain in this private
            // immutable config and are replayed verbatim into the new worker.
            validate_slot(&self.slot, &self.session_config.cartridge, &h3_profile())?;
            if PlaybackSessionIdentity::from_config(&self.session_config) != *self.session_identity
            {
                return Err(PlaybackRuntimeError::worker_protocol());
            }
            Ok(self.session_config.clone())
        }

        async fn rotate_worker_session(&mut self) -> Result<(), PlaybackRuntimeError> {
            let mut guard = SessionRotationGuard::acquire(Arc::clone(&self.session_rotating))?;
            let config = self.rotation_config()?;

            request_presenter(
                &self.presenter_tx,
                PresenterRequest::Quiesce,
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            stop_worker(&mut self.client, ShutdownReason::Recovery).await?;

            let (mut replacement_client, loaded) = spawn_loaded_worker_session(&config).await?;
            let LoadedWorkerSession {
                schedule,
                slot,
                owner,
                consumer,
            } = loaded;
            let primed_frame_count = match expected_cycle_frames(&slot.timing, 0) {
                Ok(value) => value,
                Err(error) => {
                    stop_worker(&mut replacement_client, ShutdownReason::Recovery).await?;
                    return Err(error);
                }
            };
            let replace = request_presenter(
                &self.presenter_tx,
                PresenterRequest::ReplaceSession {
                    consumer,
                    generation: INITIAL_GENERATION,
                    frame_count: config.cartridge.frame_count,
                    frame_rate_numerator: config.cartridge.frame_rate_numerator,
                    frame_rate_denominator: config.cartridge.frame_rate_denominator,
                    primed_frame_count,
                },
                ACTOR_REPLY_TIMEOUT,
            )
            .await;
            let replace = replace.and_then(|reply| validate_replace_session_reply(&reply));
            if let Err(error) = replace {
                let cleanup = stop_worker(&mut replacement_client, ShutdownReason::Recovery).await;
                return cleanup.and(Err(error));
            }

            // Commit only after PresenterActor owns and has acknowledged the
            // fresh consumer. Dropping the previous fields now releases the
            // old mapping after its worker has already stopped.
            self.client = replacement_client;
            self.schedule = schedule;
            self.slot = slot;
            self.owner = owner;
            self.session_config = config;
            self.at_end.store(false, Ordering::Release);

            apply_rotation_intent(&self.player, self.playing.load(Ordering::Acquire))?;

            // Clear before the last intent read. A concurrent Play after this
            // point sends its own Resume; a Play while the guard was set is
            // observed by the read below. Duplicate Resume is idempotent.
            guard.release();
            let latest_playing = self.playing.load(Ordering::Acquire);
            reconcile_rotation_play_intent(&self.player, latest_playing)?;
            if latest_playing {
                request_presenter(
                    &self.presenter_tx,
                    PresenterRequest::Resume,
                    ACTOR_REPLY_TIMEOUT,
                )
                .await?;
            }
            record_global(LogLevel::Info, "player.worker_session_rotated", None);
            Ok(())
        }

        async fn fail(&mut self, error: PlaybackRuntimeError) {
            self.playing.store(false, Ordering::Release);
            self.session_rotating.store(false, Ordering::Release);
            self.closed.store(true, Ordering::Release);
            self.reset_in_flight.store(false, Ordering::Release);
            record_runtime_error(&self.player, error);
            let _ = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Stop,
                ACTOR_REPLY_TIMEOUT,
            )
            .await;
            let _ = stop_worker(&mut self.client, ShutdownReason::Recovery).await;
        }

        async fn stop(&mut self, reason: ShutdownReason) -> Result<(), PlaybackRuntimeError> {
            self.playing.store(false, Ordering::Release);
            self.session_rotating.store(false, Ordering::Release);
            let _ = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Quiesce,
                ACTOR_REPLY_TIMEOUT,
            )
            .await;
            let worker_result = stop_worker(&mut self.client, reason).await;
            let _ = request_presenter(
                &self.presenter_tx,
                PresenterRequest::Stop,
                ACTOR_REPLY_TIMEOUT,
            )
            .await;
            pause_if_playing(&self.player)?;
            update_output_available(&self.player, false)?;
            worker_result
        }
    }

    struct PresenterActor {
        output: NativeOutput,
        consumer: WindowsRgbRingConsumer,
        player: Arc<Mutex<PlayerCoordinator>>,
        worker_tx: mpsc::Sender<WorkerCommand>,
        playing: Arc<AtomicBool>,
        loop_enabled: Arc<AtomicBool>,
        at_end: Arc<AtomicBool>,
        reset_in_flight: Arc<AtomicBool>,
        closed: Arc<AtomicBool>,
        generation: u64,
        frame_count: u64,
        consumed_frames: u64,
        pending_frame: Option<latentdeck_gpu::ring::RgbaFrame>,
        clock: FrameClock,
        quiesced: bool,
        diagnostic_frames_presented: u64,
        last_presented_at: Option<Instant>,
        frame_intervals: TimingSamples,
        spout_diagnostics: SpoutDiagnosticHistory,
        viewport_revision: u64,
    }

    impl PresenterActor {
        async fn run(mut self, mut receiver: mpsc::Receiver<PresenterCommand>) {
            loop {
                if self.quiesced || !self.playing.load(Ordering::Acquire) {
                    let Some(command) = receiver.recv().await else {
                        let _ = self.output.hide();
                        break;
                    };
                    if self.handle_command(command).await {
                        break;
                    }
                    continue;
                }

                let deadline = match self.clock.next_deadline() {
                    Ok(deadline) => deadline,
                    Err(error) => {
                        self.fail(error).await;
                        break;
                    }
                };
                tokio::select! {
                    biased;
                    command = receiver.recv() => {
                        let Some(command) = command else {
                            let _ = self.output.hide();
                            break;
                        };
                        if self.handle_command(command).await {
                            break;
                        }
                    }
                    () = sleep_until(deadline) => {
                        self.clock.advance();
                        if self.playing.load(Ordering::Acquire)
                            && let Err(error) = self.present_tick().await
                        {
                            self.fail(error).await;
                            break;
                        }
                    }
                }
            }
        }

        async fn handle_command(&mut self, command: PresenterCommand) -> bool {
            let (request, reply) = command.into_parts();
            let diagnostic_request = matches!(&request, PresenterRequest::Diagnostics);
            let result = match request {
                PresenterRequest::Resume => {
                    self.quiesced = false;
                    self.last_presented_at = None;
                    self.clock.restart();
                    Ok(PresenterReply::Unit)
                }
                PresenterRequest::Quiesce => {
                    self.quiesced = true;
                    self.last_presented_at = None;
                    Ok(PresenterReply::Unit)
                }
                PresenterRequest::AdoptGeneration(generation) => {
                    self.quiesced = true;
                    self.pending_frame = None;
                    let result = self
                        .consumer
                        .adopt_generation(generation)
                        .map_err(|_| PlaybackRuntimeError::ring())
                        .and_then(|()| {
                            ensure_zero_ring(
                                self.consumer
                                    .state()
                                    .map_err(|_| PlaybackRuntimeError::ring())?,
                            )
                        });
                    if result.is_ok() {
                        self.generation = generation;
                        self.consumed_frames = 0;
                        self.last_presented_at = None;
                        self.clock.restart();
                    }
                    result.map(|()| PresenterReply::Unit)
                }
                PresenterRequest::ReplaceSession {
                    consumer,
                    generation,
                    frame_count,
                    frame_rate_numerator,
                    frame_rate_denominator,
                    primed_frame_count,
                } => self.replace_session(
                    consumer,
                    generation,
                    frame_count,
                    frame_rate_numerator,
                    frame_rate_denominator,
                    primed_frame_count,
                ),
                PresenterRequest::SetViewport(viewport) => self.set_viewport_reply(viewport),
                PresenterRequest::SpoutStatus => {
                    let status = self.output.spout_status();
                    self.spout_diagnostics.observe(&status);
                    Ok(PresenterReply::Spout(status))
                }
                PresenterRequest::ConfigureSpout { name, enabled } => {
                    if let Some(name) = name {
                        let _ = self.output.set_spout_name(name);
                        self.spout_diagnostics.observe(&self.output.spout_status());
                    }
                    if let Some(enabled) = enabled {
                        let _ = self.output.set_spout_enabled(enabled);
                        self.spout_diagnostics.observe(&self.output.spout_status());
                    }
                    Ok(PresenterReply::Spout(self.output.spout_status()))
                }
                PresenterRequest::Diagnostics => self.diagnostic_snapshot(),
                PresenterRequest::Stop => {
                    self.quiesced = true;
                    self.last_presented_at = None;
                    self.playing.store(false, Ordering::Release);
                    // Evaluate both operations: a visibility failure must not
                    // skip destruction and leave the fixed Tauri label alive.
                    let hidden = self.output.hide();
                    let destroyed = self.output.destroy();
                    let result = hidden
                        .and(destroyed)
                        .map(|()| PresenterReply::Unit)
                        .map_err(|error| PlaybackRuntimeError::output(error.code()));
                    let _ = reply.send(result);
                    return true;
                }
            };
            match result {
                Ok(value) => {
                    let _ = reply.send(Ok(value));
                    false
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                    if diagnostic_request {
                        false
                    } else {
                        self.fail(error).await;
                        true
                    }
                }
            }
        }

        fn set_viewport_reply(
            &mut self,
            viewport: PlayerViewport,
        ) -> Result<PresenterReply, PlaybackRuntimeError> {
            if viewport.revision() <= self.viewport_revision {
                return Ok(PresenterReply::Resize(ResizeOutcome::Unchanged));
            }
            let outcome = match viewport.bounds() {
                Some(bounds) => self
                    .output
                    .set_embedded_bounds(bounds)
                    .map_err(|error| PlaybackRuntimeError::output(error.code()))?,
                None => self
                    .output
                    .resize(0, 0)
                    .map_err(|error| PlaybackRuntimeError::output(error.code()))?,
            };
            if viewport.visible() {
                self.output.show()
            } else {
                self.output.hide()
            }
            .map_err(|error| PlaybackRuntimeError::output(error.code()))?;
            self.viewport_revision = viewport.revision();
            Ok(PresenterReply::Resize(outcome))
        }

        fn replace_session(
            &mut self,
            consumer: WindowsRgbRingConsumer,
            generation: u64,
            frame_count: u64,
            frame_rate_numerator: u64,
            frame_rate_denominator: u64,
            primed_frame_count: u32,
        ) -> Result<PresenterReply, PlaybackRuntimeError> {
            let descriptor = consumer.descriptor();
            let state = consumer.state().map_err(|_| PlaybackRuntimeError::ring())?;
            let (output_width, output_height) = self.output.frame_dimensions();
            if !fresh_primed_ring_matches(
                FreshRingObservation {
                    generation: descriptor.generation(),
                    width: descriptor.layout().width(),
                    height: descriptor.layout().height(),
                    producer_sequence: state.producer_sequence(),
                    consumer_sequence: state.consumer_sequence(),
                    occupancy: state.occupancy(),
                },
                FreshRingExpectation {
                    generation,
                    width: output_width,
                    height: output_height,
                    primed_frame_count,
                },
            ) || generation != INITIAL_GENERATION
                || frame_count != self.frame_count
                || frame_rate_numerator != self.clock.numerator
                || frame_rate_denominator != self.clock.denominator
            {
                return Err(PlaybackRuntimeError::ring());
            }
            let clock = FrameClock::new(frame_rate_numerator, frame_rate_denominator)?;

            self.quiesced = true;
            self.pending_frame = None;
            self.consumer = consumer;
            self.generation = generation;
            self.frame_count = frame_count;
            self.consumed_frames = 0;
            self.last_presented_at = None;
            self.clock = clock;
            Ok(PresenterReply::Unit)
        }

        fn diagnostic_snapshot(&mut self) -> Result<PresenterReply, PlaybackRuntimeError> {
            let frame_intervals = self.frame_intervals.distribution()?;
            let measured_fps = self.frame_intervals.measured_fps()?;
            // LatentPlayer does not claim a transport-ack measurement as
            // control-to-effect latency. Deck soak tests own that metric.
            let control_latency = TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0)
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let status = self.output.spout_status();
            self.spout_diagnostics.observe(&status);
            Ok(PresenterReply::Diagnostics(PresenterDiagnosticSnapshot {
                device: self.output.device_identity(),
                frames_presented: self.diagnostic_frames_presented,
                spout_frames_sent: status.submitted_frames,
                measured_fps,
                frame_intervals,
                control_latency,
                stable_errors: self.spout_diagnostics.snapshot()?,
            }))
        }

        async fn present_tick(&mut self) -> Result<(), PlaybackRuntimeError> {
            if self.pending_frame.is_none() {
                match self
                    .consumer
                    .try_read()
                    .map_err(|_| PlaybackRuntimeError::ring())?
                {
                    ReadStatus::Frame(frame) => self.pending_frame = Some(frame),
                    ReadStatus::Empty => return Ok(()),
                }
            }
            let frame = self
                .pending_frame
                .as_ref()
                .ok_or_else(PlaybackRuntimeError::ring)?;
            let expected_sequence = self
                .consumed_frames
                .checked_add(1)
                .ok_or_else(PlaybackRuntimeError::schedule)?;
            if frame.generation() != self.generation || frame.sequence() != expected_sequence {
                return Err(PlaybackRuntimeError::ring());
            }
            let outcome = self
                .output
                .present_padded_rgba(
                    frame.width(),
                    frame.height(),
                    frame.row_stride(),
                    frame.padded_rgba(),
                )
                .map_err(|error| PlaybackRuntimeError::output(error.code()))?;
            self.spout_diagnostics.observe(&self.output.spout_status());
            if outcome.locally_presented() {
                let presented_at = Instant::now();
                if let Some(previous) = self.last_presented_at.replace(presented_at) {
                    self.frame_intervals
                        .push(presented_at.saturating_duration_since(previous));
                }
                self.diagnostic_frames_presented = self
                    .diagnostic_frames_presented
                    .checked_add(1)
                    .ok_or_else(PlaybackRuntimeError::diagnostics_contract)?;
            } else {
                self.last_presented_at = None;
            }
            self.pending_frame = None;
            self.consumed_frames = expected_sequence;
            let position = expected_sequence
                .checked_sub(1)
                .ok_or_else(PlaybackRuntimeError::schedule)?;
            with_player(&self.player, |player| player.set_position_frame(position))?;
            if self.consumed_frames == self.frame_count {
                self.reached_end().await?;
            }
            Ok(())
        }

        async fn reached_end(&mut self) -> Result<(), PlaybackRuntimeError> {
            self.at_end.store(true, Ordering::Release);
            self.quiesced = true;
            self.last_presented_at = None;
            let was_playing = self.playing.swap(false, Ordering::AcqRel);
            if was_playing {
                with_player(&self.player, |player| player.set_playing(false))?;
            }
            if was_playing
                && self.loop_enabled.load(Ordering::Acquire)
                && self
                    .reset_in_flight
                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                let command = WorkerCommand::Reset {
                    reason: ResetReason::Loop,
                    resume: true,
                    reply: None,
                };
                match timeout(ACTOR_REPLY_TIMEOUT, self.worker_tx.send(command)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => {
                        self.reset_in_flight.store(false, Ordering::Release);
                        return Err(PlaybackRuntimeError::channel_closed());
                    }
                    Err(_) => {
                        self.reset_in_flight.store(false, Ordering::Release);
                        return Err(PlaybackRuntimeError::reply_timeout());
                    }
                }
            }
            Ok(())
        }

        async fn fail(&mut self, error: PlaybackRuntimeError) {
            self.quiesced = true;
            self.playing.store(false, Ordering::Release);
            self.closed.store(true, Ordering::Release);
            record_runtime_error(&self.player, error);
            let _ = self.output.hide();
            let _ = timeout(
                ACTOR_REPLY_TIMEOUT,
                self.worker_tx.send(WorkerCommand::PresenterFault(error)),
            )
            .await;
        }
    }

    enum PresenterRequest {
        Resume,
        Quiesce,
        AdoptGeneration(u64),
        ReplaceSession {
            consumer: WindowsRgbRingConsumer,
            generation: u64,
            frame_count: u64,
            frame_rate_numerator: u64,
            frame_rate_denominator: u64,
            primed_frame_count: u32,
        },
        SetViewport(PlayerViewport),
        SpoutStatus,
        ConfigureSpout {
            name: Option<String>,
            enabled: Option<bool>,
        },
        Diagnostics,
        Stop,
    }

    struct PresenterDiagnosticSnapshot {
        device: NativeDeviceIdentity,
        frames_presented: u64,
        spout_frames_sent: u64,
        measured_fps: f64,
        frame_intervals: TimingDistribution,
        control_latency: TimingDistribution,
        stable_errors: Vec<StableErrorRecord>,
    }

    enum PresenterReply {
        Unit,
        Resize(ResizeOutcome),
        Spout(NativeSpoutStatus),
        Diagnostics(PresenterDiagnosticSnapshot),
    }

    struct PresenterCommand {
        request: PresenterRequest,
        reply: oneshot::Sender<Result<PresenterReply, PlaybackRuntimeError>>,
    }

    impl PresenterCommand {
        fn into_parts(
            self,
        ) -> (
            PresenterRequest,
            oneshot::Sender<Result<PresenterReply, PlaybackRuntimeError>>,
        ) {
            (self.request, self.reply)
        }
    }

    enum WorkerCommand {
        Reset {
            reason: ResetReason,
            resume: bool,
            reply: Option<oneshot::Sender<Result<PlayerView, PlaybackRuntimeError>>>,
        },
        PresenterFault(PlaybackRuntimeError),
        Diagnostics {
            reply: oneshot::Sender<Result<MetricsSnapshot, PlaybackRuntimeError>>,
        },
        Shutdown {
            reply: oneshot::Sender<Result<(), PlaybackRuntimeError>>,
        },
    }

    async fn request_presenter(
        sender: &mpsc::Sender<PresenterCommand>,
        request: PresenterRequest,
        deadline: Duration,
    ) -> Result<PresenterReply, PlaybackRuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        send_bounded(
            sender,
            PresenterCommand {
                request,
                reply: reply_tx,
            },
            deadline,
        )
        .await?;
        receive_bounded(reply_rx, deadline).await?
    }

    fn validate_replace_session_reply(reply: &PresenterReply) -> Result<(), PlaybackRuntimeError> {
        match reply {
            PresenterReply::Unit => Ok(()),
            PresenterReply::Resize(_)
            | PresenterReply::Spout(_)
            | PresenterReply::Diagnostics(_) => Err(PlaybackRuntimeError::worker_protocol()),
        }
    }

    async fn request_worker_shutdown(
        sender: &mpsc::Sender<WorkerCommand>,
    ) -> Result<(), PlaybackRuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        send_bounded(
            sender,
            WorkerCommand::Shutdown { reply: reply_tx },
            ACTOR_REPLY_TIMEOUT,
        )
        .await?;
        receive_bounded(reply_rx, ACTOR_REPLY_TIMEOUT).await?
    }

    async fn ensure_task_cleanup(
        task: &JoinHandle<()>,
        completion: watch::Receiver<bool>,
    ) -> Result<(), PlaybackRuntimeError> {
        if wait_for_task_cleanup(completion.clone(), ACTOR_REPLY_TIMEOUT)
            .await
            .is_ok()
        {
            return Ok(());
        }
        task.abort();
        wait_for_task_cleanup(completion, ACTOR_REPLY_TIMEOUT).await
    }

    async fn wait_for_task_cleanup(
        mut completion: watch::Receiver<bool>,
        deadline: Duration,
    ) -> Result<(), PlaybackRuntimeError> {
        if *completion.borrow() {
            return Ok(());
        }
        match timeout(deadline, completion.changed()).await {
            Ok(Ok(())) if *completion.borrow() => Ok(()),
            // Closing the only sender also proves that the task future and all
            // actor-owned resources have been dropped (including after abort).
            Ok(Err(_)) => Ok(()),
            Ok(Ok(())) | Err(_) => Err(PlaybackRuntimeError::reply_timeout()),
        }
    }

    async fn request_worker_diagnostics(
        sender: &mpsc::Sender<WorkerCommand>,
    ) -> Result<MetricsSnapshot, PlaybackRuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        send_bounded(
            sender,
            WorkerCommand::Diagnostics { reply: reply_tx },
            ROTATION_AWARE_REPLY_TIMEOUT,
        )
        .await?;
        receive_bounded(reply_rx, ROTATION_AWARE_REPLY_TIMEOUT).await?
    }

    async fn send_bounded<T>(
        sender: &mpsc::Sender<T>,
        command: T,
        deadline: Duration,
    ) -> Result<(), PlaybackRuntimeError> {
        timeout(deadline, sender.send(command))
            .await
            .map_err(|_| PlaybackRuntimeError::reply_timeout())?
            .map_err(|_| PlaybackRuntimeError::channel_closed())
    }

    async fn receive_bounded<T>(
        receiver: oneshot::Receiver<T>,
        deadline: Duration,
    ) -> Result<T, PlaybackRuntimeError> {
        timeout(deadline, receiver)
            .await
            .map_err(|_| PlaybackRuntimeError::reply_timeout())?
            .map_err(|_| PlaybackRuntimeError::channel_closed())
    }

    async fn decode_next(
        client: &mut WorkerClient,
        schedule: &mut PlaybackSchedule,
        slot: &SlotLoaded,
        owner: &mut WindowsRgbRingOwner,
    ) -> Result<bool, PlaybackRuntimeError> {
        let expected = expected_cycle_frames(&slot.timing, schedule.next_cycle_index())?;
        let before = owner.state().map_err(|_| PlaybackRuntimeError::ring())?;
        if !before.can_publish(expected) {
            return Ok(false);
        }
        let command = schedule
            .next_decode_command()
            .ok_or_else(PlaybackRuntimeError::schedule)?;
        let ack = match client.call(command, COMMAND_TIMEOUT).await {
            Ok(ack) => ack,
            Err(WorkerClientError::Remote(remote))
                if remote.code == ErrorCode::RingBackpressure
                    && remote.retryable
                    && !remote.fatal =>
            {
                return Ok(false);
            }
            Err(error) => return Err(map_worker_error(error)),
        };
        let Ack::SlotDecodeCycle(decoded) = ack else {
            return Err(PlaybackRuntimeError::worker_protocol());
        };
        schedule
            .accept_decode(&decoded)
            .map_err(|_| PlaybackRuntimeError::schedule())?;
        validate_decode_ring(
            &decoded,
            before,
            owner.state().map_err(|_| PlaybackRuntimeError::ring())?,
        )?;
        Ok(true)
    }

    fn validate_decode_ring(
        ack: &DecodeCycleAck,
        before: RingState,
        after: RingState,
    ) -> Result<(), PlaybackRuntimeError> {
        let first = before
            .producer_sequence()
            .checked_add(1)
            .ok_or_else(PlaybackRuntimeError::ring)?;
        let last = ack
            .ring_last_sequence_exclusive
            .checked_sub(1)
            .ok_or_else(PlaybackRuntimeError::ring)?;
        if ack.ring_first_sequence != first || after.producer_sequence() != last {
            return Err(PlaybackRuntimeError::ring());
        }
        Ok(())
    }

    fn expected_cycle_frames(
        timing: &TimingDescriptor,
        cycle_index: u64,
    ) -> Result<u32, PlaybackRuntimeError> {
        let pattern =
            cycle_pattern(timing, cycle_index).ok_or_else(PlaybackRuntimeError::schedule)?;
        Ok(pattern.decoded_count)
    }

    fn cycle_pattern(timing: &TimingDescriptor, cycle_index: u64) -> Option<&CyclePattern> {
        [&timing.initial, &timing.steady]
            .into_iter()
            .find(|pattern| {
                cycle_index >= pattern.first_cycle_index
                    && pattern
                        .first_cycle_index
                        .checked_add(pattern.cycle_count)
                        .is_some_and(|end| cycle_index < end)
            })
    }

    fn ensure_zero_ring(state: RingState) -> Result<(), PlaybackRuntimeError> {
        if state.producer_sequence() == 0
            && state.consumer_sequence() == 0
            && state.occupancy() == 0
        {
            Ok(())
        } else {
            Err(PlaybackRuntimeError::ring())
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct SessionRotationIntent {
        playing: bool,
        loop_enabled: bool,
    }

    impl SessionRotationIntent {
        const fn new(playing: bool, loop_enabled: bool) -> Self {
            Self {
                playing,
                loop_enabled,
            }
        }

        const fn rehydrated(self) -> SessionRotationState {
            SessionRotationState {
                position_frame: 0,
                playing: self.playing,
                loop_enabled: self.loop_enabled,
                at_end: false,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct SessionRotationState {
        position_frame: u64,
        playing: bool,
        loop_enabled: bool,
        at_end: bool,
    }

    struct SessionRotationGuard {
        flag: Arc<AtomicBool>,
        active: bool,
    }

    impl SessionRotationGuard {
        fn acquire(flag: Arc<AtomicBool>) -> Result<Self, PlaybackRuntimeError> {
            flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| PlaybackRuntimeError::worker_protocol())?;
            Ok(Self { flag, active: true })
        }

        fn release(&mut self) {
            self.flag.store(false, Ordering::Release);
            self.active = false;
        }
    }

    impl Drop for SessionRotationGuard {
        fn drop(&mut self) {
            if self.active {
                self.flag.store(false, Ordering::Release);
            }
        }
    }

    const fn session_rotation_required(inbound: usize, outbound: usize) -> bool {
        inbound <= SESSION_SHUTDOWN_MESSAGE_RESERVE || outbound <= SESSION_SHUTDOWN_MESSAGE_RESERVE
    }

    const fn reserved_message_capacity_available(inbound: usize, outbound: usize) -> bool {
        inbound > 1 && outbound > 1
    }

    fn apply_rotation_intent(
        player: &Arc<Mutex<PlayerCoordinator>>,
        playing: bool,
    ) -> Result<SessionRotationState, PlaybackRuntimeError> {
        with_player(player, |player| {
            let intent = SessionRotationIntent::new(playing, player.view().loop_enabled);
            let state = intent.rehydrated();
            let mut view = player.reset_to_start()?;
            if state.playing {
                view = player.set_playing(true)?;
            }
            debug_assert_eq!(view.position_frame, state.position_frame);
            debug_assert_eq!(view.loop_enabled, state.loop_enabled);
            Ok(state)
        })
    }

    fn reconcile_rotation_play_intent(
        player: &Arc<Mutex<PlayerCoordinator>>,
        playing: bool,
    ) -> Result<(), PlaybackRuntimeError> {
        let phase = player_view(player)?.phase;
        match (playing, phase) {
            (
                true,
                latentdeck_core::player::PlayerPhase::Ready
                | latentdeck_core::player::PlayerPhase::Paused,
            ) => with_player(player, |player| player.set_playing(true)).map(|_| ()),
            (false, latentdeck_core::player::PlayerPhase::Playing) => {
                with_player(player, |player| player.set_playing(false)).map(|_| ())
            }
            _ => Ok(()),
        }
    }

    #[derive(Clone, Copy)]
    struct FreshRingObservation {
        generation: u64,
        width: u32,
        height: u32,
        producer_sequence: u64,
        consumer_sequence: u64,
        occupancy: u32,
    }

    #[derive(Clone, Copy)]
    struct FreshRingExpectation {
        generation: u64,
        width: u32,
        height: u32,
        primed_frame_count: u32,
    }

    const fn fresh_primed_ring_matches(
        actual: FreshRingObservation,
        expected: FreshRingExpectation,
    ) -> bool {
        actual.generation == expected.generation
            && actual.width == expected.width
            && actual.height == expected.height
            && expected.primed_frame_count > 0
            && actual.producer_sequence == expected.primed_frame_count as u64
            && actual.consumer_sequence == 0
            && actual.occupancy == expected.primed_frame_count
    }

    async fn stop_worker(
        client: &mut WorkerClient,
        reason: ShutdownReason,
    ) -> Result<(), PlaybackRuntimeError> {
        if client
            .request_shutdown(reason, SHUTDOWN_TIMEOUT)
            .await
            .is_ok()
        {
            return Ok(());
        }
        client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|_| PlaybackRuntimeError::worker_shutdown())
    }

    #[allow(clippy::needless_pass_by_value)] // `Result::map_err` transfers ownership.
    fn map_worker_error(error: WorkerClientError) -> PlaybackRuntimeError {
        match error {
            WorkerClientError::Remote(_) => PlaybackRuntimeError::worker_rejected(),
            WorkerClientError::Supervisor(_)
            | WorkerClientError::CommandTimeout(_)
            | WorkerClientError::HeartbeatTimeout(_)
            | WorkerClientError::UnexpectedAck { .. }
            | WorkerClientError::UnexpectedReply => PlaybackRuntimeError::worker_protocol(),
        }
    }

    fn h3_profile() -> ProfileRef {
        ProfileRef {
            codec_family: CODEC_FAMILY.to_owned(),
            profile: PROFILE_ID.to_owned(),
            profile_version: PROFILE_VERSION.to_owned(),
        }
    }

    fn path_for_protocol(path: &std::path::Path) -> Result<String, PlaybackRuntimeError> {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(PlaybackRuntimeError::input_contract)
    }

    fn parse_wire_uuid(value: &str) -> Result<WireUuid, PlaybackRuntimeError> {
        let deserializer = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value);
        WireUuid::deserialize(deserializer).map_err(|_| PlaybackRuntimeError::input_contract())
    }

    fn with_player<T>(
        player: &Arc<Mutex<PlayerCoordinator>>,
        operation: impl FnOnce(
            &mut PlayerCoordinator,
        ) -> Result<T, latentdeck_core::player::PlayerCoordinatorError>,
    ) -> Result<T, PlaybackRuntimeError> {
        let mut guard = player
            .lock()
            .map_err(|_| PlaybackRuntimeError::player_state())?;
        operation(&mut guard).map_err(|_| PlaybackRuntimeError::player_state())
    }

    fn player_view(
        player: &Arc<Mutex<PlayerCoordinator>>,
    ) -> Result<PlayerView, PlaybackRuntimeError> {
        player
            .lock()
            .map(|guard| guard.view())
            .map_err(|_| PlaybackRuntimeError::player_state())
    }

    fn update_output_available(
        player: &Arc<Mutex<PlayerCoordinator>>,
        available: bool,
    ) -> Result<(), PlaybackRuntimeError> {
        with_player(player, |player| player.set_output_available(available)).map(|_| ())
    }

    fn pause_if_playing(
        player: &Arc<Mutex<PlayerCoordinator>>,
    ) -> Result<(), PlaybackRuntimeError> {
        let phase = player_view(player)?.phase;
        if phase == latentdeck_core::player::PlayerPhase::Playing {
            with_player(player, |player| player.set_playing(false)).map(|_| ())
        } else {
            Ok(())
        }
    }

    fn record_runtime_error(player: &Arc<Mutex<PlayerCoordinator>>, error: PlaybackRuntimeError) {
        record_global(LogLevel::Error, "player.runtime_failed", Some(error.code));
        if let Ok(mut player) = player.lock() {
            let _ = player.set_runtime_error(error.code, error.message, error.recoverable);
        }
    }

    /// Presenter-owned history of stable Spout error transitions.
    ///
    /// The native boundary exposes only static path-free codes. Repeated polls
    /// of one unchanged failure do not create duplicate records; recovery to a
    /// clear status allows the same code to be recorded if it later recurs.
    struct SpoutDiagnosticHistory {
        last_error_code: Option<&'static str>,
        records: VecDeque<StableErrorRecord>,
        capture_failed: bool,
    }

    impl SpoutDiagnosticHistory {
        fn from_status(status: &NativeSpoutStatus) -> Self {
            let mut history = Self {
                last_error_code: None,
                records: VecDeque::with_capacity(MAX_STABLE_ERRORS),
                capture_failed: false,
            };
            history.observe(status);
            history
        }

        fn observe(&mut self, status: &NativeSpoutStatus) {
            if status.last_error_code == self.last_error_code {
                return;
            }
            self.last_error_code = status.last_error_code;
            let Some(code) = status.last_error_code else {
                return;
            };
            let record = diagnostic_unix_ms().and_then(|timestamp| {
                StableErrorRecord::new(
                    timestamp,
                    StableErrorSource::Presentation,
                    diagnostic_token(code)?,
                )
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())
            });
            match record {
                Ok(record) => {
                    if self.records.len() == MAX_STABLE_ERRORS {
                        let _ = self.records.pop_front();
                    }
                    self.records.push_back(record);
                }
                Err(_) => self.capture_failed = true,
            }
        }

        fn snapshot(&self) -> Result<Vec<StableErrorRecord>, PlaybackRuntimeError> {
            if self.capture_failed {
                return Err(PlaybackRuntimeError::diagnostics_contract());
            }
            Ok(self.records.iter().cloned().collect())
        }
    }

    fn diagnostic_unix_ms() -> Result<u64, PlaybackRuntimeError> {
        let milliseconds = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?
            .as_millis();
        u64::try_from(milliseconds).map_err(|_| PlaybackRuntimeError::diagnostics_contract())
    }

    struct TimingSamples {
        samples_ms: VecDeque<f64>,
        capacity: usize,
    }

    impl TimingSamples {
        fn new(capacity: usize) -> Self {
            debug_assert!(capacity > 0);
            Self {
                samples_ms: VecDeque::with_capacity(capacity),
                capacity,
            }
        }

        fn push(&mut self, duration: Duration) {
            if self.samples_ms.len() == self.capacity {
                let _ = self.samples_ms.pop_front();
            }
            self.samples_ms.push_back(duration.as_secs_f64() * 1_000.0);
        }

        fn distribution(&self) -> Result<TimingDistribution, PlaybackRuntimeError> {
            if self.samples_ms.is_empty() {
                return TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0)
                    .map_err(|_| PlaybackRuntimeError::diagnostics_contract());
            }
            let mut sorted = self.samples_ms.iter().copied().collect::<Vec<_>>();
            sorted.sort_by(f64::total_cmp);
            let sample_count = u64::try_from(sorted.len())
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let divisor = u32::try_from(sorted.len())
                .map(f64::from)
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let sum = sorted.iter().try_fold(0.0, |total, value| {
                let next = total + value;
                next.is_finite().then_some(next)
            });
            let mean = sum.ok_or_else(PlaybackRuntimeError::diagnostics_contract)? / divisor;
            let p95_rank = sorted.len().saturating_mul(95).div_ceil(100);
            let p95_index = p95_rank.saturating_sub(1);
            TimingDistribution::new(
                sample_count,
                sorted[0],
                mean,
                sorted[p95_index],
                sorted[sorted.len() - 1],
            )
            .map_err(|_| PlaybackRuntimeError::diagnostics_contract())
        }

        fn measured_fps(&self) -> Result<f64, PlaybackRuntimeError> {
            if self.samples_ms.is_empty() {
                return Ok(0.0);
            }
            let sum = self.samples_ms.iter().try_fold(0.0, |total, value| {
                let next = total + value;
                next.is_finite().then_some(next)
            });
            let count = u32::try_from(self.samples_ms.len())
                .map(f64::from)
                .map_err(|_| PlaybackRuntimeError::diagnostics_contract())?;
            let mean = sum.ok_or_else(PlaybackRuntimeError::diagnostics_contract)? / count;
            if mean <= f64::EPSILON {
                Ok(0.0)
            } else {
                Ok(1_000.0 / mean)
            }
        }
    }

    struct FrameClock {
        numerator: u64,
        denominator: u64,
        epoch: Instant,
        next_tick: u64,
    }

    impl FrameClock {
        fn new(numerator: u64, denominator: u64) -> Result<Self, PlaybackRuntimeError> {
            if numerator == 0
                || denominator == 0
                || frame_offset_ns(numerator, denominator, 1)? == 0
            {
                return Err(PlaybackRuntimeError::input_contract());
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

        fn next_deadline(&self) -> Result<Instant, PlaybackRuntimeError> {
            let nanoseconds = frame_offset_ns(self.numerator, self.denominator, self.next_tick)?;
            self.epoch
                .checked_add(Duration::from_nanos(nanoseconds))
                .ok_or_else(PlaybackRuntimeError::schedule)
        }

        fn advance(&mut self) {
            self.next_tick = self.next_tick.saturating_add(1);
        }
    }

    fn frame_offset_ns(
        numerator: u64,
        denominator: u64,
        tick: u64,
    ) -> Result<u64, PlaybackRuntimeError> {
        if numerator == 0 || denominator == 0 || tick == 0 {
            return Err(PlaybackRuntimeError::input_contract());
        }
        let value = u128::from(tick)
            .checked_mul(u128::from(denominator))
            .and_then(|value| value.checked_mul(1_000_000_000))
            .ok_or_else(PlaybackRuntimeError::schedule)?
            / u128::from(numerator);
        u64::try_from(value).map_err(|_| PlaybackRuntimeError::schedule())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn session_rotation_threshold_preserves_shutdown_reserve() {
            let above = SESSION_SHUTDOWN_MESSAGE_RESERVE + 1;
            assert!(!session_rotation_required(above, above));
            assert!(session_rotation_required(
                SESSION_SHUTDOWN_MESSAGE_RESERVE,
                above
            ));
            assert!(session_rotation_required(
                above,
                SESSION_SHUTDOWN_MESSAGE_RESERVE
            ));
        }

        #[test]
        fn reset_reply_timeout_covers_the_bounded_rotation_path() {
            // One probe/connect deadline plus configure, inspect, codec load,
            // slot load, ring bind, and initial decode. Include the old-worker
            // shutdown and presenter barrier as well.
            let bounded_rotation =
                COMMAND_TIMEOUT.saturating_mul(7) + SHUTDOWN_TIMEOUT + ACTOR_REPLY_TIMEOUT;
            assert!(ROTATION_AWARE_REPLY_TIMEOUT > bounded_rotation);
        }

        #[test]
        fn atomic_reset_follow_up_requires_two_remaining_envelopes() {
            assert!(reserved_message_capacity_available(2, 2));
            assert!(!reserved_message_capacity_available(1, 2));
            assert!(!reserved_message_capacity_available(2, 1));
        }

        #[test]
        fn rotation_rehydrates_transport_at_frame_zero_without_changing_loop() {
            for (playing, loop_enabled) in
                [(false, false), (false, true), (true, false), (true, true)]
            {
                let state = SessionRotationIntent::new(playing, loop_enabled).rehydrated();
                assert_eq!(state.position_frame, 0);
                assert_eq!(state.playing, playing);
                assert_eq!(state.loop_enabled, loop_enabled);
                assert!(!state.at_end);
            }
        }

        #[test]
        fn rotation_guard_clears_on_success_and_failure_paths() {
            let flag = Arc::new(AtomicBool::new(false));
            {
                let _failure_path =
                    SessionRotationGuard::acquire(Arc::clone(&flag)).expect("first rotation");
                assert!(flag.load(Ordering::Acquire));
                assert!(SessionRotationGuard::acquire(Arc::clone(&flag)).is_err());
            }
            assert!(!flag.load(Ordering::Acquire));

            let mut success_path =
                SessionRotationGuard::acquire(Arc::clone(&flag)).expect("second rotation");
            success_path.release();
            assert!(!flag.load(Ordering::Acquire));
        }

        #[test]
        fn source_identity_covers_exact_cartridge_and_decoder_selection() {
            let exact = PlaybackSourceIdentity {
                cartridge_path: std::path::PathBuf::from(r"W:\private\portrait.lc"),
                cartridge: CartridgeSummary {
                    cartridge_id: "11111111-1111-4111-8111-111111111111".to_owned(),
                    archive_sha256: "aa".repeat(32),
                    file_name: "portrait.lc".to_owned(),
                    width: 448,
                    height: 800,
                    frame_count: 107,
                    frame_rate_numerator: 24,
                    frame_rate_denominator: 1,
                    audio_present: true,
                },
                decoder_path: std::path::PathBuf::from(r"W:\weights\taehv.safetensors"),
                decoder_asset_id: "taehv".to_owned(),
                decoder_variant_id: "taehv-h3-v1".to_owned(),
                decoder_sha256: "bb".repeat(32),
                decoder_byte_length: 123_456,
            };
            assert_eq!(exact, exact.clone());

            let mut changed = exact.clone();
            changed.cartridge.archive_sha256 = "cc".repeat(32);
            assert_ne!(exact, changed);
            let mut changed = exact.clone();
            changed.cartridge.width = 800;
            assert_ne!(exact, changed);
            let mut changed = exact.clone();
            changed.decoder_variant_id = "different".to_owned();
            assert_ne!(exact, changed);
            let mut changed = exact.clone();
            changed.decoder_sha256 = "dd".repeat(32);
            assert_ne!(exact, changed);
            let mut changed = exact.clone();
            changed.decoder_path = std::path::PathBuf::from(r"W:\weights\other.safetensors");
            assert_ne!(exact, changed);
        }

        #[test]
        fn fresh_session_accepts_only_exact_primed_generation_one_ring() {
            let expected = FreshRingExpectation {
                generation: INITIAL_GENERATION,
                width: 448,
                height: 800,
                primed_frame_count: 17,
            };
            let exact = FreshRingObservation {
                generation: INITIAL_GENERATION,
                width: 448,
                height: 800,
                producer_sequence: 17,
                consumer_sequence: 0,
                occupancy: 17,
            };
            assert!(fresh_primed_ring_matches(exact, expected));

            for incompatible in [
                FreshRingObservation {
                    generation: 2,
                    ..exact
                },
                FreshRingObservation {
                    width: 800,
                    height: 448,
                    ..exact
                },
                FreshRingObservation {
                    producer_sequence: 0,
                    occupancy: 0,
                    ..exact
                },
                FreshRingObservation {
                    consumer_sequence: 1,
                    occupancy: 16,
                    ..exact
                },
            ] {
                assert!(!fresh_primed_ring_matches(incompatible, expected));
            }
        }

        #[test]
        fn replacement_commit_requires_presenter_unit_ack() {
            assert!(validate_replace_session_reply(&PresenterReply::Unit).is_ok());
            assert!(
                validate_replace_session_reply(&PresenterReply::Resize(ResizeOutcome::Unchanged))
                    .is_err()
            );
        }

        #[tokio::test]
        async fn aborting_cross_linked_tasks_closes_both_cleanup_barriers() {
            let (worker_tx, mut worker_rx) = mpsc::channel::<()>(1);
            let (presenter_tx, mut presenter_rx) = mpsc::channel::<()>(1);
            let (worker_cleanup_sender, worker_cleanup) = watch::channel(false);
            let (presenter_cleanup_sender, presenter_cleanup) = watch::channel(false);

            let worker_task = tauri::async_runtime::spawn(async move {
                let _presenter_liveness = presenter_tx;
                let _cleanup = worker_cleanup_sender;
                while worker_rx.recv().await.is_some() {}
            });
            let presenter_task = tauri::async_runtime::spawn(async move {
                let _worker_liveness = worker_tx;
                let _cleanup = presenter_cleanup_sender;
                while presenter_rx.recv().await.is_some() {}
            });

            worker_task.abort();
            presenter_task.abort();
            wait_for_task_cleanup(worker_cleanup, Duration::from_secs(1))
                .await
                .expect("worker actor future dropped");
            wait_for_task_cleanup(presenter_cleanup, Duration::from_secs(1))
                .await
                .expect("presenter actor future dropped");
        }

        #[tokio::test]
        async fn cleanup_barrier_times_out_while_actor_still_owns_resources() {
            let (_completion_sender, completion) = watch::channel(false);
            let error = wait_for_task_cleanup(completion, Duration::from_millis(10))
                .await
                .expect_err("live actor must not satisfy cleanup barrier");
            assert_eq!(error, PlaybackRuntimeError::reply_timeout());
        }

        #[test]
        fn rational_clock_accumulates_without_per_frame_rounding_drift() {
            assert_eq!(frame_offset_ns(24, 1, 1).expect("first"), 41_666_666);
            assert_eq!(frame_offset_ns(24, 1, 2).expect("second"), 83_333_333);
            assert_eq!(frame_offset_ns(24, 1, 3).expect("third"), 125_000_000);
        }

        #[test]
        fn cycle_frame_lookup_uses_declared_initial_and_steady_ranges() {
            let timing = TimingDescriptor {
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
                latent_slot_count: 7,
                decoded_frame_count: 22,
                cycle_count: 2,
                initial: pattern(0, 1, 17),
                steady: pattern(1, 1, 5),
                reset_required_on_wrap: true,
                arbitrary_seek: false,
                max_frames_per_cycle: 17,
            };
            assert_eq!(expected_cycle_frames(&timing, 0).expect("initial"), 17);
            assert_eq!(expected_cycle_frames(&timing, 1).expect("steady"), 5);
            assert!(expected_cycle_frames(&timing, 2).is_err());
        }

        #[test]
        fn timing_samples_keep_only_the_latest_bounded_observations() {
            let mut samples = TimingSamples::new(3);
            for milliseconds in [10, 20, 30, 40] {
                samples.push(Duration::from_millis(milliseconds));
            }

            let value = serde_json::to_value(samples.distribution().expect("distribution"))
                .expect("serialize");
            assert_eq!(value["sample_count"], 3);
            assert_eq!(value["min_ms"], 20.0);
            assert_eq!(value["mean_ms"], 30.0);
            assert_eq!(value["p95_ms"], 40.0);
            assert_eq!(value["max_ms"], 40.0);
            assert!((samples.measured_fps().expect("fps") - (1_000.0 / 30.0)).abs() < 0.001);
        }

        #[test]
        fn spout_diagnostics_record_only_bounded_stable_error_transitions() {
            let clear = spout_status(None);
            let unavailable = spout_status(Some("output.spout_unavailable"));
            let invalid_name = spout_status(Some("output.spout_name_invalid"));
            let mut history = SpoutDiagnosticHistory::from_status(&clear);

            history.observe(&unavailable);
            history.observe(&unavailable);
            let first = history.snapshot().expect("first transition");
            assert_eq!(first.len(), 1);
            let first = serde_json::to_value(&first).expect("serialize first transition");
            assert_eq!(first[0]["source"], "presentation");
            assert_eq!(first[0]["code"], "output.spout_unavailable");

            history.observe(&clear);
            history.observe(&unavailable);
            assert_eq!(history.snapshot().expect("recurring transition").len(), 2);

            for index in 0..=MAX_STABLE_ERRORS {
                history.observe(if index % 2 == 0 {
                    &invalid_name
                } else {
                    &unavailable
                });
            }
            let bounded = history.snapshot().expect("bounded transitions");
            assert_eq!(bounded.len(), MAX_STABLE_ERRORS);
            let bounded = serde_json::to_value(bounded).expect("serialize bounded transitions");
            assert_eq!(
                bounded[MAX_STABLE_ERRORS - 1]["code"],
                "output.spout_name_invalid"
            );
            assert!(!bounded.to_string().contains("path"));
        }

        fn spout_status(last_error_code: Option<&'static str>) -> NativeSpoutStatus {
            NativeSpoutStatus {
                sdk_built: true,
                ready: true,
                enabled: false,
                published: false,
                requested_name: "LatentPlayer Output".to_owned(),
                active_name: "LatentPlayer Output".to_owned(),
                width: 448,
                height: 800,
                format: "rgba8_unorm",
                submitted_frames: 0,
                last_sequence: None,
                spout_frame: None,
                last_error_code,
            }
        }

        fn pattern(first_cycle_index: u64, cycle_count: u64, decoded_count: u32) -> CyclePattern {
            CyclePattern {
                first_cycle_index,
                cycle_count,
                latent_base: 0,
                latent_stride: 5,
                latent_count: 5,
                decoded_base: 0,
                decoded_stride: decoded_count,
                decoded_count,
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(
    unused_imports,
    reason = "P1 remains an explicit compatibility bridge, not the production default"
)]
pub use windows::PlaybackRuntime;

#[cfg(not(target_os = "windows"))]
pub struct PlaybackRuntime;

#[cfg(not(target_os = "windows"))]
impl PlaybackRuntime {
    pub async fn start_protocol1_h3(
        _app: AppHandle,
        _parent: WebviewWindow,
        _player: Arc<Mutex<PlayerCoordinator>>,
        _config: PlaybackLaunchConfig,
        _viewport: PlayerViewport,
    ) -> Result<Self, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn play(&self) -> Result<PlayerView, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn pause(&self) -> Result<PlayerView, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn restart(&self) -> Result<PlayerView, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub fn set_loop(&self, _enabled: bool) -> Result<PlayerView, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn set_viewport(
        &self,
        _viewport: PlayerViewport,
    ) -> Result<ResizeOutcome, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn spout_status(&self) -> Result<NativeSpoutStatus, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn configure_spout(
        &self,
        _name: Option<String>,
        _enabled: Option<bool>,
    ) -> Result<NativeSpoutStatus, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }

    pub async fn diagnostics(
        &self,
    ) -> Result<Option<PlaybackRuntimeDiagnostics>, PlaybackRuntimeError> {
        Ok(None)
    }

    pub async fn shutdown(&self) -> Result<(), PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }
}

#[cfg(test)]
mod common_tests {
    use super::*;

    #[test]
    fn public_errors_are_path_free_and_bounded() {
        let error = PlaybackRuntimeError::worker_protocol();
        assert_eq!(error.code, "worker.protocol_failed");
        assert!(error.message.len() < 256);
        assert!(!error.message.contains(':'));
    }

    #[test]
    fn gpu_identity_contains_only_sanitized_native_tokens() {
        let identity = NativeDeviceIdentity {
            adapter_name: "NVIDIA GeForce RTX 4070".to_owned(),
            backend: "dx12",
            driver: "NVIDIA".to_owned(),
            driver_info: "32.0.15.1234".to_owned(),
            vendor_id: 0x10de,
            device_id: 0x2786,
            device_type: "discrete_gpu",
        };
        let diagnostic = diagnostic_gpu_identity(&identity).expect("diagnostic identity");
        let json = serde_json::to_string(&diagnostic).expect("serialize");

        assert!(json.contains("NVIDIA-GeForce-RTX-4070"));
        assert!(json.contains("NVIDIA-32.0.15.1234-dx12"));
        assert!(!json.contains("vendor"));
        assert!(!json.contains("device_id"));
        assert!(!json.contains('\\'));
    }
}
