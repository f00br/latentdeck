//! Strongly typed, bounded realtime diagnostics and atomic support bundles.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use latentdeck_control::{
    MetricsSnapshot,
    v2::{MAX_EXTERNAL_ASSETS, MetricsSnapshot as Protocol2MetricsSnapshot},
};
use uuid::Uuid;

/// Current realtime snapshot and diagnostic bundle schema.
pub const REALTIME_DIAGNOSTIC_SCHEMA_VERSION: u16 = 1;
/// Default maximum number of allowlisted lifecycle events in one bundle.
pub const MAX_DIAGNOSTIC_EVENTS: usize = 65_536;
/// Maximum number of stable error-code transitions in one realtime session.
pub const MAX_STABLE_ERRORS: usize = 256;

const MAX_TOKEN_BYTES: usize = 128;
const MAX_HARDWARE_TOKEN_BYTES: usize = 96;
const MAX_EVENT_NAME_BYTES: usize = 64;
const MAX_EVENT_RECORD_BYTES: usize = 4 * 1024;
const DEFAULT_MAX_INPUT_FILES: usize = 48;
const DEFAULT_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_MAX_INPUT_BYTES: u64 = 24 * 1024 * 1024;
const HARD_MAX_INPUT_FILES: usize = 128;
const HARD_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const HARD_MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const HARD_MAX_EVENTS: usize = 131_072;
const MAX_ENUMERATED_FILES: usize = 512;
const MAX_EVENTS_BYTES: usize = 24 * 1024 * 1024;
const MAX_REALTIME_BYTES: usize = 256 * 1024;
const MAX_MANIFEST_BYTES: usize = 16 * 1024;
const MAX_COUNTER_VALUE: u64 = 1_000_000_000_000_000;
const MAX_RING_OCCUPANCY: u32 = 4_096;
const MAX_SESSION_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_UNIX_TIMESTAMP_MS: u64 = 253_402_300_799_999;
const MAX_TIMING_MS: f64 = 3_600_000.0;
const MAX_FPS: f64 = 1_000.0;

const ZIP_VERSION: u16 = 20;
const ZIP_VERSION_MADE_BY: u16 = 0x0314;
const ZIP_DOS_TIME: u16 = 0;
const ZIP_DOS_DATE: u16 = 0x0021;
const ZIP_LOCAL_SIGNATURE: u32 = 0x0403_4b50;
const ZIP_CENTRAL_SIGNATURE: u32 = 0x0201_4b50;
const ZIP_END_SIGNATURE: u32 = 0x0605_4b50;
const ENTRY_NAMES: [&str; 3] = ["manifest.json", "events.jsonl", "realtime.json"];

/// Closed failures emitted by diagnostic construction, collection, or publication.
#[derive(Debug, Error)]
pub enum RealtimeDiagnosticError {
    /// A token was empty, oversized, path-like, non-ASCII, or otherwise unsafe.
    #[error("diagnostic token is invalid")]
    InvalidToken,
    /// A digest was not exactly 64 hexadecimal characters.
    #[error("diagnostic SHA-256 token is invalid")]
    InvalidSha256,
    /// A timestamp was outside the supported Unix-millisecond range.
    #[error("diagnostic timestamp is invalid")]
    InvalidTimestamp,
    /// A realtime counter exceeded its fixed bound.
    #[error("diagnostic counter is invalid")]
    InvalidCounter,
    /// A timing or frame-rate measurement was non-finite or out of range.
    #[error("diagnostic measurement is invalid")]
    InvalidMeasurement,
    /// A session does not belong to the selected product.
    #[error("diagnostic session does not match product")]
    ProductSessionMismatch,
    /// A diagnostics-only Protocol 2 command returned a different typed reply.
    #[error("diagnostic Protocol 2 acknowledgement does not match the request")]
    ProtocolMismatch,
    /// A second session of the same kind was supplied.
    #[error("diagnostic session kind is duplicated")]
    DuplicateSession,
    /// Inactive and realtime session sections were combined.
    #[error("inactive and realtime diagnostic sessions cannot be combined")]
    ConflictingSessionState,
    /// A lifecycle JSON line did not match the closed application or worker schema.
    #[error("diagnostic lifecycle record is invalid")]
    InvalidLifecycleRecord,
    /// Collector limits were zero or exceeded their immutable ceilings.
    #[error("diagnostic collection limits are invalid")]
    InvalidCollectionLimits,
    /// A fixed collection or encoded entry exceeded its immutable bound.
    #[error("diagnostic bundle limit exceeded: {0}")]
    LimitExceeded(&'static str),
    /// The requested destination has no usable same-directory parent.
    #[error("diagnostic bundle destination is invalid")]
    InvalidDestination,
    /// The final destination already exists and was not modified.
    #[error("diagnostic bundle destination already exists")]
    OutputExists,
    /// Secure temporary-name generation failed.
    #[error("diagnostic bundle temporary-name generation failed")]
    Random,
    /// A closed JSON record could not be encoded.
    #[error("diagnostic bundle JSON encoding failed")]
    Encode(#[from] serde_json::Error),
    /// A local archive operation failed. No path is included in the error.
    #[error("diagnostic bundle I/O failed during {stage}")]
    Io {
        /// Closed operation stage, never a machine path.
        stage: &'static str,
        /// Underlying operating-system error.
        #[source]
        source: std::io::Error,
    },
}

/// Bounded path-free ASCII token used by descriptive diagnostic fields.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SanitizedToken(String);

impl SanitizedToken {
    /// Parse an already-normalized token.
    ///
    /// Allowed bytes are ASCII alphanumerics plus `.`, `_`, `-`, and `+`.
    /// Path components, separators, assignments, and repeated dots are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidToken`] for arbitrary text.
    pub fn parse(value: &str) -> Result<Self, RealtimeDiagnosticError> {
        if !is_safe_token(value, MAX_TOKEN_BYTES, true) {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        Ok(Self(value.to_owned()))
    }

    /// Convert a GPU adapter or driver display label into a bounded token.
    ///
    /// The source is rejected before normalization if it resembles a path,
    /// URL, environment expansion, credential assignment, or multiline text.
    /// Remaining ASCII punctuation and whitespace collapse to one `-`.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidToken`] for unsafe input or
    /// when normalization would leave no useful token.
    pub fn from_hardware_label(value: &str) -> Result<Self, RealtimeDiagnosticError> {
        let trimmed = value.trim();
        if trimmed.is_empty()
            || !trimmed.is_ascii()
            || trimmed.len() > MAX_HARDWARE_TOKEN_BYTES * 4
            || looks_sensitive_or_path_like(trimmed)
        {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }

        let mut normalized = String::with_capacity(trimmed.len().min(MAX_HARDWARE_TOKEN_BYTES));
        let mut separator_pending = false;
        for byte in trimmed.bytes() {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-') {
                if separator_pending
                    && !normalized.is_empty()
                    && !normalized.ends_with('-')
                    && normalized.len() < MAX_HARDWARE_TOKEN_BYTES
                {
                    normalized.push('-');
                }
                separator_pending = false;
                if normalized.len() >= MAX_HARDWARE_TOKEN_BYTES {
                    break;
                }
                normalized.push(char::from(byte));
            } else {
                separator_pending = true;
            }
        }
        while normalized.ends_with('-') {
            normalized.pop();
        }
        Self::parse(&normalized)
    }

    /// Borrow the normalized token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated, normalized SHA-256 hex token.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Sha256Token(String);

impl Sha256Token {
    /// Parse exactly 64 hexadecimal characters and normalize them to lowercase.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidSha256`] for any other value.
    pub fn parse(value: &str) -> Result<Self, RealtimeDiagnosticError> {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(RealtimeDiagnosticError::InvalidSha256);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// Hash bytes directly without exposing a source path or source label.
    #[must_use]
    pub fn digest(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    /// Borrow the normalized digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// First-party application producing the snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticProduct {
    /// Standalone synthesis application.
    LatentDeck,
    /// Standalone playback application.
    LatentPlayer,
}

/// Required product, application, and reusable runtime version identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticProductIdentity {
    product: DiagnosticProduct,
    app_version: SanitizedToken,
    runtime: SanitizedToken,
    runtime_version: SanitizedToken,
}

impl DiagnosticProductIdentity {
    /// Construct a closed first-party product identity.
    #[must_use]
    pub const fn new(
        product: DiagnosticProduct,
        app_version: SanitizedToken,
        runtime: SanitizedToken,
        runtime_version: SanitizedToken,
    ) -> Self {
        Self {
            product,
            app_version,
            runtime,
            runtime_version,
        }
    }

    /// Product kind used for session compatibility checks.
    #[must_use]
    pub const fn product(&self) -> DiagnosticProduct {
        self.product
    }
}

/// Sanitized GPU adapter and driver identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticGpuIdentity {
    adapter: SanitizedToken,
    driver: SanitizedToken,
}

impl DiagnosticGpuIdentity {
    /// Construct from already-sanitized hardware tokens.
    #[must_use]
    pub const fn new(adapter: SanitizedToken, driver: SanitizedToken) -> Self {
        Self { adapter, driver }
    }
}

/// Closed compute device selected by one Protocol 2 codec session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol2ComputeDevice {
    /// Adapter executes tensors on CPU.
    Cpu,
    /// Adapter executes tensors on one explicitly selected CUDA device.
    Cuda,
}

/// One exact, path-free Protocol 2 external-asset binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Protocol2ExternalAssetIdentity {
    asset_id: SanitizedToken,
    sha256: Sha256Token,
    byte_length: u64,
}

impl Protocol2ExternalAssetIdentity {
    /// Construct a bounded external-asset identity without retaining its path.
    ///
    /// # Errors
    ///
    /// Rejects a zero or oversized byte length.
    pub fn new(
        asset_id: SanitizedToken,
        sha256: Sha256Token,
        byte_length: u64,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if !(1..=MAX_COUNTER_VALUE).contains(&byte_length) {
            return Err(RealtimeDiagnosticError::InvalidCounter);
        }
        Ok(Self {
            asset_id,
            sha256,
            byte_length,
        })
    }
}

/// Exact Protocol 2 codec/profile/adapter/device identity.
///
/// External assets are sorted by `asset_id` and duplicate IDs are rejected so
/// semantically identical selections always serialize identically.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Protocol2CodecIdentity {
    profile_version: SanitizedToken,
    adapter: SanitizedToken,
    adapter_version: SanitizedToken,
    compute_device: Protocol2ComputeDevice,
    device_ordinal: u8,
    external_assets: Vec<Protocol2ExternalAssetIdentity>,
}

