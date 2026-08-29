mod support;

use std::fs;
use std::io::Read;

use latentdeck_cartridge::reader::{
    InspectOptions, ValidationLevel, ValidationOptions, inspect_path, open_validated,
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
