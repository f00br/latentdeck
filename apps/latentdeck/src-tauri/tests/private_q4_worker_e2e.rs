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
    time::Duration,
};

use latentdeck_cartridge::{
    hash::{hash_path, hash_reader},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, CartridgeId, DType, Identifier, ParentCartridge, Sha256Digest,
        parse_manifest_json,
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
    MAX_CONTROL_FRAME_BYTES, ProfileRef, Q4Algorithm, Q4CaptureAudioPolicy, Q4CaptureMode,
    Q4CaptureReceipt, Q4CaptureStart, Q4CaptureState, Q4CaptureStatus, Q4CaptureStatusRequest,
    Q4CaptureStop, Q4CaptureVisualDtype, Q4Controls, Q4ControlsSet, Q4InfluenceMode, Q4Load,
    Q4Mode, Q4ProcessSlot, Q4ProcessSlotAck, Q4Reset, Q4ResetReason, Q4Restart, Q4Roles,
    Q4RolesSet, Q4SeedSet, Q4Slot, Q4SourceBinding, Q4Transport, Q4Xs5Routing, RingBind,
    SessionConfigure, ShutdownReason, WORKER_PROTOCOL_VERSION, WireUuid,
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
const CODEC_ROOT_ENV: &str = "LATENTDECK_PRIVATE_CODEC_ROOT";
const DECODER_ENV: &str = "LATENTDECK_PRIVATE_TAEH3";

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

#[derive(Clone)]
struct SyntheticSource {
    path: PathBuf,
    cartridge_id: WireUuid,
    archive_sha256: String,
    profile: ValidatedH3Profile,
}

#[derive(Debug, PartialEq)]
struct CycleProof {
    frames: Vec<Vec<u8>>,
    provenance: Vec<Value>,
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

    let Some((pack, decoder)) = resolve_private_prerequisites() else {
        return Ok(());
    };

    match run_worker_proof(&pack, &decoder, &sources, &temporary_root).await? {
        WorkerOutcome::Executed => {
            require(
                !tree_contains_partial(&temporary_root)?,
                "Q4 E2E left an unfinished capture spool",
            )?;
        }
        WorkerOutcome::SkippedNoCuda => {
            eprintln!("SKIP private Q4 worker E2E: CUDA device ordinal 0 is unavailable");
        }
    }
    Ok(())
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
    let cartridge = open_validated(&path, &ValidationOptions::default())?;
    let cartridge_id = parse_wire_uuid(&cartridge.manifest().cartridge_id.0)?;
    Ok(SyntheticSource {
        path,
        cartridge_id,
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
        profile: cartridge.h3_profile().clone(),
    })
}

fn require_independent_compatible_sources(sources: &[SyntheticSource; 4]) -> TestResult<()> {
    let reference = &sources[0].profile;
    let ids = sources
        .iter()
        .map(|source| source.cartridge_id.to_string())
        .collect::<BTreeSet<_>>();
    let hashes = sources
        .iter()
        .map(|source| source.archive_sha256.clone())
        .collect::<BTreeSet<_>>();
    require(
        ids.len() == 4 && hashes.len() == 4,
        "synthetic Q4 sources are not four independent cartridge identities",
    )?;
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

fn resolve_private_prerequisites() -> Option<(ValidatedCodecPack, ValidatedExternalAsset)> {
    if env::var(OPT_IN_ENV).ok().as_deref() != Some("1") {
        eprintln!("SKIP private Q4 worker E2E: set {OPT_IN_ENV}=1 to opt in");
        return None;
    }
    let Some(codec_root) = env_path(CODEC_ROOT_ENV) else {
        eprintln!("SKIP private Q4 worker E2E: {CODEC_ROOT_ENV} is unset");
        return None;
    };
    let Some(decoder_path) = env_path(DECODER_ENV) else {
        eprintln!("SKIP private Q4 worker E2E: {DECODER_ENV} is unset");
        return None;
    };
    let Ok(packs) = discover_codec_packs(
        std::slice::from_ref(&codec_root),
        latentdeck_core::product_version(),
    ) else {
        eprintln!("SKIP private Q4 worker E2E: Codec Pack discovery rejected the configured root");
        return None;
    };
    let Some(pack) = select_q4_pack(packs) else {
        eprintln!("SKIP private Q4 worker E2E: no compatible H3 pack declares a Q4 entrypoint");
        return None;
    };
    let Ok(decoder) = validate_external_asset(&pack, ASSET_ID, decoder_path) else {
        eprintln!("SKIP private Q4 worker E2E: external TAEH3 validation failed");
        return None;
    };
    Some((pack, decoder))
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
    validate_provenance(&parsed, controls, roles, seed, expected_playhead)?;
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
            && provenance.pointer("/grid/height").and_then(Value::as_u64) == Some(LATENT_HEIGHT)
            && provenance.pointer("/grid/width").and_then(Value::as_u64) == Some(LATENT_WIDTH)
            && provenance.pointer("/grid/tokens").and_then(Value::as_u64)
                == Some(LATENT_HEIGHT * LATENT_WIDTH)
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
