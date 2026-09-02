use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use latentdeck_extension_manager::{
    Architecture, BundledPackageEntry, BundledPackageIndex, CodecAdapterDescriptor,
    CodecCapability, CodecCompatibility, CodecPackManifest, CodecWorkerDescriptor,
    DeckCompatibility, DeckPackManifest, DeckRoleDescriptor, DeckRuntimeDescriptor,
    DeckRuntimeKind, DeckSignalDescriptor, ErrorCode, ExtensionRoots, ExternalAssetDescriptor,
    InstallRequest, IntegrityCatalog, IntegrityDescriptor, IntegrityFile, LicenseDescriptor,
    OperatingSystem, PackRequest, PackageHealth, PackageKind, PackageReference, PlatformDescriptor,
    ProfileKey, PublisherDescriptor, PublisherIdentityClaim, PythonConstraint,
    PythonImplementation, RemoveOptions, RuntimeLockDescriptor, SignalGeometry, TensorDevice,
    TensorDtype, TimingDescriptor, compatibility_matrix, disable, enable,
    enable_if_only_installed_version, inspect, install, install_from_bundled_index, list, pack,
    remove, repair, repair_from_bundled_index, resolve_active, resolve_installed, verify,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

fn canonical<T: Serialize>(value: &T) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("canonical JSON")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let path: PathBuf = relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part));
    fs::create_dir_all(path.parent().expect("file parent")).expect("create file parent");
    fs::write(path, bytes).expect("write package file");
}

fn integrity_for(files: &[(&str, &[u8])]) -> (IntegrityCatalog, Vec<u8>, String) {
    let mut described: Vec<_> = files
        .iter()
        .map(|(path, bytes)| IntegrityFile {
            path: (*path).to_owned(),
            byte_length: bytes.len() as u64,
            sha256: sha256(bytes),
        })
        .collect();
    described.sort_by(|left, right| left.path.cmp(&right.path));
    let catalog = IntegrityCatalog {
        manifest_version: "1.0.0".to_owned(),
        files: described,
    };
    let bytes = canonical(&catalog);
    let hash = sha256(&bytes);
    (catalog, bytes, hash)
}

