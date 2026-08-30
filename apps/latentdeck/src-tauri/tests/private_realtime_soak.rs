//! Opt-in real AV realtime soak for the four v0.1 release performance modes.
//!
//! Private cartridges, decoder assets, and machine-local paths are supplied
//! only through environment variables. Persisted receipts contain identities
//! and measurements, never local paths. The harness exercises the real
//! isolated `PyTorch` worker, bounded shared-memory RGB ring, ABI upload, and
//! DX12 fullscreen-triangle render submission on an absolute 24 fps clock.

#![cfg(target_os = "windows")]

use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs, io,
    mem::size_of,
    os::windows::{fs::MetadataExt, io::AsRawHandle},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use latentdeck_cartridge::{
    hash::hash_path,
    profile::h3::ValidatedH3Profile,
    reader::{ValidationOptions, open_validated},
};
use latentdeck_control::{
    Ack, BoundedVec, CodecLoad, Command, D2Algorithm, D2Controls, D2ControlsSet, D2Load, D2Mode,
    D2ProcessSlot, D2ProcessSlotAck, D2Reset, D2SourceBinding, D2Transport, D2Xs5Routing,
    DeviceDescriptor, EmptyPayload, ExternalAssetBinding, FiniteF64, MAX_CONTROL_FRAME_BYTES,
    MetricsSnapshot, ProfileRef, Q4Algorithm, Q4Controls, Q4ControlsSet, Q4InfluenceMode, Q4Load,
    Q4Mode, Q4ProcessSlot, Q4ProcessSlotAck, Q4Reset, Q4Roles, Q4SourceBinding, Q4Transport,
    Q4Xs5Routing, RingBind, SessionConfigure, ShutdownReason, WORKER_PROTOCOL_VERSION, WireUuid,
};
use latentdeck_core::{
    codec_pack::{
        ValidatedCodecPack, ValidatedExternalAsset, discover_codec_packs, validate_external_asset,
    },
    worker_client::WorkerClient,
    worker_supervisor::{ValidatedWorkerLaunch, spawn_worker},
};
use latentdeck_gpu::{
    renderer::{Dx12Device, RgbaFrameRenderer, RgbaUpload, create_dx12_instance},
    ring::{ReadStatus, RgbaFrame, RingDescriptor},
    windows_ring::{WindowsRgbRingConsumer, WindowsRgbRingOwner},
};
use semver::Version;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokio::time::{Instant, sleep_until};
use windows_sys::Win32::{
    Foundation::HANDLE,
    System::{
        ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        Threading::GetCurrentProcess,
    },
};

#[path = "support/realtime_soak.rs"]
mod realtime_soak;

use realtime_soak::{
    ByteSample, ByteTrend, CONTROL_PROCESSED_FRAME_P95_LIMIT, CodecRuntimeInputIdentities,
    ExecutionContext, FPS_MAXIMUM, FPS_MINIMUM, FRAME_RATE_DENOMINATOR, FRAME_RATE_NUMERATOR,
    FileIdentity, FrameMeasurements, FrameSummary, LONG_INTERVAL_RATE_LIMIT,
    MEMORY_ABSOLUTE_GROWTH_THRESHOLD, MIN_RELEASE_RESOURCE_SAMPLES, MeasurementIdentity,
    RELEASE_DURATION, ReceiptExpectations, percentile_95, persist_path_free_receipt,
    read_execution_context, summarize_bytes, validate_realtime_soak_receipt,
    validate_resource_sample_coverage,
};

const OPT_IN_ENV: &str = "LATENTDECK_PRIVATE_REALTIME_SOAK";
const MODE_ENV: &str = "LATENTDECK_PRIVATE_SOAK_MODE";
const CODEC_ROOT_ENV: &str = "LATENTDECK_PRIVATE_CODEC_ROOT";
const DECODER_ENV: &str = "LATENTDECK_PRIVATE_TAEH3";
const SOURCE_A_ENV: &str = "LATENTDECK_PRIVATE_SOAK_SOURCE_A";
const SOURCE_B_ENV: &str = "LATENTDECK_PRIVATE_SOAK_SOURCE_B";
const SOURCE_C_ENV: &str = "LATENTDECK_PRIVATE_SOAK_SOURCE_C";
const RECEIPT_ENV: &str = "LATENTDECK_PRIVATE_SOAK_RECEIPT";
const DURATION_ENV: &str = "LATENTDECK_PRIVATE_SOAK_DURATION_SECONDS";
const WARMUP_ENV: &str = "LATENTDECK_PRIVATE_SOAK_WARMUP_SECONDS";
const CONTROL_INTERVAL_ENV: &str = "LATENTDECK_PRIVATE_SOAK_CONTROL_INTERVAL_SECONDS";
const RESOURCE_INTERVAL_ENV: &str = "LATENTDECK_PRIVATE_SOAK_RESOURCE_INTERVAL_SECONDS";
const EXECUTION_CONTEXT_ENV: &str = "LATENTDECK_PRIVATE_SOAK_EXECUTION_CONTEXT";
const VALIDATE_RECEIPT_ENV: &str = "LATENTDECK_PRIVATE_SOAK_VALIDATE_RECEIPT";
const EXPECTED_LEGACY_SHA256_ENV: &str = "LATENTDECK_PRIVATE_SOAK_EXPECTED_LEGACY_SHA256";

const PACK_ID: &str = "org.latentdeck.h3";
const ASSET_ID: &str = "taeh3";
const D2_DECK_ID: &str = "private-realtime-soak-d2";
const Q4_DECK_ID: &str = "private-realtime-soak-q4";
const D2_OPERATOR_ID: &str = "org.latentdeck.builtin.ld_d2";
const Q4_OPERATOR_ID: &str = "org.latentdeck.builtin.ld_q4";
const OPERATOR_VERSION: &str = "0.1.0";
const ZERO_SHA256: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const INITIAL_GENERATION: u64 = 1;
const SOAK_SEED: u64 = 42;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
const RELEASE_WARMUP: Duration = Duration::from_secs(60);
const DEFAULT_CONTROL_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_RESOURCE_INTERVAL: Duration = Duration::from_secs(5);
const MAX_CONFIGURED_DURATION: Duration = Duration::from_secs(7_200);
const TARGET_FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000_u64.div_ceil(24));

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SoakMode {
    D2Linear,
    D2Xs5,
    Q4TopK,
    Q4Sinkhorn,
}

impl SoakMode {
    fn parse(value: &str) -> TestResult<Self> {
        match value {
            "d2-linear" => Ok(Self::D2Linear),
            "d2-xs5" => Ok(Self::D2Xs5),
            "q4-topk" => Ok(Self::Q4TopK),
            "q4-sinkhorn" => Ok(Self::Q4Sinkhorn),
            _ => failure("soak mode must be d2-linear, d2-xs5, q4-topk, or q4-sinkhorn"),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::D2Linear => "d2-linear",
            Self::D2Xs5 => "d2-xs5",
            Self::Q4TopK => "q4-topk",
            Self::Q4Sinkhorn => "q4-sinkhorn",
        }
    }

    const fn deck(self) -> &'static str {
        match self {
            Self::D2Linear | Self::D2Xs5 => "LD-D2",
            Self::Q4TopK | Self::Q4Sinkhorn => "LD-Q4",
        }
    }

    const fn algorithm(self) -> &'static str {
        match self {
            Self::D2Linear => "LINEAR",
            Self::D2Xs5 | Self::Q4TopK | Self::Q4Sinkhorn => "XS5",
        }
    }

    const fn routing(self) -> Option<&'static str> {
        match self {
            Self::D2Linear => None,
            Self::D2Xs5 | Self::Q4TopK => Some("TOPK"),
            Self::Q4Sinkhorn => Some("SINKHORN"),
        }
    }

    const fn is_d2(self) -> bool {
        matches!(self, Self::D2Linear | Self::D2Xs5)
    }
}

#[derive(Clone, Copy, Debug)]
struct SoakConfig {
    duration: Duration,
    warmup: Duration,
    control_interval: Duration,
    resource_interval: Duration,
}

impl SoakConfig {
    fn from_env() -> TestResult<Self> {
        let duration = optional_seconds(DURATION_ENV)?.unwrap_or(RELEASE_DURATION);
        require(
            !duration.is_zero() && duration <= MAX_CONFIGURED_DURATION,
            "soak duration must be within 1..=7200 seconds",
        )?;
        let default_warmup = if duration >= RELEASE_DURATION {
            RELEASE_WARMUP
        } else {
            Duration::from_secs(duration.as_secs() / 5)
        };
        let warmup = optional_seconds(WARMUP_ENV)?.unwrap_or(default_warmup);
        require(
            warmup < duration,
            "soak warmup must be shorter than the configured duration",
        )?;
        let control_interval =
            optional_seconds(CONTROL_INTERVAL_ENV)?.unwrap_or(DEFAULT_CONTROL_INTERVAL);
        let resource_interval =
            optional_seconds(RESOURCE_INTERVAL_ENV)?.unwrap_or(DEFAULT_RESOURCE_INTERVAL);
        require(
            !control_interval.is_zero() && !resource_interval.is_zero(),
            "soak sampling intervals must be nonzero",
        )?;
        Ok(Self {
            duration,
            warmup,
            control_interval,
            resource_interval,
        })
    }

    const fn release_duration_exercised(self) -> bool {
        self.duration.as_secs() >= RELEASE_DURATION.as_secs()
    }

    const fn measurement_identity(self) -> MeasurementIdentity {
        MeasurementIdentity {
            duration_seconds: self.duration.as_secs(),
            warmup_seconds: self.warmup.as_secs(),
            control_interval_seconds: self.control_interval.as_secs(),
            resource_interval_seconds: self.resource_interval.as_secs(),
            frame_rate_numerator: FRAME_RATE_NUMERATOR,
            frame_rate_denominator: FRAME_RATE_DENOMINATOR,
        }
    }
}

