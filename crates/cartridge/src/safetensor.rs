use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{self, Read, Seek, SeekFrom},
};

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};

use crate::{
    error::{CartridgeError, ErrorCode, Result},
    limits::{MAX_SAFETENSORS_HEADER_BYTES, ValidationLimits},
};

const H3_ENTRY_NAME: &str = "payloads/h3.safetensors";
const HEADER_LENGTH_BYTES: usize = 8;
const SCAN_BUFFER_BYTES: usize = 64 * 1024;

/// A seekable, bounded byte range containing one Safetensors payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryRange {
    pub offset: u64,
    pub length: u64,
}

impl EntryRange {
    #[must_use]
    pub const fn new(offset: u64, length: u64) -> Self {
        Self { offset, length }
    }

    fn checked_end(self) -> Result<u64> {
        self.offset.checked_add(self.length).ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::TensorSizeOverflow,
                "Safetensors entry range overflows u64",
            )
            .at_entry(H3_ENTRY_NAME)
        })
    }
}

/// Storage dtypes accepted by the H3 LC 0.1 profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafetensorDType {
    F16,
    F32,
}

impl SafetensorDType {
    const fn byte_width(self) -> u64 {
        match self {
            Self::F16 => 2,
            Self::F32 => 4,
        }
    }
}

/// Validated tensor metadata. Offsets are relative to the Safetensors data area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafetensorTensorDescriptor {
    pub name: String,
    pub dtype: SafetensorDType,
    pub shape: Vec<u64>,
    pub data_offsets: [u64; 2],
    pub byte_length: u64,
}

/// Bounded structural receipt; full validation returns it after the finite-value scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct H3SafetensorsPreflight {
    pub payload_bytes: u64,
    pub header_bytes: u64,
    pub data_offset: u64,
    pub data_bytes: u64,
    pub video: SafetensorTensorDescriptor,
    pub audio: Option<SafetensorTensorDescriptor>,
}

