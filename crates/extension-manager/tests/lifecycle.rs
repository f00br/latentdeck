use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use latentdeck_extension_manager::{
    ActivePackageCache, Architecture, BundledPackageEntry, BundledPackageIndex,
    CodecAdapterDescriptor, CodecCapability, CodecCompatibility, CodecPackManifest,
    CodecWorkerDescriptor, CompatibilityReason, DeckCompatibility, DeckPackManifest,
    DeckRoleDescriptor, DeckRuntimeDescriptor, DeckRuntimeKind, DeckSignalDescriptor, ErrorCode,
    ExtensionRoots, ExternalAssetDescriptor, InstallRequest, IntegrityCatalog, IntegrityDescriptor,
    IntegrityFile, LicenseDescriptor, OperatingSystem, PackRequest, PackageHealth, PackageKind,
    PackageReference, PlatformDescriptor, ProfileKey, PublisherDescriptor, PublisherIdentityClaim,
    PythonConstraint, PythonImplementation, RemoveOptions, RuntimeLockDescriptor, SignalGeometry,
    TensorDevice, TensorDtype, TimingDescriptor, compatibility_matrix, disable, enable,
    enable_if_only_installed_version, inspect, install, install_from_bundled_index, inventory,
    list, list_kind, pack, remove, repair, repair_from_bundled_index, resolve_active,
    resolve_installed, verify,
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

fn rewrite_codec_identities(root: &Path, pack_id: &str, adapter_id: &str) {
    let path = root.join("codec-pack.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read Codec manifest"))
            .expect("parse Codec manifest");
    manifest["pack_id"] = serde_json::Value::String(pack_id.to_owned());
    manifest["adapter"]["adapter_id"] = serde_json::Value::String(adapter_id.to_owned());
    fs::write(path, canonical(&manifest)).expect("rewrite Codec identities");
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

fn codec_runtime_seal_path(roots: &ExtensionRoots, package: &PackageReference) -> PathBuf {
    roots
        .trust_root
        .join(".runtime-v1")
        .join("codecs")
        .join(&package.package_id)
        .join(format!("{}.json", package.package_version))
}

fn install_enabled_codec_fixture(
    temp: &TempDir,
    worker_bytes: &[u8],
) -> (ExtensionRoots, latentdeck_extension_manager::InstallReceipt) {
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), worker_bytes);
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
    .expect("install Codec fixture");
    let enabled = enable(&roots, &installed.inspection.package).expect("enable Codec fixture");
    assert!(
        enabled.runtime_seal_sha256.is_some(),
        "explicit Codec enable must persist the supported NTFS validation seal"
    );
    assert!(codec_runtime_seal_path(&roots, &installed.inspection.package).is_file());
    (roots, installed)
}

fn write_unchecked_deck_archive(source: &Path, output: &Path) -> (String, u64) {
    write_unchecked_deck_archive_with_directories(source, output, &[])
}

fn write_unchecked_deck_archive_with_directories(
    source: &Path,
    output: &Path,
    directories: &[&str],
) -> (String, u64) {
    let file = File::create(output).expect("create unchecked Deck archive");
    let mut writer = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for directory in directories {
        writer
            .add_directory(*directory, options)
            .expect("add unchecked Deck directory");
    }
    for relative in [
        "LICENSE.txt",
        "deck-pack.json",
        "faceplate.json",
        "integrity.json",
        "operator.json",
        "python/deck_operator.py",
    ] {
        writer
            .start_file(relative, options)
            .expect("start unchecked Deck entry");
        writer
            .write_all(&fs::read(source.join(relative)).expect("read unchecked Deck entry"))
            .expect("write unchecked Deck entry");
    }
    writer.finish().expect("finish unchecked Deck archive");
    let bytes = fs::read(output).expect("read unchecked Deck archive");
    (sha256(&bytes), bytes.len() as u64)
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
fn reserved_adapter_identity_requires_a_reserved_codec_pack_identity() {
    let temp = TempDir::new().expect("temp");
    for (label, adapter_id) in [("exact", "org.latentdeck"), ("child", "org.latentdeck.h3")] {
        let external_source = temp.path().join(format!("external-codec-{label}"));
        fs::create_dir(&external_source).expect("create external Codec source");
        write_codec_source(&external_source, "0.2.0", profile());
        rewrite_codec_identities(&external_source, "com.example.codec", adapter_id);
        assert_eq!(
            pack(&PackRequest {
                source_directory: external_source,
                output_path: temp
                    .path()
                    .join(format!("external-reserved-adapter-{label}.ldcodec")),
            })
            .expect_err("external pack must not claim the reserved adapter namespace")
            .code(),
            ErrorCode::ManifestInvalid
        );
    }

    let bundled_source = temp.path().join("bundled-codec");
    fs::create_dir(&bundled_source).expect("create bundled Codec source");
    write_codec_source(&bundled_source, "0.2.0", profile());
    rewrite_codec_identities(
        &bundled_source,
        "org.latentdeck.codec.synthetic",
        "org.latentdeck.synthetic",
    );
    let bundled_archive = temp.path().join("bundled-reserved-adapter.ldcodec");
    let receipt = pack(&PackRequest {
        source_directory: bundled_source,
        output_path: bundled_archive,
    })
    .expect("reserved pack may carry a reserved adapter identity");
    assert_eq!(
        receipt.inspection.package.package_id,
        "org.latentdeck.codec.synthetic"
    );
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

#[cfg(windows)]
#[test]
fn repair_rejects_case_aliased_semver_without_replacing_the_installed_version() {
    let temp = TempDir::new().expect("temp");
    let installed_source = temp.path().join("deck-alpha");
    let alias_source = temp.path().join("deck-uppercase-source");
    fs::create_dir(&installed_source).expect("create installed source");
    fs::create_dir(&alias_source).expect("create alias source");
    write_deck_source(&installed_source, "1.0.0-alpha", 45);
    write_deck_source(&alias_source, "1.0.0-ALPHA", 45);
    let installed_archive = temp.path().join("deck-alpha.ld");
    let alias_archive = temp.path().join("deck-uppercase.ld");
    let (installed_hash, _) = pack_source(&installed_source, &installed_archive);
    let (alias_hash, _) = write_unchecked_deck_archive(&alias_source, &alias_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: installed_archive,
            expected_sha256: installed_hash,
        },
    )
    .expect("install lowercase canonical version");

    assert_eq!(
        install(
            &roots,
            &InstallRequest {
                archive_path: alias_archive.clone(),
                expected_sha256: alias_hash.clone(),
            },
        )
        .expect_err("case-aliased SemVer must not address the install destination")
        .code(),
        ErrorCode::ManifestInvalid
    );
    assert_eq!(
        repair(
            &roots,
            &InstallRequest {
                archive_path: alias_archive,
                expected_sha256: alias_hash,
            },
        )
        .expect_err("case-aliased SemVer must not address the installed version")
        .code(),
        ErrorCode::ManifestInvalid
    );
    assert_eq!(
        resolve_installed(&roots, &installed.inspection.package)
            .expect("original exact version remains installed")
            .manifest()
            .package_version(),
        "1.0.0-alpha"
    );
}

#[test]
fn package_version_storage_key_accepts_115_bytes_and_rejects_116() {
    let temp = TempDir::new().expect("temp");
    let accepted_version = format!("1.0.0-{}", "a".repeat(109));
    let rejected_version = format!("1.0.0-{}", "a".repeat(110));
    assert_eq!(accepted_version.len(), 115);
    assert_eq!(rejected_version.len(), 116);

    let accepted_source = temp.path().join("accepted-source");
    fs::create_dir(&accepted_source).expect("create accepted source");
    write_deck_source(&accepted_source, &accepted_version, 45);
    pack(&PackRequest {
        source_directory: accepted_source,
        output_path: temp.path().join("accepted.ld"),
    })
    .expect("115-byte package version fits the bounded storage key");

    let rejected_source = temp.path().join("rejected-source");
    fs::create_dir(&rejected_source).expect("create rejected source");
    write_deck_source(&rejected_source, &rejected_version, 45);
    assert_eq!(
        pack(&PackRequest {
            source_directory: rejected_source,
            output_path: temp.path().join("rejected.ld"),
        })
        .expect_err("116-byte package version exceeds the bounded storage key")
        .code(),
        ErrorCode::ManifestInvalid
    );
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
fn active_package_clone_holds_the_usage_lease_until_the_last_clone_drops() {
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
    let last_clone = active.clone();
    drop(active);

    disable(&roots, &installed.inspection.package).expect("disable");
    assert_eq!(
        remove(
            &roots,
            &installed.inspection.package,
            RemoveOptions::default(),
        )
        .expect_err("a remaining clone must retain the usage lease")
        .code(),
        ErrorCode::PackageActive
    );

    drop(last_clone);
    remove(
        &roots,
        &installed.inspection.package,
        RemoveOptions::default(),
    )
    .expect("last clone releases the usage lease");
}

#[test]
fn process_cache_reuses_one_cold_validation_for_repeated_exact_checkouts() {
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
    let cache = ActivePackageCache::new();

    let first = cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("cold checkout");
    drop(first);
    let second = cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("cached checkout");

    assert_eq!(second.root(), installed.destination);
    assert_eq!(cache.stats().cold_full_hash_passes, 1);
    assert_eq!(cache.stats().cached_checkouts, 1);
}

#[cfg(windows)]
#[test]
fn fresh_process_cache_reuses_a_persisted_codec_validation_seal() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &vec![0x5a; 8 * 1024 * 1024]);
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

    let first_process = ActivePackageCache::new();
    let active = first_process
        .enable_and_prime(&roots, &installed.inspection.package)
        .expect("first process validates and seals exact Codec bytes");
    assert_eq!(first_process.stats().full_hash_attempts, 1);
    assert!(
        active.trust_receipt().runtime_seal_sha256.is_some(),
        "a supported NTFS LocalAppData tree must receive a persisted seal"
    );
    drop(active);
    drop(first_process);

    let next_process = ActivePackageCache::new();
    let active = next_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("next process checks the persisted seal without payload hashing");

    assert_eq!(active.root(), installed.destination);
    assert_eq!(next_process.stats().full_hash_attempts, 0);
    assert_eq!(next_process.stats().cold_full_hash_passes, 0);
    assert_eq!(next_process.stats().persistent_fast_checkouts, 1);
}