#[derive(Clone)]
struct PrivateSource {
    path: PathBuf,
    cartridge_id: WireUuid,
    archive_bytes: u64,
    archive_sha256: String,
    profile: ValidatedH3Profile,
}

struct SourceSet {
    a: PrivateSource,
    b: PrivateSource,
    c: PrivateSource,
}

type CodecRuntimeInputs = CodecRuntimeInputIdentities;

impl CodecRuntimeInputs {
    fn measure(pack: &ValidatedCodecPack) -> TestResult<Self> {
        Ok(Self {
            codec_pack_manifest: measure_file_identity(&pack.root.join("codec-pack.json"))?,
            integrity_catalog: measure_file_identity(
                &pack.root.join(&pack.manifest.integrity.catalog_path),
            )?,
            worker_executable: measure_file_identity(&pack.worker_executable)?,
            integrity_catalog_file_count: validate_self_contained_pack(pack)?,
            self_contained: true,
        })
    }

    fn as_value(&self) -> Value {
        json!({
            "codec_pack_manifest": self.codec_pack_manifest,
            "integrity_catalog": self.integrity_catalog,
            "worker_executable": self.worker_executable,
            "integrity_catalog_file_count": self.integrity_catalog_file_count,
            "self_contained": self.self_contained
        })
    }
}

impl SourceSet {
    fn resolve() -> TestResult<Self> {
        let sources = Self {
            a: validate_source(exact_env_path(SOURCE_A_ENV)?)?,
            b: validate_source(exact_env_path(SOURCE_B_ENV)?)?,
            c: validate_source(exact_env_path(SOURCE_C_ENV)?)?,
        };
        sources.validate_real_av_contract()?;
        Ok(sources)
    }

    fn validate_real_av_contract(&self) -> TestResult<()> {
        let all = [&self.a, &self.b, &self.c];
        let identities = all
            .iter()
            .map(|source| source.cartridge_id.to_string())
            .collect::<BTreeSet<_>>();
        let hashes = all
            .iter()
            .map(|source| source.archive_sha256.clone())
            .collect::<BTreeSet<_>>();
        require(
            identities.len() == 3 && hashes.len() == 3,
            "soak requires exactly three distinct real AV cartridge identities",
        )?;
        require(
            all.iter().all(|source| {
                source.profile.compatibility_key == self.b.profile.compatibility_key
                    && source.profile.visual.decoded_width == 448
                    && source.profile.visual.decoded_height == 800
                    && source.profile.visual.latent_width == 28
                    && source.profile.visual.latent_height == 50
                    && source.profile.audio.is_some()
            }),
            "soak sources must share the exact real 448x800 H3 AV compatibility contract",
        )?;
        require(
            self.a.profile.visual.latent_slots == 72
                && self.a.profile.visual.decoded_frame_count == 243
                && self
                    .a
                    .profile
                    .audio
                    .as_ref()
                    .is_some_and(|audio| audio.latent_slots == 405),
            "logical A must preserve T=72 -> 243 and audio T=405",
        )?;
        for source in [&self.b, &self.c] {
            require(
                source.profile.visual.latent_slots == 32
                    && source.profile.visual.decoded_frame_count == 107
                    && source
                        .profile
                        .audio
                        .as_ref()
                        .is_some_and(|audio| audio.latent_slots == 178),
                "logical B/C must preserve T=32 -> 107 and audio T=178",
            )?;
        }
        Ok(())
    }

    fn q4_slots(&self) -> [PrivateSource; 4] {
        [
            self.b.clone(),
            self.c.clone(),
            self.a.clone(),
            self.b.clone(),
        ]
    }
}

#[derive(Clone, Copy, Debug)]
struct ProcessMemory {
    working_set_bytes: u64,
    private_usage_bytes: u64,
}

#[derive(Clone, Debug)]
struct ResourceSample {
    elapsed: Duration,
    worker_memory: ProcessMemory,
    host_memory: ProcessMemory,
    worker_metrics: MetricsSnapshot,
}

struct PendingControl {
    started: Instant,
    expected_value: f64,
}

struct MeasurementState {
    started: Instant,
    clock: PresentationClock,
    frames: FrameMeasurements,
    control_latencies: Vec<Duration>,
    resources: Vec<ResourceSample>,
    rendered_frames: u64,
    frame_checksum: u64,
    host_max_ring_occupancy: u32,
    reset_count: u64,
}

impl MeasurementState {
    fn new(config: SoakConfig) -> TestResult<Self> {
        Ok(Self {
            started: Instant::now(),
            clock: PresentationClock::new(FRAME_RATE_NUMERATOR, FRAME_RATE_DENOMINATOR)?,
            frames: FrameMeasurements::new(config.warmup, TARGET_FRAME_INTERVAL),
            control_latencies: Vec::new(),
            resources: Vec::new(),
            rendered_frames: 0,
            frame_checksum: 0,
            host_max_ring_occupancy: 0,
            reset_count: 0,
        })
    }

    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

struct ModeEvidence {
    elapsed: Duration,
    frame_summary: FrameSummary,
    rendered_frames: u64,
    frame_checksum: u64,
    control_latencies: Vec<Duration>,
    resources: Vec<ResourceSample>,
    host_max_ring_occupancy: u32,
    reset_count: u64,
    session_outbound_budget_start: usize,
    session_outbound_budget_end: usize,
    session_inbound_budget_start: usize,
    session_inbound_budget_end: usize,
    renderer_adapter: String,
    renderer_final_poll_completed: bool,
    partial_files_before: u64,
    partial_files_after: u64,
    worker_environment: WorkerEnvironmentEvidence,
}

#[derive(Clone, Debug)]
struct WorkerEnvironmentEvidence {
    torch_version: String,
    cuda_runtime: String,
    device: DeviceDescriptor,
}

struct OffscreenRenderer {
    _instance: wgpu::Instance,
    context: Dx12Device,
    renderer: RgbaFrameRenderer,
    _target: wgpu::Texture,
    target_view: wgpu::TextureView,
    submitted_frames: u64,
}

impl OffscreenRenderer {
    async fn new(width: u32, height: u32) -> TestResult<Self> {
        let instance = create_dx12_instance()?;
        let context = Dx12Device::request(&instance, None).await?;
        let renderer = RgbaFrameRenderer::new(
            context.device(),
            wgpu::TextureFormat::Rgba8Unorm,
            width,
            height,
        )?;
        let target = context.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("LatentDeck private realtime soak target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(Self {
            _instance: instance,
            context,
            renderer,
            _target: target,
            target_view,
            submitted_frames: 0,
        })
    }

    fn adapter_name(&self) -> String {
        self.context.adapter().get_info().name
    }

    fn render(&mut self, frame: &RgbaFrame) -> TestResult<()> {
        self.renderer
            .upload(self.context.queue(), RgbaUpload::from_ring_frame(frame)?)?;
        let mut encoder =
            self.context
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("LatentDeck private realtime soak encoder"),
                });
        self.renderer.encode(&mut encoder, &self.target_view);
        self.context.queue().submit([encoder.finish()]);
        self.submitted_frames = self
            .submitted_frames
            .checked_add(1)
            .ok_or_else(|| io::Error::other("renderer frame counter overflowed"))?;
        if self.submitted_frames.is_multiple_of(120) {
            self.context.device().poll(wgpu::PollType::Poll)?;
        }
        Ok(())
    }

    fn finish(&self) -> TestResult<()> {
        self.context
            .device()
            .poll(wgpu::PollType::wait_indefinitely())?;
        Ok(())
    }
}

struct PresentationClock {
    numerator: u64,
    denominator: u64,
    epoch: Instant,
    next_tick: u64,
}

