//! Opt-in proof against private H3 cartridges and an installed Codec Pack.
//!
//! No private bytes or machine-local paths belong in this repository. Supply
//! all four exact paths through the documented environment variables and run
//! this ignored test explicitly on a Windows CUDA host.

#![cfg(target_os = "windows")]

use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use latentdeck_cartridge::{
    hash::hash_path,
    manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, DType, Identifier, ParentCartridge,
        Sha256Digest, SourceCartridgeRef,
    },
    profile::h3::ValidatedH3Profile,
    reader::{ValidationOptions, open_validated},
    resample::{
        CaptureMode as CartridgeCaptureMode, PayloadExpectation, ResampleManifestRequest,
        pack_resample_atomic,
    },
    writer::WriteOptions,
};
use latentdeck_control::{
    Ack, BoundedVec, CodecLoad, Command, D2Algorithm, D2CaptureAudioDtype, D2CaptureAudioPolicy,
    D2CaptureAudioPolicyReason, D2CaptureControlEvent, D2CaptureMode, D2CaptureReceipt,
    D2CaptureStart, D2CaptureState, D2CaptureStatusRequest, D2CaptureStop, D2CaptureVisualDtype,
    D2Controls, D2ControlsSet, D2Load, D2ProcessSlot, D2ProcessSlotAck, D2Reset, D2ResetReason,
    D2Restart, D2Routing, D2SourceBinding, EmptyPayload, ExternalAssetBinding, FiniteF64,
    MAX_CONTROL_FRAME_BYTES, ProfileRef, RingBind, SessionConfigure, ShutdownReason, SlotLoad,
    WORKER_PROTOCOL_VERSION, WireUuid,
};
use latentdeck_core::{
    codec_pack::{
        ValidatedCodecPack, ValidatedExternalAsset, discover_codec_packs, validate_external_asset,
    },
    playback_schedule::PlaybackSchedule,
    worker_client::WorkerClient,
    worker_supervisor::{ValidatedWorkerLaunch, spawn_worker},
};
use latentdeck_gpu::{
    ring::{ReadStatus, RingDescriptor},
    windows_ring::{WindowsRgbRingConsumer, WindowsRgbRingOwner},
};
use latentdeck_library::{ImportDisposition, Library};
use semver::Version;
use serde_json::Value;
use tempfile::tempdir;

const CODEC_ROOT_ENV: &str = "LATENTDECK_PRIVATE_CODEC_ROOT";
const DECODER_ENV: &str = "LATENTDECK_PRIVATE_TAEH3";
const SOURCE_A_ENV: &str = "LATENTDECK_PRIVATE_D2_SOURCE_A";
const SOURCE_B_ENV: &str = "LATENTDECK_PRIVATE_D2_SOURCE_B";

const PACK_ID: &str = "org.latentdeck.h3";
const ASSET_ID: &str = "taeh3";
const DECK_ID: &str = "private-d2-e2e";
const OPERATOR_ID: &str = "org.latentdeck.builtin.ld_d2";
const OPERATOR_VERSION: &str = "0.1.0";
const INITIAL_GENERATION: u64 = 1;
const TEST_SEED: u64 = 42;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

type TestResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone)]
struct PrivateSource {
    path: PathBuf,
    cartridge_id: WireUuid,
    archive_sha256: String,
    profile: ValidatedH3Profile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProvenanceProof {
    linear: String,
    xs5: String,
}

#[derive(Clone)]
struct CaptureArtifact {
    algorithm: D2Algorithm,
    payload_sha256: String,
    player_slot_id: String,
    source: PrivateSource,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit private B/C cartridges, Codec Pack, TAEH3, CUDA, and local GPU time"]
async fn private_b_c_linear_xs5_worker_ring_reset_is_deterministic() -> TestResult<()> {
    let codec_root = exact_env_path(CODEC_ROOT_ENV)?;
    let decoder_path = exact_env_path(DECODER_ENV)?;
    let source_a = validate_source(exact_env_path(SOURCE_A_ENV)?)?;
    let source_b = validate_source(exact_env_path(SOURCE_B_ENV)?)?;
    require_compatible(&source_a, &source_b)?;

    let pack = select_d2_pack(&codec_root)?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;

    let first = run_sequence(&pack, &decoder, &source_a, &source_b).await?;
    let second = run_sequence(&pack, &decoder, &source_a, &source_b).await?;
    require(
        first == second,
        "identical D2 inputs, controls, seed, and reset events changed provenance",
    )?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit private B/C cartridges, Codec Pack, TAEH3, CUDA, and local GPU time"]
async fn private_snapshot_linear_xs5_packs_imports_and_replays_in_player() -> TestResult<()> {
    run_private_snapshot_release_acceptance().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicit private B/C cartridges, Codec Pack, TAEH3, CUDA, and local GPU time"]
async fn private_live_capture_controls_stop_pack_import_and_replay() -> TestResult<()> {
    run_private_live_capture_acceptance().await
}

async fn run_private_live_capture_acceptance() -> TestResult<()> {
    let codec_root = exact_env_path(CODEC_ROOT_ENV)?;
    let decoder_path = exact_env_path(DECODER_ENV)?;
    let source_a = validate_source(exact_env_path(SOURCE_A_ENV)?)?;
    let source_b = validate_source(exact_env_path(SOURCE_B_ENV)?)?;
    require_compatible(&source_a, &source_b)?;

    let pack = select_d2_pack(&codec_root)?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;
    let controls = snapshot_controls(&source_a, &source_b);
    let carrier = structural_source(controls.routing, &source_a, &source_b);
    require(
        carrier.profile.audio.is_some() && carrier.profile.visual.latent_slots > 2,
        "private B/C Live Capture acceptance requires an AV carrier longer than T=2",
    )?;

    let temporary = tempdir()?;
    let temporary_root = fs::canonicalize(temporary.path())?;
    let mut library = Library::open(temporary_root.join("library.sqlite3"))?;
    let artifact = run_live_capture_sequence(
        &pack,
        &decoder,
        &source_a,
        &source_b,
        &temporary_root,
        &mut library,
    )
    .await?;
    run_player_replay(&pack, &decoder, &artifact).await?;
    require(
        !tree_contains_partial(&temporary_root)?,
        "private Live Capture acceptance left an unfinished .partial artifact",
    )
}

async fn run_live_capture_sequence(
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    temporary_root: &Path,
    library: &mut Library,
) -> TestResult<CaptureArtifact> {
    let launch = ValidatedWorkerLaunch::from_codec_pack_d2(pack)?;
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);

    let exercise = exercise_live_capture_sequence(
        &mut client,
        pack,
        decoder,
        source_a,
        source_b,
        temporary_root,
        library,
    )
    .await;
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "D2 Live Capture worker returned an unsuccessful orderly exit",
        ),
        Err(_) => client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>),
    };

