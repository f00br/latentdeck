#![cfg(target_os = "windows")]
#![allow(
    unsafe_code,
    reason = "the test worker consumes target-process DuplicateHandle values exactly once"
)]
#![allow(
    clippy::too_many_lines,
    reason = "one self-contained test harness mirrors the closed Protocol 2 command sequence"
)]

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Cursor, Read, Write as _},
    os::windows::io::{FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    time::Duration,
};

use latentdeck_cartridge::{
    hash::{hash_path, hash_reader},
    limits::ValidationLimits,
    manifest::{
        CartridgeId, Identifier, ManifestV0_1, OperationRecord, ParentCartridge,
        ProducerDescriptor, Rational, Sha256Digest, TensorStream, parse_manifest_json,
    },
    reader::{ValidationOptions, open_integrity_validated},
    resample::{PayloadExpectation, ProfileResampleRequest, pack_profile_resample_atomic},
    writer::{PackRequest as CartridgePackRequest, WriteOptions, pack_integrity_atomic},
};
use latentdeck_control::{
    WireUuid,
    v2::{
        Ack, AckReply, Capability, CaptureArtifact, CaptureIdentity, CaptureMode, CaptureStart,
        CaptureState, CaptureStatusSnapshot, CodecDescriptor, CodecLoaded, CodecState, Command,
        ControlValue, DeckProcess, DeckProcessAck, DeckReset, DeckState, DeckStatusSnapshot,
        DecodedAbi, EmptyPayload, Envelope, Event, EventMessage, LimitedVec, MAX_CAPABILITIES,
        MAX_CONTROLS, MAX_FRAME_BYTES, MAX_PROFILES, MAX_SOURCES, Message, PROTOCOL_VERSION,
        PlayerState, PlayerStatusSnapshot, PlayerStep, PlayerStepAck, PlayheadSnapshot,
        ProfileInspection, ProfileKey, ProfileReceipt, ProvenanceEntry, RingConfigured, RingKind,
        RoleBinding, SessionConfigured, SessionState, ShutdownAck, ShutdownReason,
        SignalGeometry as ProtocolSignalGeometry, SourceOpened, SourceTransportBinding,
        StatusSnapshot, TensorAbi, TensorDtype as ProtocolTensorDtype, WorkerHello,
        WorkerHelloAuthToken, decode_messagepack, encode_messagepack,
    },
};
use latentdeck_core::{
    deck_selection_v2::{
        DeckPackageSelectionV2, DeckSelectionV2Error, DeckSourceSelectionV2,
        prepare_exact_deck_selection, prepare_exact_deck_selection_with_cache,
    },
    deck_session_v2::{DeckSessionV2LoadRequest, start_deck_session_v2},
    player_session_v2::{PlayerSessionV2HostContract, start_player_session_v2},
};
use latentdeck_extension_manager::{
    ActivePackageCache, Architecture, BundledPackageEntry, BundledPackageIndex,
    CodecAdapterDescriptor, CodecCapability, CodecCompatibility, CodecPackManifest,
    CodecWorkerDescriptor, DeckCompatibility, DeckPackManifest, DeckRoleDescriptor,
    DeckRuntimeDescriptor, DeckRuntimeKind, DeckSignalDescriptor, ExtensionRoots, InstallRequest,
    IntegrityCatalog, IntegrityDescriptor, IntegrityFile, LicenseDescriptor, OperatingSystem,
    PackRequest, PackageKind, PackageReference, PlatformDescriptor,
    ProfileKey as ManifestProfileKey, PublisherDescriptor, PublisherIdentityClaim,
    PythonConstraint, PythonImplementation, RuntimeLockDescriptor,
    SignalGeometry as ManifestSignalGeometry, TensorDevice, TensorDtype as ManifestTensorDtype,
    TimingDescriptor, enable, install, install_from_bundled_index, pack, resolve_active,
};
use latentdeck_gpu::{
    ring_v2::{ReadV2Status, WriteV2Status, control_mapping_bytes},
    windows_ring::FramesReady,
    windows_ring_v2::WindowsRgbRingV2Producer,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::ClientOptions,
};
use uuid::Uuid;

const APP_VERSION: &str = "0.2.0";
const CODEC_ID: &str = "dev.latentdeck.synthetic.codec";
const CODEC_VERSION: &str = "2.0.0";
const ADAPTER_ID: &str = "dev.latentdeck.synthetic.adapter";
const ADAPTER_VERSION: &str = "2.0.0";
const DECK_VERSION: &str = "0.2.0";
const COMPATIBLE_DECK_ID: &str = "dev.latentdeck.synthetic.deck2";
const COMPATIBLE_DECK4_ID: &str = "dev.latentdeck.synthetic.deck4";
const SIGNAL_DECK_ID: &str = "dev.latentdeck.synthetic.bad-signal";
const TIMING_DECK_ID: &str = "dev.latentdeck.synthetic.bad-timing";
const CAPABILITY_DECK_ID: &str = "dev.latentdeck.synthetic.bad-capability";
const PROFILE_DECK_ID: &str = "dev.latentdeck.synthetic.bad-profile";
const DTYPE_DECK_ID: &str = "dev.latentdeck.synthetic.bad-dtype";
const DEVICE_DECK_ID: &str = "dev.latentdeck.synthetic.bad-device";
const BUNDLED_D2_ID: &str = "org.latentdeck.deck.d2";
const BUNDLED_Q4_ID: &str = "org.latentdeck.deck.q4";
const CARTRIDGE_ID: &str = "550e8400-e29b-41d4-a716-446655440042";
const CAPTURED_CARTRIDGE_ID: &str = "550e8400-e29b-41d4-a716-446655440043";
const PROFILE_FAMILY: &str = "synthetic_test";
const PROFILE_NAME: &str = "non_h3_latent";
const PROFILE_VERSION: &str = "0.2.0";
const TIMING_CONTRACT: &str = "synthetic_step";
const TORCH_BUILD: &str = "2.13.0+cu130";
const WORKER_HELPER: &str = "synthetic_protocol2_worker_child";
const WORKER_MARKER: &str = "synthetic-worker-started.marker";
const REPLAY_CAPTURE_MAX_BYTES: u64 = 128 * 1_024;