#[cfg(windows)]
#[test]
fn persisted_codec_seal_rejects_same_length_tamper_with_restored_mtime() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &vec![0x5a; 8 * 1024 * 1024]);
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
    let first_process = ActivePackageCache::new();
    let active = first_process
        .enable_and_prime(&roots, &installed.inspection.package)
        .expect("prime persisted seal");
    drop(active);
    drop(first_process);

    let payload = installed.destination.join("runtime/python.exe");
    let original_mtime = fs::metadata(&payload)
        .expect("payload metadata")
        .modified()
        .expect("payload mtime");
    let mut file = File::options()
        .write(true)
        .open(&payload)
        .expect("open payload for same-length tamper");
    file.write_all(&[0xa5]).expect("overwrite one byte");
    file.sync_all().expect("persist tamper");
    file.set_times(std::fs::FileTimes::new().set_modified(original_mtime))
        .expect("restore mtime");
    drop(file);

    let next_process = ActivePackageCache::new();
    assert_eq!(
        next_process
            .resolve_active(&roots, &installed.inspection.package)
            .expect_err("USN mismatch must force a full hash and reject changed bytes")
            .code(),
        ErrorCode::IntegrityFailed
    );
    assert_eq!(next_process.stats().full_hash_attempts, 1);
    assert_eq!(next_process.stats().persistent_fast_checkouts, 0);
}

#[cfg(windows)]
#[test]
fn corrupt_or_missing_codec_seal_falls_back_to_full_hash_and_refreshes() {
    let temp = TempDir::new().expect("temp");
    let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
    let seal_path = codec_runtime_seal_path(&roots, &installed.inspection.package);

    fs::write(&seal_path, b"{corrupt-seal").expect("corrupt persisted seal");
    let corrupt_process = ActivePackageCache::new();
    let active = corrupt_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("corrupt seal falls back to exact catalog hashes");
    assert_eq!(corrupt_process.stats().full_hash_attempts, 1);
    assert_eq!(corrupt_process.stats().persistent_fast_checkouts, 0);
    drop(active);
    drop(corrupt_process);

    fs::remove_file(&seal_path).expect("remove refreshed seal");
    let missing_process = ActivePackageCache::new();
    let active = missing_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("missing seal falls back to exact catalog hashes");
    assert_eq!(missing_process.stats().full_hash_attempts, 1);
    assert_eq!(missing_process.stats().persistent_fast_checkouts, 0);
    drop(active);
    drop(missing_process);

    let refreshed_process = ActivePackageCache::new();
    refreshed_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("fallback publishes a fresh seal for the following process");
    assert_eq!(refreshed_process.stats().full_hash_attempts, 0);
    assert_eq!(refreshed_process.stats().persistent_fast_checkouts, 1);
}

#[cfg(windows)]
#[test]
fn legacy_codec_receipt_hashes_once_then_persists_a_fast_start_seal() {
    let temp = TempDir::new().expect("temp");
    let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
    let mut receipt: serde_json::Value = serde_json::from_slice(
        &fs::read(&installed.trust_receipt_path).expect("read exact receipt"),
    )
    .expect("parse exact receipt");
    receipt
        .as_object_mut()
        .expect("receipt object")
        .remove("runtime_seal_sha256");
    fs::write(&installed.trust_receipt_path, canonical(&receipt))
        .expect("write legacy receipt without optional seal pointer");

    let migration_process = ActivePackageCache::new();
    let active = migration_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("legacy receipt receives one complete validation");
    assert_eq!(migration_process.stats().full_hash_attempts, 1);
    assert!(active.trust_receipt().runtime_seal_sha256.is_some());
    drop(active);
    drop(migration_process);

    let next_process = ActivePackageCache::new();
    next_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("next process uses the migrated receipt seal");
    assert_eq!(next_process.stats().full_hash_attempts, 0);
    assert_eq!(next_process.stats().persistent_fast_checkouts, 1);
}

#[cfg(windows)]
#[test]
fn exact_byte_file_replacement_invalidates_codec_seal_by_file_identity() {
    let temp = TempDir::new().expect("temp");
    let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
    let payload = installed.destination.join("runtime/python.exe");
    let exact_bytes = fs::read(&payload).expect("read exact payload bytes");
    fs::remove_file(&payload).expect("remove original file identity");
    fs::write(&payload, exact_bytes).expect("restore exact bytes under a new file identity");

    let replacement_process = ActivePackageCache::new();
    let active = replacement_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("new file identity forces full hashes but exact bytes remain trusted");
    assert_eq!(replacement_process.stats().full_hash_attempts, 1);
    assert_eq!(replacement_process.stats().persistent_fast_checkouts, 0);
    drop(active);
    drop(replacement_process);

    let next_process = ActivePackageCache::new();
    next_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("replacement receives a new seal after exact hash validation");
    assert_eq!(next_process.stats().full_hash_attempts, 0);
}

#[cfg(windows)]
#[test]
fn codec_seal_closed_tree_check_rejects_added_removed_and_reparse_children() {
    use std::os::windows::fs::symlink_dir;

    for mutation in ["added", "removed", "reparse"] {
        let temp = TempDir::new().expect("temp");
        let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
        match mutation {
            "added" => fs::write(installed.destination.join("unexpected.txt"), b"unexpected")
                .expect("add uncatalogued child"),
            "removed" => fs::remove_file(installed.destination.join("runtime/python.exe"))
                .expect("remove catalogued child"),
            "reparse" => {
                let outside = temp.path().join("outside");
                fs::create_dir(&outside).expect("create outside directory");
                symlink_dir(&outside, installed.destination.join("unexpected-link"))
                    .expect("create uncatalogued reparse child");
            }
            _ => unreachable!(),
        }

        let process = ActivePackageCache::new();
        let expected_code = if mutation == "reparse" {
            ErrorCode::LifecycleConflict
        } else {
            ErrorCode::IntegrityFailed
        };
        assert_eq!(
            process
                .resolve_active(&roots, &installed.inspection.package)
                .expect_err("closed tree mutation must fail before trusting a persisted seal")
                .code(),
            expected_code,
            "mutation={mutation}"
        );
        assert_eq!(
            process.stats().full_hash_attempts,
            0,
            "cheap closed-tree rejection precedes a payload hash: mutation={mutation}"
        );
    }
}

#[cfg(windows)]
#[test]
fn journal_identity_mismatch_falls_back_to_full_hash_validation() {
    let temp = TempDir::new().expect("temp");
    let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
    let seal_path = codec_runtime_seal_path(&roots, &installed.inspection.package);
    let mut seal: serde_json::Value =
        serde_json::from_slice(&fs::read(&seal_path).expect("read runtime seal"))
            .expect("parse runtime seal");
    seal["tree"]["usn_journal_id"] = serde_json::Value::String("ffffffffffffffff".to_owned());
    let seal_bytes = canonical(&seal);
    fs::write(&seal_path, &seal_bytes).expect("write journal-reset simulation");
    let mut receipt: serde_json::Value = serde_json::from_slice(
        &fs::read(&installed.trust_receipt_path).expect("read exact receipt"),
    )
    .expect("parse exact receipt");
    receipt["runtime_seal_sha256"] = serde_json::Value::String(sha256(&seal_bytes));
    fs::write(&installed.trust_receipt_path, canonical(&receipt))
        .expect("bind receipt to journal-reset simulation");

    let process = ActivePackageCache::new();
    process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("journal identity mismatch falls back to exact payload hashes");
    assert_eq!(process.stats().full_hash_attempts, 1);
    assert_eq!(process.stats().persistent_fast_checkouts, 0);
}

