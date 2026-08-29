use latentdeck_cartridge::{limits::ValidationLimits, manifest::parse_manifest_json, profile::h3};

fn visual_only_manifest() -> Vec<u8> {
    br#"{
        "spec_version":"0.1.0",
        "cartridge_id":"550e8400-e29b-41d4-a716-446655440000",
        "codec":{"family":"minimax_h3","profile":"h3_av_latent","profile_version":"0.1.0"},
        "payloads":[{"path":"payloads/h3.safetensors","media_type":"application/vnd.safetensors","byte_length":2150500,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}],
        "tensors":[{"stream":"visual","name":"video","payload":"payloads/h3.safetensors","storage_dtype":"F16","runtime_dtype":"F16","shape":[1,24,32,50,28]}],
        "timing":{"contract":"minimax_h3_causal","contract_version":"0.1.0","decoded_video":{"width":448,"height":800,"frame_count":107,"frame_rate":{"numerator":24,"denominator":1},"duration":{"numerator":107,"denominator":24}}},
        "audio":{"policy":"source_absent"},
        "provenance":{"created_by":{"name":"latentdeck-cartridge","version":"0.1.0"},"sources":[]},
        "parent_cartridges":[],
        "operation_history":[]
    }"#
    .to_vec()
}

#[test]
fn parses_and_validates_a_visual_only_h3_manifest() {
    let limits = ValidationLimits::specification();
    let manifest = parse_manifest_json(&visual_only_manifest(), &limits).expect("valid manifest");
    let validated = h3::validate(&manifest, &limits).expect("valid H3 profile");

    assert_eq!(validated.visual.decoded_frame_count, 107);
    assert!(validated.audio.is_none());
}

#[test]
fn rejects_unknown_nested_manifest_fields_with_a_stable_location() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["codec"]["surprise"] = serde_json::Value::Bool(true);

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("unknown fields must be rejected");

    assert_eq!(error.code(), "manifest_unknown_field");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/codec/surprise")
    );
}

#[test]
fn rejects_duplicate_json_keys_before_typed_deserialization() {
    let text = String::from_utf8(visual_only_manifest()).expect("test JSON");
    let duplicate = text.replace(
        "\"family\":\"minimax_h3\"",
        "\"family\":\"minimax_h3\",\"family\":\"other\"",
    );

    let error = parse_manifest_json(duplicate.as_bytes(), &ValidationLimits::specification())
        .expect_err("duplicate keys must be rejected");

    assert_eq!(error.code(), "manifest_duplicate_key");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/codec/family")
    );
}

#[test]
fn rejects_noncanonical_cartridge_ids() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["cartridge_id"] =
        serde_json::Value::String("550E8400-E29B-41D4-A716-446655440000".into());

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("UUID must use its canonical lowercase representation");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/cartridge_id")
    );
}

#[test]
fn rejects_noncanonical_sha256_digests() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["payloads"][0]["sha256"] = serde_json::Value::String("A".repeat(64));

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("SHA-256 must use lowercase hexadecimal");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/payloads/0/sha256")
    );
}

#[test]
fn rejects_noncanonical_genealogy_references() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["parent_cartridges"] = serde_json::json!([{
        "cartridge_id":"550E8400-E29B-41D4-A716-446655440000",
        "archive_sha256":"0000000000000000000000000000000000000000000000000000000000000000",
        "role":"carrier"
    }]);

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("parent UUIDs must use canonical lowercase representation");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/parent_cartridges/0/cartridge_id")
    );
}

#[test]
fn enforces_preview_descriptor_limits() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["preview"] = serde_json::json!({
        "path":"preview.webp",
        "media_type":"image/webp",
        "byte_length":16_777_217_u64,
        "sha256":"0000000000000000000000000000000000000000000000000000000000000000",
        "width":448,
        "height":800
    });

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("preview bytes must remain below the hard ceiling");

    assert_eq!(error.code(), "runtime_limit_exceeded");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/preview/byte_length")
    );
}

#[test]
fn rejects_nonreduced_rationals() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["timing"]["decoded_video"]["duration"] =
        serde_json::json!({"numerator": 214, "denominator": 48});

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("rationals must be reduced");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/timing/decoded_video/duration")
    );
}

