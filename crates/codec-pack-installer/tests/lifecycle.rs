use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use latentdeck_codec_pack_installer::{
    EXIT_ALREADY_INSTALLED, EXIT_CONFLICT, EXIT_INVALID_PACK, EXIT_NOT_INSTALLED, InstallRequest,
    LifecycleError, LifecycleRoots, install, uninstall,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_pack_archive(root: &Path, version: &str) -> (PathBuf, String, u64) {
    write_pack_archive_as(
        root,
        &format!("pack-{version}.zip"),
        version,
        b"synthetic worker",
        zip::CompressionMethod::Deflated,
        false,
    )
}

fn write_pack_archive_as(
    root: &Path,
    archive_name: &str,
    version: &str,
    worker: &[u8],
    compression: zip::CompressionMethod,
    reverse_entries: bool,
) -> (PathBuf, String, u64) {
    let notice = b"synthetic notice";
    let catalog = json!({
        "manifest_version": "1.0.0",
        "files": [
            {"path": "NOTICE.txt", "byte_length": notice.len(), "sha256": sha256(notice)},
            {"path": "bin/worker.exe", "byte_length": worker.len(), "sha256": sha256(worker)}
        ]
    });
    let catalog_bytes = serde_json::to_vec(&catalog).expect("catalog json");
    let manifest = json!({
        "manifest_version": "1.0.0",
        "pack_id": "org.latentdeck.h3",
        "pack_version": version,
        "display_name": "Synthetic H3 Codec Pack",
        "publisher": {"name": "LatentDeck Test", "url": null},
        "license": {"spdx_or_label": "MIT", "notice_path": "NOTICE.txt"},
        "platform": {"os": "windows", "arch": "x86_64"},
        "compatibility": {
            "app_min_inclusive": "0.1.0",
            "app_max_exclusive": "0.2.0",
            "worker_protocol_min": 1,
            "worker_protocol_max": 1,
            "lc_spec_versions": ["0.1.0"],
            "profiles": [{
                "codec_family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_versions": ["0.1.0"]
            }]
        },
        "worker": {
            "executable": "bin/worker.exe",
            "arguments": ["--worker"],
            "d2_arguments": ["--d2-worker"],
            "q4_arguments": ["--q4-worker"],
            "working_directory": "bin",
            "probe_timeout_ms": 5000
        },
        "adapter": {"adapter_id": "org.latentdeck.h3", "adapter_version": "0.1.0"},
        "integrity": {"catalog_path": "catalog.json", "catalog_sha256": sha256(&catalog_bytes)},
        "external_assets": [{
            "asset_id": "taeh3",
            "display_name": "TAEH3 decoder weight",
            "kind": "decoder_weight",
            "required": true,
            "selection": "explicit_file",
            "format": "safetensors",
            "accepted_variants": [{
                "variant_id": "synthetic",
                "sha256": "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
                "byte_length": 22_709_752,
                "source_url": "https://example.invalid/taeh3",
                "license_label": "MIT",
                "license_url": "https://example.invalid/license"
            }]
        }]
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("manifest json");
    let archive_path = root.join(archive_name);
    let file = File::create(&archive_path).expect("archive");
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(compression);
    let mut entries = vec![
        ("NOTICE.txt", notice.as_slice()),
        ("bin/worker.exe", worker),
        ("catalog.json", catalog_bytes.as_slice()),
        ("codec-pack.json", manifest_bytes.as_slice()),
    ];
    if reverse_entries {
        entries.reverse();
    }
    for (name, bytes) in entries {
        zip.start_file(name, options).expect("zip entry");
        zip.write_all(bytes).expect("zip bytes");
    }
    zip.finish().expect("finish zip");
    let bytes = fs::read(&archive_path).expect("archive bytes");
    let length = u64::try_from(bytes.len()).expect("archive length");
    (archive_path, sha256(&bytes), length)
}

#[test]
fn existing_version_still_requires_the_exact_bound_archive_and_a_healthy_tree() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let request = request(archive.clone(), hash, length, "0.1.1");
    let original_archive = fs::read(&archive).expect("archive backup");
    install(&roots, &request).expect("install healthy version");

    fs::write(&archive, b"tampered adjacent archive").expect("tamper archive");
    let error = install(&roots, &request).expect_err("installed version cannot bypass binding");
    assert!(matches!(error, LifecycleError::ArchiveInvalid(_)));
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);

    fs::write(&archive, original_archive).expect("restore bound archive");
    fs::write(
        local.join("org.latentdeck.h3/0.1.1/bin/worker.exe"),
        b"tampered installed worker",
    )
    .expect("tamper installed tree");
    let error = install(&roots, &request).expect_err("corrupt installed tree rejected");
    assert!(matches!(error, LifecycleError::PackInvalid(_)));
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);
}