/// Structurally inspect an untrusted H3 Safetensors entry without reading tensor data.
///
/// Only the bounded header is allocated. Exact H3 descriptors, sizes, and offsets are
/// validated against the entry range, but tensor values are deliberately not read.
/// Call [`scan_h3_safetensors_finite`] or [`validate_h3_safetensors`] before runtime use.
///
/// # Errors
///
/// Returns a stable [`CartridgeError`] when the entry range, header, tensor schema,
/// dtype, shape, size, or offsets violate the LC 0.1 H3 contract.
pub fn preflight_h3_safetensors<R: Read + Seek>(
    reader: &mut R,
    entry: EntryRange,
    limits: &ValidationLimits,
) -> Result<H3SafetensorsPreflight> {
    if entry.length > limits.max_h3_payload_bytes() {
        return Err(CartridgeError::new(
            ErrorCode::EntryTooLarge,
            "H3 Safetensors payload exceeds the configured limit",
        )
        .at_entry(H3_ENTRY_NAME));
    }
    if entry.length < HEADER_LENGTH_BYTES as u64 {
        return Err(invalid_safetensors(
            "Safetensors payload is shorter than its header-length prefix",
        ));
    }

    let entry_end = entry.checked_end()?;
    let stream_end = reader.seek(SeekFrom::End(0)).map_err(io_read_error)?;
    if entry_end > stream_end {
        return Err(invalid_safetensors(
            "Safetensors entry range extends beyond the readable stream",
        ));
    }

    seek_to(reader, entry.offset)?;
    let mut encoded_header_length = [0_u8; HEADER_LENGTH_BYTES];
    read_exact_payload(reader, &mut encoded_header_length)?;
    let header_bytes = u64::from_le_bytes(encoded_header_length);
    if header_bytes > MAX_SAFETENSORS_HEADER_BYTES {
        return Err(CartridgeError::new(
            ErrorCode::SafetensorsHeaderTooLarge,
            "Safetensors header exceeds the LC 0.1 ceiling",
        )
        .at_entry(H3_ENTRY_NAME));
    }
    if header_bytes == 0 || header_bytes % 8 != 0 {
        return Err(invalid_safetensors(
            "Safetensors header length must be non-zero and 8-byte aligned",
        ));
    }

    let data_relative_offset = (HEADER_LENGTH_BYTES as u64)
        .checked_add(header_bytes)
        .ok_or_else(|| size_overflow("Safetensors data offset overflows u64"))?;
    if data_relative_offset > entry.length {
        return Err(invalid_safetensors(
            "Safetensors header extends beyond the entry range",
        ));
    }

    let header_length = usize::try_from(header_bytes).map_err(|error| {
        size_overflow("Safetensors header does not fit usize").with_source(error)
    })?;
    let mut header = vec![0_u8; header_length];
    read_exact_payload(reader, &mut header)?;
    if header.first() != Some(&b'{') {
        return Err(invalid_safetensors(
            "Safetensors header must begin with a JSON object",
        ));
    }

    let raw_header = parse_header(&header)?;
    if raw_header.tensors.len() > limits.max_tensors() {
        return Err(CartridgeError::new(
            ErrorCode::TensorUnexpected,
            "H3 Safetensors contains too many tensors",
        )
        .at_entry(H3_ENTRY_NAME));
    }

    let mut tensors = raw_header.tensors;
    let video = tensors
        .remove("video")
        .ok_or_else(|| {
            CartridgeError::new(ErrorCode::TensorMissing, "missing H3 video tensor")
                .at_tensor("video")
        })
        .and_then(|descriptor| validate_video(descriptor, limits))?;
    let audio = tensors
        .remove("audio")
        .map(|descriptor| validate_audio(descriptor, limits))
        .transpose()?;
    if let Some((name, _)) = tensors.into_iter().next() {
        return Err(CartridgeError::new(
            ErrorCode::TensorUnexpected,
            "H3 Safetensors contains an unexpected tensor",
        )
        .at_tensor(name));
    }

    let data_bytes = entry.length - data_relative_offset;
    validate_offsets(&video, audio.as_ref(), data_bytes)?;

    Ok(H3SafetensorsPreflight {
        payload_bytes: entry.length,
        header_bytes,
        data_offset: data_relative_offset,
        data_bytes,
        video,
        audio,
    })
}

/// Perform structural preflight and a streaming non-finite scan in one operation.
///
/// # Errors
///
/// Returns a stable [`CartridgeError`] for every structural failure reported by
/// [`preflight_h3_safetensors`], for truncated data, or if any F16/F32 element is
/// NaN or infinity.
pub fn validate_h3_safetensors<R: Read + Seek>(
    reader: &mut R,
    entry: EntryRange,
    limits: &ValidationLimits,
) -> Result<H3SafetensorsPreflight> {
    let preflight = preflight_h3_safetensors(reader, entry, limits)?;
    scan_h3_safetensors_finite(reader, entry, &preflight)?;
    Ok(preflight)
}

