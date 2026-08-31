#[path = "../src/conversion.rs"]
mod conversion;

use std::{fs, path::PathBuf};

use conversion::{
    ConversionCoordinator, ConversionPhase, ConversionPlanRequest, ConversionStatus,
    plan_conversion,
};
use latentdeck_cartridge::reader::{ValidationOptions, open_validated};

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

#[test]
fn explicit_raw_file_preflight_returns_playable_metadata_without_writing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("performance.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::write(&source, synthetic_video_payload()).expect("raw H3 source");
    fs::create_dir(&output_directory).expect("output directory");

    let plan = plan_conversion(ConversionPlanRequest {
        inputs: vec![source],
        output_directory: output_directory.clone(),
        recursive: false,
    })
    .expect("conversion plan");

    assert_eq!(plan.items.len(), 1);
    let item = &plan.items[0];
    assert_eq!(item.source_name, "performance.safetensors");
    assert_eq!(item.relative_output, PathBuf::from("performance.lc"));
    assert_eq!(item.status, ConversionStatus::Ready);
    let metadata = item.metadata.as_ref().expect("preflight metadata");
    assert_eq!(metadata.storage_dtype, "F16");
    assert_eq!((metadata.decoded_width, metadata.decoded_height), (16, 16));
    assert_eq!(metadata.decoded_frames, 5);
    assert!(!metadata.audio_present);
    assert!(!output_directory.join("performance.lc").exists());
}

#[test]
fn folder_preflight_recurses_only_when_explicit_and_preserves_relative_outputs() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_directory = directory.path().join("raw");
    let nested = source_directory.join("nested");
    let output_directory = directory.path().join("prepared");
    fs::create_dir_all(&nested).expect("nested source directory");
    fs::create_dir(&output_directory).expect("output directory");
    fs::write(
        source_directory.join("root.safetensors"),
        synthetic_video_payload(),
    )
    .expect("root source");
    fs::write(nested.join("child.safetensors"), synthetic_video_payload()).expect("nested source");

    let shallow = plan_conversion(ConversionPlanRequest {
        inputs: vec![source_directory.clone()],
        output_directory: output_directory.clone(),
        recursive: false,
    })
    .expect("shallow plan");
    let recursive = plan_conversion(ConversionPlanRequest {
        inputs: vec![source_directory],
        output_directory,
        recursive: true,
    })
    .expect("recursive plan");

    assert_eq!(shallow.items.len(), 1);
    assert_eq!(shallow.items[0].relative_output, PathBuf::from("root.lc"));
    assert_eq!(recursive.items.len(), 2);
    assert_eq!(
        recursive
            .items
            .iter()
            .map(|item| item.relative_output.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("nested/child.lc"), PathBuf::from("root.lc")]
    );
}

#[test]
fn multiple_explicit_files_reject_output_name_collisions_before_writing() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let first = directory.path().join("first").join("clip.safetensors");
    let second = directory.path().join("second").join("CLIP.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::create_dir_all(first.parent().expect("first parent")).expect("first directory");
    fs::create_dir_all(second.parent().expect("second parent")).expect("second directory");
    fs::create_dir(&output_directory).expect("output directory");
    fs::write(&first, synthetic_video_payload()).expect("first source");
    fs::write(&second, synthetic_video_payload()).expect("second source");

    let error = plan_conversion(ConversionPlanRequest {
        inputs: vec![first, second],
        output_directory: output_directory.clone(),
        recursive: false,
    })
    .expect_err("output collision");

    assert_eq!(error.code, "conversion.output_collision");
    assert!(error.recoverable);
    assert!(
        !error
            .message
            .contains(directory.path().to_string_lossy().as_ref())
    );
    assert_eq!(fs::read_dir(output_directory).expect("outputs").count(), 0);
}

