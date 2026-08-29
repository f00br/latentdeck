mod support;

use std::{collections::BTreeMap, fs, io::Cursor};

use latentdeck_cartridge::{
    hash::hash_reader,
    manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, DType, Identifier, ParentCartridge,
        Sha256Digest, SourceCartridgeRef,
    },
    reader::{ValidationOptions, open_validated},
    resample::{CaptureMode, PayloadExpectation, ResampleManifestRequest, pack_resample_atomic},
    writer::WriteOptions,
};
use serde_json::json;
use tempfile::tempdir;

use support::synthetic_video_payload;

fn source(cartridge_id: &str, digest_byte: char) -> SourceCartridgeRef {
    SourceCartridgeRef {
        cartridge_id: CartridgeId(cartridge_id.to_owned()),
        archive_sha256: Sha256Digest(digest_byte.to_string().repeat(64)),
    }
}

fn request(payload: &[u8]) -> ResampleManifestRequest {
    let measured = hash_reader(&mut Cursor::new(payload)).expect("measure payload");
    let source_a = source("550e8400-e29b-41d4-a716-446655440010", 'a');
    let source_b = source("550e8400-e29b-41d4-a716-446655440011", 'b');
    let mut controls = BTreeMap::new();
    controls.insert("algorithm".to_owned(), json!("XS5"));
    controls.insert("routing".to_owned(), json!("A"));
    controls.insert("chaos".to_owned(), json!(0.0));
    ResampleManifestRequest {
        cartridge_id: CartridgeId("550e8400-e29b-41d4-a716-446655440099".to_owned()),
        expected_payload: PayloadExpectation {
            byte_length: measured.byte_length,
            sha256: Sha256Digest(measured.sha256.to_string()),
        },
        capture_mode: CaptureMode::Snapshot,
        audio: AudioDisposition::SourceAbsent,
        parent_cartridges: vec![
            ParentCartridge {
                cartridge_id: source_a.cartridge_id.clone(),
                archive_sha256: source_a.archive_sha256.clone(),
                role: Identifier("source_a".to_owned()),
            },
            ParentCartridge {
                cartridge_id: source_b.cartridge_id.clone(),
                archive_sha256: source_b.archive_sha256.clone(),
                role: Identifier("source_b".to_owned()),
            },
        ],
        operator_id: Identifier("org.latentdeck.builtin.ld_d2".to_owned()),
        operator_version: "0.1.0".to_owned(),
        seed: 42,
        controls,
    }
}

#[test]
fn builds_valid_fp16_snapshot_genealogy_and_packs_atomically() {
    let directory = tempdir().expect("tempdir");
    let payload = synthetic_video_payload();
    let payload_path = directory.path().join("snapshot.safetensors.partial");
    let output = directory.path().join("snapshot.lc");
    fs::write(&payload_path, &payload).expect("write payload");

    let receipt = pack_resample_atomic(
        &request(&payload),
        &payload_path,
        &output,
        &WriteOptions::default(),
    )
    .expect("pack resample");

    assert_eq!(receipt.output_path, output);
    let validated =
        open_validated(&output, &ValidationOptions::default()).expect("validate output");
    let manifest = validated.manifest();
    assert_eq!(
        manifest.cartridge_id.0,
        "550e8400-e29b-41d4-a716-446655440099"
    );
    assert_eq!(manifest.tensors[0].storage_dtype, DType::F16);
    assert_eq!(manifest.timing.decoded_video.frame_count, 5);
    assert_eq!(manifest.parent_cartridges.len(), 2);
    assert_eq!(manifest.operation_history.len(), 1);
    let operation = &manifest.operation_history[0];
    assert_eq!(operation.operator_id.0, "org.latentdeck.builtin.ld_d2");
    assert_eq!(operation.seed, 42);
    assert_eq!(operation.controls["capture_mode"], json!("snapshot"));
    assert_eq!(operation.controls["algorithm"], json!("XS5"));
    assert_eq!(manifest.provenance.sources.len(), 2);
    assert!(!payload_path.exists(), "committed spool must be consumed");
}

