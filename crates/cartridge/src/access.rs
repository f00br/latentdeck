//! Bounded metadata for transferring an already validated LC file handle.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    archive::ArchiveEntry,
    error::{CartridgeError, ErrorCode, Result},
    hash::Sha256Hash,
    limits::{
        MAX_ARCHIVE_BYTES, MAX_H3_PAYLOAD_BYTES, MAX_IDENTIFIER_BYTES, MAX_MANIFEST_BYTES,
        MAX_TENSOR_RANK, MAX_TENSORS,
    },
    reader::{IntegrityValidationReceipt, ValidationLevel},
    safetensor::{SafetensorDType, SafetensorsPreflight},
    writer::canonical_json_bytes,
};

pub const INTEGRITY_ACCESS_ABI_VERSION: u16 = 1;
pub const MAX_INTEGRITY_ACCESS_RECEIPT_BYTES: usize = 64 * 1024;

/// One exact archive byte range bound to its validated digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityByteRange {
    pub offset: u64,
    pub byte_length: u64,
    pub sha256: Sha256Hash,
}

/// One exact tensor byte range in the validated Safetensors data area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityTensorAccess {
    pub dtype: SafetensorDType,
    pub shape: Vec<u64>,
    pub offset: u64,
    pub byte_length: u64,
}

/// Closed metadata needed to consume a duplicated, already validated LC file
/// handle without reopening a path or reparsing ZIP/Safetensors structures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityAccessReceipt {
    pub access_abi_version: u16,
    pub validation: IntegrityValidationReceipt,
    pub manifest: IntegrityByteRange,
    pub payload: IntegrityByteRange,
    pub safetensors_data_offset: u64,
    pub tensors: BTreeMap<String, IntegrityTensorAccess>,
}

