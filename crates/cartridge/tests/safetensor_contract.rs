use std::io::{self, Cursor, Read, Seek, SeekFrom};

use latentdeck_cartridge::{
    error::ErrorCode,
    limits::{MAX_H3_PAYLOAD_BYTES, MAX_SAFETENSORS_HEADER_BYTES, ValidationLimits},
    safetensor::{
        EntryRange, SafetensorDType, preflight_h3_safetensors, preflight_safetensors,
        scan_h3_safetensors_finite, scan_safetensors_finite, validate_h3_safetensors,
    },
};

const VIDEO_ELEMENTS: usize = 24 * 2;

fn padded_header(json: &str) -> Vec<u8> {
    let mut header = json.as_bytes().to_vec();
    while !header.len().is_multiple_of(8) {
        header.push(b' ');
    }
    header
}

fn payload(header_json: &str, data: &[u8]) -> Vec<u8> {
    let header = padded_header(header_json);
    let mut payload = Vec::with_capacity(8 + header.len() + data.len());
    payload.extend_from_slice(&(header.len() as u64).to_le_bytes());
    payload.extend_from_slice(&header);
    payload.extend_from_slice(data);
    payload
}

fn finite_f16(count: usize) -> Vec<u8> {
    vec![0; count * 2]
}

fn finite_f32(count: usize) -> Vec<u8> {
    vec![0; count * 4]
}

struct TrackingCursor {
    inner: Cursor<Vec<u8>>,
    furthest_read_end: u64,
    max_read_request: usize,
}

impl TrackingCursor {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(bytes),
            furthest_read_end: 0,
            max_read_request: 0,
        }
    }
}

impl Read for TrackingCursor {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.max_read_request = self.max_read_request.max(buffer.len());
        let read = self.inner.read(buffer)?;
        self.furthest_read_end = self.furthest_read_end.max(self.inner.position());
        Ok(read)
    }
}

fn preflight_error(header: &str, data: &[u8]) -> latentdeck_cartridge::error::CartridgeError {
    let encoded = payload(header, data);
    preflight_h3_safetensors(
        &mut Cursor::new(&encoded),
        EntryRange::new(0, encoded.len() as u64),
        &ValidationLimits::default(),
    )
    .expect_err("payload must be rejected")
}

