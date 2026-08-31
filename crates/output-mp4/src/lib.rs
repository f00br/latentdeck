//! Bounded video-only MP4 recording for decoded RGBA frames.

#![deny(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
};

use serde::Serialize;
use thiserror::Error;

/// Fixed v0.1 recording cadence. Deck decode output is normative at 24 fps.
pub const VIDEO_FPS_NUMERATOR: u32 = 24;
/// Fixed v0.1 recording cadence denominator.
pub const VIDEO_FPS_DENOMINATOR: u32 = 1;
const FRAME_QUEUE_CAPACITY: usize = 16;
const MAX_FRAME_DIMENSION: u32 = 16_384;
// RGB Ring ABI 1 is stricter today; retain a local hard byte ceiling so this
// standalone boundary cannot be used to allocate a multi-gigabyte queue.
const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Immutable no-clobber destination selected by the native save dialog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecorderConfig {
    destination: PathBuf,
}

impl RecorderConfig {
    /// Create a decoded-video recording configuration.
    #[must_use]
    pub fn new(destination: PathBuf) -> Self {
        Self { destination }
    }

    /// Final `.mp4` path. Temporary bytes are written beside it.
    #[must_use]
    pub fn destination(&self) -> &Path {
        &self.destination
    }
}

/// Stable lifecycle reported to Deck UI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderState {
    /// No output is selected.
    #[default]
    Idle,
    /// A destination is selected; the first decoded frame fixes geometry.
    Armed,
    /// The background Media Foundation writer is accepting frames.
    Recording,
    /// The accepted frame queue is draining and the MP4 is being finalized.
    Finalizing,
    /// Finalization and the atomic destination rename completed.
    Finished,
    /// Recording stopped before any decoded frame arrived; no file was made.
    Cancelled,
    /// Recording stopped without exposing a partial final output.
    Failed,
}

/// Path-free status safe for application commands and support evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatus {
    pub state: RecorderState,
    pub frames_accepted: u64,
    pub frames_written: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub error_code: Option<&'static str>,
}

/// Sanitized recording failures. No machine path or native message crosses
/// this boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum RecorderError {
    #[error("choose an absolute local .mp4 destination")]
    InvalidDestination,
    #[error("the selected output already exists")]
    OutputExists,
    #[error("the decoded frame does not match the bounded RGBA contract")]
    InvalidFrame,
    #[error("the bounded recording queue could not keep up with decoded output")]
    QueueOverflow,
    #[error("H.264 MP4 recording is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("the operating system H.264 encoder could not start")]
    EncoderUnavailable,
    #[error("the operating system H.264 encoder rejected a frame")]
    EncodeFailed,
    #[error("the MP4 could not be finalized safely")]
    FinalizeFailed,
    #[error("the recording worker stopped unexpectedly")]
    WorkerStopped,
}

impl RecorderError {
    /// Stable UI-safe failure code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidDestination => "recording.destination_invalid",
            Self::OutputExists => "recording.output_exists",
            Self::InvalidFrame => "recording.frame_invalid",
            Self::QueueOverflow => "recording.queue_overflow",
            Self::UnsupportedPlatform => "recording.platform_unsupported",
            Self::EncoderUnavailable => "recording.encoder_unavailable",
            Self::EncodeFailed => "recording.encode_failed",
            Self::FinalizeFailed => "recording.finalize_failed",
            Self::WorkerStopped => "recording.worker_stopped",
        }
    }
}

