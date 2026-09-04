use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use latentdeck_extension_manager::{
    Architecture, CodecAdapterDescriptor, CodecCapability, CodecCompatibility, CodecPackManifest,
    CodecWorkerDescriptor, DeckCompatibility, DeckPackManifest, DeckRoleDescriptor,
    DeckRuntimeDescriptor, DeckRuntimeKind, DeckSignalDescriptor, IntegrityCatalog,
    IntegrityDescriptor, IntegrityFile, LicenseDescriptor, OperatingSystem, PackageKind,
    PlatformDescriptor, ProfileKey, PublisherDescriptor, PublisherIdentityClaim, PythonConstraint,
    PythonImplementation, RuntimeLockDescriptor, SignalGeometry, TensorDevice, TensorDtype,
    TimingDescriptor,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const DECK_ID: &str = "dev.latentdeck.cli.journey";
const CODEC_ID: &str = "dev.latentdeck.cli.codec";
const PACKAGE_VERSION: &str = "0.2.0";

#[derive(Clone, Copy)]
struct PackageSelector<'a> {
    kind: &'a str,
    id: &'a str,
}

const DECK: PackageSelector<'static> = PackageSelector {
    kind: "deck",
    id: DECK_ID,
};
const CODEC: PackageSelector<'static> = PackageSelector {
    kind: "codec",
    id: CODEC_ID,
};

fn canonical<T: Serialize>(value: &T) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("canonical fixture JSON")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let destination = relative
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component));
    fs::create_dir_all(destination.parent().expect("fixture file parent"))
        .expect("create fixture parent");
    fs::write(destination, bytes).expect("write fixture file");
}

