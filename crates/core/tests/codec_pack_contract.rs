use std::fs;
use std::path::{Path, PathBuf};

use latentdeck_core::codec_pack::{
    CodecPackErrorCode, discover_codec_packs, validate_external_asset,
};
use latentdeck_core::player::{CodecState, PlayerCoordinator};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_pack(root: &Path, id: &str, version: &str) -> PathBuf {
    let pack = root.join(id).join(version);
    fs::create_dir_all(pack.join("bin")).expect("pack directories");
    let worker = b"synthetic worker";
    let notice = b"synthetic notice";
    fs::write(pack.join("bin/worker.exe"), worker).expect("worker");
    fs::write(pack.join("NOTICE.txt"), notice).expect("notice");
    let catalog = json!({
        "manifest_version": "1.0.0",
        "files": [
            {"path": "NOTICE.txt", "byte_length": notice.len(), "sha256": sha256(notice)},
            {"path": "bin/worker.exe", "byte_length": worker.len(), "sha256": sha256(worker)}
        ]
    });
    let catalog_bytes = serde_json::to_vec(&catalog).expect("catalog json");
    fs::write(pack.join("catalog.json"), &catalog_bytes).expect("catalog");
    let manifest = json!({
        "manifest_version": "1.0.0",
        "pack_id": id,
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
                "variant_id": "taeh3-upstream-e743234",
                "sha256": "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
                "byte_length": 22_709_752,
                "source_url": "https://github.com/madebyollin/taehv",
                "license_label": "MIT",
                "license_url": "https://github.com/madebyollin/taehv/blob/e743234f3217ab3d1570f65642ab06596d1bd7c5/LICENSE"
            }]
        }]
    });
    fs::write(
        pack.join("codec-pack.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("manifest");
    pack
}

fn mutate_manifest(pack: &Path, mutate: impl FnOnce(&mut Value)) {
    let path = pack.join("codec-pack.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).expect("manifest"))
        .expect("valid manifest fixture");
    mutate(&mut value);
    fs::write(path, serde_json::to_vec(&value).expect("manifest json")).expect("manifest");
}

#[test]
fn discovers_a_fully_integrity_checked_h3_pack() {
    let root = TempDir::new().expect("root");
    write_pack(root.path(), "org.latentdeck.h3", "0.1.0");

    let packs =
        discover_codec_packs(&[root.path().to_path_buf()], "0.1.0").expect("validated pack");

    assert_eq!(packs.len(), 1);
    assert_eq!(packs[0].manifest.pack_id, "org.latentdeck.h3");
    assert!(packs[0].worker_executable.ends_with("bin/worker.exe"));
}

#[test]
fn player_requires_an_explicit_exact_pack_instead_of_selecting_newest() {
    let root = TempDir::new().expect("root");
    write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
    write_pack(root.path(), "org.latentdeck.h3", "0.1.1");

    let packs =
        discover_codec_packs(&[root.path().to_path_buf()], "0.1.0").expect("validated packs");
    assert!(
        packs
            .iter()
            .all(|pack| pack.manifest.adapter.adapter_version == "0.1.0")
    );

    let mut player = PlayerCoordinator::discover(&[root.path().to_path_buf()], "0.1.0")
        .expect("player codec discovery");
    assert_eq!(player.view().codec.pack_version, None);
    assert_eq!(
        player
            .launch_inputs()
            .err()
            .expect("discovery must never imply a selected version")
            .code,
        "codec.selection_missing"
    );

    let selected = player
        .select_codec_pack_exact("org.latentdeck.h3", "0.1.0")
        .expect("the requested older version remains selectable");
    assert_eq!(selected.codec.pack_version.as_deref(), Some("0.1.0"));

    let missing = player
        .select_codec_pack_exact("org.latentdeck.h3", "0.1.9")
        .expect_err("an absent exact version must not fall back to newest");
    assert_eq!(missing.code, "codec.pack_missing");
    assert_eq!(
        player.view().codec.pack_version.as_deref(),
        Some("0.1.0"),
        "failed reselection must not silently change the active exact version"
    );
}

#[test]
fn player_exposes_decoder_provenance_and_recovers_after_incompatible_selection() {
    let root = TempDir::new().expect("root");
    let pack_path = write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
    let expected = b"synthetic external decoder weight";
    mutate_manifest(&pack_path, |manifest| {
        manifest["external_assets"][0]["accepted_variants"][0]["sha256"] =
            Value::String(sha256(expected));
        manifest["external_assets"][0]["accepted_variants"][0]["byte_length"] =
            Value::from(expected.len());
    });
    let mut player = PlayerCoordinator::discover(&[root.path().to_path_buf()], "0.1.0")
        .expect("player codec discovery");
    player
        .select_codec_pack_exact("org.latentdeck.h3", "0.1.0")
        .expect("exact P1 pack selection");

    let initial = player.view().codec;
    assert_eq!(initial.state, CodecState::Missing);
    assert_eq!(initial.pack_id.as_deref(), Some("org.latentdeck.h3"));
    assert_eq!(initial.decoder_variants.len(), 1);
    assert_eq!(
        initial.decoder_variants[0].source_url,
        "https://github.com/madebyollin/taehv"
    );
    assert_eq!(initial.decoder_variants[0].license_label, "MIT");
    assert_eq!(initial.decoder_variants[0].sha256.len(), 64);

    let wrong_weight = root.path().join("wrong.safetensors");
    fs::write(&wrong_weight, b"not the declared decoder").expect("wrong decoder fixture");
    let error = player
        .select_decoder_asset(&wrong_weight)
        .expect_err("wrong decoder must remain visible as incompatible");

    assert_eq!(error.code, "codec.asset_incompatible");
    let incompatible = player.view();
    assert_eq!(incompatible.codec.state, CodecState::Incompatible);
    assert!(
        incompatible
            .codec
            .decoder_variants
            .iter()
            .all(|item| !item.selected)
    );
    assert_eq!(
        incompatible.error.as_ref().map(|error| error.recoverable),
        Some(true)
    );
    assert!(
        !serde_json::to_string(&incompatible)
            .expect("serialize player state")
            .contains(root.path().to_string_lossy().as_ref())
    );

    let accepted_weight = root.path().join("accepted.safetensors");
    fs::write(&accepted_weight, expected).expect("accepted decoder fixture");
    let recovered = player
        .select_decoder_asset(&accepted_weight)
        .expect("a valid retry must recover decoder selection");

    assert_eq!(recovered.codec.state, CodecState::Ready);
    assert!(recovered.codec.decoder_variants[0].selected);
    assert!(recovered.error.is_none());
    assert!(
        !serde_json::to_string(&recovered)
            .expect("serialize recovered player state")
            .contains(root.path().to_string_lossy().as_ref())
    );
}

#[test]
fn blocks_unknown_fields_and_path_traversal_before_launch() {
    let unknown_root = TempDir::new().expect("unknown root");
    let unknown_pack = write_pack(unknown_root.path(), "org.latentdeck.h3", "0.1.0");
    mutate_manifest(&unknown_pack, |manifest| {
        manifest["worker"]["shell"] = Value::Bool(true);
    });
    let unknown = discover_codec_packs(&[unknown_root.path().to_path_buf()], "0.1.0")
        .expect_err("unknown field");
    assert_eq!(unknown.code, CodecPackErrorCode::ManifestInvalid.as_str());

    let traversal_root = TempDir::new().expect("traversal root");
    let traversal_pack = write_pack(traversal_root.path(), "org.latentdeck.h3", "0.1.0");
    mutate_manifest(&traversal_pack, |manifest| {
        manifest["worker"]["executable"] = Value::String("../worker.exe".into());
    });
    let traversal = discover_codec_packs(&[traversal_root.path().to_path_buf()], "0.1.0")
        .expect_err("traversal");
    assert_eq!(traversal.code, CodecPackErrorCode::PathUnsafe.as_str());
}

#[test]
fn rejects_legacy_deck_worker_entrypoint_fields() {
    for legacy_field in ["d2_arguments", "q4_arguments"] {
        let root = TempDir::new().expect("root");
        let pack = write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
        mutate_manifest(&pack, |manifest| {
            manifest["worker"][legacy_field] = json!(["--legacy-deck-worker"]);
        });

        let error = discover_codec_packs(&[root.path().to_path_buf()], "0.1.0")
            .expect_err("legacy Deck worker entrypoint must be unknown");
        assert_eq!(error.code, CodecPackErrorCode::ManifestInvalid.as_str());
    }
}

#[test]
fn blocks_incompatible_app_protocol_platform_and_profile() {
    for (field, value, expected) in [
        (
            "/compatibility/app_min_inclusive",
            Value::String("0.2.0".into()),
            CodecPackErrorCode::PackIncompatibleApp,
        ),
        (
            "/compatibility/worker_protocol_min",
            Value::from(2),
            CodecPackErrorCode::PackIncompatibleProtocol,
        ),
        (
            "/platform/arch",
            Value::String("aarch64".into()),
            CodecPackErrorCode::PackIncompatiblePlatform,
        ),
        (
            "/compatibility/profiles/0/profile_version",
            Value::String("9.0.0".into()),
            CodecPackErrorCode::ManifestInvalid,
        ),
    ] {
        let root = TempDir::new().expect("root");
        let pack = write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
        mutate_manifest(&pack, |manifest| {
            if field == "/compatibility/profiles/0/profile_version" {
                manifest["compatibility"]["profiles"][0]["profile_versions"] = json!(["9.0.0"]);
            } else {
                *manifest.pointer_mut(field).expect("fixture pointer") = value;
            }
        });
        let error = discover_codec_packs(&[root.path().to_path_buf()], "0.1.0")
            .expect_err("incompatible pack");
        let accepted = if expected == CodecPackErrorCode::ManifestInvalid {
            CodecPackErrorCode::PackIncompatibleProfile.as_str()
        } else {
            expected.as_str()
        };
        assert_eq!(error.code, accepted);
    }
}

#[test]
fn blocks_corrupt_files_and_duplicate_id_version_across_roots() {
    let corrupt_root = TempDir::new().expect("corrupt root");
    let corrupt_pack = write_pack(corrupt_root.path(), "org.latentdeck.h3", "0.1.0");
    fs::write(corrupt_pack.join("bin/worker.exe"), b"changed").expect("corrupt worker");
    let corrupt = discover_codec_packs(&[corrupt_root.path().to_path_buf()], "0.1.0")
        .expect_err("corrupt file");
    assert_eq!(corrupt.code, CodecPackErrorCode::IntegrityFailed.as_str());

    let first = TempDir::new().expect("first root");
    let second = TempDir::new().expect("second root");
    write_pack(first.path(), "org.latentdeck.h3", "0.1.0");
    write_pack(second.path(), "org.latentdeck.h3", "0.1.0");
    let conflict = discover_codec_packs(
        &[first.path().to_path_buf(), second.path().to_path_buf()],
        "0.1.0",
    )
    .expect_err("duplicate identity");
    assert_eq!(conflict.code, CodecPackErrorCode::PackConflict.as_str());
}

#[test]
fn blocks_files_outside_the_integrity_catalog() {
    let root = TempDir::new().expect("root");
    let pack = write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
    let cache = pack.join("bin/__pycache__");
    fs::create_dir_all(&cache).expect("cache directory");
    fs::write(
        cache.join("worker.cpython-313.pyc"),
        b"uncatalogued bytecode",
    )
    .expect("uncatalogued file");

    let error = discover_codec_packs(&[root.path().to_path_buf()], "0.1.0")
        .expect_err("uncatalogued pack file");
    assert_eq!(error.code, CodecPackErrorCode::IntegrityFailed.as_str());
}

#[test]
fn missing_roots_mean_codec_not_installed_without_disk_scanning() {
    let root = TempDir::new().expect("root");
    let missing = root.path().join("does-not-exist");
    let packs = discover_codec_packs(&[missing], "0.1.0").expect("missing root is empty");
    assert!(packs.is_empty());
}

#[test]
fn blocks_an_intermediate_link_even_when_it_resolves_inside_the_pack() {
    let root = TempDir::new().expect("root");
    let pack = write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
    let bin = pack.join("bin");
    let real_bin = pack.join("real-bin");
    fs::rename(&bin, &real_bin).expect("move real worker directory");

    if !create_directory_link(&real_bin, &bin) {
        // Creating symbolic links can be disabled by Windows policy. The
        // production check still covers junctions through the reparse bit.
        return;
    }

    let error = discover_codec_packs(&[root.path().to_path_buf()], "0.1.0")
        .expect_err("intermediate filesystem link");
    assert_eq!(
        error.code,
        CodecPackErrorCode::ReparsePointForbidden.as_str()
    );
}

#[test]
fn external_decoder_asset_requires_an_explicit_exact_variant_match() {
    let root = TempDir::new().expect("root");
    let pack_path = write_pack(root.path(), "org.latentdeck.h3", "0.1.0");
    let expected = b"synthetic external decoder weight";
    mutate_manifest(&pack_path, |manifest| {
        manifest["external_assets"][0]["accepted_variants"][0]["sha256"] =
            Value::String(sha256(expected));
        manifest["external_assets"][0]["accepted_variants"][0]["byte_length"] =
            Value::from(expected.len());
    });
    let pack = discover_codec_packs(&[root.path().to_path_buf()], "0.1.0")
        .expect("pack")
        .pop()
        .expect("one pack");
    let asset_path = root.path().join("selected-taeh3.safetensors");
    fs::write(&asset_path, expected).expect("external asset");

    let selected = validate_external_asset(&pack, "taeh3", &asset_path).expect("exact asset");
    assert_eq!(selected.byte_length, expected.len() as u64);
    assert_eq!(selected.sha256, sha256(expected));

    fs::write(&asset_path, b"different").expect("mutate selected asset");
    let incompatible =
        validate_external_asset(&pack, "taeh3", &asset_path).expect_err("hash mismatch");
    assert_eq!(
        incompatible.code,
        CodecPackErrorCode::ExternalAssetIncompatible.as_str()
    );
}

#[cfg(windows)]
fn create_directory_link(original: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_dir(original, link).is_ok()
}

#[cfg(unix)]
fn create_directory_link(original: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(original, link).is_ok()
}

#[cfg(not(any(windows, unix)))]
fn create_directory_link(_original: &Path, _link: &Path) -> bool {
    false
}
