//! Exact, codec-neutral Deck + Codec + source preflight for Protocol 2.
//!
//! This boundary deliberately accepts package identities and Library-resolved
//! cartridge identities, never worker paths, entrypoints, or an implicit
//! "newest" selector.  It retains both package trees and every validated LC
//! handle before a worker or GPU allocation is attempted.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};

use latentdeck_cartridge::{
    manifest::{DType, TensorStream},
    reader::{IntegrityValidatedCartridge, ValidationOptions, open_integrity_validated},
};
use latentdeck_control::v2::{
    DecodedAbi, DeviceKind, ExternalAssetBinding, ProfileKey, SignalGeometry, TensorAbi,
    TensorDtype,
};
use latentdeck_extension_manager::{
    ActiveInstalledPackage, CodecPackManifest, DeckPackManifest, ExtensionRoots, PackageKind,
    PackageManifest, PackageReference, TensorDevice, TensorDtype as ManifestTensorDtype,
    resolve_active,
};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    deck_runtime_v2::{ActiveDeckRuntime, DeckRuntimeError},
    deck_session_v2::DeckSessionV2HostContract,
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
        self.external_assets.insert(asset_id, path);
    }
}

/// Exact Library identity supplied to the generic preflight.  The path is
/// never transported to Python; Core reopens and retains it first.
#[derive(Clone, Copy, Debug)]
pub struct DeckSourceSelectionV2<'a> {
    pub path: &'a Path,
    pub cartridge_id: &'a str,
    pub archive_sha256: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckSourceFactsV2 {
    pub cartridge_id: String,
    pub archive_sha256: String,
    pub profile_key: ProfileKey,
    pub signal_geometry: SignalGeometry,
    pub tensor_dtype: TensorDtype,
    pub latent_slot_count: u64,
    pub tensor_storage_bytes: u64,
}

/// Fully retained inputs which can be passed directly to
/// `start_deck_session_v2`; no package or LC is rediscovered by newest version.
pub struct PreparedDeckSelectionV2 {
    pub codec_package: ActiveInstalledPackage,
    pub deck_runtime: ActiveDeckRuntime,
    pub cartridges: Vec<IntegrityValidatedCartridge>,
    pub host: DeckSessionV2HostContract,
    pub external_assets: Vec<ExternalAssetBinding>,
    pub sources: Vec<DeckSourceFactsV2>,
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
        }
    }
}

impl From<DeckRuntimeError> for DeckSelectionV2Error {
    fn from(_: DeckRuntimeError) -> Self {
        Self::PackageInvalid
    }
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
    if source_inputs.is_empty()
        || source_inputs.len() > MAX_SOURCES
        || (selection.device == DeviceKind::Cpu && selection.device_ordinal != 0)
    {
        return Err(DeckSelectionV2Error::PackageInvalid);
    }

    let codec_package =
        resolve_active(roots, &selection.codec).map_err(|_| DeckSelectionV2Error::Untrusted)?;
    let deck_package =
        resolve_active(roots, &selection.deck).map_err(|_| DeckSelectionV2Error::Untrusted)?;
    let deck_runtime = ActiveDeckRuntime::from_active_package(deck_package)?;

    let codec_manifest = match codec_package.manifest() {
        PackageManifest::Codec(manifest) => manifest,
        PackageManifest::Deck(_) => return Err(DeckSelectionV2Error::PackageInvalid),
    };
    let deck_manifest = match deck_runtime.active_package().manifest() {
        PackageManifest::Deck(manifest) => manifest,
        PackageManifest::Codec(_) => return Err(DeckSelectionV2Error::PackageInvalid),
    };
    validate_package_contracts(codec_manifest, deck_manifest, selection, app_version)?;
    if usize::from(deck_manifest.signal.slots) != source_inputs.len()
        || usize::from(deck_runtime.operator_descriptor().source_count) != source_inputs.len()
    {
        return Err(DeckSelectionV2Error::UnsupportedSignal);
    }

    let external_assets = external_asset_bindings(codec_manifest, selection)?;
    let (cartridges, sources) = open_sources(source_inputs)?;
    let first = sources
        .first()
        .ok_or(DeckSelectionV2Error::PackageInvalid)?;
    validate_source_set(first, &sources[1..])?;
    validate_profile(codec_manifest, deck_manifest, first, selection.device)?;

    let host = build_host(
        codec_manifest,
        selection,
        first,
        &sources,
        &external_assets,
        app_version,
    )?;

    Ok(PreparedDeckSelectionV2 {
        codec_package,
        deck_runtime,
        cartridges,
        host,
        external_assets,
        sources,
    })
}