impl PresentationClock {
    fn new(numerator: u64, denominator: u64) -> TestResult<Self> {
        require(
            numerator > 0 && denominator > 0,
            "presentation clock ratio must be nonzero",
        )?;
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

    fn next_deadline(&self) -> TestResult<Instant> {
        let offset = u128::from(self.next_tick)
            .checked_mul(u128::from(self.denominator))
            .and_then(|value| value.checked_mul(1_000_000_000))
            .ok_or_else(|| io::Error::other("presentation clock overflowed"))?
            / u128::from(self.numerator);
        let offset = u64::try_from(offset)
            .map_err(|_| io::Error::other("presentation deadline does not fit u64"))?;
        self.epoch
            .checked_add(Duration::from_nanos(offset))
            .ok_or_else(|| io::Error::other("presentation deadline overflowed").into())
    }

    fn advance_past(&mut self, now: Instant) -> TestResult<()> {
        self.next_tick = self.next_tick.saturating_add(1);
        while self.next_deadline()? <= now {
            self.next_tick = self.next_tick.saturating_add(1);
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires private real H3 AV cartridges, physical self-contained Codec Pack, TAEH3, CUDA, DX12, and explicit GPU time"]
async fn private_realtime_soak_mode() -> TestResult<()> {
    require(
        env::var(OPT_IN_ENV).ok().as_deref() == Some("1"),
        "set LATENTDECK_PRIVATE_REALTIME_SOAK=1 to run the private soak",
    )?;
    let mode = SoakMode::parse(&required_env(MODE_ENV)?)?;
    let config = SoakConfig::from_env()?;
    let receipt_path = exact_env_path(RECEIPT_ENV)?;
    let execution_context = read_execution_context(
        &exact_env_path(EXECUTION_CONTEXT_ENV)?,
        config.release_duration_exercised(),
    )?;
    require(
        execution_context.measurement == config.measurement_identity(),
        "soak environment timing differs from the bound execution context",
    )?;
    let sources = SourceSet::resolve()?;
    let codec_root = exact_env_path(CODEC_ROOT_ENV)?;
    let decoder_path = exact_env_path(DECODER_ENV)?;
    let pack = select_pack(&codec_root, mode)?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;
    verify_execution_context_inputs(&execution_context, &decoder, &sources)?;
    let codec_runtime_inputs = CodecRuntimeInputs::measure(&pack)?;

    let evidence = run_mode(mode, config, &pack, &decoder, &sources).await?;
    let post_pack = select_pack(&codec_root, mode)?;
    require(
        post_pack.root == pack.root
            && CodecRuntimeInputs::measure(&post_pack)? == codec_runtime_inputs,
        "codec pack inputs changed during the realtime soak",
    )?;
    verify_execution_context_inputs(&execution_context, &decoder, &sources)?;
    let receipt = build_receipt(
        mode,
        config,
        &pack,
        &decoder,
        &sources,
        &execution_context,
        &codec_runtime_inputs,
        &evidence,
    )?;
    let strict_receipt_bytes = serde_json::to_vec(&receipt)?;
    validate_realtime_soak_receipt(
        &strict_receipt_bytes,
        ReceiptExpectations {
            mode: mode.name(),
            execution_context: &execution_context,
            codec_runtime_inputs: &codec_runtime_inputs,
            receipt_sha256: ZERO_SHA256,
            expected_legacy_sha256: None,
        },
    )?;
    let release_gates_passed = receipt
        .pointer("/release_gates/all_required_gates_passed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    persist_path_free_receipt(&receipt_path, &receipt)?;
    if config.release_duration_exercised() {
        require(
            release_gates_passed,
            "full-duration realtime soak failed one or more release gates",
        )?;
    }
    Ok(())
}

#[test]
#[ignore = "validates one private path-free realtime-soak receipt without starting a worker"]
fn validate_private_realtime_soak_receipt() -> TestResult<()> {
    require(
        env::var(OPT_IN_ENV).ok().as_deref() == Some("1"),
        "set LATENTDECK_PRIVATE_REALTIME_SOAK=1 to validate a private soak receipt",
    )?;
    let mode = SoakMode::parse(&required_env(MODE_ENV)?)?;
    let config = SoakConfig::from_env()?;
    let execution_context = read_execution_context(
        &exact_env_path(EXECUTION_CONTEXT_ENV)?,
        config.release_duration_exercised(),
    )?;
    require(
        execution_context.measurement == config.measurement_identity(),
        "receipt-validation timing differs from the bound execution context",
    )?;
    let sources = SourceSet::resolve()?;
    let codec_root = exact_env_path(CODEC_ROOT_ENV)?;
    let decoder_path = exact_env_path(DECODER_ENV)?;
    let pack = select_pack(&codec_root, mode)?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;
    verify_execution_context_inputs(&execution_context, &decoder, &sources)?;
    let codec_runtime_inputs = CodecRuntimeInputs::measure(&pack)?;
    let receipt_path = exact_env_path(VALIDATE_RECEIPT_ENV)?;
    let measured_receipt = measure_file_identity(&receipt_path)?;
    let bytes = fs::read(&receipt_path)?;
    let expected_legacy = env::var(EXPECTED_LEGACY_SHA256_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    let validated = validate_realtime_soak_receipt(
        &bytes,
        ReceiptExpectations {
            mode: mode.name(),
            execution_context: &execution_context,
            codec_runtime_inputs: &codec_runtime_inputs,
            receipt_sha256: &measured_receipt.sha256,
            expected_legacy_sha256: expected_legacy.as_deref(),
        },
    )?;
    if config.release_duration_exercised() {
        require(
            validated.measurement_gates_passed,
            "validated full-duration receipt failed independently recomputed gates",
        )?;
    }
    Ok(())
}

async fn run_mode(
    mode: SoakMode,
    config: SoakConfig,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &SourceSet,
) -> TestResult<ModeEvidence> {
    let launch = if mode.is_d2() {
        ValidatedWorkerLaunch::from_codec_pack_d2(pack)?
    } else {
        ValidatedWorkerLaunch::from_codec_pack_q4(pack)?
    };
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);
    let exercise = if mode.is_d2() {
        exercise_d2(&mut client, mode, config, pack, decoder, sources).await
    } else {
        exercise_q4(&mut client, mode, config, pack, decoder, sources).await
    };
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "soak worker returned an unsuccessful orderly exit",
        ),
        Err(_) => client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>),
    };
    match (exercise, shutdown) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(evidence), Ok(())) => Ok(evidence),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded D2 session keeps the worker, ring, renderer, controls, and samples causally adjacent"
)]
async fn exercise_d2(
    client: &mut WorkerClient,
    mode: SoakMode,
    config: SoakConfig,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &SourceSet,
) -> TestResult<ModeEvidence> {
    configure_session(client).await?;
    let worker_environment = inspect_runtime(client, pack).await?;
    load_codec(client, pack, decoder).await?;
    let mut controls = d2_controls(mode);
    let loaded = client
        .deck_d2_load(
            D2Load {
                deck_id: D2_DECK_ID.to_owned(),
                operator_id: D2_OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: d2_source_binding(&sources.b)?,
                source_b: d2_source_binding(&sources.c)?,
                controls: controls.clone(),
                transport: D2Transport::default(),
                seed: SOAK_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        loaded.source_a.archive_sha256 == sources.b.archive_sha256
            && loaded.source_b.archive_sha256 == sources.c.archive_sha256,
        "D2 worker changed a private source identity",
    )?;
    let descriptor = RingDescriptor::new(448, 800, INITIAL_GENERATION)?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;
    let mut renderer = OffscreenRenderer::new(448, 800).await?;
    let renderer_adapter = renderer.adapter_name();
    let temporary = tempdir()?;
    let partial_files_before = count_partial_files(temporary.path())?;
    let mut measurements = MeasurementState::new(config)?;
    let initial_resources = sample_resources(client, &measurements).await?;
    measurements.resources.push(initial_resources);
    let session_outbound_budget_start = client.remaining_outbound_message_budget();
    let session_inbound_budget_start = client.remaining_inbound_message_budget();
    let mut generation = INITIAL_GENERATION;
    let mut next_resource = config.resource_interval;
    let mut next_control = config.warmup.max(config.control_interval);
    let mut pending_control: Option<PendingControl> = None;
    let mut toggle = false;

    while measurements.elapsed() < config.duration {
        sample_if_due(
            client,
            &mut measurements,
            &mut next_resource,
            config.resource_interval,
        )
        .await?;
        if pending_control.is_none()
            && measurements.elapsed() >= next_control
            && enough_time_for_effect(config.duration, measurements.elapsed())
        {
            let started = Instant::now();
            toggle = !toggle;
            let expected_value = if matches!(mode, SoakMode::D2Linear) {
                if toggle { 0.65 } else { 0.35 }
            } else if toggle {
                0.85
            } else {
                0.60
            };
            if matches!(mode, SoakMode::D2Linear) {
                controls.mix = finite(expected_value)?;
            } else {
                controls.interaction = finite(expected_value)?;
            }
            let ack = client
                .deck_d2_controls_set(
                    D2ControlsSet {
                        deck_id: D2_DECK_ID.to_owned(),
                        deck_revision: loaded.deck_revision,
                        controls: controls.clone(),
                    },
                    COMMAND_TIMEOUT,
                )
                .await?;
            require(
                ack.controls == controls && !ack.requires_causal_reset,
                "D2 control event was not applied atomically",
            )?;
            pending_control = Some(PendingControl {
                started,
                expected_value,
            });
            advance_periodic_deadline(
                &mut next_control,
                config.control_interval,
                measurements.elapsed(),
            )?;
        }

        let before = owner.state()?;
        require(
            before.occupancy() == 0,
            "D2 scheduled decode into a nonempty ring",
        )?;
        let ack = client
            .deck_d2_process_slot(
                D2ProcessSlot {
                    deck_id: D2_DECK_ID.to_owned(),
                    deck_revision: loaded.deck_revision,
                    stream_generation: generation,
                },
                COMMAND_TIMEOUT,
            )
            .await?;
        match ack {
            D2ProcessSlotAck::DecodedSlot {
                deck_id,
                deck_revision,
                stream_generation,
                decoded_frame_count,
                ring_first_sequence,
                ring_last_sequence_exclusive,
                provenance_json,
                ..
            } => {
                require(
                    deck_id == D2_DECK_ID
                        && deck_revision == loaded.deck_revision
                        && stream_generation == generation
                        && ring_last_sequence_exclusive
                            == ring_first_sequence + u64::from(decoded_frame_count),
                    "D2 decoded receipt changed session identity or ring range",
                )?;
                validate_d2_provenance(&provenance_json, mode, &controls)?;
                let control_started =
                    take_matching_d2_control(&provenance_json, mode, &mut pending_control)?;
                measurements.host_max_ring_occupancy = measurements
                    .host_max_ring_occupancy
                    .max(owner.state()?.occupancy());
                present_batch(
                    &mut consumer,
                    &mut renderer,
                    &mut measurements,
                    generation,
                    ring_first_sequence,
                    ring_last_sequence_exclusive,
                    control_started,
                )
                .await?;
            }
            D2ProcessSlotAck::ResetBarrier {
                deck_id,
                deck_revision,
                current_generation,
                minimum_new_generation,
                reasons,
            } => {
                require(
                    deck_id == D2_DECK_ID
                        && deck_revision == loaded.deck_revision
                        && current_generation == generation
                        && minimum_new_generation > current_generation,
                    "D2 loop barrier changed the active causal stream",
                )?;
                let reset = client
                    .deck_d2_reset(
                        D2Reset {
                            deck_id: D2_DECK_ID.to_owned(),
                            deck_revision: loaded.deck_revision,
                            new_stream_generation: minimum_new_generation,
                        },
                        COMMAND_TIMEOUT,
                    )
                    .await?;
                require(
                    reset.stream_generation == minimum_new_generation
                        && reset.reasons == reasons
                        && reset.causal_state_cleared,
                    "D2 reset did not clear the exact causal loop barrier",
                )?;
                generation = reset.stream_generation;
                owner.adopt_generation(generation)?;
                consumer.adopt_generation(generation)?;
                require_zero_ring(&owner, &consumer)?;
                measurements.clock.restart();
                measurements.reset_count += 1;
            }
            D2ProcessSlotAck::Paused { .. } => {
                return failure("D2 soak worker paused while transport was active");
            }
        }
    }
    require(
        pending_control.is_none(),
        "D2 soak ended before a control effect reached a decoded frame",
    )?;
    require_zero_occupancy(&owner, &consumer)?;
    let final_resources = sample_resources(client, &measurements).await?;
    measurements.resources.push(final_resources);
    renderer.finish()?;
    let partial_files_after = count_partial_files(temporary.path())?;
    finish_evidence(
        measurements,
        &renderer,
        renderer_adapter,
        session_outbound_budget_start,
        client.remaining_outbound_message_budget(),
        session_inbound_budget_start,
        client.remaining_inbound_message_budget(),
        partial_files_before,
        partial_files_after,
        worker_environment,
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded Q4 session keeps worker, four slots, ring, renderer, controls, and samples causally adjacent"
)]
async fn exercise_q4(
    client: &mut WorkerClient,
    mode: SoakMode,
    config: SoakConfig,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &SourceSet,
) -> TestResult<ModeEvidence> {
    configure_session(client).await?;
    let worker_environment = inspect_runtime(client, pack).await?;
    load_codec(client, pack, decoder).await?;
    let slots = sources.q4_slots();
    let roles = Q4Roles::default();
    let mut controls = q4_controls(mode);
    let loaded = client
        .deck_q4_load(
            Q4Load {
                deck_id: Q4_DECK_ID.to_owned(),
                operator_id: Q4_OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: q4_source_binding(&slots[0])?,
                source_b: q4_source_binding(&slots[1])?,
                source_c: q4_source_binding(&slots[2])?,
                source_d: q4_source_binding(&slots[3])?,
                roles,
                controls: controls.clone(),
                transport: Q4Transport::default(),
                seed: SOAK_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        loaded.source_a.archive_sha256 == slots[0].archive_sha256
            && loaded.source_b.archive_sha256 == slots[1].archive_sha256
            && loaded.source_c.archive_sha256 == slots[2].archive_sha256
            && loaded.source_d.archive_sha256 == slots[3].archive_sha256,
        "Q4 worker changed the B,C,A,B source topology",
    )?;
    let descriptor = RingDescriptor::new(448, 800, INITIAL_GENERATION)?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;
    let mut renderer = OffscreenRenderer::new(448, 800).await?;
    let renderer_adapter = renderer.adapter_name();
    let temporary = tempdir()?;
    let partial_files_before = count_partial_files(temporary.path())?;
    let mut measurements = MeasurementState::new(config)?;
    let initial_resources = sample_resources(client, &measurements).await?;
    measurements.resources.push(initial_resources);
    let session_outbound_budget_start = client.remaining_outbound_message_budget();
    let session_inbound_budget_start = client.remaining_inbound_message_budget();
    let mut generation = INITIAL_GENERATION;
    let mut next_resource = config.resource_interval;
    let mut next_control = config.warmup.max(config.control_interval);
    let mut pending_control: Option<PendingControl> = None;
    let mut toggle = false;

    while measurements.elapsed() < config.duration {
        sample_if_due(
            client,
            &mut measurements,
            &mut next_resource,
            config.resource_interval,
        )
        .await?;
        if pending_control.is_none()
            && measurements.elapsed() >= next_control
            && enough_time_for_effect(config.duration, measurements.elapsed())
        {
            let started = Instant::now();
            toggle = !toggle;
            let expected_value = if toggle { 0.85 } else { 0.60 };
            controls.interaction = finite(expected_value)?;
            let ack = client
                .deck_q4_controls_set(
                    Q4ControlsSet {
                        deck_id: Q4_DECK_ID.to_owned(),
                        deck_revision: loaded.deck_revision,
                        controls: controls.clone(),
                    },
                    COMMAND_TIMEOUT,
                )
                .await?;
            require(
                ack.controls == controls && !ack.requires_causal_reset,
                "Q4 control event was not applied atomically",
            )?;
            pending_control = Some(PendingControl {
                started,
                expected_value,
            });
            advance_periodic_deadline(
                &mut next_control,
                config.control_interval,
                measurements.elapsed(),
            )?;
        }

        let before = owner.state()?;
        require(
            before.occupancy() == 0,
            "Q4 scheduled decode into a nonempty ring",
        )?;
        let ack = client
            .deck_q4_process_slot(
                Q4ProcessSlot {
                    deck_id: Q4_DECK_ID.to_owned(),
                    deck_revision: loaded.deck_revision,
                    stream_generation: generation,
                },
                COMMAND_TIMEOUT,
            )
            .await?;
        match ack {
            Q4ProcessSlotAck::DecodedSlot {
                deck_id,
                deck_revision,
                stream_generation,
                roles: ack_roles,
                decoded_frame_count,
                ring_first_sequence,
                ring_last_sequence_exclusive,
                provenance_json,
                ..
            } => {
                require(
                    deck_id == Q4_DECK_ID
                        && deck_revision == loaded.deck_revision
                        && stream_generation == generation
                        && ack_roles == roles
                        && ring_last_sequence_exclusive
                            == ring_first_sequence + u64::from(decoded_frame_count),
                    "Q4 decoded receipt changed session identity, roles, or ring range",
                )?;
                validate_q4_provenance(&provenance_json, mode, &controls)?;
                let control_started =
                    take_matching_q4_control(&provenance_json, &mut pending_control)?;
                measurements.host_max_ring_occupancy = measurements
                    .host_max_ring_occupancy
                    .max(owner.state()?.occupancy());
                present_batch(
                    &mut consumer,
                    &mut renderer,
                    &mut measurements,
                    generation,
                    ring_first_sequence,
                    ring_last_sequence_exclusive,
                    control_started,
                )
                .await?;
            }
            Q4ProcessSlotAck::ResetBarrier {
                deck_id,
                deck_revision,
                current_generation,
                minimum_new_generation,
                reasons,
            } => {
                require(
                    deck_id == Q4_DECK_ID
                        && deck_revision == loaded.deck_revision
                        && current_generation == generation
                        && minimum_new_generation > current_generation,
                    "Q4 loop barrier changed the active causal stream",
                )?;
                let reset = client
                    .deck_q4_reset(
                        Q4Reset {
                            deck_id: Q4_DECK_ID.to_owned(),
                            deck_revision: loaded.deck_revision,
                            new_stream_generation: minimum_new_generation,
                        },
                        COMMAND_TIMEOUT,
                    )
                    .await?;
                require(
                    reset.stream_generation == minimum_new_generation
                        && reset.reasons == reasons
                        && reset.causal_state_cleared,
                    "Q4 reset did not clear the exact causal loop barrier",
                )?;
                generation = reset.stream_generation;
                owner.adopt_generation(generation)?;
                consumer.adopt_generation(generation)?;
                require_zero_ring(&owner, &consumer)?;
                measurements.clock.restart();
                measurements.reset_count += 1;
            }
            Q4ProcessSlotAck::Paused { .. } => {
                return failure("Q4 soak worker paused while transport was active");
            }
        }
    }
    require(
        pending_control.is_none(),
        "Q4 soak ended before a control effect reached a decoded frame",
    )?;
    require_zero_occupancy(&owner, &consumer)?;
    let final_resources = sample_resources(client, &measurements).await?;
    measurements.resources.push(final_resources);
    renderer.finish()?;
    let partial_files_after = count_partial_files(temporary.path())?;
    finish_evidence(
        measurements,
        &renderer,
        renderer_adapter,
        session_outbound_budget_start,
        client.remaining_outbound_message_budget(),
        session_inbound_budget_start,
        client.remaining_inbound_message_budget(),
        partial_files_before,
        partial_files_after,
        worker_environment,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_evidence(
    measurements: MeasurementState,
    renderer: &OffscreenRenderer,
    renderer_adapter: String,
    session_outbound_budget_start: usize,
    session_outbound_budget_end: usize,
    session_inbound_budget_start: usize,
    session_inbound_budget_end: usize,
    partial_files_before: u64,
    partial_files_after: u64,
    worker_environment: WorkerEnvironmentEvidence,
) -> TestResult<ModeEvidence> {
    require(
        renderer.submitted_frames == measurements.rendered_frames,
        "DX12 renderer submission count differs from presented RGB frames",
    )?;
    let frame_summary = measurements.frames.summarize()?;
    require(
        !measurements.control_latencies.is_empty(),
        "soak did not measure any control-to-effect event",
    )?;
    Ok(ModeEvidence {
        elapsed: measurements.elapsed(),
        frame_summary,
        rendered_frames: measurements.rendered_frames,
        frame_checksum: measurements.frame_checksum,
        control_latencies: measurements.control_latencies,
        resources: measurements.resources,
        host_max_ring_occupancy: measurements.host_max_ring_occupancy,
        reset_count: measurements.reset_count,
        session_outbound_budget_start,
        session_outbound_budget_end,
        session_inbound_budget_start,
        session_inbound_budget_end,
        renderer_adapter,
        renderer_final_poll_completed: true,
        partial_files_before,
        partial_files_after,
        worker_environment,
    })
}

async fn present_batch(
    consumer: &mut WindowsRgbRingConsumer,
    renderer: &mut OffscreenRenderer,
    measurements: &mut MeasurementState,
    generation: u64,
    first_sequence: u64,
    last_sequence_exclusive: u64,
    control_started: Option<Instant>,
) -> TestResult<()> {
    for (offset, expected_sequence) in (first_sequence..last_sequence_exclusive).enumerate() {
        let deadline = measurements.clock.next_deadline()?;
        sleep_until(deadline).await;
        let now = Instant::now();
        measurements.clock.advance_past(now)?;
        let ReadStatus::Frame(frame) = consumer.try_read()? else {
            return failure("worker receipt claimed an RGB frame missing from the ring");
        };
        require(
            frame.generation() == generation
                && frame.sequence() == expected_sequence
                && frame.width() == 448
                && frame.height() == 800,
            "RGB ring frame differs from the decoded-slot receipt",
        )?;
        renderer.render(&frame)?;
        measurements.rendered_frames = measurements
            .rendered_frames
            .checked_add(1)
            .ok_or_else(|| io::Error::other("presented frame counter overflowed"))?;
        let bytes = frame.padded_rgba();
        let sample = u64::from(bytes[0])
            | (u64::from(bytes[bytes.len() / 2]) << 8)
            | (u64::from(bytes[bytes.len() - 1]) << 16)
            | (expected_sequence << 24);
        measurements.frame_checksum = measurements.frame_checksum.rotate_left(7) ^ sample;
        measurements.frames.record(measurements.elapsed());
        if offset == 0
            && let Some(started) = control_started
        {
            measurements.control_latencies.push(started.elapsed());
        }
    }
    require(
        matches!(consumer.try_read()?, ReadStatus::Empty),
        "worker published RGB frames outside its declared ring range",
    )
}

async fn sample_if_due(
    client: &mut WorkerClient,
    measurements: &mut MeasurementState,
    next_resource: &mut Duration,
    interval: Duration,
) -> TestResult<()> {
    if measurements.elapsed() < *next_resource {
        return Ok(());
    }
    let sample = sample_resources(client, measurements).await?;
    measurements.resources.push(sample);
    advance_periodic_deadline(next_resource, interval, measurements.elapsed())
}

async fn sample_resources(
    client: &mut WorkerClient,
    measurements: &MeasurementState,
) -> TestResult<ResourceSample> {
    let worker_metrics = metrics_get(client).await?;
    let worker_memory = client
        .with_process_handle(|process| query_process_memory(process.as_raw_handle().cast()))??;
    let host_memory = query_current_process_memory()?;
    Ok(ResourceSample {
        elapsed: measurements.elapsed(),
        worker_memory,
        host_memory,
        worker_metrics,
    })
}

async fn metrics_get(client: &mut WorkerClient) -> TestResult<MetricsSnapshot> {
    let ack = client
        .call(Command::MetricsGet(EmptyPayload {}), COMMAND_TIMEOUT)
        .await?;
    let Ack::MetricsGet(metrics) = ack else {
        return failure("worker returned the wrong metrics.get acknowledgement");
    };
    Ok(metrics)
}

fn query_current_process_memory() -> TestResult<ProcessMemory> {
    // SAFETY: this is the documented non-owning pseudo handle and requires no close.
    #[allow(unsafe_code)]
    let handle = unsafe { GetCurrentProcess() };
    Ok(query_process_memory(handle)?)
}

fn query_process_memory(handle: HANDLE) -> io::Result<ProcessMemory> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>())
            .map_err(|_| io::Error::other("process counter size overflowed"))?,
        ..Default::default()
    };
    // SAFETY: the initialized EX structure is passed as its documented base
    // pointer with the exact EX byte size; the process handle is borrowed.
    #[allow(unsafe_code)]
    let success = unsafe {
        GetProcessMemoryInfo(
            handle,
            std::ptr::from_mut(&mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
            counters.cb,
        )
    };
    if success == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcessMemory {
        working_set_bytes: u64::try_from(counters.WorkingSetSize)
            .map_err(|_| io::Error::other("working set does not fit u64"))?,
        private_usage_bytes: u64::try_from(counters.PrivateUsage)
            .map_err(|_| io::Error::other("private usage does not fit u64"))?,
    })
}

fn take_matching_d2_control(
    provenance_json: &str,
    mode: SoakMode,
    pending: &mut Option<PendingControl>,
) -> TestResult<Option<Instant>> {
    let Some(control) = pending.as_ref() else {
        return Ok(None);
    };
    let provenance: Value = serde_json::from_str(provenance_json)?;
    let pointer = if matches!(mode, SoakMode::D2Linear) {
        "/operation/controls/mix"
    } else {
        "/operation/controls/interaction"
    };
    let observed = provenance.pointer(pointer).and_then(Value::as_f64);
    require(
        observed.is_some_and(|value| (value - control.expected_value).abs() <= 1e-12),
        "D2 control acknowledgement did not reach the next decoded provenance",
    )?;
    Ok(pending.take().map(|value| value.started))
}

fn take_matching_q4_control(
    provenance_json: &str,
    pending: &mut Option<PendingControl>,
) -> TestResult<Option<Instant>> {
    let Some(control) = pending.as_ref() else {
        return Ok(None);
    };
    let provenance: Value = serde_json::from_str(provenance_json)?;
    let observed = provenance
        .pointer("/operation/controls/interaction")
        .and_then(Value::as_f64);
    require(
        observed.is_some_and(|value| (value - control.expected_value).abs() <= 1e-12),
        "Q4 control acknowledgement did not reach the next decoded provenance",
    )?;
    Ok(pending.take().map(|value| value.started))
}

fn validate_d2_provenance(
    provenance_json: &str,
    mode: SoakMode,
    controls: &D2Controls,
) -> TestResult<()> {
    let value: Value = serde_json::from_str(provenance_json)?;
    require(
        value
            .pointer("/operation/operator_id")
            .and_then(Value::as_str)
            == Some(D2_OPERATOR_ID)
            && value
                .pointer("/operation/controls/algorithm")
                .and_then(Value::as_str)
                == Some(mode.algorithm())
            && value.pointer("/operation/seed").and_then(Value::as_u64) == Some(SOAK_SEED),
        "D2 provenance changed operator, mode, or seed",
    )?;
    if let Some(routing) = mode.routing() {
        require(
            value
                .pointer("/operation/controls/xs5_routing")
                .and_then(Value::as_str)
                == Some(routing)
                && controls.algorithm == D2Algorithm::Xs5,
            "D2 XS5 provenance changed its routing mode",
        )?;
    }
    Ok(())
}

fn validate_q4_provenance(
    provenance_json: &str,
    mode: SoakMode,
    controls: &Q4Controls,
) -> TestResult<()> {
    let value: Value = serde_json::from_str(provenance_json)?;
    require(
        value
            .pointer("/operation/operator_id")
            .and_then(Value::as_str)
            == Some(Q4_OPERATOR_ID)
            && value
                .pointer("/operation/controls/algorithm")
                .and_then(Value::as_str)
                == Some("XS5")
            && value
                .pointer("/operation/controls/xs5_routing")
                .and_then(Value::as_str)
                == mode.routing()
            && value.pointer("/operation/seed").and_then(Value::as_u64) == Some(SOAK_SEED)
            && controls.algorithm == Q4Algorithm::Xs5,
        "Q4 provenance changed operator, routing, or seed",
    )
}

fn d2_controls(mode: SoakMode) -> D2Controls {
    let mut controls = D2Controls::default();
    if matches!(mode, SoakMode::D2Xs5) {
        controls.algorithm = D2Algorithm::Xs5;
        controls.mode = D2Mode::Interact;
        controls.interaction = FiniteF64::new(0.8).expect("finite first-party control");
        controls.xs5_routing = D2Xs5Routing::TopK;
        controls.chaos = FiniteF64::new(0.0).expect("finite first-party control");
        controls.top_k = 8;
    }
    controls
}

fn q4_controls(mode: SoakMode) -> Q4Controls {
    Q4Controls {
        algorithm: Q4Algorithm::Xs5,
        interaction: FiniteF64::new(0.8).expect("finite first-party control"),
        mode: Q4Mode::Interact,
        preserve: FiniteF64::new(0.25).expect("finite first-party control"),
        influence_mode: Q4InfluenceMode::Manual,
        donor_weight_b: FiniteF64::new(0.6).expect("finite first-party control"),
        donor_weight_c: FiniteF64::new(0.3).expect("finite first-party control"),
        donor_weight_d: FiniteF64::new(0.1).expect("finite first-party control"),
        triangle_x: FiniteF64::new(0.5).expect("finite first-party control"),
        triangle_y: FiniteF64::new(0.5).expect("finite first-party control"),
        xs5_routing: match mode {
            SoakMode::Q4TopK => Q4Xs5Routing::TopK,
            SoakMode::Q4Sinkhorn => Q4Xs5Routing::Sinkhorn,
            SoakMode::D2Linear | SoakMode::D2Xs5 => unreachable!("Q4 mode required"),
        },
        temperature: FiniteF64::new(0.2).expect("finite first-party control"),
        top_k: 8,
        sinkhorn_iterations: 5,
        chaos: FiniteF64::new(0.0).expect("finite first-party control"),
    }
}

async fn configure_session(client: &mut WorkerClient) -> TestResult<()> {
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
        .await?;
    let Ack::SessionConfigure(configured) = ack else {
        return failure("worker returned the wrong session.configure acknowledgement");
    };
    require(
        configured.selected_protocol_version == request.selected_protocol_version
            && configured.max_frame_bytes == request.max_frame_bytes
            && configured.max_inflight_decode_batches == 1,
        "worker changed the bounded soak session contract",
    )
}

async fn inspect_runtime(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
) -> TestResult<WorkerEnvironmentEvidence> {
    let ack = client
        .call(Command::CodecInspect(EmptyPayload {}), COMMAND_TIMEOUT)
        .await?;
    let Ack::CodecInspect(inspection) = ack else {
        return failure("worker returned the wrong codec.inspect acknowledgement");
    };
    let device = inspection
        .devices
        .iter()
        .find(|device| device.ordinal == 0)
        .cloned()
        .ok_or_else(|| io::Error::other("realtime soak requires CUDA device ordinal 0"))?;
    require(inspection.cuda_available, "realtime soak requires CUDA")?;
    require(
        inspection.adapters.iter().any(|adapter| {
            adapter.adapter_id == pack.manifest.adapter.adapter_id
                && adapter.adapter_version == pack.manifest.adapter.adapter_version
                && adapter
                    .profiles
                    .iter()
                    .any(|profile| profile == &h3_profile())
        }),
        "worker did not advertise the validated soak adapter/profile",
    )?;
    let torch_version = inspection
        .torch_version
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::other("worker omitted its PyTorch version"))?;
    let cuda_runtime = inspection
        .cuda_runtime
        .filter(|value| !value.is_empty())
        .ok_or_else(|| io::Error::other("worker omitted its CUDA runtime version"))?;
    require(
        device.total_memory_bytes > 0 && !device.name.is_empty(),
        "worker CUDA device evidence is incomplete",
    )?;
    Ok(WorkerEnvironmentEvidence {
        torch_version,
        cuda_runtime,
        device,
    })
}

