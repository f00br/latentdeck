//! Portable, path-free Deck preset contract.

use std::{collections::HashSet, fmt};

use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};
use thiserror::Error;
use uuid::Uuid;

use crate::{D2Controls, MAX_D2_SAFE_INTEGER, MAX_Q4_SAFE_INTEGER, Q4Controls, Q4Roles, WireUuid};

pub const DECK_PRESET_SCHEMA_VERSION: &str = "0.1.0";
pub const MAX_DECK_PRESET_BYTES: usize = 128 * 1024;
const MAX_DECK_PRESET_JSON_DEPTH: usize = 32;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2PresetLoops {
    pub loop_a: bool,
    pub loop_b: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Exact four-slot preset transport schema.
pub struct Q4PresetLoops {
    pub loop_a: bool,
    pub loop_b: bool,
    pub loop_c: bool,
    pub loop_d: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct D2PresetSlots {
    pub a: PresetCartridgeIdentity,
    pub b: PresetCartridgeIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Q4PresetSlots {
    pub a: PresetCartridgeIdentity,
    pub b: PresetCartridgeIdentity,
    pub c: PresetCartridgeIdentity,
    pub d: PresetCartridgeIdentity,
}

/// One exact, versioned D2 or Q4 control snapshot. It never stores paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "deck_type", deny_unknown_fields)]
pub enum DeckPresetDocument {
    #[serde(rename = "LD-D2")]
    D2 {
        schema_version: String,
        active_collection_id: String,
        slots: D2PresetSlots,
        controls: D2Controls,
        loops: D2PresetLoops,
        seed: u64,
    },
    #[serde(rename = "LD-Q4")]
    Q4 {
        schema_version: String,
        active_collection_id: String,
        slots: Q4PresetSlots,
        controls: Q4Controls,
        routing: Q4Roles,
        loops: Q4PresetLoops,
        seed: u64,
    },
}

impl DeckPresetDocument {
    #[must_use]
    pub fn d2(
        active_collection_id: String,
        source_a: PresetCartridgeIdentity,
        source_b: PresetCartridgeIdentity,
        controls: D2Controls,
        loops: D2PresetLoops,
        seed: u64,
    ) -> Self {
        Self::D2 {
            schema_version: DECK_PRESET_SCHEMA_VERSION.to_owned(),
            active_collection_id,
            slots: D2PresetSlots {
                a: source_a,
                b: source_b,
            },
            controls,
            loops,
            seed,
        }
    }

    #[must_use]
    pub fn q4(
        active_collection_id: String,
        sources: [PresetCartridgeIdentity; 4],
        controls: Q4Controls,
        routing: Q4Roles,
        loops: Q4PresetLoops,
        seed: u64,
    ) -> Self {
        let [a, b, c, d] = sources;
        Self::Q4 {
            schema_version: DECK_PRESET_SCHEMA_VERSION.to_owned(),
            active_collection_id,
            slots: Q4PresetSlots { a, b, c, d },
            controls,
            routing,
            loops,
            seed,
        }
    }

    /// Validate exact versions, source identities, controls, routing, and seed.
    ///
    /// # Errors
    ///
    /// Returns a stable preset error without clamping or substituting values.
    pub fn validate(&self) -> Result<(), PresetError> {
        match self {
            Self::D2 {
                schema_version,
                active_collection_id,
                slots,
                controls,
                seed,
                ..
            } => {
                validate_version(schema_version)?;
                validate_collection(active_collection_id)?;
                slots.a.validate("slots.a")?;
                slots.b.validate("slots.b")?;
                controls
                    .validate()
                    .map_err(|error| invalid(format!("controls are invalid: {error}")))?;
                validate_seed(*seed, MAX_D2_SAFE_INTEGER)?;
            }
            Self::Q4 {
                schema_version,
                active_collection_id,
                slots,
                controls,
                routing,
                seed,
                ..
            } => {
                validate_version(schema_version)?;
                validate_collection(active_collection_id)?;
                for (field, identity) in [
                    ("slots.a", &slots.a),
                    ("slots.b", &slots.b),
                    ("slots.c", &slots.c),
                    ("slots.d", &slots.d),
                ] {
                    identity.validate(field)?;
                }
                controls
                    .validate()
                    .map_err(|error| invalid(format!("controls are invalid: {error}")))?;
                routing
                    .validate()
                    .map_err(|error| invalid(format!("routing is invalid: {error}")))?;
                validate_seed(*seed, MAX_Q4_SAFE_INTEGER)?;
            }
        }
        Ok(())
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
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let preset = DeckPresetDocument::deserialize(&mut deserializer).map_err(|error| {
        PresetError::new(
            "preset.invalid_json",
            format!("Deck preset schema is invalid: {error}"),
        )
    })?;
    deserializer.end().map_err(|error| {
        PresetError::new(
            "preset.invalid_json",
            format!("Deck preset contains trailing data: {error}"),
        )
    })?;
    preset.validate()?;
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
