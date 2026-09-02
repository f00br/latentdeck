#![allow(dead_code)]

#[path = "../src/conversion.rs"]
mod conversion;
#[path = "../src/raw_import_runtime.rs"]
mod raw_import_runtime;

use std::{fs, path::PathBuf};

use conversion::{
    ConversionCoordinator, ConversionPhase, ConversionPlanRequest, ConversionStatus,
    plan_conversion_inventory,
};
use latentdeck_cartridge::{
    hash::hash_path,
    reader::{ValidationOptions, open_integrity_validated},
};
use latentdeck_control::v2::{
    LimitedVec, ProfileKey, RawImportArtifact, RawImportAudioPolicy, RawImportMetadata,
    RawImportPreflight, RawImportStorageDtype, RawImportTensor, RawImportTensorStream,
};
use latentdeck_core::raw_import::{
    RawImportAuthoring, RawImportExpectedAuthority, RawImportFinalizeRequest, RawImportStagingRoot,
    finalize_raw_import_atomic,
};
use raw_import_runtime::{RawImportProfileView, RawImportSelectionRequest};
use uuid::Uuid;

fn synthetic_video_payload() -> Vec<u8> {
    let tensor_bytes = vec![0_u8; 24 * 2 * 2];
    let mut header = format!(
        r#"{{"video":{{"data_offsets":[0,{}],"dtype":"F16","shape":[1,24,2,1,1]}}}}"#,
        tensor_bytes.len()
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::with_capacity(8 + header.len() + tensor_bytes.len());
    payload.extend_from_slice(&(header.len() as u64).to_le_bytes());
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&tensor_bytes);
    payload
}

fn selection() -> RawImportSelectionRequest {
    RawImportSelectionRequest {
        package_id: "org.example.codec".to_owned(),
        package_version: "0.2.0".to_owned(),
        adapter_id: "org.example.codec.adapter".to_owned(),
        adapter_version: "0.2.0".to_owned(),
        profile: RawImportProfileView {
            codec_family: "example_codec".to_owned(),
            profile: "example_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
    }
}

fn metadata() -> RawImportMetadata {
    RawImportMetadata {
        profile_key: ProfileKey {
            codec_family: "example_codec".to_owned(),
            profile: "example_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
        payload_entry: "payloads/example.safetensors".to_owned(),
        payload_media_type: "application/vnd.safetensors".to_owned(),
        tensors: LimitedVec::try_from_vec(vec![RawImportTensor {
            stream: RawImportTensorStream::Visual,
            name: "video".to_owned(),
            storage_dtype: RawImportStorageDtype::F16,
            runtime_dtype: RawImportStorageDtype::F16,
            shape: LimitedVec::try_from_vec(vec![1, 24, 2, 1, 1]).expect("shape"),
        }])
        .expect("tensors"),
        timing_contract: "example_timing".to_owned(),
        timing_contract_version: "0.1.0".to_owned(),
        decoded_width: 16,
        decoded_height: 16,
        decoded_frame_count: 5,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        duration_numerator: 5,
        duration_denominator: 24,
        audio_policy: RawImportAudioPolicy::SourceAbsent,
    }
}

fn authority_and_preflight(
    source: &std::path::Path,
) -> (RawImportExpectedAuthority, RawImportPreflight) {
    let expected = RawImportExpectedAuthority::measure_source(
        "org.example.codec",
        "0.2.0",
        "org.example.codec.adapter",
        "0.2.0",
        source,
        metadata().profile_key,
    )
    .expect("authority");
    let measured = hash_path(source).expect("source identity");
    let preflight = RawImportPreflight {
        receipt_id: Uuid::new_v4(),
        import_id: Uuid::new_v4(),
        pack_id: "org.example.codec".to_owned(),
        pack_version: "0.2.0".to_owned(),
        adapter_id: "org.example.codec.adapter".to_owned(),
        adapter_version: "0.2.0".to_owned(),
        source_sha256: measured.sha256.to_string(),
        source_byte_length: measured.byte_length,
        metadata: metadata(),
    };
    expected
        .validate_preflight(&preflight)
        .expect("matching receipt");
    (expected, preflight)
}

#[test]
fn generic_inventory_accepts_non_h3_file_extensions_without_writing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("performance.customraw");
    let output_directory = directory.path().join("prepared");
    fs::write(&source, synthetic_video_payload()).expect("raw source");
    fs::create_dir(&output_directory).expect("output directory");

    let plan = plan_conversion_inventory(
        ConversionPlanRequest {
            inputs: vec![source],
            output_directory: output_directory.clone(),
            recursive: false,
        },
        selection(),
    )
    .expect("inventory");

    assert_eq!(plan.items.len(), 1);
    assert_eq!(
        plan.items[0].relative_output,
        PathBuf::from("performance.lc")
    );
    assert!(!output_directory.join("performance.lc").exists());
}

#[test]
fn folder_inventory_recurses_only_when_explicit_and_preserves_outputs() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_directory = directory.path().join("raw");
    let nested = source_directory.join("nested");
    let shallow_output = directory.path().join("shallow");
    let recursive_output = directory.path().join("recursive");
    fs::create_dir_all(&nested).expect("nested source");
    fs::create_dir(&shallow_output).expect("shallow output");
    fs::create_dir(&recursive_output).expect("recursive output");
    fs::write(source_directory.join("root.bin"), b"root").expect("root");
    fs::write(nested.join("child.tensor"), b"child").expect("child");

    let shallow = plan_conversion_inventory(
        ConversionPlanRequest {
            inputs: vec![source_directory.clone()],
            output_directory: shallow_output,
            recursive: false,
        },
        selection(),
    )
    .expect("shallow");
    let recursive = plan_conversion_inventory(
        ConversionPlanRequest {
            inputs: vec![source_directory],
            output_directory: recursive_output,
            recursive: true,
        },
        selection(),
    )
    .expect("recursive");

    assert_eq!(shallow.items.len(), 1);
    assert_eq!(recursive.items.len(), 2);
    assert_eq!(
        recursive.items[0].relative_output,
        PathBuf::from("nested/child.lc")
    );
}