    match (exercise, shutdown) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(artifact), Ok(())) => Ok(artifact),
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "linear private Live Capture worker contract proof"
)]
async fn exercise_live_capture_sequence(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    temporary_root: &Path,
    library: &mut Library,
) -> TestResult<CaptureArtifact> {
    configure_session(client).await?;
    inspect_runtime(client, pack).await?;
    load_codec(client, pack, decoder).await?;

    let initial_controls = snapshot_controls(source_a, source_b);
    let loaded = client
        .deck_d2_load(
            D2Load {
                deck_id: DECK_ID.to_owned(),
                operator_id: OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: source_binding(source_a)?,
                source_b: source_binding(source_b)?,
                controls: initial_controls.clone(),
                transport: latentdeck_control::D2Transport::default(),
                seed: TEST_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        loaded.controls == initial_controls
            && loaded.stream_generation == INITIAL_GENERATION
            && loaded.stream_sequence == 0,
        "D2 worker did not load the exact initial Live Capture state",
    )?;

    let descriptor = RingDescriptor::new(
        source_a.profile.visual.decoded_width,
        source_a.profile.visual.decoded_height,
        INITIAL_GENERATION,
    )?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;

    let spool_container = temporary_root.join("capture-spool");
    let output_container = temporary_root.join("cartridges");
    fs::create_dir(&spool_container)?;
    fs::create_dir(&output_container)?;
    let capture_id = WireUuid::new_v4();
    let spool_root = spool_container.join(capture_id.to_string());
    fs::create_dir(&spool_root)?;
    let spool_root = fs::canonicalize(spool_root)?;
    let expected_payload = spool_root.join(format!("{capture_id}.safetensors.partial"));
    let max_latent_slots = 7;
    let max_visual_bytes = visual_bytes_for_slots(source_a, max_latent_slots)?;

    let awaiting = client
        .deck_d2_capture_start(
            D2CaptureStart {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                capture_id,
                mode: D2CaptureMode::LiveCapture,
                temporary_root: path_text(&spool_root)?,
                max_latent_slots,
                max_visual_bytes,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        awaiting.capture_id == capture_id
            && awaiting.mode == D2CaptureMode::LiveCapture
            && awaiting.state == D2CaptureState::AwaitingReset
            && awaiting.structural_carrier == initial_controls.routing
            && awaiting.latent_slots == 0
            && awaiting.current_generation == Some(INITIAL_GENERATION)
            && awaiting.target_latent_slots == Some(0)
            && awaiting.receipt.is_none(),
        "Live Capture start did not return the exact awaiting-reset contract",
    )?;
    let minimum_generation = awaiting
        .minimum_new_generation
        .ok_or_else(|| io::Error::other("Live Capture start omitted reset generation"))?;
    let reset = client
        .deck_d2_reset(
            D2Reset {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                new_stream_generation: minimum_generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        reset.causal_state_cleared
            && reset.stream_generation == minimum_generation
            && reset.reasons.contains(&D2ResetReason::TransportRestart),
        "Live Capture did not cross its exact causal restart barrier",
    )?;
    owner.adopt_generation(reset.stream_generation)?;
    consumer.adopt_generation(reset.stream_generation)?;
    require_zero_ring(&owner, &consumer)?;

    let capturing = client
        .deck_d2_capture_status(
            D2CaptureStatusRequest {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        capturing.state == D2CaptureState::Capturing
            && capturing.latent_slots == 0
            && capturing.stream_generation == Some(reset.stream_generation),
        "Live Capture did not activate at the reset stream origin",
    )?;

    process_and_drain(
        client,
        &mut consumer,
        DECK_ID,
        loaded.deck_revision,
        reset.stream_generation,
        "LINEAR",
        source_a,
        source_b,
        decoder,
    )
    .await?;
    let mut changed_controls = initial_controls.clone();
    changed_controls.algorithm = D2Algorithm::Xs5;
    changed_controls.interaction =
        FiniteF64::new(0.75).expect("finite first-party Live Capture value");
    let controls_ack = client
        .deck_d2_controls_set(
            D2ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                controls: changed_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        controls_ack.controls == changed_controls && !controls_ack.requires_causal_reset,
        "between-slot Live Capture controls were not applied atomically",
    )?;

    let armed = client
        .deck_d2_capture_stop(
            D2CaptureStop {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        armed.state == D2CaptureState::StopArmed
            && armed.latent_slots == 1
            && armed.stream_generation == Some(reset.stream_generation)
            && armed.finalize_after_latent_slots == Some(2)
            && armed.receipt.is_none(),
        "Live Capture stop was not armed through the next T=2 codec boundary",
    )?;
    process_and_drain(
        client,
        &mut consumer,
        DECK_ID,
        loaded.deck_revision,
        reset.stream_generation,
        "XS5",
        source_a,
        source_b,
        decoder,
    )
    .await?;

    let finished = client
        .deck_d2_capture_status(
            D2CaptureStatusRequest {
                deck_id: DECK_ID.to_owned(),
                deck_revision: loaded.deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        finished.state == D2CaptureState::Finished
            && finished.latent_slots == 2
            && finished.stream_generation == Some(reset.stream_generation)
            && finished.finalize_after_latent_slots.is_none(),
        "Live Capture did not finish exactly at its armed T=2 codec boundary",
    )?;
    let receipt = finished
        .receipt
        .as_ref()
        .ok_or_else(|| io::Error::other("finished Live Capture omitted its receipt"))?;
    validate_live_capture_receipt(
        receipt,
        capture_id,
        &expected_payload,
        &initial_controls,
        &changed_controls,
        source_a,
        source_b,
    )?;
    let artifact = pack_imported_live_capture(
        receipt,
        source_a,
        source_b,
        decoder,
        &output_container,
        library,
    )?;
    require(
        !expected_payload.exists(),
        "successful Live Capture pack did not consume its exact worker spool",
    )?;
    require_drained_ring(&owner, &consumer)?;
    Ok(artifact)
}

async fn run_private_snapshot_release_acceptance() -> TestResult<()> {
    let codec_root = exact_env_path(CODEC_ROOT_ENV)?;
    let decoder_path = exact_env_path(DECODER_ENV)?;
    let source_a = validate_source(exact_env_path(SOURCE_A_ENV)?)?;
    let source_b = validate_source(exact_env_path(SOURCE_B_ENV)?)?;
    require_compatible(&source_a, &source_b)?;

    let pack = select_d2_pack(&codec_root)?;
    let decoder = validate_external_asset(&pack, ASSET_ID, decoder_path)?;
    let temporary = tempdir()?;
    let temporary_root = fs::canonicalize(temporary.path())?;
    let mut library = Library::open(temporary_root.join("library.sqlite3"))?;
    let first_root = temporary_root.join("first-run");
    let repeated_root = temporary_root.join("repeated-run");
    fs::create_dir(&first_root)?;
    fs::create_dir(&repeated_root)?;

    let artifacts = run_snapshot_sequence(
        &pack,
        &decoder,
        &source_a,
        &source_b,
        &first_root,
        &mut library,
    )
    .await?;
    require(
        artifacts.len() == 2
            && artifacts[0].algorithm == D2Algorithm::Linear
            && artifacts[1].algorithm == D2Algorithm::Xs5,
        "private acceptance did not create both ordered LINEAR and XS5 snapshots",
    )?;
    assert_distinct_snapshot_payloads(&artifacts)?;
    let repeated = run_snapshot_sequence(
        &pack,
        &decoder,
        &source_a,
        &source_b,
        &repeated_root,
        &mut library,
    )
    .await?;
    assert_seeded_snapshot_repeat(&artifacts, &repeated)?;

    for artifact in &artifacts {
        run_player_replay(&pack, &decoder, artifact).await?;
    }
    require(
        !tree_contains_partial(&temporary_root)?,
        "private acceptance left an unfinished .partial artifact",
    )
}

async fn run_snapshot_sequence(
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    temporary_root: &Path,
    library: &mut Library,
) -> TestResult<Vec<CaptureArtifact>> {
    let launch = ValidatedWorkerLaunch::from_codec_pack_d2(pack)?;
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);

    let exercise = exercise_snapshot_sequence(
        &mut client,
        pack,
        decoder,
        source_a,
        source_b,
        temporary_root,
        library,
    )
    .await;
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "D2 capture worker returned an unsuccessful orderly exit",
        ),
        Err(_) => client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>),
    };

    match (exercise, shutdown) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(artifacts), Ok(())) => Ok(artifacts),
    }
}