#[derive(Clone, Copy)]
struct DeckFixture<'a> {
    id: &'a str,
    source_count: u8,
    latent_width: u32,
    fps_numerator: u32,
    required_capability: CodecCapability,
    profile_name: &'a str,
    tensor_dtype: ManifestTensorDtype,
    tensor_device: TensorDevice,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_non_h3_codec_runs_external_decks_without_p1_fallback() {
    let temp = TempDir::new().expect("temporary matrix root");
    let roots = ExtensionRoots::for_base_root(temp.path().join("LatentDeck"));
    let marker = roots.base_root.join(WORKER_MARKER);
    let current_test_exe = std::env::current_exe().expect("current integration test executable");

    install_codec(&roots, temp.path(), &current_test_exe);
    for fixture in [
        DeckFixture {
            id: COMPATIBLE_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: COMPATIBLE_DECK4_ID,
            source_count: 4,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: SIGNAL_DECK_ID,
            source_count: 2,
            latent_width: 44,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: TIMING_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 25,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: CAPABILITY_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::RawImport,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: PROFILE_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: "other_profile",
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: DTYPE_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp32,
            tensor_device: TensorDevice::Cuda,
        },
        DeckFixture {
            id: DEVICE_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cpu,
        },
    ] {
        install_deck(&roots, temp.path(), fixture);
    }

    let cartridge = write_synthetic_cartridge(temp.path());
    let library_validated =
        open_integrity_validated(&cartridge.path, &ValidationOptions::default())
            .expect("Library-equivalent retained validation");
    let sources = repeated_retained_sources(&cartridge, &library_validated, 2);

    assert_refused(
        &roots,
        &marker,
        SIGNAL_DECK_ID,
        &sources,
        DeckSelectionV2Error::UnsupportedSignal,
    );
    assert_refused(
        &roots,
        &marker,
        TIMING_DECK_ID,
        &sources,
        DeckSelectionV2Error::UnsupportedTiming,
    );
    assert_refused(
        &roots,
        &marker,
        CAPABILITY_DECK_ID,
        &sources,
        DeckSelectionV2Error::UnsupportedCapability,
    );
    assert_refused(
        &roots,
        &marker,
        PROFILE_DECK_ID,
        &sources,
        DeckSelectionV2Error::UnsupportedProfile,
    );
    assert_refused(
        &roots,
        &marker,
        DTYPE_DECK_ID,
        &sources,
        DeckSelectionV2Error::UnsupportedTensorAbi,
    );
    assert_refused(
        &roots,
        &marker,
        DEVICE_DECK_ID,
        &sources,
        DeckSelectionV2Error::UnsupportedTensorAbi,
    );
    let exact = selection(COMPATIBLE_DECK_ID);
    let active_packages = ActivePackageCache::new();
    let prepared = prepare_exact_deck_selection_with_cache(
        &roots,
        &active_packages,
        &exact,
        &sources,
        APP_VERSION,
    )
    .expect("exact synthetic Deck/Codec pair");
    assert_eq!(prepared.sources.len(), 2);
    assert_eq!(prepared.cartridges.len(), 2);
    assert_eq!(prepared.validation_work.full_cartridge_validations, 0);
    assert_eq!(prepared.validation_work.retained_handle_clones, 2);
    assert_eq!(active_packages.stats().cold_full_hash_passes, 1);
    assert_eq!(active_packages.stats().persistent_fast_checkouts, 1);
    assert_eq!(active_packages.stats().cached_checkouts, 0);

    let repeated_preflight = prepare_exact_deck_selection_with_cache(
        &roots,
        &active_packages,
        &exact,
        &sources,
        APP_VERSION,
    )
    .expect("repeat exact preflight through process cache");
    assert_eq!(
        repeated_preflight
            .validation_work
            .full_cartridge_validations,
        0
    );
    assert_eq!(repeated_preflight.validation_work.retained_handle_clones, 2);
    assert_eq!(active_packages.stats().cold_full_hash_passes, 1);
    assert_eq!(active_packages.stats().persistent_fast_checkouts, 1);
    assert_eq!(active_packages.stats().cached_checkouts, 2);
    drop(repeated_preflight);
    assert_eq!(prepared.host.profile_key, protocol_profile());
    assert_eq!(prepared.host.signal_geometry, protocol_signal());
    assert_eq!(prepared.host.tensor_abi, tensor_abi());
    assert_eq!(
        prepared.deck_runtime.runtime_binding().deck_id,
        COMPATIBLE_DECK_ID
    );
    assert_eq!(
        prepared.deck_runtime.runtime_binding().deck_version,
        DECK_VERSION
    );
    assert!(
        Path::new(&prepared.deck_runtime.runtime_binding().python_root)
            .join("synthetic_operator.py")
            .is_file(),
        "the dynamic runtime must come from the installed external .ld tree"
    );

    let load = load_request(2);
    let deck_session_id = prepared.host.deck_session_id;
    let ring_id = prepared.host.ring_id;
    let first_generation = prepared.host.stream_generation;
    let mut session = start_deck_session_v2(
        prepared.codec_package,
        prepared.deck_runtime,
        prepared.cartridges,
        prepared.host,
        prepared.external_assets,
        load.clone(),
    )
    .await
    .expect("synthetic non-H3 Deck Protocol 2 startup");

    assert!(
        marker.is_file(),
        "the exact installed Protocol 2 worker ran"
    );
    assert_eq!(session.cartridges().len(), 2);
    assert_eq!(session.profile_receipts().len(), 2);
    for receipt in session.profile_receipts() {
        assert_eq!(receipt.pack_id, CODEC_ID);
        assert_eq!(receipt.pack_version, CODEC_VERSION);
        assert_eq!(receipt.adapter_id, ADAPTER_ID);
        assert_eq!(receipt.adapter_version, ADAPTER_VERSION);
        assert_eq!(receipt.profile_key, protocol_profile());
        assert_eq!(receipt.tensor_abi, tensor_abi());
    }
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .open(&cartridge.path)
            .is_err(),
        "retained LC handles must deny share-write for the live session"
    );
    assert_eq!(
        session.initial_status().source_transport.as_slice(),
        load.source_transport
    );

    let Ack::DeckProcess(processed) = session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id,
                deck_revision: 1,
                stream_generation: first_generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("process one generic Deck tick")
    else {
        panic!("worker returned the wrong deck.process acknowledgement");
    };
    assert_eq!(processed.output_ring_id, ring_id);
    assert_eq!(processed.status.stream_sequence, 1);
    assert_eq!(
        session
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("ABI2 ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(batch) = session
        .ring_consumer_mut()
        .try_read()
        .expect("read synthetic ABI2 batch")
    else {
        panic!("deck.process must publish one complete ABI2 batch");
    };
    assert_eq!(batch.metadata().generation(), first_generation);
    assert_eq!(batch.metadata().logical_sequence(), 1);
    assert_eq!(batch.metadata().session_id(), *deck_session_id.as_bytes());
    assert_eq!((batch.width(), batch.height()), (3, 1));
    assert_eq!(batch.pixels(), &[0xa2; 12]);
    let first_output_sha256 = sha256(batch.pixels());
    let first_status = processed.status.clone();
    let first_provenance = processed.provenance.clone();

    let reset_generation = first_generation + 1;
    let Ack::DeckReset(reset) = session
        .client_mut()
        .call(
            Command::DeckReset(DeckReset {
                deck_session_id,
                deck_revision: 1,
                new_stream_generation: reset_generation,
                preserve_playheads: false,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("reset generic Deck session")
    else {
        panic!("worker returned the wrong deck.reset acknowledgement");
    };
    assert_eq!(reset.stream_generation, reset_generation);
    assert_eq!(reset.stream_sequence, 0);
    session
        .adopt_ring_generation(reset_generation)
        .expect("Core adopts exact worker-reset ABI2 generation");
    assert_eq!(
        session
            .ring_owner()
            .state()
            .expect("reset ring state")
            .occupancy(),
        0
    );

    let Ack::DeckStatus(status) = session
        .client_mut()
        .call(
            Command::DeckStatus(EmptyPayload {}),
            Duration::from_secs(10),
        )
        .await
        .expect("read generic Deck status")
    else {
        panic!("worker returned the wrong deck.status acknowledgement");
    };
    assert_eq!(status.stream_generation, reset_generation);
    assert_eq!(status.roles.as_slice(), load.roles);
    assert_eq!(status.source_transport.as_slice(), load.source_transport);

    let Ack::DeckProcess(replayed) = session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id,
                deck_revision: 1,
                stream_generation: reset_generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("replay the exact seeded Deck command after reset")
    else {
        panic!("worker returned the wrong replay deck.process acknowledgement");
    };
    assert_eq!(
        replayed.status.stream_sequence,
        first_status.stream_sequence
    );
    assert_eq!(replayed.status.playheads, first_status.playheads);
    assert_eq!(replayed.status.roles, first_status.roles);
    assert_eq!(replayed.status.controls, first_status.controls);
    assert_eq!(
        replayed.status.source_transport,
        first_status.source_transport
    );
    assert_eq!(replayed.status.seed, first_status.seed);
    assert_eq!(replayed.status.state, first_status.state);
    assert_eq!(replayed.status.capture_state, first_status.capture_state);
    assert_eq!(replayed.provenance, first_provenance);
    assert_eq!(
        session
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("replay ABI2 ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(replay_batch) = session
        .ring_consumer_mut()
        .try_read()
        .expect("read replayed synthetic ABI2 batch")
    else {
        panic!("replayed deck.process must publish one complete ABI2 batch");
    };
    assert_eq!(replay_batch.metadata().generation(), reset_generation);
    assert_eq!(replay_batch.metadata().logical_sequence(), 1);
    assert_eq!(
        replay_batch.metadata().session_id(),
        *deck_session_id.as_bytes()
    );
    assert_eq!(sha256(replay_batch.pixels()), first_output_sha256);

    let exit = session
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .expect("exact Protocol 2 shutdown");
    assert!(exit.success, "synthetic worker exits cleanly: {exit}");
    drop(session);

    let four_sources = repeated_retained_sources(&cartridge, &library_validated, 4);
    let prepared = prepare_exact_deck_selection(
        &roots,
        &selection(COMPATIBLE_DECK4_ID),
        &four_sources,
        APP_VERSION,
    )
    .expect("exact four-source synthetic Deck/Codec pair");
    assert_eq!(prepared.sources.len(), 4);
    assert_eq!(prepared.validation_work.full_cartridge_validations, 0);
    assert_eq!(prepared.validation_work.retained_handle_clones, 4);
    let four_session_id = prepared.host.deck_session_id;
    let four_ring_id = prepared.host.ring_id;
    let four_generation = prepared.host.stream_generation;
    let mut four_session = start_deck_session_v2(
        prepared.codec_package,
        prepared.deck_runtime,
        prepared.cartridges,
        prepared.host,
        prepared.external_assets,
        load_request(4),
    )
    .await
    .expect("four-source synthetic non-H3 Deck startup");
    assert_eq!(four_session.cartridges().len(), 4);
    assert_eq!(four_session.profile_receipts().len(), 4);

    let Ack::DeckProcess(processed) = four_session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id: four_session_id,
                deck_revision: 1,
                stream_generation: four_generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("process one four-source generic Deck tick")
    else {
        panic!("four-source worker returned the wrong deck.process acknowledgement");
    };
    assert_eq!(processed.output_ring_id, four_ring_id);
    assert_eq!(processed.status.playheads.len(), 4);
    assert_eq!(
        four_session
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("four-source ABI2 ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(batch) = four_session
        .ring_consumer_mut()
        .try_read()
        .expect("read four-source ABI2 batch")
    else {
        panic!("four-source deck.process must publish one ABI2 batch");
    };
    assert_eq!(batch.pixels(), &[0xa4; 12]);
    assert_eq!(batch.metadata().session_id(), *four_session_id.as_bytes());
    let exit = four_session
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .expect("four-source exact Protocol 2 shutdown");
    assert!(
        exit.success,
        "four-source synthetic worker exits cleanly: {exit}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_non_h3_codec_runs_bundled_d2_and_q4_over_authenticated_p2() {
    let temp = TempDir::new().expect("temporary bundled matrix root");
    let roots = ExtensionRoots::for_base_root(temp.path().join("LatentDeck"));
    let current_test_exe = std::env::current_exe().expect("current integration test executable");
    install_codec(&roots, temp.path(), &current_test_exe);

    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root");
    install_bundled_deck(
        &roots,
        temp.path(),
        &repository_root.join("operators/builtin/d2/package"),
        BUNDLED_D2_ID,
    );
    install_bundled_deck(
        &roots,
        temp.path(),
        &repository_root.join("operators/builtin/q4/package"),
        BUNDLED_Q4_ID,
    );

    let cartridge = write_synthetic_cartridge(temp.path());
    Box::pin(run_bundled_deck_session(
        &roots,
        &cartridge,
        BUNDLED_D2_ID,
        2,
    ))
    .await;
    Box::pin(run_bundled_deck_session(
        &roots,
        &cartridge,
        BUNDLED_Q4_ID,
        4,
    ))
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finalized_capture_replays_through_exact_synthetic_codec() {
    let temp = TempDir::new().expect("temporary capture replay root");
    let roots = ExtensionRoots::for_base_root(temp.path().join("LatentDeck"));
    let current_test_exe = std::env::current_exe().expect("current integration test executable");
    install_codec(&roots, temp.path(), &current_test_exe);
    install_deck(
        &roots,
        temp.path(),
        DeckFixture {
            id: COMPATIBLE_DECK_ID,
            source_count: 2,
            latent_width: 45,
            fps_numerator: 24,
            required_capability: CodecCapability::Realtime,
            profile_name: PROFILE_NAME,
            tensor_dtype: ManifestTensorDtype::Fp16,
            tensor_device: TensorDevice::Cuda,
        },
    );

    let source = write_synthetic_cartridge(temp.path());
    let sources = repeated_sources(&source, 2);
    let prepared = prepare_exact_deck_selection(
        &roots,
        &selection(COMPATIBLE_DECK_ID),
        &sources,
        APP_VERSION,
    )
    .expect("exact Deck capture selection");
    let deck_session_id = prepared.host.deck_session_id;
    let generation = prepared.host.stream_generation;
    let mut deck = start_deck_session_v2(
        prepared.codec_package,
        prepared.deck_runtime,
        prepared.cartridges,
        prepared.host,
        prepared.external_assets,
        load_request(2),
    )
    .await
    .expect("capture Deck session");

    let capture_id = Uuid::new_v4();
    let capture_root = roots.base_root.join("CaptureStaging").join("replay");
    fs::create_dir_all(&capture_root).expect("host-owned replay staging root");
    let Ack::CaptureStart(started) = deck
        .client_mut()
        .call(
            Command::CaptureStart(CaptureStart {
                deck_session_id,
                deck_revision: 1,
                capture_id,
                mode: CaptureMode::Snapshot,
                staging_root: capture_root.to_string_lossy().into_owned(),
                maximum_latent_slots: 8,
                maximum_visual_bytes: REPLAY_CAPTURE_MAX_BYTES,
                maximum_reset_events: 4,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("start replayable snapshot capture")
    else {
        panic!("snapshot start returned the wrong acknowledgement");
    };
    assert_eq!(started.state, CaptureState::Capturing);
    let Ack::DeckProcess(processed) = deck
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id,
                deck_revision: 1,
                stream_generation: generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("capture post-operator latent")
    else {
        panic!("capture process returned the wrong acknowledgement");
    };
    assert_eq!(processed.status.capture_state, CaptureState::Completed);
    consume_capture_ring(&mut deck, generation, 1);
    let Ack::CaptureStatus(completed) = deck
        .client_mut()
        .call(
            Command::CaptureStatus(CaptureIdentity {
                deck_session_id,
                deck_revision: 1,
                capture_id,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("read completed replayable snapshot")
    else {
        panic!("capture status returned the wrong acknowledgement");
    };
    let artifact = completed
        .artifact
        .as_ref()
        .expect("completed capture artifact")
        .clone();
    let exit = deck
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .expect("capture Deck shutdown");
    assert!(exit.success);
    drop(deck);

    let captured_path = temp.path().join("captured-replay.lc");
    finalize_synthetic_capture(&source, &artifact, &captured_path);
    let captured = open_integrity_validated(&captured_path, &ValidationOptions::default())
        .expect("finalized capture reopens before replay");
    assert_eq!(captured.manifest().cartridge_id.0, CAPTURED_CARTRIDGE_ID);
    let package = resolve_active(&roots, &codec_reference()).expect("exact active Codec Pack");
    let host = player_host_contract();
    let player_session_id = host.player_session_id;
    let generation = host.stream_generation;
    let mut player = start_player_session_v2(package, captured, host, Vec::new())
        .await
        .expect("captured LC starts on the exact synthetic Codec Pack");
    let Ack::PlayerStep(step) = player
        .client_mut()
        .call(
            Command::PlayerStep(PlayerStep {
                player_session_id,
                stream_generation: generation,
                maximum_decoded_frames: 24,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("decode finalized capture")
    else {
        panic!("captured replay returned the wrong acknowledgement");
    };
    assert_eq!(step.status.player_session_id, player_session_id);
    assert_eq!(step.decoded_frames, 1);
    assert_eq!(
        player
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("replay ABI2 ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(batch) = player
        .ring_consumer_mut()
        .try_read()
        .expect("read replayed capture frame")
    else {
        panic!("captured replay must publish one ABI2 batch");
    };
    assert_eq!(batch.pixels(), &[0xc1; 12]);
    let exit = player
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .expect("captured Player shutdown");
    assert!(exit.success);
}

async fn run_bundled_deck_session(
    roots: &ExtensionRoots,
    cartridge: &CartridgeFixture,
    deck_id: &str,
    source_count: u8,
) {
    let sources = repeated_sources(cartridge, usize::from(source_count));
    let prepared = prepare_exact_deck_selection(roots, &selection(deck_id), &sources, APP_VERSION)
        .expect("exact bundled Deck/non-H3 Codec pair");
    assert_eq!(prepared.host.profile_key, protocol_profile());
    assert_eq!(prepared.host.signal_geometry, protocol_signal());
    assert_eq!(prepared.host.tensor_abi, tensor_abi());
    assert_ne!(prepared.host.profile_key.codec_family, "minimax_h3");
    let runtime = prepared.deck_runtime.runtime_binding();
    assert_eq!(runtime.deck_id, deck_id);
    assert_eq!(runtime.deck_version, DECK_VERSION);
    let package_root = Path::new(&runtime.python_root);
    let module_name = if deck_id == BUNDLED_D2_ID {
        "latentdeck_operator_d2"
    } else {
        "latentdeck_operator_q4"
    };
    assert!(package_root.join(module_name).join("operator.py").is_file());
    assert!(!package_root.join(module_name).join("stream.py").exists());
    assert!(!package_root.join(module_name).join("trusted.py").exists());

    let deck_session_id = prepared.host.deck_session_id;
    let ring_id = prepared.host.ring_id;
    let generation = prepared.host.stream_generation;
    let load = bundled_load_request(deck_id);
    let mut session = start_deck_session_v2(
        prepared.codec_package,
        prepared.deck_runtime,
        prepared.cartridges,
        prepared.host,
        prepared.external_assets,
        load.clone(),
    )
    .await
    .expect("bundled Deck Protocol 2 startup on synthetic non-H3 codec");
    assert_eq!(session.profile_receipts().len(), usize::from(source_count));
    assert!(session.profile_receipts().iter().all(|receipt| {
        receipt.profile_key.codec_family == PROFILE_FAMILY
            && receipt.tensor_abi.device == latentdeck_control::v2::DeviceKind::Cuda
    }));
    assert_eq!(session.initial_status().roles.as_slice(), load.roles);

    let Ack::DeckProcess(processed) = session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id,
                deck_revision: 1,
                stream_generation: generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("process bundled Deck tick")
    else {
        panic!("bundled Deck returned the wrong deck.process acknowledgement");
    };
    assert_eq!(processed.output_ring_id, ring_id);
    assert_eq!(processed.status.playheads.len(), usize::from(source_count));
    assert_eq!(
        session
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("bundled Deck ABI2 ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(batch) = session
        .ring_consumer_mut()
        .try_read()
        .expect("read bundled Deck ABI2 batch")
    else {
        panic!("bundled Deck must publish one ABI2 batch");
    };
    assert_eq!(
        batch.pixels(),
        &[0xa0 + source_count; 12],
        "worker processed the exact bundled Deck source arity"
    );
    run_bundled_capture_modes(&mut session, roots, deck_id, deck_session_id, generation).await;

    let reset_generation = generation + 1;
    let Ack::DeckReset(reset) = session
        .client_mut()
        .call(
            Command::DeckReset(DeckReset {
                deck_session_id,
                deck_revision: 1,
                new_stream_generation: reset_generation,
                preserve_playheads: false,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("reset bundled Deck session")
    else {
        panic!("bundled Deck returned the wrong deck.reset acknowledgement");
    };
    assert_eq!(reset.stream_generation, reset_generation);
    assert_eq!(reset.stream_sequence, 0);

    let exit = session
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .expect("bundled Deck exact Protocol 2 shutdown");
    assert!(
        exit.success,
        "bundled Deck synthetic worker exits cleanly: {exit}"
    );
}

async fn run_bundled_capture_modes(
    session: &mut latentdeck_core::deck_session_v2::DeckSessionV2,
    roots: &ExtensionRoots,
    deck_id: &str,
    deck_session_id: Uuid,
    generation: u64,
) {
    let snapshot_id = Uuid::new_v4();
    let snapshot_root = roots
        .base_root
        .join("CaptureStaging")
        .join(deck_id.replace('.', "_"))
        .join("snapshot");
    fs::create_dir_all(&snapshot_root).expect("host-owned snapshot staging root");
    let Ack::CaptureStart(snapshot_started) = session
        .client_mut()
        .call(
            Command::CaptureStart(CaptureStart {
                deck_session_id,
                deck_revision: 1,
                capture_id: snapshot_id,
                mode: CaptureMode::Snapshot,
                staging_root: snapshot_root.to_string_lossy().into_owned(),
                maximum_latent_slots: 8,
                maximum_visual_bytes: 1_024,
                maximum_reset_events: 4,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("start bundled snapshot capture")
    else {
        panic!("bundled Deck returned the wrong snapshot start acknowledgement");
    };
    assert_eq!(snapshot_started.state, CaptureState::Capturing);
    assert_eq!(snapshot_started.mode, CaptureMode::Snapshot);
    assert_eq!(snapshot_started.latent_slots, 0);
    assert!(snapshot_started.artifact.is_none());

    let Ack::DeckProcess(snapshot_process) = session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id,
                deck_revision: 1,
                stream_generation: generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("process snapshot post-operator boundary")
    else {
        panic!("bundled Deck returned the wrong snapshot process acknowledgement");
    };
    assert_eq!(
        snapshot_process.status.capture_state,
        CaptureState::Completed
    );
    consume_capture_ring(session, generation, 2);
    let Ack::CaptureStatus(snapshot) = session
        .client_mut()
        .call(
            Command::CaptureStatus(CaptureIdentity {
                deck_session_id,
                deck_revision: 1,
                capture_id: snapshot_id,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("read completed snapshot receipt")
    else {
        panic!("bundled Deck returned the wrong snapshot status acknowledgement");
    };
    assert_capture_receipt(&snapshot, &snapshot_root, CaptureMode::Snapshot, 1, 1_024);

    let live_id = Uuid::new_v4();
    let live_root = roots
        .base_root
        .join("CaptureStaging")
        .join(deck_id.replace('.', "_"))
        .join("live");
    fs::create_dir_all(&live_root).expect("host-owned live staging root");
    let Ack::CaptureStart(live_started) = session
        .client_mut()
        .call(
            Command::CaptureStart(CaptureStart {
                deck_session_id,
                deck_revision: 1,
                capture_id: live_id,
                mode: CaptureMode::LiveCapture,
                staging_root: live_root.to_string_lossy().into_owned(),
                maximum_latent_slots: 8,
                maximum_visual_bytes: 1_024,
                maximum_reset_events: 4,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("start bundled live capture")
    else {
        panic!("bundled Deck returned the wrong live start acknowledgement");
    };
    assert_eq!(live_started.state, CaptureState::Capturing);
    assert_eq!(live_started.mode, CaptureMode::LiveCapture);

    let Ack::DeckProcess(live_process) = session
        .client_mut()
        .call(
            Command::DeckProcess(DeckProcess {
                deck_session_id,
                deck_revision: 1,
                stream_generation: generation,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("process live post-operator boundary")
    else {
        panic!("bundled Deck returned the wrong live process acknowledgement");
    };
    assert_eq!(live_process.status.capture_state, CaptureState::Capturing);
    consume_capture_ring(session, generation, 3);
    let Ack::CaptureStatus(live_active) = session
        .client_mut()
        .call(
            Command::CaptureStatus(CaptureIdentity {
                deck_session_id,
                deck_revision: 1,
                capture_id: live_id,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("read active live capture status")
    else {
        panic!("bundled Deck returned the wrong live status acknowledgement");
    };
    assert_eq!(live_active.state, CaptureState::Capturing);
    assert_eq!(live_active.latent_slots, 1);
    assert!(live_active.artifact.is_none());

    let Ack::CaptureStop(live) = session
        .client_mut()
        .call(
            Command::CaptureStop(CaptureIdentity {
                deck_session_id,
                deck_revision: 1,
                capture_id: live_id,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("stop and finalize bundled live capture")
    else {
        panic!("bundled Deck returned the wrong live stop acknowledgement");
    };
    assert_capture_receipt(&live, &live_root, CaptureMode::LiveCapture, 1, 1_024);
}

fn consume_capture_ring(
    session: &mut latentdeck_core::deck_session_v2::DeckSessionV2,
    generation: u64,
    sequence: u64,
) {
    assert_eq!(
        session
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("capture ABI2 ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(batch) = session
        .ring_consumer_mut()
        .try_read()
        .expect("read capture ABI2 batch")
    else {
        panic!("capture process must publish one ABI2 batch");
    };
    assert_eq!(batch.metadata().generation(), generation);
    assert_eq!(batch.metadata().logical_sequence(), sequence);
}

fn assert_capture_receipt(
    status: &CaptureStatusSnapshot,
    staging_root: &Path,
    mode: CaptureMode,
    latent_slots: u64,
    maximum_visual_bytes: u64,
) {
    assert_eq!(status.state, CaptureState::Completed);
    assert_eq!(status.mode, mode);
    assert_eq!(status.latent_slots, latent_slots);
    assert_eq!(status.reset_events, 0);
    let artifact = status
        .artifact
        .as_ref()
        .expect("completed capture artifact");
    assert_eq!(artifact.latent_slots, latent_slots);
    assert_eq!(artifact.decoded_frame_count, latent_slots * 24);
    assert!(artifact.payload_byte_length > 0);
    assert!(artifact.payload_byte_length <= maximum_visual_bytes);
    let artifact_path = PathBuf::from(&artifact.staged_payload_path);
    assert!(artifact_path.is_absolute());
    assert_eq!(artifact_path.parent(), Some(staging_root));
    let payload = fs::read(&artifact_path).expect("read worker-owned staged capture artifact");
    assert_eq!(
        artifact.payload_byte_length,
        u64::try_from(payload.len()).expect("payload length")
    );
    assert_eq!(artifact.payload_sha256, sha256(&payload));
}

fn finalize_synthetic_capture(
    source: &CartridgeFixture,
    artifact: &CaptureArtifact,
    output: &Path,
) {
    let source_cartridge = open_integrity_validated(&source.path, &ValidationOptions::default())
        .expect("source cartridge remains integrity-valid");
    let staged_path = PathBuf::from(&artifact.staged_payload_path);
    let measured = hash_path(&staged_path).expect("remeasure worker capture artifact");
    assert_eq!(measured.sha256.to_string(), artifact.payload_sha256);
    assert_eq!(measured.byte_length, artifact.payload_byte_length);

    let mut manifest = source_cartridge.manifest().clone();
    manifest.cartridge_id = CartridgeId(CAPTURED_CARTRIDGE_ID.to_owned());
    let payload = manifest
        .payloads
        .first_mut()
        .expect("one captured payload descriptor");
    payload.byte_length = artifact.payload_byte_length;
    payload.sha256 = Sha256Digest(artifact.payload_sha256.clone());
    let visual = manifest
        .tensors
        .iter_mut()
        .find(|tensor| tensor.stream == TensorStream::Visual)
        .expect("one captured visual tensor");
    visual.shape[2] = artifact.latent_slots;
    manifest.timing.decoded_video.frame_count = artifact.decoded_frame_count;
    let frame_rate = manifest.timing.decoded_video.frame_rate;
    manifest.timing.decoded_video.duration = Rational::reduced(
        artifact.decoded_frame_count * frame_rate.denominator,
        frame_rate.numerator,
    )
    .expect("captured duration");
    manifest.provenance.created_by = ProducerDescriptor {
        name: Identifier("latentdeck-synthetic-capture-test".to_owned()),
        version: APP_VERSION.to_owned(),
    };
    manifest.parent_cartridges = vec![ParentCartridge {
        cartridge_id: source_cartridge.manifest().cartridge_id.clone(),
        archive_sha256: Sha256Digest(source.archive_sha256.clone()),
        role: Identifier("source_1".to_owned()),
    }];
    manifest.operation_history = vec![OperationRecord {
        operator_id: Identifier("dev.latentdeck.synthetic.capture".to_owned()),
        operator_version: DECK_VERSION.to_owned(),
        seed: 7,
        controls: BTreeMap::from([(
            "capture_mode".to_owned(),
            serde_json::Value::String("snapshot".to_owned()),
        )]),
    }];
    let receipt = pack_profile_resample_atomic(
        &ProfileResampleRequest {
            manifest,
            expected_payload: PayloadExpectation {
                byte_length: artifact.payload_byte_length,
                sha256: Sha256Digest(artifact.payload_sha256.clone()),
            },
        },
        &staged_path,
        output,
        &WriteOptions::default(),
    )
    .expect("Core finalizes and reopens the captured payload");
    assert!(receipt.spool_removed, "finalized worker spool is consumed");
}

fn codec_reference() -> PackageReference {
    PackageReference {
        kind: PackageKind::CodecPack,
        package_id: CODEC_ID.to_owned(),
        package_version: CODEC_VERSION.to_owned(),
    }
}

fn player_host_contract() -> PlayerSessionV2HostContract {
    PlayerSessionV2HostContract {
        app_version: APP_VERSION.to_owned(),
        player_session_id: Uuid::new_v4(),
        ring_id: Uuid::new_v4(),
        profile_key: protocol_profile(),
        signal_geometry: protocol_signal(),
        tensor_abi: tensor_abi(),
        decoded_abi: DecodedAbi {
            pixel_format: "rgba8".to_owned(),
            maximum_batch: 24,
        },
        maximum_estimated_host_bytes: 1_024,
        maximum_estimated_device_bytes: 1_024,
        device_ordinal: 0,
        ring_slot_count: 3,
        stream_generation: 1,
        loop_enabled: false,
        heartbeat_interval_ms: 250,
        heartbeat_hard_timeout_ms: 10_000,
        command_timeout: Duration::from_secs(10),
    }
}

fn assert_refused(
    roots: &ExtensionRoots,
    worker_marker: &Path,
    deck_id: &str,
    sources: &[DeckSourceSelectionV2<'_>],
    expected: DeckSelectionV2Error,
) {
    let error = prepare_exact_deck_selection(roots, &selection(deck_id), sources, APP_VERSION)
        .err()
        .expect("incompatible pair must be rejected");
    assert_eq!(error, expected);
    assert_eq!(error.code(), expected.code());
    assert!(
        !worker_marker.exists(),
        "{deck_id} must be refused before any Protocol 2 spawn attempt or Protocol 1 fallback"
    );
}

fn selection(deck_id: &str) -> DeckPackageSelectionV2 {
    DeckPackageSelectionV2::new(
        deck_id.to_owned(),
        DECK_VERSION.to_owned(),
        CODEC_ID.to_owned(),
        CODEC_VERSION.to_owned(),
        latentdeck_control::v2::DeviceKind::Cuda,
    )
}

fn load_request(source_count: u8) -> DeckSessionV2LoadRequest {
    DeckSessionV2LoadRequest {
        roles: (1..=source_count)
            .map(|slot| RoleBinding {
                role: format!("source_{slot}"),
                physical_slot: slot,
            })
            .collect(),
        controls: Vec::new(),
        source_transport: (1..=source_count)
            .map(|slot| SourceTransportBinding {
                physical_slot: slot,
                playing: true,
                loop_enabled: true,
            })
            .collect(),
        seed: 0x5eed,
    }
}

fn bundled_load_request(deck_id: &str) -> DeckSessionV2LoadRequest {
    let roles = match deck_id {
        BUNDLED_D2_ID => vec![
            RoleBinding {
                role: "carrier".to_owned(),
                physical_slot: 1,
            },
            RoleBinding {
                role: "donor".to_owned(),
                physical_slot: 2,
            },
        ],
        BUNDLED_Q4_ID => vec![
            RoleBinding {
                role: "carrier".to_owned(),
                physical_slot: 1,
            },
            RoleBinding {
                role: "donor_b".to_owned(),
                physical_slot: 2,
            },
            RoleBinding {
                role: "donor_c".to_owned(),
                physical_slot: 3,
            },
            RoleBinding {
                role: "donor_d".to_owned(),
                physical_slot: 4,
            },
        ],
        _ => panic!("unsupported bundled Deck test identity"),
    };
    let source_count = u8::try_from(roles.len()).expect("bounded bundled source count");
    DeckSessionV2LoadRequest {
        roles,
        controls: Vec::new(),
        source_transport: (1..=source_count)
            .map(|physical_slot| SourceTransportBinding {
                physical_slot,
                playing: true,
                loop_enabled: true,
            })
            .collect(),
        seed: 0x5eed,
    }
}

struct CartridgeFixture {
    path: PathBuf,
    cartridge_id: String,
    archive_sha256: String,
}

fn repeated_sources(cartridge: &CartridgeFixture, count: usize) -> Vec<DeckSourceSelectionV2<'_>> {
    (0..count)
        .map(|_| DeckSourceSelectionV2 {
            path: &cartridge.path,
            cartridge_id: &cartridge.cartridge_id,
            archive_sha256: &cartridge.archive_sha256,
            validated_cartridge: None,
        })
        .collect()
}

fn repeated_retained_sources<'a>(
    cartridge: &'a CartridgeFixture,
    validated: &'a latentdeck_cartridge::reader::IntegrityValidatedCartridge,
    count: usize,
) -> Vec<DeckSourceSelectionV2<'a>> {
    (0..count)
        .map(|_| DeckSourceSelectionV2 {
            path: &cartridge.path,
            cartridge_id: &cartridge.cartridge_id,
            archive_sha256: &cartridge.archive_sha256,
            validated_cartridge: Some(validated),
        })
        .collect()
}

fn write_synthetic_cartridge(root: &Path) -> CartridgeFixture {
    let payload = synthetic_payload();
    let measured = hash_reader(&mut Cursor::new(&payload)).expect("measure synthetic payload");
    let manifest_json = serde_json::json!({
        "spec_version": "0.1.0",
        "cartridge_id": CARTRIDGE_ID,
        "codec": {
            "family": PROFILE_FAMILY,
            "profile": PROFILE_NAME,
            "profile_version": PROFILE_VERSION
        },
        "payloads": [{
            "path": "payloads/synthetic.safetensors",
            "media_type": "application/vnd.safetensors",
            "byte_length": measured.byte_length,
            "sha256": measured.sha256.to_string()
        }],
        "tensors": [{
            "stream": "visual",
            "name": "latent_state",
            "payload": "payloads/synthetic.safetensors",
            "storage_dtype": "F16",
            "runtime_dtype": "F16",
            "shape": [1, 24, 2, 30, 45]
        }],
        "timing": {
            "contract": TIMING_CONTRACT,
            "contract_version": PROFILE_VERSION,
            "decoded_video": {
                "width": 3,
                "height": 1,
                "frame_count": 48,
                "frame_rate": {"numerator": 24, "denominator": 1},
                "duration": {"numerator": 2, "denominator": 1}
            }
        },
        "audio": {"policy": "source_absent"},
        "provenance": {
            "created_by": {"name": "latentdeck-core-test", "version": APP_VERSION},
            "sources": []
        },
        "parent_cartridges": [],
        "operation_history": []
    });
    let manifest: ManifestV0_1 = parse_manifest_json(
        &serde_json::to_vec(&manifest_json).expect("manifest JSON"),
        &ValidationLimits::default(),
    )
    .expect("synthetic codec-neutral manifest");
    let payload_path = root.join("synthetic.safetensors");
    let cartridge_path = root.join("synthetic.lc");
    fs::write(&payload_path, payload).expect("write synthetic payload");
    let receipt = pack_integrity_atomic(
        &CartridgePackRequest::new(manifest, payload_path),
        &cartridge_path,
        &WriteOptions::default(),
    )
    .expect("pack synthetic LC");
    CartridgeFixture {
        path: cartridge_path,
        cartridge_id: CARTRIDGE_ID.to_owned(),
        archive_sha256: receipt.validation.archive_sha256.to_string(),
    }
}

fn synthetic_payload() -> Vec<u8> {
    let tensor_bytes = vec![0_u8; 24 * 2 * 30 * 45 * 2];
    let mut header = format!(
        r#"{{"latent_state":{{"data_offsets":[0,{}],"dtype":"F16","shape":[1,24,2,30,45]}}}}"#,
        tensor_bytes.len()
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::with_capacity(8 + header.len() + tensor_bytes.len());
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("header length")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&tensor_bytes);
    payload
}

fn synthetic_capture_payload() -> Vec<u8> {
    let tensor_bytes = vec![0_u8; 24 * 30 * 45 * 2];
    let mut header = format!(
        r#"{{"latent_state":{{"data_offsets":[0,{}],"dtype":"F16","shape":[1,24,1,30,45]}}}}"#,
        tensor_bytes.len()
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::with_capacity(8 + header.len() + tensor_bytes.len());
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("header length")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&tensor_bytes);
    payload
}

fn install_codec(roots: &ExtensionRoots, root: &Path, current_test_exe: &Path) {
    let source = root.join("codec-source");
    fs::create_dir(&source).expect("codec source directory");
    write_file(&source, "LICENSE.txt", b"synthetic test package\n");
    write_file(
        &source,
        "runtime/adapter.py",
        b"def descriptor():\n    return {'synthetic': True}\n",
    );
    let lock = b"python==3.13\ntorch==2.13.0+cu130\n";
    write_file(&source, "runtime/runtime.lock", lock);
    let executable = source.join("runtime/synthetic-worker.exe");
    fs::copy(current_test_exe, &executable).expect("copy test worker executable");

    let catalog_bytes = write_integrity(
        &source,
        &[
            "LICENSE.txt",
            "runtime/adapter.py",
            "runtime/runtime.lock",
            "runtime/synthetic-worker.exe",
        ],
    );
    let manifest = CodecPackManifest {
        manifest_version: "2.0.0".to_owned(),
        kind: PackageKind::CodecPack,
        pack_id: CODEC_ID.to_owned(),
        pack_version: CODEC_VERSION.to_owned(),
        display_name: "Synthetic non-H3 Codec".to_owned(),
        summary: "A test-only non-H3 Protocol 2 codec.".to_owned(),
        publisher: publisher(),
        license: license(),
        platform: PlatformDescriptor {
            os: OperatingSystem::Windows,
            arch: Architecture::X86_64,
        },
        compatibility: CodecCompatibility {
            app_min_inclusive: "0.1.0".to_owned(),
            app_max_exclusive: "1.0.0".to_owned(),
            worker_protocol: 2,
            codec_adapter_api: 1,
            tensor_abi: "latentdeck.tensor.v1".to_owned(),
            python: python(),
            torch_exact_build: TORCH_BUILD.to_owned(),
            lc_spec_versions: vec!["0.1.0".to_owned()],
            profiles: vec![manifest_profile()],
        },
        adapter: CodecAdapterDescriptor {
            adapter_id: ADAPTER_ID.to_owned(),
            adapter_version: ADAPTER_VERSION.to_owned(),
            entrypoint: "adapter:descriptor".to_owned(),
        },
        worker: CodecWorkerDescriptor {
            executable: "runtime/synthetic-worker.exe".to_owned(),
            arguments: ["--ignored", "--exact", WORKER_HELPER, "--nocapture"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            working_directory: "runtime".to_owned(),
            start_timeout_ms: 10_000,
            heartbeat_timeout_ms: 10_000,
        },
        capabilities: codec_capabilities(),
        external_assets: Vec::new(),
        runtime_lock: RuntimeLockDescriptor {
            path: "runtime/runtime.lock".to_owned(),
            sha256: sha256(lock),
        },
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: sha256(&catalog_bytes),
        },
    };
    write_json(&source, "codec-pack.json", &manifest);
    pack_install_enable(
        roots,
        &source,
        &root.join("synthetic.ldcodec"),
        &PackageReference {
            kind: PackageKind::CodecPack,
            package_id: CODEC_ID.to_owned(),
            package_version: CODEC_VERSION.to_owned(),
        },
    );
}

fn install_deck(roots: &ExtensionRoots, root: &Path, fixture: DeckFixture<'_>) {
    let source = root.join(format!("deck-source-{}", fixture.id.replace('.', "_")));
    fs::create_dir(&source).expect("Deck source directory");
    let roles: Vec<String> = (1..=fixture.source_count)
        .map(|slot| format!("source_{slot}"))
        .collect();
    let operator = serde_json::json!({
        "schema_version": "0.2.0",
        "deck_operator_api": "0.2.0",
        "deck_id": fixture.id,
        "deck_version": DECK_VERSION,
        "operator_id": format!("{}.operator", fixture.id),
        "operator_version": DECK_VERSION,
        "entrypoint": "synthetic_operator:process_sources",
        "source_count": fixture.source_count,
        "role_ids": roles,
        "controls": []
    });
    write_file(&source, "LICENSE.txt", b"synthetic test package\n");
    write_json(&source, "operator.json", &operator);
    write_json(
        &source,
        "faceplate.json",
        &serde_json::json!({"widgets": []}),
    );
    write_file(
        &source,
        "python/synthetic_operator.py",
        b"SYNTHETIC_EXTERNAL_LD = True\ndef process_sources(sources, controls, context):\n    return sources[0]\n",
    );
    let catalog_bytes = write_integrity(
        &source,
        &[
            "LICENSE.txt",
            "faceplate.json",
            "operator.json",
            "python/synthetic_operator.py",
        ],
    );
    let role_descriptors: Vec<DeckRoleDescriptor> = (1..=fixture.source_count)
        .map(|slot| DeckRoleDescriptor {
            role_id: format!("source_{slot}"),
            display_name: format!("Source {slot}"),
        })
        .collect();
    let manifest = DeckPackManifest {
        manifest_version: "1.0.0".to_owned(),
        kind: PackageKind::DeckPack,
        deck_id: fixture.id.to_owned(),
        deck_version: DECK_VERSION.to_owned(),
        display_name: "Synthetic external Deck".to_owned(),
        summary: "A test-only dynamically installed Deck.".to_owned(),
        publisher: publisher(),
        license: license(),
        compatibility: DeckCompatibility {
            app_min_inclusive: "0.1.0".to_owned(),
            app_max_exclusive: "1.0.0".to_owned(),
            deck_host_api: 1,
            worker_protocol: 2,
            deck_operator_api: 1,
            tensor_abi: "latentdeck.tensor.v1".to_owned(),
            python: python(),
            torch_exact_build: TORCH_BUILD.to_owned(),
        },
        runtime: DeckRuntimeDescriptor {
            kind: DeckRuntimeKind::PythonOperatorStreamV1,
            operator_descriptor_path: "operator.json".to_owned(),
            python_root: "python".to_owned(),
            entrypoint: "synthetic_operator:process_sources".to_owned(),
        },
        signal: DeckSignalDescriptor {
            slots: fixture.source_count,
            roles: role_descriptors,
            default_permutation: (1..=fixture.source_count)
                .map(|slot| format!("source_{slot}"))
                .collect(),
            structural_carrier_role: "source_1".to_owned(),
            geometry_allowlist: vec![ManifestSignalGeometry {
                dtype: fixture.tensor_dtype,
                device: fixture.tensor_device,
                batch: 1,
                channels: 24,
                temporal: 1,
                height: 30,
                width: fixture.latent_width,
            }],
            timing: TimingDescriptor {
                frames_per_second_numerator: fixture.fps_numerator,
                frames_per_second_denominator: 1,
                samples_per_slot: 24,
            },
            required_capabilities: vec![fixture.required_capability],
            profile_allowlist: Some(vec![ManifestProfileKey {
                profile: fixture.profile_name.to_owned(),
                ..manifest_profile()
            }]),
        },
        faceplate_path: "faceplate.json".to_owned(),
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: sha256(&catalog_bytes),
        },
    };
    write_json(&source, "deck-pack.json", &manifest);
    pack_install_enable(
        roots,
        &source,
        &root.join(format!("{}.ld", fixture.id.replace('.', "-"))),
        &PackageReference {
            kind: PackageKind::DeckPack,
            package_id: fixture.id.to_owned(),
            package_version: DECK_VERSION.to_owned(),
        },
    );
}

fn install_bundled_deck(roots: &ExtensionRoots, root: &Path, source: &Path, deck_id: &str) {
    let staged_source = root.join(format!("bundled-source-{}", deck_id.replace('.', "-")));
    fs::create_dir(&staged_source).expect("bundled Deck staging directory");
    let catalog: IntegrityCatalog = serde_json::from_slice(
        &fs::read(source.join("integrity.json")).expect("read bundled integrity catalog"),
    )
    .expect("parse bundled integrity catalog");
    for relative in ["deck-pack.json", "integrity.json"]
        .into_iter()
        .chain(catalog.files.iter().map(|file| file.path.as_str()))
    {
        let destination = portable_path(&staged_source, relative);
        fs::create_dir_all(destination.parent().expect("bundled file parent"))
            .expect("create bundled file parent");
        fs::copy(portable_path(source, relative), destination).expect("stage bundled package file");
    }
    let archive = root.join(format!("{}.ld", deck_id.replace('.', "-")));
    let packed = pack(&PackRequest {
        source_directory: staged_source,
        output_path: archive.clone(),
    })
    .expect("pack exact bundled Deck tree");
    let package = PackageReference {
        kind: PackageKind::DeckPack,
        package_id: deck_id.to_owned(),
        package_version: DECK_VERSION.to_owned(),
    };
    assert_eq!(packed.inspection.package, package);
    let expected_sha256 = packed.inspection.archive_sha256;
    let index = BundledPackageIndex {
        index_version: "1.0.0".to_owned(),
        packages: vec![BundledPackageEntry {
            package: package.clone(),
            archive_sha256: expected_sha256.clone(),
        }],
    };
    install_from_bundled_index(
        roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256,
        },
        &index,
    )
    .expect("build-generated exact hash index authorizes bundled Deck");
    enable(roots, &package).expect("enable exact bundled Deck version");
}

fn pack_install_enable(
    roots: &ExtensionRoots,
    source: &Path,
    archive: &Path,
    package: &PackageReference,
) {
    let packed = pack(&PackRequest {
        source_directory: source.to_path_buf(),
        output_path: archive.to_path_buf(),
    })
    .expect("pack synthetic extension");
    install(
        roots,
        &InstallRequest {
            archive_path: archive.to_path_buf(),
            expected_sha256: packed.inspection.archive_sha256,
        },
    )
    .expect("install exact synthetic extension");
    enable(roots, package).expect("enable exact synthetic extension");
}

fn write_integrity(root: &Path, paths: &[&str]) -> Vec<u8> {
    let mut files: Vec<_> = paths
        .iter()
        .map(|relative| {
            let bytes = fs::read(portable_path(root, relative)).expect("read catalogued file");
            IntegrityFile {
                path: (*relative).to_owned(),
                byte_length: u64::try_from(bytes.len()).expect("file length"),
                sha256: sha256(&bytes),
            }
        })
        .collect();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let bytes = serde_json::to_vec(&IntegrityCatalog {
        manifest_version: "1.0.0".to_owned(),
        files,
    })
    .expect("integrity JSON");
    write_file(root, "integrity.json", &bytes);
    bytes
}

fn write_json(root: &Path, relative: &str, value: &impl Serialize) {
    write_file(
        root,
        relative,
        &serde_json::to_vec(value).expect("package JSON"),
    );
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let path = portable_path(root, relative);
    fs::create_dir_all(path.parent().expect("file parent")).expect("create file parent");
    fs::write(path, bytes).expect("write package file");
}

fn portable_path(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn publisher() -> PublisherDescriptor {
    PublisherDescriptor {
        name: "Synthetic Test Publisher".to_owned(),
        url: Some("https://example.test".to_owned()),
        identity_claim: PublisherIdentityClaim::SelfDeclared,
    }
}

fn license() -> LicenseDescriptor {
    LicenseDescriptor {
        spdx_or_label: "Apache-2.0".to_owned(),
        notice_path: "LICENSE.txt".to_owned(),
    }
}

fn python() -> PythonConstraint {
    PythonConstraint {
        implementation: PythonImplementation::Cpython,
        version: "3.13".to_owned(),
        platform_tag: "win_amd64".to_owned(),
    }
}

fn manifest_profile() -> ManifestProfileKey {
    ManifestProfileKey {
        codec_family: PROFILE_FAMILY.to_owned(),
        profile: PROFILE_NAME.to_owned(),
        profile_version: PROFILE_VERSION.to_owned(),
    }
}

fn codec_capabilities() -> Vec<CodecCapability> {
    vec![
        CodecCapability::Player,
        CodecCapability::Realtime,
        CodecCapability::Resample,
        CodecCapability::SnapshotCapture,
        CodecCapability::LiveCapture,
    ]
}

fn protocol_capabilities() -> LimitedVec<Capability, MAX_CAPABILITIES> {
    LimitedVec::try_from_vec(Capability::REQUIRED_CODEC_V2.to_vec())
        .expect("bounded Protocol 2 capabilities")
}

fn protocol_profile() -> ProfileKey {
    ProfileKey {
        codec_family: PROFILE_FAMILY.to_owned(),
        profile: PROFILE_NAME.to_owned(),
        profile_version: PROFILE_VERSION.to_owned(),
    }
}

fn protocol_signal() -> ProtocolSignalGeometry {
    ProtocolSignalGeometry {
        channels: 24,
        latent_height: 30,
        latent_width: 45,
        decoded_height: 1,
        decoded_width: 3,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        timing_contract: TIMING_CONTRACT.to_owned(),
        timing_contract_version: PROFILE_VERSION.to_owned(),
    }
}

fn tensor_abi() -> TensorAbi {
    TensorAbi {
        python_major: 3,
        python_minor: 13,
        torch_version: TORCH_BUILD.to_owned(),
        dtype: ProtocolTensorDtype::Float16,
        shape: [1, 24, 1, 30, 45],
        contiguous: true,
        device: latentdeck_control::v2::DeviceKind::Cuda,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerBootstrap {
    bootstrap_version: u16,
    protocol_version: u16,
    session_id: WireUuid,
    pipe_name: String,
    auth_token: WorkerHelloAuthToken,
}

#[derive(Clone)]
struct SourceRecord {
    cartridge_id: Uuid,
    archive_sha256: String,
    payload_sha256: String,
    receipt_id: Option<Uuid>,
}

struct SyntheticCapture {
    status: CaptureStatusSnapshot,
    staging_root: PathBuf,
    maximum_latent_slots: u64,
    maximum_visual_bytes: u64,
}

struct SyntheticWorker {
    sources: HashMap<Uuid, SourceRecord>,
    ring_id: Option<Uuid>,
    ring: Option<WindowsRgbRingV2Producer>,
    player: Option<PlayerStatusSnapshot>,
    deck: Option<DeckStatusSnapshot>,
    capture: Option<SyntheticCapture>,
}

impl SyntheticWorker {
    fn new() -> Self {
        Self {
            sources: HashMap::new(),
            ring_id: None,
            ring: None,
            player: None,
            deck: None,
            capture: None,
        }
    }

    fn capture_start(&mut self, start: &CaptureStart) -> CaptureStatusSnapshot {
        let deck = self.deck.as_mut().expect("loaded Deck");
        assert_eq!(start.deck_session_id, deck.deck_session_id);
        assert_eq!(start.deck_revision, deck.deck_revision);
        let staging_root = PathBuf::from(&start.staging_root);
        assert!(staging_root.is_absolute());
        assert!(staging_root.is_dir());
        assert!(start.maximum_latent_slots > 0);
        assert!(start.maximum_visual_bytes > 0);
        assert!(start.maximum_reset_events > 0);
        if let Some(active) = &self.capture {
            assert!(matches!(
                active.status.state,
                CaptureState::Completed | CaptureState::Aborted | CaptureState::Faulted
            ));
        }
        let status = CaptureStatusSnapshot {
            deck_session_id: start.deck_session_id,
            deck_revision: start.deck_revision,
            capture_id: start.capture_id,
            state: CaptureState::Capturing,
            mode: start.mode,
            latent_slots: 0,
            reset_events: 0,
            artifact: None,
        };
        deck.capture_state = CaptureState::Capturing;
        self.capture = Some(SyntheticCapture {
            status: status.clone(),
            staging_root,
            maximum_latent_slots: start.maximum_latent_slots,
            maximum_visual_bytes: start.maximum_visual_bytes,
        });
        status
    }

    fn capture_append(&mut self, source_count: u8) {
        let Some(capture) = self.capture.as_mut() else {
            return;
        };
        if capture.status.state != CaptureState::Capturing {
            return;
        }
        assert!(capture.status.latent_slots < capture.maximum_latent_slots);
        capture.status.latent_slots += 1;
        let path = capture
            .staging_root
            .join(format!("{}.payload", capture.status.capture_id));
        let slot = capture.status.latent_slots;
        let mode = match capture.status.mode {
            CaptureMode::Snapshot => 0x51,
            CaptureMode::LiveCapture => 0x71,
        };
        if capture.maximum_visual_bytes == REPLAY_CAPTURE_MAX_BYTES {
            fs::write(&path, synthetic_capture_payload())
                .expect("write replayable staged Safetensors payload");
        } else {
            let bytes = [
                source_count,
                mode,
                u8::try_from(slot).unwrap_or(u8::MAX),
                0x2a,
            ];
            let mut output = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .expect("append staged capture payload");
            output
                .write_all(&bytes)
                .expect("write staged capture payload");
            output.flush().expect("flush staged capture payload");
        }
        let measured = fs::metadata(&path)
            .expect("measure staged capture payload")
            .len();
        assert!(measured <= capture.maximum_visual_bytes);
        if capture.status.mode == CaptureMode::Snapshot {
            Self::finish_capture(capture);
        }
        self.deck.as_mut().expect("loaded Deck").capture_state = capture.status.state;
    }

    fn capture_stop(&mut self, identity: &CaptureIdentity) -> CaptureStatusSnapshot {
        let capture = self.capture.as_mut().expect("active capture");
        assert_eq!(identity.deck_session_id, capture.status.deck_session_id);
        assert_eq!(identity.deck_revision, capture.status.deck_revision);
        assert_eq!(identity.capture_id, capture.status.capture_id);
        assert_eq!(capture.status.mode, CaptureMode::LiveCapture);
        assert_eq!(capture.status.state, CaptureState::Capturing);
        assert!(capture.status.latent_slots > 0);
        capture.status.state = CaptureState::Finalizing;
        Self::finish_capture(capture);
        self.deck.as_mut().expect("loaded Deck").capture_state = capture.status.state;
        capture.status.clone()
    }

    fn capture_status(&self, identity: &CaptureIdentity) -> CaptureStatusSnapshot {
        let capture = self.capture.as_ref().expect("active capture");
        assert_eq!(identity.deck_session_id, capture.status.deck_session_id);
        assert_eq!(identity.deck_revision, capture.status.deck_revision);
        assert_eq!(identity.capture_id, capture.status.capture_id);
        capture.status.clone()
    }

    fn finish_capture(capture: &mut SyntheticCapture) {
        let path = capture
            .staging_root
            .join(format!("{}.payload", capture.status.capture_id));
        let payload = fs::read(&path).expect("read staged capture payload");
        assert!(!payload.is_empty());
        assert!(
            u64::try_from(payload.len()).expect("payload length") <= capture.maximum_visual_bytes
        );
        capture.status.state = CaptureState::Completed;
        capture.status.artifact = Some(CaptureArtifact {
            staged_payload_path: path.to_str().expect("UTF-8 staged capture path").to_owned(),
            payload_sha256: sha256(&payload),
            payload_byte_length: u64::try_from(payload.len()).expect("payload length"),
            latent_slots: capture.status.latent_slots,
            decoded_frame_count: capture.status.latent_slots * 24,
        });
    }

    fn handle(&mut self, command: Command) -> Ack {
        match command {
            Command::SessionConfigure(configure) => {
                assert_eq!(configure.selected_protocol_version, PROTOCOL_VERSION);
                assert!(configure.requested_capabilities.as_slice().iter().all(
                    |capability| matches!(capability, Capability::Player | Capability::Realtime)
                ));
                Ack::SessionConfigure(SessionConfigured {
                    selected_protocol_version: PROTOCOL_VERSION,
                    maximum_frame_bytes: u32::try_from(MAX_FRAME_BYTES).expect("frame bound"),
                    accepted_capabilities: configure.requested_capabilities,
                })
            }
            Command::CodecDescriptor(request) => {
                assert_eq!(request.pack_id, CODEC_ID);
                assert_eq!(request.pack_version, CODEC_VERSION);
                assert_eq!(request.adapter_id, ADAPTER_ID);
                Ack::CodecDescriptor(CodecDescriptor {
                    pack_id: CODEC_ID.to_owned(),
                    pack_version: CODEC_VERSION.to_owned(),
                    adapter_id: ADAPTER_ID.to_owned(),
                    adapter_version: ADAPTER_VERSION.to_owned(),
                    host_api_version: "2.0".to_owned(),
                    capabilities: protocol_capabilities(),
                    profiles: LimitedVec::<_, MAX_PROFILES>::try_from_vec(vec![protocol_profile()])
                        .expect("bounded profiles"),
                })
            }
            Command::SourceOpen(open) => {
                let payload_sha256 =
                    if open.cartridge_id.hyphenated().to_string() == CAPTURED_CARTRIDGE_ID {
                        sha256(&synthetic_capture_payload())
                    } else {
                        synthetic_payload_sha256()
                    };
                self.sources.insert(
                    open.source_id,
                    SourceRecord {
                        cartridge_id: open.cartridge_id,
                        archive_sha256: open.archive_sha256.clone(),
                        payload_sha256,
                        receipt_id: None,
                    },
                );
                Ack::SourceOpen(SourceOpened {
                    source_id: open.source_id,
                    cartridge_id: open.cartridge_id,
                    archive_sha256: open.archive_sha256,
                })
            }
            Command::ProfileInspect(inspect) => {
                let source = self.sources.get(&inspect.source_id).expect("opened source");
                assert_eq!(inspect.cartridge_id, source.cartridge_id);
                assert_eq!(inspect.archive_sha256, source.archive_sha256);
                Ack::ProfileInspect(ProfileInspection {
                    source_id: inspect.source_id,
                    cartridge_id: source.cartridge_id,
                    archive_sha256: source.archive_sha256.clone(),
                    payload_sha256: source.payload_sha256.clone(),
                    profile_key: protocol_profile(),
                    signal_geometry: protocol_signal(),
                })
            }
            Command::ProfileValidate(validate) => {
                assert_eq!(validate.expected_profile, protocol_profile());
                assert!(validate.required_capabilities.as_slice().iter().all(
                    |capability| matches!(capability, Capability::Player | Capability::Realtime)
                ));
                let source = self
                    .sources
                    .get_mut(&validate.source_id)
                    .expect("opened source");
                let receipt_id = Uuid::new_v4();
                source.receipt_id = Some(receipt_id);
                Ack::ProfileValidate(Box::new(ProfileReceipt {
                    receipt_id,
                    cartridge_id: source.cartridge_id,
                    archive_sha256: source.archive_sha256.clone(),
                    payload_sha256: source.payload_sha256.clone(),
                    pack_id: CODEC_ID.to_owned(),
                    pack_version: CODEC_VERSION.to_owned(),
                    adapter_id: ADAPTER_ID.to_owned(),
                    adapter_version: ADAPTER_VERSION.to_owned(),
                    profile_key: protocol_profile(),
                    signal_geometry: protocol_signal(),
                    tensor_abi: tensor_abi(),
                    decoded_abi: DecodedAbi {
                        pixel_format: "rgba8".to_owned(),
                        maximum_batch: 24,
                    },
                    capabilities: protocol_capabilities(),
                    estimated_host_bytes: 168,
                    estimated_device_bytes: 168,
                }))
            }
            Command::CodecLoad(load) => {
                assert!(load.external_assets.is_empty());
                assert_eq!(load.device, latentdeck_control::v2::DeviceKind::Cuda);
                assert_eq!(load.device_ordinal, 0);
                Ack::CodecLoad(CodecLoaded {
                    pack_id: load.pack_id,
                    pack_version: load.pack_version,
                    adapter_id: load.adapter_id,
                    adapter_version: load.adapter_version,
                    device: load.device,
                    device_ordinal: load.device_ordinal,
                })
            }
            Command::RingConfigure(configure) => {
                assert_eq!(configure.kind, RingKind::DecodedRgba);
                let mapping_bytes =
                    control_mapping_bytes(u32::from(configure.slot_count), configure.slot_bytes)
                        .expect("ring control geometry");
                let producer =
                    WindowsRgbRingV2Producer::open_from_owned_handles_discovered_generation(
                        owned_handle(configure.mapping_handle),
                        owned_handle(configure.ready_event_handle),
                        owned_handle(configure.consumed_event_handle),
                        mapping_bytes,
                    )
                    .expect("open target-owned ABI2 handles");
                self.ring_id = Some(configure.ring_id);
                self.ring = Some(producer);
                Ack::RingConfigure(RingConfigured {
                    ring_id: configure.ring_id,
                    kind: configure.kind,
                    slot_count: configure.slot_count,
                    slot_bytes: configure.slot_bytes,
                })
            }
            Command::PlayerOpen(open) => {
                let source = self
                    .sources
                    .get(&open.source.source_id)
                    .expect("opened Player source");
                assert_eq!(open.source.cartridge_id, source.cartridge_id);
                assert_eq!(open.source.archive_sha256, source.archive_sha256);
                assert_eq!(Some(open.source.profile_receipt_id), source.receipt_id);
                let status = PlayerStatusSnapshot {
                    player_session_id: open.player_session_id,
                    state: PlayerState::Ready,
                    stream_generation: open.stream_generation,
                    stream_sequence: 0,
                    playhead_slot: 0,
                    end_of_stream: false,
                    decoded_ring_id: self.ring_id,
                };
                self.player = Some(status.clone());
                Ack::PlayerOpen(status)
            }
            Command::PlayerStep(step) => {
                let status = self.player.as_mut().expect("opened Player");
                assert_eq!(step.player_session_id, status.player_session_id);
                assert_eq!(step.stream_generation, status.stream_generation);
                assert!(step.maximum_decoded_frames > 0);
                status.state = PlayerState::Playing;
                status.stream_sequence += 1;
                status.playhead_slot += 1;
                let WriteV2Status::Written(metadata) = self
                    .ring
                    .as_mut()
                    .expect("configured Player ABI2 ring")
                    .try_write_batch(
                        *status.player_session_id.as_bytes(),
                        status.stream_sequence,
                        1,
                        3,
                        1,
                        &[0xc1; 12],
                    )
                    .expect("publish captured replay RGBA")
                else {
                    panic!("empty Player ring unexpectedly reported backpressure");
                };
                Ack::PlayerStep(PlayerStepAck {
                    status: status.clone(),
                    output_ring_id: self.ring_id,
                    output_slot_sequence: metadata.slot_sequence(),
                    decoded_frames: 1,
                })
            }
            Command::DeckLoad(load) => {
                assert!(matches!(
                    load.deck_id.as_str(),
                    COMPATIBLE_DECK_ID | COMPATIBLE_DECK4_ID | BUNDLED_D2_ID | BUNDLED_Q4_ID
                ));
                assert_eq!(load.deck_version, DECK_VERSION);
                let runtime = load.runtime.as_ref().expect("dynamic Deck runtime binding");
                assert_eq!(runtime.deck_id, load.deck_id);
                assert_eq!(runtime.deck_version, load.deck_version);
                let (operator_path, expected_entrypoint, expected_marker) = match load
                    .deck_id
                    .as_str()
                {
                    BUNDLED_D2_ID => (
                        Path::new(&runtime.python_root).join("latentdeck_operator_d2/operator.py"),
                        "latentdeck_operator_d2.operator:process_sources",
                        "def process_sources(",
                    ),
                    BUNDLED_Q4_ID => (
                        Path::new(&runtime.python_root).join("latentdeck_operator_q4/operator.py"),
                        "latentdeck_operator_q4.operator:process_sources",
                        "def process_sources(",
                    ),
                    _ => (
                        Path::new(&runtime.python_root).join("synthetic_operator.py"),
                        "synthetic_operator:process_sources",
                        "SYNTHETIC_EXTERNAL_LD = True",
                    ),
                };
                assert_eq!(runtime.entrypoint, expected_entrypoint);
                assert!(operator_path.is_file());
                assert!(
                    fs::read_to_string(operator_path)
                        .expect("read exact installed Deck operator")
                        .contains(expected_marker)
                );
                for source in load.sources.as_slice() {
                    let opened = self.sources.get(&source.source_id).expect("opened source");
                    assert_eq!(source.cartridge_id, opened.cartridge_id);
                    assert_eq!(source.archive_sha256, opened.archive_sha256);
                    assert_eq!(Some(source.profile_receipt_id), opened.receipt_id);
                }
                let transport = load
                    .sources
                    .as_slice()
                    .iter()
                    .map(|source| SourceTransportBinding {
                        physical_slot: source.physical_slot,
                        playing: false,
                        loop_enabled: source.loop_enabled,
                    })
                    .collect();
                let playheads = load
                    .sources
                    .as_slice()
                    .iter()
                    .map(|source| PlayheadSnapshot {
                        physical_slot: source.physical_slot,
                        latent_slot: 0,
                        loop_enabled: source.loop_enabled,
                        end_of_stream: false,
                    })
                    .collect();
                let status = DeckStatusSnapshot {
                    deck_session_id: load.deck_session_id,
                    state: DeckState::Ready,
                    deck_revision: 1,
                    stream_generation: load.stream_generation,
                    stream_sequence: 0,
                    playheads: LimitedVec::<_, MAX_SOURCES>::try_from_vec(playheads)
                        .expect("bounded playheads"),
                    roles: load.roles,
                    controls: load.controls,
                    source_transport: LimitedVec::<_, MAX_SOURCES>::try_from_vec(transport)
                        .expect("bounded transport"),
                    seed: load.seed,
                    capture_state: CaptureState::Idle,
                };
                self.deck = Some(status.clone());
                Ack::DeckLoad(Box::new(status))
            }
            Command::DeckTransportSet(set) => {
                let status = self.deck.as_mut().expect("loaded Deck");
                assert_eq!(set.deck_session_id, status.deck_session_id);
                assert_eq!(set.deck_revision, status.deck_revision);
                status.source_transport = set.sources;
                Ack::DeckTransportSet(Box::new(status.clone()))
            }
            Command::DeckProcess(process) => {
                let (deck_session_id, stream_sequence, source_count) = {
                    let status = self.deck.as_mut().expect("loaded Deck");
                    assert_eq!(process.deck_session_id, status.deck_session_id);
                    assert_eq!(process.deck_revision, status.deck_revision);
                    assert_eq!(process.stream_generation, status.stream_generation);
                    status.state = DeckState::Playing;
                    status.stream_sequence += 1;
                    let mut playheads = status.playheads.as_slice().to_vec();
                    for playhead in &mut playheads {
                        playhead.latent_slot += 1;
                    }
                    status.playheads = LimitedVec::<_, MAX_SOURCES>::try_from_vec(playheads)
                        .expect("bounded processed playheads");
                    (
                        status.deck_session_id,
                        status.stream_sequence,
                        u8::try_from(status.playheads.len()).expect("source count"),
                    )
                };
                // The capture payload is appended from the post-operator latent
                // boundary before this synthetic decode publishes RGBA.
                self.capture_append(source_count);
                let marker = 0xa0 + source_count;
                let WriteV2Status::Written(metadata) = self
                    .ring
                    .as_mut()
                    .expect("configured ABI2 ring")
                    .try_write_batch(
                        *deck_session_id.as_bytes(),
                        stream_sequence,
                        1,
                        3,
                        1,
                        &[marker; 12],
                    )
                    .expect("publish synthetic RGBA")
                else {
                    panic!("empty test ring unexpectedly reported backpressure");
                };
                let status = self.deck.as_ref().expect("loaded Deck").clone();
                Ack::DeckProcess(Box::new(DeckProcessAck {
                    status,
                    output_ring_id: self.ring_id.expect("configured ring ID"),
                    output_slot_sequence: metadata.slot_sequence(),
                    provenance: LimitedVec::<_, MAX_CONTROLS>::try_from_vec(vec![
                        ProvenanceEntry {
                            key: "source_count".to_owned(),
                            value: ControlValue::Integer(i64::from(source_count)),
                        },
                    ])
                    .expect("bounded provenance"),
                }))
            }
            Command::CaptureStart(start) => Ack::CaptureStart(Box::new(self.capture_start(&start))),
            Command::CaptureStop(identity) => {
                Ack::CaptureStop(Box::new(self.capture_stop(&identity)))
            }
            Command::CaptureStatus(identity) => {
                Ack::CaptureStatus(Box::new(self.capture_status(&identity)))
            }
            Command::DeckReset(reset) => {
                if self.capture.as_ref().is_some_and(|capture| {
                    matches!(
                        capture.status.state,
                        CaptureState::Completed | CaptureState::Aborted | CaptureState::Faulted
                    )
                }) {
                    self.capture = None;
                }
                let status = self.deck.as_mut().expect("loaded Deck");
                assert_eq!(reset.deck_session_id, status.deck_session_id);
                assert_eq!(reset.deck_revision, status.deck_revision);
                self.ring
                    .as_mut()
                    .expect("configured ABI2 ring")
                    .set_generation(reset.new_stream_generation)
                    .expect("strict worker ring reset");
                status.state = DeckState::Ready;
                status.capture_state = CaptureState::Idle;
                status.stream_generation = reset.new_stream_generation;
                status.stream_sequence = 0;
                if !reset.preserve_playheads {
                    let mut playheads = status.playheads.as_slice().to_vec();
                    for playhead in &mut playheads {
                        playhead.latent_slot = 0;
                        playhead.end_of_stream = false;
                    }
                    status.playheads = LimitedVec::<_, MAX_SOURCES>::try_from_vec(playheads)
                        .expect("bounded reset playheads");
                }
                Ack::DeckReset(Box::new(status.clone()))
            }
            Command::DeckStatus(_) => {
                Ack::DeckStatus(Box::new(self.deck.as_ref().expect("loaded Deck").clone()))
            }
            other => panic!("unexpected synthetic worker command: {:?}", other.name()),
        }
    }

    fn status(&self) -> StatusSnapshot {
        let deck = self
            .deck
            .as_ref()
            .map_or(DeckState::Empty, |status| status.state);
        let deck_session = self.deck.as_ref().map(|status| status.deck_session_id);
        let capture = self
            .capture
            .as_ref()
            .map_or(CaptureState::Idle, |capture| capture.status.state);
        StatusSnapshot {
            session: SessionState::Ready,
            codec: if self.ring.is_some() {
                CodecState::Ready
            } else {
                CodecState::Unloaded
            },
            player: self
                .player
                .as_ref()
                .map_or(PlayerState::Empty, |status| status.state),
            deck,
            capture,
            open_session_count: u8::from(self.deck.is_some()) + u8::from(self.player.is_some()),
            foreground_output_session: deck_session
                .or_else(|| self.player.as_ref().map(|status| status.player_session_id)),
            output_lease_pinned: matches!(
                capture,
                CaptureState::Starting | CaptureState::Capturing | CaptureState::Finalizing
            ),
        }
    }
}

#[test]
#[ignore = "spawned as the isolated synthetic Codec Pack Protocol 2 worker"]
fn synthetic_protocol2_worker_child() {
    mark_worker_started();
    let bootstrap = read_bootstrap();
    assert_eq!(bootstrap.bootstrap_version, PROTOCOL_VERSION);
    assert_eq!(bootstrap.protocol_version, PROTOCOL_VERSION);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("synthetic worker runtime");
    runtime.block_on(async move {
        let mut pipe = ClientOptions::new()
            .open(&bootstrap.pipe_name)
            .expect("connect synthetic worker pipe");
        let mut outbound_sequence = 1_u64;
        write_envelope(
            &mut pipe,
            &Envelope::new(
                bootstrap.session_id.as_uuid(),
                outbound_sequence,
                Uuid::new_v4(),
                1,
                Message::Event(EventMessage {
                    caused_by: None,
                    event: Event::WorkerHello(WorkerHello {
                        auth_token: bootstrap.auth_token,
                        worker_pid: std::process::id(),
                        worker_identity: "dev.latentdeck.synthetic.worker".to_owned(),
                        runtime_identity: "test-cpython-3.13-torch-cu130".to_owned(),
                        protocol_min: PROTOCOL_VERSION,
                        protocol_max: PROTOCOL_VERSION,
                    }),
                }),
            ),
        )
        .await;

        let mut worker = SyntheticWorker::new();
        loop {
            let envelope = read_envelope(&mut pipe).await;
            let Message::Command(command) = envelope.message else {
                panic!("synthetic worker accepts only commands");
            };
            outbound_sequence += 1;
            if let Command::SessionShutdown(shutdown) = command {
                write_envelope(
                    &mut pipe,
                    &Envelope::new(
                        bootstrap.session_id.as_uuid(),
                        outbound_sequence,
                        Uuid::new_v4(),
                        outbound_sequence,
                        Message::Ack(AckReply {
                            reply_to: envelope.message_id,
                            ack: Ack::SessionShutdown(ShutdownAck {
                                reason: shutdown.reason,
                            }),
                            status: StatusSnapshot {
                                session: SessionState::Stopping,
                                codec: CodecState::Unloaded,
                                player: PlayerState::Empty,
                                deck: DeckState::Empty,
                                capture: CaptureState::Idle,
                                open_session_count: 0,
                                foreground_output_session: None,
                                output_lease_pinned: false,
                            },
                        }),
                    ),
                )
                .await;
                break;
            }
            let ack = worker.handle(command);
            write_envelope(
                &mut pipe,
                &Envelope::new(
                    bootstrap.session_id.as_uuid(),
                    outbound_sequence,
                    Uuid::new_v4(),
                    outbound_sequence,
                    Message::Ack(AckReply {
                        reply_to: envelope.message_id,
                        ack,
                        status: worker.status(),
                    }),
                ),
            )
            .await;
        }
    });
}

fn mark_worker_started() {
    let executable = std::env::current_exe().expect("installed worker executable");
    let runtime = executable.parent().expect("runtime directory");
    let version = runtime.parent().expect("codec version directory");
    let package = version.parent().expect("codec package directory");
    let codecs = package.parent().expect("CodecPacks directory");
    let base = codecs.parent().expect("LatentDeck base directory");
    fs::write(base.join(WORKER_MARKER), b"protocol2\n").expect("write worker marker");
}

fn read_bootstrap() -> WorkerBootstrap {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(4096)
        .read_to_end(&mut bytes)
        .expect("read worker bootstrap");
    assert!(bytes.len() >= 5, "worker bootstrap must be framed");
    let declared = usize::try_from(u32::from_le_bytes(
        bytes[..4].try_into().expect("bootstrap prefix"),
    ))
    .expect("bootstrap length");
    assert_eq!(declared, bytes.len() - 4);
    rmp_serde::from_slice(&bytes[4..]).expect("decode worker bootstrap")
}

async fn read_envelope(pipe: &mut tokio::net::windows::named_pipe::NamedPipeClient) -> Envelope {
    let mut prefix = [0_u8; 4];
    pipe.read_exact(&mut prefix)
        .await
        .expect("read Protocol 2 frame prefix");
    let length = usize::try_from(u32::from_le_bytes(prefix)).expect("frame length");
    assert!((1..=MAX_FRAME_BYTES).contains(&length));
    let mut payload = vec![0_u8; length];
    pipe.read_exact(&mut payload)
        .await
        .expect("read Protocol 2 frame payload");
    decode_messagepack(&payload).expect("decode Protocol 2 command")
}

async fn write_envelope(
    pipe: &mut tokio::net::windows::named_pipe::NamedPipeClient,
    envelope: &Envelope,
) {
    let payload = encode_messagepack(envelope).expect("encode Protocol 2 reply");
    let length = u32::try_from(payload.len()).expect("bounded Protocol 2 reply");
    pipe.write_all(&length.to_le_bytes())
        .await
        .expect("write Protocol 2 frame prefix");
    pipe.write_all(&payload)
        .await
        .expect("write Protocol 2 frame payload");
    pipe.flush().await.expect("flush Protocol 2 reply");
}

fn owned_handle(value: u64) -> OwnedHandle {
    let address = usize::try_from(value).expect("target handle fits current process");
    assert_ne!(address, 0, "target handle must be non-null");
    // SAFETY: Core duplicated this target-valid handle into this exact child
    // process and the Protocol 2 ring transport consumes it exactly once.
    unsafe { OwnedHandle::from_raw_handle(std::ptr::without_provenance_mut(address)) }
}

fn synthetic_payload_sha256() -> String {
    sha256(&synthetic_payload())
}
