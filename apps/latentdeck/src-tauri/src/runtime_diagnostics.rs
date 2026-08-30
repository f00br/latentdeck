//! Shared, actor-owned diagnostic measurements for first-party Deck runtimes.

use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime},
};

use latentdeck_control::MetricsSnapshot;
use latentdeck_core::realtime_diagnostics::{
    DiagnosticCodecIdentity, DiagnosticGpuIdentity, MAX_STABLE_ERRORS,
    PresentationDiagnosticCounters, RealtimeDiagnosticError, RealtimeSessionMetrics,
    SanitizedToken, Sha256Token, StableErrorRecord, StableErrorSource, TimingDistribution,
    WorkerDiagnosticCounters,
};
use latentdeck_native_output::{NativeDeviceIdentity, NativeSpoutStatus};

const MAX_TIMING_SAMPLES: usize = 4_096;

pub(crate) struct PresentationDiagnosticSnapshot {
    pub(crate) frames_presented: u64,
    pub(crate) spout_frames_sent: u64,
    pub(crate) measured_fps: f64,
    pub(crate) frame_intervals: TimingDistribution,
    pub(crate) stable_errors: Vec<StableErrorRecord>,
}

pub(crate) struct PresentationDiagnosticState {
    frames_presented: u64,
    last_presented_at: Option<Instant>,
    frame_intervals: TimingSamples,
    spout: SpoutDiagnosticHistory,
}

impl PresentationDiagnosticState {
    pub(crate) fn new(status: &NativeSpoutStatus) -> Self {
        Self {
            frames_presented: 0,
            last_presented_at: None,
            frame_intervals: TimingSamples::new(MAX_TIMING_SAMPLES),
            spout: SpoutDiagnosticHistory::from_status(status),
        }
    }

    pub(crate) fn record_presented(
        &mut self,
        presented_at: Instant,
    ) -> Result<(), RealtimeDiagnosticError> {
        if let Some(previous) = self.last_presented_at.replace(presented_at) {
            self.frame_intervals
                .push(presented_at.saturating_duration_since(previous));
        }
        self.frames_presented = self
            .frames_presented
            .checked_add(1)
            .ok_or(RealtimeDiagnosticError::InvalidCounter)?;
        Ok(())
    }

    pub(crate) fn cut_interval(&mut self) {
        self.last_presented_at = None;
    }

    pub(crate) fn observe_spout(&mut self, status: &NativeSpoutStatus) {
        self.spout.observe(status);
    }

    pub(crate) fn snapshot(
        &mut self,
        spout: &NativeSpoutStatus,
    ) -> Result<PresentationDiagnosticSnapshot, RealtimeDiagnosticError> {
        self.spout.observe(spout);
        Ok(PresentationDiagnosticSnapshot {
            frames_presented: self.frames_presented,
            spout_frames_sent: spout.submitted_frames,
            measured_fps: self.frame_intervals.measured_fps()?,
            frame_intervals: self.frame_intervals.distribution()?,
            stable_errors: self.spout.snapshot()?,
        })
    }
}

pub(crate) fn diagnostic_token(value: &str) -> Result<SanitizedToken, RealtimeDiagnosticError> {
    SanitizedToken::parse(value)
}

pub(crate) fn diagnostic_gpu_identity(
    identity: &NativeDeviceIdentity,
) -> Result<DiagnosticGpuIdentity, RealtimeDiagnosticError> {
    let adapter = SanitizedToken::from_hardware_label(&identity.adapter_name)?;
    let driver_label = format!(
        "{} {} {}",
        identity.driver, identity.driver_info, identity.backend
    );
    let driver = SanitizedToken::from_hardware_label(&driver_label)?;
    Ok(DiagnosticGpuIdentity::new(adapter, driver))
}