#[allow(clippy::too_many_arguments)]
async fn exercise_snapshot_sequence(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    temporary_root: &Path,
    library: &mut Library,
) -> TestResult<Vec<CaptureArtifact>> {
    configure_session(client).await?;
    inspect_runtime(client, pack).await?;
    load_codec(client, pack, decoder).await?;

    let controls = snapshot_controls(source_a, source_b);
    let status = client
        .deck_d2_load(
            D2Load {
                deck_id: DECK_ID.to_owned(),
                operator_id: OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: source_binding(source_a)?,
                source_b: source_binding(source_b)?,
                controls: controls.clone(),
                transport: latentdeck_control::D2Transport::default(),
                seed: TEST_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        status.controls == controls
            && status.stream_generation == INITIAL_GENERATION
            && status.stream_sequence == 0,
        "D2 capture worker did not load the exact initial controls and stream state",
    )?;

    let descriptor = RingDescriptor::new(
        source_a.profile.visual.decoded_width,
        source_a.profile.visual.decoded_height,
        INITIAL_GENERATION,
    )?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;

    let spool_container = temporary_root.join("capture-spool");
    let output_container = temporary_root.join("cartridges");
    fs::create_dir(&spool_container)?;
    fs::create_dir(&output_container)?;

    let linear = capture_snapshot(
        client,
        &mut owner,
        &mut consumer,
        status.deck_revision,
        INITIAL_GENERATION,
        &controls,
        source_a,
        source_b,
        decoder,
        &spool_container,
        &output_container,
        library,
    )
    .await?;

    let mut xs5_controls = controls;
    xs5_controls.algorithm = D2Algorithm::Xs5;
    xs5_controls.interaction = FiniteF64::new(0.75).expect("finite first-party test value");
    let controls_ack = client
        .deck_d2_controls_set(
            D2ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision: status.deck_revision,
                controls: xs5_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        controls_ack.controls == xs5_controls && !controls_ack.requires_causal_reset,
        "XS5 controls were not applied atomically before Snapshot",
    )?;

    let xs5 = capture_snapshot(
        client,
        &mut owner,
        &mut consumer,
        status.deck_revision,
        linear.1,
        &xs5_controls,
        source_a,
        source_b,
        decoder,
        &spool_container,
        &output_container,
        library,
    )
    .await?;
    require_drained_ring(&owner, &consumer)?;
    Ok(vec![linear.0, xs5.0])
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "linear private Snapshot contract proof"
)]
async fn capture_snapshot(
    client: &mut WorkerClient,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
    deck_revision: u64,
    current_generation: u64,
    controls: &D2Controls,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    decoder: &ValidatedExternalAsset,
    spool_container: &Path,
    output_container: &Path,
    library: &mut Library,
) -> TestResult<(CaptureArtifact, u64)> {
    let capture_id = WireUuid::new_v4();
    let spool_root = spool_container.join(capture_id.to_string());
    fs::create_dir(&spool_root)?;
    let spool_root = fs::canonicalize(spool_root)?;
    let expected_payload = spool_root.join(format!("{capture_id}.safetensors.partial"));
    let carrier = structural_source(controls.routing, source_a, source_b);
    let target_latent_slots = carrier.profile.visual.latent_slots;
    let max_visual_bytes = snapshot_visual_bytes(carrier)?;

    let awaiting = client
        .deck_d2_capture_start(
            D2CaptureStart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
                mode: D2CaptureMode::Snapshot,
                temporary_root: path_text(&spool_root)?,
                max_latent_slots: target_latent_slots,
                max_visual_bytes,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        awaiting.capture_id == capture_id
            && awaiting.mode == D2CaptureMode::Snapshot
            && awaiting.state == D2CaptureState::AwaitingReset
            && awaiting.structural_carrier == controls.routing
            && awaiting.latent_slots == 0
            && awaiting.current_generation == Some(current_generation)
            && awaiting.target_latent_slots == Some(target_latent_slots)
            && awaiting.receipt.is_none(),
        "Snapshot start did not return the exact awaiting-reset contract",
    )?;
    let minimum_generation = awaiting
        .minimum_new_generation
        .ok_or_else(|| io::Error::other("Snapshot start omitted its minimum reset generation"))?;
    require(
        minimum_generation > current_generation,
        "Snapshot reset generation did not advance",
    )?;

    let reset = client
        .deck_d2_reset(
            D2Reset {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                new_stream_generation: minimum_generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        reset.causal_state_cleared
            && reset.stream_generation == minimum_generation
            && reset.reasons.contains(&D2ResetReason::TransportRestart),
        "Snapshot did not cross its exact causal restart barrier",
    )?;
    owner.adopt_generation(reset.stream_generation)?;
    consumer.adopt_generation(reset.stream_generation)?;
    require_zero_ring(owner, consumer)?;

    let capturing = client
        .deck_d2_capture_status(
            D2CaptureStatusRequest {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        capturing.state == D2CaptureState::Capturing
            && capturing.stream_generation == Some(reset.stream_generation)
            && capturing.latent_slots == 0
            && capturing.receipt.is_none(),
        "Snapshot did not activate at the reset stream origin",
    )?;

    let expected_algorithm = algorithm_name(controls.algorithm);
    for _ in 0..target_latent_slots {
        process_and_drain(
            client,
            consumer,
            DECK_ID,
            deck_revision,
            reset.stream_generation,
            expected_algorithm,
            source_a,
            source_b,
            decoder,
        )
        .await?;
    }
    let finished = client
        .deck_d2_capture_status(
            D2CaptureStatusRequest {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                capture_id,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        finished.state == D2CaptureState::Finished
            && finished.latent_slots == target_latent_slots
            && finished.stream_generation == Some(reset.stream_generation),
        "Snapshot did not finish after one complete structural-carrier cycle",
    )?;
    let receipt = finished
        .receipt
        .as_ref()
        .ok_or_else(|| io::Error::other("finished Snapshot omitted its capture receipt"))?;
    validate_snapshot_receipt(
        receipt,
        capture_id,
        &expected_payload,
        controls,
        source_a,
        source_b,
    )?;

    let artifact = pack_imported_snapshot(
        receipt,
        controls.algorithm,
        source_a,
        source_b,
        decoder,
        output_container,
        library,
    )?;
    require(
        !expected_payload.exists(),
        "successful atomic cartridge pack did not consume its exact worker spool",
    )?;
    Ok((artifact, reset.stream_generation))
}

fn snapshot_controls(source_a: &PrivateSource, source_b: &PrivateSource) -> D2Controls {
    let mut controls = D2Controls::default();
    if source_b.profile.visual.latent_slots < source_a.profile.visual.latent_slots {
        controls.routing = D2Routing::B;
    }
    controls
}

fn structural_source<'a>(
    routing: D2Routing,
    source_a: &'a PrivateSource,
    source_b: &'a PrivateSource,
) -> &'a PrivateSource {
    match routing {
        D2Routing::A => source_a,
        D2Routing::B => source_b,
    }
}

fn snapshot_visual_bytes(source: &PrivateSource) -> TestResult<u64> {
    [
        24_u64,
        source.profile.visual.latent_slots,
        source.profile.visual.latent_height,
        source.profile.visual.latent_width,
        2,
    ]
    .into_iter()
    .try_fold(1_u64, u64::checked_mul)
    .ok_or_else(|| io::Error::other("Snapshot visual byte count overflowed").into())
}

const fn algorithm_name(algorithm: D2Algorithm) -> &'static str {
    match algorithm {
        D2Algorithm::Linear => "LINEAR",
        D2Algorithm::Xs1 => "XS1",
        D2Algorithm::Xs2 => "XS2",
        D2Algorithm::Xs3 => "XS3",
        D2Algorithm::Xs4 => "XS4",
        D2Algorithm::Xs5 => "XS5",
    }
}