impl IntegrityAccessReceipt {
    pub(crate) fn from_validated_parts(
        validation: IntegrityValidationReceipt,
        manifest_entry: &ArchiveEntry,
        manifest_sha256: Sha256Hash,
        payload_entry: &ArchiveEntry,
        preflight: &SafetensorsPreflight,
    ) -> Result<Self> {
        let data_start = payload_entry
            .data_offset
            .checked_add(preflight.data_offset)
            .ok_or_else(receipt_overflow)?;
        let tensors = preflight
            .tensors
            .iter()
            .map(|(name, tensor)| {
                let offset = data_start
                    .checked_add(tensor.data_offsets[0])
                    .ok_or_else(receipt_overflow)?;
                Ok((
                    name.clone(),
                    IntegrityTensorAccess {
                        dtype: tensor.dtype,
                        shape: tensor.shape.clone(),
                        offset,
                        byte_length: tensor.byte_length,
                    },
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let receipt = Self {
            access_abi_version: INTEGRITY_ACCESS_ABI_VERSION,
            manifest: IntegrityByteRange {
                offset: manifest_entry.data_offset,
                byte_length: manifest_entry.size,
                sha256: manifest_sha256,
            },
            payload: IntegrityByteRange {
                offset: payload_entry.data_offset,
                byte_length: payload_entry.size,
                sha256: validation.payload_sha256,
            },
            safetensors_data_offset: preflight.data_offset,
            tensors,
            validation,
        };
        receipt.validate_for_archive_length(receipt.validation.archive_bytes)?;
        Ok(receipt)
    }

    /// Parse one strict bounded JSON receipt and validate every range.
    ///
    /// # Errors
    ///
    /// Rejects oversized, malformed, non-canonical, unknown-field, or
    /// internally inconsistent receipts.
    pub fn parse_json(encoded: &[u8], archive_length: u64) -> Result<Self> {
        if encoded.is_empty() || encoded.len() > MAX_INTEGRITY_ACCESS_RECEIPT_BYTES {
            return Err(invalid_receipt(
                "integrity access receipt is outside its byte bound",
            ));
        }
        let mut deserializer = serde_json::Deserializer::from_slice(encoded);
        let receipt = Self::deserialize(&mut deserializer).map_err(|error| {
            invalid_receipt("integrity access receipt is not strict JSON").with_source(error)
        })?;
        deserializer.end().map_err(|error| {
            invalid_receipt("integrity access receipt contains trailing data").with_source(error)
        })?;
        if canonical_json_bytes(&receipt)? != encoded {
            return Err(invalid_receipt(
                "integrity access receipt is not canonical JSON",
            ));
        }
        receipt.validate_for_archive_length(archive_length)?;
        Ok(receipt)
    }

    /// Encode the receipt as bounded canonical JSON for authenticated P2
    /// metadata transport.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization unexpectedly exceeds the fixed
    /// receipt bound.
    pub fn canonical_json(&self) -> Result<Vec<u8>> {
        let encoded = canonical_json_bytes(self)?;
        if encoded.is_empty() || encoded.len() > MAX_INTEGRITY_ACCESS_RECEIPT_BYTES {
            return Err(invalid_receipt(
                "integrity access receipt is outside its byte bound",
            ));
        }
        Ok(encoded)
    }

    /// Validate receipt ranges against the exact duplicated file length.
    ///
    /// # Errors
    ///
    /// Rejects wrong versions/lengths, out-of-file or overlapping metadata,
    /// malformed tensor descriptors, and incomplete Safetensors data coverage.
    pub fn validate_for_archive_length(&self, archive_length: u64) -> Result<()> {
        self.validate_header_layout(archive_length)?;
        self.validate_tensor_layout()
    }

    fn validate_header_layout(&self, archive_length: u64) -> Result<()> {
        if self.access_abi_version != INTEGRITY_ACCESS_ABI_VERSION
            || self.validation.validation_level != ValidationLevel::Full
            || archive_length == 0
            || archive_length > MAX_ARCHIVE_BYTES
            || self.validation.archive_bytes != archive_length
            || self.validation.payload_path.is_empty()
            || self.validation.payload_path.len() > MAX_IDENTIFIER_BYTES
            || self.validation.payload_path.starts_with('/')
            || self.validation.payload_path.contains(['\\', ':'])
            || self
                .validation
                .payload_path
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(invalid_receipt(
                "integrity access receipt identity is inconsistent",
            ));
        }
        validate_range(
            &self.manifest,
            archive_length,
            u64::try_from(MAX_MANIFEST_BYTES).map_err(|_| receipt_overflow())?,
            "manifest",
        )?;
        validate_range(
            &self.payload,
            archive_length,
            MAX_H3_PAYLOAD_BYTES,
            "payload",
        )?;
        if self.payload.byte_length != self.validation.payload_bytes
            || self.payload.sha256 != self.validation.payload_sha256
            || ranges_overlap(&self.manifest, &self.payload)?
            || self.safetensors_data_offset == 0
            || self.safetensors_data_offset >= self.payload.byte_length
            || self.tensors.is_empty()
            || self.tensors.len() > MAX_TENSORS
        {
            return Err(invalid_receipt(
                "integrity access receipt payload layout is inconsistent",
            ));
        }
        Ok(())
    }

    fn validate_tensor_layout(&self) -> Result<()> {
        let payload_end = checked_end(self.payload.offset, self.payload.byte_length)?;
        let data_start = self
            .payload
            .offset
            .checked_add(self.safetensors_data_offset)
            .ok_or_else(receipt_overflow)?;
        let mut ranges = Vec::with_capacity(self.tensors.len());
        let mut storage_bytes = 0_u64;
        for (name, tensor) in &self.tensors {
            if name.is_empty()
                || name.len() > MAX_IDENTIFIER_BYTES
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
                || tensor.shape.is_empty()
                || tensor.shape.len() > MAX_TENSOR_RANK
                || tensor.shape.contains(&0)
            {
                return Err(invalid_receipt(
                    "integrity access receipt tensor descriptor is invalid",
                ));
            }
            let element_count = tensor
                .shape
                .iter()
                .try_fold(1_u64, |product, axis| product.checked_mul(*axis))
                .ok_or_else(receipt_overflow)?;
            let expected_bytes = element_count
                .checked_mul(tensor.dtype.byte_width())
                .ok_or_else(receipt_overflow)?;
            let tensor_end = checked_end(tensor.offset, tensor.byte_length)?;
            if tensor.byte_length == 0
                || expected_bytes != tensor.byte_length
                || tensor.offset < data_start
                || tensor_end > payload_end
            {
                return Err(invalid_receipt(
                    "integrity access receipt tensor range is invalid",
                ));
            }
            storage_bytes = storage_bytes
                .checked_add(tensor.byte_length)
                .ok_or_else(receipt_overflow)?;
            ranges.push((tensor.offset, tensor_end));
        }
        ranges.sort_unstable_by_key(|range| range.0);
        let mut cursor = data_start;
        for (start, end) in ranges {
            if start != cursor {
                return Err(invalid_receipt(
                    "integrity access receipt tensor ranges are not contiguous",
                ));
            }
            cursor = end;
        }
        if cursor != payload_end || storage_bytes != self.validation.tensor_storage_bytes {
            return Err(invalid_receipt(
                "integrity access receipt does not cover the tensor data area",
            ));
        }
        Ok(())
    }
}

fn validate_range(
    range: &IntegrityByteRange,
    archive_length: u64,
    maximum: u64,
    name: &'static str,
) -> Result<()> {
    if range.byte_length == 0
        || range.byte_length > maximum
        || checked_end(range.offset, range.byte_length)? > archive_length
    {
        return Err(invalid_receipt(match name {
            "manifest" => "integrity access receipt manifest range is invalid",
            _ => "integrity access receipt payload range is invalid",
        }));
    }
    Ok(())
}

fn ranges_overlap(left: &IntegrityByteRange, right: &IntegrityByteRange) -> Result<bool> {
    let left_end = checked_end(left.offset, left.byte_length)?;
    let right_end = checked_end(right.offset, right.byte_length)?;
    Ok(left.offset < right_end && right.offset < left_end)
}

fn checked_end(offset: u64, byte_length: u64) -> Result<u64> {
    offset.checked_add(byte_length).ok_or_else(receipt_overflow)
}

fn receipt_overflow() -> CartridgeError {
    invalid_receipt("integrity access receipt arithmetic overflow")
}

fn invalid_receipt(detail: &'static str) -> CartridgeError {
    CartridgeError::new(ErrorCode::ManifestInvalid, detail).at_json("/integrity_access_receipt")
}
