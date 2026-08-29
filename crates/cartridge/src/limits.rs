/// Bytes in one binary gibibyte.
pub const GIB: u64 = 1024 * 1024 * 1024;
/// Bytes in one binary mebibyte.
pub const MIB: u64 = 1024 * 1024;

pub const MAX_ARCHIVE_BYTES: u64 = 16 * GIB;
pub const MAX_ARCHIVE_ENTRIES: usize = 3;
pub const MIN_ARCHIVE_ENTRIES: usize = 2;
pub const MAX_MANIFEST_BYTES: usize = 1_048_576;
pub const MAX_SAFETENSORS_HEADER_BYTES: u64 = MIB;
pub const MAX_H3_PAYLOAD_BYTES: u64 = 15 * GIB;
pub const MAX_PREVIEW_BYTES: u64 = 16 * MIB;
pub const MAX_PREVIEW_AXIS: u32 = 4096;
pub const MAX_PREVIEW_PIXELS: u64 = 16_777_216;
pub const MAX_TENSORS: usize = 2;
pub const MAX_TENSOR_RANK: usize = 5;
pub const MAX_PARENT_CARTRIDGES: usize = 256;
pub const MAX_OPERATION_RECORDS: usize = 1024;
pub const MAX_CONTROLS_PER_OPERATION: usize = 128;
pub const MAX_PROVENANCE_SOURCES: usize = 64;
pub const MAX_JSON_DEPTH: usize = 32;
pub const MAX_IDENTIFIER_BYTES: usize = 128;
pub const MAX_HUMAN_STRING_BYTES: usize = 4096;
pub const MAX_URI_BYTES: usize = 8192;
pub const MAX_JCS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_H3_DECODED_AXIS: u32 = 4096;
pub const MAX_H3_TEMPORAL_AXIS: u64 = 1_048_576;

/// Immutable specification ceilings for untrusted LC 0.1 inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidationLimits {
    manifest_bytes: usize,
    h3_payload_bytes: u64,
    preview_bytes: u64,
    tensors: usize,
    tensor_rank: usize,
    parent_cartridges: usize,
    operation_records: usize,
    controls_per_operation: usize,
    provenance_sources: usize,
    json_depth: usize,
    identifier_bytes: usize,
    human_string_bytes: usize,
    uri_bytes: usize,
    h3_decoded_axis: u32,
    h3_temporal_axis: u64,
}

macro_rules! lowering_setter {
    ($name:ident, $field:ident, $type:ty) => {
        #[must_use]
        pub const fn $name(mut self, maximum: $type) -> Self {
            if maximum < self.$field {
                self.$field = maximum;
            }
            self
        }
    };
}

impl ValidationLimits {
    #[must_use]
    pub const fn specification() -> Self {
        Self {
            manifest_bytes: MAX_MANIFEST_BYTES,
            h3_payload_bytes: MAX_H3_PAYLOAD_BYTES,
            preview_bytes: MAX_PREVIEW_BYTES,
            tensors: MAX_TENSORS,
            tensor_rank: MAX_TENSOR_RANK,
            parent_cartridges: MAX_PARENT_CARTRIDGES,
            operation_records: MAX_OPERATION_RECORDS,
            controls_per_operation: MAX_CONTROLS_PER_OPERATION,
            provenance_sources: MAX_PROVENANCE_SOURCES,
            json_depth: MAX_JSON_DEPTH,
            identifier_bytes: MAX_IDENTIFIER_BYTES,
            human_string_bytes: MAX_HUMAN_STRING_BYTES,
            uri_bytes: MAX_URI_BYTES,
            h3_decoded_axis: MAX_H3_DECODED_AXIS,
            h3_temporal_axis: MAX_H3_TEMPORAL_AXIS,
        }
    }

    #[must_use]
    pub const fn max_manifest_bytes(self) -> usize {
        self.manifest_bytes
    }
    #[must_use]
    pub const fn max_h3_payload_bytes(self) -> u64 {
        self.h3_payload_bytes
    }
    #[must_use]
    pub const fn max_preview_bytes(self) -> u64 {
        self.preview_bytes
    }
    #[must_use]
    pub const fn max_tensors(self) -> usize {
        self.tensors
    }
    #[must_use]
    pub const fn max_tensor_rank(self) -> usize {
        self.tensor_rank
    }
    #[must_use]
    pub const fn max_parent_cartridges(self) -> usize {
        self.parent_cartridges
    }
    #[must_use]
    pub const fn max_operation_records(self) -> usize {
        self.operation_records
    }
    #[must_use]
    pub const fn max_controls_per_operation(self) -> usize {
        self.controls_per_operation
    }
    #[must_use]
    pub const fn max_provenance_sources(self) -> usize {
        self.provenance_sources
    }
    #[must_use]
    pub const fn max_json_depth(self) -> usize {
        self.json_depth
    }
    #[must_use]
    pub const fn max_identifier_bytes(self) -> usize {
        self.identifier_bytes
    }
    #[must_use]
    pub const fn max_human_string_bytes(self) -> usize {
        self.human_string_bytes
    }
    #[must_use]
    pub const fn max_uri_bytes(self) -> usize {
        self.uri_bytes
    }
    #[must_use]
    pub const fn max_h3_decoded_axis(self) -> u32 {
        self.h3_decoded_axis
    }
    #[must_use]
    pub const fn max_h3_temporal_axis(self) -> u64 {
        self.h3_temporal_axis
    }

    lowering_setter!(with_max_manifest_bytes, manifest_bytes, usize);
    lowering_setter!(with_max_h3_payload_bytes, h3_payload_bytes, u64);
    lowering_setter!(with_max_preview_bytes, preview_bytes, u64);
    lowering_setter!(with_max_tensors, tensors, usize);
    lowering_setter!(with_max_tensor_rank, tensor_rank, usize);
    lowering_setter!(with_max_parent_cartridges, parent_cartridges, usize);
    lowering_setter!(with_max_operation_records, operation_records, usize);
    lowering_setter!(
        with_max_controls_per_operation,
        controls_per_operation,
        usize
    );
    lowering_setter!(with_max_provenance_sources, provenance_sources, usize);
    lowering_setter!(with_max_json_depth, json_depth, usize);
    lowering_setter!(with_max_identifier_bytes, identifier_bytes, usize);
    lowering_setter!(with_max_human_string_bytes, human_string_bytes, usize);
    lowering_setter!(with_max_uri_bytes, uri_bytes, usize);
    lowering_setter!(with_max_h3_decoded_axis, h3_decoded_axis, u32);
    lowering_setter!(with_max_h3_temporal_axis, h3_temporal_axis, u64);
}

impl Default for ValidationLimits {
    fn default() -> Self {
        Self::specification()
    }
}
