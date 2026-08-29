use std::io::Cursor;

use latentdeck_cartridge::archive::{
    EntryWrite, inspect_canonical, payload_crc32, verify_entry, write_canonical,
};

const MANIFEST: &[u8] = br#"{"spec_version":"0.1.0"}"#;
const PAYLOAD: &[u8] = b"synthetic-safetensors-payload";

fn write_minimal_archive() -> Vec<u8> {
    let mut manifest_reader = Cursor::new(MANIFEST);
    let mut payload_reader = Cursor::new(PAYLOAD);
    let mut entries = [
        EntryWrite::new(
            "manifest.json",
            u64::try_from(MANIFEST.len()).expect("fixture length"),
            payload_crc32(MANIFEST),
            &mut manifest_reader,
        ),
        EntryWrite::new(
            "payloads/h3.safetensors",
            u64::try_from(PAYLOAD.len()).expect("fixture length"),
            payload_crc32(PAYLOAD),
            &mut payload_reader,
        ),
    ];
    let mut output = Cursor::new(Vec::new());
    write_canonical(&mut output, &mut entries).expect("canonical archive");
    output.into_inner()
}

#[test]
fn canonical_zip64_roundtrip_is_byte_deterministic() {
    let first = write_minimal_archive();
    let second = write_minimal_archive();
    assert_eq!(first, second);

    let mut input = Cursor::new(first);
    let index = inspect_canonical(&mut input, 1024 * 1024).expect("canonical index");
    assert_eq!(index.entries.len(), 2);
    assert_eq!(index.entries[0].name, "manifest.json");
    assert_eq!(index.entries[1].name, "payloads/h3.safetensors");
    verify_entry(&mut input, &index.entries[0]).expect("manifest CRC");
    verify_entry(&mut input, &index.entries[1]).expect("payload CRC");
}

#[test]
fn writer_rejects_non_contract_paths_before_output() {
    let mut manifest_reader = Cursor::new(MANIFEST);
    let mut payload_reader = Cursor::new(PAYLOAD);
    let mut entries = [
        EntryWrite::new(
            "manifest.json",
            u64::try_from(MANIFEST.len()).expect("fixture length"),
            payload_crc32(MANIFEST),
            &mut manifest_reader,
        ),
        EntryWrite::new(
            "../payloads/h3.safetensors",
            u64::try_from(PAYLOAD.len()).expect("fixture length"),
            payload_crc32(PAYLOAD),
            &mut payload_reader,
        ),
    ];
    let mut output = Cursor::new(Vec::new());
    let error = write_canonical(&mut output, &mut entries).expect_err("unsafe path");
    assert_eq!(error.code(), "entry_unsafe_path");
    assert!(output.get_ref().is_empty());
}

#[test]
fn reader_rejects_trailing_bytes() {
    let mut bytes = write_minimal_archive();
    bytes.push(0);
    let mut input = Cursor::new(bytes);
    let error = inspect_canonical(&mut input, 1024 * 1024).expect_err("trailing data");
    assert_eq!(error.code(), "archive_trailing_data");
}

#[test]
fn reader_rejects_encryption_and_compression_before_payload_read() {
    let mut encrypted = write_minimal_archive();
    encrypted[6..8].copy_from_slice(&1_u16.to_le_bytes());
    let error = inspect_canonical(&mut Cursor::new(encrypted), 1024 * 1024)
        .expect_err("encrypted local entry");
    assert_eq!(error.code(), "entry_encrypted");

    let mut compressed = write_minimal_archive();
    compressed[8..10].copy_from_slice(&8_u16.to_le_bytes());
    let error = inspect_canonical(&mut Cursor::new(compressed), 1024 * 1024)
        .expect_err("compressed local entry");
    assert_eq!(error.code(), "entry_compressed");
}

#[test]
fn reader_rejects_missing_zip64_and_local_central_mismatch() {
    let mut missing_zip64 = write_minimal_archive();
    let zip64_signature = 0x0606_4b50_u32.to_le_bytes();
    let position = missing_zip64
        .windows(zip64_signature.len())
        .position(|window| window == zip64_signature)
        .expect("ZIP64 signature");
    missing_zip64[position] = 0;
    let error = inspect_canonical(&mut Cursor::new(missing_zip64), 1024 * 1024)
        .expect_err("missing ZIP64 record");
    assert_eq!(error.code(), "zip64_required");

    let mut mismatched = write_minimal_archive();
    let payload_name = b"payloads/h3.safetensors";
    let name_position = mismatched
        .windows(payload_name.len())
        .position(|window| window == payload_name)
        .expect("local payload name");
    mismatched[name_position + 9] = b'x';
    let error = inspect_canonical(&mut Cursor::new(mismatched), 1024 * 1024)
        .expect_err("local and central names differ");
    assert_eq!(error.code(), "entry_size_mismatch");
}

#[test]
fn payload_crc_is_verified_without_extraction() {
    let mut bytes = write_minimal_archive();
    let mut input = Cursor::new(bytes.clone());
    let index = inspect_canonical(&mut input, 1024 * 1024).expect("canonical index");
    let payload = &index.entries[1];
    let payload_start = usize::try_from(payload.data_offset).expect("fixture offset");
    bytes[payload_start] ^= 0xff;

    let mut corrupted = Cursor::new(bytes);
    let index = inspect_canonical(&mut corrupted, 1024 * 1024).expect("structure is intact");
    let error = verify_entry(&mut corrupted, &index.entries[1]).expect_err("CRC mismatch");
    assert_eq!(error.code(), "entry_crc_mismatch");
}

#[test]
fn archive_limit_is_checked_before_entry_parsing() {
    let bytes = write_minimal_archive();
    let maximum = u64::try_from(bytes.len() - 1).expect("fixture length");
    let error =
        inspect_canonical(&mut Cursor::new(bytes), maximum).expect_err("archive size ceiling");
    assert_eq!(error.code(), "archive_too_large");
}
