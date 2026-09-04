use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::PathBuf,
};

use latentdeck_extension_manager::{
    ActivePackageCache, CodecCapability, CompatibilityPair, CompatibilityReason, DeckPackManifest,
    ErrorCode as ExtensionErrorCode, ExtensionError, ExtensionRoots, InstallRequest,
    InstalledPackageSummary, PackageHealth, PackageKind, PackageReference, PublisherIdentityClaim,
    RemoveOptions, inspect as inspect_extension_archive, install, remove, repair, verify,
};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::library_state::CommandError;

#[derive(Debug)]
pub(crate) struct ExtensionManagerState {
    roots: ExtensionRoots,
    active_packages: ActivePackageCache,
}

impl ExtensionManagerState {
    pub(crate) fn new(roots: ExtensionRoots) -> Self {
        Self {
            roots,
            active_packages: ActivePackageCache::new(),
        }
    }

    pub(crate) const fn roots(&self) -> &ExtensionRoots {
        &self.roots
    }

    pub(crate) const fn active_packages(&self) -> &ActivePackageCache {
        &self.active_packages
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExtensionPackageRequest {
    kind: PackageKind,
    package_id: String,
    package_version: String,
}

impl ExtensionPackageRequest {
    fn into_reference(self) -> PackageReference {
        PackageReference {
            kind: self.kind,
            package_id: self.package_id,
            package_version: self.package_version,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionPackageReferenceView {
    kind: PackageKind,
    package_id: String,
    package_version: String,
}

impl From<&PackageReference> for ExtensionPackageReferenceView {
    fn from(value: &PackageReference) -> Self {
        Self {
            kind: value.kind,
            package_id: value.package_id.clone(),
            package_version: value.package_version.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionPackageSummaryView {
    package: ExtensionPackageReferenceView,
    display_name: Option<String>,
    publisher_name: Option<String>,
    enabled: bool,
    health: PackageHealth,
    error_code: Option<String>,
    error_detail: Option<String>,
}

impl From<InstalledPackageSummary> for ExtensionPackageSummaryView {
    fn from(value: InstalledPackageSummary) -> Self {
        // The lifecycle error detail may contain an OS path. The host UI receives
        // only stable, path-free guidance; detailed evidence remains in local logs.
        let error_detail = value.error_detail.as_ref().map(|_| match value.health {
            PackageHealth::Healthy => "The exact package version is healthy.".to_owned(),
            PackageHealth::VerificationRequired => {
                "The disabled exact package needs full payload verification before enable or use."
                    .to_owned()
            }
            PackageHealth::Corrupt => {
                "The installed package tree is corrupt; verify or repair this exact version."
                    .to_owned()
            }
            PackageHealth::Untrusted => {
                "The exact package version has no valid hash-bound trust receipt.".to_owned()
            }
        });
        Self {
            package: (&value.package).into(),
            display_name: value.display_name,
            publisher_name: value.publisher_name,
            enabled: value.enabled,
            health: value.health,
            error_code: value.error_code,
            error_detail,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionProfileView {
    codec_family: String,
    profile: String,
    profile_version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionCompatibilityPairView {
    deck: ExtensionPackageReferenceView,
    codec: ExtensionPackageReferenceView,
    reason: CompatibilityReason,
    compatible_profiles: Vec<ExtensionProfileView>,
    compatible_profile: Option<ExtensionProfileView>,
}

impl From<CompatibilityPair> for ExtensionCompatibilityPairView {
    fn from(value: CompatibilityPair) -> Self {
        Self {
            deck: (&value.deck).into(),
            codec: (&value.codec).into(),
            reason: value.reason,
            compatible_profiles: value
                .compatible_profiles
                .into_iter()
                .map(|profile| ExtensionProfileView {
                    codec_family: profile.codec_family,
                    profile: profile.profile,
                    profile_version: profile.profile_version,
                })
                .collect(),
            compatible_profile: value
                .compatible_profile
                .map(|profile| ExtensionProfileView {
                    codec_family: profile.codec_family,
                    profile: profile.profile,
                    profile_version: profile.profile_version,
                }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExtensionManagerSnapshot {
    packages: Vec<ExtensionPackageSummaryView>,
    matrix: Vec<ExtensionCompatibilityPairView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InspectedExtensionView {
    package: ExtensionPackageReferenceView,
    display_name: String,
    publisher_name: String,
    publisher_identity_claim: PublisherIdentityClaim,
    archive_sha256: String,
    archive_byte_length: u64,
    file_count: usize,
    extracted_byte_length: u64,
}

const MAX_DECK_UI_JSON_BYTES: u64 = 1_048_576;
const MAX_DECK_UI_CONTROLS: usize = 128;
const MAX_DECK_UI_SECTIONS: usize = 16;
const MAX_DECK_UI_WIDGETS: usize = 128;
const MAX_DECK_UI_VISIBILITY_PREDICATES: usize = 8;
const MAX_DECK_UI_VISIBILITY_VALUES: usize = 16;
const MAX_DECK_UI_CATALOG_ENTRIES: usize = 256;
const MAX_DECK_UI_CATALOG_JSON_BYTES: u64 = 16 * 1_048_576;
const MAX_JS_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeckUiCatalogView {
    decks: Vec<DeckUiPackageView>,
    issues: Vec<DeckUiCatalogIssueView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckUiCatalogIssueView {
    package: ExtensionPackageReferenceView,
    code: String,
    detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckUiPackageView {
    package: ExtensionPackageReferenceView,
    deck: DeckUiDeckView,
    operator: DeckUiOperatorView,
    faceplate: DeckFaceplateDescriptor,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckUiDeckView {
    deck_id: String,
    deck_version: String,
    display_name: String,
    summary: String,
    slots: u8,
    roles: Vec<DeckUiRoleView>,
    default_permutation: Vec<String>,
    structural_carrier_role: String,
    required_capabilities: Vec<CodecCapability>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckUiRoleView {
    role_id: String,
    display_name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckUiOperatorView {
    operator_id: String,
    controls: Vec<DeckControlDescriptor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeckOperatorDescriptor {
    schema_version: String,
    deck_operator_api: String,
    deck_id: String,
    deck_version: String,
    operator_id: String,
    operator_version: String,
    entrypoint: String,
    source_count: u8,
    role_ids: Vec<String>,
    controls: Vec<DeckControlDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "value_type", rename_all = "snake_case", deny_unknown_fields)]
enum DeckControlDescriptor {
    Number {
        control_id: String,
        default: f64,
        minimum: f64,
        maximum: f64,
        step: f64,
    },
    Integer {
        control_id: String,
        default: i64,
        minimum: i64,
        maximum: i64,
        step: u64,
    },
    Boolean {
        control_id: String,
        default: bool,
    },
    Enum {
        control_id: String,
        default: String,
        options: Vec<String>,
    },
}

impl DeckControlDescriptor {
    fn control_id(&self) -> &str {
        match self {
            Self::Number { control_id, .. }
            | Self::Integer { control_id, .. }
            | Self::Boolean { control_id, .. }
            | Self::Enum { control_id, .. } => control_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeckFaceplateDescriptor {
    schema_version: u16,
    title: String,
    sections: Vec<DeckFaceplateSection>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeckFaceplateSection {
    section_id: String,
    title: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    region: Option<DeckFaceplateSectionRegion>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    columns: Option<u8>,
    widgets: Vec<DeckFaceplateWidget>,
}

fn deserialize_present_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeckFaceplateSectionRegion {
    Output,
    Actions,
    Controls,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeckFaceplateVisibilityPredicate {
    control_id: String,
    one_of: Vec<DeckFaceplateVisibilityValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(untagged)]
enum DeckFaceplateVisibilityValue {
    Text(String),
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum DeckFaceplateWidget {
    SourcePicker {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        slot_index: u8,
    },
    Slider {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        control_id: String,
        minimum: f64,
        maximum: f64,
        step: f64,
    },
    Number {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        control_id: String,
        minimum: f64,
        maximum: f64,
        step: f64,
    },
    Toggle {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        control_id: String,
    },
    Select {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        control_id: String,
        options: Vec<DeckFaceplateOption>,
    },
    RoleEditor {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        role_ids: Vec<String>,
    },
    Barycentric3 {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        x_control_id: String,
        y_control_id: String,
        vertex_role_ids: [String; 3],
    },
    Transport {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        slot_indices: Vec<u8>,
    },
    Seed {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
    },
    Capture {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
        modes: Vec<DeckFaceplateCaptureMode>,
    },
    Monitor {
        id: String,
        label: String,
        #[serde(
            default,
            deserialize_with = "deserialize_present_non_null",
            skip_serializing_if = "Option::is_none"
        )]
        visible_when: Option<Vec<DeckFaceplateVisibilityPredicate>>,
    },
}

impl DeckFaceplateWidget {
    fn visible_when(&self) -> Option<&[DeckFaceplateVisibilityPredicate]> {
        match self {
            Self::SourcePicker { visible_when, .. }
            | Self::Slider { visible_when, .. }
            | Self::Number { visible_when, .. }
            | Self::Toggle { visible_when, .. }
            | Self::Select { visible_when, .. }
            | Self::RoleEditor { visible_when, .. }
            | Self::Barycentric3 { visible_when, .. }
            | Self::Transport { visible_when, .. }
            | Self::Seed { visible_when, .. }
            | Self::Capture { visible_when, .. }
            | Self::Monitor { visible_when, .. } => visible_when.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeckFaceplateOption {
    value: String,
    label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DeckFaceplateCaptureMode {
    Snapshot,
    LiveCapture,
}

fn extension_snapshot_for(
    roots: &ExtensionRoots,
    active_packages: &ActivePackageCache,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let inventory = active_packages
        .runtime_inventory(roots)
        .map_err(extension_command_error)?;
    let packages = inventory.packages.into_iter().map(Into::into).collect();
    let matrix = inventory.matrix.into_iter().map(Into::into).collect();
    Ok(ExtensionManagerSnapshot { packages, matrix })
}

#[allow(clippy::needless_pass_by_value)]
fn extension_command_error(error: ExtensionError) -> CommandError {
    let message = match error.code() {
        ExtensionErrorCode::InvalidArguments => {
            "The extension request or exact package identity is invalid."
        }
        ExtensionErrorCode::ArchiveInvalid => {
            "The local package archive is malformed or violates its bounded format."
        }
        ExtensionErrorCode::ManifestInvalid => {
            "The package manifest or declared compatibility contract is invalid."
        }
        ExtensionErrorCode::IntegrityFailed => {
            "The package bytes no longer match their integrity catalog or trust receipt."
        }
        ExtensionErrorCode::PackageExists => {
            "That exact immutable package version is already installed."
        }
        ExtensionErrorCode::PackageMissing => "That exact package version is not installed.",
        ExtensionErrorCode::PackageActive => {
            "Disable and close every session using this exact package version first."
        }
        ExtensionErrorCode::PackageDisabled => "Enable that exact package version before using it.",
        ExtensionErrorCode::PackageUntrusted => {
            "The package is not authorized by an exact local hash-bound trust receipt."
        }
        ExtensionErrorCode::LifecycleBusy => {
            "Another extension lifecycle operation is still in progress."
        }
        ExtensionErrorCode::LifecycleConflict => {
            "The extension tree changed during the operation; refresh and try again."
        }
        ExtensionErrorCode::Io => {
            "The local extension operation could not access its bounded storage."
        }
    };
    CommandError::new(error.code().as_str(), message)
}

fn extension_task_failed() -> CommandError {
    CommandError::new(
        "extension.task_failed",
        "The local extension operation stopped unexpectedly; refresh its state before retrying.",
    )
}

fn inspected_extension_view(
    value: latentdeck_extension_manager::InspectedPackage,
) -> InspectedExtensionView {
    InspectedExtensionView {
        package: (&value.package).into(),
        display_name: value.display_name,
        publisher_name: value.publisher_name,
        publisher_identity_claim: value.publisher_identity_claim,
        archive_sha256: value.archive_sha256,
        archive_byte_length: value.archive_byte_length,
        file_count: value.file_count,
        extracted_byte_length: value.extracted_byte_length,
    }
}

fn deck_ui_catalog_for(
    roots: &ExtensionRoots,
    active_packages: &ActivePackageCache,
) -> Result<DeckUiCatalogView, CommandError> {
    let mut decks = Vec::new();
    let mut issues = Vec::new();
    let candidates = active_packages
        .runtime_list_kind(roots, PackageKind::DeckPack)
        .map_err(extension_command_error)?
        .into_iter()
        .filter(|summary| summary.enabled && summary.health == PackageHealth::Healthy)
        .collect::<Vec<_>>();
    if candidates.len() > MAX_DECK_UI_CATALOG_ENTRIES {
        return Err(deck_ui_catalog_limit());
    }
    let mut remaining_json_bytes = MAX_DECK_UI_CATALOG_JSON_BYTES;
    for summary in candidates {
        let package = summary.package;
        match deck_ui_package_from_active(
            roots,
            active_packages,
            &package,
            &mut remaining_json_bytes,
        ) {
            Ok(deck) => decks.push(deck),
            Err(DeckUiPackageLoadError::Package) => issues.push(DeckUiCatalogIssueView {
                package: (&package).into(),
                code: "deck_ui.package_invalid".to_owned(),
                detail: "The exact Deck UI contract is invalid; verify or repair this version."
                    .to_owned(),
            }),
            Err(DeckUiPackageLoadError::CatalogLimit) => return Err(deck_ui_catalog_limit()),
        }
    }
    decks.sort_by(|left, right| {
        left.deck
            .deck_id
            .cmp(&right.deck.deck_id)
            .then_with(|| left.deck.deck_version.cmp(&right.deck.deck_version))
    });
    issues.sort_by(|left, right| {
        left.package
            .package_id
            .cmp(&right.package.package_id)
            .then_with(|| {
                left.package
                    .package_version
                    .cmp(&right.package.package_version)
            })
    });
    Ok(DeckUiCatalogView { decks, issues })
}

#[derive(Debug)]
enum DeckUiPackageLoadError {
    Package,
    CatalogLimit,
}

fn deck_ui_package_from_active(
    roots: &ExtensionRoots,
    active_packages: &ActivePackageCache,
    package: &PackageReference,
    remaining_json_bytes: &mut u64,
) -> Result<DeckUiPackageView, DeckUiPackageLoadError> {
    let active = active_packages
        .resolve_active(roots, package)
        .map_err(extension_command_error)
        .map_err(|_| DeckUiPackageLoadError::Package)?;
    let latentdeck_extension_manager::PackageManifest::Deck(manifest) = active.manifest() else {
        return Err(DeckUiPackageLoadError::Package);
    };
    let operator =
        read_catalog_deck_ui_json(active.root().join("operator.json"), remaining_json_bytes)?;
    let faceplate =
        read_catalog_deck_ui_json(active.root().join("faceplate.json"), remaining_json_bytes)?;
    deck_ui_view_from_parts(package, manifest, &operator, &faceplate)
        .map_err(|_| DeckUiPackageLoadError::Package)
}

fn read_catalog_deck_ui_json(
    path: PathBuf,
    remaining_json_bytes: &mut u64,
) -> Result<Vec<u8>, DeckUiPackageLoadError> {
    let bytes = read_bounded_deck_ui_json(path).map_err(|_| DeckUiPackageLoadError::Package)?;
    let length = u64::try_from(bytes.len()).map_err(|_| DeckUiPackageLoadError::CatalogLimit)?;
    if length > *remaining_json_bytes {
        return Err(DeckUiPackageLoadError::CatalogLimit);
    }
    *remaining_json_bytes -= length;
    Ok(bytes)
}

fn read_bounded_deck_ui_json(path: PathBuf) -> Result<Vec<u8>, CommandError> {
    let file = File::open(path)
        .map_err(|_| deck_ui_invalid("The validated Deck UI file could not be opened."))?;
    let length = file
        .metadata()
        .map_err(|_| deck_ui_invalid("The validated Deck UI file could not be measured."))?
        .len();
    if length == 0 || length > MAX_DECK_UI_JSON_BYTES {
        return Err(deck_ui_invalid(
            "The validated Deck UI file is empty or exceeds its byte limit.",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
    file.take(MAX_DECK_UI_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| deck_ui_invalid("The validated Deck UI file could not be read."))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != length {
        return Err(deck_ui_invalid(
            "The validated Deck UI file changed while it was being read.",
        ));
    }
    Ok(bytes)
}

fn deck_ui_view_from_parts(
    package: &PackageReference,
    manifest: &DeckPackManifest,
    operator_json: &[u8],
    faceplate_json: &[u8],
) -> Result<DeckUiPackageView, CommandError> {
    if package.kind != PackageKind::DeckPack
        || package.package_id != manifest.deck_id
        || package.package_version != manifest.deck_version
    {
        return Err(deck_ui_invalid(
            "The exact package and Deck manifest identities differ.",
        ));
    }
    let operator = serde_json::from_slice::<DeckOperatorDescriptor>(operator_json)
        .map_err(|_| deck_ui_invalid("operator.json is not a closed Deck UI descriptor."))?;
    let faceplate = serde_json::from_slice::<DeckFaceplateDescriptor>(faceplate_json)
        .map_err(|_| deck_ui_invalid("faceplate.json is not a closed host-rendered descriptor."))?;
    validate_deck_operator(manifest, &operator)?;
    validate_deck_faceplate(manifest, &operator, &faceplate)?;
    Ok(DeckUiPackageView {
        package: package.into(),
        deck: DeckUiDeckView {
            deck_id: manifest.deck_id.clone(),
            deck_version: manifest.deck_version.clone(),
            display_name: manifest.display_name.clone(),
            summary: manifest.summary.clone(),
            slots: manifest.signal.slots,
            roles: manifest
                .signal
                .roles
                .iter()
                .map(|role| DeckUiRoleView {
                    role_id: role.role_id.clone(),
                    display_name: role.display_name.clone(),
                })
                .collect(),
            default_permutation: manifest.signal.default_permutation.clone(),
            structural_carrier_role: manifest.signal.structural_carrier_role.clone(),
            required_capabilities: manifest.signal.required_capabilities.clone(),
        },
        operator: DeckUiOperatorView {
            operator_id: operator.operator_id,
            controls: operator.controls,
        },
        faceplate,
    })
}

fn validate_deck_operator(
    manifest: &DeckPackManifest,
    operator: &DeckOperatorDescriptor,
) -> Result<(), CommandError> {
    if operator.schema_version != "0.2.0"
        || operator.deck_operator_api != "0.2.0"
        || operator.deck_id != manifest.deck_id
        || operator.deck_version != manifest.deck_version
        || operator.source_count != manifest.signal.slots
        || operator.role_ids
            != manifest
                .signal
                .roles
                .iter()
                .map(|role| role.role_id.clone())
                .collect::<Vec<_>>()
        || operator.entrypoint != manifest.runtime.entrypoint
        || semver::Version::parse(&operator.operator_version).is_err()
        || !is_deck_ui_identifier(&operator.operator_id)
        || operator.controls.is_empty()
        || operator.controls.len() > MAX_DECK_UI_CONTROLS
    {
        return Err(deck_ui_invalid(
            "operator.json does not match the exact Deck package contract.",
        ));
    }
    let mut ids = BTreeSet::new();
    for control in &operator.controls {
        let id = control.control_id();
        if !is_deck_ui_identifier(id) || !ids.insert(id) || !valid_control(control) {
            return Err(deck_ui_invalid(
                "operator.json contains an invalid or duplicate typed control.",
            ));
        }
    }
    Ok(())
}

fn valid_control(control: &DeckControlDescriptor) -> bool {
    match control {
        DeckControlDescriptor::Number {
            default,
            minimum,
            maximum,
            step,
            ..
        } => {
            default.is_finite()
                && minimum.is_finite()
                && maximum.is_finite()
                && step.is_finite()
                && minimum < maximum
                && *step > 0.0
                && default >= minimum
                && default <= maximum
        }
        DeckControlDescriptor::Integer {
            default,
            minimum,
            maximum,
            step,
            ..
        } => {
            minimum < maximum
                && *step > 0
                && default >= minimum
                && default <= maximum
                && *minimum >= -MAX_JS_SAFE_INTEGER
                && *maximum <= MAX_JS_SAFE_INTEGER
                && *default >= -MAX_JS_SAFE_INTEGER
                && *default <= MAX_JS_SAFE_INTEGER
                && *step <= MAX_JS_SAFE_INTEGER as u64
        }
        DeckControlDescriptor::Boolean { .. } => true,
        DeckControlDescriptor::Enum {
            default, options, ..
        } => {
            !options.is_empty()
                && options.len() <= 64
                && options.iter().all(|value| is_deck_ui_identifier(value))
                && options.iter().collect::<BTreeSet<_>>().len() == options.len()
                && options.contains(default)
        }
    }
}

#[allow(clippy::float_cmp, clippy::too_many_lines)]
fn validate_deck_faceplate(
    manifest: &DeckPackManifest,
    operator: &DeckOperatorDescriptor,
    faceplate: &DeckFaceplateDescriptor,
) -> Result<(), CommandError> {
    if !matches!(faceplate.schema_version, 1 | 2)
        || !valid_deck_ui_text(&faceplate.title)
        || faceplate.sections.is_empty()
        || faceplate.sections.len() > MAX_DECK_UI_SECTIONS
    {
        return Err(deck_ui_invalid(
            "faceplate.json has an unsupported schema or exceeds its limits.",
        ));
    }
    let controls = operator
        .controls
        .iter()
        .map(|control| (control.control_id(), control))
        .collect::<BTreeMap<_, _>>();
    let role_ids = manifest
        .signal
        .roles
        .iter()
        .map(|role| role.role_id.as_str())
        .collect::<BTreeSet<_>>();
    let expected_slots = (0..manifest.signal.slots).collect::<BTreeSet<_>>();
    let mut section_ids = BTreeSet::new();
    let mut widget_ids = BTreeSet::new();
    let mut source_slots = BTreeSet::new();
    let mut bound_controls = BTreeSet::new();
    let mut role_editor_count = 0;
    let mut transport_count = 0;
    let mut seed_count = 0;
    let mut capture_count = 0;
    let mut monitor_count = 0;
    let mut output_region_count = 0;
    let mut widget_count = 0;
    for section in &faceplate.sections {
        if !is_deck_ui_identifier(&section.section_id)
            || !section_ids.insert(section.section_id.as_str())
            || !valid_deck_ui_text(&section.title)
        {
            return Err(deck_ui_invalid(
                "faceplate.json contains an invalid or duplicate section.",
            ));
        }
        if faceplate.schema_version == 1 {
            if section.region.is_some() || section.columns.is_some() {
                return Err(deck_ui_invalid(
                    "Faceplate schema v1 cannot declare schema-v2 layout fields.",
                ));
            }
        } else {
            let Some(region) = section.region else {
                return Err(deck_ui_invalid(
                    "Faceplate schema v2 requires a region and bounded column count per section.",
                ));
            };
            if !matches!(section.columns, Some(1..=4)) {
                return Err(deck_ui_invalid(
                    "Faceplate schema v2 requires a region and bounded column count per section.",
                ));
            }
            if region == DeckFaceplateSectionRegion::Output {
                output_region_count += 1;
            }
        }
        widget_count += section.widgets.len();
        if widget_count > MAX_DECK_UI_WIDGETS {
            return Err(deck_ui_invalid(
                "faceplate.json exceeds the host-rendered widget limit.",
            ));
        }
        for widget in &section.widgets {
            if faceplate.schema_version == 1 {
                if widget.visible_when().is_some() {
                    return Err(deck_ui_invalid(
                        "Faceplate schema v1 cannot declare schema-v2 visibility predicates.",
                    ));
                }
            } else {
                validate_faceplate_visibility(&controls, widget.visible_when())?;
                let valid_region = match widget {
                    DeckFaceplateWidget::Monitor { .. } => {
                        section.region == Some(DeckFaceplateSectionRegion::Output)
                    }
                    DeckFaceplateWidget::Capture { .. } => {
                        section.region == Some(DeckFaceplateSectionRegion::Actions)
                    }
                    DeckFaceplateWidget::SourcePicker { .. }
                    | DeckFaceplateWidget::Slider { .. }
                    | DeckFaceplateWidget::Number { .. }
                    | DeckFaceplateWidget::Toggle { .. }
                    | DeckFaceplateWidget::Select { .. }
                    | DeckFaceplateWidget::RoleEditor { .. }
                    | DeckFaceplateWidget::Barycentric3 { .. }
                    | DeckFaceplateWidget::Transport { .. }
                    | DeckFaceplateWidget::Seed { .. } => {
                        section.region == Some(DeckFaceplateSectionRegion::Controls)
                    }
                };
                if !valid_region {
                    return Err(deck_ui_invalid(
                        "A schema-v2 faceplate widget occupies an invalid layout region.",
                    ));
                }
            }
            let (id, label) = faceplate_widget_identity(widget);
            if !is_deck_ui_identifier(id) || !widget_ids.insert(id) || !valid_deck_ui_text(label) {
                return Err(deck_ui_invalid(
                    "faceplate.json contains an invalid or duplicate widget.",
                ));
            }
            match widget {
                DeckFaceplateWidget::SourcePicker { slot_index, .. } => {
                    if !expected_slots.contains(slot_index) || !source_slots.insert(*slot_index) {
                        return Err(deck_ui_invalid(
                            "A source picker references an absent or duplicate physical slot.",
                        ));
                    }
                }
                DeckFaceplateWidget::Slider {
                    control_id,
                    minimum,
                    maximum,
                    step,
                    ..
                } => validate_numeric_widget(
                    &controls,
                    &mut bound_controls,
                    control_id,
                    *minimum,
                    *maximum,
                    *step,
                    false,
                )?,
                DeckFaceplateWidget::Number {
                    control_id,
                    minimum,
                    maximum,
                    step,
                    ..
                } => validate_numeric_widget(
                    &controls,
                    &mut bound_controls,
                    control_id,
                    *minimum,
                    *maximum,
                    *step,
                    true,
                )?,
                DeckFaceplateWidget::Toggle { control_id, .. } => {
                    if !matches!(
                        controls.get(control_id.as_str()),
                        Some(DeckControlDescriptor::Boolean { .. })
                    ) || !bound_controls.insert(control_id.as_str())
                    {
                        return Err(deck_ui_invalid(
                            "A toggle does not match one unique boolean control.",
                        ));
                    }
                }
                DeckFaceplateWidget::Select {
                    control_id,
                    options,
                    ..
                } => {
                    let option_values = options
                        .iter()
                        .map(|option| option.value.as_str())
                        .collect::<BTreeSet<_>>();
                    if options.is_empty()
                        || options.len() > 64
                        || option_values.len() != options.len()
                        || options.iter().any(|option| {
                            !is_deck_ui_identifier(&option.value)
                                || !valid_deck_ui_text(&option.label)
                        })
                        || !matches!(
                            controls.get(control_id.as_str()),
                            Some(DeckControlDescriptor::Enum { options: declared, .. })
                                if declared.iter().map(String::as_str).collect::<BTreeSet<_>>() == option_values
                        )
                        || !bound_controls.insert(control_id.as_str())
                    {
                        return Err(deck_ui_invalid(
                            "A select does not match one unique enum control.",
                        ));
                    }
                }
                DeckFaceplateWidget::RoleEditor {
                    role_ids: roles, ..
                } => {
                    role_editor_count += 1;
                    if roles.len() != role_ids.len()
                        || roles.iter().map(String::as_str).collect::<BTreeSet<_>>() != role_ids
                    {
                        return Err(deck_ui_invalid(
                            "The role editor does not cover the Deck role contract.",
                        ));
                    }
                }
                DeckFaceplateWidget::Barycentric3 {
                    x_control_id,
                    y_control_id,
                    vertex_role_ids,
                    ..
                } => {
                    if vertex_role_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>()
                        .len()
                        != 3
                        || vertex_role_ids
                            .iter()
                            .any(|role| !role_ids.contains(role.as_str()))
                    {
                        return Err(deck_ui_invalid(
                            "The barycentric widget does not reference three distinct Deck roles.",
                        ));
                    }
                    let mut defaults = Vec::with_capacity(2);
                    for control_id in [x_control_id, y_control_id] {
                        let Some(DeckControlDescriptor::Number {
                            default,
                            minimum,
                            maximum,
                            ..
                        }) = controls.get(control_id.as_str())
                        else {
                            return Err(deck_ui_invalid(
                                "The barycentric widget must bind two normalized number controls.",
                            ));
                        };
                        if *minimum != 0.0
                            || *maximum != 1.0
                            || !bound_controls.insert(control_id.as_str())
                        {
                            return Err(deck_ui_invalid(
                                "The barycentric widget must bind two unique normalized number controls.",
                            ));
                        }
                        defaults.push(*default);
                    }
                    let [x, y] = defaults.as_slice() else {
                        unreachable!("two barycentric controls are always collected")
                    };
                    if *x < 0.5 * *y - 1e-12 || *x > 1.0 - 0.5 * *y + 1e-12 {
                        return Err(deck_ui_invalid(
                            "The barycentric control defaults lie outside the declared triangle.",
                        ));
                    }
                }
                DeckFaceplateWidget::Transport { slot_indices, .. } => {
                    transport_count += 1;
                    if slot_indices.iter().copied().collect::<BTreeSet<_>>() != expected_slots
                        || slot_indices.len() != expected_slots.len()
                    {
                        return Err(deck_ui_invalid(
                            "The transport widget does not cover every physical slot.",
                        ));
                    }
                }
                DeckFaceplateWidget::Seed { .. } => seed_count += 1,
                DeckFaceplateWidget::Capture { modes, .. } => {
                    capture_count += 1;
                    if modes.is_empty()
                        || modes.len() > 2
                        || modes.iter().copied().collect::<BTreeSet<_>>().len() != modes.len()
                        || modes.iter().any(|mode| match mode {
                            DeckFaceplateCaptureMode::Snapshot => !manifest
                                .signal
                                .required_capabilities
                                .contains(&CodecCapability::SnapshotCapture),
                            DeckFaceplateCaptureMode::LiveCapture => !manifest
                                .signal
                                .required_capabilities
                                .contains(&CodecCapability::LiveCapture),
                        })
                    {
                        return Err(deck_ui_invalid(
                            "The capture widget exceeds the declared Deck capabilities.",
                        ));
                    }
                }
                DeckFaceplateWidget::Monitor { .. } => monitor_count += 1,
            }
        }
    }
    if source_slots != expected_slots
        || bound_controls.len() != controls.len()
        || bound_controls.iter().any(|id| !controls.contains_key(*id))
        || role_editor_count != 1
        || transport_count != 1
        || seed_count != 1
        || capture_count > 1
        || monitor_count != 1
        || (faceplate.schema_version == 2 && output_region_count != 1)
    {
        return Err(deck_ui_invalid(
            "faceplate.json does not expose the complete closed Deck contract exactly once.",
        ));
    }
    Ok(())
}

fn validate_faceplate_visibility(
    controls: &BTreeMap<&str, &DeckControlDescriptor>,
    predicates: Option<&[DeckFaceplateVisibilityPredicate]>,
) -> Result<(), CommandError> {
    let Some(predicates) = predicates else {
        return Ok(());
    };
    if predicates.is_empty() || predicates.len() > MAX_DECK_UI_VISIBILITY_PREDICATES {
        return Err(deck_ui_invalid(
            "A visibility predicate list is empty or exceeds its limit.",
        ));
    }
    for predicate in predicates {
        if !is_deck_ui_identifier(&predicate.control_id)
            || predicate.one_of.is_empty()
            || predicate.one_of.len() > MAX_DECK_UI_VISIBILITY_VALUES
            || predicate.one_of.iter().collect::<BTreeSet<_>>().len()
                != predicate.one_of.len()
            || predicate.one_of.iter().any(|value| {
                matches!(value, DeckFaceplateVisibilityValue::Text(text) if !is_deck_ui_identifier(text))
            })
        {
            return Err(deck_ui_invalid(
                "A visibility predicate is invalid, duplicated, unsafe, or exceeds its limits.",
            ));
        }
        let Some(control) = controls.get(predicate.control_id.as_str()) else {
            return Err(deck_ui_invalid(
                "A visibility predicate references an absent operator control.",
            ));
        };
        let matches_control = match control {
            DeckControlDescriptor::Enum { options, .. } =>
                predicate.one_of.iter().all(|value| {
                    matches!(value, DeckFaceplateVisibilityValue::Text(text) if options.contains(text))
                }),
            DeckControlDescriptor::Boolean { .. } => predicate
                .one_of
                .iter()
                .all(|value| matches!(value, DeckFaceplateVisibilityValue::Boolean(_))),
            DeckControlDescriptor::Number { .. } | DeckControlDescriptor::Integer { .. } => false,
        };
        if !matches_control {
            return Err(deck_ui_invalid(
                "A visibility predicate must match one enum or boolean control exactly.",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss, clippy::float_cmp)]
fn validate_numeric_widget<'a>(
    controls: &BTreeMap<&'a str, &'a DeckControlDescriptor>,
    bound_controls: &mut BTreeSet<&'a str>,
    control_id: &'a str,
    minimum: f64,
    maximum: f64,
    step: f64,
    number_widget: bool,
) -> Result<(), CommandError> {
    let Some(control) = controls.get(control_id) else {
        return Err(deck_ui_invalid(
            "A numeric widget references an absent operator control.",
        ));
    };
    let matches = match control {
        DeckControlDescriptor::Number {
            minimum: declared_minimum,
            maximum: declared_maximum,
            step: declared_step,
            ..
        } => minimum == *declared_minimum && maximum == *declared_maximum && step == *declared_step,
        DeckControlDescriptor::Integer {
            minimum: declared_minimum,
            maximum: declared_maximum,
            step: declared_step,
            ..
        } => {
            number_widget
                && minimum == *declared_minimum as f64
                && maximum == *declared_maximum as f64
                && step == *declared_step as f64
        }
        DeckControlDescriptor::Boolean { .. } | DeckControlDescriptor::Enum { .. } => false,
    };
    if !matches || !bound_controls.insert(control_id) {
        return Err(deck_ui_invalid(
            "A numeric widget does not match one unique typed operator control.",
        ));
    }
    Ok(())
}

fn faceplate_widget_identity(widget: &DeckFaceplateWidget) -> (&str, &str) {
    match widget {
        DeckFaceplateWidget::SourcePicker { id, label, .. }
        | DeckFaceplateWidget::Slider { id, label, .. }
        | DeckFaceplateWidget::Number { id, label, .. }
        | DeckFaceplateWidget::Toggle { id, label, .. }
        | DeckFaceplateWidget::Select { id, label, .. }
        | DeckFaceplateWidget::RoleEditor { id, label, .. }
        | DeckFaceplateWidget::Barycentric3 { id, label, .. }
        | DeckFaceplateWidget::Transport { id, label, .. }
        | DeckFaceplateWidget::Seed { id, label, .. }
        | DeckFaceplateWidget::Capture { id, label, .. }
        | DeckFaceplateWidget::Monitor { id, label, .. } => (id, label),
    }
}

fn is_deck_ui_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return false;
    }
    value.split(['.', '_', '-']).all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

fn valid_deck_ui_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn deck_ui_invalid(message: &'static str) -> CommandError {
    CommandError::new("deck_ui.package_invalid", message)
}

fn deck_ui_catalog_limit() -> CommandError {
    CommandError::new(
        "deck_ui.catalog_limit_exceeded",
        "The enabled Deck UI catalog exceeds its closed entry or JSON byte limit.",
    )
}

#[tauri::command]
pub(crate) async fn extensions_deck_catalog(
    state: State<'_, ExtensionManagerState>,
) -> Result<DeckUiCatalogView, CommandError> {
    let roots = state.roots().clone();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || deck_ui_catalog_for(&roots, &active_packages))
        .await
        .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_snapshot(
    state: State<'_, ExtensionManagerState>,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = state.roots().clone();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || extension_snapshot_for(&roots, &active_packages))
        .await
        .map_err(|_| extension_task_failed())?
}

#[tauri::command]
pub(crate) async fn extensions_inspect(
    path: String,
) -> Result<InspectedExtensionView, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_extension_archive(PathBuf::from(path), None)
            .map(inspected_extension_view)
            .map_err(extension_command_error)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_install(
    state: State<'_, ExtensionManagerState>,
    path: String,
    expected_sha256: String,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = state.roots().clone();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || {
        install(
            &roots,
            &InstallRequest {
                archive_path: PathBuf::from(path),
                expected_sha256,
            },
        )
        .map_err(extension_command_error)?;
        extension_snapshot_for(&roots, &active_packages)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_repair(
    state: State<'_, ExtensionManagerState>,
    path: String,
    expected_sha256: String,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = state.roots().clone();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || {
        active_packages.invalidate_all();
        repair(
            &roots,
            &InstallRequest {
                archive_path: PathBuf::from(path),
                expected_sha256,
            },
        )
        .map_err(extension_command_error)?;
        extension_snapshot_for(&roots, &active_packages)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_verify(
    state: State<'_, ExtensionManagerState>,
    package: ExtensionPackageRequest,
) -> Result<ExtensionPackageSummaryView, CommandError> {
    let roots = state.roots().clone();
    let package = package.into_reference();
    tauri::async_runtime::spawn_blocking(move || {
        verify(&roots, &package)
            .map(Into::into)
            .map_err(extension_command_error)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_enable(
    state: State<'_, ExtensionManagerState>,
    package: ExtensionPackageRequest,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = state.roots().clone();
    let package = package.into_reference();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || {
        active_packages
            .enable_and_prime(&roots, &package)
            .map_err(extension_command_error)?;
        extension_snapshot_for(&roots, &active_packages)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_disable(
    state: State<'_, ExtensionManagerState>,
    package: ExtensionPackageRequest,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = state.roots().clone();
    let package = package.into_reference();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || {
        active_packages
            .disable(&roots, &package)
            .map_err(extension_command_error)?;
        extension_snapshot_for(&roots, &active_packages)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn extensions_remove(
    state: State<'_, ExtensionManagerState>,
    package: ExtensionPackageRequest,
    allow_corrupt: bool,
) -> Result<ExtensionManagerSnapshot, CommandError> {
    let roots = state.roots().clone();
    let package = package.into_reference();
    let active_packages = state.active_packages().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = active_packages.invalidate_exact(&roots, &package);
        remove(&roots, &package, RemoveOptions { allow_corrupt })
            .map_err(extension_command_error)?;
        extension_snapshot_for(&roots, &active_packages)
    })
    .await
    .map_err(|_| extension_task_failed())?
}

#[cfg(test)]
mod tests {
    use std::fs;

    use latentdeck_extension_manager::{
        DeckPackManifest, PackageHealth, ScaffoldRequest, scaffold,
    };

    use super::*;

    fn package() -> PackageReference {
        PackageReference {
            kind: PackageKind::DeckPack,
            package_id: "org.example.deck".to_owned(),
            package_version: "1.2.3".to_owned(),
        }
    }

    fn d2_manifest_and_package() -> (DeckPackManifest, PackageReference) {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/d2/package/deck-pack.json"
        ))
        .expect("bundled D2 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        (manifest, package)
    }

    fn d2_faceplate_json() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../../../../operators/builtin/d2/package/faceplate.json"
        ))
        .expect("bundled D2 faceplate")
    }

    fn validate_d2_faceplate_json(
        faceplate: &serde_json::Value,
    ) -> Result<DeckUiPackageView, CommandError> {
        let (manifest, package) = d2_manifest_and_package();
        deck_ui_view_from_parts(
            &package,
            &manifest,
            include_bytes!("../../../../operators/builtin/d2/package/operator.json"),
            &serde_json::to_vec(faceplate).expect("serialize faceplate"),
        )
    }

    #[test]
    fn public_starter_deck_is_accepted_by_the_host_parsers() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../examples/extensions/starter-deck/deck-pack.json"
        ))
        .expect("public starter Deck manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        let view = deck_ui_view_from_parts(
            &package,
            &manifest,
            include_bytes!("../../../../examples/extensions/starter-deck/operator.json"),
            include_bytes!("../../../../examples/extensions/starter-deck/faceplate.json"),
        )
        .expect("public starter Deck host view");

        assert_eq!(view.deck.slots, 1);
        assert_eq!(view.operator.controls.len(), 1);
        assert_eq!(view.faceplate.sections.len(), 6);
    }

    #[test]
    fn scaffolded_starter_deck_is_accepted_by_the_host_parsers() {
        let temporary = tempfile::tempdir().expect("temporary scaffold root");
        let source = if let Some(path) = std::env::var_os("LATENTDECK_SCAFFOLDED_DECK_PATH") {
            PathBuf::from(path)
        } else {
            let source = temporary.path().join("deck");
            scaffold(&ScaffoldRequest {
                kind: PackageKind::DeckPack,
                package_id: "com.example.host-parser".to_owned(),
                package_version: "0.1.0".to_owned(),
                output_directory: source.clone(),
            })
            .expect("scaffold public Deck");
            source
        };
        let manifest = serde_json::from_slice::<DeckPackManifest>(
            &fs::read(source.join("deck-pack.json")).expect("read scaffold manifest"),
        )
        .expect("parse scaffold manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        let view = deck_ui_view_from_parts(
            &package,
            &manifest,
            &fs::read(source.join("operator.json")).expect("read scaffold operator"),
            &fs::read(source.join("faceplate.json")).expect("read scaffold faceplate"),
        )
        .expect("scaffolded starter Deck host view");

        assert_eq!(view.deck.slots, 1);
        assert_eq!(view.operator.controls.len(), 1);
        assert_eq!(view.faceplate.sections.len(), 6);
    }

    fn legacy_d2_faceplate_json() -> serde_json::Value {
        let mut faceplate = d2_faceplate_json();
        faceplate["schema_version"] = serde_json::json!(1);
        for section in faceplate["sections"].as_array_mut().expect("sections") {
            let section = section.as_object_mut().expect("section object");
            section.remove("region");
            section.remove("columns");
            for widget in section["widgets"].as_array_mut().expect("widgets") {
                widget
                    .as_object_mut()
                    .expect("widget object")
                    .remove("visible_when");
            }
        }
        faceplate
    }

    #[test]
    fn deck_ui_catalog_view_is_closed_path_free_and_bound_to_exact_identity() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/d2/package/deck-pack.json"
        ))
        .expect("bundled D2 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };

        let view = deck_ui_view_from_parts(
            &package,
            &manifest,
            include_bytes!("../../../../operators/builtin/d2/package/operator.json"),
            include_bytes!("../../../../operators/builtin/d2/package/faceplate.json"),
        )
        .expect("validated UI-only catalog view");
        let json = serde_json::to_string(&view).expect("serialize view");

        assert!(json.contains("org.latentdeck.deck.d2"));
        assert!(json.contains("org.latentdeck.builtin.ld_d2"));
        assert!(json.contains("source_picker"));
        assert!(!json.contains("python_root"));
        assert!(!json.contains("entrypoint"));
        assert!(!json.contains("python/deck_operator"));
        assert!(!json.contains("installed_path"));
    }

    #[test]
    fn deck_ui_catalog_accepts_closed_faceplate_v2_layout_and_visibility() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/d2/package/deck-pack.json"
        ))
        .expect("bundled D2 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };

        let view = deck_ui_view_from_parts(
            &package,
            &manifest,
            include_bytes!("../../../../operators/builtin/d2/package/operator.json"),
            include_bytes!("../../../../operators/builtin/d2/package/faceplate.json"),
        )
        .expect("closed schema-v2 faceplate");
        let json = serde_json::to_string(&view.faceplate).expect("serialize faceplate");

        assert_eq!(view.faceplate.schema_version, 2);
        assert!(json.contains("\"region\":\"output\""));
        assert!(json.contains("\"visible_when\""));
    }

    #[test]
    fn deck_ui_catalog_preserves_v1_and_rejects_v2_fields_on_v1() {
        let legacy = legacy_d2_faceplate_json();
        let view = validate_d2_faceplate_json(&legacy).expect("legacy faceplate remains valid");
        let json = serde_json::to_string(&view.faceplate).expect("serialize legacy faceplate");
        assert!(!json.contains("\"region\""));
        assert!(!json.contains("\"columns\""));
        assert!(!json.contains("\"visible_when\""));

        let mut with_layout = legacy.clone();
        with_layout["sections"][0]["region"] = serde_json::json!("controls");
        assert!(validate_d2_faceplate_json(&with_layout).is_err());

        let mut with_visibility = legacy;
        with_visibility["sections"][3]["widgets"][0]["visible_when"] = serde_json::json!([
            { "control_id": "algorithm", "one_of": ["xs1"] }
        ]);
        assert!(validate_d2_faceplate_json(&with_visibility).is_err());
    }

    #[test]
    fn deck_ui_catalog_rejects_explicit_null_for_optional_schema_v2_fields() {
        let legacy = legacy_d2_faceplate_json();

        let mut null_region = legacy.clone();
        null_region["sections"][0]["region"] = serde_json::Value::Null;
        assert!(validate_d2_faceplate_json(&null_region).is_err());

        let mut null_columns = legacy.clone();
        null_columns["sections"][0]["columns"] = serde_json::Value::Null;
        assert!(validate_d2_faceplate_json(&null_columns).is_err());

        let mut null_v1_visibility = legacy;
        null_v1_visibility["sections"][3]["widgets"][0]["visible_when"] = serde_json::Value::Null;
        assert!(validate_d2_faceplate_json(&null_v1_visibility).is_err());

        let mut null_v2_visibility = d2_faceplate_json();
        null_v2_visibility["sections"][3]["widgets"]
            .as_array_mut()
            .expect("widgets")
            .iter_mut()
            .find(|widget| widget["id"] == "mode")
            .expect("mode widget")["visible_when"] = serde_json::Value::Null;
        assert!(validate_d2_faceplate_json(&null_v2_visibility).is_err());
    }

    #[test]
    fn deck_ui_catalog_enforces_faceplate_v2_regions_and_columns() {
        let valid = d2_faceplate_json();

        let mut missing_columns = valid.clone();
        missing_columns["sections"][0]
            .as_object_mut()
            .expect("section")
            .remove("columns");
        assert!(validate_d2_faceplate_json(&missing_columns).is_err());

        let mut excessive_columns = valid.clone();
        excessive_columns["sections"][0]["columns"] = serde_json::json!(5);
        assert!(validate_d2_faceplate_json(&excessive_columns).is_err());

        let mut monitor_outside_output = valid.clone();
        monitor_outside_output["sections"][5]["region"] = serde_json::json!("controls");
        assert!(validate_d2_faceplate_json(&monitor_outside_output).is_err());

        let mut capture_outside_actions = valid.clone();
        capture_outside_actions["sections"][4]["region"] = serde_json::json!("controls");
        assert!(validate_d2_faceplate_json(&capture_outside_actions).is_err());

        let mut control_outside_controls = valid.clone();
        control_outside_controls["sections"][0]["region"] = serde_json::json!("actions");
        assert!(validate_d2_faceplate_json(&control_outside_controls).is_err());

        let mut duplicate_output = valid;
        duplicate_output["sections"]
            .as_array_mut()
            .expect("sections")
            .push(serde_json::json!({
                "section_id": "second_output",
                "title": "Second output",
                "region": "output",
                "columns": 1,
                "widgets": []
            }));
        assert!(validate_d2_faceplate_json(&duplicate_output).is_err());
    }

    #[test]
    fn deck_ui_catalog_rejects_open_unbounded_or_mistyped_visibility_predicates() {
        fn mode_visibility(faceplate: &mut serde_json::Value) -> &mut serde_json::Value {
            faceplate["sections"][3]["widgets"]
                .as_array_mut()
                .expect("widgets")
                .iter_mut()
                .find(|widget| widget["id"] == "mode")
                .expect("mode widget")
                .get_mut("visible_when")
                .expect("mode visibility")
        }

        let valid = d2_faceplate_json();

        let mut empty = valid.clone();
        *mode_visibility(&mut empty) = serde_json::json!([]);
        assert!(validate_d2_faceplate_json(&empty).is_err());

        let mut duplicated = valid.clone();
        *mode_visibility(&mut duplicated) = serde_json::json!([
            { "control_id": "algorithm", "one_of": ["xs1", "xs1"] }
        ]);
        assert!(validate_d2_faceplate_json(&duplicated).is_err());

        let mut absent_control = valid.clone();
        *mode_visibility(&mut absent_control) = serde_json::json!([
            { "control_id": "missing", "one_of": ["xs1"] }
        ]);
        assert!(validate_d2_faceplate_json(&absent_control).is_err());

        let mut numeric_control = valid.clone();
        *mode_visibility(&mut numeric_control) = serde_json::json!([
            { "control_id": "mix", "one_of": ["xs1"] }
        ]);
        assert!(validate_d2_faceplate_json(&numeric_control).is_err());

        let mut invalid_enum_value = valid.clone();
        *mode_visibility(&mut invalid_enum_value) = serde_json::json!([
            { "control_id": "algorithm", "one_of": ["bogus"] }
        ]);
        assert!(validate_d2_faceplate_json(&invalid_enum_value).is_err());

        let mut numeric_value = valid.clone();
        *mode_visibility(&mut numeric_value) = serde_json::json!([
            { "control_id": "algorithm", "one_of": [1] }
        ]);
        assert!(validate_d2_faceplate_json(&numeric_value).is_err());

        let mut unsafe_value = valid.clone();
        *mode_visibility(&mut unsafe_value) = serde_json::json!([
            { "control_id": "algorithm", "one_of": ["XS1"] }
        ]);
        assert!(validate_d2_faceplate_json(&unsafe_value).is_err());

        let mut open_predicate = valid.clone();
        *mode_visibility(&mut open_predicate) = serde_json::json!([
            { "control_id": "algorithm", "one_of": ["xs1"], "script": "run()" }
        ]);
        assert!(validate_d2_faceplate_json(&open_predicate).is_err());

        let mut too_many_predicates = valid.clone();
        *mode_visibility(&mut too_many_predicates) = serde_json::json!([
            { "control_id": "algorithm", "one_of": ["linear"] },
            { "control_id": "algorithm", "one_of": ["xs1"] },
            { "control_id": "algorithm", "one_of": ["xs2"] },
            { "control_id": "algorithm", "one_of": ["xs3"] },
            { "control_id": "algorithm", "one_of": ["xs4"] },
            { "control_id": "algorithm", "one_of": ["xs5"] },
            { "control_id": "mode", "one_of": ["hybridize"] },
            { "control_id": "mode", "one_of": ["interact"] },
            { "control_id": "algorithm", "one_of": ["linear"] }
        ]);
        assert!(validate_d2_faceplate_json(&too_many_predicates).is_err());

        let mut too_many_values = valid;
        *mode_visibility(&mut too_many_values) = serde_json::json!([
            {
                "control_id": "algorithm",
                "one_of": [
                    "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8",
                    "v9", "v10", "v11", "v12", "v13", "v14", "v15", "v16"
                ]
            }
        ]);
        assert!(validate_d2_faceplate_json(&too_many_values).is_err());
    }

    #[test]
    fn deck_ui_visibility_predicates_accept_only_boolean_values_for_boolean_controls() {
        let boolean = DeckControlDescriptor::Boolean {
            control_id: "enabled".to_owned(),
            default: false,
        };
        let controls = BTreeMap::from([("enabled", &boolean)]);
        let valid = vec![DeckFaceplateVisibilityPredicate {
            control_id: "enabled".to_owned(),
            one_of: vec![DeckFaceplateVisibilityValue::Boolean(true)],
        }];
        assert!(validate_faceplate_visibility(&controls, Some(&valid)).is_ok());

        let mismatched = vec![DeckFaceplateVisibilityPredicate {
            control_id: "enabled".to_owned(),
            one_of: vec![DeckFaceplateVisibilityValue::Text("true".to_owned())],
        }];
        assert!(validate_faceplate_visibility(&controls, Some(&mismatched)).is_err());
    }

    #[test]
    fn deck_ui_catalog_rejects_executable_or_unknown_faceplate_fields() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/d2/package/deck-pack.json"
        ))
        .expect("bundled D2 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        let mut faceplate = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../../operators/builtin/d2/package/faceplate.json"
        ))
        .expect("faceplate JSON");
        faceplate["sections"][0]["widgets"][0]["html"] =
            serde_json::json!("<script>invoke('shell')</script>");

        let error = deck_ui_view_from_parts(
            &package,
            &manifest,
            include_bytes!("../../../../operators/builtin/d2/package/operator.json"),
            &serde_json::to_vec(&faceplate).expect("mutated faceplate"),
        )
        .expect_err("open faceplate fields must fail closed");
        let serialized = serde_json::to_string(&error).expect("serialize error");
        assert!(serialized.contains("deck_ui.package_invalid"));
        assert!(!serialized.contains("<script>"));
        assert!(!serialized.contains("invoke"));
        assert!(!serialized.contains("shell"));
    }

    #[test]
    fn deck_ui_catalog_rejects_role_editor_duplicates_before_frontend_parsing() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/d2/package/deck-pack.json"
        ))
        .expect("bundled D2 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        let mut faceplate = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../../operators/builtin/d2/package/faceplate.json"
        ))
        .expect("faceplate JSON");
        let role_editor = faceplate["sections"]
            .as_array_mut()
            .expect("sections")
            .iter_mut()
            .flat_map(|section| section["widgets"].as_array_mut().expect("widgets"))
            .find(|widget| widget["kind"] == "role_editor")
            .expect("role editor");
        role_editor["role_ids"] = serde_json::json!(["carrier", "donor", "donor"]);

        let result = deck_ui_view_from_parts(
            &package,
            &manifest,
            include_bytes!("../../../../operators/builtin/d2/package/operator.json"),
            &serde_json::to_vec(&faceplate).expect("mutated faceplate"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn deck_ui_catalog_rejects_js_unsafe_integer_controls() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/q4/package/deck-pack.json"
        ))
        .expect("bundled Q4 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        let mut operator = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../../operators/builtin/q4/package/operator.json"
        ))
        .expect("operator JSON");
        let top_k = operator["controls"]
            .as_array_mut()
            .expect("controls")
            .iter_mut()
            .find(|control| control["control_id"] == "top_k")
            .expect("top_k control");
        top_k["default"] = serde_json::json!(9_007_199_254_740_992_i64);
        top_k["minimum"] = serde_json::json!(9_007_199_254_740_992_i64);
        top_k["maximum"] = serde_json::json!(9_007_199_254_740_994_i64);
        top_k["step"] = serde_json::json!(2_u64);
        let mut faceplate = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../../operators/builtin/q4/package/faceplate.json"
        ))
        .expect("faceplate JSON");
        let top_k_widget = faceplate["sections"]
            .as_array_mut()
            .expect("sections")
            .iter_mut()
            .flat_map(|section| section["widgets"].as_array_mut().expect("widgets"))
            .find(|widget| widget["control_id"] == "top_k")
            .expect("top_k widget");
        top_k_widget["minimum"] = serde_json::json!(9_007_199_254_740_992_f64);
        top_k_widget["maximum"] = serde_json::json!(9_007_199_254_740_994_f64);
        top_k_widget["step"] = serde_json::json!(2_f64);

        let result = deck_ui_view_from_parts(
            &package,
            &manifest,
            &serde_json::to_vec(&operator).expect("mutated operator"),
            &serde_json::to_vec(&faceplate).expect("mutated faceplate"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn deck_ui_catalog_requires_normalized_barycentric_controls() {
        let manifest = serde_json::from_str::<DeckPackManifest>(include_str!(
            "../../../../operators/builtin/q4/package/deck-pack.json"
        ))
        .expect("bundled Q4 manifest");
        let package = PackageReference {
            kind: PackageKind::DeckPack,
            package_id: manifest.deck_id.clone(),
            package_version: manifest.deck_version.clone(),
        };
        let mut operator = serde_json::from_str::<serde_json::Value>(include_str!(
            "../../../../operators/builtin/q4/package/operator.json"
        ))
        .expect("operator JSON");
        let triangle_x = operator["controls"]
            .as_array_mut()
            .expect("controls")
            .iter_mut()
            .find(|control| control["control_id"] == "triangle_x")
            .expect("triangle_x control");
        triangle_x["minimum"] = serde_json::json!(-1.0);

        let result = deck_ui_view_from_parts(
            &package,
            &manifest,
            &serde_json::to_vec(&operator).expect("mutated operator"),
            include_bytes!("../../../../operators/builtin/q4/package/faceplate.json"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn deck_ui_catalog_json_budget_fails_closed_before_unbounded_aggregation() {
        let directory = tempfile::tempdir().expect("temporary catalog directory");
        let path = directory.path().join("operator.json");
        std::fs::write(&path, br#"{"schema_version":1}"#).expect("write bounded JSON");
        let length = std::fs::metadata(&path).expect("measure JSON").len();

        let mut insufficient = length - 1;
        assert!(matches!(
            read_catalog_deck_ui_json(path.clone(), &mut insufficient),
            Err(DeckUiPackageLoadError::CatalogLimit)
        ));

        let mut exact = length;
        let bytes = read_catalog_deck_ui_json(path, &mut exact).expect("exact remaining budget");
        assert_eq!(u64::try_from(bytes.len()).expect("bounded length"), length);
        assert_eq!(exact, 0);

        let serialized = serde_json::to_string(&deck_ui_catalog_limit()).expect("serialize error");
        assert!(serialized.contains("deck_ui.catalog_limit_exceeded"));
        assert!(!serialized.contains(directory.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn package_request_denies_unknown_fields_and_preserves_exact_identity() {
        let request: ExtensionPackageRequest = serde_json::from_value(serde_json::json!({
            "kind": "deck_pack",
            "packageId": "org.example.deck",
            "packageVersion": "1.2.3"
        }))
        .expect("closed request");
        assert_eq!(request.into_reference(), package());

        let extra = serde_json::from_value::<ExtensionPackageRequest>(serde_json::json!({
            "kind": "deck_pack",
            "packageId": "org.example.deck",
            "packageVersion": "1.2.3",
            "chooseNewest": true
        }));
        assert!(extra.is_err());
    }

    #[test]
    fn corrupt_summary_never_exposes_lifecycle_path_detail() {
        let view = ExtensionPackageSummaryView::from(InstalledPackageSummary {
            package: package(),
            display_name: Some("Example".to_owned()),
            publisher_name: Some("Publisher".to_owned()),
            enabled: false,
            health: PackageHealth::Corrupt,
            error_code: Some("extension.integrity_failed".to_owned()),
            error_detail: Some("PRIVATE_PATH_MARKER/owner/secret/deck-pack.json".to_owned()),
        });
        let json = serde_json::to_string(&view).expect("serialize view");
        assert!(!json.contains("PRIVATE_PATH_MARKER"));
        assert!(!json.contains("secret"));
        assert!(json.contains("verify or repair"));
    }

    #[test]
    fn lifecycle_error_mapping_is_stable_and_path_free() {
        let error = ExtensionError::new(
            ExtensionErrorCode::IntegrityFailed,
            "hash mismatch at PRIVATE_PATH_MARKER/owner/secret.ld",
        );
        let command = extension_command_error(error);
        let json = serde_json::to_string(&command).expect("serialize command error");
        assert!(json.contains("extension.integrity_failed"));
        assert!(!json.contains("PRIVATE_PATH_MARKER"));
        assert!(!json.contains("secret.ld"));
    }

    #[test]
    fn state_retains_the_exact_shared_roots() {
        let directory = tempfile::tempdir().expect("temporary extension root");
        let roots = ExtensionRoots::for_base_root(directory.path());
        let state = ExtensionManagerState::new(roots.clone());
        assert_eq!(state.roots().base_root, roots.base_root);
        assert_eq!(state.roots().decks_root, roots.decks_root);
        assert_eq!(state.roots().codec_packs_root, roots.codec_packs_root);
    }
}