fn validate_snapshot_receipt(
    receipt: &D2CaptureReceipt,
    capture_id: WireUuid,
    expected_payload: &Path,
    controls: &D2Controls,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
) -> TestResult<()> {
    let carrier = structural_source(controls.routing, source_a, source_b);
    let measured = hash_path(expected_payload)?;
    require(
        receipt.capture_id == capture_id
            && receipt.mode == D2CaptureMode::Snapshot
            && Path::new(&receipt.payload_path) == expected_payload
            && receipt.payload_bytes == measured.byte_length
            && receipt.payload_sha256 == measured.sha256.to_string()
            && receipt.storage_dtype == D2CaptureVisualDtype::F16,
        "Snapshot receipt did not bind its exact finalized F16 spool",
    )?;
    require(
        receipt.visual_shape
            == [
                1,
                24,
                carrier.profile.visual.latent_slots,
                carrier.profile.visual.latent_height,
                carrier.profile.visual.latent_width,
            ]
            && receipt.decoded_frame_count == carrier.profile.visual.decoded_frame_count,
        "Snapshot receipt visual descriptor does not match its structural carrier cycle",
    )?;
    require(
        receipt.structural_carrier == controls.routing
            && receipt.parents[0].slot == D2Routing::A
            && receipt.parents[0].cartridge_id == source_a.cartridge_id
            && receipt.parents[0].archive_sha256 == source_a.archive_sha256
            && receipt.parents[1].slot == D2Routing::B
            && receipt.parents[1].cartridge_id == source_b.cartridge_id
            && receipt.parents[1].archive_sha256 == source_b.archive_sha256,
        "Snapshot receipt did not retain the exact ordered A/B parent identities",
    )?;
    require(
        receipt.frozen_seed == Some(TEST_SEED)
            && receipt.frozen_controls.as_ref() == Some(controls)
            && receipt.control_events.is_none(),
        "Snapshot receipt did not freeze the exact controls and seed",
    )?;

    match &carrier.profile.audio {
        None => require(
            receipt.audio_policy == D2CaptureAudioPolicy::SourceAbsent
                && receipt.audio_policy_reason.is_none()
                && receipt.audio_descriptor.is_none(),
            "visual-only Snapshot receipt did not record source_absent audio",
        ),
        Some(audio) => {
            let descriptor = receipt.audio_descriptor.as_ref().ok_or_else(|| {
                io::Error::other("AV Snapshot receipt omitted its exact audio descriptor")
            })?;
            require(
                receipt.audio_policy == D2CaptureAudioPolicy::CopiedFromCarrierExact
                    && receipt.audio_policy_reason.is_none()
                    && descriptor.storage_dtype == capture_audio_dtype(audio.storage_dtype)?
                    && descriptor.shape == [1, 32, 2, audio.latent_slots],
                "AV Snapshot receipt did not copy structural-carrier audio exactly",
            )
        }
    }
}

fn capture_audio_dtype(dtype: DType) -> TestResult<D2CaptureAudioDtype> {
    match dtype {
        DType::F16 => Ok(D2CaptureAudioDtype::F16),
        DType::F32 => Ok(D2CaptureAudioDtype::F32),
        _ => failure("validated private audio has an unsupported storage dtype"),
    }
}

fn visual_bytes_for_slots(source: &PrivateSource, latent_slots: u64) -> TestResult<u64> {
    [
        24_u64,
        latent_slots,
        source.profile.visual.latent_height,
        source.profile.visual.latent_width,
        2,
    ]
    .into_iter()
    .try_fold(1_u64, u64::checked_mul)
    .ok_or_else(|| io::Error::other("Live Capture visual byte count overflowed").into())
}

