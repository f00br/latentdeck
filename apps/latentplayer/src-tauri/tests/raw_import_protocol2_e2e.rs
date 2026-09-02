#![cfg(target_os = "windows")]
#![allow(dead_code)]
#![allow(
    unsafe_code,
    reason = "the test worker consumes target-process DuplicateHandle values exactly once"
)]
#![allow(
    clippy::too_many_lines,
    reason = "one test-only worker proves the complete production Raw Import and Player P2 path"
)]

#[path = "../src/conversion.rs"]
mod conversion;
#[path = "../src/player_selection_v2.rs"]
mod player_selection_v2;
#[path = "../src/raw_import_runtime.rs"]
mod raw_import_runtime;

use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read},
    os::windows::io::{FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use conversion::{ConversionCoordinator, ConversionPhase, ConversionPlanRequest, ConversionStatus};
use latentdeck_cartridge::hash::{hash_path, hash_reader};
use latentdeck_control::{
    WireUuid,
    v2::{
        Ack, AckReply, Capability, CaptureState, CodecDescriptor, CodecLoaded, CodecState, Command,
        DeckState, DecodedAbi, DeviceKind, Envelope, Event, EventMessage, LimitedVec,
        MAX_CAPABILITIES, MAX_FRAME_BYTES, MAX_PROFILES, Message, PROTOCOL_VERSION, PlayerState,
        PlayerStatusSnapshot, PlayerStep, PlayerStepAck, ProfileInspection, ProfileKey,
        ProfileReceipt, RawImportAborted, RawImportArtifact, RawImportAudioPolicy,
        RawImportMetadata, RawImportPreflight, RawImportStorageDtype, RawImportTensor,
        RawImportTensorStream, RingConfigured, RingKind, SessionConfigured, SessionState,
        ShutdownAck, ShutdownReason, SignalGeometry, SourceOpened, StatusSnapshot, TensorAbi,
        TensorDtype, WorkerHello, WorkerHelloAuthToken, decode_messagepack, encode_messagepack,
    },
};
use latentdeck_core::{
    player::{CodecState as PlayerCodecState, PlayerCoordinator},
    player_session_v2::start_player_session_v2_with_retained_assets,
};
use latentdeck_extension_manager::{
    ActivePackageCache, Architecture, CodecAdapterDescriptor, CodecCapability, CodecCompatibility,
    CodecPackManifest, CodecWorkerDescriptor, ExtensionRoots, ExternalAssetDescriptor,
    InstallRequest, IntegrityCatalog, IntegrityDescriptor, IntegrityFile, LicenseDescriptor,
    OperatingSystem, PackRequest, PackageKind, PackageReference, PlatformDescriptor,
    ProfileKey as ManifestProfileKey, PublisherDescriptor, PublisherIdentityClaim,
    PythonConstraint, PythonImplementation, RuntimeLockDescriptor, enable, install, pack,
};
use latentdeck_gpu::{
    ring_v2::{ReadV2Status, WriteV2Status, control_mapping_bytes},
    windows_ring::FramesReady,
    windows_ring_v2::WindowsRgbRingV2Producer,
};
use player_selection_v2::{
    PlayerCodecSelectionV2, prepare_exact_launch, select_external_asset, validate_exact_selection,
};
use raw_import_runtime::{
    RawImportProfileView, RawImportSelectionRequest, preflight_conversion_plan,
    prepare_exact_raw_import, raw_import_options_for, run_conversion_batch,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::ClientOptions,
};
use uuid::Uuid;

const CODEC_ID: &str = "dev.latentdeck.raw-import.codec";
const CODEC_VERSION: &str = "2.0.0";
const ADAPTER_ID: &str = "dev.latentdeck.raw-import.adapter";
const ADAPTER_VERSION: &str = "2.0.0";
const PROFILE_FAMILY: &str = "synthetic_import";
const PROFILE_NAME: &str = "raw_latent";
const PROFILE_VERSION: &str = "0.2.0";
const TIMING_CONTRACT: &str = "synthetic_step";
const TORCH_BUILD: &str = "2.13.0+cu130";
const WORKER_HELPER: &str = "raw_import_protocol2_worker_child";
const DECODER_ASSET_ID: &str = "synthetic_decoder";
const DECODER_BYTES: &[u8] = b"bounded synthetic decoder asset";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_synthetic_codec_imports_and_replays_raw_source() {
    let temp = TempDir::new().expect("temporary raw import root");
    let roots = ExtensionRoots::for_base_root(temp.path().join("LatentDeck"));
    let current_test_exe = std::env::current_exe().expect("current integration test executable");
    let decoder_path = install_codec(&roots, temp.path(), &current_test_exe);
    let package = package_reference();
    let selection = selection();
    let cache = ActivePackageCache::new();
    let mut player_selection = PlayerCodecSelectionV2::new(
        CODEC_ID.to_owned(),
        CODEC_VERSION.to_owned(),
        DeviceKind::Cuda,
    );
    let missing = validate_exact_selection(&cache, &roots, &player_selection)
        .expect("first exact activation");
    assert_eq!(missing.state, PlayerCodecState::Missing);
    assert_cache_work(&cache, 0, 0, "initial exact selection");

    let ready = select_external_asset(
        &cache,
        &roots,
        &mut player_selection,
        DECODER_ASSET_ID.to_owned(),
        &decoder_path,
    )
    .expect("decoder selection validates and retains exact bytes once");
    assert_eq!(ready.state, PlayerCodecState::Ready);
    assert_cache_work(&cache, 0, 1, "decoder bind");
    assert!(
        fs::write(&decoder_path, vec![b'x'; DECODER_BYTES.len()]).is_err(),
        "selected decoder evidence must deny mutation before Player launch"
    );

    let options = raw_import_options_for(&cache, &roots, Some(&package), env!("CARGO_PKG_VERSION"))
        .expect("raw import options reuse selected package");
    assert_eq!(options.package_id, CODEC_ID);
    assert_cache_work(&cache, 0, 2, "raw import options");
    let source = temp.path().join("performance.syntheticraw");
    let output_root = temp.path().join("converted");
    fs::write(&source, b"bounded synthetic raw source").expect("raw source");
    fs::create_dir(&output_root).expect("output root");

    let prepared = prepare_exact_raw_import(
        &cache,
        &roots,
        selection.clone(),
        Some(&package),
        env!("CARGO_PKG_VERSION"),
    )
    .expect("exact installed raw-import Codec Pack");
    assert_cache_work(&cache, 0, 3, "raw import preflight launch");
    let plan = preflight_conversion_plan(
        ConversionPlanRequest {
            inputs: vec![source],
            output_directory: output_root.clone(),
            recursive: false,
        },
        selection.clone(),
        prepared,
    )
    .await
    .expect("production CPU preflight");
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].status, ConversionStatus::Ready);
    assert_eq!(
        plan.items[0]
            .metadata
            .as_ref()
            .expect("metadata")
            .latent_slots,
        1
    );

    let coordinator = Arc::new(ConversionCoordinator::from_plan(plan));
    coordinator
        .begin()
        .expect("begin production conversion queue");
    let prepared = prepare_exact_raw_import(
        &cache,
        &roots,
        selection,
        Some(&package),
        env!("CARGO_PKG_VERSION"),
    )
    .expect("exact package is checked out from the same active lease for conversion");
    assert_cache_work(&cache, 0, 4, "raw import execution launch");
    let snapshot = run_conversion_batch(
        Arc::clone(&coordinator),
        prepared,
        temp.path().join("RawImportStaging"),
    )
    .await
    .expect("production raw import batch");
    assert_eq!(snapshot.phase, ConversionPhase::Complete);
    assert_eq!(snapshot.completed, 1, "conversion snapshot: {snapshot:#?}");
    assert_eq!(snapshot.failed, 0);
    assert_eq!(snapshot.items[0].status, ConversionStatus::Complete);
    assert!(snapshot.items[0].archive_sha256.is_some());

    let output = output_root.join("performance.lc");
    let mut player_coordinator = PlayerCoordinator::without_codec();
    player_coordinator
        .open_cartridge(&output)
        .expect("Player validates and retains the finalized cartridge once");
    let source = player_coordinator
        .protocol2_source_inputs()
        .expect("retained Player source");
    assert_eq!(
        source.retained_cartridge.manifest().codec.family.0,
        PROFILE_FAMILY
    );
    let first_launch = prepare_exact_launch(
        &cache,
        &roots,
        Some(&player_selection),
        &source,
        env!("CARGO_PKG_VERSION"),
        false,
    )
    .expect("Player launch clones the retained LC and active package leases");
    assert_eq!(first_launch.latent_slot_count, 1);
    assert_eq!(
        first_launch.cartridge.receipt(),
        source.retained_cartridge.receipt()
    );
    assert_eq!(first_launch.retained_external_assets.len(), 1);
    assert_cache_work(&cache, 0, 5, "Player launch preparation");
    drop(first_launch);

    let launch = prepare_exact_launch(
        &cache,
        &roots,
        Some(&player_selection),
        &source,
        env!("CARGO_PKG_VERSION"),
        false,
    )
    .expect("Player restart clones the same retained LC and active package leases");
    assert_eq!(
        launch.cartridge.receipt(),
        source.retained_cartridge.receipt()
    );
    assert_eq!(launch.retained_external_assets.len(), 1);
    assert_cache_work(&cache, 0, 6, "Player restart preparation");
    let host = launch.host;
    let player_session_id = host.player_session_id;
    let generation = host.stream_generation;
    let mut player = start_player_session_v2_with_retained_assets(
        launch.package,
        launch.cartridge,
        host,
        launch.external_assets,
        launch.retained_external_assets,
    )
    .await
    .expect("exact Player P2 replay startup");
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
        .expect("decode imported cartridge")
    else {
        panic!("Player replay returned the wrong acknowledgement");
    };
    assert_eq!(step.status.player_session_id, player_session_id);
    assert_eq!(step.decoded_frames, 1);
    assert_eq!(
        player
            .ring_consumer_mut()
            .wait_ready(Duration::from_secs(5))
            .expect("import replay ready event"),
        FramesReady::Signaled
    );
    let ReadV2Status::Batch(batch) = player
        .ring_consumer_mut()
        .try_read()
        .expect("read imported replay frame")
    else {
        panic!("imported replay must publish one ABI2 batch");
    };
    assert_eq!(batch.pixels(), &[0xd1; 12]);
    let exit = player
        .client_mut()
        .request_shutdown(ShutdownReason::HostExit, Duration::from_secs(10))
        .await
        .expect("raw import Player shutdown");
    assert!(exit.success);
}

