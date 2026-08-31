mod support;

use std::{collections::BTreeMap, fs};

use latentdeck_cartridge::authoring::{RawH3AuthoringOptions, inspect_raw_h3, pack_raw_h3_atomic};
use latentdeck_cartridge::reader::{ValidationLevel, ValidationOptions, open_validated};
use latentdeck_cartridge::safetensor::SafetensorDType;

#[test]
fn raw_h3_authoring_builds_and_fully_validates_a_cartridge_without_mutating_source() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.safetensors");
    let output = directory.path().join("source.lc");
    let payload = support::synthetic_video_payload();
    fs::write(&source, &payload).expect("synthetic raw H3 payload");

    let options = RawH3AuthoringOptions::new("latentplayer", "0.1.0");
    let receipt =
        pack_raw_h3_atomic(&source, &output, &options).expect("raw H3 cartridge authoring");

    assert_eq!(receipt.validation.validation_level, ValidationLevel::Full);
    assert_eq!(receipt.validation.payload_bytes, payload.len() as u64);
    assert_eq!(fs::read(&source).expect("source bytes"), payload);
    assert!(output.is_file());
}

#[test]
fn raw_h3_authoring_preserves_f32_av_tensor_contract() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source-av.safetensors");
    let output = directory.path().join("source-av.lc");
    fs::write(&source, support::synthetic_av_f32_payload()).expect("synthetic AV payload");

    pack_raw_h3_atomic(
        &source,
        &output,
        &RawH3AuthoringOptions::new("latentplayer", "0.1.0"),
    )
    .expect("raw H3 AV authoring");
    let validated =
        open_validated(&output, &ValidationOptions::default()).expect("validated AV cartridge");

    assert_eq!(validated.manifest().tensors.len(), 2);
    assert_eq!(
        serde_json::to_value(&validated.manifest().audio).expect("audio disposition"),
        serde_json::json!({"policy": "preserved_source"})
    );
    assert_eq!(
        validated
            .h3_profile()
            .audio
            .as_ref()
            .expect("audio profile")
            .latent_slots,
        405
    );
}

#[test]
fn raw_h3_authoring_records_explicit_bounded_provenance() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.safetensors");
    let output = directory.path().join("source.lc");
    fs::write(&source, support::synthetic_video_payload()).expect("synthetic payload");
    let options = RawH3AuthoringOptions::new("latentplayer", "0.1.0")
        .with_cartridge_id("550e8400-e29b-41d4-a716-446655440099")
        .with_created_at("2026-08-31T12:00:00Z")
        .with_source_kind("raw_h3_safetensors")
        .with_source_metadata(BTreeMap::from([(
            "workflow_sha256".to_owned(),
            serde_json::json!("a".repeat(64)),
        )]));

    pack_raw_h3_atomic(&source, &output, &options).expect("raw H3 authoring");
    let validated =
        open_validated(&output, &ValidationOptions::default()).expect("validated cartridge");
    let manifest = validated.manifest();

    assert_eq!(
        manifest.cartridge_id.0,
        "550e8400-e29b-41d4-a716-446655440099"
    );
    assert_eq!(manifest.provenance.created_by.name.0, "latentplayer");
    assert_eq!(
        manifest.provenance.created_at.as_deref(),
        Some("2026-08-31T12:00:00Z")
    );
    assert_eq!(manifest.provenance.sources[0].kind.0, "raw_h3_safetensors");
    assert_eq!(
        manifest.provenance.sources[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("workflow_sha256")),
        Some(&serde_json::json!("a".repeat(64)))
    );
}

#[test]
fn raw_h3_authoring_attaches_an_optional_validated_webp_preview() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.safetensors");
    let preview = directory.path().join("preview.webp");
    let output = directory.path().join("source.lc");
    fs::write(&source, support::synthetic_video_payload()).expect("synthetic payload");
    fs::write(&preview, support::synthetic_preview()).expect("synthetic preview");

    let options = RawH3AuthoringOptions::new("latentplayer", "0.1.0").with_preview(&preview);
    pack_raw_h3_atomic(&source, &output, &options).expect("preview authoring");
    let validated =
        open_validated(&output, &ValidationOptions::default()).expect("validated cartridge");

    let descriptor = validated.manifest().preview.as_ref().expect("preview");
    assert_eq!((descriptor.width, descriptor.height), (448, 800));
}

#[test]
fn raw_h3_authoring_replaces_an_existing_target_only_when_explicit() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.safetensors");
    let output = directory.path().join("source.lc");
    fs::write(&source, support::synthetic_video_payload()).expect("synthetic payload");
    fs::write(&output, b"owned output").expect("existing output");

    let forbidden = pack_raw_h3_atomic(
        &source,
        &output,
        &RawH3AuthoringOptions::new("latentplayer", "0.1.0"),
    )
    .expect_err("default no clobber");
    assert_eq!(forbidden.code(), "target_exists");
    assert_eq!(
        fs::read(&output).expect("preserved output"),
        b"owned output"
    );

    let options = RawH3AuthoringOptions::new("latentplayer", "0.1.0").with_overwrite(true);
    pack_raw_h3_atomic(&source, &output, &options).expect("explicit replacement");
    assert_ne!(fs::read(&output).expect("replacement"), b"owned output");
}

#[test]
fn raw_h3_inspection_returns_fully_validated_profile_metadata_without_writing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source-av.safetensors");
    let payload = support::synthetic_av_f32_payload();
    fs::write(&source, &payload).expect("synthetic AV payload");

    let inspection = inspect_raw_h3(&source).expect("raw H3 inspection");

    assert_eq!(inspection.payload_bytes, payload.len() as u64);
    assert_eq!(inspection.safetensors.video.dtype, SafetensorDType::F32);
    assert_eq!(inspection.profile.visual.decoded_frame_count, 243);
    assert_eq!(
        inspection
            .profile
            .audio
            .as_ref()
            .expect("audio")
            .latent_slots,
        405
    );
    assert_eq!(
        fs::read_dir(directory.path()).expect("inventory").count(),
        1
    );
}

#[test]
fn raw_h3_authoring_rejects_a_source_changed_after_approved_preflight() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.safetensors");
    let output = directory.path().join("source.lc");
    let original = support::synthetic_video_payload();
    fs::write(&source, &original).expect("original raw payload");
    let expected = inspect_raw_h3(&source)
        .expect("approved preflight")
        .payload_sha256;
    let mut replacement = original;
    let last = replacement.last_mut().expect("tensor byte");
    *last = 1;
    fs::write(&source, replacement).expect("valid changed raw payload");

    let error = pack_raw_h3_atomic(
        &source,
        &output,
        &RawH3AuthoringOptions::new("latentplayer", "0.1.0").with_expected_payload_sha256(expected),
    )
    .expect_err("changed source must not pass the approved plan");

    assert_eq!(error.code(), "payload_hash_mismatch");
    assert!(!output.exists());
}
