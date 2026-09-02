use std::{fs, path::Path};

use latentdeck_control::v2::{Command, ControlBinding, ControlValue, RoleBinding, SourceBinding};
use latentdeck_core::deck_runtime_v2::{ActiveDeckRuntime, DeckLoadRequest, DeckRuntimeError};
use latentdeck_extension_manager::{
    ExtensionRoots, InstallRequest, PackRequest, enable, install, pack, resolve_active,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

const DECK_ID: &str = "com.example.external.deck";
const DECK_VERSION: &str = "0.2.0";
const OPERATOR_ID: &str = "com.example.external.average";
const OPERATOR_VERSION: &str = "0.2.0";
const ENTRYPOINT: &str = "external_deck:process_sources";

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let path = relative
        .split('/')
        .fold(root.to_path_buf(), |path, component| path.join(component));
    fs::create_dir_all(path.parent().expect("file parent")).expect("create file parent");
    fs::write(path, bytes).expect("write package source file");
}

fn operator_bytes(operator_overrides: &[(&str, Value)]) -> Vec<u8> {
    let mut operator = json!({
        "schema_version": "0.2.0",
        "deck_operator_api": "0.2.0",
        "deck_id": DECK_ID,
        "deck_version": DECK_VERSION,
        "operator_id": OPERATOR_ID,
        "operator_version": OPERATOR_VERSION,
        "entrypoint": ENTRYPOINT,
        "source_count": 2,
        "role_ids": ["carrier", "donor"],
        "controls": [
            {
                "control_id": "mix",
                "value_type": "number",
                "default": 0.5,
                "minimum": 0.0,
                "maximum": 1.0,
                "step": 0.01
            }
        ]
    });
    for (field, value) in operator_overrides {
        operator
            .as_object_mut()
            .expect("operator object")
            .insert((*field).to_owned(), value.clone());
    }
    serde_json::to_vec(&operator).expect("operator JSON")
}

fn package_source(root: &Path, operator_overrides: &[(&str, Value)]) {
    fs::create_dir_all(root).expect("create package source");
    let license = b"test-only notice\n".to_vec();
    let faceplate = serde_json::to_vec(&json!({"widgets": []})).expect("faceplate JSON");
    let operator = operator_bytes(operator_overrides);
    let python =
        b"def process_sources(sources, controls, context):\n    return sources[0]\n".to_vec();
    let files = [
        ("LICENSE.txt", license),
        ("faceplate.json", faceplate),
        ("operator.json", operator),
        ("python/external_deck.py", python),
    ];
    for (path, bytes) in &files {
        write_file(root, path, bytes);
    }

    let integrity = json!({
        "manifest_version": "1.0.0",
        "files": files
            .iter()
            .map(|(path, bytes)| json!({
                "path": path,
                "byte_length": bytes.len(),
                "sha256": sha256(bytes),
            }))
            .collect::<Vec<_>>(),
    });
    let integrity = serde_json::to_vec(&integrity).expect("integrity JSON");
    write_file(root, "integrity.json", &integrity);
    write_file(root, "deck-pack.json", &manifest_bytes(&integrity));
}

fn manifest_bytes(integrity: &[u8]) -> Vec<u8> {
    let manifest = json!({
        "manifest_version": "1.0.0",
        "kind": "deck_pack",
        "deck_id": DECK_ID,
        "deck_version": DECK_VERSION,
        "display_name": "External Test Deck",
        "summary": "Test-generated dynamic Deck package.",
        "publisher": {
            "name": "Test Publisher",
            "url": null,
            "identity_claim": "self_declared"
        },
        "license": {
            "spdx_or_label": "Apache-2.0",
            "notice_path": "LICENSE.txt"
        },
        "compatibility": {
            "app_min_inclusive": "0.1.0",
            "app_max_exclusive": "1.0.0",
            "deck_host_api": 1,
            "worker_protocol": 2,
            "deck_operator_api": 1,
            "tensor_abi": "latentdeck.tensor.v1",
            "python": {
                "implementation": "cpython",
                "version": "3.13",
                "platform_tag": "win_amd64"
            },
            "torch_exact_build": "2.13.0+cu130"
        },
        "runtime": {
            "kind": "python_operator_stream_v1",
            "operator_descriptor_path": "operator.json",
            "python_root": "python",
            "entrypoint": ENTRYPOINT
        },
        "signal": {
            "slots": 2,
            "roles": [
                {"role_id": "carrier", "display_name": "Carrier"},
                {"role_id": "donor", "display_name": "Donor"}
            ],
            "default_permutation": ["carrier", "donor"],
            "structural_carrier_role": "carrier",
            "geometry_allowlist": [{
                "dtype": "fp16",
                "device": "cuda",
                "batch": 1,
                "channels": 4,
                "temporal": 1,
                "height": 8,
                "width": 8
            }],
            "timing": {
                "frames_per_second_numerator": 24,
                "frames_per_second_denominator": 1,
                "samples_per_slot": 24
            },
            "required_capabilities": [
                "player",
                "realtime",
                "resample",
                "snapshot_capture",
                "live_capture"
            ],
            "profile_allowlist": null
        },
        "faceplate_path": "faceplate.json",
        "integrity": {
            "catalog_path": "integrity.json",
            "catalog_sha256": sha256(integrity)
        }
    });
    serde_json::to_vec(&manifest).expect("manifest JSON")
}