impl Protocol2CodecIdentity {
    /// Construct one deterministic, bounded Protocol 2 selection identity.
    ///
    /// # Errors
    ///
    /// Rejects more than 16 assets or duplicate asset IDs.
    pub fn new(
        profile_version: SanitizedToken,
        adapter: SanitizedToken,
        adapter_version: SanitizedToken,
        compute_device: Protocol2ComputeDevice,
        device_ordinal: u8,
        mut external_assets: Vec<Protocol2ExternalAssetIdentity>,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if external_assets.len() > MAX_EXTERNAL_ASSETS {
            return Err(RealtimeDiagnosticError::LimitExceeded(
                "protocol2_external_assets",
            ));
        }
        external_assets.sort_by(|left, right| left.asset_id.as_str().cmp(right.asset_id.as_str()));
        if external_assets
            .windows(2)
            .any(|pair| pair[0].asset_id == pair[1].asset_id)
        {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        Ok(Self {
            profile_version,
            adapter,
            adapter_version,
            compute_device,
            device_ordinal,
            external_assets,
        })
    }
}

/// Path-free codec pack identity. Protocol 1 retains the historical selected
/// decoder shape; Protocol 2 adds its exact adapter/profile/assets selection
/// under the optional `protocol2` section without changing Protocol 1 JSON.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticCodecIdentity {
    codec_family: SanitizedToken,
    profile: SanitizedToken,
    codec_pack: SanitizedToken,
    codec_pack_version: SanitizedToken,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoder: Option<SanitizedToken>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoder_sha256: Option<Sha256Token>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol2: Option<Protocol2CodecIdentity>,
}

impl DiagnosticCodecIdentity {
    /// Construct an installed, missing, or incompatible Protocol 1 identity.
    ///
    /// Closed tokens such as `missing` may represent unavailable components;
    /// the decoder hash is optional for that reason.
    #[must_use]
    pub const fn new(
        codec_family: SanitizedToken,
        profile: SanitizedToken,
        codec_pack: SanitizedToken,
        codec_pack_version: SanitizedToken,
        decoder: SanitizedToken,
        decoder_sha256: Option<Sha256Token>,
    ) -> Self {
        Self {
            codec_family,
            profile,
            codec_pack,
            codec_pack_version,
            decoder: Some(decoder),
            decoder_sha256,
            protocol2: None,
        }
    }

    /// Construct an exact Protocol 2 identity without inventing a decoder.
    #[must_use]
    pub const fn new_protocol2(
        codec_family: SanitizedToken,
        profile: SanitizedToken,
        codec_pack: SanitizedToken,
        codec_pack_version: SanitizedToken,
        protocol2: Protocol2CodecIdentity,
    ) -> Self {
        Self {
            codec_family,
            profile,
            codec_pack,
            codec_pack_version,
            decoder: None,
            decoder_sha256: None,
            protocol2: Some(protocol2),
        }
    }
}

/// Bounded worker-side cumulative counters for one session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct WorkerDiagnosticCounters {
    worker_uptime_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    decode_batches_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decoded_frames_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ring_backpressure_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presentation_skipped_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_decode_duration_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ring_write_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ring_read_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ring_occupancy: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu_allocated_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gpu_reserved_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    protocol2: Option<Protocol2WorkerDiagnosticCounters>,
}

/// Exact cumulative counters declared by Protocol 2 `metrics.get`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Protocol2WorkerDiagnosticCounters {
    #[serde(rename = "commands_total")]
    commands: u64,
    #[serde(rename = "commands_failed_total")]
    commands_failed: u64,
    #[serde(rename = "player_steps_total")]
    player_steps: u64,
    #[serde(rename = "deck_process_total")]
    deck_processes: u64,
    #[serde(rename = "capture_slots_total")]
    capture_slots: u64,
    #[serde(rename = "decoded_frames_total")]
    decoded_frames: u64,
}