fn assert_cache_work(
    cache: &ActivePackageCache,
    cold_full_hash_passes: u64,
    cached_checkouts: u64,
    boundary: &str,
) {
    let stats = cache.stats();
    assert_eq!(
        stats.cold_full_hash_passes, cold_full_hash_passes,
        "{boundary} must not repeat package payload hashes"
    );
    assert_eq!(
        stats.cached_checkouts, cached_checkouts,
        "{boundary} must reuse the process-local active lease"
    );
}

fn selection() -> RawImportSelectionRequest {
    RawImportSelectionRequest {
        package_id: CODEC_ID.to_owned(),
        package_version: CODEC_VERSION.to_owned(),
        adapter_id: ADAPTER_ID.to_owned(),
        adapter_version: ADAPTER_VERSION.to_owned(),
        profile: RawImportProfileView {
            codec_family: PROFILE_FAMILY.to_owned(),
            profile: PROFILE_NAME.to_owned(),
            profile_version: PROFILE_VERSION.to_owned(),
        },
    }
}

fn package_reference() -> PackageReference {
    PackageReference {
        kind: PackageKind::CodecPack,
        package_id: CODEC_ID.to_owned(),
        package_version: CODEC_VERSION.to_owned(),
    }
}

fn install_codec(roots: &ExtensionRoots, root: &Path, current_test_exe: &Path) -> PathBuf {
    let source = root.join("raw-import-codec-source");
    let decoder_path = root.join("synthetic-decoder.bin");
    fs::write(&decoder_path, DECODER_BYTES).expect("synthetic decoder asset");
    fs::create_dir(&source).expect("codec source directory");
    write_file(&source, "LICENSE.txt", b"synthetic test package\n");
    write_file(
        &source,
        "runtime/adapter.py",
        b"def descriptor():\n    return {'synthetic_raw_import': True}\n",
    );
    let lock = b"python==3.13\ntorch==2.13.0+cu130\n";
    write_file(&source, "runtime/runtime.lock", lock);
    fs::copy(
        current_test_exe,
        source.join("runtime/synthetic-worker.exe"),
    )
    .expect("copy test worker executable");
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
        display_name: "Synthetic Raw Import Codec".to_owned(),
        summary: "A test-only Raw Import and Player Protocol 2 codec.".to_owned(),
        publisher: PublisherDescriptor {
            name: "Synthetic Test Publisher".to_owned(),
            url: Some("https://example.test".to_owned()),
            identity_claim: PublisherIdentityClaim::SelfDeclared,
        },
        license: LicenseDescriptor {
            spdx_or_label: "Apache-2.0".to_owned(),
            notice_path: "LICENSE.txt".to_owned(),
        },
        platform: PlatformDescriptor {
            os: OperatingSystem::Windows,
            arch: Architecture::X86_64,
        },
        compatibility: CodecCompatibility {
            app_min_inclusive: "0.1.0".to_owned(),
            app_max_exclusive: "1.0.0".to_owned(),
            worker_protocol: PROTOCOL_VERSION,
            codec_adapter_api: 1,
            tensor_abi: "latentdeck.tensor.v1".to_owned(),
            python: PythonConstraint {
                implementation: PythonImplementation::Cpython,
                version: "3.13".to_owned(),
                platform_tag: "win_amd64".to_owned(),
            },
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
        capabilities: vec![
            CodecCapability::Player,
            CodecCapability::Realtime,
            CodecCapability::Resample,
            CodecCapability::SnapshotCapture,
            CodecCapability::LiveCapture,
            CodecCapability::RawImport,
        ],
        external_assets: vec![ExternalAssetDescriptor {
            asset_id: DECODER_ASSET_ID.to_owned(),
            display_name: "Synthetic decoder".to_owned(),
            required: true,
            byte_length: u64::try_from(DECODER_BYTES.len()).expect("decoder length"),
            sha256: sha256(DECODER_BYTES),
            source_url: None,
            license_label: "Test-only".to_owned(),
            license_url: None,
        }],
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
    let archive = root.join("synthetic-raw-import.ldcodec");
    let packed = pack(&PackRequest {
        source_directory: source,
        output_path: archive.clone(),
    })
    .expect("pack synthetic Codec Pack");
    install(
        roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: packed.inspection.archive_sha256,
        },
    )
    .expect("install synthetic Codec Pack");
    enable(roots, &package_reference()).expect("enable exact synthetic Codec Pack");
    decoder_path
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
    hash_reader(&mut Cursor::new(bytes))
        .expect("hash bytes")
        .sha256
        .to_string()
}

