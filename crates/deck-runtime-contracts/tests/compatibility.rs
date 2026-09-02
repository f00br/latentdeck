use std::collections::BTreeSet;

use latentdeck_deck_runtime_contracts::{
    AssetState, COMPATIBILITY_REASON_PRECEDENCE, CodecContract, CodecVersion, CompatibilityReason,
    CompatibilityResolver, ContractId, ContractValidationError, DeckRequirements, DeckVersion,
    HostApiRequirement, HostRuntime, MatrixError, PackageIdentity, PackageReadiness, PackageState,
    ProfileContract, SignalContract, TensorAbiContract, TimingContract, TrustState,
};
use semver::Version;

fn id(value: &str) -> ContractId {
    ContractId::new(value).expect("valid contract id")
}

fn version(value: &str) -> Version {
    Version::parse(value).expect("valid semantic version")
}

fn requirement(value: &str) -> HostApiRequirement {
    HostApiRequirement::parse(value).expect("valid host API requirement")
}

fn tensor_abi() -> TensorAbiContract {
    TensorAbiContract {
        python_implementation: id("cpython"),
        python_version: version("3.13.0"),
        torch_version: version("2.8.0"),
        dtype: id("float16"),
        layout: id("bcthw-contiguous"),
    }
}

fn profile() -> ProfileContract {
    ProfileContract {
        codec_family: id("h3"),
        profile: id("h3-video"),
        profile_version: version("1.0.0"),
    }
}

fn signal() -> SignalContract {
    SignalContract {
        channels: 16,
        latent_height: 30,
        latent_width: 45,
        decoded_height: 480,
        decoded_width: 720,
        pixel_format: id("rgba8"),
    }
}

fn timing() -> TimingContract {
    TimingContract {
        contract: id("fixed-frame-step"),
        contract_version: version("1.0.0"),
        frame_rate_numerator: 30,
        frame_rate_denominator: 1,
    }
}

fn capabilities() -> BTreeSet<ContractId> {
    [id("player"), id("realtime"), id("resample")]
        .into_iter()
        .collect()
}

fn package_identity(package_id: &str, package_version: &str) -> PackageIdentity {
    PackageIdentity::new(id(package_id), version(package_version))
}

fn deck(package_id: &str, package_version: &str) -> DeckVersion {
    DeckVersion {
        identity: package_identity(package_id, package_version),
        readiness: PackageReadiness::READY,
        requires: DeckRequirements {
            protocol_version: 2,
            host_api: requirement(">=0.1.0, <0.2.0"),
            tensor_abi: tensor_abi(),
            profile: profile(),
            signal: signal(),
            timing: timing(),
            capabilities: capabilities(),
        },
    }
}

fn codec(package_id: &str, package_version: &str) -> CodecVersion {
    CodecVersion {
        identity: package_identity(package_id, package_version),
        readiness: PackageReadiness::READY,
        provides: CodecContract {
            protocol_versions: [2].into_iter().collect(),
            host_api: requirement("^0.1.0"),
            tensor_abis: [tensor_abi()].into_iter().collect(),
            profiles: [profile()].into_iter().collect(),
            signals: [signal()].into_iter().collect(),
            timings: [timing()].into_iter().collect(),
            capabilities: capabilities(),
        },
    }
}

fn resolver() -> CompatibilityResolver {
    CompatibilityResolver::new(HostRuntime {
        host_api_version: version("0.1.7"),
        protocol_versions: [2].into_iter().collect(),
        tensor_abis: [tensor_abi()].into_iter().collect(),
        signals: [signal()].into_iter().collect(),
        timings: [timing()].into_iter().collect(),
        capabilities: capabilities(),
    })
    .expect("valid host runtime")
}

fn reason(deck: &DeckVersion, codec: &CodecVersion) -> CompatibilityReason {
    resolver().resolve_pair(deck, codec).reason
}

