//! Data-only legacy D2/Q4 preset values.
//!
//! These types exist solely to parse accepted 0.1 preset documents and write
//! their deterministic Preset v2 equivalents. They are not Worker Protocol 1
//! commands and are never dispatched to a codec worker.

use std::{fmt, marker::PhantomData};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

use crate::ValidationError;

/// Largest integer that can make a lossless round trip through JavaScript.
pub const MAX_D2_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// A preset number whose representation is known not to be NaN or infinity.
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
    /// Validate the accepted legacy preset control block without clamping.
    ///
    /// # Errors
    ///
    /// Returns a validation error when an accepted legacy preset value is
    /// outside its original closed bounds.
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
    /// Reject aliases and omissions in accepted legacy preset role bindings.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a physical slot is repeated or omitted.
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
    /// Validate the accepted legacy preset control block without clamping.
    ///
    /// # Errors
    ///
    /// Returns a validation error for a closed-enum or bounded-control
    /// violation, an empty manual distribution, or an invalid triangle point.
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

fn finite(value: f64) -> FiniteF64 {
    FiniteF64::new(value).expect("preset defaults are finite")
}

fn validate_number(
    field: &'static str,
    value: FiniteF64,
    minimum: f64,
    maximum: f64,
) -> Result<(), ValidationError> {
    if !(minimum..=maximum).contains(&value.get()) {
        return invalid(field, "is outside the allowed finite range");
    }
    Ok(())
}

fn invalid<T>(field: &'static str, reason: &'static str) -> Result<T, ValidationError> {
    Err(ValidationError::InvalidField { field, reason })
}