fn validate_source_set(
    first: &DeckSourceFactsV2,
    remaining: &[DeckSourceFactsV2],
) -> Result<(), DeckSelectionV2Error> {
    for source in remaining {
        if source.profile_key != first.profile_key {
            return Err(DeckSelectionV2Error::UnsupportedProfile);
        }
        if source.tensor_dtype != first.tensor_dtype
            || source.signal_geometry.channels != first.signal_geometry.channels
            || source.signal_geometry.latent_height != first.signal_geometry.latent_height
            || source.signal_geometry.latent_width != first.signal_geometry.latent_width
            || source.signal_geometry.decoded_height != first.signal_geometry.decoded_height
            || source.signal_geometry.decoded_width != first.signal_geometry.decoded_width
        {
            return Err(DeckSelectionV2Error::UnsupportedSignal);
        }
        if source.signal_geometry.frame_rate_numerator != first.signal_geometry.frame_rate_numerator
            || source.signal_geometry.frame_rate_denominator
                != first.signal_geometry.frame_rate_denominator
            || source.signal_geometry.timing_contract != first.signal_geometry.timing_contract
            || source.signal_geometry.timing_contract_version
                != first.signal_geometry.timing_contract_version
        {
            return Err(DeckSelectionV2Error::UnsupportedTiming);
        }
    }
    Ok(())
}

fn open_sources(
    source_inputs: &[DeckSourceSelectionV2<'_>],
) -> Result<(Vec<IntegrityValidatedCartridge>, Vec<DeckSourceFactsV2>), DeckSelectionV2Error> {
    let mut cartridges = Vec::with_capacity(source_inputs.len());
    let mut sources = Vec::with_capacity(source_inputs.len());
    for source in source_inputs {
        let cartridge = open_integrity_validated(source.path, &ValidationOptions::default())
            .map_err(|_| DeckSelectionV2Error::PackageInvalid)?;
        let facts = source_facts(&cartridge)?;
        if facts.cartridge_id != source.cartridge_id
            || facts.archive_sha256 != source.archive_sha256
        {
            return Err(DeckSelectionV2Error::PackageInvalid);
        }
        cartridges.push(cartridge);
        sources.push(facts);
    }
    Ok((cartridges, sources))
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
    selection: &DeckPackageSelectionV2,
    app_version: &str,
) -> Result<(), DeckSelectionV2Error> {
    if codec.manifest_version != "2.0.0"
        || deck.manifest_version != "1.0.0"
        || codec.compatibility.worker_protocol != 2
        || deck.compatibility.worker_protocol != 2
    {
        return Err(DeckSelectionV2Error::UnsupportedProtocol);
    }
    let app = semver::Version::parse(app_version)
        .map_err(|_| DeckSelectionV2Error::UnsupportedHostApi)?;
    if !version_in_range(
        &app,
        &codec.compatibility.app_min_inclusive,
        &codec.compatibility.app_max_exclusive,
    ) || !version_in_range(
        &app,
        &deck.compatibility.app_min_inclusive,
        &deck.compatibility.app_max_exclusive,
    ) {
        return Err(DeckSelectionV2Error::UnsupportedHostApi);
    }
    if codec.compatibility.codec_adapter_api != 1
        || deck.compatibility.deck_host_api != 1
        || deck.compatibility.deck_operator_api != 1
        || codec.compatibility.tensor_abi != "latentdeck.tensor.v1"
        || deck.compatibility.tensor_abi != "latentdeck.tensor.v1"
        || codec.compatibility.python != deck.compatibility.python
        || codec.compatibility.torch_exact_build != deck.compatibility.torch_exact_build
    {
        return Err(DeckSelectionV2Error::UnsupportedTensorAbi);
    }
    if !deck.signal.geometry_allowlist.iter().any(|geometry| {
        matches!(
            (geometry.device, selection.device),
            (TensorDevice::Cpu, DeviceKind::Cpu) | (TensorDevice::Cuda, DeviceKind::Cuda)
        )
    }) {
        return Err(DeckSelectionV2Error::UnsupportedTensorAbi);
    }
    let provided: HashSet<_> = codec.capabilities.iter().copied().collect();
    if deck
        .signal
        .required_capabilities
        .iter()
        .any(|required| !provided.contains(required))
    {
        return Err(DeckSelectionV2Error::UnsupportedCapability);
    }
    Ok(())
}

fn validate_profile(
    codec: &CodecPackManifest,
    deck: &DeckPackManifest,
    source: &DeckSourceFactsV2,
    device: DeviceKind,
) -> Result<(), DeckSelectionV2Error> {
    let profile_matches = |profile: &latentdeck_extension_manager::ProfileKey| {
        profile.codec_family == source.profile_key.codec_family
            && profile.profile == source.profile_key.profile
            && profile.profile_version == source.profile_key.profile_version
    };
    if !codec.compatibility.profiles.iter().any(profile_matches)
        || deck
            .signal
            .profile_allowlist
            .as_ref()
            .is_some_and(|profiles| !profiles.iter().any(profile_matches))
    {
        return Err(DeckSelectionV2Error::UnsupportedProfile);
    }
    select_exact_geometry(deck, source, device)?;
    let timing = &deck.signal.timing;
    if timing.frames_per_second_numerator != source.signal_geometry.frame_rate_numerator
        || timing.frames_per_second_denominator != source.signal_geometry.frame_rate_denominator
    {
        return Err(DeckSelectionV2Error::UnsupportedTiming);
    }
    Ok(())
}

fn select_exact_geometry<'a>(
    deck: &'a DeckPackManifest,
    source: &DeckSourceFactsV2,
    device: DeviceKind,
) -> Result<&'a latentdeck_extension_manager::SignalGeometry, DeckSelectionV2Error> {
    let tensor_abi_matches = |geometry: &latentdeck_extension_manager::SignalGeometry| {
        matches!(
            (geometry.dtype, source.tensor_dtype),
            (ManifestTensorDtype::Fp16, TensorDtype::Float16)
                | (ManifestTensorDtype::Fp32, TensorDtype::Float32)
        ) && matches!(
            (geometry.device, device),
            (TensorDevice::Cpu, DeviceKind::Cpu) | (TensorDevice::Cuda, DeviceKind::Cuda)
        )
    };
    if !deck
        .signal
        .geometry_allowlist
        .iter()
        .any(tensor_abi_matches)
    {
        return Err(DeckSelectionV2Error::UnsupportedTensorAbi);
    }
    deck.signal
        .geometry_allowlist
        .iter()
        .find(|geometry| {
            tensor_abi_matches(geometry)
                && geometry.batch == 1
                && geometry.temporal == 1
                && u32::from(geometry.channels) == source.signal_geometry.channels
                && geometry.height == source.signal_geometry.latent_height
                && geometry.width == source.signal_geometry.latent_width
        })
        .ok_or(DeckSelectionV2Error::UnsupportedSignal)
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

