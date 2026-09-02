//! Production `LatentPlayer` Protocol 2 runtime.

use std::{
    fmt,
    sync::{Arc, Mutex},
    time::Duration,
};

use latentdeck_core::player::{PlayerCoordinator, PlayerView};
use latentdeck_native_output::NativeSpoutStatus;
use tauri::{AppHandle, WebviewWindow};

use crate::{
    native_output::{PlayerViewport, ResizeOutcome},
    player_selection_v2::PreparedPlayerV2Launch,
};

pub type PlaybackLaunchConfig = PreparedPlayerV2Launch;

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

    const fn unavailable() -> Self {
        Self::new(
            "player.runtime_unavailable",
            "The Protocol 2 playback runtime is unavailable; restart playback.",
            true,
        )
    }

    const fn timeout() -> Self {
        Self::new(
            "player.runtime_timeout",
            "The Protocol 2 playback runtime did not answer within its bounded deadline.",
            true,
        )
    }

    const fn protocol() -> Self {
        Self::new(
            "worker.protocol_failed",
            "The selected Codec Pack violated the Protocol 2 Player contract.",
            true,
        )
    }

    const fn output() -> Self {
        Self::new(
            "output.runtime_failed",
            "Native DX12 output failed and playback was stopped.",
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
    };

    use latentdeck_control::v2::{
        Ack, Command, EmptyPayload, PlayerReset, PlayerState, PlayerStatusSnapshot, PlayerStep,
        ShutdownReason,
    };
    use latentdeck_core::{
        player_session_v2::{PlayerSessionV2, start_player_session_v2},
        worker_client_v2::WorkerClientV2Error,
    };
    use latentdeck_gpu::{
        ring::RingLayout,
        ring_v2::{ReadV2Status, RgbaBatchV2},
    };
    use tauri::async_runtime::JoinHandle;
    use tokio::{
        sync::{mpsc, oneshot, watch},
        time::{Instant, sleep_until, timeout},
    };

    use super::{
        AppHandle, Arc, Duration, Mutex, NativeSpoutStatus, PlaybackLaunchConfig,
        PlaybackRuntimeError, PlayerCoordinator, PlayerView, PlayerViewport, ResizeOutcome,
        WebviewWindow,
    };
    use crate::{
        native_output::{NativeOutput, native_output_config},
        playback_runtime::PlaybackRuntimeDiagnostics,
    };

    const CHANNEL_CAPACITY: usize = 8;
    const ACTOR_DEADLINE: Duration = Duration::from_secs(130);
    const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(10);

    pub struct PlaybackRuntime {
        tx: mpsc::Sender<RuntimeCommand>,
        player: Arc<Mutex<PlayerCoordinator>>,
        closed: Arc<AtomicBool>,
        cleanup_complete: watch::Receiver<bool>,
        task: JoinHandle<()>,
    }

    impl PlaybackRuntime {
        pub async fn start_protocol2(
            app: AppHandle,
            parent: WebviewWindow,
            player: Arc<Mutex<PlayerCoordinator>>,
            config: PlaybackLaunchConfig,
            viewport: PlayerViewport,
        ) -> Result<Self, PlaybackRuntimeError> {
            let player_session_id = config.host.player_session_id;
            let ring_id = config.host.ring_id;
            let generation = config.host.stream_generation;
            let command_timeout = config.host.command_timeout;
            let initial_loop_enabled = config.host.loop_enabled;
            let frame_duration = frame_duration(
                config.host.signal_geometry.frame_rate_numerator,
                config.host.signal_geometry.frame_rate_denominator,
            )?;
            let dimensions = (
                config.host.signal_geometry.decoded_width,
                config.host.signal_geometry.decoded_height,
            );
            let session = start_player_session_v2(
                config.package,
                config.cartridge,
                config.host,
                config.external_assets,
            )
            .await
            .map_err(|_| PlaybackRuntimeError::protocol())?;
            validate_status(
                session.client().last_status(),
                player_session_id,
                ring_id,
                generation,
            )?;
            let bounds = viewport.bounds().ok_or_else(PlaybackRuntimeError::output)?;
            let output = NativeOutput::new_embedded(
                &app,
                &parent,
                native_output_config(dimensions.0, dimensions.1),
                bounds,
            )
            .await
            .map_err(|_| PlaybackRuntimeError::output())?;
            if output.frame_dimensions() != dimensions
                || output.present_mode() != wgpu::PresentMode::Fifo
            {
                return Err(PlaybackRuntimeError::output());
            }
            if viewport.visible() {
                output.show().map_err(|_| PlaybackRuntimeError::output())?;
            }
            with_player(&player, |state| state.set_output_available(true))?;

            let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
            let closed = Arc::new(AtomicBool::new(false));
            let actor_closed = Arc::clone(&closed);
            let actor_player = Arc::clone(&player);
            let actor = RuntimeActor {
                session,
                output,
                player: Arc::clone(&player),
                player_session_id,
                ring_id,
                generation,
                command_timeout,
                dimensions,
                latent_slot_count: config.latent_slot_count,
                decoded_frame_count: config.cartridge_summary.frame_count,
                frame_duration,
                pending_frames: VecDeque::new(),
                presented_frames: 0,
                last_playhead_slot: 0,
                playing: false,
                loop_enabled: initial_loop_enabled,
                end_of_stream: false,
                viewport_revision: viewport.revision(),
            };
            let (cleanup_sender, cleanup_complete) = watch::channel(false);
            let task = tauri::async_runtime::spawn(async move {
                actor.run(rx).await;
                actor_closed.store(true, Ordering::Release);
                let _ = with_player(&actor_player, |state| state.set_output_available(false));
                cleanup_sender.send_replace(true);
            });
            Ok(Self {
                tx,
                player,
                closed,
                cleanup_complete,
                task,
            })
        }

        pub async fn play(&self) -> Result<PlayerView, PlaybackRuntimeError> {
            self.request(RuntimeCommand::Play).await?;
            player_view(&self.player)
        }

        pub async fn pause(&self) -> Result<PlayerView, PlaybackRuntimeError> {
            self.request(RuntimeCommand::Pause).await?;
            player_view(&self.player)
        }

        pub async fn restart(&self) -> Result<PlayerView, PlaybackRuntimeError> {
            self.request(RuntimeCommand::Restart).await?;
            player_view(&self.player)
        }

        pub fn set_loop(&self, enabled: bool) -> Result<PlayerView, PlaybackRuntimeError> {
            self.tx
                .try_send(RuntimeCommand::SetLoop(enabled))
                .map_err(|_| PlaybackRuntimeError::unavailable())?;
            with_player(&self.player, |state| state.set_loop_enabled(enabled))
        }

        pub async fn set_viewport(
            &self,
            viewport: PlayerViewport,
        ) -> Result<ResizeOutcome, PlaybackRuntimeError> {
            self.request(|reply| RuntimeCommand::Viewport(viewport, reply))
                .await
        }

        pub async fn spout_status(&self) -> Result<NativeSpoutStatus, PlaybackRuntimeError> {
            self.request(RuntimeCommand::SpoutStatus).await
        }

        pub async fn configure_spout(
            &self,
            name: Option<String>,
            enabled: Option<bool>,
        ) -> Result<NativeSpoutStatus, PlaybackRuntimeError> {
            self.request(|reply| RuntimeCommand::ConfigureSpout {
                name,
                enabled,
                reply,
            })
            .await
        }

        pub async fn diagnostics(
            &self,
        ) -> Result<Option<PlaybackRuntimeDiagnostics>, PlaybackRuntimeError> {
            self.request(RuntimeCommand::Status).await?;
            Ok(None)
        }

        pub async fn shutdown(&self) -> Result<(), PlaybackRuntimeError> {
            if self.closed.load(Ordering::Acquire) {
                return Ok(());
            }
            let result = self
                .request_with_timeout(RuntimeCommand::Shutdown, SHUTDOWN_DEADLINE)
                .await;
            self.closed.store(true, Ordering::Release);
            if result.is_err() {
                self.task.abort();
                return result;
            }
            let mut cleanup = self.cleanup_complete.clone();
            timeout(SHUTDOWN_DEADLINE, async {
                while !*cleanup.borrow() {
                    cleanup
                        .changed()
                        .await
                        .map_err(|_| PlaybackRuntimeError::unavailable())?;
                }
                Ok(())
            })
            .await
            .map_err(|_| PlaybackRuntimeError::timeout())??;
            Ok(())
        }

        async fn request<T>(
            &self,
            build: impl FnOnce(oneshot::Sender<Result<T, PlaybackRuntimeError>>) -> RuntimeCommand,
        ) -> Result<T, PlaybackRuntimeError> {
            self.request_with_timeout(build, ACTOR_DEADLINE).await
        }

        async fn request_with_timeout<T>(
            &self,
            build: impl FnOnce(oneshot::Sender<Result<T, PlaybackRuntimeError>>) -> RuntimeCommand,
            deadline: Duration,
        ) -> Result<T, PlaybackRuntimeError> {
            if self.closed.load(Ordering::Acquire) {
                return Err(PlaybackRuntimeError::unavailable());
            }
            let (reply, receive) = oneshot::channel();
            self.tx
                .send(build(reply))
                .await
                .map_err(|_| PlaybackRuntimeError::unavailable())?;
            timeout(deadline, receive)
                .await
                .map_err(|_| PlaybackRuntimeError::timeout())?
                .map_err(|_| PlaybackRuntimeError::unavailable())?
        }
    }

    impl Drop for PlaybackRuntime {
        fn drop(&mut self) {
            self.closed.store(true, Ordering::Release);
            self.task.abort();
        }
    }

    enum RuntimeCommand {
        Play(oneshot::Sender<Result<(), PlaybackRuntimeError>>),
        Pause(oneshot::Sender<Result<(), PlaybackRuntimeError>>),
        Restart(oneshot::Sender<Result<(), PlaybackRuntimeError>>),
        SetLoop(bool),
        Status(oneshot::Sender<Result<(), PlaybackRuntimeError>>),
        Viewport(
            PlayerViewport,
            oneshot::Sender<Result<ResizeOutcome, PlaybackRuntimeError>>,
        ),
        SpoutStatus(oneshot::Sender<Result<NativeSpoutStatus, PlaybackRuntimeError>>),
        ConfigureSpout {
            name: Option<String>,
            enabled: Option<bool>,
            reply: oneshot::Sender<Result<NativeSpoutStatus, PlaybackRuntimeError>>,
        },
        Shutdown(oneshot::Sender<Result<(), PlaybackRuntimeError>>),
    }

    struct RuntimeActor {
        session: PlayerSessionV2,
        output: NativeOutput,
        player: Arc<Mutex<PlayerCoordinator>>,
        player_session_id: uuid::Uuid,
        ring_id: uuid::Uuid,
        generation: u64,
        command_timeout: Duration,
        dimensions: (u32, u32),
        latent_slot_count: u64,
        decoded_frame_count: u64,
        frame_duration: Duration,
        pending_frames: VecDeque<Vec<u8>>,
        presented_frames: u64,
        last_playhead_slot: u64,
        playing: bool,
        loop_enabled: bool,
        end_of_stream: bool,
        viewport_revision: u64,
    }

    impl RuntimeActor {
        async fn run(mut self, mut rx: mpsc::Receiver<RuntimeCommand>) {
            let mut next_frame = Instant::now();
            loop {
                let command = if self.playing {
                    tokio::select! {
                        command = rx.recv() => command,
                        () = sleep_until(next_frame) => {
                            if let Err(error) = self.tick().await {
                                record_error(&self.player, error);
                                break;
                            }
                            next_frame = Instant::now() + self.frame_duration;
                            continue;
                        }
                    }
                } else {
                    rx.recv().await
                };
                let Some(command) = command else {
                    break;
                };
                if self.handle(command).await {
                    break;
                }
                next_frame = Instant::now();
            }
            let _ = self.output.hide();
            let _ = self.output.destroy();
        }

        async fn handle(&mut self, command: RuntimeCommand) -> bool {
            match command {
                RuntimeCommand::Play(reply) => {
                    let result = self.status().await.and_then(|()| {
                        self.playing = true;
                        with_player(&self.player, |state| state.set_playing_protocol2(true))
                            .map(|_| ())
                    });
                    let _ = reply.send(result);
                }
                RuntimeCommand::Pause(reply) => {
                    self.playing = false;
                    let result = self.status().await.and_then(|()| {
                        with_player(&self.player, |state| state.set_playing_protocol2(false))
                            .map(|_| ())
                    });
                    let _ = reply.send(result);
                }
                RuntimeCommand::Restart(reply) => {
                    self.playing = false;
                    let result = self.reset().await;
                    let _ = reply.send(result);
                }
                RuntimeCommand::SetLoop(enabled) => self.loop_enabled = enabled,
                RuntimeCommand::Status(reply) => {
                    let result = self.status().await;
                    let _ = reply.send(result);
                }
                RuntimeCommand::Viewport(viewport, reply) => {
                    let result = self.viewport(viewport);
                    let _ = reply.send(result);
                }
                RuntimeCommand::SpoutStatus(reply) => {
                    let _ = reply.send(Ok(self.output.spout_status()));
                }
                RuntimeCommand::ConfigureSpout {
                    name,
                    enabled,
                    reply,
                } => {
                    let result = (|| {
                        if let Some(name) = name {
                            self.output
                                .set_spout_name(name)
                                .map_err(|_| PlaybackRuntimeError::output())?;
                        }
                        if let Some(enabled) = enabled {
                            self.output
                                .set_spout_enabled(enabled)
                                .map_err(|_| PlaybackRuntimeError::output())?;
                        }
                        Ok(self.output.spout_status())
                    })();
                    let _ = reply.send(result);
                }
                RuntimeCommand::Shutdown(reply) => {
                    self.playing = false;
                    let result = self
                        .session
                        .client_mut()
                        .request_shutdown(ShutdownReason::HostExit, SHUTDOWN_DEADLINE)
                        .await
                        .map(|_| ())
                        .map_err(|_| PlaybackRuntimeError::protocol());
                    let _ = reply.send(result);
                    return true;
                }
            }
            false
        }

        async fn status(&mut self) -> Result<(), PlaybackRuntimeError> {
            let ack = self
                .session
                .client_mut()
                .call(Command::PlayerStatus(EmptyPayload {}), self.command_timeout)
                .await
                .map_err(|_| PlaybackRuntimeError::protocol())?;
            let Ack::PlayerStatus(status) = ack else {
                return Err(PlaybackRuntimeError::protocol());
            };
            validate_player_status(
                &status,
                self.player_session_id,
                self.ring_id,
                self.generation,
            )
        }

        async fn tick(&mut self) -> Result<(), PlaybackRuntimeError> {
            if self.pending_frames.is_empty() {
                if self.last_playhead_slot >= self.latent_slot_count || self.end_of_stream {
                    if self.loop_enabled {
                        self.reset().await?;
                        self.playing = true;
                    } else {
                        self.playing = false;
                        self.end_of_stream = true;
                        let _ =
                            with_player(&self.player, |state| state.set_playing_protocol2(false))?;
                        return Ok(());
                    }
                }
                self.step().await?;
            }
            let frame = self
                .pending_frames
                .pop_front()
                .ok_or_else(PlaybackRuntimeError::protocol)?;
            let layout = RingLayout::new(self.dimensions.0, self.dimensions.1)
                .map_err(|_| PlaybackRuntimeError::output())?;
            let padded = pad_tight_rgba(
                &frame,
                self.dimensions.0,
                self.dimensions.1,
                layout.row_stride(),
            )?;
            self.output
                .present_padded_rgba(
                    self.dimensions.0,
                    self.dimensions.1,
                    layout.row_stride(),
                    &padded,
                )
                .map_err(|_| PlaybackRuntimeError::output())?;
            if self.decoded_frame_count > 0 {
                let position = self.presented_frames.min(self.decoded_frame_count - 1);
                with_player(&self.player, |state| state.set_position_frame(position))?;
            }
            self.presented_frames = self.presented_frames.saturating_add(1);
            Ok(())
        }

        async fn step(&mut self) -> Result<(), PlaybackRuntimeError> {
            let ack = self
                .session
                .client_mut()
                .call(
                    Command::PlayerStep(PlayerStep {
                        player_session_id: self.player_session_id,
                        stream_generation: self.generation,
                        maximum_decoded_frames: 24,
                    }),
                    self.command_timeout,
                )
                .await
                .map_err(map_client_error)?;
            let Ack::PlayerStep(step) = ack else {
                return Err(PlaybackRuntimeError::protocol());
            };
            validate_player_status(
                &step.status,
                self.player_session_id,
                self.ring_id,
                self.generation,
            )?;
            if step.decoded_frames == 0
                || step.output_ring_id != Some(self.ring_id)
                || step.output_slot_sequence == 0
            {
                return Err(PlaybackRuntimeError::protocol());
            }
            let batch = match self
                .session
                .ring_consumer_mut()
                .try_read()
                .map_err(|_| PlaybackRuntimeError::protocol())?
            {
                ReadV2Status::Batch(batch) => batch,
                ReadV2Status::Empty => return Err(PlaybackRuntimeError::protocol()),
            };
            validate_batch(
                &batch,
                &step,
                self.player_session_id,
                self.generation,
                self.dimensions,
            )?;
            self.last_playhead_slot = step.status.playhead_slot;
            self.end_of_stream = step.status.end_of_stream;
            self.pending_frames = split_batch(&batch)?;
            Ok(())
        }

        async fn reset(&mut self) -> Result<(), PlaybackRuntimeError> {
            let new_generation = self
                .generation
                .checked_add(1)
                .ok_or_else(PlaybackRuntimeError::protocol)?;
            let ack = self
                .session
                .client_mut()
                .call(
                    Command::PlayerReset(PlayerReset {
                        player_session_id: self.player_session_id,
                        new_stream_generation: new_generation,
                    }),
                    self.command_timeout,
                )
                .await
                .map_err(|_| PlaybackRuntimeError::protocol())?;
            let Ack::PlayerReset(status) = ack else {
                return Err(PlaybackRuntimeError::protocol());
            };
            validate_player_status(
                &status,
                self.player_session_id,
                self.ring_id,
                new_generation,
            )?;
            if status.stream_sequence != 0 || status.playhead_slot != 0 || status.end_of_stream {
                return Err(PlaybackRuntimeError::protocol());
            }
            self.session
                .adopt_ring_generation(new_generation)
                .map_err(|_| PlaybackRuntimeError::protocol())?;
            self.generation = new_generation;
            self.pending_frames.clear();
            self.presented_frames = 0;
            self.last_playhead_slot = 0;
            self.end_of_stream = false;
            with_player(
                &self.player,
                latentdeck_core::player::PlayerCoordinator::reset_to_start_protocol2,
            )
            .map(|_| ())
        }

        fn viewport(
            &mut self,
            viewport: PlayerViewport,
        ) -> Result<ResizeOutcome, PlaybackRuntimeError> {
            if viewport.revision() <= self.viewport_revision {
                return Ok(ResizeOutcome::Unchanged);
            }
            let outcome = match viewport.bounds() {
                Some(bounds) => self.output.set_embedded_bounds(bounds),
                None => self.output.resize(0, 0),
            }
            .map_err(|_| PlaybackRuntimeError::output())?;
            if viewport.visible() {
                self.output.show()
            } else {
                self.output.hide()
            }
            .map_err(|_| PlaybackRuntimeError::output())?;
            self.viewport_revision = viewport.revision();
            Ok(outcome)
        }
    }

    fn validate_status(
        status: Option<&latentdeck_control::v2::StatusSnapshot>,
        _player_id: uuid::Uuid,
        _ring_id: uuid::Uuid,
        _generation: u64,
    ) -> Result<(), PlaybackRuntimeError> {
        let status = status.ok_or_else(PlaybackRuntimeError::protocol)?;
        if matches!(status.player, PlayerState::Empty | PlayerState::Faulted) {
            return Err(PlaybackRuntimeError::protocol());
        }
        Ok(())
    }

    fn validate_player_status(
        status: &PlayerStatusSnapshot,
        player_id: uuid::Uuid,
        ring_id: uuid::Uuid,
        generation: u64,
    ) -> Result<(), PlaybackRuntimeError> {
        if status.player_session_id != player_id
            || status.stream_generation != generation
            || status.decoded_ring_id != Some(ring_id)
            || matches!(status.state, PlayerState::Empty | PlayerState::Faulted)
        {
            return Err(PlaybackRuntimeError::protocol());
        }
        Ok(())
    }

    fn validate_batch(
        batch: &RgbaBatchV2,
        step: &latentdeck_control::v2::PlayerStepAck,
        player_id: uuid::Uuid,
        generation: u64,
        dimensions: (u32, u32),
    ) -> Result<(), PlaybackRuntimeError> {
        let metadata = batch.metadata();
        if metadata.session_id() != *player_id.as_bytes()
            || metadata.generation() != generation
            || metadata.logical_sequence() != step.status.stream_sequence
            || metadata.slot_sequence() != step.output_slot_sequence
            || metadata.batch() != u32::from(step.decoded_frames)
            || (batch.width(), batch.height()) != dimensions
        {
            return Err(PlaybackRuntimeError::protocol());
        }
        Ok(())
    }

    fn split_batch(batch: &RgbaBatchV2) -> Result<VecDeque<Vec<u8>>, PlaybackRuntimeError> {
        let frame_bytes = usize::try_from(u64::from(batch.width()) * u64::from(batch.height()) * 4)
            .map_err(|_| PlaybackRuntimeError::protocol())?;
        if frame_bytes == 0
            || batch.pixels().len()
                != frame_bytes
                    * usize::try_from(batch.metadata().batch())
                        .map_err(|_| PlaybackRuntimeError::protocol())?
        {
            return Err(PlaybackRuntimeError::protocol());
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
    ) -> Result<Vec<u8>, PlaybackRuntimeError> {
        let tight_stride = width
            .checked_mul(4)
            .ok_or_else(PlaybackRuntimeError::output)?;
        let expected = usize::try_from(u64::from(tight_stride) * u64::from(height))
            .map_err(|_| PlaybackRuntimeError::output())?;
        if bytes.len() != expected || row_stride < tight_stride {
            return Err(PlaybackRuntimeError::output());
        }
        let output_len = usize::try_from(u64::from(row_stride) * u64::from(height))
            .map_err(|_| PlaybackRuntimeError::output())?;
        let mut padded = vec![0_u8; output_len];
        for row in 0..usize::try_from(height).map_err(|_| PlaybackRuntimeError::output())? {
            let source =
                row * usize::try_from(tight_stride).map_err(|_| PlaybackRuntimeError::output())?;
            let target =
                row * usize::try_from(row_stride).map_err(|_| PlaybackRuntimeError::output())?;
            padded[target
                ..target
                    + usize::try_from(tight_stride).map_err(|_| PlaybackRuntimeError::output())?]
                .copy_from_slice(
                    &bytes[source
                        ..source
                            + usize::try_from(tight_stride)
                                .map_err(|_| PlaybackRuntimeError::output())?],
                );
        }
        Ok(padded)
    }

    fn frame_duration(numerator: u32, denominator: u32) -> Result<Duration, PlaybackRuntimeError> {
        if numerator == 0 || denominator == 0 {
            return Err(PlaybackRuntimeError::protocol());
        }
        Ok(Duration::from_secs_f64(
            f64::from(denominator) / f64::from(numerator),
        ))
    }

    fn map_client_error(_error: WorkerClientV2Error) -> PlaybackRuntimeError {
        PlaybackRuntimeError::protocol()
    }

    fn with_player<T>(
        player: &Arc<Mutex<PlayerCoordinator>>,
        operation: impl FnOnce(
            &mut PlayerCoordinator,
        ) -> Result<T, latentdeck_core::player::PlayerCoordinatorError>,
    ) -> Result<T, PlaybackRuntimeError> {
        let mut player = player
            .lock()
            .map_err(|_| PlaybackRuntimeError::unavailable())?;
        operation(&mut player).map_err(|_| PlaybackRuntimeError::unavailable())
    }

    fn player_view(
        player: &Arc<Mutex<PlayerCoordinator>>,
    ) -> Result<PlayerView, PlaybackRuntimeError> {
        Ok(player
            .lock()
            .map_err(|_| PlaybackRuntimeError::unavailable())?
            .view())
    }

    fn record_error(player: &Arc<Mutex<PlayerCoordinator>>, error: PlaybackRuntimeError) {
        if let Ok(mut player) = player.lock() {
            let _ = player.set_runtime_error(error.code, error.message, error.recoverable);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn tight_protocol2_rgba_is_padded_only_for_native_upload() {
            let tight = (0_u8..24).collect::<Vec<_>>();
            let padded = pad_tight_rgba(&tight, 3, 2, 16).expect("padded upload");

            assert_eq!(padded.len(), 32);
            assert_eq!(&padded[0..12], &tight[0..12]);
            assert_eq!(&padded[12..16], &[0; 4]);
            assert_eq!(&padded[16..28], &tight[12..24]);
            assert_eq!(&padded[28..32], &[0; 4]);
        }

        #[test]
        fn malformed_protocol2_rgba_is_rejected_before_native_output() {
            let error = pad_tight_rgba(&[0; 23], 3, 2, 16).expect_err("short frame");
            assert_eq!(error.code, "output.runtime_failed");
        }
    }
}

#[cfg(target_os = "windows")]
pub use windows::PlaybackRuntime;

#[cfg(not(target_os = "windows"))]
pub struct PlaybackRuntime;

#[cfg(not(target_os = "windows"))]
impl PlaybackRuntime {
    pub async fn start_protocol2(
        _app: AppHandle,
        _parent: WebviewWindow,
        _player: Arc<Mutex<PlayerCoordinator>>,
        _config: PlaybackLaunchConfig,
        _viewport: PlayerViewport,
    ) -> Result<Self, PlaybackRuntimeError> {
        Err(PlaybackRuntimeError::unsupported())
    }
}