async fn load_codec(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
) -> TestResult<()> {
    let request = CodecLoad {
        pack_id: pack.manifest.pack_id.clone(),
        pack_version: pack.manifest.pack_version.clone(),
        adapter_id: pack.manifest.adapter.adapter_id.clone(),
        profile: h3_profile(),
        device_ordinal: 0,
        assets: BoundedVec::try_from_vec(vec![ExternalAssetBinding {
            asset_id: decoder.asset_id.clone(),
            path: path_text(&decoder.path)?,
            sha256: decoder.sha256.clone(),
            byte_length: decoder.byte_length,
        }])?,
    };
    let ack = client
        .call(Command::CodecLoad(request.clone()), COMMAND_TIMEOUT)
        .await?;
    let Ack::CodecLoad(loaded) = ack else {
        return failure("worker returned the wrong codec.load acknowledgement");
    };
    require(
        loaded.pack_id == request.pack_id
            && loaded.pack_version == request.pack_version
            && loaded.adapter_id == request.adapter_id
            && loaded.adapter_version == pack.manifest.adapter.adapter_version
            && loaded.profile == request.profile
            && loaded.device.ordinal == 0,
        "worker loaded a codec identity different from the validated soak selection",
    )
}

async fn bind_ring(client: &mut WorkerClient, owner: &WindowsRgbRingOwner) -> TestResult<()> {
    let binding = client.with_process_handle(|process| owner.duplicate_into(process))??;
    let request = RingBind {
        layout_version: 1,
        mapping_handle: binding.mapping_handle(),
        mapping_bytes: binding.mapping_bytes(),
        frames_ready_event_handle: binding.frames_ready_event_handle(),
        ring_id: WireUuid::new_v4(),
    };
    let ack = client
        .call(Command::RingBind(request.clone()), COMMAND_TIMEOUT)
        .await?;
    let Ack::RingBind(bound) = ack else {
        return failure("worker returned the wrong ring.bind acknowledgement");
    };
    require(
        bound.layout_version == request.layout_version
            && bound.mapping_bytes == request.mapping_bytes
            && bound.ring_id == request.ring_id,
        "worker changed the anonymous RGB ring binding",
    )
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the path-free receipt keeps its exact inputs, raw measurements, and every derived release gate adjacent"
)]
fn build_receipt(
    mode: SoakMode,
    config: SoakConfig,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &SourceSet,
    execution_context: &ExecutionContext,
    codec_runtime_inputs: &CodecRuntimeInputs,
    evidence: &ModeEvidence,
) -> TestResult<Value> {
    validate_resource_sample_coverage(
        &evidence
            .resources
            .iter()
            .map(|sample| sample.elapsed)
            .collect::<Vec<_>>(),
        config.warmup,
        config.duration,
        config.resource_interval,
        config.release_duration_exercised(),
    )?;
    let post_warmup = evidence
        .resources
        .iter()
        .filter(|sample| sample.elapsed >= config.warmup)
        .cloned()
        .collect::<Vec<_>>();
    require(
        !post_warmup.is_empty(),
        "soak has no post-warmup resource measurements",
    )?;
    let first_post_warmup_elapsed = post_warmup
        .first()
        .ok_or_else(|| io::Error::other("soak lost its first post-warmup resource sample"))?
        .elapsed;
    let last_post_warmup_elapsed = post_warmup
        .last()
        .ok_or_else(|| io::Error::other("soak lost its last post-warmup resource sample"))?
        .elapsed;
    let maximum_post_warmup_gap = post_warmup
        .windows(2)
        .map(|pair| pair[1].elapsed.saturating_sub(pair[0].elapsed))
        .max()
        .unwrap_or(Duration::ZERO);
    let total_resource_samples = u64::try_from(evidence.resources.len())
        .map_err(|_| io::Error::other("resource sample count overflowed"))?;
    let post_warmup_resource_samples = u64::try_from(post_warmup.len())
        .map_err(|_| io::Error::other("post-warmup resource sample count overflowed"))?;
    let release_minimum_resource_samples = u64::try_from(MIN_RELEASE_RESOURCE_SAMPLES)
        .map_err(|_| io::Error::other("release resource sample minimum overflowed"))?;
    let worker_working_set = summarize_resource(&post_warmup, |sample| {
        Some(sample.worker_memory.working_set_bytes)
    })?;
    let worker_private = summarize_resource(&post_warmup, |sample| {
        Some(sample.worker_memory.private_usage_bytes)
    })?;
    let host_working_set = summarize_resource(&post_warmup, |sample| {
        Some(sample.host_memory.working_set_bytes)
    })?;
    let host_private = summarize_resource(&post_warmup, |sample| {
        Some(sample.host_memory.private_usage_bytes)
    })?;
    let torch_allocated = summarize_resource(&post_warmup, |sample| {
        sample.worker_metrics.gpu_allocated_bytes
    })?;
    let torch_reserved = summarize_resource(&post_warmup, |sample| {
        sample.worker_metrics.gpu_reserved_bytes
    })?;
    let control_p95 = percentile_95(&evidence.control_latencies)?;
    let first_metrics = &evidence
        .resources
        .first()
        .ok_or_else(|| io::Error::other("soak lost initial worker metrics"))?
        .worker_metrics;
    let last_metrics = &evidence
        .resources
        .last()
        .ok_or_else(|| io::Error::other("soak lost final worker metrics"))?
        .worker_metrics;
    let decoded_frames_delta = checked_counter_delta(
        last_metrics.decoded_frames_total,
        first_metrics.decoded_frames_total,
    )?;
    let decoded_batches_delta = checked_counter_delta(
        last_metrics.decode_batches_total,
        first_metrics.decode_batches_total,
    )?;
    let backpressure_delta = checked_counter_delta(
        last_metrics.ring_backpressure_total,
        first_metrics.ring_backpressure_total,
    )?;
    let presentation_skipped_delta = checked_counter_delta(
        last_metrics.presentation_skipped_total,
        first_metrics.presentation_skipped_total,
    )?;
    require(
        decoded_frames_delta == evidence.rendered_frames,
        "worker decoded-frame delta differs from DX12 render submissions",
    )?;

    let fps_pass = (FPS_MINIMUM..=FPS_MAXIMUM).contains(&evidence.frame_summary.output_fps);
    let interval_pass =
        evidence.frame_summary.intervals_over_two_frames_rate < LONG_INTERVAL_RATE_LIMIT;
    let control_pass = control_p95 <= CONTROL_PROCESSED_FRAME_P95_LIMIT;
    let queue_pass = backpressure_delta == 0
        && presentation_skipped_delta == 0
        && last_metrics.ring_occupancy == 0
        && evidence.host_max_ring_occupancy <= 4;
    let partial_pass = evidence.partial_files_before == 0 && evidence.partial_files_after == 0;
    let worker_ram_growth = growth_assessment(&worker_private);
    let host_ram_growth = growth_assessment(&host_private);
    let torch_allocated_growth = growth_assessment(&torch_allocated);
    let torch_reserved_growth = growth_assessment(&torch_reserved);
    let memory_pass = !worker_ram_growth.progressive_growth_detected
        && !host_ram_growth.progressive_growth_detected
        && !torch_allocated_growth.progressive_growth_detected
        && !torch_reserved_growth.progressive_growth_detected;
    let release_profile_pass = !cfg!(debug_assertions);
    let all_required_gates_passed = config.release_duration_exercised()
        && release_profile_pass
        && fps_pass
        && interval_pass
        && control_pass
        && queue_pass
        && partial_pass
        && memory_pass;

    let q4_slots = sources.q4_slots();
    let source_entries = if mode.is_d2() {
        vec![
            source_evidence("A", "B", &sources.b),
            source_evidence("B", "C", &sources.c),
        ]
    } else {
        ["B", "C", "A", "B"]
            .into_iter()
            .enumerate()
            .map(|(index, logical)| source_evidence(slot_label(index), logical, &q4_slots[index]))
            .collect()
    };
    let source_order = if mode.is_d2() {
        json!(["B", "C"])
    } else {
        json!(["B", "C", "A", "B"])
    };
    let duplicate_label = if mode.is_d2() {
        Value::Null
    } else {
        json!(
            "slot D intentionally reuses logical cartridge B; 3 distinct real AV sources across 4 slots"
        )
    };
    let independent_acceptance = if mode.is_d2() {
        Value::Null
    } else {
        json!(false)
    };
    let gate_evaluation = if config.release_duration_exercised() {
        "full_duration"
    } else {
        "short_override_not_release_acceptance"
    };

    Ok(json!({
        "schema_version": 2,
        "evidence_kind": "latentdeck_private_realtime_soak",
        "generated_at_unix_seconds": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        "mode": mode.name(),
        "deck": mode.deck(),
        "algorithm": mode.algorithm(),
        "xs5_routing": mode.routing(),
        "execution_context": execution_context,
        "codec_runtime_inputs": codec_runtime_inputs.as_value(),
        "configuration": {
            "duration_seconds": config.duration.as_secs_f64(),
            "warmup_seconds": config.warmup.as_secs_f64(),
            "control_interval_seconds": config.control_interval.as_secs_f64(),
            "resource_interval_seconds": config.resource_interval.as_secs_f64(),
            "release_default_duration_seconds": RELEASE_DURATION.as_secs(),
            "release_duration_exercised": config.release_duration_exercised(),
            "target_frame_rate": {"numerator": 24, "denominator": 1}
        },
        "runtime": {
            "elapsed_seconds": evidence.elapsed.as_secs_f64(),
            "host_build_profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "codec_pack": {
                "pack_id": pack.manifest.pack_id,
                "pack_version": pack.manifest.pack_version,
                "adapter_id": pack.manifest.adapter.adapter_id,
                "adapter_version": pack.manifest.adapter.adapter_version
            },
            "decoder": {
                "asset_id": decoder.asset_id,
                "sha256": decoder.sha256,
                "byte_length": decoder.byte_length
            },
            "seed": SOAK_SEED,
            "causal_reset_count": evidence.reset_count,
            "worker_environment": {
                "torch_version": evidence.worker_environment.torch_version,
                "cuda_runtime": evidence.worker_environment.cuda_runtime,
                "device": {
                    "ordinal": evidence.worker_environment.device.ordinal,
                    "name": evidence.worker_environment.device.name,
                    "total_memory_bytes": evidence.worker_environment.device.total_memory_bytes
                }
            }
        },
        "sources": {
            "geometry": {
                "decoded_width": 448,
                "decoded_height": 800,
                "latent_width": 28,
                "latent_height": 50
            },
            "slot_order": source_order,
            "distinct_real_cartridges": if mode.is_d2() { 2 } else { 3 },
            "duplicate_label": duplicate_label,
            "four_independent_cartridge_acceptance": independent_acceptance,
            "entries": source_entries
        },
        "presentation": {
            "clock": "absolute_rational_24fps",
            "post_warmup_frames": evidence.frame_summary.frames,
            "post_warmup_intervals": evidence.frame_summary.intervals,
            "measured_output_fps": evidence.frame_summary.output_fps,
            "intervals_over_two_frames": evidence.frame_summary.intervals_over_two_frames,
            "intervals_over_two_frames_rate": evidence.frame_summary.intervals_over_two_frames_rate,
            "all_rendered_frames": evidence.rendered_frames,
            "frame_checksum": format!("{:016x}", evidence.frame_checksum)
        },
        "renderer": {
            "backend": "DX12",
            "target": "offscreen_rgba8_unorm",
            "pipeline": "LatentDeck RgbaFrameRenderer fullscreen triangle",
            "adapter": evidence.renderer_adapter,
            "submitted_frames": evidence.rendered_frames,
            "final_device_poll_completed": evidence.renderer_final_poll_completed
        },
        "control_to_processed_frame": {
            "definition": "controls.set command start to first DX12-submitted frame processed with the exact new value recorded in worker provenance",
            "samples": evidence.control_latencies.len(),
            "p95_ms": control_p95.as_secs_f64() * 1_000.0,
            "limit_ms": CONTROL_PROCESSED_FRAME_P95_LIMIT.as_secs_f64() * 1_000.0
        },
        "resource_sampling": {
            "total_samples": total_resource_samples,
            "post_warmup_samples": post_warmup_resource_samples,
            "first_post_warmup_elapsed_seconds": first_post_warmup_elapsed.as_secs_f64(),
            "last_post_warmup_elapsed_seconds": last_post_warmup_elapsed.as_secs_f64(),
            "maximum_post_warmup_gap_seconds": maximum_post_warmup_gap.as_secs_f64(),
            "release_minimum_post_warmup_samples": release_minimum_resource_samples
        },
        "queue_and_backpressure": {
            "worker_decoded_batches_delta": decoded_batches_delta,
            "worker_decoded_frames_delta": decoded_frames_delta,
            "worker_ring_backpressure_delta": backpressure_delta,
            "worker_presentation_skipped_delta": presentation_skipped_delta,
            "host_max_ring_occupancy": evidence.host_max_ring_occupancy,
            "final_worker_ring_occupancy": last_metrics.ring_occupancy,
            "final_worker_ring_write_sequence": last_metrics.ring_write_sequence,
            "final_worker_ring_read_sequence": last_metrics.ring_read_sequence,
            "outbound_message_budget": {"start": evidence.session_outbound_budget_start, "end": evidence.session_outbound_budget_end},
            "inbound_message_budget": {"start": evidence.session_inbound_budget_start, "end": evidence.session_inbound_budget_end}
        },
        "memory": {
            "measurement_window": "post_warmup",
            "growth_rule": "progressive only when both end delta and least-squares hourly slope exceed max(64 MiB, 5 percent of start)",
            "worker_process_working_set": trend_evidence(&worker_working_set),
            "worker_process_private_usage": trend_with_assessment(&worker_private, worker_ram_growth),
            "host_process_working_set": trend_evidence(&host_working_set),
            "host_process_private_usage": trend_with_assessment(&host_private, host_ram_growth),
            "vram_scope": "worker torch CUDA allocator only; native renderer VRAM is not measured",
            "native_renderer_vram_measured": false,
            "torch_cuda_allocated": trend_with_assessment(&torch_allocated, torch_allocated_growth),
            "torch_cuda_reserved": trend_with_assessment(&torch_reserved, torch_reserved_growth)
        },
        "partial_cleanup": {
            "capture_or_resample_attempted": false,
            "scoped_partial_files_before": evidence.partial_files_before,
            "scoped_partial_files_after": evidence.partial_files_after,
            "clean": partial_pass
        },
        "release_gates": {
            "evaluation": gate_evaluation,
            "host_build_profile_is_release": release_profile_pass,
            "fps_23_9_to_24_1": fps_pass,
            "intervals_over_two_frames_below_0_5_percent": interval_pass,
            "control_to_processed_frame_p95_at_most_200ms": control_pass,
            "no_ring_backpressure_or_queue_growth": queue_pass,
            "no_progressive_ram_or_worker_allocator_vram_growth": memory_pass,
            "no_partial_files": partial_pass,
            "all_required_gates_passed": all_required_gates_passed
        },
        "privacy": {
            "receipt_is_path_free": true,
            "private_payload_embedded": false
        }
    }))
}

