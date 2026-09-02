//! Exact user-selected Codec Pack v2 and cartridge launch preparation.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use latentdeck_cartridge::{
    manifest::{DType, TensorStream},
    reader::{IntegrityValidatedCartridge, ValidationOptions, open_integrity_validated},
};
use latentdeck_control::v2::{
    DecodedAbi, DeviceKind, ExternalAssetBinding, ProfileKey, SignalGeometry, TensorAbi,
    TensorDtype,
};
use latentdeck_core::{
    player::{
        CartridgeSummary, CodecState, CodecSummary, DecoderVariantSummary,
        PlayerProtocol2SourceInputs,
    },
    player_session_v2::PlayerSessionV2HostContract,
};
use latentdeck_extension_manager::{
    ActiveInstalledPackage, CodecCapability, ExtensionRoots, PackageKind, PackageManifest,
    PackageReference, resolve_active,
};
use thiserror::Error;
use uuid::Uuid;

const MAX_DECODE_BATCH: u8 = 24;
const RING_SLOT_COUNT: u8 = 2;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Debug)]
pub struct PlayerCodecSelectionV2 {
    package: PackageReference,
    device: DeviceKind,
    device_ordinal: u8,
    external_assets: BTreeMap<String, PathBuf>,
}

impl PlayerCodecSelectionV2 {
    #[must_use]
    pub fn new(package_id: String, package_version: String, device: DeviceKind) -> Self {
        Self {
            package: PackageReference {
                kind: PackageKind::CodecPack,
                package_id,
                package_version,
            },
            device,
            device_ordinal: 0,
            external_assets: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn package(&self) -> &PackageReference {
        &self.package
    }

    pub fn bind_external_asset(&mut self, asset_id: String, path: PathBuf) {
        self.external_assets.insert(asset_id, path);
    }
}

pub struct PreparedPlayerV2Launch {
    pub package: ActiveInstalledPackage,
    pub cartridge: IntegrityValidatedCartridge,
    pub host: PlayerSessionV2HostContract,
    pub external_assets: Vec<ExternalAssetBinding>,
    pub cartridge_summary: CartridgeSummary,
    pub latent_slot_count: u64,
}

#[derive(Debug, Error)]
pub enum PlayerSelectionV2Error {
    #[error("no exact Codec Pack v2 version is selected")]
    MissingSelection,
    #[error("the exact selected Codec Pack is not active and trusted")]
    PackageUnavailable,
    #[error("the selected package is not a Player-capable Codec Pack v2")]
    PackageIncompatible,
    #[error("the selected Codec Pack requires an external asset that is not bound")]
    MissingAsset,
    #[error("the selected external asset identity is not declared by the exact package")]
    AssetInvalid,
    #[error("the cartridge failed codec-neutral integrity validation")]
    CartridgeInvalid,
    #[error("the cartridge cannot be represented by the Protocol 2 host ABI")]
    CartridgeIncompatible,
}

impl PlayerSelectionV2Error {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MissingSelection => "codec.selection_missing",
            Self::PackageUnavailable => "codec.package_unavailable",
            Self::PackageIncompatible => "codec.package_incompatible",
            Self::MissingAsset => "codec.asset_missing",
            Self::AssetInvalid => "codec.asset_incompatible",
            Self::CartridgeInvalid => "player.cartridge_invalid",
            Self::CartridgeIncompatible => "codec.profile_incompatible",
        }
    }
}

pub fn validate_exact_selection(
    roots: &ExtensionRoots,
    selection: &PlayerCodecSelectionV2,
) -> Result<CodecSummary, PlayerSelectionV2Error> {
    let package = resolve_active(roots, selection.package())
        .map_err(|_| PlayerSelectionV2Error::PackageUnavailable)?;
    codec_summary(&package, selection)
}

