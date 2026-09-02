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