/// Stream-scan an already preflighted entry and reject every F16/F32 NaN or infinity.
///
/// The scan uses a fixed 64 KiB heap buffer and never materializes a complete tensor.
///
/// # Errors
///
/// Returns [`ErrorCode::TensorDescriptorMismatch`] if the supplied receipt does not
/// describe this entry, an I/O/structural error for truncated data, or
/// [`ErrorCode::TensorNonFinite`] when a non-finite value is encountered.
pub fn scan_h3_safetensors_finite<R: Read + Seek>(
    reader: &mut R,
    entry: EntryRange,
    preflight: &H3SafetensorsPreflight,
) -> Result<()> {
    if preflight.payload_bytes != entry.length
        || preflight.data_offset
            != (HEADER_LENGTH_BYTES as u64)
                .checked_add(preflight.header_bytes)
                .ok_or_else(|| size_overflow("Safetensors data offset overflows u64"))?
        || preflight
            .data_offset
            .checked_add(preflight.data_bytes)
            .ok_or_else(|| size_overflow("Safetensors data range overflows u64"))?
            != entry.length
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "Safetensors preflight receipt does not match the entry range",
        )
        .at_entry(H3_ENTRY_NAME));
    }
    let entry_end = entry.checked_end()?;
    let stream_end = reader.seek(SeekFrom::End(0)).map_err(io_read_error)?;
    if entry_end > stream_end {
        return Err(invalid_safetensors(
            "Safetensors entry range extends beyond the readable stream",
        ));
    }
    validate_offsets(
        &preflight.video,
        preflight.audio.as_ref(),
        preflight.data_bytes,
    )?;
    let data_absolute_offset = entry
        .offset
        .checked_add(preflight.data_offset)
        .ok_or_else(|| size_overflow("Safetensors absolute data offset overflows u64"))?;
    scan_tensor(reader, data_absolute_offset, &preflight.video)?;
    if let Some(audio) = &preflight.audio {
        scan_tensor(reader, data_absolute_offset, audio)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTensorDescriptor {
    dtype: String,
    shape: Vec<u64>,
    data_offsets: [u64; 2],
}

#[derive(Debug)]
struct RawHeader {
    tensors: BTreeMap<String, RawTensorDescriptor>,
}

impl<'de> Deserialize<'de> for RawHeader {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawHeaderVisitor;

        impl<'de> Visitor<'de> for RawHeaderVisitor {
            type Value = RawHeader;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a Safetensors header object")
            }

            fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut seen = BTreeSet::new();
                let mut tensors = BTreeMap::new();
                while let Some(name) = map.next_key::<String>()? {
                    if !seen.insert(name.clone()) {
                        return Err(de::Error::custom(format_args!(
                            "duplicate Safetensors key {name}"
                        )));
                    }
                    if name == "__metadata__" {
                        let _: UniqueMetadata = map.next_value()?;
                    } else {
                        tensors.insert(name, map.next_value()?);
                    }
                }
                Ok(RawHeader { tensors })
            }
        }

        deserializer.deserialize_map(RawHeaderVisitor)
    }
}

struct UniqueMetadata;

impl<'de> Deserialize<'de> for UniqueMetadata {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UniqueMetadataVisitor;

        impl<'de> Visitor<'de> for UniqueMetadataVisitor {
            type Value = UniqueMetadata;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string-to-string Safetensors metadata object")
            }

            fn visit_map<M>(self, mut map: M) -> std::result::Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut seen = BTreeSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(de::Error::custom(format_args!(
                            "duplicate Safetensors metadata key {key}"
                        )));
                    }
                    let _: String = map.next_value()?;
                }
                Ok(UniqueMetadata)
            }
        }

        deserializer.deserialize_map(UniqueMetadataVisitor)
    }
}

fn parse_header(header: &[u8]) -> Result<RawHeader> {
    let mut deserializer = serde_json::Deserializer::from_slice(header);
    let parsed = RawHeader::deserialize(&mut deserializer).map_err(|error| {
        invalid_safetensors("Safetensors header JSON or descriptor is invalid").with_source(error)
    })?;
    deserializer.end().map_err(|error| {
        invalid_safetensors("Safetensors header has trailing non-whitespace data")
            .with_source(error)
    })?;
    Ok(parsed)
}

fn validate_video(
    raw: RawTensorDescriptor,
    limits: &ValidationLimits,
) -> Result<SafetensorTensorDescriptor> {
    let descriptor = validate_common("video", raw, limits)?;
    if descriptor.shape.len() != 5
        || descriptor.shape[0] != 1
        || descriptor.shape[1] != 24
        || descriptor.shape[2..].contains(&0)
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "H3 video shape must be [1,24,T,H,W] with non-zero axes",
        )
        .at_tensor("video"));
    }
    validate_h3_axes(&descriptor, limits, 2)?;
    Ok(descriptor)
}