fn publisher() -> PublisherDescriptor {
    PublisherDescriptor {
        name: "Test Publisher".to_owned(),
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

fn profile() -> ProfileKey {
    ProfileKey {
        codec_family: "synthetic".to_owned(),
        profile: "test_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    }
}

fn geometry(width: u32) -> SignalGeometry {
    SignalGeometry {
        dtype: TensorDtype::Fp16,
        device: TensorDevice::Cuda,
        batch: 1,
        channels: 16,
        temporal: 1,
        height: 30,
        width,
    }
}

fn timing() -> TimingDescriptor {
    TimingDescriptor {
        frames_per_second_numerator: 24,
        frames_per_second_denominator: 1,
        samples_per_slot: 24,
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

fn write_deck_source(root: &Path, version: &str, width: u32) {
    write_deck_source_with_id(root, "com.example.deck", version, width);
}

fn write_deck_source_with_id(root: &Path, deck_id: &str, version: &str, width: u32) {
    let license_bytes = b"test notice\n".as_slice();
    let operator = canonical(&serde_json::json!({"api":1}));
    let faceplate = canonical(&serde_json::json!({"widgets":[]}));
    let python_bytes = b"def process_sources(*args):\n    return args[0]\n".as_slice();
    let files: Vec<(&str, &[u8])> = vec![
        ("LICENSE.txt", license_bytes),
        ("faceplate.json", &faceplate),
        ("operator.json", &operator),
        ("python/deck_operator.py", python_bytes),
    ];
    let (_, integrity_bytes, integrity_hash) = integrity_for(&files);
    let manifest = DeckPackManifest {
        manifest_version: "1.0.0".to_owned(),
        kind: PackageKind::DeckPack,
        deck_id: deck_id.to_owned(),
        deck_version: version.to_owned(),
        display_name: "Test Deck".to_owned(),
        summary: "A deterministic test Deck package.".to_owned(),
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
            geometry_allowlist: vec![geometry(width)],
            timing: timing(),
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
    write_file(root, "integrity.json", &integrity_bytes);
    write_file(root, "deck-pack.json", &canonical(&manifest));
}

fn write_codec_source(root: &Path, version: &str, codec_profile: ProfileKey) {
    write_codec_source_with_worker(root, version, codec_profile, b"synthetic worker");
}

fn write_codec_source_with_worker(
    root: &Path,
    version: &str,
    codec_profile: ProfileKey,
    worker_bytes: &[u8],
) {
    let license_bytes = b"test notice\n".as_slice();
    let adapter_bytes = b"def load(*args):\n    return None\n".as_slice();
    let lock_bytes = b"python==3.13\ntorch==2.13.0+cu130\n".as_slice();
    let files: Vec<(&str, &[u8])> = vec![
        ("LICENSE.txt", license_bytes),
        ("runtime/adapter.py", adapter_bytes),
        ("runtime/python.exe", worker_bytes),
        ("runtime/runtime.lock", lock_bytes),
    ];
    let (_, integrity_bytes, integrity_hash) = integrity_for(&files);
    let manifest = CodecPackManifest {
        manifest_version: "2.0.0".to_owned(),
        kind: PackageKind::CodecPack,
        pack_id: "com.example.codec".to_owned(),
        pack_version: version.to_owned(),
        display_name: "Synthetic Codec".to_owned(),
        summary: "A deterministic synthetic Codec package.".to_owned(),
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
            profiles: vec![codec_profile],
        },
        adapter: CodecAdapterDescriptor {
            adapter_id: "com.example.adapter".to_owned(),
            adapter_version: "0.2.0".to_owned(),
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
        external_assets: Vec::<ExternalAssetDescriptor>::new(),
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
    write_file(root, "integrity.json", &integrity_bytes);
    write_file(root, "codec-pack.json", &canonical(&manifest));
}

fn pack_source(source: &Path, output: &Path) -> (String, u64) {
    let receipt = pack(&PackRequest {
        source_directory: source.to_path_buf(),
        output_path: output.to_path_buf(),
    })
    .expect("pack source");
    (
        receipt.inspection.archive_sha256,
        receipt.inspection.archive_byte_length,
    )
}

#[test]
fn deterministic_deck_and_codec_packages_pass_closed_schema_inspection() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck-source");
    let codec_source = temp.path().join("codec-source");
    fs::create_dir(&deck_source).expect("deck source");
    fs::create_dir(&codec_source).expect("codec source");
    write_deck_source(&deck_source, "0.2.0", 45);
    write_codec_source(&codec_source, "0.2.0", profile());

    let first = temp.path().join("first.ld");
    let second = temp.path().join("second.ld");
    let codec = temp.path().join("synthetic.ldcodec");
    let (first_hash, _) = pack_source(&deck_source, &first);
    let (second_hash, _) = pack_source(&deck_source, &second);
    let (codec_hash, _) = pack_source(&codec_source, &codec);

    assert_eq!(fs::read(&first).unwrap(), fs::read(&second).unwrap());
    assert_eq!(first_hash, second_hash);
    assert_eq!(
        inspect(&first, Some(&first_hash)).unwrap().package.kind,
        PackageKind::DeckPack
    );
    assert_eq!(
        inspect(&codec, Some(&codec_hash)).unwrap().package.kind,
        PackageKind::CodecPack
    );
    assert_eq!(
        inspect(&first, Some(&"0".repeat(64))).unwrap_err().code(),
        ErrorCode::IntegrityFailed
    );
    assert_eq!(
        pack(&PackRequest {
            source_directory: deck_source,
            output_path: first,
        })
        .unwrap_err()
        .code(),
        ErrorCode::PackageExists
    );
}

#[test]
fn legacy_lddeck_alias_is_rejected_for_pack_and_inspect() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck-source");
    fs::create_dir(&deck_source).expect("deck source");
    write_deck_source(&deck_source, "0.2.0", 45);

    let canonical = temp.path().join("deck.ld");
    pack_source(&deck_source, &canonical);
    let legacy_alias = temp.path().join("deck.lddeck");
    fs::copy(&canonical, &legacy_alias).expect("copy exact archive bytes to legacy alias");

    assert_eq!(
        inspect(&legacy_alias, None).unwrap_err().code(),
        ErrorCode::InvalidArguments
    );
    assert_eq!(
        pack(&PackRequest {
            source_directory: deck_source,
            output_path: temp.path().join("new.lddeck"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::InvalidArguments
    );
}

#[test]
fn reserved_namespace_requires_an_exact_build_generated_hash_index() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("bundled-deck");
    fs::create_dir(&source).unwrap();
    write_deck_source_with_id(&source, "org.latentdeck.d2", "0.2.0", 45);
    let archive = temp.path().join("d2.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::from_local_app_data(temp.path().join("LocalAppData"));
    let request = InstallRequest {
        archive_path: archive.clone(),
        expected_sha256: hash.clone(),
    };

    assert_eq!(
        install(&roots, &request).unwrap_err().code(),
        ErrorCode::PackageUntrusted
    );
    assert_eq!(
        repair(&roots, &request).unwrap_err().code(),
        ErrorCode::PackageUntrusted,
        "ordinary unprivileged repair must not authorize the reserved namespace"
    );
    let package = PackageReference {
        kind: PackageKind::DeckPack,
        package_id: "org.latentdeck.d2".to_owned(),
        package_version: "0.2.0".to_owned(),
    };
    let wrong_index = BundledPackageIndex {
        index_version: "1.0.0".to_owned(),
        packages: vec![BundledPackageEntry {
            package: package.clone(),
            archive_sha256: "0".repeat(64),
        }],
    };
    assert_eq!(
        install_from_bundled_index(&roots, &request, &wrong_index)
            .unwrap_err()
            .code(),
        ErrorCode::PackageUntrusted
    );
    let exact_index = BundledPackageIndex {
        index_version: "1.0.0".to_owned(),
        packages: vec![BundledPackageEntry {
            package,
            archive_sha256: hash,
        }],
    };
    let receipt = install_from_bundled_index(&roots, &request, &exact_index)
        .expect("exact bundled index authorizes reserved package");
    assert_eq!(
        receipt.destination,
        temp.path()
            .join("LocalAppData/LatentDeck/Decks/org.latentdeck.d2/0.2.0")
    );
    assert_eq!(
        receipt.trust_receipt_path,
        temp.path()
            .join("LocalAppData/LatentDeck/PackageTrust/decks/org.latentdeck.d2/0.2.0.json")
    );
    fs::write(
        receipt.destination.join("python/deck_operator.py"),
        b"deliberate corruption",
    )
    .expect("corrupt reserved package fixture");
    assert_eq!(
        repair(&roots, &request).unwrap_err().code(),
        ErrorCode::PackageUntrusted,
        "installed reserved packages still require build authorization for repair"
    );
    repair_from_bundled_index(&roots, &request, &exact_index)
        .expect("exact build index authorizes reserved package repair");
    verify(&roots, &receipt.inspection.package).expect("repaired reserved package verifies");
}

#[test]
fn lifecycle_is_hash_bound_immutable_side_by_side_and_explicitly_selected() {
    let temp = TempDir::new().expect("temp");
    let source_020 = temp.path().join("deck-020");
    let source_021 = temp.path().join("deck-021");
    fs::create_dir(&source_020).unwrap();
    fs::create_dir(&source_021).unwrap();
    write_deck_source(&source_020, "0.2.0", 45);
    write_deck_source(&source_021, "0.2.1", 45);
    let archive_020 = temp.path().join("deck-020.ld");
    let archive_021 = temp.path().join("deck-021.ld");
    let (hash_020, _) = pack_source(&source_020, &archive_020);
    let (hash_021, _) = pack_source(&source_021, &archive_021);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));

    let install_020 = install(
        &roots,
        &InstallRequest {
            archive_path: archive_020.clone(),
            expected_sha256: hash_020.clone(),
        },
    )
    .expect("install 0.2.0");
    install(
        &roots,
        &InstallRequest {
            archive_path: archive_021,
            expected_sha256: hash_021,
        },
    )
    .expect("install 0.2.1");
    assert_eq!(
        install(
            &roots,
            &InstallRequest {
                archive_path: archive_020,
                expected_sha256: hash_020,
            }
        )
        .unwrap_err()
        .code(),
        ErrorCode::PackageExists
    );
    assert!(install_020.destination.ends_with("com.example.deck/0.2.0"));
    assert!(
        install_020
            .trust_receipt_path
            .ends_with("decks/com.example.deck/0.2.0.json")
    );

    let package_020 = PackageReference {
        kind: PackageKind::DeckPack,
        package_id: "com.example.deck".to_owned(),
        package_version: "0.2.0".to_owned(),
    };
    let package_021 = PackageReference {
        package_version: "0.2.1".to_owned(),
        ..package_020.clone()
    };
    assert!(!verify(&roots, &package_020).unwrap().enabled);
    let resolved_disabled = resolve_installed(&roots, &package_020).expect("resolve exact version");
    assert_eq!(resolved_disabled.root(), install_020.destination);
    assert!(!resolved_disabled.trust_receipt().enabled);
    assert_eq!(resolved_disabled.manifest().reference(), package_020);
    assert_eq!(
        resolve_active(&roots, &package_020).unwrap_err().code(),
        ErrorCode::PackageDisabled
    );
    enable(&roots, &package_020).expect("enable explicit version");
    let active = resolve_active(&roots, &package_020).expect("active package lease");
    assert!(
        resolve_installed(&roots, &package_020)
            .expect("resolve enabled exact version")
            .trust_receipt()
            .enabled
    );
    assert_eq!(
        enable(&roots, &package_021).unwrap_err().code(),
        ErrorCode::LifecycleConflict
    );
    assert_eq!(
        remove(&roots, &package_020, RemoveOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::PackageActive
    );
    disable(&roots, &package_020).expect("disable first");
    assert_eq!(
        remove(&roots, &package_020, RemoveOptions::default())
            .unwrap_err()
            .code(),
        ErrorCode::PackageActive
    );
    drop(active);
    enable(&roots, &package_021).expect("enable second");
    disable(&roots, &package_021).expect("disable second");
    remove(&roots, &package_020, RemoveOptions::default()).expect("remove exact old version");
    assert_eq!(
        verify(&roots, &package_020).unwrap_err().code(),
        ErrorCode::PackageMissing
    );
    assert!(verify(&roots, &package_021).is_ok());
}

#[test]
fn atomic_first_version_enable_rejects_any_other_installed_version_without_mutation() {
    let temp = TempDir::new().expect("temp");
    let source_020 = temp.path().join("deck-020");
    let source_021 = temp.path().join("deck-021");
    fs::create_dir(&source_020).expect("create 0.2.0 source");
    fs::create_dir(&source_021).expect("create 0.2.1 source");
    write_deck_source(&source_020, "0.2.0", 45);
    write_deck_source(&source_021, "0.2.1", 45);
    let archive_020 = temp.path().join("deck-020.ld");
    let archive_021 = temp.path().join("deck-021.ld");
    let (hash_020, _) = pack_source(&source_020, &archive_020);
    let (hash_021, _) = pack_source(&source_021, &archive_021);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let package_020 = install(
        &roots,
        &InstallRequest {
            archive_path: archive_020,
            expected_sha256: hash_020,
        },
    )
    .expect("install first version")
    .inspection
    .package;

    let enabled = enable_if_only_installed_version(&roots, &package_020)
        .expect("sole installed version may become default");
    assert!(enabled.enabled);
    disable(&roots, &package_020).expect("disable first version");
    let package_021 = install(
        &roots,
        &InstallRequest {
            archive_path: archive_021,
            expected_sha256: hash_021,
        },
    )
    .expect("install alternate version")
    .inspection
    .package;

    assert_eq!(
        enable_if_only_installed_version(&roots, &package_020)
            .expect_err("alternate installed version must block implicit activation")
            .code(),
        ErrorCode::LifecycleConflict
    );
    assert!(!verify(&roots, &package_020).expect("verify 0.2.0").enabled);
    assert!(!verify(&roots, &package_021).expect("verify 0.2.1").enabled);
}

#[cfg(windows)]
#[test]
fn active_package_pins_catalogued_runtime_files_and_root_until_drop() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install");
    enable(&roots, &installed.inspection.package).expect("enable");

    let active = resolve_active(&roots, &installed.inspection.package).expect("resolve active");
    let operator = active.root().join("python/deck_operator.py");
    let faceplate = active.root().join("faceplate.json");
    let notice = active.root().join("LICENSE.txt");
    let moved_notice = active.root().join("LICENSE.moved.txt");
    let moved_root = active.root().with_extension("moved");
    let replacement = temp.path().join("replacement.py");
    fs::write(&replacement, b"replacement").expect("write replacement fixture");

    File::open(&operator).expect("active package remains readable by runtime consumers");
    assert!(
        fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&operator)
            .is_err(),
        "catalogued runtime bytes must not be writable while active"
    );
    assert!(
        fs::remove_file(&faceplate).is_err(),
        "catalogued runtime files must not be deletable while active"
    );
    assert!(
        fs::rename(&notice, &moved_notice).is_err(),
        "catalogued runtime files must not be replaceable by rename while active"
    );
    assert!(
        fs::rename(&replacement, &operator).is_err(),
        "a catalogued runtime path must reject atomic replacement while active"
    );
    assert!(
        fs::rename(active.root(), &moved_root).is_err(),
        "the pinned package root must not be replaceable while active"
    );

    drop(active);

    fs::rename(&replacement, &operator).expect("drop releases atomic replacement denial");
    fs::write(&operator, b"replacement after drop").expect("drop releases write denial");
    fs::remove_file(&faceplate).expect("drop releases delete denial");
    fs::rename(&notice, &moved_notice).expect("drop releases file rename denial");
    fs::rename(&installed.destination, &moved_root).expect("drop releases root rename denial");
}

#[cfg(windows)]
#[test]
fn active_codec_worker_path_remains_executable_while_pinned() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create source");
    let command_processor = std::env::var_os("COMSPEC").map_or_else(
        || PathBuf::from(r"C:\Windows\System32\cmd.exe"),
        PathBuf::from,
    );
    let worker_bytes = fs::read(&command_processor).expect("read Windows command processor");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &worker_bytes);
    let archive = temp.path().join("codec.ldcodec");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install");
    enable(&roots, &installed.inspection.package).expect("enable");
    let active = resolve_active(&roots, &installed.inspection.package).expect("resolve active");

    let status = std::process::Command::new(active.root().join("runtime/python.exe"))
        .args(["/d", "/c", "exit", "0"])
        .status()
        .expect("launch pinned worker path");
    assert!(
        status.success(),
        "pinned worker path must remain executable"
    );
}

#[test]
fn resolve_active_rejects_tamper_before_runtime_files_are_pinned() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install");
    enable(&roots, &installed.inspection.package).expect("enable");
    fs::write(
        installed.destination.join("python/deck_operator.py"),
        b"tampered before pin",
    )
    .expect("tamper fixture");

    assert_eq!(
        resolve_active(&roots, &installed.inspection.package)
            .unwrap_err()
            .code(),
        ErrorCode::IntegrityFailed
    );
}

#[test]
fn tamper_is_isolated_and_explicit_repair_restores_the_exact_archive() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).unwrap();
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive.clone(),
            expected_sha256: hash.clone(),
        },
    )
    .expect("install");
    fs::write(
        installed.destination.join("python/deck_operator.py"),
        b"tampered",
    )
    .unwrap();
    assert_eq!(
        verify(&roots, &installed.inspection.package)
            .unwrap_err()
            .code(),
        ErrorCode::IntegrityFailed
    );
    let summaries = list(&roots).expect("list isolates corrupt package");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].health, PackageHealth::Corrupt);
    assert_eq!(
        install(
            &roots,
            &InstallRequest {
                archive_path: archive.clone(),
                expected_sha256: hash.clone(),
            },
        )
        .expect_err("corrupt exact version is not a healthy already-installed result")
        .code(),
        ErrorCode::IntegrityFailed
    );

    repair(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("repair from exact trusted archive");
    assert!(verify(&roots, &installed.inspection.package).is_ok());
}

