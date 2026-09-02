//! Shared decoded MP4 recording coordinator for Deck runtimes.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use latentdeck_output_mp4::{
    Mp4Recorder, RecorderConfig, RecorderError, RecorderState, RecorderStatus,
};

/// A clonable, lock-bounded handoff between Tauri commands and one Deck actor.
/// Encoding and finalization live on the recorder's dedicated worker thread.
#[derive(Clone, Default)]
pub(crate) struct DecodedRecordingController {
    inner: Arc<Mutex<ControllerInner>>,
    completion: Arc<Condvar>,
}

#[derive(Default)]
struct ControllerInner {
    recorder: Option<Mp4Recorder>,
    last_status: RecorderStatus,
    finalizing_epoch: Option<u64>,
    completed_finalization: Option<CompletedFinalization>,
    finalization_waiters: usize,
    next_finalization_epoch: u64,
}

#[derive(Clone)]
struct CompletedFinalization {
    epoch: u64,
    status: RecorderStatus,
    error: Option<RecorderError>,
}

impl CompletedFinalization {
    fn outcome(&self) -> Result<RecorderStatus, DecodedRecordingError> {
        self.error.map_or_else(
            || Ok(self.status.clone()),
            |error| Err(DecodedRecordingError::Recorder(error)),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodedRecordingError {
    Active,
    #[cfg(test)]
    CaptureActive,
    Recorder(RecorderError),
    State,
}

impl DecodedRecordingError {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Active => "recording.already_active",
            #[cfg(test)]
            Self::CaptureActive => "recording.capture_conflict",
            Self::Recorder(error) => error.code(),
            Self::State => "recording.state_unavailable",
        }
    }

    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Active => {
                "A decoded MP4 recording is already active; stop it before choosing another output."
            }
            #[cfg(test)]
            Self::CaptureActive => {
                "Finish or cancel latent Snapshot/Live Capture before recording decoded MP4."
            }
            Self::Recorder(RecorderError::InvalidDestination) => {
                "Choose a writable local file name ending in .mp4."
            }
            Self::Recorder(RecorderError::OutputExists) => {
                "The selected MP4 already exists and was not overwritten; choose a new file name."
            }
            Self::Recorder(RecorderError::InvalidFrame) => {
                "Decoded output changed or violated the recording frame contract."
            }
            Self::Recorder(RecorderError::QueueOverflow) => {
                "MP4 encoding could not keep up; recording stopped without slowing Deck playback."
            }
            Self::Recorder(RecorderError::UnsupportedPlatform) => {
                "Decoded MP4 recording requires the Windows release build."
            }
            Self::Recorder(RecorderError::EncoderUnavailable) => {
                "Windows Media Foundation could not start an H.264 encoder."
            }
            Self::Recorder(RecorderError::EncodeFailed) => {
                "Windows Media Foundation rejected a decoded recording frame."
            }
            Self::Recorder(RecorderError::FinalizeFailed) => {
                "The MP4 could not be finalized; no partial final file was exposed."
            }
            Self::Recorder(RecorderError::WorkerStopped) | Self::State => {
                "The decoded recording worker is unavailable."
            }
        }
    }
}

impl From<RecorderError> for DecodedRecordingError {
    fn from(value: RecorderError) -> Self {
        Self::Recorder(value)
    }
}