#[test]
fn same_version_requires_proven_archive_tree_equivalence() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local, None);
    let (original, original_hash, original_length) = write_pack_archive(temp.path(), "0.1.1");
    install(
        &roots,
        &request(original, original_hash, original_length, "0.1.1"),
    )
    .expect("install original tree");

    let (equivalent, equivalent_hash, equivalent_length) = write_pack_archive_as(
        temp.path(),
        "equivalent-repacked.zip",
        "0.1.1",
        b"synthetic worker",
        zip::CompressionMethod::Stored,
        true,
    );
    let error = install(
        &roots,
        &request(equivalent, equivalent_hash, equivalent_length, "0.1.1"),
    )
    .expect_err("equivalent tree remains an immutable installed version");
    assert_eq!(error.exit_code(), EXIT_ALREADY_INSTALLED);

    let (different, different_hash, different_length) = write_pack_archive_as(
        temp.path(),
        "different-valid-tree.zip",
        "0.1.1",
        b"different valid worker",
        zip::CompressionMethod::Deflated,
        false,
    );
    let error = install(
        &roots,
        &request(different, different_hash, different_length, "0.1.1"),
    )
    .expect_err("different valid tree cannot alias the installed version");
    assert!(matches!(error, LifecycleError::Conflict(_)));
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
}

#[test]
fn cross_scope_duplicate_wins_over_a_healthy_local_already_installed_result() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let program = temp.path().join("ProgramData/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local, Some(program.clone()));
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let request = request(archive, hash, length, "0.1.1");
    install(&roots, &request).expect("install healthy local version");
    fs::create_dir_all(program.join("org.latentdeck.h3/0.1.1"))
        .expect("create conflicting all-users version");

    let error = install(&roots, &request).expect_err("cross-scope duplicate rejected first");
    assert!(matches!(error, LifecycleError::Conflict(_)));
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
}

#[test]
fn stable_version_never_aliases_or_removes_a_prerelease_quarantine() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let request = request(archive, hash, length, "0.1.1");
    install(&roots, &request).expect("install stable version");

    let prerelease_quarantine = roots
        .trash_root
        .join(".remove-org.latentdeck.h3-0.1.1-alpha-deadbeef");
    fs::create_dir_all(&prerelease_quarantine).expect("prerelease quarantine fixture");
    fs::write(prerelease_quarantine.join("keep.txt"), b"keep").expect("quarantine sentinel");

    uninstall(&roots, "0.1.1", false).expect("remove only stable version");
    assert!(prerelease_quarantine.join("keep.txt").is_file());
    install(&roots, &request).expect("prerelease quarantine does not block stable install");
    assert!(prerelease_quarantine.join("keep.txt").is_file());
    assert!(local.join("org.latentdeck.h3/0.1.1").is_dir());
}

#[test]
fn install_cleans_only_its_exact_version_quarantine_before_publication() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let stable_quarantine =
        exact_quarantine_path(&roots, "0.1.1", "00000000000000000000000000000001");
    let prerelease_quarantine =
        exact_quarantine_path(&roots, "0.1.1-alpha", "00000000000000000000000000000002");
    fs::create_dir_all(&stable_quarantine).expect("stable quarantine fixture");
    fs::create_dir_all(&prerelease_quarantine).expect("prerelease quarantine fixture");
    fs::write(stable_quarantine.join("stale.bin"), b"stale").expect("stable sentinel");
    fs::write(prerelease_quarantine.join("keep.bin"), b"keep").expect("prerelease sentinel");
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");

    install(&roots, &request(archive, hash, length, "0.1.1"))
        .expect("exact stale quarantine is recovered before install");

    assert!(!stable_quarantine.exists());
    assert!(prerelease_quarantine.join("keep.bin").is_file());
    assert!(local.join("org.latentdeck.h3/0.1.1").is_dir());
}