#[test]
fn repair_ignores_enabled_bit_from_a_mismatched_receipt() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive.clone(),
            expected_sha256: hash.clone(),
        },
    )
    .expect("install");
    enable(&roots, &installed.inspection.package).expect("enable healthy fixture");
    assert_eq!(
        repair(
            &roots,
            &InstallRequest {
                archive_path: archive.clone(),
                expected_sha256: hash.clone(),
            },
        )
        .expect_err("healthy enabled package remains protected")
        .code(),
        ErrorCode::PackageActive
    );
    disable(&roots, &installed.inspection.package).expect("disable healthy fixture");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&installed.trust_receipt_path).expect("read receipt"))
            .expect("parse receipt");
    receipt["manifest_sha256"] = serde_json::Value::String("a".repeat(64));
    receipt["enabled"] = serde_json::Value::Bool(true);
    fs::write(&installed.trust_receipt_path, canonical(&receipt)).expect("poison receipt");

    repair(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("mismatched receipt cannot forge an active repair gate");

    assert!(
        !verify(&roots, &installed.inspection.package)
            .unwrap()
            .enabled
    );
}

#[test]
fn authorized_corrupt_removal_ignores_enabled_bit_from_a_mismatched_receipt() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install");
    enable(&roots, &installed.inspection.package).expect("enable fixture");
    let active = resolve_active(&roots, &installed.inspection.package).expect("active lease");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&installed.trust_receipt_path).expect("read receipt"))
            .expect("parse receipt");
    receipt["manifest_sha256"] = serde_json::Value::String("a".repeat(64));
    receipt["enabled"] = serde_json::Value::Bool(true);
    fs::write(&installed.trust_receipt_path, canonical(&receipt)).expect("poison receipt");

    assert_eq!(
        remove(
            &roots,
            &installed.inspection.package,
            RemoveOptions {
                allow_corrupt: true,
            },
        )
        .expect_err("real usage lease remains authoritative")
        .code(),
        ErrorCode::PackageActive
    );
    drop(active);

    remove(
        &roots,
        &installed.inspection.package,
        RemoveOptions {
            allow_corrupt: true,
        },
    )
    .expect("mismatched receipt cannot forge an active removal gate");

    assert!(!installed.destination.exists());
    assert!(!installed.trust_receipt_path.exists());
}