impl Seek for TrackingCursor {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

#[test]
fn preflights_finite_f16_video_inside_a_bounded_entry() {
    let video = finite_f16(VIDEO_ELEMENTS);
    let header = format!(
        r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#,
        video.len()
    );
    let encoded = payload(&header, &video);
    let mut container = b"ignored-prefix".to_vec();
    let entry_offset = container.len() as u64;
    container.extend_from_slice(&encoded);
    container.extend_from_slice(b"ignored-suffix");

    let result = validate_h3_safetensors(
        &mut Cursor::new(container),
        EntryRange::new(entry_offset, encoded.len() as u64),
        &ValidationLimits::default(),
    )
    .expect("valid bounded H3 payload");

    assert_eq!(result.payload_bytes, encoded.len() as u64);
    assert_eq!(result.data_bytes, video.len() as u64);
    assert_eq!(result.video.name, "video");
    assert_eq!(result.video.dtype, SafetensorDType::F16);
    assert_eq!(result.video.shape, [1, 24, 2, 1, 1]);
    assert_eq!(result.video.data_offsets, [0, video.len() as u64]);
    assert!(result.audio.is_none());
}

#[test]
fn codec_neutral_preflight_accepts_profile_owned_tensor_names_and_geometry() {
    let data = finite_f32(7 * 3);
    let header = format!(
        r#"{{"latent_state":{{"dtype":"F32","shape":[1,7,1,3,1],"data_offsets":[0,{}]}}}}"#,
        data.len()
    );
    let encoded = payload(&header, &data);
    let entry = EntryRange::new(0, encoded.len() as u64);

    let receipt = preflight_safetensors(
        &mut Cursor::new(&encoded),
        entry,
        "payloads/synthetic.safetensors",
        &ValidationLimits::default(),
    )
    .expect("codec-neutral tensor envelope");
    assert_eq!(receipt.tensors.len(), 1);
    assert_eq!(receipt.tensors["latent_state"].shape, [1, 7, 1, 3, 1]);
    scan_safetensors_finite(
        &mut Cursor::new(&encoded),
        entry,
        "payloads/synthetic.safetensors",
        &receipt,
    )
    .expect("finite profile-owned tensor");

    let error = preflight_h3_safetensors(
        &mut Cursor::new(&encoded),
        entry,
        &ValidationLimits::default(),
    )
    .expect_err("H3 semantics remain a separate validation layer");
    assert_eq!(error.code, ErrorCode::TensorMissing);
}

#[test]
fn preflights_f32_video_with_optional_f32_audio() {
    let video = finite_f32(VIDEO_ELEMENTS);
    let audio = finite_f32(32 * 2);
    let audio_start = video.len();
    let audio_end = audio_start + audio.len();
    let header = format!(
        concat!(
            r#"{{"video":{{"dtype":"F32","shape":[1,24,2,1,1],"data_offsets":[0,{}]}},"#,
            r#""audio":{{"dtype":"F32","shape":[1,32,2,1],"data_offsets":[{},{}]}}}}"#
        ),
        video.len(),
        audio_start,
        audio_end
    );
    let mut data = video;
    data.extend_from_slice(&audio);
    let encoded = payload(&header, &data);

    let result = validate_h3_safetensors(
        &mut Cursor::new(&encoded),
        EntryRange::new(0, encoded.len() as u64),
        &ValidationLimits::default(),
    )
    .expect("valid H3 AV payload");

    assert_eq!(result.video.dtype, SafetensorDType::F32);
    let audio = result.audio.expect("audio descriptor");
    assert_eq!(audio.name, "audio");
    assert_eq!(audio.dtype, SafetensorDType::F32);
    assert_eq!(audio.shape, [1, 32, 2, 1]);
    assert_eq!(audio.data_offsets, [audio_start as u64, audio_end as u64]);
}

#[test]
fn rejects_duplicate_metadata_keys_in_the_untrusted_header() {
    let video = finite_f16(VIDEO_ELEMENTS);
    let header = format!(
        concat!(
            r#"{{"__metadata__":{{"source":"first","source":"second"}},"#,
            r#""video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#
        ),
        video.len()
    );
    let encoded = payload(&header, &video);

    let error = preflight_h3_safetensors(
        &mut Cursor::new(&encoded),
        EntryRange::new(0, encoded.len() as u64),
        &ValidationLimits::default(),
    )
    .expect_err("duplicate metadata key must be rejected");

    assert_eq!(error.code, ErrorCode::SafetensorsInvalid);
}

#[test]
fn structural_preflight_stops_before_data_and_finite_scan_rejects_f16_infinity() {
    let mut video = finite_f16(VIDEO_ELEMENTS);
    video[0..2].copy_from_slice(&0x7c00_u16.to_le_bytes());
    let header = format!(
        r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#,
        video.len()
    );
    let encoded = payload(&header, &video);
    let entry = EntryRange::new(0, encoded.len() as u64);
    let mut tracked = TrackingCursor::new(encoded.clone());

    let preflight = preflight_h3_safetensors(&mut tracked, entry, &ValidationLimits::default())
        .expect("structure itself is valid");

    assert_eq!(tracked.furthest_read_end, preflight.data_offset);

    let error = scan_h3_safetensors_finite(&mut Cursor::new(encoded), entry, &preflight)
        .expect_err("full scan must reject infinity");
    assert_eq!(error.code, ErrorCode::TensorNonFinite);
    assert_eq!(error.location.tensor.as_deref(), Some("video"));
}

#[test]
fn rejects_oversized_malformed_and_duplicate_descriptor_headers() {
    let mut oversized_prefix = Vec::from((MAX_SAFETENSORS_HEADER_BYTES + 8).to_le_bytes());
    let error = preflight_h3_safetensors(
        &mut Cursor::new(&mut oversized_prefix),
        EntryRange::new(0, 8),
        &ValidationLimits::default(),
    )
    .expect_err("oversized header declaration");
    assert_eq!(error.code, ErrorCode::SafetensorsHeaderTooLarge);

    let malformed = preflight_error(r#"{"video":not-json}"#, &[]);
    assert_eq!(malformed.code, ErrorCode::SafetensorsInvalid);

    let video = finite_f16(VIDEO_ELEMENTS);
    let duplicate_field = format!(
        concat!(
            r#"{{"video":{{"dtype":"F16","dtype":"F32","shape":[1,24,2,1,1],"#,
            r#""data_offsets":[0,{}]}}}}"#
        ),
        video.len()
    );
    let error = preflight_error(&duplicate_field, &video);
    assert_eq!(error.code, ErrorCode::SafetensorsInvalid);

    let duplicate_tensor = format!(
        concat!(
            r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}},"#,
            r#""video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#
        ),
        video.len(),
        video.len()
    );
    let error = preflight_error(&duplicate_tensor, &video);
    assert_eq!(error.code, ErrorCode::SafetensorsInvalid);