impl WorkerDiagnosticCounters {
    /// Copy the exact closed worker protocol counters without approximation.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidCounter`] if a received value
    /// is outside the diagnostic safety envelope.
    pub fn from_metrics_snapshot(
        metrics: &MetricsSnapshot,
    ) -> Result<Self, RealtimeDiagnosticError> {
        validate_counters(&[
            metrics.worker_uptime_ns,
            metrics.decode_batches_total,
            metrics.decoded_frames_total,
            metrics.ring_backpressure_total,
            metrics.presentation_skipped_total,
            metrics.last_decode_duration_ns,
            metrics.ring_write_sequence,
            metrics.ring_read_sequence,
        ])?;
        validate_optional_counters(&[metrics.gpu_allocated_bytes, metrics.gpu_reserved_bytes])?;
        if metrics.ring_occupancy > MAX_RING_OCCUPANCY {
            return Err(RealtimeDiagnosticError::InvalidCounter);
        }
        Ok(Self {
            worker_uptime_ns: metrics.worker_uptime_ns,
            decode_batches_total: Some(metrics.decode_batches_total),
            decoded_frames_total: Some(metrics.decoded_frames_total),
            ring_backpressure_total: Some(metrics.ring_backpressure_total),
            presentation_skipped_total: Some(metrics.presentation_skipped_total),
            last_decode_duration_ns: Some(metrics.last_decode_duration_ns),
            ring_write_sequence: Some(metrics.ring_write_sequence),
            ring_read_sequence: Some(metrics.ring_read_sequence),
            ring_occupancy: Some(metrics.ring_occupancy),
            gpu_allocated_bytes: metrics.gpu_allocated_bytes,
            gpu_reserved_bytes: metrics.gpu_reserved_bytes,
            protocol2: None,
        })
    }

    /// Copy exact Protocol 2 counters without inventing legacy ring/GPU data.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidCounter`] if any received
    /// value is outside the diagnostic safety envelope.
    pub fn from_protocol2_metrics_snapshot(
        metrics: &Protocol2MetricsSnapshot,
    ) -> Result<Self, RealtimeDiagnosticError> {
        validate_counters(&[
            metrics.worker_uptime_ns,
            metrics.commands_total,
            metrics.commands_failed_total,
            metrics.player_steps_total,
            metrics.deck_process_total,
            metrics.capture_slots_total,
            metrics.decoded_frames_total,
        ])?;
        Ok(Self {
            worker_uptime_ns: metrics.worker_uptime_ns,
            decode_batches_total: None,
            decoded_frames_total: None,
            ring_backpressure_total: None,
            presentation_skipped_total: None,
            last_decode_duration_ns: None,
            ring_write_sequence: None,
            ring_read_sequence: None,
            ring_occupancy: None,
            gpu_allocated_bytes: None,
            gpu_reserved_bytes: None,
            protocol2: Some(Protocol2WorkerDiagnosticCounters {
                commands: metrics.commands_total,
                commands_failed: metrics.commands_failed_total,
                player_steps: metrics.player_steps_total,
                deck_processes: metrics.deck_process_total,
                capture_slots: metrics.capture_slots_total,
                decoded_frames: metrics.decoded_frames_total,
            }),
        })
    }
}

/// Bounded presentation and output cumulative counters for one session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PresentationDiagnosticCounters {
    frames_presented: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    frames_dropped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spout_frames_sent: Option<u64>,
}

impl PresentationDiagnosticCounters {
    /// Validate and construct presentation counters.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidCounter`] when any counter is
    /// above the immutable diagnostic bound.
    pub fn new(
        frames_presented: u64,
        frames_dropped: Option<u64>,
        spout_frames_sent: Option<u64>,
    ) -> Result<Self, RealtimeDiagnosticError> {
        validate_counters(&[frames_presented])?;
        validate_optional_counters(&[frames_dropped, spout_frames_sent])?;
        Ok(Self {
            frames_presented,
            frames_dropped,
            spout_frames_sent,
        })
    }
}

/// Fixed-size summary of frame intervals or control-to-effect latency.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TimingDistribution {
    sample_count: u64,
    min_ms: f64,
    mean_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

impl TimingDistribution {
    /// Validate and construct a bounded distribution summary.
    ///
    /// A zero-sample summary must contain four exact zero measurements.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidMeasurement`] for non-finite,
    /// negative, inconsistent, or oversized values.
    pub fn new(
        sample_count: u64,
        min_ms: f64,
        mean_ms: f64,
        p95_ms: f64,
        max_ms: f64,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if sample_count > MAX_COUNTER_VALUE
            || ![min_ms, mean_ms, p95_ms, max_ms]
                .into_iter()
                .all(|value| value.is_finite() && (0.0..=MAX_TIMING_MS).contains(&value))
            || (sample_count == 0
                && ![min_ms, mean_ms, p95_ms, max_ms]
                    .into_iter()
                    .all(|value| value.to_bits() == 0.0_f64.to_bits()))
            || (sample_count > 0
                && (min_ms > mean_ms || mean_ms > max_ms || min_ms > p95_ms || p95_ms > max_ms))
        {
            return Err(RealtimeDiagnosticError::InvalidMeasurement);
        }
        Ok(Self {
            sample_count,
            min_ms,
            mean_ms,
            p95_ms,
            max_ms,
        })
    }
}

/// Origin category for a stable, path-free runtime error code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StableErrorSource {
    /// Cartridge validation or compatibility.
    Cartridge,
    /// Codec discovery or decoder selection.
    Codec,
    /// Worker control or compute.
    Worker,
    /// Native presentation or output.
    Presentation,
    /// Typed application control boundary.
    Control,
}

/// One bounded stable error-code transition, never an exception message.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StableErrorRecord {
    timestamp_unix_ms: u64,
    source: StableErrorSource,
    code: SanitizedToken,
}

impl StableErrorRecord {
    /// Construct a stable error record.
    ///
    /// # Errors
    ///
    /// Returns [`RealtimeDiagnosticError::InvalidTimestamp`] for an invalid
    /// Unix-millisecond timestamp.
    pub fn new(
        timestamp_unix_ms: u64,
        source: StableErrorSource,
        code: SanitizedToken,
    ) -> Result<Self, RealtimeDiagnosticError> {
        validate_timestamp(timestamp_unix_ms)?;
        Ok(Self {
            timestamp_unix_ms,
            source,
            code,
        })
    }
}

/// Shared realtime measurements for a Deck or Player session.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RealtimeSessionMetrics {
    duration_ms: u64,
    target_fps: f64,
    measured_fps: f64,
    frame_intervals_ms: TimingDistribution,
    control_latency_ms: TimingDistribution,
    worker: WorkerDiagnosticCounters,
    presentation: PresentationDiagnosticCounters,
    stable_errors: Vec<StableErrorRecord>,
}

impl RealtimeSessionMetrics {
    /// Construct one bounded session measurement block.
    ///
    /// # Errors
    ///
    /// Returns a validation error for invalid duration/FPS or more than 256
    /// stable error-code transitions.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        duration_ms: u64,
        target_fps: f64,
        measured_fps: f64,
        frame_intervals_ms: TimingDistribution,
        control_latency_ms: TimingDistribution,
        worker: WorkerDiagnosticCounters,
        presentation: PresentationDiagnosticCounters,
        stable_errors: Vec<StableErrorRecord>,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if duration_ms > MAX_SESSION_DURATION_MS
            || !target_fps.is_finite()
            || !measured_fps.is_finite()
            || !(f64::EPSILON..=MAX_FPS).contains(&target_fps)
            || !(0.0..=MAX_FPS).contains(&measured_fps)
        {
            return Err(RealtimeDiagnosticError::InvalidMeasurement);
        }
        if stable_errors.len() > MAX_STABLE_ERRORS {
            return Err(RealtimeDiagnosticError::LimitExceeded("stable_errors"));
        }
        Ok(Self {
            duration_ms,
            target_fps,
            measured_fps,
            frame_intervals_ms,
            control_latency_ms,
            worker,
            presentation,
            stable_errors,
        })
    }
}

