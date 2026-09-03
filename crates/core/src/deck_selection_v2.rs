//! Exact, codec-neutral Deck + Codec + source preflight for Protocol 2.
//!
//! This boundary deliberately accepts package identities and Library-resolved
//! cartridge identities, never worker paths, entrypoints, or an implicit
//! "newest" selector.  It retains both package trees and every validated LC
//! handle before a worker or GPU allocation is attempted.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use latentdeck_cartridge::{
    manifest::{DType, ManifestV0_1, TensorStream},
    reader::{IntegrityValidatedCartridge, ValidationOptions, open_integrity_validated},
};
use latentdeck_control::v2::{
    DecodedAbi, DeviceKind, ExternalAssetBinding, ProfileKey, SignalGeometry, TensorAbi,
    TensorDtype,
};
use latentdeck_extension_manager::{
    ActiveInstalledPackage, ActivePackageCache, CodecPackManifest, CompatibilityReason,
    DeckPackManifest, ErrorCode as ExtensionErrorCode, ExtensionError, ExtensionRoots, PackageKind,
    PackageManifest, PackageReference, SelectedSourceCompatibility, SelectedSourceScope,
    SignalGeometry as ManifestSignalGeometry, TensorDevice, TensorDtype as ManifestTensorDtype,
    resolve_package_compatibility, resolve_selected_compatibility,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    deck_runtime_v2::{ActiveDeckRuntime, DeckRuntimeError},
    deck_session_v2::DeckSessionV2HostContract,
    external_asset_v2::IntegrityValidatedExternalAsset,
};

const MAX_SOURCES: usize = 16;
const MAX_DECODE_BATCH: u8 = 24;
const RING_SLOT_COUNT: u8 = 2;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// One exact user choice.  There is intentionally no optional version,
/// version range, auto-update, or newest-package representation.
#[derive(Clone, Debug)]
pub struct DeckPackageSelectionV2 {
    deck: PackageReference,
    codec: PackageReference,
    device: DeviceKind,
    device_ordinal: u8,
    external_assets: BTreeMap<String, PathBuf>,
    retained_external_assets: BTreeMap<String, IntegrityValidatedExternalAsset>,
}

impl DeckPackageSelectionV2 {
    #[must_use]
    pub fn new(
        deck_id: String,
        deck_version: String,
        codec_id: String,
        codec_version: String,
        device: DeviceKind,
    ) -> Self {
        Self {
            deck: PackageReference {
                kind: PackageKind::DeckPack,
                package_id: deck_id,
                package_version: deck_version,
            },
            codec: PackageReference {
                kind: PackageKind::CodecPack,
                package_id: codec_id,
                package_version: codec_version,
            },
            device,
            device_ordinal: 0,
            external_assets: BTreeMap::new(),
            retained_external_assets: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn deck(&self) -> &PackageReference {
        &self.deck
    }

    #[must_use]
    pub const fn codec(&self) -> &PackageReference {
        &self.codec
    }

    #[must_use]
    pub const fn device(&self) -> DeviceKind {
        self.device
    }

    pub fn set_device_ordinal(&mut self, ordinal: u8) {
        self.device_ordinal = ordinal;
    }

    pub fn bind_external_asset(&mut self, asset_id: String, path: PathBuf) {
        self.retained_external_assets.remove(&asset_id);
        self.external_assets.insert(asset_id, path);
    }

    pub fn bind_integrity_validated_external_asset(
        &mut self,
        asset: IntegrityValidatedExternalAsset,
    ) {
        let binding = asset.binding();
        self.external_assets.remove(&binding.asset_id);
        self.retained_external_assets
            .insert(binding.asset_id.clone(), asset);
    }
}

/// Exact Library identity supplied to the generic preflight.  The path is
/// never transported to Python; Core reopens and retains it first.
#[derive(Clone, Copy, Debug)]
pub struct DeckSourceSelectionV2<'a> {
    pub path: &'a Path,
    pub cartridge_id: &'a str,
    pub archive_sha256: &'a str,
    /// Optional backend-retained result of the Library's exact source
    /// resolution. Supplying it avoids reopening and hashing the same LC at
    /// the Core boundary; identity and receipt fields are still cross-checked.
    pub validated_cartridge: Option<&'a IntegrityValidatedCartridge>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckSourceFactsV2 {
    pub cartridge_id: String,
    pub archive_sha256: String,
    pub lc_spec_version: String,
    pub profile_key: ProfileKey,
    pub signal_geometry: SignalGeometry,
    pub tensor_dtype: TensorDtype,
    pub latent_slot_count: u64,
    pub tensor_storage_bytes: u64,
}

/// One immutable Library-index entry supplied to metadata-only Deck
/// compatibility preflight. Exact launch still reopens and retains the LC
/// bytes before starting a worker.
#[derive(Clone, Copy)]
pub struct IndexedDeckSourceSelection<'a> {
    pub manifest: &'a ManifestV0_1,
    pub expected_cartridge_id: &'a str,
    pub archive_sha256: &'a str,
}

/// Fully retained inputs which can be passed directly to
/// `start_deck_session_v2`; no package or LC is rediscovered by newest version.
pub struct PreparedDeckSelectionV2 {
    pub codec_package: ActiveInstalledPackage,
    pub deck_runtime: ActiveDeckRuntime,
    pub cartridges: Vec<IntegrityValidatedCartridge>,
    pub host: DeckSessionV2HostContract,
    pub external_assets: Vec<ExternalAssetBinding>,
    /// Exact no-share-write/delete evidence captured by the host UI. This is
    /// reused only on Windows, where the retained handle prevents in-place
    /// mutation; other platforms must revalidate at launch.
    pub retained_external_assets: Vec<IntegrityValidatedExternalAsset>,
    pub sources: Vec<DeckSourceFactsV2>,
    pub validation_work: DeckSelectionValidationWorkV2,
}

