use std::{
    fs,
    io::{self, Write},
    path::Path,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const RELEASE_DURATION: Duration = Duration::from_secs(1_800);
pub const FRAME_RATE_NUMERATOR: u64 = 24;
pub const FRAME_RATE_DENOMINATOR: u64 = 1;
pub const EXECUTION_CONTEXT_KIND: &str = "latentdeck_private_realtime_soak_execution_context";
pub const MIN_RELEASE_RESOURCE_SAMPLES: usize = 16;
pub const FPS_MINIMUM: f64 = 23.9;
pub const FPS_MAXIMUM: f64 = 24.1;
pub const LONG_INTERVAL_RATE_LIMIT: f64 = 0.005;
pub const CONTROL_PROCESSED_FRAME_P95_LIMIT: Duration = Duration::from_millis(200);
pub const MEMORY_ABSOLUTE_GROWTH_THRESHOLD: u64 = 64 * 1024 * 1024;

const EXECUTION_CONTEXT_SCHEMA_VERSION: u32 = 2;
const MAX_EXECUTION_CONTEXT_BYTES: u64 = 64 * 1024;
const MAX_RELEASE_SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub sha256: String,
    pub byte_length: u64,
}

impl FileIdentity {
    pub fn validate(&self, label: &str) -> io::Result<()> {
        if self.byte_length == 0 {
            return Err(io::Error::other(format!(
                "{label} byte length must be nonzero"
            )));
        }
        if !is_canonical_sha256(&self.sha256) {
            return Err(io::Error::other(format!(
                "{label} SHA-256 must be canonical lowercase hex"
            )));
        }
        Ok(())
    }

