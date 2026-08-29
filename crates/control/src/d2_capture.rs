//! Closed Worker Protocol 1 schema for bounded LD-D2 resample capture.

use serde::{Deserialize, Serialize};

use super::{
    d2::{
        D2Controls, D2Routing, invalid, validate_identity, validate_nonzero, validate_path,
        validate_safe_integer, validate_sha256, validate_text, validate_uuid,
    },
    protocol::{BoundedVec, ValidationError, WireUuid},
};

pub const MAX_D2_CAPTURE_LATENT_SLOTS: u64 = 1_048_576;
pub const MAX_D2_CAPTURE_VISUAL_BYTES: u64 = 15 * 1024 * 1024 * 1024;
pub const MAX_D2_CAPTURE_CONTROL_EVENTS: usize = 32;
pub const MAX_D2_CAPTURE_RECEIPT_BYTES: usize = 32_768;
const MAX_D2_CAPTURE_LATENT_AXIS: u64 = 256;
const MAX_D2_CAPTURE_REASON_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum D2CaptureMode {
    Snapshot,
    LiveCapture,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum D2CaptureState {
    AwaitingReset,
    Capturing,
    StopArmed,
    Finished,
    Aborted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureStart {
    pub deck_id: String,
    pub deck_revision: u64,
    pub capture_id: WireUuid,
    pub mode: D2CaptureMode,
    pub temporary_root: String,
    pub max_latent_slots: u64,
    pub max_visual_bytes: u64,
}

impl D2CaptureStart {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_capture_identity(&self.deck_id, self.deck_revision, self.capture_id)?;
        validate_path("d2.capture.temporary_root", &self.temporary_root)?;
        if !(2..=MAX_D2_CAPTURE_LATENT_SLOTS).contains(&self.max_latent_slots) {
            return invalid("d2.capture.max_latent_slots", "must be within 2..=1048576");
        }
        if !(1..=MAX_D2_CAPTURE_VISUAL_BYTES).contains(&self.max_visual_bytes) {
            return invalid(
                "d2.capture.max_visual_bytes",
                "must be within the 15 GiB H3 payload limit",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureStop {
    pub deck_id: String,
    pub deck_revision: u64,
    pub capture_id: WireUuid,
}

impl D2CaptureStop {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_capture_identity(&self.deck_id, self.deck_revision, self.capture_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureStatusRequest {
    pub deck_id: String,
    pub deck_revision: u64,
    pub capture_id: WireUuid,
}

impl D2CaptureStatusRequest {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_capture_identity(&self.deck_id, self.deck_revision, self.capture_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2CaptureVisualDtype {
    #[serde(rename = "F16")]
    F16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum D2CaptureAudioDtype {
    #[serde(rename = "F16")]
    F16,
    #[serde(rename = "F32")]
    F32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum D2CaptureAudioPolicy {
    SourceAbsent,
    CopiedFromCarrierExact,
    OmittedTimingMismatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum D2CaptureAudioPolicyReason {
    DurationAndMappingMismatch,
    DurationMismatch,
    TemporalMappingMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureAudioDescriptor {
    pub storage_dtype: D2CaptureAudioDtype,
    pub shape: [u64; 4],
    pub byte_length: u64,
}

impl D2CaptureAudioDescriptor {
    fn validate(&self) -> Result<(), ValidationError> {
        let [batch, channels, stereo, temporal] = self.shape;
        if batch != 1
            || channels != 32
            || stereo != 2
            || temporal == 0
            || temporal > MAX_D2_CAPTURE_LATENT_SLOTS
        {
            return invalid(
                "d2.capture.audio_descriptor.shape",
                "must be [1,32,2,T] within the H3 temporal limit",
            );
        }
        let element_bytes = match self.storage_dtype {
            D2CaptureAudioDtype::F16 => 2,
            D2CaptureAudioDtype::F32 => 4,
        };
        let expected = 32_u64
            .checked_mul(2)
            .and_then(|value| value.checked_mul(temporal))
            .and_then(|value| value.checked_mul(element_bytes))
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.audio_descriptor.byte_length",
                reason: "audio descriptor size overflows",
            })?;
        if self.byte_length != expected {
            return invalid(
                "d2.capture.audio_descriptor.byte_length",
                "does not match dtype and shape",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureParent {
    pub slot: D2Routing,
    pub cartridge_id: WireUuid,
    pub archive_sha256: String,
}

impl D2CaptureParent {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid("d2.capture.parent.cartridge_id", self.cartridge_id)?;
        validate_sha256("d2.capture.parent.archive_sha256", &self.archive_sha256)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureControlEvent {
    pub slot_offset: u64,
    pub controls: D2Controls,
    pub seed: u64,
}

impl D2CaptureControlEvent {
    fn validate(&self, latent_slots: u64) -> Result<(), ValidationError> {
        if self.slot_offset >= latent_slots {
            return invalid(
                "d2.capture.control_event.slot_offset",
                "must identify a captured latent-slot boundary",
            );
        }
        self.controls.validate()?;
        validate_capture_seed(self.seed)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureReceipt {
    pub capture_id: WireUuid,
    pub mode: D2CaptureMode,
    pub payload_path: String,
    pub payload_sha256: String,
    pub payload_bytes: u64,
    pub storage_dtype: D2CaptureVisualDtype,
    pub visual_shape: [u64; 5],
    pub decoded_frame_count: u64,
    pub audio_policy: D2CaptureAudioPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_policy_reason: Option<D2CaptureAudioPolicyReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_descriptor: Option<D2CaptureAudioDescriptor>,
    pub structural_carrier: D2Routing,
    pub parents: [D2CaptureParent; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_controls: Option<D2Controls>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_events: Option<BoundedVec<D2CaptureControlEvent, MAX_D2_CAPTURE_CONTROL_EVENTS>>,
}

impl D2CaptureReceipt {
    fn validate(&self) -> Result<(), ValidationError> {
        let (temporal, visual_bytes, expected_frames) = self.validate_payload_descriptor()?;
        let audio_bytes = validate_audio_policy(self, expected_frames)?;
        self.validate_payload_size(visual_bytes, audio_bytes)?;
        self.validate_parents()?;
        self.validate_mode_provenance(temporal)?;
        validate_json_size(self, "d2.capture.receipt")
    }

    fn validate_payload_descriptor(&self) -> Result<(u64, u64, u64), ValidationError> {
        validate_uuid("d2.capture.receipt.capture_id", self.capture_id)?;
        validate_path("d2.capture.receipt.payload_path", &self.payload_path)?;
        validate_capture_payload_name(&self.payload_path, self.capture_id)?;
        validate_sha256("d2.capture.receipt.payload_sha256", &self.payload_sha256)?;
        if self.payload_bytes == 0 || self.payload_bytes > MAX_D2_CAPTURE_VISUAL_BYTES {
            return invalid(
                "d2.capture.receipt.payload_bytes",
                "must fit the nonzero 15 GiB H3 payload limit",
            );
        }
        let [batch, channels, temporal, height, width] = self.visual_shape;
        if batch != 1
            || channels != 24
            || !is_codec_valid_slots(temporal)
            || !(1..=MAX_D2_CAPTURE_LATENT_AXIS).contains(&height)
            || !(1..=MAX_D2_CAPTURE_LATENT_AXIS).contains(&width)
        {
            return invalid(
                "d2.capture.receipt.visual_shape",
                "must be codec-valid [1,24,T,H,W]",
            );
        }
        let visual_bytes = 24_u64
            .checked_mul(temporal)
            .and_then(|value| value.checked_mul(height))
            .and_then(|value| value.checked_mul(width))
            .and_then(|value| value.checked_mul(2))
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.receipt.visual_shape",
                reason: "visual tensor size overflows",
            })?;
        let expected_frames = decoded_frames(temporal)?;
        if self.decoded_frame_count != expected_frames {
            return invalid(
                "d2.capture.receipt.decoded_frame_count",
                "does not match the H3 capture cadence",
            );
        }
        Ok((temporal, visual_bytes, expected_frames))
    }

    fn validate_payload_size(
        &self,
        visual_bytes: u64,
        audio_bytes: u64,
    ) -> Result<(), ValidationError> {
        let minimum_payload_bytes = visual_bytes
            .checked_add(audio_bytes)
            .and_then(|value| value.checked_add(8))
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.receipt.payload_bytes",
                reason: "payload size overflows",
            })?;
        if self.payload_bytes <= minimum_payload_bytes {
            return invalid(
                "d2.capture.receipt.payload_bytes",
                "must include a bounded Safetensors header and declared tensors",
            );
        }
        Ok(())
    }

    fn validate_parents(&self) -> Result<(), ValidationError> {
        if self.parents[0].slot != D2Routing::A || self.parents[1].slot != D2Routing::B {
            return invalid(
                "d2.capture.receipt.parents",
                "must contain ordered A and B source identities",
            );
        }
        self.parents[0].validate()?;
        self.parents[1].validate()
    }

    fn validate_mode_provenance(&self, temporal: u64) -> Result<(), ValidationError> {
        match self.mode {
            D2CaptureMode::Snapshot => self.validate_snapshot_provenance(),
            D2CaptureMode::LiveCapture => self.validate_live_provenance(temporal),
        }
    }

    fn validate_snapshot_provenance(&self) -> Result<(), ValidationError> {
        let seed = self.frozen_seed.ok_or(ValidationError::InvalidField {
            field: "d2.capture.receipt.frozen_seed",
            reason: "is required for snapshot capture",
        })?;
        let controls = self
            .frozen_controls
            .as_ref()
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.receipt.frozen_controls",
                reason: "is required for snapshot capture",
            })?;
        if self.control_events.is_some() {
            return invalid(
                "d2.capture.receipt.control_events",
                "is forbidden for snapshot capture",
            );
        }
        validate_capture_seed(seed)?;
        controls.validate()?;
        if controls.routing != self.structural_carrier {
            return invalid(
                "d2.capture.receipt.structural_carrier",
                "must match frozen snapshot controls",
            );
        }
        if self.audio_policy == D2CaptureAudioPolicy::OmittedTimingMismatch {
            return invalid(
                "d2.capture.receipt.audio_policy",
                "snapshot timing cannot be omitted",
            );
        }
        Ok(())
    }

    fn validate_live_provenance(&self, temporal: u64) -> Result<(), ValidationError> {
        if self.frozen_seed.is_some() || self.frozen_controls.is_some() {
            return invalid(
                "d2.capture.receipt.frozen_controls",
                "frozen snapshot fields are forbidden for live capture",
            );
        }
        let events = self
            .control_events
            .as_ref()
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.receipt.control_events",
                reason: "is required for live capture",
            })?;
        if events.is_empty() || events[0].slot_offset != 0 {
            return invalid(
                "d2.capture.receipt.control_events",
                "must begin with the initial state at slot offset zero",
            );
        }
        let mut previous_offset = 0;
        for event in events.iter() {
            event.validate(temporal)?;
            if event.slot_offset < previous_offset {
                return invalid(
                    "d2.capture.receipt.control_events",
                    "slot offsets must be nondecreasing",
                );
            }
            previous_offset = event.slot_offset;
        }
        if events[0].controls.routing != self.structural_carrier {
            return invalid(
                "d2.capture.receipt.structural_carrier",
                "must match the initial live controls",
            );
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2CaptureStatus {
    pub capture_id: WireUuid,
    pub mode: D2CaptureMode,
    pub state: D2CaptureState,
    pub structural_carrier: D2Routing,
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
    pub receipt: Option<D2CaptureReceipt>,
}

impl D2CaptureStatus {
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid("d2.capture.capture_id", self.capture_id)?;
        if self.latent_slots > MAX_D2_CAPTURE_LATENT_SLOTS {
            return invalid(
                "d2.capture.latent_slots",
                "exceeds the H3 temporal-axis limit",
            );
        }
        match self.state {
            D2CaptureState::AwaitingReset => self.validate_awaiting_reset()?,
            D2CaptureState::Capturing => self.validate_capturing()?,
            D2CaptureState::StopArmed => self.validate_stop_armed()?,
            D2CaptureState::Finished => self.validate_finished()?,
            D2CaptureState::Aborted => self.validate_aborted()?,
        }
        validate_json_size(self, "d2.capture.status")
    }

    fn validate_awaiting_reset(&self) -> Result<(), ValidationError> {
        if self.latent_slots != 0
            || self.stream_generation.is_some()
            || self.finalize_after_latent_slots.is_some()
            || self.reason.is_some()
            || self.receipt.is_some()
        {
            return invalid(
                "d2.capture.status",
                "awaiting_reset fields are inconsistent",
            );
        }
        let current = self
            .current_generation
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.current_generation",
                reason: "is required while awaiting reset",
            })?;
        let minimum = self
            .minimum_new_generation
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.minimum_new_generation",
                reason: "is required while awaiting reset",
            })?;
        if current == 0 || minimum <= current {
            return invalid(
                "d2.capture.minimum_new_generation",
                "must be greater than a nonzero current generation",
            );
        }
        match self.mode {
            D2CaptureMode::Snapshot => {
                let target = self
                    .target_latent_slots
                    .ok_or(ValidationError::InvalidField {
                        field: "d2.capture.target_latent_slots",
                        reason: "is required for snapshot capture",
                    })?;
                if !is_codec_valid_slots(target) {
                    return invalid(
                        "d2.capture.target_latent_slots",
                        "must be a codec-valid snapshot length",
                    );
                }
            }
            D2CaptureMode::LiveCapture => {
                if self.target_latent_slots != Some(0) {
                    return invalid(
                        "d2.capture.target_latent_slots",
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
            return invalid("d2.capture.status", "capturing fields are inconsistent");
        }
        validate_required_stream_generation(self.stream_generation, "capturing")
    }

    fn validate_stop_armed(&self) -> Result<(), ValidationError> {
        if self.mode != D2CaptureMode::LiveCapture
            || self.current_generation.is_some()
            || self.minimum_new_generation.is_some()
            || self.target_latent_slots.is_some()
            || self.reason.is_some()
            || self.receipt.is_some()
        {
            return invalid("d2.capture.status", "stop_armed fields are inconsistent");
        }
        validate_required_stream_generation(self.stream_generation, "stop_armed")?;
        let finalize = self
            .finalize_after_latent_slots
            .ok_or(ValidationError::InvalidField {
                field: "d2.capture.finalize_after_latent_slots",
                reason: "is required while a live stop is armed",
            })?;
        if !is_codec_valid_slots(finalize) || finalize <= self.latent_slots {
            return invalid(
                "d2.capture.finalize_after_latent_slots",
                "must be the next codec-valid boundary after latent_slots",
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
            return invalid("d2.capture.status", "finished fields are inconsistent");
        }
        validate_nonzero(
            "d2.capture.stream_generation",
            self.stream_generation
                .ok_or(ValidationError::InvalidField {
                    field: "d2.capture.stream_generation",
                    reason: "is required for a finished capture",
                })?,
        )?;
        let receipt = self.receipt.as_ref().ok_or(ValidationError::InvalidField {
            field: "d2.capture.receipt",
            reason: "is required for a finished capture",
        })?;
        receipt.validate()?;
        if receipt.capture_id != self.capture_id
            || receipt.mode != self.mode
            || receipt.structural_carrier != self.structural_carrier
            || receipt.visual_shape[2] != self.latent_slots
        {
            return invalid("d2.capture.receipt", "does not match its capture status");
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
            return invalid("d2.capture.status", "aborted fields are inconsistent");
        }
        if let Some(generation) = self.stream_generation {
            validate_nonzero("d2.capture.stream_generation", generation)?;
        }
        validate_text(
            "d2.capture.reason",
            self.reason
                .as_deref()
                .ok_or(ValidationError::InvalidField {
                    field: "d2.capture.reason",
                    reason: "is required for an aborted capture",
                })?,
            MAX_D2_CAPTURE_REASON_BYTES,
        )
    }
}

fn validate_capture_identity(
    deck_id: &str,
    deck_revision: u64,
    capture_id: WireUuid,
) -> Result<(), ValidationError> {
    validate_identity(deck_id, deck_revision)?;
    validate_uuid("d2.capture.capture_id", capture_id)
}

fn is_codec_valid_slots(value: u64) -> bool {
    (2..=MAX_D2_CAPTURE_LATENT_SLOTS).contains(&value) && (value - 2).is_multiple_of(5)
}

fn validate_capture_seed(seed: u64) -> Result<(), ValidationError> {
    validate_safe_integer("d2.capture.seed", seed)
}

fn validate_capture_payload_name(
    payload_path: &str,
    capture_id: WireUuid,
) -> Result<(), ValidationError> {
    let expected = format!("{capture_id}.safetensors.partial");
    let basename = payload_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if basename != expected {
        return invalid(
            "d2.capture.receipt.payload_path",
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
        "d2.capture.stream_generation",
        value.ok_or(ValidationError::InvalidField {
            field: "d2.capture.stream_generation",
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
                field: "d2.capture.receipt.decoded_frame_count",
                reason: "H3 cadence overflows",
            },
        )?)
        .ok_or(ValidationError::InvalidField {
            field: "d2.capture.receipt.decoded_frame_count",
            reason: "H3 cadence overflows",
        })
}

fn validate_audio_policy(
    receipt: &D2CaptureReceipt,
    decoded_frame_count: u64,
) -> Result<u64, ValidationError> {
    match receipt.audio_policy {
        D2CaptureAudioPolicy::SourceAbsent => {
            if receipt.audio_policy_reason.is_some() || receipt.audio_descriptor.is_some() {
                return invalid(
                    "d2.capture.receipt.audio_policy",
                    "source_absent forbids a reason and audio descriptor",
                );
            }
            Ok(0)
        }
        D2CaptureAudioPolicy::CopiedFromCarrierExact => {
            if receipt.audio_policy_reason.is_some() {
                return invalid(
                    "d2.capture.receipt.audio_policy_reason",
                    "copied audio must not have an omission reason",
                );
            }
            let descriptor =
                receipt
                    .audio_descriptor
                    .as_ref()
                    .ok_or(ValidationError::InvalidField {
                        field: "d2.capture.receipt.audio_descriptor",
                        reason: "is required for exact copied audio",
                    })?;
            descriptor.validate()?;
            let expected_audio_slots = decoded_frame_count
                .checked_mul(5)
                .and_then(|value| value.checked_add(1))
                .map(|value| value / 3)
                .ok_or(ValidationError::InvalidField {
                    field: "d2.capture.audio_descriptor.shape",
                    reason: "audio cadence overflows",
                })?;
            if descriptor.shape[3] != expected_audio_slots {
                return invalid(
                    "d2.capture.audio_descriptor.shape",
                    "does not match the H3 decoded-frame cadence",
                );
            }
            Ok(descriptor.byte_length)
        }
        D2CaptureAudioPolicy::OmittedTimingMismatch => {
            if receipt.audio_policy_reason.is_none() || receipt.audio_descriptor.is_some() {
                return invalid(
                    "d2.capture.receipt.audio_policy",
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
    if encoded.len() > MAX_D2_CAPTURE_RECEIPT_BYTES {
        return invalid(field, "exceeds the 32768-byte capture receipt limit");
    }
    Ok(())
}
