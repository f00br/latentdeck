use std::{fs::File, path::PathBuf, sync::Arc};

pub use latentdeck_deck_runtime_contracts::CompatibilityReason;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    DeckPack,
    CodecPack,
}

impl PackageKind {
    #[must_use]
    pub const fn archive_extension(self) -> &'static str {
        match self {
            Self::DeckPack => "ld",
            Self::CodecPack => "ldcodec",
        }
    }

    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::DeckPack => "deck-pack.json",
            Self::CodecPack => "codec-pack.json",
        }
    }

    #[must_use]
    pub const fn root_name(self) -> &'static str {
        match self {
            Self::DeckPack => "Decks",
            Self::CodecPack => "CodecPacks",
        }
    }

    #[must_use]
    pub const fn receipt_root_name(self) -> &'static str {
        match self {
            Self::DeckPack => "decks",
            Self::CodecPack => "codecs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageReference {
    pub kind: PackageKind,
    pub package_id: String,
    pub package_version: String,
}

/// One exact official package archive authorized by a build-generated index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledPackageEntry {
    pub package: PackageReference,
    pub archive_sha256: String,
}

/// Closed build-generated authorization for the reserved
/// `org.latentdeck.*` namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundledPackageIndex {
    pub index_version: String,
    pub packages: Vec<BundledPackageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublisherIdentityClaim {
    SelfDeclared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherDescriptor {
    pub name: String,
    pub url: Option<String>,
    pub identity_claim: PublisherIdentityClaim,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseDescriptor {
    pub spdx_or_label: String,
    pub notice_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileKey {
    pub codec_family: String,
    pub profile: String,
    pub profile_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PythonConstraint {
    pub implementation: PythonImplementation,
    pub version: String,
    pub platform_tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PythonImplementation {
    Cpython,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecCapability {
    Player,
    Realtime,
    Resample,
    SnapshotCapture,
    LiveCapture,
    RawImport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TensorDtype {
    Fp16,
    Fp32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TensorDevice {
    Cpu,
    Cuda,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalGeometry {
    pub dtype: TensorDtype,
    pub device: TensorDevice,
    pub batch: u8,
    pub channels: u16,
    pub temporal: u8,
    pub height: u32,
    pub width: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingDescriptor {
    pub frames_per_second_numerator: u32,
    pub frames_per_second_denominator: u32,
    pub samples_per_slot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityDescriptor {
    pub catalog_path: String,
    pub catalog_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityCatalog {
    pub manifest_version: String,
    pub files: Vec<IntegrityFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrityFile {
    pub path: String,
    pub byte_length: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckCompatibility {
    pub app_min_inclusive: String,
    pub app_max_exclusive: String,
    pub deck_host_api: u16,
    pub worker_protocol: u16,
    pub deck_operator_api: u16,
    pub tensor_abi: String,
    pub python: PythonConstraint,
    pub torch_exact_build: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeckRuntimeKind {
    PythonOperatorStreamV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckRuntimeDescriptor {
    pub kind: DeckRuntimeKind,
    pub operator_descriptor_path: String,
    pub python_root: String,
    pub entrypoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckRoleDescriptor {
    pub role_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckSignalDescriptor {
    pub slots: u8,
    pub roles: Vec<DeckRoleDescriptor>,
    pub default_permutation: Vec<String>,
    pub structural_carrier_role: String,
    pub geometry_allowlist: Vec<SignalGeometry>,
    pub timing: TimingDescriptor,
    pub required_capabilities: Vec<CodecCapability>,
    pub profile_allowlist: Option<Vec<ProfileKey>>,
}

/// Closed `deck-pack.json` version 1 contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeckPackManifest {
    pub manifest_version: String,
    pub kind: PackageKind,
    pub deck_id: String,
    pub deck_version: String,
    pub display_name: String,
    pub summary: String,
    pub publisher: PublisherDescriptor,
    pub license: LicenseDescriptor,
    pub compatibility: DeckCompatibility,
    pub runtime: DeckRuntimeDescriptor,
    pub signal: DeckSignalDescriptor,
    pub faceplate_path: String,
    pub integrity: IntegrityDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformDescriptor {
    pub os: OperatingSystem,
    pub arch: Architecture,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingSystem {
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    X86_64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecCompatibility {
    pub app_min_inclusive: String,
    pub app_max_exclusive: String,
    pub worker_protocol: u16,
    pub codec_adapter_api: u16,
    pub tensor_abi: String,
    pub python: PythonConstraint,
    pub torch_exact_build: String,
    pub lc_spec_versions: Vec<String>,
    /// Profile identities implemented by this adapter. Exact cartridge
    /// geometry and timing are bound later by the adapter's `ProfileReceipt`;
    /// a `ProfileKey` may intentionally cover more than one signal extent.
    pub profiles: Vec<ProfileKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecAdapterDescriptor {
    pub adapter_id: String,
    pub adapter_version: String,
    pub entrypoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecWorkerDescriptor {
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub start_timeout_ms: u32,
    pub heartbeat_timeout_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAssetDescriptor {
    pub asset_id: String,
    pub display_name: String,
    pub required: bool,
    pub byte_length: u64,
    pub sha256: String,
    pub source_url: Option<String>,
    pub license_label: String,
    pub license_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLockDescriptor {
    pub path: String,
    pub sha256: String,
}

/// Closed `codec-pack.json` version 2 contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodecPackManifest {
    pub manifest_version: String,
    pub kind: PackageKind,
    pub pack_id: String,
    pub pack_version: String,
    pub display_name: String,
    pub summary: String,
    pub publisher: PublisherDescriptor,
    pub license: LicenseDescriptor,
    pub platform: PlatformDescriptor,
    pub compatibility: CodecCompatibility,
    pub adapter: CodecAdapterDescriptor,
    pub worker: CodecWorkerDescriptor,
    pub capabilities: Vec<CodecCapability>,
    pub external_assets: Vec<ExternalAssetDescriptor>,
    pub runtime_lock: RuntimeLockDescriptor,
    pub integrity: IntegrityDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PackageManifest {
    Deck(DeckPackManifest),
    Codec(CodecPackManifest),
}

impl PackageManifest {
    #[must_use]
    pub const fn kind(&self) -> PackageKind {
        match self {
            Self::Deck(_) => PackageKind::DeckPack,
            Self::Codec(_) => PackageKind::CodecPack,
        }
    }

    #[must_use]
    pub fn package_id(&self) -> &str {
        match self {
            Self::Deck(manifest) => &manifest.deck_id,
            Self::Codec(manifest) => &manifest.pack_id,
        }
    }

    #[must_use]
    pub fn package_version(&self) -> &str {
        match self {
            Self::Deck(manifest) => &manifest.deck_version,
            Self::Codec(manifest) => &manifest.pack_version,
        }
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        match self {
            Self::Deck(manifest) => &manifest.display_name,
            Self::Codec(manifest) => &manifest.display_name,
        }
    }

    #[must_use]
    pub const fn publisher(&self) -> &PublisherDescriptor {
        match self {
            Self::Deck(manifest) => &manifest.publisher,
            Self::Codec(manifest) => &manifest.publisher,
        }
    }

    #[must_use]
    pub const fn license(&self) -> &LicenseDescriptor {
        match self {
            Self::Deck(manifest) => &manifest.license,
            Self::Codec(manifest) => &manifest.license,
        }
    }

    #[must_use]
    pub const fn integrity(&self) -> &IntegrityDescriptor {
        match self {
            Self::Deck(manifest) => &manifest.integrity,
            Self::Codec(manifest) => &manifest.integrity,
        }
    }

    #[must_use]
    pub fn reference(&self) -> PackageReference {
        PackageReference {
            kind: self.kind(),
            package_id: self.package_id().to_owned(),
            package_version: self.package_version().to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustReceipt {
    pub receipt_version: String,
    pub package: PackageReference,
    pub archive_sha256: String,
    pub archive_byte_length: u64,
    pub manifest_sha256: String,
    pub integrity_catalog_sha256: String,
    pub publisher_name: String,
    pub publisher_identity_claim: PublisherIdentityClaim,
    pub installed_at_utc: String,
    pub enabled: bool,
    /// Optional pre-public Protocol 2 migration field. Current binaries accept
    /// legacy receipts without it; binaries predating this closed-schema field
    /// may reject a migrated receipt and are not a supported downgrade path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_seal_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InspectedPackage {
    pub package: PackageReference,
    pub display_name: String,
    pub publisher_name: String,
    pub publisher_identity_claim: PublisherIdentityClaim,
    pub archive_sha256: String,
    pub archive_byte_length: u64,
    pub manifest_sha256: String,
    pub integrity_catalog_sha256: String,
    pub file_count: usize,
    pub extracted_byte_length: u64,
    pub manifest: PackageManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackReceipt {
    pub output_path: PathBuf,
    /// Exact sorted archive paths included by the closed-tree packer.
    pub included_files: Vec<String>,
    pub inspection: InspectedPackage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstallReceipt {
    pub destination: PathBuf,
    pub trust_receipt_path: PathBuf,
    pub inspection: InspectedPackage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageHealth {
    Healthy,
    VerificationRequired,
    Corrupt,
    Untrusted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledPackageSummary {
    pub package: PackageReference,
    pub display_name: Option<String>,
    pub publisher_name: Option<String>,
    pub enabled: bool,
    pub health: PackageHealth,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
}

/// One bounded, internally consistent inventory pass over installed packages
/// and every Deck-version by Codec-version compatibility pair. Each healthy
/// tree is validated once and the matrix is derived from those same results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInventory {
    pub packages: Vec<InstalledPackageSummary>,
    pub matrix: Vec<CompatibilityPair>,
}

/// One exact installed tree revalidated against its atomic trust receipt.
///
/// Construction is restricted to the lifecycle module so callers cannot turn
/// an arbitrary directory or manifest into a trusted runtime input.
#[derive(Debug, Clone)]
pub struct ValidatedInstalledPackage {
    root: PathBuf,
    manifest: PackageManifest,
    trust_receipt: TrustReceipt,
}

impl ValidatedInstalledPackage {
    pub(crate) fn new(
        root: PathBuf,
        manifest: PackageManifest,
        trust_receipt: TrustReceipt,
    ) -> Self {
        Self {
            root,
            manifest,
            trust_receipt,
        }
    }

    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    #[must_use]
    pub const fn manifest(&self) -> &PackageManifest {
        &self.manifest
    }

    #[must_use]
    pub const fn trust_receipt(&self) -> &TrustReceipt {
        &self.trust_receipt
    }
}

/// An enabled, revalidated exact package version with a live shared usage
/// lease and retained handles for its validated tree. On Windows those handles
/// permit concurrent reads while denying write/delete sharing for the existing
/// validated paths until this value is dropped. This is not an OS sandbox and
/// does not prevent the same user from creating a new child name; subsequent
/// closed-tree validation detects that addition.
#[derive(Debug, Clone)]
pub struct ActiveInstalledPackage {
    inner: Arc<ActiveInstalledPackageInner>,
}

#[derive(Debug)]
struct ActiveInstalledPackageInner {
    package: ValidatedInstalledPackage,
    expected_files: std::collections::BTreeMap<String, crate::archive::FileMeasurement>,
    full_hash_passes: u64,
    _usage_lock: File,
    retained_tree_handles: Vec<File>,
}

impl ActiveInstalledPackage {
    pub(crate) fn new(
        package: ValidatedInstalledPackage,
        expected_files: std::collections::BTreeMap<String, crate::archive::FileMeasurement>,
        full_hash_passes: u64,
        usage_lock: File,
        retained_tree_handles: Vec<File>,
    ) -> Self {
        Self {
            inner: Arc::new(ActiveInstalledPackageInner {
                package,
                expected_files,
                full_hash_passes,
                _usage_lock: usage_lock,
                retained_tree_handles,
            }),
        }
    }

    #[must_use]
    pub fn package(&self) -> &ValidatedInstalledPackage {
        &self.inner.package
    }

    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        self.inner.package.root()
    }

    #[must_use]
    pub fn manifest(&self) -> &PackageManifest {
        self.inner.package.manifest()
    }

    #[must_use]
    pub fn trust_receipt(&self) -> &TrustReceipt {
        self.inner.package.trust_receipt()
    }

    pub(crate) fn expected_files(
        &self,
    ) -> &std::collections::BTreeMap<String, crate::archive::FileMeasurement> {
        &self.inner.expected_files
    }

    pub(crate) fn full_hash_passes(&self) -> u64 {
        self.inner.full_hash_passes
    }

    pub(crate) fn retained_handle_count(&self) -> usize {
        self.inner.retained_tree_handles.len().saturating_add(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompatibilityPair {
    pub deck: PackageReference,
    pub codec: PackageReference,
    pub reason: CompatibilityReason,
    pub compatible_profiles: Vec<ProfileKey>,
    /// Backward-compatible deterministic witness for older UI clients. New
    /// consumers must use the complete `compatible_profiles` intersection.
    pub compatible_profile: Option<ProfileKey>,
}