/// Exact worker and Deck package identity for one Protocol 2 Deck actor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Protocol2DeckSessionIdentity {
    protocol_version: u16,
    worker_session_id: Uuid,
    deck_session_id: Uuid,
    deck_package: SanitizedToken,
    deck_package_version: SanitizedToken,
    operator_version: SanitizedToken,
}

impl Protocol2DeckSessionIdentity {
    /// Construct the path-free identity for one authenticated P2 worker/Deck
    /// session pair.
    ///
    /// # Errors
    ///
    /// Rejects nil session IDs.
    pub fn new(
        worker_session_id: Uuid,
        deck_session_id: Uuid,
        deck_package: SanitizedToken,
        deck_package_version: SanitizedToken,
        operator_version: SanitizedToken,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if worker_session_id.is_nil() || deck_session_id.is_nil() {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        Ok(Self {
            protocol_version: 2,
            worker_session_id,
            deck_session_id,
            deck_package,
            deck_package_version,
            operator_version,
        })
    }
}

/// Codec-neutral Protocol 2 Deck session with one to sixteen exact cartridge
/// archive identities. This is the authoritative diagnostic shape after the
/// installed `.ld` runtime replaces historical hardcoded Deck actors.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GenericDeckDiagnosticSession {
    operator: SanitizedToken,
    cartridge_sha256: Vec<Sha256Token>,
    metrics: RealtimeSessionMetrics,
    protocol2: Protocol2DeckSessionIdentity,
}

impl GenericDeckDiagnosticSession {
    /// Construct one bounded, path-free generic Deck diagnostic section.
    ///
    /// # Errors
    ///
    /// Rejects an empty or over-limit source identity set.
    pub fn new(
        operator: SanitizedToken,
        cartridge_sha256: Vec<Sha256Token>,
        metrics: RealtimeSessionMetrics,
        protocol2: Protocol2DeckSessionIdentity,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if cartridge_sha256.is_empty() || cartridge_sha256.len() > 16 {
            return Err(RealtimeDiagnosticError::LimitExceeded(
                "deck_cartridge_sha256",
            ));
        }
        Ok(Self {
            operator,
            cartridge_sha256,
            metrics,
            protocol2,
        })
    }
}

/// `LatentPlayer` realtime section with one cartridge archive hash.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlayerDiagnosticSession {
    cartridge_sha256: Sha256Token,
    metrics: RealtimeSessionMetrics,
}

impl PlayerDiagnosticSession {
    /// Construct a `LatentPlayer` session section.
    #[must_use]
    pub const fn new(cartridge_sha256: Sha256Token, metrics: RealtimeSessionMetrics) -> Self {
        Self {
            cartridge_sha256,
            metrics,
        }
    }
}

/// Explicit lifecycle-only state when no realtime actor is active at capture.
///
/// This form never invents performance counters. When a prior actor failed,
/// `last_error_code` preserves the stable failure identity without exception
/// text or a machine path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct InactiveApplicationDiagnosticSession {
    no_active_session: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error_code: Option<SanitizedToken>,
}

impl InactiveApplicationDiagnosticSession {
    /// Construct the only valid inactive-session marker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            no_active_session: true,
            last_error_code: None,
        }
    }

    /// Attach the stable final runtime error to a lifecycle-only snapshot.
    #[must_use]
    pub const fn with_last_error(last_error_code: SanitizedToken) -> Self {
        Self {
            no_active_session: true,
            last_error_code: Some(last_error_code),
        }
    }
}

impl Default for InactiveApplicationDiagnosticSession {
    fn default() -> Self {
        Self::new()
    }
}

/// One of the supported application session kinds.
#[derive(Clone, Debug, PartialEq)]
pub enum RealtimeDiagnosticSession {
    /// No active actor at capture; an optional stable final error distinguishes
    /// startup/missing-codec state from an ended failed session.
    NoActiveSession(InactiveApplicationDiagnosticSession),
    /// One exact installed `.ld` Deck running through Protocol 2.
    Deck(GenericDeckDiagnosticSession),
    /// `LatentPlayer` playback session.
    Player(PlayerDiagnosticSession),
}

/// Complete bounded snapshot. At least one truthful session section is required.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RealtimeDiagnosticSnapshot {
    schema_version: u16,
    captured_at_unix_ms: u64,
    product: DiagnosticProductIdentity,
    gpu: DiagnosticGpuIdentity,
    codec: DiagnosticCodecIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    inactive_application: Option<InactiveApplicationDiagnosticSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deck: Option<GenericDeckDiagnosticSession>,
    #[serde(skip_serializing_if = "Option::is_none")]
    player: Option<PlayerDiagnosticSession>,
}

impl RealtimeDiagnosticSnapshot {
    /// Construct a snapshot with its required first session section.
    ///
    /// # Errors
    ///
    /// Rejects an invalid timestamp or a Deck/Player product mismatch.
    pub fn new(
        captured_at_unix_ms: u64,
        product: DiagnosticProductIdentity,
        gpu: DiagnosticGpuIdentity,
        codec: DiagnosticCodecIdentity,
        primary_session: RealtimeDiagnosticSession,
    ) -> Result<Self, RealtimeDiagnosticError> {
        validate_timestamp(captured_at_unix_ms)?;
        let mut snapshot = Self {
            schema_version: REALTIME_DIAGNOSTIC_SCHEMA_VERSION,
            captured_at_unix_ms,
            product,
            gpu,
            codec,
            inactive_application: None,
            deck: None,
            player: None,
        };
        snapshot.insert_session(primary_session)?;
        Ok(snapshot)
    }

    /// Add another compatible session section while preserving the required section.
    ///
    /// # Errors
    ///
    /// Rejects product mismatches, duplicate kinds, and inactive/realtime mixes.
    pub fn with_session(
        mut self,
        session: RealtimeDiagnosticSession,
    ) -> Result<Self, RealtimeDiagnosticError> {
        self.insert_session(session)?;
        Ok(self)
    }

    fn insert_session(
        &mut self,
        session: RealtimeDiagnosticSession,
    ) -> Result<(), RealtimeDiagnosticError> {
        match session {
            RealtimeDiagnosticSession::NoActiveSession(value) => {
                if self.deck.is_some() || self.player.is_some() {
                    return Err(RealtimeDiagnosticError::ConflictingSessionState);
                }
                if self.inactive_application.replace(value).is_some() {
                    return Err(RealtimeDiagnosticError::DuplicateSession);
                }
            }
            RealtimeDiagnosticSession::Deck(value) => {
                if self.product.product() != DiagnosticProduct::LatentDeck {
                    return Err(RealtimeDiagnosticError::ProductSessionMismatch);
                }
                if self.inactive_application.is_some() {
                    return Err(RealtimeDiagnosticError::ConflictingSessionState);
                }
                if self.deck.replace(value).is_some() {
                    return Err(RealtimeDiagnosticError::DuplicateSession);
                }
            }
            RealtimeDiagnosticSession::Player(value) => {
                if self.product.product() != DiagnosticProduct::LatentPlayer {
                    return Err(RealtimeDiagnosticError::ProductSessionMismatch);
                }
                if self.inactive_application.is_some() {
                    return Err(RealtimeDiagnosticError::ConflictingSessionState);
                }
                if self.player.replace(value).is_some() {
                    return Err(RealtimeDiagnosticError::DuplicateSession);
                }
            }
        }
        Ok(())
    }
}

/// Closed lifecycle source accepted by the collector and archive.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventSource {
    /// `LatentDeck` application log.
    Deck,
    /// `LatentPlayer` application log.
    Player,
    /// Isolated codec worker log.
    Worker,
}