    pub fn matches(&self, sha256: &str, byte_length: u64) -> bool {
        self.sha256 == sha256 && self.byte_length == byte_length
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryIdentity {
    pub git_commit: String,
    pub tracked_tree_clean: bool,
    pub nonignored_untracked_clean: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFileIdentities {
    pub a: FileIdentity,
    pub b: FileIdentity,
    pub c: FileIdentity,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MeasurementIdentity {
    pub duration_seconds: u64,
    pub warmup_seconds: u64,
    pub control_interval_seconds: u64,
    pub resource_interval_seconds: u64,
    pub frame_rate_numerator: u64,
    pub frame_rate_denominator: u64,
}

impl MeasurementIdentity {
    pub fn validate(&self, release_duration: bool) -> io::Result<()> {
        if self.duration_seconds == 0
            || self.warmup_seconds >= self.duration_seconds
            || self.control_interval_seconds == 0
            || self.resource_interval_seconds == 0
            || self.frame_rate_numerator == 0
            || self.frame_rate_denominator == 0
        {
            return Err(io::Error::other(
                "execution context contains an invalid measurement schedule",
            ));
        }
        if self.frame_rate_numerator != FRAME_RATE_NUMERATOR
            || self.frame_rate_denominator != FRAME_RATE_DENOMINATOR
        {
            return Err(io::Error::other(
                "execution context frame rate differs from the release contract",
            ));
        }
        if release_duration
            && (self.resource_interval_seconds > MAX_RELEASE_SAMPLE_INTERVAL.as_secs()
                || self.control_interval_seconds > MAX_RELEASE_SAMPLE_INTERVAL.as_secs()
                || self.warmup_seconds > self.duration_seconds / 3)
        {
            return Err(io::Error::other(
                "release-duration measurement schedule is too sparse",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionContext {
    pub schema_version: u32,
    pub evidence_kind: String,
    pub repository: RepositoryIdentity,
    pub measurement: MeasurementIdentity,
    pub cargo_lock: FileIdentity,
    pub test_binary: FileIdentity,
    pub decoder: FileIdentity,
    pub sources: SourceFileIdentities,
}

impl ExecutionContext {
    pub fn validate(&self, release_duration: bool) -> io::Result<()> {
        if self.schema_version != EXECUTION_CONTEXT_SCHEMA_VERSION
            || self.evidence_kind != EXECUTION_CONTEXT_KIND
        {
            return Err(io::Error::other(
                "unsupported realtime-soak execution context",
            ));
        }
        if !is_lower_hex(&self.repository.git_commit, 40) {
            return Err(io::Error::other(
                "execution context Git commit must be 40 lowercase hex characters",
            ));
        }
        if release_duration
            && (!self.repository.tracked_tree_clean || !self.repository.nonignored_untracked_clean)
        {
            return Err(io::Error::other(
                "release-duration evidence requires a clean tracked tree and no nonignored untracked files",
            ));
        }
        self.measurement.validate(release_duration)?;
        self.cargo_lock.validate("Cargo.lock")?;
        self.test_binary.validate("test binary")?;
        self.decoder.validate("decoder")?;
        self.sources.a.validate("source A")?;
        self.sources.b.validate("source B")?;
        self.sources.c.validate("source C")?;
        Ok(())
    }
}

pub fn read_execution_context(path: &Path, release_duration: bool) -> io::Result<ExecutionContext> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_EXECUTION_CONTEXT_BYTES {
        return Err(io::Error::other(
            "execution context must be a bounded non-empty regular file",
        ));
    }
    let bytes = fs::read(path)?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| io::Error::other(error.to_string()))?;
    ensure_json_path_free(&value)?;
    let context: ExecutionContext =
        serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))?;
    context.validate(release_duration)?;
    Ok(context)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSummary {
    pub frames: u64,
    pub intervals: u64,
    pub output_fps: f64,
    pub intervals_over_two_frames: u64,
    pub intervals_over_two_frames_rate: f64,
}

#[derive(Debug)]
pub struct FrameMeasurements {
    warmup: Duration,
    expected_interval: Duration,
    timestamps: Vec<Duration>,
}

impl FrameMeasurements {
    pub fn new(warmup: Duration, expected_interval: Duration) -> Self {
        Self {
            warmup,
            expected_interval,
            timestamps: Vec::new(),
        }
    }

    pub fn record(&mut self, elapsed: Duration) {
        self.timestamps.push(elapsed);
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "bounded soak counts are converted to f64 only for measured rates"
    )]
    pub fn summarize(&self) -> io::Result<FrameSummary> {
        if self.expected_interval.is_zero() {
            return Err(io::Error::other("expected frame interval must be nonzero"));
        }
        let measured = self
            .timestamps
            .iter()
            .copied()
            .filter(|elapsed| *elapsed >= self.warmup)
            .collect::<Vec<_>>();
        if measured.len() < 2 {
            return Err(io::Error::other(
                "at least two post-warmup frames are required",
            ));
        }
        if measured.windows(2).any(|pair| pair[1] <= pair[0]) {
            return Err(io::Error::other(
                "frame timestamps must be strictly increasing",
            ));
        }
        let interval_count = measured.len() - 1;
        let elapsed = measured[interval_count]
            .checked_sub(measured[0])
            .ok_or_else(|| io::Error::other("frame elapsed time moved backwards"))?;
        let output_fps = interval_count as f64 / elapsed.as_secs_f64();
        let long_interval_threshold = self
            .expected_interval
            .checked_mul(2)
            .ok_or_else(|| io::Error::other("frame interval threshold overflowed"))?;
        let intervals_over_two_frames = measured
            .windows(2)
            .filter(|pair| {
                pair[1]
                    .checked_sub(pair[0])
                    .is_some_and(|interval| interval > long_interval_threshold)
            })
            .count();
        Ok(FrameSummary {
            frames: u64::try_from(measured.len())
                .map_err(|_| io::Error::other("frame count overflowed"))?,
            intervals: u64::try_from(interval_count)
                .map_err(|_| io::Error::other("interval count overflowed"))?,
            output_fps,
            intervals_over_two_frames: u64::try_from(intervals_over_two_frames)
                .map_err(|_| io::Error::other("long interval count overflowed"))?,
            intervals_over_two_frames_rate: intervals_over_two_frames as f64
                / interval_count as f64,
        })
    }
}

pub fn percentile_95(values: &[Duration]) -> io::Result<Duration> {
    if values.is_empty() {
        return Err(io::Error::other("p95 requires at least one measurement"));
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = sorted.len().saturating_mul(95).div_ceil(100);
    sorted
        .get(rank.saturating_sub(1))
        .copied()
        .ok_or_else(|| io::Error::other("p95 rank overflowed"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteSample {
    pub elapsed: Duration,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ByteTrend {
    pub start_bytes: u64,
    pub peak_bytes: u64,
    pub end_bytes: u64,
    pub end_minus_start_bytes: i64,
    pub least_squares_bytes_per_hour: f64,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "byte counters use f64 only for least-squares trend reporting"
)]
pub fn summarize_bytes(samples: &[ByteSample]) -> io::Result<ByteTrend> {
    let Some(first) = samples.first() else {
        return Err(io::Error::other("byte trend requires at least one sample"));
    };
    if samples
        .windows(2)
        .any(|pair| pair[1].elapsed < pair[0].elapsed)
    {
        return Err(io::Error::other("byte sample timestamps must be monotonic"));
    }
    let last = samples
        .last()
        .ok_or_else(|| io::Error::other("byte trend lost its last sample"))?;
    let peak_bytes = samples
        .iter()
        .map(|sample| sample.bytes)
        .max()
        .ok_or_else(|| io::Error::other("byte trend lost its peak"))?;
    let count = samples.len() as f64;
    let mean_x = samples
        .iter()
        .map(|sample| sample.elapsed.as_secs_f64() / 3_600.0)
        .sum::<f64>()
        / count;
    let mean_y = samples
        .iter()
        .map(|sample| sample.bytes as f64)
        .sum::<f64>()
        / count;
    let (numerator, denominator) = samples.iter().fold((0.0, 0.0), |acc, sample| {
        let x = sample.elapsed.as_secs_f64() / 3_600.0 - mean_x;
        let y = sample.bytes as f64 - mean_y;
        (acc.0 + x * y, acc.1 + x * x)
    });
    let slope = if denominator == 0.0 {
        0.0
    } else {
        numerator / denominator
    };
    let delta = i128::from(last.bytes) - i128::from(first.bytes);
    Ok(ByteTrend {
        start_bytes: first.bytes,
        peak_bytes,
        end_bytes: last.bytes,
        end_minus_start_bytes: i64::try_from(delta)
            .map_err(|_| io::Error::other("byte delta does not fit i64"))?,
        least_squares_bytes_per_hour: slope,
    })
}

pub fn validate_resource_sample_coverage(
    sample_elapsed: &[Duration],
    warmup: Duration,
    duration: Duration,
    interval: Duration,
    release_duration: bool,
) -> io::Result<()> {
    if interval.is_zero() || warmup >= duration {
        return Err(io::Error::other(
            "resource coverage received an invalid measurement schedule",
        ));
    }
    if sample_elapsed.windows(2).any(|pair| pair[1] < pair[0]) {
        return Err(io::Error::other(
            "resource sample timestamps must be monotonic",
        ));
    }
    let post_warmup = sample_elapsed
        .iter()
        .copied()
        .filter(|elapsed| *elapsed >= warmup)
        .collect::<Vec<_>>();
    if post_warmup.is_empty() {
        return Err(io::Error::other(
            "resource coverage has no post-warmup samples",
        ));
    }
    if !release_duration {
        return Ok(());
    }
    if interval > MAX_RELEASE_SAMPLE_INTERVAL {
        return Err(io::Error::other(
            "release resource interval exceeds 60 seconds",
        ));
    }
    if post_warmup.len() < MIN_RELEASE_RESOURCE_SAMPLES {
        return Err(io::Error::other(format!(
            "release resource coverage requires at least {MIN_RELEASE_RESOURCE_SAMPLES} post-warmup samples",
        )));
    }

    let grace = (interval * 3).max(Duration::from_secs(30));
    let first = post_warmup[0];
    let last = *post_warmup
        .last()
        .ok_or_else(|| io::Error::other("resource coverage lost its last sample"))?;
    if first > warmup.saturating_add(grace) {
        return Err(io::Error::other(
            "release resource sampling started too late after warmup",
        ));
    }
    if last.saturating_add(grace) < duration {
        return Err(io::Error::other(
            "release resource sampling ended too early",
        ));
    }
    if post_warmup
        .windows(2)
        .any(|pair| pair[1].saturating_sub(pair[0]) > grace)
    {
        return Err(io::Error::other(
            "release resource sampling contains an excessive gap",
        ));
    }
    let required_window = duration.saturating_sub(warmup);
    let measured_window = last.saturating_sub(first);
    if measured_window.saturating_add(grace * 2) < required_window {
        return Err(io::Error::other(
            "release resource samples do not cover the measurement window",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodecRuntimeInputIdentities {
    pub codec_pack_manifest: FileIdentity,
    pub integrity_catalog: FileIdentity,
    pub worker_executable: FileIdentity,
    pub integrity_catalog_file_count: u64,
    pub self_contained: bool,
}

impl CodecRuntimeInputIdentities {
    pub fn validate(&self) -> io::Result<()> {
        self.codec_pack_manifest.validate("codec-pack manifest")?;
        self.integrity_catalog
            .validate("codec-pack integrity catalog")?;
        self.worker_executable
            .validate("codec-pack worker executable")?;
        if self.integrity_catalog_file_count == 0 || !self.self_contained {
            return Err(io::Error::other(
                "codec runtime is not a self-contained physical pack",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RealtimeSoakReceipt {
    schema_version: u32,
    evidence_kind: String,
    generated_at_unix_seconds: u64,
    mode: String,
    deck: String,
    algorithm: String,
    xs5_routing: Option<String>,
    #[serde(default)]
    execution_context: Option<ExecutionContext>,
    #[serde(default)]
    codec_runtime_inputs: Option<CodecRuntimeInputIdentities>,
    configuration: ReceiptConfiguration,
    runtime: ReceiptRuntime,
    sources: ReceiptSources,
    presentation: ReceiptPresentation,
    renderer: ReceiptRenderer,
    #[serde(alias = "control_to_effect")]
    control_to_processed_frame: ReceiptControlLatency,
    #[serde(default)]
    resource_sampling: Option<ReceiptResourceSampling>,
    queue_and_backpressure: ReceiptQueue,
    memory: ReceiptMemory,
    partial_cleanup: ReceiptPartialCleanup,
    release_gates: ReceiptReleaseGates,
    privacy: ReceiptPrivacy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptConfiguration {
    duration_seconds: f64,
    warmup_seconds: f64,
    control_interval_seconds: f64,
    resource_interval_seconds: f64,
    release_default_duration_seconds: u64,
    release_duration_exercised: bool,
    target_frame_rate: ReceiptFrameRate,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptFrameRate {
    numerator: u64,
    denominator: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRuntime {
    elapsed_seconds: f64,
    host_build_profile: String,
    codec_pack: ReceiptCodecPack,
    decoder: ReceiptDecoder,
    seed: u64,
    causal_reset_count: u64,
    #[serde(default)]
    worker_environment: Option<ReceiptWorkerEnvironment>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptCodecPack {
    pack_id: String,
    pack_version: String,
    adapter_id: String,
    adapter_version: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptDecoder {
    asset_id: String,
    sha256: String,
    byte_length: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWorkerEnvironment {
    torch_version: String,
    cuda_runtime: String,
    device: ReceiptWorkerDevice,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWorkerDevice {
    ordinal: u16,
    name: String,
    total_memory_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptSources {
    geometry: ReceiptGeometry,
    slot_order: Vec<String>,
    distinct_real_cartridges: u64,
    duplicate_label: Option<String>,
    four_independent_cartridge_acceptance: Option<bool>,
    entries: Vec<ReceiptSourceEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptGeometry {
    decoded_width: u32,
    decoded_height: u32,
    latent_width: u64,
    latent_height: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptSourceEntry {
    slot: String,
    logical_source: String,
    cartridge_id: String,
    archive_sha256: String,
    #[serde(default)]
    archive_byte_length: Option<u64>,
    visual_latent_slots: u64,
    decoded_frame_count: u64,
    audio_latent_slots: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptPresentation {
    clock: String,
    post_warmup_frames: u64,
    post_warmup_intervals: u64,
    measured_output_fps: f64,
    intervals_over_two_frames: u64,
    intervals_over_two_frames_rate: f64,
    all_rendered_frames: u64,
    frame_checksum: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRenderer {
    backend: String,
    target: String,
    pipeline: String,
    adapter: String,
    submitted_frames: u64,
    final_device_poll_completed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptControlLatency {
    definition: String,
    samples: u64,
    p95_ms: f64,
    limit_ms: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptResourceSampling {
    total_samples: u64,
    post_warmup_samples: u64,
    first_post_warmup_elapsed_seconds: f64,
    last_post_warmup_elapsed_seconds: f64,
    maximum_post_warmup_gap_seconds: f64,
    release_minimum_post_warmup_samples: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptQueue {
    worker_decoded_batches_delta: u64,
    worker_decoded_frames_delta: u64,
    worker_ring_backpressure_delta: u64,
    worker_presentation_skipped_delta: u64,
    host_max_ring_occupancy: u32,
    final_worker_ring_occupancy: u32,
    final_worker_ring_write_sequence: u64,
    final_worker_ring_read_sequence: u64,
    outbound_message_budget: ReceiptBudget,
    inbound_message_budget: ReceiptBudget,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptBudget {
    start: u64,
    end: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptMemory {
    measurement_window: String,
    growth_rule: String,
    worker_process_working_set: ReceiptTrend,
    worker_process_private_usage: ReceiptAssessedTrend,
    host_process_working_set: ReceiptTrend,
    host_process_private_usage: ReceiptAssessedTrend,
    vram_scope: String,
    #[serde(default)]
    native_renderer_vram_measured: Option<bool>,
    torch_cuda_allocated: ReceiptAssessedTrend,
    torch_cuda_reserved: ReceiptAssessedTrend,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptTrend {
    start_bytes: u64,
    peak_bytes: u64,
    end_bytes: u64,
    end_minus_start_bytes: i64,
    least_squares_bytes_per_hour: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptAssessedTrend {
    start_bytes: u64,
    peak_bytes: u64,
    end_bytes: u64,
    end_minus_start_bytes: i64,
    least_squares_bytes_per_hour: f64,
    delta_threshold_bytes: u64,
    slope_threshold_bytes_per_hour: f64,
    progressive_growth_detected: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptPartialCleanup {
    capture_or_resample_attempted: bool,
    scoped_partial_files_before: u64,
    scoped_partial_files_after: u64,
    clean: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "strict receipt parsing preserves each independently recomputed release gate"
)]
struct ReceiptReleaseGates {
    evaluation: String,
    host_build_profile_is_release: bool,
    fps_23_9_to_24_1: bool,
    intervals_over_two_frames_below_0_5_percent: bool,
    #[serde(alias = "control_to_effect_p95_at_most_200ms")]
    control_to_processed_frame_p95_at_most_200ms: bool,
    no_ring_backpressure_or_queue_growth: bool,
    no_progressive_ram_or_worker_allocator_vram_growth: bool,
    no_partial_files: bool,
    all_required_gates_passed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptPrivacy {
    receipt_is_path_free: bool,
    private_payload_embedded: bool,
}

#[derive(Clone, Copy)]
pub struct ReceiptExpectations<'a> {
    pub mode: &'a str,
    pub execution_context: &'a ExecutionContext,
    pub codec_runtime_inputs: &'a CodecRuntimeInputIdentities,
    pub receipt_sha256: &'a str,
    pub expected_legacy_sha256: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedReceipt {
    pub schema_version: u32,
    pub measurement_gates_passed: bool,
    pub provenance_v2_bound: bool,
}

pub fn validate_realtime_soak_receipt(
    bytes: &[u8],
    expectations: ReceiptExpectations<'_>,
) -> io::Result<ValidatedReceipt> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|error| io::Error::other(error.to_string()))?;
    ensure_json_path_free(&value)?;
    let receipt: RealtimeSoakReceipt =
        serde_json::from_value(value).map_err(|error| io::Error::other(error.to_string()))?;
    validate_receipt_identity(&receipt, &expectations)?;
    validate_receipt_measurements(&receipt, expectations.execution_context)?;
    Ok(ValidatedReceipt {
        schema_version: receipt.schema_version,
        measurement_gates_passed: receipt.release_gates.all_required_gates_passed,
        provenance_v2_bound: receipt.schema_version == 2,
    })
}

#[allow(
    clippy::too_many_lines,
    clippy::type_complexity,
    reason = "one strict identity audit keeps schema, mode, topology, and runtime bindings adjacent"
)]
fn validate_receipt_identity(
    receipt: &RealtimeSoakReceipt,
    expectations: &ReceiptExpectations<'_>,
) -> io::Result<()> {
    if receipt.evidence_kind != "latentdeck_private_realtime_soak"
        || receipt.generated_at_unix_seconds == 0
        || receipt.mode != expectations.mode
    {
        return Err(io::Error::other(
            "realtime-soak receipt identity is invalid",
        ));
    }
    match receipt.schema_version {
        1 => {
            let expected = expectations.expected_legacy_sha256.ok_or_else(|| {
                io::Error::other("legacy v1 receipt requires an explicit expected SHA-256")
            })?;
            if !is_canonical_sha256(expected)
                || expectations.receipt_sha256 != expected
                || receipt.execution_context.is_some()
                || receipt.codec_runtime_inputs.is_some()
            {
                return Err(io::Error::other(
                    "legacy v1 receipt bytes are not the explicitly authorized artifact",
                ));
            }
        }
        2 => {
            if receipt.execution_context.as_ref() != Some(expectations.execution_context)
                || receipt.codec_runtime_inputs.as_ref() != Some(expectations.codec_runtime_inputs)
            {
                return Err(io::Error::other(
                    "v2 receipt execution or codec context differs from this suite",
                ));
            }
            expectations.codec_runtime_inputs.validate()?;
        }
        _ => return Err(io::Error::other("unsupported realtime-soak receipt schema")),
    }

    let (deck, algorithm, routing, expected_sources): (
        &str,
        &str,
        Option<&str>,
        Vec<(&str, &str, &FileIdentity)>,
    ) = match expectations.mode {
        "d2-linear" => (
            "LD-D2",
            "LINEAR",
            None,
            vec![
                ("A", "B", &expectations.execution_context.sources.b),
                ("B", "C", &expectations.execution_context.sources.c),
            ],
        ),
        "d2-xs5" => (
            "LD-D2",
            "XS5",
            Some("TOPK"),
            vec![
                ("A", "B", &expectations.execution_context.sources.b),
                ("B", "C", &expectations.execution_context.sources.c),
            ],
        ),
        "q4-topk" => (
            "LD-Q4",
            "XS5",
            Some("TOPK"),
            vec![
                ("A", "B", &expectations.execution_context.sources.b),
                ("B", "C", &expectations.execution_context.sources.c),
                ("C", "A", &expectations.execution_context.sources.a),
                ("D", "B", &expectations.execution_context.sources.b),
            ],
        ),
        "q4-sinkhorn" => (
            "LD-Q4",
            "XS5",
            Some("SINKHORN"),
            vec![
                ("A", "B", &expectations.execution_context.sources.b),
                ("B", "C", &expectations.execution_context.sources.c),
                ("C", "A", &expectations.execution_context.sources.a),
                ("D", "B", &expectations.execution_context.sources.b),
            ],
        ),
        _ => return Err(io::Error::other("unsupported realtime-soak mode")),
    };
    if receipt.deck != deck
        || receipt.algorithm != algorithm
        || receipt.xs5_routing.as_deref() != routing
        || receipt.sources.entries.len() != expected_sources.len()
        || receipt.sources.slot_order
            != expected_sources
                .iter()
                .map(|(_, logical, _)| (*logical).to_owned())
                .collect::<Vec<_>>()
    {
        return Err(io::Error::other(
            "receipt deck, algorithm, routing, or source order is invalid",
        ));
    }
    for (entry, (slot, logical, identity)) in
        receipt.sources.entries.iter().zip(expected_sources.iter())
    {
        if entry.slot != *slot
            || entry.logical_source != *logical
            || entry.archive_sha256 != identity.sha256
            || (receipt.schema_version == 2
                && entry.archive_byte_length != Some(identity.byte_length))
            || entry.cartridge_id.is_empty()
            || match *logical {
                "A" => {
                    entry.visual_latent_slots != 72
                        || entry.decoded_frame_count != 243
                        || entry.audio_latent_slots != Some(405)
                }
                "B" | "C" => {
                    entry.visual_latent_slots != 32
                        || entry.decoded_frame_count != 107
                        || entry.audio_latent_slots != Some(178)
                }
                _ => true,
            }
        {
            return Err(io::Error::other(
                "receipt source identity or slot order is invalid",
            ));
        }
    }
    let is_d2 = receipt.deck == "LD-D2";
    if receipt.sources.distinct_real_cartridges != if is_d2 { 2 } else { 3 }
        || if is_d2 {
            receipt.sources.duplicate_label.is_some()
                || receipt
                    .sources
                    .four_independent_cartridge_acceptance
                    .is_some()
        } else {
            receipt.sources.duplicate_label.as_deref()
                != Some(
                    "slot D intentionally reuses logical cartridge B; 3 distinct real AV sources across 4 slots",
                )
                || receipt.sources.four_independent_cartridge_acceptance != Some(false)
        }
    {
        return Err(io::Error::other(
            "receipt distinct-source and duplicate disclosure is invalid",
        ));
    }
    if receipt.sources.geometry.decoded_width != 448
        || receipt.sources.geometry.decoded_height != 800
        || receipt.sources.geometry.latent_width != 28
        || receipt.sources.geometry.latent_height != 50
        || receipt.runtime.decoder.asset_id != "taeh3"
        || receipt.runtime.decoder.sha256 != expectations.execution_context.decoder.sha256
        || receipt.runtime.decoder.byte_length != expectations.execution_context.decoder.byte_length
        || receipt.runtime.seed != 42
        || receipt.runtime.codec_pack.pack_id.is_empty()
        || receipt.runtime.codec_pack.pack_version.is_empty()
        || receipt.runtime.codec_pack.adapter_id.is_empty()
        || receipt.runtime.codec_pack.adapter_version.is_empty()
    {
        return Err(io::Error::other(
            "receipt geometry, decoder, seed, or codec identity is invalid",
        ));
    }
    if receipt.schema_version == 2 {
        let environment =
            receipt.runtime.worker_environment.as_ref().ok_or_else(|| {
                io::Error::other("v2 receipt omitted worker GPU/runtime evidence")
            })?;
        if environment.torch_version.is_empty()
            || environment.cuda_runtime.is_empty()
            || environment.device.ordinal != 0
            || environment.device.name.is_empty()
            || environment.device.total_memory_bytes == 0
        {
            return Err(io::Error::other(
                "v2 worker GPU/runtime evidence is incomplete",
            ));
        }
        if receipt.memory.native_renderer_vram_measured != Some(false) {
            return Err(io::Error::other(
                "v2 receipt must explicitly disclose that native renderer VRAM is not measured",
            ));
        }
    }
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    clippy::too_many_lines,
    reason = "bounded receipt counters are converted to f64 only to recompute persisted rates"
)]
fn validate_receipt_measurements(
    receipt: &RealtimeSoakReceipt,
    context: &ExecutionContext,
) -> io::Result<()> {
    let measurement = &context.measurement;
    let configuration = &receipt.configuration;
    let expected_release = measurement.duration_seconds >= RELEASE_DURATION.as_secs();
    if !same_seconds(configuration.duration_seconds, measurement.duration_seconds)
        || !same_seconds(configuration.warmup_seconds, measurement.warmup_seconds)
        || !same_seconds(
            configuration.control_interval_seconds,
            measurement.control_interval_seconds,
        )
        || !same_seconds(
            configuration.resource_interval_seconds,
            measurement.resource_interval_seconds,
        )
        || configuration.release_default_duration_seconds != RELEASE_DURATION.as_secs()
        || configuration.release_duration_exercised != expected_release
        || configuration.target_frame_rate.numerator != FRAME_RATE_NUMERATOR
        || configuration.target_frame_rate.denominator != FRAME_RATE_DENOMINATOR
        || !receipt.runtime.elapsed_seconds.is_finite()
        || receipt.runtime.elapsed_seconds < configuration.duration_seconds
        || receipt.runtime.causal_reset_count == u64::MAX
    {
        return Err(io::Error::other(
            "receipt measurement schedule differs from its bound execution context",
        ));
    }
    if receipt.schema_version == 2 {
        validate_receipt_resource_sampling(
            receipt.resource_sampling.as_ref().ok_or_else(|| {
                io::Error::other("v2 receipt omitted resource-sampling coverage evidence")
            })?,
            measurement,
            expected_release,
        )?;
    }

    let presentation = &receipt.presentation;
    if presentation.clock != "absolute_rational_24fps"
        || presentation.post_warmup_frames < 2
        || presentation.post_warmup_intervals + 1 != presentation.post_warmup_frames
        || presentation.all_rendered_frames < presentation.post_warmup_frames
        || presentation.intervals_over_two_frames > presentation.post_warmup_intervals
        || !presentation.measured_output_fps.is_finite()
        || !presentation.intervals_over_two_frames_rate.is_finite()
        || !is_lower_hex(&presentation.frame_checksum, 16)
    {
        return Err(io::Error::other(
            "receipt presentation measurements are invalid",
        ));
    }
    let derived_interval_rate =
        presentation.intervals_over_two_frames as f64 / presentation.post_warmup_intervals as f64;
    if (derived_interval_rate - presentation.intervals_over_two_frames_rate).abs() > 1e-12 {
        return Err(io::Error::other(
            "receipt long-interval rate differs from its raw counts",
        ));
    }

    let control = &receipt.control_to_processed_frame;
    let expected_definition = if receipt.schema_version == 1 {
        "controls.set command start to first DX12-submitted frame whose worker provenance contains the exact new value"
    } else {
        "controls.set command start to first DX12-submitted frame processed with the exact new value recorded in worker provenance"
    };
    if control.definition != expected_definition
        || control.samples == 0
        || !control.p95_ms.is_finite()
        || control.p95_ms < 0.0
        || !control.limit_ms.is_finite()
        || (control.limit_ms - CONTROL_PROCESSED_FRAME_P95_LIMIT.as_secs_f64() * 1_000.0).abs()
            > f64::EPSILON
    {
        return Err(io::Error::other(
            "receipt control-to-processed-frame measurements are invalid",
        ));
    }

    let queue = &receipt.queue_and_backpressure;
    if queue.worker_decoded_batches_delta == 0
        || queue.worker_decoded_frames_delta != presentation.all_rendered_frames
        || queue.final_worker_ring_write_sequence < queue.final_worker_ring_read_sequence
        || u64::from(queue.final_worker_ring_occupancy)
            != queue
                .final_worker_ring_write_sequence
                .saturating_sub(queue.final_worker_ring_read_sequence)
        || queue.outbound_message_budget.end > queue.outbound_message_budget.start
        || queue.inbound_message_budget.end > queue.inbound_message_budget.start
        || receipt.renderer.submitted_frames != presentation.all_rendered_frames
        || receipt.renderer.backend != "DX12"
        || receipt.renderer.target != "offscreen_rgba8_unorm"
        || receipt.renderer.pipeline.is_empty()
        || receipt.renderer.adapter.is_empty()
        || !receipt.renderer.final_device_poll_completed
    {
        return Err(io::Error::other(
            "receipt renderer or queue measurements are inconsistent",
        ));
    }

    validate_trend(&receipt.memory.worker_process_working_set)?;
    validate_trend(&receipt.memory.host_process_working_set)?;
    let worker_private = validate_assessed_trend(&receipt.memory.worker_process_private_usage)?;
    let host_private = validate_assessed_trend(&receipt.memory.host_process_private_usage)?;
    let torch_allocated = validate_assessed_trend(&receipt.memory.torch_cuda_allocated)?;
    let torch_reserved = validate_assessed_trend(&receipt.memory.torch_cuda_reserved)?;
    let expected_vram_scope = if receipt.schema_version == 1 {
        "worker torch CUDA allocator only; renderer submission is proven separately and is not included in these byte counters"
    } else {
        "worker torch CUDA allocator only; native renderer VRAM is not measured"
    };
    if receipt.memory.measurement_window != "post_warmup"
        || receipt.memory.growth_rule.is_empty()
        || receipt.memory.vram_scope != expected_vram_scope
    {
        return Err(io::Error::other("receipt memory scope is invalid"));
    }

    let fps_pass = (FPS_MINIMUM..=FPS_MAXIMUM).contains(&presentation.measured_output_fps);
    let interval_pass = presentation.intervals_over_two_frames_rate < LONG_INTERVAL_RATE_LIMIT;
    let control_pass = control.p95_ms <= CONTROL_PROCESSED_FRAME_P95_LIMIT.as_secs_f64() * 1_000.0;
    let queue_pass = queue.worker_ring_backpressure_delta == 0
        && queue.worker_presentation_skipped_delta == 0
        && queue.final_worker_ring_occupancy == 0
        && queue.host_max_ring_occupancy <= 4;
    let memory_pass = !worker_private && !host_private && !torch_allocated && !torch_reserved;
    let partial_pass = receipt.partial_cleanup.scoped_partial_files_before == 0
        && receipt.partial_cleanup.scoped_partial_files_after == 0;
    let release_profile_pass = receipt.runtime.host_build_profile == "release";
    let all_required = expected_release
        && release_profile_pass
        && fps_pass
        && interval_pass
        && control_pass
        && queue_pass
        && memory_pass
        && partial_pass;
    let gates = &receipt.release_gates;
    let expected_evaluation = if expected_release {
        "full_duration"
    } else {
        "short_override_not_release_acceptance"
    };
    if gates.evaluation != expected_evaluation
        || gates.host_build_profile_is_release != release_profile_pass
        || gates.fps_23_9_to_24_1 != fps_pass
        || gates.intervals_over_two_frames_below_0_5_percent != interval_pass
        || gates.control_to_processed_frame_p95_at_most_200ms != control_pass
        || gates.no_ring_backpressure_or_queue_growth != queue_pass
        || gates.no_progressive_ram_or_worker_allocator_vram_growth != memory_pass
        || gates.no_partial_files != partial_pass
        || gates.all_required_gates_passed != all_required
        || receipt.partial_cleanup.clean != partial_pass
        || receipt.partial_cleanup.capture_or_resample_attempted
        || !receipt.privacy.receipt_is_path_free
        || receipt.privacy.private_payload_embedded
    {
        return Err(io::Error::other(
            "receipt release gates differ from independently recomputed measurements",
        ));
    }
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    reason = "bounded receipt sample counts and seconds are converted only for coverage checks"
)]
fn validate_receipt_resource_sampling(
    sampling: &ReceiptResourceSampling,
    measurement: &MeasurementIdentity,
    release_duration: bool,
) -> io::Result<()> {
    let release_minimum = u64::try_from(MIN_RELEASE_RESOURCE_SAMPLES)
        .map_err(|_| io::Error::other("release sample minimum overflowed"))?;
    let first = sampling.first_post_warmup_elapsed_seconds;
    let last = sampling.last_post_warmup_elapsed_seconds;
    let maximum_gap = sampling.maximum_post_warmup_gap_seconds;
    if sampling.release_minimum_post_warmup_samples != release_minimum
        || sampling.total_samples < sampling.post_warmup_samples
        || sampling.post_warmup_samples == 0
        || !first.is_finite()
        || !last.is_finite()
        || !maximum_gap.is_finite()
        || first < measurement.warmup_seconds as f64
        || last < first
        || maximum_gap < 0.0
    {
        return Err(io::Error::other(
            "receipt resource-sampling coverage evidence is invalid",
        ));
    }
    if !release_duration {
        return Ok(());
    }
    if sampling.post_warmup_samples < release_minimum {
        return Err(io::Error::other(
            "release receipt has too few post-warmup resource samples",
        ));
    }
    let grace = (measurement.resource_interval_seconds as f64 * 3.0).max(30.0);
    let duration = measurement.duration_seconds as f64;
    let warmup = measurement.warmup_seconds as f64;
    if first > warmup + grace
        || last + grace < duration
        || maximum_gap > grace
        || (last - first) + grace * 2.0 < duration - warmup
    {
        return Err(io::Error::other(
            "release receipt resource samples do not cover the measurement window",
        ));
    }
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    reason = "configured soak seconds are bounded to 7200 before conversion"
)]
fn same_seconds(value: f64, expected: u64) -> bool {
    value.is_finite() && (value - expected as f64).abs() <= f64::EPSILON
}

fn validate_trend(trend: &ReceiptTrend) -> io::Result<()> {
    let delta = i128::from(trend.end_bytes) - i128::from(trend.start_bytes);
    if trend.peak_bytes < trend.start_bytes.max(trend.end_bytes)
        || i64::try_from(delta).ok() != Some(trend.end_minus_start_bytes)
        || !trend.least_squares_bytes_per_hour.is_finite()
    {
        return Err(io::Error::other("receipt memory trend is inconsistent"));
    }
    Ok(())
}

#[allow(
    clippy::cast_precision_loss,
    reason = "bounded byte thresholds are converted only to verify persisted trend evidence"
)]
fn validate_assessed_trend(trend: &ReceiptAssessedTrend) -> io::Result<bool> {
    validate_trend(&ReceiptTrend {
        start_bytes: trend.start_bytes,
        peak_bytes: trend.peak_bytes,
        end_bytes: trend.end_bytes,
        end_minus_start_bytes: trend.end_minus_start_bytes,
        least_squares_bytes_per_hour: trend.least_squares_bytes_per_hour,
    })?;
    let threshold = MEMORY_ABSOLUTE_GROWTH_THRESHOLD.max(trend.start_bytes / 20);
    let signed_threshold = i64::try_from(threshold).unwrap_or(i64::MAX);
    let progressive = trend.end_minus_start_bytes > signed_threshold
        && trend.least_squares_bytes_per_hour > threshold as f64;
    if trend.delta_threshold_bytes != threshold
        || (trend.slope_threshold_bytes_per_hour - threshold as f64).abs() > f64::EPSILON
        || trend.progressive_growth_detected != progressive
    {
        return Err(io::Error::other(
            "receipt progressive-growth assessment was not derived from its raw trend",
        ));
    }
    Ok(progressive)
}

pub fn persist_path_free_receipt(path: &Path, receipt: &Value) -> io::Result<()> {
    ensure_json_path_free(receipt)?;
    if path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "soak receipt already exists",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("soak receipt has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let partial_extension = if extension.is_empty() {
        "partial".to_owned()
    } else {
        format!("{extension}.partial")
    };
    let partial = path.with_extension(partial_extension);
    let payload =
        serde_json::to_vec_pretty(receipt).map_err(|error| io::Error::other(error.to_string()))?;
    let result = (|| {
        let mut output = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&partial)?;
        output.write_all(&payload)?;
        output.sync_all()?;
        drop(output);
        fs::rename(&partial, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result
}

pub fn ensure_json_path_free(value: &Value) -> io::Result<()> {
    if json_contains_machine_path(value) {
        return Err(io::Error::other(
            "soak evidence contains a machine-local path",
        ));
    }
    Ok(())
}

fn json_contains_machine_path(value: &Value) -> bool {
    match value {
        Value::String(text) => string_is_machine_path(text),
        Value::Array(values) => values.iter().any(json_contains_machine_path),
        Value::Object(values) => values.values().any(json_contains_machine_path),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn is_canonical_sha256(value: &str) -> bool {
    is_lower_hex(value, 64)
}

fn is_lower_hex(value: &str, expected_length: usize) -> bool {
    value.len() == expected_length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn string_is_machine_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.starts_with('/')
        || text.starts_with("\\\\")
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/'))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn file_identity(seed: char) -> FileIdentity {
        FileIdentity {
            sha256: seed.to_string().repeat(64),
            byte_length: 123,
        }
    }

    fn execution_context() -> ExecutionContext {
        ExecutionContext {
            schema_version: 2,
            evidence_kind: EXECUTION_CONTEXT_KIND.to_owned(),
            repository: RepositoryIdentity {
                git_commit: "1".repeat(40),
                tracked_tree_clean: true,
                nonignored_untracked_clean: true,
            },
            measurement: MeasurementIdentity {
                duration_seconds: 1_800,
                warmup_seconds: 60,
                control_interval_seconds: 5,
                resource_interval_seconds: 5,
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
            },
            cargo_lock: file_identity('a'),
            test_binary: file_identity('b'),
            decoder: file_identity('c'),
            sources: SourceFileIdentities {
                a: file_identity('d'),
                b: file_identity('e'),
                c: file_identity('f'),
            },
        }
    }

    fn codec_runtime_inputs() -> CodecRuntimeInputIdentities {
        CodecRuntimeInputIdentities {
            codec_pack_manifest: file_identity('7'),
            integrity_catalog: file_identity('8'),
            worker_executable: file_identity('9'),
            integrity_catalog_file_count: 12,
            self_contained: true,
        }
    }

    fn stable_trend() -> Value {
        json!({
            "start_bytes": 100,
            "peak_bytes": 100,
            "end_bytes": 100,
            "end_minus_start_bytes": 0,
            "least_squares_bytes_per_hour": 0.0
        })
    }

    #[allow(
        clippy::cast_precision_loss,
        reason = "the synthetic test uses the exact bounded production threshold"
    )]
    fn stable_assessed_trend() -> Value {
        json!({
            "start_bytes": 100,
            "peak_bytes": 100,
            "end_bytes": 100,
            "end_minus_start_bytes": 0,
            "least_squares_bytes_per_hour": 0.0,
            "delta_threshold_bytes": MEMORY_ABSOLUTE_GROWTH_THRESHOLD,
            "slope_threshold_bytes_per_hour": MEMORY_ABSOLUTE_GROWTH_THRESHOLD as f64,
            "progressive_growth_detected": false
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the strict synthetic fixture deliberately enumerates the complete receipt schema"
    )]
    fn valid_legacy_receipt(context: &ExecutionContext) -> Value {
        json!({
            "schema_version": 1,
            "evidence_kind": "latentdeck_private_realtime_soak",
            "generated_at_unix_seconds": 1,
            "mode": "d2-linear",
            "deck": "LD-D2",
            "algorithm": "LINEAR",
            "xs5_routing": null,
            "configuration": {
                "duration_seconds": 1800.0,
                "warmup_seconds": 60.0,
                "control_interval_seconds": 5.0,
                "resource_interval_seconds": 5.0,
                "release_default_duration_seconds": 1800,
                "release_duration_exercised": true,
                "target_frame_rate": {"numerator": 24, "denominator": 1}
            },
            "runtime": {
                "elapsed_seconds": 1800.1,
                "host_build_profile": "release",
                "codec_pack": {
                    "pack_id": "org.latentdeck.h3",
                    "pack_version": "0.1.0",
                    "adapter_id": "org.latentdeck.h3",
                    "adapter_version": "0.1.0"
                },
                "decoder": {
                    "asset_id": "taeh3",
                    "sha256": context.decoder.sha256,
                    "byte_length": context.decoder.byte_length
                },
                "seed": 42,
                "causal_reset_count": 1
            },
            "sources": {
                "geometry": {
                    "decoded_width": 448,
                    "decoded_height": 800,
                    "latent_width": 28,
                    "latent_height": 50
                },
                "slot_order": ["B", "C"],
                "distinct_real_cartridges": 2,
                "duplicate_label": null,
                "four_independent_cartridge_acceptance": null,
                "entries": [
                    {
                        "slot": "A",
                        "logical_source": "B",
                        "cartridge_id": "00000000-0000-0000-0000-000000000001",
                        "archive_sha256": context.sources.b.sha256,
                        "visual_latent_slots": 32,
                        "decoded_frame_count": 107,
                        "audio_latent_slots": 178
                    },
                    {
                        "slot": "B",
                        "logical_source": "C",
                        "cartridge_id": "00000000-0000-0000-0000-000000000002",
                        "archive_sha256": context.sources.c.sha256,
                        "visual_latent_slots": 32,
                        "decoded_frame_count": 107,
                        "audio_latent_slots": 178
                    }
                ]
            },
            "presentation": {
                "clock": "absolute_rational_24fps",
                "post_warmup_frames": 100,
                "post_warmup_intervals": 99,
                "measured_output_fps": 24.0,
                "intervals_over_two_frames": 0,
                "intervals_over_two_frames_rate": 0.0,
                "all_rendered_frames": 100,
                "frame_checksum": "0123456789abcdef"
            },
            "renderer": {
                "backend": "DX12",
                "target": "offscreen_rgba8_unorm",
                "pipeline": "LatentDeck RgbaFrameRenderer fullscreen triangle",
                "adapter": "Synthetic adapter",
                "submitted_frames": 100,
                "final_device_poll_completed": true
            },
            "control_to_effect": {
                "definition": "controls.set command start to first DX12-submitted frame whose worker provenance contains the exact new value",
                "samples": 1,
                "p95_ms": 50.0,
                "limit_ms": 200.0
            },
            "queue_and_backpressure": {
                "worker_decoded_batches_delta": 25,
                "worker_decoded_frames_delta": 100,
                "worker_ring_backpressure_delta": 0,
                "worker_presentation_skipped_delta": 0,
                "host_max_ring_occupancy": 4,
                "final_worker_ring_occupancy": 0,
                "final_worker_ring_write_sequence": 100,
                "final_worker_ring_read_sequence": 100,
                "outbound_message_budget": {"start": 1000, "end": 900},
                "inbound_message_budget": {"start": 1000, "end": 900}
            },
            "memory": {
                "measurement_window": "post_warmup",
                "growth_rule": "bounded synthetic rule",
                "worker_process_working_set": stable_trend(),
                "worker_process_private_usage": stable_assessed_trend(),
                "host_process_working_set": stable_trend(),
                "host_process_private_usage": stable_assessed_trend(),
                "vram_scope": "worker torch CUDA allocator only; renderer submission is proven separately and is not included in these byte counters",
                "torch_cuda_allocated": stable_assessed_trend(),
                "torch_cuda_reserved": stable_assessed_trend()
            },
            "partial_cleanup": {
                "capture_or_resample_attempted": false,
                "scoped_partial_files_before": 0,
                "scoped_partial_files_after": 0,
                "clean": true
            },
            "release_gates": {
                "evaluation": "full_duration",
                "host_build_profile_is_release": true,
                "fps_23_9_to_24_1": true,
                "intervals_over_two_frames_below_0_5_percent": true,
                "control_to_effect_p95_at_most_200ms": true,
                "no_ring_backpressure_or_queue_growth": true,
                "no_progressive_ram_or_worker_allocator_vram_growth": true,
                "no_partial_files": true,
                "all_required_gates_passed": true
            },
            "privacy": {
                "receipt_is_path_free": true,
                "private_payload_embedded": false
            }
        })
    }

    #[test]
    fn execution_context_is_strict_path_free_and_release_clean() {
        let temporary = tempdir().expect("temporary directory");
        let context_path = temporary.path().join("execution-context.json");
        let context = execution_context();
        fs::write(
            &context_path,
            serde_json::to_vec_pretty(&context).expect("serialize context"),
        )
        .expect("write context");
        assert_eq!(
            read_execution_context(&context_path, true).expect("read context"),
            context
        );

        let mut dirty = execution_context();
        dirty.repository.tracked_tree_clean = false;
        assert!(dirty.validate(true).is_err());
        assert!(dirty.validate(false).is_ok());

        let unsafe_context = json!({
            "schema_version": 2,
            "evidence_kind": EXECUTION_CONTEXT_KIND,
            "repository": {
                "git_commit": "1".repeat(40),
                "tracked_tree_clean": true,
                "nonignored_untracked_clean": true
            },
            "measurement": {
                "duration_seconds": 1800,
                "warmup_seconds": 60,
                "control_interval_seconds": 5,
                "resource_interval_seconds": 5,
                "frame_rate_numerator": 24,
                "frame_rate_denominator": 1
            },
            "cargo_lock": {"sha256": "a".repeat(64), "byte_length": 1},
            "test_binary": {"sha256": "b".repeat(64), "byte_length": 1},
            "decoder": {"sha256": "c".repeat(64), "byte_length": 1},
            "sources": {
                "a": {"sha256": "d".repeat(64), "byte_length": 1},
                "b": {"sha256": "e".repeat(64), "byte_length": 1},
                "c": {"sha256": "f".repeat(64), "byte_length": 1}
            },
            "machine_path": r"W:\private\receipt.json"
        });
        fs::write(
            &context_path,
            serde_json::to_vec(&unsafe_context).expect("serialize unsafe context"),
        )
        .expect("replace context");
        assert!(read_execution_context(&context_path, true).is_err());
    }

    #[test]
    fn frame_summary_measures_absolute_24_fps_and_long_intervals() {
        let interval = Duration::from_nanos(1_000_000_000 / 24);
        let mut measurements = FrameMeasurements::new(Duration::from_secs(1), interval);
        for tick in 0_u64..=72 {
            measurements.record(Duration::from_nanos(tick * 1_000_000_000 / 24));
        }
        let summary = measurements.summarize().expect("frame summary");
        assert_eq!(summary.frames, 49);
        assert_eq!(summary.intervals, 48);
        assert!((summary.output_fps - 24.0).abs() < 0.000_01);
        assert_eq!(summary.intervals_over_two_frames, 0);

        let mut delayed = FrameMeasurements::new(Duration::ZERO, interval);
        for elapsed in [Duration::ZERO, interval, interval * 2, interval * 5] {
            delayed.record(elapsed);
        }
        let delayed_summary = delayed.summarize().expect("delayed summary");
        assert_eq!(delayed_summary.intervals_over_two_frames, 1);
        assert!((delayed_summary.intervals_over_two_frames_rate - (1.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn p95_uses_nearest_rank_without_hiding_the_tail() {
        let values = (1_u64..=100)
            .map(Duration::from_millis)
            .rev()
            .collect::<Vec<_>>();
        assert_eq!(
            percentile_95(&values).expect("p95"),
            Duration::from_millis(95)
        );
        assert!(percentile_95(&[]).is_err());
    }

    #[test]
    fn byte_summary_reports_start_peak_end_and_linear_trend() {
        let samples = [
            ByteSample {
                elapsed: Duration::ZERO,
                bytes: 100,
            },
            ByteSample {
                elapsed: Duration::from_secs(1_800),
                bytes: 250,
            },
            ByteSample {
                elapsed: Duration::from_secs(3_600),
                bytes: 200,
            },
        ];
        let summary = summarize_bytes(&samples).expect("byte summary");
        assert_eq!(summary.start_bytes, 100);
        assert_eq!(summary.peak_bytes, 250);
        assert_eq!(summary.end_bytes, 200);
        assert_eq!(summary.end_minus_start_bytes, 100);
        assert!((summary.least_squares_bytes_per_hour - 100.0).abs() < 1e-9);
    }

    #[test]
    fn release_resource_coverage_rejects_one_sample_and_large_gaps() {
        let covered = (60_u64..=1_800)
            .step_by(5)
            .map(Duration::from_secs)
            .collect::<Vec<_>>();
        validate_resource_sample_coverage(
            &covered,
            Duration::from_secs(60),
            RELEASE_DURATION,
            Duration::from_secs(5),
            true,
        )
        .expect("covered release samples");
        assert!(
            validate_resource_sample_coverage(
                &[RELEASE_DURATION],
                Duration::from_secs(60),
                RELEASE_DURATION,
                Duration::from_secs(5),
                true,
            )
            .is_err()
        );
        let mut gap = covered;
        gap.retain(|elapsed| {
            *elapsed <= Duration::from_secs(600) || *elapsed >= Duration::from_secs(900)
        });
        assert!(
            validate_resource_sample_coverage(
                &gap,
                Duration::from_secs(60),
                RELEASE_DURATION,
                Duration::from_secs(5),
                true,
            )
            .is_err()
        );
    }

    #[test]
    fn persisted_resource_coverage_is_independently_rechecked() {
        let measurement = execution_context().measurement;
        let mut sampling = ReceiptResourceSampling {
            total_samples: 350,
            post_warmup_samples: 349,
            first_post_warmup_elapsed_seconds: 60.0,
            last_post_warmup_elapsed_seconds: 1_800.0,
            maximum_post_warmup_gap_seconds: 5.1,
            release_minimum_post_warmup_samples: 16,
        };
        validate_receipt_resource_sampling(&sampling, &measurement, true)
            .expect("covered persisted resource samples");

        sampling.post_warmup_samples = 1;
        assert!(validate_receipt_resource_sampling(&sampling, &measurement, true).is_err());
        sampling.post_warmup_samples = 349;
        sampling.maximum_post_warmup_gap_seconds = 300.0;
        assert!(validate_receipt_resource_sampling(&sampling, &measurement, true).is_err());
    }

    #[test]
    fn strict_receipt_recomputes_gates_and_requires_explicit_legacy_hash() {
        let context = execution_context();
        let codec = codec_runtime_inputs();
        let expected_hash = "9".repeat(64);
        let receipt = valid_legacy_receipt(&context);
        let bytes = serde_json::to_vec(&receipt).expect("serialize strict receipt");
        let validated = validate_realtime_soak_receipt(
            &bytes,
            ReceiptExpectations {
                mode: "d2-linear",
                execution_context: &context,
                codec_runtime_inputs: &codec,
                receipt_sha256: &expected_hash,
                expected_legacy_sha256: Some(&expected_hash),
            },
        )
        .expect("valid legacy receipt");
        assert_eq!(validated.schema_version, 1);
        assert!(validated.measurement_gates_passed);
        assert!(!validated.provenance_v2_bound);

        assert!(
            validate_realtime_soak_receipt(
                &bytes,
                ReceiptExpectations {
                    mode: "d2-linear",
                    execution_context: &context,
                    codec_runtime_inputs: &codec,
                    receipt_sha256: &expected_hash,
                    expected_legacy_sha256: None,
                },
            )
            .is_err()
        );

        let mut forged = receipt;
        forged["presentation"]["measured_output_fps"] = json!(1.0);
        let forged_bytes = serde_json::to_vec(&forged).expect("serialize forged receipt");
        assert!(
            validate_realtime_soak_receipt(
                &forged_bytes,
                ReceiptExpectations {
                    mode: "d2-linear",
                    execution_context: &context,
                    codec_runtime_inputs: &codec,
                    receipt_sha256: &expected_hash,
                    expected_legacy_sha256: Some(&expected_hash),
                },
            )
            .is_err()
        );

        forged["release_gates"]["all_required_gates_passed"] = json!("false");
        let wrong_type = serde_json::to_vec(&forged).expect("serialize wrong-type receipt");
        assert!(
            validate_realtime_soak_receipt(
                &wrong_type,
                ReceiptExpectations {
                    mode: "d2-linear",
                    execution_context: &context,
                    codec_runtime_inputs: &codec,
                    receipt_sha256: &expected_hash,
                    expected_legacy_sha256: Some(&expected_hash),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn receipt_writer_is_atomic_path_free_and_non_overwriting() {
        let temporary = tempdir().expect("temporary directory");
        let receipt_path = temporary.path().join("receipt.json");
        let receipt = json!({
            "schema_version": 1,
            "source_order": ["B", "C", "A", "B"],
            "archive_sha256": [
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            ]
        });
        persist_path_free_receipt(&receipt_path, &receipt).expect("write receipt");
        assert_eq!(
            fs::read(&receipt_path).expect("read receipt"),
            serde_json::to_vec_pretty(&receipt).expect("serialize receipt")
        );
        assert!(!receipt_path.with_extension("json.partial").exists());
        assert!(persist_path_free_receipt(&receipt_path, &receipt).is_err());

        let unsafe_receipt = json!({"source": r"W:\private\source.lc"});
        assert!(
            persist_path_free_receipt(&temporary.path().join("unsafe.json"), &unsafe_receipt)
                .is_err()
        );
    }
}
