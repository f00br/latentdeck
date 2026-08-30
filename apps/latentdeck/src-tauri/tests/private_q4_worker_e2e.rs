//! Opt-in synthetic LD-Q4 proof against a linked development Codec Pack.
//!
//! The four source cartridges are generated from finite synthetic tensors in a
//! temporary directory. No cartridge, weight, media, or machine-local path is
//! checked into the repository. A real run additionally requires an installed
//! development Codec Pack, its externally bound TAEH3 decoder, and CUDA:
//!
//! ```text
//! LATENTDECK_PRIVATE_Q4_WORKER_E2E=1
//! LATENTDECK_PRIVATE_CODEC_ROOT=<codec-pack-discovery-root>
//! LATENTDECK_PRIVATE_TAEH3=<validated-external-decoder>
//! cargo test -p latentdeck-app --test private_q4_worker_e2e -- --ignored --nocapture
//! ```

#![cfg(target_os = "windows")]

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error,
    fs,
    io::{self, Cursor},
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
    time::Duration,
};

use atomicwrites::move_atomic;
use latentdeck_cartridge::{
    hash::{hash_path, hash_reader},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, DType, Identifier, ParentCartridge,
        Sha256Digest, SourceCartridgeRef, parse_manifest_json,
    },
    profile::h3::ValidatedH3Profile,
    reader::{ValidationOptions, open_validated},
    resample::{
        CaptureMode as CartridgeCaptureMode, PayloadExpectation, ResampleManifestRequest,
        pack_resample_atomic,
    },
    writer::{PackRequest, WriteOptions, pack_atomic},
};
use latentdeck_control::{
    Ack, BoundedVec, CodecLoad, Command, EmptyPayload, ExternalAssetBinding, FiniteF64,
    MAX_CONTROL_FRAME_BYTES, ProfileRef, Q4Algorithm, Q4CaptureAudioDtype, Q4CaptureAudioPolicy,
    Q4CaptureAudioPolicyReason, Q4CaptureMode, Q4CaptureReceipt, Q4CaptureStart, Q4CaptureState,
    Q4CaptureStatus, Q4CaptureStatusRequest, Q4CaptureStop, Q4CaptureVisualDtype, Q4Controls,
    Q4ControlsSet, Q4InfluenceMode, Q4Load, Q4Mode, Q4ProcessSlot, Q4ProcessSlotAck, Q4Reset,
    Q4ResetReason, Q4Restart, Q4Roles, Q4RolesSet, Q4SeedSet, Q4Slot, Q4SourceBinding, Q4Transport,
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
    ring::{ReadStatus, RingDescriptor},
    windows_ring::{WindowsRgbRingConsumer, WindowsRgbRingOwner},
};
use semver::Version;
use serde_json::{Value, json};
use tempfile::tempdir;

const OPT_IN_ENV: &str = "LATENTDECK_PRIVATE_Q4_WORKER_E2E";
const EXTERNAL_OPT_IN_ENV: &str = "LATENTDECK_PRIVATE_Q4_EXTERNAL_AV_E2E";
const CODEC_ROOT_ENV: &str = "LATENTDECK_PRIVATE_CODEC_ROOT";
const DECODER_ENV: &str = "LATENTDECK_PRIVATE_TAEH3";
const EXTERNAL_SOURCE_A_ENV: &str = "LATENTDECK_PRIVATE_Q4_SOURCE_A";
const EXTERNAL_SOURCE_B_ENV: &str = "LATENTDECK_PRIVATE_Q4_SOURCE_B";
const EXTERNAL_SOURCE_C_ENV: &str = "LATENTDECK_PRIVATE_Q4_SOURCE_C";
const EXTERNAL_SOURCE_D_ENV: &str = "LATENTDECK_PRIVATE_Q4_SOURCE_D";
const EXTERNAL_RECEIPT_ENV: &str = "LATENTDECK_PRIVATE_Q4_RECEIPT";

