//! Shared signal-geometry and compatibility policy for every Deck.
//!
//! This module deliberately reports compatibility without converting media.
//! A Deck that needs crop, alignment, resize, or re-encoding must expose that
//! as a separate, explicit operation which creates a new cartridge.

use latentdeck_cartridge::{
    manifest::{DType, Rational},
    profile::h3::ValidatedH3Profile,
};
use serde::{Deserialize, Serialize};

/// The complete visual signal contract presented to a Deck implementation.
///
/// `latent_slots` and `decoded_frame_count` describe clip length. The remaining
/// fields describe the spatial/runtime contract and are stable across clips of
/// different duration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalGeometry {
    pub codec_family: String,
    pub profile: String,
    pub profile_version: String,
    pub runtime_dtype: DType,
    pub batch: u64,
    pub latent_channels: u64,
    pub latent_slots: u64,
    pub latent_height: u64,
    pub latent_width: u64,
    pub decoded_frame_count: u64,
    pub decoded_height: u32,
    pub decoded_width: u32,
    pub timing_contract: String,
    pub timing_contract_version: String,
    pub frame_rate: Rational,
}

impl SignalGeometry {
    /// Build the shared descriptor from an already validated H3 profile.
    #[must_use]
    pub fn from_h3(profile: &ValidatedH3Profile) -> Self {
        let key = &profile.compatibility_key;
        Self {
            codec_family: key.codec_family.to_owned(),
            profile: key.profile.to_owned(),
            profile_version: key.profile_version.to_owned(),
            runtime_dtype: key.runtime_dtype,
            batch: key.batch,
            latent_channels: key.latent_channels,
            latent_slots: profile.visual.latent_slots,
            latent_height: key.latent_height,
            latent_width: key.latent_width,
            decoded_frame_count: profile.visual.decoded_frame_count,
            decoded_height: profile.visual.decoded_height,
            decoded_width: profile.visual.decoded_width,
            timing_contract: key.timing_contract.to_owned(),
            timing_contract_version: key.timing_contract_version.to_owned(),
            frame_rate: key.frame_rate,
        }
    }

    /// Human-independent presentation facts which UIs may render as badges.
    #[must_use]
    pub fn presentation(&self) -> SignalPresentation {
        SignalPresentation {
            orientation: Orientation::from_extent(self.decoded_width, self.decoded_height),
            aspect_ratio: reduce_ratio(
                u64::from(self.decoded_width),
                u64::from(self.decoded_height),
            ),
            decoded_width: self.decoded_width,
            decoded_height: self.decoded_height,
        }
    }

    /// Exact workload indicators. `None` means checked arithmetic overflowed;
    /// callers must not replace it with a guessed or downscaled value.
    #[must_use]
    pub fn workload(&self) -> SignalWorkload {
        let latent_sites_per_slot = self.latent_height.checked_mul(self.latent_width);
        let latent_values_per_slot = latent_sites_per_slot
            .and_then(|sites| sites.checked_mul(self.latent_channels))
            .and_then(|values| values.checked_mul(self.batch));
        let latent_values_per_clip =
            latent_values_per_slot.and_then(|values| values.checked_mul(self.latent_slots));
        let decoded_pixels_per_frame =
            u64::from(self.decoded_width).checked_mul(u64::from(self.decoded_height));

        SignalWorkload {
            latent_sites_per_slot,
            latent_values_per_slot,
            latent_values_per_clip,
            decoded_pixels_per_frame,
        }
    }
}

/// Orientation is descriptive metadata, never a synthesis policy by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Portrait,
    Landscape,
    Square,
}

