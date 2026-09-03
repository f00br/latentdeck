#![deny(unsafe_code)]
#![allow(clippy::missing_errors_doc)]

mod broker;
mod compatibility;

pub use broker::{
    BrokerError, ForegroundLease, MAX_WARM_SESSIONS, OutputPinKind, OutputPinToken, SessionBroker,
    SessionId, SessionSnapshot, WarmSession, WorkerId,
};
pub use compatibility::{
    AssetState, COMPATIBILITY_REASON_PRECEDENCE, CodecContract, CodecPackageProvides,
    CodecPackageVersion, CodecVersion, CompatibilityDecision, CompatibilityReason,
    CompatibilityResolver, ContractId, ContractValidationError, DeckPackageRequirements,
    DeckPackageVersion, DeckRequirements, DeckTimingContract, DeckVersion, FrameTimingContract,
    HostApiRequirement, HostRuntime, MatrixError, PACKAGE_COMPATIBILITY_REASON_PRECEDENCE,
    PackageCompatibilityDecision, PackageHostRuntime, PackageIdentity, PackageReadiness,
    PackageRuntimeContract, PackageState, ProfileContract,
    SELECTED_COMPATIBILITY_REASON_PRECEDENCE, SelectedSourceContract, SignalContract,
    SourceSelectionScope, TensorAbiContract, TensorGeometryContract, TimingContract, TrustState,
};