#[test]
fn compatible_pair_preserves_both_exact_identities() {
    let deck = deck("org.latentdeck.d2", "0.1.0+deck.7");
    let codec = codec("org.latentdeck.h3", "0.1.1+codec.4");

    let decision = resolver().resolve_pair(&deck, &codec);

    assert_eq!(decision.reason, CompatibilityReason::Compatible);
    assert!(decision.is_compatible());
    assert_eq!(decision.deck, deck.identity);
    assert_eq!(decision.codec, codec.identity);
}

#[test]
fn every_reason_has_the_exact_stable_wire_string_and_precedence() {
    let expected = [
        "untrusted",
        "missing_asset",
        "package_invalid",
        "unsupported_protocol",
        "unsupported_host_api",
        "unsupported_tensor_abi",
        "unsupported_profile",
        "unsupported_signal",
        "unsupported_timing",
        "unsupported_capability",
        "compatible",
    ];

    assert_eq!(
        COMPATIBILITY_REASON_PRECEDENCE.map(CompatibilityReason::as_str),
        expected
    );
    for (reason, expected) in COMPATIBILITY_REASON_PRECEDENCE.into_iter().zip(expected) {
        assert_eq!(reason.to_string(), expected);
        assert_eq!(
            serde_json::to_string(&reason).unwrap(),
            format!("\"{expected}\"")
        );
    }
}

#[test]
fn readiness_and_package_validity_follow_stable_precedence() {
    let mut deck = deck("deck", "1.0.0");
    let mut codec = codec("codec", "1.0.0");
    deck.readiness = PackageReadiness {
        trust: TrustState::Untrusted,
        assets: AssetState::Missing,
        package: PackageState::Invalid,
    };
    codec.readiness = deck.readiness;
    assert_eq!(reason(&deck, &codec), CompatibilityReason::Untrusted);

    deck.readiness.trust = TrustState::Trusted;
    codec.readiness.trust = TrustState::Trusted;
    assert_eq!(reason(&deck, &codec), CompatibilityReason::MissingAsset);

    deck.readiness.assets = AssetState::Present;
    codec.readiness.assets = AssetState::Present;
    assert_eq!(reason(&deck, &codec), CompatibilityReason::PackageInvalid);
}

#[test]
fn unsupported_protocol_is_reported_before_later_constraint_failures() {
    let mut deck = deck("deck", "1.0.0");
    let mut codec = codec("codec", "1.0.0");
    deck.requires.protocol_version = 3;
    codec.provides.tensor_abis = [different_tensor_abi()].into_iter().collect();
    codec.provides.profiles = [different_profile()].into_iter().collect();

    assert_eq!(
        reason(&deck, &codec),
        CompatibilityReason::UnsupportedProtocol
    );
}

#[test]
fn unsupported_host_api_is_deterministic_for_deck_or_codec_requirement() {
    let mut deck_value = deck("deck", "1.0.0");
    let codec_value = codec("codec", "1.0.0");
    deck_value.requires.host_api = requirement(">=9.0.0, <10.0.0");
    assert_eq!(
        reason(&deck_value, &codec_value),
        CompatibilityReason::UnsupportedHostApi
    );

    let deck_value = deck("deck", "1.0.0");
    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.host_api = requirement("=9.0.0");
    assert_eq!(
        reason(&deck_value, &codec_value),
        CompatibilityReason::UnsupportedHostApi
    );
}

fn different_tensor_abi() -> TensorAbiContract {
    TensorAbiContract {
        torch_version: version("2.9.0"),
        ..tensor_abi()
    }
}

fn different_profile() -> ProfileContract {
    ProfileContract {
        profile: id("other-profile"),
        ..profile()
    }
}

fn different_signal() -> SignalContract {
    SignalContract {
        decoded_width: 1280,
        ..signal()
    }
}

fn different_timing() -> TimingContract {
    TimingContract {
        frame_rate_numerator: 60,
        ..timing()
    }
}