impl Orientation {
    #[must_use]
    pub const fn from_extent(width: u32, height: u32) -> Self {
        if width > height {
            Self::Landscape
        } else if width < height {
            Self::Portrait
        } else {
            Self::Square
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AspectRatio {
    pub width: u64,
    pub height: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalPresentation {
    pub orientation: Orientation,
    pub aspect_ratio: AspectRatio,
    pub decoded_width: u32,
    pub decoded_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalWorkload {
    pub latent_sites_per_slot: Option<u64>,
    pub latent_values_per_slot: Option<u64>,
    pub latent_values_per_clip: Option<u64>,
    pub decoded_pixels_per_frame: Option<u64>,
}

/// Cross-source geometry policy chosen by an operator or Deck.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalCompatibilityPolicy {
    /// A validated source can be played at its intrinsic geometry.
    Playback,
    /// Spatial operators require a shared grid, but clips may have independent
    /// temporal lengths/playheads.
    SpatialSynthesis,
    /// Whole-tensor operators additionally require identical temporal length.
    FullTensorSynthesis,
}

/// Stable machine-readable reason for an incompatible input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalGeometryMismatchCode {
    CodecFamily,
    Profile,
    ProfileVersion,
    RuntimeDtype,
    Batch,
    LatentChannels,
    LatentHeight,
    LatentWidth,
    LatentSlots,
    DecodedHeight,
    DecodedWidth,
    DecodedFrameCount,
    TimingContract,
    TimingContractVersion,
    FrameRate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalGeometryMismatch {
    /// Zero-based index in the candidate slice passed to
    /// [`check_signal_compatibility`].
    pub candidate_index: usize,
    pub code: SignalGeometryMismatchCode,
    pub expected: String,
    pub actual: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalCompatibilityReport {
    pub policy: SignalCompatibilityPolicy,
    pub compatible: bool,
    pub mismatches: Vec<SignalGeometryMismatch>,
}

/// Compare every candidate against an unchanged reference signal.
///
/// Playback never imposes a cross-source geometry constraint. Spatial
/// synthesis intentionally ignores clip duration (`T` and decoded frame
/// count), while full-tensor synthesis requires an exact temporal match.
/// This function only reports differences and never mutates either input.
#[must_use]
pub fn check_signal_compatibility(
    policy: SignalCompatibilityPolicy,
    reference: &SignalGeometry,
    candidates: &[SignalGeometry],
) -> SignalCompatibilityReport {
    if policy == SignalCompatibilityPolicy::Playback {
        return SignalCompatibilityReport {
            policy,
            compatible: true,
            mismatches: Vec::new(),
        };
    }

    let mut mismatches = Vec::new();
    for (candidate_index, candidate) in candidates.iter().enumerate() {
        compare_common(reference, candidate, candidate_index, &mut mismatches);
        if policy == SignalCompatibilityPolicy::FullTensorSynthesis {
            push_u64_mismatch(
                &mut mismatches,
                candidate_index,
                SignalGeometryMismatchCode::LatentSlots,
                reference.latent_slots,
                candidate.latent_slots,
            );
            push_u64_mismatch(
                &mut mismatches,
                candidate_index,
                SignalGeometryMismatchCode::DecodedFrameCount,
                reference.decoded_frame_count,
                candidate.decoded_frame_count,
            );
        }
    }

    SignalCompatibilityReport {
        policy,
        compatible: mismatches.is_empty(),
        mismatches,
    }
}

fn compare_common(
    reference: &SignalGeometry,
    candidate: &SignalGeometry,
    candidate_index: usize,
    mismatches: &mut Vec<SignalGeometryMismatch>,
) {
    push_text_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::CodecFamily,
        &reference.codec_family,
        &candidate.codec_family,
    );
    push_text_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::Profile,
        &reference.profile,
        &candidate.profile,
    );
    push_text_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::ProfileVersion,
        &reference.profile_version,
        &candidate.profile_version,
    );
    push_dtype_mismatch(
        mismatches,
        candidate_index,
        reference.runtime_dtype,
        candidate.runtime_dtype,
    );
    push_u64_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::Batch,
        reference.batch,
        candidate.batch,
    );
    push_u64_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::LatentChannels,
        reference.latent_channels,
        candidate.latent_channels,
    );
    push_u64_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::LatentHeight,
        reference.latent_height,
        candidate.latent_height,
    );
    push_u64_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::LatentWidth,
        reference.latent_width,
        candidate.latent_width,
    );
    push_u32_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::DecodedHeight,
        reference.decoded_height,
        candidate.decoded_height,
    );
    push_u32_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::DecodedWidth,
        reference.decoded_width,
        candidate.decoded_width,
    );
    push_text_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::TimingContract,
        &reference.timing_contract,
        &candidate.timing_contract,
    );
    push_text_mismatch(
        mismatches,
        candidate_index,
        SignalGeometryMismatchCode::TimingContractVersion,
        &reference.timing_contract_version,
        &candidate.timing_contract_version,
    );
    if reference.frame_rate != candidate.frame_rate {
        mismatches.push(SignalGeometryMismatch {
            candidate_index,
            code: SignalGeometryMismatchCode::FrameRate,
            expected: rational_text(reference.frame_rate),
            actual: rational_text(candidate.frame_rate),
        });
    }
}