pub(crate) fn diagnostic_codec_identity(
    codec_family: &str,
    profile: &str,
    codec_pack: &str,
    codec_pack_version: &str,
    decoder: &str,
    decoder_sha256: &str,
) -> Result<DiagnosticCodecIdentity, RealtimeDiagnosticError> {
    Ok(DiagnosticCodecIdentity::new(
        diagnostic_token(codec_family)?,
        diagnostic_token(profile)?,
        diagnostic_token(codec_pack)?,
        diagnostic_token(codec_pack_version)?,
        diagnostic_token(decoder)?,
        Some(Sha256Token::parse(decoder_sha256)?),
    ))
}

pub(crate) fn realtime_metrics(
    duration_ms: u64,
    target_fps: f64,
    worker: &MetricsSnapshot,
    presentation: PresentationDiagnosticSnapshot,
) -> Result<RealtimeSessionMetrics, RealtimeDiagnosticError> {
    let worker = WorkerDiagnosticCounters::from_metrics_snapshot(worker)?;
    let counters = PresentationDiagnosticCounters::new(
        presentation.frames_presented,
        None,
        Some(presentation.spout_frames_sent),
    )?;
    let control_latency = TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0)?;
    RealtimeSessionMetrics::new(
        duration_ms,
        target_fps,
        presentation.measured_fps,
        presentation.frame_intervals,
        control_latency,
        worker,
        counters,
        presentation.stable_errors,
    )
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

    fn distribution(&self) -> Result<TimingDistribution, RealtimeDiagnosticError> {
        if self.samples_ms.is_empty() {
            return TimingDistribution::new(0, 0.0, 0.0, 0.0, 0.0);
        }
        let mut sorted = self.samples_ms.iter().copied().collect::<Vec<_>>();
        sorted.sort_by(f64::total_cmp);
        let sample_count =
            u64::try_from(sorted.len()).map_err(|_| RealtimeDiagnosticError::InvalidCounter)?;
        let divisor = u32::try_from(sorted.len())
            .map(f64::from)
            .map_err(|_| RealtimeDiagnosticError::InvalidMeasurement)?;
        let sum = sorted.iter().try_fold(0.0, |total, value| {
            let next = total + value;
            next.is_finite().then_some(next)
        });
        let mean = sum.ok_or(RealtimeDiagnosticError::InvalidMeasurement)? / divisor;
        let p95_rank = sorted.len().saturating_mul(95).div_ceil(100);
        let p95_index = p95_rank.saturating_sub(1);
        TimingDistribution::new(
            sample_count,
            sorted[0],
            mean,
            sorted[p95_index],
            sorted[sorted.len() - 1],
        )
    }

    fn measured_fps(&self) -> Result<f64, RealtimeDiagnosticError> {
        if self.samples_ms.is_empty() {
            return Ok(0.0);
        }
        let sum = self.samples_ms.iter().try_fold(0.0, |total, value| {
            let next = total + value;
            next.is_finite().then_some(next)
        });
        let count = u32::try_from(self.samples_ms.len())
            .map(f64::from)
            .map_err(|_| RealtimeDiagnosticError::InvalidMeasurement)?;
        let mean = sum.ok_or(RealtimeDiagnosticError::InvalidMeasurement)? / count;
        if mean <= f64::EPSILON {
            Ok(0.0)
        } else {
            Ok(1_000.0 / mean)
        }
    }
}

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

    fn snapshot(&self) -> Result<Vec<StableErrorRecord>, RealtimeDiagnosticError> {
        if self.capture_failed {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        Ok(self.records.iter().cloned().collect())
    }
}