/// Background recorder handle. Encoding begins lazily on the first decoded
/// frame so the file always uses the intrinsic cartridge geometry.
pub struct Mp4Recorder {
    config: RecorderConfig,
    status: Arc<Mutex<RecorderStatus>>,
    terminal_error: Arc<Mutex<Option<RecorderError>>>,
    cancelled: Arc<AtomicBool>,
    sender: Option<SyncSender<WorkerMessage>>,
    worker: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Mp4Recorder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Mp4Recorder")
            .field("config", &self.config)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Mp4Recorder {
    /// Validate and arm a new no-clobber recording.
    ///
    /// # Errors
    ///
    /// Rejects relative/non-MP4 destinations and existing outputs.
    pub fn start(config: RecorderConfig) -> Result<Self, RecorderError> {
        validate_destination(config.destination())?;
        let temporary = temporary_path(config.destination())?;
        let status = Arc::new(Mutex::new(RecorderStatus {
            state: RecorderState::Armed,
            ..RecorderStatus::default()
        }));
        let terminal_error = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = sync_channel(FRAME_QUEUE_CAPACITY);
        let worker_status = Arc::clone(&status);
        let worker_error = Arc::clone(&terminal_error);
        let worker_cancelled = Arc::clone(&cancelled);
        let destination = config.destination.clone();
        let worker = thread::Builder::new()
            .name("latentdeck-mp4-writer".to_owned())
            .spawn(move || {
                worker_loop(
                    &receiver,
                    &worker_status,
                    &worker_error,
                    &worker_cancelled,
                    &temporary,
                    &destination,
                );
            })
            .map_err(|_| RecorderError::WorkerStopped)?;
        Ok(Self {
            config,
            status,
            terminal_error,
            cancelled,
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    /// Read the latest path-free lifecycle snapshot.
    #[must_use]
    pub fn status(&self) -> RecorderStatus {
        self.status
            .lock()
            .map_or_else(|_| poisoned_status(), |status| status.clone())
    }

    /// Final selected path, retained privately from wire status.
    #[must_use]
    pub fn destination(&self) -> &Path {
        self.config.destination()
    }

    /// Queue one exact padded RGBA frame without waiting for the encoder.
    ///
    /// # Errors
    ///
    /// Rejects malformed/changing geometry. Queue overflow terminally stops
    /// only this recorder; Deck presentation remains independent.
    pub fn submit_padded_rgba(
        &mut self,
        width: u32,
        height: u32,
        row_stride: u32,
        padded_rgba: &[u8],
    ) -> Result<RecorderStatus, RecorderError> {
        if let Err(error) = validate_frame(width, height, row_stride, padded_rgba) {
            self.fail(error);
            return Err(error);
        }
        if let Some(error) = self.current_error() {
            return Err(error);
        }
        {
            let status = self
                .status
                .lock()
                .map_err(|_| RecorderError::WorkerStopped)?;
            if !matches!(
                status.state,
                RecorderState::Armed | RecorderState::Recording
            ) {
                return Err(RecorderError::WorkerStopped);
            }
            if status
                .width
                .zip(status.height)
                .is_some_and(|geometry| geometry != (width, height))
            {
                drop(status);
                self.fail(RecorderError::InvalidFrame);
                return Err(RecorderError::InvalidFrame);
            }
        }
        let message = WorkerMessage::Frame(DecodedFrame {
            width,
            height,
            row_stride,
            padded_rgba: padded_rgba.to_vec(),
        });
        let Some(sender) = self.sender.as_ref() else {
            return Err(RecorderError::WorkerStopped);
        };
        match sender.try_send(message) {
            Ok(()) => {
                let mut status = self
                    .status
                    .lock()
                    .map_err(|_| RecorderError::WorkerStopped)?;
                status.frames_accepted = status
                    .frames_accepted
                    .checked_add(1)
                    .ok_or(RecorderError::InvalidFrame)?;
                status.width = Some(width);
                status.height = Some(height);
                Ok(status.clone())
            }
            Err(TrySendError::Full(_)) => {
                self.fail(RecorderError::QueueOverflow);
                Err(RecorderError::QueueOverflow)
            }
            Err(TrySendError::Disconnected(_)) => {
                let error = self.current_error().unwrap_or(RecorderError::WorkerStopped);
                self.fail(error);
                Err(error)
            }
        }
    }

    /// Drain accepted frames, finalize the MP4 and atomically publish it.
    ///
    /// # Errors
    ///
    /// Returns a sanitized terminal failure and never exposes a partial final
    /// destination.
    pub fn stop(mut self) -> Result<RecorderStatus, RecorderError> {
        self.sender.take();
        self.join_worker();
        if let Some(error) = self.current_error() {
            return Err(error);
        }
        let status = self.status();
        if matches!(
            status.state,
            RecorderState::Finished | RecorderState::Cancelled
        ) {
            Ok(status)
        } else {
            Err(RecorderError::WorkerStopped)
        }
    }

    fn current_error(&self) -> Option<RecorderError> {
        self.terminal_error.lock().ok().and_then(|error| *error)
    }

    fn fail(&mut self, error: RecorderError) {
        self.cancelled.store(true, Ordering::Release);
        if let Ok(mut terminal) = self.terminal_error.lock() {
            *terminal = Some(error);
        }
        if let Ok(mut status) = self.status.lock() {
            status.state = RecorderState::Failed;
            status.error_code = Some(error.code());
        }
        self.sender.take();
    }

    fn join_worker(&mut self) {
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            self.fail(RecorderError::WorkerStopped);
        }
    }
}

impl Drop for Mp4Recorder {
    fn drop(&mut self) {
        if self.worker.is_none() {
            return;
        }
        self.cancelled.store(true, Ordering::Release);
        self.sender.take();
        self.join_worker();
    }
}

fn validate_destination(destination: &Path) -> Result<(), RecorderError> {
    if !destination.is_absolute()
        || destination.file_name().is_none()
        || !destination
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"))
        || !destination.parent().is_some_and(Path::is_dir)
    {
        return Err(RecorderError::InvalidDestination);
    }
    if destination.exists() {
        return Err(RecorderError::OutputExists);
    }
    Ok(())
}

fn validate_frame(
    width: u32,
    height: u32,
    row_stride: u32,
    padded_rgba: &[u8],
) -> Result<(), RecorderError> {
    let packed_stride = width.checked_mul(4).ok_or(RecorderError::InvalidFrame)?;
    let expected_length = usize::try_from(
        row_stride
            .checked_mul(height)
            .ok_or(RecorderError::InvalidFrame)?,
    )
    .map_err(|_| RecorderError::InvalidFrame)?;
    if width == 0
        || height == 0
        || width > MAX_FRAME_DIMENSION
        || height > MAX_FRAME_DIMENSION
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || row_stride < packed_stride
        || expected_length > MAX_FRAME_BYTES
        || padded_rgba.len() != expected_length
    {
        return Err(RecorderError::InvalidFrame);
    }
    Ok(())
}

fn temporary_path(destination: &Path) -> Result<PathBuf, RecorderError> {
    let parent = destination
        .parent()
        .filter(|parent| parent.is_dir())
        .ok_or(RecorderError::InvalidDestination)?;
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(RecorderError::InvalidDestination)?;
    for _ in 0..32 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.latentdeck-{}-{sequence}.partial.mp4",
            std::process::id()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(RecorderError::InvalidDestination)
}

fn poisoned_status() -> RecorderStatus {
    RecorderStatus {
        state: RecorderState::Failed,
        error_code: Some(RecorderError::WorkerStopped.code()),
        ..RecorderStatus::default()
    }
}

struct DecodedFrame {
    width: u32,
    height: u32,
    row_stride: u32,
    padded_rgba: Vec<u8>,
}

enum WorkerMessage {
    Frame(DecodedFrame),
}

fn worker_loop(
    receiver: &Receiver<WorkerMessage>,
    status: &Arc<Mutex<RecorderStatus>>,
    terminal_error: &Arc<Mutex<Option<RecorderError>>>,
    cancelled: &Arc<AtomicBool>,
    temporary: &Path,
    destination: &Path,
) {
    let mut sink: Option<platform::VideoSink> = None;
    while let Ok(WorkerMessage::Frame(frame)) = receiver.recv() {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        if sink.is_none() {
            match platform::VideoSink::open(temporary, frame.width, frame.height) {
                Ok(opened) => {
                    sink = Some(opened);
                    update_status(status, |snapshot| snapshot.state = RecorderState::Recording);
                }
                Err(error) => {
                    set_worker_failure(status, terminal_error, cancelled, error);
                    break;
                }
            }
        }
        let Some(writer) = sink.as_mut() else {
            break;
        };
        if let Err(error) = writer.write_rgba(
            frame.width,
            frame.height,
            frame.row_stride,
            &frame.padded_rgba,
        ) {
            set_worker_failure(status, terminal_error, cancelled, error);
            break;
        }
        update_status(status, |snapshot| {
            snapshot.frames_written = snapshot.frames_written.saturating_add(1);
        });
    }

    if cancelled.load(Ordering::Acquire) {
        drop(sink);
        cleanup_temporary(temporary);
        return;
    }
    let Some(writer) = sink else {
        update_status(status, |snapshot| snapshot.state = RecorderState::Cancelled);
        return;
    };
    update_status(status, |snapshot| {
        snapshot.state = RecorderState::Finalizing;
    });
    if writer.finish().is_err() {
        set_worker_failure(
            status,
            terminal_error,
            cancelled,
            RecorderError::FinalizeFailed,
        );
        cleanup_temporary(temporary);
        return;
    }
    if let Err(error) = platform::publish_no_clobber(temporary, destination) {
        set_worker_failure(status, terminal_error, cancelled, error);
        cleanup_temporary(temporary);
        return;
    }
    update_status(status, |snapshot| {
        snapshot.state = RecorderState::Finished;
        snapshot.error_code = None;
    });
}

fn update_status(status: &Arc<Mutex<RecorderStatus>>, update: impl FnOnce(&mut RecorderStatus)) {
    if let Ok(mut status) = status.lock() {
        update(&mut status);
    }
}

fn set_worker_failure(
    status: &Arc<Mutex<RecorderStatus>>,
    terminal_error: &Arc<Mutex<Option<RecorderError>>>,
    cancelled: &Arc<AtomicBool>,
    error: RecorderError,
) {
    cancelled.store(true, Ordering::Release);
    if let Ok(mut terminal) = terminal_error.lock() {
        *terminal = Some(error);
    }
    update_status(status, |snapshot| {
        snapshot.state = RecorderState::Failed;
        snapshot.error_code = Some(error.code());
    });
}

fn cleanup_temporary(temporary: &Path) {
    if temporary.is_file() {
        let _ = fs::remove_file(temporary);
    }
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    use super::RecorderError;

    pub(super) struct VideoSink;

    impl VideoSink {
        pub(super) fn open(_path: &Path, _width: u32, _height: u32) -> Result<Self, RecorderError> {
            Err(RecorderError::UnsupportedPlatform)
        }

        pub(super) fn write_rgba(
            &mut self,
            _width: u32,
            _height: u32,
            _row_stride: u32,
            _rgba: &[u8],
        ) -> Result<(), RecorderError> {
            Err(RecorderError::UnsupportedPlatform)
        }

        pub(super) fn finish(self) -> Result<(), RecorderError> {
            Err(RecorderError::UnsupportedPlatform)
        }
    }

    pub(super) fn publish_no_clobber(
        _temporary: &Path,
        _destination: &Path,
    ) -> Result<(), RecorderError> {
        Err(RecorderError::UnsupportedPlatform)
    }
}