#[cfg(windows)]
#[test]
fn explicit_verify_refreshes_codec_seal_for_the_next_process() {
    let temp = TempDir::new().expect("temp");
    let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
    let seal_path = codec_runtime_seal_path(&roots, &installed.inspection.package);
    fs::write(&seal_path, b"invalid").expect("invalidate runtime seal");

    verify(&roots, &installed.inspection.package)
        .expect("explicit verify performs full validation and refreshes the seal");

    let next_process = ActivePackageCache::new();
    next_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("fresh process uses seal refreshed by explicit verify");
    assert_eq!(next_process.stats().full_hash_attempts, 0);
    assert_eq!(next_process.stats().persistent_fast_checkouts, 1);
}

#[cfg(windows)]
#[test]
fn repair_and_remove_delete_codec_runtime_seal_sidecars() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &vec![0x5a; 1024 * 1024]);
    let archive = temp.path().join("codec.ldcodec");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let installed = install(
        &roots,
        &InstallRequest {
            archive_path: archive.clone(),
            expected_sha256: hash.clone(),
        },
    )
    .expect("install disabled Codec");
    let seal_path = codec_runtime_seal_path(&roots, &installed.inspection.package);

    verify(&roots, &installed.inspection.package).expect("verify creates disabled-package seal");
    assert!(seal_path.is_file());
    repair(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("repair exact disabled Codec");
    assert!(!seal_path.exists(), "repair removes stale runtime seal");

    verify(&roots, &installed.inspection.package).expect("verify recreates runtime seal");
    assert!(seal_path.is_file());
    remove(
        &roots,
        &installed.inspection.package,
        RemoveOptions::default(),
    )
    .expect("remove exact disabled Codec");
    assert!(!seal_path.exists(), "remove deletes runtime seal sidecar");
}

#[cfg(windows)]
#[test]
fn unavailable_optional_seal_storage_cannot_block_enable_verify_or_runtime() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &vec![0x5a; 1024 * 1024]);
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
    .expect("install disabled Codec");
    fs::write(
        roots.trust_root.join(".runtime-v1"),
        b"unavailable optional cache root",
    )
    .expect("block optional seal directory with a regular file");

    let enabled = enable(&roots, &installed.inspection.package)
        .expect("full validation remains authoritative when optional seal write fails");
    assert!(enabled.enabled);
    assert!(enabled.runtime_seal_sha256.is_none());
    verify(&roots, &installed.inspection.package)
        .expect("explicit full verify cannot be blocked by optional seal storage");

    let process = ActivePackageCache::new();
    let active = process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("unsafe optional seal path falls back to complete hashes");
    assert_eq!(process.stats().full_hash_attempts, 1);
    assert!(active.trust_receipt().runtime_seal_sha256.is_none());
}

#[cfg(windows)]
#[test]
fn optional_seal_reparse_falls_back_and_partial_cleanup_is_bounded_to_owned_names() {
    use std::os::windows::fs::symlink_dir;

    let temp = TempDir::new().expect("temp");
    let (roots, installed) = install_enabled_codec_fixture(&temp, &vec![0x5a; 1024 * 1024]);
    let seal_path = codec_runtime_seal_path(&roots, &installed.inspection.package);
    let seal_parent = seal_path.parent().expect("seal parent");
    let owned_partial = seal_parent.join(".seal-00000000000000000000000000000001.partial");
    let unrelated = seal_parent.join(".seal-owner-notes.partial");
    fs::write(&owned_partial, b"stale").expect("write owned stale partial");
    fs::write(&unrelated, b"keep").expect("write unrelated file");

    let cleanup_process = ActivePackageCache::new();
    cleanup_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("valid seal remains usable while owned stale partial is cleaned");
    assert!(!owned_partial.exists());
    assert!(unrelated.is_file());
    drop(cleanup_process);

    let runtime_root = roots.trust_root.join(".runtime-v1");
    fs::remove_dir_all(&runtime_root).expect("remove test-owned optional cache root");
    let outside = temp.path().join("outside-runtime-seal");
    let outside_seal = outside
        .join("codecs")
        .join(&installed.inspection.package.package_id)
        .join(format!(
            "{}.json",
            installed.inspection.package.package_version
        ));
    fs::create_dir_all(outside_seal.parent().expect("outside seal parent"))
        .expect("create optional reparse target");
    fs::write(&outside_seal, b"external sentinel").expect("write external seal sentinel");
    symlink_dir(&outside, &runtime_root).expect("create optional cache reparse point");

    let fallback_process = ActivePackageCache::new();
    let active = fallback_process
        .resolve_active(&roots, &installed.inspection.package)
        .expect("optional cache reparse falls back to full hash validation");
    assert_eq!(fallback_process.stats().full_hash_attempts, 1);
    assert!(active.trust_receipt().runtime_seal_sha256.is_none());
    assert_eq!(
        fs::read(&outside_seal).expect("read sentinel"),
        b"external sentinel"
    );
    drop(active);
    drop(fallback_process);

    disable(&roots, &installed.inspection.package).expect("disable with optional reparse present");
    assert_eq!(
        fs::read(&outside_seal).expect("read sentinel"),
        b"external sentinel"
    );
    let archive = temp.path().join("codec.ldcodec");
    let archive_hash = sha256(&fs::read(&archive).expect("read repair archive"));
    repair(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: archive_hash,
        },
    )
    .expect("repair with optional reparse present");
    assert_eq!(
        fs::read(&outside_seal).expect("read sentinel"),
        b"external sentinel"
    );
    remove(
        &roots,
        &installed.inspection.package,
        RemoveOptions::default(),
    )
    .expect("remove with optional reparse present");
    assert_eq!(
        fs::read(&outside_seal).expect("read sentinel"),
        b"external sentinel"
    );
}

#[test]
fn runtime_inventory_primes_matrix_and_runtime_boundaries_with_one_hash_per_package() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck-source");
    let codec_source = temp.path().join("codec-source");
    fs::create_dir(&deck_source).expect("create Deck source");
    fs::create_dir(&codec_source).expect("create Codec source");
    write_deck_source(&deck_source, "0.2.0", 45);
    write_codec_source(&codec_source, "0.2.0", profile());
    let deck_archive = temp.path().join("deck.ld");
    let codec_archive = temp.path().join("codec.ldcodec");
    let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
    let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let deck = install(
        &roots,
        &InstallRequest {
            archive_path: deck_archive,
            expected_sha256: deck_hash,
        },
    )
    .expect("install Deck");
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: codec_archive,
            expected_sha256: codec_hash,
        },
    )
    .expect("install Codec");
    enable(&roots, &deck.inspection.package).expect("enable Deck");
    enable(&roots, &codec.inspection.package).expect("enable Codec");
    let cache = ActivePackageCache::new();

    let matrix = cache
        .runtime_inventory(&roots)
        .expect("authoritative runtime inventory");
    assert_eq!(matrix.packages.len(), 2);
    assert_eq!(matrix.matrix.len(), 1);
    assert_eq!(matrix.matrix[0].reason, CompatibilityReason::Compatible);
    let expected_full_hash_passes = if cfg!(windows) { 1 } else { 2 };
    assert_eq!(
        cache.stats().cold_full_hash_passes,
        expected_full_hash_passes
    );
    assert_eq!(
        cache.stats().persistent_fast_checkouts,
        u64::from(cfg!(windows))
    );

    for package in [&deck.inspection.package, &codec.inspection.package] {
        cache
            .resolve_active(&roots, package)
            .expect("runtime options checkout");
    }
    for package in [&deck.inspection.package, &codec.inspection.package] {
        cache
            .resolve_active(&roots, package)
            .expect("open checkout");
    }

    assert_eq!(
        cache.stats().cold_full_hash_passes,
        expected_full_hash_passes
    );
    assert_eq!(cache.stats().cached_checkouts, 4);
}

#[test]
fn disabled_package_matrix_matches_runtime_untrusted_refusal() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck-source");
    let codec_source = temp.path().join("codec-source");
    fs::create_dir(&deck_source).expect("create Deck source");
    fs::create_dir(&codec_source).expect("create Codec source");
    write_deck_source(&deck_source, "0.2.0", 45);
    write_codec_source(&codec_source, "0.2.0", profile());
    let deck_archive = temp.path().join("deck.ld");
    let codec_archive = temp.path().join("codec.ldcodec");
    let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
    let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let deck = install(
        &roots,
        &InstallRequest {
            archive_path: deck_archive,
            expected_sha256: deck_hash,
        },
    )
    .expect("install disabled Deck");
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: codec_archive,
            expected_sha256: codec_hash,
        },
    )
    .expect("install disabled Codec");
    let cache = ActivePackageCache::new();

    let inventory = cache
        .runtime_inventory(&roots)
        .expect("disabled matrix inventory");

    assert_eq!(inventory.matrix.len(), 1);
    assert_eq!(inventory.matrix[0].reason, CompatibilityReason::Untrusted);
    assert_eq!(
        cache
            .resolve_active(&roots, &deck.inspection.package)
            .expect_err("disabled Deck runtime refusal")
            .code(),
        ErrorCode::PackageDisabled
    );
    assert_eq!(
        cache
            .resolve_active(&roots, &codec.inspection.package)
            .expect_err("disabled Codec runtime refusal")
            .code(),
        ErrorCode::PackageDisabled
    );
}

