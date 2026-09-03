//! Dedicated opt-in executable for the private H3 Protocol 2 GPU gate.
//!
//! This binary is absent from normal application builds. It is compiled only
//! with `private-protocol2-gpu-e2e`, starts a real Tauri/DX12 event loop on its
//! own process main thread, and emits one closed path-free receipt after every
//! asserted operation succeeds. Machine-local inputs are supplied only through
//! environment variables and are never serialized into the receipt.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    env, fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode},
    sync::mpsc,
    time::{Duration, Instant},
};

use latentdeck_cartridge::{
    hash::{Sha256Hash, hash_path},
    reader::{ValidationOptions, open_integrity_validated},
};
use latentdeck_control::v2::{
    Ack, CaptureArtifact, CaptureIdentity, CaptureMode, CaptureStart, CaptureState,
    CaptureStatusSnapshot, Command, ControlBinding, ControlValue, DeckProcess, DeckReset,
    DeckState, EmptyPayload, ExternalAssetBinding, MetricsSnapshot, PlayerReset, PlayerState,
    PlayerStep, ProvenanceEntry, RoleBinding, ShutdownReason, SourceTransportBinding,
};
use latentdeck_core::{
    deck_runtime_v2::{DeckOperatorControlDescriptor, DeckOperatorControlKind},
    deck_selection_v2::{
        DeckPackageSelectionV2, DeckSourceSelectionV2, PreparedDeckSelectionV2,
        prepare_exact_deck_selection,
    },
    deck_session_v2::{
        DeckSessionV2, DeckSessionV2LoadRequest, start_deck_session_v2_with_retained_assets,
    },
    player_session_v2::{PlayerSessionV2, PlayerSessionV2HostContract, start_player_session_v2},
};
use latentdeck_extension_manager::{
    CodecCapability, DeckCompatibility, DeckPackManifest, DeckRoleDescriptor,
    DeckRuntimeDescriptor, DeckRuntimeKind, DeckSignalDescriptor, ExtensionRoots, IntegrityCatalog,
    IntegrityDescriptor, IntegrityFile, LicenseDescriptor, PackRequest, PackageKind,
    PackageManifest, PackageReference, ProfileKey, PublisherDescriptor, PublisherIdentityClaim,
    PythonConstraint, PythonImplementation, SignalGeometry as ManifestSignalGeometry, TensorDevice,
    TensorDtype as ManifestTensorDtype, TimingDescriptor, enable, install, pack, resolve_active,
};
use latentdeck_gpu::{
    ring::RingLayout,
    ring_v2::{ReadV2Status, RgbaBatchV2},
    windows_ring::FramesReady,
};
use latentdeck_library::Library;
use latentdeck_native_output::{NativeOutput, NativeOutputConfig, NativeSpoutStatus};
use latentdeck_output_mp4::{RecorderState, RecorderStatus};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tauri::Manager as _;
use uuid::Uuid;

#[path = "bundled_decks.rs"]
mod bundled_decks;
#[allow(dead_code)]
#[path = "capture_finalizer_v2.rs"]
mod capture_finalizer_v2;
#[allow(dead_code)]
#[path = "decoded_recording.rs"]
mod decoded_recording;
#[allow(dead_code)]
#[path = "library_state.rs"]
mod library_state;

use capture_finalizer_v2::{
    CaptureArtifactEvidence, CaptureFinalizationContext, CaptureSourceEvidence, CaptureStagingRoot,
    finalize_capture_with_carrier,
};
use decoded_recording::DecodedRecordingController;

const OPT_IN_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE";
const BASE_ROOT_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_BASE_ROOT";
const WORK_ROOT_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_WORK_ROOT";
const RECEIPT_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT";
const TAEH3_ENV: &str = "LATENTDECK_PRIVATE_PROTOCOL2_TAEH3";
const SOURCE_ENVS: [&str; 4] = [
    "LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_1",
    "LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_2",
    "LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_3",
    "LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_4",
];

const CODEC_ID: &str = "org.latentdeck.h3";
const CODEC_VERSION: &str = "0.2.0";
const ADAPTER_ID: &str = "org.latentdeck.h3";
const ADAPTER_VERSION: &str = "0.2.0";
const D2_ID: &str = "org.latentdeck.deck.d2";
const Q4_ID: &str = "org.latentdeck.deck.q4";
const EXTERNAL_DECK_ID: &str = "dev.latentdeck.private.h3-probe";
const BUNDLED_DECK_VERSION: &str = "0.2.1";
const EXTERNAL_DECK_VERSION: &str = "0.2.0";
const STABILITY_SECONDS: u64 = 360;
const SAMPLE_INTERVAL_SECONDS: u64 = 5;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const RING_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CAPTURE_LATENT_SLOTS: u64 = 16_382;
const MAX_CAPTURE_VISUAL_BYTES: u64 = 1024 * 1024 * 1024;
const SNAPSHOT_BOUNDARY_ATTEMPTS: usize = 8;

type GateResult<T> = Result<T, &'static str>;

#[derive(Clone)]
struct GateConfig {
    roots: ExtensionRoots,
    work_root: PathBuf,
    receipt_path: PathBuf,
    decoder_path: PathBuf,
    sources: Vec<PrivateSource>,
    source_commit: String,
}

#[derive(Clone)]
struct PrivateSource {
    path: PathBuf,
    cartridge_id: String,
    archive_sha256: String,
}

#[derive(Clone)]
struct ExactCodecEvidence {
    archive_sha256: String,
    decoder_binding: ExternalAssetBinding,
    worker_module: String,
    adapter_entrypoint: String,
    capabilities: Vec<String>,
}

#[derive(Clone)]
struct DecodedFrame {
    width: u32,
    height: u32,
    row_stride: u32,
    padded_rgba: Vec<u8>,
}

#[derive(Clone)]
struct SpoutObservation {
    enabled_confirmed: bool,
    published_frames: u64,
    sender_renamed: bool,
    renamed_published_frames: u64,
    disabled_confirmed: bool,
}

#[derive(Clone)]
struct Mp4Observation {
    finished: bool,
    frames_written: u64,
    byte_length: u64,
}

#[derive(Clone)]
struct CaptureObservation {
    finished: bool,
    imported: bool,
    reopened: bool,
    latent_slots: u64,
    decoded_frames: u64,
}

#[derive(Clone)]
struct SurfaceObservation {
    processed_frames: u64,
    reset_generation_before: u64,
    reset_generation_after: u64,
    status_state: &'static str,
    spout: SpoutObservation,
}

#[derive(Clone)]
struct DeckObservation {
    surface: SurfaceObservation,
    snapshot: CaptureObservation,
    live_capture: CaptureObservation,
    mp4: Mp4Observation,
}

#[derive(Clone)]
struct ExternalDeckObservation {
    surface: SurfaceObservation,
    preexisting_sessions_remained_healthy: bool,
}

#[derive(Clone)]
struct StabilityObservation {
    surface: &'static str,
    duration_seconds: u64,
    samples: u64,
}

struct OpenPlayer {
    session: PlayerSessionV2,
    session_id: Uuid,
    generation: u64,
    ring_id: Uuid,
    decoded_frames: u64,
}

struct OpenDeck {
    session: DeckSessionV2,
    session_id: Uuid,
    generation: u64,
    ring_id: Uuid,
    revision: u64,
    capture_context: CaptureFinalizationContext,
    structural_carrier_role: String,
    player_host_template: PlayerSessionV2HostContract,
    processed_frames: u64,
    last_provenance: Vec<ProvenanceEntry>,
}