#[test]
fn rejects_noncanonical_ascii_identifiers() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["codec"]["family"] = serde_json::Value::String("MiniMax H3".into());

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("identifiers must be bounded lowercase ASCII tokens");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/codec/family")
    );
}

#[test]
fn enforces_immutable_manifest_collection_limits() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    let parent = serde_json::json!({
        "cartridge_id":"550e8400-e29b-41d4-a716-446655440000",
        "archive_sha256":"0000000000000000000000000000000000000000000000000000000000000000",
        "role":"carrier"
    });
    value["parent_cartridges"] =
        serde_json::Value::Array(std::iter::repeat_n(parent, 257).collect());

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("parent count must be bounded");

    assert_eq!(error.code(), "runtime_limit_exceeded");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/parent_cartridges")
    );
}

#[test]
fn rejects_operation_seeds_outside_the_json_safe_integer_range() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["operation_history"] = serde_json::json!([{
        "operator_id":"linear",
        "operator_version":"0.1.0",
        "seed":9_007_199_254_740_992_u64,
        "controls":{}
    }]);

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("seeds must be exactly representable in the JCS number model");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/operation_history/0/seed")
    );
}

#[test]
fn rejects_json_nesting_beyond_the_specification_ceiling() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    let mut nested = serde_json::Value::Null;
    for _ in 0..33 {
        nested = serde_json::json!({"next": nested});
    }
    value["provenance"]["sources"] = serde_json::json!([{
        "kind":"generated",
        "metadata":{"nested":nested}
    }]);

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("deep JSON must be rejected during bounded preflight");

    assert_eq!(error.code(), "runtime_limit_exceeded");
    assert!(
        error
            .location
            .json_pointer
            .as_deref()
            .is_some_and(|pointer| pointer.starts_with("/provenance/sources/0/metadata"))
    );
}

#[test]
fn rejects_provenance_uris_beyond_the_utf8_byte_ceiling() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["provenance"]["sources"] = serde_json::json!([{
        "kind":"generated",
        "uri":"x".repeat(8_193)
    }]);

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("URIs must obey the UTF-8 byte ceiling");

    assert_eq!(error.code(), "runtime_limit_exceeded");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/provenance/sources/0/uri")
    );
}

#[test]
fn rejects_nested_numbers_outside_the_jcs_safe_range() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["operation_history"] = serde_json::json!([{
        "operator_id":"linear",
        "operator_version":"0.1.0",
        "seed":0,
        "controls":{"top_k":9_007_199_254_740_992_u64}
    }]);

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("nested integers must be exactly representable in JCS");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/operation_history/0/controls/top_k")
    );
}

#[test]
fn caller_limits_can_only_be_lowered() {
    let specification = ValidationLimits::specification();
    let lowered = specification
        .with_max_manifest_bytes(512)
        .with_max_h3_payload_bytes(u64::MAX)
        .with_max_json_depth(8)
        .with_max_h3_decoded_axis(u32::MAX)
        .with_max_manifest_bytes(2_048);

    assert_eq!(lowered.max_manifest_bytes(), 512);
    assert_eq!(
        lowered.max_h3_payload_bytes(),
        specification.max_h3_payload_bytes()
    );
    assert_eq!(lowered.max_json_depth(), 8);
    assert_eq!(
        lowered.max_h3_decoded_axis(),
        specification.max_h3_decoded_axis()
    );
}

#[test]
fn rejects_non_utc_rfc3339_provenance_timestamps() {
    let mut value: serde_json::Value =
        serde_json::from_slice(&visual_only_manifest()).expect("test JSON");
    value["provenance"]["created_at"] =
        serde_json::Value::String("2026-08-30T10:00:00+07:00".into());

    let error = parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect_err("provenance timestamps must be RFC 3339 UTC with Z");

    assert_eq!(error.code(), "manifest_invalid");
    assert_eq!(
        error.location.json_pointer.as_deref(),
        Some("/provenance/created_at")
    );

    value["provenance"]["created_at"] = serde_json::Value::String("2026-08-30T03:00:00Z".into());
    parse_manifest_json(
        &serde_json::to_vec(&value).expect("test JSON serialization"),
        &ValidationLimits::specification(),
    )
    .expect("canonical RFC 3339 UTC timestamp");
}
