use std::collections::BTreeSet;

use latentdeck_deck_runtime_contracts::{
    AssetState, CodecPackageProvides, CodecPackageVersion, CompatibilityReason,
    CompatibilityResolver, ContractId, DeckPackageRequirements, DeckPackageVersion,
    DeckTimingContract, FrameTimingContract, HostApiRequirement, PackageCompatibilityDecision,
    PackageHostRuntime, PackageIdentity, PackageReadiness, PackageRuntimeContract, PackageState,
    ProfileContract, SelectedSourceContract, SourceSelectionScope, TensorGeometryContract,
    TrustState,
};
use semver::Version;

fn set<T: Ord>(values: impl IntoIterator<Item = T>) -> BTreeSet<T> {
    values.into_iter().collect()
}

fn id(value: &str) -> ContractId {
    ContractId::new(value).expect("valid contract id")
}

fn version(value: &str) -> Version {
    Version::parse(value).expect("valid semantic version")
}

fn requirement(value: &str) -> HostApiRequirement {
    HostApiRequirement::parse(value).expect("valid host API requirement")
}

fn profile(name: &str) -> ProfileContract {
    ProfileContract {
        codec_family: id("h3"),
        profile: id(name),
        profile_version: version("1.0.0"),
    }
}

fn runtime(torch_exact_build: &str) -> PackageRuntimeContract {
    PackageRuntimeContract {
        tensor_abi: id("torch-bcthw-contiguous-v1"),
        python_implementation: id("cpython"),
        python_version: version("3.13.0"),
        python_platform: id("win-amd64"),
        torch_exact_build: torch_exact_build.to_owned(),
    }
}

fn geometry() -> TensorGeometryContract {
    TensorGeometryContract {
        dtype: id("float16"),
        device: id("cuda"),
        batch: 1,
        channels: 16,
        temporal: 1,
        height: 30,
        width: 45,
    }
}

fn timing() -> FrameTimingContract {
    FrameTimingContract {
        frame_rate_numerator: 30,
        frame_rate_denominator: 1,
    }
}

fn capabilities() -> BTreeSet<ContractId> {
    set([id("player"), id("realtime"), id("resample")])
}

fn deck() -> DeckPackageVersion {
    DeckPackageVersion {
        identity: PackageIdentity::new(id("org.latentdeck.d2"), version("0.1.0+deck.7")),
        readiness: PackageReadiness::READY,
        requires: Some(DeckPackageRequirements {
            slots: 2,
            protocol_version: 2,
            app_host_api: requirement(">=0.1.0, <0.2.0"),
            deck_host_api: 1,
            deck_operator_api: 1,
            runtime: runtime("2.8.0+cu130"),
            profile_allowlist: Some(set([profile("h3-video")])),
            geometries: set([geometry()]),
            timing: DeckTimingContract {
                frame: timing(),
                samples_per_slot: 1_024,
            },
            capabilities: capabilities(),
        }),
    }
}

fn codec() -> CodecPackageVersion {
    CodecPackageVersion {
        identity: PackageIdentity::new(id("org.latentdeck.h3"), version("0.1.1+codec.4")),
        readiness: PackageReadiness::READY,
        provides: Some(CodecPackageProvides {
            protocol_version: 2,
            app_host_api: requirement("^0.1.0"),
            codec_adapter_api: 1,
            runtime: runtime("2.8.0+cu130"),
            lc_spec_versions: set([version("0.1.0")]),
            profiles: set([profile("h3-video")]),
            capabilities: capabilities(),
        }),
    }
}

fn host() -> PackageHostRuntime {
    PackageHostRuntime {
        app_version: version("0.1.7"),
        protocol_versions: set([2]),
        deck_host_apis: set([1]),
        deck_operator_apis: set([1]),
        codec_adapter_apis: set([1]),
        tensor_abis: set([id("torch-bcthw-contiguous-v1")]),
        python_implementations: set([id("cpython")]),
        python_versions: set([version("3.13.0")]),
        python_platforms: set([id("win-amd64")]),
        lc_spec_versions: set([version("0.1.0")]),
        tensor_dtypes: set([id("float16")]),
        tensor_devices: set([id("cuda")]),
        samples_per_slot: set([1_024]),
        capabilities: capabilities(),
    }
}

