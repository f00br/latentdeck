use std::{fs, path::Path};

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
use tempfile::tempdir;
use uuid::Uuid;

fn safetensors_payload() -> Vec<u8> {
    let mut header =
        br#"{"video":{"dtype":"F16","shape":[1,4,2,1,1],"data_offsets":[0,16]}}"#.to_vec();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut encoded = u64::try_from(header.len()).unwrap().to_le_bytes().to_vec();
    encoded.extend(header);
    encoded.extend([0_u8; 16]);
    encoded
}

fn metadata() -> RawImportMetadata {
    RawImportMetadata {
        profile_key: ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "test_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        },
        payload_entry: "payloads/synthetic.safetensors".to_owned(),
        payload_media_type: "application/vnd.safetensors".to_owned(),
        tensors: LimitedVec::try_from_vec(vec![RawImportTensor {
            stream: RawImportTensorStream::Visual,
            name: "video".to_owned(),
            storage_dtype: RawImportStorageDtype::F16,
            runtime_dtype: RawImportStorageDtype::F16,
            shape: LimitedVec::try_from_vec(vec![1, 4, 2, 1, 1]).unwrap(),
        }])
        .unwrap(),
        timing_contract: "synthetic_ticks".to_owned(),
        timing_contract_version: "0.1.0".to_owned(),
        decoded_width: 8,
        decoded_height: 8,
        decoded_frame_count: 2,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        duration_numerator: 1,
        duration_denominator: 12,
        audio_policy: RawImportAudioPolicy::SourceAbsent,
    }
}

fn request(staged: &Path) -> RawImportFinalizeRequest {
    let measured = hash_path(staged).unwrap();
    let import_id = Uuid::from_u128(70);
    let receipt_id = Uuid::from_u128(71);
    let metadata = metadata();
    RawImportFinalizeRequest {
        expected: RawImportExpectedAuthority::measure_source(
            "org.example.synthetic",
            "0.2.0",
            "org.example.synthetic",
            "0.2.0",
            staged,
            metadata.profile_key.clone(),
        )
        .unwrap(),
        preflight: RawImportPreflight {
            receipt_id,
            import_id,
            pack_id: "org.example.synthetic".to_owned(),
            pack_version: "0.2.0".to_owned(),
            adapter_id: "org.example.synthetic".to_owned(),
            adapter_version: "0.2.0".to_owned(),
            source_sha256: measured.sha256.to_string(),
            source_byte_length: measured.byte_length,
            metadata,
        },
        artifact: RawImportArtifact {
            receipt_id,
            import_id,
            staged_payload_path: staged.to_string_lossy().into_owned(),
            payload_sha256: measured.sha256.to_string(),
            payload_byte_length: measured.byte_length,
        },
        authoring: RawImportAuthoring::new("latentdeck-test", "0.2.0"),
    }
}

#[test]
fn core_rejects_adapter_receipts_that_do_not_match_host_authority() {
    let temporary = tempdir().unwrap();
    let staging = RawImportStagingRoot::create_in(temporary.path()).unwrap();
    let staged = staging.path().join("adapter-payload.safetensors");
    fs::write(&staged, safetensors_payload()).unwrap();

    let mut cases = Vec::new();
    let mut wrong_pack = request(&staged);
    wrong_pack.preflight.pack_id = "org.example.impostor".to_owned();
    cases.push(wrong_pack);
    let mut wrong_adapter = request(&staged);
    wrong_adapter.preflight.adapter_version = "0.2.1".to_owned();
    cases.push(wrong_adapter);
    let mut wrong_source = request(&staged);
    wrong_source.preflight.source_sha256 = "b".repeat(64);
    cases.push(wrong_source);
    let mut wrong_profile = request(&staged);
    wrong_profile.preflight.metadata.profile_key.profile = "other_latent".to_owned();
    cases.push(wrong_profile);

    for (index, invalid) in cases.iter().enumerate() {
        let output = temporary.path().join(format!("malicious-{index}.lc"));
        let error = finalize_raw_import_atomic(&staging, invalid, &output).unwrap_err();
        assert_eq!(error.stable_code(), "raw_import.authority_mismatch");
        assert!(!output.exists());
        assert!(staged.is_file());
    }
}

#[test]
fn staging_root_rejects_a_link_parent() {
    let temporary = tempdir().unwrap();
    let real = temporary.path().join("real");
    let linked = temporary.path().join("linked");
    fs::create_dir(&real).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&real, &linked).is_err() {
        return;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &linked).unwrap();

    let error = RawImportStagingRoot::create_in(&linked).unwrap_err();
    assert_eq!(error.stable_code(), "raw_import.staging_root_unavailable");
}