fn manifest_profile() -> ManifestProfileKey {
    ManifestProfileKey {
        codec_family: PROFILE_FAMILY.to_owned(),
        profile: PROFILE_NAME.to_owned(),
        profile_version: PROFILE_VERSION.to_owned(),
    }
}

fn protocol_profile() -> ProfileKey {
    ProfileKey {
        codec_family: PROFILE_FAMILY.to_owned(),
        profile: PROFILE_NAME.to_owned(),
        profile_version: PROFILE_VERSION.to_owned(),
    }
}

fn protocol_capabilities() -> LimitedVec<Capability, MAX_CAPABILITIES> {
    let mut capabilities = Capability::REQUIRED_CODEC_V2.to_vec();
    capabilities.push(Capability::RawImport);
    LimitedVec::try_from_vec(capabilities).expect("bounded Protocol 2 capabilities")
}

fn protocol_signal() -> SignalGeometry {
    SignalGeometry {
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
        dtype: TensorDtype::Float16,
        shape: [1, 24, 1, 30, 45],
        contiguous: true,
        device: latentdeck_control::v2::DeviceKind::Cuda,
    }
}

fn raw_import_metadata() -> RawImportMetadata {
    RawImportMetadata {
        profile_key: protocol_profile(),
        payload_entry: "payloads/synthetic.safetensors".to_owned(),
        payload_media_type: "application/vnd.safetensors".to_owned(),
        tensors: LimitedVec::try_from_vec(vec![RawImportTensor {
            stream: RawImportTensorStream::Visual,
            name: "latent_state".to_owned(),
            storage_dtype: RawImportStorageDtype::F16,
            runtime_dtype: RawImportStorageDtype::F16,
            shape: LimitedVec::try_from_vec(vec![1, 24, 1, 30, 45]).expect("tensor shape"),
        }])
        .expect("raw import tensors"),
        timing_contract: TIMING_CONTRACT.to_owned(),
        timing_contract_version: PROFILE_VERSION.to_owned(),
        decoded_width: 3,
        decoded_height: 1,
        decoded_frame_count: 24,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        duration_numerator: 1,
        duration_denominator: 1,
        audio_policy: RawImportAudioPolicy::SourceAbsent,
    }
}