fn integrity_for(files: &[(&str, &[u8])]) -> (Vec<u8>, String) {
    let mut files = files
        .iter()
        .map(|(path, bytes)| IntegrityFile {
            path: (*path).to_owned(),
            byte_length: u64::try_from(bytes.len()).expect("fixture byte length fits u64"),
            sha256: sha256(bytes),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let bytes = canonical(&IntegrityCatalog {
        manifest_version: "1.0.0".to_owned(),
        files,
    });
    let hash = sha256(&bytes);
    (bytes, hash)
}

fn publisher() -> PublisherDescriptor {
    PublisherDescriptor {
        name: "CLI Journey Publisher".to_owned(),
        url: None,
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

fn profile() -> ProfileKey {
    ProfileKey {
        codec_family: "synthetic".to_owned(),
        profile: "cli_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    }
}

fn mandatory_capabilities() -> Vec<CodecCapability> {
    vec![
        CodecCapability::Player,
        CodecCapability::Realtime,
        CodecCapability::Resample,
        CodecCapability::SnapshotCapture,
        CodecCapability::LiveCapture,
    ]
}

fn create_external_deck_source(root: &Path) {
    let license_bytes = b"CLI journey test notice\n".as_slice();
    let faceplate = canonical(&json!({"widgets": []}));
    let operator = canonical(&json!({"api": 1}));
    let python_bytes = b"def process_sources(*args):\n    return args[0]\n".as_slice();
    let files: Vec<(&str, &[u8])> = vec![
        ("LICENSE.txt", license_bytes),
        ("faceplate.json", &faceplate),
        ("operator.json", &operator),
        ("python/deck_operator.py", python_bytes),
    ];
    let (integrity, integrity_hash) = integrity_for(&files);
    let manifest = DeckPackManifest {
        manifest_version: "1.0.0".to_owned(),
        kind: PackageKind::DeckPack,
        deck_id: DECK_ID.to_owned(),
        deck_version: PACKAGE_VERSION.to_owned(),
        display_name: "CLI Journey Deck".to_owned(),
        summary: "A temporary synthetic Deck used by the CLI journey test.".to_owned(),
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
            torch_exact_build: "2.13.0+cu130".to_owned(),
        },
        runtime: DeckRuntimeDescriptor {
            kind: DeckRuntimeKind::PythonOperatorStreamV1,
            operator_descriptor_path: "operator.json".to_owned(),
            python_root: "python".to_owned(),
            entrypoint: "deck_operator:process_sources".to_owned(),
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
            geometry_allowlist: vec![SignalGeometry {
                dtype: TensorDtype::Fp16,
                device: TensorDevice::Cuda,
                batch: 1,
                channels: 16,
                temporal: 1,
                height: 30,
                width: 45,
            }],
            timing: TimingDescriptor {
                frames_per_second_numerator: 24,
                frames_per_second_denominator: 1,
                samples_per_slot: 24,
            },
            required_capabilities: mandatory_capabilities(),
            profile_allowlist: Some(vec![profile()]),
        },
        faceplate_path: "faceplate.json".to_owned(),
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: integrity_hash,
        },
    };
    for (path, bytes) in files {
        write_file(root, path, bytes);
    }
    write_file(root, "integrity.json", &integrity);
    write_file(root, "deck-pack.json", &canonical(&manifest));
}

fn create_synthetic_codec_source(root: &Path) {
    let license_bytes = b"CLI journey test notice\n".as_slice();
    let adapter_bytes = b"def load(*args):\n    return None\n".as_slice();
    let worker_bytes = b"synthetic worker".as_slice();
    let lock_bytes = b"python==3.13\ntorch==2.13.0+cu130\n".as_slice();
    let files: Vec<(&str, &[u8])> = vec![
        ("LICENSE.txt", license_bytes),
        ("runtime/adapter.py", adapter_bytes),
        ("runtime/python.exe", worker_bytes),
        ("runtime/runtime.lock", lock_bytes),
    ];
    let (integrity, integrity_hash) = integrity_for(&files);
    let manifest = CodecPackManifest {
        manifest_version: "2.0.0".to_owned(),
        kind: PackageKind::CodecPack,
        pack_id: CODEC_ID.to_owned(),
        pack_version: PACKAGE_VERSION.to_owned(),
        display_name: "CLI Journey Codec".to_owned(),
        summary: "A temporary synthetic Codec used by the CLI journey test.".to_owned(),
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
            torch_exact_build: "2.13.0+cu130".to_owned(),
            lc_spec_versions: vec!["0.1.0".to_owned()],
            profiles: vec![profile()],
        },
        adapter: CodecAdapterDescriptor {
            adapter_id: "dev.latentdeck.cli.adapter".to_owned(),
            adapter_version: PACKAGE_VERSION.to_owned(),
            entrypoint: "adapter:load".to_owned(),
        },
        worker: CodecWorkerDescriptor {
            executable: "runtime/python.exe".to_owned(),
            arguments: vec!["-m".to_owned(), "adapter".to_owned()],
            working_directory: "runtime".to_owned(),
            start_timeout_ms: 30_000,
            heartbeat_timeout_ms: 5_000,
        },
        capabilities: mandatory_capabilities(),
        external_assets: Vec::new(),
        runtime_lock: RuntimeLockDescriptor {
            path: "runtime/runtime.lock".to_owned(),
            sha256: sha256(lock_bytes),
        },
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: integrity_hash,
        },
    };
    for (path, bytes) in files {
        write_file(root, path, bytes);
    }
    write_file(root, "integrity.json", &integrity);
    write_file(root, "codec-pack.json", &canonical(&manifest));
}

fn command(local_app_data: &Path, subcommand: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_latentdeck-extension-manager"));
    command
        .arg("--local-app-data")
        .arg(local_app_data)
        .arg(subcommand);
    command
}

fn success_json(mut command: Command) -> Value {
    let output = command.output().expect("execute extension-manager CLI");
    assert_success(&output);
    serde_json::from_slice(&output.stdout).expect("CLI success emits one JSON value")
}

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "successful CLI command wrote stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty(), "successful CLI emitted no JSON");
}

fn selector(command: &mut Command, package: PackageSelector<'_>) {
    command.args([
        "--kind",
        package.kind,
        "--id",
        package.id,
        "--version",
        PACKAGE_VERSION,
    ]);
}