fn source() -> SelectedSourceContract {
    SelectedSourceContract {
        lc_spec_version: version("0.1.0"),
        profile: profile("h3-video"),
        geometry: geometry(),
        decoded_height: 480,
        decoded_width: 720,
        timing: timing(),
        timing_contract: id("fixed-frame-step"),
        timing_contract_version: version("1.0.0"),
    }
}

fn package_decision(
    host: &PackageHostRuntime,
    deck: &DeckPackageVersion,
    codec: &CodecPackageVersion,
) -> PackageCompatibilityDecision {
    CompatibilityResolver::resolve_package_pair(host, deck, codec)
        .expect("valid package host fixture")
}

#[allow(clippy::too_many_arguments)]
fn selected_decision(
    host: &PackageHostRuntime,
    deck: &DeckPackageVersion,
    codec: &CodecPackageVersion,
    assets: AssetState,
    selected_profile: Option<&ProfileContract>,
    selected_device: Option<&ContractId>,
    sources: &[SelectedSourceContract],
    scope: SourceSelectionScope,
) -> PackageCompatibilityDecision {
    CompatibilityResolver::resolve_selected_pair(
        host,
        deck,
        codec,
        assets,
        selected_profile,
        selected_device,
        sources,
        scope,
    )
    .expect("valid package host fixture")
}

fn baseline_selected_decision(
    host: &PackageHostRuntime,
    deck: &DeckPackageVersion,
    codec: &CodecPackageVersion,
    sources: &[SelectedSourceContract],
    scope: SourceSelectionScope,
) -> PackageCompatibilityDecision {
    let selected_profile = profile("h3-video");
    let selected_device = id("cuda");
    selected_decision(
        host,
        deck,
        codec,
        AssetState::Present,
        Some(&selected_profile),
        Some(&selected_device),
        sources,
        scope,
    )
}

fn assert_incompatible(decision: &PackageCompatibilityDecision, expected: CompatibilityReason) {
    assert_eq!(decision.reason, expected);
    assert!(!decision.is_compatible());
    assert!(
        decision.compatible_profiles.is_empty(),
        "{expected} retained profiles: {:?}",
        decision.compatible_profiles
    );
}

#[test]
fn current_fixture_is_compatible_at_both_stages() {
    let host = host();
    let deck = deck();
    let codec = codec();

    let package = package_decision(&host, &deck, &codec);
    assert_eq!(package.reason, CompatibilityReason::Compatible);
    assert!(package.is_compatible());
    assert_eq!(package.deck, deck.identity);
    assert_eq!(package.codec, codec.identity);
    assert_eq!(package.compatible_profiles, set([profile("h3-video")]));

    let selected = baseline_selected_decision(
        &host,
        &deck,
        &codec,
        &[source(), source()],
        SourceSelectionScope::CompleteSet,
    );
    assert_eq!(selected.reason, CompatibilityReason::Compatible);
    assert!(selected.is_compatible());
    assert_eq!(selected.compatible_profiles, set([profile("h3-video")]));
}