#[allow(
    clippy::too_many_arguments,
    reason = "exact private Live Capture receipt proof"
)]
fn validate_live_capture_receipt(
    receipt: &D2CaptureReceipt,
    capture_id: WireUuid,
    expected_payload: &Path,
    initial_controls: &D2Controls,
    changed_controls: &D2Controls,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
) -> TestResult<()> {
    let carrier = structural_source(initial_controls.routing, source_a, source_b);
    let measured = hash_path(expected_payload)?;
    require(
        receipt.capture_id == capture_id
            && receipt.mode == D2CaptureMode::LiveCapture
            && Path::new(&receipt.payload_path) == expected_payload
            && receipt.payload_bytes == measured.byte_length
            && receipt.payload_sha256 == measured.sha256.to_string()
            && receipt.storage_dtype == D2CaptureVisualDtype::F16,
        "Live Capture receipt did not bind its exact finalized F16 spool",
    )?;
    require(
        receipt.visual_shape
            == [
                1,
                24,
                2,
                carrier.profile.visual.latent_height,
                carrier.profile.visual.latent_width,
            ]
            && receipt.decoded_frame_count == 5,
        "Live Capture receipt did not finalize the exact T=2 H3 boundary",
    )?;
    require(
        receipt.structural_carrier == initial_controls.routing
            && receipt.parents[0].slot == D2Routing::A
            && receipt.parents[0].cartridge_id == source_a.cartridge_id
            && receipt.parents[0].archive_sha256 == source_a.archive_sha256
            && receipt.parents[1].slot == D2Routing::B
            && receipt.parents[1].cartridge_id == source_b.cartridge_id
            && receipt.parents[1].archive_sha256 == source_b.archive_sha256,
        "Live Capture receipt did not retain the exact ordered A/B parent identities",
    )?;
    let events = receipt
        .control_events
        .as_ref()
        .ok_or_else(|| io::Error::other("Live Capture receipt omitted control-event history"))?;
    require(
        receipt.frozen_seed.is_none()
            && receipt.frozen_controls.is_none()
            && events.len() == 2
            && events[0].slot_offset == 0
            && events[0].controls == *initial_controls
            && events[0].seed == TEST_SEED
            && events[1].slot_offset == 1
            && events[1].controls == *changed_controls
            && events[1].seed == TEST_SEED,
        "Live Capture receipt did not record the exact bounded between-slot controls history",
    )?;
    require(
        carrier.profile.audio.is_some()
            && receipt.audio_policy == D2CaptureAudioPolicy::OmittedTimingMismatch
            && receipt.audio_policy_reason == Some(D2CaptureAudioPolicyReason::DurationMismatch)
            && receipt.audio_descriptor.is_none(),
        "short AV Live Capture did not record explicit duration-mismatch audio omission",
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "linear private Live Capture pack, validate, and Library proof"
)]
fn pack_imported_live_capture(
    receipt: &D2CaptureReceipt,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    decoder: &ValidatedExternalAsset,
    output_container: &Path,
    library: &mut Library,
) -> TestResult<CaptureArtifact> {
    let events = receipt
        .control_events
        .as_ref()
        .ok_or_else(|| io::Error::other("Live Capture receipt omitted control events"))?;
    let first_event = events
        .first()
        .ok_or_else(|| io::Error::other("Live Capture control history is empty"))?;
    let carrier = structural_source(receipt.structural_carrier, source_a, source_b);
    let audio_source = source_cartridge_ref(carrier);
    let audio = match receipt.audio_policy {
        D2CaptureAudioPolicy::SourceAbsent => AudioDisposition::SourceAbsent,
        D2CaptureAudioPolicy::CopiedFromCarrierExact => AudioDisposition::CopiedFromCarrierExact {
            source_cartridge: audio_source,
        },
        D2CaptureAudioPolicy::OmittedTimingMismatch => {
            let reason = match receipt.audio_policy_reason {
                Some(D2CaptureAudioPolicyReason::DurationMismatch) => {
                    AudioOmissionReason::DurationMismatch
                }
                Some(D2CaptureAudioPolicyReason::TemporalMappingMismatch) => {
                    AudioOmissionReason::TemporalMappingMismatch
                }
                Some(D2CaptureAudioPolicyReason::DurationAndMappingMismatch) => {
                    AudioOmissionReason::DurationAndMappingMismatch
                }
                None => return failure("Live Capture audio omission omitted its exact reason"),
            };
            AudioDisposition::OmittedTimingMismatch {
                source_cartridge: audio_source,
                reason,
            }
        }
    };
    let control_events = serde_json::to_value(events)?;
    let structural_carrier = serde_json::to_value(receipt.structural_carrier)?;
    let controls = BTreeMap::from([
        ("control_events".to_owned(), control_events.clone()),
        ("structural_carrier".to_owned(), structural_carrier.clone()),
    ]);
    let request = ResampleManifestRequest {
        cartridge_id: CartridgeId(WireUuid::new_v4().to_string()),
        expected_payload: PayloadExpectation {
            byte_length: receipt.payload_bytes,
            sha256: Sha256Digest(receipt.payload_sha256.clone()),
        },
        capture_mode: CartridgeCaptureMode::LiveCapture,
        audio,
        parent_cartridges: vec![
            parent_cartridge(source_a, receipt.structural_carrier == D2Routing::A),
            parent_cartridge(source_b, receipt.structural_carrier == D2Routing::B),
        ],
        operator_id: Identifier(OPERATOR_ID.to_owned()),
        operator_version: OPERATOR_VERSION.to_owned(),
        seed: first_event.seed,
        controls,
    };
    let output = output_container.join("live-capture.lc");
    let spool_path = PathBuf::from(&receipt.payload_path);
    let write = pack_resample_atomic(&request, &spool_path, &output, &WriteOptions::default())?;
    require(
        write.output_path == output && write.spool_removed,
        "public resample API did not atomically finalize the Live Capture spool",
    )?;

    let validated = open_validated(&output, &ValidationOptions::default())?;
    let manifest = validated.manifest();
    let operation = manifest
        .operation_history
        .first()
        .ok_or_else(|| io::Error::other("Live Capture manifest omitted operation history"))?;
    let persisted_control_events = operation
        .controls
        .get("control_events")
        .cloned()
        .ok_or_else(|| io::Error::other("Live Capture manifest omitted control events"))?;
    let persisted_control_events =
        serde_json::from_value::<Vec<D2CaptureControlEvent>>(persisted_control_events)?;
    require(
        manifest.cartridge_id == request.cartridge_id
            && manifest.parent_cartridges == request.parent_cartridges
            && manifest.audio == request.audio,
        "fully validated Live Capture cartridge lost identity, genealogy, or audio policy",
    )?;
    require(
        manifest.operation_history.len() == 1
            && operation.operator_id.0 == OPERATOR_ID
            && operation.operator_version == OPERATOR_VERSION
            && operation.seed == TEST_SEED,
        "fully validated Live Capture cartridge lost its operator identity or initial seed",
    )?;
    require(
        persisted_control_events.len() == events.len()
            && persisted_control_events
                .iter()
                .zip(events.iter())
                .all(|(persisted, expected)| persisted == expected),
        "fully validated Live Capture cartridge changed its bounded control-event history",
    )?;
    require(
        operation.controls.get("structural_carrier") == Some(&structural_carrier),
        "fully validated Live Capture cartridge changed its structural carrier",
    )?;
    require(
        operation
            .controls
            .get("capture_mode")
            .and_then(Value::as_str)
            == Some("live_capture"),
        "fully validated Live Capture cartridge lost its capture mode",
    )?;
    require(
        manifest
            .tensors
            .iter()
            .find(|tensor| tensor.name.0 == "video")
            .is_some_and(|tensor| {
                tensor.storage_dtype == DType::F16 && tensor.shape.get(2) == Some(&2)
            }),
        "fully validated Live Capture cartridge lost its post-operator F16 T=2 tensor",
    )?;
    require(
        manifest
            .provenance
            .sources
            .iter()
            .all(|source| source.uri.is_none()),
        "Live Capture provenance serialized a source URI",
    )?;
    let manifest_value = serde_json::to_value(manifest)?;
    let private_paths = [
        source_a.path.as_path(),
        source_b.path.as_path(),
        decoder.path.as_path(),
        spool_path.as_path(),
        output.as_path(),
    ];
    require(
        private_paths.iter().try_fold(true, |clean, path| {
            Ok::<_, Box<dyn Error + Send + Sync>>(
                clean && !json_contains_fragment(&manifest_value, &path_text(path)?),
            )
        })?,
        "Live Capture manifest or provenance serialized a private machine-local path",
    )?;

    let expected_archive_sha256 = validated.receipt().archive_sha256.to_string();
    drop(validated);
    let imported = library.import_file(&output)?;
    require(
        imported.disposition == ImportDisposition::Added
            && imported.key.as_str() == expected_archive_sha256,
        "temporary Library did not add the fully validated Live Capture identity",
    )?;
    let indexed = library
        .get_cartridge(&imported.key)?
        .ok_or_else(|| io::Error::other("temporary Library lost the Live Capture import"))?;
    require(
        indexed.metadata.archive_sha256 == expected_archive_sha256
            && indexed.paths.iter().any(|path| path.path == imported.path),
        "temporary Library did not retain the Live Capture path and archive hash",
    )?;

    Ok(CaptureArtifact {
        algorithm: D2Algorithm::Xs5,
        payload_sha256: receipt.payload_sha256.clone(),
        player_slot_id: "live-capture-player".to_owned(),
        source: validate_source(output)?,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "linear private LC pack, validate, and Library contract proof"
)]
fn pack_imported_snapshot(
    receipt: &D2CaptureReceipt,
    algorithm: D2Algorithm,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    decoder: &ValidatedExternalAsset,
    output_container: &Path,
    library: &mut Library,
) -> TestResult<CaptureArtifact> {
    let controls = receipt
        .frozen_controls
        .as_ref()
        .ok_or_else(|| io::Error::other("Snapshot receipt omitted frozen controls"))?;
    let controls_value = serde_json::to_value(controls)?;
    let Value::Object(controls_object) = controls_value else {
        return failure("Snapshot controls did not serialize as a closed JSON object");
    };
    let controls_map = controls_object.into_iter().collect::<BTreeMap<_, _>>();
    let carrier = structural_source(receipt.structural_carrier, source_a, source_b);
    let audio = match receipt.audio_policy {
        D2CaptureAudioPolicy::SourceAbsent => AudioDisposition::SourceAbsent,
        D2CaptureAudioPolicy::CopiedFromCarrierExact => AudioDisposition::CopiedFromCarrierExact {
            source_cartridge: source_cartridge_ref(carrier),
        },
        D2CaptureAudioPolicy::OmittedTimingMismatch => {
            return failure("Snapshot receipt illegally omitted carrier audio");
        }
    };
    let request = ResampleManifestRequest {
        cartridge_id: CartridgeId(WireUuid::new_v4().to_string()),
        expected_payload: PayloadExpectation {
            byte_length: receipt.payload_bytes,
            sha256: Sha256Digest(receipt.payload_sha256.clone()),
        },
        capture_mode: CartridgeCaptureMode::Snapshot,
        audio,
        parent_cartridges: vec![
            parent_cartridge(source_a, receipt.structural_carrier == D2Routing::A),
            parent_cartridge(source_b, receipt.structural_carrier == D2Routing::B),
        ],
        operator_id: Identifier(OPERATOR_ID.to_owned()),
        operator_version: OPERATOR_VERSION.to_owned(),
        seed: receipt
            .frozen_seed
            .ok_or_else(|| io::Error::other("Snapshot receipt omitted frozen seed"))?,
        controls: controls_map,
    };
    let output = output_container.join(format!(
        "snapshot-{}.lc",
        algorithm_name(algorithm).to_ascii_lowercase()
    ));
    let spool_path = PathBuf::from(&receipt.payload_path);
    let write = pack_resample_atomic(&request, &spool_path, &output, &WriteOptions::default())?;
    require(
        write.output_path == output && write.spool_removed,
        "public resample API did not atomically finalize and consume its worker spool",
    )?;

    let validated = open_validated(&output, &ValidationOptions::default())?;
    let manifest = validated.manifest();
    require(
        manifest.cartridge_id == request.cartridge_id
            && manifest.parent_cartridges == request.parent_cartridges
            && manifest.operation_history.len() == 1
            && manifest.operation_history[0].operator_id.0 == OPERATOR_ID
            && manifest.operation_history[0].operator_version == OPERATOR_VERSION
            && manifest.operation_history[0].seed == TEST_SEED
            && manifest.operation_history[0]
                .controls
                .get("algorithm")
                .and_then(Value::as_str)
                == Some(algorithm_name(algorithm))
            && manifest.operation_history[0]
                .controls
                .get("capture_mode")
                .and_then(Value::as_str)
                == Some("snapshot")
            && manifest
                .tensors
                .iter()
                .find(|tensor| tensor.name.0 == "video")
                .is_some_and(|tensor| tensor.storage_dtype == DType::F16),
        "fully validated resample cartridge lost Snapshot genealogy or F16 semantics",
    )?;
    require(
        manifest
            .provenance
            .sources
            .iter()
            .all(|source| source.uri.is_none()),
        "resample provenance serialized a source URI",
    )?;
    let manifest_value = serde_json::to_value(manifest)?;
    let private_paths = [
        source_a.path.as_path(),
        source_b.path.as_path(),
        decoder.path.as_path(),
        spool_path.as_path(),
        output.as_path(),
    ];
    require(
        private_paths.iter().try_fold(true, |clean, path| {
            Ok::<_, Box<dyn Error + Send + Sync>>(
                clean && !json_contains_fragment(&manifest_value, &path_text(path)?),
            )
        })?,
        "resample manifest or provenance serialized a private machine-local path",
    )?;

    let expected_archive_sha256 = validated.receipt().archive_sha256.to_string();
    drop(validated);
    let imported = library.import_file(&output)?;
    require(
        imported.disposition == ImportDisposition::Added
            && imported.key.as_str() == expected_archive_sha256,
        "temporary Library did not add the fully validated resample identity",
    )?;
    let indexed = library
        .get_cartridge(&imported.key)?
        .ok_or_else(|| io::Error::other("temporary Library lost the imported resample"))?;
    require(
        indexed.metadata.archive_sha256 == expected_archive_sha256
            && indexed.paths.iter().any(|path| path.path == imported.path),
        "temporary Library did not retain the imported resample path and archive hash",
    )?;

    Ok(CaptureArtifact {
        algorithm,
        payload_sha256: receipt.payload_sha256.clone(),
        player_slot_id: format!(
            "snapshot-{}-player",
            algorithm_name(algorithm).to_ascii_lowercase()
        ),
        source: validate_source(output)?,
    })
}

fn assert_distinct_snapshot_payloads(artifacts: &[CaptureArtifact]) -> TestResult<()> {
    require(
        artifacts.len() == 2 && artifacts[0].payload_sha256 != artifacts[1].payload_sha256,
        "LINEAR and XS5 produced identical post-operator Snapshot payload bytes",
    )
}

fn assert_seeded_snapshot_repeat(
    first: &[CaptureArtifact],
    repeated: &[CaptureArtifact],
) -> TestResult<()> {
    require(
        first.len() == 2
            && first.len() == repeated.len()
            && first.iter().zip(repeated).all(|(left, right)| {
                left.algorithm == right.algorithm && left.payload_sha256 == right.payload_sha256
            }),
        "identical D2 inputs, controls, seed, and reset sequence changed Snapshot payload bytes",
    )
}

fn source_cartridge_ref(source: &PrivateSource) -> SourceCartridgeRef {
    SourceCartridgeRef {
        cartridge_id: CartridgeId(source.cartridge_id.to_string()),
        archive_sha256: Sha256Digest(source.archive_sha256.clone()),
    }
}

fn parent_cartridge(source: &PrivateSource, structural_carrier: bool) -> ParentCartridge {
    ParentCartridge {
        cartridge_id: CartridgeId(source.cartridge_id.to_string()),
        archive_sha256: Sha256Digest(source.archive_sha256.clone()),
        role: Identifier(if structural_carrier {
            "structural_carrier".to_owned()
        } else {
            "donor".to_owned()
        }),
    }
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

async fn run_player_replay(
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    artifact: &CaptureArtifact,
) -> TestResult<()> {
    let launch = ValidatedWorkerLaunch::from_codec_pack(pack);
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);

    let exercise = exercise_player_replay(&mut client, pack, decoder, artifact).await;
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "Player replay worker returned an unsuccessful orderly exit",
        ),
        Err(_) => client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>),
    };

    match (exercise, shutdown) {
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn exercise_player_replay(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    artifact: &CaptureArtifact,
) -> TestResult<()> {
    configure_session(client).await?;
    inspect_runtime(client, pack).await?;
    load_codec(client, pack, decoder).await?;

    let loaded = client
        .call(
            Command::SlotLoad(SlotLoad {
                slot_id: artifact.player_slot_id.clone(),
                cartridge_path: path_text(&artifact.source.path)?,
                cartridge_id: artifact.source.cartridge_id,
                expected_archive_sha256: artifact.source.archive_sha256.clone(),
                stream_generation: INITIAL_GENERATION,
            }),
            COMMAND_TIMEOUT,
        )
        .await?;
    let Ack::SlotLoad(slot) = loaded else {
        return failure("Player worker returned the wrong slot.load acknowledgement");
    };
    require(
        slot.width == artifact.source.profile.visual.decoded_width
            && slot.height == artifact.source.profile.visual.decoded_height
            && slot.timing.latent_slot_count == artifact.source.profile.visual.latent_slots
            && slot.timing.decoded_frame_count
                == artifact.source.profile.visual.decoded_frame_count,
        "Player worker loaded resample timing or geometry different from its LC manifest",
    )?;

    let descriptor = RingDescriptor::new(slot.width, slot.height, INITIAL_GENERATION)?;
    let owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;
    let mut schedule = PlaybackSchedule::new(slot, INITIAL_GENERATION)?;
    let command = schedule
        .next_decode_command()
        .ok_or_else(|| io::Error::other("fresh Player schedule omitted its prime decode"))?;
    let decode_result = client.call(command, COMMAND_TIMEOUT).await?;
    let Ack::SlotDecodeCycle(decode_ack) = decode_result else {
        return failure("Player worker returned the wrong slot.decode_cycle acknowledgement");
    };
    schedule.accept_decode(&decode_ack)?;
    require(
        decode_ack.ring_last_sequence_exclusive
            == decode_ack.ring_first_sequence + u64::from(decode_ack.decoded_frame_count),
        "Player replay decode did not declare one exact ring frame range",
    )?;
    for expected_sequence in decode_ack.ring_first_sequence..decode_ack.ring_last_sequence_exclusive
    {
        let ReadStatus::Frame(frame) = consumer.try_read()? else {
            return failure("Player replay receipt claimed a missing RGB ring frame");
        };
        require(
            frame.generation() == INITIAL_GENERATION
                && frame.sequence() == expected_sequence
                && frame.width() == artifact.source.profile.visual.decoded_width
                && frame.height() == artifact.source.profile.visual.decoded_height
                && !frame.padded_rgba().is_empty(),
            "Player replay RGB frame did not match the validated resample cartridge",
        )?;
    }
    require(
        matches!(consumer.try_read()?, ReadStatus::Empty),
        "Player replay published RGB frames outside its declared range",
    )?;
    require_drained_ring(&owner, &consumer)
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

fn exact_env_path(name: &'static str) -> TestResult<PathBuf> {
    let value = env::var_os(name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::other(format!("required environment variable {name} is unset"))
        })?;
    Ok(PathBuf::from(value))
}

fn select_d2_pack(root: &Path) -> TestResult<ValidatedCodecPack> {
    let mut candidates =
        discover_codec_packs(&[root.to_path_buf()], latentdeck_core::product_version())?
            .into_iter()
            .filter(|pack| pack.manifest.pack_id == PACK_ID)
            .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let left = Version::parse(&left.manifest.pack_version).expect("validated SemVer");
        let right = Version::parse(&right.manifest.pack_version).expect("validated SemVer");
        left.cmp(&right)
    });
    let pack = candidates
        .pop()
        .ok_or_else(|| io::Error::other("the exact discovery root has no compatible H3 pack"))?;
    require(
        pack.manifest.worker.d2_arguments.is_some(),
        "the selected H3 pack has no explicit D2 worker entrypoint",
    )?;
    Ok(pack)
}