/// Observable heavy LC work performed for one exact Deck preparation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeckSelectionValidationWorkV2 {
    pub full_cartridge_validations: usize,
    pub retained_handle_clones: usize,
    pub retained_external_asset_clones: usize,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DeckSelectionV2Error {
    #[error("the exact selected package is not active and trusted")]
    Untrusted,
    #[error("a required exact external codec asset is not bound")]
    MissingAsset,
    #[error("the selected package or bounded source is invalid")]
    PackageInvalid,
    #[error("the selected pair does not support Protocol 2")]
    UnsupportedProtocol,
    #[error("the selected pair does not support this host API")]
    UnsupportedHostApi,
    #[error("the selected pair does not support this tensor ABI")]
    UnsupportedTensorAbi,
    #[error("the selected codec does not support the cartridge profile")]
    UnsupportedProfile,
    #[error("the selected Deck does not support the exact signal")]
    UnsupportedSignal,
    #[error("the selected Deck does not support the exact timing")]
    UnsupportedTiming,
    #[error("the selected codec does not provide every required capability")]
    UnsupportedCapability,
    #[error("the extension lifecycle is temporarily unavailable: {0:?}")]
    ExtensionLifecycle(ExtensionErrorCode),
}

impl DeckSelectionV2Error {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
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
            Self::ExtensionLifecycle(code) => code.as_str(),
        }
    }
}

impl From<DeckRuntimeError> for DeckSelectionV2Error {
    fn from(_: DeckRuntimeError) -> Self {
        Self::PackageInvalid
    }
}

const fn selection_error_from_extension(error: &ExtensionError) -> DeckSelectionV2Error {
    match error.code() {
        ExtensionErrorCode::InvalidArguments
        | ExtensionErrorCode::ArchiveInvalid
        | ExtensionErrorCode::ManifestInvalid
        | ExtensionErrorCode::IntegrityFailed => DeckSelectionV2Error::PackageInvalid,
        ExtensionErrorCode::PackageMissing
        | ExtensionErrorCode::PackageDisabled
        | ExtensionErrorCode::PackageUntrusted => DeckSelectionV2Error::Untrusted,
        code @ (ExtensionErrorCode::PackageExists
        | ExtensionErrorCode::PackageActive
        | ExtensionErrorCode::LifecycleBusy
        | ExtensionErrorCode::LifecycleConflict
        | ExtensionErrorCode::Io) => DeckSelectionV2Error::ExtensionLifecycle(code),
    }
}

/// Check one immutable Library-index snapshot against the same profile,
/// tensor, signal, and timing rules used by exact launch preparation.
///
/// This is a lightweight UI eligibility check only. The indexed manifest was
/// fully validated when it entered the Library, but this function does not
/// reopen or trust the current file bytes. Exact selected sources must still
/// pass retained full validation before worker or GPU allocation.
///
/// # Errors
///
/// Returns the same stable source-compatibility reason as launch preparation.
pub fn check_indexed_deck_source_compatibility(
    codec: &CodecPackManifest,
    deck: &DeckPackManifest,
    manifest: &ManifestV0_1,
    expected_cartridge_id: &str,
    archive_sha256: &str,
    selected_profile: &ProfileKey,
    device: DeviceKind,
) -> Result<(), DeckSelectionV2Error> {
    validate_indexed_deck_sources(
        codec,
        deck,
        &[IndexedDeckSourceSelection {
            manifest,
            expected_cartridge_id,
            archive_sha256,
        }],
        selected_profile,
        device,
        SelectedSourceScope::Candidate,
    )
}