impl DiagnosticEventSource {
    const fn file_prefix(self) -> &'static str {
        match self {
            Self::Deck => "latentdeck-",
            Self::Player => "latentplayer-",
            Self::Worker => "worker-",
        }
    }
}

/// Closed lifecycle event severity accepted by the bundle writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventLevel {
    /// Normal lifecycle evidence.
    Info,
    /// Recoverable degraded state.
    Warn,
    /// Stable failure transition.
    Error,
}

/// Already-sanitized lifecycle record accepted instead of arbitrary log text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticEventRecord {
    schema_version: u16,
    timestamp_unix_ms: u64,
    source: DiagnosticEventSource,
    level: DiagnosticEventLevel,
    event: SanitizedToken,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<SanitizedToken>,
}

impl DiagnosticEventRecord {
    /// Construct an allowlisted JSONL lifecycle record.
    ///
    /// # Errors
    ///
    /// Rejects invalid timestamps or event/code tokens outside the lifecycle
    /// grammar. No arbitrary message or exception field exists.
    pub fn new(
        timestamp_unix_ms: u64,
        source: DiagnosticEventSource,
        level: DiagnosticEventLevel,
        event: SanitizedToken,
        code: Option<SanitizedToken>,
    ) -> Result<Self, RealtimeDiagnosticError> {
        validate_timestamp(timestamp_unix_ms)?;
        if !is_safe_token(event.as_str(), MAX_EVENT_NAME_BYTES, false)
            || code
                .as_ref()
                .is_some_and(|value| !is_safe_token(value.as_str(), MAX_TOKEN_BYTES, false))
        {
            return Err(RealtimeDiagnosticError::InvalidToken);
        }
        Ok(Self {
            schema_version: REALTIME_DIAGNOSTIC_SCHEMA_VERSION,
            timestamp_unix_ms,
            source,
            level,
            event,
            code,
        })
    }

    /// Parse one strict application JSONL record into the allowlisted form.
    ///
    /// # Errors
    ///
    /// Rejects worker source, oversized input, unknown schemas, invalid levels,
    /// arbitrary required tokens, and malformed JSON. Unknown fields and an
    /// unsafe optional code are ignored and never copied to the result.
    pub fn from_application_json_line(
        source: DiagnosticEventSource,
        line: &[u8],
    ) -> Result<Self, RealtimeDiagnosticError> {
        if source == DiagnosticEventSource::Worker || line.len() > MAX_EVENT_RECORD_BYTES {
            return Err(RealtimeDiagnosticError::InvalidLifecycleRecord);
        }
        let raw: RawApplicationEvent = serde_json::from_slice(line)
            .map_err(|_| RealtimeDiagnosticError::InvalidLifecycleRecord)?;
        if raw.schema_version != REALTIME_DIAGNOSTIC_SCHEMA_VERSION {
            return Err(RealtimeDiagnosticError::InvalidLifecycleRecord);
        }
        let level = match raw.level.as_str() {
            "info" => DiagnosticEventLevel::Info,
            "warn" => DiagnosticEventLevel::Warn,
            "error" => DiagnosticEventLevel::Error,
            _ => return Err(RealtimeDiagnosticError::InvalidLifecycleRecord),
        };
        let event = parse_lifecycle_token(&raw.event, MAX_EVENT_NAME_BYTES)?;
        let code = raw
            .code
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .and_then(|value| parse_lifecycle_token(value, MAX_TOKEN_BYTES).ok());
        Self::new(raw.timestamp_unix_ms, source, level, event, code)
            .map_err(|_| RealtimeDiagnosticError::InvalidLifecycleRecord)
    }

    /// Parse one strict worker JSONL record into the allowlisted form.
    ///
    /// Worker nanoseconds are reduced to Unix milliseconds. Only the first
    /// valid `code`, `cause_code`, or `error_type` token is retained.
    ///
    /// # Errors
    ///
    /// Rejects oversized input, unknown schemas, arbitrary required tokens, and
    /// malformed JSON. Unknown fields and unsafe optional code candidates are
    /// ignored and never copied to the result.
    pub fn from_worker_json_line(line: &[u8]) -> Result<Self, RealtimeDiagnosticError> {
        if line.len() > MAX_EVENT_RECORD_BYTES {
            return Err(RealtimeDiagnosticError::InvalidLifecycleRecord);
        }
        let raw: RawWorkerEvent = serde_json::from_slice(line)
            .map_err(|_| RealtimeDiagnosticError::InvalidLifecycleRecord)?;
        if raw.schema_version != REALTIME_DIAGNOSTIC_SCHEMA_VERSION {
            return Err(RealtimeDiagnosticError::InvalidLifecycleRecord);
        }
        let timestamp_unix_ms = raw.timestamp_ns / 1_000_000;
        let event = parse_lifecycle_token(&raw.event, MAX_EVENT_NAME_BYTES)?;
        let level = if worker_event_is_error(&raw.event) {
            DiagnosticEventLevel::Error
        } else {
            DiagnosticEventLevel::Info
        };
        let code = [raw.code, raw.cause_code, raw.error_type]
            .iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .find_map(|value| parse_lifecycle_token(value, MAX_TOKEN_BYTES).ok());
        Self::new(
            timestamp_unix_ms,
            DiagnosticEventSource::Worker,
            level,
            event,
            code,
        )
        .map_err(|_| RealtimeDiagnosticError::InvalidLifecycleRecord)
    }
}

#[derive(Deserialize)]
struct RawApplicationEvent {
    schema_version: u16,
    timestamp_unix_ms: u64,
    level: String,
    event: String,
    code: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct RawWorkerEvent {
    schema_version: u16,
    timestamp_ns: u64,
    event: String,
    code: Option<serde_json::Value>,
    cause_code: Option<serde_json::Value>,
    error_type: Option<serde_json::Value>,
}

/// One closed top-level log root. Filename matching is fixed by `source`.
#[derive(Clone, Copy, Debug)]
pub struct DiagnosticLogSource<'a> {
    source: DiagnosticEventSource,
    root: &'a Path,
}

impl<'a> DiagnosticLogSource<'a> {
    /// Create a non-recursive source root with a fixed safe filename prefix.
    #[must_use]
    pub const fn new(source: DiagnosticEventSource, root: &'a Path) -> Self {
        Self { source, root }
    }
}

/// Immutable limits for installed log collection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticCollectionLimits {
    input_files: usize,
    file_bytes: u64,
    input_bytes: u64,
    events: usize,
}

impl DiagnosticCollectionLimits {
    /// Validate custom limits, primarily for isolated hosts and tests.
    ///
    /// # Errors
    ///
    /// All values must be non-zero and remain below hard safety ceilings.
    pub fn new(
        max_input_files: usize,
        max_file_bytes: u64,
        max_input_bytes: u64,
        max_events: usize,
    ) -> Result<Self, RealtimeDiagnosticError> {
        if max_input_files == 0
            || max_input_files > HARD_MAX_INPUT_FILES
            || max_file_bytes == 0
            || max_file_bytes > HARD_MAX_FILE_BYTES
            || max_input_bytes == 0
            || max_input_bytes > HARD_MAX_INPUT_BYTES
            || max_events == 0
            || max_events > HARD_MAX_EVENTS
        {
            return Err(RealtimeDiagnosticError::InvalidCollectionLimits);
        }
        Ok(Self {
            input_files: max_input_files,
            file_bytes: max_file_bytes,
            input_bytes: max_input_bytes,
            events: max_events,
        })
    }
}