#[cfg(windows)]
#[test]
fn locked_exact_quarantine_returns_50_then_retry_cleans_and_installs() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let quarantine = exact_quarantine_path(&roots, "0.1.1", "00000000000000000000000000000001");
    fs::create_dir_all(&quarantine).expect("quarantine fixture");
    let held_path = quarantine.join("worker.exe");
    fs::write(&held_path, b"held").expect("held quarantine file");
    let held = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&held_path)
        .expect("hold quarantine file without delete sharing");
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let request = request(archive, hash, length, "0.1.1");

    let error = install(&roots, &request).expect_err("locked rollback quarantine blocks cleanup");
    assert_eq!(error.exit_code(), 50);
    assert!(quarantine.is_dir());
    assert!(!local.join("org.latentdeck.h3/0.1.1").exists());

    drop(held);
    install(&roots, &request).expect("retry recovers quarantine and installs");
    assert!(!quarantine.exists());
    assert!(local.join("org.latentdeck.h3/0.1.1").is_dir());
}

#[test]
fn next_install_cleans_only_exact_stale_staging_directories() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local, None);
    let stale = roots
        .staging_root
        .join(".install-00000000000000000000000000000001");
    let similarly_named = roots.staging_root.join(".install-owner-notes");
    let outside = roots
        .staging_root
        .parent()
        .expect("lifecycle parent")
        .join("keep-outside.txt");
    fs::create_dir_all(&stale).expect("stale staging");
    fs::write(stale.join("partial.bin"), b"partial").expect("stale payload");
    fs::create_dir_all(&similarly_named).expect("similar non-lifecycle directory");
    fs::write(similarly_named.join("keep.txt"), b"keep").expect("similar sentinel");
    fs::write(&outside, b"keep").expect("outside sentinel");
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");

    install(&roots, &request(archive, hash, length, "0.1.1"))
        .expect("install recovers stale extraction");

    assert!(!stale.exists());
    assert!(similarly_named.join("keep.txt").is_file());
    assert!(outside.is_file());
}

#[test]
fn uninstall_exact_destination_probe_errors_fail_closed_instead_of_exit_31() {
    let temp = TempDir::new().expect("temp");
    let invalid_install_root = temp.path().join("invalid\0CodecPacks");
    let roots = LifecycleRoots::for_install_root(invalid_install_root, None);

    let error = uninstall(&roots, "0.1.1", false)
        .expect_err("invalid exact destination probe must not become not-installed");

    assert!(matches!(error, LifecycleError::Conflict(_)));
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
}

#[test]
fn uninstall_quarantine_root_probe_errors_fail_closed_instead_of_exit_31() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("CodecPacks");
    let mut roots = LifecycleRoots::for_install_root(local, None);
    roots.trash_root = temp.path().join("invalid\0CodecPackTrash");

    let error = uninstall(&roots, "0.1.1", false)
        .expect_err("invalid quarantine-root probe must not become not-installed");

    assert!(matches!(error, LifecycleError::Conflict(_)));
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
}

fn request(path: PathBuf, hash: String, length: u64, version: &str) -> InstallRequest {
    InstallRequest {
        archive_path: path,
        expected_sha256: hash,
        expected_length: length,
        expected_version: version.to_owned(),
    }
}

fn exact_quarantine_path(roots: &LifecycleRoots, version: &str, token: &str) -> PathBuf {
    roots.trash_root.join(format!(
        ".remove-org.latentdeck.h3-v{}-{version}-{token}",
        version.len()
    ))
}

fn write_raw_archive(root: &Path, name: &str, entries: &[(&str, &[u8])]) -> (PathBuf, String, u64) {
    let archive_path = root.join(name);
    let file = File::create(&archive_path).expect("raw archive");
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default();
    for (entry_name, bytes) in entries {
        zip.start_file(*entry_name, options).expect("raw entry");
        zip.write_all(bytes).expect("raw bytes");
    }
    zip.finish().expect("finish raw zip");
    let bytes = fs::read(&archive_path).expect("raw archive bytes");
    let length = u64::try_from(bytes.len()).expect("raw archive length");
    (archive_path, sha256(&bytes), length)
}