impl DecodedRecordingController {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn arm(
        &self,
        destination: PathBuf,
    ) -> Result<RecorderStatus, DecodedRecordingError> {
        let retired = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| DecodedRecordingError::State)?;
            if inner.finalizing_epoch.is_some()
                || inner.finalization_waiters > 0
                || inner.recorder.as_ref().is_some_and(|recorder| {
                    matches!(
                        recorder.status().state,
                        RecorderState::Armed | RecorderState::Recording | RecorderState::Finalizing
                    )
                })
            {
                return Err(DecodedRecordingError::Active);
            }
            inner.recorder.take()
        };
        // A terminal recorder may still own a joined worker handle. Retire it
        // off the controller lock before arming the replacement.
        drop(retired);
        let recorder = Mp4Recorder::start(RecorderConfig::new(destination))?;
        let status = recorder.status();
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| DecodedRecordingError::State)?;
        if inner.finalizing_epoch.is_some()
            || inner.finalization_waiters > 0
            || inner.recorder.is_some()
        {
            return Err(DecodedRecordingError::Active);
        }
        inner.completed_finalization = None;
        inner.last_status = status.clone();
        inner.recorder = Some(recorder);
        Ok(status)
    }

    /// Submit without propagating an encoder failure into the Deck actor.
    /// The returned status is emitted to UI; playback remains authoritative.
    #[cfg(test)]
    pub(crate) fn submit(
        &self,
        width: u32,
        height: u32,
        row_stride: u32,
        padded_rgba: &[u8],
    ) -> RecorderStatus {
        self.submit_if_active(width, height, row_stride, padded_rgba)
            .unwrap_or_else(|| self.status())
    }

    /// Attempt frame handoff only while a recorder handle exists. The Deck
    /// actor uses this on every presentation tick and never emits per-frame
    /// webview events; UI reads bounded status snapshots instead.
    pub(crate) fn submit_if_active(
        &self,
        width: u32,
        height: u32,
        row_stride: u32,
        padded_rgba: &[u8],
    ) -> Option<RecorderStatus> {
        let Ok(mut inner) = self.inner.lock() else {
            return Some(unavailable_status());
        };
        let recorder = inner.recorder.as_mut()?;
        let status = match recorder.submit_padded_rgba(width, height, row_stride, padded_rgba) {
            Ok(status) => status,
            Err(error) => {
                let mut status = recorder.status();
                status.state = RecorderState::Failed;
                status.error_code = Some(error.code());
                status
            }
        };
        inner.last_status = status.clone();
        Some(status)
    }

    pub(crate) fn status(&self) -> RecorderStatus {
        let Ok(mut inner) = self.inner.lock() else {
            return unavailable_status();
        };
        if let Some(recorder) = inner.recorder.as_ref() {
            inner.last_status = recorder.status();
        }
        inner.last_status.clone()
    }

    pub(crate) fn is_active(&self) -> bool {
        matches!(
            self.status().state,
            RecorderState::Armed | RecorderState::Recording | RecorderState::Finalizing
        )
    }

    pub(crate) fn stop(&self) -> Result<RecorderStatus, DecodedRecordingError> {
        let (recorder, epoch) = {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| DecodedRecordingError::State)?;
            if let Some(awaited_epoch) = inner.finalizing_epoch {
                inner.finalization_waiters = inner
                    .finalization_waiters
                    .checked_add(1)
                    .ok_or(DecodedRecordingError::State)?;
                loop {
                    if let Some(completed) = inner
                        .completed_finalization
                        .as_ref()
                        .filter(|completed| completed.epoch == awaited_epoch)
                        .cloned()
                    {
                        inner.finalization_waiters = inner.finalization_waiters.saturating_sub(1);
                        return completed.outcome();
                    }
                    inner = self
                        .completion
                        .wait(inner)
                        .map_err(|_| DecodedRecordingError::State)?;
                }
            }
            let Some(recorder) = inner.recorder.take() else {
                if let Some(completed) = inner.completed_finalization.as_ref() {
                    return completed.outcome();
                }
                return Ok(inner.last_status.clone());
            };
            let epoch = inner
                .next_finalization_epoch
                .checked_add(1)
                .ok_or(DecodedRecordingError::State)?;
            inner.next_finalization_epoch = epoch;
            inner.finalizing_epoch = Some(epoch);
            inner.completed_finalization = None;
            inner.last_status.state = RecorderState::Finalizing;
            (recorder, epoch)
        };
        let before = recorder.status();
        let result = catch_unwind(AssertUnwindSafe(|| recorder.stop()))
            .unwrap_or(Err(RecorderError::WorkerStopped));
        let (terminal, error) = match result {
            Ok(status) => (status, None),
            Err(error) => {
                let mut failed = before;
                failed.state = RecorderState::Failed;
                failed.error_code = Some(error.code());
                (failed, Some(error))
            }
        };
        let completed = CompletedFinalization {
            epoch,
            status: terminal.clone(),
            error,
        };
        let Ok(mut inner) = self.inner.lock() else {
            self.completion.notify_all();
            return Err(DecodedRecordingError::State);
        };
        if inner.finalizing_epoch != Some(epoch) {
            self.completion.notify_all();
            return Err(DecodedRecordingError::State);
        }
        inner.last_status = terminal;
        inner.completed_finalization = Some(completed.clone());
        inner.finalizing_epoch = None;
        drop(inner);
        self.completion.notify_all();
        completed.outcome()
    }
}

#[cfg(test)]
fn ensure_latent_capture_idle(capture_state: &str) -> Result<(), DecodedRecordingError> {
    if matches!(
        capture_state,
        "awaiting_reset" | "capturing" | "stop_armed" | "finalizing"
    ) {
        Err(DecodedRecordingError::CaptureActive)
    } else {
        Ok(())
    }
}

pub(crate) fn normalize_mp4_destination(
    mut destination: PathBuf,
) -> Result<PathBuf, DecodedRecordingError> {
    match destination
        .extension()
        .and_then(|extension| extension.to_str())
    {
        None => {
            let _ = destination.set_extension("mp4");
        }
        Some(extension) if extension.eq_ignore_ascii_case("mp4") => {}
        Some(_) => {
            return Err(RecorderError::InvalidDestination.into());
        }
    }
    Ok(destination)
}