#[test]
fn runtime_inventory_isolates_a_disabled_candidate_summary_failure() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck-source");
    let codec_source = temp.path().join("codec-source");
    fs::create_dir(&deck_source).expect("create Deck source");
    fs::create_dir(&codec_source).expect("create Codec source");
    write_deck_source(&deck_source, "0.2.0", 45);
    write_codec_source(&codec_source, "0.2.0", profile());
    let deck_archive = temp.path().join("deck.ld");
    let codec_archive = temp.path().join("codec.ldcodec");
    let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
    let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let deck = install(
        &roots,
        &InstallRequest {
            archive_path: deck_archive,
            expected_sha256: deck_hash,
        },
    )
    .expect("install healthy Deck");
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: codec_archive,
            expected_sha256: codec_hash,
        },
    )
    .expect("install Codec");
    fs::write(&codec.trust_receipt_path, b"{broken receipt")
        .expect("corrupt only one candidate receipt");
    let cache = ActivePackageCache::new();

    let inventory = cache
        .runtime_inventory(&roots)
        .expect("one candidate failure must not abort the snapshot");
    assert_eq!(inventory.packages.len(), 2);
    let deck_summary = inventory
        .packages
        .iter()
        .find(|summary| summary.package == deck.inspection.package)
        .expect("healthy Deck remains visible");
    assert_eq!(deck_summary.health, PackageHealth::Healthy);
    let codec_summary = inventory
        .packages
        .iter()
        .find(|summary| summary.package == codec.inspection.package)
        .expect("failed Codec is isolated as an exact summary");
    assert_eq!(codec_summary.health, PackageHealth::Corrupt);
    assert_eq!(inventory.matrix.len(), 1);
    assert_eq!(inventory.matrix[0].reason, CompatibilityReason::Untrusted);
}

#[test]
fn disabled_codec_inventory_is_metadata_only_until_explicit_full_verification() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    let worker = vec![0x5a; 8 * 1024 * 1024];
    write_codec_source_with_worker(&source, "0.2.0", profile(), &worker);
    let archive = temp.path().join("codec.ldcodec");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install disabled Codec");
    let cache = ActivePackageCache::new();

    for _ in 0..2 {
        let inventory = cache
            .runtime_inventory(&roots)
            .expect("metadata-only disabled Codec inventory");
        assert_eq!(inventory.packages.len(), 1);
        assert!(!inventory.packages[0].enabled);
        assert_eq!(
            inventory.packages[0].health,
            PackageHealth::VerificationRequired
        );
    }
    assert_eq!(cache.stats().full_hash_attempts, 0);

    let payload = codec.destination.join("runtime/python.exe");
    let mut changed = fs::read(&payload).expect("read installed worker");
    changed[0] ^= 1;
    fs::write(&payload, changed).expect("same-length disabled payload change");
    let metadata_snapshot = cache
        .runtime_inventory(&roots)
        .expect("metadata remains bounded and non-executable");
    assert_eq!(
        metadata_snapshot.packages[0].health,
        PackageHealth::VerificationRequired
    );
    assert_eq!(cache.stats().full_hash_attempts, 0);
    assert_eq!(
        enable(&roots, &codec.inspection.package)
            .expect_err("enable must hash and reject changed disabled bytes")
            .code(),
        ErrorCode::IntegrityFailed
    );
    assert_eq!(
        verify(&roots, &codec.inspection.package)
            .expect_err("explicit verify must hash and reject changed disabled bytes")
            .code(),
        ErrorCode::IntegrityFailed
    );
}

#[test]
fn cached_enable_primes_runtime_inventory_with_one_full_hash_pass() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &vec![0x5a; 8 * 1024 * 1024]);
    let archive = temp.path().join("codec.ldcodec");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install disabled Codec");
    let cache = ActivePackageCache::new();

    let active = cache
        .enable_and_prime(&roots, &codec.inspection.package)
        .expect("validate, enable, and retain exact Codec in one pass");
    assert!(active.trust_receipt().enabled);
    assert_eq!(cache.stats().full_hash_attempts, 1);
    assert_eq!(cache.stats().cold_full_hash_passes, 1);

    let inventory = cache
        .runtime_inventory(&roots)
        .expect("enabled Codec snapshot reuses primed exact lease");
    assert_eq!(inventory.packages[0].health, PackageHealth::Healthy);
    assert!(inventory.packages[0].enabled);
    let checkout = cache
        .resolve_active(&roots, &codec.inspection.package)
        .expect("runtime checkout reuses primed exact lease");
    assert!(checkout.trust_receipt().enabled);
    assert_eq!(cache.stats().full_hash_attempts, 1);
    assert_eq!(cache.stats().cold_full_hash_passes, 1);
}

#[test]
fn enable_conflict_is_rejected_before_any_payload_hash_pass() {
    let temp = TempDir::new().expect("temp");
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let mut installed = Vec::new();
    for version in ["0.2.0", "0.2.1"] {
        let source = temp.path().join(format!("codec-{version}"));
        fs::create_dir(&source).expect("create Codec source");
        write_codec_source_with_worker(&source, version, profile(), &vec![0x5a; 2 * 1024 * 1024]);
        let archive = temp.path().join(format!("codec-{version}.ldcodec"));
        let (hash, _) = pack_source(&source, &archive);
        installed.push(
            install(
                &roots,
                &InstallRequest {
                    archive_path: archive,
                    expected_sha256: hash,
                },
            )
            .expect("install side-by-side Codec"),
        );
    }
    enable(&roots, &installed[0].inspection.package).expect("enable first exact version");
    let blocked_payload = installed[1].destination.join("runtime/python.exe");
    let mut changed = fs::read(&blocked_payload).expect("read blocked payload");
    changed[0] ^= 1;
    fs::write(&blocked_payload, changed).expect("change blocked payload");

    let cache = ActivePackageCache::new();
    assert_eq!(
        cache
            .enable_and_prime(&roots, &installed[1].inspection.package)
            .expect_err("active alternate version must reject before payload hashing")
            .code(),
        ErrorCode::LifecycleConflict
    );
    assert_eq!(cache.stats().full_hash_attempts, 0);
    assert_eq!(
        enable(&roots, &installed[1].inspection.package)
            .expect_err("public enable also rejects the known conflict first")
            .code(),
        ErrorCode::LifecycleConflict
    );
}

#[test]
fn disable_revokes_future_use_without_hashing_payload_again() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &vec![0x5a; 8 * 1024 * 1024]);
    let archive = temp.path().join("codec.ldcodec");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install disabled Codec");
    let cache = ActivePackageCache::new();
    let live = cache
        .enable_and_prime(&roots, &codec.inspection.package)
        .expect("enable and prime");
    assert_eq!(cache.stats().full_hash_attempts, 1);

    let receipt = cache
        .disable(&roots, &codec.inspection.package)
        .expect("disable only narrows authority");
    assert!(!receipt.enabled);
    assert!(receipt.runtime_seal_sha256.is_none());
    assert!(!codec_runtime_seal_path(&roots, &codec.inspection.package).exists());
    assert_eq!(cache.stats().full_hash_attempts, 1);
    assert_eq!(
        cache
            .resolve_active(&roots, &codec.inspection.package)
            .expect_err("future checkout observes revocation")
            .code(),
        ErrorCode::PackageDisabled
    );
    assert!(
        live.trust_receipt().enabled,
        "existing lease is point-in-time"
    );
}

#[test]
fn fast_disable_rejects_a_corrupt_exact_receipt_without_payload_hashing() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source_with_worker(&source, "0.2.0", profile(), &[0x5a; 1024]);
    let archive = temp.path().join("codec.ldcodec");
    let (hash, _) = pack_source(&source, &archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("install disabled Codec");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&codec.trust_receipt_path).expect("read exact receipt"))
            .expect("parse exact receipt");
    receipt["package"]["package_version"] = serde_json::Value::String("9.9.9".to_owned());
    fs::write(&codec.trust_receipt_path, canonical(&receipt)).expect("poison exact receipt");
    let cache = ActivePackageCache::new();

    assert_eq!(
        cache
            .disable(&roots, &codec.inspection.package)
            .expect_err("corrupt receipt must fail closed")
            .code(),
        ErrorCode::PackageUntrusted
    );
    assert_eq!(cache.stats().full_hash_attempts, 0);
}

#[test]
fn runtime_inventory_bounds_process_leases_by_entry_and_handle_budget() {
    const PACKAGE_COUNT: usize = 17;
    let temp = TempDir::new().expect("temp");
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    for index in 0..PACKAGE_COUNT {
        let source = temp.path().join(format!("deck-source-{index:02}"));
        fs::create_dir(&source).expect("create Deck source");
        write_deck_source_with_id(&source, &format!("com.example.deck{index:02}"), "0.2.0", 45);
        let archive = temp.path().join(format!("deck-{index:02}.ld"));
        let (hash, _) = pack_source(&source, &archive);
        let installed = install(
            &roots,
            &InstallRequest {
                archive_path: archive,
                expected_sha256: hash,
            },
        )
        .expect("install Deck");
        enable(&roots, &installed.inspection.package).expect("enable Deck");
    }
    let cache = ActivePackageCache::new();

    let packages = cache
        .runtime_list_kind(&roots, PackageKind::DeckPack)
        .expect("bounded runtime inventory");
    let stats = cache.stats();

    assert_eq!(packages.len(), PACKAGE_COUNT);
    assert_eq!(stats.cold_full_hash_passes, PACKAGE_COUNT as u64);
    assert_eq!(stats.retained_entries, 16);
    assert!(stats.retained_handles <= 16_384);
    assert_eq!(stats.capacity_evictions, 1);
}

