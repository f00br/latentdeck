mod support;

use std::fs;
use std::io::Read;

use latentdeck_cartridge::access::IntegrityAccessReceipt;
use latentdeck_cartridge::reader::{
    InspectOptions, ValidationLevel, ValidationOptions, inspect_integrity_path, inspect_path,
    open_integrity_validated, open_validated,
};

#[test]
fn inspect_validate_and_read_use_the_same_data_only_cartridge() {
    let (archive, payload, expected_manifest) = support::synthetic_lc();
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("synthetic.lc");
    fs::write(&path, archive).expect("write synthetic LC");

    let inspection = inspect_path(&path, &InspectOptions::default()).expect("structural inspect");
    assert_eq!(inspection.validation_level, ValidationLevel::Structure);
    assert_eq!(inspection.manifest, expected_manifest);

    let mut validated =
        open_validated(&path, &ValidationOptions::default()).expect("full validation");
    assert_eq!(validated.receipt().validation_level, ValidationLevel::Full);
    assert_eq!(
        validated.receipt().payload_sha256.to_string(),
        expected_manifest.payloads[0].sha256.0
    );

    let mut tensor = validated
        .tensor_reader("video")
        .expect("validated tensor stream");
    let mut bytes = Vec::new();
    tensor.read_to_end(&mut bytes).expect("read tensor bytes");
    assert_eq!(bytes, payload[payload.len() - bytes.len()..]);
}

#[test]
fn corrupted_payload_never_yields_a_validated_tensor_reader() {
    let (mut archive, _, _) = support::synthetic_lc();
    let marker = archive
        .windows(8)
        .position(|window| window == b"\0\0\0\0\0\0\0\0")
        .expect("synthetic zero payload");
    archive[marker] ^= 0x01;

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("corrupt.lc");
    fs::write(&path, archive).expect("write corrupt LC");
    let error =
        open_validated(&path, &ValidationOptions::default()).expect_err("corrupt cartridge");
    assert!(
        matches!(
            error.code(),
            "entry_crc_mismatch"
                | "payload_hash_mismatch"
                | "safetensors_invalid"
                | "tensor_non_finite"
        ),
        "unexpected code: {}",
        error.code()
    );
}

#[test]
fn codec_neutral_integrity_boundary_accepts_a_non_h3_profile() {
    let (archive, payload, expected_manifest) = support::synthetic_non_h3_lc();
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("synthetic-non-h3.lc");
    fs::write(&path, archive).expect("write synthetic LC");

    let inspection = inspect_integrity_path(&path, &InspectOptions::default())
        .expect("codec-neutral structural inspect");
    assert_eq!(inspection.validation_level, ValidationLevel::Structure);
    assert_eq!(inspection.manifest, expected_manifest);
    assert_eq!(
        inspection.safetensors.tensors.keys().collect::<Vec<_>>(),
        vec!["latent_state"]
    );

    let h3_error = inspect_path(&path, &InspectOptions::default())
        .expect_err("the legacy H3 boundary must still reject a non-H3 profile");
    assert!(
        matches!(
            h3_error.code(),
            "unsupported_profile" | "manifest_invalid" | "tensor_descriptor_mismatch"
        ),
        "unexpected code: {}",
        h3_error.code()
    );

    let mut validated = open_integrity_validated(&path, &ValidationOptions::default())
        .expect("codec-neutral full validation");
    assert_eq!(validated.receipt().validation_level, ValidationLevel::Full);
    assert_eq!(
        validated.receipt().payload_path,
        "payloads/synthetic.safetensors"
    );
    assert_eq!(validated.receipt().tensor_storage_bytes, 7 * 3 * 4);
    assert_eq!(
        validated.receipt().payload_sha256.to_string(),
        expected_manifest.payloads[0].sha256.0
    );

    let mut tensor = validated
        .tensor_reader("latent_state")
        .expect("validated generic tensor stream");
    let mut bytes = Vec::new();
    tensor.read_to_end(&mut bytes).expect("read tensor bytes");
    assert_eq!(bytes, payload[payload.len() - bytes.len()..]);
}

#[test]
fn retained_handle_access_receipt_round_trips_exact_bounded_ranges() {
    let (archive, _, _) = support::synthetic_non_h3_lc();
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("access-receipt.lc");
    fs::write(&path, &archive).expect("write synthetic LC");
    let validated = open_integrity_validated(&path, &ValidationOptions::default())
        .expect("codec-neutral full validation");

    let encoded = validated
        .access_receipt()
        .canonical_json()
        .expect("canonical access receipt");
    let decoded = IntegrityAccessReceipt::parse_json(
        &encoded,
        u64::try_from(archive.len()).expect("archive length"),
    )
    .expect("strict access receipt");
    assert_eq!(&decoded, validated.access_receipt());
    assert_eq!(
        decoded.tensors.keys().collect::<Vec<_>>(),
        vec!["latent_state"]
    );
}

#[test]
fn malicious_access_receipt_ranges_and_file_length_are_rejected() {
    let (archive, _, _) = support::synthetic_non_h3_lc();
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("malicious-receipt.lc");
    fs::write(&path, &archive).expect("write synthetic LC");
    let validated = open_integrity_validated(&path, &ValidationOptions::default())
        .expect("codec-neutral full validation");
    let archive_length = u64::try_from(archive.len()).expect("archive length");

    let encoded = validated
        .access_receipt()
        .canonical_json()
        .expect("canonical access receipt");
    assert!(IntegrityAccessReceipt::parse_json(&encoded, archive_length + 1).is_err());

    let mut tampered = validated.access_receipt().clone();
    tampered
        .tensors
        .get_mut("latent_state")
        .expect("tensor receipt")
        .offset += 1;
    let encoded_tampered = tampered.canonical_json().expect("encode tampered receipt");
    assert!(IntegrityAccessReceipt::parse_json(&encoded_tampered, archive_length).is_err());
}

#[cfg(windows)]
#[test]
fn retained_integrity_handle_denies_share_write_and_delete() {
    let (archive, _, _) = support::synthetic_non_h3_lc();
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("retained.lc");
    fs::write(&path, archive).expect("write synthetic LC");
    let mut validated = open_integrity_validated(&path, &ValidationOptions::default())
        .expect("retain validated handle");

    let write_error = fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .expect_err("retained handle must deny a share-write open");
    assert_eq!(write_error.raw_os_error(), Some(32));
    let remove_error = fs::remove_file(&path).expect_err("retained handle must deny delete");
    assert_eq!(remove_error.raw_os_error(), Some(32));

    let mut tensor = validated
        .tensor_reader("latent_state")
        .expect("retained tensor remains readable");
    let mut bytes = Vec::new();
    tensor
        .read_to_end(&mut bytes)
        .expect("read retained tensor");
    assert_eq!(bytes.len(), 7 * 3 * 4);
}