fn unavailable_status() -> RecorderStatus {
    RecorderStatus {
        state: RecorderState::Failed,
        error_code: Some("recording.state_unavailable"),
        ..RecorderStatus::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use latentdeck_output_mp4::RecorderState;
    use tempfile::tempdir;

    #[cfg(windows)]
    #[test]
    fn controller_arms_records_and_finalizes_without_blocking_frame_submission() {
        let root = tempdir().expect("temporary directory");
        let destination = root.path().join("deck-output.mp4");
        let controller = DecodedRecordingController::new();
        assert_eq!(controller.status().state, RecorderState::Idle);

        let armed = controller.arm(destination.clone()).expect("armed");
        assert_eq!(armed.state, RecorderState::Armed);
        let width = 64;
        let height = 64;
        let row_stride = 256;
        let rgba = vec![127_u8; (row_stride * height) as usize];
        for _ in 0..6 {
            let accepted = controller.submit(width, height, row_stride, &rgba);
            assert_ne!(accepted.state, RecorderState::Failed);
        }

        let finished = controller.stop().expect("finalized");
        assert_eq!(finished.state, RecorderState::Finished);
        assert_eq!(finished.frames_accepted, 6);
        assert_eq!(finished.frames_written, 6);
        assert!(destination.is_file());
    }

    #[test]
    fn latent_capture_conflict_and_mp4_suffix_are_explicit() {
        for active in ["awaiting_reset", "capturing", "stop_armed", "finalizing"] {
            let error = ensure_latent_capture_idle(active).expect_err("active capture blocks MP4");
            assert_eq!(error.code(), "recording.capture_conflict");
        }
        for terminal in ["idle", "finished", "aborted", "error"] {
            ensure_latent_capture_idle(terminal).expect("terminal capture permits MP4");
        }

        let root = tempdir().expect("temporary directory");
        assert_eq!(
            normalize_mp4_destination(root.path().join("deck-output")).expect("suffix"),
            root.path().join("deck-output.mp4")
        );
        assert!(normalize_mp4_destination(root.path().join("deck-output.mov")).is_err());
    }

    #[test]
    fn a_terminally_failed_recorder_can_be_replaced_without_restarting_the_deck() {
        let root = tempdir().expect("temporary directory");
        let controller = DecodedRecordingController::new();
        controller
            .arm(root.path().join("first.mp4"))
            .expect("first recorder armed");

        let invalid = controller.submit(63, 64, 63 * 4, &vec![0_u8; 63 * 64 * 4]);
        assert_eq!(invalid.state, RecorderState::Failed);

        let replacement = controller
            .arm(root.path().join("replacement.mp4"))
            .expect("terminal recorder can be replaced");
        assert_eq!(replacement.state, RecorderState::Armed);
        assert_eq!(
            controller.stop().expect("cancel replacement").state,
            RecorderState::Cancelled
        );
    }

    #[cfg(windows)]
    #[test]
    fn failed_finalization_result_is_stable_until_a_new_recording_is_armed() {
        let root = tempdir().expect("temporary directory");
        let controller = DecodedRecordingController::new();
        controller
            .arm(root.path().join("invalid-frame.mp4"))
            .expect("recorder armed");

        let invalid = controller.submit(63, 64, 63 * 4, &vec![0_u8; 63 * 64 * 4]);
        assert_eq!(invalid.state, RecorderState::Failed);
        let first_error = controller.stop().expect_err("invalid recording fails");
        assert_eq!(
            first_error,
            DecodedRecordingError::Recorder(RecorderError::InvalidFrame)
        );
        let repeated_error = controller.stop().expect_err("terminal error is stable");
        assert_eq!(repeated_error, first_error);

        let replacement = controller
            .arm(root.path().join("replacement.mp4"))
            .expect("new arm clears the completed result");
        assert_eq!(replacement.state, RecorderState::Armed);
        assert_eq!(
            controller.stop().expect("cancel replacement").state,
            RecorderState::Cancelled
        );
    }

    #[cfg(windows)]
    #[test]
    fn concurrent_stop_waits_for_the_single_finalizer_result() {
        let root = tempdir().expect("temporary directory");
        let destination = root.path().join("concurrent-stop.mp4");
        let controller = DecodedRecordingController::new();
        controller.arm(destination.clone()).expect("armed");
        let width = 1024;
        let height = 1024;
        let row_stride = width * 4;
        let rgba = vec![64_u8; (row_stride * height) as usize];
        for _ in 0..12 {
            let status = controller.submit(width, height, row_stride, &rgba);
            assert_ne!(status.state, RecorderState::Failed);
        }

        let first = controller.clone();
        let finalizer = std::thread::spawn(move || first.stop());
        for _ in 0..250 {
            if controller.status().state == RecorderState::Finalizing {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(controller.status().state, RecorderState::Finalizing);
        let replacement = root.path().join("must-not-start.mp4");
        assert_eq!(
            controller
                .arm(replacement.clone())
                .expect_err("arm is gated"),
            DecodedRecordingError::Active
        );
        assert!(!replacement.exists());

        let joined = controller.stop().expect("second stop joins finalization");
        assert_eq!(joined.state, RecorderState::Finished);
        assert_eq!(
            finalizer
                .join()
                .expect("finalizer thread")
                .expect("finalized")
                .state,
            RecorderState::Finished
        );
        assert!(destination.is_file());
    }
}
