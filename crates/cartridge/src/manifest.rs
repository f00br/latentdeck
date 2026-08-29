use std::collections::BTreeMap;

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use serde_json::Value;

use crate::{
    error::{CartridgeError, ErrorCode, Result},
    limits::{MAX_JCS_SAFE_INTEGER, MAX_PREVIEW_AXIS, MAX_PREVIEW_PIXELS, ValidationLimits},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpecVersion(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CartridgeId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sha256Digest(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Identifier(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rational {
    pub numerator: u64,
    pub denominator: u64,
}

impl Rational {
    #[must_use]
    pub fn reduced(numerator: u64, denominator: u64) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            return None;
        }
        let divisor = greatest_common_divisor(numerator, denominator);
        Some(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    #[must_use]
    pub fn is_canonical(self) -> bool {
        self.numerator > 0
            && self.denominator > 0
            && greatest_common_divisor(self.numerator, self.denominator) == 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecDescriptor {
    pub family: Identifier,
    pub profile: Identifier,
    pub profile_version: SpecVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PayloadDescriptor {
    pub path: String,
    pub media_type: String,
    pub byte_length: u64,
    pub sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TensorStream {
    Visual,
    Audio,
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DType {
    F16,
    F32,
    Unsupported,
}

impl DType {
    #[must_use]
    pub const fn byte_width(self) -> Option<u64> {
        match self {
            Self::F16 => Some(2),
            Self::F32 => Some(4),
            Self::Unsupported => None,
        }
    }

    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::F16 | Self::F32)
    }
}

impl Serialize for DType {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::F16 => "F16",
            Self::F32 => "F32",
            Self::Unsupported => "UNSUPPORTED",
        })
    }
}

impl<'de> Deserialize<'de> for DType {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "F16" => Self::F16,
            "F32" => Self::F32,
            _ => Self::Unsupported,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TensorDescriptor {
    pub stream: TensorStream,
    pub name: Identifier,
    pub payload: String,
    pub storage_dtype: DType,
    pub runtime_dtype: DType,
    pub shape: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodedVideoDescriptor {
    pub width: u32,
    pub height: u32,
    pub frame_count: u64,
    pub frame_rate: Rational,
    pub duration: Rational,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingDescriptor {
    pub contract: Identifier,
    pub contract_version: SpecVersion,
    pub decoded_video: DecodedVideoDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCartridgeRef {
    pub cartridge_id: CartridgeId,
    pub archive_sha256: Sha256Digest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioOmissionReason {
    DurationMismatch,
    TemporalMappingMismatch,
    DurationAndMappingMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioDisposition {
    SourceAbsent,
    PreservedSource,
    CopiedFromCarrierExact {
        source_cartridge: SourceCartridgeRef,
    },
    OmittedTimingMismatch {
        source_cartridge: SourceCartridgeRef,
        reason: AudioOmissionReason,
    },
}

impl AudioDisposition {
    #[must_use]
    pub const fn requires_audio_tensor(&self) -> bool {
        matches!(
            self,
            Self::PreservedSource | Self::CopiedFromCarrierExact { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewDescriptor {
    pub path: String,
    pub media_type: String,
    pub byte_length: u64,
    pub sha256: Sha256Digest,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerDescriptor {
    pub name: Identifier,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceSource {
    pub kind: Identifier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub created_by: ProducerDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    pub sources: Vec<ProvenanceSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentCartridge {
    pub cartridge_id: CartridgeId,
    pub archive_sha256: Sha256Digest,
    pub role: Identifier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationRecord {
    pub operator_id: Identifier,
    pub operator_version: String,
    pub seed: u64,
    pub controls: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestV0_1 {
    pub spec_version: SpecVersion,
    pub cartridge_id: CartridgeId,
    pub codec: CodecDescriptor,
    pub payloads: Vec<PayloadDescriptor>,
    pub tensors: Vec<TensorDescriptor>,
    pub timing: TimingDescriptor,
    pub audio: AudioDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewDescriptor>,
    pub provenance: Provenance,
    pub parent_cartridges: Vec<ParentCartridge>,
    pub operation_history: Vec<OperationRecord>,
}

impl ManifestV0_1 {
    /// Validate codec-neutral LC 0.1 manifest invariants.
    ///
    /// # Errors
    ///
    /// Returns a stable manifest error when identity or version fields are not
    /// canonical for LC 0.1.
    pub fn validate_common(&self, limits: &ValidationLimits) -> Result<()> {
        validate_manifest_shape(self, limits)?;
        validate_manifest_identity(self, limits)?;
        validate_tensor_descriptors(self, limits)?;
        validate_provenance(self, limits)?;
        validate_genealogy(self, limits)?;
        Ok(())
    }
}

fn validate_manifest_shape(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<()> {
    if manifest.spec_version.0 != crate::LC_SPEC_VERSION {
        return Err(CartridgeError::new(
            ErrorCode::UnsupportedSpecVersion,
            format!(
                "unsupported LC specification version {}",
                manifest.spec_version.0
            ),
        )
        .at_json("/spec_version"));
    }
    if manifest.payloads.len() != 1 {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "LC 0.1 requires exactly one payload descriptor",
        )
        .at_json("/payloads"));
    }
    ensure_max_count(
        manifest.tensors.len(),
        limits.max_tensors(),
        "/tensors",
        "tensor descriptors",
    )?;
    if manifest.tensors.is_empty() {
        return Err(CartridgeError::new(
            ErrorCode::TensorMissing,
            "LC 0.1 requires at least one tensor descriptor",
        )
        .at_json("/tensors"));
    }
    ensure_max_count(
        manifest.parent_cartridges.len(),
        limits.max_parent_cartridges(),
        "/parent_cartridges",
        "parent cartridges",
    )?;
    ensure_max_count(
        manifest.operation_history.len(),
        limits.max_operation_records(),
        "/operation_history",
        "operation records",
    )?;
    ensure_max_count(
        manifest.provenance.sources.len(),
        limits.max_provenance_sources(),
        "/provenance/sources",
        "provenance sources",
    )
}

fn validate_manifest_identity(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<()> {
    validate_cartridge_id(&manifest.cartridge_id, "/cartridge_id")?;
    for (index, payload) in manifest.payloads.iter().enumerate() {
        validate_sha256(&payload.sha256, &format!("/payloads/{index}/sha256"))?;
    }
    if let Some(preview) = &manifest.preview {
        validate_preview(preview, limits)?;
    }
    validate_rational(
        manifest.timing.decoded_video.frame_rate,
        "/timing/decoded_video/frame_rate",
    )?;
    validate_rational(
        manifest.timing.decoded_video.duration,
        "/timing/decoded_video/duration",
    )?;
    validate_identifier(&manifest.codec.family, "/codec/family", limits)?;
    validate_identifier(&manifest.codec.profile, "/codec/profile", limits)?;
    validate_identifier(&manifest.timing.contract, "/timing/contract", limits)?;
    validate_identifier(
        &manifest.provenance.created_by.name,
        "/provenance/created_by/name",
        limits,
    )
}

fn validate_preview(preview: &PreviewDescriptor, limits: &ValidationLimits) -> Result<()> {
    if preview.path != "preview.webp" || preview.media_type != "image/webp" {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "preview must be the image/webp entry preview.webp",
        )
        .at_json("/preview"));
    }
    if preview.byte_length == 0 || preview.byte_length > limits.max_preview_bytes() {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "preview byte length is outside the LC 0.1 ceiling",
        )
        .at_json("/preview/byte_length"));
    }
    let pixels = u64::from(preview.width)
        .checked_mul(u64::from(preview.height))
        .ok_or_else(|| {
            CartridgeError::new(
                ErrorCode::RuntimeLimitExceeded,
                "preview pixel-count arithmetic overflow",
            )
            .at_json("/preview")
        })?;
    if preview.width == 0
        || preview.height == 0
        || preview.width > MAX_PREVIEW_AXIS
        || preview.height > MAX_PREVIEW_AXIS
        || pixels > MAX_PREVIEW_PIXELS
    {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "preview dimensions exceed the LC 0.1 ceiling",
        )
        .at_json("/preview"));
    }
    validate_sha256(&preview.sha256, "/preview/sha256")
}

fn validate_tensor_descriptors(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<()> {
    for (index, tensor) in manifest.tensors.iter().enumerate() {
        ensure_max_count(
            tensor.shape.len(),
            limits.max_tensor_rank(),
            &format!("/tensors/{index}/shape"),
            "tensor rank",
        )?;
        validate_identifier(&tensor.name, &format!("/tensors/{index}/name"), limits)?;
        if !tensor.storage_dtype.is_supported() {
            return Err(CartridgeError::new(
                ErrorCode::TensorDtypeForbidden,
                "LC 0.1 storage dtype must be F16 or F32",
            )
            .at_json(format!("/tensors/{index}/storage_dtype")));
        }
        if !tensor.runtime_dtype.is_supported() {
            return Err(CartridgeError::new(
                ErrorCode::TensorDtypeForbidden,
                "LC 0.1 runtime dtype must be F16 or F32",
            )
            .at_json(format!("/tensors/{index}/runtime_dtype")));
        }
    }
    Ok(())
}

fn validate_provenance(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<()> {
    validate_bounded_string(
        &manifest.provenance.created_by.version,
        limits.max_identifier_bytes(),
        "/provenance/created_by/version",
        "version",
    )?;
    if let Some(created_at) = &manifest.provenance.created_at {
        validate_bounded_string(created_at, 64, "/provenance/created_at", "timestamp")?;
        validate_created_at(created_at)?;
    }
    for (index, source) in manifest.provenance.sources.iter().enumerate() {
        let pointer = format!("/provenance/sources/{index}");
        validate_identifier(&source.kind, &format!("{pointer}/kind"), limits)?;
        if let Some(digest) = &source.sha256 {
            validate_sha256(digest, &format!("{pointer}/sha256"))?;
        }
        if let Some(uri) = &source.uri {
            validate_bounded_string(
                uri,
                limits.max_uri_bytes(),
                &format!("{pointer}/uri"),
                "URI",
            )?;
        }
        if let Some(license) = &source.license {
            validate_bounded_string(
                license,
                limits.max_human_string_bytes(),
                &format!("{pointer}/license"),
                "license label",
            )?;
        }
        if let Some(metadata) = &source.metadata {
            validate_value_map(metadata, &format!("{pointer}/metadata"), limits)?;
        }
    }
    Ok(())
}

fn validate_created_at(created_at: &str) -> Result<()> {
    let parsed =
        time::OffsetDateTime::parse(created_at, &time::format_description::well_known::Rfc3339)
            .map_err(|error| {
                CartridgeError::new(
                    ErrorCode::ManifestInvalid,
                    "created_at must be a valid RFC 3339 UTC timestamp ending in Z",
                )
                .at_json("/provenance/created_at")
                .with_source(error)
            })?;
    if parsed.offset() != time::UtcOffset::UTC || !created_at.ends_with('Z') {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "created_at must be a valid RFC 3339 UTC timestamp ending in Z",
        )
        .at_json("/provenance/created_at"));
    }
    Ok(())
}

fn validate_genealogy(manifest: &ManifestV0_1, limits: &ValidationLimits) -> Result<()> {
    for (index, parent) in manifest.parent_cartridges.iter().enumerate() {
        validate_cartridge_id(
            &parent.cartridge_id,
            &format!("/parent_cartridges/{index}/cartridge_id"),
        )?;
        validate_sha256(
            &parent.archive_sha256,
            &format!("/parent_cartridges/{index}/archive_sha256"),
        )?;
        validate_identifier(
            &parent.role,
            &format!("/parent_cartridges/{index}/role"),
            limits,
        )?;
    }
    for (index, operation) in manifest.operation_history.iter().enumerate() {
        ensure_max_count(
            operation.controls.len(),
            limits.max_controls_per_operation(),
            &format!("/operation_history/{index}/controls"),
            "operation controls",
        )?;
        if operation.seed > MAX_JCS_SAFE_INTEGER {
            return Err(CartridgeError::new(
                ErrorCode::ManifestInvalid,
                "operation seed exceeds the JCS safe integer range",
            )
            .at_json(format!("/operation_history/{index}/seed")));
        }
        validate_identifier(
            &operation.operator_id,
            &format!("/operation_history/{index}/operator_id"),
            limits,
        )?;
        validate_bounded_string(
            &operation.operator_version,
            limits.max_identifier_bytes(),
            &format!("/operation_history/{index}/operator_version"),
            "version",
        )?;
        validate_value_map(
            &operation.controls,
            &format!("/operation_history/{index}/controls"),
            limits,
        )?;
    }
    if let Some(source) = audio_source_cartridge(&manifest.audio) {
        validate_source_cartridge(source, "/audio/source_cartridge")?;
    }
    Ok(())
}

fn audio_source_cartridge(audio: &AudioDisposition) -> Option<&SourceCartridgeRef> {
    match audio {
        AudioDisposition::CopiedFromCarrierExact { source_cartridge }
        | AudioDisposition::OmittedTimingMismatch {
            source_cartridge, ..
        } => Some(source_cartridge),
        AudioDisposition::SourceAbsent | AudioDisposition::PreservedSource => None,
    }
}

fn validate_source_cartridge(source: &SourceCartridgeRef, json_pointer: &str) -> Result<()> {
    validate_cartridge_id(
        &source.cartridge_id,
        &format!("{json_pointer}/cartridge_id"),
    )?;
    validate_sha256(
        &source.archive_sha256,
        &format!("{json_pointer}/archive_sha256"),
    )
}

fn validate_bounded_string(
    value: &str,
    maximum_bytes: usize,
    json_pointer: &str,
    label: &str,
) -> Result<()> {
    if value.len() > maximum_bytes {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            format!("{label} exceeds the {maximum_bytes}-byte ceiling"),
        )
        .at_json(json_pointer));
    }
    Ok(())
}

fn validate_value_map(
    values: &BTreeMap<String, Value>,
    json_pointer: &str,
    limits: &ValidationLimits,
) -> Result<()> {
    for (key, value) in values {
        validate_bounded_string(
            key,
            limits.max_identifier_bytes(),
            &format!("{json_pointer}/{}", escape_json_pointer(key)),
            "JSON object key",
        )?;
        validate_json_value(
            value,
            &format!("{json_pointer}/{}", escape_json_pointer(key)),
            limits,
        )?;
    }
    Ok(())
}

fn validate_json_value(value: &Value, json_pointer: &str, limits: &ValidationLimits) -> Result<()> {
    match value {
        Value::Null | Value::Bool(_) => Ok(()),
        Value::String(text) => validate_bounded_string(
            text,
            limits.max_human_string_bytes(),
            json_pointer,
            "JSON string",
        ),
        Value::Number(number) => validate_json_number(number, json_pointer),
        Value::Array(values) => values.iter().enumerate().try_for_each(|(index, nested)| {
            validate_json_value(nested, &format!("{json_pointer}/{index}"), limits)
        }),
        Value::Object(values) => values.iter().try_for_each(|(key, nested)| {
            validate_bounded_string(
                key,
                limits.max_identifier_bytes(),
                &format!("{json_pointer}/{}", escape_json_pointer(key)),
                "JSON object key",
            )?;
            validate_json_value(
                nested,
                &format!("{json_pointer}/{}", escape_json_pointer(key)),
                limits,
            )
        }),
    }
}

fn validate_json_number(number: &serde_json::Number, json_pointer: &str) -> Result<()> {
    let maximum = i64::try_from(MAX_JCS_SAFE_INTEGER).expect("JCS ceiling fits i64");
    let out_of_range = if let Some(value) = number.as_i64() {
        value < -maximum || value > maximum
    } else if let Some(value) = number.as_u64() {
        value > MAX_JCS_SAFE_INTEGER
    } else {
        number.as_f64().is_none_or(|value| !value.is_finite())
    };
    if out_of_range {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "JSON number is outside the finite JCS-safe range",
        )
        .at_json(json_pointer));
    }
    Ok(())
}

fn validate_cartridge_id(cartridge_id: &CartridgeId, json_pointer: &str) -> Result<()> {
    let parsed_id = uuid::Uuid::parse_str(&cartridge_id.0).map_err(|error| {
        CartridgeError::new(ErrorCode::ManifestInvalid, "cartridge_id must be a UUID")
            .at_json(json_pointer)
            .with_source(error)
    })?;
    if parsed_id.is_nil() || parsed_id.hyphenated().to_string() != cartridge_id.0 {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "cartridge_id must be a non-nil canonical lowercase UUID",
        )
        .at_json(json_pointer));
    }
    Ok(())
}

/// Parse and validate one bounded LC 0.1 manifest.
///
/// # Errors
///
/// Returns a stable error for size, UTF-8, duplicate-key, strict-schema,
/// version, or codec-neutral semantic violations.
pub fn parse_manifest_json(bytes: &[u8], limits: &ValidationLimits) -> Result<ManifestV0_1> {
    if bytes.len() > limits.max_manifest_bytes() {
        return Err(CartridgeError::new(
            ErrorCode::ManifestTooLarge,
            format!(
                "manifest is {} bytes; limit is {}",
                bytes.len(),
                limits.max_manifest_bytes()
            ),
        )
        .at_entry("manifest.json"));
    }

    let text = std::str::from_utf8(bytes).map_err(|error| {
        CartridgeError::new(ErrorCode::ManifestNotUtf8, "manifest must be UTF-8")
            .at_entry("manifest.json")
            .with_source(error)
    })?;

    reject_duplicate_keys(text, limits.max_json_depth())?;

    let mut deserializer = serde_json::Deserializer::from_str(text);
    let manifest: ManifestV0_1 =
        serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
            let json_pointer = serde_path_to_json_pointer(error.path());
            manifest_json_error(error.into_inner(), Some(json_pointer))
        })?;
    deserializer
        .end()
        .map_err(|error| manifest_json_error(error, None))?;

    manifest.validate_common(limits)?;

    Ok(manifest)
}

const DUPLICATE_KEY_MARKER: &str = "__latentdeck_duplicate_key__:";
const JSON_DEPTH_MARKER: &str = "__latentdeck_json_depth__:";

fn reject_duplicate_keys(text: &str, maximum_depth: usize) -> Result<()> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    DuplicateRejectingSeed {
        json_pointer: String::new(),
        depth: 0,
        maximum_depth,
    }
    .deserialize(&mut deserializer)
    .map_err(duplicate_preflight_error)?;
    deserializer
        .end()
        .map_err(|error| manifest_json_error(error, None))
}

#[derive(Debug)]
struct DuplicateRejectingSeed {
    json_pointer: String,
    depth: usize,
    maximum_depth: usize,
}

impl<'de> DeserializeSeed<'de> for DuplicateRejectingSeed {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if self.depth > self.maximum_depth {
            return Err(de::Error::custom(format!(
                "{JSON_DEPTH_MARKER}{}|",
                self.json_pointer
            )));
        }
        deserializer.deserialize_any(DuplicateRejectingVisitor {
            json_pointer: self.json_pointer,
            depth: self.depth,
            maximum_depth: self.maximum_depth,
        })
    }
}

struct DuplicateRejectingVisitor {
    json_pointer: String,
    depth: usize,
    maximum_depth: usize,
}

impl<'de> Visitor<'de> for DuplicateRejectingVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, _value: bool) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E>(self, _value: i64) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E>(self, _value: u64) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E>(self, _value: f64) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E>(self, _value: &str) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_string<E>(self, _value: String) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_none<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> std::result::Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        DuplicateRejectingSeed {
            json_pointer: self.json_pointer,
            depth: self.depth,
            maximum_depth: self.maximum_depth,
        }
        .deserialize(deserializer)
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(())
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut index = 0_usize;
        loop {
            let item_pointer = format!("{}/{}", self.json_pointer, index);
            if sequence
                .next_element_seed(DuplicateRejectingSeed {
                    json_pointer: item_pointer,
                    depth: self.depth.saturating_add(1),
                    maximum_depth: self.maximum_depth,
                })?
                .is_none()
            {
                break;
            }
            index = index.checked_add(1).ok_or_else(|| {
                de::Error::custom("JSON array index overflow during duplicate-key preflight")
            })?;
        }
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = std::collections::BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            let key_pointer = format!("{}/{}", self.json_pointer, escape_json_pointer(&key));
            if !keys.insert(key) {
                return Err(de::Error::custom(format!(
                    "{DUPLICATE_KEY_MARKER}{key_pointer}|"
                )));
            }
            map.next_value_seed(DuplicateRejectingSeed {
                json_pointer: key_pointer,
                depth: self.depth.saturating_add(1),
                maximum_depth: self.maximum_depth,
            })?;
        }
        Ok(())
    }
}