#[test]
fn staging_root_rejects_a_regular_child_below_a_linked_ancestor() {
    let temporary = tempdir().unwrap();
    let real = temporary.path().join("real");
    let linked = temporary.path().join("linked");
    fs::create_dir(&real).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(&real, &linked).is_err() {
        return;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let child = linked.join("ordinary-child");
    fs::create_dir(&child).unwrap();

    let error = RawImportStagingRoot::create_in(&child).unwrap_err();

    assert_eq!(error.stable_code(), "raw_import.staging_root_unavailable");
}

#[test]
fn host_remeasurement_detects_same_length_source_rewrite_after_preflight() {
    let temporary = tempdir().unwrap();
    let source = temporary.path().join("raw-source.bin");
    fs::write(&source, b"original-bytes").unwrap();
    let expected = RawImportExpectedAuthority::measure_source(
        "org.example.synthetic",
        "0.2.0",
        "org.example.synthetic",
        "0.2.0",
        &source,
        metadata().profile_key,
    )
    .unwrap();

    fs::write(&source, b"rewritten-byte").unwrap();
    let error = expected.validate_source_unchanged(&source).unwrap_err();

    assert_eq!(error.stable_code(), "raw_import.source_untrusted");
}

#[cfg(windows)]
#[test]
fn retained_staging_root_cannot_be_renamed_while_the_host_owns_it() {
    let temporary = tempdir().unwrap();
    let staging = RawImportStagingRoot::create_in(temporary.path()).unwrap();
    let moved = temporary.path().join("moved-staging");

    assert!(fs::rename(staging.path(), &moved).is_err());
    assert!(staging.path().is_dir());
    assert!(!moved.exists());
}

#[test]
fn core_constructs_reopens_and_atomically_publishes_a_profile_import() {
    let temporary = tempdir().unwrap();
    let staging = RawImportStagingRoot::create_in(temporary.path()).unwrap();
    let staged = staging.path().join("adapter-payload.safetensors");
    fs::write(&staged, safetensors_payload()).unwrap();
    let output = temporary.path().join("imported.lc");

    let receipt = finalize_raw_import_atomic(&staging, &request(&staged), &output)
        .expect("finalize raw import");

    assert_eq!(receipt.output_path, output);
    assert!(output.is_file());
    assert!(
        !staged.exists(),
        "Core consumes only the admitted staged file"
    );
    let validated = open_integrity_validated(&output, &ValidationOptions::default()).unwrap();
    assert_eq!(validated.manifest().codec.family.0, "synthetic");
    assert_eq!(
        validated.manifest().payloads[0].path,
        "payloads/synthetic.safetensors"
    );
    assert_eq!(
        validated.manifest().provenance.created_by.name.0,
        "latentdeck-test"
    );

    let second_staged = staging.path().join("second.safetensors");
    fs::write(&second_staged, safetensors_payload()).unwrap();
    let collision =
        finalize_raw_import_atomic(&staging, &request(&second_staged), &output).unwrap_err();
    assert_eq!(collision.stable_code(), "target_exists");
    assert!(second_staged.is_file());
}

#[test]
fn default_authoring_uses_distinct_host_uuids_for_identical_payloads() {
    let temporary = tempdir().unwrap();
    let staging = RawImportStagingRoot::create_in(temporary.path()).unwrap();
    let mut cartridge_ids = Vec::new();
    for index in 0..2 {
        let staged = staging.path().join(format!("payload-{index}.safetensors"));
        fs::write(&staged, safetensors_payload()).unwrap();
        let output = temporary.path().join(format!("imported-{index}.lc"));
        finalize_raw_import_atomic(&staging, &request(&staged), &output).unwrap();
        let validated = open_integrity_validated(&output, &ValidationOptions::default()).unwrap();
        cartridge_ids.push(validated.manifest().cartridge_id.0.clone());
    }

    assert_ne!(cartridge_ids[0], cartridge_ids[1]);
}

#[test]
fn core_refuses_adapter_paths_outside_its_retained_staging_root() {
    let temporary = tempdir().unwrap();
    let staging = RawImportStagingRoot::create_in(temporary.path()).unwrap();
    let outside = temporary.path().join("outside.safetensors");
    fs::write(&outside, safetensors_payload()).unwrap();
    let output = temporary.path().join("outside.lc");

    let error = finalize_raw_import_atomic(&staging, &request(&outside), &output).unwrap_err();

    assert_eq!(error.stable_code(), "raw_import.staged_path_untrusted");
    assert!(outside.is_file());
    assert!(!output.exists());
}

#[test]
fn core_crosschecks_typed_metadata_against_staged_safetensors() {
    let temporary = tempdir().unwrap();
    let staging = RawImportStagingRoot::create_in(temporary.path()).unwrap();
    let staged = staging.path().join("adapter-payload.safetensors");
    fs::write(&staged, safetensors_payload()).unwrap();
    let output = temporary.path().join("mismatched.lc");
    let mut invalid = request(&staged);
    invalid.preflight.metadata.tensors = LimitedVec::try_from_vec(vec![RawImportTensor {
        stream: RawImportTensorStream::Visual,
        name: "video".to_owned(),
        storage_dtype: RawImportStorageDtype::F16,
        runtime_dtype: RawImportStorageDtype::F16,
        shape: LimitedVec::try_from_vec(vec![1, 8, 2, 1, 1]).unwrap(),
    }])
    .unwrap();

    assert!(finalize_raw_import_atomic(&staging, &invalid, &output).is_err());
    assert!(
        staged.is_file(),
        "failed finalization leaves the spool for abort/recovery"
    );
    assert!(!output.exists());
}
