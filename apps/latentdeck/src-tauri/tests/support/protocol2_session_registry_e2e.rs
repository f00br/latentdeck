use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use latentdeck_cartridge::{
    hash::hash_reader,
    limits::ValidationLimits,
    manifest::{ManifestV0_1, parse_manifest_json},
    writer::{PackRequest as CartridgePackRequest, WriteOptions, pack_integrity_atomic},
};
use latentdeck_control::{
    WireUuid,
    v2::{
        Ack, AckReply, Capability, CaptureState, CodecDescriptor, CodecLoaded, CodecState, Command,
        DeckState, DeckStatusSnapshot, DecodedAbi, EmptyPayload, Envelope, Event, EventMessage,
        LimitedVec, MAX_CAPABILITIES, MAX_FRAME_BYTES, MAX_PROFILES, MAX_SOURCES, Message,
        PROTOCOL_VERSION, PlayerState, PlayheadSnapshot, ProfileInspection, ProfileKey,
        ProfileReceipt, RingConfigured, RingKind, RoleBinding, SessionConfigured, SessionState,
        ShutdownAck, ShutdownReason, SignalGeometry as ProtocolSignalGeometry, SourceOpened,
        SourceTransportBinding, StatusSnapshot, TensorAbi, TensorDtype as ProtocolTensorDtype,
        WorkerHello, WorkerHelloAuthToken, decode_messagepack, encode_messagepack,
    },
};
use latentdeck_core::{
    deck_selection_v2::{
        DeckPackageSelectionV2, DeckSourceSelectionV2, prepare_exact_deck_selection,
    },
    deck_session_v2::{DeckSessionV2, DeckSessionV2LoadRequest, start_deck_session_v2},
};
use latentdeck_deck_runtime_contracts::{
    BrokerError, ContractId, OutputPinKind, PackageIdentity, SessionId, WarmSession, WorkerId,
};
use latentdeck_extension_manager::{
    Architecture, CodecAdapterDescriptor, CodecCapability, CodecCompatibility, CodecPackManifest,
    CodecWorkerDescriptor, DeckCompatibility, DeckPackManifest, DeckRoleDescriptor,
    DeckRuntimeDescriptor, DeckRuntimeKind, DeckSignalDescriptor, ExtensionRoots, InstallRequest,
    IntegrityCatalog, IntegrityDescriptor, IntegrityFile, LicenseDescriptor, OperatingSystem,
    PackRequest, PackageKind, PackageReference, PlatformDescriptor,
    ProfileKey as ManifestProfileKey, PublisherDescriptor, PublisherIdentityClaim,
    PythonConstraint, PythonImplementation, RuntimeLockDescriptor,
    SignalGeometry as ManifestSignalGeometry, TensorDevice, TensorDtype as ManifestTensorDtype,
    TimingDescriptor, enable, install, pack,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::windows::named_pipe::ClientOptions,
};
use uuid::Uuid;

use super::super::GenericSessionRegistry;

const APP_VERSION: &str = "0.2.0";
const CODEC_ID: &str = "dev.latentdeck.registry-test.codec";
const CODEC_VERSION: &str = "2.0.0";
const ADAPTER_ID: &str = "dev.latentdeck.registry-test.adapter";
const ADAPTER_VERSION: &str = "2.0.0";
const DECK_ID: &str = "dev.latentdeck.registry-test.deck";
const DECK_VERSION: &str = "0.2.0";
const CARTRIDGE_ID: &str = "550e8400-e29b-41d4-a716-446655440061";
const PROFILE_FAMILY: &str = "registry_test";
const PROFILE_NAME: &str = "cpu_latent";
const PROFILE_VERSION: &str = "0.2.0";
const TIMING_CONTRACT: &str = "registry_step";
const TORCH_BUILD: &str = "2.13.0+cpu";
const WORKER_HELPER: &str =
    "generic_deck_state::tests::protocol2_session_registry_e2e::registry_protocol2_worker_child";

