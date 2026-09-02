//! Closed, transport-independent Worker Protocol 2 contract.
//!
//! Protocol 2 is intentionally additive to the accepted Protocol 1 runtime.
//! This module owns bounded JSON and named-MessagePack representations; process
//! supervision, Named Pipes, shared-memory tensors, and codec execution remain
//! runtime responsibilities.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    io::Cursor,
    marker::PhantomData,
    path::Path,
};

use serde::{
    Deserialize, Serialize,
    de::{self, DeserializeOwned, SeqAccess, Visitor},
};
use thiserror::Error;
use uuid::Uuid;

mod wire_uuid {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use uuid::Uuid;

    pub fn serialize<S: Serializer>(value: &Uuid, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.hyphenated().to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Uuid, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let value = Uuid::parse_str(&encoded).map_err(de::Error::custom)?;
        if encoded != value.hyphenated().to_string() {
            return Err(de::Error::custom(
                "UUID must be canonical lowercase hyphenated text",
            ));
        }
        Ok(value)
    }
}

mod optional_wire_uuid {
    use serde::{Deserialize, Deserializer, Serializer, de};
    use uuid::Uuid;

    #[allow(clippy::ref_option)] // serde's `with` module requires `&Option<T>`.
    pub fn serialize<S: Serializer>(
        value: &Option<Uuid>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.serialize_some(&value.hyphenated().to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Uuid>, D::Error> {
        let encoded = Option::<String>::deserialize(deserializer)?;
        encoded
            .map(|encoded| {
                let value = Uuid::parse_str(&encoded).map_err(de::Error::custom)?;
                if encoded != value.hyphenated().to_string() {
                    return Err(de::Error::custom(
                        "UUID must be canonical lowercase hyphenated text",
                    ));
                }
                Ok(value)
            })
            .transpose()
    }
}

pub const PROTOCOL_VERSION: u16 = 2;
pub const MAX_FRAME_BYTES: usize = 262_144;
pub const MAX_INTEGRITY_ACCESS_RECEIPT_BYTES: usize = 64 * 1024;
pub const MAX_CAPABILITIES: usize = 16;
pub const MAX_EXTERNAL_ASSETS: usize = 16;
pub const MAX_PROFILES: usize = 64;
pub const MAX_SOURCES: usize = 16;
pub const MAX_CONTROLS: usize = 64;
pub const MAX_ROLES: usize = 16;
pub const MAX_ERROR_DETAILS: usize = 16;
pub const MAX_CAPTURE_EVENTS: u32 = 32;
pub const MAX_WARM_SESSIONS: u8 = 4;
pub const MAX_DECODE_BATCH: u8 = 24;
pub const MAX_CAPTURE_LATENT_SLOTS: u64 = 1_048_576;
pub const MAX_CAPTURE_VISUAL_BYTES: u64 = 15 * 1024 * 1024 * 1024;
pub const MAX_MESSAGES_PER_SESSION: usize = 65_536;
pub const MAX_PENDING_COMMANDS: usize = 256;
pub const MAX_RAW_IMPORT_SOURCE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub const MAX_RAW_IMPORT_TENSORS: usize = 64;
pub const MAX_RAW_IMPORT_TENSOR_AXES: usize = 8;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 4_096;
const MAX_PATH_BYTES: usize = 32_768;

/// Authentication proof carried only by the first worker-to-host P2 frame.
///
/// The wire representation is exactly 64 lowercase hexadecimal characters.
/// Debug output is deliberately redacted so transport diagnostics cannot leak
/// the bootstrap secret.
#[derive(Clone, PartialEq, Eq)]
pub struct WorkerHelloAuthToken([u8; 32]);

impl WorkerHelloAuthToken {
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn constant_time_eq(&self, other: &Self) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
    }
}

impl fmt::Debug for WorkerHelloAuthToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WorkerHelloAuthToken([REDACTED])")
    }
}