pub fn prepare_exact_launch(
    roots: &ExtensionRoots,
    selection: Option<&PlayerCodecSelectionV2>,
    source: &PlayerProtocol2SourceInputs<'_>,
    app_version: &str,
    loop_enabled: bool,
) -> Result<PreparedPlayerV2Launch, PlayerSelectionV2Error> {
    let selection = selection.ok_or(PlayerSelectionV2Error::MissingSelection)?;
    let package = resolve_active(roots, selection.package())
        .map_err(|_| PlayerSelectionV2Error::PackageUnavailable)?;
    let manifest = codec_manifest(&package)?;
    let external_assets = external_asset_bindings(manifest, selection)?;
    let cartridge = open_integrity_validated(source.cartridge_path, &ValidationOptions::default())
        .map_err(|_| PlayerSelectionV2Error::CartridgeInvalid)?;
    let host = host_contract(
        manifest,
        &cartridge,
        &external_assets,
        selection,
        app_version,
        loop_enabled,
    )?;
    let latent_slot_count = cartridge
        .manifest()
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Visual)
        .and_then(|tensor| tensor.shape.get(2))
        .copied()
        .filter(|count| *count > 0)
        .ok_or(PlayerSelectionV2Error::CartridgeIncompatible)?;
    Ok(PreparedPlayerV2Launch {
        package,
        cartridge,
        host,
        external_assets,
        cartridge_summary: source.cartridge.clone(),
        latent_slot_count,
    })
}

fn codec_manifest(
    package: &ActiveInstalledPackage,
) -> Result<&latentdeck_extension_manager::CodecPackManifest, PlayerSelectionV2Error> {
    let PackageManifest::Codec(manifest) = package.manifest() else {
        return Err(PlayerSelectionV2Error::PackageIncompatible);
    };
    if manifest.manifest_version != "2.0.0"
        || manifest.compatibility.worker_protocol != 2
        || !manifest.capabilities.contains(&CodecCapability::Player)
    {
        return Err(PlayerSelectionV2Error::PackageIncompatible);
    }
    Ok(manifest)
}

fn codec_summary(
    package: &ActiveInstalledPackage,
    selection: &PlayerCodecSelectionV2,
) -> Result<CodecSummary, PlayerSelectionV2Error> {
    let manifest = codec_manifest(package)?;
    let ready = manifest
        .external_assets
        .iter()
        .filter(|asset| asset.required)
        .all(|asset| selection.external_assets.contains_key(&asset.asset_id));
    let single_asset = (manifest.external_assets.len() == 1).then(|| &manifest.external_assets[0]);
    Ok(CodecSummary {
        state: if ready {
            CodecState::Ready
        } else {
            CodecState::Missing
        },
        display_name: Some(manifest.display_name.clone()),
        detail: (!ready).then(|| "Bind every required external codec asset.".to_owned()),
        pack_id: Some(manifest.pack_id.clone()),
        pack_version: Some(manifest.pack_version.clone()),
        publisher_name: Some(manifest.publisher.name.clone()),
        publisher_url: manifest.publisher.url.clone(),
        pack_license_label: Some(manifest.license.spdx_or_label.clone()),
        decoder_asset_id: single_asset.map(|asset| asset.asset_id.clone()),
        decoder_display_name: single_asset.map(|asset| asset.display_name.clone()),
        decoder_variants: manifest
            .external_assets
            .iter()
            .map(|asset| DecoderVariantSummary {
                variant_id: asset.asset_id.clone(),
                sha256: asset.sha256.clone(),
                byte_length: asset.byte_length,
                source_url: asset.source_url.clone().unwrap_or_default(),
                license_label: asset.license_label.clone(),
                license_url: asset.license_url.clone().unwrap_or_default(),
                selected: selection.external_assets.contains_key(&asset.asset_id),
            })
            .collect(),
    })
}