fn main() -> ExitCode {
    match run_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("private Protocol 2 GPU gate failed: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run_main() -> GateResult<()> {
    require_feature_and_opt_in()?;
    let config = GateConfig::from_environment()?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let task_config = config.clone();
    let app = tauri::Builder::default()
        .setup(move |app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
            let app_handle = app.handle().clone();
            let exit_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let gate = tauri::async_runtime::spawn(Box::pin(run_gate(app_handle, task_config)));
                let result = gate
                    .await
                    .unwrap_or(Err("private Protocol 2 GPU task stopped unexpectedly"));
                let _ = sender.send(result);
                exit_handle.exit(0);
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .map_err(|_| "tauri host initialization failed")?;
    let exit_code = app.run_return(|_, _| {});
    if exit_code != 0 {
        return Err("private GPU Tauri host exited with a nonzero status");
    }
    let receipt = receiver
        .recv()
        .map_err(|_| "private GPU runner ended without an evidence result")??;
    write_validated_receipt(&config.receipt_path, &receipt)
}

fn require_feature_and_opt_in() -> GateResult<()> {
    if !cfg!(feature = "spout-sdk") || !cfg!(feature = "private-protocol2-gpu-e2e") {
        return Err("private GPU runner was built without its exact feature set");
    }
    if env::var(OPT_IN_ENV).as_deref() != Ok("1") {
        return Err("explicit private GPU opt-in is required");
    }
    Ok(())
}

impl GateConfig {
    fn from_environment() -> GateResult<Self> {
        let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .canonicalize()
            .map(|path| without_windows_verbatim_prefix(&path))
            .map_err(|_| "repository root is unavailable")?;
        let source_commit = clean_source_commit(&repository_root)?;
        let base_root = required_existing_directory(BASE_ROOT_ENV)?;
        let work_root = required_new_absolute_path(WORK_ROOT_ENV)?;
        let receipt_path = required_new_absolute_path(RECEIPT_ENV)?;
        let decoder_path = required_regular_file(TAEH3_ENV)?;
        let sources = SOURCE_ENVS
            .iter()
            .map(|name| private_source(name))
            .collect::<GateResult<Vec<_>>>()?;
        if sources.len() != 4 {
            return Err("exactly four private LC sources are required");
        }
        for private_path in std::iter::once(&base_root)
            .chain(std::iter::once(&work_root))
            .chain(std::iter::once(&receipt_path))
            .chain(std::iter::once(&decoder_path))
            .chain(sources.iter().map(|source| &source.path))
        {
            if private_path.starts_with(&repository_root) {
                return Err("private GPU inputs and outputs must remain outside the source tree");
            }
        }
        fs::create_dir(&work_root).map_err(|_| "private work root could not be created")?;
        Ok(Self {
            roots: ExtensionRoots::for_base_root(base_root),
            work_root,
            receipt_path,
            decoder_path,
            sources,
            source_commit,
        })
    }
}

async fn run_gate(app: tauri::AppHandle, config: GateConfig) -> GateResult<Value> {
    let codec = validate_exact_codec(&config)?;
    provision_exact_bundled_decks(&config.roots)?;

    let d2_prepared = prepare_deck(
        &config,
        bundled_deck_reference(D2_ID)?,
        &config.sources[..2],
        &codec,
    )?;
    let player_host = player_host_from_deck(&d2_prepared);
    let profile = d2_prepared.host.profile_key.clone();
    let tensor = d2_prepared.host.tensor_abi.clone();
    let PackageManifest::Deck(d2_manifest) = d2_prepared.deck_runtime.active_package().manifest()
    else {
        return Err("exact D2 package kind changed before external Deck fixture creation");
    };
    let external_timing = d2_manifest.signal.timing.clone();
    let mut player = start_player(&config, &config.sources[0], player_host, &codec).await?;
    let d2_load = d2_load(&d2_prepared)?;
    let mut d2 = start_deck(d2_prepared, d2_load).await?;

    let q4_prepared = prepare_deck(
        &config,
        bundled_deck_reference(Q4_ID)?,
        &config.sources,
        &codec,
    )?;
    let q4_load = q4_load(&q4_prepared)?;
    let mut q4 = start_deck(q4_prepared, q4_load).await?;

    let player_observation = exercise_player_surface(&app, &mut player).await?;
    let d2_observation = exercise_deck_surface(&app, &config, &codec, &mut d2, "d2").await?;
    let q4_observation = exercise_deck_surface(&app, &config, &codec, &mut q4, "q4").await?;

    let external_archive_sha256 =
        install_external_probe_deck(&config, &tensor, &profile, &external_timing)?;
    let external_prepared = prepare_deck(
        &config,
        external_deck_reference(),
        &config.sources[..2],
        &codec,
    )?;
    let external_load = external_load(&external_prepared)?;
    let mut external = start_deck(external_prepared, external_load).await?;
    let external_surface = exercise_external_deck(&mut external).await?;
    shutdown_deck(&mut external).await?;
    let player_after_external = player_status(&mut player).await?;
    let d2_after_external = deck_status(&mut d2).await?;
    let q4_after_external = deck_status(&mut q4).await?;
    let external_observation = ExternalDeckObservation {
        surface: external_surface,
        preexisting_sessions_remained_healthy: player_state_token(player_after_external.state)
            .is_ok()
            && deck_state_token(d2_after_external.state).is_ok()
            && deck_state_token(q4_after_external.state).is_ok(),
    };

    let (player_stability, d2_stability, q4_stability) = tokio::join!(
        soak_player(&mut player),
        soak_deck(&mut d2, "d2"),
        soak_deck(&mut q4, "q4"),
    );
    let stability = vec![player_stability?, d2_stability?, q4_stability?];

    shutdown_player(&mut player).await?;
    shutdown_deck(&mut d2).await?;
    shutdown_deck(&mut q4).await?;

    build_receipt(
        &config,
        &codec,
        &profile,
        &tensor,
        &player_observation,
        &d2_observation,
        &q4_observation,
        &external_observation,
        &external_archive_sha256,
        stability,
    )
}

fn clean_source_commit(repository_root: &Path) -> GateResult<String> {
    let head = ProcessCommand::new("git")
        .args([
            "-C",
            repository_root
                .to_str()
                .ok_or("repository path is not UTF-8")?,
            "rev-parse",
            "HEAD",
        ])
        .output()
        .map_err(|_| "git HEAD query failed")?;
    if !head.status.success() {
        return Err("git HEAD query failed");
    }
    let commit = String::from_utf8(head.stdout)
        .map_err(|_| "git HEAD is not UTF-8")?
        .trim()
        .to_owned();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("git HEAD is not one exact full commit");
    }
    let status = ProcessCommand::new("git")
        .args([
            "-C",
            repository_root
                .to_str()
                .ok_or("repository path is not UTF-8")?,
            "status",
            "--porcelain",
            "--untracked-files=all",
        ])
        .output()
        .map_err(|_| "git status query failed")?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err("private GPU evidence requires a clean exact source commit");
    }
    Ok(commit.to_ascii_lowercase())
}

fn required_existing_directory(name: &str) -> GateResult<PathBuf> {
    let path = required_absolute(name)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| "required directory is unavailable")?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err("required directory must be a non-reparse directory");
    }
    path.canonicalize()
        .map(|path| without_windows_verbatim_prefix(&path))
        .map_err(|_| "required directory cannot be canonicalized")
}

fn required_regular_file(name: &str) -> GateResult<PathBuf> {
    let path = required_absolute(name)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| "required file is unavailable")?;
    if !metadata.is_file() || is_reparse(&metadata) || metadata.len() == 0 {
        return Err("required file must be one non-empty non-reparse regular file");
    }
    path.canonicalize()
        .map(|path| without_windows_verbatim_prefix(&path))
        .map_err(|_| "required file cannot be canonicalized")
}

fn required_new_absolute_path(name: &str) -> GateResult<PathBuf> {
    let path = required_absolute(name)?;
    if path.exists() {
        return Err("private output path must be new below an existing directory");
    }
    let file_name = path
        .file_name()
        .ok_or("private output path must name one file or directory")?;
    let parent = path
        .parent()
        .ok_or("private output path must have one existing parent")?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|_| "private output parent directory is unavailable")?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err("private output parent must be one non-reparse directory");
    }
    let parent = parent
        .canonicalize()
        .map(|path| without_windows_verbatim_prefix(&path))
        .map_err(|_| "private output parent cannot be canonicalized")?;
    Ok(parent.join(file_name))
}

fn without_windows_verbatim_prefix(path: &Path) -> PathBuf {
    let rendered = path.to_string_lossy();
    if let Some(rest) = rendered.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    rendered
        .strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn required_absolute(name: &str) -> GateResult<PathBuf> {
    let value = env::var_os(name).ok_or("required private environment variable is missing")?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("private environment path must be absolute");
    }
    Ok(path)
}

fn private_source(name: &str) -> GateResult<PrivateSource> {
    let path = required_regular_file(name)?;
    let cartridge = open_integrity_validated(&path, &ValidationOptions::default())
        .map_err(|_| "private source failed the generic LC validator")?;
    let cartridge_id = cartridge.manifest().cartridge_id.0.clone();
    let parsed =
        Uuid::parse_str(&cartridge_id).map_err(|_| "private source has invalid identity")?;
    if parsed.is_nil() || parsed.hyphenated().to_string() != cartridge_id {
        return Err("private source identity is not canonical");
    }
    Ok(PrivateSource {
        path,
        cartridge_id,
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
    })
}

#[cfg(target_os = "windows")]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    metadata.file_attributes() & 0x0400 != 0
}

#[cfg(not(target_os = "windows"))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn validate_exact_codec(config: &GateConfig) -> GateResult<ExactCodecEvidence> {
    let package = resolve_active(&config.roots, &codec_reference())
        .map_err(|_| "exact H3 Codec Pack v2 is not active and trusted")?;
    let PackageManifest::Codec(manifest) = package.manifest() else {
        return Err("exact H3 package has the wrong package kind");
    };
    if manifest.manifest_version != "2.0.0"
        || manifest.pack_id != CODEC_ID
        || manifest.pack_version != CODEC_VERSION
        || manifest.adapter.adapter_id != ADAPTER_ID
        || manifest.adapter.adapter_version != ADAPTER_VERSION
        || manifest.compatibility.worker_protocol != 2
        || manifest.compatibility.python.version != "3.13"
        || manifest.compatibility.torch_exact_build != "2.13.0+cu130"
    {
        return Err("exact H3 Codec Pack identity or Protocol 2 ABI is invalid");
    }
    let expected_capabilities = [
        CodecCapability::Player,
        CodecCapability::Realtime,
        CodecCapability::Resample,
        CodecCapability::SnapshotCapture,
        CodecCapability::LiveCapture,
        CodecCapability::RawImport,
    ];
    if manifest.capabilities.as_slice() != expected_capabilities {
        return Err("exact H3 Codec Pack capability set is incomplete or reordered");
    }
    let required_assets = manifest
        .external_assets
        .iter()
        .filter(|asset| asset.required)
        .collect::<Vec<_>>();
    if required_assets.len() != 1 {
        return Err("exact H3 Codec Pack must declare one required decoder asset");
    }
    let descriptor = required_assets[0];
    let measured = hash_path(&config.decoder_path)
        .map_err(|_| "private decoder asset could not be measured")?;
    if measured.byte_length != descriptor.byte_length
        || measured.sha256.to_string() != descriptor.sha256
    {
        return Err("private decoder asset does not match the exact package declaration");
    }
    let worker_module = module_argument(&manifest.worker.arguments)?;
    if worker_module != "latentdeck_codec_host"
        || manifest.adapter.entrypoint != "latentdeck_codec_h3.adapter:make_adapter"
    {
        return Err(
            "exact H3 worker or adapter entrypoint is not the closed Protocol 2 entrypoint",
        );
    }
    Ok(ExactCodecEvidence {
        archive_sha256: package.trust_receipt().archive_sha256.clone(),
        decoder_binding: ExternalAssetBinding {
            asset_id: descriptor.asset_id.clone(),
            path: config
                .decoder_path
                .to_str()
                .ok_or("private decoder path is not UTF-8")?
                .to_owned(),
            sha256: descriptor.sha256.clone(),
            byte_length: descriptor.byte_length,
        },
        worker_module,
        adapter_entrypoint: manifest.adapter.entrypoint.clone(),
        capabilities: manifest
            .capabilities
            .iter()
            .map(|capability| capability_token(*capability).to_owned())
            .collect(),
    })
}

fn module_argument(arguments: &[String]) -> GateResult<String> {
    let mut modules = arguments
        .windows(2)
        .filter(|pair| pair[0] == "-m")
        .map(|pair| pair[1].clone());
    let module = modules
        .next()
        .ok_or("Protocol 2 worker module argument is missing")?;
    if modules.next().is_some() {
        return Err("Protocol 2 worker module argument is ambiguous");
    }
    Ok(module)
}

const fn capability_token(capability: CodecCapability) -> &'static str {
    match capability {
        CodecCapability::Player => "player",
        CodecCapability::Realtime => "realtime",
        CodecCapability::Resample => "resample",
        CodecCapability::SnapshotCapture => "snapshot_capture",
        CodecCapability::LiveCapture => "live_capture",
        CodecCapability::RawImport => "raw_import",
    }
}

fn provision_exact_bundled_decks(roots: &ExtensionRoots) -> GateResult<()> {
    let report = bundled_decks::provision_bundled_decks(roots)
        .map_err(|_| "build-authorized bundled Deck provisioning failed")?;
    if !report.issues.is_empty() {
        return Err("build-authorized bundled Deck provisioning reported an issue");
    }
    for deck_id in [D2_ID, Q4_ID] {
        resolve_active(roots, &bundled_deck_reference(deck_id)?)
            .map_err(|_| "exact bundled Deck version is not active and trusted")?;
    }
    Ok(())
}

fn codec_reference() -> PackageReference {
    PackageReference {
        kind: PackageKind::CodecPack,
        package_id: CODEC_ID.to_owned(),
        package_version: CODEC_VERSION.to_owned(),
    }
}