fn validate_audio(
    raw: RawTensorDescriptor,
    limits: &ValidationLimits,
) -> Result<SafetensorTensorDescriptor> {
    let descriptor = validate_common("audio", raw, limits)?;
    if descriptor.shape.len() != 4
        || descriptor.shape[0] != 1
        || descriptor.shape[1] != 32
        || descriptor.shape[2] != 2
        || descriptor.shape[3] == 0
    {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "H3 audio shape must be [1,32,2,T_audio] with non-zero T_audio",
        )
        .at_tensor("audio"));
    }
    validate_h3_axes(&descriptor, limits, 3)?;
    Ok(descriptor)
}

fn validate_common(
    name: &str,
    raw: RawTensorDescriptor,
    limits: &ValidationLimits,
) -> Result<SafetensorTensorDescriptor> {
    if raw.shape.is_empty() || raw.shape.len() > limits.max_tensor_rank() {
        return Err(CartridgeError::new(
            ErrorCode::TensorShapeInvalid,
            "tensor rank is outside the LC 0.1 limits",
        )
        .at_tensor(name));
    }
    let dtype = match raw.dtype.as_str() {
        "F16" => SafetensorDType::F16,
        "F32" => SafetensorDType::F32,
        _ => {
            return Err(CartridgeError::new(
                ErrorCode::TensorDtypeForbidden,
                "H3 Safetensors accepts only F16 or F32 storage",
            )
            .at_tensor(name));
        }
    };
    let elements = raw.shape.iter().try_fold(1_u64, |product, axis| {
        product
            .checked_mul(*axis)
            .ok_or_else(|| size_overflow("tensor element count overflows u64").at_tensor(name))
    })?;
    let byte_length = elements
        .checked_mul(dtype.byte_width())
        .ok_or_else(|| size_overflow("tensor byte length overflows u64").at_tensor(name))?;
    if raw.data_offsets[1] < raw.data_offsets[0] {
        return Err(invalid_safetensors("tensor data offsets are reversed").at_tensor(name));
    }
    let described_length = raw.data_offsets[1] - raw.data_offsets[0];
    if described_length != byte_length {
        return Err(CartridgeError::new(
            ErrorCode::TensorDescriptorMismatch,
            "tensor shape/dtype byte size disagrees with data_offsets",
        )
        .at_tensor(name));
    }

    Ok(SafetensorTensorDescriptor {
        name: name.to_owned(),
        dtype,
        shape: raw.shape,
        data_offsets: raw.data_offsets,
        byte_length,
    })
}

fn validate_h3_axes(
    descriptor: &SafetensorTensorDescriptor,
    limits: &ValidationLimits,
    temporal_index: usize,
) -> Result<()> {
    if descriptor.shape[temporal_index] > limits.max_h3_temporal_axis() {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "H3 temporal axis exceeds the profile ceiling",
        )
        .at_tensor(&descriptor.name));
    }
    if descriptor.name == "video" {
        for axis in &descriptor.shape[3..=4] {
            let decoded = axis.checked_mul(16).ok_or_else(|| {
                size_overflow("decoded H3 geometry overflows u64").at_tensor("video")
            })?;
            if decoded > u64::from(limits.max_h3_decoded_axis()) {
                return Err(CartridgeError::new(
                    ErrorCode::RuntimeLimitExceeded,
                    "decoded H3 geometry exceeds the profile ceiling",
                )
                .at_tensor("video"));
            }
        }
    }
    Ok(())
}