    let unknown_descriptor_field = format!(
        concat!(
            r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}],"#,
            r#""executable":"never"}}}}"#
        ),
        video.len()
    );
    let error = preflight_error(&unknown_descriptor_field, &video);
    assert_eq!(error.code, ErrorCode::SafetensorsInvalid);
}

#[test]
fn rejects_entry_range_overflow_truncation_and_payload_ceiling_before_allocation() {
    let error = preflight_h3_safetensors(
        &mut Cursor::new(vec![0; 8]),
        EntryRange::new(u64::MAX - 3, 8),
        &ValidationLimits::default(),
    )
    .expect_err("entry-end overflow");
    assert_eq!(error.code, ErrorCode::TensorSizeOverflow);

    let video = finite_f16(VIDEO_ELEMENTS);
    let header = format!(
        r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#,
        video.len()
    );
    let encoded = payload(&header, &video);
    let error = preflight_h3_safetensors(
        &mut Cursor::new(&encoded),
        EntryRange::new(0, encoded.len() as u64 + 1),
        &ValidationLimits::default(),
    )
    .expect_err("entry range beyond stream");
    assert_eq!(error.code, ErrorCode::SafetensorsInvalid);

    let error = preflight_h3_safetensors(
        &mut Cursor::new(Vec::<u8>::new()),
        EntryRange::new(0, MAX_H3_PAYLOAD_BYTES + 1),
        &ValidationLimits::default(),
    )
    .expect_err("payload size ceiling");
    assert_eq!(error.code, ErrorCode::EntryTooLarge);
}

#[test]
fn rejects_forbidden_dtype_missing_unexpected_and_invalid_h3_shapes() {
    let cases = [
        (
            r#"{"video":{"dtype":"BF16","shape":[1,24,2,1,1],"data_offsets":[0,96]}}"#,
            ErrorCode::TensorDtypeForbidden,
        ),
        (
            r#"{"video":{"dtype":"F16","shape":[2,24,2,1,1],"data_offsets":[0,192]}}"#,
            ErrorCode::TensorShapeInvalid,
        ),
        (
            r#"{"audio":{"dtype":"F16","shape":[1,32,2,1],"data_offsets":[0,128]}}"#,
            ErrorCode::TensorMissing,
        ),
        (
            concat!(
                r#"{"video":{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,96]},"#,
                r#""evil":{"dtype":"F16","shape":[1],"data_offsets":[96,98]}}"#
            ),
            ErrorCode::TensorUnexpected,
        ),
        (
            concat!(
                r#"{"video":{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,96]},"#,
                r#""audio":{"dtype":"F16","shape":[1,32,1,2],"data_offsets":[96,224]}}"#
            ),
            ErrorCode::TensorShapeInvalid,
        ),
    ];

    for (header, expected) in cases {
        let data = vec![0; 224];
        let error = preflight_error(header, &data);
        assert_eq!(error.code, expected, "header: {header}");
    }

    let overflow = preflight_error(
        r#"{"video":{"dtype":"F32","shape":[1,24,18446744073709551615,1,1],"data_offsets":[0,0]}}"#,
        &[],
    );
    assert_eq!(overflow.code, ErrorCode::TensorSizeOverflow);
}

#[test]
fn rejects_offset_gaps_overlaps_out_of_range_and_unclaimed_bytes() {
    let cases = [
        (
            concat!(
                r#"{"video":{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,96]},"#,
                r#""audio":{"dtype":"F16","shape":[1,32,2,1],"data_offsets":[98,226]}}"#
            ),
            226,
        ),
        (
            concat!(
                r#"{"video":{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,96]},"#,
                r#""audio":{"dtype":"F16","shape":[1,32,2,1],"data_offsets":[94,222]}}"#
            ),
            222,
        ),
        (
            r#"{"video":{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,96]}}"#,
            95,
        ),
        (
            r#"{"video":{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,96]}}"#,
            97,
        ),
    ];

    for (header, data_bytes) in cases {
        let error = preflight_error(header, &vec![0; data_bytes]);
        assert_eq!(
            error.code,
            ErrorCode::SafetensorsInvalid,
            "header: {header}"
        );
    }
}

