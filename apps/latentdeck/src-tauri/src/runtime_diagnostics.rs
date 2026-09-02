//! Shared, actor-owned diagnostic measurements for first-party Deck runtimes.

use std::{
    collections::VecDeque,
    time::{Duration, Instant, SystemTime},
};

use latentdeck_control::v2::{
    Ack, DeviceKind, ExternalAssetBinding, MetricsSnapshot as Protocol2MetricsSnapshot, ProfileKey,
};
use latentdeck_core::realtime_diagnostics::{
    DiagnosticCodecIdentity, DiagnosticGpuIdentity, MAX_STABLE_ERRORS,
    PresentationDiagnosticCounters, Protocol2CodecIdentity, Protocol2ComputeDevice,
    Protocol2DeckSessionIdentity, Protocol2ExternalAssetIdentity, RealtimeDiagnosticError,
    RealtimeSessionMetrics, SanitizedToken, Sha256Token, StableErrorRecord, StableErrorSource,
    TimingDistribution, WorkerDiagnosticCounters,
};
use latentdeck_native_output::{NativeDeviceIdentity, NativeSpoutStatus, PresentOutcome};
use uuid::Uuid;

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
    capture_failed: bool,
}

impl PresentationDiagnosticState {
    pub(crate) fn new(status: &NativeSpoutStatus) -> Self {
        Self {
            frames_presented: 0,
            last_presented_at: None,
            frame_intervals: TimingSamples::new(MAX_TIMING_SAMPLES),
            spout: SpoutDiagnosticHistory::from_status(status),
            capture_failed: false,
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

    pub(crate) fn observe_local_outcome(
        &mut self,
        outcome: PresentOutcome,
        observed_at: Instant,
    ) -> Result<(), RealtimeDiagnosticError> {
        if outcome.locally_presented() {
            self.record_presented(observed_at)
        } else {
            self.cut_interval();
            Ok(())
        }
    }

    /// Record presentation evidence without allowing a diagnostic counter
    /// failure to stop or otherwise mutate the realtime actor. The failure is
    /// retained and makes the next diagnostic snapshot fail closed.
    pub(crate) fn observe_runtime_outcome(
        &mut self,
        outcome: PresentOutcome,
        observed_at: Instant,
    ) {
        if self.observe_local_outcome(outcome, observed_at).is_err() {
            self.capture_failed = true;
        }
    }

    pub(crate) fn observe_spout(&mut self, status: &NativeSpoutStatus) {
        self.spout.observe(status);
    }

    pub(crate) fn snapshot(
        &mut self,
        spout: &NativeSpoutStatus,
    ) -> Result<PresentationDiagnosticSnapshot, RealtimeDiagnosticError> {
        if self.capture_failed {
            return Err(RealtimeDiagnosticError::InvalidCounter);
        }
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

#[derive(Clone)]
pub(crate) struct PreparedProtocol2DeckDiagnosticIdentity {
    codec: DiagnosticCodecIdentity,
    deck_session_id: Uuid,
    deck_package: SanitizedToken,
    deck_package_version: SanitizedToken,
    operator: SanitizedToken,
    operator_version: SanitizedToken,
    target_fps: f64,
}

#[derive(Clone)]
pub(crate) struct Protocol2DeckDiagnosticIdentity {
    pub(crate) codec: DiagnosticCodecIdentity,
    pub(crate) session: Protocol2DeckSessionIdentity,
    pub(crate) operator: SanitizedToken,
    pub(crate) target_fps: f64,
}

#[derive(Clone, Copy)]
pub(crate) struct Protocol2DeckDiagnosticSelection<'a> {
    pub(crate) profile: &'a ProfileKey,
    pub(crate) codec_pack: &'a str,
    pub(crate) codec_pack_version: &'a str,
    pub(crate) adapter: &'a str,
    pub(crate) adapter_version: &'a str,
    pub(crate) compute_device: DeviceKind,
    pub(crate) device_ordinal: u8,
    pub(crate) external_assets: &'a [ExternalAssetBinding],
    pub(crate) deck_session_id: Uuid,
    pub(crate) deck_package: &'a str,
    pub(crate) deck_package_version: &'a str,
    pub(crate) operator: &'a str,
    pub(crate) operator_version: &'a str,
    pub(crate) frame_rate_numerator: u32,
    pub(crate) frame_rate_denominator: u32,
}

impl PreparedProtocol2DeckDiagnosticIdentity {
    pub(crate) fn new(
        selection: Protocol2DeckDiagnosticSelection<'_>,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if selection.deck_session_id.is_nil()
            || selection.frame_rate_numerator == 0
            || selection.frame_rate_denominator == 0
        {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        let target_fps =
            f64::from(selection.frame_rate_numerator) / f64::from(selection.frame_rate_denominator);
        if !target_fps.is_finite() || target_fps <= f64::EPSILON {
            return Err(RealtimeDiagnosticError::InvalidMeasurement);
        }
        let assets = selection
            .external_assets
            .iter()
            .map(|asset| {
                Protocol2ExternalAssetIdentity::new(
                    diagnostic_token(&asset.asset_id)?,
                    Sha256Token::parse(&asset.sha256)?,
                    asset.byte_length,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let protocol2 = Protocol2CodecIdentity::new(
            diagnostic_token(&selection.profile.profile_version)?,
            diagnostic_token(selection.adapter)?,
            diagnostic_token(selection.adapter_version)?,
            match selection.compute_device {
                DeviceKind::Cpu => Protocol2ComputeDevice::Cpu,
                DeviceKind::Cuda => Protocol2ComputeDevice::Cuda,
            },
            selection.device_ordinal,
            assets,
        )?;
        Ok(Self {
            codec: DiagnosticCodecIdentity::new_protocol2(
                diagnostic_token(&selection.profile.codec_family)?,
                diagnostic_token(&selection.profile.profile)?,
                diagnostic_token(selection.codec_pack)?,
                diagnostic_token(selection.codec_pack_version)?,
                protocol2,
            ),
            deck_session_id: selection.deck_session_id,
            deck_package: diagnostic_token(selection.deck_package)?,
            deck_package_version: diagnostic_token(selection.deck_package_version)?,
            operator_version: diagnostic_token(selection.operator_version)?,
            operator: diagnostic_token(selection.operator)?,
            target_fps,
        })
    }

    pub(crate) fn complete(
        &self,
        worker_session_id: Uuid,
    ) -> Result<Protocol2DeckDiagnosticIdentity, RealtimeDiagnosticError> {
        Ok(Protocol2DeckDiagnosticIdentity {
            codec: self.codec.clone(),
            session: Protocol2DeckSessionIdentity::new(
                worker_session_id,
                self.deck_session_id,
                self.deck_package.clone(),
                self.deck_package_version.clone(),
                self.operator_version.clone(),
            )?,
            operator: self.operator.clone(),
            target_fps: self.target_fps,
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

pub(crate) fn protocol2_metrics_from_ack(
    ack: Ack,
) -> Result<Protocol2MetricsSnapshot, RealtimeDiagnosticError> {
    match ack {
        Ack::MetricsGet(metrics) => Ok(metrics),
        _ => Err(RealtimeDiagnosticError::ProtocolMismatch),
    }
}

pub(crate) fn realtime_metrics_v2(
    duration_ms: u64,
    target_fps: f64,
    worker: &Protocol2MetricsSnapshot,
    presentation: PresentationDiagnosticSnapshot,
) -> Result<RealtimeSessionMetrics, RealtimeDiagnosticError> {
    let worker = WorkerDiagnosticCounters::from_protocol2_metrics_snapshot(worker)?;
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
    fn every_locally_skipped_outcome_cuts_the_visible_frame_interval() {
        let skipped = [
            PresentOutcome::SkippedZeroSized,
            PresentOutcome::SkippedTimeout,
            PresentOutcome::SkippedOccluded,
            PresentOutcome::SkippedOutdated,
            PresentOutcome::SkippedSurfaceRecreated,
        ];

        for outcome in skipped {
            let mut state = PresentationDiagnosticState::new(&spout_status(None, 0));
            let first = Instant::now();
            state
                .observe_local_outcome(PresentOutcome::Presented, first)
                .expect("first visible frame");
            state
                .observe_local_outcome(outcome, first + Duration::from_secs(5))
                .expect("local skip");
            state
                .observe_local_outcome(PresentOutcome::Presented, first + Duration::from_secs(10))
                .expect("first restored frame");
            state
                .observe_local_outcome(
                    PresentOutcome::Presented,
                    first + Duration::from_millis(10_040),
                )
                .expect("second restored frame");

            let snapshot = state.snapshot(&spout_status(None, 0)).expect("snapshot");
            let intervals = serde_json::to_value(snapshot.frame_intervals).expect("distribution");
            assert_eq!(snapshot.frames_presented, 3);
            assert_eq!(intervals["sample_count"], 1);
            assert_eq!(intervals["min_ms"], 40.0);
            assert_eq!(intervals["max_ms"], 40.0);
        }
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
    fn protocol2_metrics_requires_the_exact_metrics_ack() {
        let expected = Protocol2MetricsSnapshot {
            worker_uptime_ns: 1,
            commands_total: 2,
            commands_failed_total: 0,
            player_steps_total: 0,
            deck_process_total: 3,
            capture_slots_total: 4,
            decoded_frames_total: 5,
        };
        assert_eq!(
            protocol2_metrics_from_ack(Ack::MetricsGet(expected.clone()))
                .expect("metrics acknowledgement"),
            expected
        );
        assert!(matches!(
            protocol2_metrics_from_ack(Ack::SessionShutdown(latentdeck_control::v2::ShutdownAck {
                reason: latentdeck_control::v2::ShutdownReason::ProtocolFault,
            },)),
            Err(RealtimeDiagnosticError::ProtocolMismatch)
        ));
    }

    #[test]
    fn prepared_protocol2_identity_keeps_exact_selection_without_paths() {
        let profile = ProfileKey {
            codec_family: "synthetic_codec".to_owned(),
            profile: "latent_signal".to_owned(),
            profile_version: "3.2.1".to_owned(),
        };
        let assets = vec![
            ExternalAssetBinding {
                asset_id: "asset-z".to_owned(),
                path: "C:\\private\\must-not-serialize-z.bin".to_owned(),
                sha256: "b".repeat(64),
                byte_length: 22,
            },
            ExternalAssetBinding {
                asset_id: "asset-a".to_owned(),
                path: "W:\\secret\\must-not-serialize-a.bin".to_owned(),
                sha256: "a".repeat(64),
                byte_length: 11,
            },
        ];
        let deck_session_id = Uuid::parse_str("30000000-0000-4000-8000-000000000001").unwrap();
        let worker_session_id = Uuid::parse_str("30000000-0000-4000-8000-000000000002").unwrap();
        let prepared =
            PreparedProtocol2DeckDiagnosticIdentity::new(Protocol2DeckDiagnosticSelection {
                profile: &profile,
                codec_pack: "org.example.codec",
                codec_pack_version: "2.8.0",
                adapter: "org.example.adapter",
                adapter_version: "0.2.0",
                compute_device: DeviceKind::Cpu,
                device_ordinal: 0,
                external_assets: &assets,
                deck_session_id,
                deck_package: "org.example.deck",
                deck_package_version: "0.2.0",
                operator: "org.example.operator",
                operator_version: "0.2.0",
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
            })
            .expect("prepared identity");
        let identity = prepared
            .complete(worker_session_id)
            .expect("active identity");
        let codec = serde_json::to_value(identity.codec).expect("codec identity");
        let session = serde_json::to_value(identity.session).expect("session identity");
        let encoded = format!("{codec}{session}");

        assert_eq!(codec["codec_family"], "synthetic_codec");
        assert_eq!(codec["codec_pack"], "org.example.codec");
        assert_eq!(codec["protocol2"]["profile_version"], "3.2.1");
        assert_eq!(codec["protocol2"]["adapter"], "org.example.adapter");
        assert_eq!(codec["protocol2"]["compute_device"], "cpu");
        assert_eq!(
            codec["protocol2"]["external_assets"][0]["asset_id"],
            "asset-a"
        );
        assert_eq!(session["worker_session_id"], worker_session_id.to_string());
        assert_eq!(session["deck_session_id"], deck_session_id.to_string());
        assert_eq!(session["deck_package"], "org.example.deck");
        assert_eq!(identity.operator.as_str(), "org.example.operator");
        assert!((identity.target_fps - 24.0).abs() < f64::EPSILON);
        assert!(!encoded.contains("private"));
        assert!(!encoded.contains("secret"));
        assert!(!encoded.contains("C:\\"));
        assert!(!encoded.contains("W:\\"));

        let unsafe_selection = Protocol2DeckDiagnosticSelection {
            adapter: "C:\\private\\adapter.py",
            ..Protocol2DeckDiagnosticSelection {
                profile: &profile,
                codec_pack: "org.example.codec",
                codec_pack_version: "2.8.0",
                adapter: "org.example.adapter",
                adapter_version: "0.2.0",
                compute_device: DeviceKind::Cpu,
                device_ordinal: 0,
                external_assets: &assets,
                deck_session_id,
                deck_package: "org.example.deck",
                deck_package_version: "0.2.0",
                operator: "org.example.operator",
                operator_version: "0.2.0",
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
            }
        };
        assert!(PreparedProtocol2DeckDiagnosticIdentity::new(unsafe_selection).is_err());
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