fn push_text_mismatch(
    mismatches: &mut Vec<SignalGeometryMismatch>,
    candidate_index: usize,
    code: SignalGeometryMismatchCode,
    expected: &str,
    actual: &str,
) {
    if expected != actual {
        mismatches.push(SignalGeometryMismatch {
            candidate_index,
            code,
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        });
    }
}

fn push_u64_mismatch(
    mismatches: &mut Vec<SignalGeometryMismatch>,
    candidate_index: usize,
    code: SignalGeometryMismatchCode,
    expected: u64,
    actual: u64,
) {
    if expected != actual {
        mismatches.push(SignalGeometryMismatch {
            candidate_index,
            code,
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
}

fn push_u32_mismatch(
    mismatches: &mut Vec<SignalGeometryMismatch>,
    candidate_index: usize,
    code: SignalGeometryMismatchCode,
    expected: u32,
    actual: u32,
) {
    if expected != actual {
        mismatches.push(SignalGeometryMismatch {
            candidate_index,
            code,
            expected: expected.to_string(),
            actual: actual.to_string(),
        });
    }
}

fn push_dtype_mismatch(
    mismatches: &mut Vec<SignalGeometryMismatch>,
    candidate_index: usize,
    expected: DType,
    actual: DType,
) {
    if expected != actual {
        mismatches.push(SignalGeometryMismatch {
            candidate_index,
            code: SignalGeometryMismatchCode::RuntimeDtype,
            expected: dtype_text(expected).to_owned(),
            actual: dtype_text(actual).to_owned(),
        });
    }
}

const fn dtype_text(dtype: DType) -> &'static str {
    match dtype {
        DType::F16 => "F16",
        DType::F32 => "F32",
        _ => "UNSUPPORTED",
    }
}

fn rational_text(value: Rational) -> String {
    format!("{}/{}", value.numerator, value.denominator)
}

const fn reduce_ratio(width: u64, height: u64) -> AspectRatio {
    let divisor = greatest_common_divisor(width, height);
    AspectRatio {
        width: width / divisor,
        height: height / divisor,
    }
}

const fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    if left == 0 { 1 } else { left }
}

#[cfg(test)]
mod tests {
    use latentdeck_cartridge::{
        manifest::{DType, Rational},
        profile::h3::{
            CODEC_FAMILY, H3CompatibilityKey, PROFILE, PROFILE_VERSION, TIMING_CONTRACT,
            TIMING_CONTRACT_VERSION, ValidatedH3Profile, ValidatedVisual,
        },
    };

    use super::{
        AspectRatio, Orientation, SignalCompatibilityPolicy, SignalGeometry,
        SignalGeometryMismatchCode, check_signal_compatibility,
    };

    #[test]
    fn h3_descriptor_exposes_portrait_presentation_and_exact_workload() {
        let geometry = SignalGeometry::from_h3(&h3_profile(32, 28, 50, 448, 800));

        assert_eq!(geometry.presentation().orientation, Orientation::Portrait);
        assert_eq!(
            geometry.presentation().aspect_ratio,
            AspectRatio {
                width: 14,
                height: 25,
            }
        );
        assert_eq!(geometry.workload().latent_sites_per_slot, Some(1_400));
        assert_eq!(geometry.workload().latent_values_per_slot, Some(33_600));
        assert_eq!(geometry.workload().latent_values_per_clip, Some(1_075_200));
        assert_eq!(geometry.workload().decoded_pixels_per_frame, Some(358_400));
    }

    #[test]
    fn spatial_synthesis_allows_independent_clip_lengths() {
        let reference = SignalGeometry::from_h3(&h3_profile(32, 28, 48, 448, 768));
        let candidate = SignalGeometry::from_h3(&h3_profile(72, 28, 48, 448, 768));

        let report = check_signal_compatibility(
            SignalCompatibilityPolicy::SpatialSynthesis,
            &reference,
            &[candidate],
        );

        assert!(report.compatible);
        assert!(report.mismatches.is_empty());
    }

    #[test]
    fn full_tensor_synthesis_requires_exact_temporal_length() {
        let reference = SignalGeometry::from_h3(&h3_profile(32, 28, 48, 448, 768));
        let candidate = SignalGeometry::from_h3(&h3_profile(72, 28, 48, 448, 768));

        let report = check_signal_compatibility(
            SignalCompatibilityPolicy::FullTensorSynthesis,
            &reference,
            &[candidate],
        );

        assert!(!report.compatible);
        assert_eq!(report.mismatches.len(), 2);
        assert_eq!(
            report.mismatches[0].code,
            SignalGeometryMismatchCode::LatentSlots
        );
        assert_eq!(
            report.mismatches[1].code,
            SignalGeometryMismatchCode::DecodedFrameCount
        );
    }

    #[test]
    fn portrait_and_landscape_are_reported_without_conversion() {
        let portrait = SignalGeometry::from_h3(&h3_profile(32, 28, 50, 448, 800));
        let landscape = SignalGeometry::from_h3(&h3_profile(32, 84, 48, 1_344, 768));
        let unchanged = landscape.clone();

        let report = check_signal_compatibility(
            SignalCompatibilityPolicy::SpatialSynthesis,
            &portrait,
            &[landscape],
        );

        assert!(!report.compatible);
        let codes = report
            .mismatches
            .iter()
            .map(|mismatch| mismatch.code)
            .collect::<Vec<_>>();
        assert_eq!(
            codes,
            vec![
                SignalGeometryMismatchCode::LatentHeight,
                SignalGeometryMismatchCode::LatentWidth,
                SignalGeometryMismatchCode::DecodedHeight,
                SignalGeometryMismatchCode::DecodedWidth,
            ]
        );
        assert_eq!(unchanged.latent_width, 84);
        assert_eq!(unchanged.decoded_width, 1_344);
    }

    #[test]
    fn playback_accepts_each_valid_intrinsic_geometry() {
        let portrait = SignalGeometry::from_h3(&h3_profile(32, 28, 50, 448, 800));
        let landscape = SignalGeometry::from_h3(&h3_profile(107, 84, 48, 1_344, 768));

        let report = check_signal_compatibility(
            SignalCompatibilityPolicy::Playback,
            &portrait,
            &[landscape],
        );

        assert!(report.compatible);
        assert!(report.mismatches.is_empty());
    }

    fn h3_profile(
        latent_slots: u64,
        latent_width: u64,
        latent_height: u64,
        decoded_width: u32,
        decoded_height: u32,
    ) -> ValidatedH3Profile {
        let decoded_frame_count = ((latent_slots - 2) / 5) * 17 + 5;
        ValidatedH3Profile {
            visual: ValidatedVisual {
                latent_slots,
                latent_height,
                latent_width,
                decoded_frame_count,
                decoded_height,
                decoded_width,
            },
            audio: None,
            compatibility_key: H3CompatibilityKey {
                codec_family: CODEC_FAMILY,
                profile: PROFILE,
                profile_version: PROFILE_VERSION,
                runtime_dtype: DType::F16,
                batch: 1,
                latent_channels: 24,
                latent_height,
                latent_width,
                timing_contract: TIMING_CONTRACT,
                timing_contract_version: TIMING_CONTRACT_VERSION,
                frame_rate: Rational {
                    numerator: 24,
                    denominator: 1,
                },
            },
        }
    }
}