fn active_package(
    temp: &TempDir,
    operator_overrides: &[(&str, Value)],
) -> latentdeck_extension_manager::ActiveInstalledPackage {
    let source = temp.path().join("source");
    let archive = temp.path().join("external.ld");
    package_source(&source, operator_overrides);
    let packed = pack(&PackRequest {
        source_directory: source,
        output_path: archive.clone(),
    })
    .expect("pack test Deck");
    let roots = ExtensionRoots::for_base_root(temp.path().join("installed"));
    install(
        &roots,
        &InstallRequest {
            archive_path: archive,
            expected_sha256: packed.inspection.archive_sha256,
        },
    )
    .expect("install test Deck");
    enable(&roots, &packed.inspection.package).expect("enable test Deck");
    resolve_active(&roots, &packed.inspection.package).expect("active package lease")
}

fn source(physical_slot: u8) -> SourceBinding {
    SourceBinding {
        physical_slot,
        source_id: Uuid::new_v4(),
        cartridge_id: Uuid::new_v4(),
        archive_sha256: format!("{physical_slot:02x}").repeat(32),
        profile_receipt_id: Uuid::new_v4(),
        loop_enabled: true,
    }
}

#[test]
fn installed_external_deck_lease_builds_a_hash_bound_typed_command() {
    let temp = TempDir::new().expect("temp dir");
    let runtime = ActiveDeckRuntime::from_active_package(active_package(&temp, &[]))
        .expect("verified Deck runtime");
    let command = runtime
        .build_load_command(DeckLoadRequest {
            deck_session_id: Uuid::new_v4(),
            sources: vec![source(1), source(2)],
            roles: vec![
                RoleBinding {
                    role: "carrier".to_owned(),
                    physical_slot: 1,
                },
                RoleBinding {
                    role: "donor".to_owned(),
                    physical_slot: 2,
                },
            ],
            controls: vec![ControlBinding {
                name: "mix".to_owned(),
                value: ControlValue::Number(0.25),
            }],
            seed: 7,
            stream_generation: 1,
        })
        .expect("typed deck.load");

    let Command::DeckLoad(load) = command else {
        panic!("expected deck.load command");
    };
    let load = *load;
    let binding = load.runtime.expect("dynamic runtime binding");
    assert_eq!(binding.deck_id, DECK_ID);
    assert_eq!(binding.deck_version, DECK_VERSION);
    assert_eq!(binding.operator_id, OPERATOR_ID);
    assert_eq!(binding.operator_version, OPERATOR_VERSION);
    assert_eq!(binding.entrypoint, ENTRYPOINT);
    assert!(Path::new(&binding.python_root).is_absolute());
    assert!(Path::new(&binding.python_root).is_dir());
    assert_eq!(binding.package_manifest_sha256.len(), 64);
    assert_eq!(binding.integrity_catalog_sha256.len(), 64);
    assert_eq!(runtime.operator_descriptor().source_count, 2);
}

#[test]
fn operator_identity_source_count_and_role_ids_are_crosschecked() {
    let cases = [
        (
            vec![("deck_id", json!("com.example.other.deck"))],
            "deck_id",
        ),
        (vec![("deck_version", json!("0.3.0"))], "deck_version"),
        (
            vec![("entrypoint", json!("other:process_sources"))],
            "entrypoint",
        ),
        (
            vec![("source_count", json!(1)), ("role_ids", json!(["carrier"]))],
            "source_count",
        ),
        (vec![("role_ids", json!(["donor", "carrier"]))], "role_ids"),
    ];
    for (overrides, expected) in cases {
        let temp = TempDir::new().expect("temp dir");
        let error = ActiveDeckRuntime::from_active_package(active_package(&temp, &overrides))
            .expect_err("mismatched operator must fail");
        assert_eq!(error, DeckRuntimeError::ManifestOperatorMismatch(expected));
    }
}

#[test]
fn operator_descriptor_and_load_request_are_closed() {
    let temp = TempDir::new().expect("temp dir");
    let runtime = ActiveDeckRuntime::from_active_package(active_package(
        &temp,
        &[("unexpected", json!(true))],
    ));
    assert!(matches!(
        runtime,
        Err(DeckRuntimeError::OperatorDescriptorInvalid)
    ));

    let temp = TempDir::new().expect("temp dir");
    let runtime = ActiveDeckRuntime::from_active_package(active_package(&temp, &[]))
        .expect("verified Deck runtime");
    let error = runtime
        .build_load_command(DeckLoadRequest {
            deck_session_id: Uuid::new_v4(),
            sources: vec![source(1), source(2)],
            roles: vec![
                RoleBinding {
                    role: "carrier".to_owned(),
                    physical_slot: 1,
                },
                RoleBinding {
                    role: "unknown".to_owned(),
                    physical_slot: 2,
                },
            ],
            controls: Vec::new(),
            seed: 0,
            stream_generation: 1,
        })
        .expect_err("unknown role must fail");
    assert_eq!(error, DeckRuntimeError::LoadRequestInvalid("roles"));
}
