//! Portable, path-free Deck preset contract.

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
};

use semver::Version;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    D2Algorithm, D2Controls, D2Mode, D2Routing, D2Xs5Routing, FiniteF64, MAX_D2_SAFE_INTEGER,
    Q4Algorithm, Q4Controls, Q4InfluenceMode, Q4Mode, Q4Roles, Q4Slot, Q4Xs5Routing, WireUuid,
};

pub const DECK_PRESET_SCHEMA_VERSION: &str = "2.0.0";
pub const LEGACY_DECK_PRESET_SCHEMA_VERSION: &str = "0.1.0";
pub const D2_DECK_ID: &str = "org.latentdeck.deck.d2";
pub const Q4_DECK_ID: &str = "org.latentdeck.deck.q4";
pub const BUNDLED_DECK_VERSION: &str = "0.2.0";
pub const MAX_DECK_PRESET_BYTES: usize = 128 * 1024;
const MAX_DECK_PRESET_JSON_DEPTH: usize = 32;
const MAX_PRESET_CONTROLS: usize = 128;
const MAX_PRESET_TEXT_BYTES: usize = 1_024;
const MAX_PRESET_IDENTIFIER_BYTES: usize = 128;
const ALL_CARTRIDGES_ID: &str = "latentdeck.virtual.all";
const UNASSIGNED_ID: &str = "latentdeck.virtual.unassigned";
const DUPLICATE_MARKER: &str = "__latentdeck_preset_duplicate__:";
const DEPTH_MARKER: &str = "__latentdeck_preset_depth__";

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{code}: {detail}")]
pub struct PresetError {
    code: &'static str,
    detail: String,
}