impl Default for DiagnosticCollectionLimits {
    fn default() -> Self {
        Self {
            input_files: DEFAULT_MAX_INPUT_FILES,
            file_bytes: DEFAULT_MAX_FILE_BYTES,
            input_bytes: DEFAULT_MAX_INPUT_BYTES,
            events: MAX_DIAGNOSTIC_EVENTS,
        }
    }
}

/// Path-free accounting for a bounded log collection pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DiagnosticCollectionReport {
    /// Accepted allowlisted records.
    pub accepted_event_count: usize,
    /// Invalid or over-limit source records.
    pub dropped_record_count: u64,
    /// Files opened within both byte budgets.
    pub processed_file_count: usize,
    /// Reparse, non-file, oversized, unreadable, or over-budget files.
    pub skipped_file_count: u64,
    /// Total bytes read from accepted bounded files.
    pub input_byte_count: u64,
    /// Closed source kinds represented by at least one accepted event.
    pub included_sources: Vec<DiagnosticEventSource>,
}

/// Typed events and path-free report returned by the installed collector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticEventCollection {
    events: Vec<DiagnosticEventRecord>,
    report: DiagnosticCollectionReport,
}

impl DiagnosticEventCollection {
    /// Borrow accepted typed event records.
    #[must_use]
    pub fn events(&self) -> &[DiagnosticEventRecord] {
        &self.events
    }

    /// Borrow path-free collection accounting.
    #[must_use]
    pub const fn report(&self) -> &DiagnosticCollectionReport {
        &self.report
    }
}

/// Collect the newest bounded lifecycle files without recursion or raw copying.
///
/// Missing roots are valid. Roots and candidates that are symlinks/reparse
/// points are rejected. Only fixed `latentdeck-*`, `latentplayer-*`, and
/// `worker-*` JSONL basenames are considered. Every line is decoded through a
/// closed schema and reserialized later from [`DiagnosticEventRecord`].
///
/// # Errors
///
/// Collection is best effort; only an internal accounting overflow is fatal.
pub fn collect_diagnostic_events(
    sources: &[DiagnosticLogSource<'_>],
    limits: DiagnosticCollectionLimits,
) -> Result<DiagnosticEventCollection, RealtimeDiagnosticError> {
    let (candidates, pre_skipped) = discover_log_candidates(sources, limits.input_files);

    let mut events = Vec::new();
    let mut report = DiagnosticCollectionReport {
        accepted_event_count: 0,
        dropped_record_count: 0,
        processed_file_count: 0,
        skipped_file_count: pre_skipped,
        input_byte_count: 0,
        included_sources: Vec::new(),
    };
    for candidate in candidates {
        let remaining = limits.input_bytes.saturating_sub(report.input_byte_count);
        if remaining == 0 {
            report.skipped_file_count = report.skipped_file_count.saturating_add(1);
            continue;
        }
        let byte_limit = remaining.min(limits.file_bytes);
        let Some(bytes) = read_bounded_regular_file(&candidate.path, byte_limit) else {
            report.skipped_file_count = report.skipped_file_count.saturating_add(1);
            continue;
        };
        report.processed_file_count = report.processed_file_count.saturating_add(1);
        report.input_byte_count =
            report
                .input_byte_count
                .checked_add(u64::try_from(bytes.len()).map_err(|_| {
                    RealtimeDiagnosticError::LimitExceeded("collection_input_bytes")
                })?)
                .ok_or(RealtimeDiagnosticError::LimitExceeded(
                    "collection_input_bytes",
                ))?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            report.dropped_record_count = report.dropped_record_count.saturating_add(1);
            continue;
        };
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            if line.len() > MAX_EVENT_RECORD_BYTES || events.len() >= limits.events {
                report.dropped_record_count = report.dropped_record_count.saturating_add(1);
                continue;
            }
            let parsed = match candidate.source {
                DiagnosticEventSource::Worker => {
                    DiagnosticEventRecord::from_worker_json_line(line.as_bytes())
                }
                source => {
                    DiagnosticEventRecord::from_application_json_line(source, line.as_bytes())
                }
            };
            match parsed {
                Ok(event) => {
                    events.push(event);
                    if !report.included_sources.contains(&candidate.source) {
                        report.included_sources.push(candidate.source);
                    }
                }
                Err(_) => {
                    report.dropped_record_count = report.dropped_record_count.saturating_add(1);
                }
            }
        }
    }
    report.included_sources.sort_unstable();
    report.accepted_event_count = events.len();
    Ok(DiagnosticEventCollection { events, report })
}

struct LogCandidate {
    source: DiagnosticEventSource,
    path: PathBuf,
    name: String,
    modified: SystemTime,
}

fn discover_log_candidates(
    sources: &[DiagnosticLogSource<'_>],
    input_file_limit: usize,
) -> (Vec<LogCandidate>, u64) {
    let enumeration_limit = input_file_limit
        .saturating_mul(4)
        .clamp(input_file_limit, MAX_ENUMERATED_FILES);
    let mut candidates = Vec::new();
    let mut skipped = 0_u64;
    for source in sources {
        append_source_candidates(*source, enumeration_limit, &mut candidates, &mut skipped);
    }
    candidates.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| right.name.cmp(&left.name))
    });
    candidates.truncate(input_file_limit);
    (candidates, skipped)
}

fn append_source_candidates(
    source: DiagnosticLogSource<'_>,
    enumeration_limit: usize,
    candidates: &mut Vec<LogCandidate>,
    skipped: &mut u64,
) {
    let Ok(root_metadata) = fs::symlink_metadata(source.root) else {
        return;
    };
    if !root_metadata.is_dir() || metadata_is_reparse(&root_metadata) {
        *skipped = skipped.saturating_add(1);
        return;
    }
    let Ok(entries) = fs::read_dir(source.root) else {
        *skipped = skipped.saturating_add(1);
        return;
    };
    for entry in entries.take(enumeration_limit) {
        let Ok(entry) = entry else {
            *skipped = skipped.saturating_add(1);
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            *skipped = skipped.saturating_add(1);
            continue;
        };
        let is_jsonl = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"));
        if !name.starts_with(source.source.file_prefix())
            || !is_jsonl
            || name.contains('/')
            || name.contains('\\')
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            *skipped = skipped.saturating_add(1);
            continue;
        };
        if !metadata.is_file() || metadata_is_reparse(&metadata) {
            *skipped = skipped.saturating_add(1);
            continue;
        }
        candidates.push(LogCandidate {
            source: source.source,
            path: entry.path(),
            name: name.to_owned(),
            modified: metadata.modified().unwrap_or(UNIX_EPOCH),
        });
    }
}

/// Explicit input boundary for a realtime bundle.
///
/// Lifecycle evidence is accepted only as typed, already-sanitized records;
/// there is intentionally no API that accepts arbitrary exception or log text.
#[derive(Clone, Copy, Debug)]
pub struct DiagnosticBundleInput<'a> {
    snapshot: &'a RealtimeDiagnosticSnapshot,
    events: &'a [DiagnosticEventRecord],
    collection_report: Option<&'a DiagnosticCollectionReport>,
}

impl<'a> DiagnosticBundleInput<'a> {
    /// Borrow a closed snapshot and manually assembled allowlisted events.
    #[must_use]
    pub const fn new(
        snapshot: &'a RealtimeDiagnosticSnapshot,
        events: &'a [DiagnosticEventRecord],
    ) -> Self {
        Self {
            snapshot,
            events,
            collection_report: None,
        }
    }

