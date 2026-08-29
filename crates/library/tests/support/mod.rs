#![allow(dead_code)]

use std::{fs, io::Cursor, path::Path};

use latentdeck_cartridge::{
    archive::{EntryWrite, payload_crc32, write_canonical},
    hash::hash_reader,
    limits::ValidationLimits,
    manifest::parse_manifest_json,
    writer::canonical_json_bytes,
};

pub fn write_synthetic_lc(path: &Path, cartridge_id: &str) -> Vec<u8> {
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
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("synthetic header length")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&tensor_bytes);

    let measured = hash_reader(&mut Cursor::new(&payload)).expect("payload hash");
    let manifest_value = serde_json::json!({
        "spec_version": "0.1.0",
        "cartridge_id": cartridge_id,
        "codec": {
            "family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0"
        },
        "payloads": [{
            "path": "payloads/h3.safetensors",
            "media_type": "application/vnd.safetensors",
            "byte_length": measured.byte_length,
            "sha256": measured.sha256.to_string()
        }],
        "tensors": [{
            "stream": "visual",
            "name": "video",
            "payload": "payloads/h3.safetensors",
            "storage_dtype": "F16",
            "runtime_dtype": "F16",
            "shape": [1, 24, 2, 1, 1]
        }],
        "timing": {
            "contract": "minimax_h3_causal",
            "contract_version": "0.1.0",
            "decoded_video": {
                "width": 16,
                "height": 16,
                "frame_count": 5,
                "frame_rate": {"numerator": 24, "denominator": 1},
                "duration": {"numerator": 5, "denominator": 24}
            }
        },
        "audio": {"policy": "source_absent"},
        "provenance": {
            "created_by": {"name": "latentdeck-library-tests", "version": "0.1.0"},
            "sources": []
        },
        "parent_cartridges": [],
        "operation_history": []
    });
    let raw_manifest = serde_json::to_vec(&manifest_value).expect("manifest JSON");
    let manifest = parse_manifest_json(&raw_manifest, &ValidationLimits::default())
        .expect("synthetic manifest");
    let manifest_bytes = canonical_json_bytes(&manifest).expect("canonical manifest");

    let mut manifest_reader = Cursor::new(&manifest_bytes);
    let mut payload_reader = Cursor::new(&payload);
    let mut entries = [
        EntryWrite::new(
            "manifest.json",
            u64::try_from(manifest_bytes.len()).expect("manifest length"),
            payload_crc32(&manifest_bytes),
            &mut manifest_reader,
        ),
        EntryWrite::new(
            "payloads/h3.safetensors",
            u64::try_from(payload.len()).expect("payload length"),
            payload_crc32(&payload),
            &mut payload_reader,
        ),
    ];
    let mut archive = Cursor::new(Vec::new());
    write_canonical(&mut archive, &mut entries).expect("synthetic archive");
    let bytes = archive.into_inner();
    fs::write(path, &bytes).expect("write synthetic cartridge");
    bytes
}

pub const ID_A: &str = "550e8400-e29b-41d4-a716-446655440000";
pub const ID_B: &str = "550e8400-e29b-41d4-a716-446655440001";
pub const ID_C: &str = "550e8400-e29b-41d4-a716-446655440002";
