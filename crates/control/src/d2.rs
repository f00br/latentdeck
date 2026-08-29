use std::{fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use super::protocol::{BoundedVec, ValidationError, WireUuid};

/// Largest integer that can make a lossless round trip through JavaScript.
pub const MAX_D2_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

const MAX_D2_TEXT_BYTES: usize = 128;
const MAX_D2_PATH_BYTES: usize = 32_768;
const MAX_D2_PROVENANCE_BYTES: usize = 32_768;

/// A wire number whose representation is known not to be NaN or infinity.
///
/// The inner value is private so the `Eq` implementation remains sound.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct FiniteF64(f64);

impl FiniteF64 {
    #[must_use]
    pub fn new(value: f64) -> Option<Self> {
        value.is_finite().then_some(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> f64 {
        self.0
    }
}

impl Eq for FiniteF64 {}

impl Serialize for FiniteF64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.0)
    }
}

impl<'de> Deserialize<'de> for FiniteF64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FiniteVisitor(PhantomData<FiniteF64>);

        impl Visitor<'_> for FiniteVisitor {
            type Value = FiniteF64;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a finite number")
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                FiniteF64::new(value).ok_or_else(|| E::custom("number must be finite"))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                let parsed = value
                    .to_string()
                    .parse::<f64>()
                    .map_err(|_| E::custom("number must be representable as f64"))?;
                self.visit_f64(parsed)
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                let parsed = value
                    .to_string()
                    .parse::<f64>()
                    .map_err(|_| E::custom("number must be representable as f64"))?;
                self.visit_f64(parsed)
            }
        }

        deserializer.deserialize_any(FiniteVisitor(PhantomData))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2Algorithm {
    #[serde(rename = "LINEAR")]
    Linear,
    #[serde(rename = "XS1")]
    Xs1,
    #[serde(rename = "XS2")]
    Xs2,
    #[serde(rename = "XS3")]
    Xs3,
    #[serde(rename = "XS4")]
    Xs4,
    #[serde(rename = "XS5")]
    Xs5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2Mode {
    #[serde(rename = "HYBRIDIZE")]
    Hybridize,
    #[serde(rename = "INTERACT")]
    Interact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2Routing {
    A,
    B,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2Xs5Routing {
    #[serde(rename = "TOPK")]
    TopK,
    #[serde(rename = "SINKHORN")]
    Sinkhorn,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2Controls {
    pub algorithm: D2Algorithm,
    pub mix: FiniteF64,
    pub mode: D2Mode,
    pub routing: D2Routing,
    pub interaction: FiniteF64,
    pub preserve: FiniteF64,
    pub chaos: FiniteF64,
    pub xs1_channel_a: u8,
    pub xs1_channel_b: u8,
    pub xs1_angle_degrees: FiniteF64,
    pub xs2_radius: u8,
    pub xs3_high_gain: FiniteF64,
    pub xs4_epsilon: FiniteF64,
    pub xs5_routing: D2Xs5Routing,
    pub temperature: FiniteF64,
    pub top_k: u8,
    pub sinkhorn_iterations: u8,
}

impl Default for D2Controls {
    fn default() -> Self {
        Self {
            algorithm: D2Algorithm::Linear,
            mix: finite(0.5),
            mode: D2Mode::Hybridize,
            routing: D2Routing::A,
            interaction: finite(0.0),
            preserve: finite(0.55),
            chaos: finite(0.0),
            xs1_channel_a: 0,
            xs1_channel_b: 1,
            xs1_angle_degrees: finite(30.0),
            xs2_radius: 1,
            xs3_high_gain: finite(0.5),
            xs4_epsilon: finite(0.000_001),
            xs5_routing: D2Xs5Routing::TopK,
            temperature: finite(0.12),
            top_k: 8,
            sinkhorn_iterations: 5,
        }
    }
}

impl D2Controls {
    /// Validate all closed enums, finite bounds, and cross-field constraints.
    ///
    /// # Errors
    ///
    /// Returns a protocol validation error when a control is outside the
    /// exact LD-D2 0.1 contract.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_number("d2.controls.mix", self.mix, 0.0, 1.0)?;
        validate_number("d2.controls.interaction", self.interaction, 0.0, 1.0)?;
        validate_number("d2.controls.preserve", self.preserve, 0.0, 1.0)?;
        validate_number("d2.controls.chaos", self.chaos, 0.0, 1.0)?;
        if self.xs1_channel_a > 23 {
            return invalid("d2.controls.xs1_channel_a", "must be within 0..=23");
        }
        if self.xs1_channel_b > 23 {
            return invalid("d2.controls.xs1_channel_b", "must be within 0..=23");
        }
        if self.xs1_channel_a == self.xs1_channel_b {
            return invalid("d2.controls.xs1_channels", "channels must differ");
        }
        validate_number(
            "d2.controls.xs1_angle_degrees",
            self.xs1_angle_degrees,
            -180.0,
            180.0,
        )?;
        if !(1..=8).contains(&self.xs2_radius) {
            return invalid("d2.controls.xs2_radius", "must be within 1..=8");
        }
        validate_number("d2.controls.xs3_high_gain", self.xs3_high_gain, -2.0, 2.0)?;
        validate_number(
            "d2.controls.xs4_epsilon",
            self.xs4_epsilon,
            0.000_000_01,
            0.001,
        )?;
        validate_number("d2.controls.temperature", self.temperature, 0.02, 1.0)?;
        if !(1..=64).contains(&self.top_k) {
            return invalid("d2.controls.top_k", "must be within 1..=64");
        }
        if !(2..=12).contains(&self.sinkhorn_iterations) {
            return invalid("d2.controls.sinkhorn_iterations", "must be within 2..=12");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Exact four-flag Python wire schema.
pub struct D2Transport {
    pub playing_a: bool,
    pub playing_b: bool,
    pub loop_a: bool,
    pub loop_b: bool,
}

impl Default for D2Transport {
    fn default() -> Self {
        Self {
            playing_a: true,
            playing_b: true,
            loop_a: true,
            loop_b: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2SourceBinding {
    pub cartridge_path: String,
    pub cartridge_id: WireUuid,
    pub expected_archive_sha256: String,
}

impl D2SourceBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_path("d2.source.cartridge_path", &self.cartridge_path)?;
        validate_uuid("d2.source.cartridge_id", self.cartridge_id)?;
        validate_sha256(
            "d2.source.expected_archive_sha256",
            &self.expected_archive_sha256,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2Load {
    pub deck_id: String,
    pub operator_id: String,
    pub operator_version: String,
    pub source_a: D2SourceBinding,
    pub source_b: D2SourceBinding,
    pub controls: D2Controls,
    pub transport: D2Transport,
    pub seed: u64,
    pub stream_generation: u64,
}

impl D2Load {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_text("d2.deck_id", &self.deck_id, MAX_D2_TEXT_BYTES)?;
        validate_text("d2.operator_id", &self.operator_id, MAX_D2_TEXT_BYTES)?;
        validate_text(
            "d2.operator_version",
            &self.operator_version,
            MAX_D2_TEXT_BYTES,
        )?;
        self.source_a.validate()?;
        self.source_b.validate()?;
        self.controls.validate()?;
        validate_safe_integer("d2.seed", self.seed)?;
        validate_nonzero("d2.stream_generation", self.stream_generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2ProcessSlot {
    pub deck_id: String,
    pub deck_revision: u64,
    pub stream_generation: u64,
}

impl D2ProcessSlot {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_nonzero("d2.stream_generation", self.stream_generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2Reset {
    pub deck_id: String,
    pub deck_revision: u64,
    pub new_stream_generation: u64,
}

impl D2Reset {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_nonzero("d2.new_stream_generation", self.new_stream_generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2Restart {
    pub deck_id: String,
    pub deck_revision: u64,
}

impl D2Restart {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2ControlsSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub controls: D2Controls,
}

impl D2ControlsSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        self.controls.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2TransportSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub transport: D2Transport,
}

impl D2TransportSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2SeedSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub seed: u64,
}

impl D2SeedSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_safe_integer("d2.seed", self.seed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2SourceStatus {
    pub cartridge_id: WireUuid,
    pub archive_sha256: String,
    pub latent_slot_count: u64,
}

impl D2SourceStatus {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid("d2.source.cartridge_id", self.cartridge_id)?;
        validate_sha256("d2.source.archive_sha256", &self.archive_sha256)?;
        validate_nonzero("d2.source.latent_slot_count", self.latent_slot_count)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2ResetReason {
    #[serde(rename = "slot_a.loop")]
    SlotALoop,
    #[serde(rename = "slot_b.loop")]
    SlotBLoop,
    #[serde(rename = "transport.restart")]
    TransportRestart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2Status {
    pub deck_id: String,
    pub deck_revision: u64,
    pub operator_id: String,
    pub operator_version: String,
    pub stream_generation: u64,
    pub stream_sequence: u64,
    pub playhead_a: u64,
    pub playhead_b: u64,
    pub transport: D2Transport,
    pub controls: D2Controls,
    pub seed: u64,
    pub pending_reset: bool,
    pub pending_reset_reasons: BoundedVec<D2ResetReason, 2>,
    pub decoded_start_frame: u64,
    pub source_a: D2SourceStatus,
    pub source_b: D2SourceStatus,
}

impl D2Status {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_text("d2.operator_id", &self.operator_id, MAX_D2_TEXT_BYTES)?;
        validate_text(
            "d2.operator_version",
            &self.operator_version,
            MAX_D2_TEXT_BYTES,
        )?;
        validate_nonzero("d2.stream_generation", self.stream_generation)?;
        validate_safe_integer("d2.playhead_a", self.playhead_a)?;
        validate_safe_integer("d2.playhead_b", self.playhead_b)?;
        self.controls.validate()?;
        validate_safe_integer("d2.seed", self.seed)?;
        if self.pending_reset == self.pending_reset_reasons.is_empty() {
            return invalid(
                "d2.pending_reset_reasons",
                "must be nonempty exactly when pending_reset is true",
            );
        }
        self.source_a.validate()?;
        self.source_b.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum D2ProcessSlotAck {
    DecodedSlot {
        deck_id: String,
        deck_revision: u64,
        stream_generation: u64,
        stream_sequence: u64,
        playhead_a: u64,
        playhead_b: u64,
        transport: D2Transport,
        decoded_start_frame: u64,
        decoded_frame_count: u32,
        ring_first_sequence: u64,
        ring_last_sequence_exclusive: u64,
        provenance_json: String,
    },
    ResetBarrier {
        deck_id: String,
        deck_revision: u64,
        current_generation: u64,
        minimum_new_generation: u64,
        reasons: BoundedVec<D2ResetReason, 2>,
    },
    Paused {
        deck_id: String,
        deck_revision: u64,
        stream_generation: u64,
        playhead_a: u64,
        playhead_b: u64,
        transport: D2Transport,
    },
}

impl D2ProcessSlotAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::DecodedSlot {
                deck_id,
                deck_revision,
                stream_generation,
                stream_sequence,
                playhead_a,
                playhead_b,
                decoded_start_frame,
                decoded_frame_count,
                ring_first_sequence,
                ring_last_sequence_exclusive,
                provenance_json,
                ..
            } => {
                validate_identity(deck_id, *deck_revision)?;
                validate_nonzero("d2.stream_generation", *stream_generation)?;
                validate_safe_integer("d2.stream_sequence", *stream_sequence)?;
                validate_safe_integer("d2.playhead_a", *playhead_a)?;
                validate_safe_integer("d2.playhead_b", *playhead_b)?;
                validate_safe_integer("d2.decoded_start_frame", *decoded_start_frame)?;
                if !(1..=4).contains(decoded_frame_count) {
                    return invalid("d2.decoded_frame_count", "must contain one to four frames");
                }
                validate_ring_range(
                    *ring_first_sequence,
                    *ring_last_sequence_exclusive,
                    *decoded_frame_count,
                )?;
                validate_provenance_json(provenance_json)
            }
            Self::ResetBarrier {
                deck_id,
                deck_revision,
                current_generation,
                minimum_new_generation,
                reasons,
            } => validate_reset_barrier(
                deck_id,
                *deck_revision,
                *current_generation,
                *minimum_new_generation,
                reasons,
            ),
            Self::Paused {
                deck_id,
                deck_revision,
                stream_generation,
                playhead_a,
                playhead_b,
                transport: _,
            } => {
                validate_identity(deck_id, *deck_revision)?;
                validate_nonzero("d2.stream_generation", *stream_generation)?;
                validate_safe_integer("d2.playhead_a", *playhead_a)?;
                validate_safe_integer("d2.playhead_b", *playhead_b)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2ResetAppliedKind {
    #[serde(rename = "reset_applied")]
    ResetApplied,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2ResetAck {
    pub kind: D2ResetAppliedKind,
    pub deck_id: String,
    pub deck_revision: u64,
    pub stream_generation: u64,
    pub playhead_a: u64,
    pub playhead_b: u64,
    pub reasons: BoundedVec<D2ResetReason, 2>,
    pub causal_state_cleared: bool,
}

impl D2ResetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_nonzero("d2.stream_generation", self.stream_generation)?;
        validate_safe_integer("d2.playhead_a", self.playhead_a)?;
        validate_safe_integer("d2.playhead_b", self.playhead_b)?;
        validate_reasons(&self.reasons)?;
        if !self.causal_state_cleared {
            return invalid("d2.causal_state_cleared", "must be true after reset");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2ResetBarrierKind {
    #[serde(rename = "reset_barrier")]
    ResetBarrier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2RestartAck {
    pub kind: D2ResetBarrierKind,
    pub deck_id: String,
    pub deck_revision: u64,
    pub current_generation: u64,
    pub minimum_new_generation: u64,
    pub reasons: BoundedVec<D2ResetReason, 2>,
}

impl D2RestartAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_reset_barrier(
            &self.deck_id,
            self.deck_revision,
            self.current_generation,
            self.minimum_new_generation,
            &self.reasons,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2ControlsSetAck {
    pub deck_id: String,
    pub deck_revision: u64,
    pub controls: D2Controls,
    pub requires_causal_reset: bool,
}

impl D2ControlsSetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        self.controls.validate()?;
        validate_no_reset(self.requires_causal_reset)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2TransportSetAck {
    pub deck_id: String,
    pub deck_revision: u64,
    pub transport: D2Transport,
    pub requires_causal_reset: bool,
}

impl D2TransportSetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_no_reset(self.requires_causal_reset)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2SeedSetAck {
    pub deck_id: String,
    pub deck_revision: u64,
    pub seed: u64,
    pub requires_causal_reset: bool,
}

impl D2SeedSetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_safe_integer("d2.seed", self.seed)?;
        validate_no_reset(self.requires_causal_reset)
    }
}

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("first-party D2 defaults are finite")
}

pub(crate) fn validate_identity(deck_id: &str, deck_revision: u64) -> Result<(), ValidationError> {
    validate_text("d2.deck_id", deck_id, MAX_D2_TEXT_BYTES)?;
    validate_nonzero("d2.deck_revision", deck_revision)
}

fn validate_number(
    field: &'static str,
    value: FiniteF64,
    minimum: f64,
    maximum: f64,
) -> Result<(), ValidationError> {
    if !(minimum..=maximum).contains(&value.get()) {
        return invalid(field, "is outside its finite bound");
    }
    Ok(())
}

pub(crate) fn validate_text(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        return invalid(field, "must be bounded nonempty UTF-8 text without NUL");
    }
    Ok(())
}

fn validate_provenance_json(value: &str) -> Result<(), ValidationError> {
    validate_text("d2.provenance_json", value, MAX_D2_PROVENANCE_BYTES)?;
    let parsed = serde_json::from_str::<serde_json::Value>(value).map_err(|_| {
        ValidationError::InvalidField {
            field: "d2.provenance_json",
            reason: "must be valid JSON",
        }
    })?;
    if !parsed.is_object() {
        return invalid("d2.provenance_json", "must be a JSON object");
    }
    Ok(())
}

pub(crate) fn validate_path(field: &'static str, value: &str) -> Result<(), ValidationError> {
    validate_text(field, value, MAX_D2_PATH_BYTES)
}

pub(crate) fn validate_uuid(field: &'static str, value: WireUuid) -> Result<(), ValidationError> {
    if value.is_nil() {
        return invalid(field, "must be a canonical non-nil UUID");
    }
    Ok(())
}

pub(crate) fn validate_sha256(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid(field, "must be 64 lowercase hexadecimal characters");
    }
    Ok(())
}

pub(crate) fn validate_nonzero(field: &'static str, value: u64) -> Result<(), ValidationError> {
    if value == 0 {
        return invalid(field, "must be a nonzero u64");
    }
    Ok(())
}

pub(crate) fn validate_safe_integer(
    field: &'static str,
    value: u64,
) -> Result<(), ValidationError> {
    if value > MAX_D2_SAFE_INTEGER {
        return invalid(field, "must be within the exact u53 range");
    }
    Ok(())
}

fn validate_reasons(reasons: &BoundedVec<D2ResetReason, 2>) -> Result<(), ValidationError> {
    if reasons.is_empty() {
        return invalid("d2.reset.reasons", "must contain one or two reasons");
    }
    Ok(())
}

fn validate_reset_barrier(
    deck_id: &str,
    deck_revision: u64,
    current_generation: u64,
    minimum_new_generation: u64,
    reasons: &BoundedVec<D2ResetReason, 2>,
) -> Result<(), ValidationError> {
    validate_identity(deck_id, deck_revision)?;
    validate_nonzero("d2.current_generation", current_generation)?;
    if minimum_new_generation <= current_generation {
        return invalid(
            "d2.minimum_new_generation",
            "must be greater than current_generation",
        );
    }
    validate_reasons(reasons)
}

fn validate_ring_range(
    first: u64,
    last_exclusive: u64,
    decoded_frame_count: u32,
) -> Result<(), ValidationError> {
    if first == 0 || first.checked_add(u64::from(decoded_frame_count)) != Some(last_exclusive) {
        return invalid(
            "d2.ring_sequence_range",
            "must exactly contain the decoded frame count",
        );
    }
    Ok(())
}

fn validate_no_reset(requires_causal_reset: bool) -> Result<(), ValidationError> {
    if requires_causal_reset {
        return invalid(
            "d2.requires_causal_reset",
            "realtime control updates must not request a reset",
        );
    }
    Ok(())
}

pub(crate) fn invalid<T>(field: &'static str, reason: &'static str) -> Result<T, ValidationError> {
    Err(ValidationError::InvalidField { field, reason })
}