#[test]
fn malicious_adapter_identity_never_reaches_the_ui_snapshot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("clip.raw");
    let output = directory.path().join("prepared");
    fs::write(&source, synthetic_video_payload()).expect("source");
    fs::create_dir(&output).expect("output");
    let mut plan = plan_conversion_inventory(
        ConversionPlanRequest {
            inputs: vec![source.clone()],
            output_directory: output,
            recursive: false,
        },
        selection(),
    )
    .expect("inventory");
    let (expected, mut preflight) = authority_and_preflight(&source);
    preflight.adapter_id = "org.attacker.adapter".to_owned();

    let error = plan
        .accept_preflight(0, expected, preflight)
        .expect_err("malicious receipt");

    assert_eq!(error.code, "raw_import.authority_mismatch");
    assert!(plan.items[0].metadata.is_none());
}

#[test]
fn core_alone_builds_reopens_and_no_clobber_commits_the_adapter_staged_payload() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("source.raw");
    let staging_parent = directory.path().join("staging");
    let output = directory.path().join("prepared.lc");
    let payload = synthetic_video_payload();
    fs::write(&source, &payload).expect("source");
    fs::create_dir(&staging_parent).expect("staging parent");
    let staging = RawImportStagingRoot::create_in(&staging_parent).expect("staging");
    let staged = staging.path().join("payload.safetensors");
    fs::write(&staged, payload).expect("adapter stage simulation");
    let staged_identity = hash_path(&staged).expect("staged identity");
    let (expected, preflight) = authority_and_preflight(&source);
    let artifact = RawImportArtifact {
        receipt_id: preflight.receipt_id,
        import_id: preflight.import_id,
        staged_payload_path: staged.to_string_lossy().into_owned(),
        payload_sha256: staged_identity.sha256.to_string(),
        payload_byte_length: staged_identity.byte_length,
    };

    let receipt = finalize_raw_import_atomic(
        &staging,
        &RawImportFinalizeRequest {
            expected,
            preflight,
            artifact,
            authoring: RawImportAuthoring::new("latentplayer-test", "0.2.0"),
        },
        &output,
    )
    .expect("Core finalization");

    assert!(receipt.staged_payload_removed);
    open_integrity_validated(&output, &ValidationOptions::default()).expect("reopened LC");
    let original_output = hash_path(&output).expect("original output identity");
    let second_staging = RawImportStagingRoot::create_in(&staging_parent).expect("second staging");
    let second_staged = second_staging.path().join("payload.safetensors");
    fs::write(&second_staged, synthetic_video_payload()).expect("second staged payload");
    let second_staged_identity = hash_path(&second_staged).expect("second staged identity");
    let (second_expected, second_preflight) = authority_and_preflight(&source);
    let error = finalize_raw_import_atomic(
        &second_staging,
        &RawImportFinalizeRequest {
            expected: second_expected,
            preflight: second_preflight.clone(),
            artifact: RawImportArtifact {
                receipt_id: second_preflight.receipt_id,
                import_id: second_preflight.import_id,
                staged_payload_path: second_staged.to_string_lossy().into_owned(),
                payload_sha256: second_staged_identity.sha256.to_string(),
                payload_byte_length: second_staged_identity.byte_length,
            },
            authoring: RawImportAuthoring::new("latentplayer-test", "0.2.0"),
        },
        &output,
    )
    .expect_err("no clobber");
    assert_eq!(error.stable_code(), "target_exists");
    let preserved_output = hash_path(&output).expect("preserved output identity");
    assert_eq!(preserved_output.sha256, original_output.sha256);
    assert_eq!(preserved_output.byte_length, original_output.byte_length);
    assert!(second_staged.is_file());
}

#[test]
fn stop_after_current_cancels_queued_items_without_starting_them() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let first = directory.path().join("first.raw");
    let second = directory.path().join("second.raw");
    let output = directory.path().join("prepared");
    fs::write(&first, synthetic_video_payload()).expect("first");
    fs::write(&second, synthetic_video_payload()).expect("second");
    fs::create_dir(&output).expect("output");
    let mut plan = plan_conversion_inventory(
        ConversionPlanRequest {
            inputs: vec![first.clone(), second.clone()],
            output_directory: output,
            recursive: false,
        },
        selection(),
    )
    .expect("inventory");
    for (index, source) in [first, second].iter().enumerate() {
        let (expected, preflight) = authority_and_preflight(source);
        plan.accept_preflight(index, expected, preflight)
            .expect("preflight");
    }
    let coordinator = ConversionCoordinator::from_plan(plan);
    coordinator.begin().expect("begin");
    let active = coordinator.next_work().expect("work").expect("active");
    coordinator.request_stop().expect("stop requested");
    coordinator
        .settle(active.index, Ok("0".repeat(64)))
        .expect("settle active");
    assert!(coordinator.next_work().expect("stop boundary").is_none());
    let snapshot = coordinator.snapshot().expect("snapshot");

    assert_eq!(snapshot.phase, ConversionPhase::Stopped);
    assert_eq!(snapshot.items[0].status, ConversionStatus::Complete);
    assert_eq!(snapshot.items[1].status, ConversionStatus::Cancelled);
}
