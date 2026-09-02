mod support;

use std::{collections::BTreeMap, fs, io::Read};

use serde::Serialize;

use latentdeck_cartridge::manifest::{
    CartridgeId, Identifier, OperationRecord, ParentCartridge, Sha256Digest,
};
use latentdeck_cartridge::reader::{ValidationOptions, open_integrity_validated, open_validated};
use latentdeck_cartridge::writer::{
    PackRequest, WriteOptions, canonical_json_bytes, pack_atomic, pack_integrity_atomic,
};

#[derive(Serialize)]
struct OutOfOrder<'a> {
    zeta: u64,
    alpha: &'a str,
}

#[test]
fn manifest_serialization_uses_rfc8785_key_order_without_whitespace() {
    let bytes = canonical_json_bytes(&OutOfOrder {
        zeta: 3,
        alpha: "LC",
    })
    .expect("canonical JSON");
    assert_eq!(bytes, br#"{"alpha":"LC","zeta":3}"#);
}

#[test]
fn atomic_pack_is_deterministic_validated_and_no_clobber() {
    let payload = support::synthetic_video_payload();
    let manifest = support::synthetic_manifest(&payload);
    let directory = tempfile::tempdir().expect("temporary directory");
    let payload_path = directory.path().join("input.safetensors");
    fs::write(&payload_path, &payload).expect("write synthetic payload");
    let first_path = directory.path().join("first.lc");
    let second_path = directory.path().join("second.lc");
    let request = PackRequest::new(manifest, &payload_path);

    let first =
        pack_atomic(&request, &first_path, &WriteOptions::default()).expect("first atomic pack");
    let second =
        pack_atomic(&request, &second_path, &WriteOptions::default()).expect("second atomic pack");
    assert_eq!(
        first.validation.archive_sha256,
        second.validation.archive_sha256
    );
    assert_eq!(
        fs::read(&first_path).expect("first cartridge"),
        fs::read(&second_path).expect("second cartridge")
    );

    let error =
        pack_atomic(&request, &first_path, &WriteOptions::default()).expect_err("no clobber");
    assert_eq!(error.code(), "target_exists");
    let partials = fs::read_dir(directory.path())
        .expect("temporary inventory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
        .count();
    assert_eq!(partials, 0);
}

#[test]
fn f32_av_and_optional_preview_roundtrip_without_dtype_loss() {
    let payload = support::synthetic_av_f32_payload();
    let preview = support::synthetic_preview();
    let manifest = support::with_preview(support::synthetic_av_f32_manifest(&payload), &preview);
    let directory = tempfile::tempdir().expect("temporary directory");
    let payload_path = directory.path().join("av.safetensors");
    let preview_path = directory.path().join("preview.webp");
    let output_path = directory.path().join("av-preview.lc");
    fs::write(&payload_path, &payload).expect("AV payload");
    fs::write(&preview_path, &preview).expect("preview");

    let request = PackRequest::new(manifest, &payload_path).with_preview(&preview_path);
    pack_atomic(&request, &output_path, &WriteOptions::default()).expect("AV preview pack");
    let mut validated =
        open_validated(&output_path, &ValidationOptions::default()).expect("AV preview validation");
    assert_eq!(
        validated
            .h3_profile()
            .audio
            .as_ref()
            .expect("audio profile")
            .latent_slots,
        405
    );
    let mut audio = validated.tensor_reader("audio").expect("validated audio");
    let mut audio_bytes = Vec::new();
    audio.read_to_end(&mut audio_bytes).expect("audio bytes");
    assert_eq!(audio_bytes.len(), 32 * 2 * 405 * 4);
    assert!(audio_bytes.iter().all(|byte| *byte == 0));
}

#[test]
fn fp16_resample_genealogy_and_seed_zero_survive_roundtrip() {
    let payload = support::synthetic_video_payload();
    let mut manifest = support::synthetic_manifest(&payload);
    manifest.parent_cartridges.push(ParentCartridge {
        cartridge_id: CartridgeId("550e8400-e29b-41d4-a716-446655440002".to_owned()),
        archive_sha256: Sha256Digest("1".repeat(64)),
        role: Identifier("carrier".to_owned()),
    });
    manifest.operation_history.push(OperationRecord {
        operator_id: Identifier("ld-d2-xs5".to_owned()),
        operator_version: "0.1.0".to_owned(),
        seed: 0,
        controls: BTreeMap::from([("interaction".to_owned(), serde_json::json!(0.5))]),
    });

    let directory = tempfile::tempdir().expect("temporary directory");
    let payload_path = directory.path().join("resample.safetensors");
    let output_path = directory.path().join("resample.lc");
    fs::write(&payload_path, payload).expect("resample payload");
    let request = PackRequest::new(manifest, &payload_path);
    pack_atomic(&request, &output_path, &WriteOptions::default()).expect("resample pack");
    let validated =
        open_validated(&output_path, &ValidationOptions::default()).expect("resample validation");
    assert_eq!(validated.manifest().parent_cartridges.len(), 1);
    assert_eq!(validated.manifest().operation_history[0].seed, 0);
    assert_eq!(
        validated.manifest().operation_history[0].operator_id.0,
        "ld-d2-xs5"
    );
}

#[test]
fn codec_neutral_core_finalizer_commits_a_non_h3_payload_without_profile_adaptation() {
    let payload = support::synthetic_non_h3_payload();
    let manifest = support::synthetic_non_h3_manifest(&payload);
    let directory = tempfile::tempdir().expect("temporary directory");
    let payload_path = directory.path().join("synthetic.safetensors");
    let output_path = directory.path().join("synthetic.lc");
    fs::write(&payload_path, &payload).expect("synthetic payload");
    let request = PackRequest::new(manifest.clone(), &payload_path);

    let legacy_error = pack_atomic(
        &request,
        directory.path().join("legacy-h3.lc"),
        &WriteOptions::default(),
    )
    .expect_err("legacy H3 finalizer must reject a non-H3 profile");
    assert!(matches!(
        legacy_error.code(),
        "unsupported_codec" | "unsupported_profile_version" | "manifest_invalid"
    ));

    let receipt = pack_integrity_atomic(&request, &output_path, &WriteOptions::default())
        .expect("codec-neutral atomic finalization");
    assert_eq!(
        receipt.validation.payload_path,
        "payloads/synthetic.safetensors"
    );
    let validated = open_integrity_validated(&output_path, &ValidationOptions::default())
        .expect("reopen finalized cartridge");
    assert_eq!(validated.manifest(), &manifest);
    assert_eq!(
        validated.receipt().archive_sha256,
        receipt.validation.archive_sha256
    );
}