#[test]
fn preflight_rejects_existing_outputs_without_clobbering_or_partial_batch_writes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let first = directory.path().join("first.safetensors");
    let second = directory.path().join("second.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::create_dir(&output_directory).expect("output directory");
    fs::write(&first, synthetic_video_payload()).expect("first source");
    fs::write(&second, synthetic_video_payload()).expect("second source");
    fs::write(output_directory.join("second.lc"), b"owned output").expect("existing output");

    let error = plan_conversion(ConversionPlanRequest {
        inputs: vec![first, second],
        output_directory: output_directory.clone(),
        recursive: false,
    })
    .expect_err("existing output");

    assert_eq!(error.code, "conversion.output_exists");
    assert!(!output_directory.join("first.lc").exists());
    assert_eq!(
        fs::read(output_directory.join("second.lc")).expect("preserved output"),
        b"owned output"
    );
}

#[test]
fn coordinator_converts_a_preflighted_batch_sequentially_to_valid_cartridges() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source_directory = directory.path().join("raw");
    let nested = source_directory.join("nested");
    let output_directory = directory.path().join("prepared");
    fs::create_dir_all(&nested).expect("nested source directory");
    fs::create_dir(&output_directory).expect("output directory");
    fs::write(
        source_directory.join("first.safetensors"),
        synthetic_video_payload(),
    )
    .expect("first source");
    fs::write(nested.join("second.safetensors"), synthetic_video_payload()).expect("second source");
    let plan = plan_conversion(ConversionPlanRequest {
        inputs: vec![source_directory],
        output_directory: output_directory.clone(),
        recursive: true,
    })
    .expect("conversion plan");
    let coordinator = ConversionCoordinator::from_plan(plan);

    let snapshot = coordinator.run_to_completion().expect("completed batch");

    assert_eq!(snapshot.phase, ConversionPhase::Complete);
    assert_eq!(snapshot.completed, 2);
    assert_eq!(snapshot.failed, 0);
    assert_eq!(snapshot.active_index, None);
    assert!(
        snapshot
            .items
            .iter()
            .all(|item| item.status == ConversionStatus::Complete)
    );
    let first_output = output_directory.join("first.lc");
    let second_output = output_directory.join("nested/second.lc");
    open_validated(&first_output, &ValidationOptions::default()).expect("valid first LC");
    open_validated(&second_output, &ValidationOptions::default()).expect("valid second LC");
    assert!(first_output.is_file());
    assert!(second_output.is_file());
}

#[test]
fn conversion_rejects_a_valid_source_replaced_after_preflight() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("clip.safetensors");
    let output_directory = directory.path().join("prepared");
    let original = synthetic_video_payload();
    fs::write(&source, &original).expect("original raw source");
    fs::create_dir(&output_directory).expect("output directory");
    let plan = plan_conversion(ConversionPlanRequest {
        inputs: vec![source.clone()],
        output_directory: output_directory.clone(),
        recursive: false,
    })
    .expect("approved plan");
    let mut replacement = original;
    *replacement.last_mut().expect("tensor byte") = 1;
    fs::write(source, replacement).expect("valid changed source");

    let snapshot = ConversionCoordinator::from_plan(plan)
        .run_to_completion()
        .expect("batch settles with an item error");

    assert_eq!(snapshot.phase, ConversionPhase::Complete);
    assert_eq!(snapshot.completed, 0);
    assert_eq!(snapshot.failed, 1);
    assert_eq!(snapshot.items[0].status, ConversionStatus::Failed);
    assert_eq!(
        snapshot.items[0]
            .error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("payload_hash_mismatch")
    );
    assert!(!output_directory.join("clip.lc").exists());
}

#[test]
fn planned_snapshot_serializes_for_the_ui_without_absolute_paths() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("clip.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::write(&source, synthetic_video_payload()).expect("raw H3 source");
    fs::create_dir(&output_directory).expect("output directory");
    let coordinator = ConversionCoordinator::from_plan(
        plan_conversion(ConversionPlanRequest {
            inputs: vec![source],
            output_directory,
            recursive: false,
        })
        .expect("conversion plan"),
    );

    let value = serde_json::to_value(coordinator.snapshot().expect("snapshot"))
        .expect("serializable snapshot");

    assert_eq!(value["phase"], "planned");
    assert_eq!(value["items"][0]["sourceName"], "clip.safetensors");
    assert_eq!(value["items"][0]["relativeOutput"], "clip.lc");
    assert_eq!(value["items"][0]["status"], "ready");
    assert_eq!(value["items"][0]["metadata"]["decodedWidth"], 16);
    let json = value.to_string();
    assert!(!json.contains(directory.path().to_string_lossy().as_ref()));
    assert!(!json.contains("sourcePath"));
    assert!(!json.contains("outputPath"));
}