fn write_symlink_archive(root: &Path) -> (PathBuf, String, u64) {
    let archive_path = root.join("symlink.zip");
    let file = File::create(&archive_path).expect("symlink archive");
    let mut zip = zip::ZipWriter::new(file);
    zip.add_symlink("runtime/link", "../outside", SimpleFileOptions::default())
        .expect("symlink entry");
    zip.finish().expect("finish symlink zip");
    let bytes = fs::read(&archive_path).expect("symlink archive bytes");
    let length = u64::try_from(bytes.len()).expect("symlink archive length");
    (archive_path, sha256(&bytes), length)
}

#[test]
fn installs_a_hash_bound_pack_only_after_full_validation() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let program = temp.path().join("ProgramData/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), Some(program));
    let (archive_path, expected_sha256, expected_length) = write_pack_archive(temp.path(), "0.1.1");

    let receipt = install(
        &roots,
        &InstallRequest {
            archive_path,
            expected_sha256: expected_sha256.clone(),
            expected_length,
            expected_version: "0.1.1".to_owned(),
        },
    )
    .expect("valid pack installs");

    assert_eq!(receipt.destination, local.join("org.latentdeck.h3/0.1.1"));
    assert_eq!(receipt.archive_sha256, expected_sha256);
    assert!(receipt.destination.join("codec-pack.json").is_file());
    assert!(!roots.staging_root.exists());
}

#[test]
fn rejects_wrong_identity_and_unsafe_zip_names_before_discovery() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let (archive_path, expected_sha256, expected_length) = write_pack_archive(temp.path(), "0.1.1");
    let wrong_hash = "0".repeat(64);

    let error = install(
        &roots,
        &request(archive_path, wrong_hash, expected_length, "0.1.1"),
    )
    .expect_err("wrong hash rejected");
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);
    assert!(!local.join("org.latentdeck.h3/0.1.1").exists());

    let (duplicates, hash, length) = write_raw_archive(
        temp.path(),
        "duplicates.zip",
        &[("A.txt", b"a"), ("a.txt", b"b")],
    );
    let error = install(&roots, &request(duplicates, hash, length, "0.1.1"))
        .expect_err("case aliases rejected");
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);

    let (traversal, hash, length) =
        write_raw_archive(temp.path(), "traversal.zip", &[("../escape", b"x")]);
    let error = install(&roots, &request(traversal, hash, length, "0.1.1"))
        .expect_err("traversal rejected");
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);
    assert!(!temp.path().join("escape").exists());

    let (symlink, hash, length) = write_symlink_archive(temp.path());
    let error = install(&roots, &request(symlink, hash, length, "0.1.1"))
        .expect_err("archive symlink rejected");
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);

    for (archive_name, entry_name) in [
        ("forbidden-windows-character.zip", "runtime/bad?.bin"),
        ("windows-control-character.zip", "runtime/bad\u{1}.bin"),
        ("windows-superscript-device.zip", "runtime/COM¹.txt"),
    ] {
        let (archive, hash, length) =
            write_raw_archive(temp.path(), archive_name, &[(entry_name, b"x")]);
        let error = install(&roots, &request(archive, hash, length, "0.1.1"))
            .expect_err("unsafe Windows archive name rejected");
        assert_eq!(error.exit_code(), EXIT_INVALID_PACK);
        let LifecycleError::ArchiveInvalid(detail) = error else {
            panic!("unsafe Windows name must fail archive preflight");
        };
        assert!(detail.contains("unsafe Windows path component"));
    }

    let (wrong_version, hash, length) = write_pack_archive(temp.path(), "0.1.0");
    let error = install(&roots, &request(wrong_version, hash, length, "0.1.2"))
        .expect_err("manifest version mismatch rejected");
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);
    assert!(!local.join("org.latentdeck.h3/0.1.2").exists());
    assert!(!roots.staging_root.exists());
    assert_eq!(expected_sha256.len(), 64);
}