fn diagnostic_unix_ms() -> Result<u64, RealtimeDiagnosticError> {
    let milliseconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| RealtimeDiagnosticError::InvalidTimestamp)?
        .as_millis();
    u64::try_from(milliseconds).map_err(|_| RealtimeDiagnosticError::InvalidTimestamp)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn successful_presentations_produce_cumulative_count_and_real_intervals() {
        let mut state = PresentationDiagnosticState::new(&spout_status(None, 0));
        let first = Instant::now();
        state.record_presented(first).expect("first frame");
        state
            .record_presented(first + Duration::from_millis(40))
            .expect("second frame");

        let snapshot = state.snapshot(&spout_status(None, 0)).expect("snapshot");
        assert_eq!(snapshot.frames_presented, 2);
        assert!((snapshot.measured_fps - 25.0).abs() < 1e-9);
        let value = serde_json::to_value(snapshot.frame_intervals).expect("distribution");
        assert_eq!(value["sample_count"], 1);
        assert_eq!(value["min_ms"], 40.0);
    }

    #[test]
    fn pause_or_reset_cuts_only_the_interval_not_the_cumulative_frame_count() {
        let mut state = PresentationDiagnosticState::new(&spout_status(None, 0));
        let first = Instant::now();
        state.record_presented(first).expect("first frame");
        state.cut_interval();
        state
            .record_presented(first + Duration::from_secs(5))
            .expect("first frame after resume");
        state
            .record_presented(first + Duration::from_millis(5_040))
            .expect("next frame");

        let snapshot = state.snapshot(&spout_status(None, 0)).expect("snapshot");
        let value = serde_json::to_value(snapshot.frame_intervals).expect("distribution");
        assert_eq!(snapshot.frames_presented, 3);
        assert_eq!(value["sample_count"], 1);
        assert_eq!(value["min_ms"], 40.0);
    }

    #[test]
    fn spout_history_is_transition_based_and_bounded() {
        let mut state = PresentationDiagnosticState::new(&spout_status(None, 0));
        for _ in 0..(MAX_STABLE_ERRORS + 20) {
            state.observe_spout(&spout_status(Some("spout.send_failed"), 0));
            state.observe_spout(&spout_status(None, 0));
        }

        let snapshot = state
            .snapshot(&spout_status(None, 0))
            .expect("bounded snapshot");
        assert_eq!(snapshot.stable_errors.len(), MAX_STABLE_ERRORS);
        let serialized = serde_json::to_string(&snapshot.stable_errors).expect("serialize");
        assert!(!serialized.contains('\\'));
        assert!(serialized.contains("spout.send_failed"));
    }

    #[test]
    fn worker_and_presentation_metrics_do_not_invent_drops_or_control_latency() {
        let mut state = PresentationDiagnosticState::new(&spout_status(None, 7));
        let snapshot = state
            .snapshot(&spout_status(None, 7))
            .expect("presentation");
        let metrics = realtime_metrics(
            1_000,
            24.0,
            &MetricsSnapshot {
                worker_uptime_ns: 10,
                decode_batches_total: 1,
                decoded_frames_total: 4,
                ring_backpressure_total: 2,
                presentation_skipped_total: 3,
                last_decode_duration_ns: 4,
                ring_write_sequence: 5,
                ring_read_sequence: 4,
                ring_occupancy: 1,
                gpu_allocated_bytes: None,
                gpu_reserved_bytes: None,
            },
            snapshot,
        )
        .expect("metrics");
        let value = serde_json::to_value(metrics).expect("serialize");

        assert!(value["presentation"].get("frames_dropped").is_none());
        assert_eq!(value["control_latency_ms"]["sample_count"], 0);
        assert_eq!(value["worker"]["presentation_skipped_total"], 3);
        assert_eq!(value["presentation"]["spout_frames_sent"], 7);
    }

    fn spout_status(
        last_error_code: Option<&'static str>,
        submitted_frames: u64,
    ) -> NativeSpoutStatus {
        NativeSpoutStatus {
            sdk_built: true,
            ready: true,
            enabled: true,
            published: submitted_frames > 0,
            requested_name: "LatentDeck".to_owned(),
            active_name: "LatentDeck".to_owned(),
            width: 448,
            height: 800,
            format: "rgba8_unorm",
            submitted_frames,
            last_sequence: submitted_frames.checked_sub(1),
            spout_frame: None,
            last_error_code,
        }
    }
}