struct CartridgeFixture {
    path: PathBuf,
    cartridge_id: String,
    archive_sha256: String,
}

struct ManagedSession {
    session_id: SessionId,
    worker_id: WorkerId,
    runtime: DeckSessionV2,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(
    clippy::too_many_lines,
    reason = "the vertical test keeps four real worker lifecycles and broker assertions together"
)]
async fn production_registry_with_four_real_protocol2_workers_enforces_capacity_pin_and_fault_isolation()
 {
    let temp = TempDir::new().expect("temporary production-registry root");
    let roots = ExtensionRoots::for_base_root(temp.path().join("LatentDeck"));
    let current_test_exe = std::env::current_exe().expect("current LatentDeck test executable");
    install_codec(&roots, temp.path(), &current_test_exe);
    install_deck(&roots, temp.path());
    let cartridge = write_synthetic_cartridge(temp.path());
    let mut registry = GenericSessionRegistry::default();
    let mut sessions = Vec::new();
    let mut worker_pids = HashSet::new();

    for number in 1..=4 {
        let session_id = session_id(number);
        registry
            .reserve(session_id.clone())
            .expect("one of four production reservations");
        let sources = [DeckSourceSelectionV2 {
            path: &cartridge.path,
            cartridge_id: &cartridge.cartridge_id,
            archive_sha256: &cartridge.archive_sha256,
        }];
        let prepared = prepare_exact_deck_selection(
            &roots,
            &DeckPackageSelectionV2::new(
                DECK_ID.to_owned(),
                DECK_VERSION.to_owned(),
                CODEC_ID.to_owned(),
                CODEC_VERSION.to_owned(),
                latentdeck_control::v2::DeviceKind::Cpu,
            ),
            &sources,
            APP_VERSION,
        )
        .expect("exact installed CPU synthetic pair");
        let runtime = start_deck_session_v2(
            prepared.codec_package,
            prepared.deck_runtime,
            prepared.cartridges,
            prepared.host,
            prepared.external_assets,
            load_request(),
        )
        .await
        .expect("real authenticated Protocol 2 Deck worker");
        let child_pid = runtime.client().worker_pid();
        assert!(worker_pids.insert(child_pid), "worker PIDs must be unique");
        let worker_identity =
            WorkerId::new(format!("worker-{child_pid}")).expect("production worker identity");
        registry
            .commit(WarmSession {
                session_id: session_id.clone(),
                worker_id: worker_identity.clone(),
                deck: package_identity(DECK_ID, DECK_VERSION),
                codec: package_identity(CODEC_ID, CODEC_VERSION),
            })
            .expect("commit real worker to the production registry");
        sessions.push(ManagedSession {
            session_id,
            worker_id: worker_identity,
            runtime,
        });
    }

    assert_eq!(
        worker_pids.len(),
        4,
        "four distinct child processes started"
    );
    assert_eq!(
        registry.reserve(session_id(5)).expect_err("fifth refused"),
        BrokerError::SessionCapacityExceeded
    );

    registry
        .switch_foreground(&sessions[0].session_id)
        .expect("first worker owns output");
    let pin = registry
        .pin_foreground(&sessions[0].session_id, OutputPinKind::Capture)
        .expect("capture pins exact foreground worker");
    assert_eq!(
        registry
            .switch_foreground(&sessions[1].session_id)
            .expect_err("capture blocks switching"),
        BrokerError::SessionOutputLeasePinned
    );

    let killed = sessions[2]
        .runtime
        .client_mut()
        .force_kill()
        .await
        .expect("kill exactly one real worker job");
    assert!(!killed.success, "forced worker exit must not look orderly");
    let removed = registry
        .worker_fault(&sessions[2].worker_id)
        .expect("isolate exact faulted worker");
    assert_eq!(removed.session_id, sessions[2].session_id);
    assert_eq!(registry.broker.len(), 3);
    assert_eq!(registry.broker.output_pin(), Some(&pin));

    for (index, managed) in sessions.iter_mut().enumerate() {
        if index == 2 {
            continue;
        }
        let Ack::DeckStatus(status) = managed
            .runtime
            .client_mut()
            .call(Command::DeckStatus(EmptyPayload {}), Duration::from_secs(5))
            .await
            .expect("remaining real worker answers after isolated fault")
        else {
            panic!("remaining worker returned the wrong status acknowledgement");
        };
        assert_eq!(status.state, DeckState::Ready);
        assert_eq!(
            status.deck_session_id,
            managed.runtime.initial_status().deck_session_id
        );
    }

    registry
        .release_output_pin(&pin)
        .expect("release exact capture pin");
    for (index, managed) in sessions.iter_mut().enumerate() {
        if index == 2 {
            continue;
        }
        let exit = managed
            .runtime
            .client_mut()
            .request_shutdown(ShutdownReason::UserRequest, Duration::from_secs(5))
            .await
            .expect("orderly remaining worker shutdown");
        assert!(exit.success, "remaining worker exits cleanly: {exit}");
        registry
            .close(&managed.session_id)
            .expect("close exact production registry session");
    }
    assert!(registry.broker.is_empty());
}

