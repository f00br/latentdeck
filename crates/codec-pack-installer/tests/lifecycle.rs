use std::fs;
use std::path::{Path, PathBuf};

use latentdeck_codec_pack_installer::{
    EXIT_IN_USE, InstallRequest, LifecycleRoots, install, uninstall,
};
use latentdeck_extension_manager::{
    Architecture, BundledPackageEntry, BundledPackageIndex, CodecAdapterDescriptor,
    CodecCapability, CodecCompatibility, CodecPackManifest, CodecWorkerDescriptor, ErrorCode,
    ExternalAssetDescriptor, InstallRequest as ExtensionInstallRequest, IntegrityCatalog,
    IntegrityDescriptor, IntegrityFile, LicenseDescriptor, OperatingSystem, PackRequest,
    PackageKind, PackageReference, PlatformDescriptor, ProfileKey, PublisherDescriptor,
    PublisherIdentityClaim, PythonConstraint, PythonImplementation, RuntimeLockDescriptor, disable,
    enable, install_from_bundled_index, pack,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn canonical(value: &impl Serialize) -> Vec<u8> {
    serde_jcs::to_vec(value).expect("canonical JSON")
}

fn h3_profile() -> ProfileKey {
    ProfileKey {
        codec_family: "minimax_h3".to_owned(),
        profile: "h3_av_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    }
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let destination = root.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    fs::create_dir_all(destination.parent().expect("file parent")).expect("create parent");
    fs::write(destination, bytes).expect("write fixture file");
}

#[allow(clippy::too_many_lines)] // One closed manifest is clearer as one test fixture.
fn write_h3_source(root: &Path, adapter_version: &str, profile: ProfileKey, asset_sha256: &str) {
    let notice = b"Synthetic public test notice\n".as_slice();
    let adapter = b"def make_adapter():\n    raise RuntimeError('test-only')\n".as_slice();
    let worker = b"synthetic python executable".as_slice();
    let runtime_lock = b"python=3.13.14\ntorch=2.13.0+cu130\n".as_slice();
    let payloads: Vec<(&str, &[u8])> = vec![
        ("THIRD_PARTY_NOTICES.md", notice),
        ("runtime/latentdeck_codec_h3/adapter.py", adapter),
        ("runtime/python.exe", worker),
        ("runtime/runtime.lock", runtime_lock),
    ];
    let files: Vec<IntegrityFile> = payloads
        .iter()
        .map(|(path, bytes)| IntegrityFile {
            path: (*path).to_owned(),
            byte_length: u64::try_from(bytes.len()).expect("fixture length"),
            sha256: sha256(bytes),
        })
        .collect();
    let catalog = IntegrityCatalog {
        manifest_version: "1.0.0".to_owned(),
        files,
    };
    let catalog_bytes = canonical(&catalog);
    let manifest = CodecPackManifest {
        manifest_version: "2.0.0".to_owned(),
        kind: PackageKind::CodecPack,
        pack_id: "org.latentdeck.h3".to_owned(),
        pack_version: "0.2.0".to_owned(),
        display_name: "LatentDeck H3 Codec Pack".to_owned(),
        summary: "MiniMax H3 adapter and isolated runtime for LatentDeck.".to_owned(),
        publisher: PublisherDescriptor {
            name: "LatentDeck Project".to_owned(),
            url: None,
            identity_claim: PublisherIdentityClaim::SelfDeclared,
        },
        license: LicenseDescriptor {
            spdx_or_label: "SEE-NOTICES".to_owned(),
            notice_path: "THIRD_PARTY_NOTICES.md".to_owned(),
        },
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
            python: PythonConstraint {
                implementation: PythonImplementation::Cpython,
                version: "3.13".to_owned(),
                platform_tag: "win_amd64".to_owned(),
            },
            torch_exact_build: "2.13.0+cu130".to_owned(),
            lc_spec_versions: vec!["0.1.0".to_owned()],
            profiles: vec![profile],
        },
        adapter: CodecAdapterDescriptor {
            adapter_id: "org.latentdeck.h3".to_owned(),
            adapter_version: adapter_version.to_owned(),
            entrypoint: "latentdeck_codec_h3.adapter:make_adapter".to_owned(),
        },
        worker: CodecWorkerDescriptor {
            executable: "runtime/python.exe".to_owned(),
            arguments: vec![
                "-I".to_owned(),
                "-s".to_owned(),
                "-B".to_owned(),
                "-m".to_owned(),
                "latentdeck_codec_host".to_owned(),
            ],
            working_directory: "runtime".to_owned(),
            start_timeout_ms: 120_000,
            heartbeat_timeout_ms: 5_000,
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
            asset_id: "taeh3".to_owned(),
            display_name: "TAEH3 decoder weight".to_owned(),
            required: true,
            byte_length: 22_709_752,
            sha256: asset_sha256.to_owned(),
            source_url: Some(
                "https://huggingface.co/madebyollin/taehv/resolve/main/taeh3.safetensors"
                    .to_owned(),
            ),
            license_label: "MIT".to_owned(),
            license_url: Some(
                "https://github.com/madebyollin/taehv/blob/e743234f/LICENSE".to_owned(),
            ),
        }],
        runtime_lock: RuntimeLockDescriptor {
            path: "runtime/runtime.lock".to_owned(),
            sha256: sha256(runtime_lock),
        },
        integrity: IntegrityDescriptor {
            catalog_path: "integrity.json".to_owned(),
            catalog_sha256: sha256(&catalog_bytes),
        },
    };
    for (path, bytes) in payloads {
        write_file(root, path, bytes);
    }
    write_file(root, "integrity.json", &catalog_bytes);
    write_file(root, "codec-pack.json", &canonical(&manifest));
}

