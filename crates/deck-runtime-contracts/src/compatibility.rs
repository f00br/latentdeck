use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use semver::{Op, Version, VersionReq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

const MAX_CONTRACT_ID_BYTES: usize = 128;
const MAX_HOST_API_REQUIREMENT_BYTES: usize = 256;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContractId(String);

impl ContractId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractValidationError> {
        let value = value.into();
        validate_contract_id(&value)?;
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContractId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ContractId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContractId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

fn validate_contract_id(value: &str) -> Result<(), ContractValidationError> {
    if value.is_empty() {
        return Err(ContractValidationError::EmptyContractId);
    }
    if value.len() > MAX_CONTRACT_ID_BYTES {
        return Err(ContractValidationError::ContractIdTooLong);
    }
    if value == "*" || value.eq_ignore_ascii_case("any") {
        return Err(ContractValidationError::AnyConstraintForbidden);
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(ContractValidationError::InvalidContractIdCharacters);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostApiRequirement {
    source: String,
    requirement: VersionReq,
}

impl HostApiRequirement {
    pub fn parse(source: impl Into<String>) -> Result<Self, ContractValidationError> {
        let source = source.into();
        if source.is_empty()
            || source.len() > MAX_HOST_API_REQUIREMENT_BYTES
            || source.trim() != source
        {
            return Err(ContractValidationError::InvalidHostApiRequirement);
        }
        if source.eq_ignore_ascii_case("any") {
            return Err(ContractValidationError::AnyConstraintForbidden);
        }
        let requirement = VersionReq::parse(&source)
            .map_err(|_| ContractValidationError::InvalidHostApiRequirement)?;
        if requirement.comparators.is_empty()
            || requirement
                .comparators
                .iter()
                .any(|comparator| comparator.op == Op::Wildcard)
        {
            return Err(ContractValidationError::AnyConstraintForbidden);
        }
        Ok(Self {
            source,
            requirement,
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn matches(&self, version: &Version) -> bool {
        self.requirement.matches(version)
    }
}

impl Serialize for HostApiRequirement {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.source)
    }
}

impl<'de> Deserialize<'de> for HostApiRequirement {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = String::deserialize(deserializer)?;
        Self::parse(source).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub package_id: ContractId,
    pub version: Version,
}

impl PackageIdentity {
    #[must_use]
    pub const fn new(package_id: ContractId, version: Version) -> Self {
        Self {
            package_id,
            version,
        }
    }

    fn exact_key(&self) -> (String, String) {
        (self.package_id.0.clone(), self.version.to_string())
    }
}

fn compare_identity(left: &PackageIdentity, right: &PackageIdentity) -> Ordering {
    left.package_id
        .cmp(&right.package_id)
        .then_with(|| left.version.cmp(&right.version))
        .then_with(|| {
            left.version
                .build
                .as_str()
                .cmp(right.version.build.as_str())
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustState {
    Trusted,
    Untrusted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetState {
    Present,
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageState {
    Valid,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageReadiness {
    pub trust: TrustState,
    pub assets: AssetState,
    pub package: PackageState,
}

impl PackageReadiness {
    pub const READY: Self = Self {
        trust: TrustState::Trusted,
        assets: AssetState::Present,
        package: PackageState::Valid,
    };
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TensorAbiContract {
    pub python_implementation: ContractId,
    pub python_version: Version,
    pub torch_version: Version,
    pub dtype: ContractId,
    pub layout: ContractId,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileContract {
    pub codec_family: ContractId,
    pub profile: ContractId,
    pub profile_version: Version,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalContract {
    pub channels: u32,
    pub latent_height: u32,
    pub latent_width: u32,
    pub decoded_height: u32,
    pub decoded_width: u32,
    pub pixel_format: ContractId,
}

impl SignalContract {
    fn is_valid(&self) -> bool {
        self.channels != 0
            && self.latent_height != 0
            && self.latent_width != 0
            && self.decoded_height != 0
            && self.decoded_width != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingContract {
    pub contract: ContractId,
    pub contract_version: Version,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
}

impl TimingContract {
    fn is_valid(&self) -> bool {
        self.frame_rate_numerator != 0 && self.frame_rate_denominator != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckRequirements {
    pub protocol_version: u16,
    pub host_api: HostApiRequirement,
    pub tensor_abi: TensorAbiContract,
    pub profile: ProfileContract,
    pub signal: SignalContract,
    pub timing: TimingContract,
    pub capabilities: BTreeSet<ContractId>,
}

impl DeckRequirements {
    fn is_valid(&self) -> bool {
        self.protocol_version != 0 && self.signal.is_valid() && self.timing.is_valid()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecContract {
    pub protocol_versions: BTreeSet<u16>,
    pub host_api: HostApiRequirement,
    pub tensor_abis: BTreeSet<TensorAbiContract>,
    pub profiles: BTreeSet<ProfileContract>,
    pub signals: BTreeSet<SignalContract>,
    pub timings: BTreeSet<TimingContract>,
    pub capabilities: BTreeSet<ContractId>,
}

impl CodecContract {
    fn is_valid(&self) -> bool {
        !self.protocol_versions.is_empty()
            && !self.tensor_abis.is_empty()
            && !self.profiles.is_empty()
            && !self.signals.is_empty()
            && !self.timings.is_empty()
            && self.protocol_versions.iter().all(|version| *version != 0)
            && self.signals.iter().all(SignalContract::is_valid)
            && self.timings.iter().all(TimingContract::is_valid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckVersion {
    pub identity: PackageIdentity,
    pub readiness: PackageReadiness,
    pub requires: DeckRequirements,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecVersion {
    pub identity: PackageIdentity,
    pub readiness: PackageReadiness,
    pub provides: CodecContract,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRuntime {
    pub host_api_version: Version,
    pub protocol_versions: BTreeSet<u16>,
    pub tensor_abis: BTreeSet<TensorAbiContract>,
    pub signals: BTreeSet<SignalContract>,
    pub timings: BTreeSet<TimingContract>,
    pub capabilities: BTreeSet<ContractId>,
}

impl HostRuntime {
    pub fn validate(&self) -> Result<(), ContractValidationError> {
        if self.protocol_versions.is_empty()
            || self.tensor_abis.is_empty()
            || self.signals.is_empty()
            || self.timings.is_empty()
            || self.protocol_versions.iter().any(|version| *version == 0)
            || self.signals.iter().any(|signal| !signal.is_valid())
            || self.timings.iter().any(|timing| !timing.is_valid())
        {
            return Err(ContractValidationError::InvalidHostRuntime);
        }
        Ok(())
    }
}

/// Package-level runtime ABI shared by one Deck and one isolated Codec Pack.
///
/// The exact Torch build is negotiated between the two packages. The host
/// constrains the ABI, Python implementation/version, and platform, but does
/// not pretend that one globally installed Torch build serves every pack.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageRuntimeContract {
    pub tensor_abi: ContractId,
    pub python_implementation: ContractId,
    pub python_version: Version,
    pub python_platform: ContractId,
    /// Retained as exact text because build metadata is part of the isolated
    /// runtime identity (`+cpu` and `+cu130` are not interchangeable).
    pub torch_exact_build: String,
}

impl PackageRuntimeContract {
    fn is_valid(&self) -> bool {
        let torch = self.torch_exact_build.as_str();
        !torch.is_empty()
            && torch.len() <= 120
            && torch.trim() == torch
            && Version::parse(torch).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TensorGeometryContract {
    pub dtype: ContractId,
    pub device: ContractId,
    pub batch: u32,
    pub channels: u32,
    pub temporal: u32,
    pub height: u32,
    pub width: u32,
}

impl TensorGeometryContract {
    fn is_valid(&self) -> bool {
        self.batch != 0
            && self.channels != 0
            && self.temporal != 0
            && self.height != 0
            && self.width != 0
    }

    fn same_dtype_and_device(&self, other: &Self) -> bool {
        self.dtype == other.dtype && self.device == other.device
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameTimingContract {
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
}

impl FrameTimingContract {
    fn is_valid(&self) -> bool {
        self.frame_rate_numerator != 0 && self.frame_rate_denominator != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckTimingContract {
    pub frame: FrameTimingContract,
    pub samples_per_slot: u32,
}

impl DeckTimingContract {
    fn is_valid(&self) -> bool {
        self.frame.is_valid() && self.samples_per_slot != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckPackageRequirements {
    pub slots: u8,
    pub protocol_version: u16,
    pub app_host_api: HostApiRequirement,
    pub deck_host_api: u16,
    pub deck_operator_api: u16,
    pub runtime: PackageRuntimeContract,
    /// `None` means the Deck imposes no additional profile restriction; the
    /// finite Codec Pack profile catalog remains authoritative.
    pub profile_allowlist: Option<BTreeSet<ProfileContract>>,
    pub geometries: BTreeSet<TensorGeometryContract>,
    pub timing: DeckTimingContract,
    pub capabilities: BTreeSet<ContractId>,
}

impl DeckPackageRequirements {
    fn is_valid(&self) -> bool {
        (1..=16).contains(&self.slots)
            && self.protocol_version != 0
            && self.deck_host_api != 0
            && self.deck_operator_api != 0
            && self.runtime.is_valid()
            && !self.geometries.is_empty()
            && self.geometries.iter().all(TensorGeometryContract::is_valid)
            && self.timing.is_valid()
            && self
                .profile_allowlist
                .as_ref()
                .is_none_or(|profiles| !profiles.is_empty())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecPackageProvides {
    pub protocol_version: u16,
    pub app_host_api: HostApiRequirement,
    pub codec_adapter_api: u16,
    pub runtime: PackageRuntimeContract,
    pub lc_spec_versions: BTreeSet<Version>,
    pub profiles: BTreeSet<ProfileContract>,
    pub capabilities: BTreeSet<ContractId>,
}

impl CodecPackageProvides {
    fn is_valid(&self) -> bool {
        self.protocol_version != 0
            && self.codec_adapter_api != 0
            && self.runtime.is_valid()
            && !self.lc_spec_versions.is_empty()
            && !self.profiles.is_empty()
    }
}

/// One installed Deck candidate. Invalid or unreadable packages deliberately
/// carry no parsed contract so the resolver can still emit `package_invalid`
/// for the complete installed-version matrix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckPackageVersion {
    pub identity: PackageIdentity,
    pub readiness: PackageReadiness,
    pub requires: Option<DeckPackageRequirements>,
}

/// One installed Codec candidate; see [`DeckPackageVersion`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecPackageVersion {
    pub identity: PackageIdentity,
    pub readiness: PackageReadiness,
    pub provides: Option<CodecPackageProvides>,
}

/// Host-owned package compatibility policy. Source geometry/timing and
/// external-asset selection are intentionally absent from this first stage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageHostRuntime {
    pub app_version: Version,
    pub protocol_versions: BTreeSet<u16>,
    pub deck_host_apis: BTreeSet<u16>,
    pub deck_operator_apis: BTreeSet<u16>,
    pub codec_adapter_apis: BTreeSet<u16>,
    pub tensor_abis: BTreeSet<ContractId>,
    pub python_implementations: BTreeSet<ContractId>,
    pub python_versions: BTreeSet<Version>,
    pub python_platforms: BTreeSet<ContractId>,
    pub lc_spec_versions: BTreeSet<Version>,
    pub tensor_dtypes: BTreeSet<ContractId>,
    pub tensor_devices: BTreeSet<ContractId>,
    pub samples_per_slot: BTreeSet<u32>,
    pub capabilities: BTreeSet<ContractId>,
}

impl PackageHostRuntime {
    pub fn validate(&self) -> Result<(), ContractValidationError> {
        if self.protocol_versions.is_empty()
            || self.deck_host_apis.is_empty()
            || self.deck_operator_apis.is_empty()
            || self.codec_adapter_apis.is_empty()
            || self.tensor_abis.is_empty()
            || self.python_implementations.is_empty()
            || self.python_versions.is_empty()
            || self.python_platforms.is_empty()
            || self.lc_spec_versions.is_empty()
            || self.tensor_dtypes.is_empty()
            || self.tensor_devices.is_empty()
            || self.samples_per_slot.is_empty()
            || self.protocol_versions.contains(&0)
            || self.deck_host_apis.contains(&0)
            || self.deck_operator_apis.contains(&0)
            || self.codec_adapter_apis.contains(&0)
            || self.samples_per_slot.contains(&0)
        {
            return Err(ContractValidationError::InvalidHostRuntime);
        }
        Ok(())
    }

    fn supports_runtime(&self, runtime: &PackageRuntimeContract) -> bool {
        self.tensor_abis.contains(&runtime.tensor_abi)
            && self
                .python_implementations
                .contains(&runtime.python_implementation)
            && self.python_versions.contains(&runtime.python_version)
            && self.python_platforms.contains(&runtime.python_platform)
    }

    fn supports_geometry_runtime(&self, geometry: &TensorGeometryContract) -> bool {
        self.tensor_dtypes.contains(&geometry.dtype)
            && self.tensor_devices.contains(&geometry.device)
    }
}

/// Source facts available only after the user selects exact cartridges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedSourceContract {
    pub lc_spec_version: Version,
    pub profile: ProfileContract,
    pub geometry: TensorGeometryContract,
    pub decoded_height: u32,
    pub decoded_width: u32,
    pub timing: FrameTimingContract,
    pub timing_contract: ContractId,
    pub timing_contract_version: Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceSelectionScope {
    /// One or more indexed candidates are being described before the complete
    /// Deck slot assignment exists.
    Candidate,
    /// The exact source set supplied to runtime launch.
    CompleteSet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageCompatibilityDecision {
    pub deck: PackageIdentity,
    pub codec: PackageIdentity,
    pub reason: CompatibilityReason,
    /// Finite deterministic profile intersection. It is populated only when
    /// the package stage is compatible.
    pub compatible_profiles: BTreeSet<ProfileContract>,
}

impl PackageCompatibilityDecision {
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        matches!(self.reason, CompatibilityReason::Compatible)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompatibilityReason {
    #[serde(rename = "compatible")]
    Compatible,
    #[serde(rename = "untrusted")]
    Untrusted,
    #[serde(rename = "missing_asset")]
    MissingAsset,
    #[serde(rename = "package_invalid")]
    PackageInvalid,
    #[serde(rename = "unsupported_protocol")]
    UnsupportedProtocol,
    #[serde(rename = "unsupported_host_api")]
    UnsupportedHostApi,
    #[serde(rename = "unsupported_tensor_abi")]
    UnsupportedTensorAbi,
    #[serde(rename = "unsupported_profile")]
    UnsupportedProfile,
    #[serde(rename = "unsupported_signal")]
    UnsupportedSignal,
    #[serde(rename = "unsupported_timing")]
    UnsupportedTiming,
    #[serde(rename = "unsupported_capability")]
    UnsupportedCapability,
}

impl CompatibilityReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::Untrusted => "untrusted",
            Self::MissingAsset => "missing_asset",
            Self::PackageInvalid => "package_invalid",
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::UnsupportedHostApi => "unsupported_host_api",
            Self::UnsupportedTensorAbi => "unsupported_tensor_abi",
            Self::UnsupportedProfile => "unsupported_profile",
            Self::UnsupportedSignal => "unsupported_signal",
            Self::UnsupportedTiming => "unsupported_timing",
            Self::UnsupportedCapability => "unsupported_capability",
        }
    }
}

impl fmt::Display for CompatibilityReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub const COMPATIBILITY_REASON_PRECEDENCE: [CompatibilityReason; 11] = [
    CompatibilityReason::Untrusted,
    CompatibilityReason::MissingAsset,
    CompatibilityReason::PackageInvalid,
    CompatibilityReason::UnsupportedProtocol,
    CompatibilityReason::UnsupportedHostApi,
    CompatibilityReason::UnsupportedTensorAbi,
    CompatibilityReason::UnsupportedProfile,
    CompatibilityReason::UnsupportedSignal,
    CompatibilityReason::UnsupportedTiming,
    CompatibilityReason::UnsupportedCapability,
    CompatibilityReason::Compatible,
];

/// Production package-stage precedence. Selected facts are never allowed to
/// mask an incompatible or invalid package pair.
pub const PACKAGE_COMPATIBILITY_REASON_PRECEDENCE: [CompatibilityReason; 8] = [
    CompatibilityReason::Untrusted,
    CompatibilityReason::PackageInvalid,
    CompatibilityReason::UnsupportedProtocol,
    CompatibilityReason::UnsupportedHostApi,
    CompatibilityReason::UnsupportedTensorAbi,
    CompatibilityReason::UnsupportedProfile,
    CompatibilityReason::UnsupportedCapability,
    CompatibilityReason::Compatible,
];

/// Second-stage precedence, applied only after the package stage returned
/// `compatible`.
pub const SELECTED_COMPATIBILITY_REASON_PRECEDENCE: [CompatibilityReason; 7] = [
    CompatibilityReason::MissingAsset,
    CompatibilityReason::PackageInvalid,
    CompatibilityReason::UnsupportedProfile,
    CompatibilityReason::UnsupportedTensorAbi,
    CompatibilityReason::UnsupportedSignal,
    CompatibilityReason::UnsupportedTiming,
    CompatibilityReason::Compatible,
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityDecision {
    pub deck: PackageIdentity,
    pub codec: PackageIdentity,
    pub reason: CompatibilityReason,
}

impl CompatibilityDecision {
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        matches!(self.reason, CompatibilityReason::Compatible)
    }
}

#[derive(Clone, Debug)]
pub struct CompatibilityResolver {
    host: HostRuntime,
}

impl CompatibilityResolver {
    pub fn new(host: HostRuntime) -> Result<Self, ContractValidationError> {
        host.validate()?;
        Ok(Self { host })
    }

    #[must_use]
    pub const fn host(&self) -> &HostRuntime {
        &self.host
    }

    #[must_use]
    pub fn resolve_pair(&self, deck: &DeckVersion, codec: &CodecVersion) -> CompatibilityDecision {
        let reason = if deck.readiness.trust == TrustState::Untrusted
            || codec.readiness.trust == TrustState::Untrusted
        {
            CompatibilityReason::Untrusted
        } else if deck.readiness.assets == AssetState::Missing
            || codec.readiness.assets == AssetState::Missing
        {
            CompatibilityReason::MissingAsset
        } else if deck.readiness.package == PackageState::Invalid
            || codec.readiness.package == PackageState::Invalid
            || !deck.requires.is_valid()
            || !codec.provides.is_valid()
        {
            CompatibilityReason::PackageInvalid
        } else if !self
            .host
            .protocol_versions
            .contains(&deck.requires.protocol_version)
            || !codec
                .provides
                .protocol_versions
                .contains(&deck.requires.protocol_version)
        {
            CompatibilityReason::UnsupportedProtocol
        } else if !deck.requires.host_api.matches(&self.host.host_api_version)
            || !codec.provides.host_api.matches(&self.host.host_api_version)
        {
            CompatibilityReason::UnsupportedHostApi
        } else if !self.host.tensor_abis.contains(&deck.requires.tensor_abi)
            || !codec
                .provides
                .tensor_abis
                .contains(&deck.requires.tensor_abi)
        {
            CompatibilityReason::UnsupportedTensorAbi
        } else if !codec.provides.profiles.contains(&deck.requires.profile) {
            CompatibilityReason::UnsupportedProfile
        } else if !self.host.signals.contains(&deck.requires.signal)
            || !codec.provides.signals.contains(&deck.requires.signal)
        {
            CompatibilityReason::UnsupportedSignal
        } else if !self.host.timings.contains(&deck.requires.timing)
            || !codec.provides.timings.contains(&deck.requires.timing)
        {
            CompatibilityReason::UnsupportedTiming
        } else if !deck
            .requires
            .capabilities
            .is_subset(&self.host.capabilities)
            || !deck
                .requires
                .capabilities
                .is_subset(&codec.provides.capabilities)
        {
            CompatibilityReason::UnsupportedCapability
        } else {
            CompatibilityReason::Compatible
        };

        CompatibilityDecision {
            deck: deck.identity.clone(),
            codec: codec.identity.clone(),
            reason,
        }
    }

    /// Resolve the installed package-only stage used by the CLI, Extensions
    /// Manager, and Deck runtime discovery. External assets and cartridge
    /// signal/timing are deliberately deferred to [`Self::resolve_selected_pair`].
    pub fn resolve_package_pair(
        host: &PackageHostRuntime,
        deck: &DeckPackageVersion,
        codec: &CodecPackageVersion,
    ) -> Result<PackageCompatibilityDecision, ContractValidationError> {
        host.validate()?;
        let empty = BTreeSet::new();
        let decision = |reason, compatible_profiles| PackageCompatibilityDecision {
            deck: deck.identity.clone(),
            codec: codec.identity.clone(),
            reason,
            compatible_profiles,
        };

        if deck.readiness.trust == TrustState::Untrusted
            || codec.readiness.trust == TrustState::Untrusted
        {
            return Ok(decision(CompatibilityReason::Untrusted, empty));
        }
        let (Some(deck_contract), Some(codec_contract)) = (&deck.requires, &codec.provides) else {
            return Ok(decision(CompatibilityReason::PackageInvalid, empty));
        };
        if deck.readiness.package == PackageState::Invalid
            || codec.readiness.package == PackageState::Invalid
            || !deck_contract.is_valid()
            || !codec_contract.is_valid()
        {
            return Ok(decision(CompatibilityReason::PackageInvalid, empty));
        }
        if deck_contract.protocol_version != codec_contract.protocol_version
            || !host
                .protocol_versions
                .contains(&deck_contract.protocol_version)
        {
            return Ok(decision(CompatibilityReason::UnsupportedProtocol, empty));
        }
        if !deck_contract.app_host_api.matches(&host.app_version)
            || !codec_contract.app_host_api.matches(&host.app_version)
            || !host.deck_host_apis.contains(&deck_contract.deck_host_api)
            || !host
                .deck_operator_apis
                .contains(&deck_contract.deck_operator_api)
            || !host
                .codec_adapter_apis
                .contains(&codec_contract.codec_adapter_api)
        {
            return Ok(decision(CompatibilityReason::UnsupportedHostApi, empty));
        }
        if deck_contract.runtime != codec_contract.runtime
            || !host.supports_runtime(&deck_contract.runtime)
            || !deck_contract
                .geometries
                .iter()
                .any(|geometry| host.supports_geometry_runtime(geometry))
        {
            return Ok(decision(CompatibilityReason::UnsupportedTensorAbi, empty));
        }

        let compatible_profiles = codec_contract
            .profiles
            .iter()
            .filter(|profile| {
                deck_contract
                    .profile_allowlist
                    .as_ref()
                    .is_none_or(|allowlist| allowlist.contains(*profile))
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        if compatible_profiles.is_empty()
            || codec_contract
                .lc_spec_versions
                .is_disjoint(&host.lc_spec_versions)
        {
            return Ok(decision(CompatibilityReason::UnsupportedProfile, empty));
        }
        if !deck_contract.capabilities.is_subset(&host.capabilities)
            || !deck_contract
                .capabilities
                .is_subset(&codec_contract.capabilities)
        {
            return Ok(decision(CompatibilityReason::UnsupportedCapability, empty));
        }
        Ok(decision(
            CompatibilityReason::Compatible,
            compatible_profiles,
        ))
    }

    /// Refine one package-compatible pair with the exact user-selected asset,
    /// profile, device, and source set. No cast, resize, crop, re-encode, or
    /// equivalent-rate normalization is performed.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn resolve_selected_pair(
        host: &PackageHostRuntime,
        deck: &DeckPackageVersion,
        codec: &CodecPackageVersion,
        assets: AssetState,
        selected_profile: Option<&ProfileContract>,
        selected_device: Option<&ContractId>,
        sources: &[SelectedSourceContract],
        source_scope: SourceSelectionScope,
    ) -> Result<PackageCompatibilityDecision, ContractValidationError> {
        let mut decision = Self::resolve_package_pair(host, deck, codec)?;
        if !decision.is_compatible() {
            return Ok(decision);
        }
        if assets == AssetState::Missing {
            decision.reason = CompatibilityReason::MissingAsset;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        let (Some(profile), Some(device)) = (selected_profile, selected_device) else {
            decision.reason = CompatibilityReason::PackageInvalid;
            decision.compatible_profiles.clear();
            return Ok(decision);
        };
        if !decision.compatible_profiles.contains(profile) {
            decision.reason = CompatibilityReason::UnsupportedProfile;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        let Some(deck_contract) = deck.requires.as_ref() else {
            decision.reason = CompatibilityReason::PackageInvalid;
            decision.compatible_profiles.clear();
            return Ok(decision);
        };
        if !host.tensor_devices.contains(device)
            || !deck_contract
                .geometries
                .iter()
                .any(|geometry| &geometry.device == device)
        {
            decision.reason = CompatibilityReason::UnsupportedTensorAbi;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        if !host
            .samples_per_slot
            .contains(&deck_contract.timing.samples_per_slot)
        {
            decision.reason = CompatibilityReason::UnsupportedTiming;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        if sources.is_empty() {
            if source_scope == SourceSelectionScope::Candidate {
                return Ok(decision);
            }
            decision.reason = CompatibilityReason::UnsupportedSignal;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        if sources.len() > 16
            || sources.iter().any(|source| {
                !source.geometry.is_valid()
                    || source.decoded_height == 0
                    || source.decoded_width == 0
                    || !source.timing.is_valid()
            })
        {
            decision.reason = CompatibilityReason::PackageInvalid;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        let Some(codec_contract) = codec.provides.as_ref() else {
            decision.reason = CompatibilityReason::PackageInvalid;
            decision.compatible_profiles.clear();
            return Ok(decision);
        };
        if sources.iter().any(|source| {
            source.profile != *profile
                || !host.lc_spec_versions.contains(&source.lc_spec_version)
                || !codec_contract
                    .lc_spec_versions
                    .contains(&source.lc_spec_version)
        }) {
            decision.reason = CompatibilityReason::UnsupportedProfile;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        if source_scope == SourceSelectionScope::CompleteSet
            && sources.len() != usize::from(deck_contract.slots)
        {
            decision.reason = CompatibilityReason::UnsupportedSignal;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }

        let first = &sources[0];
        if sources.iter().any(|source| {
            source.geometry.device != *device
                || !source.geometry.same_dtype_and_device(&first.geometry)
                || !host.tensor_dtypes.contains(&source.geometry.dtype)
                || !host.tensor_devices.contains(&source.geometry.device)
                || !deck_contract
                    .geometries
                    .iter()
                    .any(|geometry| geometry.same_dtype_and_device(&source.geometry))
        }) {
            decision.reason = CompatibilityReason::UnsupportedTensorAbi;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        if sources.iter().any(|source| {
            source.geometry != first.geometry
                || source.decoded_height != first.decoded_height
                || source.decoded_width != first.decoded_width
        }) || !deck_contract.geometries.contains(&first.geometry)
        {
            decision.reason = CompatibilityReason::UnsupportedSignal;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        if sources.iter().any(|source| {
            source.timing != first.timing
                || source.timing_contract != first.timing_contract
                || source.timing_contract_version != first.timing_contract_version
        }) || deck_contract.timing.frame != first.timing
        {
            decision.reason = CompatibilityReason::UnsupportedTiming;
            decision.compatible_profiles.clear();
            return Ok(decision);
        }
        Ok(decision)
    }

    pub fn resolve_matrix(
        &self,
        decks: &[DeckVersion],
        codecs: &[CodecVersion],
    ) -> Result<Vec<CompatibilityDecision>, MatrixError> {
        reject_duplicate_identities(decks.iter().map(|item| &item.identity), true)?;
        reject_duplicate_identities(codecs.iter().map(|item| &item.identity), false)?;

        let mut decks = decks.iter().collect::<Vec<_>>();
        decks.sort_by(|left, right| compare_identity(&left.identity, &right.identity));
        let mut codecs = codecs.iter().collect::<Vec<_>>();
        codecs.sort_by(|left, right| compare_identity(&left.identity, &right.identity));

        let mut decisions = Vec::with_capacity(decks.len().saturating_mul(codecs.len()));
        for deck in decks {
            for codec in &codecs {
                decisions.push(self.resolve_pair(deck, codec));
            }
        }
        Ok(decisions)
    }
}

fn reject_duplicate_identities<'a>(
    identities: impl Iterator<Item = &'a PackageIdentity>,
    deck: bool,
) -> Result<(), MatrixError> {
    let mut seen = BTreeSet::new();
    for identity in identities {
        if !seen.insert(identity.exact_key()) {
            return Err(if deck {
                MatrixError::DuplicateDeck(identity.clone())
            } else {
                MatrixError::DuplicateCodec(identity.clone())
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ContractValidationError {
    #[error("contract id must not be empty")]
    EmptyContractId,
    #[error("contract id exceeds {MAX_CONTRACT_ID_BYTES} UTF-8 bytes")]
    ContractIdTooLong,
    #[error("contract id contains a character outside ASCII alphanumeric, '.', '_', ':', '-'")]
    InvalidContractIdCharacters,
    #[error("wildcard and 'any' constraints are forbidden")]
    AnyConstraintForbidden,
    #[error("host API requirement is invalid")]
    InvalidHostApiRequirement,
    #[error("host runtime has an empty or invalid explicit constraint set")]
    InvalidHostRuntime,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum MatrixError {
    #[error("duplicate deck identity {0:?}")]
    DuplicateDeck(PackageIdentity),
    #[error("duplicate codec identity {0:?}")]
    DuplicateCodec(PackageIdentity),
}