fn session_id(number: usize) -> SessionId {
    SessionId::new(format!("real-session-{number}")).expect("session identity")
}

fn package_identity(id: &str, version: &str) -> PackageIdentity {
    PackageIdentity::new(
        ContractId::new(id).expect("package ID"),
        Version::parse(version).expect("package version"),
    )
}

fn load_request() -> DeckSessionV2LoadRequest {
    DeckSessionV2LoadRequest {
        roles: vec![RoleBinding {
            role: "source_1".to_owned(),
            physical_slot: 1,
        }],
        controls: Vec::new(),
        source_transport: vec![SourceTransportBinding {
            physical_slot: 1,
            playing: true,
            loop_enabled: true,
        }],
        seed: 0x5eed,
    }
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
            "created_by": {"name": "latentdeck-registry-test", "version": APP_VERSION},
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
    let payload_path = root.join("registry-synthetic.safetensors");
    let cartridge_path = root.join("registry-synthetic.lc");
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

fn install_codec(roots: &ExtensionRoots, root: &Path, current_test_exe: &Path) {
    let source = root.join("registry-codec-source");
    fs::create_dir(&source).expect("codec source directory");
    write_file(&source, "LICENSE.txt", b"synthetic test package\n");
    write_file(
        &source,
        "runtime/adapter.py",
        b"def descriptor():\n    return {'synthetic': True}\n",
    );
    let lock = b"python==3.13\ntorch==2.13.0+cpu\n";
    write_file(&source, "runtime/runtime.lock", lock);
    let executable = source.join("runtime/registry-worker.exe");
    fs::copy(current_test_exe, &executable).expect("copy test worker executable");
    let catalog_bytes = write_integrity(
        &source,
        &[
            "LICENSE.txt",
            "runtime/adapter.py",
            "runtime/runtime.lock",
            "runtime/registry-worker.exe",
        ],
    );
    let manifest = CodecPackManifest {
        manifest_version: "2.0.0".to_owned(),
        kind: PackageKind::CodecPack,
        pack_id: CODEC_ID.to_owned(),
        pack_version: CODEC_VERSION.to_owned(),
        display_name: "Registry synthetic CPU Codec".to_owned(),
        summary: "Test-only Protocol 2 session-registry codec.".to_owned(),
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
            executable: "runtime/registry-worker.exe".to_owned(),
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
        &root.join("registry-synthetic.ldcodec"),
        &PackageReference {
            kind: PackageKind::CodecPack,
            package_id: CODEC_ID.to_owned(),
            package_version: CODEC_VERSION.to_owned(),
        },
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed test package manifest is intentionally declared in one fixture builder"
)]
fn install_deck(roots: &ExtensionRoots, root: &Path) {
    let source = root.join("registry-deck-source");
    fs::create_dir(&source).expect("Deck source directory");
    write_file(&source, "LICENSE.txt", b"synthetic test package\n");
    write_json(
        &source,
        "operator.json",
        &serde_json::json!({
            "schema_version": "0.2.0",
            "deck_operator_api": "0.2.0",
            "deck_id": DECK_ID,
            "deck_version": DECK_VERSION,
            "operator_id": format!("{DECK_ID}.operator"),
            "operator_version": DECK_VERSION,
            "entrypoint": "registry_operator:process_sources",
            "source_count": 1,
            "role_ids": ["source_1"],
            "controls": []
        }),
    );
    write_json(
        &source,
        "faceplate.json",
        &serde_json::json!({"widgets": []}),
    );
    write_file(
        &source,
        "python/registry_operator.py",
        b"def process_sources(sources, controls, context):\n    return sources[0]\n",
    );
    let catalog_bytes = write_integrity(
        &source,
        &[
            "LICENSE.txt",
            "faceplate.json",
            "operator.json",
            "python/registry_operator.py",
        ],
    );
    let manifest = DeckPackManifest {
        manifest_version: "1.0.0".to_owned(),
        kind: PackageKind::DeckPack,
        deck_id: DECK_ID.to_owned(),
        deck_version: DECK_VERSION.to_owned(),
        display_name: "Registry synthetic Deck".to_owned(),
        summary: "Test-only one-source production-registry Deck.".to_owned(),
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
            entrypoint: "registry_operator:process_sources".to_owned(),
        },
        signal: DeckSignalDescriptor {
            slots: 1,
            roles: vec![DeckRoleDescriptor {
                role_id: "source_1".to_owned(),
                display_name: "Source 1".to_owned(),
            }],
            default_permutation: vec!["source_1".to_owned()],
            structural_carrier_role: "source_1".to_owned(),
            geometry_allowlist: vec![ManifestSignalGeometry {
                dtype: ManifestTensorDtype::Fp16,
                device: TensorDevice::Cpu,
                batch: 1,
                channels: 24,
                temporal: 1,
                height: 30,
                width: 45,
            }],
            timing: TimingDescriptor {
                frames_per_second_numerator: 24,
                frames_per_second_denominator: 1,
                samples_per_slot: 24,
            },
            required_capabilities: vec![CodecCapability::Realtime],
            profile_allowlist: Some(vec![manifest_profile()]),
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
        &root.join("registry-synthetic.ld"),
        &PackageReference {
            kind: PackageKind::DeckPack,
            package_id: DECK_ID.to_owned(),
            package_version: DECK_VERSION.to_owned(),
        },
    );
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
    hash_reader(&mut Cursor::new(bytes))
        .expect("hash bytes")
        .sha256
        .to_string()
}

fn synthetic_payload_sha256() -> String {
    sha256(&synthetic_payload())
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
        device: latentdeck_control::v2::DeviceKind::Cpu,
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
    receipt_id: Option<Uuid>,
}

struct RegistryWorker {
    sources: HashMap<Uuid, SourceRecord>,
    codec_loaded: bool,
    ring_id: Option<Uuid>,
    deck: Option<DeckStatusSnapshot>,
}

impl RegistryWorker {
    fn new() -> Self {
        Self {
            sources: HashMap::new(),
            codec_loaded: false,
            ring_id: None,
            deck: None,
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one closed match mirrors the bounded Protocol 2 startup command sequence"
    )]
    fn handle(&mut self, command: Command) -> Ack {
        match command {
            Command::SessionConfigure(configure) => {
                assert_eq!(configure.selected_protocol_version, PROTOCOL_VERSION);
                assert_eq!(
                    configure.requested_capabilities.as_slice(),
                    &[Capability::Realtime]
                );
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
                    payload_sha256: synthetic_payload_sha256(),
                    profile_key: protocol_profile(),
                    signal_geometry: protocol_signal(),
                })
            }
            Command::ProfileValidate(validate) => {
                assert_eq!(validate.expected_profile, protocol_profile());
                assert_eq!(
                    validate.required_capabilities.as_slice(),
                    &[Capability::Realtime]
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
                    payload_sha256: synthetic_payload_sha256(),
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
                    estimated_host_bytes: 1_024,
                    estimated_device_bytes: 0,
                }))
            }
            Command::CodecLoad(load) => {
                assert!(load.external_assets.is_empty());
                assert_eq!(load.device, latentdeck_control::v2::DeviceKind::Cpu);
                assert_eq!(load.device_ordinal, 0);
                self.codec_loaded = true;
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
                self.ring_id = Some(configure.ring_id);
                Ack::RingConfigure(RingConfigured {
                    ring_id: configure.ring_id,
                    kind: configure.kind,
                    slot_count: configure.slot_count,
                    slot_bytes: configure.slot_bytes,
                })
            }
            Command::DeckLoad(load) => {
                assert_eq!(load.deck_id, DECK_ID);
                assert_eq!(load.deck_version, DECK_VERSION);
                let runtime = load.runtime.as_ref().expect("dynamic Deck runtime binding");
                assert_eq!(runtime.deck_id, DECK_ID);
                assert_eq!(runtime.deck_version, DECK_VERSION);
                assert_eq!(runtime.entrypoint, "registry_operator:process_sources");
                assert!(
                    Path::new(&runtime.python_root)
                        .join("registry_operator.py")
                        .is_file()
                );
                for binding in load.sources.as_slice() {
                    let source = self
                        .sources
                        .get(&binding.source_id)
                        .expect("opened source binding");
                    assert_eq!(binding.cartridge_id, source.cartridge_id);
                    assert_eq!(binding.archive_sha256, source.archive_sha256);
                    assert_eq!(Some(binding.profile_receipt_id), source.receipt_id);
                }
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
            Command::DeckStatus(_) => {
                Ack::DeckStatus(Box::new(self.deck.as_ref().expect("loaded Deck").clone()))
            }
            Command::SessionStatus(_) => Ack::SessionStatus(self.status()),
            other => panic!("unexpected registry worker command: {:?}", other.name()),
        }
    }

    fn status(&self) -> StatusSnapshot {
        let deck = self
            .deck
            .as_ref()
            .map_or(DeckState::Empty, |status| status.state);
        StatusSnapshot {
            session: SessionState::Ready,
            codec: if self.codec_loaded {
                CodecState::Ready
            } else {
                CodecState::Unloaded
            },
            player: PlayerState::Empty,
            deck,
            capture: CaptureState::Idle,
            open_session_count: u8::from(self.deck.is_some()),
            foreground_output_session: self.deck.as_ref().map(|status| status.deck_session_id),
            output_lease_pinned: false,
        }
    }
}

#[test]
#[ignore = "spawned as one isolated synthetic Codec Pack Protocol 2 worker"]
fn registry_protocol2_worker_child() {
    let bootstrap = read_bootstrap();
    assert_eq!(bootstrap.bootstrap_version, PROTOCOL_VERSION);
    assert_eq!(bootstrap.protocol_version, PROTOCOL_VERSION);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("registry worker runtime");
    runtime.block_on(async move {
        let mut pipe = ClientOptions::new()
            .open(&bootstrap.pipe_name)
            .expect("connect registry worker pipe");
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
                        worker_identity: "dev.latentdeck.registry-test.worker".to_owned(),
                        runtime_identity: "test-cpython-3.13-torch-cpu".to_owned(),
                        protocol_min: PROTOCOL_VERSION,
                        protocol_max: PROTOCOL_VERSION,
                    }),
                }),
            ),
        )
        .await;

        let mut worker = RegistryWorker::new();
        loop {
            let envelope = read_envelope(&mut pipe).await;
            let Message::Command(command) = envelope.message else {
                panic!("registry worker accepts only commands");
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
                                capture: latentdeck_control::v2::CaptureState::Idle,
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