#[test]
fn preserves_side_by_side_versions_and_removes_only_the_requested_version() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let program = temp.path().join("ProgramData/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), Some(program));
    let (archive_010, hash_010, length_010) = write_pack_archive(temp.path(), "0.1.0");
    let (archive_011, hash_011, length_011) = write_pack_archive(temp.path(), "0.1.1");
    let request_010 = request(archive_010, hash_010, length_010, "0.1.0");
    let request_011 = request(archive_011, hash_011, length_011, "0.1.1");

    install(&roots, &request_010).expect("install 0.1.0");
    install(&roots, &request_011).expect("install 0.1.1");
    let error = install(&roots, &request_011).expect_err("immutable overwrite refused");
    assert_eq!(error.exit_code(), EXIT_ALREADY_INSTALLED);

    let removed = uninstall(&roots, "0.1.0", false).expect("remove exact old version");
    assert_eq!(removed.removed_version, "0.1.0");
    assert!(!local.join("org.latentdeck.h3/0.1.0").exists());
    assert!(local.join("org.latentdeck.h3/0.1.1").is_dir());
}

#[test]
fn rejects_cross_scope_duplicates_and_a_seventeenth_version() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let program = temp.path().join("ProgramData/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), Some(program.clone()));
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let request = request(archive, hash, length, "0.1.1");

    fs::create_dir_all(program.join("org.latentdeck.h3/0.1.1")).expect("other scope fixture");
    let error = install(&roots, &request).expect_err("cross-scope duplicate rejected");
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
    fs::remove_dir_all(&program).expect("remove conflict fixture");

    let pack_parent = local.join("org.latentdeck.h3");
    for patch in 0..16 {
        fs::create_dir_all(pack_parent.join(format!("0.0.{patch}"))).expect("version fixture");
    }
    let error = install(&roots, &request).expect_err("seventeenth version rejected");
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
    assert!(!pack_parent.join("0.1.1").exists());
}

#[test]
fn operating_system_lock_is_busy_only_while_the_owner_handle_lives() {
    use fs2::FileExt;

    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local, None);
    fs::create_dir_all(roots.lock_path.parent().expect("lock parent")).expect("lock parent");
    let lock_file = File::create(&roots.lock_path).expect("lock file");
    lock_file.try_lock_exclusive().expect("hold lifecycle lock");
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let request = request(archive, hash, length, "0.1.1");

    let error = install(&roots, &request).expect_err("live OS lock serializes lifecycle");
    assert_eq!(error.exit_code(), EXIT_CONFLICT);
    drop(lock_file);

    install(&roots, &request).expect("OS releases lock when owner handle closes");
}

#[test]
fn corrupt_removal_is_explicit_and_does_not_touch_another_version() {
    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let (archive_010, hash_010, length_010) = write_pack_archive(temp.path(), "0.1.0");
    let (archive_011, hash_011, length_011) = write_pack_archive(temp.path(), "0.1.1");
    install(&roots, &request(archive_010, hash_010, length_010, "0.1.0")).expect("install 0.1.0");
    install(&roots, &request(archive_011, hash_011, length_011, "0.1.1")).expect("install 0.1.1");
    fs::write(
        local.join("org.latentdeck.h3/0.1.1/bin/worker.exe"),
        b"corrupt",
    )
    .expect("corrupt worker");

    let error = uninstall(&roots, "0.1.1", false).expect_err("healthy removal fails closed");
    assert_eq!(error.exit_code(), EXIT_INVALID_PACK);
    assert!(local.join("org.latentdeck.h3/0.1.1").is_dir());

    uninstall(&roots, "0.1.1", true).expect("explicit corrupt removal");
    assert!(!local.join("org.latentdeck.h3/0.1.1").exists());
    assert!(local.join("org.latentdeck.h3/0.1.0").is_dir());
    let error = uninstall(&roots, "0.1.1", true).expect_err("missing exact version");
    assert_eq!(error.exit_code(), EXIT_NOT_INSTALLED);
}