fn validate_offsets(
    video: &SafetensorTensorDescriptor,
    audio: Option<&SafetensorTensorDescriptor>,
    data_bytes: u64,
) -> Result<()> {
    let mut tensors = vec![video];
    if let Some(audio) = audio {
        tensors.push(audio);
    }
    tensors.sort_by_key(|tensor| tensor.data_offsets[0]);

    let mut expected_start = 0_u64;
    for tensor in tensors {
        let [start, end] = tensor.data_offsets;
        if end > data_bytes {
            return Err(
                invalid_safetensors("tensor data offset is outside the entry")
                    .at_tensor(&tensor.name),
            );
        }
        if start != expected_start {
            let detail = if start < expected_start {
                "Safetensors tensor ranges overlap"
            } else {
                "Safetensors tensor ranges contain a gap"
            };
            return Err(invalid_safetensors(detail).at_tensor(&tensor.name));
        }
        expected_start = end;
    }
    if expected_start != data_bytes {
        return Err(invalid_safetensors(
            "Safetensors tensor ranges do not cover the complete data area",
        ));
    }
    Ok(())
}

fn scan_tensor<R: Read + Seek>(
    reader: &mut R,
    data_absolute_offset: u64,
    tensor: &SafetensorTensorDescriptor,
) -> Result<()> {
    let tensor_offset = data_absolute_offset
        .checked_add(tensor.data_offsets[0])
        .ok_or_else(|| size_overflow("tensor absolute offset overflows u64"))?;
    seek_to(reader, tensor_offset)?;

    let width = usize::try_from(tensor.dtype.byte_width()).map_err(|error| {
        size_overflow("tensor dtype width does not fit usize").with_source(error)
    })?;
    let mut buffer = vec![0_u8; SCAN_BUFFER_BYTES];
    let mut remaining = tensor.byte_length;
    let mut element_index = 0_u64;
    while remaining != 0 {
        let requested = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|error| size_overflow("scan chunk does not fit usize").with_source(error))?;
        let requested = requested - (requested % width);
        if requested == 0 {
            return Err(
                invalid_safetensors("tensor byte length is not dtype-aligned")
                    .at_tensor(&tensor.name),
            );
        }
        read_exact_payload(reader, &mut buffer[..requested])?;
        for element in buffer[..requested].chunks_exact(width) {
            let non_finite = match tensor.dtype {
                SafetensorDType::F16 => {
                    let bits = u16::from_le_bytes([element[0], element[1]]);
                    bits & 0x7c00 == 0x7c00
                }
                SafetensorDType::F32 => {
                    let bits = u32::from_le_bytes([element[0], element[1], element[2], element[3]]);
                    bits & 0x7f80_0000 == 0x7f80_0000
                }
            };
            if non_finite {
                return Err(CartridgeError::new(
                    ErrorCode::TensorNonFinite,
                    format!("tensor contains NaN or infinity at element {element_index}"),
                )
                .at_tensor(&tensor.name));
            }
            element_index += 1;
        }
        remaining -= requested as u64;
    }
    Ok(())
}

fn seek_to<R: Seek>(reader: &mut R, offset: u64) -> Result<()> {
    reader
        .seek(SeekFrom::Start(offset))
        .map(|_| ())
        .map_err(io_read_error)
}

fn read_exact_payload<R: Read>(reader: &mut R, bytes: &mut [u8]) -> Result<()> {
    reader.read_exact(bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid_safetensors("Safetensors entry is truncated").with_source(error)
        } else {
            io_read_error(error)
        }
    })
}

fn io_read_error(error: io::Error) -> CartridgeError {
    CartridgeError::new(ErrorCode::IoRead, "failed to read Safetensors payload")
        .at_entry(H3_ENTRY_NAME)
        .with_source(error)
}

fn invalid_safetensors(detail: impl Into<String>) -> CartridgeError {
    CartridgeError::new(ErrorCode::SafetensorsInvalid, detail).at_entry(H3_ENTRY_NAME)
}

fn size_overflow(detail: impl Into<String>) -> CartridgeError {
    CartridgeError::new(ErrorCode::TensorSizeOverflow, detail).at_entry(H3_ENTRY_NAME)
}