fn pack_and_inspect(
    local_app_data: &Path,
    source: &Path,
    archive: &Path,
    package: PackageSelector<'_>,
) -> String {
    let mut pack = command(local_app_data, "pack");
    pack.arg("--source")
        .arg(source)
        .arg("--output")
        .arg(archive);
    let packed = success_json(pack);
    let archive_sha256 = packed["inspection"]["archive_sha256"]
        .as_str()
        .expect("pack receipt archive SHA-256")
        .to_owned();
    assert_eq!(archive_sha256.len(), 64);
    assert_eq!(packed["inspection"]["package"]["package_id"], package.id);

    let mut inspect = command(local_app_data, "inspect");
    inspect
        .arg("--archive")
        .arg(archive)
        .arg("--expected-sha256")
        .arg(&archive_sha256);
    let inspected = success_json(inspect);
    assert_eq!(inspected["archive_sha256"], archive_sha256);
    assert_eq!(inspected["package"]["package_id"], package.id);
    archive_sha256
}

fn install_package(
    local_app_data: &Path,
    archive: &Path,
    archive_sha256: &str,
    package: PackageSelector<'_>,
) -> PathBuf {
    let mut install = command(local_app_data, "install");
    install
        .arg("--archive")
        .arg(archive)
        .arg("--expected-sha256")
        .arg(archive_sha256);
    let installed = success_json(install);
    assert_eq!(installed["inspection"]["package"]["package_id"], package.id);
    let destination = PathBuf::from(
        installed["destination"]
            .as_str()
            .expect("install receipt destination"),
    );
    assert!(destination.is_dir());
    destination
}

fn verify_package(local_app_data: &Path, package: PackageSelector<'_>) {
    let mut verify = command(local_app_data, "verify");
    selector(&mut verify, package);
    let verified = success_json(verify);
    assert_eq!(verified["package"]["package_id"], package.id);
}

fn set_enabled(local_app_data: &Path, package: PackageSelector<'_>, enabled: bool) {
    let mut command = command(local_app_data, if enabled { "enable" } else { "disable" });
    selector(&mut command, package);
    assert_eq!(success_json(command)["enabled"], enabled);
}

fn repair_package(
    local_app_data: &Path,
    archive: &Path,
    archive_sha256: &str,
    package: PackageSelector<'_>,
) {
    let mut repair = command(local_app_data, "repair");
    repair
        .arg("--archive")
        .arg(archive)
        .arg("--expected-sha256")
        .arg(archive_sha256);
    let repaired = success_json(repair);
    assert_eq!(repaired["inspection"]["package"]["package_id"], package.id);
    assert_eq!(repaired["inspection"]["archive_sha256"], archive_sha256);
}

fn remove_package(local_app_data: &Path, package: PackageSelector<'_>) {
    let mut remove = command(local_app_data, "remove");
    selector(&mut remove, package);
    let removed = success_json(remove);
    assert_eq!(removed["package_id"], package.id);
}

fn assert_corrupt_verify(local_app_data: &Path, package: PackageSelector<'_>) {
    let mut verify = command(local_app_data, "verify");
    selector(&mut verify, package);
    let output = verify.output().expect("verify corrupt install");
    assert_eq!(output.status.code(), Some(20));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1);
    assert!(stderr.contains("extension.integrity_failed"));
}