#[test]
fn runtime_inventory_does_not_rehash_an_enabled_corrupt_package_for_its_summary() {
    let temp = TempDir::new().expect("temp");
    let deck_source = temp.path().join("deck-source");
    let codec_source = temp.path().join("codec-source");
    fs::create_dir(&deck_source).expect("create Deck source");
    fs::create_dir(&codec_source).expect("create Codec source");
    write_deck_source(&deck_source, "0.2.0", 45);
    write_codec_source(&codec_source, "0.2.0", profile());
    let deck_archive = temp.path().join("deck.ld");
    let codec_archive = temp.path().join("codec.ldcodec");
    let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
    let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let deck = install(
        &roots,
        &InstallRequest {
            archive_path: deck_archive,
            expected_sha256: deck_hash,
        },
    )
    .expect("install Deck");
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: codec_archive,
            expected_sha256: codec_hash,
        },
    )
    .expect("install Codec");
    enable(&roots, &deck.inspection.package).expect("enable Deck");
    enable(&roots, &codec.inspection.package).expect("enable Codec");
    let payload = deck.destination.join("python/deck_operator.py");
    let mut bytes = fs::read(&payload).expect("read installed payload");
    bytes[0] ^= 1;
    fs::write(&payload, bytes).expect("tamper installed payload");
    let cache = ActivePackageCache::new();

    let inventory = cache
        .runtime_inventory(&roots)
        .expect("corrupt package is isolated");
    let stats = cache.stats();

    assert_eq!(inventory.packages.len(), 2);
    let corrupt = inventory
        .packages
        .iter()
        .find(|summary| summary.package == deck.inspection.package)
        .expect("corrupt Deck summary");
    assert_eq!(corrupt.health, PackageHealth::Corrupt);
    assert_eq!(
        corrupt.error_code.as_deref(),
        Some(ErrorCode::IntegrityFailed.as_str())
    );
    assert_eq!(inventory.matrix.len(), 1);
    assert_eq!(
        inventory.matrix[0].reason,
        CompatibilityReason::PackageInvalid
    );
    let expected_codec_full_hashes = u64::from(!cfg!(windows));
    assert_eq!(stats.full_hash_attempts, 1 + expected_codec_full_hashes);
    assert_eq!(stats.cold_full_hash_passes, expected_codec_full_hashes);
    assert_eq!(stats.persistent_fast_checkouts, u64::from(cfg!(windows)));
}

#[test]
fn process_cache_rejects_a_disabled_receipt_before_cached_checkout() {
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
    let cache = ActivePackageCache::new();
    cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("prime cache");

    disable(&roots, &installed.inspection.package).expect("disable receipt");

    assert_eq!(
        cache
            .resolve_active(&roots, &installed.inspection.package)
            .expect_err("disabled receipt must revoke future cached checkouts")
            .code(),
        ErrorCode::PackageDisabled
    );
}

#[test]
fn process_cache_rejects_an_added_child_before_cached_checkout() {
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
    let cache = ActivePackageCache::new();
    cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("prime cache");

    fs::write(
        installed.destination.join("unexpected.txt"),
        b"uncatalogued",
    )
    .expect("add uncatalogued child");

    assert_eq!(
        cache
            .resolve_active(&roots, &installed.inspection.package)
            .expect_err("closed-tree scan must reject an added child")
            .code(),
        ErrorCode::IntegrityFailed
    );
}

#[test]
fn process_cache_rejects_a_deep_added_child_before_cached_checkout() {
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
    let cache = ActivePackageCache::new();
    cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("prime cache");

    fs::write(
        installed.destination.join("python/unexpected.txt"),
        b"uncatalogued",
    )
    .expect("add deep uncatalogued child");

    assert_eq!(
        cache
            .resolve_active(&roots, &installed.inspection.package)
            .expect_err("closed-tree scan must reject a deep added child")
            .code(),
        ErrorCode::IntegrityFailed
    );
}

#[test]
fn process_cache_rejects_a_changed_exact_receipt_before_cached_checkout() {
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
    let cache = ActivePackageCache::new();
    cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("prime cache");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&installed.trust_receipt_path).expect("read receipt"))
            .expect("parse receipt");
    receipt["manifest_sha256"] = serde_json::Value::String("a".repeat(64));
    fs::write(&installed.trust_receipt_path, canonical(&receipt)).expect("change receipt");

    assert_eq!(
        cache
            .resolve_active(&roots, &installed.inspection.package)
            .expect_err("changed exact receipt must revoke cached checkout")
            .code(),
        ErrorCode::PackageUntrusted
    );
}

#[test]
fn process_cache_singleflights_concurrent_cold_checkouts_per_exact_package() {
    const CALLERS: usize = 8;
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create source");
    let worker = vec![0x5a; 8 * 1024 * 1024];
    write_codec_source_with_worker(&source, "0.2.0", profile(), &worker);
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
    let cache = Arc::new(ActivePackageCache::new());
    let barrier = Arc::new(Barrier::new(CALLERS));
    let mut callers = Vec::new();
    for _ in 0..CALLERS {
        let cache = Arc::clone(&cache);
        let barrier = Arc::clone(&barrier);
        let roots = roots.clone();
        let package = installed.inspection.package.clone();
        callers.push(std::thread::spawn(move || {
            barrier.wait();
            cache.resolve_active(&roots, &package)
        }));
    }

    for caller in callers {
        caller
            .join()
            .expect("checkout thread")
            .expect("singleflight caller succeeds");
    }
    assert_eq!(
        cache.stats().cold_full_hash_passes,
        u64::from(!cfg!(windows))
    );
    assert_eq!(
        cache.stats().persistent_fast_checkouts,
        u64::from(cfg!(windows))
    );
    assert_eq!(cache.stats().cached_checkouts, (CALLERS - 1) as u64);
}

#[test]
fn kind_filtered_list_does_not_inspect_a_poisoned_other_kind_root() {
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
    fs::remove_dir(&roots.codec_packs_root).expect("remove empty other-kind root");
    fs::write(&roots.codec_packs_root, b"poisoned other kind").expect("poison other-kind root");

    let decks = list_kind(&roots, PackageKind::DeckPack).expect("list only Deck packages");

    assert_eq!(decks.len(), 1);
    assert_eq!(decks[0].package, installed.inspection.package);
}

#[test]
fn cache_owned_lease_blocks_remove_and_repair_until_exact_invalidation() {
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
    enable(&roots, &installed.inspection.package).expect("enable");
    let cache = ActivePackageCache::new();
    cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("prime cache");
    disable(&roots, &installed.inspection.package).expect("disable");

    assert_eq!(
        remove(
            &roots,
            &installed.inspection.package,
            RemoveOptions::default(),
        )
        .expect_err("cache-owned lease blocks removal")
        .code(),
        ErrorCode::PackageActive
    );
    assert_eq!(
        repair(
            &roots,
            &InstallRequest {
                archive_path: archive.clone(),
                expected_sha256: hash.clone(),
            },
        )
        .expect_err("cache-owned lease blocks repair")
        .code(),
        ErrorCode::PackageActive
    );

    assert!(cache.invalidate_exact(&roots, &installed.inspection.package));
    repair(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
    )
    .expect("repair after exact cache invalidation");
    remove(
        &roots,
        &installed.inspection.package,
        RemoveOptions::default(),
    )
    .expect("remove after exact cache invalidation");
}

#[test]
fn explicit_verify_remains_independent_of_the_process_cache() {
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
    let cache = ActivePackageCache::new();
    cache
        .resolve_active(&roots, &installed.inspection.package)
        .expect("prime cache");
    fs::write(
        installed.destination.join("unexpected.txt"),
        b"uncatalogued",
    )
    .expect("add uncatalogued child");

    assert_eq!(
        verify(&roots, &installed.inspection.package)
            .expect_err("explicit verify must independently inspect the complete tree")
            .code(),
        ErrorCode::IntegrityFailed
    );
    assert_eq!(cache.stats().cold_full_hash_passes, 1);
    assert_eq!(cache.stats().cached_checkouts, 0);
}