#[test]
fn each_pair_constraint_maps_to_its_specific_reason() {
    let deck = deck("deck", "1.0.0");

    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.tensor_abis = [different_tensor_abi()].into_iter().collect();
    assert_eq!(
        reason(&deck, &codec_value),
        CompatibilityReason::UnsupportedTensorAbi
    );

    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.profiles = [different_profile()].into_iter().collect();
    assert_eq!(
        reason(&deck, &codec_value),
        CompatibilityReason::UnsupportedProfile
    );

    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.signals = [different_signal()].into_iter().collect();
    assert_eq!(
        reason(&deck, &codec_value),
        CompatibilityReason::UnsupportedSignal
    );

    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.timings = [different_timing()].into_iter().collect();
    assert_eq!(
        reason(&deck, &codec_value),
        CompatibilityReason::UnsupportedTiming
    );

    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.capabilities.remove(&id("realtime"));
    assert_eq!(
        reason(&deck, &codec_value),
        CompatibilityReason::UnsupportedCapability
    );
}

#[test]
fn host_constraints_participate_in_tensor_signal_timing_and_capability_checks() {
    let deck = deck("deck", "1.0.0");
    let codec = codec("codec", "1.0.0");

    let mut host = resolver().host().clone();
    host.tensor_abis = [different_tensor_abi()].into_iter().collect();
    assert_eq!(
        CompatibilityResolver::new(host)
            .unwrap()
            .resolve_pair(&deck, &codec)
            .reason,
        CompatibilityReason::UnsupportedTensorAbi
    );

    let mut host = resolver().host().clone();
    host.signals = [different_signal()].into_iter().collect();
    assert_eq!(
        CompatibilityResolver::new(host)
            .unwrap()
            .resolve_pair(&deck, &codec)
            .reason,
        CompatibilityReason::UnsupportedSignal
    );

    let mut host = resolver().host().clone();
    host.timings = [different_timing()].into_iter().collect();
    assert_eq!(
        CompatibilityResolver::new(host)
            .unwrap()
            .resolve_pair(&deck, &codec)
            .reason,
        CompatibilityReason::UnsupportedTiming
    );

    let mut host = resolver().host().clone();
    host.capabilities.remove(&id("resample"));
    assert_eq!(
        CompatibilityResolver::new(host)
            .unwrap()
            .resolve_pair(&deck, &codec)
            .reason,
        CompatibilityReason::UnsupportedCapability
    );
}

#[test]
fn structurally_invalid_package_contract_is_not_repaired() {
    let mut deck_value = deck("deck", "1.0.0");
    let codec_value = codec("codec", "1.0.0");
    deck_value.requires.signal.channels = 0;
    assert_eq!(
        reason(&deck_value, &codec_value),
        CompatibilityReason::PackageInvalid
    );

    let deck_value = deck("deck", "1.0.0");
    let mut codec_value = codec("codec", "1.0.0");
    codec_value.provides.protocol_versions.clear();
    assert_eq!(
        reason(&deck_value, &codec_value),
        CompatibilityReason::PackageInvalid
    );
}

#[test]
fn no_any_constraint_or_identifier_normalization_is_accepted() {
    for forbidden in [
        "",
        "*",
        "any",
        "ANY",
        " padded",
        "padded ",
        "internal space",
        "bad/value",
        "h3-é",
        "bad\nvalue",
    ] {
        assert!(
            ContractId::new(forbidden).is_err(),
            "accepted {forbidden:?}"
        );
    }
    for forbidden in ["*", "1.*", "ANY", " >=1.0.0", ""] {
        assert!(
            HostApiRequirement::parse(forbidden).is_err(),
            "accepted {forbidden:?}"
        );
    }

    let mut codec = codec("codec", "1.0.0");
    let mut case_changed = profile();
    case_changed.codec_family = id("H3");
    codec.provides.profiles = [case_changed].into_iter().collect();
    assert_eq!(
        reason(&deck("deck", "1.0.0"), &codec),
        CompatibilityReason::UnsupportedProfile
    );
}

#[test]
fn equivalent_frame_rate_fractions_are_not_normalized() {
    let deck = deck("deck", "1.0.0");
    let mut codec = codec("codec", "1.0.0");
    codec.provides.timings = [TimingContract {
        frame_rate_numerator: 60,
        frame_rate_denominator: 2,
        ..timing()
    }]
    .into_iter()
    .collect();

    assert_eq!(
        reason(&deck, &codec),
        CompatibilityReason::UnsupportedTiming
    );
}

