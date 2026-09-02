#![allow(dead_code)]

use std::io::Cursor;

use latentdeck_cartridge::{
    archive::{EntryWrite, payload_crc32, write_canonical},
    hash::hash_reader,
    limits::ValidationLimits,
    manifest::{ManifestV0_1, PreviewDescriptor, Sha256Digest, parse_manifest_json},
    writer::canonical_json_bytes,
};

pub fn synthetic_video_payload() -> Vec<u8> {
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
    payload
}

pub fn synthetic_av_f32_payload() -> Vec<u8> {
    let audio = vec![0_u8; 32 * 2 * 405 * 4];
    let video = vec![0_u8; 24 * 72 * 4];
    let audio_end = audio.len();
    let video_end = audio_end + video.len();
    let mut header = format!(
        concat!(
            r#"{{"audio":{{"data_offsets":[0,{}],"dtype":"F32","shape":[1,32,2,405]}},"#,
            r#""video":{{"data_offsets":[{},{}],"dtype":"F32","shape":[1,24,72,1,1]}}}}"#
        ),
        audio_end, audio_end, video_end
    )
    .into_bytes();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    let mut payload = Vec::with_capacity(8 + header.len() + video_end);
    payload.extend_from_slice(
        &u64::try_from(header.len())
            .expect("synthetic header length")
            .to_le_bytes(),
    );
    payload.extend_from_slice(&header);
    payload.extend_from_slice(&audio);
    payload.extend_from_slice(&video);
    payload
}

pub fn synthetic_non_h3_payload() -> Vec<u8> {
    let tensor_bytes = vec![0_u8; 7 * 3 * 4];
    let mut header = format!(
        r#"{{"latent_state":{{"data_offsets":[0,{}],"dtype":"F32","shape":[1,7,1,3,1]}}}}"#,
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
    payload
}

pub fn synthetic_preview() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&22_u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(b"VP8X");
    bytes.extend_from_slice(&10_u32.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    bytes.extend_from_slice(&447_u32.to_le_bytes()[..3]);
    bytes.extend_from_slice(&799_u32.to_le_bytes()[..3]);
    bytes
}

pub fn synthetic_manifest(payload: &[u8]) -> ManifestV0_1 {
    let measured = hash_reader(&mut Cursor::new(payload)).expect("synthetic payload hash");
    let value = serde_json::json!({
        "spec_version": "0.1.0",
        "cartridge_id": "550e8400-e29b-41d4-a716-446655440000",
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
            "created_by": {"name": "latentdeck-cartridge", "version": "0.1.0"},
            "sources": []
        },
        "parent_cartridges": [],
        "operation_history": []
    });
    let bytes = serde_json::to_vec(&value).expect("synthetic manifest JSON");
    parse_manifest_json(&bytes, &ValidationLimits::default()).expect("synthetic manifest")
}

pub fn synthetic_av_f32_manifest(payload: &[u8]) -> ManifestV0_1 {
    let measured = hash_reader(&mut Cursor::new(payload)).expect("synthetic payload hash");
    let value = serde_json::json!({
        "spec_version": "0.1.0",
        "cartridge_id": "550e8400-e29b-41d4-a716-446655440001",
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
        "tensors": [
            {
                "stream": "visual",
                "name": "video",
                "payload": "payloads/h3.safetensors",
                "storage_dtype": "F32",
                "runtime_dtype": "F16",
                "shape": [1, 24, 72, 1, 1]
            },
            {
                "stream": "audio",
                "name": "audio",
                "payload": "payloads/h3.safetensors",
                "storage_dtype": "F32",
                "runtime_dtype": "F32",
                "shape": [1, 32, 2, 405]
            }
        ],
        "timing": {
            "contract": "minimax_h3_causal",
            "contract_version": "0.1.0",
            "decoded_video": {
                "width": 16,
                "height": 16,
                "frame_count": 243,
                "frame_rate": {"numerator": 24, "denominator": 1},
                "duration": {"numerator": 81, "denominator": 8}
            }
        },
        "audio": {"policy": "preserved_source"},
        "provenance": {
            "created_by": {"name": "latentdeck-cartridge", "version": "0.1.0"},
            "sources": []
        },
        "parent_cartridges": [],
        "operation_history": []
    });
    let bytes = serde_json::to_vec(&value).expect("synthetic AV manifest JSON");
    parse_manifest_json(&bytes, &ValidationLimits::default()).expect("synthetic AV manifest")
}

pub fn synthetic_non_h3_manifest(payload: &[u8]) -> ManifestV0_1 {
    let measured = hash_reader(&mut Cursor::new(payload)).expect("synthetic payload hash");
    let value = serde_json::json!({
        "spec_version": "0.1.0",
        "cartridge_id": "550e8400-e29b-41d4-a716-446655440002",
        "codec": {
            "family": "synthetic_test",
            "profile": "non_h3_latent",
            "profile_version": "0.2.0"
        },
        "payloads": [{
            "path": "payloads/synthetic.safetensors",
            "media_type": "application/vnd.safetensors",
            "byte_length": measured.byte_length,
            "sha256": measured.sha256.to_string()
        }],
        "tensors": [{
            "stream": "visual",
            "name": "latent_state",
            "payload": "payloads/synthetic.safetensors",
            "storage_dtype": "F32",
            "runtime_dtype": "F32",
            "shape": [1, 7, 1, 3, 1]
        }],
        "timing": {
            "contract": "synthetic_step",
            "contract_version": "0.2.0",
            "decoded_video": {
                "width": 3,
                "height": 1,
                "frame_count": 1,
                "frame_rate": {"numerator": 1, "denominator": 1},
                "duration": {"numerator": 1, "denominator": 1}
            }
        },
        "audio": {"policy": "source_absent"},
        "provenance": {
            "created_by": {"name": "latentdeck-cartridge", "version": "0.2.0-test"},
            "sources": []
        },
        "parent_cartridges": [],
        "operation_history": []
    });
    let bytes = serde_json::to_vec(&value).expect("synthetic manifest JSON");
    parse_manifest_json(&bytes, &ValidationLimits::default()).expect("synthetic manifest")
}

pub fn with_preview(mut manifest: ManifestV0_1, preview: &[u8]) -> ManifestV0_1 {
    let measured = hash_reader(&mut Cursor::new(preview)).expect("synthetic preview hash");
    manifest.preview = Some(PreviewDescriptor {
        path: "preview.webp".to_owned(),
        media_type: "image/webp".to_owned(),
        byte_length: measured.byte_length,
        sha256: Sha256Digest(measured.sha256.to_string()),
        width: 448,
        height: 800,
    });
    manifest
}

pub fn synthetic_lc() -> (Vec<u8>, Vec<u8>, ManifestV0_1) {
    let payload = synthetic_video_payload();
    let manifest = synthetic_manifest(&payload);
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
    let mut output = Cursor::new(Vec::new());
    write_canonical(&mut output, &mut entries).expect("synthetic LC archive");
    (output.into_inner(), payload, manifest)
}

pub fn synthetic_non_h3_lc() -> (Vec<u8>, Vec<u8>, ManifestV0_1) {
    let payload = synthetic_non_h3_payload();
    let manifest = synthetic_non_h3_manifest(&payload);
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
            "payloads/synthetic.safetensors",
            u64::try_from(payload.len()).expect("payload length"),
            payload_crc32(&payload),
            &mut payload_reader,
        ),
    ];
    let mut output = Cursor::new(Vec::new());
    write_canonical(&mut output, &mut entries).expect("synthetic LC archive");
    (output.into_inner(), payload, manifest)
}