#[test]
#[ignore = "requires an explicitly supplied installed package for local performance evidence"]
fn installed_package_enable_and_seal_timing_from_environment() {
    let local_app_data =
        std::env::var_os("LATENTDECK_PERF_LOCAL_APP_DATA").expect("LATENTDECK_PERF_LOCAL_APP_DATA");
    let package = PackageReference {
        kind: PackageKind::CodecPack,
        package_id: std::env::var("LATENTDECK_PERF_CODEC_ID").expect("LATENTDECK_PERF_CODEC_ID"),
        package_version: std::env::var("LATENTDECK_PERF_CODEC_VERSION")
            .expect("LATENTDECK_PERF_CODEC_VERSION"),
    };
    let roots = ExtensionRoots::from_local_app_data(local_app_data);
    let cache = ActivePackageCache::new();

    let started = std::time::Instant::now();
    let active = cache
        .enable_and_prime(&roots, &package)
        .expect("full-validate, enable, seal, and prime installed Codec");
    let elapsed = started.elapsed();
    let stats = cache.stats();

    assert!(active.trust_receipt().enabled);
    assert_eq!(stats.full_hash_attempts, 1);
    assert_eq!(stats.cold_full_hash_passes, 1);
    #[cfg(windows)]
    assert!(active.trust_receipt().runtime_seal_sha256.is_some());
    eprintln!(
        "enable_and_seal_seconds={:.6} full_hash_attempts={} successful_full_hash_passes={} runtime_seal={}",
        elapsed.as_secs_f64(),
        stats.full_hash_attempts,
        stats.cold_full_hash_passes,
        active.trust_receipt().runtime_seal_sha256.is_some()
    );
}

#[test]
#[ignore = "requires an explicitly supplied installed package for local performance evidence"]
fn installed_package_cache_timing_from_environment() {
    let local_app_data =
        std::env::var_os("LATENTDECK_PERF_LOCAL_APP_DATA").expect("LATENTDECK_PERF_LOCAL_APP_DATA");
    let package = PackageReference {
        kind: PackageKind::CodecPack,
        package_id: std::env::var("LATENTDECK_PERF_CODEC_ID").expect("LATENTDECK_PERF_CODEC_ID"),
        package_version: std::env::var("LATENTDECK_PERF_CODEC_VERSION")
            .expect("LATENTDECK_PERF_CODEC_VERSION"),
    };
    let roots = ExtensionRoots::from_local_app_data(local_app_data);
    let cache = ActivePackageCache::new();

    let cold_started = std::time::Instant::now();
    let cold = cache
        .resolve_active(&roots, &package)
        .expect("cold active checkout");
    let cold_elapsed = cold_started.elapsed();
    let cached_started = std::time::Instant::now();
    let cached = cache
        .resolve_active(&roots, &package)
        .expect("cached active checkout");
    let cached_elapsed = cached_started.elapsed();

    assert_eq!(cold.root(), cached.root());
    let stats = cache.stats();
    assert_eq!(
        stats.cold_full_hash_passes + stats.persistent_fast_checkouts,
        1,
        "one new-process validation path must complete"
    );
    assert_eq!(stats.cached_checkouts, 1);
    eprintln!(
        "cold_seconds={:.6} cached_seconds={:.6} full_hash_attempts={} successful_full_hash_passes={} persistent_fast_checkouts={}",
        cold_elapsed.as_secs_f64(),
        cached_elapsed.as_secs_f64(),
        stats.full_hash_attempts,
        stats.cold_full_hash_passes,
        stats.persistent_fast_checkouts
    );
}