fn external_asset_bindings(
    manifest: &latentdeck_extension_manager::CodecPackManifest,
    selection: &PlayerCodecSelectionV2,
) -> Result<Vec<ExternalAssetBinding>, PlayerSelectionV2Error> {
    if selection.external_assets.keys().any(|id| {
        !manifest
            .external_assets
            .iter()
            .any(|asset| &asset.asset_id == id)
    }) {
        return Err(PlayerSelectionV2Error::AssetInvalid);
    }
    manifest
        .external_assets
        .iter()
        .filter_map(|asset| {
            selection.external_assets.get(&asset.asset_id).map_or_else(
                || {
                    if asset.required {
                        Some(Err(PlayerSelectionV2Error::MissingAsset))
                    } else {
                        None
                    }
                },
                |path| {
                    Some(
                        path.to_str()
                            .ok_or(PlayerSelectionV2Error::AssetInvalid)
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

fn host_contract(
    manifest: &latentdeck_extension_manager::CodecPackManifest,
    cartridge: &IntegrityValidatedCartridge,
    assets: &[ExternalAssetBinding],
    selection: &PlayerCodecSelectionV2,
    app_version: &str,
    loop_enabled: bool,
) -> Result<PlayerSessionV2HostContract, PlayerSelectionV2Error> {
    let cartridge_manifest = cartridge.manifest();
    let visual = cartridge_manifest
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Visual)
        .ok_or(PlayerSelectionV2Error::CartridgeIncompatible)?;
    let [batch, channels, _temporal, latent_height, latent_width]: [u64; 5] = visual
        .shape
        .clone()
        .try_into()
        .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?;
    if batch != 1 {
        return Err(PlayerSelectionV2Error::CartridgeIncompatible);
    }
    let dtype = match visual.runtime_dtype {
        DType::F16 => TensorDtype::Float16,
        DType::F32 => TensorDtype::Float32,
        _ => return Err(PlayerSelectionV2Error::CartridgeIncompatible),
    };
    let python_minor = manifest
        .compatibility
        .python
        .version
        .strip_prefix("3.")
        .and_then(|value| value.parse::<u8>().ok())
        .ok_or(PlayerSelectionV2Error::PackageIncompatible)?;
    let video = &cartridge_manifest.timing.decoded_video;
    let asset_bytes = assets.iter().try_fold(0_u64, |sum, asset| {
        sum.checked_add(asset.byte_length)
            .ok_or(PlayerSelectionV2Error::PackageIncompatible)
    })?;
    let maximum_estimated_host_bytes = cartridge.receipt().payload_bytes;
    let maximum_estimated_device_bytes = match selection.device {
        DeviceKind::Cpu => 0,
        DeviceKind::Cuda => maximum_estimated_host_bytes
            .checked_add(asset_bytes)
            .ok_or(PlayerSelectionV2Error::PackageIncompatible)?,
    };
    let heartbeat_hard_timeout_ms = manifest.worker.heartbeat_timeout_ms;
    Ok(PlayerSessionV2HostContract {
        app_version: app_version.to_owned(),
        player_session_id: Uuid::new_v4(),
        ring_id: Uuid::new_v4(),
        profile_key: ProfileKey {
            codec_family: cartridge_manifest.codec.family.0.clone(),
            profile: cartridge_manifest.codec.profile.0.clone(),
            profile_version: cartridge_manifest.codec.profile_version.0.clone(),
        },
        signal_geometry: SignalGeometry {
            channels: u32::try_from(channels)
                .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
            latent_height: u32::try_from(latent_height)
                .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
            latent_width: u32::try_from(latent_width)
                .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
            decoded_height: video.height,
            decoded_width: video.width,
            frame_rate_numerator: u32::try_from(video.frame_rate.numerator)
                .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
            frame_rate_denominator: u32::try_from(video.frame_rate.denominator)
                .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
            timing_contract: cartridge_manifest.timing.contract.0.clone(),
            timing_contract_version: cartridge_manifest.timing.contract_version.0.clone(),
        },
        tensor_abi: TensorAbi {
            python_major: 3,
            python_minor,
            torch_version: manifest.compatibility.torch_exact_build.clone(),
            dtype,
            shape: [
                1,
                u32::try_from(channels)
                    .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
                1,
                u32::try_from(latent_height)
                    .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
                u32::try_from(latent_width)
                    .map_err(|_| PlayerSelectionV2Error::CartridgeIncompatible)?,
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
        loop_enabled,
        heartbeat_interval_ms: (heartbeat_hard_timeout_ms / 4).max(100),
        heartbeat_hard_timeout_ms,
        command_timeout: DEFAULT_COMMAND_TIMEOUT,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_is_exact_and_never_has_an_auto_or_newest_form() {
        let selection = PlayerCodecSelectionV2::new(
            "org.example.codec".to_owned(),
            "0.2.0".to_owned(),
            DeviceKind::Cpu,
        );

        assert_eq!(selection.package().kind, PackageKind::CodecPack);
        assert_eq!(selection.package().package_id, "org.example.codec");
        assert_eq!(selection.package().package_version, "0.2.0");
    }
}