#[cfg(windows)]
#[test]
fn trust_receipt_rejects_a_reparse_point_ancestor() {
    use std::os::windows::fs::symlink_dir;

    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install");
    let receipt_id_root = installed
        .trust_receipt_path
        .parent()
        .expect("receipt identity root")
        .to_path_buf();
    let target = temp.path().join("receipt-target");
    fs::rename(&receipt_id_root, &target).expect("move receipt identity root");
    symlink_dir(&target, &receipt_id_root).expect("create receipt ancestor reparse point");

    assert_eq!(
        verify(&roots, &installed.inspection.package)
            .expect_err("receipt ancestor reparse point must fail closed")
            .code(),
        ErrorCode::LifecycleConflict
    );
}

#[test]
fn trust_receipt_read_is_bounded_to_one_mebibyte() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-source");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install");
    fs::write(&installed.trust_receipt_path, vec![b' '; 1024 * 1024 + 1])
        .expect("write oversized receipt");

    assert_eq!(
        verify(&roots, &installed.inspection.package)
            .expect_err("oversized trust receipt must fail closed")
            .code(),
        ErrorCode::PackageUntrusted
    );
}

#[test]
fn excess_version_directories_are_isolated_without_blocking_list_or_matrix() {
    let temp = TempDir::new().expect("temp");
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let identity = roots.decks_root.join("org.example.flooded");
    for version in 0..17 {
        fs::create_dir_all(identity.join(format!("1.0.{version}"))).expect("version fixture");
    }

    let summaries = list(&roots).expect("version overflow is isolated");

    assert!(summaries.len() <= 17);
    assert!(
        summaries.iter().any(|summary| {
            summary.error_code.as_deref() == Some("extension.lifecycle_conflict")
        })
    );
    assert!(compatibility_matrix(&roots).is_ok());
}