#[test]
fn package_stage_emits_every_package_reason_and_clears_profiles() {
    let base_host = host();
    let base_deck = deck();
    let base_codec = codec();

    let mut candidate = base_deck.clone();
    candidate.readiness.trust = TrustState::Untrusted;
    assert_incompatible(
        &package_decision(&base_host, &candidate, &base_codec),
        CompatibilityReason::Untrusted,
    );

    let mut candidate = base_deck.clone();
    candidate.readiness.package = PackageState::Invalid;
    assert_incompatible(
        &package_decision(&base_host, &candidate, &base_codec),
        CompatibilityReason::PackageInvalid,
    );

    let mut candidate = base_deck.clone();
    candidate
        .requires
        .as_mut()
        .expect("deck contract")
        .protocol_version = 3;
    assert_incompatible(
        &package_decision(&base_host, &candidate, &base_codec),
        CompatibilityReason::UnsupportedProtocol,
    );

    let mut candidate = base_deck.clone();
    candidate
        .requires
        .as_mut()
        .expect("deck contract")
        .app_host_api = requirement(">=9.0.0, <10.0.0");
    assert_incompatible(
        &package_decision(&base_host, &candidate, &base_codec),
        CompatibilityReason::UnsupportedHostApi,
    );

    let mut candidate = base_codec.clone();
    candidate
        .provides
        .as_mut()
        .expect("codec contract")
        .runtime
        .python_version = version("3.12.0");
    assert_incompatible(
        &package_decision(&base_host, &base_deck, &candidate),
        CompatibilityReason::UnsupportedTensorAbi,
    );

    let mut candidate = base_codec.clone();
    candidate
        .provides
        .as_mut()
        .expect("codec contract")
        .profiles = set([profile("other-profile")]);
    assert_incompatible(
        &package_decision(&base_host, &base_deck, &candidate),
        CompatibilityReason::UnsupportedProfile,
    );

    let mut candidate = base_codec.clone();
    candidate
        .provides
        .as_mut()
        .expect("codec contract")
        .capabilities
        .remove(&id("realtime"));
    assert_incompatible(
        &package_decision(&base_host, &base_deck, &candidate),
        CompatibilityReason::UnsupportedCapability,
    );
}

#[test]
fn exact_torch_build_metadata_is_part_of_package_runtime_identity() {
    let host = host();
    let deck = deck();
    let mut codec = codec();
    codec
        .provides
        .as_mut()
        .expect("codec contract")
        .runtime
        .torch_exact_build = "2.8.0+cpu".to_owned();

    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedTensorAbi,
    );

    codec
        .provides
        .as_mut()
        .expect("codec contract")
        .runtime
        .torch_exact_build = "2.8.0+cu130".to_owned();
    assert_eq!(
        package_decision(&host, &deck, &codec).reason,
        CompatibilityReason::Compatible
    );
}

#[test]
fn package_requires_at_least_one_geometry_supported_by_the_host() {
    let deck = deck();
    let codec = codec();

    let mut unsupported_dtype = host();
    unsupported_dtype.tensor_dtypes = set([id("float32")]);
    assert_incompatible(
        &package_decision(&unsupported_dtype, &deck, &codec),
        CompatibilityReason::UnsupportedTensorAbi,
    );

    let mut unsupported_device = host();
    unsupported_device.tensor_devices = set([id("cpu")]);
    assert_incompatible(
        &package_decision(&unsupported_device, &deck, &codec),
        CompatibilityReason::UnsupportedTensorAbi,
    );
}

#[test]
fn disjoint_lc_spec_versions_are_an_unsupported_profile() {
    let host = host();
    let deck = deck();
    let mut codec = codec();
    codec
        .provides
        .as_mut()
        .expect("codec contract")
        .lc_spec_versions = set([version("0.2.0")]);

    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedProfile,
    );
}

#[test]
fn package_reason_precedence_is_stable_when_multiple_constraints_fail() {
    let host = host();
    let mut deck = deck();
    let mut codec = codec();

    deck.readiness.trust = TrustState::Untrusted;
    deck.readiness.package = PackageState::Invalid;
    deck.requires = None;
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::Untrusted,
    );

    deck.readiness.trust = TrustState::Trusted;
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::PackageInvalid,
    );

    codec
        .provides
        .as_mut()
        .expect("codec contract")
        .capabilities
        .remove(&id("realtime"));
    deck = crate_deck_with_all_later_failures();
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedProtocol,
    );

    deck.requires
        .as_mut()
        .expect("deck contract")
        .protocol_version = 2;
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedHostApi,
    );

    deck.requires.as_mut().expect("deck contract").app_host_api = requirement("^0.1.0");
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedTensorAbi,
    );

    deck.requires.as_mut().expect("deck contract").runtime = runtime("2.8.0+cu130");
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedProfile,
    );

    deck.requires
        .as_mut()
        .expect("deck contract")
        .profile_allowlist = Some(set([profile("h3-video")]));
    assert_incompatible(
        &package_decision(&host, &deck, &codec),
        CompatibilityReason::UnsupportedCapability,
    );

    codec
        .provides
        .as_mut()
        .expect("codec contract")
        .capabilities = capabilities();
    assert_eq!(
        package_decision(&host, &deck, &codec).reason,
        CompatibilityReason::Compatible
    );
}