fn bundled_deck_reference(deck_id: &str) -> GateResult<PackageReference> {
    if !matches!(deck_id, D2_ID | Q4_ID) {
        return Err("bundled Deck reference must name D2 or Q4");
    }
    Ok(deck_reference(deck_id, BUNDLED_DECK_VERSION))
}

fn external_deck_reference() -> PackageReference {
    deck_reference(EXTERNAL_DECK_ID, EXTERNAL_DECK_VERSION)
}

fn deck_reference(deck_id: &str, deck_version: &str) -> PackageReference {
    PackageReference {
        kind: PackageKind::DeckPack,
        package_id: deck_id.to_owned(),
        package_version: deck_version.to_owned(),
    }
}

fn prepare_deck(
    config: &GateConfig,
    deck: PackageReference,
    sources: &[PrivateSource],
    codec: &ExactCodecEvidence,
) -> GateResult<PreparedDeckSelectionV2> {
    let mut selection = DeckPackageSelectionV2::new(
        deck.package_id,
        deck.package_version,
        CODEC_ID.to_owned(),
        CODEC_VERSION.to_owned(),
        latentdeck_control::v2::DeviceKind::Cuda,
    );
    selection.set_device_ordinal(0);
    selection.bind_external_asset(
        codec.decoder_binding.asset_id.clone(),
        config.decoder_path.clone(),
    );
    let inputs = sources
        .iter()
        .map(|source| DeckSourceSelectionV2 {
            path: &source.path,
            cartridge_id: &source.cartridge_id,
            archive_sha256: &source.archive_sha256,
            validated_cartridge: None,
        })
        .collect::<Vec<_>>();
    let prepared = prepare_exact_deck_selection(
        &config.roots,
        &selection,
        &inputs,
        latentdeck_core::product_version(),
    )
    .map_err(|_| "exact Deck/Codec/source preflight failed")?;
    if prepared.host.profile_key.codec_family != "minimax_h3"
        || prepared.host.profile_key.profile != "h3_av_latent"
        || prepared.host.profile_key.profile_version != "0.1.0"
        || prepared.host.tensor_abi.device != latentdeck_control::v2::DeviceKind::Cuda
        || prepared.host.device_ordinal != 0
        || prepared.external_assets != vec![codec.decoder_binding.clone()]
    {
        return Err("exact Deck selection changed the selected H3 profile, device, or asset");
    }
    Ok(prepared)
}

fn player_host_from_deck(prepared: &PreparedDeckSelectionV2) -> PlayerSessionV2HostContract {
    let host = &prepared.host;
    PlayerSessionV2HostContract {
        app_version: host.app_version.clone(),
        player_session_id: Uuid::new_v4(),
        ring_id: Uuid::new_v4(),
        profile_key: host.profile_key.clone(),
        signal_geometry: host.signal_geometry.clone(),
        tensor_abi: host.tensor_abi.clone(),
        decoded_abi: host.decoded_abi.clone(),
        maximum_estimated_host_bytes: host.maximum_estimated_host_bytes,
        maximum_estimated_device_bytes: host.maximum_estimated_device_bytes,
        device_ordinal: host.device_ordinal,
        ring_slot_count: host.ring_slot_count,
        stream_generation: 1,
        loop_enabled: false,
        heartbeat_interval_ms: host.heartbeat_interval_ms,
        heartbeat_hard_timeout_ms: host.heartbeat_hard_timeout_ms,
        command_timeout: host.command_timeout,
    }
}

async fn start_player(
    config: &GateConfig,
    source: &PrivateSource,
    host: PlayerSessionV2HostContract,
    codec: &ExactCodecEvidence,
) -> GateResult<OpenPlayer> {
    let package = resolve_active(&config.roots, &codec_reference())
        .map_err(|_| "exact H3 Codec Pack disappeared before Player startup")?;
    let cartridge = open_integrity_validated(&source.path, &ValidationOptions::default())
        .map_err(|_| "Player source failed retained LC validation")?;
    let session_id = host.player_session_id;
    let generation = host.stream_generation;
    let ring_id = host.ring_id;
    let session = start_player_session_v2(
        package,
        cartridge,
        host,
        vec![codec.decoder_binding.clone()],
    )
    .await
    .map_err(|_| "exact Protocol 2 Player startup failed")?;
    if session.profile_receipt().pack_id != CODEC_ID
        || session.profile_receipt().pack_version != CODEC_VERSION
        || session.profile_receipt().adapter_id != ADAPTER_ID
        || session.profile_receipt().adapter_version != ADAPTER_VERSION
    {
        return Err("Player profile receipt changed the exact Codec Pack identity");
    }
    Ok(OpenPlayer {
        session,
        session_id,
        generation,
        ring_id,
        decoded_frames: 0,
    })
}

async fn start_deck(
    prepared: PreparedDeckSelectionV2,
    load: DeckSessionV2LoadRequest,
) -> GateResult<OpenDeck> {
    let PreparedDeckSelectionV2 {
        codec_package,
        deck_runtime,
        cartridges,
        host,
        external_assets,
        retained_external_assets,
        sources,
        validation_work: _,
    } = prepared;
    let session_id = host.deck_session_id;
    let generation = host.stream_generation;
    let ring_id = host.ring_id;
    let player_host_template = PlayerSessionV2HostContract {
        app_version: host.app_version.clone(),
        player_session_id: Uuid::new_v4(),
        ring_id: Uuid::new_v4(),
        profile_key: host.profile_key.clone(),
        signal_geometry: host.signal_geometry.clone(),
        tensor_abi: host.tensor_abi.clone(),
        decoded_abi: host.decoded_abi.clone(),
        maximum_estimated_host_bytes: host.maximum_estimated_host_bytes,
        maximum_estimated_device_bytes: host.maximum_estimated_device_bytes,
        device_ordinal: host.device_ordinal,
        ring_slot_count: host.ring_slot_count,
        stream_generation: 1,
        loop_enabled: false,
        heartbeat_interval_ms: host.heartbeat_interval_ms,
        heartbeat_hard_timeout_ms: host.heartbeat_hard_timeout_ms,
        command_timeout: host.command_timeout,
    };
    let operator = deck_runtime.operator_descriptor().clone();
    let PackageManifest::Deck(deck_manifest) = deck_runtime.active_package().manifest() else {
        return Err("prepared Deck package kind changed before startup");
    };
    let structural_carrier_role = deck_manifest.signal.structural_carrier_role.clone();
    let capture_sources = cartridges
        .iter()
        .zip(&sources)
        .enumerate()
        .map(|(index, (cartridge, source))| {
            Ok(CaptureSourceEvidence {
                physical_slot: u8::try_from(index + 1)
                    .map_err(|_| "Deck source count exceeded the closed bound")?,
                archive_sha256: source.archive_sha256.clone(),
                manifest: cartridge.manifest().clone(),
            })
        })
        .collect::<GateResult<Vec<_>>>()?;
    let capture_context = CaptureFinalizationContext {
        sources: capture_sources,
        roles: load.roles.clone(),
        controls: load.controls.clone(),
        operator_id: operator.operator_id.clone(),
        operator_version: operator.operator_version.clone(),
        seed: load.seed,
    };
    let session = start_deck_session_v2_with_retained_assets(
        codec_package,
        deck_runtime,
        cartridges,
        host,
        external_assets,
        retained_external_assets,
        load,
    )
    .await
    .map_err(|_| "exact installed Deck Protocol 2 startup failed")?;
    if session.profile_receipts().len() != capture_context.sources.len()
        || session.profile_receipts().iter().any(|receipt| {
            receipt.pack_id != CODEC_ID
                || receipt.pack_version != CODEC_VERSION
                || receipt.adapter_id != ADAPTER_ID
                || receipt.adapter_version != ADAPTER_VERSION
        })
    {
        return Err("Deck profile receipts changed the exact Codec Pack identity");
    }
    let initial = session.initial_status();
    if initial.deck_session_id != session_id
        || initial.stream_generation != generation
        || initial.deck_revision != 1
        || !matches!(
            initial.state,
            DeckState::Ready | DeckState::Paused | DeckState::Playing
        )
    {
        return Err("Deck startup returned an invalid exact status");
    }
    let revision = initial.deck_revision;
    Ok(OpenDeck {
        session,
        session_id,
        generation,
        ring_id,
        revision,
        capture_context,
        structural_carrier_role,
        player_host_template,
        processed_frames: 0,
        last_provenance: Vec::new(),
    })
}

fn d2_load(prepared: &PreparedDeckSelectionV2) -> GateResult<DeckSessionV2LoadRequest> {
    let load = deck_load(prepared, &["carrier", "donor"])?;
    if load.controls.is_empty() {
        return Err("validated bundled D2 operator declared no controls");
    }
    Ok(load)
}

fn q4_load(prepared: &PreparedDeckSelectionV2) -> GateResult<DeckSessionV2LoadRequest> {
    let load = deck_load(prepared, &["carrier", "donor_b", "donor_c", "donor_d"])?;
    if load.controls.is_empty() {
        return Err("validated bundled Q4 operator declared no controls");
    }
    Ok(load)
}

fn external_load(prepared: &PreparedDeckSelectionV2) -> GateResult<DeckSessionV2LoadRequest> {
    deck_load(prepared, &["carrier", "donor"])
}

fn deck_load(
    prepared: &PreparedDeckSelectionV2,
    roles: &[&str],
) -> GateResult<DeckSessionV2LoadRequest> {
    Ok(DeckSessionV2LoadRequest {
        roles: roles
            .iter()
            .enumerate()
            .map(|(index, role)| RoleBinding {
                role: (*role).to_owned(),
                physical_slot: u8::try_from(index + 1).expect("closed Deck role bound"),
            })
            .collect(),
        controls: operator_default_controls_in_declaration_order(prepared)?,
        source_transport: roles
            .iter()
            .enumerate()
            .map(|(index, _)| SourceTransportBinding {
                physical_slot: u8::try_from(index + 1).expect("closed Deck source bound"),
                playing: true,
                loop_enabled: true,
            })
            .collect(),
        seed: 0x5eed,
    })
}

fn operator_default_controls_in_declaration_order(
    prepared: &PreparedDeckSelectionV2,
) -> GateResult<Vec<ControlBinding>> {
    prepared
        .deck_runtime
        .operator_descriptor()
        .controls
        .iter()
        .map(|control| {
            Ok(ControlBinding {
                name: control.control_id.clone(),
                value: operator_default_control_value(control)?,
            })
        })
        .collect()
}