#[test]
fn excess_package_directories_are_bounded_without_blocking_the_other_kind() {
    let temp = TempDir::new().expect("temp");
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    for package in 0..257 {
        fs::create_dir_all(
            roots
                .decks_root
                .join(format!("org.example.flooded{package:03}")),
        )
        .expect("package fixture");
    }

    let summaries = list(&roots).expect("package overflow is isolated");

    assert!(summaries.len() <= 257);
    assert!(
        summaries.iter().any(|summary| {
            summary.error_code.as_deref() == Some("extension.lifecycle_conflict")
        })
    );
    assert!(compatibility_matrix(&roots).is_ok());
}

#[test]
fn archive_preflight_rejects_traversal_case_aliases_and_symlinks() {
    let temp = TempDir::new().expect("temp");
    let traversal = raw_archive(temp.path(), "traversal.ld", &[("../escape", b"x")]);
    assert_eq!(
        inspect(&traversal.0, Some(&traversal.1))
            .unwrap_err()
            .code(),
        ErrorCode::ArchiveInvalid
    );
    assert!(!temp.path().join("escape").exists());

    let duplicate = raw_archive(
        temp.path(),
        "duplicate.ld",
        &[("A.txt", b"a"), ("a.txt", b"b")],
    );
    assert_eq!(
        inspect(&duplicate.0, Some(&duplicate.1))
            .unwrap_err()
            .code(),
        ErrorCode::ArchiveInvalid
    );

    let symlink_path = temp.path().join("symlink.ld");
    let file = File::create(&symlink_path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    writer
        .add_symlink("runtime/link", "../outside", SimpleFileOptions::default())
        .unwrap();
    writer.finish().unwrap();
    let bytes = fs::read(&symlink_path).unwrap();
    assert_eq!(
        inspect(&symlink_path, Some(&sha256(&bytes)))
            .unwrap_err()
            .code(),
        ErrorCode::ArchiveInvalid
    );
}

#[test]
fn archive_preflight_rejects_windows_path_aliases_and_file_parent_collisions() {
    let temp = TempDir::new().expect("temp");
    for (name, entry) in [
        ("ads.ld", "python/operator.py:payload"),
        ("drive.ld", "C:/python/operator.py"),
        ("absolute.ld", "/python/operator.py"),
        ("device.ld", "python/CON.txt"),
        ("console-input-device.ld", "python/CONIN$.txt"),
        ("console-output-device.ld", "python/conout$.JSON"),
        ("backslash.ld", "python\\operator.py"),
    ] {
        let archive = raw_archive(temp.path(), name, &[(entry, b"x")]);
        assert_eq!(
            inspect(&archive.0, Some(&archive.1)).unwrap_err().code(),
            ErrorCode::ArchiveInvalid,
            "unsafe Windows path {entry:?} must fail closed"
        );
    }

    let file_parent = raw_archive(
        temp.path(),
        "file-parent.ld",
        &[("python", b"file"), ("python/operator.py", b"child")],
    );
    assert_eq!(
        inspect(&file_parent.0, Some(&file_parent.1))
            .unwrap_err()
            .code(),
        ErrorCode::ArchiveInvalid
    );
}

#[test]
fn deck_archive_preflight_rejects_compressed_expansion_past_extracted_bound() {
    let temp = TempDir::new().expect("temp");
    let archive_path = temp.path().join("expansion.ld");
    let file = File::create(&archive_path).expect("create archive");
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("deck-pack.json", options)
        .expect("start manifest");
    writer.write_all(b"{}").expect("write manifest");
    writer
        .start_file("payload.txt", options)
        .expect("start expansion payload");
    writer
        .write_all(&vec![0_u8; 17 * 1024 * 1024])
        .expect("write expansion payload");
    writer.finish().expect("finish archive");
    let bytes = fs::read(&archive_path).expect("read archive");
    assert!(
        bytes.len() < 8 * 1024 * 1024,
        "fixture must exercise expansion, not archive size"
    );

    assert_eq!(
        inspect(&archive_path, Some(&sha256(&bytes)))
            .unwrap_err()
            .code(),
        ErrorCode::ArchiveInvalid
    );
}

#[test]
fn compatibility_matrix_resolves_profile_identity_without_assuming_one_fixed_extent() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck");
    let codec_source = temp.path().join("codec");
    let incompatible_codec_source = temp.path().join("codec-incompatible");
    fs::create_dir(&deck_source).unwrap();
    fs::create_dir(&codec_source).unwrap();
    fs::create_dir(&incompatible_codec_source).unwrap();
    write_deck_source(&deck_source, "0.2.0", 45);
    write_codec_source(&codec_source, "0.2.0", profile());
    write_codec_source(
        &incompatible_codec_source,
        "0.2.1",
        ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "other_profile".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
    );
    let deck_archive = temp.path().join("deck.ld");
    let codec_archive = temp.path().join("codec.ldcodec");
    let incompatible_codec_archive = temp.path().join("codec-incompatible.ldcodec");
    let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
    let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
    let (incompatible_codec_hash, _) =
        pack_source(&incompatible_codec_source, &incompatible_codec_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    install(
        &roots,
        &InstallRequest {
            archive_path: deck_archive,
            expected_sha256: deck_hash,
        },
    )
    .unwrap();
    install(
        &roots,
        &InstallRequest {
            archive_path: codec_archive,
            expected_sha256: codec_hash,
        },
    )
    .unwrap();
    install(
        &roots,
        &InstallRequest {
            archive_path: incompatible_codec_archive,
            expected_sha256: incompatible_codec_hash,
        },
    )
    .unwrap();
    let matrix = compatibility_matrix(&roots).unwrap();
    assert_eq!(matrix.len(), 2);
    assert_eq!(
        matrix[0].reason,
        latentdeck_extension_manager::CompatibilityReason::Compatible
    );
    assert_eq!(matrix[0].compatible_profile, Some(profile()));
    assert_eq!(
        matrix[1].reason,
        latentdeck_extension_manager::CompatibilityReason::UnsupportedProfile
    );
    assert_eq!(matrix[1].compatible_profile, None);
}

#[test]
fn deck_source_rejects_forbidden_executable_before_packing() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck");
    fs::create_dir(&source).unwrap();
    write_deck_source(&source, "0.2.0", 45);
    write_file(&source, "bad.exe", b"not allowed");
    let error = pack(&PackRequest {
        source_directory: source,
        output_path: temp.path().join("bad.ld"),
    })
    .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ManifestInvalid);
}