fn validate_source(path: PathBuf) -> TestResult<PrivateSource> {
    let path = fs::canonicalize(path)?;
    let cartridge = open_validated(&path, &ValidationOptions::default())?;
    let cartridge_id = parse_wire_uuid(&cartridge.manifest().cartridge_id.0)?;
    Ok(PrivateSource {
        path,
        cartridge_id,
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
        profile: cartridge.h3_profile().clone(),
    })
}

fn require_compatible(left: &PrivateSource, right: &PrivateSource) -> TestResult<()> {
    require(
        left.profile.compatibility_key == right.profile.compatibility_key,
        "private D2 sources have different H3 compatibility keys",
    )?;
    require(
        left.profile.visual.decoded_width == right.profile.visual.decoded_width
            && left.profile.visual.decoded_height == right.profile.visual.decoded_height,
        "private D2 sources have different decoded presentation geometry",
    )
}

async fn run_sequence(
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
) -> TestResult<ProvenanceProof> {
    let launch = ValidatedWorkerLaunch::from_codec_pack_d2(pack)?;
    let pending = spawn_worker(launch).await?;
    let session = pending.connect().await?;
    let mut client = WorkerClient::new(session);

    let exercise = exercise_sequence(&mut client, pack, decoder, source_a, source_b).await;
    let shutdown = match client
        .request_shutdown(ShutdownReason::ApplicationExit, SHUTDOWN_TIMEOUT)
        .await
    {
        Ok(exit) => require(
            exit.success,
            "D2 worker returned an unsuccessful orderly exit",
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

async fn exercise_sequence(
    client: &mut WorkerClient,
    pack: &ValidatedCodecPack,
    decoder: &ValidatedExternalAsset,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
) -> TestResult<ProvenanceProof> {
    configure_session(client).await?;
    inspect_runtime(client, pack).await?;
    load_codec(client, pack, decoder).await?;

    let controls = D2Controls::default();
    let status = client
        .deck_d2_load(
            D2Load {
                deck_id: DECK_ID.to_owned(),
                operator_id: OPERATOR_ID.to_owned(),
                operator_version: OPERATOR_VERSION.to_owned(),
                source_a: source_binding(source_a)?,
                source_b: source_binding(source_b)?,
                controls: controls.clone(),
                transport: latentdeck_control::D2Transport::default(),
                seed: TEST_SEED,
                stream_generation: INITIAL_GENERATION,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        status.operator_id == OPERATOR_ID
            && status.operator_version == OPERATOR_VERSION
            && status.stream_generation == INITIAL_GENERATION
            && status.stream_sequence == 0,
        "D2 worker did not load the exact builtin operator and initial stream state",
    )?;

    let descriptor = RingDescriptor::new(
        source_a.profile.visual.decoded_width,
        source_a.profile.visual.decoded_height,
        INITIAL_GENERATION,
    )?;
    let mut owner = WindowsRgbRingOwner::create(descriptor)?;
    let mut consumer = owner.open_consumer()?;
    bind_ring(client, &owner).await?;

    let linear = process_and_drain(
        client,
        &mut consumer,
        DECK_ID,
        status.deck_revision,
        INITIAL_GENERATION,
        "LINEAR",
        source_a,
        source_b,
        decoder,
    )
    .await?;

    let xs5_generation = restart_into_xs5(
        client,
        status.deck_revision,
        controls,
        &mut owner,
        &mut consumer,
    )
    .await?;
    let xs5 = process_and_drain(
        client,
        &mut consumer,
        DECK_ID,
        status.deck_revision,
        xs5_generation,
        "XS5",
        source_a,
        source_b,
        decoder,
    )
    .await?;
    require_drained_ring(&owner, &consumer)?;
    Ok(ProvenanceProof { linear, xs5 })
}

async fn restart_into_xs5(
    client: &mut WorkerClient,
    deck_revision: u64,
    controls: D2Controls,
    owner: &mut WindowsRgbRingOwner,
    consumer: &mut WindowsRgbRingConsumer,
) -> TestResult<u64> {
    let barrier = client
        .deck_d2_restart(
            D2Restart {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        barrier.current_generation == INITIAL_GENERATION
            && barrier.minimum_new_generation > barrier.current_generation
            && barrier.reasons.contains(&D2ResetReason::TransportRestart),
        "explicit restart did not return the required causal barrier",
    )?;
    let reset = client
        .deck_d2_reset(
            D2Reset {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                new_stream_generation: barrier.minimum_new_generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        reset.causal_state_cleared
            && reset.stream_generation == barrier.minimum_new_generation
            && reset.reasons == barrier.reasons,
        "worker did not acknowledge the exact causal reset barrier",
    )?;
    owner.adopt_generation(reset.stream_generation)?;
    consumer.adopt_generation(reset.stream_generation)?;
    require_zero_ring(owner, consumer)?;

    let mut xs5_controls = controls;
    xs5_controls.algorithm = D2Algorithm::Xs5;
    xs5_controls.interaction = FiniteF64::new(0.75).expect("finite first-party test value");
    let controls_ack = client
        .deck_d2_controls_set(
            D2ControlsSet {
                deck_id: DECK_ID.to_owned(),
                deck_revision,
                controls: xs5_controls.clone(),
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    require(
        controls_ack.controls == xs5_controls && !controls_ack.requires_causal_reset,
        "XS5 control update was not applied atomically",
    )?;
    Ok(reset.stream_generation)
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
        "worker changed the bounded session contract",
    )
}

async fn inspect_runtime(client: &mut WorkerClient, pack: &ValidatedCodecPack) -> TestResult<()> {
    let ack = client
        .call(Command::CodecInspect(EmptyPayload {}), COMMAND_TIMEOUT)
        .await?;
    let Ack::CodecInspect(inspection) = ack else {
        return failure("worker returned the wrong codec.inspect acknowledgement");
    };
    require(
        inspection.cuda_available && inspection.devices.iter().any(|device| device.ordinal == 0),
        "private D2 harness requires CUDA device ordinal 0",
    )?;
    require(
        inspection.adapters.iter().any(|adapter| {
            adapter.adapter_id == pack.manifest.adapter.adapter_id
                && adapter.adapter_version == pack.manifest.adapter.adapter_version
                && adapter
                    .profiles
                    .iter()
                    .any(|profile| profile == &h3_profile())
        }),
        "D2 worker did not advertise the validated pack adapter/profile",
    )
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
            && loaded.device.ordinal == request.device_ordinal,
        "worker loaded a codec identity different from the validated selection",
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

#[allow(clippy::too_many_arguments)]
async fn process_and_drain(
    client: &mut WorkerClient,
    consumer: &mut WindowsRgbRingConsumer,
    deck_id: &str,
    deck_revision: u64,
    stream_generation: u64,
    expected_algorithm: &str,
    source_a: &PrivateSource,
    source_b: &PrivateSource,
    decoder: &ValidatedExternalAsset,
) -> TestResult<String> {
    let ack = client
        .deck_d2_process_slot(
            D2ProcessSlot {
                deck_id: deck_id.to_owned(),
                deck_revision,
                stream_generation,
            },
            COMMAND_TIMEOUT,
        )
        .await?;
    let D2ProcessSlotAck::DecodedSlot {
        deck_id: ack_deck_id,
        deck_revision: ack_revision,
        stream_generation: ack_generation,
        decoded_frame_count,
        ring_first_sequence,
        ring_last_sequence_exclusive,
        provenance_json,
        ..
    } = ack
    else {
        return failure("D2 process_slot did not produce a decoded slot");
    };
    require(
        ack_deck_id == deck_id
            && ack_revision == deck_revision
            && ack_generation == stream_generation
            && ring_first_sequence > 0
            && ring_last_sequence_exclusive == ring_first_sequence + u64::from(decoded_frame_count),
        "D2 decoded-slot receipt does not match the active stream/ring",
    )?;

    let provenance: serde_json::Value = serde_json::from_str(&provenance_json)?;
    require(
        provenance
            .pointer("/operation/operator_id")
            .and_then(serde_json::Value::as_str)
            == Some(OPERATOR_ID)
            && provenance
                .pointer("/operation/operator_version")
                .and_then(serde_json::Value::as_str)
                == Some(OPERATOR_VERSION)
            && provenance
                .pointer("/operation/controls/algorithm")
                .and_then(serde_json::Value::as_str)
                == Some(expected_algorithm)
            && provenance
                .pointer("/operation/seed")
                .and_then(serde_json::Value::as_u64)
                == Some(TEST_SEED),
        "D2 provenance did not bind the exact operator, algorithm, and seed",
    )?;
    let encoded_private_paths = [
        path_text(&source_a.path)?,
        path_text(&source_b.path)?,
        path_text(&decoder.path)?,
    ];
    require(
        encoded_private_paths
            .iter()
            .all(|path| !provenance_json.contains(path)),
        "D2 provenance exposed a private machine-local path",
    )?;

    for expected_sequence in ring_first_sequence..ring_last_sequence_exclusive {
        let ReadStatus::Frame(frame) = consumer.try_read()? else {
            return failure("D2 worker receipt claimed a frame that was not in the RGB ring");
        };
        require(
            frame.generation() == stream_generation
                && frame.sequence() == expected_sequence
                && frame.width() == source_a.profile.visual.decoded_width
                && frame.height() == source_a.profile.visual.decoded_height
                && !frame.padded_rgba().is_empty(),
            "RGB ring frame metadata does not match the D2 decoded-slot receipt",
        )?;
    }
    require(
        matches!(consumer.try_read()?, ReadStatus::Empty),
        "D2 process_slot published frames outside its declared ring range",
    )?;
    Ok(provenance_json)
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

fn require_drained_ring(
    owner: &WindowsRgbRingOwner,
    consumer: &WindowsRgbRingConsumer,
) -> TestResult<()> {
    let owner_state = owner.state()?;
    let consumer_state = consumer.state()?;
    require(
        owner_state.producer_sequence() == owner_state.consumer_sequence()
            && owner_state.occupancy() == 0
            && consumer_state.producer_sequence() == consumer_state.consumer_sequence()
            && consumer_state.occupancy() == 0,
        "RGB ring retained frames after the declared decoded-slot range was consumed",
    )
}

fn source_binding(source: &PrivateSource) -> TestResult<D2SourceBinding> {
    Ok(D2SourceBinding {
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
    Ok(serde_json::from_value(serde_json::Value::String(
        value.to_owned(),
    ))?)
}

fn path_text(path: &Path) -> TestResult<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("private harness path is not valid UTF-8").into())
}

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition { Ok(()) } else { failure(message) }
}

fn failure<T>(message: &'static str) -> TestResult<T> {
    Err(io::Error::other(message).into())
}
