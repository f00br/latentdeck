use std::collections::BTreeSet;

use latentdeck_deck_runtime_contracts::{
    AssetState, CodecPackageProvides, CodecPackageVersion, CompatibilityReason,
    CompatibilityResolver, ContractId, DeckPackageRequirements, DeckPackageVersion,
    DeckTimingContract, FrameTimingContract, HostApiRequirement, PackageHostRuntime,
    PackageIdentity, PackageReadiness, PackageRuntimeContract, PackageState, ProfileContract,
    SelectedSourceContract, SourceSelectionScope, TensorGeometryContract, TrustState,
};
use semver::Version;

use crate::model::{
    CodecCapability, CodecPackManifest, CompatibilityPair, DeckPackManifest,
    InstalledPackageSummary, PackageHealth, PackageManifest, ProfileKey, SignalGeometry,
    TensorDevice, TensorDtype,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageCompatibility {
    pub reason: CompatibilityReason,
    pub compatible_profiles: Vec<ProfileKey>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedSourceCompatibility {
    pub lc_spec_version: String,
    pub profile: ProfileKey,
    pub geometry: SignalGeometry,
    pub decoded_height: u32,
    pub decoded_width: u32,
    pub frame_rate_numerator: u32,
    pub frame_rate_denominator: u32,
    pub timing_contract: String,
    pub timing_contract_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectedSourceScope {
    Candidate,
    CompleteSet,
}

#[must_use]
pub fn resolve_package_compatibility(
    deck: &DeckPackManifest,
    codec: &CodecPackManifest,
    app_version: &str,
) -> PackageCompatibility {
    resolve_manifests(
        deck,
        PackageReadiness::READY,
        codec,
        PackageReadiness::READY,
        app_version,
        None,
    )
}

#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn resolve_selected_compatibility(
    deck: &DeckPackManifest,
    codec: &CodecPackManifest,
    app_version: &str,
    assets_present: bool,
    selected_profile: Option<&ProfileKey>,
    selected_device: TensorDevice,
    sources: &[SelectedSourceCompatibility],
    source_scope: SelectedSourceScope,
) -> PackageCompatibility {
    resolve_manifests(
        deck,
        PackageReadiness::READY,
        codec,
        PackageReadiness::READY,
        app_version,
        Some(SelectedCompatibility {
            assets_present,
            selected_profile,
            selected_device,
            sources,
            source_scope,
        }),
    )
}

pub(crate) fn resolve_inventory_pair(
    deck: &InstalledPackageSummary,
    deck_manifest: Option<&PackageManifest>,
    codec: &InstalledPackageSummary,
    codec_manifest: Option<&PackageManifest>,
) -> CompatibilityPair {
    let compatibility = match (deck_manifest, codec_manifest) {
        (
            Some(PackageManifest::Deck(deck_manifest)),
            Some(PackageManifest::Codec(codec_manifest)),
        ) => resolve_manifests(
            deck_manifest,
            readiness(deck),
            codec_manifest,
            readiness(codec),
            env!("CARGO_PKG_VERSION"),
            None,
        ),
        _ if is_untrusted(deck) || is_untrusted(codec) => PackageCompatibility {
            reason: CompatibilityReason::Untrusted,
            compatible_profiles: Vec::new(),
        },
        _ => PackageCompatibility {
            reason: CompatibilityReason::PackageInvalid,
            compatible_profiles: Vec::new(),
        },
    };
    let compatible_profile = compatibility.compatible_profiles.first().cloned();
    CompatibilityPair {
        deck: deck.package.clone(),
        codec: codec.package.clone(),
        reason: compatibility.reason,
        compatible_profiles: compatibility.compatible_profiles,
        compatible_profile,
    }
}

struct SelectedCompatibility<'a> {
    assets_present: bool,
    selected_profile: Option<&'a ProfileKey>,
    selected_device: TensorDevice,
    sources: &'a [SelectedSourceCompatibility],
    source_scope: SelectedSourceScope,
}

fn resolve_manifests(
    deck: &DeckPackManifest,
    deck_readiness: PackageReadiness,
    codec: &CodecPackManifest,
    codec_readiness: PackageReadiness,
    app_version: &str,
    selected: Option<SelectedCompatibility<'_>>,
) -> PackageCompatibility {
    let Some(host) = host_runtime(app_version) else {
        return invalid();
    };
    let Some(deck_contract) = deck_version(deck, deck_readiness) else {
        return invalid();
    };
    let Some(codec_contract) = codec_version(codec, codec_readiness) else {
        return invalid();
    };
    let decision = if let Some(selected) = selected {
        let selected_profile = selected.selected_profile.and_then(profile_contract);
        if selected.selected_profile.is_some() && selected_profile.is_none() {
            return invalid();
        }
        let Some(selected_device) = device_contract(selected.selected_device) else {
            return invalid();
        };
        let Some(sources) = selected
            .sources
            .iter()
            .map(selected_source_contract)
            .collect::<Option<Vec<_>>>()
        else {
            return invalid();
        };
        CompatibilityResolver::resolve_selected_pair(
            &host,
            &deck_contract,
            &codec_contract,
            if selected.assets_present {
                AssetState::Present
            } else {
                AssetState::Missing
            },
            selected_profile.as_ref(),
            Some(&selected_device),
            &sources,
            match selected.source_scope {
                SelectedSourceScope::Candidate => SourceSelectionScope::Candidate,
                SelectedSourceScope::CompleteSet => SourceSelectionScope::CompleteSet,
            },
        )
    } else {
        CompatibilityResolver::resolve_package_pair(&host, &deck_contract, &codec_contract)
    };
    let Ok(decision) = decision else {
        return invalid();
    };
    PackageCompatibility {
        reason: decision.reason,
        compatible_profiles: decision
            .compatible_profiles
            .iter()
            .map(profile_key)
            .collect(),
    }
}

fn invalid() -> PackageCompatibility {
    PackageCompatibility {
        reason: CompatibilityReason::PackageInvalid,
        compatible_profiles: Vec::new(),
    }
}

fn readiness(summary: &InstalledPackageSummary) -> PackageReadiness {
    PackageReadiness {
        trust: if is_untrusted(summary) {
            TrustState::Untrusted
        } else {
            TrustState::Trusted
        },
        assets: AssetState::Present,
        package: if summary.health == PackageHealth::Corrupt {
            PackageState::Invalid
        } else {
            PackageState::Valid
        },
    }
}

fn is_untrusted(summary: &InstalledPackageSummary) -> bool {
    summary.health != PackageHealth::Corrupt
        && (!summary.enabled
            || matches!(
                summary.health,
                PackageHealth::Untrusted | PackageHealth::VerificationRequired
            ))
}

fn host_runtime(app_version: &str) -> Option<PackageHostRuntime> {
    Some(PackageHostRuntime {
        app_version: Version::parse(app_version).ok()?,
        protocol_versions: [2].into_iter().collect(),
        deck_host_apis: [1].into_iter().collect(),
        deck_operator_apis: [1].into_iter().collect(),
        codec_adapter_apis: [1].into_iter().collect(),
        tensor_abis: contract_set(["latentdeck.tensor.v1"])?,
        python_implementations: contract_set(["cpython"])?,
        python_versions: [Version::parse("3.13.0").ok()?].into_iter().collect(),
        python_platforms: contract_set(["win_amd64"])?,
        lc_spec_versions: [Version::parse("0.1.0").ok()?].into_iter().collect(),
        tensor_dtypes: contract_set(["float16", "float32"])?,
        tensor_devices: contract_set(["cpu", "cuda"])?,
        samples_per_slot: [24].into_iter().collect(),
        capabilities: capability_contracts([
            CodecCapability::Player,
            CodecCapability::Realtime,
            CodecCapability::Resample,
            CodecCapability::SnapshotCapture,
            CodecCapability::LiveCapture,
            CodecCapability::RawImport,
        ])?,
    })
}

fn deck_version(
    manifest: &DeckPackManifest,
    readiness: PackageReadiness,
) -> Option<DeckPackageVersion> {
    Some(DeckPackageVersion {
        identity: package_identity(&manifest.deck_id, &manifest.deck_version)?,
        readiness,
        requires: Some(DeckPackageRequirements {
            slots: manifest.signal.slots,
            protocol_version: manifest.compatibility.worker_protocol,
            app_host_api: app_requirement(
                &manifest.compatibility.app_min_inclusive,
                &manifest.compatibility.app_max_exclusive,
            )?,
            deck_host_api: manifest.compatibility.deck_host_api,
            deck_operator_api: manifest.compatibility.deck_operator_api,
            runtime: runtime_contract(
                &manifest.compatibility.tensor_abi,
                &manifest.compatibility.python,
                &manifest.compatibility.torch_exact_build,
            )?,
            profile_allowlist: profile_allowlist(manifest.signal.profile_allowlist.as_deref())
                .ok()?,
            geometries: manifest
                .signal
                .geometry_allowlist
                .iter()
                .map(geometry_contract)
                .collect::<Option<BTreeSet<_>>>()?,
            timing: DeckTimingContract {
                frame: FrameTimingContract {
                    frame_rate_numerator: manifest.signal.timing.frames_per_second_numerator,
                    frame_rate_denominator: manifest.signal.timing.frames_per_second_denominator,
                },
                samples_per_slot: manifest.signal.timing.samples_per_slot,
            },
            capabilities: capability_contracts(
                manifest.signal.required_capabilities.iter().copied(),
            )?,
        }),
    })
}

fn codec_version(
    manifest: &CodecPackManifest,
    readiness: PackageReadiness,
) -> Option<CodecPackageVersion> {
    Some(CodecPackageVersion {
        identity: package_identity(&manifest.pack_id, &manifest.pack_version)?,
        readiness,
        provides: Some(CodecPackageProvides {
            protocol_version: manifest.compatibility.worker_protocol,
            app_host_api: app_requirement(
                &manifest.compatibility.app_min_inclusive,
                &manifest.compatibility.app_max_exclusive,
            )?,
            codec_adapter_api: manifest.compatibility.codec_adapter_api,
            runtime: runtime_contract(
                &manifest.compatibility.tensor_abi,
                &manifest.compatibility.python,
                &manifest.compatibility.torch_exact_build,
            )?,
            lc_spec_versions: manifest
                .compatibility
                .lc_spec_versions
                .iter()
                .map(|version| Version::parse(version).ok())
                .collect::<Option<BTreeSet<_>>>()?,
            profiles: manifest
                .compatibility
                .profiles
                .iter()
                .map(profile_contract)
                .collect::<Option<BTreeSet<_>>>()?,
            capabilities: capability_contracts(manifest.capabilities.iter().copied())?,
        }),
    })
}

fn package_identity(id: &str, version: &str) -> Option<PackageIdentity> {
    Some(PackageIdentity::new(
        ContractId::new(id).ok()?,
        Version::parse(version).ok()?,
    ))
}

fn app_requirement(minimum: &str, maximum: &str) -> Option<HostApiRequirement> {
    HostApiRequirement::parse(format!(">={minimum}, <{maximum}")).ok()
}

fn runtime_contract(
    tensor_abi: &str,
    python: &crate::model::PythonConstraint,
    torch: &str,
) -> Option<PackageRuntimeContract> {
    let python_implementation = match python.implementation {
        crate::model::PythonImplementation::Cpython => "cpython",
    };
    Some(PackageRuntimeContract {
        tensor_abi: ContractId::new(tensor_abi).ok()?,
        python_implementation: ContractId::new(python_implementation).ok()?,
        python_version: normalized_python_version(&python.version)?,
        python_platform: ContractId::new(&python.platform_tag).ok()?,
        torch_exact_build: torch.to_owned(),
    })
}

fn normalized_python_version(value: &str) -> Option<Version> {
    Version::parse(value).ok().or_else(|| {
        let mut parts = value.split('.');
        let major = parts.next()?;
        let minor = parts.next()?;
        if parts.next().is_some()
            || !major.bytes().all(|byte| byte.is_ascii_digit())
            || !minor.bytes().all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        Version::parse(&format!("{major}.{minor}.0")).ok()
    })
}

fn profile_allowlist(
    profiles: Option<&[ProfileKey]>,
) -> Result<Option<BTreeSet<ProfileContract>>, ()> {
    match profiles {
        Some(profiles) => Ok(Some(
            profiles
                .iter()
                .map(profile_contract)
                .collect::<Option<BTreeSet<_>>>()
                .ok_or(())?,
        )),
        None => Ok(None),
    }
}

fn profile_contract(profile: &ProfileKey) -> Option<ProfileContract> {
    Some(ProfileContract {
        codec_family: ContractId::new(&profile.codec_family).ok()?,
        profile: ContractId::new(&profile.profile).ok()?,
        profile_version: Version::parse(&profile.profile_version).ok()?,
    })
}

fn profile_key(profile: &ProfileContract) -> ProfileKey {
    ProfileKey {
        codec_family: profile.codec_family.as_str().to_owned(),
        profile: profile.profile.as_str().to_owned(),
        profile_version: profile.profile_version.to_string(),
    }
}

fn geometry_contract(geometry: &SignalGeometry) -> Option<TensorGeometryContract> {
    Some(TensorGeometryContract {
        dtype: dtype_contract(geometry.dtype)?,
        device: device_contract(geometry.device)?,
        batch: u32::from(geometry.batch),
        channels: u32::from(geometry.channels),
        temporal: u32::from(geometry.temporal),
        height: geometry.height,
        width: geometry.width,
    })
}

fn selected_source_contract(
    source: &SelectedSourceCompatibility,
) -> Option<SelectedSourceContract> {
    Some(SelectedSourceContract {
        lc_spec_version: Version::parse(&source.lc_spec_version).ok()?,
        profile: profile_contract(&source.profile)?,
        geometry: geometry_contract(&source.geometry)?,
        decoded_height: source.decoded_height,
        decoded_width: source.decoded_width,
        timing: FrameTimingContract {
            frame_rate_numerator: source.frame_rate_numerator,
            frame_rate_denominator: source.frame_rate_denominator,
        },
        timing_contract: ContractId::new(&source.timing_contract).ok()?,
        timing_contract_version: Version::parse(&source.timing_contract_version).ok()?,
    })
}

fn dtype_contract(dtype: TensorDtype) -> Option<ContractId> {
    ContractId::new(match dtype {
        TensorDtype::Fp16 => "float16",
        TensorDtype::Fp32 => "float32",
    })
    .ok()
}

fn device_contract(device: TensorDevice) -> Option<ContractId> {
    ContractId::new(match device {
        TensorDevice::Cpu => "cpu",
        TensorDevice::Cuda => "cuda",
    })
    .ok()
}

fn capability_contracts(
    capabilities: impl IntoIterator<Item = CodecCapability>,
) -> Option<BTreeSet<ContractId>> {
    capabilities
        .into_iter()
        .map(|capability| ContractId::new(capability_name(capability)).ok())
        .collect()
}

fn capability_name(capability: CodecCapability) -> &'static str {
    match capability {
        CodecCapability::Player => "player",
        CodecCapability::Realtime => "realtime",
        CodecCapability::Resample => "resample",
        CodecCapability::SnapshotCapture => "snapshot_capture",
        CodecCapability::LiveCapture => "live_capture",
        CodecCapability::RawImport => "raw_import",
    }
}

fn contract_set<const N: usize>(values: [&str; N]) -> Option<BTreeSet<ContractId>> {
    values
        .into_iter()
        .map(|value| ContractId::new(value).ok())
        .collect()
}
