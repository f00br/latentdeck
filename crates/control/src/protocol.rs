use std::{
    collections::{HashMap, HashSet},
    fmt,
    marker::PhantomData,
    ops::Deref,
};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, SeqAccess, Visitor},
};
use thiserror::Error;
use uuid::Uuid;

pub const WORKER_PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_FRAME_BYTES: u32 = 262_144;
pub const MAX_MESSAGES_PER_SESSION: usize = 65_536;
pub const MAX_PENDING_COMMANDS: usize = 256;
pub const MAX_RING_MAPPING_BYTES: u64 = 256 * 1024 * 1024;

const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_HUMAN_STRING_BYTES: usize = 4_096;
const MAX_PATH_BYTES: usize = 32_768;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolMarker {
    #[serde(rename = "latentdeck.worker")]
    LatentDeckWorker,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WireUuid(Uuid);

impl WireUuid {
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn as_uuid(self) -> Uuid {
        self.0
    }

    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0.is_nil()
    }
}

impl fmt::Debug for WireUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for WireUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0.hyphenated())
    }
}

impl Serialize for WireUuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for WireUuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let parsed = Uuid::parse_str(&encoded).map_err(de::Error::custom)?;
        let canonical = parsed.hyphenated().to_string();
        if encoded != canonical {
            return Err(de::Error::custom(
                "UUID must be canonical lowercase hyphenated text",
            ));
        }
        Ok(Self(parsed))
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AuthToken([u8; 32]);

impl AuthToken {
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

impl fmt::Debug for AuthToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthToken([REDACTED])")
    }
}

impl Serialize for AuthToken {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for AuthToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct AuthTokenVisitor;

        impl Visitor<'_> for AuthTokenVisitor {
            type Value = AuthToken;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("exactly 32 authentication bytes")
            }

            fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                let bytes: [u8; 32] = value
                    .try_into()
                    .map_err(|_| E::invalid_length(value.len(), &self))?;
                Ok(AuthToken(bytes))
            }

            fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
                self.visit_bytes(&value)
            }
        }

        deserializer.deserialize_bytes(AuthTokenVisitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoundedVec<T, const MAX: usize>(Vec<T>);

impl<T, const MAX: usize> BoundedVec<T, MAX> {
    /// Construct a bounded vector without truncating its input.
    ///
    /// # Errors
    ///
    /// Returns an error when `values` contains more than `MAX` elements.
    pub fn try_from_vec(values: Vec<T>) -> Result<Self, ValidationError> {
        if values.len() > MAX {
            return Err(ValidationError::InvalidField {
                field: "bounded_vec",
                reason: "too many elements",
            });
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
}

impl<T, const MAX: usize> Default for BoundedVec<T, MAX> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T, const MAX: usize> Deref for BoundedVec<T, MAX> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Serialize, const MAX: usize> Serialize for BoundedVec<T, MAX> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>, const MAX: usize> Deserialize<'de> for BoundedVec<T, MAX> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BoundedVecVisitor<T, const MAX: usize>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>, const MAX: usize> Visitor<'de> for BoundedVecVisitor<T, MAX> {
            type Value = BoundedVec<T, MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "an array containing at most {MAX} elements")
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                if sequence.size_hint().is_some_and(|size| size > MAX) {
                    return Err(de::Error::custom(format_args!(
                        "array exceeds the {MAX}-element limit"
                    )));
                }
                let capacity = sequence.size_hint().unwrap_or(0).min(MAX);
                let mut values = Vec::with_capacity(capacity);
                while let Some(value) = sequence.next_element()? {
                    if values.len() == MAX {
                        return Err(de::Error::custom(format_args!(
                            "array exceeds the {MAX}-element limit"
                        )));
                    }
                    values.push(value);
                }
                Ok(BoundedVec(values))
            }
        }

        deserializer.deserialize_seq(BoundedVecVisitor::<T, MAX>(PhantomData))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub protocol: ProtocolMarker,
    pub protocol_version: u16,
    pub session_id: WireUuid,
    pub sequence: u64,
    pub message_id: WireUuid,
    pub sender_uptime_ns: u64,
    pub message: Message,
}

impl Envelope {
    #[must_use]
    pub const fn new(
        session_id: WireUuid,
        sequence: u64,
        message_id: WireUuid,
        sender_uptime_ns: u64,
        message: Message,
    ) -> Self {
        Self {
            protocol: ProtocolMarker::LatentDeckWorker,
            protocol_version: WORKER_PROTOCOL_VERSION,
            session_id,
            sequence,
            message_id,
            sender_uptime_ns,
            message,
        }
    }