fn operator_default_control_value(
    control: &DeckOperatorControlDescriptor,
) -> GateResult<ControlValue> {
    let invalid = || "validated Deck operator default could not be represented";
    match control.value_type {
        DeckOperatorControlKind::Boolean => control
            .default
            .as_bool()
            .map(ControlValue::Boolean)
            .ok_or_else(invalid),
        DeckOperatorControlKind::Integer => control
            .default
            .as_i64()
            .or_else(|| {
                control
                    .default
                    .as_u64()
                    .and_then(|value| i64::try_from(value).ok())
            })
            .map(ControlValue::Integer)
            .ok_or_else(invalid),
        DeckOperatorControlKind::Number => control
            .default
            .as_f64()
            .filter(|value| value.is_finite())
            .map(ControlValue::Number)
            .ok_or_else(invalid),
        DeckOperatorControlKind::Enum | DeckOperatorControlKind::Text => control
            .default
            .as_str()
            .map(|value| ControlValue::Text(value.to_owned()))
            .ok_or_else(invalid),
    }
}

async fn exercise_player_surface(
    app: &tauri::AppHandle,
    player: &mut OpenPlayer,
) -> GateResult<SurfaceObservation> {
    let status = player_status(player).await?;
    if !matches!(status.state, PlayerState::Ready | PlayerState::Paused) {
        return Err("Player did not open in a ready state");
    }
    let mut frames = Vec::new();
    for _ in 0..4 {
        frames.extend(player_step_frames(player).await?);
        if frames.len() >= 2 {
            break;
        }
    }
    if frames.len() < 2 {
        return Err("Player did not decode enough real frames for the output proof");
    }
    let spout = prove_spout(app, "player", &frames[..2]).await?;
    let reset_generation_before = player.generation;
    player_reset(player).await?;
    let reset_generation_after = player.generation;
    let status = player_status(player).await?;
    Ok(SurfaceObservation {
        processed_frames: player.decoded_frames,
        reset_generation_before,
        reset_generation_after,
        status_state: player_state_token(status.state)?,
        spout,
    })
}

async fn exercise_deck_surface(
    app: &tauri::AppHandle,
    config: &GateConfig,
    codec: &ExactCodecEvidence,
    deck: &mut OpenDeck,
    label: &'static str,
) -> GateResult<DeckObservation> {
    let status = deck_status(deck).await?;
    if !matches!(
        status.state,
        DeckState::Ready | DeckState::Paused | DeckState::Playing
    ) {
        return Err("Deck did not open in a ready state");
    }
    let mut frames = Vec::new();
    for _ in 0..4 {
        frames.extend(deck_process_frames(deck).await?);
        if frames.len() >= 6 {
            break;
        }
    }
    if frames.len() < 2 {
        return Err("Deck did not decode enough real frames for the output proof");
    }
    let spout = prove_spout(app, label, &frames[..2]).await?;
    let mp4 = record_mp4(config, label, &frames)?;

    let reset_generation_before = deck.generation;
    deck_reset(deck).await?;
    let reset_generation_after = deck.generation;
    let snapshot = run_capture(config, codec, deck, label, CaptureMode::Snapshot).await?;
    let live_capture = run_capture(config, codec, deck, label, CaptureMode::LiveCapture).await?;
    let status = deck_status(deck).await?;
    Ok(DeckObservation {
        surface: SurfaceObservation {
            processed_frames: deck.processed_frames,
            reset_generation_before,
            reset_generation_after,
            status_state: deck_state_token(status.state)?,
            spout,
        },
        snapshot,
        live_capture,
        mp4,
    })
}

async fn exercise_external_deck(deck: &mut OpenDeck) -> GateResult<SurfaceObservation> {
    let before = deck.generation;
    let frames = deck_process_frames(deck).await?;
    if frames.is_empty() {
        return Err("dynamically installed external Deck produced no decoded frames");
    }
    let external_marker = deck.last_provenance.iter().any(|entry| {
        entry.key == "external_package_loaded" && entry.value == ControlValue::Boolean(true)
    });
    let operator_marker = deck.last_provenance.iter().any(|entry| {
        entry.key == "operator_id"
            && entry.value
                == ControlValue::Text("dev.latentdeck.private.h3_probe.operator".to_owned())
    });
    if !external_marker || !operator_marker {
        return Err("external Deck process omitted its exact operator provenance");
    }
    deck_reset(deck).await?;
    let after = deck.generation;
    let status = deck_status(deck).await?;
    Ok(SurfaceObservation {
        processed_frames: deck.processed_frames,
        reset_generation_before: before,
        reset_generation_after: after,
        status_state: deck_state_token(status.state)?,
        spout: SpoutObservation {
            enabled_confirmed: false,
            published_frames: 0,
            sender_renamed: false,
            renamed_published_frames: 0,
            disabled_confirmed: false,
        },
    })
}

async fn player_step_frames(player: &mut OpenPlayer) -> GateResult<Vec<DecodedFrame>> {
    let Ack::PlayerStep(step) = player
        .session
        .client_mut()
        .call(
            Command::PlayerStep(PlayerStep {
                player_session_id: player.session_id,
                stream_generation: player.generation,
                maximum_decoded_frames: 24,
            }),
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|_| "Protocol 2 player.step failed")?
    else {
        return Err("Protocol 2 player.step returned the wrong acknowledgement");
    };
    if step.status.player_session_id != player.session_id
        || step.status.stream_generation != player.generation
        || step.status.decoded_ring_id != Some(player.ring_id)
    {
        return Err("Protocol 2 player.step returned a mismatched status identity");
    }
    if step.decoded_frames == 0 {
        if !step.status.end_of_stream || step.output_ring_id.is_some() {
            return Err("Protocol 2 player.step returned an invalid empty batch");
        }
        player_reset(player).await?;
        return Ok(Vec::new());
    }
    if step.output_ring_id != Some(player.ring_id) {
        return Err("Protocol 2 player.step changed the decoded ring identity");
    }
    let batch = read_player_batch(player)?;
    if batch.metadata().session_id() != *player.session_id.as_bytes()
        || batch.metadata().generation() != player.generation
        || batch.metadata().logical_sequence() != step.status.stream_sequence
        || batch.metadata().slot_sequence() != step.output_slot_sequence
        || batch.metadata().batch() != u32::from(step.decoded_frames)
    {
        return Err("Player RGB Ring ABI 2 metadata did not match player.step");
    }
    let frames = decoded_frames(&batch)?;
    player.decoded_frames = player
        .decoded_frames
        .checked_add(u64::try_from(frames.len()).map_err(|_| "Player frame count overflow")?)
        .ok_or("Player frame count overflow")?;
    // The final non-empty batch carries the EOS marker. Reset immediately
    // after consuming that batch so the stability loop never issues an
    // out-of-range player.step; Protocol 2 requires an explicit generation
    // transition instead of an implicit decoder wrap.
    if step.status.end_of_stream {
        player_reset(player).await?;
    }
    Ok(frames)
}

fn read_player_batch(player: &mut OpenPlayer) -> GateResult<RgbaBatchV2> {
    if player
        .session
        .ring_consumer_mut()
        .wait_ready(RING_TIMEOUT)
        .map_err(|_| "Player RGB Ring ABI 2 wait failed")?
        != FramesReady::Signaled
    {
        return Err("Player RGB Ring ABI 2 did not signal a decoded batch");
    }
    match player
        .session
        .ring_consumer_mut()
        .try_read()
        .map_err(|_| "Player RGB Ring ABI 2 read failed")?
    {
        ReadV2Status::Batch(batch) => Ok(batch),
        ReadV2Status::Empty => Err("Player RGB Ring ABI 2 signaled without a batch"),
    }
}

async fn player_reset(player: &mut OpenPlayer) -> GateResult<()> {
    let next = player
        .generation
        .checked_add(1)
        .ok_or("Player stream generation overflow")?;
    let Ack::PlayerReset(status) = player
        .session
        .client_mut()
        .call(
            Command::PlayerReset(PlayerReset {
                player_session_id: player.session_id,
                new_stream_generation: next,
            }),
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|_| "Protocol 2 player.reset failed")?
    else {
        return Err("Protocol 2 player.reset returned the wrong acknowledgement");
    };
    if status.player_session_id != player.session_id
        || status.stream_generation != next
        || status.stream_sequence != 0
        || status.playhead_slot != 0
        || status.end_of_stream
        || status.decoded_ring_id != Some(player.ring_id)
    {
        return Err("Protocol 2 player.reset returned a mismatched status");
    }
    player
        .session
        .adopt_ring_generation(next)
        .map_err(|_| "Core could not adopt the Player RGB Ring reset")?;
    player.generation = next;
    Ok(())
}

async fn player_status(
    player: &mut OpenPlayer,
) -> GateResult<latentdeck_control::v2::PlayerStatusSnapshot> {
    let Ack::PlayerStatus(status) = player
        .session
        .client_mut()
        .call(Command::PlayerStatus(EmptyPayload {}), COMMAND_TIMEOUT)
        .await
        .map_err(|_| "Protocol 2 player.status failed")?
    else {
        return Err("Protocol 2 player.status returned the wrong acknowledgement");
    };
    if status.player_session_id != player.session_id
        || status.stream_generation != player.generation
        || status.decoded_ring_id != Some(player.ring_id)
        || matches!(status.state, PlayerState::Empty | PlayerState::Faulted)
    {
        return Err("Protocol 2 player.status returned a mismatched or faulted status");
    }
    Ok(status)
}

async fn deck_process_frames(deck: &mut OpenDeck) -> GateResult<Vec<DecodedFrame>> {
    let Ack::DeckProcess(processed) = deck
        .session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id: deck.session_id,
                deck_revision: deck.revision,
                stream_generation: deck.generation,
            }),
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|_| "Protocol 2 deck.process failed")?
    else {
        return Err("Protocol 2 deck.process returned the wrong acknowledgement");
    };
    if processed.status.deck_session_id != deck.session_id
        || processed.status.deck_revision != deck.revision
        || processed.status.stream_generation != deck.generation
        || processed.output_ring_id != deck.ring_id
        || matches!(
            processed.status.state,
            DeckState::Empty | DeckState::Faulted
        )
    {
        return Err("Protocol 2 deck.process returned a mismatched or faulted status");
    }
    let batch = read_deck_batch(deck)?;
    if batch.metadata().session_id() != *deck.session_id.as_bytes()
        || batch.metadata().generation() != deck.generation
        || batch.metadata().logical_sequence() != processed.status.stream_sequence
        || batch.metadata().slot_sequence() != processed.output_slot_sequence
    {
        return Err("Deck RGB Ring ABI 2 metadata did not match deck.process");
    }
    let frames = decoded_frames(&batch)?;
    deck.last_provenance = processed.provenance.as_slice().to_vec();
    deck.processed_frames = deck
        .processed_frames
        .checked_add(u64::try_from(frames.len()).map_err(|_| "Deck frame count overflow")?)
        .ok_or("Deck frame count overflow")?;
    Ok(frames)
}

