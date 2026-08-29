use latentdeck_cartridge::{
    limits::ValidationLimits,
    manifest::{DType, parse_manifest_json},
    profile::h3,
};

fn av_manifest_t72() -> Vec<u8> {
    br#"{
        "spec_version":"0.1.0",
        "cartridge_id":"550e8400-e29b-41d4-a716-446655440000",
        "codec":{"family":"minimax_h3","profile":"h3_av_latent","profile_version":"0.1.0"},
        "payloads":[{"path":"payloads/h3.safetensors","media_type":"application/vnd.safetensors","byte_length":9780744,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}],
        "tensors":[
            {"stream":"visual","name":"video","payload":"payloads/h3.safetensors","storage_dtype":"F32","runtime_dtype":"F16","shape":[1,24,72,50,28]},
            {"stream":"audio","name":"audio","payload":"payloads/h3.safetensors","storage_dtype":"F32","runtime_dtype":"F32","shape":[1,32,2,405]}
        ],
        "timing":{"contract":"minimax_h3_causal","contract_version":"0.1.0","decoded_video":{"width":448,"height":800,"frame_count":243,"frame_rate":{"numerator":24,"denominator":1},"duration":{"numerator":81,"denominator":8}}},
        "audio":{"policy":"preserved_source"},
        "provenance":{"created_by":{"name":"latentdeck-cartridge","version":"0.1.0"},"sources":[]},
        "parent_cartridges":[],
        "operation_history":[]
    }"#
    .to_vec()
}

#[test]
fn validates_t72_av_cadence_and_preserved_audio_dtype() {
    let limits = ValidationLimits::specification();
    let manifest = parse_manifest_json(&av_manifest_t72(), &limits).expect("valid manifest");
    let validated = h3::validate(&manifest, &limits).expect("valid H3 AV profile");

    assert_eq!(validated.visual.decoded_frame_count, 243);
    let audio = validated.audio.expect("audio is present");
    assert_eq!(audio.latent_slots, 405);
    assert_eq!(audio.storage_dtype, DType::F32);
    assert_eq!(audio.runtime_dtype, DType::F32);
}

#[test]
fn keeps_streaming_five_slot_cadence_separate_from_full_clip_cadence() {
    assert_eq!(h3::streaming_usable_frames(5).expect("one block"), 17);
    assert_eq!(h3::streaming_usable_frames(10).expect("two blocks"), 34);
    assert_eq!(
        h3::streaming_usable_frames(4)
            .expect_err("partial block")
            .code(),
        "timing_mismatch"
    );
    assert!(h3::decoded_frame_count(5).is_err());
}

#[test]
fn rejects_unknown_h3_tensor_descriptors() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&av_manifest_t72()).expect("test JSON");
    value["tensors"][0]["name"] = serde_json::Value::String("mystery".into());
    let limits = ValidationLimits::specification();
    let manifest = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &limits,
    )
    .expect("generic manifest remains well formed");

    let error = h3::validate(&manifest, &limits).expect_err("unknown H3 tensor");

    assert_eq!(error.code(), "tensor_unexpected");
    assert_eq!(error.location.tensor.as_deref(), Some("mystery"));
}

#[test]
fn rejects_forbidden_tensor_dtypes_with_a_stable_profile_error() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&av_manifest_t72()).expect("test JSON");
    value["tensors"][0]["storage_dtype"] = serde_json::Value::String("BF16".into());

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("BF16 is outside LC 0.1");

    assert_eq!(error.code(), "tensor_dtype_forbidden");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/tensors/0/storage_dtype")
    );
}

#[test]
fn requires_the_exact_h3_payload_descriptor_and_tensor_reference() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&av_manifest_t72()).expect("test JSON");
    value["payloads"][0]["path"] = serde_json::Value::String("payloads/other.safetensors".into());
    value["tensors"][0]["payload"] = serde_json::Value::String("payloads/other.safetensors".into());
    value["tensors"][1]["payload"] = serde_json::Value::String("payloads/other.safetensors".into());
    let limits = ValidationLimits::specification();
    let manifest = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &limits,
    )
    .expect("generic profile path remains well formed");

    let error = h3::validate(&manifest, &limits).expect_err("H3 path is fixed");

    assert_eq!(error.code(), "tensor_descriptor_mismatch");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/payloads/0/path")
    );
}

#[test]
fn rejects_payload_lengths_smaller_than_checked_tensor_data() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&av_manifest_t72()).expect("test JSON");
    value["payloads"][0]["byte_length"] = serde_json::Value::from(16_u64);
    let limits = ValidationLimits::specification();
    let manifest = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &limits,
    )
    .expect("generic manifest remains well formed");

    let error = h3::validate(&manifest, &limits).expect_err("payload is too short");

    assert_eq!(error.code(), "tensor_descriptor_mismatch");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/payloads/0/byte_length")
    );
}

#[test]
fn compatibility_key_excludes_storage_dtype_and_clip_duration() {
    let limits = ValidationLimits::specification();
    let long_manifest = parse_manifest_json(&av_manifest_t72(), &limits).expect("long manifest");
    let long = h3::validate(&long_manifest, &limits).expect("long profile");

    let mut short_value: serde_json::Value =
        serde_json::from_slice(&av_manifest_t72()).expect("test JSON");
    short_value["payloads"][0]["byte_length"] = serde_json::Value::from(2_150_500_u64);
    short_value["tensors"] = serde_json::json!([{
        "stream":"visual","name":"video","payload":"payloads/h3.safetensors",
        "storage_dtype":"F16","runtime_dtype":"F16","shape":[1,24,32,50,28]
    }]);
    short_value["timing"]["decoded_video"]["frame_count"] = serde_json::Value::from(107_u64);
    short_value["timing"]["decoded_video"]["duration"] =
        serde_json::json!({"numerator":107,"denominator":24});
    short_value["audio"] = serde_json::json!({"policy":"source_absent"});
    let short_manifest = parse_manifest_json(
        &serde_json::to_vec(&short_value).expect("test JSON serialization"),
        &limits,
    )
    .expect("short manifest");
    let short = h3::validate(&short_manifest, &limits).expect("short profile");

    assert_eq!(short.compatibility_key, long.compatibility_key);
    assert_eq!(short.compatibility_key.codec_family, "minimax_h3");
    assert_eq!(short.compatibility_key.profile_version, "0.1.0");
    assert_eq!(short.compatibility_key.timing_contract, "minimax_h3_causal");
    assert_eq!(short.compatibility_key.batch, 1);
}