fn source_facts(
    cartridge: &IntegrityValidatedCartridge,
) -> Result<DeckSourceFactsV2, DeckSelectionV2Error> {
    let manifest = cartridge.manifest();
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
        archive_sha256: cartridge.receipt().archive_sha256.to_string(),
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
        tensor_storage_bytes: cartridge.receipt().tensor_storage_bytes,
    })
}

fn version_in_range(version: &semver::Version, minimum: &str, maximum: &str) -> bool {
    semver::Version::parse(minimum)
        .ok()
        .zip(semver::Version::parse(maximum).ok())
        .is_some_and(|(minimum, maximum)| version >= &minimum && version < &maximum)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> DeckSourceFactsV2 {
        DeckSourceFactsV2 {
            cartridge_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            archive_sha256: "aa".repeat(32),
            profile_key: ProfileKey {
                codec_family: "synthetic".to_owned(),
                profile: "latent".to_owned(),
                profile_version: "1.0.0".to_owned(),
            },
            signal_geometry: SignalGeometry {
                channels: 4,
                latent_height: 8,
                latent_width: 12,
                decoded_height: 64,
                decoded_width: 96,
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
                timing_contract: "synthetic_ticks".to_owned(),
                timing_contract_version: "1.0.0".to_owned(),
            },
            tensor_dtype: TensorDtype::Float32,
            latent_slot_count: 8,
            tensor_storage_bytes: 12_288,
        }
    }

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
    }

    #[test]
    fn source_set_reports_profile_signal_and_timing_without_adaptation() {
        let first = source();

        let mut profile = source();
        profile.profile_key.profile = "other".to_owned();
        assert_eq!(
            validate_source_set(&first, &[profile]),
            Err(DeckSelectionV2Error::UnsupportedProfile)
        );

        let mut signal = source();
        signal.signal_geometry.latent_width = 13;
        assert_eq!(
            validate_source_set(&first, &[signal]),
            Err(DeckSelectionV2Error::UnsupportedSignal)
        );

        let mut timing = source();
        timing.signal_geometry.timing_contract_version = "2.0.0".to_owned();
        assert_eq!(
            validate_source_set(&first, &[timing]),
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
                let selected = select_exact_geometry(&deck, source, DeviceKind::Cuda)
                    .expect("accepted H3 geometry has one exact allowlist entry");
                assert_eq!(
                    u32::from(selected.channels),
                    source.signal_geometry.channels
                );
                assert_eq!(selected.height, source.signal_geometry.latent_height);
                assert_eq!(selected.width, source.signal_geometry.latent_width);
                validate_profile(&codec, &deck, source, DeviceKind::Cuda)
                    .expect("accepted H3 tensor geometry must match exactly at preflight");
            }

            let mut unsupported_signal = h3_source(49, 28, 784, 448);
            assert_eq!(
                validate_profile(&codec, &deck, &unsupported_signal, DeviceKind::Cuda),
                Err(DeckSelectionV2Error::UnsupportedSignal)
            );
            unsupported_signal.tensor_dtype = TensorDtype::Float32;
            assert_eq!(
                validate_profile(&codec, &deck, &unsupported_signal, DeviceKind::Cuda),
                Err(DeckSelectionV2Error::UnsupportedTensorAbi)
            );
            assert_eq!(
                validate_profile(&codec, &deck, &accepted[0], DeviceKind::Cpu),
                Err(DeckSelectionV2Error::UnsupportedTensorAbi)
            );
            let mut unsupported_profile = accepted[0].clone();
            unsupported_profile.profile_key.profile = "other".to_owned();
            assert_eq!(
                validate_profile(&codec, &deck, &unsupported_profile, DeviceKind::Cuda),
                Err(DeckSelectionV2Error::UnsupportedProfile)
            );
        }
    }
}