fn crate_deck_with_all_later_failures() -> DeckPackageVersion {
    let mut value = deck();
    let contract = value.requires.as_mut().expect("deck contract");
    contract.protocol_version = 3;
    contract.app_host_api = requirement(">=9.0.0, <10.0.0");
    contract.runtime = runtime("2.8.0+cpu");
    contract.profile_allowlist = Some(set([profile("other-profile")]));
    value
}

#[test]
fn selected_stage_reports_missing_assets_before_selection_details() {
    let host = host();
    let deck = deck();
    let codec = codec();

    assert_incompatible(
        &selected_decision(
            &host,
            &deck,
            &codec,
            AssetState::Missing,
            None,
            None,
            &[],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::MissingAsset,
    );
}

#[test]
fn selected_stage_rejects_missing_or_unsupported_selection_identity() {
    let host = host();
    let deck = deck();
    let codec = codec();
    let selected_profile = profile("h3-video");
    let selected_device = id("cuda");

    assert_incompatible(
        &selected_decision(
            &host,
            &deck,
            &codec,
            AssetState::Present,
            None,
            Some(&selected_device),
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::PackageInvalid,
    );
    assert_incompatible(
        &selected_decision(
            &host,
            &deck,
            &codec,
            AssetState::Present,
            Some(&selected_profile),
            None,
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::PackageInvalid,
    );

    let wrong_profile = profile("other-profile");
    assert_incompatible(
        &selected_decision(
            &host,
            &deck,
            &codec,
            AssetState::Present,
            Some(&wrong_profile),
            Some(&selected_device),
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedProfile,
    );

    let wrong_device = id("cpu");
    assert_incompatible(
        &selected_decision(
            &host,
            &deck,
            &codec,
            AssetState::Present,
            Some(&selected_profile),
            Some(&wrong_device),
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTensorAbi,
    );
}

#[test]
fn complete_selection_requires_exactly_one_source_for_every_declared_slot() {
    let host = host();
    let codec = codec();

    for slots in 1..=16 {
        let mut deck = deck();
        deck.requires.as_mut().expect("deck contract").slots = slots;
        let exact_sources = vec![source(); usize::from(slots)];
        let exact = baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &exact_sources,
            SourceSelectionScope::CompleteSet,
        );
        assert_eq!(
            exact.reason,
            CompatibilityReason::Compatible,
            "slot count {slots}"
        );

        let mismatched_count = if slots == 1 {
            2
        } else {
            usize::from(slots - 1)
        };
        let mismatched_sources = vec![source(); mismatched_count];
        assert_incompatible(
            &baseline_selected_decision(
                &host,
                &deck,
                &codec,
                &mismatched_sources,
                SourceSelectionScope::CompleteSet,
            ),
            CompatibilityReason::UnsupportedSignal,
        );
    }
}

#[test]
fn slot_count_outside_one_through_sixteen_is_package_invalid() {
    let host = host();
    let codec = codec();

    for slots in [0, 17] {
        let mut deck = deck();
        deck.requires.as_mut().expect("deck contract").slots = slots;
        assert_incompatible(
            &package_decision(&host, &deck, &codec),
            CompatibilityReason::PackageInvalid,
        );
    }
}

#[test]
fn candidate_scope_can_describe_a_partial_or_empty_source_set() {
    let host = host();
    let mut deck = deck();
    let codec = codec();
    deck.requires.as_mut().expect("deck contract").slots = 16;

    for sources in [Vec::new(), vec![source()]] {
        let decision = baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &sources,
            SourceSelectionScope::Candidate,
        );
        assert_eq!(decision.reason, CompatibilityReason::Compatible);
    }
}

#[test]
fn source_profile_and_lc_spec_are_exact() {
    let host = host();
    let deck = deck();
    let codec = codec();

    let mut wrong_profile = source();
    wrong_profile.profile = profile("other-profile");
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[wrong_profile, source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedProfile,
    );

    let mut wrong_lc_spec = source();
    wrong_lc_spec.lc_spec_version = version("0.2.0");
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[wrong_lc_spec, source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedProfile,
    );
}

#[test]
fn source_dtype_and_device_fail_as_tensor_abi_before_shape() {
    let host = host();
    let deck = deck();
    let codec = codec();

    let mut wrong_dtype = source();
    wrong_dtype.geometry.dtype = id("float32");
    wrong_dtype.geometry.height = 60;
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[wrong_dtype, source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTensorAbi,
    );

    let mut wrong_device = source();
    wrong_device.geometry.device = id("cpu");
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[wrong_device, source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTensorAbi,
    );
}

#[test]
fn latent_and_decoded_geometry_must_match_exactly_across_the_source_set() {
    let host = host();
    let deck = deck();
    let codec = codec();

    let mut latent_shape = source();
    latent_shape.geometry.width += 1;
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[source(), latent_shape],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedSignal,
    );

    let mut decoded_height = source();
    decoded_height.decoded_height += 1;
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[source(), decoded_height],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedSignal,
    );

    let mut decoded_width = source();
    decoded_width.decoded_width += 1;
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[source(), decoded_width],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedSignal,
    );
}