impl Serialize for WorkerHelloAuthToken {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for WorkerHelloAuthToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        if encoded.len() != 64
            || !encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(de::Error::custom(
                "worker hello auth token must be exactly 64 lowercase hex characters",
            ));
        }
        let mut bytes = [0_u8; 32];
        hex::decode_to_slice(encoded, &mut bytes).map_err(de::Error::custom)?;
        Ok(Self(bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolMarker {
    #[serde(rename = "latentdeck.worker")]
    LatentDeckWorker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Player,
    Realtime,
    Resample,
    SnapshotCapture,
    LiveCapture,
    RawImport,
}

impl Capability {
    pub const REQUIRED_CODEC_V2: [Self; 5] = [
        Self::Player,
        Self::Realtime,
        Self::Resample,
        Self::SnapshotCapture,
        Self::LiveCapture,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "protocol.invalid_message")]
    ProtocolInvalidMessage,
    #[serde(rename = "protocol.unsupported_version")]
    ProtocolUnsupportedVersion,
    #[serde(rename = "protocol.bound_exceeded")]
    ProtocolBoundExceeded,
    #[serde(rename = "session.not_configured")]
    SessionNotConfigured,
    #[serde(rename = "session.capacity_exceeded")]
    SessionCapacityExceeded,
    #[serde(rename = "session.output_lease_busy")]
    SessionOutputLeaseBusy,
    #[serde(rename = "session.output_lease_pinned")]
    SessionOutputLeasePinned,
    #[serde(rename = "codec.not_loaded")]
    CodecNotLoaded,
    #[serde(rename = "codec.untrusted")]
    CodecUntrusted,
    #[serde(rename = "codec.capability_unsupported")]
    CodecCapabilityUnsupported,
    #[serde(rename = "profile.invalid")]
    ProfileInvalid,
    #[serde(rename = "profile.incompatible")]
    ProfileIncompatible,
    #[serde(rename = "source.invalid")]
    SourceInvalid,
    #[serde(rename = "source.not_loaded")]
    SourceNotLoaded,
    #[serde(rename = "deck.invalid")]
    DeckInvalid,
    #[serde(rename = "deck.incompatible")]
    DeckIncompatible,
    #[serde(rename = "capture.invalid_state")]
    CaptureInvalidState,
    #[serde(rename = "capture.not_ready")]
    CaptureNotReady,
    #[serde(rename = "capture.limit_exceeded")]
    CaptureLimitExceeded,
    #[serde(rename = "state.busy")]
    StateBusy,
    #[serde(rename = "worker.internal")]
    WorkerInternal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Unconfigured,
    Ready,
    Busy,
    Faulted,
    Stopping,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecState {
    Unloaded,
    Loading,
    Ready,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerState {
    Empty,
    Loading,
    Ready,
    Playing,
    Paused,
    EndOfStream,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeckState {
    Empty,
    Loading,
    Ready,
    Playing,
    Paused,
    Capturing,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    Idle,
    Starting,
    Capturing,
    Finalizing,
    Completed,
    Aborted,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Snapshot,
    LiveCapture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Cpu,
    Cuda,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TensorDtype {
    Float16,
    Bfloat16,
    Float32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportIntent {
    Play,
    Pause,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    UserRequest,
    HostExit,
    ProtocolFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingKind {
    LatentTensor,
    DecodedRgba,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LimitedVec<T, const MAX: usize>(Vec<T>);

impl<T, const MAX: usize> LimitedVec<T, MAX> {
    /// Construct a bounded vector without truncation.
    ///
    /// # Errors
    ///
    /// Returns an error if the value contains more than `MAX` entries.
    pub fn try_from_vec(values: Vec<T>) -> Result<Self, ValidationError> {
        if values.len() > MAX {
            return Err(ValidationError::BoundExceeded("array"));
        }
        Ok(Self(values))
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}

impl<T, const MAX: usize> Default for LimitedVec<T, MAX> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T: Serialize, const MAX: usize> Serialize for LimitedVec<T, MAX> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for LimitedVec<T, MAX> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct LimitedVecVisitor<T, const MAX: usize>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for LimitedVecVisitor<T, MAX> {
            type Value = LimitedVec<T, MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "an array containing at most {MAX} items")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                if sequence.size_hint().is_some_and(|size| size > MAX) {
                    return Err(de::Error::custom(format_args!(
                        "array exceeds the {MAX}-item bound"
                    )));
                }
                let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or(0).min(MAX));
                while let Some(value) = sequence.next_element()? {
                    if values.len() == MAX {
                        return Err(de::Error::custom(format_args!(
                            "array exceeds the {MAX}-item bound"
                        )));
                    }
                    values.push(value);
                }
                Ok(LimitedVec(values))
            }
        }

        deserializer.deserialize_seq(LimitedVecVisitor::<T, MAX>(PhantomData))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileKey {
    pub codec_family: String,
    pub profile: String,
    pub profile_version: String,
}

impl ProfileKey {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.codec_family, "profile_key.codec_family")?;
        identifier(&self.profile, "profile_key.profile")?;
        version(&self.profile_version, "profile_key.profile_version")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalGeometry {
    pub channels: u32,
    pub latent_height: u32,
    pub latent_width: u32,
    pub decoded_height: u32,
    pub decoded_width: u32,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
    pub timing_contract: String,
    pub timing_contract_version: String,
}

impl SignalGeometry {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.channels == 0
            || self.latent_height == 0
            || self.latent_width == 0
            || self.decoded_height == 0
            || self.decoded_width == 0
            || self.frame_rate_numerator == 0
            || self.frame_rate_denominator == 0
        {
            return Err(ValidationError::InvalidField("signal_geometry"));
        }
        identifier(&self.timing_contract, "signal_geometry.timing_contract")?;
        version(
            &self.timing_contract_version,
            "signal_geometry.timing_contract_version",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TensorAbi {
    pub python_major: u8,
    pub python_minor: u8,
    pub torch_version: String,
    pub dtype: TensorDtype,
    pub shape: [u32; 5],
    pub contiguous: bool,
    pub device: DeviceKind,
}

impl TensorAbi {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.python_major != 3
            || self.python_minor != 13
            || self.shape[0] != 1
            || self.shape[2] != 1
            || self.shape.contains(&0)
            || !self.contiguous
        {
            return Err(ValidationError::InvalidField("tensor_abi"));
        }
        version(&self.torch_version, "tensor_abi.torch_version")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodedAbi {
    pub pixel_format: String,
    pub maximum_batch: u8,
}

impl DecodedAbi {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.pixel_format != "rgba8" || !(1..=MAX_DECODE_BATCH).contains(&self.maximum_batch) {
            return Err(ValidationError::InvalidField("decoded_abi"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileReceipt {
    #[serde(with = "wire_uuid")]
    pub receipt_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
    pub payload_sha256: String,
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub profile_key: ProfileKey,
    pub signal_geometry: SignalGeometry,
    pub tensor_abi: TensorAbi,
    pub decoded_abi: DecodedAbi,
    pub capabilities: LimitedVec<Capability, MAX_CAPABILITIES>,
    pub estimated_host_bytes: u64,
    pub estimated_device_bytes: u64,
}

impl ProfileReceipt {
    /// Validate that the receipt binds exact immutable codec and cartridge identities.
    ///
    /// # Errors
    ///
    /// Returns the first identity, hash, ABI, capability, or bound violation.
    pub fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.receipt_id, "profile_receipt.receipt_id")?;
        non_nil(self.cartridge_id, "profile_receipt.cartridge_id")?;
        sha256(&self.archive_sha256, "profile_receipt.archive_sha256")?;
        sha256(&self.payload_sha256, "profile_receipt.payload_sha256")?;
        identifier(&self.pack_id, "profile_receipt.pack_id")?;
        version(&self.pack_version, "profile_receipt.pack_version")?;
        identifier(&self.adapter_id, "profile_receipt.adapter_id")?;
        version(&self.adapter_version, "profile_receipt.adapter_version")?;
        self.profile_key.validate()?;
        self.signal_geometry.validate()?;
        self.tensor_abi.validate()?;
        self.decoded_abi.validate()?;
        if self.tensor_abi.shape[1] != self.signal_geometry.channels
            || self.tensor_abi.shape[3] != self.signal_geometry.latent_height
            || self.tensor_abi.shape[4] != self.signal_geometry.latent_width
        {
            return Err(ValidationError::InvalidField(
                "profile_receipt.tensor_geometry",
            ));
        }
        if self.capabilities.is_empty() {
            return Err(ValidationError::InvalidField(
                "profile_receipt.capabilities",
            ));
        }
        unique_capabilities(&self.capabilities)?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawImportTensorStream {
    Visual,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RawImportStorageDtype {
    F16,
    F32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawImportAudioPolicy {
    SourceAbsent,
    PreservedSource,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportTensor {
    pub stream: RawImportTensorStream,
    pub name: String,
    pub storage_dtype: RawImportStorageDtype,
    pub runtime_dtype: RawImportStorageDtype,
    pub shape: LimitedVec<u64, MAX_RAW_IMPORT_TENSOR_AXES>,
}

impl RawImportTensor {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.name, "raw_import.tensor.name")?;
        if self.shape.is_empty() {
            return Err(ValidationError::InvalidField("raw_import.tensor.shape"));
        }
        let mut values = 1_u64;
        for axis in self.shape.as_slice() {
            values = values
                .checked_mul(*axis)
                .ok_or(ValidationError::InvalidField("raw_import.tensor.shape"))?;
            if *axis == 0 || values > MAX_RAW_IMPORT_SOURCE_BYTES {
                return Err(ValidationError::InvalidField("raw_import.tensor.shape"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportMetadata {
    pub profile_key: ProfileKey,
    pub payload_entry: String,
    pub payload_media_type: String,
    pub tensors: LimitedVec<RawImportTensor, MAX_RAW_IMPORT_TENSORS>,
    pub timing_contract: String,
    pub timing_contract_version: String,
    pub decoded_width: u32,
    pub decoded_height: u32,
    pub decoded_frame_count: u64,
    pub frame_rate_numerator: u64,
    pub frame_rate_denominator: u64,
    pub duration_numerator: u64,
    pub duration_denominator: u64,
    pub audio_policy: RawImportAudioPolicy,
}

impl RawImportMetadata {
    /// Validate bounded adapter metadata before Core constructs an LC manifest.
    ///
    /// # Errors
    ///
    /// Returns the first profile, entry, tensor, timing, or audio-policy violation.
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.profile_key.validate()?;
        archive_entry(&self.payload_entry, "raw_import.payload_entry")?;
        if self.payload_media_type != "application/vnd.safetensors" || self.tensors.is_empty() {
            return Err(ValidationError::InvalidField("raw_import.payload"));
        }
        let mut visual_count = 0_usize;
        let mut audio_count = 0_usize;
        for tensor in self.tensors.as_slice() {
            tensor.validate()?;
            match tensor.stream {
                RawImportTensorStream::Visual => visual_count += 1,
                RawImportTensorStream::Audio => audio_count += 1,
            }
        }
        unique_by(
            self.tensors.as_slice(),
            |tensor| tensor.name.as_str(),
            "raw_import.tensors",
        )?;
        if visual_count != 1
            || audio_count > 1
            || (audio_count == 1) != (self.audio_policy == RawImportAudioPolicy::PreservedSource)
        {
            return Err(ValidationError::InvalidField("raw_import.audio_policy"));
        }
        identifier(&self.timing_contract, "raw_import.timing_contract")?;
        version(
            &self.timing_contract_version,
            "raw_import.timing_contract_version",
        )?;
        if self.decoded_width == 0
            || self.decoded_height == 0
            || self.decoded_frame_count == 0
            || self.frame_rate_numerator == 0
            || self.frame_rate_denominator == 0
            || self.duration_numerator == 0
            || self.duration_denominator == 0
        {
            return Err(ValidationError::InvalidField("raw_import.timing"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportPreflightRequest {
    #[serde(with = "wire_uuid")]
    pub import_id: Uuid,
    pub source_path: String,
    pub maximum_source_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportStage {
    #[serde(with = "wire_uuid")]
    pub import_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub receipt_id: Uuid,
    pub staging_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportAbort {
    #[serde(with = "wire_uuid")]
    pub import_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub receipt_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportPreflight {
    #[serde(with = "wire_uuid")]
    pub receipt_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub import_id: Uuid,
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub source_sha256: String,
    pub source_byte_length: u64,
    pub metadata: RawImportMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportArtifact {
    #[serde(with = "wire_uuid")]
    pub receipt_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub import_id: Uuid,
    pub staged_payload_path: String,
    pub payload_sha256: String,
    pub payload_byte_length: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawImportAborted {
    #[serde(with = "wire_uuid")]
    pub import_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub receipt_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAssetBinding {
    pub asset_id: String,
    pub path: String,
    pub sha256: String,
    pub byte_length: u64,
}

impl ExternalAssetBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.asset_id, "external_asset.asset_id")?;
        bounded_text(&self.path, "external_asset.path", MAX_PATH_BYTES)?;
        sha256(&self.sha256, "external_asset.sha256")?;
        if self.byte_length == 0 {
            return Err(ValidationError::InvalidField("external_asset.byte_length"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ControlValue {
    Boolean(bool),
    Integer(i64),
    Number(f64),
    Text(String),
}

impl ControlValue {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Number(value) if !value.is_finite() => {
                Err(ValidationError::InvalidField("control.value"))
            }
            Self::Text(value) => bounded_text(value, "control.value", MAX_TEXT_BYTES),
            Self::Boolean(_) | Self::Integer(_) | Self::Number(_) => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlBinding {
    pub name: String,
    pub value: ControlValue,
}

impl ControlBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.name, "control.name")?;
        self.value.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBinding {
    pub role: String,
    pub physical_slot: u8,
}

impl RoleBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.role, "role.role")?;
        if self.physical_slot == 0 || usize::from(self.physical_slot) > MAX_SOURCES {
            return Err(ValidationError::InvalidField("role.physical_slot"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBinding {
    pub physical_slot: u8,
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
    #[serde(with = "wire_uuid")]
    pub profile_receipt_id: Uuid,
    pub loop_enabled: bool,
}

impl SourceBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.physical_slot == 0 || usize::from(self.physical_slot) > MAX_SOURCES {
            return Err(ValidationError::InvalidField("source.physical_slot"));
        }
        non_nil(self.source_id, "source.source_id")?;
        non_nil(self.cartridge_id, "source.cartridge_id")?;
        non_nil(self.profile_receipt_id, "source.profile_receipt_id")?;
        sha256(&self.archive_sha256, "source.archive_sha256")
    }
}

/// Independent playback intent retained by one physical Deck source slot.
///
/// Slot identity, play/pause, and looping stay separate from logical role
/// bindings so carrier/donor permutation never moves causal source history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTransportBinding {
    pub physical_slot: u8,
    pub playing: bool,
    pub loop_enabled: bool,
}

impl SourceTransportBinding {
    fn validate(self) -> Result<(), ValidationError> {
        if self.physical_slot == 0 || usize::from(self.physical_slot) > MAX_SOURCES {
            return Err(ValidationError::InvalidField(
                "deck.transport.physical_slot",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyPayload {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfigure {
    pub selected_protocol_version: u16,
    pub app_version: String,
    pub heartbeat_interval_ms: u32,
    pub heartbeat_hard_timeout_ms: u32,
    pub max_frame_bytes: u32,
    pub max_inflight_batches: u8,
    pub requested_capabilities: LimitedVec<Capability, MAX_CAPABILITIES>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionShutdown {
    pub reason: ShutdownReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecDescriptorRequest {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecLoad {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub device: DeviceKind,
    pub device_ordinal: u8,
    pub external_assets: LimitedVec<ExternalAssetBinding, MAX_EXTERNAL_ASSETS>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecUnload {
    pub pack_id: String,
    pub pack_version: String,
}

/// Transfers only a duplicated native handle identity to the worker.
/// Cartridge or tensor bytes never enter the Protocol 2 frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOpen {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub retained_native_handle: u64,
    pub integrity_access_receipt: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceClose {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
}

/// Binds a host-created shared ring by duplicated handles, never inline bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingConfigure {
    #[serde(with = "wire_uuid")]
    pub ring_id: Uuid,
    pub kind: RingKind,
    pub mapping_handle: u64,
    pub ready_event_handle: u64,
    pub consumed_event_handle: u64,
    pub slot_count: u8,
    pub slot_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingRelease {
    #[serde(with = "wire_uuid")]
    pub ring_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInspect {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileValidate {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
    pub expected_profile: ProfileKey,
    pub required_capabilities: LimitedVec<Capability, MAX_CAPABILITIES>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerOpen {
    #[serde(with = "wire_uuid")]
    pub player_session_id: Uuid,
    pub source: SourceBinding,
    pub stream_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerStep {
    #[serde(with = "wire_uuid")]
    pub player_session_id: Uuid,
    pub stream_generation: u64,
    pub maximum_decoded_frames: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerReset {
    #[serde(with = "wire_uuid")]
    pub player_session_id: Uuid,
    pub new_stream_generation: u64,
}

/// Exact host-verified Deck package runtime selected for this load.
///
/// The worker may import only this entrypoint from this absolute Python root.
/// The hashes bind the runtime to the active immutable package receipt; package
/// bytes themselves never cross Protocol 2.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckRuntimeBinding {
    pub deck_id: String,
    pub deck_version: String,
    pub operator_id: String,
    pub operator_version: String,
    pub python_root: String,
    pub entrypoint: String,
    pub package_manifest_sha256: String,
    pub integrity_catalog_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckLoad {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_id: String,
    pub deck_version: String,
    pub operator_id: String,
    pub operator_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<DeckRuntimeBinding>,
    pub sources: LimitedVec<SourceBinding, MAX_SOURCES>,
    pub roles: LimitedVec<RoleBinding, MAX_ROLES>,
    pub controls: LimitedVec<ControlBinding, MAX_CONTROLS>,
    pub seed: u64,
    pub stream_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckIdentity {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckProcess {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    pub stream_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckControlsSet {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    pub controls: LimitedVec<ControlBinding, MAX_CONTROLS>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckRolesSet {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    pub roles: LimitedVec<RoleBinding, MAX_ROLES>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckTransportSet {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    pub sources: LimitedVec<SourceTransportBinding, MAX_SOURCES>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckSeedSet {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    pub seed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckReset {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    pub new_stream_generation: u64,
    pub preserve_playheads: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStart {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    #[serde(with = "wire_uuid")]
    pub capture_id: Uuid,
    pub mode: CaptureMode,
    pub staging_root: String,
    pub maximum_latent_slots: u64,
    pub maximum_visual_bytes: u64,
    pub maximum_reset_events: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureIdentity {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    #[serde(with = "wire_uuid")]
    pub capture_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandName {
    #[serde(rename = "session.configure")]
    SessionConfigure,
    #[serde(rename = "session.status")]
    SessionStatus,
    #[serde(rename = "session.shutdown")]
    SessionShutdown,
    #[serde(rename = "codec.descriptor")]
    CodecDescriptor,
    #[serde(rename = "codec.load")]
    CodecLoad,
    #[serde(rename = "codec.unload")]
    CodecUnload,
    #[serde(rename = "source.open")]
    SourceOpen,
    #[serde(rename = "source.close")]
    SourceClose,
    #[serde(rename = "ring.configure")]
    RingConfigure,
    #[serde(rename = "ring.release")]
    RingRelease,
    #[serde(rename = "profile.inspect")]
    ProfileInspect,
    #[serde(rename = "profile.validate")]
    ProfileValidate,
    #[serde(rename = "raw_import.preflight")]
    RawImportPreflight,
    #[serde(rename = "raw_import.stage")]
    RawImportStage,
    #[serde(rename = "raw_import.abort")]
    RawImportAbort,
    #[serde(rename = "player.open")]
    PlayerOpen,
    #[serde(rename = "player.step")]
    PlayerStep,
    #[serde(rename = "player.reset")]
    PlayerReset,
    #[serde(rename = "player.status")]
    PlayerStatus,
    #[serde(rename = "deck.load")]
    DeckLoad,
    #[serde(rename = "deck.process")]
    DeckProcess,
    #[serde(rename = "deck.controls.set")]
    DeckControlsSet,
    #[serde(rename = "deck.roles.set")]
    DeckRolesSet,
    #[serde(rename = "deck.transport.set")]
    DeckTransportSet,
    #[serde(rename = "deck.seed.set")]
    DeckSeedSet,
    #[serde(rename = "deck.reset")]
    DeckReset,
    #[serde(rename = "deck.restart")]
    DeckRestart,
    #[serde(rename = "deck.status")]
    DeckStatus,
    #[serde(rename = "capture.start")]
    CaptureStart,
    #[serde(rename = "capture.stop")]
    CaptureStop,
    #[serde(rename = "capture.status")]
    CaptureStatus,
    #[serde(rename = "metrics.get")]
    MetricsGet,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "name", content = "payload", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "session.configure")]
    SessionConfigure(SessionConfigure),
    #[serde(rename = "session.status")]
    SessionStatus(EmptyPayload),
    #[serde(rename = "session.shutdown")]
    SessionShutdown(SessionShutdown),
    #[serde(rename = "codec.descriptor")]
    CodecDescriptor(CodecDescriptorRequest),
    #[serde(rename = "codec.load")]
    CodecLoad(CodecLoad),
    #[serde(rename = "codec.unload")]
    CodecUnload(CodecUnload),
    #[serde(rename = "source.open")]
    SourceOpen(SourceOpen),
    #[serde(rename = "source.close")]
    SourceClose(SourceClose),
    #[serde(rename = "ring.configure")]
    RingConfigure(RingConfigure),
    #[serde(rename = "ring.release")]
    RingRelease(RingRelease),
    #[serde(rename = "profile.inspect")]
    ProfileInspect(ProfileInspect),
    #[serde(rename = "profile.validate")]
    ProfileValidate(ProfileValidate),
    #[serde(rename = "raw_import.preflight")]
    RawImportPreflight(RawImportPreflightRequest),
    #[serde(rename = "raw_import.stage")]
    RawImportStage(RawImportStage),
    #[serde(rename = "raw_import.abort")]
    RawImportAbort(RawImportAbort),
    #[serde(rename = "player.open")]
    PlayerOpen(PlayerOpen),
    #[serde(rename = "player.step")]
    PlayerStep(PlayerStep),
    #[serde(rename = "player.reset")]
    PlayerReset(PlayerReset),
    #[serde(rename = "player.status")]
    PlayerStatus(EmptyPayload),
    #[serde(rename = "deck.load")]
    DeckLoad(Box<DeckLoad>),
    #[serde(rename = "deck.process")]
    DeckProcess(DeckProcess),
    #[serde(rename = "deck.controls.set")]
    DeckControlsSet(DeckControlsSet),
    #[serde(rename = "deck.roles.set")]
    DeckRolesSet(DeckRolesSet),
    #[serde(rename = "deck.transport.set")]
    DeckTransportSet(DeckTransportSet),
    #[serde(rename = "deck.seed.set")]
    DeckSeedSet(DeckSeedSet),
    #[serde(rename = "deck.reset")]
    DeckReset(DeckReset),
    #[serde(rename = "deck.restart")]
    DeckRestart(DeckIdentity),
    #[serde(rename = "deck.status")]
    DeckStatus(EmptyPayload),
    #[serde(rename = "capture.start")]
    CaptureStart(CaptureStart),
    #[serde(rename = "capture.stop")]
    CaptureStop(CaptureIdentity),
    #[serde(rename = "capture.status")]
    CaptureStatus(CaptureIdentity),
    #[serde(rename = "metrics.get")]
    MetricsGet(EmptyPayload),
}

impl Command {
    #[must_use]
    pub const fn name(&self) -> CommandName {
        match self {
            Self::SessionConfigure(_) => CommandName::SessionConfigure,
            Self::SessionStatus(_) => CommandName::SessionStatus,
            Self::SessionShutdown(_) => CommandName::SessionShutdown,
            Self::CodecDescriptor(_) => CommandName::CodecDescriptor,
            Self::CodecLoad(_) => CommandName::CodecLoad,
            Self::CodecUnload(_) => CommandName::CodecUnload,
            Self::SourceOpen(_) => CommandName::SourceOpen,
            Self::SourceClose(_) => CommandName::SourceClose,
            Self::RingConfigure(_) => CommandName::RingConfigure,
            Self::RingRelease(_) => CommandName::RingRelease,
            Self::ProfileInspect(_) => CommandName::ProfileInspect,
            Self::ProfileValidate(_) => CommandName::ProfileValidate,
            Self::RawImportPreflight(_) => CommandName::RawImportPreflight,
            Self::RawImportStage(_) => CommandName::RawImportStage,
            Self::RawImportAbort(_) => CommandName::RawImportAbort,
            Self::PlayerOpen(_) => CommandName::PlayerOpen,
            Self::PlayerStep(_) => CommandName::PlayerStep,
            Self::PlayerReset(_) => CommandName::PlayerReset,
            Self::PlayerStatus(_) => CommandName::PlayerStatus,
            Self::DeckLoad(_) => CommandName::DeckLoad,
            Self::DeckProcess(_) => CommandName::DeckProcess,
            Self::DeckControlsSet(_) => CommandName::DeckControlsSet,
            Self::DeckRolesSet(_) => CommandName::DeckRolesSet,
            Self::DeckTransportSet(_) => CommandName::DeckTransportSet,
            Self::DeckSeedSet(_) => CommandName::DeckSeedSet,
            Self::DeckReset(_) => CommandName::DeckReset,
            Self::DeckRestart(_) => CommandName::DeckRestart,
            Self::DeckStatus(_) => CommandName::DeckStatus,
            Self::CaptureStart(_) => CommandName::CaptureStart,
            Self::CaptureStop(_) => CommandName::CaptureStop,
            Self::CaptureStatus(_) => CommandName::CaptureStatus,
            Self::MetricsGet(_) => CommandName::MetricsGet,
        }
    }

    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::SessionConfigure(value) => value.validate(),
            Self::SessionStatus(_)
            | Self::SessionShutdown(_)
            | Self::PlayerStatus(_)
            | Self::DeckStatus(_)
            | Self::MetricsGet(_) => Ok(()),
            Self::CodecDescriptor(value) => value.validate(),
            Self::CodecLoad(value) => value.validate(),
            Self::CodecUnload(value) => value.validate(),
            Self::SourceOpen(value) => value.validate(),
            Self::SourceClose(value) => value.validate(),
            Self::RingConfigure(value) => value.validate(),
            Self::RingRelease(value) => value.validate(),
            Self::ProfileInspect(value) => value.validate(),
            Self::ProfileValidate(value) => value.validate(),
            Self::RawImportPreflight(value) => value.validate(),
            Self::RawImportStage(value) => value.validate(),
            Self::RawImportAbort(value) => value.validate(),
            Self::PlayerOpen(value) => value.validate(),
            Self::PlayerStep(value) => value.validate(),
            Self::PlayerReset(value) => value.validate(),
            Self::DeckLoad(value) => value.validate(),
            Self::DeckProcess(value) => value.validate(),
            Self::DeckControlsSet(value) => value.validate(),
            Self::DeckRolesSet(value) => value.validate(),
            Self::DeckTransportSet(value) => value.validate(),
            Self::DeckSeedSet(value) => value.validate(),
            Self::DeckReset(value) => value.validate(),
            Self::DeckRestart(value) => value.validate(),
            Self::CaptureStart(value) => value.validate(),
            Self::CaptureStop(value) | Self::CaptureStatus(value) => value.validate(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusSnapshot {
    pub session: SessionState,
    pub codec: CodecState,
    pub player: PlayerState,
    pub deck: DeckState,
    pub capture: CaptureState,
    pub open_session_count: u8,
    #[serde(with = "optional_wire_uuid")]
    pub foreground_output_session: Option<Uuid>,
    pub output_lease_pinned: bool,
}

impl StatusSnapshot {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.open_session_count > MAX_WARM_SESSIONS {
            return Err(ValidationError::BoundExceeded("status.open_session_count"));
        }
        if self.output_lease_pinned && self.foreground_output_session.is_none() {
            return Err(ValidationError::InvalidField("status.output_lease_pinned"));
        }
        if self
            .foreground_output_session
            .is_some_and(|value| value.is_nil())
        {
            return Err(ValidationError::NilIdentifier(
                "status.foreground_output_session",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfigured {
    pub selected_protocol_version: u16,
    pub maximum_frame_bytes: u32,
    pub accepted_capabilities: LimitedVec<Capability, MAX_CAPABILITIES>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecDescriptor {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub host_api_version: String,
    pub capabilities: LimitedVec<Capability, MAX_CAPABILITIES>,
    pub profiles: LimitedVec<ProfileKey, MAX_PROFILES>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecLoaded {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub device: DeviceKind,
    pub device_ordinal: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecUnloaded {
    pub pack_id: String,
    pub pack_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOpened {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceClosed {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingConfigured {
    #[serde(with = "wire_uuid")]
    pub ring_id: Uuid,
    pub kind: RingKind,
    pub slot_count: u8,
    pub slot_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingReleased {
    #[serde(with = "wire_uuid")]
    pub ring_id: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInspection {
    #[serde(with = "wire_uuid")]
    pub source_id: Uuid,
    #[serde(with = "wire_uuid")]
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
    pub payload_sha256: String,
    pub profile_key: ProfileKey,
    pub signal_geometry: SignalGeometry,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerStatusSnapshot {
    #[serde(with = "wire_uuid")]
    pub player_session_id: Uuid,
    pub state: PlayerState,
    pub stream_generation: u64,
    pub stream_sequence: u64,
    pub playhead_slot: u64,
    pub end_of_stream: bool,
    #[serde(with = "optional_wire_uuid")]
    pub decoded_ring_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerStepAck {
    pub status: PlayerStatusSnapshot,
    #[serde(with = "optional_wire_uuid")]
    pub output_ring_id: Option<Uuid>,
    pub output_slot_sequence: u64,
    pub decoded_frames: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayheadSnapshot {
    pub physical_slot: u8,
    pub latent_slot: u64,
    pub loop_enabled: bool,
    pub end_of_stream: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceEntry {
    pub key: String,
    pub value: ControlValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckStatusSnapshot {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub state: DeckState,
    pub deck_revision: u64,
    pub stream_generation: u64,
    pub stream_sequence: u64,
    pub playheads: LimitedVec<PlayheadSnapshot, MAX_SOURCES>,
    pub roles: LimitedVec<RoleBinding, MAX_ROLES>,
    pub controls: LimitedVec<ControlBinding, MAX_CONTROLS>,
    pub source_transport: LimitedVec<SourceTransportBinding, MAX_SOURCES>,
    pub seed: u64,
    pub capture_state: CaptureState,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckProcessAck {
    pub status: DeckStatusSnapshot,
    #[serde(with = "wire_uuid")]
    pub output_ring_id: Uuid,
    pub output_slot_sequence: u64,
    pub provenance: LimitedVec<ProvenanceEntry, MAX_CONTROLS>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureArtifact {
    pub staged_payload_path: String,
    pub payload_sha256: String,
    pub payload_byte_length: u64,
    pub latent_slots: u64,
    pub decoded_frame_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStatusSnapshot {
    #[serde(with = "wire_uuid")]
    pub deck_session_id: Uuid,
    pub deck_revision: u64,
    #[serde(with = "wire_uuid")]
    pub capture_id: Uuid,
    pub state: CaptureState,
    pub mode: CaptureMode,
    pub latent_slots: u64,
    pub reset_events: u32,
    pub artifact: Option<CaptureArtifact>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsSnapshot {
    pub worker_uptime_ns: u64,
    pub commands_total: u64,
    pub commands_failed_total: u64,
    pub player_steps_total: u64,
    pub deck_process_total: u64,
    pub capture_slots_total: u64,
    pub decoded_frames_total: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownAck {
    pub reason: ShutdownReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "name", content = "payload", deny_unknown_fields)]
pub enum Ack {
    #[serde(rename = "session.configure")]
    SessionConfigure(SessionConfigured),
    #[serde(rename = "session.status")]
    SessionStatus(StatusSnapshot),
    #[serde(rename = "session.shutdown")]
    SessionShutdown(ShutdownAck),
    #[serde(rename = "codec.descriptor")]
    CodecDescriptor(CodecDescriptor),
    #[serde(rename = "codec.load")]
    CodecLoad(CodecLoaded),
    #[serde(rename = "codec.unload")]
    CodecUnload(CodecUnloaded),
    #[serde(rename = "source.open")]
    SourceOpen(SourceOpened),
    #[serde(rename = "source.close")]
    SourceClose(SourceClosed),
    #[serde(rename = "ring.configure")]
    RingConfigure(RingConfigured),
    #[serde(rename = "ring.release")]
    RingRelease(RingReleased),
    #[serde(rename = "profile.inspect")]
    ProfileInspect(ProfileInspection),
    #[serde(rename = "profile.validate")]
    ProfileValidate(Box<ProfileReceipt>),
    #[serde(rename = "raw_import.preflight")]
    RawImportPreflight(Box<RawImportPreflight>),
    #[serde(rename = "raw_import.stage")]
    RawImportStage(RawImportArtifact),
    #[serde(rename = "raw_import.abort")]
    RawImportAbort(RawImportAborted),
    #[serde(rename = "player.open")]
    PlayerOpen(PlayerStatusSnapshot),
    #[serde(rename = "player.step")]
    PlayerStep(PlayerStepAck),
    #[serde(rename = "player.reset")]
    PlayerReset(PlayerStatusSnapshot),
    #[serde(rename = "player.status")]
    PlayerStatus(PlayerStatusSnapshot),
    #[serde(rename = "deck.load")]
    DeckLoad(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.process")]
    DeckProcess(Box<DeckProcessAck>),
    #[serde(rename = "deck.controls.set")]
    DeckControlsSet(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.roles.set")]
    DeckRolesSet(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.transport.set")]
    DeckTransportSet(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.seed.set")]
    DeckSeedSet(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.reset")]
    DeckReset(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.restart")]
    DeckRestart(Box<DeckStatusSnapshot>),
    #[serde(rename = "deck.status")]
    DeckStatus(Box<DeckStatusSnapshot>),
    #[serde(rename = "capture.start")]
    CaptureStart(Box<CaptureStatusSnapshot>),
    #[serde(rename = "capture.stop")]
    CaptureStop(Box<CaptureStatusSnapshot>),
    #[serde(rename = "capture.status")]
    CaptureStatus(Box<CaptureStatusSnapshot>),
    #[serde(rename = "metrics.get")]
    MetricsGet(MetricsSnapshot),
}

impl Ack {
    #[must_use]
    pub const fn name(&self) -> CommandName {
        match self {
            Self::SessionConfigure(_) => CommandName::SessionConfigure,
            Self::SessionStatus(_) => CommandName::SessionStatus,
            Self::SessionShutdown(_) => CommandName::SessionShutdown,
            Self::CodecDescriptor(_) => CommandName::CodecDescriptor,
            Self::CodecLoad(_) => CommandName::CodecLoad,
            Self::CodecUnload(_) => CommandName::CodecUnload,
            Self::SourceOpen(_) => CommandName::SourceOpen,
            Self::SourceClose(_) => CommandName::SourceClose,
            Self::RingConfigure(_) => CommandName::RingConfigure,
            Self::RingRelease(_) => CommandName::RingRelease,
            Self::ProfileInspect(_) => CommandName::ProfileInspect,
            Self::ProfileValidate(_) => CommandName::ProfileValidate,
            Self::RawImportPreflight(_) => CommandName::RawImportPreflight,
            Self::RawImportStage(_) => CommandName::RawImportStage,
            Self::RawImportAbort(_) => CommandName::RawImportAbort,
            Self::PlayerOpen(_) => CommandName::PlayerOpen,
            Self::PlayerStep(_) => CommandName::PlayerStep,
            Self::PlayerReset(_) => CommandName::PlayerReset,
            Self::PlayerStatus(_) => CommandName::PlayerStatus,
            Self::DeckLoad(_) => CommandName::DeckLoad,
            Self::DeckProcess(_) => CommandName::DeckProcess,
            Self::DeckControlsSet(_) => CommandName::DeckControlsSet,
            Self::DeckRolesSet(_) => CommandName::DeckRolesSet,
            Self::DeckTransportSet(_) => CommandName::DeckTransportSet,
            Self::DeckSeedSet(_) => CommandName::DeckSeedSet,
            Self::DeckReset(_) => CommandName::DeckReset,
            Self::DeckRestart(_) => CommandName::DeckRestart,
            Self::DeckStatus(_) => CommandName::DeckStatus,
            Self::CaptureStart(_) => CommandName::CaptureStart,
            Self::CaptureStop(_) => CommandName::CaptureStop,
            Self::CaptureStatus(_) => CommandName::CaptureStatus,
            Self::MetricsGet(_) => CommandName::MetricsGet,
        }
    }

    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::SessionConfigure(value) => value.validate(),
            Self::SessionStatus(value) => value.validate(),
            Self::SessionShutdown(_) | Self::MetricsGet(_) => Ok(()),
            Self::CodecDescriptor(value) => value.validate(),
            Self::CodecLoad(value) => value.validate(),
            Self::CodecUnload(value) => value.validate(),
            Self::SourceOpen(value) => value.validate(),
            Self::SourceClose(value) => value.validate(),
            Self::RingConfigure(value) => value.validate(),
            Self::RingRelease(value) => value.validate(),
            Self::ProfileInspect(value) => value.validate(),
            Self::ProfileValidate(value) => value.validate(),
            Self::RawImportPreflight(value) => value.validate(),
            Self::RawImportStage(value) => value.validate(),
            Self::RawImportAbort(value) => value.validate(),
            Self::PlayerOpen(value) | Self::PlayerReset(value) | Self::PlayerStatus(value) => {
                value.validate()
            }
            Self::PlayerStep(value) => value.validate(),
            Self::DeckLoad(value)
            | Self::DeckControlsSet(value)
            | Self::DeckRolesSet(value)
            | Self::DeckTransportSet(value)
            | Self::DeckSeedSet(value)
            | Self::DeckReset(value)
            | Self::DeckRestart(value)
            | Self::DeckStatus(value) => value.validate(),
            Self::DeckProcess(value) => value.validate(),
            Self::CaptureStart(value) | Self::CaptureStop(value) | Self::CaptureStatus(value) => {
                value.validate()
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorDetail {
    pub key: String,
    pub value: String,
}

impl ErrorDetail {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.key, "error.detail.key")?;
        bounded_text(&self.value, "error.detail.value", MAX_TEXT_BYTES)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub fatal: bool,
    pub status: StatusSnapshot,
    #[serde(with = "wire_uuid")]
    pub diagnostic_id: Uuid,
    pub details: LimitedVec<ErrorDetail, MAX_ERROR_DETAILS>,
}

impl ErrorPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        bounded_text(&self.message, "error.message", MAX_TEXT_BYTES)?;
        self.status.validate()?;
        non_nil(self.diagnostic_id, "error.diagnostic_id")?;
        for detail in self.details.as_slice() {
            detail.validate()?;
        }
        unique_by(
            self.details.as_slice(),
            |detail| detail.key.as_str(),
            "error.details",
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckReply {
    #[serde(with = "wire_uuid")]
    pub reply_to: Uuid,
    pub ack: Ack,
    pub status: StatusSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorReply {
    #[serde(with = "wire_uuid")]
    pub reply_to: Uuid,
    pub name: CommandName,
    pub error: ErrorPayload,
}

/// Strict first-frame authentication and runtime identity for Protocol 2.
///
/// This event is never accepted after another inbound worker frame. The
/// stateful session validator and Windows supervisor enforce that ordering.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerHello {
    pub auth_token: WorkerHelloAuthToken,
    pub worker_pid: u32,
    pub worker_identity: String,
    pub runtime_identity: String,
    pub protocol_min: u16,
    pub protocol_max: u16,
}

impl WorkerHello {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.worker_pid == 0 {
            return Err(ValidationError::InvalidField("worker_hello.worker_pid"));
        }
        bounded_text(
            &self.worker_identity,
            "worker_hello.worker_identity",
            MAX_IDENTIFIER_BYTES,
        )?;
        bounded_text(
            &self.runtime_identity,
            "worker_hello.runtime_identity",
            MAX_TEXT_BYTES,
        )?;
        if self.protocol_min != PROTOCOL_VERSION || self.protocol_max != PROTOCOL_VERSION {
            return Err(ValidationError::UnsupportedVersion(
                self.protocol_min.min(self.protocol_max),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", content = "payload", deny_unknown_fields)]
pub enum Event {
    #[serde(rename = "worker.hello")]
    WorkerHello(WorkerHello),
    #[serde(rename = "status.changed")]
    StatusChanged(StatusSnapshot),
    #[serde(rename = "worker.heartbeat")]
    WorkerHeartbeat(StatusSnapshot),
    #[serde(rename = "worker.fault")]
    WorkerFault(ErrorPayload),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventMessage {
    #[serde(with = "optional_wire_uuid")]
    pub caused_by: Option<Uuid>,
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Message {
    Command(Command),
    Ack(AckReply),
    Error(ErrorReply),
    Event(EventMessage),
}

impl Message {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Command(command) => command.validate(),
            Self::Ack(reply) => {
                non_nil(reply.reply_to, "ack.reply_to")?;
                reply.ack.validate()?;
                reply.status.validate()
            }
            Self::Error(reply) => {
                non_nil(reply.reply_to, "error.reply_to")?;
                reply.error.validate()
            }
            Self::Event(event) => {
                if event.caused_by.is_some_and(|value| value.is_nil()) {
                    return Err(ValidationError::NilIdentifier("event.caused_by"));
                }
                match &event.event {
                    Event::WorkerHello(hello) => hello.validate(),
                    Event::StatusChanged(status) | Event::WorkerHeartbeat(status) => {
                        status.validate()
                    }
                    Event::WorkerFault(error) => error.validate(),
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub protocol: ProtocolMarker,
    pub protocol_version: u16,
    #[serde(with = "wire_uuid")]
    pub session_id: Uuid,
    pub sequence: u64,
    #[serde(with = "wire_uuid")]
    pub message_id: Uuid,
    pub sender_uptime_ns: u64,
    pub message: Message,
}

impl Envelope {
    #[must_use]
    pub const fn new(
        session_id: Uuid,
        sequence: u64,
        message_id: Uuid,
        sender_uptime_ns: u64,
        message: Message,
    ) -> Self {
        Self {
            protocol: ProtocolMarker::LatentDeckWorker,
            protocol_version: PROTOCOL_VERSION,
            session_id,
            sequence,
            message_id,
            sender_uptime_ns,
            message,
        }
    }

    /// Validate all transport-independent envelope and payload invariants.
    ///
    /// # Errors
    ///
    /// Returns the first version, identity, message, or bound violation.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol_version != PROTOCOL_VERSION {
            return Err(ValidationError::UnsupportedVersion(self.protocol_version));
        }
        non_nil(self.session_id, "envelope.session_id")?;
        non_nil(self.message_id, "envelope.message_id")?;
        if self.sequence == 0 {
            return Err(ValidationError::InvalidField("envelope.sequence"));
        }
        self.message.validate()
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("Protocol 2 does not accept version {0}")]
    UnsupportedVersion(u16),
    #[error("{0} is invalid")]
    InvalidField(&'static str),
    #[error("{0} exceeds its bound")]
    BoundExceeded(&'static str),
    #[error("{0} must not be nil")]
    NilIdentifier(&'static str),
    #[error("{0} contains duplicate values")]
    DuplicateValue(&'static str),
    #[error("envelope belongs to a different Protocol 2 session")]
    SessionMismatch,
    #[error("Protocol 2 sequence mismatch: expected {expected}, received {actual}")]
    SequenceMismatch { expected: u64, actual: u64 },
    #[error("Protocol 2 inbound message identifier was reused")]
    DuplicateMessageId,
    #[error("Protocol 2 outbound command identifier was reused")]
    DuplicateCommandId,
    #[error("Protocol 2 expected an outbound command envelope")]
    ExpectedCommand,
    #[error("Protocol 2 session message budget is exhausted")]
    SessionMessageLimit,
    #[error("Protocol 2 pending command budget is exhausted")]
    PendingCommandLimit,
    #[error("Protocol 2 requires worker.hello as the first worker frame")]
    WorkerHelloRequired,
    #[error("Protocol 2 worker.hello is allowed exactly once as the first worker frame")]
    UnexpectedWorkerHello,
    #[error("Protocol 2 reply does not reference a pending command")]
    UnknownReply,
    #[error("Protocol 2 reply name mismatch: expected {expected:?}, received {actual:?}")]
    ReplyNameMismatch {
        expected: CommandName,
        actual: CommandName,
    },
    #[error("Protocol 2 event references an unknown command")]
    UnknownCause,
    #[error("Protocol 2 message direction is invalid")]
    UnexpectedMessageKind,
}

impl ValidationError {
    #[must_use]
    pub const fn stable_code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedVersion(_) => ErrorCode::ProtocolUnsupportedVersion,
            Self::BoundExceeded(_) | Self::SessionMessageLimit | Self::PendingCommandLimit => {
                ErrorCode::ProtocolBoundExceeded
            }
            Self::InvalidField(_)
            | Self::NilIdentifier(_)
            | Self::DuplicateValue(_)
            | Self::SessionMismatch
            | Self::SequenceMismatch { .. }
            | Self::DuplicateMessageId
            | Self::DuplicateCommandId
            | Self::ExpectedCommand
            | Self::WorkerHelloRequired
            | Self::UnexpectedWorkerHello
            | Self::UnknownReply
            | Self::ReplyNameMismatch { .. }
            | Self::UnknownCause
            | Self::UnexpectedMessageKind => ErrorCode::ProtocolInvalidMessage,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundPolicy {
    CommandsOnly,
    ResponsesAndEvents,
}

/// Stateful validator for one ordered reliable Protocol 2 session.
///
/// Host-side validation requires `worker.hello` as the exact first inbound
/// frame and retains every message/command identity for bounded duplicate and
/// correlation checks. Worker-side command validation uses `CommandsOnly` and
/// therefore does not expect the worker-originated hello on its inbound side.
pub struct SessionValidator {
    session_id: Uuid,
    policy: InboundPolicy,
    next_inbound_sequence: u64,
    next_outbound_sequence: u64,
    inbound_message_ids: HashSet<Uuid>,
    outbound_commands: HashMap<Uuid, CommandName>,
    pending_replies: HashSet<Uuid>,
    worker_hello_received: bool,
}

impl SessionValidator {
    #[must_use]
    pub fn new(session_id: Uuid, policy: InboundPolicy) -> Self {
        Self {
            session_id,
            policy,
            next_inbound_sequence: 1,
            next_outbound_sequence: 1,
            inbound_message_ids: HashSet::new(),
            outbound_commands: HashMap::new(),
            pending_replies: HashSet::new(),
            worker_hello_received: false,
        }
    }

    /// Record one host-to-worker command before writing its frame.
    ///
    /// # Errors
    ///
    /// Rejects invalid envelopes, wrong sessions or sequence numbers,
    /// duplicate command IDs, missing worker authentication, and session or
    /// pending-command bound violations.
    pub fn track_outbound_command(&mut self, envelope: &Envelope) -> Result<(), ValidationError> {
        envelope.validate()?;
        if envelope.session_id != self.session_id {
            return Err(ValidationError::SessionMismatch);
        }
        if self.policy == InboundPolicy::ResponsesAndEvents && !self.worker_hello_received {
            return Err(ValidationError::WorkerHelloRequired);
        }
        let Message::Command(command) = &envelope.message else {
            return Err(ValidationError::ExpectedCommand);
        };
        if envelope.sequence != self.next_outbound_sequence {
            return Err(ValidationError::SequenceMismatch {
                expected: self.next_outbound_sequence,
                actual: envelope.sequence,
            });
        }
        if self.outbound_commands.contains_key(&envelope.message_id) {
            return Err(ValidationError::DuplicateCommandId);
        }
        if self.outbound_commands.len() >= MAX_MESSAGES_PER_SESSION {
            return Err(ValidationError::SessionMessageLimit);
        }
        if self.pending_replies.len() >= MAX_PENDING_COMMANDS {
            return Err(ValidationError::PendingCommandLimit);
        }
        self.outbound_commands
            .insert(envelope.message_id, command.name());
        self.pending_replies.insert(envelope.message_id);
        self.next_outbound_sequence = self
            .next_outbound_sequence
            .checked_add(1)
            .ok_or(ValidationError::SessionMessageLimit)?;
        Ok(())
    }

    /// Validate and advance one ordered inbound worker frame.
    ///
    /// # Errors
    ///
    /// Rejects invalid envelopes, wrong sessions or sequence numbers,
    /// duplicate IDs, an invalid bootstrap order, unexpected message kinds,
    /// mismatched replies, and session bound violations.
    pub fn validate_inbound(&mut self, envelope: &Envelope) -> Result<(), ValidationError> {
        envelope.validate()?;
        if envelope.session_id != self.session_id {
            return Err(ValidationError::SessionMismatch);
        }
        if envelope.sequence != self.next_inbound_sequence {
            return Err(ValidationError::SequenceMismatch {
                expected: self.next_inbound_sequence,
                actual: envelope.sequence,
            });
        }
        if self.inbound_message_ids.len() >= MAX_MESSAGES_PER_SESSION {
            return Err(ValidationError::SessionMessageLimit);
        }
        if self.inbound_message_ids.contains(&envelope.message_id) {
            return Err(ValidationError::DuplicateMessageId);
        }

        let completed_reply = match (&self.policy, &envelope.message) {
            (InboundPolicy::CommandsOnly, Message::Command(_)) => None,
            (InboundPolicy::ResponsesAndEvents, Message::Event(event)) => {
                match &event.event {
                    Event::WorkerHello(_) => {
                        if self.worker_hello_received
                            || !self.inbound_message_ids.is_empty()
                            || event.caused_by.is_some()
                        {
                            return Err(ValidationError::UnexpectedWorkerHello);
                        }
                        self.worker_hello_received = true;
                    }
                    _ if !self.worker_hello_received => {
                        return Err(ValidationError::WorkerHelloRequired);
                    }
                    _ => {
                        if let Some(cause) = event.caused_by
                            && !self.outbound_commands.contains_key(&cause)
                        {
                            return Err(ValidationError::UnknownCause);
                        }
                    }
                }
                None
            }
            (InboundPolicy::ResponsesAndEvents, Message::Ack(reply)) => {
                if !self.worker_hello_received {
                    return Err(ValidationError::WorkerHelloRequired);
                }
                self.validate_reply(reply.reply_to, reply.ack.name())?;
                Some(reply.reply_to)
            }
            (InboundPolicy::ResponsesAndEvents, Message::Error(reply)) => {
                if !self.worker_hello_received {
                    return Err(ValidationError::WorkerHelloRequired);
                }
                self.validate_reply(reply.reply_to, reply.name)?;
                Some(reply.reply_to)
            }
            _ => return Err(ValidationError::UnexpectedMessageKind),
        };

        if let Some(reply_to) = completed_reply {
            self.pending_replies.remove(&reply_to);
        }
        self.inbound_message_ids.insert(envelope.message_id);
        self.next_inbound_sequence = self
            .next_inbound_sequence
            .checked_add(1)
            .ok_or(ValidationError::SessionMessageLimit)?;
        Ok(())
    }

    fn validate_reply(
        &self,
        reply_to: Uuid,
        actual_name: CommandName,
    ) -> Result<(), ValidationError> {
        if !self.pending_replies.contains(&reply_to) {
            return Err(ValidationError::UnknownReply);
        }
        let expected_name = self
            .outbound_commands
            .get(&reply_to)
            .copied()
            .ok_or(ValidationError::UnknownReply)?;
        if actual_name != expected_name {
            return Err(ValidationError::ReplyNameMismatch {
                expected: expected_name,
                actual: actual_name,
            });
        }
        Ok(())
    }

    #[must_use]
    pub const fn next_inbound_sequence(&self) -> u64 {
        self.next_inbound_sequence
    }

    #[must_use]
    pub const fn next_outbound_sequence(&self) -> u64 {
        self.next_outbound_sequence
    }

    #[must_use]
    pub fn remaining_inbound_message_budget(&self) -> usize {
        MAX_MESSAGES_PER_SESSION - self.inbound_message_ids.len()
    }

    #[must_use]
    pub fn remaining_outbound_message_budget(&self) -> usize {
        MAX_MESSAGES_PER_SESSION - self.outbound_commands.len()
    }

    #[must_use]
    pub fn has_pending_reply(&self, command_id: Uuid) -> bool {
        self.pending_replies.contains(&command_id)
    }
}

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("Protocol 2 payload is outside 1..={MAX_FRAME_BYTES} bytes")]
    FrameLength,
    #[error("Protocol 2 JSON failed: {0}")]
    Json(String),
    #[error("Protocol 2 MessagePack failed: {0}")]
    MessagePack(String),
    #[error("Protocol 2 payload contains trailing data")]
    TrailingData,
    #[error(transparent)]
    Validation(#[from] ValidationError),
}

/// Encode one validated Protocol 2 envelope as bounded JSON.
///
/// # Errors
///
/// Returns a validation, serialization, or frame-bound error.
pub fn encode_json(envelope: &Envelope) -> Result<Vec<u8>, CodecError> {
    envelope.validate()?;
    let payload =
        serde_json::to_vec(envelope).map_err(|error| CodecError::Json(error.to_string()))?;
    ensure_frame_length(payload.len())?;
    Ok(payload)
}

/// Decode one complete bounded JSON envelope and reject trailing input.
///
/// # Errors
///
/// Returns a decoding, validation, trailing-data, or frame-bound error.
pub fn decode_json(payload: &[u8]) -> Result<Envelope, CodecError> {
    ensure_frame_length(payload.len())?;
    let mut deserializer = serde_json::Deserializer::from_slice(payload);
    let envelope = Envelope::deserialize(&mut deserializer)
        .map_err(|error| CodecError::Json(error.to_string()))?;
    deserializer.end().map_err(|_| CodecError::TrailingData)?;
    envelope.validate()?;
    Ok(envelope)
}

/// Encode one validated Protocol 2 envelope as named `MessagePack`.
///
/// # Errors
///
/// Returns a validation, serialization, or frame-bound error.
pub fn encode_messagepack(envelope: &Envelope) -> Result<Vec<u8>, CodecError> {
    envelope.validate()?;
    let payload = rmp_serde::to_vec_named(envelope)
        .map_err(|error| CodecError::MessagePack(error.to_string()))?;
    ensure_frame_length(payload.len())?;
    Ok(payload)
}

/// Decode one complete bounded named-MessagePack envelope.
///
/// # Errors
///
/// Returns a decoding, validation, trailing-data, or frame-bound error.
pub fn decode_messagepack(payload: &[u8]) -> Result<Envelope, CodecError> {
    ensure_frame_length(payload.len())?;
    decode_messagepack_value(payload)
}

fn decode_messagepack_value<T: DeserializeOwned>(payload: &[u8]) -> Result<T, CodecError> {
    let mut cursor = Cursor::new(payload);
    let mut deserializer = rmp_serde::Deserializer::new(&mut cursor);
    let value = T::deserialize(&mut deserializer)
        .map_err(|error| CodecError::MessagePack(error.to_string()))?;
    drop(deserializer);
    if cursor.position() != payload.len() as u64 {
        return Err(CodecError::TrailingData);
    }
    Ok(value)
}

fn ensure_frame_length(length: usize) -> Result<(), CodecError> {
    if !(1..=MAX_FRAME_BYTES).contains(&length) {
        return Err(CodecError::FrameLength);
    }
    Ok(())
}

fn non_nil(value: Uuid, field: &'static str) -> Result<(), ValidationError> {
    if value.is_nil() {
        return Err(ValidationError::NilIdentifier(field));
    }
    Ok(())
}

fn identifier(value: &str, field: &'static str) -> Result<(), ValidationError> {
    bounded_text(value, field, MAX_IDENTIFIER_BYTES)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}

fn version(value: &str, field: &'static str) -> Result<(), ValidationError> {
    bounded_text(value, field, MAX_VERSION_BYTES)?;
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-' | b'_'))
    {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}

fn bounded_text(value: &str, field: &'static str, maximum: usize) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(ValidationError::BoundExceeded(field));
    }
    Ok(())
}

fn archive_entry(value: &str, field: &'static str) -> Result<(), ValidationError> {
    bounded_text(value, field, 512)?;
    if value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}

fn absolute_path(value: &str, field: &'static str) -> Result<(), ValidationError> {
    bounded_text(value, field, MAX_PATH_BYTES)?;
    if !Path::new(value).is_absolute() {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}

fn sha256(value: &str, field: &'static str) -> Result<(), ValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}

fn python_entrypoint(value: &str, field: &'static str) -> Result<(), ValidationError> {
    bounded_text(value, field, 512)?;
    let Some((module, attribute)) = value.split_once(':') else {
        return Err(ValidationError::InvalidField(field));
    };
    if attribute.contains(':')
        || module.is_empty()
        || attribute.is_empty()
        || !module.split('.').all(python_identifier)
        || !python_identifier(attribute)
    {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}

fn python_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn unique_capabilities(
    capabilities: &LimitedVec<Capability, MAX_CAPABILITIES>,
) -> Result<(), ValidationError> {
    let unique: HashSet<_> = capabilities.as_slice().iter().copied().collect();
    if unique.len() != capabilities.len() {
        return Err(ValidationError::DuplicateValue("capabilities"));
    }
    Ok(())
}

fn unique_by<'a, T, K: Eq + std::hash::Hash + ?Sized + 'a>(
    values: &'a [T],
    key: impl Fn(&'a T) -> &'a K,
    field: &'static str,
) -> Result<(), ValidationError> {
    let unique: HashSet<_> = values.iter().map(key).collect();
    if unique.len() != values.len() {
        return Err(ValidationError::DuplicateValue(field));
    }
    Ok(())
}

impl SessionConfigure {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.selected_protocol_version != PROTOCOL_VERSION
            || !(250..=60_000).contains(&self.heartbeat_interval_ms)
            || self.heartbeat_hard_timeout_ms < self.heartbeat_interval_ms.saturating_mul(3)
            || usize::try_from(self.max_frame_bytes).ok() != Some(MAX_FRAME_BYTES)
            || !(1..=24).contains(&self.max_inflight_batches)
        {
            return Err(ValidationError::InvalidField("session.configure"));
        }
        version(&self.app_version, "session.app_version")?;
        unique_capabilities(&self.requested_capabilities)
    }
}

impl CodecDescriptorRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.pack_id, "codec.pack_id")?;
        version(&self.pack_version, "codec.pack_version")?;
        identifier(&self.adapter_id, "codec.adapter_id")
    }
}

impl CodecLoad {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.pack_id, "codec.pack_id")?;
        version(&self.pack_version, "codec.pack_version")?;
        identifier(&self.adapter_id, "codec.adapter_id")?;
        version(&self.adapter_version, "codec.adapter_version")?;
        for asset in self.external_assets.as_slice() {
            asset.validate()?;
        }
        unique_by(
            self.external_assets.as_slice(),
            |asset| asset.asset_id.as_str(),
            "codec.external_assets",
        )
    }
}

impl CodecUnload {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.pack_id, "codec.pack_id")?;
        version(&self.pack_version, "codec.pack_version")
    }
}

impl SourceOpen {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "source.source_id")?;
        non_nil(self.cartridge_id, "source.cartridge_id")?;
        sha256(&self.archive_sha256, "source.archive_sha256")?;
        positive(self.archive_bytes, "source.archive_bytes")?;
        positive(self.retained_native_handle, "source.retained_native_handle")?;
        bounded_text(
            &self.integrity_access_receipt,
            "source.integrity_access_receipt",
            MAX_INTEGRITY_ACCESS_RECEIPT_BYTES,
        )
    }
}

impl SourceClose {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "source.source_id")
    }
}

impl RingConfigure {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.ring_id, "ring.ring_id")?;
        positive(self.mapping_handle, "ring.mapping_handle")?;
        positive(self.ready_event_handle, "ring.ready_event_handle")?;
        positive(self.consumed_event_handle, "ring.consumed_event_handle")?;
        if !(2..=MAX_DECODE_BATCH).contains(&self.slot_count) || self.slot_bytes == 0 {
            return Err(ValidationError::InvalidField("ring.geometry"));
        }
        Ok(())
    }
}

impl RingRelease {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.ring_id, "ring.ring_id")
    }
}

impl ProfileInspect {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "profile.source_id")?;
        non_nil(self.cartridge_id, "profile.cartridge_id")?;
        sha256(&self.archive_sha256, "profile.archive_sha256")
    }
}

impl ProfileValidate {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "profile.source_id")?;
        self.expected_profile.validate()?;
        unique_capabilities(&self.required_capabilities)
    }
}

impl RawImportPreflightRequest {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.import_id, "raw_import.import_id")?;
        absolute_path(&self.source_path, "raw_import.source_path")?;
        if !(1..=MAX_RAW_IMPORT_SOURCE_BYTES).contains(&self.maximum_source_bytes) {
            return Err(ValidationError::InvalidField(
                "raw_import.maximum_source_bytes",
            ));
        }
        Ok(())
    }
}

impl RawImportStage {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.import_id, "raw_import.import_id")?;
        non_nil(self.receipt_id, "raw_import.receipt_id")?;
        absolute_path(&self.staging_root, "raw_import.staging_root")
    }
}

impl RawImportAbort {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.import_id, "raw_import.import_id")?;
        non_nil(self.receipt_id, "raw_import.receipt_id")
    }
}

impl RawImportPreflight {
    /// Validate exact codec/source identity and bounded typed manifest metadata.
    ///
    /// # Errors
    ///
    /// Returns the first identity, hash, length, or metadata violation.
    pub fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.receipt_id, "raw_import.receipt_id")?;
        non_nil(self.import_id, "raw_import.import_id")?;
        identifier(&self.pack_id, "raw_import.pack_id")?;
        version(&self.pack_version, "raw_import.pack_version")?;
        identifier(&self.adapter_id, "raw_import.adapter_id")?;
        version(&self.adapter_version, "raw_import.adapter_version")?;
        sha256(&self.source_sha256, "raw_import.source_sha256")?;
        if !(1..=MAX_RAW_IMPORT_SOURCE_BYTES).contains(&self.source_byte_length) {
            return Err(ValidationError::InvalidField(
                "raw_import.source_byte_length",
            ));
        }
        self.metadata.validate()
    }
}

impl RawImportArtifact {
    /// Validate an adapter's staged payload identity without opening its path.
    ///
    /// # Errors
    ///
    /// Returns the first receipt, path, hash, or length violation.
    pub fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.receipt_id, "raw_import.receipt_id")?;
        non_nil(self.import_id, "raw_import.import_id")?;
        absolute_path(&self.staged_payload_path, "raw_import.staged_payload_path")?;
        sha256(&self.payload_sha256, "raw_import.payload_sha256")?;
        if !(1..=MAX_RAW_IMPORT_SOURCE_BYTES).contains(&self.payload_byte_length) {
            return Err(ValidationError::InvalidField(
                "raw_import.payload_byte_length",
            ));
        }
        Ok(())
    }
}

impl RawImportAborted {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.import_id, "raw_import.import_id")?;
        non_nil(self.receipt_id, "raw_import.receipt_id")
    }
}

impl PlayerOpen {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.player_session_id, "player.session_id")?;
        self.source.validate()?;
        positive(self.stream_generation, "player.stream_generation")
    }
}

impl PlayerStep {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.player_session_id, "player.session_id")?;
        positive(self.stream_generation, "player.stream_generation")?;
        if !(1..=MAX_DECODE_BATCH).contains(&self.maximum_decoded_frames) {
            return Err(ValidationError::InvalidField(
                "player.maximum_decoded_frames",
            ));
        }
        Ok(())
    }
}

impl PlayerReset {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.player_session_id, "player.session_id")?;
        positive(self.new_stream_generation, "player.new_stream_generation")
    }
}

impl DeckLoad {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        identifier(&self.deck_id, "deck.deck_id")?;
        version(&self.deck_version, "deck.deck_version")?;
        identifier(&self.operator_id, "deck.operator_id")?;
        version(&self.operator_version, "deck.operator_version")?;
        if let Some(runtime) = &self.runtime {
            runtime.validate()?;
            if runtime.deck_id != self.deck_id
                || runtime.deck_version != self.deck_version
                || runtime.operator_id != self.operator_id
                || runtime.operator_version != self.operator_version
            {
                return Err(ValidationError::InvalidField("deck.runtime.identity"));
            }
        }
        if self.sources.is_empty() {
            return Err(ValidationError::InvalidField("deck.sources"));
        }
        for source in self.sources.as_slice() {
            source.validate()?;
        }
        unique_by(
            self.sources.as_slice(),
            |source| &source.physical_slot,
            "deck.sources",
        )?;
        for role in self.roles.as_slice() {
            role.validate()?;
            if !self
                .sources
                .as_slice()
                .iter()
                .any(|source| source.physical_slot == role.physical_slot)
            {
                return Err(ValidationError::InvalidField("deck.roles"));
            }
        }
        unique_by(
            self.roles.as_slice(),
            |role| role.role.as_str(),
            "deck.roles",
        )?;
        for control in self.controls.as_slice() {
            control.validate()?;
        }
        unique_by(
            self.controls.as_slice(),
            |control| control.name.as_str(),
            "deck.controls",
        )?;
        positive(self.stream_generation, "deck.stream_generation")
    }
}

impl DeckRuntimeBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.deck_id, "deck.runtime.deck_id")?;
        version(&self.deck_version, "deck.runtime.deck_version")?;
        identifier(&self.operator_id, "deck.runtime.operator_id")?;
        version(&self.operator_version, "deck.runtime.operator_version")?;
        bounded_text(
            &self.python_root,
            "deck.runtime.python_root",
            MAX_PATH_BYTES,
        )?;
        if !Path::new(&self.python_root).is_absolute() {
            return Err(ValidationError::InvalidField("deck.runtime.python_root"));
        }
        python_entrypoint(&self.entrypoint, "deck.runtime.entrypoint")?;
        sha256(
            &self.package_manifest_sha256,
            "deck.runtime.package_manifest_sha256",
        )?;
        sha256(
            &self.integrity_catalog_sha256,
            "deck.runtime.integrity_catalog_sha256",
        )
    }
}

impl DeckIdentity {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")
    }
}

impl DeckProcess {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")?;
        positive(self.stream_generation, "deck.stream_generation")
    }
}

impl DeckControlsSet {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")?;
        for control in self.controls.as_slice() {
            control.validate()?;
        }
        unique_by(
            self.controls.as_slice(),
            |control| control.name.as_str(),
            "deck.controls",
        )
    }
}

impl DeckRolesSet {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")?;
        for role in self.roles.as_slice() {
            role.validate()?;
        }
        unique_by(
            self.roles.as_slice(),
            |role| role.role.as_str(),
            "deck.roles",
        )
    }
}

impl DeckTransportSet {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")?;
        if self.sources.is_empty() {
            return Err(ValidationError::InvalidField("deck.transport.sources"));
        }
        for source in self.sources.as_slice() {
            source.validate()?;
        }
        unique_by(
            self.sources.as_slice(),
            |source| &source.physical_slot,
            "deck.transport.sources",
        )
    }
}

impl DeckSeedSet {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")
    }
}

impl DeckReset {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        positive(self.deck_revision, "deck.revision")?;
        positive(self.new_stream_generation, "deck.new_stream_generation")
    }
}

impl CaptureStart {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "capture.deck_session_id")?;
        non_nil(self.capture_id, "capture.capture_id")?;
        positive(self.deck_revision, "capture.deck_revision")?;
        absolute_path(&self.staging_root, "capture.staging_root")?;
        if !(1..=MAX_CAPTURE_LATENT_SLOTS).contains(&self.maximum_latent_slots)
            || !(1..=MAX_CAPTURE_VISUAL_BYTES).contains(&self.maximum_visual_bytes)
            || !(1..=MAX_CAPTURE_EVENTS).contains(&self.maximum_reset_events)
        {
            return Err(ValidationError::InvalidField("capture.bounds"));
        }
        Ok(())
    }
}

impl CaptureIdentity {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "capture.deck_session_id")?;
        non_nil(self.capture_id, "capture.capture_id")?;
        positive(self.deck_revision, "capture.deck_revision")
    }
}

impl SessionConfigured {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.selected_protocol_version != PROTOCOL_VERSION
            || usize::try_from(self.maximum_frame_bytes).ok() != Some(MAX_FRAME_BYTES)
            || self.accepted_capabilities.is_empty()
        {
            return Err(ValidationError::InvalidField("session.configured"));
        }
        unique_capabilities(&self.accepted_capabilities)
    }
}

impl CodecDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.pack_id, "codec.pack_id")?;
        version(&self.pack_version, "codec.pack_version")?;
        identifier(&self.adapter_id, "codec.adapter_id")?;
        version(&self.adapter_version, "codec.adapter_version")?;
        if self.host_api_version != "2.0"
            || self.capabilities.is_empty()
            || self.profiles.is_empty()
        {
            return Err(ValidationError::InvalidField("codec.descriptor"));
        }
        unique_capabilities(&self.capabilities)?;
        if !Capability::REQUIRED_CODEC_V2
            .iter()
            .all(|required| self.capabilities.as_slice().contains(required))
        {
            return Err(ValidationError::InvalidField("codec.capabilities"));
        }
        for profile in self.profiles.as_slice() {
            profile.validate()?;
        }
        let unique: HashSet<_> = self
            .profiles
            .as_slice()
            .iter()
            .map(|profile| {
                (
                    profile.codec_family.as_str(),
                    profile.profile.as_str(),
                    profile.profile_version.as_str(),
                )
            })
            .collect();
        if unique.len() != self.profiles.len() {
            return Err(ValidationError::DuplicateValue("codec.profiles"));
        }
        Ok(())
    }
}

impl CodecLoaded {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.pack_id, "codec.pack_id")?;
        version(&self.pack_version, "codec.pack_version")?;
        identifier(&self.adapter_id, "codec.adapter_id")?;
        version(&self.adapter_version, "codec.adapter_version")
    }
}

impl CodecUnloaded {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.pack_id, "codec.pack_id")?;
        version(&self.pack_version, "codec.pack_version")
    }
}

impl SourceOpened {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "source.source_id")?;
        non_nil(self.cartridge_id, "source.cartridge_id")?;
        sha256(&self.archive_sha256, "source.archive_sha256")
    }
}

impl SourceClosed {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "source.source_id")
    }
}

impl RingConfigured {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.ring_id, "ring.ring_id")?;
        if !(2..=MAX_DECODE_BATCH).contains(&self.slot_count) || self.slot_bytes == 0 {
            return Err(ValidationError::InvalidField("ring.geometry"));
        }
        Ok(())
    }
}

impl RingReleased {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.ring_id, "ring.ring_id")
    }
}

impl ProfileInspection {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.source_id, "profile.source_id")?;
        non_nil(self.cartridge_id, "profile.cartridge_id")?;
        sha256(&self.archive_sha256, "profile.archive_sha256")?;
        sha256(&self.payload_sha256, "profile.payload_sha256")?;
        self.profile_key.validate()?;
        self.signal_geometry.validate()
    }
}

impl PlayerStatusSnapshot {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.player_session_id, "player.session_id")?;
        if self.state != PlayerState::Empty && self.stream_generation == 0 {
            return Err(ValidationError::InvalidField("player.stream_generation"));
        }
        if self.decoded_ring_id.is_some_and(|ring| ring.is_nil()) {
            return Err(ValidationError::NilIdentifier("player.decoded_ring_id"));
        }
        Ok(())
    }
}

impl PlayerStepAck {
    fn validate(&self) -> Result<(), ValidationError> {
        self.status.validate()?;
        if self.decoded_frames > MAX_DECODE_BATCH
            || self.output_ring_id.is_some_and(|ring| ring.is_nil())
            || (self.decoded_frames > 0 && self.output_ring_id.is_none())
        {
            return Err(ValidationError::InvalidField("player.step"));
        }
        Ok(())
    }
}

impl PlayheadSnapshot {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.physical_slot == 0 || usize::from(self.physical_slot) > MAX_SOURCES {
            return Err(ValidationError::InvalidField("deck.playhead.physical_slot"));
        }
        Ok(())
    }
}

impl ProvenanceEntry {
    fn validate(&self) -> Result<(), ValidationError> {
        identifier(&self.key, "deck.provenance.key")?;
        self.value.validate()
    }
}

impl DeckStatusSnapshot {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "deck.session_id")?;
        if self.state != DeckState::Empty
            && (self.deck_revision == 0 || self.stream_generation == 0)
        {
            return Err(ValidationError::InvalidField("deck.status_identity"));
        }
        for playhead in self.playheads.as_slice() {
            playhead.validate()?;
        }
        unique_by(
            self.playheads.as_slice(),
            |playhead| &playhead.physical_slot,
            "deck.playheads",
        )?;
        if self.source_transport.len() != self.playheads.len() {
            return Err(ValidationError::InvalidField("deck.source_transport"));
        }
        for source in self.source_transport.as_slice() {
            source.validate()?;
            if !self
                .playheads
                .as_slice()
                .iter()
                .any(|playhead| playhead.physical_slot == source.physical_slot)
            {
                return Err(ValidationError::InvalidField("deck.source_transport"));
            }
        }
        unique_by(
            self.source_transport.as_slice(),
            |source| &source.physical_slot,
            "deck.source_transport",
        )?;
        for role in self.roles.as_slice() {
            role.validate()?;
        }
        unique_by(
            self.roles.as_slice(),
            |role| role.role.as_str(),
            "deck.roles",
        )?;
        for control in self.controls.as_slice() {
            control.validate()?;
        }
        unique_by(
            self.controls.as_slice(),
            |control| control.name.as_str(),
            "deck.controls",
        )
    }
}

impl DeckProcessAck {
    fn validate(&self) -> Result<(), ValidationError> {
        self.status.validate()?;
        non_nil(self.output_ring_id, "deck.output_ring_id")?;
        positive(self.output_slot_sequence, "deck.output_slot_sequence")?;
        for entry in self.provenance.as_slice() {
            entry.validate()?;
        }
        unique_by(
            self.provenance.as_slice(),
            |entry| entry.key.as_str(),
            "deck.provenance",
        )
    }
}

impl CaptureArtifact {
    fn validate(&self) -> Result<(), ValidationError> {
        absolute_path(&self.staged_payload_path, "capture.staged_payload_path")?;
        sha256(&self.payload_sha256, "capture.payload_sha256")?;
        positive(self.payload_byte_length, "capture.payload_byte_length")?;
        if !(1..=MAX_CAPTURE_LATENT_SLOTS).contains(&self.latent_slots) {
            return Err(ValidationError::InvalidField("capture.latent_slots"));
        }
        positive(self.decoded_frame_count, "capture.decoded_frame_count")?;
        Ok(())
    }
}

impl CaptureStatusSnapshot {
    fn validate(&self) -> Result<(), ValidationError> {
        non_nil(self.deck_session_id, "capture.deck_session_id")?;
        non_nil(self.capture_id, "capture.capture_id")?;
        positive(self.deck_revision, "capture.deck_revision")?;
        if self.latent_slots > MAX_CAPTURE_LATENT_SLOTS || self.reset_events > MAX_CAPTURE_EVENTS {
            return Err(ValidationError::InvalidField("capture.bounds"));
        }
        if self.state == CaptureState::Idle {
            return Err(ValidationError::InvalidField("capture.state"));
        }
        match (&self.state, &self.artifact) {
            (CaptureState::Completed, Some(artifact)) => {
                artifact.validate()?;
                if artifact.latent_slots != self.latent_slots {
                    return Err(ValidationError::InvalidField("capture.artifact"));
                }
            }
            (CaptureState::Completed, None) | (_, Some(_)) => {
                return Err(ValidationError::InvalidField("capture.artifact"));
            }
            (_, None) => {}
        }
        Ok(())
    }
}

fn positive(value: u64, field: &'static str) -> Result<(), ValidationError> {
    if value == 0 {
        return Err(ValidationError::InvalidField(field));
    }
    Ok(())
}