#[test]
fn cli_executes_both_package_lifecycles_with_json_and_stable_exit_codes() {
    let temp = TempDir::new().expect("temporary CLI journey root");
    let local_app_data = temp.path().join("LocalAppData");
    let deck_source = temp.path().join("deck-source");
    let codec_source = temp.path().join("codec-source");
    let deck_archive = temp.path().join("journey.ld");
    let codec_archive = temp.path().join("journey.ldcodec");
    create_external_deck_source(&deck_source);
    create_synthetic_codec_source(&codec_source);

    let deck_hash = pack_and_inspect(&local_app_data, &deck_source, &deck_archive, DECK);
    let codec_hash = pack_and_inspect(&local_app_data, &codec_source, &codec_archive, CODEC);

    let wrong_hash = "0".repeat(64);
    let mut rejected = command(&local_app_data, "install");
    rejected
        .arg("--archive")
        .arg(&deck_archive)
        .arg("--expected-sha256")
        .arg(&wrong_hash);
    let rejected = rejected.output().expect("execute wrong-hash install");
    assert_eq!(rejected.status.code(), Some(20));
    assert!(rejected.stdout.is_empty());
    let rejected_stderr = String::from_utf8_lossy(&rejected.stderr);
    assert_eq!(rejected_stderr.lines().count(), 1);
    assert!(rejected_stderr.contains("extension.integrity_failed"));

    let deck_destination = install_package(&local_app_data, &deck_archive, &deck_hash, DECK);
    let codec_destination = install_package(&local_app_data, &codec_archive, &codec_hash, CODEC);
    verify_package(&local_app_data, DECK);
    verify_package(&local_app_data, CODEC);

    set_enabled(&local_app_data, DECK, false);
    set_enabled(&local_app_data, CODEC, false);
    set_enabled(&local_app_data, DECK, true);
    set_enabled(&local_app_data, CODEC, true);

    let listed = success_json(command(&local_app_data, "list"));
    let listed = listed.as_array().expect("list array");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|item| {
        item["package"]["kind"] == "deck_pack" && item["package"]["package_id"] == DECK_ID
    }));
    assert!(listed.iter().any(|item| {
        item["package"]["kind"] == "codec_pack" && item["package"]["package_id"] == CODEC_ID
    }));

    let matrix = success_json(command(&local_app_data, "matrix"));
    let matrix = matrix.as_array().expect("matrix array");
    assert_eq!(matrix.len(), 1);
    assert_eq!(matrix[0]["deck"]["package_id"], DECK_ID);
    assert_eq!(matrix[0]["codec"]["package_id"], CODEC_ID);
    assert_eq!(matrix[0]["reason"], "compatible");
    assert_eq!(
        matrix[0]["compatible_profile"],
        serde_json::to_value(profile()).expect("serialize expected profile")
    );

    set_enabled(&local_app_data, DECK, false);
    set_enabled(&local_app_data, CODEC, false);
    fs::write(deck_destination.join("LICENSE.txt"), b"corrupt\n")
        .expect("corrupt installed Deck tree");
    fs::write(codec_destination.join("runtime/adapter.py"), b"corrupt\n")
        .expect("corrupt installed Codec tree");
    assert_corrupt_verify(&local_app_data, DECK);
    assert_corrupt_verify(&local_app_data, CODEC);

    repair_package(&local_app_data, &deck_archive, &deck_hash, DECK);
    repair_package(&local_app_data, &codec_archive, &codec_hash, CODEC);
    verify_package(&local_app_data, DECK);
    verify_package(&local_app_data, CODEC);

    remove_package(&local_app_data, DECK);
    remove_package(&local_app_data, CODEC);
    assert!(!deck_destination.exists());
    assert!(!codec_destination.exists());
    assert!(
        success_json(command(&local_app_data, "list"))
            .as_array()
            .expect("final list array")
            .is_empty()
    );
    assert!(
        success_json(command(&local_app_data, "matrix"))
            .as_array()
            .expect("final matrix array")
            .is_empty()
    );
}

#[test]
fn cli_scaffolds_and_builds_a_no_clobber_external_deck() {
    let temp = TempDir::new().expect("temporary CLI authoring root");
    let local_app_data = temp.path().join("LocalAppData");
    let source = temp.path().join("starter-deck");
    let archive = temp.path().join("starter-deck.ld");

    let mut scaffold = command(&local_app_data, "scaffold");
    scaffold
        .args(["--kind", "deck", "--id", "com.example.starter"])
        .arg("--version")
        .arg("0.1.0")
        .arg("--output")
        .arg(&source);
    let scaffolded = success_json(scaffold);
    assert_eq!(scaffolded["package"]["package_id"], "com.example.starter");
    assert_eq!(scaffolded["ready_to_build"], true);
    assert!(!source.join("integrity.json").exists());

    let mut build_command = command(&local_app_data, "build");
    build_command
        .arg("--source")
        .arg(&source)
        .arg("--output")
        .arg(&archive);
    let receipt = success_json(build_command);
    assert_eq!(
        receipt["inspection"]["package"]["package_id"],
        "com.example.starter"
    );
    assert!(archive.is_file());
    assert!(!source.join("integrity.json").exists());

    let mut repeated = command(&local_app_data, "build");
    repeated
        .arg("--source")
        .arg(&source)
        .arg("--output")
        .arg(&archive);
    let output = repeated.output().expect("execute repeated build");
    assert_eq!(output.status.code(), Some(30));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("extension.package_exists"));
}