#[test]
fn converted_output_is_resolved_from_trusted_state_only_after_success() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("clip.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::write(&source, synthetic_video_payload()).expect("raw H3 source");
    fs::create_dir(&output_directory).expect("output directory");
    let coordinator = ConversionCoordinator::from_plan(
        plan_conversion(ConversionPlanRequest {
            inputs: vec![source],
            output_directory: output_directory.clone(),
            recursive: false,
        })
        .expect("conversion plan"),
    );

    let unavailable = coordinator
        .completed_output(0)
        .expect_err("not converted yet");
    assert_eq!(unavailable.code, "conversion.output_unavailable");
    coordinator.run_to_completion().expect("conversion");
    assert_eq!(
        coordinator.completed_output(0).expect("trusted output"),
        output_directory.join("clip.lc")
    );
}

#[test]
fn preflight_keeps_valid_metadata_and_path_free_errors_on_each_item() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let valid = directory.path().join("valid.safetensors");
    let invalid = directory.path().join("invalid.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::write(&valid, synthetic_video_payload()).expect("valid raw source");
    fs::write(&invalid, b"not safetensors").expect("invalid raw source");
    fs::create_dir(&output_directory).expect("output directory");

    let coordinator = ConversionCoordinator::from_plan(
        plan_conversion(ConversionPlanRequest {
            inputs: vec![valid, invalid],
            output_directory: output_directory.clone(),
            recursive: false,
        })
        .expect("item-level preflight"),
    );
    let snapshot = coordinator.snapshot().expect("wire snapshot");

    let valid_item = snapshot
        .items
        .iter()
        .find(|item| item.source_name == "valid.safetensors")
        .expect("valid item");
    let invalid_item = snapshot
        .items
        .iter()
        .find(|item| item.source_name == "invalid.safetensors")
        .expect("invalid item");
    assert_eq!(valid_item.status, ConversionStatus::Ready);
    assert!(valid_item.metadata.is_some());
    assert!(valid_item.error.is_none());
    assert_eq!(invalid_item.status, ConversionStatus::Failed);
    assert!(invalid_item.metadata.is_none());
    let error = invalid_item.error.as_ref().expect("preflight error");
    assert!(!error.code.is_empty());
    assert!(
        !error
            .message
            .contains(directory.path().to_string_lossy().as_ref())
    );
    let wire = serde_json::to_string(&snapshot).expect("wire JSON");
    assert!(!wire.contains(directory.path().to_string_lossy().as_ref()));
    assert!(!wire.contains("sourcePath"));
    assert!(!wire.contains("outputPath"));
    assert_eq!(fs::read_dir(output_directory).expect("outputs").count(), 0);
}

#[test]
fn an_all_failed_preflight_cannot_start_an_empty_conversion() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let invalid = directory.path().join("invalid.safetensors");
    let output_directory = directory.path().join("prepared");
    fs::write(&invalid, b"not safetensors").expect("invalid raw source");
    fs::create_dir(&output_directory).expect("output directory");
    let coordinator = ConversionCoordinator::from_plan(
        plan_conversion(ConversionPlanRequest {
            inputs: vec![invalid],
            output_directory,
            recursive: false,
        })
        .expect("item-level preflight"),
    );

    let error = coordinator
        .run_to_completion()
        .expect_err("no ready inputs");

    assert_eq!(error.code, "conversion.no_ready_inputs");
    assert_eq!(
        coordinator.snapshot().expect("snapshot").phase,
        ConversionPhase::Planned
    );
}