    /// Borrow a snapshot and a bounded installed log collection.
    #[must_use]
    pub fn from_collection(
        snapshot: &'a RealtimeDiagnosticSnapshot,
        collection: &'a DiagnosticEventCollection,
    ) -> Self {
        Self {
            snapshot,
            events: &collection.events,
            collection_report: Some(&collection.report),
        }
    }
}

/// Path-free publication result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticBundleReceipt {
    /// Final archive byte length.
    pub archive_bytes: u64,
    /// Number of allowlisted events included.
    pub event_count: usize,
    /// Snapshot and bundle schema written.
    pub schema_version: u16,
}

#[derive(Serialize)]
struct BundleManifest<'a> {
    schema_version: u16,
    bundle_format: &'static str,
    created_at_unix_ms: u64,
    product: DiagnosticProduct,
    product_version: &'a SanitizedToken,
    app_version: &'a SanitizedToken,
    runtime: &'a SanitizedToken,
    runtime_version: &'a SanitizedToken,
    accepted_event_count: usize,
    dropped_record_count: u64,
    processed_file_count: usize,
    skipped_file_count: u64,
    input_byte_count: u64,
    included_sources: Vec<DiagnosticEventSource>,
    entries: [&'static str; 3],
    privacy_exclusions: [&'static str; 12],
}

struct EncodedBundle {
    manifest: Vec<u8>,
    events: Vec<u8>,
    realtime: Vec<u8>,
}

impl EncodedBundle {
    fn entries(&self) -> [ZipEntry<'_>; 3] {
        [
            ZipEntry::new(ENTRY_NAMES[0], &self.manifest),
            ZipEntry::new(ENTRY_NAMES[1], &self.events),
            ZipEntry::new(ENTRY_NAMES[2], &self.realtime),
        ]
    }
}

/// Validate, encode, and atomically publish a no-clobber diagnostic ZIP.
///
/// A `create_new` temporary file is placed beside the destination, flushed and
/// synced, then published with an atomic same-directory hard link. The link
/// cannot replace an existing destination. The temporary file is removed on
/// success and on every recoverable error path.
///
/// # Errors
///
/// Returns a closed validation, encoding, collision, randomness, or I/O error.
pub fn write_diagnostic_bundle_atomic(
    destination: &Path,
    input: DiagnosticBundleInput<'_>,
) -> Result<DiagnosticBundleReceipt, RealtimeDiagnosticError> {
    let encoded = encode_bundle(input)?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or(RealtimeDiagnosticError::InvalidDestination)?;
    if file_name.is_empty() || file_name == "." || file_name == ".." {
        return Err(RealtimeDiagnosticError::InvalidDestination);
    }
    let metadata = fs::metadata(parent).map_err(|source| io_error("inspect_parent", source))?;
    if !metadata.is_dir() {
        return Err(RealtimeDiagnosticError::InvalidDestination);
    }

    let (mut output, temporary_path) = create_temporary(parent)?;
    let mut guard = TemporaryGuard::new(temporary_path.clone());
    write_stored_zip(&mut output, &encoded.entries())?;
    output
        .flush()
        .map_err(|source| io_error("flush_temporary", source))?;
    output
        .sync_all()
        .map_err(|source| io_error("sync_temporary", source))?;
    let archive_bytes = output
        .stream_position()
        .map_err(|source| io_error("measure_temporary", source))?;
    drop(output);

    match fs::hard_link(&temporary_path, destination) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(RealtimeDiagnosticError::OutputExists);
        }
        Err(source) => return Err(io_error("publish_bundle", source)),
    }
    fs::remove_file(&temporary_path).map_err(|source| {
        let _ = fs::remove_file(destination);
        io_error("cleanup_temporary", source)
    })?;
    guard.disarm();
    Ok(DiagnosticBundleReceipt {
        archive_bytes,
        event_count: input.events.len(),
        schema_version: REALTIME_DIAGNOSTIC_SCHEMA_VERSION,
    })
}

fn encode_bundle(
    input: DiagnosticBundleInput<'_>,
) -> Result<EncodedBundle, RealtimeDiagnosticError> {
    if input.events.len() > MAX_DIAGNOSTIC_EVENTS {
        return Err(RealtimeDiagnosticError::LimitExceeded("events"));
    }
    let realtime = serde_json::to_vec(input.snapshot)?;
    if realtime.len() > MAX_REALTIME_BYTES {
        return Err(RealtimeDiagnosticError::LimitExceeded("realtime.json"));
    }

    let mut events = Vec::new();
    for event in input.events {
        let mut encoded = serde_json::to_vec(event)?;
        encoded.push(b'\n');
        if encoded.len() > MAX_EVENT_RECORD_BYTES {
            return Err(RealtimeDiagnosticError::LimitExceeded("event_record"));
        }
        let next = events
            .len()
            .checked_add(encoded.len())
            .ok_or(RealtimeDiagnosticError::LimitExceeded("events.jsonl"))?;
        if next > MAX_EVENTS_BYTES {
            return Err(RealtimeDiagnosticError::LimitExceeded("events.jsonl"));
        }
        events.extend_from_slice(&encoded);
    }

    let identity = &input.snapshot.product;
    let (dropped, processed, skipped, input_bytes, included_sources) =
        input.collection_report.map_or_else(
            || {
                let mut sources = input
                    .events
                    .iter()
                    .map(|event| event.source)
                    .collect::<Vec<_>>();
                sources.sort_unstable();
                sources.dedup();
                (0, 0, 0, 0, sources)
            },
            |report| {
                (
                    report.dropped_record_count,
                    report.processed_file_count,
                    report.skipped_file_count,
                    report.input_byte_count,
                    report.included_sources.clone(),
                )
            },
        );
    let manifest = BundleManifest {
        schema_version: REALTIME_DIAGNOSTIC_SCHEMA_VERSION,
        bundle_format: "latentdeck.realtime-diagnostic-bundle",
        created_at_unix_ms: input.snapshot.captured_at_unix_ms,
        product: identity.product,
        product_version: &identity.app_version,
        app_version: &identity.app_version,
        runtime: &identity.runtime,
        runtime_version: &identity.runtime_version,
        accepted_event_count: input.events.len(),
        dropped_record_count: dropped,
        processed_file_count: processed,
        skipped_file_count: skipped,
        input_byte_count: input_bytes,
        included_sources,
        entries: ENTRY_NAMES,
        privacy_exclusions: [
            "absolute_paths",
            "library_database",
            "cartridge_payloads",
            "latent_tensors",
            "model_weights",
            "decoder_assets",
            "user_media",
            "prompts",
            "environment_variables",
            "credentials",
            "process_identifiers",
            "arbitrary_exception_text",
        ],
    };
    let manifest = serde_json::to_vec(&manifest)?;
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err(RealtimeDiagnosticError::LimitExceeded("manifest.json"));
    }

    Ok(EncodedBundle {
        manifest,
        events,
        realtime,
    })
}

struct ZipEntry<'a> {
    name: &'static str,
    bytes: &'a [u8],
    crc32: u32,
}

impl<'a> ZipEntry<'a> {
    fn new(name: &'static str, bytes: &'a [u8]) -> Self {
        Self {
            name,
            bytes,
            crc32: crc32fast::hash(bytes),
        }
    }
}

struct CentralEntry {
    name: &'static str,
    size: u32,
    crc32: u32,
    local_offset: u32,
}

