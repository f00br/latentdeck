mod support;

use std::fs;
use std::process::Command;

use latentdeck_cartridge::writer::canonical_json_bytes;

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_latentdeck-cartridge"))
        .args(arguments)
        .output()
        .expect("run cartridge CLI")
}

#[test]
fn cli_packs_inspects_validates_and_hashes_synthetic_cartridge() {
    let payload = support::synthetic_video_payload();
    let manifest = support::synthetic_manifest(&payload);
    let directory = tempfile::tempdir().expect("temporary directory");
    let payload_path = directory.path().join("input.safetensors");
    let manifest_path = directory.path().join("manifest.json");
    let output_path = directory.path().join("output.lc");
    fs::write(&payload_path, payload).expect("payload");
    fs::write(
        &manifest_path,
        canonical_json_bytes(&manifest).expect("canonical manifest"),
    )
    .expect("manifest");

    let pack = run(&[
        "pack",
        "--manifest",
        manifest_path.to_str().expect("manifest path"),
        "--payload",
        payload_path.to_str().expect("payload path"),
        "--output",
        output_path.to_str().expect("output path"),
    ]);
    assert!(
        pack.status.success(),
        "{}",
        String::from_utf8_lossy(&pack.stderr)
    );
    let packed: serde_json::Value = serde_json::from_slice(&pack.stdout).expect("pack JSON");
    assert_eq!(packed["status"], "ok");
    assert_eq!(packed["validation"]["validation_level"], "full");

    for command in ["inspect", "validate", "hash"] {
        let result = run(&[command, output_path.to_str().expect("output path")]);
        assert!(
            result.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let value: serde_json::Value =
            serde_json::from_slice(&result.stdout).expect("command JSON");
        assert_eq!(value["status"], "ok");
    }
}

#[test]
fn cli_uses_stable_error_json_and_exit_status() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let invalid = directory.path().join("invalid.lc");
    fs::write(&invalid, b"not a cartridge").expect("invalid input");
    let result = run(&["validate", invalid.to_str().expect("invalid path")]);
    assert_eq!(result.status.code(), Some(3));
    let value: serde_json::Value =
        serde_json::from_slice(&result.stderr).expect("structured error JSON");
    assert_eq!(value["status"], "error");
    assert_eq!(value["code"], "archive_malformed");
}