#[test]
fn deck_geometry_allowlist_is_required_bounded_unique_and_closed() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck");
    fs::create_dir(&source).unwrap();
    write_deck_source(&source, "0.2.0", 45);
    let manifest_path = source.join("deck-pack.json");
    let allowlisted: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    let geometry = allowlisted["signal"]["geometry_allowlist"][0].clone();
    fs::write(&manifest_path, canonical(&allowlisted)).unwrap();
    pack(&PackRequest {
        source_directory: source.clone(),
        output_path: temp.path().join("allowlisted.ld"),
    })
    .expect("one exact geometry is a valid closed allowlist");

    let mut invalid_geometry = geometry.clone();
    invalid_geometry["batch"] = serde_json::Value::from(2);
    for (label, geometries) in [
        ("empty", Vec::new()),
        ("duplicate", vec![geometry.clone(), geometry.clone()]),
        ("oversized", vec![geometry.clone(); 65]),
        ("invalid-entry", vec![invalid_geometry]),
    ] {
        let mut invalid = allowlisted.clone();
        invalid["signal"]["geometry_allowlist"] = serde_json::Value::Array(geometries);
        fs::write(&manifest_path, canonical(&invalid)).unwrap();
        assert_eq!(
            pack(&PackRequest {
                source_directory: source.clone(),
                output_path: temp.path().join(format!("{label}.ld")),
            })
            .unwrap_err()
            .code(),
            ErrorCode::ManifestInvalid
        );
    }

    let mut legacy = allowlisted;
    let signal = legacy["signal"].as_object_mut().unwrap();
    signal.remove("geometry_allowlist");
    signal.insert("geometry".to_owned(), geometry);
    fs::write(&manifest_path, canonical(&legacy)).unwrap();
    assert_eq!(
        pack(&PackRequest {
            source_directory: source,
            output_path: temp.path().join("legacy-singular.ld"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::ManifestInvalid
    );
}

#[test]
fn closed_json_rejects_duplicate_keys_unknown_fields_and_missing_v2_capabilities() {
    let temp = TempDir::new().expect("temp");
    let deck = temp.path().join("deck");
    fs::create_dir(&deck).unwrap();
    write_deck_source(&deck, "0.2.0", 45);
    fs::write(
        deck.join("deck-pack.json"),
        br#"{"manifest_version":"1.0.0","manifest_version":"1.0.0"}"#,
    )
    .unwrap();
    assert_eq!(
        pack(&PackRequest {
            source_directory: deck,
            output_path: temp.path().join("duplicate.ld"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::ManifestInvalid
    );

    let codec = temp.path().join("codec");
    fs::create_dir(&codec).unwrap();
    write_codec_source(&codec, "0.2.0", profile());
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(codec.join("codec-pack.json")).unwrap()).unwrap();
    manifest
        .as_object_mut()
        .unwrap()
        .insert("unknown".to_owned(), serde_json::Value::Bool(true));
    fs::write(codec.join("codec-pack.json"), canonical(&manifest)).unwrap();
    assert_eq!(
        pack(&PackRequest {
            source_directory: codec.clone(),
            output_path: temp.path().join("unknown.ldcodec"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::ManifestInvalid
    );

    write_codec_source(&codec, "0.2.0", profile());
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(codec.join("codec-pack.json")).unwrap()).unwrap();
    manifest["capabilities"]
        .as_array_mut()
        .unwrap()
        .retain(|value| value.as_str() != Some("live_capture"));
    fs::write(codec.join("codec-pack.json"), canonical(&manifest)).unwrap();
    assert_eq!(
        pack(&PackRequest {
            source_directory: codec,
            output_path: temp.path().join("missing-capability.ldcodec"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::ManifestInvalid
    );
}

#[test]
fn codec_external_assets_require_at_most_sixteen_exact_hash_length_records() {
    let temp = TempDir::new().expect("temp");
    let codec = temp.path().join("codec");
    fs::create_dir(&codec).unwrap();
    write_codec_source(&codec, "0.2.0", profile());
    let asset = serde_json::json!({
        "asset_id": "decoder",
        "display_name": "Synthetic decoder",
        "required": true,
        "byte_length": 1,
        "sha256": "a".repeat(64),
        "source_url": "https://example.invalid/decoder",
        "license_label": "MIT",
        "license_url": "https://example.invalid/license"
    });
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(codec.join("codec-pack.json")).unwrap()).unwrap();
    manifest["external_assets"] = serde_json::Value::Array(
        (0..17)
            .map(|index| {
                let mut value = asset.clone();
                value["asset_id"] = serde_json::Value::String(format!("decoder-{index}"));
                value
            })
            .collect(),
    );
    fs::write(codec.join("codec-pack.json"), canonical(&manifest)).unwrap();
    assert_eq!(
        pack(&PackRequest {
            source_directory: codec.clone(),
            output_path: temp.path().join("too-many-assets.ldcodec"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::ManifestInvalid
    );

    write_codec_source(&codec, "0.2.0", profile());
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(codec.join("codec-pack.json")).unwrap()).unwrap();
    let mut invalid_asset = asset;
    invalid_asset["byte_length"] = serde_json::Value::from(0);
    invalid_asset["sha256"] = serde_json::Value::String("A".repeat(64));
    manifest["external_assets"] = serde_json::Value::Array(vec![invalid_asset]);
    fs::write(codec.join("codec-pack.json"), canonical(&manifest)).unwrap();
    assert_eq!(
        pack(&PackRequest {
            source_directory: codec,
            output_path: temp.path().join("inexact-asset.ldcodec"),
        })
        .unwrap_err()
        .code(),
        ErrorCode::ManifestInvalid
    );
}

#[test]
fn corrupt_and_orphan_receipts_recover_without_blocking_other_packages() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck");
    fs::create_dir(&source).unwrap();
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let stale = roots
        .staging_root
        .join(".install-00000000000000000000000000000001");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("partial.bin"), b"partial").unwrap();
    let receipt = install(
        &roots,
        &InstallRequest {
            archive_path: archive.clone(),
            expected_sha256: hash.clone(),
        },
    )
    .unwrap();
    assert!(!stale.exists());
    fs::write(&receipt.trust_receipt_path, b"{corrupt receipt").unwrap();
    let summaries = list(&roots).unwrap();
    assert_eq!(summaries[0].health, PackageHealth::Corrupt);
    repair(
        &roots,
        &InstallRequest {
            archive_path: archive.clone(),
            expected_sha256: hash.clone(),
        },
    )
    .unwrap();
    assert!(verify(&roots, &receipt.inspection.package).is_ok());

    fs::remove_dir_all(&receipt.destination).unwrap();
    assert!(receipt.trust_receipt_path.is_file());
    assert!(list(&roots).unwrap().is_empty());
    repair(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .unwrap();
    assert!(verify(&roots, &receipt.inspection.package).is_ok());
}

#[test]
fn lifecycle_recovers_only_bounded_owned_remove_and_repair_trash() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let stale_remove = roots
        .trash_root
        .join(".remove-00000000000000000000000000000001");
    let stale_repair = roots
        .trash_root
        .join(".repair-00000000000000000000000000000002");
    let unrelated = roots.trash_root.join("owner-file.txt");
    fs::create_dir_all(&stale_remove).expect("create stale removal");
    fs::create_dir_all(stale_repair.join("nested")).expect("create stale repair");
    fs::write(stale_remove.join("partial.bin"), b"partial").expect("write stale removal");
    fs::write(stale_repair.join("nested/partial.bin"), b"partial").expect("write stale repair");
    fs::write(&unrelated, b"preserve").expect("write unrelated trash entry");

    install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("lifecycle operation recovers owned crash trash");

    assert!(!stale_remove.exists());
    assert!(!stale_repair.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"preserve");
}

#[test]
fn lifecycle_fails_closed_when_trash_recovery_inventory_exceeds_its_bound() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    fs::create_dir_all(&roots.trash_root).expect("create trash root");
    for index in 0..65 {
        fs::write(
            roots.trash_root.join(format!("owner-{index:02}.txt")),
            b"keep",
        )
        .expect("write bounded trash fixture");
    }

    assert_eq!(
        install(
            &roots,
            &InstallRequest {
                archive_path: archive,
                expected_sha256: hash,
            },
        )
        .expect_err("unbounded trash inventory must stop lifecycle recovery")
        .code(),
        ErrorCode::LifecycleConflict
    );
}

#[cfg(windows)]
#[test]
fn lifecycle_fails_closed_on_owned_trash_reparse_point() {
    use std::os::windows::fs::symlink_dir;

    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck");
    fs::create_dir(&source).expect("create source");
    write_deck_source(&source, "0.2.0", 45);
    let archive = temp.path().join("deck.ld");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    fs::create_dir_all(&roots.trash_root).expect("create trash root");
    let outside = temp.path().join("outside-trash-target");
    fs::create_dir(&outside).expect("create outside target");
    let sentinel = outside.join("keep.txt");
    fs::write(&sentinel, b"keep").expect("write outside sentinel");
    let owned = roots
        .trash_root
        .join(".repair-00000000000000000000000000000003");
    symlink_dir(&outside, &owned).expect("create owned trash reparse point");

    assert_eq!(
        install(
            &roots,
            &InstallRequest {
                archive_path: archive,
                expected_sha256: hash,
            },
        )
        .expect_err("owned trash reparse point must fail closed")
        .code(),
        ErrorCode::LifecycleConflict
    );
    assert_eq!(fs::read(sentinel).unwrap(), b"keep");
    assert!(owned.exists());
}

#[test]
fn cli_exposes_stable_help_invalid_argument_and_empty_list_surfaces() {
    let executable = env!("CARGO_BIN_EXE_latentdeck-extension-manager");
    let help = std::process::Command::new(executable)
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    for command in [
        "inspect", "pack", "install", "verify", "enable", "disable", "remove", "repair", "list",
        "matrix",
    ] {
        assert!(help.contains(command), "CLI help is missing {command}");
    }

    let invalid = std::process::Command::new(executable).output().unwrap();
    assert_eq!(invalid.status.code(), Some(10));
    assert_eq!(String::from_utf8_lossy(&invalid.stderr).lines().count(), 1);

    let temp = TempDir::new().unwrap();
    let empty = std::process::Command::new(executable)
        .args(["--local-app-data", temp.path().to_str().unwrap(), "list"])
        .output()
        .unwrap();
    assert!(empty.status.success());
    assert_eq!(String::from_utf8_lossy(&empty.stdout).trim(), "[]");
}

fn raw_archive(root: &Path, name: &str, entries: &[(&str, &[u8])]) -> (PathBuf, String) {
    let path = root.join(name);
    let file = File::create(&path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for (entry, bytes) in entries {
        writer
            .start_file(*entry, SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
    let bytes = fs::read(&path).unwrap();
    (path, sha256(&bytes))
}