#[cfg(windows)]
#[test]
fn an_in_use_version_returns_exit_50_and_a_retry_finishes_cleanup() {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    let temp = TempDir::new().expect("temp");
    let local = temp.path().join("Local/LatentDeck/CodecPacks");
    let roots = LifecycleRoots::for_install_root(local.clone(), None);
    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    install(&roots, &request(archive, hash, length, "0.1.1")).expect("install pack");
    let worker_path = local.join("org.latentdeck.h3/0.1.1/bin/worker.exe");
    let locked_worker = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&worker_path)
        .expect("lock worker file");

    let error = uninstall(&roots, "0.1.1", false).expect_err("locked worker blocks cleanup");
    assert_eq!(error.exit_code(), 50);
    drop(locked_worker);

    uninstall(&roots, "0.1.1", false).expect("retry removes or cleans quarantine");
    assert!(!local.join("org.latentdeck.h3/0.1.1").exists());
    assert!(!roots.trash_root.exists());
}

#[test]
fn cli_uses_stable_exit_codes_and_one_line_diagnostics() {
    let temp = TempDir::new().expect("temp");
    let executable = env!("CARGO_BIN_EXE_latentdeck-codec-pack-installer");
    let invalid = std::process::Command::new(executable)
        .output()
        .expect("run invalid CLI");
    assert_eq!(invalid.status.code(), Some(10));
    assert_eq!(String::from_utf8_lossy(&invalid.stderr).lines().count(), 1);

    let local = temp.path().join("Local");
    let program = temp.path().join("ProgramData");
    let ambient_local = temp.path().join("AmbientLocal");
    let ambient_program = temp.path().join("AmbientProgramData");
    let ambient_only = std::process::Command::new(executable)
        .args(["uninstall", "--version", "0.1.1"])
        .env("LOCALAPPDATA", &ambient_local)
        .env("PROGRAMDATA", &ambient_program)
        .output()
        .expect("run ambient-only uninstall");
    assert_eq!(ambient_only.status.code(), Some(10));

    let roots_args = vec![
        "--local-app-data".to_owned(),
        local.to_string_lossy().into_owned(),
        "--program-data".to_owned(),
        program.to_string_lossy().into_owned(),
    ];
    let mut missing_args = roots_args.clone();
    missing_args.extend([
        "uninstall".to_owned(),
        "--version".to_owned(),
        "0.1.1".to_owned(),
    ]);
    let missing = std::process::Command::new(executable)
        .args(&missing_args)
        .env("LOCALAPPDATA", &ambient_local)
        .env("PROGRAMDATA", &ambient_program)
        .output()
        .expect("run missing uninstall");
    assert_eq!(missing.status.code(), Some(31));
    assert_eq!(String::from_utf8_lossy(&missing.stderr).lines().count(), 1);

    let (archive, hash, length) = write_pack_archive(temp.path(), "0.1.1");
    let mut install_args = roots_args.clone();
    install_args.extend([
        "install".to_owned(),
        "--archive".to_owned(),
        archive.to_string_lossy().into_owned(),
        "--expected-sha256".to_owned(),
        hash,
        "--expected-length".to_owned(),
        length.to_string(),
        "--expected-version".to_owned(),
        "0.1.1".to_owned(),
    ]);
    let installed = std::process::Command::new(executable)
        .args(&install_args)
        .env("LOCALAPPDATA", &ambient_local)
        .env("PROGRAMDATA", &ambient_program)
        .output()
        .expect("run CLI install");
    assert!(installed.status.success());
    assert!(
        local
            .join("LatentDeck/CodecPacks/org.latentdeck.h3/0.1.1")
            .is_dir()
    );
    assert!(!ambient_local.join("LatentDeck/CodecPacks").exists());

    let already = std::process::Command::new(executable)
        .args(&install_args)
        .env("LOCALAPPDATA", &ambient_local)
        .env("PROGRAMDATA", &ambient_program)
        .output()
        .expect("run repeated CLI install");
    assert_eq!(
        already.status.code(),
        Some(i32::from(EXIT_ALREADY_INSTALLED))
    );
    assert_eq!(String::from_utf8_lossy(&already.stderr).lines().count(), 1);

    let mut remove_args = roots_args;
    remove_args.extend([
        "uninstall".to_owned(),
        "--version".to_owned(),
        "0.1.1".to_owned(),
    ]);
    let removed = std::process::Command::new(executable)
        .args(&remove_args)
        .env("LOCALAPPDATA", &ambient_local)
        .env("PROGRAMDATA", &ambient_program)
        .output()
        .expect("run CLI uninstall");
    assert!(removed.status.success());
}