fn write_stored_zip(
    output: &mut File,
    entries: &[ZipEntry<'_>; 3],
) -> Result<(), RealtimeDiagnosticError> {
    let mut central = Vec::with_capacity(entries.len());
    for entry in entries {
        let local_offset = checked_u32(
            output
                .stream_position()
                .map_err(|source| io_error("measure_local_entry", source))?,
            "archive_offset",
        )?;
        let size = checked_u32_usize(entry.bytes.len(), "archive_entry")?;
        let name_length = checked_u16(entry.name.len(), "entry_name")?;
        write_u32(output, ZIP_LOCAL_SIGNATURE)?;
        write_u16(output, ZIP_VERSION)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u16(output, ZIP_DOS_TIME)?;
        write_u16(output, ZIP_DOS_DATE)?;
        write_u32(output, entry.crc32)?;
        write_u32(output, size)?;
        write_u32(output, size)?;
        write_u16(output, name_length)?;
        write_u16(output, 0)?;
        write_bytes(output, entry.name.as_bytes())?;
        write_bytes(output, entry.bytes)?;
        central.push(CentralEntry {
            name: entry.name,
            size,
            crc32: entry.crc32,
            local_offset,
        });
    }

    let central_offset = checked_u32(
        output
            .stream_position()
            .map_err(|source| io_error("measure_central_directory", source))?,
        "archive_offset",
    )?;
    for entry in &central {
        let name_length = checked_u16(entry.name.len(), "entry_name")?;
        write_u32(output, ZIP_CENTRAL_SIGNATURE)?;
        write_u16(output, ZIP_VERSION_MADE_BY)?;
        write_u16(output, ZIP_VERSION)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u16(output, ZIP_DOS_TIME)?;
        write_u16(output, ZIP_DOS_DATE)?;
        write_u32(output, entry.crc32)?;
        write_u32(output, entry.size)?;
        write_u32(output, entry.size)?;
        write_u16(output, name_length)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u16(output, 0)?;
        write_u32(output, 0)?;
        write_u32(output, entry.local_offset)?;
        write_bytes(output, entry.name.as_bytes())?;
    }
    let central_end = checked_u32(
        output
            .stream_position()
            .map_err(|source| io_error("measure_central_directory", source))?,
        "archive_offset",
    )?;
    let central_size = central_end
        .checked_sub(central_offset)
        .ok_or(RealtimeDiagnosticError::LimitExceeded("central_directory"))?;
    let entry_count = u16::try_from(central.len())
        .map_err(|_| RealtimeDiagnosticError::LimitExceeded("entry_count"))?;
    write_u32(output, ZIP_END_SIGNATURE)?;
    write_u16(output, 0)?;
    write_u16(output, 0)?;
    write_u16(output, entry_count)?;
    write_u16(output, entry_count)?;
    write_u32(output, central_size)?;
    write_u32(output, central_offset)?;
    write_u16(output, 0)
}

fn create_temporary(parent: &Path) -> Result<(File, PathBuf), RealtimeDiagnosticError> {
    for _ in 0..16 {
        let mut random = [0_u8; 12];
        getrandom::fill(&mut random).map_err(|_| RealtimeDiagnosticError::Random)?;
        let path = parent.join(format!(
            ".latentdeck-diagnostic-{}.partial",
            hex::encode(random)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(source) => return Err(io_error("create_temporary", source)),
        }
    }
    Err(RealtimeDiagnosticError::Io {
        stage: "create_temporary",
        source: std::io::Error::new(std::io::ErrorKind::AlreadyExists, "collision budget"),
    })
}

struct TemporaryGuard {
    path: PathBuf,
    armed: bool,
}

impl TemporaryGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn parse_lifecycle_token(
    value: &str,
    maximum: usize,
) -> Result<SanitizedToken, RealtimeDiagnosticError> {
    if !is_safe_token(value, maximum, false) {
        return Err(RealtimeDiagnosticError::InvalidLifecycleRecord);
    }
    SanitizedToken::parse(value).map_err(|_| RealtimeDiagnosticError::InvalidLifecycleRecord)
}

fn worker_event_is_error(event: &str) -> bool {
    event.ends_with("_failed")
        || event
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix == "error")
}

fn is_safe_token(value: &str, maximum: usize, allow_plus: bool) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.is_ascii()
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'-')
                || (allow_plus && byte == b'+')
        })
}

fn looks_sensitive_or_path_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.contains('/')
        || value.contains('\\')
        || value.contains(':')
        || value.contains('$')
        || value.contains('%')
        || value.contains('@')
        || value.contains('\n')
        || value.contains('\r')
        || value.contains("..")
        || lower.contains("password=")
        || lower.contains("secret=")
        || lower.contains("token=")
        || lower.contains("api_key=")
        || lower.contains("apikey=")
        || lower.contains("bearer ")
}

fn validate_timestamp(value: u64) -> Result<(), RealtimeDiagnosticError> {
    if (1..=MAX_UNIX_TIMESTAMP_MS).contains(&value) {
        Ok(())
    } else {
        Err(RealtimeDiagnosticError::InvalidTimestamp)
    }
}

fn validate_counters(values: &[u64]) -> Result<(), RealtimeDiagnosticError> {
    if values.iter().all(|value| *value <= MAX_COUNTER_VALUE) {
        Ok(())
    } else {
        Err(RealtimeDiagnosticError::InvalidCounter)
    }
}

fn validate_optional_counters(values: &[Option<u64>]) -> Result<(), RealtimeDiagnosticError> {
    if values
        .iter()
        .flatten()
        .all(|value| *value <= MAX_COUNTER_VALUE)
    {
        Ok(())
    } else {
        Err(RealtimeDiagnosticError::InvalidCounter)
    }
}

fn read_bounded_regular_file(path: &Path, byte_limit: u64) -> Option<Vec<u8>> {
    if byte_limit == 0 {
        return None;
    }
    let before = fs::symlink_metadata(path).ok()?;
    if !before.is_file() || metadata_is_reparse(&before) || before.len() > byte_limit {
        return None;
    }
    let file = File::open(path).ok()?;
    let after = file.metadata().ok()?;
    if !after.is_file() || after.len() > byte_limit {
        return None;
    }
    let capacity = usize::try_from(byte_limit.min(HARD_MAX_FILE_BYTES)).ok()?;
    let mut bytes = Vec::with_capacity(capacity.min(64 * 1024));
    file.take(byte_limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > byte_limit {
        None
    } else {
        Some(bytes)
    }
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn checked_u16(value: usize, label: &'static str) -> Result<u16, RealtimeDiagnosticError> {
    u16::try_from(value).map_err(|_| RealtimeDiagnosticError::LimitExceeded(label))
}

fn checked_u32(value: u64, label: &'static str) -> Result<u32, RealtimeDiagnosticError> {
    u32::try_from(value).map_err(|_| RealtimeDiagnosticError::LimitExceeded(label))
}

fn checked_u32_usize(value: usize, label: &'static str) -> Result<u32, RealtimeDiagnosticError> {
    u32::try_from(value).map_err(|_| RealtimeDiagnosticError::LimitExceeded(label))
}

fn write_u16(output: &mut File, value: u16) -> Result<(), RealtimeDiagnosticError> {
    write_bytes(output, &value.to_le_bytes())
}

fn write_u32(output: &mut File, value: u32) -> Result<(), RealtimeDiagnosticError> {
    write_bytes(output, &value.to_le_bytes())
}

fn write_bytes(output: &mut File, bytes: &[u8]) -> Result<(), RealtimeDiagnosticError> {
    output
        .write_all(bytes)
        .map_err(|source| io_error("write_archive", source))
}

fn io_error(stage: &'static str, source: std::io::Error) -> RealtimeDiagnosticError {
    RealtimeDiagnosticError::Io { stage, source }
}