/// Check one complete selected Library-index source set with the same exact
/// slot/profile/signal/timing policy used immediately before worker launch.
///
/// # Errors
///
/// Returns the stable compatibility reason for the complete selection.
pub fn check_indexed_deck_source_set_compatibility(
    codec: &CodecPackManifest,
    deck: &DeckPackManifest,
    sources: &[IndexedDeckSourceSelection<'_>],
    selected_profile: &ProfileKey,
    device: DeviceKind,
) -> Result<(), DeckSelectionV2Error> {
    validate_indexed_deck_sources(
        codec,
        deck,
        sources,
        selected_profile,
        device,
        SelectedSourceScope::CompleteSet,
    )
}

fn validate_indexed_deck_sources(
    codec: &CodecPackManifest,
    deck: &DeckPackManifest,
    sources: &[IndexedDeckSourceSelection<'_>],
    selected_profile: &ProfileKey,
    device: DeviceKind,
    scope: SelectedSourceScope,
) -> Result<(), DeckSelectionV2Error> {
    let sources = sources
        .iter()
        .map(|input| {
            let source = source_facts_from_manifest(input.manifest, input.archive_sha256, 0)?;
            if source.cartridge_id != input.expected_cartridge_id {
                return Err(DeckSelectionV2Error::PackageInvalid);
            }
            Ok(source)
        })
        .collect::<Result<Vec<_>, DeckSelectionV2Error>>()?;
    validate_selected_compatibility(
        codec,
        deck,
        &sources,
        Some(selected_profile),
        device,
        true,
        crate::product_version(),
        scope,
    )
}

/// Resolve and retain one exact Deck/Codec pair and all source cartridges.
///
/// # Errors
///
/// Returns one stable compatibility reason before worker/GPU allocation.  No
/// failure is converted, resized, cropped, re-encoded, or retried through P1.
pub fn prepare_exact_deck_selection(
    roots: &ExtensionRoots,
    selection: &DeckPackageSelectionV2,
    source_inputs: &[DeckSourceSelectionV2<'_>],
    app_version: &str,
) -> Result<PreparedDeckSelectionV2, DeckSelectionV2Error> {
    prepare_exact_deck_selection_with_cache(
        roots,
        &ActivePackageCache::new(),
        selection,
        source_inputs,
        app_version,
    )
}

/// Resolve one exact selection through a process-owned active-package cache.
///
/// LC validation remains local to this preparation and deduplicates repeated
/// physical selections while retaining an owned read-only handle for every
/// logical slot.
///
/// # Errors
///
/// Returns the same exact compatibility refusal as
/// [`prepare_exact_deck_selection`].
pub fn prepare_exact_deck_selection_with_cache(
    roots: &ExtensionRoots,
    package_cache: &ActivePackageCache,
    selection: &DeckPackageSelectionV2,
    source_inputs: &[DeckSourceSelectionV2<'_>],
    app_version: &str,
) -> Result<PreparedDeckSelectionV2, DeckSelectionV2Error> {
    if source_inputs.is_empty()
        || source_inputs.len() > MAX_SOURCES
        || (selection.device == DeviceKind::Cpu && selection.device_ordinal != 0)
    {
        return Err(DeckSelectionV2Error::PackageInvalid);
    }

    let codec_package = package_cache
        .resolve_active(roots, &selection.codec)
        .map_err(|error| selection_error_from_extension(&error))?;
    let deck_package = package_cache
        .resolve_active(roots, &selection.deck)
        .map_err(|error| selection_error_from_extension(&error))?;
    let deck_runtime = ActiveDeckRuntime::from_active_package(deck_package)?;

    let codec_manifest = match codec_package.manifest() {
        PackageManifest::Codec(manifest) => manifest,
        PackageManifest::Deck(_) => return Err(DeckSelectionV2Error::PackageInvalid),
    };
    let deck_manifest = match deck_runtime.active_package().manifest() {
        PackageManifest::Deck(manifest) => manifest,
        PackageManifest::Codec(_) => return Err(DeckSelectionV2Error::PackageInvalid),
    };
    validate_package_contracts(codec_manifest, deck_manifest, app_version)?;
    if usize::from(deck_manifest.signal.slots) != source_inputs.len()
        || usize::from(deck_runtime.operator_descriptor().source_count) != source_inputs.len()
    {
        return Err(DeckSelectionV2Error::UnsupportedSignal);
    }

    if !required_assets_are_bound(codec_manifest, selection) {
        validate_selected_compatibility(
            codec_manifest,
            deck_manifest,
            &[],
            None,
            selection.device,
            false,
            app_version,
            SelectedSourceScope::Candidate,
        )?;
    }
    let external_assets = external_asset_bindings(codec_manifest, selection)?;
    let retained_external_assets = retained_external_asset_bindings(&external_assets, selection)?;
    let (cartridges, sources, validation_work) = open_sources(source_inputs)?;
    let first = sources
        .first()
        .ok_or(DeckSelectionV2Error::PackageInvalid)?;
    validate_selected_compatibility(
        codec_manifest,
        deck_manifest,
        &sources,
        Some(&first.profile_key),
        selection.device,
        true,
        app_version,
        SelectedSourceScope::CompleteSet,
    )?;

    let host = build_host(
        codec_manifest,
        selection,
        first,
        &sources,
        &external_assets,
        app_version,
    )?;
    let retained_external_asset_clones = retained_external_assets.len();

    Ok(PreparedDeckSelectionV2 {
        codec_package,
        deck_runtime,
        cartridges,
        host,
        external_assets,
        retained_external_assets,
        sources,
        validation_work: DeckSelectionValidationWorkV2 {
            retained_external_asset_clones,
            ..validation_work
        },
    })
}

fn open_sources(
    source_inputs: &[DeckSourceSelectionV2<'_>],
) -> Result<
    (
        Vec<IntegrityValidatedCartridge>,
        Vec<DeckSourceFactsV2>,
        DeckSelectionValidationWorkV2,
    ),
    DeckSelectionV2Error,
> {
    let mut cartridges = Vec::<IntegrityValidatedCartridge>::with_capacity(source_inputs.len());
    let mut sources = Vec::<DeckSourceFactsV2>::with_capacity(source_inputs.len());
    let mut validated = BTreeMap::<(PathBuf, String, String), usize>::new();
    let mut validation_work = DeckSelectionValidationWorkV2::default();
    for source in source_inputs {
        let key = (
            source.path.to_path_buf(),
            source.cartridge_id.to_owned(),
            source.archive_sha256.to_owned(),
        );
        if let Some(index) = validated.get(&key).copied() {
            let cartridge = cartridges[index]
                .try_clone_retained()
                .map_err(|_| DeckSelectionV2Error::PackageInvalid)?;
            cartridges.push(cartridge);
            sources.push(sources[index].clone());
            validation_work.retained_handle_clones = validation_work
                .retained_handle_clones
                .checked_add(1)
                .ok_or(DeckSelectionV2Error::PackageInvalid)?;
            continue;
        }
        let cartridge = if let Some(validated_cartridge) = source.validated_cartridge {
            validation_work.retained_handle_clones = validation_work
                .retained_handle_clones
                .checked_add(1)
                .ok_or(DeckSelectionV2Error::PackageInvalid)?;
            validated_cartridge
                .try_clone_retained()
                .map_err(|_| DeckSelectionV2Error::PackageInvalid)?
        } else {
            validation_work.full_cartridge_validations = validation_work
                .full_cartridge_validations
                .checked_add(1)
                .ok_or(DeckSelectionV2Error::PackageInvalid)?;
            open_integrity_validated(source.path, &ValidationOptions::default())
                .map_err(|_| DeckSelectionV2Error::PackageInvalid)?
        };
        let facts = source_facts(&cartridge)?;
        if facts.cartridge_id != source.cartridge_id
            || facts.archive_sha256 != source.archive_sha256
        {
            return Err(DeckSelectionV2Error::PackageInvalid);
        }
        validated.insert(key, cartridges.len());
        cartridges.push(cartridge);
        sources.push(facts);
    }
    Ok((cartridges, sources, validation_work))
}

fn build_host(
    codec: &CodecPackManifest,
    selection: &DeckPackageSelectionV2,
    first: &DeckSourceFactsV2,
    sources: &[DeckSourceFactsV2],
    external_assets: &[ExternalAssetBinding],
    app_version: &str,
) -> Result<DeckSessionV2HostContract, DeckSelectionV2Error> {
    let maximum_estimated_host_bytes = sources
        .iter()
        .try_fold(0_u64, |total, source| {
            total.checked_add(source.tensor_storage_bytes)
        })
        .filter(|bytes| *bytes > 0)
        .ok_or(DeckSelectionV2Error::PackageInvalid)?;
    let asset_bytes = external_assets
        .iter()
        .try_fold(0_u64, |total, asset| total.checked_add(asset.byte_length))
        .ok_or(DeckSelectionV2Error::PackageInvalid)?;
    let maximum_estimated_device_bytes = match selection.device {
        DeviceKind::Cpu => 0,
        DeviceKind::Cuda => maximum_estimated_host_bytes
            .checked_add(asset_bytes)
            .ok_or(DeckSelectionV2Error::PackageInvalid)?,
    };
    let python_minor = codec
        .compatibility
        .python
        .version
        .strip_prefix("3.")
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or(DeckSelectionV2Error::UnsupportedTensorAbi)?;
    let heartbeat_hard_timeout_ms = codec.worker.heartbeat_timeout_ms;
    Ok(DeckSessionV2HostContract {
        app_version: app_version.to_owned(),
        deck_session_id: Uuid::new_v4(),
        ring_id: Uuid::new_v4(),
        profile_key: first.profile_key.clone(),
        signal_geometry: first.signal_geometry.clone(),
        tensor_abi: TensorAbi {
            python_major: 3,
            python_minor,
            torch_version: codec.compatibility.torch_exact_build.clone(),
            dtype: first.tensor_dtype,
            shape: [
                1,
                first.signal_geometry.channels,
                1,
                first.signal_geometry.latent_height,
                first.signal_geometry.latent_width,
            ],
            contiguous: true,
            device: selection.device,
        },
        decoded_abi: DecodedAbi {
            pixel_format: "rgba8".to_owned(),
            maximum_batch: MAX_DECODE_BATCH,
        },
        maximum_estimated_host_bytes,
        maximum_estimated_device_bytes,
        device_ordinal: selection.device_ordinal,
        ring_slot_count: RING_SLOT_COUNT,
        stream_generation: 1,
        heartbeat_interval_ms: (heartbeat_hard_timeout_ms / 4).max(250),
        heartbeat_hard_timeout_ms,
        command_timeout: DEFAULT_COMMAND_TIMEOUT,
    })
}

fn validate_package_contracts(
    codec: &CodecPackManifest,
    deck: &DeckPackManifest,
    app_version: &str,
) -> Result<(), DeckSelectionV2Error> {
    compatibility_result(resolve_package_compatibility(deck, codec, app_version).reason)
}

#[allow(clippy::too_many_arguments)]
fn validate_selected_compatibility(
    codec: &CodecPackManifest,
    deck: &DeckPackManifest,
    sources: &[DeckSourceFactsV2],
    selected_profile: Option<&ProfileKey>,
    device: DeviceKind,
    assets_present: bool,
    app_version: &str,
    source_scope: SelectedSourceScope,
) -> Result<(), DeckSelectionV2Error> {
    let selected_profile =
        selected_profile.map(|profile| latentdeck_extension_manager::ProfileKey {
            codec_family: profile.codec_family.clone(),
            profile: profile.profile.clone(),
            profile_version: profile.profile_version.clone(),
        });
    let device = match device {
        DeviceKind::Cpu => TensorDevice::Cpu,
        DeviceKind::Cuda => TensorDevice::Cuda,
    };
    let sources = sources
        .iter()
        .map(|source| {
            Ok(SelectedSourceCompatibility {
                lc_spec_version: source.lc_spec_version.clone(),
                profile: latentdeck_extension_manager::ProfileKey {
                    codec_family: source.profile_key.codec_family.clone(),
                    profile: source.profile_key.profile.clone(),
                    profile_version: source.profile_key.profile_version.clone(),
                },
                geometry: ManifestSignalGeometry {
                    dtype: match source.tensor_dtype {
                        TensorDtype::Float16 => ManifestTensorDtype::Fp16,
                        TensorDtype::Float32 => ManifestTensorDtype::Fp32,
                        TensorDtype::Bfloat16 => {
                            return Err(DeckSelectionV2Error::UnsupportedTensorAbi);
                        }
                    },
                    device,
                    batch: 1,
                    channels: u16::try_from(source.signal_geometry.channels)
                        .map_err(|_| DeckSelectionV2Error::PackageInvalid)?,
                    temporal: 1,
                    height: source.signal_geometry.latent_height,
                    width: source.signal_geometry.latent_width,
                },
                decoded_height: source.signal_geometry.decoded_height,
                decoded_width: source.signal_geometry.decoded_width,
                frame_rate_numerator: source.signal_geometry.frame_rate_numerator,
                frame_rate_denominator: source.signal_geometry.frame_rate_denominator,
                timing_contract: source.signal_geometry.timing_contract.clone(),
                timing_contract_version: source.signal_geometry.timing_contract_version.clone(),
            })
        })
        .collect::<Result<Vec<_>, DeckSelectionV2Error>>()?;
    compatibility_result(
        resolve_selected_compatibility(
            deck,
            codec,
            app_version,
            assets_present,
            selected_profile.as_ref(),
            device,
            &sources,
            source_scope,
        )
        .reason,
    )
}

const fn compatibility_result(reason: CompatibilityReason) -> Result<(), DeckSelectionV2Error> {
    match reason {
        CompatibilityReason::Compatible => Ok(()),
        CompatibilityReason::Untrusted => Err(DeckSelectionV2Error::Untrusted),
        CompatibilityReason::MissingAsset => Err(DeckSelectionV2Error::MissingAsset),
        CompatibilityReason::PackageInvalid => Err(DeckSelectionV2Error::PackageInvalid),
        CompatibilityReason::UnsupportedProtocol => Err(DeckSelectionV2Error::UnsupportedProtocol),
        CompatibilityReason::UnsupportedHostApi => Err(DeckSelectionV2Error::UnsupportedHostApi),
        CompatibilityReason::UnsupportedTensorAbi => {
            Err(DeckSelectionV2Error::UnsupportedTensorAbi)
        }
        CompatibilityReason::UnsupportedProfile => Err(DeckSelectionV2Error::UnsupportedProfile),
        CompatibilityReason::UnsupportedSignal => Err(DeckSelectionV2Error::UnsupportedSignal),
        CompatibilityReason::UnsupportedTiming => Err(DeckSelectionV2Error::UnsupportedTiming),
        CompatibilityReason::UnsupportedCapability => {
            Err(DeckSelectionV2Error::UnsupportedCapability)
        }
    }
}

fn required_assets_are_bound(
    manifest: &CodecPackManifest,
    selection: &DeckPackageSelectionV2,
) -> bool {
    manifest.external_assets.iter().all(|asset| {
        !asset.required
            || selection.external_assets.contains_key(&asset.asset_id)
            || selection
                .retained_external_assets
                .get(&asset.asset_id)
                .is_some_and(|retained| {
                    retained.binding().sha256 == asset.sha256
                        && retained.binding().byte_length == asset.byte_length
                })
    })
}

fn external_asset_bindings(
    manifest: &CodecPackManifest,
    selection: &DeckPackageSelectionV2,
) -> Result<Vec<ExternalAssetBinding>, DeckSelectionV2Error> {
    if selection.external_assets.keys().any(|asset_id| {
        !manifest
            .external_assets
            .iter()
            .any(|asset| &asset.asset_id == asset_id)
    }) {
        return Err(DeckSelectionV2Error::PackageInvalid);
    }
    manifest
        .external_assets
        .iter()
        .filter_map(|asset| {
            if let Some(retained) = selection
                .retained_external_assets
                .get(&asset.asset_id)
                .filter(|retained| {
                    retained.binding().sha256 == asset.sha256
                        && retained.binding().byte_length == asset.byte_length
                })
            {
                return Some(Ok(retained.binding().clone()));
            }
            selection.external_assets.get(&asset.asset_id).map_or_else(
                || {
                    if asset.required {
                        Some(Err(DeckSelectionV2Error::MissingAsset))
                    } else {
                        None
                    }
                },
                |path| {
                    Some(
                        path.to_str()
                            .filter(|path| !path.is_empty())
                            .ok_or(DeckSelectionV2Error::PackageInvalid)
                            .map(|path| ExternalAssetBinding {
                                asset_id: asset.asset_id.clone(),
                                path: path.to_owned(),
                                sha256: asset.sha256.clone(),
                                byte_length: asset.byte_length,
                            }),
                    )
                },
            )
        })
        .collect()
}

fn retained_external_asset_bindings(
    bindings: &[ExternalAssetBinding],
    selection: &DeckPackageSelectionV2,
) -> Result<Vec<IntegrityValidatedExternalAsset>, DeckSelectionV2Error> {
    let retained = bindings
        .iter()
        .filter_map(|binding| {
            selection
                .retained_external_assets
                .get(&binding.asset_id)
                .map(|retained| {
                    if retained.binding() == binding {
                        Ok(retained.clone_retained())
                    } else {
                        Err(DeckSelectionV2Error::PackageInvalid)
                    }
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(windows)]
    {
        Ok(retained)
    }
    #[cfg(not(windows))]
    {
        drop(retained);
        Ok(Vec::new())
    }
}

fn source_facts(
    cartridge: &IntegrityValidatedCartridge,
) -> Result<DeckSourceFactsV2, DeckSelectionV2Error> {
    source_facts_from_manifest(
        cartridge.manifest(),
        &cartridge.receipt().archive_sha256.to_string(),
        cartridge.receipt().tensor_storage_bytes,
    )
}

fn source_facts_from_manifest(
    manifest: &ManifestV0_1,
    archive_sha256: &str,
    tensor_storage_bytes: u64,
) -> Result<DeckSourceFactsV2, DeckSelectionV2Error> {
    let visual = manifest
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Visual)
        .ok_or(DeckSelectionV2Error::PackageInvalid)?;
    let [batch, channels, latent_slots, latent_height, latent_width]: [u64; 5] = visual
        .shape
        .clone()
        .try_into()
        .map_err(|_| DeckSelectionV2Error::PackageInvalid)?;
    if batch != 1 || latent_slots == 0 {
        return Err(DeckSelectionV2Error::PackageInvalid);
    }
    let tensor_dtype = match visual.runtime_dtype {
        DType::F16 => TensorDtype::Float16,
        DType::F32 => TensorDtype::Float32,
        _ => return Err(DeckSelectionV2Error::UnsupportedTensorAbi),
    };
    let video = &manifest.timing.decoded_video;
    Ok(DeckSourceFactsV2 {
        cartridge_id: manifest.cartridge_id.0.clone(),
        archive_sha256: archive_sha256.to_owned(),
        lc_spec_version: manifest.spec_version.0.clone(),
        profile_key: ProfileKey {
            codec_family: manifest.codec.family.0.clone(),
            profile: manifest.codec.profile.0.clone(),
            profile_version: manifest.codec.profile_version.0.clone(),
        },
        signal_geometry: SignalGeometry {
            channels: u32::try_from(channels)
                .map_err(|_| DeckSelectionV2Error::UnsupportedSignal)?,
            latent_height: u32::try_from(latent_height)
                .map_err(|_| DeckSelectionV2Error::UnsupportedSignal)?,
            latent_width: u32::try_from(latent_width)
                .map_err(|_| DeckSelectionV2Error::UnsupportedSignal)?,
            decoded_height: video.height,
            decoded_width: video.width,
            frame_rate_numerator: u32::try_from(video.frame_rate.numerator)
                .map_err(|_| DeckSelectionV2Error::UnsupportedTiming)?,
            frame_rate_denominator: u32::try_from(video.frame_rate.denominator)
                .map_err(|_| DeckSelectionV2Error::UnsupportedTiming)?,
            timing_contract: manifest.timing.contract.0.clone(),
            timing_contract_version: manifest.timing.contract_version.0.clone(),
        },
        tensor_dtype,
        latent_slot_count: latent_slots,
        tensor_storage_bytes,
    })
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use latentdeck_extension_manager::ExternalAssetDescriptor;
    #[cfg(windows)]
    use sha2::{Digest as _, Sha256};

    use super::*;

    fn bundled_deck(source_name: &str) -> DeckPackManifest {
        let source = match source_name {
            "d2" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../operators/builtin/d2/package/deck-pack.json"
            )),
            "q4" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../operators/builtin/q4/package/deck-pack.json"
            )),
            _ => panic!("unknown bundled Deck fixture"),
        };
        serde_json::from_str(source).expect("bundled Deck manifest uses the closed v1 schema")
    }

    fn h3_source(
        latent_height: u32,
        latent_width: u32,
        decoded_height: u32,
        decoded_width: u32,
    ) -> DeckSourceFactsV2 {
        DeckSourceFactsV2 {
            cartridge_id: "550e8400-e29b-41d4-a716-446655440001".to_owned(),
            archive_sha256: "bb".repeat(32),
            lc_spec_version: "0.1.0".to_owned(),
            profile_key: ProfileKey {
                codec_family: "minimax_h3".to_owned(),
                profile: "h3_av_latent".to_owned(),
                profile_version: "0.1.0".to_owned(),
            },
            signal_geometry: SignalGeometry {
                channels: 24,
                latent_height,
                latent_width,
                decoded_height,
                decoded_width,
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
                timing_contract: "minimax_h3_causal".to_owned(),
                timing_contract_version: "0.1.0".to_owned(),
            },
            tensor_dtype: TensorDtype::Float16,
            latent_slot_count: 32,
            tensor_storage_bytes: 1,
        }
    }

    fn h3_codec() -> CodecPackManifest {
        serde_json::from_value(serde_json::json!({
            "manifest_version": "2.0.0",
            "kind": "codec_pack",
            "pack_id": "org.latentdeck.codec.h3",
            "pack_version": "0.2.0",
            "display_name": "H3 test codec",
            "summary": "CPU-free preflight fixture.",
            "publisher": {"name": "LatentDeck", "url": null, "identity_claim": "self_declared"},
            "license": {"spdx_or_label": "test-only", "notice_path": "NOTICE.txt"},
            "platform": {"os": "windows", "arch": "x86_64"},
            "compatibility": {
                "app_min_inclusive": "0.1.0",
                "app_max_exclusive": "1.0.0",
                "worker_protocol": 2,
                "codec_adapter_api": 1,
                "tensor_abi": "latentdeck.tensor.v1",
                "python": {
                    "implementation": "cpython",
                    "version": "3.13",
                    "platform_tag": "win_amd64"
                },
                "torch_exact_build": "2.13.0+cu130",
                "lc_spec_versions": ["0.1.0"],
                "profiles": [{
                    "codec_family": "minimax_h3",
                    "profile": "h3_av_latent",
                    "profile_version": "0.1.0"
                }]
            },
            "adapter": {
                "adapter_id": "org.latentdeck.codec.h3.adapter",
                "adapter_version": "0.2.0",
                "entrypoint": "latentdeck_codec_h3.adapter:H3Adapter"
            },
            "worker": {
                "executable": "runtime/python.exe",
                "arguments": [],
                "working_directory": "runtime",
                "start_timeout_ms": 1000,
                "heartbeat_timeout_ms": 5000
            },
            "capabilities": [
                "player", "realtime", "resample", "snapshot_capture", "live_capture"
            ],
            "external_assets": [],
            "runtime_lock": {"path": "runtime/runtime.lock", "sha256": "aa"},
            "integrity": {"catalog_path": "integrity.json", "catalog_sha256": "bb"}
        }))
        .expect("closed H3 preflight fixture")
    }

    fn h3_manifest() -> ManifestV0_1 {
        serde_json::from_value(serde_json::json!({
            "spec_version": "0.1.0",
            "cartridge_id": "550e8400-e29b-41d4-a716-446655440001",
            "codec": {
                "family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_version": "0.1.0"
            },
            "payloads": [{
                "path": "payloads/h3.safetensors",
                "media_type": "application/vnd.safetensors",
                "byte_length": 1,
                "sha256": "aa".repeat(32)
            }],
            "tensors": [{
                "stream": "visual",
                "name": "video",
                "payload": "payloads/h3.safetensors",
                "storage_dtype": "F16",
                "runtime_dtype": "F16",
                "shape": [1, 24, 32, 50, 28]
            }],
            "timing": {
                "contract": "minimax_h3_causal",
                "contract_version": "0.1.0",
                "decoded_video": {
                    "width": 448,
                    "height": 800,
                    "frame_count": 107,
                    "frame_rate": {"numerator": 24, "denominator": 1},
                    "duration": {"numerator": 107, "denominator": 24}
                }
            },
            "audio": {"policy": "source_absent"},
            "provenance": {
                "created_by": {"name": "core-tests", "version": "0.1.0"},
                "sources": []
            },
            "parent_cartridges": [],
            "operation_history": []
        }))
        .expect("indexed H3 manifest fixture")
    }

    #[test]
    fn selection_keeps_two_exact_versions_and_has_no_newest_form() {
        let selection = DeckPackageSelectionV2::new(
            "org.example.deck".to_owned(),
            "1.2.3".to_owned(),
            "org.example.codec".to_owned(),
            "2.3.4".to_owned(),
            DeviceKind::Cpu,
        );

        assert_eq!(selection.deck().package_version, "1.2.3");
        assert_eq!(selection.codec().package_version, "2.3.4");
        assert_eq!(selection.device(), DeviceKind::Cpu);
    }

    #[test]
    fn errors_are_the_exact_public_matrix_reasons() {
        let cases = [
            (DeckSelectionV2Error::Untrusted, "untrusted"),
            (DeckSelectionV2Error::MissingAsset, "missing_asset"),
            (DeckSelectionV2Error::PackageInvalid, "package_invalid"),
            (
                DeckSelectionV2Error::UnsupportedProtocol,
                "unsupported_protocol",
            ),
            (
                DeckSelectionV2Error::UnsupportedHostApi,
                "unsupported_host_api",
            ),
            (
                DeckSelectionV2Error::UnsupportedTensorAbi,
                "unsupported_tensor_abi",
            ),
            (
                DeckSelectionV2Error::UnsupportedProfile,
                "unsupported_profile",
            ),
            (
                DeckSelectionV2Error::UnsupportedSignal,
                "unsupported_signal",
            ),
            (
                DeckSelectionV2Error::UnsupportedTiming,
                "unsupported_timing",
            ),
            (
                DeckSelectionV2Error::UnsupportedCapability,
                "unsupported_capability",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.code(), expected);
        }
        assert_eq!(
            DeckSelectionV2Error::ExtensionLifecycle(ExtensionErrorCode::LifecycleBusy).code(),
            "extension.lifecycle_busy"
        );
    }

    #[test]
    fn package_resolution_preserves_invalid_untrusted_and_transient_classes() {
        for code in [
            ExtensionErrorCode::InvalidArguments,
            ExtensionErrorCode::ArchiveInvalid,
            ExtensionErrorCode::ManifestInvalid,
            ExtensionErrorCode::IntegrityFailed,
        ] {
            assert_eq!(
                selection_error_from_extension(&ExtensionError::new(code, "private detail")),
                DeckSelectionV2Error::PackageInvalid
            );
        }
        for code in [
            ExtensionErrorCode::PackageMissing,
            ExtensionErrorCode::PackageDisabled,
            ExtensionErrorCode::PackageUntrusted,
        ] {
            assert_eq!(
                selection_error_from_extension(&ExtensionError::new(code, "private detail")),
                DeckSelectionV2Error::Untrusted
            );
        }
        for code in [
            ExtensionErrorCode::PackageExists,
            ExtensionErrorCode::PackageActive,
            ExtensionErrorCode::LifecycleBusy,
            ExtensionErrorCode::LifecycleConflict,
            ExtensionErrorCode::Io,
        ] {
            assert_eq!(
                selection_error_from_extension(&ExtensionError::new(code, "private detail")),
                DeckSelectionV2Error::ExtensionLifecycle(code)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn retained_external_asset_is_reused_only_for_the_exact_current_descriptor() {
        let root = tempfile::tempdir().expect("temporary external asset root");
        let path = root.path().join("decoder.safetensors");
        let bytes = b"exact current decoder asset";
        std::fs::write(&path, bytes).expect("write decoder asset");
        let sha256 = hex::encode(Sha256::digest(bytes));
        let descriptor = ExternalAssetDescriptor {
            asset_id: "decoder".to_owned(),
            display_name: "Decoder".to_owned(),
            required: true,
            byte_length: u64::try_from(bytes.len()).expect("asset length"),
            sha256: sha256.clone(),
            source_url: None,
            license_label: "test-only".to_owned(),
            license_url: None,
        };
        let binding = ExternalAssetBinding {
            asset_id: descriptor.asset_id.clone(),
            path: path.to_string_lossy().into_owned(),
            sha256,
            byte_length: descriptor.byte_length,
        };
        let retained = IntegrityValidatedExternalAsset::validate_and_retain(binding.clone())
            .expect("validate exact external asset once");
        let mut selection = DeckPackageSelectionV2::new(
            "org.example.deck".to_owned(),
            "1.0.0".to_owned(),
            "org.latentdeck.codec.h3".to_owned(),
            "0.2.0".to_owned(),
            DeviceKind::Cuda,
        );
        selection.bind_integrity_validated_external_asset(retained);
        let mut codec = h3_codec();
        codec.external_assets.push(descriptor);

        let bindings = external_asset_bindings(&codec, &selection)
            .expect("current descriptor accepts retained evidence");
        assert_eq!(bindings, vec![binding]);
        let retained = retained_external_asset_bindings(&bindings, &selection)
            .expect("prepare clones retained evidence without rehashing");
        assert_eq!(retained.len(), 1);

        codec.external_assets[0].sha256 = "ff".repeat(32);
        assert_eq!(
            external_asset_bindings(&codec, &selection),
            Err(DeckSelectionV2Error::MissingAsset),
            "same id/version with changed descriptor requires an explicit rebind"
        );
    }

    #[test]
    fn source_set_reports_profile_signal_and_timing_without_adaptation() {
        let codec = h3_codec();
        let deck = bundled_deck("d2");
        let first = h3_source(50, 28, 800, 448);

        let mut profile = first.clone();
        profile.profile_key.profile = "other".to_owned();
        assert_eq!(
            validate_selected_compatibility(
                &codec,
                &deck,
                &[first.clone(), profile],
                Some(&first.profile_key),
                DeviceKind::Cuda,
                true,
                crate::product_version(),
                SelectedSourceScope::CompleteSet,
            ),
            Err(DeckSelectionV2Error::UnsupportedProfile)
        );

        let mut signal = first.clone();
        signal.signal_geometry.latent_width = 13;
        assert_eq!(
            validate_selected_compatibility(
                &codec,
                &deck,
                &[first.clone(), signal],
                Some(&first.profile_key),
                DeviceKind::Cuda,
                true,
                crate::product_version(),
                SelectedSourceScope::CompleteSet,
            ),
            Err(DeckSelectionV2Error::UnsupportedSignal)
        );

        let mut timing = first.clone();
        timing.signal_geometry.timing_contract_version = "2.0.0".to_owned();
        assert_eq!(
            validate_selected_compatibility(
                &codec,
                &deck,
                &[first.clone(), timing],
                Some(&first.profile_key),
                DeviceKind::Cuda,
                true,
                crate::product_version(),
                SelectedSourceScope::CompleteSet,
            ),
            Err(DeckSelectionV2Error::UnsupportedTiming)
        );
    }

    #[test]
    fn bundled_decks_preflight_the_three_accepted_h3_geometries_without_gpu_allocation() {
        let codec = h3_codec();
        let accepted = [
            h3_source(50, 28, 800, 448),
            h3_source(48, 28, 768, 448),
            h3_source(48, 84, 768, 1_344),
        ];
        for deck_name in ["d2", "q4"] {
            let deck = bundled_deck(deck_name);
            assert_eq!(deck.signal.geometry_allowlist.len(), 4);
            for source in &accepted {
                validate_selected_compatibility(
                    &codec,
                    &deck,
                    std::slice::from_ref(source),
                    Some(&source.profile_key),
                    DeviceKind::Cuda,
                    true,
                    crate::product_version(),
                    SelectedSourceScope::Candidate,
                )
                .expect("accepted H3 tensor geometry must match exactly at preflight");
            }

            let mut unsupported_signal = h3_source(49, 28, 784, 448);
            assert_eq!(
                validate_selected_compatibility(
                    &codec,
                    &deck,
                    std::slice::from_ref(&unsupported_signal),
                    Some(&unsupported_signal.profile_key),
                    DeviceKind::Cuda,
                    true,
                    crate::product_version(),
                    SelectedSourceScope::Candidate,
                ),
                Err(DeckSelectionV2Error::UnsupportedSignal)
            );
            unsupported_signal.tensor_dtype = TensorDtype::Float32;
            assert_eq!(
                validate_selected_compatibility(
                    &codec,
                    &deck,
                    std::slice::from_ref(&unsupported_signal),
                    Some(&unsupported_signal.profile_key),
                    DeviceKind::Cuda,
                    true,
                    crate::product_version(),
                    SelectedSourceScope::Candidate,
                ),
                Err(DeckSelectionV2Error::UnsupportedTensorAbi)
            );
            assert_eq!(
                validate_selected_compatibility(
                    &codec,
                    &deck,
                    std::slice::from_ref(&accepted[0]),
                    Some(&accepted[0].profile_key),
                    DeviceKind::Cpu,
                    true,
                    crate::product_version(),
                    SelectedSourceScope::Candidate,
                ),
                Err(DeckSelectionV2Error::UnsupportedTensorAbi)
            );
            let mut unsupported_profile = accepted[0].clone();
            unsupported_profile.profile_key.profile = "other".to_owned();
            assert_eq!(
                validate_selected_compatibility(
                    &codec,
                    &deck,
                    std::slice::from_ref(&unsupported_profile),
                    Some(&unsupported_profile.profile_key),
                    DeviceKind::Cuda,
                    true,
                    crate::product_version(),
                    SelectedSourceScope::Candidate,
                ),
                Err(DeckSelectionV2Error::UnsupportedProfile)
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn indexed_eligibility_uses_exact_launch_profile_signal_and_timing_reasons() {
        let codec = h3_codec();
        let deck = bundled_deck("q4");
        let profile = ProfileKey {
            codec_family: "minimax_h3".to_owned(),
            profile: "h3_av_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        };
        let expected_id = "550e8400-e29b-41d4-a716-446655440001";
        let archive_sha256 = &"bb".repeat(32);
        let manifest = h3_manifest();

        assert_eq!(
            check_indexed_deck_source_compatibility(
                &codec,
                &deck,
                &manifest,
                expected_id,
                archive_sha256,
                &profile,
                DeviceKind::Cuda,
            ),
            Ok(())
        );
        let selected = IndexedDeckSourceSelection {
            manifest: &manifest,
            expected_cartridge_id: expected_id,
            archive_sha256,
        };
        assert_eq!(
            check_indexed_deck_source_set_compatibility(
                &codec,
                &deck,
                &[selected; 4],
                &profile,
                DeviceKind::Cuda,
            ),
            Ok(())
        );

        let mut mixed_manifest = manifest.clone();
        mixed_manifest.timing.decoded_video.width = 512;
        let mixed = [
            selected,
            IndexedDeckSourceSelection {
                manifest: &mixed_manifest,
                expected_cartridge_id: expected_id,
                archive_sha256,
            },
            selected,
            selected,
        ];
        assert_eq!(
            check_indexed_deck_source_set_compatibility(
                &codec,
                &deck,
                &mixed,
                &profile,
                DeviceKind::Cuda,
            ),
            Err(DeckSelectionV2Error::UnsupportedSignal)
        );

        let mut wrong_profile = profile.clone();
        wrong_profile.profile = "other".to_owned();
        assert_eq!(
            check_indexed_deck_source_compatibility(
                &codec,
                &deck,
                &manifest,
                expected_id,
                archive_sha256,
                &wrong_profile,
                DeviceKind::Cuda,
            ),
            Err(DeckSelectionV2Error::UnsupportedProfile)
        );
        assert_eq!(
            check_indexed_deck_source_compatibility(
                &codec,
                &deck,
                &manifest,
                "550e8400-e29b-41d4-a716-446655440099",
                archive_sha256,
                &profile,
                DeviceKind::Cuda,
            ),
            Err(DeckSelectionV2Error::PackageInvalid)
        );

        let mut unsupported_signal = manifest.clone();
        unsupported_signal.tensors[0].shape[4] = 29;
        assert_eq!(
            check_indexed_deck_source_compatibility(
                &codec,
                &deck,
                &unsupported_signal,
                expected_id,
                archive_sha256,
                &profile,
                DeviceKind::Cuda,
            ),
            Err(DeckSelectionV2Error::UnsupportedSignal)
        );

        let mut unsupported_timing = manifest;
        unsupported_timing.timing.decoded_video.frame_rate.numerator = 25;
        assert_eq!(
            check_indexed_deck_source_compatibility(
                &codec,
                &deck,
                &unsupported_timing,
                expected_id,
                archive_sha256,
                &profile,
                DeviceKind::Cuda,
            ),
            Err(DeckSelectionV2Error::UnsupportedTiming)
        );
    }
}