#[test]
fn full_validation_rejects_all_f16_non_finite_classes() {
    for bits in [0x7c00_u16, 0xfc00, 0x7e00, 0x7d00] {
        let mut video = finite_f16(VIDEO_ELEMENTS);
        let final_element = video.len() - 2;
        video[final_element..].copy_from_slice(&bits.to_le_bytes());
        let header = format!(
            r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#,
            video.len()
        );
        let encoded = payload(&header, &video);
        let error = validate_h3_safetensors(
            &mut Cursor::new(&encoded),
            EntryRange::new(0, encoded.len() as u64),
            &ValidationLimits::default(),
        )
        .expect_err("F16 non-finite value");
        assert_eq!(error.code, ErrorCode::TensorNonFinite, "bits: {bits:#06x}");
    }
}

#[test]
fn full_validation_rejects_all_f32_non_finite_classes_in_optional_audio() {
    for bits in [0x7f80_0000_u32, 0xff80_0000, 0x7fc0_0000, 0x7f80_0001] {
        let video = finite_f32(VIDEO_ELEMENTS);
        let mut audio = finite_f32(32 * 2);
        audio[0..4].copy_from_slice(&bits.to_le_bytes());
        let audio_start = video.len();
        let audio_end = audio_start + audio.len();
        let header = format!(
            concat!(
                r#"{{"video":{{"dtype":"F32","shape":[1,24,2,1,1],"data_offsets":[0,{}]}},"#,
                r#""audio":{{"dtype":"F32","shape":[1,32,2,1],"data_offsets":[{},{}]}}}}"#
            ),
            video.len(),
            audio_start,
            audio_end
        );
        let mut data = video;
        data.extend_from_slice(&audio);
        let encoded = payload(&header, &data);

        let error = validate_h3_safetensors(
            &mut Cursor::new(&encoded),
            EntryRange::new(0, encoded.len() as u64),
            &ValidationLimits::default(),
        )
        .expect_err("F32 non-finite value in audio");
        assert_eq!(error.code, ErrorCode::TensorNonFinite, "bits: {bits:#010x}");
        assert_eq!(error.location.tensor.as_deref(), Some("audio"));
    }
}

#[test]
fn full_scan_uses_a_bounded_buffer_instead_of_materializing_the_tensor() {
    const TEMPORAL_SLOTS: usize = 2048;
    let video = finite_f32(24 * TEMPORAL_SLOTS);
    let header = format!(
        r#"{{"video":{{"dtype":"F32","shape":[1,24,{TEMPORAL_SLOTS},1,1],"data_offsets":[0,{}]}}}}"#,
        video.len()
    );
    let encoded = payload(&header, &video);
    let entry = EntryRange::new(0, encoded.len() as u64);
    let mut tracked = TrackingCursor::new(encoded);

    validate_h3_safetensors(&mut tracked, entry, &ValidationLimits::default())
        .expect("large finite tensor");

    assert!(tracked.max_read_request <= 64 * 1024);
    assert!(entry.length < MAX_H3_PAYLOAD_BYTES);
}

#[test]
fn finite_scan_rejects_a_receipt_that_does_not_match_the_entry() {
    let video = finite_f16(VIDEO_ELEMENTS);
    let header = format!(
        r#"{{"video":{{"dtype":"F16","shape":[1,24,2,1,1],"data_offsets":[0,{}]}}}}"#,
        video.len()
    );
    let encoded = payload(&header, &video);
    let entry = EntryRange::new(0, encoded.len() as u64);
    let mut cursor = Cursor::new(&encoded);
    let mut preflight = preflight_h3_safetensors(&mut cursor, entry, &ValidationLimits::default())
        .expect("valid structure");
    preflight.data_bytes += 1;

    let error = scan_h3_safetensors_finite(&mut cursor, entry, &preflight)
        .expect_err("forged or stale receipt");
    assert_eq!(error.code, ErrorCode::TensorDescriptorMismatch);
}