impl PresetError {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetCartridgeIdentity {
    pub cartridge_id: WireUuid,
    pub archive_sha256: String,
}

impl PresetCartridgeIdentity {
    fn validate(&self, field: &str) -> Result<(), PresetError> {
        if self.cartridge_id.is_nil() {
            return Err(invalid(format!("{field}.cartridge_id must not be nil")));
        }
        if self.archive_sha256.len() != 64
            || !self
                .archive_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid(format!(
                "{field}.archive_sha256 must be lowercase SHA-256"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2PresetLoops {
    pub loop_a: bool,
    pub loop_b: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Exact legacy four-slot transport bridge.
pub struct Q4PresetLoops {
    pub loop_a: bool,
    pub loop_b: bool,
    pub loop_c: bool,
    pub loop_d: bool,
}

/// One physical slot and its immutable, path-free cartridge identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetSlot {
    pub physical_slot: u8,
    pub source: PresetCartridgeIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetLoop {
    pub physical_slot: u8,
    pub enabled: bool,
}

/// A control value whose JSON representation preserves its declared type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PresetControlValue {
    Boolean(bool),
    Integer(i64),
    Number(FiniteF64),
    Enum(String),
    Text(String),
}

impl PresetControlValue {
    fn validate(&self, field: &str) -> Result<(), PresetError> {
        match self {
            Self::Integer(value) if value.unsigned_abs() > MAX_D2_SAFE_INTEGER => Err(invalid(
                format!("{field} integer must be inside the exact JavaScript integer range"),
            )),
            Self::Enum(value) => validate_identifier(value, field),
            Self::Text(value) if value.len() > MAX_PRESET_TEXT_BYTES => Err(invalid(format!(
                "{field} text exceeds {MAX_PRESET_TEXT_BYTES} UTF-8 bytes"
            ))),
            Self::Text(_) | Self::Boolean(_) | Self::Integer(_) | Self::Number(_) => Ok(()),
        }
    }
}

/// Generic Preset v2. The selected Deck package interprets the closed role and
/// control IDs; Core still validates the bounded typed envelope independently.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckPresetDocument {
    pub schema_version: String,
    pub deck_id: String,
    pub deck_version: String,
    pub active_collection_id: String,
    pub slots: Vec<PresetSlot>,
    pub roles: BTreeMap<String, u8>,
    pub controls: BTreeMap<String, PresetControlValue>,
    pub loops: Vec<PresetLoop>,
    pub seed: u64,
}

impl DeckPresetDocument {
    #[must_use]
    pub fn d2(
        active_collection_id: String,
        source_a: PresetCartridgeIdentity,
        source_b: PresetCartridgeIdentity,
        controls: &D2Controls,
        loops: D2PresetLoops,
        seed: u64,
    ) -> Self {
        let roles = match controls.routing {
            D2Routing::A => BTreeMap::from([("carrier".to_owned(), 1), ("donor".to_owned(), 2)]),
            D2Routing::B => BTreeMap::from([("carrier".to_owned(), 2), ("donor".to_owned(), 1)]),
        };
        Self {
            schema_version: DECK_PRESET_SCHEMA_VERSION.to_owned(),
            deck_id: D2_DECK_ID.to_owned(),
            deck_version: BUNDLED_DECK_VERSION.to_owned(),
            active_collection_id,
            slots: vec![
                PresetSlot {
                    physical_slot: 1,
                    source: source_a,
                },
                PresetSlot {
                    physical_slot: 2,
                    source: source_b,
                },
            ],
            roles,
            controls: d2_control_values(controls),
            loops: vec![
                PresetLoop {
                    physical_slot: 1,
                    enabled: loops.loop_a,
                },
                PresetLoop {
                    physical_slot: 2,
                    enabled: loops.loop_b,
                },
            ],
            seed,
        }
    }

    #[must_use]
    pub fn q4(
        active_collection_id: String,
        sources: [PresetCartridgeIdentity; 4],
        controls: &Q4Controls,
        routing: Q4Roles,
        loops: Q4PresetLoops,
        seed: u64,
    ) -> Self {
        let [a, b, c, d] = sources;
        Self {
            schema_version: DECK_PRESET_SCHEMA_VERSION.to_owned(),
            deck_id: Q4_DECK_ID.to_owned(),
            deck_version: BUNDLED_DECK_VERSION.to_owned(),
            active_collection_id,
            slots: (1u8..=4)
                .zip([a, b, c, d])
                .map(|(physical_slot, source)| PresetSlot {
                    physical_slot,
                    source,
                })
                .collect(),
            roles: BTreeMap::from([
                ("carrier".to_owned(), q4_slot_number(routing.carrier)),
                ("donor_b".to_owned(), q4_slot_number(routing.donor_b)),
                ("donor_c".to_owned(), q4_slot_number(routing.donor_c)),
                ("donor_d".to_owned(), q4_slot_number(routing.donor_d)),
            ]),
            controls: q4_control_values(controls),
            loops: (1u8..=4)
                .zip([loops.loop_a, loops.loop_b, loops.loop_c, loops.loop_d])
                .map(|(physical_slot, enabled)| PresetLoop {
                    physical_slot,
                    enabled,
                })
                .collect(),
            seed,
        }
    }

    /// Validate exact versions, source identities, controls, routing, and seed.
    ///
    /// # Errors
    ///
    /// Returns a stable preset error without clamping or substituting values.
    pub fn validate(&self) -> Result<(), PresetError> {
        validate_version(&self.schema_version)?;
        validate_deck_id(&self.deck_id)?;
        validate_package_version(&self.deck_version)?;
        validate_collection(&self.active_collection_id)?;

        if !(1..=16).contains(&self.slots.len()) {
            return Err(invalid("slots must contain 1..=16 physical sources"));
        }
        for ((index, slot), expected) in self.slots.iter().enumerate().zip(1u8..=16) {
            if slot.physical_slot != expected {
                return Err(invalid(
                    "slots must be ordered and contiguous from physical_slot 1",
                ));
            }
            slot.source.validate(&format!("slots[{index}].source"))?;
        }

        if self.roles.len() != self.slots.len() {
            return Err(invalid("roles must bind every physical slot exactly once"));
        }
        let mut role_slots = HashSet::new();
        for (role, physical_slot) in &self.roles {
            validate_identifier(role, "roles key")?;
            if *physical_slot == 0 || usize::from(*physical_slot) > self.slots.len() {
                return Err(invalid(format!(
                    "role {role:?} references an unavailable physical slot"
                )));
            }
            if !role_slots.insert(*physical_slot) {
                return Err(invalid("roles must form a physical-slot permutation"));
            }
        }

        if self.controls.len() > MAX_PRESET_CONTROLS {
            return Err(invalid(format!(
                "controls exceeds the {MAX_PRESET_CONTROLS}-entry bound"
            )));
        }
        for (control, value) in &self.controls {
            validate_identifier(control, "controls key")?;
            value.validate(&format!("controls.{control}"))?;
        }

        if self.loops.len() != self.slots.len() {
            return Err(invalid("loops must contain one entry per physical slot"));
        }
        for (loop_state, expected) in self.loops.iter().zip(1u8..=16) {
            if loop_state.physical_slot != expected {
                return Err(invalid(
                    "loops must be ordered and contiguous from physical_slot 1",
                ));
            }
        }
        validate_seed(self.seed, MAX_D2_SAFE_INTEGER)?;
        Ok(())
    }
}

fn d2_control_values(controls: &D2Controls) -> BTreeMap<String, PresetControlValue> {
    BTreeMap::from([
        (
            "algorithm".to_owned(),
            enum_value(match controls.algorithm {
                D2Algorithm::Linear => "LINEAR",
                D2Algorithm::Xs1 => "XS1",
                D2Algorithm::Xs2 => "XS2",
                D2Algorithm::Xs3 => "XS3",
                D2Algorithm::Xs4 => "XS4",
                D2Algorithm::Xs5 => "XS5",
            }),
        ),
        ("mix".to_owned(), number_value(controls.mix)),
        (
            "mode".to_owned(),
            enum_value(match controls.mode {
                D2Mode::Hybridize => "HYBRIDIZE",
                D2Mode::Interact => "INTERACT",
            }),
        ),
        ("interaction".to_owned(), number_value(controls.interaction)),
        ("preserve".to_owned(), number_value(controls.preserve)),
        ("chaos".to_owned(), number_value(controls.chaos)),
        (
            "xs1_channel_a".to_owned(),
            integer_value(controls.xs1_channel_a),
        ),
        (
            "xs1_channel_b".to_owned(),
            integer_value(controls.xs1_channel_b),
        ),
        (
            "xs1_angle_degrees".to_owned(),
            number_value(controls.xs1_angle_degrees),
        ),
        ("xs2_radius".to_owned(), integer_value(controls.xs2_radius)),
        (
            "xs3_high_gain".to_owned(),
            number_value(controls.xs3_high_gain),
        ),
        ("xs4_epsilon".to_owned(), number_value(controls.xs4_epsilon)),
        (
            "xs5_routing".to_owned(),
            enum_value(match controls.xs5_routing {
                D2Xs5Routing::TopK => "TOPK",
                D2Xs5Routing::Sinkhorn => "SINKHORN",
            }),
        ),
        ("temperature".to_owned(), number_value(controls.temperature)),
        ("top_k".to_owned(), integer_value(controls.top_k)),
        (
            "sinkhorn_iterations".to_owned(),
            integer_value(controls.sinkhorn_iterations),
        ),
    ])
}

fn q4_control_values(controls: &Q4Controls) -> BTreeMap<String, PresetControlValue> {
    BTreeMap::from([
        (
            "algorithm".to_owned(),
            enum_value(match controls.algorithm {
                Q4Algorithm::Linear => "LINEAR",
                Q4Algorithm::Xs5 => "XS5",
            }),
        ),
        ("interaction".to_owned(), number_value(controls.interaction)),
        (
            "mode".to_owned(),
            enum_value(match controls.mode {
                Q4Mode::Hybridize => "HYBRIDIZE",
                Q4Mode::Interact => "INTERACT",
            }),
        ),
        ("preserve".to_owned(), number_value(controls.preserve)),
        (
            "influence_mode".to_owned(),
            enum_value(match controls.influence_mode {
                Q4InfluenceMode::Manual => "MANUAL",
                Q4InfluenceMode::Triangle => "TRIANGLE",
            }),
        ),
        (
            "donor_weight_b".to_owned(),
            number_value(controls.donor_weight_b),
        ),
        (
            "donor_weight_c".to_owned(),
            number_value(controls.donor_weight_c),
        ),
        (
            "donor_weight_d".to_owned(),
            number_value(controls.donor_weight_d),
        ),
        ("triangle_x".to_owned(), number_value(controls.triangle_x)),
        ("triangle_y".to_owned(), number_value(controls.triangle_y)),
        (
            "xs5_routing".to_owned(),
            enum_value(match controls.xs5_routing {
                Q4Xs5Routing::TopK => "TOPK",
                Q4Xs5Routing::Sinkhorn => "SINKHORN",
            }),
        ),
        ("temperature".to_owned(), number_value(controls.temperature)),
        ("top_k".to_owned(), integer_value(controls.top_k)),
        (
            "sinkhorn_iterations".to_owned(),
            integer_value(controls.sinkhorn_iterations),
        ),
        ("chaos".to_owned(), number_value(controls.chaos)),
    ])
}

fn enum_value(value: &str) -> PresetControlValue {
    PresetControlValue::Enum(value.to_owned())
}

fn number_value(value: FiniteF64) -> PresetControlValue {
    PresetControlValue::Number(value)
}

fn integer_value(value: u8) -> PresetControlValue {
    PresetControlValue::Integer(i64::from(value))
}

const fn q4_slot_number(slot: Q4Slot) -> u8 {
    match slot {
        Q4Slot::A => 1,
        Q4Slot::B => 2,
        Q4Slot::C => 3,
        Q4Slot::D => 4,
    }
}

fn validate_version(value: &str) -> Result<(), PresetError> {
    if value != DECK_PRESET_SCHEMA_VERSION {
        return Err(PresetError::new(
            "preset.unsupported_version",
            format!("unsupported Deck preset schema version {value:?}"),
        ));
    }
    Ok(())
}

fn validate_deck_id(value: &str) -> Result<(), PresetError> {
    let labels: Vec<_> = value.split('.').collect();
    if value.len() > MAX_PRESET_IDENTIFIER_BYTES || labels.len() < 3 {
        return Err(invalid("deck_id must be a bounded reverse-DNS identifier"));
    }
    for label in labels {
        if label.is_empty()
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(invalid(
                "deck_id must be a lowercase reverse-DNS identifier",
            ));
        }
    }
    Ok(())
}

fn validate_package_version(value: &str) -> Result<(), PresetError> {
    let parsed = Version::parse(value)
        .map_err(|_| invalid("deck_version must be an exact semantic version"))?;
    if parsed.to_string() != value {
        return Err(invalid(
            "deck_version must use canonical semantic-version text",
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, field: &str) -> Result<(), PresetError> {
    if value.is_empty()
        || value.len() > MAX_PRESET_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        return Err(invalid(format!(
            "{field} must be a bounded portable identifier"
        )));
    }
    Ok(())
}

fn validate_collection(value: &str) -> Result<(), PresetError> {
    if value == ALL_CARTRIDGES_ID || value == UNASSIGNED_ID {
        return Ok(());
    }
    let parsed = Uuid::parse_str(value)
        .map_err(|_| invalid("active_collection_id must be a canonical UUID or virtual ID"))?;
    if parsed.is_nil() || parsed.hyphenated().to_string() != value {
        return Err(invalid(
            "active_collection_id must be a canonical non-nil lowercase UUID",
        ));
    }
    Ok(())
}

fn validate_seed(seed: u64, maximum: u64) -> Result<(), PresetError> {
    if seed > maximum {
        return Err(invalid(format!("seed must be inside 0..={maximum}")));
    }
    Ok(())
}

fn invalid(detail: impl Into<String>) -> PresetError {
    PresetError::new("preset.invalid_field", detail)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyD2PresetSlots {
    a: PresetCartridgeIdentity,
    b: PresetCartridgeIdentity,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyQ4PresetSlots {
    a: PresetCartridgeIdentity,
    b: PresetCartridgeIdentity,
    c: PresetCartridgeIdentity,
    d: PresetCartridgeIdentity,
}

#[derive(Deserialize)]
#[serde(tag = "deck_type", deny_unknown_fields)]
enum LegacyDeckPresetDocument {
    #[serde(rename = "LD-D2")]
    D2 {
        schema_version: String,
        active_collection_id: String,
        slots: LegacyD2PresetSlots,
        controls: D2Controls,
        loops: D2PresetLoops,
        seed: u64,
    },
    #[serde(rename = "LD-Q4")]
    Q4 {
        schema_version: String,
        active_collection_id: String,
        slots: LegacyQ4PresetSlots,
        controls: Q4Controls,
        routing: Q4Roles,
        loops: Q4PresetLoops,
        seed: u64,
    },
}

impl LegacyDeckPresetDocument {
    fn migrate(self) -> Result<DeckPresetDocument, PresetError> {
        let migrated = match self {
            Self::D2 {
                schema_version,
                active_collection_id,
                slots,
                controls,
                loops,
                seed,
            } => {
                validate_legacy_version(&schema_version)?;
                controls
                    .validate()
                    .map_err(|error| invalid(format!("legacy D2 controls are invalid: {error}")))?;
                DeckPresetDocument::d2(
                    active_collection_id,
                    slots.a,
                    slots.b,
                    &controls,
                    loops,
                    seed,
                )
            }
            Self::Q4 {
                schema_version,
                active_collection_id,
                slots,
                controls,
                routing,
                loops,
                seed,
            } => {
                validate_legacy_version(&schema_version)?;
                controls
                    .validate()
                    .map_err(|error| invalid(format!("legacy Q4 controls are invalid: {error}")))?;
                routing
                    .validate()
                    .map_err(|error| invalid(format!("legacy Q4 routing is invalid: {error}")))?;
                DeckPresetDocument::q4(
                    active_collection_id,
                    [slots.a, slots.b, slots.c, slots.d],
                    &controls,
                    routing,
                    loops,
                    seed,
                )
            }
        };
        migrated.validate()?;
        Ok(migrated)
    }
}

fn validate_legacy_version(value: &str) -> Result<(), PresetError> {
    if value != LEGACY_DECK_PRESET_SCHEMA_VERSION {
        return Err(PresetError::new(
            "preset.unsupported_version",
            format!("unsupported Deck preset schema version {value:?}"),
        ));
    }
    Ok(())
}

/// Parse one bounded strict UTF-8 preset document.
///
/// # Errors
///
/// Rejects oversized input, duplicate/unknown fields, unsupported versions,
/// invalid source identities, non-finite controls, and invalid routing.
pub fn parse_deck_preset_json(bytes: &[u8]) -> Result<DeckPresetDocument, PresetError> {
    if bytes.is_empty() || bytes.len() > MAX_DECK_PRESET_BYTES {
        return Err(PresetError::new(
            "preset.too_large",
            format!("Deck preset must contain 1..={MAX_DECK_PRESET_BYTES} bytes"),
        ));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| PresetError::new("preset.invalid_utf8", "Deck preset must be strict UTF-8"))?;
    reject_duplicate_keys(text)?;
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| {
        PresetError::new(
            "preset.invalid_json",
            format!("Deck preset JSON is invalid: {error}"),
        )
    })?;
    let schema_version = value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            PresetError::new(
                "preset.invalid_json",
                "Deck preset must contain a string schema_version",
            )
        })?;
    let preset = match schema_version {
        DECK_PRESET_SCHEMA_VERSION => {
            serde_json::from_value::<DeckPresetDocument>(value).map_err(|error| {
                PresetError::new(
                    "preset.invalid_json",
                    format!("Preset v2 schema is invalid: {error}"),
                )
            })?
        }
        LEGACY_DECK_PRESET_SCHEMA_VERSION => {
            serde_json::from_value::<LegacyDeckPresetDocument>(value)
                .map_err(|error| {
                    PresetError::new(
                        "preset.invalid_json",
                        format!("Legacy Deck preset schema is invalid: {error}"),
                    )
                })?
                .migrate()?
        }
        unsupported => {
            return Err(PresetError::new(
                "preset.unsupported_version",
                format!("unsupported Deck preset schema version {unsupported:?}"),
            ));
        }
    };
    preset.validate().map_err(|error| {
        if error.code() == "preset.unsupported_version" {
            error
        } else {
            PresetError::new(error.code(), error.detail().to_owned())
        }
    })?;
    Ok(preset)
}

/// Serialize one validated preset using deterministic struct-field order.
///
/// # Errors
///
/// Refuses invalid in-memory state or an unexpectedly oversized result.
pub fn write_deck_preset_json(preset: &DeckPresetDocument) -> Result<Vec<u8>, PresetError> {
    preset.validate()?;
    let mut bytes = serde_json::to_vec_pretty(preset).map_err(|error| {
        PresetError::new(
            "preset.invalid_json",
            format!("Deck preset could not be serialized: {error}"),
        )
    })?;
    bytes.push(b'\n');
    if bytes.len() > MAX_DECK_PRESET_BYTES {
        return Err(PresetError::new(
            "preset.too_large",
            "serialized Deck preset exceeds its byte bound",
        ));
    }
    Ok(bytes)
}

fn reject_duplicate_keys(text: &str) -> Result<(), PresetError> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    PresetPreflight { depth: 0 }
        .deserialize(&mut deserializer)
        .map_err(|error| {
            let detail = error.to_string();
            if let Some(position) = detail.find(DUPLICATE_MARKER) {
                let key = detail[position + DUPLICATE_MARKER.len()..]
                    .split_whitespace()
                    .next()
                    .unwrap_or("unknown");
                PresetError::new(
                    "preset.duplicate_key",
                    format!("Deck preset contains duplicate key {key:?}"),
                )
            } else if detail.contains(DEPTH_MARKER) {
                PresetError::new(
                    "preset.invalid_json",
                    "Deck preset exceeds the JSON nesting bound",
                )
            } else {
                PresetError::new(
                    "preset.invalid_json",
                    format!("Deck preset JSON is invalid: {error}"),
                )
            }
        })?;
    deserializer.end().map_err(|error| {
        PresetError::new(
            "preset.invalid_json",
            format!("Deck preset contains trailing data: {error}"),
        )
    })
}

struct PresetPreflight {
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for PresetPreflight {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        if self.depth > MAX_DECK_PRESET_JSON_DEPTH {
            return Err(de::Error::custom(DEPTH_MARKER));
        }
        deserializer.deserialize_any(PresetPreflightVisitor { depth: self.depth })
    }
}

struct PresetPreflightVisitor {
    depth: usize,
}

impl<'de> Visitor<'de> for PresetPreflightVisitor {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON")
    }

    fn visit_bool<E: de::Error>(self, _value: bool) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _value: i64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _value: u64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _value: f64) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, _value: &str) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(())
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        PresetPreflight {
            depth: self.depth + 1,
        }
        .deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        while sequence
            .next_element_seed(PresetPreflight {
                depth: self.depth + 1,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut keys = HashSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(de::Error::custom(format!("{DUPLICATE_MARKER}{key}")));
            }
            map.next_value_seed(PresetPreflight {
                depth: self.depth + 1,
            })?;
        }
        Ok(())
    }
}