#[derive(Clone, Copy)]
struct GrowthAssessment {
    delta_threshold_bytes: u64,
    slope_threshold_bytes_per_hour: f64,
    progressive_growth_detected: bool,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "bounded byte thresholds are converted to f64 only for trend comparison"
)]
fn growth_assessment(trend: &ByteTrend) -> GrowthAssessment {
    let proportional = trend.start_bytes / 20;
    let threshold = MEMORY_ABSOLUTE_GROWTH_THRESHOLD.max(proportional);
    let signed_threshold = i64::try_from(threshold).unwrap_or(i64::MAX);
    GrowthAssessment {
        delta_threshold_bytes: threshold,
        slope_threshold_bytes_per_hour: threshold as f64,
        progressive_growth_detected: trend.end_minus_start_bytes > signed_threshold
            && trend.least_squares_bytes_per_hour > threshold as f64,
    }
}

fn trend_evidence(trend: &ByteTrend) -> Value {
    json!({
        "start_bytes": trend.start_bytes,
        "peak_bytes": trend.peak_bytes,
        "end_bytes": trend.end_bytes,
        "end_minus_start_bytes": trend.end_minus_start_bytes,
        "least_squares_bytes_per_hour": trend.least_squares_bytes_per_hour
    })
}

fn trend_with_assessment(trend: &ByteTrend, assessment: GrowthAssessment) -> Value {
    let mut value = trend_evidence(trend);
    let object = value
        .as_object_mut()
        .expect("trend evidence is always a JSON object");
    object.insert(
        "delta_threshold_bytes".to_owned(),
        json!(assessment.delta_threshold_bytes),
    );
    object.insert(
        "slope_threshold_bytes_per_hour".to_owned(),
        json!(assessment.slope_threshold_bytes_per_hour),
    );
    object.insert(
        "progressive_growth_detected".to_owned(),
        json!(assessment.progressive_growth_detected),
    );
    value
}