fn read_deck_batch(deck: &mut OpenDeck) -> GateResult<RgbaBatchV2> {
    if deck
        .session
        .ring_consumer_mut()
        .wait_ready(RING_TIMEOUT)
        .map_err(|_| "Deck RGB Ring ABI 2 wait failed")?
        != FramesReady::Signaled
    {
        return Err("Deck RGB Ring ABI 2 did not signal a decoded batch");
    }
    match deck
        .session
        .ring_consumer_mut()
        .try_read()
        .map_err(|_| "Deck RGB Ring ABI 2 read failed")?
    {
        ReadV2Status::Batch(batch) => Ok(batch),
        ReadV2Status::Empty => Err("Deck RGB Ring ABI 2 signaled without a batch"),
    }
}

async fn deck_reset(deck: &mut OpenDeck) -> GateResult<()> {
    let next = deck
        .generation
        .checked_add(1)
        .ok_or("Deck stream generation overflow")?;
    let Ack::DeckReset(status) = deck
        .session
        .client_mut()
        .call(
            Command::DeckReset(DeckReset {
                deck_session_id: deck.session_id,
                deck_revision: deck.revision,
                new_stream_generation: next,
                preserve_playheads: false,
            }),
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|_| "Protocol 2 deck.reset failed")?
    else {
        return Err("Protocol 2 deck.reset returned the wrong acknowledgement");
    };
    if status.deck_session_id != deck.session_id
        || status.deck_revision != deck.revision
        || status.stream_generation != next
        || status.stream_sequence != 0
        || status.capture_state != CaptureState::Idle
        || matches!(status.state, DeckState::Empty | DeckState::Faulted)
    {
        return Err("Protocol 2 deck.reset returned a mismatched or faulted status");
    }
    deck.session
        .adopt_ring_generation(next)
        .map_err(|_| "Core could not adopt the Deck RGB Ring reset")?;
    deck.generation = next;
    Ok(())
}

async fn deck_status(
    deck: &mut OpenDeck,
) -> GateResult<latentdeck_control::v2::DeckStatusSnapshot> {
    let Ack::DeckStatus(status) = deck
        .session
        .client_mut()
        .call(Command::DeckStatus(EmptyPayload {}), COMMAND_TIMEOUT)
        .await
        .map_err(|_| "Protocol 2 deck.status failed")?
    else {
        return Err("Protocol 2 deck.status returned the wrong acknowledgement");
    };
    let status = *status;
    if status.deck_session_id != deck.session_id
        || status.deck_revision != deck.revision
        || status.stream_generation != deck.generation
        || matches!(status.state, DeckState::Empty | DeckState::Faulted)
    {
        return Err("Protocol 2 deck.status returned a mismatched or faulted status");
    }
    Ok(status)
}

fn decoded_frames(batch: &RgbaBatchV2) -> GateResult<Vec<DecodedFrame>> {
    let width = batch.width();
    let height = batch.height();
    let batch_size =
        usize::try_from(batch.metadata().batch()).map_err(|_| "decoded batch count overflow")?;
    if batch_size == 0 || batch_size > 24 {
        return Err("decoded batch count violated the ABI 2 bound");
    }
    let tight_stride = width
        .checked_mul(4)
        .ok_or("decoded tight row stride overflow")?;
    let tight_frame_bytes = usize::try_from(u64::from(tight_stride) * u64::from(height))
        .map_err(|_| "decoded frame byte count overflow")?;
    if batch.pixels().len() != tight_frame_bytes * batch_size {
        return Err("decoded ABI 2 batch byte count is invalid");
    }
    let layout =
        RingLayout::new(width, height).map_err(|_| "decoded output geometry is invalid")?;
    let row_stride = layout.row_stride();
    let output_bytes = usize::try_from(u64::from(row_stride) * u64::from(height))
        .map_err(|_| "padded output frame byte count overflow")?;
    let mut result = Vec::with_capacity(batch_size);
    for tight in batch.pixels().chunks_exact(tight_frame_bytes) {
        let mut padded = vec![0_u8; output_bytes];
        for row in 0..usize::try_from(height).map_err(|_| "decoded height overflow")? {
            let source =
                row * usize::try_from(tight_stride).map_err(|_| "decoded stride overflow")?;
            let target = row * usize::try_from(row_stride).map_err(|_| "padded stride overflow")?;
            padded
                [target..target + usize::try_from(tight_stride).map_err(|_| "stride overflow")?]
                .copy_from_slice(
                    &tight[source
                        ..source + usize::try_from(tight_stride).map_err(|_| "stride overflow")?],
                );
        }
        result.push(DecodedFrame {
            width,
            height,
            row_stride,
            padded_rgba: padded,
        });
    }
    Ok(result)
}

async fn prove_spout(
    app: &tauri::AppHandle,
    label: &str,
    frames: &[DecodedFrame],
) -> GateResult<SpoutObservation> {
    let [first, second, ..] = frames else {
        return Err("Spout proof requires two real decoded frames");
    };
    if (first.width, first.height) != (second.width, second.height) {
        return Err("Spout proof frames changed decoded geometry");
    }
    let unique = Uuid::new_v4().simple().to_string();
    let initial_name = format!("LatentDeck P2 {label} {unique}");
    let renamed = format!("LatentDeck P2 {label} renamed {unique}");
    let window_label = format!("private-p2-{label}-{unique}");
    let mut output = NativeOutput::new(
        app,
        NativeOutputConfig::new(
            first.width,
            first.height,
            window_label,
            "LatentDeck private Protocol 2 GPU gate",
        )
        .with_spout_sender_name(initial_name.clone()),
    )
    .await
    .map_err(|_| "real DX12 output could not be created")?;
    let initial = output.spout_status();
    require_spout_ready(&initial)?;
    if initial.enabled || initial.published || initial.submitted_frames != 0 {
        return Err("Spout sender did not begin in the closed disabled state");
    }
    let enabled = output
        .set_spout_enabled(true)
        .map_err(|_| "Spout sender could not be enabled")?;
    if !enabled.enabled {
        return Err("Spout sender did not confirm enablement");
    }
    output
        .present_padded_rgba(
            first.width,
            first.height,
            first.row_stride,
            &first.padded_rgba,
        )
        .map_err(|_| "real decoded frame could not be presented to Spout")?;
    let published = output.spout_status();
    if !published.enabled
        || !published.published
        || published.submitted_frames == 0
        || published.requested_name != initial_name
    {
        return Err("Spout did not publish the first real decoded texture");
    }
    let renamed_status = output
        .set_spout_name(renamed.clone())
        .map_err(|_| "Spout sender could not be renamed")?;
    if renamed_status.requested_name != renamed || renamed_status.published {
        return Err("Spout rename did not unregister the previous sender identity");
    }
    output
        .present_padded_rgba(
            second.width,
            second.height,
            second.row_stride,
            &second.padded_rgba,
        )
        .map_err(|_| "renamed Spout sender could not publish a real decoded texture")?;
    let republished = output.spout_status();
    if !republished.enabled
        || !republished.published
        || republished.requested_name != renamed
        || republished.active_name.is_empty()
        || republished.submitted_frames <= published.submitted_frames
    {
        return Err("renamed Spout sender did not republish the decoded texture");
    }
    let disabled = output
        .set_spout_enabled(false)
        .map_err(|_| "Spout sender could not be disabled")?;
    if disabled.enabled || disabled.published || disabled.spout_frame.is_some() {
        return Err("Spout sender did not reach the closed disabled state");
    }
    output
        .destroy()
        .map_err(|_| "private native output could not be destroyed")?;
    Ok(SpoutObservation {
        enabled_confirmed: enabled.enabled,
        published_frames: published.submitted_frames,
        sender_renamed: renamed_status.requested_name == renamed && !renamed_status.published,
        renamed_published_frames: republished.submitted_frames,
        disabled_confirmed: !disabled.enabled && !disabled.published,
    })
}

fn require_spout_ready(status: &NativeSpoutStatus) -> GateResult<()> {
    if !status.sdk_built || !status.ready || status.last_error_code.is_some() {
        return Err("real pinned Spout2 SDK is not ready on the exact DX12 device");
    }
    Ok(())
}

fn record_mp4(
    config: &GateConfig,
    label: &str,
    frames: &[DecodedFrame],
) -> GateResult<Mp4Observation> {
    if frames.is_empty() {
        return Err("MP4 proof requires real decoded frames");
    }
    let destination = config.work_root.join(format!("{label}-decoded.mp4"));
    let controller = DecodedRecordingController::new();
    let armed = controller
        .arm(destination.clone())
        .map_err(|_| "production decoded recorder could not be armed")?;
    if armed.state != RecorderState::Armed {
        return Err("production decoded recorder did not enter Armed");
    }
    for frame in frames.iter().take(6) {
        let status = controller
            .submit_if_active(
                frame.width,
                frame.height,
                frame.row_stride,
                &frame.padded_rgba,
            )
            .ok_or("production decoded recorder disappeared during frame submission")?;
        if status.state == RecorderState::Failed {
            return Err("production decoded recorder rejected a real decoded frame");
        }
    }
    let finished = controller
        .stop()
        .map_err(|_| "production decoded recorder finalization failed")?;
    validate_finished_recording(&finished)?;
    let metadata = fs::symlink_metadata(&destination)
        .map_err(|_| "production decoded recorder did not publish an MP4")?;
    if !metadata.is_file() || is_reparse(&metadata) || metadata.len() == 0 {
        return Err("production decoded recorder published an invalid MP4");
    }
    Ok(Mp4Observation {
        finished: finished.state == RecorderState::Finished,
        frames_written: finished.frames_written,
        byte_length: metadata.len(),
    })
}