const PACK_ID: &str = "org.latentdeck.h3";
const ASSET_ID: &str = "taeh3";
const DECK_ID: &str = "synthetic-q4-e2e";
const OPERATOR_ID: &str = "org.latentdeck.builtin.ld_q4";
const OPERATOR_VERSION: &str = "0.1.0";
const INITIAL_GENERATION: u64 = 1;
const TEST_SEED: u64 = 4_204;
const CHANGED_SEED: u64 = 9_001;
const LATENT_SLOTS: u64 = 2;
const LATENT_HEIGHT: u64 = 2;
const LATENT_WIDTH: u64 = 2;
const DECODED_WIDTH: u32 = 32;
const DECODED_HEIGHT: u32 = 32;
const DECODED_FRAMES: u64 = 5;
const CAPTURE_VISUAL_LIMIT: u64 = 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn external_evidence_writer_is_atomic_and_rejects_machine_paths() -> TestResult<()> {
    let temporary = tempdir()?;
    let receipt_path = temporary.path().join("receipt.json");
    let safe = json!({
        "schema_version": 1,
        "source_order": ["B", "C", "A", "B"],
        "archives": [
            {"slot": "A", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
            {"slot": "B", "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
            {"slot": "C", "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},
            {"slot": "D", "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
        ]
    });
    persist_path_free_evidence(&receipt_path, &safe)?;
    require(
        fs::read(&receipt_path)? == serde_json::to_vec_pretty(&safe)?,
        "external Q4 evidence writer changed the canonical JSON payload",
    )?;
    require(
        !receipt_path.with_extension("json.partial").exists(),
        "external Q4 evidence writer left its atomic temporary file",
    )?;

    let unsafe_path = temporary.path().join("private-source.lc");
    let unsafe_value = json!({"source": path_text(&unsafe_path)?});
    let unsafe_receipt = temporary.path().join("unsafe.json");
    require(
        persist_path_free_evidence(&unsafe_receipt, &unsafe_value).is_err()
            && !unsafe_receipt.exists()
            && !unsafe_receipt.with_extension("json.partial").exists(),
        "external Q4 evidence writer accepted or partially wrote a machine-local path",
    )
}

#[test]
fn explicit_private_worker_run_rejects_missing_prerequisites_instead_of_skipping() {
    assert!(
        require_private_prerequisite_inputs(None, None, None).is_err(),
        "an explicitly selected ignored test must not turn missing opt-in into success",
    );
    assert!(
        require_private_prerequisite_inputs(Some("1"), None, None).is_err(),
        "an opted-in run must not turn missing paths into success",
    );
}

#[test]
fn four_source_topology_requires_four_unique_archives_and_keeps_duplicate_proof_separate()
-> TestResult<()> {
    let temporary = tempdir()?;
    let temporary_root = fs::canonicalize(temporary.path())?;
    let independent = create_synthetic_sources(&temporary_root)?;
    require_four_independent_topology(&independent)?;

    let mut disguised_payload_reuse = independent.clone();
    disguised_payload_reuse[3].video_payload_sha256 = independent[0].video_payload_sha256.clone();
    require(
        require_four_independent_topology(&disguised_payload_reuse).is_err(),
        "four unique IDs and archives must not hide a reused video payload",
    )?;

    let mut disguised_parent_reuse = independent.clone();
    disguised_parent_reuse[3].lineage_anchors = independent[0].lineage_anchors.clone();
    require(
        require_four_independent_topology(&disguised_parent_reuse).is_err(),
        "different derived outputs from one declared parent must not satisfy four-independent acceptance",
    )?;

    let duplicate = [
        independent[0].clone(),
        independent[1].clone(),
        independent[2].clone(),
        independent[1].clone(),
    ];
    require(
        require_four_independent_topology(&duplicate).is_err(),
        "duplicate topology must never satisfy four-independent acceptance",
    )?;
    require_declared_duplicate_sources(&duplicate)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LineageAnchor {
    cartridge_id: String,
    archive_sha256: String,
}

#[derive(Clone)]
struct SyntheticSource {
    path: PathBuf,
    cartridge_id: WireUuid,
    archive_sha256: String,
    video_payload_sha256: String,
    lineage_anchors: BTreeSet<LineageAnchor>,
    has_declared_parents: bool,
    profile: ValidatedH3Profile,
}

#[derive(Debug, PartialEq)]
struct CycleProof {
    frames: Vec<Vec<u8>>,
    provenance: Vec<Value>,
}

#[derive(Debug, PartialEq, Eq)]
struct ExternalStepProof {
    frame_sha256: Vec<String>,
    provenance_sha256: String,
    decoded_frame_count: u32,
}

#[derive(Debug)]
struct ExternalRuntimeProof {
    topk: ExternalStepProof,
    sinkhorn: ExternalStepProof,
    reassigned_carrier: ExternalStepProof,
    snapshot: Value,
    live_capture: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkerOutcome {
    Executed,
    SkippedNoCuda,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit opt-in, linked Q4 Codec Pack, external TAEH3, CUDA, and local GPU time"]
async fn synthetic_q4_topk_sinkhorn_roles_and_captures_are_deterministic() -> TestResult<()> {
    let temporary = tempdir()?;
    let temporary_root = fs::canonicalize(temporary.path())?;
    let sources = create_synthetic_sources(&temporary_root)?;
    require_independent_compatible_sources(&sources)?;

    let (pack, decoder) = resolve_private_prerequisites()?;

    match run_worker_proof(&pack, &decoder, &sources, &temporary_root).await? {
        WorkerOutcome::Executed => {
            require(
                !tree_contains_partial(&temporary_root)?,
                "Q4 E2E left an unfinished capture spool",
            )?;
        }
        WorkerOutcome::SkippedNoCuda => {
            return failure("private Q4 worker E2E requires CUDA device ordinal 0");
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit opt-in, linked Q4 Codec Pack, external TAEH3, CUDA, and local GPU time"]
async fn duplicate_source_q4_is_functional_without_claiming_four_source_acceptance()
-> TestResult<()> {
    let temporary = tempdir()?;
    let temporary_root = fs::canonicalize(temporary.path())?;
    let independent = create_synthetic_sources(&temporary_root)?;
    let sources = [
        independent[0].clone(),
        independent[1].clone(),
        independent[2].clone(),
        independent[1].clone(),
    ];
    require_declared_duplicate_sources(&sources)?;

    let (pack, decoder) = resolve_private_prerequisites()?;
    match run_worker_proof(&pack, &decoder, &sources, &temporary_root).await? {
        WorkerOutcome::Executed => {
            require(
                !tree_contains_partial(&temporary_root)?,
                "duplicate-source Q4 E2E left an unfinished capture spool",
            )?;
            eprintln!(
                "Q4 FUNCTIONAL ONLY: 3 distinct archives across 4 slots; slot D explicitly reuses slot B; this never satisfies four-independent-source release acceptance"
            );
        }
        WorkerOutcome::SkippedNoCuda => {
            return failure("duplicate-source Q4 worker E2E requires CUDA device ordinal 0");
        }
    }
    Ok(())
}

#[test]
#[ignore = "requires explicit private A/B/C AV cartridges"]
fn private_external_b_c_a_b_sources_are_exact_and_compatible() -> TestResult<()> {
    require_external_opt_in()?;
    let sources = resolve_duplicate_external_sources()?;
    require_duplicate_external_source_contract(&sources)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit private A/B/C AV cartridges, linked Q4 Codec Pack, external TAEH3, CUDA, and local GPU time"]
async fn private_external_b_c_a_b_av_q4_functional_proof() -> TestResult<()> {
    require_external_opt_in()?;
    let sources = resolve_duplicate_external_sources()?;
    require_duplicate_external_source_contract(&sources)?;
    let receipt_path = exact_env_path(EXTERNAL_RECEIPT_ENV)?;
    run_external_av_acceptance(&sources, &receipt_path).await
}

#[test]
#[ignore = "requires explicit private A/B/C/D AV cartridges"]
fn private_external_a_b_c_d_sources_are_exact_and_compatible() -> TestResult<()> {
    require_external_opt_in()?;
    let sources = resolve_four_independent_external_sources()?;
    require_four_independent_external_source_contract(&sources)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires four explicit private AV cartridges, linked Q4 Codec Pack, external TAEH3, CUDA, and local GPU time"]
async fn private_external_a_b_c_d_av_q4_release_proof() -> TestResult<()> {
    require_external_opt_in()?;
    let sources = resolve_four_independent_external_sources()?;
    require_four_independent_external_source_contract(&sources)?;
    let receipt_path = exact_env_path(EXTERNAL_RECEIPT_ENV)?;
    run_external_av_acceptance(&sources, &receipt_path).await
}

fn create_synthetic_sources(root: &Path) -> TestResult<[SyntheticSource; 4]> {
    const IDS: [&str; 4] = [
        "550e8400-e29b-41d4-a716-4466554400a0",
        "550e8400-e29b-41d4-a716-4466554400b0",
        "550e8400-e29b-41d4-a716-4466554400c0",
        "550e8400-e29b-41d4-a716-4466554400d0",
    ];
    let mut built = Vec::with_capacity(4);
    for (index, cartridge_id) in IDS.into_iter().enumerate() {
        let payload = synthetic_payload(index);
        let payload_path = root.join(format!("synthetic-{index}.safetensors"));
        let output_path = root.join(format!("synthetic-{index}.lc"));
        fs::write(&payload_path, &payload)?;
        let manifest = synthetic_manifest(&payload, cartridge_id)?;
        pack_atomic(
            &PackRequest::new(manifest, &payload_path),
            &output_path,
            &WriteOptions::default(),
        )?;
        built.push(validate_source(output_path)?);
    }
    built.try_into().map_err(|_| {
        io::Error::other("synthetic Q4 source construction did not produce four cartridges").into()
    })
}

fn synthetic_payload(variant: usize) -> Vec<u8> {
    const FINITE_HALF: [u16; 16] = [
        0x2400, 0x2800, 0x2c00, 0x3000, 0x3400, 0x3800, 0x3a00, 0x3c00, 0xa800, 0xac00, 0xb000,
        0xb400, 0xb800, 0xba00, 0xbc00, 0x0000,
    ];
    let element_count = usize::try_from(24 * LATENT_SLOTS * LATENT_HEIGHT * LATENT_WIDTH)
        .expect("small synthetic tensor size");
    let mut tensor_bytes = Vec::with_capacity(element_count * 2);
    for element in 0..element_count {
        let palette_index = (element * (variant + 1) + variant * 5) % FINITE_HALF.len();
        tensor_bytes.extend_from_slice(&FINITE_HALF[palette_index].to_le_bytes());
    }
    let mut header = format!(
        concat!(
            r#"{{"video":{{"data_offsets":[0,{}],"dtype":"F16","#,
            r#""shape":[1,24,{},{},{}]}}}}"#
        ),
        tensor_bytes.len(),
        LATENT_SLOTS,
        LATENT_HEIGHT,
        LATENT_WIDTH,
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::with_capacity(8 + header.len() + tensor_bytes.len());
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("small synthetic Safetensors header")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&tensor_bytes);
    payload
}

fn synthetic_manifest(
    payload: &[u8],
    cartridge_id: &str,
) -> TestResult<latentdeck_cartridge::manifest::ManifestV0_1> {
    let measured = hash_reader(&mut Cursor::new(payload))?;
    let value = json!({
        "spec_version": "0.1.0",
        "cartridge_id": cartridge_id,
        "codec": {
            "family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0"
        },
        "payloads": [{
            "path": "payloads/h3.safetensors",
            "media_type": "application/vnd.safetensors",
            "byte_length": measured.byte_length,
            "sha256": measured.sha256.to_string()
        }],
        "tensors": [{
            "stream": "visual",
            "name": "video",
            "payload": "payloads/h3.safetensors",
            "storage_dtype": "F16",
            "runtime_dtype": "F16",
            "shape": [1, 24, LATENT_SLOTS, LATENT_HEIGHT, LATENT_WIDTH]
        }],
        "timing": {
            "contract": "minimax_h3_causal",
            "contract_version": "0.1.0",
            "decoded_video": {
                "width": DECODED_WIDTH,
                "height": DECODED_HEIGHT,
                "frame_count": DECODED_FRAMES,
                "frame_rate": {"numerator": 24, "denominator": 1},
                "duration": {"numerator": 5, "denominator": 24}
            }
        },
        "audio": {"policy": "source_absent"},
        "provenance": {
            "created_by": {"name": "latentdeck-cartridge", "version": "0.1.0"},
            "sources": []
        },
        "parent_cartridges": [],
        "operation_history": []
    });
    let encoded = serde_json::to_vec(&value)?;
    Ok(parse_manifest_json(&encoded, &ValidationLimits::default())?)
}

fn validate_source(path: PathBuf) -> TestResult<SyntheticSource> {
    let path = fs::canonicalize(path)?;
    let mut cartridge = open_validated(&path, &ValidationOptions::default())?;
    let cartridge_id = parse_wire_uuid(&cartridge.manifest().cartridge_id.0)?;
    let archive_sha256 = cartridge.receipt().archive_sha256.to_string();
    let has_declared_parents = !cartridge.manifest().parent_cartridges.is_empty();
    let lineage_anchors = if has_declared_parents {
        cartridge
            .manifest()
            .parent_cartridges
            .iter()
            .map(|parent| LineageAnchor {
                cartridge_id: parent.cartridge_id.0.clone(),
                archive_sha256: parent.archive_sha256.0.clone(),
            })
            .collect()
    } else {
        [LineageAnchor {
            cartridge_id: cartridge.manifest().cartridge_id.0.clone(),
            archive_sha256: archive_sha256.clone(),
        }]
        .into_iter()
        .collect()
    };
    let video_payload_sha256 = hash_reader(&mut cartridge.tensor_reader("video")?)?
        .sha256
        .to_string();
    Ok(SyntheticSource {
        path,
        cartridge_id,
        archive_sha256,
        video_payload_sha256,
        lineage_anchors,
        has_declared_parents,
        profile: cartridge.h3_profile().clone(),
    })
}

fn require_independent_compatible_sources(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    let reference = &sources[0].profile;
    require_four_independent_topology(sources)?;
    require(
        sources.iter().all(|source| {
            source.profile.compatibility_key == reference.compatibility_key
                && source.profile.visual.decoded_width == DECODED_WIDTH
                && source.profile.visual.decoded_height == DECODED_HEIGHT
                && source.profile.visual.latent_slots == LATENT_SLOTS
                && source.profile.visual.latent_height == LATENT_HEIGHT
                && source.profile.visual.latent_width == LATENT_WIDTH
                && source.profile.audio.is_none()
        }),
        "synthetic Q4 sources do not share the exact visual-only H3 contract",
    )
}

fn require_declared_duplicate_sources(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    let ids = sources
        .iter()
        .map(|source| source.cartridge_id.to_string())
        .collect::<BTreeSet<_>>();
    let hashes = sources
        .iter()
        .map(|source| source.archive_sha256.clone())
        .collect::<BTreeSet<_>>();
    let video_payload_hashes = sources
        .iter()
        .map(|source| source.video_payload_sha256.clone())
        .collect::<BTreeSet<_>>();
    require(
        ids.len() == 3
            && hashes.len() == 3
            && video_payload_hashes.len() == 3
            && sources[1].cartridge_id == sources[3].cartridge_id
            && sources[1].archive_sha256 == sources[3].archive_sha256
            && sources[1].video_payload_sha256 == sources[3].video_payload_sha256
            && sources[1].lineage_anchors == sources[3].lineage_anchors
            && sources[1].path == sources[3].path,
        "duplicate-source Q4 fixture must declare exactly that slot D reuses slot B",
    )?;
    require(
        sources.iter().all(|source| {
            source.profile.compatibility_key == sources[0].profile.compatibility_key
                && source.profile.visual.decoded_width == DECODED_WIDTH
                && source.profile.visual.decoded_height == DECODED_HEIGHT
        }),
        "duplicate-source Q4 fixture is not codec compatible",
    )
}

fn require_four_independent_topology(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    let ids = sources
        .iter()
        .map(|source| source.cartridge_id.to_string())
        .collect::<BTreeSet<_>>();
    let hashes = sources
        .iter()
        .map(|source| source.archive_sha256.clone())
        .collect::<BTreeSet<_>>();
    let video_payload_hashes = sources
        .iter()
        .map(|source| source.video_payload_sha256.clone())
        .collect::<BTreeSet<_>>();
    require(
        ids.len() == 4
            && hashes.len() == 4
            && video_payload_hashes.len() == 4
            && lineage_anchors_are_pairwise_disjoint(sources),
        "four-independent Q4 acceptance requires four unique cartridge IDs, archive hashes, video payload hashes, and pairwise-disjoint declared immediate-parent (or original self) lineage",
    )
}

fn lineage_anchors_are_pairwise_disjoint(sources: &[SyntheticSource; 4]) -> bool {
    sources
        .iter()
        .all(|source| !source.lineage_anchors.is_empty())
        && (0..sources.len()).all(|left| {
            ((left + 1)..sources.len()).all(|right| {
                sources[left]
                    .lineage_anchors
                    .is_disjoint(&sources[right].lineage_anchors)
            })
        })
}

fn require_external_opt_in() -> TestResult<()> {
    require(
        env::var(EXTERNAL_OPT_IN_ENV).ok().as_deref() == Some("1"),
        "set LATENTDECK_PRIVATE_Q4_EXTERNAL_AV_E2E=1 to run private external AV acceptance",
    )
}

fn exact_env_path(name: &'static str) -> TestResult<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            io::Error::other(format!("required environment variable {name} is unset")).into()
        })
}

fn resolve_duplicate_external_sources() -> TestResult<[SyntheticSource; 4]> {
    let source_b = validate_source(exact_env_path(EXTERNAL_SOURCE_B_ENV)?)?;
    let source_c = validate_source(exact_env_path(EXTERNAL_SOURCE_C_ENV)?)?;
    let source_a = validate_source(exact_env_path(EXTERNAL_SOURCE_A_ENV)?)?;
    Ok([source_b.clone(), source_c, source_a, source_b])
}

fn resolve_four_independent_external_sources() -> TestResult<[SyntheticSource; 4]> {
    Ok([
        validate_source(exact_env_path(EXTERNAL_SOURCE_A_ENV)?)?,
        validate_source(exact_env_path(EXTERNAL_SOURCE_B_ENV)?)?,
        validate_source(exact_env_path(EXTERNAL_SOURCE_C_ENV)?)?,
        validate_source(exact_env_path(EXTERNAL_SOURCE_D_ENV)?)?,
    ])
}

fn require_duplicate_external_source_contract(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    let distinct_ids = sources
        .iter()
        .map(|source| source.cartridge_id.to_string())
        .collect::<BTreeSet<_>>();
    let distinct_hashes = sources
        .iter()
        .map(|source| source.archive_sha256.clone())
        .collect::<BTreeSet<_>>();
    let distinct_video_payloads = sources
        .iter()
        .map(|source| source.video_payload_sha256.clone())
        .collect::<BTreeSet<_>>();
    require(
        distinct_ids.len() == 3
            && distinct_hashes.len() == 3
            && distinct_video_payloads.len() == 3
            && sources[0].cartridge_id == sources[3].cartridge_id
            && sources[0].archive_sha256 == sources[3].archive_sha256
            && sources[0].video_payload_sha256 == sources[3].video_payload_sha256
            && sources[0].path == sources[3].path,
        "external Q4 topology must be exactly B,C,A,B with slot D reusing logical source B",
    )?;
    require_external_av_compatibility(sources)
}

fn require_four_independent_external_source_contract(
    sources: &[SyntheticSource; 4],
) -> TestResult<()> {
    require_four_independent_topology(sources)?;
    require_external_av_compatibility(sources)
}

fn require_external_av_compatibility(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    let reference = &sources[0].profile;
    require(
        sources
            .iter()
            .all(|source| source.profile.compatibility_key == reference.compatibility_key),
        "external Q4 sources have incompatible codec/profile, latent geometry, or timing contracts",
    )?;
    require(
        sources.iter().all(|source| {
            source.profile.visual.decoded_width == reference.visual.decoded_width
                && source.profile.visual.decoded_height == reference.visual.decoded_height
                && source.profile.visual.latent_width == reference.visual.latent_width
                && source.profile.visual.latent_height == reference.visual.latent_height
                && source.profile.audio.is_some()
        }),
        "external Q4 AV sources must share exact decoded and latent spatial geometry and retain audio",
    )
}

fn require_external_sources_unchanged(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    for source in sources {
        let measured = validate_source(source.path.clone())?;
        require(
            measured.path == source.path
                && measured.cartridge_id == source.cartridge_id
                && measured.archive_sha256 == source.archive_sha256
                && measured.video_payload_sha256 == source.video_payload_sha256
                && measured.lineage_anchors == source.lineage_anchors
                && measured.has_declared_parents == source.has_declared_parents,
            "an external AV source changed while the private Q4 proof was running",
        )?;
    }
    Ok(())
}

fn external_source_order(sources: &[SyntheticSource; 4]) -> [&'static str; 4] {
    if sources[0].archive_sha256 == sources[3].archive_sha256 {
        ["B", "C", "A", "B"]
    } else {
        ["A", "B", "C", "D"]
    }
}

fn collect_evidence_execution_context(pack: &ValidatedCodecPack) -> TestResult<Value> {
    let repository_root = fs::canonicalize(PathBuf::from(git_text(
        None,
        &["rev-parse", "--show-toplevel"],
    )?))?;
    let git_commit = git_text(Some(&repository_root), &["rev-parse", "--verify", "HEAD"])?;
    let head_tree = git_text(
        Some(&repository_root),
        &["rev-parse", "--verify", "HEAD^{tree}"],
    )?;
    let index_tree = git_text(Some(&repository_root), &["write-tree"])?;
    require(
        is_git_object_id(&git_commit)
            && is_git_object_id(&head_tree)
            && is_git_object_id(&index_tree)
            && head_tree == index_tree,
        "private Q4 evidence requires an index identical to the committed HEAD tree",
    )?;
    require(
        git_exit_success(
            &repository_root,
            &[
                "diff",
                "--quiet",
                "--exit-code",
                "--ignore-submodules=none",
                "--",
            ],
        )? && git_exit_success(
            &repository_root,
            &[
                "diff",
                "--cached",
                "--quiet",
                "--exit-code",
                "--ignore-submodules=none",
                "--",
            ],
        )? && git_text(
            Some(&repository_root),
            &["ls-files", "--others", "--exclude-standard"],
        )?
        .is_empty(),
        "private Q4 evidence requires clean index, worktree, and untracked state",
    )?;

    let cargo_lock = hash_path(repository_root.join("Cargo.lock"))?;
    let test_executable = hash_path(&env::current_exe()?)?;
    let worker_executable = hash_path(&pack.worker_executable)?;
    let codec_pack_manifest = hash_path(pack.root.join("codec-pack.json"))?;
    let integrity_catalog = hash_path(
        pack.root
            .join(pack.manifest.integrity.catalog_path.as_str()),
    )?;
    require(
        integrity_catalog.sha256.to_string() == pack.manifest.integrity.catalog_sha256,
        "validated Codec Pack integrity catalog changed before Q4 execution",
    )?;

    Ok(json!({
        "schema_version": 1,
        "git": {
            "commit": git_commit,
            "head_tree": head_tree,
            "index_tree": index_tree,
            "index_clean": true,
            "worktree_clean": true,
            "untracked_clean": true
        },
        "cargo_lock": {
            "sha256": cargo_lock.sha256.to_string(),
            "byte_length": cargo_lock.byte_length
        },
        "test_executable": {
            "sha256": test_executable.sha256.to_string(),
            "byte_length": test_executable.byte_length
        },
        "worker_executable": {
            "sha256": worker_executable.sha256.to_string(),
            "byte_length": worker_executable.byte_length
        },
        "codec_pack_manifest": {
            "sha256": codec_pack_manifest.sha256.to_string(),
            "byte_length": codec_pack_manifest.byte_length
        },
        "codec_pack_integrity_catalog": {
            "sha256": integrity_catalog.sha256.to_string(),
            "byte_length": integrity_catalog.byte_length,
            "validated": true
        }
    }))
}

fn git_text(repository_root: Option<&Path>, arguments: &[&str]) -> TestResult<String> {
    let mut command = ProcessCommand::new("git");
    if let Some(root) = repository_root {
        command.current_dir(root);
    }
    let output = command.args(arguments).output()?;
    require(
        output.status.success(),
        "git could not resolve the private Q4 evidence execution context",
    )?;
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn git_exit_success(repository_root: &Path, arguments: &[&str]) -> TestResult<bool> {
    let status = ProcessCommand::new("git")
        .current_dir(repository_root)
        .args(arguments)
        .status()?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => failure("git failed while checking the private Q4 evidence tree"),
    }
}

fn is_git_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[allow(
    clippy::too_many_lines,
    reason = "path-free external receipt assembly keeps every measured acceptance claim adjacent"
)]
async fn run_external_av_acceptance(
    sources: &[SyntheticSource; 4],
    receipt_path: &Path,
) -> TestResult<()> {
    let codec_root = exact_env_path(CODEC_ROOT_ENV)?;
    let decoder_path = exact_env_path(DECODER_ENV)?;
    let packs = discover_codec_packs(
        std::slice::from_ref(&codec_root),
        latentdeck_core::product_version(),
    )?;
    let pack = select_q4_pack(packs)
        .ok_or_else(|| io::Error::other("the exact discovery root has no compatible H3 Q4 pack"))?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;
    let execution_context = collect_evidence_execution_context(&pack)?;
    let temporary = tempdir()?;
    let temporary_root = fs::canonicalize(temporary.path())?;

    let runtime = run_external_worker_proof(&pack, &decoder, sources, &temporary_root).await?;
    require_external_sources_unchanged(sources)?;
    let post_pack = select_q4_pack(discover_codec_packs(
        std::slice::from_ref(&codec_root),
        latentdeck_core::product_version(),
    )?)
    .ok_or_else(|| {
        io::Error::other("the validated H3 Q4 pack disappeared during private execution")
    })?;
    let post_decoder = validate_external_asset(&post_pack, ASSET_ID, decoder.path.clone())?;
    require(
        collect_evidence_execution_context(&post_pack)? == execution_context
            && post_decoder.asset_id == decoder.asset_id
            && post_decoder.variant_id == decoder.variant_id
            && post_decoder.sha256 == decoder.sha256
            && post_decoder.byte_length == decoder.byte_length,
        "private Q4 execution context changed while the worker proof was running",
    )?;
    require(
        !tree_contains_partial(&temporary_root)?,
        "external AV Q4 acceptance left an unfinished capture or resample partial",
    )?;
    let source_order = external_source_order(sources);
    let source_slots = ["A", "B", "C", "D"];
    let distinct_cartridge_id_count = sources
        .iter()
        .map(|source| source.cartridge_id.to_string())
        .collect::<BTreeSet<_>>()
        .len();
    let distinct_archive_count = sources
        .iter()
        .map(|source| &source.archive_sha256)
        .collect::<BTreeSet<_>>()
        .len();
    let distinct_video_payload_count = sources
        .iter()
        .map(|source| &source.video_payload_sha256)
        .collect::<BTreeSet<_>>()
        .len();
    let lineage_anchor_count = sources
        .iter()
        .map(|source| source.lineage_anchors.len())
        .sum::<usize>();
    let distinct_lineage_anchor_count = sources
        .iter()
        .flat_map(|source| source.lineage_anchors.iter())
        .collect::<BTreeSet<_>>()
        .len();
    let lineage_pairwise_disjoint = lineage_anchors_are_pairwise_disjoint(sources);
    let four_independent_source_acceptance = distinct_cartridge_id_count == 4
        && distinct_archive_count == 4
        && distinct_video_payload_count == 4
        && lineage_pairwise_disjoint;
    let duplicate_binding = if four_independent_source_acceptance {
        Value::Null
    } else {
        json!({
            "slot": "D",
            "logical_source": "B",
            "same_physical_identity_as_slot": "A"
        })
    };
    let initial_carrier = &sources[0];
    let source_evidence = sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            json!({
                "slot": source_slots[index],
                "logical_source": source_order[index],
                "cartridge_id": source.cartridge_id.to_string(),
                "archive_sha256": source.archive_sha256,
                "video_payload_sha256": source.video_payload_sha256,
                "lineage_basis": if source.has_declared_parents {
                    "declared_immediate_parents"
                } else {
                    "original_self"
                },
                "lineage_anchors": source.lineage_anchors.iter().map(|anchor| json!({
                    "cartridge_id": anchor.cartridge_id,
                    "archive_sha256": anchor.archive_sha256
                })).collect::<Vec<_>>(),
                "visual_shape": [
                    1,
                    24,
                    source.profile.visual.latent_slots,
                    source.profile.visual.latent_height,
                    source.profile.visual.latent_width
                ],
                "decoded": {
                    "width": source.profile.visual.decoded_width,
                    "height": source.profile.visual.decoded_height,
                    "frame_count": source.profile.visual.decoded_frame_count,
                    "frame_rate": "24/1"
                },
                "audio_shape": source.profile.audio.as_ref().map(|audio| [1, 32, 2, audio.latent_slots])
            })
        })
        .collect::<Vec<_>>();
    let evidence = json!({
        "schema_version": 2,
        "test_id": if four_independent_source_acceptance {
            "private_external_a_b_c_d_av_q4_release_proof"
        } else {
            "private_external_b_c_a_b_av_q4_functional_proof"
        },
        "acceptance_class": if four_independent_source_acceptance {
            "release_four_independent"
        } else {
            "functional_duplicate_reuse_only"
        },
        "result": if four_independent_source_acceptance {
            "passed"
        } else {
            "functional_only_passed"
        },
        "latentdeck_version": latentdeck_core::product_version(),
        "execution_context": execution_context,
        "codec_pack": {
            "pack_id": pack.manifest.pack_id,
            "pack_version": pack.manifest.pack_version,
            "adapter_id": pack.manifest.adapter.adapter_id,
            "adapter_version": pack.manifest.adapter.adapter_version,
            "integrity_catalog_sha256": pack.manifest.integrity.catalog_sha256,
            "decoder_asset_id": decoder.asset_id,
            "decoder_sha256": decoder.sha256,
            "decoder_byte_length": decoder.byte_length
        },
        "source_order": source_order,
        "source_count": 4,
        "distinct_cartridge_id_count": distinct_cartridge_id_count,
        "distinct_archive_count": distinct_archive_count,
        "distinct_video_payload_count": distinct_video_payload_count,
        "lineage_rule": "declared_immediate_parents_or_original_self",
        "lineage_anchor_count": lineage_anchor_count,
        "distinct_lineage_anchor_count": distinct_lineage_anchor_count,
        "lineage_pairwise_disjoint": lineage_pairwise_disjoint,
        "four_independent_source_acceptance": four_independent_source_acceptance,
        "duplicate_binding": duplicate_binding,
        "sources": source_evidence,
        "initial_carrier": {
            "slot": "A",
            "logical_source": source_order[0],
            "latent_slots": initial_carrier.profile.visual.latent_slots,
            "decoded_frame_count": initial_carrier.profile.visual.decoded_frame_count,
            "decoded_width": initial_carrier.profile.visual.decoded_width,
            "decoded_height": initial_carrier.profile.visual.decoded_height,
            "audio_latent_slots": initial_carrier.profile.audio.as_ref().map(|audio| audio.latent_slots)
        },
        "effects": {
            "topk": external_step_evidence(&runtime.topk),
            "sinkhorn": external_step_evidence(&runtime.sinkhorn),
            "topk_vs_sinkhorn_distinct": runtime.topk.frame_sha256 != runtime.sinkhorn.frame_sha256,
            "deterministic_restart_replay": true,
            "carrier_reassignment": {
                "from_slot": "A",
                "from_logical_source": source_order[0],
                "to_slot": "B",
                "to_logical_source": source_order[1],
                "decoded_effect": external_step_evidence(&runtime.reassigned_carrier),
                "distinct_from_preceding_sinkhorn": runtime.reassigned_carrier.frame_sha256 != runtime.sinkhorn.frame_sha256
            }
        },
        "snapshot": runtime.snapshot,
        "live_capture": runtime.live_capture,
        "cleanup": {
            "partial_files_remaining": false,
            "private_paths_persisted": false
        }
    });
    let forbidden_paths = sources
        .iter()
        .map(|source| source.path.as_path())
        .chain([
            codec_root.as_path(),
            decoder.path.as_path(),
            temporary_root.as_path(),
            receipt_path,
        ])
        .collect::<Vec<_>>();
    require(
        forbidden_paths.iter().try_fold(true, |clean, path| {
            Ok::<_, Box<dyn Error + Send + Sync>>(
                clean && !json_contains_fragment(&evidence, &path_text(path)?),
            )
        })?,
        "external Q4 evidence serialized a private source, runtime, spool, or output path",
    )?;
    persist_path_free_evidence(receipt_path, &evidence)?;
    let persisted: Value = serde_json::from_slice(&fs::read(receipt_path)?)?;
    require(
        persisted == evidence && !json_contains_machine_path(&persisted),
        "persisted external Q4 evidence changed or gained a machine-local path",
    )
}

fn external_step_evidence(proof: &ExternalStepProof) -> Value {
    json!({
        "decoded_frame_count": proof.decoded_frame_count,
        "frame_sha256": proof.frame_sha256,
        "provenance_sha256": proof.provenance_sha256
    })
}

async fn run_external_worker_proof(
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &[SyntheticSource; 4],
    temporary_root: &Path,
) -> TestResult<ExternalRuntimeProof> {
    let launch = ValidatedWorkerLaunch::from_codec_pack_q4(pack)?;
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);
    let exercise =
        exercise_external_worker(&mut client, pack, decoder, sources, temporary_root).await;
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "external Q4 worker returned an unsuccessful orderly exit",
        ),
        Err(_) => client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>),
    };
    match (exercise, shutdown) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(proof), Ok(())) => Ok(proof),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one bounded external worker session keeps algorithms, roles, and capture evidence causally adjacent"
)]
async fn exercise_external_worker(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &[SyntheticSource; 4],
    temporary_root: &Path,
) -> TestResult<ExternalRuntimeProof> {
    configure_session(client).await?;
    require(
        inspect_runtime(client, pack).await?,
        "external AV Q4 acceptance requires CUDA device ordinal 0",
    )?;
    load_codec(client, pack, decoder).await?;

    let initial_roles = Q4Roles::default();
    let topk_controls = Q4Controls {
        chaos: finite(0.0),
        top_k: 8,
        ..xs5_controls(Q4Xs5Routing::TopK)
    };
    let loaded = client
        .deck_q4_load(
            Q4Load {
                deck_id: DECK_ID.to_owned(),
                operator_id: OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: source_binding(&sources[0])?,
                source_b: source_binding(&sources[1])?,
                source_c: source_binding(&sources[2])?,
                source_d: source_binding(&sources[3])?,
                roles: initial_roles,
                controls: topk_controls.clone(),
                transport: Q4Transport::default(),
                seed: TEST_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require_external_loaded_sources(&loaded, sources)?;

    let carrier = &sources[0];
    let descriptor = RingDescriptor::new(
        carrier.profile.visual.decoded_width,
        carrier.profile.visual.decoded_height,
        INITIAL_GENERATION,
    )?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;

    let mut generation = INITIAL_GENERATION;
    let topk = process_external_one(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        0,
        0,
        initial_roles,
        &topk_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let topk_replay = process_external_one(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        0,
        0,
        initial_roles,
        &topk_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    require(
        topk == topk_replay,
        "real external Q4 TOPK changed decoded output or provenance after restart/replay",
    )?;

    let sinkhorn_controls = Q4Controls {
        xs5_routing: Q4Xs5Routing::Sinkhorn,
        sinkhorn_iterations: 5,
        ..topk_controls.clone()
    };
    client
        .deck_q4_controls_set(
            Q4ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                controls: sinkhorn_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let sinkhorn = process_external_one(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        0,
        0,
        initial_roles,
        &sinkhorn_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let sinkhorn_replay = process_external_one(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        0,
        0,
        initial_roles,
        &sinkhorn_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    require(
        sinkhorn == sinkhorn_replay,
        "real external Q4 Sinkhorn changed decoded output or provenance after restart/replay",
    )?;
    require(
        topk.frame_sha256 != sinkhorn.frame_sha256,
        "real external Q4 TOPK and Sinkhorn produced the same decoded effect",
    )?;

    let reassigned_roles = Q4Roles {
        carrier: Q4Slot::B,
        donor_b: Q4Slot::A,
        donor_c: Q4Slot::C,
        donor_d: Q4Slot::D,
    };
    let roles_ack = client
        .deck_q4_roles_set(
            Q4RolesSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                roles: reassigned_roles,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        roles_ack.roles == reassigned_roles && !roles_ack.requires_causal_reset,
        "real Q4 carrier reassignment did not preserve the exact requested role permutation",
    )?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let reassigned_carrier = process_external_one(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        0,
        0,
        reassigned_roles,
        &sinkhorn_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    require(
        reassigned_carrier.frame_sha256 != sinkhorn.frame_sha256,
        "real Q4 carrier reassignment from logical B to logical C had no decoded effect",
    )?;

    client
        .deck_q4_roles_set(
            Q4RolesSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                roles: initial_roles,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let (snapshot, snapshot_generation) = external_snapshot_capture(
        client,
        loaded.deck_revision,
        generation,
        initial_roles,
        &sinkhorn_controls,
        sources,
        decoder,
        temporary_root,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let live_capture = external_live_capture(
        client,
        loaded.deck_revision,
        snapshot_generation,
        initial_roles,
        &sinkhorn_controls,
        sources,
        decoder,
        temporary_root,
        &mut owner,
        &mut consumer,
    )
    .await?;
    Ok(ExternalRuntimeProof {
        topk,
        sinkhorn,
        reassigned_carrier,
        snapshot,
        live_capture,
    })
}

fn require_external_loaded_sources(
    status: &latentdeck_control::Q4Status,
    sources: &[SyntheticSource; 4],
) -> TestResult<()> {
    let reported = [
        &status.source_a,
        &status.source_b,
        &status.source_c,
        &status.source_d,
    ];
    require(
        reported.iter().zip(sources).all(|(reported, source)| {
            reported.cartridge_id == source.cartridge_id
                && reported.archive_sha256 == source.archive_sha256
                && reported.latent_slot_count == source.profile.visual.latent_slots
        }),
        "Q4 worker load status changed an external source identity or temporal length",
    )
}

fn require_private_prerequisite_inputs(
    opt_in: Option<&str>,
    codec_root: Option<PathBuf>,
    decoder_path: Option<PathBuf>,
) -> TestResult<(PathBuf, PathBuf)> {
    require(
        opt_in == Some("1"),
        "set LATENTDECK_PRIVATE_Q4_WORKER_E2E=1 to run the private Q4 worker E2E",
    )?;
    let codec_root = codec_root.ok_or_else(|| {
        io::Error::other("LATENTDECK_PRIVATE_CODEC_ROOT is required for private Q4 worker E2E")
    })?;
    let decoder_path = decoder_path.ok_or_else(|| {
        io::Error::other("LATENTDECK_PRIVATE_TAEH3 is required for private Q4 worker E2E")
    })?;
    Ok((codec_root, decoder_path))
}

fn resolve_private_prerequisites() -> TestResult<(ValidatedCodecPack, ValidatedExternalAsset)> {
    let opt_in = env::var(OPT_IN_ENV).ok();
    let (codec_root, decoder_path) = require_private_prerequisite_inputs(
        opt_in.as_deref(),
        env_path(CODEC_ROOT_ENV),
        env_path(DECODER_ENV),
    )?;
    let packs = discover_codec_packs(
        std::slice::from_ref(&codec_root),
        latentdeck_core::product_version(),
    )?;
    let pack = select_q4_pack(packs).ok_or_else(|| {
        io::Error::other("configured Codec Pack root has no compatible H3 Q4 entrypoint")
    })?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;
    Ok((pack, decoder))
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn select_q4_pack(mut packs: Vec<ValidatedCodecPack>) -> Option<ValidatedCodecPack> {
    packs.retain(|pack| {
        pack.manifest.pack_id == PACK_ID && pack.manifest.worker.q4_arguments.is_some()
    });
    packs.sort_by(|left, right| {
        let left = Version::parse(&left.manifest.pack_version).expect("validated pack SemVer");
        let right = Version::parse(&right.manifest.pack_version).expect("validated pack SemVer");
        left.cmp(&right)
    });
    packs.pop()
}

async fn run_worker_proof(
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &[SyntheticSource; 4],
    temporary_root: &Path,
) -> TestResult<WorkerOutcome> {
    let launch = ValidatedWorkerLaunch::from_codec_pack_q4(pack)?;
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);

    let exercise = exercise_worker(&mut client, pack, decoder, sources, temporary_root).await;
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "Q4 worker returned an unsuccessful orderly exit",
        ),
        Err(_) => client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>),
    };
    match (exercise, shutdown) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(outcome), Ok(())) => Ok(outcome),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "linear opt-in proof keeps one real worker session and its causal generations explicit"
)]
async fn exercise_worker(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    sources: &[SyntheticSource; 4],
    temporary_root: &Path,
) -> TestResult<WorkerOutcome> {
    configure_session(client).await?;
    if !inspect_runtime(client, pack).await? {
        return Ok(WorkerOutcome::SkippedNoCuda);
    }
    load_codec(client, pack, decoder).await?;

    let initial_roles = Q4Roles::default();
    let topk_controls = xs5_controls(Q4Xs5Routing::TopK);
    let loaded = client
        .deck_q4_load(
            Q4Load {
                deck_id: DECK_ID.to_owned(),
                operator_id: OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: source_binding(&sources[0])?,
                source_b: source_binding(&sources[1])?,
                source_c: source_binding(&sources[2])?,
                source_d: source_binding(&sources[3])?,
                roles: initial_roles,
                controls: topk_controls.clone(),
                transport: Q4Transport::default(),
                seed: TEST_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        loaded.operator_id == OPERATOR_ID
            && loaded.operator_version == OPERATOR_VERSION
            && loaded.roles == initial_roles
            && loaded.controls == topk_controls
            && loaded.seed == TEST_SEED
            && loaded.stream_generation == INITIAL_GENERATION
            && loaded.stream_sequence == 0,
        "Q4 worker changed the exact initial operator, roles, controls, seed, or clock",
    )?;
    require_loaded_sources(&loaded, sources)?;

    let descriptor = RingDescriptor::new(DECODED_WIDTH, DECODED_HEIGHT, INITIAL_GENERATION)?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;

    let mut generation = INITIAL_GENERATION;
    let topk_first = process_cycle(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        initial_roles,
        &topk_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let topk_repeat = process_cycle(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        initial_roles,
        &topk_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    require(
        topk_first == topk_repeat,
        "Q4 TOPK changed decoded frames or canonical provenance after restart/reset replay",
    )?;

    let permuted_roles = Q4Roles {
        carrier: Q4Slot::C,
        donor_b: Q4Slot::A,
        donor_c: Q4Slot::D,
        donor_d: Q4Slot::B,
    };
    let influenced_controls = Q4Controls {
        donor_weight_b: finite(0.1),
        donor_weight_c: finite(0.2),
        donor_weight_d: finite(0.7),
        ..topk_controls.clone()
    };
    let roles_ack = client
        .deck_q4_roles_set(
            Q4RolesSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                roles: permuted_roles,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    let controls_ack = client
        .deck_q4_controls_set(
            Q4ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                controls: influenced_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        roles_ack.roles == permuted_roles
            && controls_ack.controls == influenced_controls
            && !roles_ack.requires_causal_reset
            && !controls_ack.requires_causal_reset,
        "Q4 role permutation or donor influence update was not atomic",
    )?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let influenced = process_cycle(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        permuted_roles,
        &influenced_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    require(
        influenced.frames != topk_first.frames,
        "Q4 carrier permutation and donor influences had no decoded effect",
    )?;

    let sinkhorn_controls = Q4Controls {
        xs5_routing: Q4Xs5Routing::Sinkhorn,
        ..influenced_controls.clone()
    };
    let sinkhorn_ack = client
        .deck_q4_controls_set(
            Q4ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                controls: sinkhorn_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        sinkhorn_ack.controls == sinkhorn_controls && !sinkhorn_ack.requires_causal_reset,
        "Q4 Sinkhorn routing update was not atomic",
    )?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let sinkhorn_first = process_cycle(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        permuted_roles,
        &sinkhorn_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    generation = restart_and_reset(
        client,
        loaded.deck_revision,
        generation,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let sinkhorn_repeat = process_cycle(
        client,
        &mut consumer,
        loaded.deck_revision,
        generation,
        permuted_roles,
        &sinkhorn_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    require(
        sinkhorn_first == sinkhorn_repeat,
        "Q4 Sinkhorn changed decoded frames or canonical provenance after restart/reset replay",
    )?;

    generation = snapshot_capture(
        client,
        loaded.deck_revision,
        generation,
        permuted_roles,
        &sinkhorn_controls,
        sources,
        decoder,
        temporary_root,
        &mut owner,
        &mut consumer,
    )
    .await?;
    live_capture(
        client,
        loaded.deck_revision,
        generation,
        permuted_roles,
        &sinkhorn_controls,
        sources,
        decoder,
        temporary_root,
        &mut owner,
        &mut consumer,
    )
    .await?;
    Ok(WorkerOutcome::Executed)
}

fn require_loaded_sources(
    status: &latentdeck_control::Q4Status,
    sources: &[SyntheticSource; 4],
) -> TestResult<()> {
    let reported = [
        &status.source_a,
        &status.source_b,
        &status.source_c,
        &status.source_d,
    ];
    require(
        reported.iter().zip(sources).all(|(reported, source)| {
            reported.cartridge_id == source.cartridge_id
                && reported.archive_sha256 == source.archive_sha256
                && reported.latent_slot_count == LATENT_SLOTS
        }),
        "Q4 worker load status changed a synthetic source identity or temporal length",
    )
}

fn xs5_controls(routing: Q4Xs5Routing) -> Q4Controls {
    Q4Controls {
        algorithm: Q4Algorithm::Xs5,
        interaction: finite(0.8),
        mode: Q4Mode::Interact,
        preserve: finite(0.25),
        influence_mode: Q4InfluenceMode::Manual,
        donor_weight_b: finite(0.6),
        donor_weight_c: finite(0.3),
        donor_weight_d: finite(0.1),
        triangle_x: finite(0.5),
        triangle_y: finite(0.5),
        xs5_routing: routing,
        temperature: finite(0.2),
        top_k: 2,
        sinkhorn_iterations: 4,
        chaos: finite(0.125),
    }
}

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("finite in-range first-party Q4 test value")
}

#[allow(clippy::too_many_arguments)]
async fn process_cycle(
    client: &mut WorkerClient,
    consumer: &mut WindowsRgbRingConsumer,
    deck_revision: u64,
    generation: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    seed: u64,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
) -> TestResult<CycleProof> {
    let mut frames = Vec::new();
    let mut provenance = Vec::new();
    let mut decoded_start = 0_u64;
    for expected_playhead in 0..LATENT_SLOTS {
        let step = process_one(
            client,
            consumer,
            deck_revision,
            generation,
            expected_playhead,
            decoded_start,
            roles,
            controls,
            seed,
            sources,
            decoder,
        )
        .await?;
        decoded_start += u64::try_from(step.frames.len()).expect("bounded decoded frame count");
        frames.extend(step.frames);
        provenance.push(step.provenance);
    }
    require(
        decoded_start == DECODED_FRAMES
            && frames.len() == usize::try_from(DECODED_FRAMES).expect("small frame count"),
        "Q4 synthetic T=2 cycle did not decode exactly five H3 frames",
    )?;
    Ok(CycleProof { frames, provenance })
}

struct StepProof {
    frames: Vec<Vec<u8>>,
    provenance: Value,
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one decoded-slot proof binds worker acknowledgement, provenance, ring frames, and hashes"
)]
async fn process_external_one(
    client: &mut WorkerClient,
    consumer: &mut WindowsRgbRingConsumer,
    deck_revision: u64,
    generation: u64,
    expected_step: u64,
    expected_decoded_start: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    seed: u64,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
) -> TestResult<ExternalStepProof> {
    let ack = client
        .deck_q4_process_slot(
            Q4ProcessSlot {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                stream_generation: generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    let Q4ProcessSlotAck::DecodedSlot {
        deck_id,
        deck_revision: ack_revision,
        stream_generation,
        playhead_a,
        playhead_b,
        playhead_c,
        playhead_d,
        roles: ack_roles,
        decoded_start_frame,
        decoded_frame_count,
        ring_first_sequence,
        ring_last_sequence_exclusive,
        provenance_json,
        ..
    } = ack
    else {
        return failure("external Q4 process_slot did not produce a decoded slot");
    };
    let expected_playheads = [
        expected_step % sources[0].profile.visual.latent_slots,
        expected_step % sources[1].profile.visual.latent_slots,
        expected_step % sources[2].profile.visual.latent_slots,
        expected_step % sources[3].profile.visual.latent_slots,
    ];
    require(
        deck_id == DECK_ID
            && ack_revision == deck_revision
            && stream_generation == generation
            && [playhead_a, playhead_b, playhead_c, playhead_d] == expected_playheads
            && ack_roles == roles
            && decoded_start_frame == expected_decoded_start
            && ring_first_sequence > 0
            && ring_last_sequence_exclusive == ring_first_sequence + u64::from(decoded_frame_count),
        "external Q4 decoded-slot acknowledgement changed deck, clock, roles, playheads, or ring range",
    )?;

    let parsed: Value = serde_json::from_str(&provenance_json)?;
    let carrier_playhead = expected_playheads[slot_index(roles.carrier)];
    validate_provenance(
        &parsed,
        controls,
        roles,
        seed,
        carrier_playhead,
        sources[0].profile.visual.latent_height,
        sources[0].profile.visual.latent_width,
    )?;
    let private_fragments = sources
        .iter()
        .map(|source| path_text(&source.path))
        .chain(std::iter::once(path_text(&decoder.path)))
        .collect::<TestResult<Vec<_>>>()?;
    require(
        private_fragments
            .iter()
            .all(|fragment| !json_contains_fragment(&parsed, fragment)),
        "external Q4 provenance exposed a cartridge or decoder path",
    )?;

    let width = sources[0].profile.visual.decoded_width;
    let height = sources[0].profile.visual.decoded_height;
    let mut frame_sha256 = Vec::with_capacity(decoded_frame_count as usize);
    for expected_sequence in ring_first_sequence..ring_last_sequence_exclusive {
        let ReadStatus::Frame(frame) = consumer.try_read()? else {
            return failure("external Q4 receipt claimed a frame missing from the RGB ring");
        };
        require(
            frame.generation() == generation
                && frame.sequence() == expected_sequence
                && frame.width() == width
                && frame.height() == height
                && !frame.padded_rgba().is_empty(),
            "external Q4 RGB frame metadata differs from its decoded-slot receipt",
        )?;
        frame_sha256.push(
            hash_reader(&mut Cursor::new(frame.padded_rgba()))?
                .sha256
                .to_string(),
        );
    }
    require(
        matches!(consumer.try_read()?, ReadStatus::Empty),
        "external Q4 worker published frames outside its declared ring range",
    )?;
    let deterministic = deterministic_provenance(parsed)?;
    Ok(ExternalStepProof {
        frame_sha256,
        provenance_sha256: hash_reader(&mut Cursor::new(serde_json::to_vec(&deterministic)?))?
            .sha256
            .to_string(),
        decoded_frame_count,
    })
}

#[allow(clippy::too_many_arguments)]
async fn process_external_cycle(
    client: &mut WorkerClient,
    consumer: &mut WindowsRgbRingConsumer,
    deck_revision: u64,
    generation: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    seed: u64,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
) -> TestResult<()> {
    let carrier = &sources[slot_index(roles.carrier)];
    let mut decoded_start = 0_u64;
    for expected_step in 0..carrier.profile.visual.latent_slots {
        let proof = process_external_one(
            client,
            consumer,
            deck_revision,
            generation,
            expected_step,
            decoded_start,
            roles,
            controls,
            seed,
            sources,
            decoder,
        )
        .await?;
        decoded_start = decoded_start
            .checked_add(u64::from(proof.decoded_frame_count))
            .ok_or_else(|| io::Error::other("external Q4 decoded frame count overflowed"))?;
    }
    require(
        decoded_start == carrier.profile.visual.decoded_frame_count,
        "external Q4 carrier cycle did not produce its exact H3 decoded frame count",
    )
}

const fn slot_index(slot: Q4Slot) -> usize {
    match slot {
        Q4Slot::A => 0,
        Q4Slot::B => 1,
        Q4Slot::C => 2,
        Q4Slot::D => 3,
    }
}

#[allow(clippy::too_many_arguments)]
async fn process_one(
    client: &mut WorkerClient,
    consumer: &mut WindowsRgbRingConsumer,
    deck_revision: u64,
    generation: u64,
    expected_playhead: u64,
    expected_decoded_start: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    seed: u64,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
) -> TestResult<StepProof> {
    let ack = client
        .deck_q4_process_slot(
            Q4ProcessSlot {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                stream_generation: generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    let Q4ProcessSlotAck::DecodedSlot {
        deck_id,
        deck_revision: ack_revision,
        stream_generation,
        playhead_a,
        playhead_b,
        playhead_c,
        playhead_d,
        roles: ack_roles,
        decoded_start_frame,
        decoded_frame_count,
        ring_first_sequence,
        ring_last_sequence_exclusive,
        provenance_json,
        ..
    } = ack
    else {
        return failure("Q4 process_slot did not produce a decoded slot");
    };
    require(
        deck_id == DECK_ID
            && ack_revision == deck_revision
            && stream_generation == generation
            && [playhead_a, playhead_b, playhead_c, playhead_d] == [expected_playhead; 4]
            && ack_roles == roles
            && decoded_start_frame == expected_decoded_start
            && ring_first_sequence > 0
            && ring_last_sequence_exclusive == ring_first_sequence + u64::from(decoded_frame_count),
        "Q4 decoded-slot acknowledgement changed deck, clock, roles, playheads, or ring range",
    )?;

    let parsed: Value = serde_json::from_str(&provenance_json)?;
    validate_provenance(
        &parsed,
        controls,
        roles,
        seed,
        expected_playhead,
        LATENT_HEIGHT,
        LATENT_WIDTH,
    )?;
    let private_fragments = sources
        .iter()
        .map(|source| path_text(&source.path))
        .chain(std::iter::once(path_text(&decoder.path)))
        .collect::<TestResult<Vec<_>>>()?;
    require(
        private_fragments
            .iter()
            .all(|fragment| !json_contains_fragment(&parsed, fragment)),
        "Q4 provenance exposed a cartridge or decoder path",
    )?;

    let mut frames = Vec::with_capacity(decoded_frame_count as usize);
    for expected_sequence in ring_first_sequence..ring_last_sequence_exclusive {
        let ReadStatus::Frame(frame) = consumer.try_read()? else {
            return failure("Q4 receipt claimed a frame missing from the RGB ring");
        };
        require(
            frame.generation() == generation
                && frame.sequence() == expected_sequence
                && frame.width() == DECODED_WIDTH
                && frame.height() == DECODED_HEIGHT
                && !frame.padded_rgba().is_empty(),
            "Q4 RGB frame metadata differs from its decoded-slot receipt",
        )?;
        frames.push(frame.padded_rgba().to_vec());
    }
    require(
        matches!(consumer.try_read()?, ReadStatus::Empty),
        "Q4 worker published frames outside its declared ring range",
    )?;
    Ok(StepProof {
        frames,
        provenance: deterministic_provenance(parsed)?,
    })
}

fn validate_provenance(
    provenance: &Value,
    controls: &Q4Controls,
    roles: Q4Roles,
    seed: u64,
    playhead: u64,
    latent_height: u64,
    latent_width: u64,
) -> TestResult<()> {
    require(
        provenance
            .pointer("/operation/operator_id")
            .and_then(Value::as_str)
            == Some(OPERATOR_ID)
            && provenance
                .pointer("/operation/operator_version")
                .and_then(Value::as_str)
                == Some(OPERATOR_VERSION)
            && provenance
                .pointer("/operation/seed")
                .and_then(Value::as_u64)
                == Some(seed)
            && provenance
                .pointer("/operation/controls/algorithm")
                .and_then(Value::as_str)
                == Some("XS5")
            && provenance
                .pointer("/operation/controls/xs5_routing")
                .and_then(Value::as_str)
                == Some(match controls.xs5_routing {
                    Q4Xs5Routing::TopK => "TOPK",
                    Q4Xs5Routing::Sinkhorn => "SINKHORN",
                })
            && provenance
                .pointer("/roles/carrier/slot")
                .and_then(Value::as_str)
                == Some(slot_name(roles.carrier))
            && provenance
                .pointer("/roles/carrier/playhead")
                .and_then(Value::as_u64)
                == Some(playhead)
            && provenance
                .pointer("/roles/donors/0/role")
                .and_then(Value::as_str)
                == Some("B")
            && provenance
                .pointer("/roles/donors/0/slot")
                .and_then(Value::as_str)
                == Some(slot_name(roles.donor_b))
            && provenance
                .pointer("/roles/donors/1/role")
                .and_then(Value::as_str)
                == Some("C")
            && provenance
                .pointer("/roles/donors/1/slot")
                .and_then(Value::as_str)
                == Some(slot_name(roles.donor_c))
            && provenance
                .pointer("/roles/donors/2/role")
                .and_then(Value::as_str)
                == Some("D")
            && provenance
                .pointer("/roles/donors/2/slot")
                .and_then(Value::as_str)
                == Some(slot_name(roles.donor_d)),
        "Q4 provenance lost operator identity, routing method, roles, donor order, or seed",
    )?;
    let weight_sum = ["B", "C", "D"]
        .into_iter()
        .map(|role| {
            provenance
                .pointer(&format!("/resolved_donor_weights/{role}"))
                .and_then(Value::as_f64)
                .ok_or_else(|| io::Error::other("Q4 provenance omitted a resolved donor weight"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum::<f64>();
    require(
        (weight_sum - 1.0).abs() <= 1e-12
            && provenance
                .pointer("/routing/reference")
                .and_then(Value::as_str)
                == Some("UNCHANGED_CARRIER")
            && provenance
                .pointer("/routing/carrier_affinity_reused")
                .and_then(Value::as_bool)
                == Some(true)
            && provenance
                .pointer("/routing/accumulation_order")
                .and_then(Value::as_array)
                == Some(&vec![json!("B"), json!("C"), json!("D")])
            && provenance.pointer("/grid/height").and_then(Value::as_u64) == Some(latent_height)
            && provenance.pointer("/grid/width").and_then(Value::as_u64) == Some(latent_width)
            && provenance.pointer("/grid/tokens").and_then(Value::as_u64)
                == Some(latent_height * latent_width)
            && provenance.pointer("/grid/full").and_then(Value::as_bool) == Some(true),
        "Q4 provenance lost normalized weights, unchanged-carrier routing, fixed B/C/D accumulation, or full-grid proof",
    )
}

fn deterministic_provenance(mut value: Value) -> TestResult<Value> {
    let stream = value
        .get_mut("stream")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| io::Error::other("Q4 provenance omitted its stream object"))?;
    stream.remove("generation");
    Ok(value)
}

fn slot_name(slot: Q4Slot) -> &'static str {
    match slot {
        Q4Slot::A => "A",
        Q4Slot::B => "B",
        Q4Slot::C => "C",
        Q4Slot::D => "D",
    }
}

async fn restart_and_reset(
    client: &mut WorkerClient,
    deck_revision: u64,
    current_generation: u64,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<u64> {
    let barrier = client
        .deck_q4_restart(
            Q4Restart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        barrier.current_generation == current_generation
            && barrier.minimum_new_generation > current_generation
            && barrier.reasons.contains(&Q4ResetReason::TransportRestart),
        "Q4 restart did not expose the exact causal reset barrier",
    )?;
    apply_reset(
        client,
        deck_revision,
        barrier.minimum_new_generation,
        &barrier.reasons,
        owner,
        consumer,
    )
    .await
}

async fn apply_capture_reset(
    client: &mut WorkerClient,
    deck_revision: u64,
    status: &Q4CaptureStatus,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<u64> {
    let generation = status
        .minimum_new_generation
        .ok_or_else(|| io::Error::other("Q4 capture start omitted its reset generation"))?;
    let reasons = BoundedVec::try_from_vec(vec![Q4ResetReason::TransportRestart])?;
    apply_reset(client, deck_revision, generation, &reasons, owner, consumer).await
}

async fn apply_reset(
    client: &mut WorkerClient,
    deck_revision: u64,
    generation: u64,
    expected_reasons: &BoundedVec<Q4ResetReason, 5>,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<u64> {
    let reset = client
        .deck_q4_reset(
            Q4Reset {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                new_stream_generation: generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        reset.stream_generation == generation
            && reset.reasons == *expected_reasons
            && reset.causal_state_cleared
            && [
                reset.playhead_a,
                reset.playhead_b,
                reset.playhead_c,
                reset.playhead_d,
            ] == [0; 4],
        "Q4 reset did not atomically clear decoder state and all four playheads",
    )?;
    owner.adopt_generation(generation)?;
    consumer.adopt_generation(generation)?;
    require_zero_ring(owner, consumer)?;
    Ok(generation)
}

#[allow(clippy::too_many_arguments)]
async fn external_snapshot_capture(
    client: &mut WorkerClient,
    deck_revision: u64,
    current_generation: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
    temporary_root: &Path,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<(Value, u64)> {
    let carrier = &sources[slot_index(roles.carrier)];
    let target_slots = carrier.profile.visual.latent_slots;
    let spool_root = temporary_root.join("external-q4-snapshot-spool");
    fs::create_dir(&spool_root)?;
    let capture_id = WireUuid::new_v4();
    let started = client
        .deck_q4_capture_start(
            Q4CaptureStart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
                mode: Q4CaptureMode::Snapshot,
                temporary_root: path_text(&fs::canonicalize(&spool_root)?)?,
                max_latent_slots: target_slots,
                max_visual_bytes: external_visual_bytes(carrier, target_slots)?,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        started.capture_id == capture_id
            && started.mode == Q4CaptureMode::Snapshot
            && started.state == Q4CaptureState::AwaitingReset
            && started.current_generation == Some(current_generation)
            && started.target_latent_slots == Some(target_slots)
            && started.structural_carrier == roles.carrier,
        "external Q4 Snapshot did not arm for one exact real carrier cycle",
    )?;
    let generation = apply_capture_reset(client, deck_revision, &started, owner, consumer).await?;
    process_external_cycle(
        client,
        consumer,
        deck_revision,
        generation,
        roles,
        controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    let finished = capture_status(client, deck_revision, capture_id).await?;
    require(
        finished.state == Q4CaptureState::Finished
            && finished.latent_slots == target_slots
            && finished.stream_generation == Some(generation),
        "external Q4 Snapshot did not finish after the exact selected carrier cycle",
    )?;
    let receipt = finished
        .receipt
        .as_deref()
        .ok_or_else(|| io::Error::other("external Q4 Snapshot omitted its receipt"))?;
    validate_external_snapshot_receipt(receipt, capture_id, roles, controls, sources, &spool_root)?;
    let proof = pack_external_capture(
        receipt,
        roles,
        sources,
        &temporary_root.join("external-q4-snapshot.lc"),
    )?;
    Ok((proof, generation))
}

#[allow(clippy::too_many_arguments)]
async fn external_live_capture(
    client: &mut WorkerClient,
    deck_revision: u64,
    current_generation: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
    temporary_root: &Path,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<Value> {
    let carrier = &sources[slot_index(roles.carrier)];
    let spool_root = temporary_root.join("external-q4-live-spool");
    fs::create_dir(&spool_root)?;
    let capture_id = WireUuid::new_v4();
    let started = client
        .deck_q4_capture_start(
            Q4CaptureStart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
                mode: Q4CaptureMode::LiveCapture,
                temporary_root: path_text(&fs::canonicalize(&spool_root)?)?,
                max_latent_slots: 2,
                max_visual_bytes: external_visual_bytes(carrier, 2)?,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        started.capture_id == capture_id
            && started.mode == Q4CaptureMode::LiveCapture
            && started.state == Q4CaptureState::AwaitingReset
            && started.current_generation == Some(current_generation)
            && started.structural_carrier == roles.carrier,
        "external Q4 Live Capture did not arm at the next codec-valid reset boundary",
    )?;
    let generation = apply_capture_reset(client, deck_revision, &started, owner, consumer).await?;
    let first = process_external_one(
        client,
        consumer,
        deck_revision,
        generation,
        0,
        0,
        roles,
        controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    let armed = client
        .deck_q4_capture_stop(
            Q4CaptureStop {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        armed.state == Q4CaptureState::StopArmed
            && armed.latent_slots == 1
            && armed.finalize_after_latent_slots == Some(2),
        "external Q4 Live Capture did not arm the next exact T=2 boundary",
    )?;
    process_external_one(
        client,
        consumer,
        deck_revision,
        generation,
        1,
        u64::from(first.decoded_frame_count),
        roles,
        controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    let finished = capture_status(client, deck_revision, capture_id).await?;
    require(
        finished.state == Q4CaptureState::Finished
            && finished.latent_slots == 2
            && finished.stream_generation == Some(generation),
        "external Q4 Live Capture did not finish at the exact T=2 boundary",
    )?;
    let receipt = finished
        .receipt
        .as_deref()
        .ok_or_else(|| io::Error::other("external Q4 Live Capture omitted its receipt"))?;
    validate_external_live_receipt(receipt, capture_id, roles, controls, sources, &spool_root)?;
    pack_external_capture(
        receipt,
        roles,
        sources,
        &temporary_root.join("external-q4-live.lc"),
    )
}

fn external_visual_bytes(source: &SyntheticSource, latent_slots: u64) -> TestResult<u64> {
    24_u64
        .checked_mul(latent_slots)
        .and_then(|value| value.checked_mul(source.profile.visual.latent_height))
        .and_then(|value| value.checked_mul(source.profile.visual.latent_width))
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| io::Error::other("external Q4 capture visual byte limit overflowed").into())
}

fn validate_external_snapshot_receipt(
    receipt: &Q4CaptureReceipt,
    capture_id: WireUuid,
    roles: Q4Roles,
    controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    spool_root: &Path,
) -> TestResult<()> {
    let carrier = &sources[slot_index(roles.carrier)];
    validate_external_common_receipt(
        receipt,
        capture_id,
        carrier.profile.visual.latent_slots,
        sources,
        spool_root,
    )?;
    let audio = carrier
        .profile
        .audio
        .as_ref()
        .ok_or_else(|| io::Error::other("external Q4 Snapshot carrier has no audio"))?;
    let (audio_dtype, element_bytes) = match audio.storage_dtype {
        DType::F16 => (Q4CaptureAudioDtype::F16, 2_u64),
        DType::F32 => (Q4CaptureAudioDtype::F32, 4_u64),
        _ => return failure("external Q4 Snapshot carrier uses a forbidden audio dtype"),
    };
    let expected_audio_bytes = 32_u64
        .checked_mul(2)
        .and_then(|value| value.checked_mul(audio.latent_slots))
        .and_then(|value| value.checked_mul(element_bytes))
        .ok_or_else(|| io::Error::other("external Snapshot audio byte count overflowed"))?;
    require(
        receipt.mode == Q4CaptureMode::Snapshot
            && receipt.structural_carrier == roles.carrier
            && receipt.frozen_seed == Some(TEST_SEED)
            && receipt.frozen_roles == Some(roles)
            && receipt.frozen_controls.as_ref() == Some(controls)
            && receipt.control_events.is_none()
            && receipt.audio_policy == Q4CaptureAudioPolicy::CopiedFromCarrierExact
            && receipt.audio_policy_reason.is_none()
            && receipt.audio_descriptor.as_ref().is_some_and(|descriptor| {
                descriptor.storage_dtype == audio_dtype
                    && descriptor.shape == [1, 32, 2, audio.latent_slots]
                    && descriptor.byte_length == expected_audio_bytes
            }),
        "external Q4 Snapshot did not freeze exact state and copy carrier audio exactly",
    )
}

fn validate_external_live_receipt(
    receipt: &Q4CaptureReceipt,
    capture_id: WireUuid,
    roles: Q4Roles,
    controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    spool_root: &Path,
) -> TestResult<()> {
    validate_external_common_receipt(receipt, capture_id, 2, sources, spool_root)?;
    let events = receipt
        .control_events
        .as_ref()
        .ok_or_else(|| io::Error::other("external Q4 Live Capture omitted state events"))?;
    require(
        receipt.mode == Q4CaptureMode::LiveCapture
            && receipt.structural_carrier == roles.carrier
            && receipt.frozen_seed.is_none()
            && receipt.frozen_roles.is_none()
            && receipt.frozen_controls.is_none()
            && events.len() == 1
            && events[0].slot_offset == 0
            && events[0].roles == roles
            && events[0].controls == *controls
            && events[0].seed == TEST_SEED
            && receipt.audio_policy == Q4CaptureAudioPolicy::OmittedTimingMismatch
            && receipt.audio_policy_reason == Some(Q4CaptureAudioPolicyReason::DurationMismatch)
            && receipt.audio_descriptor.is_none(),
        "short external Q4 Live Capture did not record explicit duration-mismatch audio omission",
    )
}

fn validate_external_common_receipt(
    receipt: &Q4CaptureReceipt,
    capture_id: WireUuid,
    expected_slots: u64,
    sources: &[SyntheticSource; 4],
    spool_root: &Path,
) -> TestResult<()> {
    let payload_path = PathBuf::from(&receipt.payload_path);
    let canonical_payload = fs::canonicalize(&payload_path)?;
    let canonical_root = fs::canonicalize(spool_root)?;
    let measured = hash_path(&canonical_payload)?;
    let reference = &sources[0].profile.visual;
    let expected_frames = latentdeck_cartridge::profile::h3::decoded_frame_count(expected_slots)?;
    require(
        receipt.capture_id == capture_id
            && receipt.storage_dtype == Q4CaptureVisualDtype::F16
            && receipt.visual_shape
                == [
                    1,
                    24,
                    expected_slots,
                    reference.latent_height,
                    reference.latent_width,
                ]
            && receipt.decoded_frame_count == expected_frames
            && receipt.payload_bytes == measured.byte_length
            && receipt.payload_sha256 == measured.sha256.to_string()
            && canonical_payload.starts_with(&canonical_root)
            && canonical_payload
                .extension()
                .is_some_and(|value| value == "partial"),
        "external Q4 capture receipt did not bind its exact full-grid F16 spool",
    )?;
    require(
        receipt
            .parents
            .iter()
            .zip(sources)
            .enumerate()
            .all(|(index, (parent, source))| {
                parent.slot == [Q4Slot::A, Q4Slot::B, Q4Slot::C, Q4Slot::D][index]
                    && parent.cartridge_id == source.cartridge_id
                    && parent.archive_sha256 == source.archive_sha256
            }),
        "external Q4 capture changed the exact ordered parent identities",
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "private capture pack, validation, reload, and exact audio proof are one atomic boundary"
)]
fn pack_external_capture(
    receipt: &Q4CaptureReceipt,
    roles: Q4Roles,
    sources: &[SyntheticSource; 4],
    output: &Path,
) -> TestResult<Value> {
    let controls = match receipt.mode {
        Q4CaptureMode::Snapshot => {
            let frozen = receipt
                .frozen_controls
                .as_ref()
                .ok_or_else(|| io::Error::other("external Snapshot omitted controls"))?;
            let Value::Object(object) = serde_json::to_value(frozen)? else {
                return failure("external Snapshot controls did not serialize as an object");
            };
            let mut controls = object.into_iter().collect::<BTreeMap<_, _>>();
            controls.insert("roles".to_owned(), serde_json::to_value(roles)?);
            controls.insert(
                "structural_carrier".to_owned(),
                serde_json::to_value(receipt.structural_carrier)?,
            );
            controls
        }
        Q4CaptureMode::LiveCapture => BTreeMap::from([
            (
                "control_events".to_owned(),
                serde_json::to_value(&receipt.control_events)?,
            ),
            (
                "structural_carrier".to_owned(),
                serde_json::to_value(receipt.structural_carrier)?,
            ),
        ]),
    };
    let seed = match receipt.mode {
        Q4CaptureMode::Snapshot => receipt
            .frozen_seed
            .ok_or_else(|| io::Error::other("external Snapshot omitted seed"))?,
        Q4CaptureMode::LiveCapture => receipt
            .control_events
            .as_ref()
            .and_then(|events| events.first())
            .map(|event| event.seed)
            .ok_or_else(|| io::Error::other("external Live Capture omitted initial event"))?,
    };
    let carrier = &sources[slot_index(receipt.structural_carrier)];
    let source_cartridge = SourceCartridgeRef {
        cartridge_id: CartridgeId(carrier.cartridge_id.to_string()),
        archive_sha256: Sha256Digest(carrier.archive_sha256.clone()),
    };
    let audio = match receipt.audio_policy {
        Q4CaptureAudioPolicy::SourceAbsent => AudioDisposition::SourceAbsent,
        Q4CaptureAudioPolicy::CopiedFromCarrierExact => AudioDisposition::CopiedFromCarrierExact {
            source_cartridge: source_cartridge.clone(),
        },
        Q4CaptureAudioPolicy::OmittedTimingMismatch => {
            let reason = match receipt.audio_policy_reason {
                Some(Q4CaptureAudioPolicyReason::DurationMismatch) => {
                    AudioOmissionReason::DurationMismatch
                }
                Some(Q4CaptureAudioPolicyReason::TemporalMappingMismatch) => {
                    AudioOmissionReason::TemporalMappingMismatch
                }
                Some(Q4CaptureAudioPolicyReason::DurationAndMappingMismatch) => {
                    AudioOmissionReason::DurationAndMappingMismatch
                }
                None => return failure("external omitted-audio receipt has no explicit reason"),
            };
            AudioDisposition::OmittedTimingMismatch {
                source_cartridge: source_cartridge.clone(),
                reason,
            }
        }
    };
    let request = ResampleManifestRequest {
        cartridge_id: CartridgeId(WireUuid::new_v4().to_string()),
        expected_payload: PayloadExpectation {
            byte_length: receipt.payload_bytes,
            sha256: Sha256Digest(receipt.payload_sha256.clone()),
        },
        capture_mode: match receipt.mode {
            Q4CaptureMode::Snapshot => CartridgeCaptureMode::Snapshot,
            Q4CaptureMode::LiveCapture => CartridgeCaptureMode::LiveCapture,
        },
        audio: audio.clone(),
        parent_cartridges: sources
            .iter()
            .enumerate()
            .map(|(index, source)| ParentCartridge {
                cartridge_id: CartridgeId(source.cartridge_id.to_string()),
                archive_sha256: Sha256Digest(source.archive_sha256.clone()),
                role: Identifier(format!(
                    "source_{}_slot_{}",
                    external_source_order(sources)[index].to_ascii_lowercase(),
                    ['a', 'b', 'c', 'd'][index]
                )),
            })
            .collect(),
        operator_id: Identifier(OPERATOR_ID.to_owned()),
        operator_version: OPERATOR_VERSION.to_owned(),
        seed,
        controls,
    };
    let spool_path = PathBuf::from(&receipt.payload_path);
    let write = pack_resample_atomic(&request, &spool_path, output, &WriteOptions::default())?;
    require(
        write.output_path == output && write.spool_removed && !spool_path.exists(),
        "external Q4 resample did not atomically commit and consume its exact spool",
    )?;

    let mut validated = open_validated(output, &ValidationOptions::default())?;
    let archive_sha256 = validated.receipt().archive_sha256.to_string();
    let manifest = validated.manifest();
    require(
        manifest.audio == audio
            && manifest.parent_cartridges == request.parent_cartridges
            && manifest.operation_history.len() == 1
            && manifest.operation_history[0].operator_id.0 == OPERATOR_ID
            && manifest.operation_history[0].operator_version == OPERATOR_VERSION
            && manifest.operation_history[0].seed == seed
            && manifest.tensors.iter().any(|tensor| {
                tensor.name.0 == "video"
                    && tensor.storage_dtype == DType::F16
                    && tensor.shape.as_slice() == receipt.visual_shape
            })
            && manifest
                .provenance
                .sources
                .iter()
                .all(|source| source.uri.is_none()),
        "external Q4 packed LC lost shape, genealogy, audio policy, or operator provenance",
    )?;
    let output_audio_sha256 =
        if receipt.audio_policy == Q4CaptureAudioPolicy::CopiedFromCarrierExact {
            Some(
                hash_reader(&mut validated.tensor_reader("audio")?)?
                    .sha256
                    .to_string(),
            )
        } else {
            require(
                validated.tensor_reader("audio").is_err(),
                "short external Live Capture unexpectedly retained an audio tensor",
            )?;
            None
        };
    let manifest_json = serde_json::to_value(validated.manifest())?;
    require(
        sources.iter().all(|source| {
            path_text(&source.path).is_ok_and(|path| !json_contains_fragment(&manifest_json, &path))
        }) && !json_contains_fragment(&manifest_json, &receipt.payload_path)
            && path_text(output).is_ok_and(|path| !json_contains_fragment(&manifest_json, &path)),
        "external Q4 resample manifest serialized a machine-local source, spool, or output path",
    )?;
    drop(validated);

    let reloaded = open_validated(output, &ValidationOptions::default())?;
    require(
        reloaded.receipt().archive_sha256.to_string() == archive_sha256
            && reloaded.manifest().audio == audio
            && reloaded.h3_profile().visual.latent_slots == receipt.visual_shape[2]
            && reloaded.h3_profile().visual.decoded_frame_count == receipt.decoded_frame_count,
        "external Q4 LC did not preserve identity, audio policy, or cadence after reload",
    )?;
    drop(reloaded);

    let source_audio_sha256 =
        if receipt.audio_policy == Q4CaptureAudioPolicy::CopiedFromCarrierExact {
            let mut source = open_validated(&carrier.path, &ValidationOptions::default())?;
            Some(
                hash_reader(&mut source.tensor_reader("audio")?)?
                    .sha256
                    .to_string(),
            )
        } else {
            None
        };
    require(
        source_audio_sha256.is_none() || source_audio_sha256 == output_audio_sha256,
        "external Q4 Snapshot changed the structural carrier audio bytes",
    )?;

    let logical_carrier = external_source_order(sources)[slot_index(receipt.structural_carrier)];
    let audio_bytes_identical =
        source_audio_sha256.is_some() && source_audio_sha256 == output_audio_sha256;
    Ok(json!({
        "mode": match receipt.mode {
            Q4CaptureMode::Snapshot => "snapshot",
            Q4CaptureMode::LiveCapture => "live_capture",
        },
        "structural_carrier_slot": slot_name(receipt.structural_carrier),
        "structural_carrier_logical_source": logical_carrier,
        "visual_shape": receipt.visual_shape,
        "decoded_frame_count": receipt.decoded_frame_count,
        "post_operator_payload_sha256": receipt.payload_sha256,
        "packed_archive_sha256": archive_sha256,
        "audio_policy": match receipt.audio_policy {
            Q4CaptureAudioPolicy::SourceAbsent => "source_absent",
            Q4CaptureAudioPolicy::CopiedFromCarrierExact => "copied_from_carrier_exact",
            Q4CaptureAudioPolicy::OmittedTimingMismatch => "omitted_timing_mismatch",
        },
        "audio_policy_reason": receipt.audio_policy_reason.map(|reason| match reason {
            Q4CaptureAudioPolicyReason::DurationMismatch => "duration_mismatch",
            Q4CaptureAudioPolicyReason::TemporalMappingMismatch => "temporal_mapping_mismatch",
            Q4CaptureAudioPolicyReason::DurationAndMappingMismatch => "duration_and_mapping_mismatch",
        }),
        "source_audio_sha256": source_audio_sha256,
        "output_audio_sha256": output_audio_sha256,
        "audio_bytes_identical": audio_bytes_identical,
        "atomic_pack": true,
        "validation_passed": true,
        "reload_passed": true,
        "partial_remaining": false
    }))
}

#[allow(clippy::too_many_arguments)]
async fn snapshot_capture(
    client: &mut WorkerClient,
    deck_revision: u64,
    current_generation: u64,
    roles: Q4Roles,
    controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
    temporary_root: &Path,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<u64> {
    let spool_root = temporary_root.join("q4-snapshot-spool");
    fs::create_dir(&spool_root)?;
    let capture_id = WireUuid::new_v4();
    let started = client
        .deck_q4_capture_start(
            Q4CaptureStart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
                mode: Q4CaptureMode::Snapshot,
                temporary_root: path_text(&fs::canonicalize(&spool_root)?)?,
                max_latent_slots: LATENT_SLOTS,
                max_visual_bytes: CAPTURE_VISUAL_LIMIT,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        started.capture_id == capture_id
            && started.mode == Q4CaptureMode::Snapshot
            && started.state == Q4CaptureState::AwaitingReset
            && started.current_generation == Some(current_generation)
            && started.target_latent_slots == Some(LATENT_SLOTS)
            && started.structural_carrier == roles.carrier,
        "Q4 Snapshot did not arm at the next exact carrier-cycle reset boundary",
    )?;
    let generation = apply_capture_reset(client, deck_revision, &started, owner, consumer).await?;
    process_cycle(
        client,
        consumer,
        deck_revision,
        generation,
        roles,
        controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;
    let finished = capture_status(client, deck_revision, capture_id).await?;
    require(
        finished.state == Q4CaptureState::Finished
            && finished.latent_slots == LATENT_SLOTS
            && finished.stream_generation == Some(generation),
        "Q4 Snapshot did not finish after one complete synthetic carrier cycle",
    )?;
    let receipt = finished
        .receipt
        .as_deref()
        .ok_or_else(|| io::Error::other("Q4 Snapshot omitted its finalized receipt"))?;
    validate_snapshot_receipt(receipt, capture_id, roles, controls, sources, &spool_root)?;
    pack_capture(receipt, sources, &temporary_root.join("q4-snapshot.lc"))?;
    Ok(generation)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn live_capture(
    client: &mut WorkerClient,
    deck_revision: u64,
    current_generation: u64,
    initial_roles: Q4Roles,
    initial_controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    decoder: &ValidatedExternalAsset,
    temporary_root: &Path,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<()> {
    let spool_root = temporary_root.join("q4-live-spool");
    fs::create_dir(&spool_root)?;
    let capture_id = WireUuid::new_v4();
    let started = client
        .deck_q4_capture_start(
            Q4CaptureStart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
                mode: Q4CaptureMode::LiveCapture,
                temporary_root: path_text(&fs::canonicalize(&spool_root)?)?,
                max_latent_slots: LATENT_SLOTS,
                max_visual_bytes: CAPTURE_VISUAL_LIMIT,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        started.capture_id == capture_id
            && started.mode == Q4CaptureMode::LiveCapture
            && started.state == Q4CaptureState::AwaitingReset
            && started.current_generation == Some(current_generation)
            && started.structural_carrier == initial_roles.carrier,
        "Q4 Live Capture did not arm at the next codec-valid reset boundary",
    )?;
    let generation = apply_capture_reset(client, deck_revision, &started, owner, consumer).await?;
    process_one(
        client,
        consumer,
        deck_revision,
        generation,
        0,
        0,
        initial_roles,
        initial_controls,
        TEST_SEED,
        sources,
        decoder,
    )
    .await?;

    let changed_roles = Q4Roles {
        carrier: Q4Slot::D,
        donor_b: Q4Slot::B,
        donor_c: Q4Slot::A,
        donor_d: Q4Slot::C,
    };
    let changed_controls = Q4Controls {
        influence_mode: Q4InfluenceMode::Triangle,
        triangle_x: finite(0.5),
        triangle_y: finite(0.5),
        chaos: finite(0.0),
        ..initial_controls.clone()
    };
    client
        .deck_q4_roles_set(
            Q4RolesSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                roles: changed_roles,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    client
        .deck_q4_controls_set(
            Q4ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                controls: changed_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    client
        .deck_q4_seed_set(
            Q4SeedSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                seed: CHANGED_SEED,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    let armed = client
        .deck_q4_capture_stop(
            Q4CaptureStop {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        armed.state == Q4CaptureState::StopArmed
            && armed.latent_slots == 1
            && armed.finalize_after_latent_slots == Some(LATENT_SLOTS),
        "Q4 Live Capture did not arm its next T=2 codec-valid stop boundary",
    )?;
    process_one(
        client,
        consumer,
        deck_revision,
        generation,
        1,
        1,
        changed_roles,
        &changed_controls,
        CHANGED_SEED,
        sources,
        decoder,
    )
    .await?;
    let finished = capture_status(client, deck_revision, capture_id).await?;
    require(
        finished.state == Q4CaptureState::Finished
            && finished.latent_slots == LATENT_SLOTS
            && finished.stream_generation == Some(generation),
        "Q4 Live Capture did not finish at its armed T=2 boundary",
    )?;
    let receipt = finished
        .receipt
        .as_deref()
        .ok_or_else(|| io::Error::other("Q4 Live Capture omitted its finalized receipt"))?;
    validate_live_receipt(
        receipt,
        capture_id,
        initial_roles,
        initial_controls,
        changed_roles,
        &changed_controls,
        sources,
        &spool_root,
    )?;
    pack_capture(receipt, sources, &temporary_root.join("q4-live.lc"))?;
    Ok(())
}

async fn capture_status(
    client: &mut WorkerClient,
    deck_revision: u64,
    capture_id: WireUuid,
) -> TestResult<Q4CaptureStatus> {
    Ok(client
        .deck_q4_capture_status(
            Q4CaptureStatusRequest {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?)
}

fn validate_snapshot_receipt(
    receipt: &Q4CaptureReceipt,
    capture_id: WireUuid,
    roles: Q4Roles,
    controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    spool_root: &Path,
) -> TestResult<()> {
    validate_common_receipt(receipt, capture_id, sources, spool_root)?;
    require(
        receipt.mode == Q4CaptureMode::Snapshot
            && receipt.structural_carrier == roles.carrier
            && receipt.frozen_seed == Some(TEST_SEED)
            && receipt.frozen_roles == Some(roles)
            && receipt.frozen_controls.as_ref() == Some(controls)
            && receipt.control_events.is_none(),
        "Q4 Snapshot receipt did not freeze exact roles, controls, carrier, and seed",
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_live_receipt(
    receipt: &Q4CaptureReceipt,
    capture_id: WireUuid,
    initial_roles: Q4Roles,
    initial_controls: &Q4Controls,
    changed_roles: Q4Roles,
    changed_controls: &Q4Controls,
    sources: &[SyntheticSource; 4],
    spool_root: &Path,
) -> TestResult<()> {
    validate_common_receipt(receipt, capture_id, sources, spool_root)?;
    let events = receipt
        .control_events
        .as_ref()
        .ok_or_else(|| io::Error::other("Q4 Live Capture omitted control_events"))?;
    let first = events
        .first()
        .ok_or_else(|| io::Error::other("Q4 Live Capture control_events is empty"))?;
    let last = events
        .last()
        .ok_or_else(|| io::Error::other("Q4 Live Capture control_events is empty"))?;
    require(
        receipt.mode == Q4CaptureMode::LiveCapture
            && receipt.structural_carrier == initial_roles.carrier
            && receipt.frozen_seed.is_none()
            && receipt.frozen_roles.is_none()
            && receipt.frozen_controls.is_none()
            && events.len() == 4
            && first.slot_offset == 0
            && first.roles == initial_roles
            && first.controls == *initial_controls
            && first.seed == TEST_SEED
            && events.iter().skip(1).all(|event| event.slot_offset == 1)
            && last.roles == changed_roles
            && last.controls == *changed_controls
            && last.seed == CHANGED_SEED,
        "Q4 Live Capture receipt lost its bounded roles/controls/seed event history",
    )
}

fn validate_common_receipt(
    receipt: &Q4CaptureReceipt,
    capture_id: WireUuid,
    sources: &[SyntheticSource; 4],
    spool_root: &Path,
) -> TestResult<()> {
    let payload_path = PathBuf::from(&receipt.payload_path);
    let canonical_payload = fs::canonicalize(&payload_path)?;
    let canonical_root = fs::canonicalize(spool_root)?;
    let measured = hash_path(&canonical_payload)?;
    require(
        receipt.capture_id == capture_id
            && receipt.storage_dtype == Q4CaptureVisualDtype::F16
            && receipt.visual_shape == [1, 24, LATENT_SLOTS, LATENT_HEIGHT, LATENT_WIDTH]
            && receipt.decoded_frame_count == DECODED_FRAMES
            && receipt.payload_bytes == measured.byte_length
            && receipt.payload_sha256 == measured.sha256.to_string()
            && canonical_payload.starts_with(&canonical_root)
            && canonical_payload
                .extension()
                .is_some_and(|value| value == "partial")
            && receipt.audio_policy == Q4CaptureAudioPolicy::SourceAbsent
            && receipt.audio_policy_reason.is_none()
            && receipt.audio_descriptor.is_none(),
        "Q4 capture receipt did not bind its exact bounded visual-only F16 spool",
    )?;
    require(
        receipt
            .parents
            .iter()
            .zip(sources)
            .enumerate()
            .all(|(index, (parent, source))| {
                parent.slot == [Q4Slot::A, Q4Slot::B, Q4Slot::C, Q4Slot::D][index]
                    && parent.cartridge_id == source.cartridge_id
                    && parent.archive_sha256 == source.archive_sha256
            }),
        "Q4 capture receipt changed its ordered A/B/C/D parent identities",
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "linear spool-to-LC proof keeps receipt binding and validation adjacent"
)]
fn pack_capture(
    receipt: &Q4CaptureReceipt,
    sources: &[SyntheticSource; 4],
    output: &Path,
) -> TestResult<()> {
    let controls = match receipt.mode {
        Q4CaptureMode::Snapshot => {
            let frozen = receipt
                .frozen_controls
                .as_ref()
                .ok_or_else(|| io::Error::other("Snapshot receipt omitted controls"))?;
            let Value::Object(object) = serde_json::to_value(frozen)? else {
                return failure("Q4 Snapshot controls did not serialize as an object");
            };
            let mut controls = object.into_iter().collect::<BTreeMap<_, _>>();
            controls.insert(
                "roles".to_owned(),
                serde_json::to_value(receipt.frozen_roles)?,
            );
            controls.insert(
                "structural_carrier".to_owned(),
                serde_json::to_value(receipt.structural_carrier)?,
            );
            controls
        }
        Q4CaptureMode::LiveCapture => BTreeMap::from([
            (
                "control_events".to_owned(),
                serde_json::to_value(&receipt.control_events)?,
            ),
            (
                "structural_carrier".to_owned(),
                serde_json::to_value(receipt.structural_carrier)?,
            ),
        ]),
    };
    let seed = match receipt.mode {
        Q4CaptureMode::Snapshot => receipt
            .frozen_seed
            .ok_or_else(|| io::Error::other("Snapshot receipt omitted seed"))?,
        Q4CaptureMode::LiveCapture => receipt
            .control_events
            .as_ref()
            .and_then(|events| events.first())
            .map(|event| event.seed)
            .ok_or_else(|| io::Error::other("Live Capture receipt omitted initial event"))?,
    };
    let request = ResampleManifestRequest {
        cartridge_id: CartridgeId(WireUuid::new_v4().to_string()),
        expected_payload: PayloadExpectation {
            byte_length: receipt.payload_bytes,
            sha256: Sha256Digest(receipt.payload_sha256.clone()),
        },
        capture_mode: match receipt.mode {
            Q4CaptureMode::Snapshot => CartridgeCaptureMode::Snapshot,
            Q4CaptureMode::LiveCapture => CartridgeCaptureMode::LiveCapture,
        },
        audio: AudioDisposition::SourceAbsent,
        parent_cartridges: sources
            .iter()
            .enumerate()
            .map(|(index, source)| ParentCartridge {
                cartridge_id: CartridgeId(source.cartridge_id.to_string()),
                archive_sha256: Sha256Digest(source.archive_sha256.clone()),
                role: Identifier(format!("source_{}", ['a', 'b', 'c', 'd'][index])),
            })
            .collect(),
        operator_id: Identifier(OPERATOR_ID.to_owned()),
        operator_version: OPERATOR_VERSION.to_owned(),
        seed,
        controls,
    };
    let spool_path = PathBuf::from(&receipt.payload_path);
    let write = pack_resample_atomic(&request, &spool_path, output, &WriteOptions::default())?;
    require(
        write.output_path == output && write.spool_removed && !spool_path.exists(),
        "Q4 resample packer did not atomically commit and consume the exact spool",
    )?;
    let cartridge = open_validated(output, &ValidationOptions::default())?;
    let manifest = cartridge.manifest();
    require(
        manifest.audio == AudioDisposition::SourceAbsent
            && manifest.parent_cartridges == request.parent_cartridges
            && manifest.operation_history.len() == 1
            && manifest.operation_history[0].operator_id.0 == OPERATOR_ID
            && manifest.operation_history[0].operator_version == OPERATOR_VERSION
            && manifest.operation_history[0].seed == seed
            && manifest.tensors.iter().any(|tensor| {
                tensor.name.0 == "video"
                    && tensor.storage_dtype == DType::F16
                    && tensor.shape == vec![1, 24, LATENT_SLOTS, LATENT_HEIGHT, LATENT_WIDTH]
            })
            && manifest
                .provenance
                .sources
                .iter()
                .all(|source| source.uri.is_none()),
        "validated Q4 resample LC lost F16 shape, genealogy, audio policy, or operator provenance",
    )?;
    let manifest_json = serde_json::to_value(manifest)?;
    require(
        sources.iter().all(|source| {
            path_text(&source.path).is_ok_and(|path| !json_contains_fragment(&manifest_json, &path))
        }) && !json_contains_fragment(&manifest_json, &receipt.payload_path)
            && path_text(output).is_ok_and(|path| !json_contains_fragment(&manifest_json, &path)),
        "Q4 resample manifest serialized a machine-local source, spool, or output path",
    )
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
        return failure("Q4 worker returned the wrong session.configure acknowledgement");
    };
    require(
        configured.selected_protocol_version == request.selected_protocol_version
            && configured.max_frame_bytes == request.max_frame_bytes
            && configured.max_inflight_decode_batches == 1,
        "Q4 worker changed the bounded session contract",
    )
}

async fn inspect_runtime(client: &mut WorkerClient, pack: &ValidatedCodecPack) -> TestResult<bool> {
    let ack = client
        .call(Command::CodecInspect(EmptyPayload {}), COMMAND_TIMEOUT)
        .await?;
    let Ack::CodecInspect(inspection) = ack else {
        return failure("Q4 worker returned the wrong codec.inspect acknowledgement");
    };
    require(
        inspection.adapters.iter().any(|adapter| {
            adapter.adapter_id == pack.manifest.adapter.adapter_id
                && adapter.adapter_version == pack.manifest.adapter.adapter_version
                && adapter
                    .profiles
                    .iter()
                    .any(|profile| profile == &h3_profile())
        }),
        "Q4 worker did not advertise the validated adapter/profile",
    )?;
    Ok(inspection.cuda_available && inspection.devices.iter().any(|device| device.ordinal == 0))
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
        return failure("Q4 worker returned the wrong codec.load acknowledgement");
    };
    require(
        loaded.pack_id == request.pack_id
            && loaded.pack_version == request.pack_version
            && loaded.adapter_id == request.adapter_id
            && loaded.adapter_version == pack.manifest.adapter.adapter_version
            && loaded.profile == request.profile
            && loaded.device.ordinal == 0,
        "Q4 worker loaded a codec identity different from the validated selection",
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
        return failure("Q4 worker returned the wrong ring.bind acknowledgement");
    };
    require(
        bound.layout_version == request.layout_version
            && bound.mapping_bytes == request.mapping_bytes
            && bound.ring_id == request.ring_id,
        "Q4 worker changed the anonymous RGB ring binding",
    )
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
        "Q4 causal reset did not clear RGB ring counters",
    )
}

fn source_binding(source: &SyntheticSource) -> TestResult<Q4SourceBinding> {
    Ok(Q4SourceBinding {
        cartridge_path: path_text(&source.path)?,
        cartridge_id: source.cartridge_id,
        expected_archive_sha256: source.archive_sha256.clone(),
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
        .ok_or_else(|| io::Error::other("private Q4 harness path is not valid UTF-8").into())
}

fn json_contains_fragment(value: &Value, fragment: &str) -> bool {
    match value {
        Value::String(text) => text.contains(fragment),
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_fragment(value, fragment)),
        Value::Object(values) => values
            .values()
            .any(|value| json_contains_fragment(value, fragment)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn persist_path_free_evidence(path: &Path, value: &Value) -> TestResult<()> {
    require(
        !json_contains_machine_path(value),
        "external Q4 evidence contains a machine-local path",
    )?;
    require(
        path.extension()
            .is_some_and(|extension| extension == "json"),
        "external Q4 evidence path must end in .json",
    )?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("external Q4 evidence path has no parent"))?;
    require(
        parent.is_dir(),
        "external Q4 evidence parent directory does not exist",
    )?;
    let partial = path.with_extension("json.partial");
    require(
        !path.exists() && !partial.exists(),
        "external Q4 evidence writer refuses to replace an existing artifact",
    )?;
    let encoded = serde_json::to_vec_pretty(value)?;
    fs::write(&partial, encoded)?;
    if let Err(error) = move_atomic(&partial, path) {
        let _ = fs::remove_file(&partial);
        return Err(error.into());
    }
    Ok(())
}

fn json_contains_machine_path(value: &Value) -> bool {
    match value {
        Value::String(text) => {
            Path::new(text).is_absolute()
                || text.starts_with("\\\\")
                || text.starts_with("//")
                || (text.len() >= 3
                    && text.as_bytes()[0].is_ascii_alphabetic()
                    && text.as_bytes()[1] == b':'
                    && matches!(text.as_bytes()[2], b'\\' | b'/'))
        }
        Value::Array(values) => values.iter().any(json_contains_machine_path),
        Value::Object(values) => values.values().any(json_contains_machine_path),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn tree_contains_partial(root: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if tree_contains_partial(&entry.path())? {
                return Ok(true);
            }
        } else if file_type.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|value| value == "partial")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition { Ok(()) } else { failure(message) }
}

fn failure<T>(message: &'static str) -> TestResult<T> {
    Err(io::Error::other(message).into())
}