fn summarize_resource(
    samples: &[ResourceSample],
    select: impl Fn(&ResourceSample) -> Option<u64>,
) -> TestResult<ByteTrend> {
    let values = samples
        .iter()
        .map(|sample| {
            select(sample)
                .map(|bytes| ByteSample {
                    elapsed: sample.elapsed,
                    bytes,
                })
                .ok_or_else(|| io::Error::other("resource sample omitted a required byte counter"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(summarize_bytes(&values)?)
}

fn source_evidence(slot: &str, logical: &str, source: &PrivateSource) -> Value {
    json!({
        "slot": slot,
        "logical_source": logical,
        "cartridge_id": source.cartridge_id.to_string(),
        "archive_sha256": source.archive_sha256,
        "archive_byte_length": source.archive_bytes,
        "visual_latent_slots": source.profile.visual.latent_slots,
        "decoded_frame_count": source.profile.visual.decoded_frame_count,
        "audio_latent_slots": source.profile.audio.as_ref().map(|audio| audio.latent_slots)
    })
}

const fn slot_label(index: usize) -> &'static str {
    match index {
        0 => "A",
        1 => "B",
        2 => "C",
        3 => "D",
        _ => unreachable!(),
    }
}

fn select_pack(root: &Path, mode: SoakMode) -> TestResult<ValidatedCodecPack> {
    let mut candidates = discover_codec_packs(
        std::slice::from_ref(&root.to_path_buf()),
        latentdeck_core::product_version(),
    )?
    .into_iter()
    .filter(|pack| {
        pack.manifest.pack_id == PACK_ID
            && if mode.is_d2() {
                pack.manifest.worker.d2_arguments.is_some()
            } else {
                pack.manifest.worker.q4_arguments.is_some()
            }
    })
    .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let left = Version::parse(&left.manifest.pack_version).expect("validated pack SemVer");
        let right = Version::parse(&right.manifest.pack_version).expect("validated pack SemVer");
        left.cmp(&right)
    });
    candidates
        .pop()
        .ok_or_else(|| io::Error::other("discovery root has no compatible H3 soak worker").into())
}

fn validate_self_contained_pack(pack: &ValidatedCodecPack) -> TestResult<u64> {
    const MAX_CATALOG_BYTES: u64 = 16 * 1024 * 1024;
    const MAX_PACK_FILES: usize = 250_000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let catalog_path = pack.root.join(&pack.manifest.integrity.catalog_path);
    let catalog_metadata = fs::metadata(&catalog_path)?;
    require(
        catalog_metadata.is_file()
            && catalog_metadata.len() > 0
            && catalog_metadata.len() <= MAX_CATALOG_BYTES,
        "codec-pack integrity catalog is outside the bounded physical-pack contract",
    )?;
    let catalog: Value = serde_json::from_slice(&fs::read(&catalog_path)?)?;
    let entries = catalog
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other("codec-pack integrity catalog omitted its file list"))?;
    require(
        !entries.is_empty() && entries.len() <= MAX_PACK_FILES,
        "codec-pack integrity catalog file count is invalid",
    )?;
    let mut expected = BTreeSet::new();
    expected.insert("codec-pack.json".to_owned());
    expected.insert(portable_relative_path(
        &pack.root,
        &catalog_path,
        "codec-pack integrity catalog",
    )?);
    for entry in entries {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| io::Error::other("codec-pack catalog entry omitted its path"))?;
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
            || !expected.insert(path.replace('\\', "/"))
        {
            return failure("codec-pack catalog contains an unsafe or duplicate path");
        }
    }

    let mut actual = BTreeSet::new();
    let mut pending = vec![pack.root.clone()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            require(
                metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "physical codec pack contains a reparse point",
            )?;
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            require(
                metadata.is_file(),
                "physical codec pack contains a non-file entry",
            )?;
            let portable = portable_relative_path(&pack.root, &path, "codec-pack file")?;
            if !actual.insert(portable.clone()) || actual.len() > MAX_PACK_FILES + 2 {
                return failure("physical codec pack file inventory is invalid");
            }
            if portable.to_ascii_lowercase().ends_with(".pth")
                || portable.to_ascii_lowercase().ends_with("._pth")
            {
                validate_pack_path_file(&pack.root, &path)?;
            }
        }
    }
    require(
        actual == expected,
        "physical codec pack contains files outside its integrity catalog or omits cataloged files",
    )?;
    u64::try_from(entries.len())
        .map_err(|_| io::Error::other("codec-pack catalog file count overflowed").into())
}