fn duplicate_preflight_error(error: serde_json::Error) -> CartridgeError {
    let detail = error.to_string();
    if let Some(pointer) = marker_pointer(&detail, DUPLICATE_KEY_MARKER) {
        return CartridgeError::new(ErrorCode::ManifestDuplicateKey, "duplicate JSON key")
            .at_entry("manifest.json")
            .at_json(pointer)
            .with_source(error);
    }
    if let Some(pointer) = marker_pointer(&detail, JSON_DEPTH_MARKER) {
        return CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            "JSON nesting exceeds the LC 0.1 ceiling",
        )
        .at_entry("manifest.json")
        .at_json(pointer)
        .with_source(error);
    }
    manifest_json_error(error, None)
}

fn marker_pointer(detail: &str, marker: &str) -> Option<String> {
    let pointer_start = detail.find(marker)? + marker.len();
    let relative_end = detail[pointer_start..].find('|')?;
    Some(detail[pointer_start..pointer_start + relative_end].to_owned())
}

fn serde_path_to_json_pointer(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;

    let mut pointer = String::new();
    for segment in path {
        pointer.push('/');
        match segment {
            Segment::Seq { index } => pointer.push_str(&index.to_string()),
            Segment::Map { key } => pointer.push_str(&escape_json_pointer(key)),
            Segment::Enum { variant } => pointer.push_str(&escape_json_pointer(variant)),
            Segment::Unknown => pointer.push('?'),
        }
    }
    pointer
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn manifest_json_error(error: serde_json::Error, json_pointer: Option<String>) -> CartridgeError {
    let code = if error.to_string().contains("unknown field") {
        ErrorCode::ManifestUnknownField
    } else {
        ErrorCode::ManifestJsonInvalid
    };
    let mut cartridge_error = CartridgeError::new(code, error.to_string())
        .at_entry("manifest.json")
        .with_source(error);
    if let Some(pointer) = json_pointer {
        cartridge_error = cartridge_error.at_json(pointer);
    }
    cartridge_error
}

fn validate_sha256(digest: &Sha256Digest, json_pointer: &str) -> Result<()> {
    let canonical = digest.0.len() == 64
        && digest
            .0
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !canonical {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "SHA-256 must be exactly 64 lowercase hexadecimal characters",
        )
        .at_json(json_pointer));
    }
    Ok(())
}

fn validate_rational(rational: Rational, json_pointer: &str) -> Result<()> {
    if !rational.is_canonical() {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "rational numerator and denominator must be positive and reduced",
        )
        .at_json(json_pointer));
    }
    Ok(())
}

fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn validate_identifier(
    identifier: &Identifier,
    json_pointer: &str,
    limits: &ValidationLimits,
) -> Result<()> {
    let bytes = identifier.0.as_bytes();
    let valid_byte = |byte: u8| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'.' | b'-')
    };
    let canonical = !bytes.is_empty()
        && bytes.len() <= limits.max_identifier_bytes()
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes.iter().copied().all(valid_byte);
    if !canonical {
        return Err(CartridgeError::new(
            ErrorCode::ManifestInvalid,
            "identifier must be a bounded lowercase ASCII token",
        )
        .at_json(json_pointer));
    }
    Ok(())
}

fn ensure_max_count(
    actual: usize,
    maximum: usize,
    json_pointer: &str,
    resource: &str,
) -> Result<()> {
    if actual > maximum {
        return Err(CartridgeError::new(
            ErrorCode::RuntimeLimitExceeded,
            format!("{resource} count {actual} exceeds limit {maximum}"),
        )
        .at_json(json_pointer));
    }
    Ok(())
}