fn validate_finished_recording(status: &RecorderStatus) -> GateResult<()> {
    if status.state != RecorderState::Finished
        || status.frames_accepted == 0
        || status.frames_written == 0
        || status.frames_written != status.frames_accepted
        || status.width.is_none()
        || status.height.is_none()
        || status.error_code.is_some()
    {
        return Err("production decoded recorder returned an incomplete terminal receipt");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run_capture(
    config: &GateConfig,
    codec: &ExactCodecEvidence,
    deck: &mut OpenDeck,
    label: &str,
    mode: CaptureMode,
) -> GateResult<CaptureObservation> {
    let capture_id = Uuid::new_v4();
    let staging = CaptureStagingRoot::create(&config.work_root, capture_id)
        .map_err(|_| "production capture staging root could not be created")?;
    let staging_path = staging
        .root()
        .to_str()
        .ok_or("capture staging path is not UTF-8")?
        .to_owned();
    let Ack::CaptureStart(started) = deck
        .session
        .client_mut()
        .call(
            Command::CaptureStart(CaptureStart {
                deck_session_id: deck.session_id,
                deck_revision: deck.revision,
                capture_id,
                mode,
                staging_root: staging_path,
                maximum_latent_slots: MAX_CAPTURE_LATENT_SLOTS,
                maximum_visual_bytes: MAX_CAPTURE_VISUAL_BYTES,
                maximum_reset_events: 32,
            }),
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|_| "Protocol 2 capture.start failed")?
    else {
        return Err("Protocol 2 capture.start returned the wrong acknowledgement");
    };
    validate_capture_identity(&started, deck, capture_id, mode)?;
    if started.state != CaptureState::Capturing
        || started.latent_slots != 0
        || started.artifact.is_some()
    {
        return Err("Protocol 2 capture.start did not enter the exact Capturing state");
    }

    // Snapshot is one codec-valid payload, not necessarily one latent slot.
    // H3, for example, first becomes serializable at T=2. Keep the probe
    // bounded while allowing the trusted adapter to report that boundary.
    let process_count = if mode == CaptureMode::Snapshot {
        SNAPSHOT_BOUNDARY_ATTEMPTS
    } else {
        2
    };
    let mut last_process_state = CaptureState::Capturing;
    for _ in 0..process_count {
        let Ack::DeckProcess(processed) = deck
            .session
            .client_mut()
            .call(
                Command::DeckProcess(DeckProcess {
                    deck_session_id: deck.session_id,
                    deck_revision: deck.revision,
                    stream_generation: deck.generation,
                }),
                COMMAND_TIMEOUT,
            )
            .await
            .map_err(|_| "Protocol 2 capture deck.process failed")?
        else {
            return Err("Protocol 2 capture deck.process returned the wrong acknowledgement");
        };
        // The capture state and latent-slot receipt are acknowledged from the
        // post-operator boundary before the host consumes decoded RGBA.
        if processed.status.deck_session_id != deck.session_id
            || processed.status.stream_generation != deck.generation
            || processed.output_ring_id != deck.ring_id
            || processed.status.capture_state == CaptureState::Faulted
        {
            return Err("capture deck.process returned a mismatched or faulted status");
        }
        last_process_state = processed.status.capture_state;
        let batch = read_deck_batch(deck)?;
        if batch.metadata().logical_sequence() != processed.status.stream_sequence
            || batch.metadata().session_id() != *deck.session_id.as_bytes()
            || batch.metadata().generation() != deck.generation
        {
            return Err("capture RGB Ring ABI 2 metadata did not match deck.process");
        }
        let frames = decoded_frames(&batch)?;
        deck.processed_frames = deck
            .processed_frames
            .checked_add(u64::try_from(frames.len()).map_err(|_| "capture frame overflow")?)
            .ok_or("capture frame overflow")?;
        if mode == CaptureMode::Snapshot && last_process_state == CaptureState::Completed {
            break;
        }
    }
    if mode == CaptureMode::Snapshot && last_process_state != CaptureState::Completed {
        return Err("Snapshot did not finish on the first codec-valid boundary");
    }
    if mode == CaptureMode::LiveCapture && last_process_state != CaptureState::Capturing {
        return Err("Live Capture did not remain active before explicit stop");
    }

    let status = if mode == CaptureMode::Snapshot {
        capture_status(deck, capture_id, mode).await?
    } else {
        let active = capture_status(deck, capture_id, mode).await?;
        if active.state != CaptureState::Capturing || active.latent_slots == 0 {
            return Err("Live Capture did not report appended post-operator latent slots");
        }
        let Ack::CaptureStop(stopped) = deck
            .session
            .client_mut()
            .call(
                Command::CaptureStop(CaptureIdentity {
                    deck_session_id: deck.session_id,
                    deck_revision: deck.revision,
                    capture_id,
                }),
                COMMAND_TIMEOUT,
            )
            .await
            .map_err(|_| "Protocol 2 capture.stop failed")?
        else {
            return Err("Protocol 2 capture.stop returned the wrong acknowledgement");
        };
        let stopped = *stopped;
        validate_capture_identity(&stopped, deck, capture_id, mode)?;
        await_capture_terminal(deck, stopped, capture_id, mode).await?
    };
    if status.state != CaptureState::Completed || status.latent_slots == 0 {
        return Err("Protocol 2 capture did not return a completed non-empty receipt");
    }
    let artifact = status
        .artifact
        .as_ref()
        .ok_or("completed capture omitted its staged payload receipt")?;
    validate_capture_artifact(artifact, &status)?;
    let artifact_evidence = CaptureArtifactEvidence {
        capture_id,
        staged_payload_path: artifact.staged_payload_path.clone(),
        payload_sha256: artifact.payload_sha256.clone(),
        payload_byte_length: artifact.payload_byte_length,
        latent_slots: artifact.latent_slots,
        decoded_frame_count: artifact.decoded_frame_count,
    };
    let mode_token = if mode == CaptureMode::Snapshot {
        "snapshot"
    } else {
        "live"
    };
    let output = config
        .work_root
        .join(format!("{label}-{mode_token}-{capture_id}.lc"));
    let library = library_state::AppState::new(
        Library::in_memory().map_err(|_| "private capture Library could not be opened")?,
    );
    let finalized = finalize_capture_with_carrier(
        staging,
        artifact_evidence,
        mode,
        status.reset_events,
        deck.capture_context.clone(),
        &deck.structural_carrier_role,
        MAX_CAPTURE_LATENT_SLOTS,
        MAX_CAPTURE_VISUAL_BYTES,
        24,
        output.clone(),
        library.importer(),
    )
    .await
    .map_err(|_| "production generic capture finalizer rejected the worker artifact")?;
    let reopened = open_integrity_validated(&output, &ValidationOptions::default())
        .map_err(|_| "finalized capture did not reopen through the generic LC validator")?;
    if reopened.receipt().archive_sha256.to_string() != finalized.archive_sha256
        || reopened.manifest().cartridge_id.0 != finalized.cartridge_id
    {
        return Err("finalized capture identity changed during reopen validation");
    }
    drop(reopened);
    let captured_source = private_source_from_path(output)?;
    let mut host = deck.player_host_template.clone();
    host.player_session_id = Uuid::new_v4();
    host.ring_id = Uuid::new_v4();
    host.stream_generation = 1;
    let mut replay = start_player(config, &captured_source, host, codec).await?;
    let replay_frames = first_player_frames(&mut replay).await?;
    shutdown_player(&mut replay).await?;
    deck_reset(deck).await?;
    Ok(CaptureObservation {
        finished: status.state == CaptureState::Completed,
        imported: !finalized.archive_sha256.is_empty(),
        reopened: captured_source.archive_sha256 == finalized.archive_sha256,
        latent_slots: status.latent_slots,
        decoded_frames: replay_frames,
    })
}

fn private_source_from_path(path: PathBuf) -> GateResult<PrivateSource> {
    let cartridge = open_integrity_validated(&path, &ValidationOptions::default())
        .map_err(|_| "captured source failed generic LC validation")?;
    Ok(PrivateSource {
        cartridge_id: cartridge.manifest().cartridge_id.0.clone(),
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
        path,
    })
}

async fn first_player_frames(player: &mut OpenPlayer) -> GateResult<u64> {
    for _ in 0..8 {
        let frames = player_step_frames(player).await?;
        if !frames.is_empty() {
            return u64::try_from(frames.len()).map_err(|_| "captured replay frame overflow");
        }
    }
    Err("captured LC did not decode through the exact Protocol 2 Player")
}

async fn capture_status(
    deck: &mut OpenDeck,
    capture_id: Uuid,
    mode: CaptureMode,
) -> GateResult<CaptureStatusSnapshot> {
    let Ack::CaptureStatus(status) = deck
        .session
        .client_mut()
        .call(
            Command::CaptureStatus(CaptureIdentity {
                deck_session_id: deck.session_id,
                deck_revision: deck.revision,
                capture_id,
            }),
            COMMAND_TIMEOUT,
        )
        .await
        .map_err(|_| "Protocol 2 capture.status failed")?
    else {
        return Err("Protocol 2 capture.status returned the wrong acknowledgement");
    };
    let status = *status;
    validate_capture_identity(&status, deck, capture_id, mode)?;
    Ok(status)
}

async fn await_capture_terminal(
    deck: &mut OpenDeck,
    mut status: CaptureStatusSnapshot,
    capture_id: Uuid,
    mode: CaptureMode,
) -> GateResult<CaptureStatusSnapshot> {
    for _ in 0..120 {
        if matches!(
            status.state,
            CaptureState::Completed | CaptureState::Aborted | CaptureState::Faulted
        ) {
            return Ok(status);
        }
        if status.state != CaptureState::Finalizing {
            return Err("capture.stop returned an invalid nonterminal transition");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        status = capture_status(deck, capture_id, mode).await?;
    }
    Err("capture finalization exceeded the bounded status deadline")
}

fn validate_capture_identity(
    status: &CaptureStatusSnapshot,
    deck: &OpenDeck,
    capture_id: Uuid,
    mode: CaptureMode,
) -> GateResult<()> {
    if status.deck_session_id != deck.session_id
        || status.deck_revision != deck.revision
        || status.capture_id != capture_id
        || status.mode != mode
        || status.reset_events > 32
    {
        return Err("capture status identity violated the exact request");
    }
    Ok(())
}

fn validate_capture_artifact(
    artifact: &CaptureArtifact,
    status: &CaptureStatusSnapshot,
) -> GateResult<()> {
    if artifact.latent_slots != status.latent_slots
        || artifact.latent_slots == 0
        || artifact.latent_slots > MAX_CAPTURE_LATENT_SLOTS
        || artifact.payload_byte_length == 0
        || artifact.payload_byte_length > MAX_CAPTURE_VISUAL_BYTES
        || artifact.decoded_frame_count < artifact.latent_slots
        || artifact.decoded_frame_count > artifact.latent_slots.saturating_mul(24)
        || Sha256Hash::parse(&artifact.payload_sha256).is_err()
    {
        return Err("capture artifact violated host-side receipt bounds");
    }
    Ok(())
}

async fn shutdown_player(player: &mut OpenPlayer) -> GateResult<()> {
    let ack = player
        .session
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .map_err(|_| "Protocol 2 Player shutdown failed")?;
    if !ack.success {
        return Err("Protocol 2 Player worker did not exit cleanly");
    }
    Ok(())
}

async fn shutdown_deck(deck: &mut OpenDeck) -> GateResult<()> {
    let ack = deck
        .session
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .map_err(|_| "Protocol 2 Deck shutdown failed")?;
    if !ack.success {
        return Err("Protocol 2 Deck worker did not exit cleanly");
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn install_external_probe_deck(
    config: &GateConfig,
    tensor: &latentdeck_control::v2::TensorAbi,
    profile: &latentdeck_control::v2::ProfileKey,
    timing: &TimingDescriptor,
) -> GateResult<String> {
    let source = config.work_root.join("external-deck-source");
    fs::create_dir(&source).map_err(|_| "external Deck source root could not be created")?;
    write_file(
        &source,
        "NOTICE.txt",
        b"Private executable Protocol 2 gate fixture.\n",
    )?;
    write_json(
        &source,
        "faceplate.json",
        &json!({"schema_version": 1, "title": "Private P2 probe", "sections": []}),
    )?;
    write_json(
        &source,
        "operator.json",
        &json!({
            "schema_version": "0.2.0",
            "deck_operator_api": "0.2.0",
            "deck_id": EXTERNAL_DECK_ID,
            "deck_version": EXTERNAL_DECK_VERSION,
            "operator_id": "dev.latentdeck.private.h3_probe.operator",
            "operator_version": EXTERNAL_DECK_VERSION,
            "entrypoint": "latentdeck_private_h3_probe.operator:process_sources",
            "source_count": 2,
            "role_ids": ["carrier", "donor"],
            "controls": []
        }),
    )?;
    write_file(
        &source,
        "python/latentdeck_private_h3_probe/__init__.py",
        b"from .operator import process_sources\n\n__all__ = [\"process_sources\"]\n",
    )?;
    write_file(
        &source,
        "python/latentdeck_private_h3_probe/operator.py",
        br#"from __future__ import annotations

from latentdeck_deck_sdk import DeckOperatorResult, validate_process_call, validate_process_result


def process_sources(sources, controls, context):
    validate_process_call(sources, controls, context)
    if len(sources) != 2:
        raise ValueError("private probe requires exactly two sources")
    output = sources[0].clone().contiguous()
    result = DeckOperatorResult(
        output=output,
        provenance={
            "operator_id": "dev.latentdeck.private.h3_probe.operator",
            "operator_version": "0.2.0",
            "external_package_loaded": True,
        },
    )
    return validate_process_result(result, sources)
"#,
    )?;
    let catalog_bytes = write_integrity(
        &source,
        &[
            "NOTICE.txt",
            "faceplate.json",
            "operator.json",
            "python/latentdeck_private_h3_probe/__init__.py",
            "python/latentdeck_private_h3_probe/operator.py",
        ],
    )?;
    let dtype = match tensor.dtype {
        latentdeck_control::v2::TensorDtype::Float16 => ManifestTensorDtype::Fp16,
        latentdeck_control::v2::TensorDtype::Float32 => ManifestTensorDtype::Fp32,
        latentdeck_control::v2::TensorDtype::Bfloat16 => {
            return Err("external Deck manifest cannot declare the negotiated bfloat16 ABI");
        }
    };
    let channels = u16::try_from(tensor.shape[1])
        .map_err(|_| "external Deck channel count exceeded manifest bounds")?;
    let manifest = DeckPackManifest {
        manifest_version: "1.0.0".to_owned(),
        kind: PackageKind::DeckPack,
        deck_id: EXTERNAL_DECK_ID.to_owned(),
        deck_version: EXTERNAL_DECK_VERSION.to_owned(),
        display_name: "Private Protocol 2 external Deck probe".to_owned(),
        summary: "Test-generated exact installed Deck used only by the private GPU gate."
            .to_owned(),
        publisher: PublisherDescriptor {
            name: "LatentDeck private gate".to_owned(),
            url: None,
            identity_claim: PublisherIdentityClaim::SelfDeclared,
        },
        license: LicenseDescriptor {
            spdx_or_label: "Apache-2.0".to_owned(),
            notice_path: "NOTICE.txt".to_owned(),
        },
        compatibility: DeckCompatibility {
            app_min_inclusive: "0.1.0".to_owned(),
            app_max_exclusive: "1.0.0".to_owned(),
            deck_host_api: 1,
            worker_protocol: 2,
            deck_operator_api: 1,
            tensor_abi: "latentdeck.tensor.v1".to_owned(),
            python: PythonConstraint {
                implementation: PythonImplementation::Cpython,
                version: format!("{}.{}", tensor.python_major, tensor.python_minor),
                platform_tag: "win_amd64".to_owned(),
            },
            torch_exact_build: tensor.torch_version.clone(),
        },
        runtime: DeckRuntimeDescriptor {
            kind: DeckRuntimeKind::PythonOperatorStreamV1,
            operator_descriptor_path: "operator.json".to_owned(),
            python_root: "python".to_owned(),
            entrypoint: "latentdeck_private_h3_probe.operator:process_sources".to_owned(),
        },
        signal: DeckSignalDescriptor {
            slots: 2,
            roles: vec![
                DeckRoleDescriptor {
                    role_id: "carrier".to_owned(),
                    display_name: "Carrier".to_owned(),
                },
                DeckRoleDescriptor {
                    role_id: "donor".to_owned(),
                    display_name: "Donor".to_owned(),
                },
            ],
            default_permutation: vec!["carrier".to_owned(), "donor".to_owned()],
            structural_carrier_role: "carrier".to_owned(),
            geometry_allowlist: vec![ManifestSignalGeometry {
                dtype,
                device: TensorDevice::Cuda,
                batch: u8::try_from(tensor.shape[0])
                    .map_err(|_| "external Deck batch exceeded manifest bounds")?,
                channels,
                temporal: u8::try_from(tensor.shape[2])
                    .map_err(|_| "external Deck temporal extent exceeded manifest bounds")?,
                height: tensor.shape[3],
                width: tensor.shape[4],
            }],
            timing: timing.clone(),
            required_capabilities: vec![
                CodecCapability::Player,
                CodecCapability::Realtime,
                CodecCapability::Resample,
                CodecCapability::SnapshotCapture,
                CodecCapability::LiveCapture,
            ],
            profile_allowlist: Some(vec![ProfileKey {
                codec_family: profile.codec_family.clone(),
                profile: profile.profile.clone(),
                profile_version: profile.profile_version.clone(),
            }]),
        },
        faceplate_path: "faceplate.json".to_owned(),
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: sha256(&catalog_bytes),
        },
    };
    write_json(&source, "deck-pack.json", &manifest)?;
    let archive = config.work_root.join("private-h3-probe.ld");
    let packed = pack(&PackRequest {
        source_directory: source,
        output_path: archive.clone(),
    })
    .map_err(|_| "test-generated external .ld could not be packed")?;
    let package = external_deck_reference();
    if packed.inspection.package != package {
        return Err("test-generated external .ld changed its exact identity");
    }
    let archive_sha256 = packed.inspection.archive_sha256.clone();
    install(
        &config.roots,
        &latentdeck_extension_manager::InstallRequest {
            archive_path: archive,
            expected_sha256: archive_sha256.clone(),
        },
    )
    .map_err(|_| "test-generated external .ld could not be installed")?;
    enable(&config.roots, &package)
        .map_err(|_| "test-generated external .ld could not be enabled exactly")?;
    let active = resolve_active(&config.roots, &package)
        .map_err(|_| "test-generated external .ld did not become active without restart")?;
    if active.trust_receipt().archive_sha256 != archive_sha256 {
        return Err("test-generated external .ld trust identity changed after installation");
    }
    Ok(archive_sha256)
}

fn write_integrity(root: &Path, relative_paths: &[&str]) -> GateResult<Vec<u8>> {
    let mut files = relative_paths
        .iter()
        .map(|relative| {
            let bytes = fs::read(portable_path(root, relative))
                .map_err(|_| "external Deck integrity input could not be read")?;
            Ok(IntegrityFile {
                path: (*relative).to_owned(),
                byte_length: u64::try_from(bytes.len())
                    .map_err(|_| "external Deck file length overflow")?,
                sha256: sha256(&bytes),
            })
        })
        .collect::<GateResult<Vec<_>>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let bytes = serde_json::to_vec(&IntegrityCatalog {
        manifest_version: "1.0.0".to_owned(),
        files,
    })
    .map_err(|_| "external Deck integrity catalog could not be encoded")?;
    write_file(root, "integrity.json", &bytes)?;
    Ok(bytes)
}

fn write_json(root: &Path, relative: &str, value: &impl serde::Serialize) -> GateResult<()> {
    let bytes = serde_json::to_vec(value).map_err(|_| "external Deck JSON could not be encoded")?;
    write_file(root, relative, &bytes)
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) -> GateResult<()> {
    let path = portable_path(root, relative);
    let parent = path
        .parent()
        .ok_or("external Deck file has no parent directory")?;
    fs::create_dir_all(parent).map_err(|_| "external Deck directory could not be created")?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "external Deck file could not be created without overwrite")?;
    file.write_all(bytes)
        .map_err(|_| "external Deck file could not be written")?;
    file.sync_all()
        .map_err(|_| "external Deck file could not be committed")
}

fn portable_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

#[allow(clippy::too_many_arguments)]
fn build_receipt(
    config: &GateConfig,
    codec: &ExactCodecEvidence,
    profile: &latentdeck_control::v2::ProfileKey,
    tensor: &latentdeck_control::v2::TensorAbi,
    player: &SurfaceObservation,
    d2: &DeckObservation,
    q4: &DeckObservation,
    external: &ExternalDeckObservation,
    external_archive_sha256: &str,
    stability: Vec<StabilityObservation>,
) -> GateResult<Value> {
    let device = match tensor.device {
        latentdeck_control::v2::DeviceKind::Cuda => "cuda",
        latentdeck_control::v2::DeviceKind::Cpu => {
            return Err("private H3 GPU receipt cannot report a CPU tensor ABI");
        }
    };
    let source_hashes = config
        .sources
        .iter()
        .map(|source| Value::String(source.archive_sha256.clone()))
        .collect::<Vec<_>>();
    let stability = stability
        .into_iter()
        .map(|run| {
            json!({
                "surface": run.surface,
                "duration_seconds": run.duration_seconds,
                "sample_interval_seconds": SAMPLE_INTERVAL_SECONDS,
                "samples": run.samples,
                "worker_faults": 0,
                "host_faults": 0
            })
        })
        .collect::<Vec<_>>();
    let receipt = json!({
        "schema_version": 1,
        "evidence_kind": "latentdeck_private_protocol2_gpu_gate",
        "result": "passed",
        "source_commit": config.source_commit,
        "git_dirty": false,
        "protocol": {
            "worker_protocol": 2,
            "worker_module": codec.worker_module,
            "codec_host_api": "2.0",
            "codec_manifest_version": "2.0.0",
            "adapter_entrypoint": codec.adapter_entrypoint,
            "capabilities": codec.capabilities,
            "p1_fallback_attempted": false
        },
        "packages": {
            "codec": {"id": CODEC_ID, "version": CODEC_VERSION},
            "adapter": {"id": ADAPTER_ID, "version": ADAPTER_VERSION},
            "decks": [
                {"id": D2_ID, "version": BUNDLED_DECK_VERSION},
                {"id": Q4_ID, "version": BUNDLED_DECK_VERSION}
            ],
            "external_deck": {"id": EXTERNAL_DECK_ID, "version": EXTERNAL_DECK_VERSION}
        },
        "profile": {
            "codec_family": profile.codec_family,
            "profile": profile.profile,
            "profile_version": profile.profile_version,
            "python": format!("{}.{}", tensor.python_major, tensor.python_minor),
            "torch": tensor.torch_version,
            "device": device,
            "device_ordinal": 0
        },
        "inputs": {
            "codec_pack_sha256": codec.archive_sha256,
            "decoder_sha256": codec.decoder_binding.sha256,
            "source_archive_sha256": source_hashes,
            "external_deck_archive_sha256": external_archive_sha256
        },
        "coverage": {
            "player": player_coverage(player),
            "d2": deck_coverage(d2),
            "q4": deck_coverage(q4),
            "external_deck": external_deck_coverage(external),
            "stability": stability
        },
        "safety": {
            "conversion_attempted": false,
            "resize_attempted": false,
            "crop_attempted": false,
            "latent_reencode_attempted": false,
            "hidden_fallback_attempted": false,
            "private_paths_persisted": false
        }
    });
    validate_path_free_receipt(&receipt)?;
    Ok(receipt)
}

fn player_coverage(surface: &SurfaceObservation) -> Value {
    json!({
        "opened": surface.processed_frames > 0,
        "decoded_frames": surface.processed_frames,
        "reset_generation_before": surface.reset_generation_before,
        "reset_generation_after": surface.reset_generation_after,
        "reset_confirmed": reset_confirmed(surface),
        "status_checked": healthy_status(surface.status_state),
        "status_state": surface.status_state,
        "spout": spout_coverage(&surface.spout)
    })
}

fn deck_coverage(deck: &DeckObservation) -> Value {
    json!({
        "opened": deck.surface.processed_frames > 0,
        "processed_frames": deck.surface.processed_frames,
        "reset_generation_before": deck.surface.reset_generation_before,
        "reset_generation_after": deck.surface.reset_generation_after,
        "reset_confirmed": reset_confirmed(&deck.surface),
        "status_checked": healthy_status(deck.surface.status_state),
        "status_state": deck.surface.status_state,
        "snapshot": capture_coverage(&deck.snapshot),
        "live_capture": capture_coverage(&deck.live_capture),
        "mp4": {
            "finished": deck.mp4.finished,
            "frames_written": deck.mp4.frames_written,
            "byte_length": deck.mp4.byte_length
        },
        "spout": spout_coverage(&deck.surface.spout)
    })
}

fn external_deck_coverage(observation: &ExternalDeckObservation) -> Value {
    let surface = &observation.surface;
    json!({
        "opened": surface.processed_frames > 0,
        "processed_frames": surface.processed_frames,
        "reset_generation_before": surface.reset_generation_before,
        "reset_generation_after": surface.reset_generation_after,
        "reset_confirmed": reset_confirmed(surface),
        "status_checked": healthy_status(surface.status_state),
        "status_state": surface.status_state,
        "installed_after_runtime_start": observation.preexisting_sessions_remained_healthy
    })
}

fn capture_coverage(capture: &CaptureObservation) -> Value {
    json!({
        "finished": capture.finished,
        "imported": capture.imported,
        "reopened": capture.reopened,
        "latent_slots": capture.latent_slots,
        "decoded_frames": capture.decoded_frames
    })
}

fn spout_coverage(spout: &SpoutObservation) -> Value {
    json!({
        "enabled": spout.enabled_confirmed && spout.published_frames > 0,
        "published_frames": spout.published_frames,
        "sender_renamed": spout.sender_renamed,
        "renamed_published_frames": spout.renamed_published_frames,
        "disabled": spout.disabled_confirmed
    })
}

fn reset_confirmed(surface: &SurfaceObservation) -> bool {
    surface.reset_generation_after == surface.reset_generation_before.saturating_add(1)
}

fn healthy_status(value: &str) -> bool {
    matches!(value, "ready" | "playing" | "paused" | "end_of_stream")
}

fn write_validated_receipt(path: &Path, receipt: &Value) -> GateResult<()> {
    validate_path_free_receipt(receipt)?;
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|_| "private Protocol 2 receipt could not be encoded")?;
    if bytes.len() > 1024 * 1024 {
        return Err("private Protocol 2 receipt exceeded the 1 MiB bound");
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "private Protocol 2 receipt could not be created without overwrite")?;
    file.write_all(&bytes)
        .map_err(|_| "private Protocol 2 receipt could not be written")?;
    file.sync_all()
        .map_err(|_| "private Protocol 2 receipt could not be committed")
}

fn validate_path_free_receipt(value: &Value) -> GateResult<()> {
    match value {
        Value::String(text) if receipt_path_like(text) => {
            Err("private Protocol 2 receipt contains a machine-local path")
        }
        Value::Array(values) => values.iter().try_for_each(validate_path_free_receipt),
        Value::Object(values) => values.values().try_for_each(validate_path_free_receipt),
        _ => Ok(()),
    }
}

fn receipt_path_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.starts_with('/')
        || value.starts_with(r"\\")
        || lower.contains(":\\")
        || lower.contains(":/")
        || lower.contains("file://")
        || lower.contains("%localappdata%")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256Hash::from_bytes(Sha256::digest(bytes).into()).to_string()
}

async fn soak_player(player: &mut OpenPlayer) -> GateResult<StabilityObservation> {
    let baseline = player_metrics(player).await?;
    if baseline.commands_failed_total != 0 {
        return Err("Player entered stability sampling with worker command faults");
    }
    let started = Instant::now();
    let interval = Duration::from_secs(SAMPLE_INTERVAL_SECONDS);
    let mut next = tokio::time::Instant::now() + interval;
    let mut samples = 0_u64;
    let mut previous = baseline;
    loop {
        let _ = player_step_frames(player).await?;
        tokio::task::yield_now().await;
        if tokio::time::Instant::now() >= next {
            let _ = player_status(player).await?;
            let current = player_metrics(player).await?;
            validate_metrics_progress(&previous, &current, true)?;
            previous = current;
            samples = samples
                .checked_add(1)
                .ok_or("Player stability sample count overflow")?;
            next = tokio::time::Instant::now() + interval;
        }
        if started.elapsed() >= Duration::from_secs(STABILITY_SECONDS)
            && samples >= STABILITY_SECONDS / SAMPLE_INTERVAL_SECONDS
        {
            break;
        }
    }
    let duration_seconds = started.elapsed().as_secs();
    if duration_seconds < STABILITY_SECONDS
        || samples < STABILITY_SECONDS / SAMPLE_INTERVAL_SECONDS
        || previous.commands_failed_total != 0
    {
        return Err("Player did not complete the exact 360-second zero-fault stability gate");
    }
    Ok(StabilityObservation {
        surface: "player",
        duration_seconds,
        samples,
    })
}

async fn soak_deck(deck: &mut OpenDeck, label: &'static str) -> GateResult<StabilityObservation> {
    let baseline = deck_metrics(deck).await?;
    if baseline.commands_failed_total != 0 {
        return Err("Deck entered stability sampling with worker command faults");
    }
    let started = Instant::now();
    let interval = Duration::from_secs(SAMPLE_INTERVAL_SECONDS);
    let mut next = tokio::time::Instant::now() + interval;
    let mut samples = 0_u64;
    let mut previous = baseline;
    loop {
        let _ = deck_process_frames(deck).await?;
        tokio::task::yield_now().await;
        if tokio::time::Instant::now() >= next {
            let _ = deck_status(deck).await?;
            let current = deck_metrics(deck).await?;
            validate_metrics_progress(&previous, &current, false)?;
            previous = current;
            samples = samples
                .checked_add(1)
                .ok_or("Deck stability sample count overflow")?;
            next = tokio::time::Instant::now() + interval;
        }
        if started.elapsed() >= Duration::from_secs(STABILITY_SECONDS)
            && samples >= STABILITY_SECONDS / SAMPLE_INTERVAL_SECONDS
        {
            break;
        }
    }
    let duration_seconds = started.elapsed().as_secs();
    if duration_seconds < STABILITY_SECONDS
        || samples < STABILITY_SECONDS / SAMPLE_INTERVAL_SECONDS
        || previous.commands_failed_total != 0
    {
        return Err("Deck did not complete the exact 360-second zero-fault stability gate");
    }
    Ok(StabilityObservation {
        surface: label,
        duration_seconds,
        samples,
    })
}

async fn player_metrics(player: &mut OpenPlayer) -> GateResult<MetricsSnapshot> {
    let Ack::MetricsGet(metrics) = player
        .session
        .client_mut()
        .call(Command::MetricsGet(EmptyPayload {}), COMMAND_TIMEOUT)
        .await
        .map_err(|_| "Protocol 2 Player metrics.get failed")?
    else {
        return Err("Protocol 2 Player metrics.get returned the wrong acknowledgement");
    };
    Ok(metrics)
}

async fn deck_metrics(deck: &mut OpenDeck) -> GateResult<MetricsSnapshot> {
    let Ack::MetricsGet(metrics) = deck
        .session
        .client_mut()
        .call(Command::MetricsGet(EmptyPayload {}), COMMAND_TIMEOUT)
        .await
        .map_err(|_| "Protocol 2 Deck metrics.get failed")?
    else {
        return Err("Protocol 2 Deck metrics.get returned the wrong acknowledgement");
    };
    Ok(metrics)
}

fn validate_metrics_progress(
    previous: &MetricsSnapshot,
    current: &MetricsSnapshot,
    player: bool,
) -> GateResult<()> {
    if current.worker_uptime_ns < previous.worker_uptime_ns
        || current.commands_total <= previous.commands_total
        || current.commands_failed_total != previous.commands_failed_total
        || current.decoded_frames_total <= previous.decoded_frames_total
        || (player && current.player_steps_total <= previous.player_steps_total)
        || (!player && current.deck_process_total <= previous.deck_process_total)
    {
        return Err("Protocol 2 stability metrics stopped or reported a worker fault");
    }
    Ok(())
}

const fn player_state_token(state: PlayerState) -> GateResult<&'static str> {
    match state {
        PlayerState::Ready => Ok("ready"),
        PlayerState::Playing => Ok("playing"),
        PlayerState::Paused => Ok("paused"),
        PlayerState::EndOfStream => Ok("end_of_stream"),
        PlayerState::Empty | PlayerState::Loading | PlayerState::Faulted => {
            Err("Player status is not healthy")
        }
    }
}

const fn deck_state_token(state: DeckState) -> GateResult<&'static str> {
    match state {
        DeckState::Ready => Ok("ready"),
        DeckState::Playing => Ok("playing"),
        DeckState::Paused => Ok("paused"),
        DeckState::Capturing => Ok("capturing"),
        DeckState::Empty | DeckState::Loading | DeckState::Faulted => {
            Err("Deck status is not healthy")
        }
    }
}