#[test]
fn host_api_requirement_preserves_its_source_spelling() {
    let requirement = requirement(">=0.1.0, <0.2.0");
    assert_eq!(requirement.as_str(), ">=0.1.0, <0.2.0");
    assert_eq!(
        serde_json::to_string(&requirement).unwrap(),
        "\">=0.1.0, <0.2.0\""
    );
}

#[test]
fn matrix_is_complete_and_stably_sorted_independent_of_input_order() {
    let decks = [
        deck("z.deck", "1.0.0"),
        deck("a.deck", "2.0.0"),
        deck("a.deck", "1.0.0"),
    ];
    let codecs = [
        codec("z.codec", "1.0.0"),
        codec("a.codec", "2.0.0"),
        codec("a.codec", "1.0.0"),
    ];

    let matrix = resolver().resolve_matrix(&decks, &codecs).unwrap();
    let keys = matrix
        .iter()
        .map(|entry| {
            (
                entry.deck.package_id.as_str(),
                entry.deck.version.to_string(),
                entry.codec.package_id.as_str(),
                entry.codec.version.to_string(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(matrix.len(), decks.len() * codecs.len());
    assert_eq!(
        keys,
        vec![
            ("a.deck", "1.0.0".into(), "a.codec", "1.0.0".into()),
            ("a.deck", "1.0.0".into(), "a.codec", "2.0.0".into()),
            ("a.deck", "1.0.0".into(), "z.codec", "1.0.0".into()),
            ("a.deck", "2.0.0".into(), "a.codec", "1.0.0".into()),
            ("a.deck", "2.0.0".into(), "a.codec", "2.0.0".into()),
            ("a.deck", "2.0.0".into(), "z.codec", "1.0.0".into()),
            ("z.deck", "1.0.0".into(), "a.codec", "1.0.0".into()),
            ("z.deck", "1.0.0".into(), "a.codec", "2.0.0".into()),
            ("z.deck", "1.0.0".into(), "z.codec", "1.0.0".into()),
        ]
    );
    assert!(
        matrix
            .iter()
            .all(|entry| entry.reason == CompatibilityReason::Compatible)
    );
}

#[test]
fn matrix_uses_build_metadata_as_a_stable_total_order_tiebreaker() {
    let decks = [deck("deck", "1.0.0+z"), deck("deck", "1.0.0+a")];
    let codecs = [codec("codec", "1.0.0")];

    let matrix = resolver().resolve_matrix(&decks, &codecs).unwrap();

    assert_eq!(matrix[0].deck.version.to_string(), "1.0.0+a");
    assert_eq!(matrix[1].deck.version.to_string(), "1.0.0+z");
}

#[test]
fn duplicate_exact_identity_is_rejected_instead_of_deduplicated() {
    let duplicated_decks = [deck("deck", "1.0.0"), deck("deck", "1.0.0")];
    let codec_values = [codec("codec", "1.0.0")];
    assert!(matches!(
        resolver().resolve_matrix(&duplicated_decks, &codec_values),
        Err(MatrixError::DuplicateDeck(_))
    ));

    let deck_values = [deck("deck", "1.0.0")];
    let duplicated_codecs = [codec("codec", "1.0.0"), codec("codec", "1.0.0")];
    assert!(matches!(
        resolver().resolve_matrix(&deck_values, &duplicated_codecs),
        Err(MatrixError::DuplicateCodec(_))
    ));
}

#[test]
fn invalid_or_empty_host_runtime_is_rejected_before_resolution() {
    let mut host = resolver().host().clone();
    host.protocol_versions.clear();
    assert_eq!(
        CompatibilityResolver::new(host).unwrap_err(),
        ContractValidationError::InvalidHostRuntime
    );

    let empty_matrix = resolver().resolve_matrix(&[], &[]).unwrap();
    assert!(empty_matrix.is_empty());
}