#[test]
#[ignore = "requires an explicitly supplied installed package for local performance evidence"]
fn installed_runtime_inventory_primes_selected_package_from_environment() {
    let local_app_data =
        std::env::var_os("LATENTDECK_PERF_LOCAL_APP_DATA").expect("LATENTDECK_PERF_LOCAL_APP_DATA");
    let package = PackageReference {
        kind: PackageKind::CodecPack,
        package_id: std::env::var("LATENTDECK_PERF_CODEC_ID").expect("LATENTDECK_PERF_CODEC_ID"),
        package_version: std::env::var("LATENTDECK_PERF_CODEC_VERSION")
            .expect("LATENTDECK_PERF_CODEC_VERSION"),
    };
    let roots = ExtensionRoots::from_local_app_data(local_app_data);
    let cache = ActivePackageCache::new();

    let inventory_started = std::time::Instant::now();
    let inventory = cache
        .runtime_inventory(&roots)
        .expect("authoritative runtime inventory");
    let inventory_elapsed = inventory_started.elapsed();
    assert!(
        inventory
            .packages
            .iter()
            .any(|summary| summary.package == package && summary.enabled)
    );
    let after_inventory = cache.stats();
    let checkout_started = std::time::Instant::now();
    cache
        .resolve_active(&roots, &package)
        .expect("selected package cached checkout");
    let checkout_elapsed = checkout_started.elapsed();
    let after_checkout = cache.stats();

    #[cfg(windows)]
    {
        assert_eq!(
            after_checkout.cold_full_hash_passes,
            after_inventory.cold_full_hash_passes
        );
        assert_eq!(
            after_checkout.full_hash_attempts,
            after_inventory.full_hash_attempts
        );
        assert_eq!(
            after_checkout.cached_checkouts,
            after_inventory.cached_checkouts + 1
        );
    }
    eprintln!(
        "inventory_seconds={:.6} selected_checkout_seconds={:.6} full_hash_attempts={} successful_full_hash_passes={}",
        inventory_elapsed.as_secs_f64(),
        checkout_elapsed.as_secs_f64(),
        after_checkout.full_hash_attempts,
        after_checkout.cold_full_hash_passes
    );
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

    let exact_duplicate = raw_archive(
        temp.path(),
        "exact-duplicate.ld",
        &[("same-one.txt", b"first"), ("same-two.txt", b"second")],
    );
    replace_zip_name(&exact_duplicate.0, b"same-two.txt", b"same-one.txt");
    let exact_duplicate_bytes = fs::read(&exact_duplicate.0).expect("read duplicate fixture");
    let exact_duplicate_error =
        inspect(&exact_duplicate.0, Some(&sha256(&exact_duplicate_bytes))).unwrap_err();
    assert_eq!(exact_duplicate_error.code(), ErrorCode::ArchiveInvalid);

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
fn archive_preflight_rejects_an_encrypted_entry_before_reading_payload() {
    let temp = TempDir::new().expect("temp");
    let archive = raw_archive(temp.path(), "encrypted.ld", &[("payload.txt", b"fixture")]);
    mark_zip_entries_encrypted(&archive.0);
    let bytes = fs::read(&archive.0).expect("read encrypted fixture");

    let error = inspect(&archive.0, Some(&sha256(&bytes))).unwrap_err();

    assert_eq!(error.code(), ErrorCode::ArchiveInvalid);
    assert_eq!(error.detail(), "encrypted ZIP entries are forbidden");
}

#[test]
fn archive_preflight_rejects_declared_entry_counts_before_zip_metadata_allocation() {
    let temp = TempDir::new().expect("temp");
    for (name, bytes) in [
        ("too-many-zip32.ldcodec", forged_zip32_eocd(32_769)),
        ("too-many-zip64.ldcodec", forged_zip64_eocd(32_769)),
    ] {
        let path = temp.path().join(name);
        fs::write(&path, &bytes).expect("write forged entry-count archive");
        let error = inspect(&path, Some(&sha256(&bytes)))
            .expect_err("oversized declared entry count must fail before ZIP metadata allocation");
        assert_eq!(error.code(), ErrorCode::ArchiveInvalid);
        assert_eq!(
            error.detail(),
            "ZIP declares more than 32768 entries before metadata allocation"
        );
    }
}

#[test]
fn malformed_tiny_zip64_tail_returns_archive_invalid_without_panicking() {
    let temp = TempDir::new().expect("temp");
    let path = temp.path().join("tiny-malformed.ldcodec");
    let bytes = forged_tiny_zip64_tail();
    fs::write(&path, &bytes).expect("write tiny malformed ZIP64 archive");

    let error = inspect(&path, Some(&sha256(&bytes)))
        .expect_err("tiny malformed ZIP64 metadata must fail closed without panicking");
    assert_eq!(error.code(), ErrorCode::ArchiveInvalid);
}

#[test]
fn archive_preflight_accepts_zip64_metadata_when_entry_count_is_bounded() {
    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("codec-source");
    fs::create_dir(&source).expect("create Codec source");
    write_codec_source(&source, "0.2.0", profile());
    let archive = temp.path().join("bounded-zip64.ldcodec");
    pack_source(&source, &archive);
    promote_archive_end_to_zip64(&archive);
    let bytes = fs::read(&archive).expect("read ZIP64 archive");

    assert_eq!(
        inspect(&archive, Some(&sha256(&bytes)))
            .expect("bounded ZIP64 entry count remains valid")
            .package
            .kind,
        PackageKind::CodecPack
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
fn closed_tree_rejects_unknown_directories_before_descending_or_inflating() {
    let temp = TempDir::new().expect("temp");
    let filesystem_source = temp.path().join("filesystem-source");
    fs::create_dir(&filesystem_source).expect("create filesystem source");
    write_deck_source(&filesystem_source, "0.2.0", 45);
    fs::create_dir_all(filesystem_source.join("unexpected/deep/empty"))
        .expect("create unexpected directory tree");
    let filesystem_error = pack(&PackRequest {
        source_directory: filesystem_source,
        output_path: temp.path().join("filesystem.ld"),
    })
    .expect_err("unknown filesystem directory must fail at its root");
    assert_eq!(filesystem_error.code(), ErrorCode::IntegrityFailed);
    assert_eq!(
        filesystem_error.detail(),
        "package tree contains unexpected or empty directory: unexpected"
    );

    let archive_source = temp.path().join("archive-source");
    fs::create_dir(&archive_source).expect("create archive source");
    write_deck_source(&archive_source, "0.2.0", 45);
    let derived_directory_archive = temp.path().join("derived-directory.ld");
    let (derived_hash, _) = write_unchecked_deck_archive_with_directories(
        &archive_source,
        &derived_directory_archive,
        &["python/"],
    );
    inspect(&derived_directory_archive, Some(&derived_hash))
        .expect("explicit parent implied by a catalogued file remains valid");

    let archive_path = temp.path().join("unexpected-directory.ld");
    let (archive_hash, _) = write_unchecked_deck_archive_with_directories(
        &archive_source,
        &archive_path,
        &["unexpected/"],
    );
    let archive_error = inspect(&archive_path, Some(&archive_hash))
        .expect_err("unknown archive directory must fail before file inflation");
    assert_eq!(archive_error.code(), ErrorCode::ArchiveInvalid);
    assert_eq!(
        archive_error.detail(),
        "ZIP contains unexpected or empty directory: unexpected"
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
    let alternate_profile = ProfileKey {
        codec_family: "synthetic".to_owned(),
        profile: "alternate_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    };
    let deck_manifest_path = deck_source.join("deck-pack.json");
    let mut deck_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&deck_manifest_path).unwrap()).unwrap();
    deck_manifest["signal"]["profile_allowlist"] =
        serde_json::to_value([profile(), alternate_profile.clone()]).unwrap();
    fs::write(&deck_manifest_path, canonical(&deck_manifest)).unwrap();
    let codec_manifest_path = codec_source.join("codec-pack.json");
    let mut codec_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&codec_manifest_path).unwrap()).unwrap();
    codec_manifest["compatibility"]["profiles"] =
        serde_json::to_value([profile(), alternate_profile.clone()]).unwrap();
    fs::write(&codec_manifest_path, canonical(&codec_manifest)).unwrap();
    write_codec_source(
        &incompatible_codec_source,
        "0.2.1",
        ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "other_profile".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
    );
    rewrite_codec_identities(
        &incompatible_codec_source,
        "com.example.codec.other",
        "com.example.adapter.other",
    );
    let deck_archive = temp.path().join("deck.ld");
    let codec_archive = temp.path().join("codec.ldcodec");
    let incompatible_codec_archive = temp.path().join("codec-incompatible.ldcodec");
    let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
    let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
    let (incompatible_codec_hash, _) =
        pack_source(&incompatible_codec_source, &incompatible_codec_archive);
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let deck = install(
        &roots,
        &InstallRequest {
            archive_path: deck_archive,
            expected_sha256: deck_hash,
        },
    )
    .unwrap();
    let codec = install(
        &roots,
        &InstallRequest {
            archive_path: codec_archive,
            expected_sha256: codec_hash,
        },
    )
    .unwrap();
    let incompatible_codec = install(
        &roots,
        &InstallRequest {
            archive_path: incompatible_codec_archive,
            expected_sha256: incompatible_codec_hash,
        },
    )
    .unwrap();
    enable(&roots, &deck.inspection.package).unwrap();
    enable(&roots, &codec.inspection.package).unwrap();
    enable(&roots, &incompatible_codec.inspection.package).unwrap();
    let inventory = inventory(&roots).unwrap();
    assert_eq!(inventory.packages.len(), 3);
    assert_eq!(inventory.packages, list(&roots).unwrap());
    assert_eq!(inventory.matrix, compatibility_matrix(&roots).unwrap());
    let matrix = inventory.matrix;
    assert_eq!(matrix.len(), 2);
    assert_eq!(
        matrix[0].reason,
        latentdeck_extension_manager::CompatibilityReason::Compatible
    );
    assert_eq!(
        matrix[0].compatible_profiles,
        vec![alternate_profile.clone(), profile()]
    );
    assert_eq!(
        matrix[0].compatible_profile,
        Some(alternate_profile),
        "legacy witness remains the first deterministic profile"
    );
    assert_eq!(
        matrix[1].reason,
        latentdeck_extension_manager::CompatibilityReason::UnsupportedProfile
    );
    assert_eq!(matrix[1].compatible_profile, None);
    assert!(matrix[1].compatible_profiles.is_empty());
}

#[test]
#[allow(clippy::too_many_lines)]
fn future_contract_declarations_install_but_matrix_refuses_without_executing_worker() {
    let cases = [
        (
            "protocol",
            "deck_worker_protocol",
            CompatibilityReason::UnsupportedProtocol,
        ),
        (
            "host-api",
            "deck_host_api",
            CompatibilityReason::UnsupportedHostApi,
        ),
        (
            "tensor-abi",
            "deck_tensor_abi",
            CompatibilityReason::UnsupportedTensorAbi,
        ),
        (
            "python-version",
            "deck_python_version",
            CompatibilityReason::UnsupportedTensorAbi,
        ),
        (
            "torch-build",
            "deck_torch_build",
            CompatibilityReason::UnsupportedTensorAbi,
        ),
        (
            "lc-spec",
            "codec_lc_spec",
            CompatibilityReason::UnsupportedProfile,
        ),
        (
            "capability",
            "deck_capability",
            CompatibilityReason::UnsupportedCapability,
        ),
    ];

    for (case_name, mutation, expected) in cases {
        let temp = TempDir::new().expect("temp");
        let deck_source = temp.path().join("deck");
        let codec_source = temp.path().join("codec");
        fs::create_dir(&deck_source).unwrap();
        fs::create_dir(&codec_source).unwrap();
        write_deck_source(&deck_source, "0.2.0", 45);
        write_codec_source_with_worker(
            &codec_source,
            "0.2.0",
            profile(),
            b"test worker bytes that must never execute",
        );

        let deck_manifest_path = deck_source.join("deck-pack.json");
        let codec_manifest_path = codec_source.join("codec-pack.json");
        let mut deck_manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&deck_manifest_path).unwrap()).unwrap();
        let mut codec_manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&codec_manifest_path).unwrap()).unwrap();
        match mutation {
            "deck_worker_protocol" => {
                deck_manifest["compatibility"]["worker_protocol"] = serde_json::json!(3);
            }
            "deck_host_api" => {
                deck_manifest["compatibility"]["deck_host_api"] = serde_json::json!(9);
            }
            "deck_torch_build" => {
                deck_manifest["compatibility"]["torch_exact_build"] =
                    serde_json::json!("2.13.0+cpu");
            }
            "deck_tensor_abi" => {
                deck_manifest["compatibility"]["tensor_abi"] =
                    serde_json::json!("latentdeck.tensor.v9");
            }
            "deck_python_version" => {
                deck_manifest["compatibility"]["python"]["version"] = serde_json::json!("3.14");
            }
            "codec_lc_spec" => {
                codec_manifest["compatibility"]["lc_spec_versions"] = serde_json::json!(["9.0.0"]);
            }
            "deck_capability" => {
                deck_manifest["signal"]["required_capabilities"] =
                    serde_json::json!(["raw_import"]);
            }
            _ => unreachable!(),
        }
        fs::write(&deck_manifest_path, canonical(&deck_manifest)).unwrap();
        fs::write(&codec_manifest_path, canonical(&codec_manifest)).unwrap();

        let deck_archive = temp.path().join("deck.ld");
        let codec_archive = temp.path().join("codec.ldcodec");
        let (deck_hash, _) = pack_source(&deck_source, &deck_archive);
        let (codec_hash, _) = pack_source(&codec_source, &codec_archive);
        let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
        let deck = install(
            &roots,
            &InstallRequest {
                archive_path: deck_archive,
                expected_sha256: deck_hash,
            },
        )
        .unwrap();
        let codec = install(
            &roots,
            &InstallRequest {
                archive_path: codec_archive,
                expected_sha256: codec_hash,
            },
        )
        .unwrap();
        enable(&roots, &deck.inspection.package).unwrap();
        enable(&roots, &codec.inspection.package).unwrap();

        let snapshot = inventory(&roots).unwrap();
        assert_eq!(snapshot.matrix.len(), 1, "{case_name}");
        assert_eq!(snapshot.matrix[0].reason, expected, "{case_name}");
        assert!(
            snapshot.matrix[0].compatible_profiles.is_empty(),
            "{case_name}"
        );
        assert!(
            !temp.path().join("worker-started.marker").exists(),
            "matrix evaluation must never start Codec Pack code: {case_name}"
        );
    }

    for forbidden in ["any", "*"] {
        let temp = TempDir::new().expect("temp");
        let source = temp.path().join("deck");
        fs::create_dir(&source).unwrap();
        write_deck_source(&source, "0.2.0", 45);
        let manifest_path = source.join("deck-pack.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["compatibility"]["tensor_abi"] = serde_json::json!(forbidden);
        fs::write(&manifest_path, canonical(&manifest)).unwrap();
        let error = pack(&PackRequest {
            source_directory: source,
            output_path: temp.path().join("forbidden.ld"),
        })
        .expect_err("open-ended tensor contracts remain forbidden");
        assert_eq!(error.code(), ErrorCode::ManifestInvalid, "{forbidden}");
    }

    let temp = TempDir::new().expect("temp");
    let source = temp.path().join("deck-platform-any");
    fs::create_dir(&source).unwrap();
    write_deck_source(&source, "0.2.0", 45);
    let manifest_path = source.join("deck-pack.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["compatibility"]["python"]["platform_tag"] = serde_json::json!("any");
    fs::write(&manifest_path, canonical(&manifest)).unwrap();
    let error = pack(&PackRequest {
        source_directory: source,
        output_path: temp.path().join("platform-any.ld"),
    })
    .expect_err("open-ended Python platform remains forbidden");
    assert_eq!(error.code(), ErrorCode::ManifestInvalid);
}

#[test]
fn inventory_preserves_untrusted_and_corrupt_matrix_precedence() {
    let temp = TempDir::new().expect("temp");
    let roots = ExtensionRoots::for_base_root(temp.path().join("Local/LatentDeck"));
    let install_fixture = |kind: &str, version: &str| {
        let source = temp.path().join(format!("{kind}-{version}"));
        fs::create_dir(&source).unwrap();
        if kind == "deck" {
            write_deck_source(&source, version, 45);
        } else {
            write_codec_source(&source, version, profile());
        }
        let extension = if kind == "deck" { "ld" } else { "ldcodec" };
        let archive = temp.path().join(format!("{kind}-{version}.{extension}"));
        let (hash, _) = pack_source(&source, &archive);
        install(
            &roots,
            &InstallRequest {
                archive_path: archive,
                expected_sha256: hash,
            },
        )
        .unwrap()
    };

    let healthy_deck = install_fixture("deck", "0.2.0");
    let corrupt_deck = install_fixture("deck", "0.2.1");
    let healthy_codec = install_fixture("codec", "0.2.0");
    let untrusted_codec = install_fixture("codec", "0.2.1");
    enable(&roots, &healthy_deck.inspection.package).unwrap();
    enable(&roots, &healthy_codec.inspection.package).unwrap();
    fs::write(
        corrupt_deck.destination.join("python/deck_operator.py"),
        b"tampered after install",
    )
    .unwrap();
    fs::remove_file(untrusted_codec.trust_receipt_path).unwrap();

    let inventory = inventory(&roots).unwrap();
    let summary = |kind, version| {
        inventory
            .packages
            .iter()
            .find(|summary| {
                summary.package.kind == kind && summary.package.package_version == version
            })
            .unwrap()
    };
    assert_eq!(
        summary(PackageKind::DeckPack, "0.2.1").health,
        PackageHealth::Corrupt
    );
    assert_eq!(
        summary(PackageKind::CodecPack, "0.2.1").health,
        PackageHealth::Untrusted
    );
    assert!(!summary(PackageKind::CodecPack, "0.2.1").enabled);

    let reason = |deck_version, codec_version| {
        inventory
            .matrix
            .iter()
            .find(|pair| {
                pair.deck.package_version == deck_version
                    && pair.codec.package_version == codec_version
            })
            .unwrap()
            .reason
    };
    assert_eq!(
        reason("0.2.0", "0.2.0"),
        latentdeck_extension_manager::CompatibilityReason::Compatible
    );
    assert_eq!(
        reason("0.2.0", "0.2.1"),
        latentdeck_extension_manager::CompatibilityReason::Untrusted
    );
    assert_eq!(
        reason("0.2.1", "0.2.0"),
        latentdeck_extension_manager::CompatibilityReason::PackageInvalid
    );
    assert_eq!(
        reason("0.2.1", "0.2.1"),
        latentdeck_extension_manager::CompatibilityReason::Untrusted
    );
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

fn forged_zip32_eocd(entry_count: u16) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(22);
    bytes.extend_from_slice(b"PK\x05\x06");
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&entry_count.to_le_bytes());
    bytes.extend_from_slice(&entry_count.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn forged_zip64_eocd(entry_count: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(98);
    bytes.extend_from_slice(b"PK\x06\x06");
    bytes.extend_from_slice(&44_u64.to_le_bytes());
    bytes.extend_from_slice(&45_u16.to_le_bytes());
    bytes.extend_from_slice(&45_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&entry_count.to_le_bytes());
    bytes.extend_from_slice(&entry_count.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(b"PK\x06\x07");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(b"PK\x05\x06");
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn forged_tiny_zip64_tail() -> Vec<u8> {
    let mut bytes = b"PK\x06\x06\0\0\0\0\0\0".to_vec();
    bytes.extend_from_slice(b"PK\x06\x07");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u64.to_le_bytes());
    bytes.extend_from_slice(&1_u32.to_le_bytes());
    bytes.extend_from_slice(b"PK\x05\x06");
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u16::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&u32::MAX.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn promote_archive_end_to_zip64(path: &Path) {
    let bytes = fs::read(path).expect("read ZIP32 archive");
    let eocd_offset = bytes
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .expect("find ZIP32 EOCD");
    assert_eq!(
        eocd_offset + 22,
        bytes.len(),
        "fixture must have no ZIP comment"
    );
    let entry_count = u16::from_le_bytes(
        bytes[eocd_offset + 10..eocd_offset + 12]
            .try_into()
            .expect("entry count"),
    );
    let central_size = u32::from_le_bytes(
        bytes[eocd_offset + 12..eocd_offset + 16]
            .try_into()
            .expect("central size"),
    );
    let central_offset = u32::from_le_bytes(
        bytes[eocd_offset + 16..eocd_offset + 20]
            .try_into()
            .expect("central offset"),
    );
    let mut promoted = bytes[..eocd_offset].to_vec();
    promoted.extend_from_slice(b"PK\x06\x06");
    promoted.extend_from_slice(&44_u64.to_le_bytes());
    promoted.extend_from_slice(&45_u16.to_le_bytes());
    promoted.extend_from_slice(&45_u16.to_le_bytes());
    promoted.extend_from_slice(&0_u32.to_le_bytes());
    promoted.extend_from_slice(&0_u32.to_le_bytes());
    promoted.extend_from_slice(&u64::from(entry_count).to_le_bytes());
    promoted.extend_from_slice(&u64::from(entry_count).to_le_bytes());
    promoted.extend_from_slice(&u64::from(central_size).to_le_bytes());
    promoted.extend_from_slice(&u64::from(central_offset).to_le_bytes());
    promoted.extend_from_slice(b"PK\x06\x07");
    promoted.extend_from_slice(&0_u32.to_le_bytes());
    promoted.extend_from_slice(&(eocd_offset as u64).to_le_bytes());
    promoted.extend_from_slice(&1_u32.to_le_bytes());
    promoted.extend_from_slice(b"PK\x05\x06");
    promoted.extend_from_slice(&0_u16.to_le_bytes());
    promoted.extend_from_slice(&0_u16.to_le_bytes());
    promoted.extend_from_slice(&u16::MAX.to_le_bytes());
    promoted.extend_from_slice(&u16::MAX.to_le_bytes());
    promoted.extend_from_slice(&u32::MAX.to_le_bytes());
    promoted.extend_from_slice(&u32::MAX.to_le_bytes());
    promoted.extend_from_slice(&0_u16.to_le_bytes());
    fs::write(path, promoted).expect("write ZIP64 archive");
}

fn mark_zip_entries_encrypted(path: &Path) {
    let mut bytes = fs::read(path).expect("read ZIP fixture");
    let mut local_headers = 0;
    let mut central_headers = 0;
    for offset in 0..bytes.len().saturating_sub(3) {
        if bytes[offset..].starts_with(b"PK\x03\x04") {
            bytes[offset + 6] |= 1;
            local_headers += 1;
        } else if bytes[offset..].starts_with(b"PK\x01\x02") {
            bytes[offset + 8] |= 1;
            central_headers += 1;
        }
    }
    assert_eq!(local_headers, 1, "fixture must have one local header");
    assert_eq!(central_headers, 1, "fixture must have one central header");
    fs::write(path, bytes).expect("write encrypted ZIP fixture");
}

fn replace_zip_name(path: &Path, old: &[u8], new: &[u8]) {
    assert_eq!(
        old.len(),
        new.len(),
        "ZIP names must keep their encoded size"
    );
    let mut bytes = fs::read(path).expect("read ZIP fixture");
    let mut replacements = 0;
    for offset in 0..=bytes.len().saturating_sub(old.len()) {
        if bytes[offset..].starts_with(old) {
            bytes[offset..offset + old.len()].copy_from_slice(new);
            replacements += 1;
        }
    }
    assert_eq!(
        replacements, 2,
        "fixture name must occur in local and central headers"
    );
    fs::write(path, bytes).expect("write duplicate-name ZIP fixture");
}