fn synthetic_payload() -> Vec<u8> {
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerBootstrap {
    bootstrap_version: u16,
    protocol_version: u16,
    session_id: WireUuid,
    pipe_name: String,
    auth_token: WorkerHelloAuthToken,
}

struct ImportRecord {
    preflight: RawImportPreflight,
    staged_path: Option<PathBuf>,
}

struct SourceRecord {
    cartridge_id: Uuid,
    archive_sha256: String,
    receipt_id: Option<Uuid>,
}

struct SyntheticWorker {
    imports: HashMap<Uuid, ImportRecord>,
    sources: HashMap<Uuid, SourceRecord>,
    ring_id: Option<Uuid>,
    ring: Option<WindowsRgbRingV2Producer>,
    player: Option<PlayerStatusSnapshot>,
}

impl SyntheticWorker {
    fn new() -> Self {
        Self {
            imports: HashMap::new(),
            sources: HashMap::new(),
            ring_id: None,
            ring: None,
            player: None,
        }
    }

    fn handle(&mut self, command: Command) -> Ack {
        match command {
            Command::SessionConfigure(configure) => {
                assert_eq!(configure.selected_protocol_version, PROTOCOL_VERSION);
                assert!(!configure.requested_capabilities.is_empty());
                assert!(configure.requested_capabilities.as_slice().iter().all(
                    |capability| matches!(capability, Capability::RawImport | Capability::Player)
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
            Command::RawImportPreflight(request) => {
                let path = PathBuf::from(&request.source_path);
                assert!(path.is_absolute());
                let measured = hash_path(&path).expect("measure raw source");
                assert!(measured.byte_length <= request.maximum_source_bytes);
                let preflight = RawImportPreflight {
                    receipt_id: Uuid::new_v4(),
                    import_id: request.import_id,
                    pack_id: CODEC_ID.to_owned(),
                    pack_version: CODEC_VERSION.to_owned(),
                    adapter_id: ADAPTER_ID.to_owned(),
                    adapter_version: ADAPTER_VERSION.to_owned(),
                    source_sha256: measured.sha256.to_string(),
                    source_byte_length: measured.byte_length,
                    metadata: raw_import_metadata(),
                };
                self.imports.insert(
                    request.import_id,
                    ImportRecord {
                        preflight: preflight.clone(),
                        staged_path: None,
                    },
                );
                Ack::RawImportPreflight(Box::new(preflight))
            }
            Command::RawImportStage(request) => {
                let record = self
                    .imports
                    .get_mut(&request.import_id)
                    .expect("known raw import");
                assert_eq!(request.receipt_id, record.preflight.receipt_id);
                let staging_root = PathBuf::from(&request.staging_root);
                assert!(staging_root.is_absolute());
                assert!(staging_root.is_dir());
                let staged_path = staging_root.join("synthetic.safetensors");
                fs::write(&staged_path, synthetic_payload())
                    .expect("stage synthetic Safetensors payload");
                let measured = hash_path(&staged_path).expect("measure staged payload");
                record.staged_path = Some(staged_path.clone());
                Ack::RawImportStage(RawImportArtifact {
                    receipt_id: request.receipt_id,
                    import_id: request.import_id,
                    staged_payload_path: staged_path.to_string_lossy().into_owned(),
                    payload_sha256: measured.sha256.to_string(),
                    payload_byte_length: measured.byte_length,
                })
            }
            Command::RawImportAbort(request) => {
                let record = self
                    .imports
                    .remove(&request.import_id)
                    .expect("known raw import");
                assert_eq!(request.receipt_id, record.preflight.receipt_id);
                if let Some(path) = record.staged_path {
                    let _ = fs::remove_file(path);
                }
                Ack::RawImportAbort(RawImportAborted {
                    import_id: request.import_id,
                    receipt_id: request.receipt_id,
                })
            }
            Command::SourceOpen(open) => {
                self.sources.insert(
                    open.source_id,
                    SourceRecord {
                        cartridge_id: open.cartridge_id,
                        archive_sha256: open.archive_sha256.clone(),
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
                    payload_sha256: sha256(&synthetic_payload()),
                    profile_key: protocol_profile(),
                    signal_geometry: protocol_signal(),
                })
            }
            Command::ProfileValidate(validate) => {
                assert_eq!(validate.expected_profile, protocol_profile());
                assert!(
                    validate
                        .required_capabilities
                        .as_slice()
                        .contains(&Capability::Player)
                );
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
                    payload_sha256: sha256(&synthetic_payload()),
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
                assert_eq!(load.external_assets.len(), 1);
                let decoder = &load.external_assets.as_slice()[0];
                assert_eq!(decoder.asset_id, DECODER_ASSET_ID);
                assert_eq!(decoder.sha256, sha256(DECODER_BYTES));
                assert_eq!(
                    decoder.byte_length,
                    u64::try_from(DECODER_BYTES.len()).expect("decoder length")
                );
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
                        &[0xd1; 12],
                    )
                    .expect("publish imported replay RGBA")
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
            other => panic!("unexpected synthetic worker command: {:?}", other.name()),
        }
    }

    fn status(&self) -> StatusSnapshot {
        let player = self
            .player
            .as_ref()
            .map_or(PlayerState::Empty, |status| status.state);
        StatusSnapshot {
            session: SessionState::Ready,
            codec: if self.ring.is_some() {
                CodecState::Ready
            } else {
                CodecState::Unloaded
            },
            player,
            deck: DeckState::Empty,
            capture: CaptureState::Idle,
            open_session_count: u8::from(self.player.is_some()),
            foreground_output_session: self.player.as_ref().map(|status| status.player_session_id),
            output_lease_pinned: false,
        }
    }
}

#[test]
#[ignore = "spawned as the isolated synthetic Raw Import/Player Protocol 2 worker"]
fn raw_import_protocol2_worker_child() {
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
                        worker_identity: "dev.latentdeck.raw-import.worker".to_owned(),
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

fn read_bootstrap() -> WorkerBootstrap {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(4_096)
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