fn portable_relative_path(root: &Path, path: &Path, label: &'static str) -> TestResult<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| io::Error::other(format!("{label} escaped its physical pack")))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn validate_pack_path_file(root: &Path, path: &Path) -> TestResult<()> {
    const MAX_PATH_FILE_BYTES: u64 = 64 * 1024;
    let metadata = fs::metadata(path)?;
    require(
        metadata.len() <= MAX_PATH_FILE_BYTES,
        "codec-pack Python path file is oversized",
    )?;
    let text = std::str::from_utf8(&fs::read(path)?)?.to_owned();
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("codec-pack Python path file has no parent"))?;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("import ") || line.starts_with("import\t") {
            return failure("physical codec pack may not execute Python path-file directives");
        }
        let relative = Path::new(line);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return failure("physical codec pack Python path file references an external path");
        }
        let resolved = fs::canonicalize(parent.join(relative))?;
        require(
            resolved.starts_with(root),
            "physical codec pack Python path entry escaped its root",
        )?;
    }
    Ok(())
}

fn validate_source(path: PathBuf) -> TestResult<PrivateSource> {
    let path = fs::canonicalize(path)?;
    let cartridge = open_validated(&path, &ValidationOptions::default())?;
    let cartridge_id = parse_wire_uuid(&cartridge.manifest().cartridge_id.0)?;
    Ok(PrivateSource {
        path,
        cartridge_id,
        archive_bytes: cartridge.receipt().archive_bytes,
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
        profile: cartridge.h3_profile().clone(),
    })
}