#[test]
fn invalid_zero_decoded_geometry_is_package_invalid() {
    let host = host();
    let deck = deck();
    let codec = codec();
    let mut invalid = source();
    invalid.decoded_width = 0;

    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck,
            &codec,
            &[invalid, source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::PackageInvalid,
    );
}

#[test]
fn fps_timing_contract_and_timing_version_are_exact() {
    let host = host();
    let baseline_deck = deck();
    let codec = codec();

    let mut fps = source();
    fps.timing.frame_rate_numerator = 60;
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &baseline_deck,
            &codec,
            &[source(), fps],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTiming,
    );

    let mut contract = source();
    contract.timing_contract = id("timestamped-frame-step");
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &baseline_deck,
            &codec,
            &[source(), contract],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTiming,
    );

    let mut contract_version = source();
    contract_version.timing_contract_version = version("2.0.0");
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &baseline_deck,
            &codec,
            &[source(), contract_version],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTiming,
    );

    let mut deck_fps = deck();
    deck_fps
        .requires
        .as_mut()
        .expect("deck contract")
        .timing
        .frame
        .frame_rate_numerator = 24;
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &deck_fps,
            &codec,
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTiming,
    );
}

#[test]
fn samples_per_slot_is_a_selected_stage_timing_constraint() {
    let mut host = host();
    let baseline_deck = deck();
    let codec = codec();
    host.samples_per_slot = set([2_048]);

    assert_eq!(
        package_decision(&host, &baseline_deck, &codec).reason,
        CompatibilityReason::Compatible,
        "source-independent package discovery must remain possible"
    );
    assert_incompatible(
        &baseline_selected_decision(
            &host,
            &baseline_deck,
            &codec,
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::UnsupportedTiming,
    );

    let mut invalid_deck = deck();
    invalid_deck
        .requires
        .as_mut()
        .expect("deck contract")
        .timing
        .samples_per_slot = 0;
    assert_incompatible(
        &package_decision(&host, &invalid_deck, &codec),
        CompatibilityReason::PackageInvalid,
    );
}

#[test]
fn selected_stage_resolves_package_readiness_before_asset_state() {
    let host = host();
    let codec = codec();
    let selected_profile = profile("h3-video");
    let selected_device = id("cuda");

    let mut untrusted = deck();
    untrusted.readiness.trust = TrustState::Untrusted;
    untrusted.readiness.package = PackageState::Invalid;
    assert_incompatible(
        &selected_decision(
            &host,
            &untrusted,
            &codec,
            AssetState::Missing,
            Some(&selected_profile),
            Some(&selected_device),
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::Untrusted,
    );

    let mut invalid = deck();
    invalid.readiness.package = PackageState::Invalid;
    assert_incompatible(
        &selected_decision(
            &host,
            &invalid,
            &codec,
            AssetState::Missing,
            Some(&selected_profile),
            Some(&selected_device),
            &[source(), source()],
            SourceSelectionScope::CompleteSet,
        ),
        CompatibilityReason::PackageInvalid,
    );
}