    /// Validate fields that do not depend on peer/session history.
    ///
    /// # Errors
    ///
    /// Returns the first protocol or payload invariant violation.
    pub fn validate_static(&self) -> Result<(), ValidationError> {
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            return Err(ValidationError::UnsupportedProtocolVersion {
                actual: self.protocol_version,
            });
        }
        if self.sequence == 0 {
            return Err(ValidationError::SequenceZero);
        }
        if self.session_id.is_nil() {
            return Err(ValidationError::NilIdentifier("session_id"));
        }
        if self.message_id.is_nil() {
            return Err(ValidationError::NilIdentifier("message_id"));
        }
        self.message.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
            Self::Ack(reply) => reply.validate(),
            Self::Error(reply) => reply.validate(),
            Self::Event(event) => event.validate(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CommandName {
    #[serde(rename = "session.configure")]
    SessionConfigure,
    #[serde(rename = "codec.inspect")]
    CodecInspect,
    #[serde(rename = "codec.load")]
    CodecLoad,
    #[serde(rename = "slot.load")]
    SlotLoad,
    #[serde(rename = "slot.reset")]
    SlotReset,
    #[serde(rename = "slot.decode_cycle")]
    SlotDecodeCycle,
    #[serde(rename = "ring.bind")]
    RingBind,
    #[serde(rename = "worker.status")]
    WorkerStatus,
    #[serde(rename = "metrics.get")]
    MetricsGet,
    #[serde(rename = "worker.shutdown")]
    WorkerShutdown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", content = "payload", deny_unknown_fields)]
pub enum Command {
    #[serde(rename = "session.configure")]
    SessionConfigure(SessionConfigure),
    #[serde(rename = "codec.inspect")]
    CodecInspect(EmptyPayload),
    #[serde(rename = "codec.load")]
    CodecLoad(CodecLoad),
    #[serde(rename = "slot.load")]
    SlotLoad(SlotLoad),
    #[serde(rename = "slot.reset")]
    SlotReset(SlotReset),
    #[serde(rename = "slot.decode_cycle")]
    SlotDecodeCycle(SlotDecodeCycle),
    #[serde(rename = "ring.bind")]
    RingBind(RingBind),
    #[serde(rename = "worker.status")]
    WorkerStatus(EmptyPayload),
    #[serde(rename = "metrics.get")]
    MetricsGet(EmptyPayload),
    #[serde(rename = "worker.shutdown")]
    WorkerShutdown(WorkerShutdown),
}

impl Command {
    #[must_use]
    pub const fn name(&self) -> CommandName {
        match self {
            Self::SessionConfigure(_) => CommandName::SessionConfigure,
            Self::CodecInspect(_) => CommandName::CodecInspect,
            Self::CodecLoad(_) => CommandName::CodecLoad,
            Self::SlotLoad(_) => CommandName::SlotLoad,
            Self::SlotReset(_) => CommandName::SlotReset,
            Self::SlotDecodeCycle(_) => CommandName::SlotDecodeCycle,
            Self::RingBind(_) => CommandName::RingBind,
            Self::WorkerStatus(_) => CommandName::WorkerStatus,
            Self::MetricsGet(_) => CommandName::MetricsGet,
            Self::WorkerShutdown(_) => CommandName::WorkerShutdown,
        }
    }

    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::SessionConfigure(value) => value.validate(),
            Self::CodecInspect(_)
            | Self::WorkerStatus(_)
            | Self::MetricsGet(_)
            | Self::WorkerShutdown(_) => Ok(()),
            Self::CodecLoad(value) => value.validate(),
            Self::SlotLoad(value) => value.validate(),
            Self::SlotReset(value) => value.validate(),
            Self::SlotDecodeCycle(value) => value.validate(),
            Self::RingBind(value) => value.validate(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckReply {
    pub reply_to: WireUuid,
    pub ack: Ack,
}

impl AckReply {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.reply_to.is_nil() {
            return Err(ValidationError::NilIdentifier("reply_to"));
        }
        self.ack.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", content = "payload", deny_unknown_fields)]
pub enum Ack {
    #[serde(rename = "session.configure")]
    SessionConfigure(SessionConfigured),
    #[serde(rename = "codec.inspect")]
    CodecInspect(CodecInspection),
    #[serde(rename = "codec.load")]
    CodecLoad(CodecLoaded),
    #[serde(rename = "slot.load")]
    SlotLoad(SlotLoaded),
    #[serde(rename = "slot.reset")]
    SlotReset(SlotResetAck),
    #[serde(rename = "slot.decode_cycle")]
    SlotDecodeCycle(DecodeCycleAck),
    #[serde(rename = "ring.bind")]
    RingBind(RingBound),
    #[serde(rename = "worker.status")]
    WorkerStatus(StatusSnapshot),
    #[serde(rename = "metrics.get")]
    MetricsGet(MetricsSnapshot),
    #[serde(rename = "worker.shutdown")]
    WorkerShutdown(ShutdownAck),
}

impl Ack {
    #[must_use]
    pub const fn name(&self) -> CommandName {
        match self {
            Self::SessionConfigure(_) => CommandName::SessionConfigure,
            Self::CodecInspect(_) => CommandName::CodecInspect,
            Self::CodecLoad(_) => CommandName::CodecLoad,
            Self::SlotLoad(_) => CommandName::SlotLoad,
            Self::SlotReset(_) => CommandName::SlotReset,
            Self::SlotDecodeCycle(_) => CommandName::SlotDecodeCycle,
            Self::RingBind(_) => CommandName::RingBind,
            Self::WorkerStatus(_) => CommandName::WorkerStatus,
            Self::MetricsGet(_) => CommandName::MetricsGet,
            Self::WorkerShutdown(_) => CommandName::WorkerShutdown,
        }
    }

    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::SessionConfigure(value) => value.validate(),
            Self::CodecInspect(value) => value.validate(),
            Self::CodecLoad(value) => value.validate(),
            Self::SlotLoad(value) => value.validate(),
            Self::SlotReset(value) => value.validate(),
            Self::SlotDecodeCycle(value) => value.validate(),
            Self::RingBind(value) => value.validate(),
            Self::WorkerStatus(value) => value.validate(),
            Self::MetricsGet(_) | Self::WorkerShutdown(_) => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorReply {
    pub reply_to: WireUuid,
    pub name: CommandName,
    pub error: ErrorPayload,
}

impl ErrorReply {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.reply_to.is_nil() {
            return Err(ValidationError::NilIdentifier("reply_to"));
        }
        self.error.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by: Option<WireUuid>,
    pub event: Event,
}

impl EventMessage {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.caused_by.is_some_and(WireUuid::is_nil) {
            return Err(ValidationError::NilIdentifier("caused_by"));
        }
        self.event.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", content = "payload", deny_unknown_fields)]
pub enum Event {
    #[serde(rename = "worker.hello")]
    WorkerHello(WorkerHello),
    #[serde(rename = "worker.heartbeat")]
    WorkerHeartbeat(WorkerHeartbeat),
    #[serde(rename = "worker.state_changed")]
    WorkerStateChanged(StateChanged),
    #[serde(rename = "metrics.snapshot")]
    MetricsSnapshot(MetricsSnapshot),
    #[serde(rename = "worker.fault")]
    WorkerFault(ErrorPayload),
}

impl Event {
    fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::WorkerHello(value) => value.validate(),
            Self::WorkerHeartbeat(value) => value.validate(),
            Self::WorkerStateChanged(value) => value.validate(),
            Self::MetricsSnapshot(_) => Ok(()),
            Self::WorkerFault(value) => value.validate(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    pub max_inflight_decode_batches: u16,
}

impl SessionConfigure {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.selected_protocol_version != WORKER_PROTOCOL_VERSION {
            return invalid("selected_protocol_version", "must equal 1");
        }
        validate_human("app_version", &self.app_version)?;
        if !(100..=10_000).contains(&self.heartbeat_interval_ms) {
            return invalid("heartbeat_interval_ms", "must be within 100..=10000");
        }
        if self.heartbeat_hard_timeout_ms < self.heartbeat_interval_ms.saturating_mul(3) {
            return invalid(
                "heartbeat_hard_timeout_ms",
                "must be at least three heartbeat intervals",
            );
        }
        if self.max_frame_bytes != MAX_CONTROL_FRAME_BYTES {
            return invalid("max_frame_bytes", "must equal protocol maximum");
        }
        if self.max_inflight_decode_batches != 1 {
            return invalid("max_inflight_decode_batches", "v1 requires exactly one");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfigured {
    pub selected_protocol_version: u16,
    pub heartbeat_interval_ms: u32,
    pub heartbeat_hard_timeout_ms: u32,
    pub max_frame_bytes: u32,
    pub max_inflight_decode_batches: u16,
}

impl SessionConfigured {
    fn validate(&self) -> Result<(), ValidationError> {
        SessionConfigure {
            selected_protocol_version: self.selected_protocol_version,
            app_version: "ack".to_owned(),
            heartbeat_interval_ms: self.heartbeat_interval_ms,
            heartbeat_hard_timeout_ms: self.heartbeat_hard_timeout_ms,
            max_frame_bytes: self.max_frame_bytes,
            max_inflight_decode_batches: self.max_inflight_decode_batches,
        }
        .validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRef {
    pub codec_family: String,
    pub profile: String,
    pub profile_version: String,
}

impl ProfileRef {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("codec_family", &self.codec_family)?;
        validate_identifier("profile", &self.profile)?;
        validate_human("profile_version", &self.profile_version)
    }
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
        validate_identifier("asset_id", &self.asset_id)?;
        validate_path("asset.path", &self.path)?;
        validate_sha256("asset.sha256", &self.sha256)?;
        if self.byte_length == 0 {
            return invalid("asset.byte_length", "must be nonzero");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecLoad {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub profile: ProfileRef,
    pub device_ordinal: u16,
    pub assets: BoundedVec<ExternalAssetBinding, 8>,
}

impl CodecLoad {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("pack_id", &self.pack_id)?;
        validate_human("pack_version", &self.pack_version)?;
        validate_identifier("adapter_id", &self.adapter_id)?;
        self.profile.validate()?;
        if self.assets.is_empty() {
            return invalid("assets", "at least one explicit decoder asset is required");
        }
        for asset in self.assets.iter() {
            asset.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecLoaded {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub profile: ProfileRef,
    pub device: DeviceDescriptor,
}

impl CodecLoaded {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("pack_id", &self.pack_id)?;
        validate_human("pack_version", &self.pack_version)?;
        validate_identifier("adapter_id", &self.adapter_id)?;
        validate_human("adapter_version", &self.adapter_version)?;
        self.profile.validate()?;
        self.device.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceDescriptor {
    pub ordinal: u16,
    pub name: String,
    pub total_memory_bytes: u64,
}

impl DeviceDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_human("device.name", &self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterDescriptor {
    pub adapter_id: String,
    pub adapter_version: String,
    pub profiles: BoundedVec<ProfileRef, 8>,
}

impl AdapterDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("adapter_id", &self.adapter_id)?;
        validate_human("adapter_version", &self.adapter_version)?;
        for profile in self.profiles.iter() {
            profile.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecInspection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torch_version: Option<String>,
    pub cuda_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cuda_runtime: Option<String>,
    pub devices: BoundedVec<DeviceDescriptor, 16>,
    pub adapters: BoundedVec<AdapterDescriptor, 16>,
}

impl CodecInspection {
    fn validate(&self) -> Result<(), ValidationError> {
        if let Some(value) = &self.torch_version {
            validate_human("torch_version", value)?;
        }
        if let Some(value) = &self.cuda_runtime {
            validate_human("cuda_runtime", value)?;
        }
        for device in self.devices.iter() {
            device.validate()?;
        }
        for adapter in self.adapters.iter() {
            adapter.validate()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotLoad {
    pub slot_id: String,
    pub cartridge_path: String,
    pub cartridge_id: WireUuid,
    pub expected_archive_sha256: String,
    pub stream_generation: u64,
}

impl SlotLoad {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("slot_id", &self.slot_id)?;
        validate_path("cartridge_path", &self.cartridge_path)?;
        if self.cartridge_id.is_nil() {
            return Err(ValidationError::NilIdentifier("cartridge_id"));
        }
        validate_sha256("expected_archive_sha256", &self.expected_archive_sha256)?;
        if self.stream_generation == 0 {
            return invalid("stream_generation", "must be nonzero");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingDescriptor {
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
    pub latent_slot_count: u64,
    pub decoded_frame_count: u64,
    pub cycle_count: u64,
    pub initial: CyclePattern,
    pub steady: CyclePattern,
    pub reset_required_on_wrap: bool,
    pub arbitrary_seek: bool,
    pub max_frames_per_cycle: u32,
}

impl TimingDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.frame_rate_numerator == 0 || self.frame_rate_denominator == 0 {
            return invalid("frame_rate", "numerator and denominator must be nonzero");
        }
        if self.latent_slot_count == 0 || self.decoded_frame_count == 0 || self.cycle_count == 0 {
            return invalid("timing", "slot, frame, and cycle counts must be nonzero");
        }
        if !self.reset_required_on_wrap || self.arbitrary_seek {
            return invalid(
                "timing.transport",
                "v1 requires reset-on-wrap and forbids arbitrary seek",
            );
        }
        if self.max_frames_per_cycle == 0 {
            return invalid("max_frames_per_cycle", "must be nonzero");
        }
        self.initial.validate()?;
        self.steady.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CyclePattern {
    pub first_cycle_index: u64,
    pub cycle_count: u64,
    pub latent_base: u64,
    pub latent_stride: u32,
    pub latent_count: u32,
    pub decoded_base: u64,
    pub decoded_stride: u32,
    pub decoded_count: u32,
}

impl CyclePattern {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.latent_count == 0 || self.decoded_count == 0 {
            return invalid("cycle_pattern", "per-cycle counts must be nonzero");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotLoaded {
    pub slot_id: String,
    pub slot_revision: u64,
    pub width: u32,
    pub height: u32,
    pub profile: ProfileRef,
    pub timing: TimingDescriptor,
}

impl SlotLoaded {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("slot_id", &self.slot_id)?;
        if self.slot_revision == 0 || self.width == 0 || self.height == 0 {
            return invalid("slot", "revision and dimensions must be nonzero");
        }
        self.profile.validate()?;
        self.timing.validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    Load,
    Loop,
    Restart,
    Recovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotReset {
    pub slot_id: String,
    pub slot_revision: u64,
    pub new_stream_generation: u64,
    pub reason: ResetReason,
}

impl SlotReset {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("slot_id", &self.slot_id)?;
        if self.slot_revision == 0 || self.new_stream_generation == 0 {
            return invalid("slot.reset", "revision and generation must be nonzero");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotResetAck {
    pub slot_id: String,
    pub slot_revision: u64,
    pub stream_generation: u64,
    pub next_cycle_index: u64,
    pub ring_write_sequence: u64,
}

impl SlotResetAck {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("slot_id", &self.slot_id)?;
        if self.slot_revision == 0 || self.stream_generation == 0 || self.next_cycle_index != 0 {
            return invalid(
                "slot.reset_ack",
                "revision/generation must be nonzero and next cycle must be zero",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotDecodeCycle {
    pub slot_id: String,
    pub slot_revision: u64,
    pub stream_generation: u64,
    pub cycle_index: u64,
}

impl SlotDecodeCycle {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("slot_id", &self.slot_id)?;
        if self.slot_revision == 0 || self.stream_generation == 0 {
            return invalid(
                "slot.decode_cycle",
                "revision and generation must be nonzero",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodeCycleAck {
    pub slot_id: String,
    pub slot_revision: u64,
    pub stream_generation: u64,
    pub cycle_index: u64,
    pub latent_start: u64,
    pub latent_count: u32,
    pub decoded_start_frame: u64,
    pub decoded_frame_count: u32,
    pub ring_first_sequence: u64,
    pub ring_last_sequence_exclusive: u64,
    pub end_of_stream: bool,
}

impl DecodeCycleAck {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_identifier("slot_id", &self.slot_id)?;
        if self.slot_revision == 0
            || self.stream_generation == 0
            || self.latent_count == 0
            || self.decoded_frame_count == 0
        {
            return invalid(
                "decode_cycle_ack",
                "revision, generation, and counts must be nonzero",
            );
        }
        let expected_end = self
            .ring_first_sequence
            .checked_add(u64::from(self.decoded_frame_count))
            .ok_or(ValidationError::InvalidField {
                field: "ring sequence",
                reason: "overflow",
            })?;
        if expected_end != self.ring_last_sequence_exclusive {
            return invalid(
                "ring_last_sequence_exclusive",
                "must equal first sequence plus decoded frame count",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingBind {
    pub layout_version: u16,
    pub mapping_handle: u64,
    pub mapping_bytes: u64,
    pub frames_ready_event_handle: u64,
    pub ring_id: WireUuid,
}

impl RingBind {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.layout_version != 1 {
            return invalid("layout_version", "must equal 1");
        }
        if self.mapping_handle == 0 || self.frames_ready_event_handle == 0 {
            return invalid("ring handles", "must be nonzero target-process handles");
        }
        if !(4_096..=MAX_RING_MAPPING_BYTES).contains(&self.mapping_bytes) {
            return invalid("mapping_bytes", "outside v1 runtime bounds");
        }
        if self.ring_id.is_nil() {
            return Err(ValidationError::NilIdentifier("ring_id"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RingBound {
    pub layout_version: u16,
    pub ring_id: WireUuid,
    pub mapping_bytes: u64,
}

impl RingBound {
    fn validate(&self) -> Result<(), ValidationError> {
        RingBind {
            layout_version: self.layout_version,
            mapping_handle: 1,
            mapping_bytes: self.mapping_bytes,
            frames_ready_event_handle: 1,
            ring_id: self.ring_id,
        }
        .validate()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Handshaking,
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
pub enum SlotState {
    Empty,
    Loading,
    Ready,
    Decoding,
    EndOfStream,
    Faulted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingState {
    Unbound,
    Binding,
    Ready,
    Closing,
    Faulted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusSnapshot {
    pub worker_state: WorkerState,
    pub codec_state: CodecState,
    pub slot_state: SlotState,
    pub ring_state: RingState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_slot_id: Option<String>,
    pub worker_version: String,
    pub protocol_version: u16,
}

impl StatusSnapshot {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.active_generation == Some(0) {
            return invalid("active_generation", "must be nonzero when present");
        }
        if let Some(value) = &self.active_slot_id {
            validate_identifier("active_slot_id", value)?;
        }
        validate_human("worker_version", &self.worker_version)?;
        if self.protocol_version != WORKER_PROTOCOL_VERSION {
            return invalid("protocol_version", "must equal 1");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsSnapshot {
    pub worker_uptime_ns: u64,
    pub decode_batches_total: u64,
    pub decoded_frames_total: u64,
    pub ring_backpressure_total: u64,
    pub presentation_skipped_total: u64,
    pub last_decode_duration_ns: u64,
    pub ring_write_sequence: u64,
    pub ring_read_sequence: u64,
    pub ring_occupancy: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_allocated_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu_reserved_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerHello {
    pub auth_token: AuthToken,
    pub worker_version: String,
    pub protocol_min: u16,
    pub protocol_max: u16,
    pub pid: u32,
    pub os: String,
    pub arch: String,
    pub python_version: String,
    pub available_adapters: BoundedVec<String, 16>,
}

impl WorkerHello {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_human("worker_version", &self.worker_version)?;
        if self.protocol_min == 0
            || self.protocol_min > WORKER_PROTOCOL_VERSION
            || self.protocol_max < WORKER_PROTOCOL_VERSION
            || self.protocol_min > self.protocol_max
        {
            return invalid("protocol range", "does not include worker protocol 1");
        }
        if self.pid == 0 {
            return invalid("pid", "must be nonzero");
        }
        validate_identifier("os", &self.os)?;
        validate_identifier("arch", &self.arch)?;
        validate_human("python_version", &self.python_version)?;
        for adapter in self.available_adapters.iter() {
            validate_identifier("available_adapter", adapter)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerHeartbeat {
    pub worker_state: WorkerState,
    pub codec_state: CodecState,
    pub slot_state: SlotState,
    pub ring_state: RingState,
    pub stream_generation: u64,
    pub last_completed_core_sequence: u64,
    pub decode_in_flight: bool,
    pub worker_uptime_ns: u64,
}

impl WorkerHeartbeat {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.stream_generation == 0
            && !matches!(self.slot_state, SlotState::Empty | SlotState::Loading)
        {
            return invalid(
                "stream_generation",
                "must be nonzero while a slot is active",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateChanged {
    pub status: StatusSnapshot,
    pub reason: String,
}

impl StateChanged {
    fn validate(&self) -> Result<(), ValidationError> {
        self.status.validate()?;
        validate_human("state_change.reason", &self.reason)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    UserRequest,
    ApplicationExit,
    Recovery,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerShutdown {
    pub reason: ShutdownReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownAck {
    pub accepted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "protocol.bad_length")]
    ProtocolBadLength,
    #[serde(rename = "protocol.frame_too_large")]
    ProtocolFrameTooLarge,
    #[serde(rename = "protocol.truncated_frame")]
    ProtocolTruncatedFrame,
    #[serde(rename = "protocol.invalid_msgpack")]
    ProtocolInvalidMessagePack,
    #[serde(rename = "protocol.duplicate_key")]
    ProtocolDuplicateKey,
    #[serde(rename = "protocol.schema_invalid")]
    ProtocolSchemaInvalid,
    #[serde(rename = "protocol.unsupported_version")]
    ProtocolUnsupportedVersion,
    #[serde(rename = "protocol.session_mismatch")]
    ProtocolSessionMismatch,
    #[serde(rename = "protocol.authentication_failed")]
    ProtocolAuthenticationFailed,
    #[serde(rename = "protocol.sequence_invalid")]
    ProtocolSequenceInvalid,
    #[serde(rename = "protocol.duplicate_command_id")]
    ProtocolDuplicateCommandId,
    #[serde(rename = "protocol.unknown_command")]
    ProtocolUnknownCommand,
    #[serde(rename = "state.invalid_transition")]
    StateInvalidTransition,
    #[serde(rename = "state.busy")]
    StateBusy,
    #[serde(rename = "state.stale_slot_revision")]
    StateStaleSlotRevision,
    #[serde(rename = "state.stale_generation")]
    StateStaleGeneration,
    #[serde(rename = "codec.pack_missing")]
    CodecPackMissing,
    #[serde(rename = "codec.pack_invalid")]
    CodecPackInvalid,
    #[serde(rename = "codec.pack_incompatible")]
    CodecPackIncompatible,
    #[serde(rename = "codec.runtime_corrupt")]
    CodecRuntimeCorrupt,
    #[serde(rename = "codec.adapter_missing")]
    CodecAdapterMissing,
    #[serde(rename = "codec.cuda_unavailable")]
    CodecCudaUnavailable,
    #[serde(rename = "codec.asset_unbound")]
    CodecAssetUnbound,
    #[serde(rename = "codec.asset_missing")]
    CodecAssetMissing,
    #[serde(rename = "codec.asset_license_unconfirmed")]
    CodecAssetLicenseUnconfirmed,
    #[serde(rename = "codec.asset_hash_mismatch")]
    CodecAssetHashMismatch,
    #[serde(rename = "codec.asset_format_invalid")]
    CodecAssetFormatInvalid,
    #[serde(rename = "codec.asset_incompatible")]
    CodecAssetIncompatible,
    #[serde(rename = "codec.load_failed")]
    CodecLoadFailed,
    #[serde(rename = "slot.cartridge_missing")]
    SlotCartridgeMissing,
    #[serde(rename = "slot.cartridge_hash_mismatch")]
    SlotCartridgeHashMismatch,
    #[serde(rename = "slot.cartridge_invalid")]
    SlotCartridgeInvalid,
    #[serde(rename = "slot.profile_incompatible")]
    SlotProfileIncompatible,
    #[serde(rename = "decode.cycle_out_of_order")]
    DecodeCycleOutOfOrder,
    #[serde(rename = "decode.causal_state_invalid")]
    DecodeCausalStateInvalid,
    #[serde(rename = "decode.failed")]
    DecodeFailed,
    #[serde(rename = "decode.gpu_oom")]
    DecodeGpuOutOfMemory,
    #[serde(rename = "ring.unbound")]
    RingUnbound,
    #[serde(rename = "ring.layout_incompatible")]
    RingLayoutIncompatible,
    #[serde(rename = "ring.invalid_handle")]
    RingInvalidHandle,
    #[serde(rename = "ring.runtime_geometry_too_large")]
    RingRuntimeGeometryTooLarge,
    #[serde(rename = "ring.backpressure")]
    RingBackpressure,
    #[serde(rename = "ring.publish_failed")]
    RingPublishFailed,
    #[serde(rename = "ring.sequence_exhausted")]
    RingSequenceExhausted,
    #[serde(rename = "worker.heartbeat_timeout")]
    WorkerHeartbeatTimeout,
    #[serde(rename = "worker.command_timeout")]
    WorkerCommandTimeout,
    #[serde(rename = "worker.process_exited")]
    WorkerProcessExited,
    #[serde(rename = "worker.internal")]
    WorkerInternal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorDetail {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    pub fatal: bool,
    pub worker_state: WorkerState,
    pub diagnostic_id: WireUuid,
    pub details: BoundedVec<ErrorDetail, 16>,
}

impl ErrorPayload {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_human("error.message", &self.message)?;
        if self.diagnostic_id.is_nil() {
            return Err(ValidationError::NilIdentifier("diagnostic_id"));
        }
        for detail in self.details.iter() {
            validate_identifier("error.detail.key", &detail.key)?;
            validate_human("error.detail.value", &detail.value)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundPolicy {
    CommandsOnly,
    ResponsesAndEvents,
}

/// Stateful validation for one ordered, reliable worker session.
pub struct SessionValidator {
    session_id: WireUuid,
    policy: InboundPolicy,
    next_inbound_sequence: u64,
    next_outbound_sequence: u64,
    inbound_message_ids: HashSet<WireUuid>,
    outbound_commands: HashMap<WireUuid, CommandName>,
    pending_replies: HashSet<WireUuid>,
}

impl SessionValidator {
    #[must_use]
    pub fn new(session_id: WireUuid, policy: InboundPolicy) -> Self {
        Self {
            session_id,
            policy,
            next_inbound_sequence: 1,
            next_outbound_sequence: 1,
            inbound_message_ids: HashSet::new(),
            outbound_commands: HashMap::new(),
            pending_replies: HashSet::new(),
        }
    }

    /// Register a Core command so a later acknowledgement/error can be
    /// correlated without accepting an arbitrary `reply_to`.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid/different-session envelope, a reused
    /// command ID, or a bounded session/pending-command limit.
    pub fn track_outbound_command(&mut self, envelope: &Envelope) -> Result<(), ValidationError> {
        envelope.validate_static()?;
        if envelope.session_id != self.session_id {
            return Err(ValidationError::SessionMismatch);
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

    /// Validate and commit the next inbound envelope in this ordered session.
    ///
    /// # Errors
    ///
    /// Returns an error for schema/session/sequence/direction violations or an
    /// acknowledgement, error, or event that cannot be correlated.
    pub fn validate_inbound(&mut self, envelope: &Envelope) -> Result<(), ValidationError> {
        envelope.validate_static()?;
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
            (InboundPolicy::ResponsesAndEvents, Message::Ack(reply)) => {
                self.validate_reply(reply.reply_to, reply.ack.name())?;
                Some(reply.reply_to)
            }
            (InboundPolicy::ResponsesAndEvents, Message::Error(reply)) => {
                self.validate_reply(reply.reply_to, reply.name)?;
                Some(reply.reply_to)
            }
            (InboundPolicy::ResponsesAndEvents, Message::Event(event)) => {
                if let Some(cause) = event.caused_by
                    && !self.outbound_commands.contains_key(&cause)
                {
                    return Err(ValidationError::UnknownCause);
                }
                None
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
        reply_to: WireUuid,
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
    pub fn has_pending_reply(&self, command_id: WireUuid) -> bool {
        self.pending_replies.contains(&command_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("unsupported worker protocol version {actual}")]
    UnsupportedProtocolVersion { actual: u16 },
    #[error("sequence must begin at one")]
    SequenceZero,
    #[error("{0} must not be nil")]
    NilIdentifier(&'static str),
    #[error("message belongs to a different session")]
    SessionMismatch,
    #[error("expected inbound sequence {expected}, received {actual}")]
    SequenceMismatch { expected: u64, actual: u64 },
    #[error("message ID was already used by this peer")]
    DuplicateMessageId,
    #[error("command ID was already used")]
    DuplicateCommandId,
    #[error("message kind is not valid in this direction")]
    UnexpectedMessageKind,
    #[error("an outbound command envelope was required")]
    ExpectedCommand,
    #[error("reply does not match a pending command")]
    UnknownReply,
    #[error("reply name mismatch: expected {expected:?}, received {actual:?}")]
    ReplyNameMismatch {
        expected: CommandName,
        actual: CommandName,
    },
    #[error("event cause is not a command from this session")]
    UnknownCause,
    #[error("too many commands are awaiting replies")]
    PendingCommandLimit,
    #[error("session reached its bounded message limit")]
    SessionMessageLimit,
    #[error("invalid {field}: {reason}")]
    InvalidField {
        field: &'static str,
        reason: &'static str,
    },
}

impl ValidationError {
    #[must_use]
    pub const fn stable_code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedProtocolVersion { .. } => ErrorCode::ProtocolUnsupportedVersion,
            Self::SessionMismatch => ErrorCode::ProtocolSessionMismatch,
            Self::SequenceZero | Self::SequenceMismatch { .. } => {
                ErrorCode::ProtocolSequenceInvalid
            }
            Self::DuplicateCommandId | Self::DuplicateMessageId => {
                ErrorCode::ProtocolDuplicateCommandId
            }
            Self::NilIdentifier(_)
            | Self::UnexpectedMessageKind
            | Self::ExpectedCommand
            | Self::UnknownReply
            | Self::ReplyNameMismatch { .. }
            | Self::UnknownCause
            | Self::PendingCommandLimit
            | Self::SessionMessageLimit
            | Self::InvalidField { .. } => ErrorCode::ProtocolSchemaInvalid,
        }
    }
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return invalid(field, "must contain 1..=128 ASCII bytes");
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte))
    {
        return invalid(field, "must be a lowercase ASCII token");
    }
    Ok(())
}

fn validate_human(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > MAX_HUMAN_STRING_BYTES || value.contains('\0') {
        return invalid(field, "must contain 1..=4096 UTF-8 bytes without NUL");
    }
    Ok(())
}

fn validate_path(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > MAX_PATH_BYTES || value.contains('\0') {
        return invalid(field, "must contain 1..=32768 UTF-8 bytes without NUL");
    }
    Ok(())
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(field, "must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

fn invalid<T>(field: &'static str, reason: &'static str) -> Result<T, ValidationError> {
    Err(ValidationError::InvalidField { field, reason })
}