fn package(
    temp: &TempDir,
    name: &str,
    adapter_version: &str,
    profile: ProfileKey,
    asset_sha256: &str,
) -> (PathBuf, String, u64) {
    let source = temp.path().join(format!("{name}-source"));
    fs::create_dir(&source).expect("source root");
    write_h3_source(&source, adapter_version, profile, asset_sha256);
    let archive = temp.path().join(format!("{name}.ldcodec"));
    let receipt = pack(&PackRequest {
        source_directory: source,
        output_path: archive.clone(),
    })
    .expect("pack fixture");
    (
        archive,
        receipt.inspection.archive_sha256,
        receipt.inspection.archive_byte_length,
    )
}

fn request(archive: PathBuf, _hash: String, _length: u64) -> InstallRequest {
    InstallRequest {
        archive_path: archive,
    }
}

fn package_reference() -> PackageReference {
    PackageReference {
        kind: PackageKind::CodecPack,
        package_id: "org.latentdeck.h3".to_owned(),
        package_version: "0.2.0".to_owned(),
    }
}

#[test]
fn user_supplied_exact_identity_never_authorizes_the_reserved_h3_namespace() {
    let temp = TempDir::new().expect("temp");
    let roots = LifecycleRoots::from_known_folders(
        temp.path().join("Local"),
        temp.path().join("ProgramData"),
    );
    let (archive, hash, length) = package(
        &temp,
        "untrusted-user-input",
        "0.2.0",
        h3_profile(),
        "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
    );

    let error = install(&roots, &request(archive, hash, length))
        .expect_err("runtime arguments cannot authorize org.latentdeck.h3");
    assert_eq!(error.code(), ErrorCode::PackageUntrusted);
}

#[test]
fn helper_without_build_authorization_does_not_create_install_roots() {
    let temp = TempDir::new().expect("temp");
    let local_app_data = temp.path().join("Local");
    let program_data = temp.path().join("ProgramData");
    let roots = LifecycleRoots::from_known_folders(&local_app_data, &program_data);
    let (archive, hash, length) = package(
        &temp,
        "h3",
        "0.2.0",
        h3_profile(),
        "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
    );

    let error = install(&roots, &request(archive, hash, length))
        .expect_err("default developer helper has an empty authorization allowlist");
    assert_eq!(error.code(), ErrorCode::PackageUntrusted);
    assert!(!local_app_data.join("LatentDeck/CodecPacks").exists());
    assert!(!program_data.join("LatentDeck").exists());
}

#[test]
fn wrapper_preserves_common_active_package_gate() {
    let temp = TempDir::new().expect("temp");
    let roots = LifecycleRoots::from_known_folders(
        temp.path().join("Local"),
        temp.path().join("ProgramData"),
    );
    let (archive, hash, _length) = package(
        &temp,
        "active",
        "0.2.0",
        h3_profile(),
        "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
    );
    let index = BundledPackageIndex {
        index_version: "1.0.0".to_owned(),
        packages: vec![BundledPackageEntry {
            package: package_reference(),
            archive_sha256: hash.clone(),
        }],
    };
    install_from_bundled_index(
        roots.extension_roots(),
        &ExtensionInstallRequest {
            archive_path: archive,
            expected_sha256: hash,
        },
        &index,
    )
    .expect("common lifecycle fixture install");
    enable(roots.extension_roots(), &package_reference()).expect("enable exact version");

    let error = uninstall(&roots, "0.2.0", false).expect_err("active remove rejected");
    assert_eq!(error.code(), ErrorCode::PackageActive);
    assert_eq!(error.exit_code(), EXIT_IN_USE);

    disable(roots.extension_roots(), &package_reference()).expect("disable exact version");
    uninstall(&roots, "0.2.0", false).expect("remove disabled exact version");
}

#[test]
fn non_allowlisted_h3_variants_fail_as_untrusted_before_reserved_install() {
    let temp = TempDir::new().expect("temp");
    let roots = LifecycleRoots::from_known_folders(
        temp.path().join("Local"),
        temp.path().join("ProgramData"),
    );
    let (old_adapter, hash, length) = package(
        &temp,
        "old-adapter",
        "0.1.0",
        h3_profile(),
        "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
    );
    let error = install(&roots, &request(old_adapter, hash, length))
        .expect_err("old adapter bytes are not build-authorized");
    assert_eq!(error.code(), ErrorCode::PackageUntrusted);

    let (wrong_asset, hash, length) = package(
        &temp,
        "wrong-asset",
        "0.2.0",
        h3_profile(),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let error = install(&roots, &request(wrong_asset, hash, length))
        .expect_err("non-exact asset variant is not build-authorized");
    assert_eq!(error.code(), ErrorCode::PackageUntrusted);
}

#[test]
fn wrapper_requires_ldcodec_and_unknown_valid_hash_is_untrusted() {
    let temp = TempDir::new().expect("temp");
    let roots = LifecycleRoots::from_known_folders(
        temp.path().join("Local"),
        temp.path().join("ProgramData"),
    );
    let (archive, hash, length) = package(
        &temp,
        "identity",
        "0.2.0",
        h3_profile(),
        "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
    );
    let zip_path = archive.with_extension("zip");
    fs::rename(&archive, &zip_path).expect("rename fixture");
    let error = install(&roots, &request(zip_path, hash, length))
        .expect_err("legacy zip extension rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArguments);

    let (wrong_profile, hash, length) = package(
        &temp,
        "wrong-profile",
        "0.2.0",
        ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "other".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
        "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
    );
    let error = install(&roots, &request(wrong_profile, hash, length))
        .expect_err("unknown valid archive hash is not build-authorized");
    assert_eq!(error.code(), ErrorCode::PackageUntrusted);
}