fn verify_execution_context_inputs(
    context: &ExecutionContext,
    decoder: &ValidatedExternalAsset,
    sources: &SourceSet,
) -> TestResult<()> {
    let measured_decoder = measure_file_identity(&decoder.path)?;
    require(
        context
            .decoder
            .matches(&decoder.sha256, decoder.byte_length)
            && context.decoder == measured_decoder,
        "execution context decoder identity differs from the validated external asset",
    )?;
    for (expected, source) in [
        (&context.sources.a, &sources.a),
        (&context.sources.b, &sources.b),
        (&context.sources.c, &sources.c),
    ] {
        let measured_source = measure_file_identity(&source.path)?;
        require(
            expected.matches(&source.archive_sha256, source.archive_bytes)
                && *expected == measured_source,
            "execution context source identity differs from the validated cartridge",
        )?;
    }
    let current_executable = env::current_exe()?;
    let measured_executable = measure_file_identity(&current_executable)?;
    require(
        context.test_binary == measured_executable,
        "execution context test-binary identity differs from the running executable",
    )
}

fn measure_file_identity(path: &Path) -> TestResult<FileIdentity> {
    let measured = hash_path(path)?;
    Ok(FileIdentity {
        sha256: measured.sha256.to_string(),
        byte_length: measured.byte_length,
    })
}

fn d2_source_binding(source: &PrivateSource) -> TestResult<D2SourceBinding> {
    Ok(D2SourceBinding {
        cartridge_path: path_text(&source.path)?,
        cartridge_id: source.cartridge_id,
        expected_archive_sha256: source.archive_sha256.clone(),
    })
}

fn q4_source_binding(source: &PrivateSource) -> TestResult<Q4SourceBinding> {
    Ok(Q4SourceBinding {
        cartridge_path: path_text(&source.path)?,
        cartridge_id: source.cartridge_id,
        expected_archive_sha256: source.archive_sha256.clone(),
    })
}

fn require_zero_ring(
    owner: &WindowsRgbRingOwner,
    consumer: &WindowsRgbRingConsumer,
) -> TestResult<()> {
    let owner_state = owner.state()?;
    let consumer_state = consumer.state()?;
    require(
        owner_state.producer_sequence() == 0
            && owner_state.consumer_sequence() == 0
            && owner_state.occupancy() == 0
            && consumer_state.producer_sequence() == 0
            && consumer_state.consumer_sequence() == 0
            && consumer_state.occupancy() == 0,
        "causal reset did not clear RGB ring counters",
    )
}

fn require_zero_occupancy(
    owner: &WindowsRgbRingOwner,
    consumer: &WindowsRgbRingConsumer,
) -> TestResult<()> {
    require(
        owner.state()?.occupancy() == 0 && consumer.state()?.occupancy() == 0,
        "soak ended with a growing RGB queue",
    )
}

fn count_partial_files(root: &Path) -> io::Result<u64> {
    let mut count = 0_u64;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            count = count
                .checked_add(count_partial_files(&path)?)
                .ok_or_else(|| io::Error::other("partial file count overflowed"))?;
        } else if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("partial"))
        {
            count = count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("partial file count overflowed"))?;
        }
    }
    Ok(count)
}

fn advance_periodic_deadline(
    deadline: &mut Duration,
    interval: Duration,
    elapsed: Duration,
) -> TestResult<()> {
    while *deadline <= elapsed {
        *deadline = deadline
            .checked_add(interval)
            .ok_or_else(|| io::Error::other("periodic deadline overflowed"))?;
    }
    Ok(())
}

fn enough_time_for_effect(duration: Duration, elapsed: Duration) -> bool {
    duration.saturating_sub(elapsed) >= TARGET_FRAME_INTERVAL * 8
}

fn checked_counter_delta(end: u64, start: u64) -> TestResult<u64> {
    end.checked_sub(start)
        .ok_or_else(|| io::Error::other("worker metric counter moved backwards").into())
}

fn optional_seconds(name: &str) -> TestResult<Option<Duration>> {
    let Some(value) = env::var_os(name).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let text = value
        .to_str()
        .ok_or_else(|| io::Error::other(format!("{name} is not valid UTF-8")))?;
    let seconds = text
        .parse::<u64>()
        .map_err(|_| io::Error::other(format!("{name} must be an integer number of seconds")))?;
    Ok(Some(Duration::from_secs(seconds)))
}

fn required_env(name: &'static str) -> TestResult<String> {
    env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::other(format!("required environment variable {name} is unset")).into()
        })
}

fn exact_env_path(name: &'static str) -> TestResult<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::other(format!("required environment variable {name} is unset")).into()
        })
}

fn h3_profile() -> ProfileRef {
    ProfileRef {
        codec_family: "minimax_h3".to_owned(),
        profile: "h3_av_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    }
}

fn parse_wire_uuid(value: &str) -> TestResult<WireUuid> {
    Ok(serde_json::from_value(Value::String(value.to_owned()))?)
}

fn path_text(path: &Path) -> TestResult<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("private soak path is not valid UTF-8").into())
}

fn finite(value: f64) -> TestResult<FiniteF64> {
    FiniteF64::new(value).ok_or_else(|| io::Error::other("soak control is not finite").into())
}

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition { Ok(()) } else { failure(message) }
}

fn failure<T>(message: &'static str) -> TestResult<T> {
    Err(io::Error::other(message).into())
}
