//! Closed Worker Protocol 1 schema for deterministic LD-Q4 synthesis.
//!
//! Physical slots remain `A` through `D`. [`Q4Roles`] independently assigns
//! one slot as structural carrier and the other three to the stable donor
//! roles `B`, `C`, and `D`; every role set is therefore an exact permutation.

use serde::{Deserialize, Serialize};

use super::{
    d2::{
        FiniteF64, invalid, validate_nonzero, validate_path, validate_safe_integer,
        validate_sha256, validate_text, validate_uuid,
    },
    protocol::{BoundedVec, ValidationError, WireUuid},
};

pub const MAX_Q4_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_Q4_CAPTURE_LATENT_SLOTS: u64 = 1_048_576;
pub const MAX_Q4_CAPTURE_VISUAL_BYTES: u64 = 15 * 1024 * 1024 * 1024;
pub const MAX_Q4_CAPTURE_CONTROL_EVENTS: usize = 32;

const MAX_Q4_TEXT_BYTES: usize = 128;
const MAX_Q4_PROVENANCE_BYTES: usize = 32_768;
const MAX_Q4_CAPTURE_RECEIPT_BYTES: usize = 32_768;
const MAX_Q4_CAPTURE_LATENT_AXIS: u64 = 256;
const MAX_Q4_CAPTURE_REASON_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4Algorithm {
    #[serde(rename = "LINEAR")]
    Linear,
    #[serde(rename = "XS5")]
    Xs5,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4Mode {
    #[serde(rename = "HYBRIDIZE")]
    Hybridize,
    #[serde(rename = "INTERACT")]
    Interact,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4InfluenceMode {
    #[serde(rename = "MANUAL")]
    Manual,
    #[serde(rename = "TRIANGLE")]
    Triangle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4Xs5Routing {
    #[serde(rename = "TOPK")]
    TopK,
    #[serde(rename = "SINKHORN")]
    Sinkhorn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Q4Slot {
    A,
    B,
    C,
    D,
}

impl Q4Slot {
    const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
            Self::C => 2,
            Self::D => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4Roles {
    pub carrier: Q4Slot,
    pub donor_b: Q4Slot,
    pub donor_c: Q4Slot,
    pub donor_d: Q4Slot,
}

impl Default for Q4Roles {
    fn default() -> Self {
        Self {
            carrier: Q4Slot::A,
            donor_b: Q4Slot::B,
            donor_c: Q4Slot::C,
            donor_d: Q4Slot::D,
        }
    }
}

impl Q4Roles {
    /// Reject aliases and omissions: the four roles must cover `A..D` once.
    ///
    /// # Errors
    ///
    /// Returns a schema error when any physical slot is repeated or omitted.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let mut seen = [false; 4];
        for slot in [self.carrier, self.donor_b, self.donor_c, self.donor_d] {
            let index = slot.index();
            if seen[index] {
                return invalid(
                    "q4.roles",
                    "carrier and donor roles must be an exact A/B/C/D permutation",
                );
            }
            seen[index] = true;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4Controls {
    pub algorithm: Q4Algorithm,
    pub interaction: FiniteF64,
    pub mode: Q4Mode,
    pub preserve: FiniteF64,
    pub influence_mode: Q4InfluenceMode,
    pub donor_weight_b: FiniteF64,
    pub donor_weight_c: FiniteF64,
    pub donor_weight_d: FiniteF64,
    pub triangle_x: FiniteF64,
    pub triangle_y: FiniteF64,
    pub xs5_routing: Q4Xs5Routing,
    pub temperature: FiniteF64,
    pub top_k: u8,
    pub sinkhorn_iterations: u8,
    pub chaos: FiniteF64,
}

impl Default for Q4Controls {
    fn default() -> Self {
        Self {
            algorithm: Q4Algorithm::Linear,
            interaction: finite(0.0),
            mode: Q4Mode::Hybridize,
            preserve: finite(0.55),
            influence_mode: Q4InfluenceMode::Manual,
            donor_weight_b: finite(1.0),
            donor_weight_c: finite(1.0),
            donor_weight_d: finite(1.0),
            triangle_x: finite(0.5),
            triangle_y: finite(1.0 / 3.0),
            xs5_routing: Q4Xs5Routing::TopK,
            temperature: finite(0.12),
            top_k: 8,
            sinkhorn_iterations: 5,
            chaos: finite(0.0),
        }
    }
}

impl Q4Controls {
    /// Validate the complete closed Q4 control block without clamping.
    ///
    /// # Errors
    ///
    /// Returns a schema error for non-finite or out-of-range controls, an
    /// empty manual donor distribution, or a point outside the triangle.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_number("q4.controls.interaction", self.interaction, 0.0, 1.0)?;
        validate_number("q4.controls.preserve", self.preserve, 0.0, 1.0)?;
        validate_number("q4.controls.chaos", self.chaos, 0.0, 1.0)?;
        for (field, value) in [
            ("q4.controls.donor_weight_b", self.donor_weight_b),
            ("q4.controls.donor_weight_c", self.donor_weight_c),
            ("q4.controls.donor_weight_d", self.donor_weight_d),
            ("q4.controls.triangle_x", self.triangle_x),
            ("q4.controls.triangle_y", self.triangle_y),
        ] {
            validate_number(field, value, 0.0, 1.0)?;
        }
        validate_number("q4.controls.temperature", self.temperature, 0.02, 1.0)?;
        if !(1..=64).contains(&self.top_k) {
            return invalid("q4.controls.top_k", "must be within 1..=64");
        }
        if !(2..=12).contains(&self.sinkhorn_iterations) {
            return invalid("q4.controls.sinkhorn_iterations", "must be within 2..=12");
        }
        match self.influence_mode {
            Q4InfluenceMode::Manual => {
                if self.donor_weight_b.get() + self.donor_weight_c.get() + self.donor_weight_d.get()
                    == 0.0
                {
                    return invalid(
                        "q4.controls.donor_weights",
                        "at least one manual donor weight must be positive",
                    );
                }
            }
            Q4InfluenceMode::Triangle => {
                let x = self.triangle_x.get();
                let y = self.triangle_y.get();
                let minimum = (1.0 - x - 0.5 * y).min(x - 0.5 * y).min(y);
                if minimum < -1e-12 {
                    return invalid(
                        "q4.controls.triangle",
                        "point must lie inside the B/C/D influence triangle",
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Exact eight-flag transport schema.
pub struct Q4Transport {
    pub playing_a: bool,
    pub playing_b: bool,
    pub playing_c: bool,
    pub playing_d: bool,
    pub loop_a: bool,
    pub loop_b: bool,
    pub loop_c: bool,
    pub loop_d: bool,
}

impl Default for Q4Transport {
    fn default() -> Self {
        Self {
            playing_a: true,
            playing_b: true,
            playing_c: true,
            playing_d: true,
            loop_a: true,
            loop_b: true,
            loop_c: true,
            loop_d: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4SourceBinding {
    /// Worker-private, host-resolved path. This field is never a webview input.
    pub cartridge_path: String,
    pub cartridge_id: WireUuid,
    pub expected_archive_sha256: String,
}

impl Q4SourceBinding {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_path("q4.source.cartridge_path", &self.cartridge_path)?;
        validate_uuid("q4.source.cartridge_id", self.cartridge_id)?;
        validate_sha256(
            "q4.source.expected_archive_sha256",
            &self.expected_archive_sha256,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4Load {
    pub deck_id: String,
    pub operator_id: String,
    pub operator_version: String,
    pub source_a: Q4SourceBinding,
    pub source_b: Q4SourceBinding,
    pub source_c: Q4SourceBinding,
    pub source_d: Q4SourceBinding,
    pub roles: Q4Roles,
    pub controls: Q4Controls,
    pub transport: Q4Transport,
    pub seed: u64,
    pub stream_generation: u64,
}

impl Q4Load {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_text("q4.deck_id", &self.deck_id, MAX_Q4_TEXT_BYTES)?;
        validate_text("q4.operator_id", &self.operator_id, MAX_Q4_TEXT_BYTES)?;
        validate_text(
            "q4.operator_version",
            &self.operator_version,
            MAX_Q4_TEXT_BYTES,
        )?;
        self.source_a.validate()?;
        self.source_b.validate()?;
        self.source_c.validate()?;
        self.source_d.validate()?;
        self.roles.validate()?;
        self.controls.validate()?;
        validate_safe_integer("q4.seed", self.seed)?;
        validate_nonzero("q4.stream_generation", self.stream_generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4ProcessSlot {
    pub deck_id: String,
    pub deck_revision: u64,
    pub stream_generation: u64,
}

impl Q4ProcessSlot {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_nonzero("q4.stream_generation", self.stream_generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4Reset {
    pub deck_id: String,
    pub deck_revision: u64,
    pub new_stream_generation: u64,
}

impl Q4Reset {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_nonzero("q4.new_stream_generation", self.new_stream_generation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4Restart {
    pub deck_id: String,
    pub deck_revision: u64,
}

impl Q4Restart {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4ControlsSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub controls: Q4Controls,
}

impl Q4ControlsSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        self.controls.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4RolesSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub roles: Q4Roles,
}

impl Q4RolesSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        self.roles.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4TransportSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub transport: Q4Transport,
}

impl Q4TransportSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4SeedSet {
    pub deck_id: String,
    pub deck_revision: u64,
    pub seed: u64,
}

impl Q4SeedSet {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_safe_integer("q4.seed", self.seed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4SourceStatus {
    pub cartridge_id: WireUuid,
    pub archive_sha256: String,
    pub latent_slot_count: u64,
}

impl Q4SourceStatus {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid("q4.source.cartridge_id", self.cartridge_id)?;
        validate_sha256("q4.source.archive_sha256", &self.archive_sha256)?;
        validate_nonzero("q4.source.latent_slot_count", self.latent_slot_count)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4ResetReason {
    #[serde(rename = "slot_a.loop")]
    SlotALoop,
    #[serde(rename = "slot_b.loop")]
    SlotBLoop,
    #[serde(rename = "slot_c.loop")]
    SlotCLoop,
    #[serde(rename = "slot_d.loop")]
    SlotDLoop,
    #[serde(rename = "transport.restart")]
    TransportRestart,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4Status {
    pub deck_id: String,
    pub deck_revision: u64,
    pub operator_id: String,
    pub operator_version: String,
    pub stream_generation: u64,
    pub stream_sequence: u64,
    pub playhead_a: u64,
    pub playhead_b: u64,
    pub playhead_c: u64,
    pub playhead_d: u64,
    pub roles: Q4Roles,
    pub transport: Q4Transport,
    pub controls: Q4Controls,
    pub seed: u64,
    pub pending_reset: bool,
    pub pending_reset_reasons: BoundedVec<Q4ResetReason, 5>,
    pub decoded_start_frame: u64,
    pub source_a: Q4SourceStatus,
    pub source_b: Q4SourceStatus,
    pub source_c: Q4SourceStatus,
    pub source_d: Q4SourceStatus,
}

impl Q4Status {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_text("q4.operator_id", &self.operator_id, MAX_Q4_TEXT_BYTES)?;
        validate_text(
            "q4.operator_version",
            &self.operator_version,
            MAX_Q4_TEXT_BYTES,
        )?;
        validate_nonzero("q4.stream_generation", self.stream_generation)?;
        for (field, value) in [
            ("q4.playhead_a", self.playhead_a),
            ("q4.playhead_b", self.playhead_b),
            ("q4.playhead_c", self.playhead_c),
            ("q4.playhead_d", self.playhead_d),
        ] {
            validate_safe_integer(field, value)?;
        }
        self.roles.validate()?;
        self.controls.validate()?;
        validate_safe_integer("q4.seed", self.seed)?;
        if self.pending_reset == self.pending_reset_reasons.is_empty() {
            return invalid(
                "q4.pending_reset_reasons",
                "must be nonempty exactly when pending_reset is true",
            );
        }
        self.source_a.validate()?;
        self.source_b.validate()?;
        self.source_c.validate()?;
        self.source_d.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Q4ProcessSlotAck {
    DecodedSlot {
        deck_id: String,
        deck_revision: u64,
        stream_generation: u64,
        stream_sequence: u64,
        playhead_a: u64,
        playhead_b: u64,
        playhead_c: u64,
        playhead_d: u64,
        roles: Q4Roles,
        transport: Q4Transport,
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
        reasons: BoundedVec<Q4ResetReason, 5>,
    },
    Paused {
        deck_id: String,
        deck_revision: u64,
        stream_generation: u64,
        playhead_a: u64,
        playhead_b: u64,
        playhead_c: u64,
        playhead_d: u64,
        roles: Q4Roles,
        transport: Q4Transport,
    },
}

impl Q4ProcessSlotAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::DecodedSlot {
                deck_id,
                deck_revision,
                stream_generation,
                stream_sequence,
                playhead_a,
                playhead_b,
                playhead_c,
                playhead_d,
                roles,
                decoded_start_frame,
                decoded_frame_count,
                ring_first_sequence,
                ring_last_sequence_exclusive,
                provenance_json,
                ..
            } => {
                validate_identity(deck_id, *deck_revision)?;
                validate_nonzero("q4.stream_generation", *stream_generation)?;
                for (field, value) in [
                    ("q4.stream_sequence", *stream_sequence),
                    ("q4.playhead_a", *playhead_a),
                    ("q4.playhead_b", *playhead_b),
                    ("q4.playhead_c", *playhead_c),
                    ("q4.playhead_d", *playhead_d),
                    ("q4.decoded_start_frame", *decoded_start_frame),
                ] {
                    validate_safe_integer(field, value)?;
                }
                roles.validate()?;
                if !(1..=4).contains(decoded_frame_count) {
                    return invalid("q4.decoded_frame_count", "must contain one to four frames");
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
                playhead_c,
                playhead_d,
                roles,
                ..
            } => {
                validate_identity(deck_id, *deck_revision)?;
                validate_nonzero("q4.stream_generation", *stream_generation)?;
                for (field, value) in [
                    ("q4.playhead_a", *playhead_a),
                    ("q4.playhead_b", *playhead_b),
                    ("q4.playhead_c", *playhead_c),
                    ("q4.playhead_d", *playhead_d),
                ] {
                    validate_safe_integer(field, value)?;
                }
                roles.validate()
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4ResetAppliedKind {
    #[serde(rename = "reset_applied")]
    ResetApplied,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4ResetAck {
    pub kind: Q4ResetAppliedKind,
    pub deck_id: String,
    pub deck_revision: u64,
    pub stream_generation: u64,
    pub playhead_a: u64,
    pub playhead_b: u64,
    pub playhead_c: u64,
    pub playhead_d: u64,
    pub reasons: BoundedVec<Q4ResetReason, 5>,
    pub causal_state_cleared: bool,
}

impl Q4ResetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_nonzero("q4.stream_generation", self.stream_generation)?;
        for (field, value) in [
            ("q4.playhead_a", self.playhead_a),
            ("q4.playhead_b", self.playhead_b),
            ("q4.playhead_c", self.playhead_c),
            ("q4.playhead_d", self.playhead_d),
        ] {
            validate_safe_integer(field, value)?;
        }
        validate_reasons(&self.reasons)?;
        if !self.causal_state_cleared {
            return invalid("q4.causal_state_cleared", "must be true after reset");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4ResetBarrierKind {
    #[serde(rename = "reset_barrier")]
    ResetBarrier,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4RestartAck {
    pub kind: Q4ResetBarrierKind,
    pub deck_id: String,
    pub deck_revision: u64,
    pub current_generation: u64,
    pub minimum_new_generation: u64,
    pub reasons: BoundedVec<Q4ResetReason, 5>,
}

impl Q4RestartAck {
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

macro_rules! q4_realtime_ack {
    ($name:ident, $field:ident, $value:ty, $validate:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            pub deck_id: String,
            pub deck_revision: u64,
            pub $field: $value,
            pub requires_causal_reset: bool,
        }

        impl $name {
            pub(crate) fn validate(&self) -> Result<(), ValidationError> {
                validate_identity(&self.deck_id, self.deck_revision)?;
                ($validate)(&self.$field)?;
                validate_no_reset(self.requires_causal_reset)
            }
        }
    };
}

q4_realtime_ack!(Q4ControlsSetAck, controls, Q4Controls, Q4Controls::validate);
q4_realtime_ack!(Q4RolesSetAck, roles, Q4Roles, Q4Roles::validate);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4TransportSetAck {
    pub deck_id: String,
    pub deck_revision: u64,
    pub transport: Q4Transport,
    pub requires_causal_reset: bool,
}

impl Q4TransportSetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_no_reset(self.requires_causal_reset)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4SeedSetAck {
    pub deck_id: String,
    pub deck_revision: u64,
    pub seed: u64,
    pub requires_causal_reset: bool,
}

impl Q4SeedSetAck {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_identity(&self.deck_id, self.deck_revision)?;
        validate_safe_integer("q4.seed", self.seed)?;
        validate_no_reset(self.requires_causal_reset)
    }
}

// Capture types follow below; their receipts are worker-private control data.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Q4CaptureMode {
    Snapshot,
    LiveCapture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Q4CaptureState {
    AwaitingReset,
    Capturing,
    StopArmed,
    Finished,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureStart {
    pub deck_id: String,
    pub deck_revision: u64,
    pub capture_id: WireUuid,
    pub mode: Q4CaptureMode,
    pub temporary_root: String,
    pub max_latent_slots: u64,
    pub max_visual_bytes: u64,
}

impl Q4CaptureStart {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_capture_identity(&self.deck_id, self.deck_revision, self.capture_id)?;
        validate_path("q4.capture.temporary_root", &self.temporary_root)?;
        if !(2..=MAX_Q4_CAPTURE_LATENT_SLOTS).contains(&self.max_latent_slots) {
            return invalid("q4.capture.max_latent_slots", "must be within 2..=1048576");
        }
        if !(1..=MAX_Q4_CAPTURE_VISUAL_BYTES).contains(&self.max_visual_bytes) {
            return invalid(
                "q4.capture.max_visual_bytes",
                "must be within the 15 GiB H3 payload limit",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureStop {
    pub deck_id: String,
    pub deck_revision: u64,
    pub capture_id: WireUuid,
}

impl Q4CaptureStop {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_capture_identity(&self.deck_id, self.deck_revision, self.capture_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureStatusRequest {
    pub deck_id: String,
    pub deck_revision: u64,
    pub capture_id: WireUuid,
}

impl Q4CaptureStatusRequest {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_capture_identity(&self.deck_id, self.deck_revision, self.capture_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4CaptureVisualDtype {
    #[serde(rename = "F16")]
    F16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Q4CaptureAudioDtype {
    #[serde(rename = "F16")]
    F16,
    #[serde(rename = "F32")]
    F32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Q4CaptureAudioPolicy {
    SourceAbsent,
    CopiedFromCarrierExact,
    OmittedTimingMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Q4CaptureAudioPolicyReason {
    DurationAndMappingMismatch,
    DurationMismatch,
    TemporalMappingMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureAudioDescriptor {
    pub storage_dtype: Q4CaptureAudioDtype,
    pub shape: [u64; 4],
    pub byte_length: u64,
}

impl Q4CaptureAudioDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        let [batch, channels, stereo, temporal] = self.shape;
        if batch != 1
            || channels != 32
            || stereo != 2
            || temporal == 0
            || temporal > MAX_Q4_CAPTURE_LATENT_SLOTS
        {
            return invalid(
                "q4.capture.audio_descriptor.shape",
                "must be [1,32,2,T] within the H3 temporal limit",
            );
        }
        let element_bytes = match self.storage_dtype {
            Q4CaptureAudioDtype::F16 => 2,
            Q4CaptureAudioDtype::F32 => 4,
        };
        let expected = 32_u64
            .checked_mul(2)
            .and_then(|value| value.checked_mul(temporal))
            .and_then(|value| value.checked_mul(element_bytes))
            .ok_or(ValidationError::InvalidField {
                field: "q4.capture.audio_descriptor.byte_length",
                reason: "audio descriptor size overflows",
            })?;
        if self.byte_length != expected {
            return invalid(
                "q4.capture.audio_descriptor.byte_length",
                "does not match dtype and shape",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureParent {
    pub slot: Q4Slot,
    pub cartridge_id: WireUuid,
    pub archive_sha256: String,
}

impl Q4CaptureParent {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid("q4.capture.parent.cartridge_id", self.cartridge_id)?;
        validate_sha256("q4.capture.parent.archive_sha256", &self.archive_sha256)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureControlEvent {
    pub slot_offset: u64,
    pub roles: Q4Roles,
    pub controls: Q4Controls,
    pub seed: u64,
}

impl Q4CaptureControlEvent {
    fn validate(&self, latent_slots: u64) -> Result<(), ValidationError> {
        if self.slot_offset >= latent_slots {
            return invalid(
                "q4.capture.control_event.slot_offset",
                "must identify a captured latent-slot boundary",
            );
        }
        self.roles.validate()?;
        self.controls.validate()?;
        validate_safe_integer("q4.capture.seed", self.seed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureReceipt {
    pub capture_id: WireUuid,
    pub mode: Q4CaptureMode,
    pub payload_path: String,
    pub payload_sha256: String,
    pub payload_bytes: u64,
    pub storage_dtype: Q4CaptureVisualDtype,
    pub visual_shape: [u64; 5],
    pub decoded_frame_count: u64,
    pub audio_policy: Q4CaptureAudioPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_policy_reason: Option<Q4CaptureAudioPolicyReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_descriptor: Option<Q4CaptureAudioDescriptor>,
    pub structural_carrier: Q4Slot,
    pub parents: [Q4CaptureParent; 4],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_roles: Option<Q4Roles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_controls: Option<Q4Controls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_events: Option<BoundedVec<Q4CaptureControlEvent, MAX_Q4_CAPTURE_CONTROL_EVENTS>>,
}

impl Q4CaptureReceipt {
    fn validate(&self) -> Result<(), ValidationError> {
        let (temporal, visual_bytes, expected_frames) = self.validate_payload_descriptor()?;
        let audio_bytes = validate_audio_policy(self, expected_frames)?;
        let minimum_payload_bytes = visual_bytes
            .checked_add(audio_bytes)
            .and_then(|value| value.checked_add(8))
            .ok_or(ValidationError::InvalidField {
                field: "q4.capture.receipt.payload_bytes",
                reason: "payload size overflows",
            })?;
        if self.payload_bytes <= minimum_payload_bytes {
            return invalid(
                "q4.capture.receipt.payload_bytes",
                "must include a bounded Safetensors header and declared tensors",
            );
        }
        self.validate_parents()?;
        self.validate_mode_provenance(temporal)?;
        validate_json_size(self, "q4.capture.receipt")
    }

    fn validate_payload_descriptor(&self) -> Result<(u64, u64, u64), ValidationError> {
        validate_uuid("q4.capture.receipt.capture_id", self.capture_id)?;
        validate_path("q4.capture.receipt.payload_path", &self.payload_path)?;
        validate_capture_payload_name(&self.payload_path, self.capture_id)?;
        validate_sha256("q4.capture.receipt.payload_sha256", &self.payload_sha256)?;
        if self.payload_bytes == 0 || self.payload_bytes > MAX_Q4_CAPTURE_VISUAL_BYTES {
            return invalid(
                "q4.capture.receipt.payload_bytes",
                "must fit the nonzero 15 GiB H3 payload limit",
            );
        }
        let [batch, channels, temporal, height, width] = self.visual_shape;
        if batch != 1
            || channels != 24
            || !is_codec_valid_slots(temporal)
            || !(1..=MAX_Q4_CAPTURE_LATENT_AXIS).contains(&height)
            || !(1..=MAX_Q4_CAPTURE_LATENT_AXIS).contains(&width)
        {
            return invalid(
                "q4.capture.receipt.visual_shape",
                "must be codec-valid [1,24,T,H,W]",
            );
        }
        let visual_bytes = 24_u64
            .checked_mul(temporal)
            .and_then(|value| value.checked_mul(height))
            .and_then(|value| value.checked_mul(width))
            .and_then(|value| value.checked_mul(2))
            .ok_or(ValidationError::InvalidField {
                field: "q4.capture.receipt.visual_shape",
                reason: "visual tensor size overflows",
            })?;
        let expected_frames = decoded_frames(temporal)?;
        if self.decoded_frame_count != expected_frames {
            return invalid(
                "q4.capture.receipt.decoded_frame_count",
                "does not match the H3 capture cadence",
            );
        }
        Ok((temporal, visual_bytes, expected_frames))
    }

    fn validate_parents(&self) -> Result<(), ValidationError> {
        for (expected, parent) in [Q4Slot::A, Q4Slot::B, Q4Slot::C, Q4Slot::D]
            .into_iter()
            .zip(&self.parents)
        {
            if parent.slot != expected {
                return invalid(
                    "q4.capture.receipt.parents",
                    "must contain ordered A, B, C, and D source identities",
                );
            }
            parent.validate()?;
        }
        Ok(())
    }

    fn validate_mode_provenance(&self, temporal: u64) -> Result<(), ValidationError> {
        match self.mode {
            Q4CaptureMode::Snapshot => {
                let seed = self.frozen_seed.ok_or(ValidationError::InvalidField {
                    field: "q4.capture.receipt.frozen_seed",
                    reason: "is required for snapshot capture",
                })?;
                let roles = self
                    .frozen_roles
                    .as_ref()
                    .ok_or(ValidationError::InvalidField {
                        field: "q4.capture.receipt.frozen_roles",
                        reason: "is required for snapshot capture",
                    })?;
                let controls =
                    self.frozen_controls
                        .as_ref()
                        .ok_or(ValidationError::InvalidField {
                            field: "q4.capture.receipt.frozen_controls",
                            reason: "is required for snapshot capture",
                        })?;
                if self.control_events.is_some() {
                    return invalid(
                        "q4.capture.receipt.control_events",
                        "is forbidden for snapshot capture",
                    );
                }
                validate_safe_integer("q4.capture.seed", seed)?;
                roles.validate()?;
                controls.validate()?;
                if roles.carrier != self.structural_carrier {
                    return invalid(
                        "q4.capture.receipt.structural_carrier",
                        "must match frozen snapshot roles",
                    );
                }
                if self.audio_policy == Q4CaptureAudioPolicy::OmittedTimingMismatch {
                    return invalid(
                        "q4.capture.receipt.audio_policy",
                        "snapshot timing cannot be omitted",
                    );
                }
                Ok(())
            }
            Q4CaptureMode::LiveCapture => {
                if self.frozen_seed.is_some()
                    || self.frozen_roles.is_some()
                    || self.frozen_controls.is_some()
                {
                    return invalid(
                        "q4.capture.receipt.frozen_controls",
                        "frozen snapshot fields are forbidden for live capture",
                    );
                }
                let events = self
                    .control_events
                    .as_ref()
                    .ok_or(ValidationError::InvalidField {
                        field: "q4.capture.receipt.control_events",
                        reason: "is required for live capture",
                    })?;
                if events.is_empty() || events[0].slot_offset != 0 {
                    return invalid(
                        "q4.capture.receipt.control_events",
                        "must begin with the initial state at slot offset zero",
                    );
                }
                let mut previous_offset = 0;
                for event in events.iter() {
                    event.validate(temporal)?;
                    if event.slot_offset < previous_offset {
                        return invalid(
                            "q4.capture.receipt.control_events",
                            "slot offsets must be nondecreasing",
                        );
                    }
                    previous_offset = event.slot_offset;
                }
                if events[0].roles.carrier != self.structural_carrier {
                    return invalid(
                        "q4.capture.receipt.structural_carrier",
                        "must match the initial live roles",
                    );
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4CaptureStatus {
    pub capture_id: WireUuid,
    pub mode: Q4CaptureMode,
    pub state: Q4CaptureState,
    pub structural_carrier: Q4Slot,
    pub latent_slots: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_new_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_latent_slots: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finalize_after_latent_slots: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<Box<Q4CaptureReceipt>>,
}

impl Q4CaptureStatus {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid("q4.capture.capture_id", self.capture_id)?;
        if self.latent_slots > MAX_Q4_CAPTURE_LATENT_SLOTS {
            return invalid(
                "q4.capture.latent_slots",
                "exceeds the H3 temporal-axis limit",
            );
        }
        match self.state {
            Q4CaptureState::AwaitingReset => self.validate_awaiting_reset()?,
            Q4CaptureState::Capturing => self.validate_capturing()?,
            Q4CaptureState::StopArmed => self.validate_stop_armed()?,
            Q4CaptureState::Finished => self.validate_finished()?,
            Q4CaptureState::Aborted => self.validate_aborted()?,
        }
        validate_json_size(self, "q4.capture.status")
    }

    fn validate_awaiting_reset(&self) -> Result<(), ValidationError> {
        if self.latent_slots != 0
            || self.stream_generation.is_some()
            || self.finalize_after_latent_slots.is_some()
            || self.reason.is_some()
            || self.receipt.is_some()
        {
            return invalid(
                "q4.capture.status",
                "awaiting_reset fields are inconsistent",
            );
        }
        let current = self
            .current_generation
            .ok_or(ValidationError::InvalidField {
                field: "q4.capture.current_generation",
                reason: "is required while awaiting reset",
            })?;
        let minimum = self
            .minimum_new_generation
            .ok_or(ValidationError::InvalidField {
                field: "q4.capture.minimum_new_generation",
                reason: "is required while awaiting reset",
            })?;
        if current == 0 || minimum <= current {
            return invalid(
                "q4.capture.minimum_new_generation",
                "must be greater than a nonzero current generation",
            );
        }
        match self.mode {
            Q4CaptureMode::Snapshot => {
                let target = self
                    .target_latent_slots
                    .ok_or(ValidationError::InvalidField {
                        field: "q4.capture.target_latent_slots",
                        reason: "is required for snapshot capture",
                    })?;
                if !is_codec_valid_slots(target) {
                    return invalid(
                        "q4.capture.target_latent_slots",
                        "must be a codec-valid snapshot length",
                    );
                }
            }
            Q4CaptureMode::LiveCapture => {
                if self.target_latent_slots != Some(0) {
                    return invalid(
                        "q4.capture.target_latent_slots",
                        "must be zero while live capture awaits reset",
                    );
                }
            }
        }
        Ok(())
    }

    fn validate_capturing(&self) -> Result<(), ValidationError> {
        if self.current_generation.is_some()
            || self.minimum_new_generation.is_some()
            || self.target_latent_slots.is_some()
            || self.finalize_after_latent_slots.is_some()
            || self.reason.is_some()
            || self.receipt.is_some()
        {
            return invalid("q4.capture.status", "capturing fields are inconsistent");
        }
        validate_required_stream_generation(self.stream_generation, "capturing")
    }

    fn validate_stop_armed(&self) -> Result<(), ValidationError> {
        if self.mode != Q4CaptureMode::LiveCapture
            || self.current_generation.is_some()
            || self.minimum_new_generation.is_some()
            || self.target_latent_slots.is_some()
            || self.reason.is_some()
            || self.receipt.is_some()
        {
            return invalid("q4.capture.status", "stop_armed fields are inconsistent");
        }
        validate_required_stream_generation(self.stream_generation, "stop_armed")?;
        let finalize = self
            .finalize_after_latent_slots
            .ok_or(ValidationError::InvalidField {
                field: "q4.capture.finalize_after_latent_slots",
                reason: "is required while a live stop is armed",
            })?;
        if !is_codec_valid_slots(finalize) || finalize <= self.latent_slots {
            return invalid(
                "q4.capture.finalize_after_latent_slots",
                "must be the first later codec-valid boundary",
            );
        }
        Ok(())
    }

    fn validate_finished(&self) -> Result<(), ValidationError> {
        if self.current_generation.is_some()
            || self.minimum_new_generation.is_some()
            || self.target_latent_slots.is_some()
            || self.finalize_after_latent_slots.is_some()
            || self.reason.is_some()
        {
            return invalid("q4.capture.status", "finished fields are inconsistent");
        }
        validate_required_stream_generation(self.stream_generation, "finished")?;
        let receipt = self.receipt.as_ref().ok_or(ValidationError::InvalidField {
            field: "q4.capture.receipt",
            reason: "is required for finished capture",
        })?;
        receipt.validate()?;
        if receipt.capture_id != self.capture_id
            || receipt.mode != self.mode
            || receipt.structural_carrier != self.structural_carrier
            || receipt.visual_shape[2] != self.latent_slots
        {
            return invalid(
                "q4.capture.receipt",
                "must match the enclosing finished capture status",
            );
        }
        Ok(())
    }

    fn validate_aborted(&self) -> Result<(), ValidationError> {
        if self.current_generation.is_some()
            || self.minimum_new_generation.is_some()
            || self.target_latent_slots.is_some()
            || self.finalize_after_latent_slots.is_some()
            || self.receipt.is_some()
        {
            return invalid("q4.capture.status", "aborted fields are inconsistent");
        }
        if let Some(generation) = self.stream_generation {
            validate_nonzero("q4.capture.stream_generation", generation)?;
        }
        let reason = self.reason.as_ref().ok_or(ValidationError::InvalidField {
            field: "q4.capture.reason",
            reason: "is required for an aborted capture",
        })?;
        validate_text("q4.capture.reason", reason, MAX_Q4_CAPTURE_REASON_BYTES)
    }
}

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("first-party Q4 defaults are finite")
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

fn validate_identity(deck_id: &str, deck_revision: u64) -> Result<(), ValidationError> {
    validate_text("q4.deck_id", deck_id, MAX_Q4_TEXT_BYTES)?;
    validate_nonzero("q4.deck_revision", deck_revision)
}

fn validate_provenance_json(value: &str) -> Result<(), ValidationError> {
    validate_text("q4.provenance_json", value, MAX_Q4_PROVENANCE_BYTES)?;
    let parsed = serde_json::from_str::<serde_json::Value>(value).map_err(|_| {
        ValidationError::InvalidField {
            field: "q4.provenance_json",
            reason: "must be valid JSON",
        }
    })?;
    if !parsed.is_object() {
        return invalid("q4.provenance_json", "must be a JSON object");
    }
    Ok(())
}

fn validate_reasons(reasons: &BoundedVec<Q4ResetReason, 5>) -> Result<(), ValidationError> {
    if reasons.is_empty() {
        return invalid("q4.reset.reasons", "must contain at least one reason");
    }
    Ok(())
}

fn validate_reset_barrier(
    deck_id: &str,
    deck_revision: u64,
    current_generation: u64,
    minimum_new_generation: u64,
    reasons: &BoundedVec<Q4ResetReason, 5>,
) -> Result<(), ValidationError> {
    validate_identity(deck_id, deck_revision)?;
    validate_nonzero("q4.current_generation", current_generation)?;
    if minimum_new_generation <= current_generation {
        return invalid(
            "q4.minimum_new_generation",
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
            "q4.ring_sequence_range",
            "must exactly contain the decoded frame count",
        );
    }
    Ok(())
}

fn validate_no_reset(requires_causal_reset: bool) -> Result<(), ValidationError> {
    if requires_causal_reset {
        return invalid(
            "q4.requires_causal_reset",
            "realtime updates must not request a reset",
        );
    }
    Ok(())
}

fn validate_capture_identity(
    deck_id: &str,
    deck_revision: u64,
    capture_id: WireUuid,
) -> Result<(), ValidationError> {
    validate_identity(deck_id, deck_revision)?;
    validate_uuid("q4.capture.capture_id", capture_id)
}

fn is_codec_valid_slots(value: u64) -> bool {
    (2..=MAX_Q4_CAPTURE_LATENT_SLOTS).contains(&value) && (value - 2).is_multiple_of(5)
}

fn validate_capture_payload_name(
    payload_path: &str,
    capture_id: WireUuid,
) -> Result<(), ValidationError> {
    let expected = format!("{capture_id}.safetensors.partial");
    let basename = payload_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if basename != expected {
        return invalid(
            "q4.capture.receipt.payload_path",
            "basename must match the capture-owned partial payload",
        );
    }
    Ok(())
}

fn validate_required_stream_generation(
    value: Option<u64>,
    state: &'static str,
) -> Result<(), ValidationError> {
    validate_nonzero(
        "q4.capture.stream_generation",
        value.ok_or(ValidationError::InvalidField {
            field: "q4.capture.stream_generation",
            reason: match state {
                "capturing" => "is required for an active capture",
                "stop_armed" => "is required while a live stop is armed",
                _ => "is required for this capture state",
            },
        })?,
    )
}

fn decoded_frames(latent_slots: u64) -> Result<u64, ValidationError> {
    5_u64
        .checked_add(17_u64.checked_mul((latent_slots - 2) / 5).ok_or(
            ValidationError::InvalidField {
                field: "q4.capture.receipt.decoded_frame_count",
                reason: "H3 cadence overflows",
            },
        )?)
        .ok_or(ValidationError::InvalidField {
            field: "q4.capture.receipt.decoded_frame_count",
            reason: "H3 cadence overflows",
        })
}

fn validate_audio_policy(
    receipt: &Q4CaptureReceipt,
    decoded_frame_count: u64,
) -> Result<u64, ValidationError> {
    match receipt.audio_policy {
        Q4CaptureAudioPolicy::SourceAbsent => {
            if receipt.audio_policy_reason.is_some() || receipt.audio_descriptor.is_some() {
                return invalid(
                    "q4.capture.receipt.audio_policy",
                    "source_absent forbids a reason and audio descriptor",
                );
            }
            Ok(0)
        }
        Q4CaptureAudioPolicy::CopiedFromCarrierExact => {
            if receipt.audio_policy_reason.is_some() {
                return invalid(
                    "q4.capture.receipt.audio_policy_reason",
                    "copied audio must not have an omission reason",
                );
            }
            let descriptor =
                receipt
                    .audio_descriptor
                    .as_ref()
                    .ok_or(ValidationError::InvalidField {
                        field: "q4.capture.receipt.audio_descriptor",
                        reason: "is required for exact copied audio",
                    })?;
            descriptor.validate()?;
            let expected_audio_slots = decoded_frame_count
                .checked_mul(5)
                .and_then(|value| value.checked_add(1))
                .map(|value| value / 3)
                .ok_or(ValidationError::InvalidField {
                    field: "q4.capture.audio_descriptor.shape",
                    reason: "audio cadence overflows",
                })?;
            if descriptor.shape[3] != expected_audio_slots {
                return invalid(
                    "q4.capture.audio_descriptor.shape",
                    "does not match the H3 decoded-frame cadence",
                );
            }
            Ok(descriptor.byte_length)
        }
        Q4CaptureAudioPolicy::OmittedTimingMismatch => {
            if receipt.audio_policy_reason.is_none() || receipt.audio_descriptor.is_some() {
                return invalid(
                    "q4.capture.receipt.audio_policy",
                    "omitted audio requires one reason and no descriptor",
                );
            }
            Ok(0)
        }
    }
}

fn validate_json_size(value: &impl Serialize, field: &'static str) -> Result<(), ValidationError> {
    let encoded = serde_json::to_vec(value).map_err(|_| ValidationError::InvalidField {
        field,
        reason: "must remain JSON-safe",
    })?;
    if encoded.len() > MAX_Q4_CAPTURE_RECEIPT_BYTES {
        return invalid(field, "exceeds the 32768-byte capture receipt limit");
    }
    Ok(())
}