#[test]
fn preserves_exact_carrier_audio_descriptor_for_snapshot() {
    let directory = tempdir().expect("tempdir");
    let payload = synthetic_av_resample_payload();
    let payload_path = directory.path().join("snapshot-av.safetensors.partial");
    let output = directory.path().join("snapshot-av.lc");
    fs::write(&payload_path, &payload).expect("write payload");
    let carrier = source("550e8400-e29b-41d4-a716-446655440010", 'a');
    let mut request = request(&payload);
    request.audio = AudioDisposition::CopiedFromCarrierExact {
        source_cartridge: carrier.clone(),
    };

    pack_resample_atomic(&request, &payload_path, &output, &WriteOptions::default())
        .expect("pack AV resample");

    let validated = open_validated(&output, &ValidationOptions::default()).expect("validate AV");
    assert_eq!(validated.manifest().tensors.len(), 2);
    assert_eq!(validated.manifest().audio, request.audio);
    assert_eq!(
        validated
            .h3_profile()
            .audio
            .as_ref()
            .expect("audio")
            .latent_slots,
        8
    );
}

#[test]
fn records_explicit_live_audio_omission_without_inventing_an_audio_tensor() {
    let directory = tempdir().expect("tempdir");
    let payload = synthetic_video_payload();
    let payload_path = directory.path().join("live.safetensors.partial");
    let output = directory.path().join("live.lc");
    fs::write(&payload_path, &payload).expect("write payload");
    let carrier = source("550e8400-e29b-41d4-a716-446655440010", 'a');
    let mut request = request(&payload);
    request.capture_mode = CaptureMode::LiveCapture;
    request.audio = AudioDisposition::OmittedTimingMismatch {
        source_cartridge: carrier,
        reason: AudioOmissionReason::DurationAndMappingMismatch,
    };

    pack_resample_atomic(&request, &payload_path, &output, &WriteOptions::default())
        .expect("pack live visual-only resample");

    let validated = open_validated(&output, &ValidationOptions::default()).expect("validate live");
    assert_eq!(validated.manifest().audio, request.audio);
    assert!(validated.h3_profile().audio.is_none());
    assert_eq!(
        validated.manifest().operation_history[0].controls["capture_mode"],
        json!("live_capture")
    );
}

#[test]
fn rejects_payload_swap_and_non_fp16_post_operator_visual() {
    let directory = tempdir().expect("tempdir");
    let payload = synthetic_video_payload();
    let payload_path = directory.path().join("changed.safetensors.partial");
    let output = directory.path().join("changed.lc");
    fs::write(&payload_path, &payload).expect("write payload");
    let expected = request(&payload);
    let mut changed = payload.clone();
    *changed.last_mut().expect("payload byte") = 1;
    fs::write(&payload_path, changed).expect("mutate payload");

    let error = pack_resample_atomic(&expected, &payload_path, &output, &WriteOptions::default())
        .expect_err("hash swap must fail");
    assert_eq!(error.code(), "payload_hash_mismatch");
    assert!(!output.exists());

    let f32 = synthetic_f32_visual_payload();
    fs::write(&payload_path, &f32).expect("write F32 payload");
    let error = pack_resample_atomic(
        &request(&f32),
        &payload_path,
        &output,
        &WriteOptions::default(),
    )
    .expect_err("post-operator F32 must fail");
    assert_eq!(error.code(), "tensor_dtype_forbidden");
}

#[test]
fn rejects_reserved_provenance_control_override() {
    let payload = synthetic_video_payload();
    let mut request = request(&payload);
    request
        .controls
        .insert("capture_mode".to_owned(), json!("forged"));
    let directory = tempdir().expect("tempdir");
    let payload_path = directory.path().join("reserved.safetensors.partial");
    fs::write(&payload_path, &payload).expect("write payload");
    let error = pack_resample_atomic(
        &request,
        &payload_path,
        directory.path().join("reserved.lc"),
        &WriteOptions::default(),
    )
    .expect_err("reserved control must fail");
    assert_eq!(error.code(), "manifest_invalid");
}

fn synthetic_av_resample_payload() -> Vec<u8> {
    let video = vec![0_u8; 24 * 2 * 2];
    let audio = vec![0_u8; 32 * 2 * 8 * 4];
    let video_end = video.len();
    let audio_end = video_end + audio.len();
    let mut header = format!(
        concat!(
            r#"{{"audio":{{"data_offsets":[{},{}],"dtype":"F32","shape":[1,32,2,8]}},"#,
            r#""video":{{"data_offsets":[0,{}],"dtype":"F16","shape":[1,24,2,1,1]}}}}"#
        ),
        video_end, audio_end, video_end
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("header length")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&video);
    payload.extend_from_slice(&audio);
    payload
}

fn synthetic_f32_visual_payload() -> Vec<u8> {
    let video = vec![0_u8; 24 * 2 * 4];
    let mut header = format!(
        r#"{{"video":{{"data_offsets":[0,{}],"dtype":"F32","shape":[1,24,2,1,1]}}}}"#,
        video.len()
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::new();
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("header length")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&video);
    payload
}
