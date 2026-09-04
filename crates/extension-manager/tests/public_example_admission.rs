use latentdeck_deck_runtime_contracts::CompatibilityReason;
use latentdeck_extension_manager::{
    CodecPackManifest, DeckPackManifest, ProfileKey, SelectedSourceCompatibility,
    SelectedSourceScope, SignalGeometry, TensorDevice, TensorDtype, resolve_selected_compatibility,
};

#[test]
fn cpu_starter_deck_and_synthetic_codec_pass_exact_selected_source_admission() {
    let deck = serde_json::from_str::<DeckPackManifest>(include_str!(
        "../../../examples/extensions/starter-deck/deck-pack.json"
    ))
    .expect("public starter Deck manifest");
    let codec = serde_json::from_str::<CodecPackManifest>(include_str!(
        "../../../examples/extensions/synthetic-codec/codec-pack.json"
    ))
    .expect("public synthetic Codec manifest");
    let profile = ProfileKey {
        codec_family: "synthetic".to_owned(),
        profile: "example_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    };
    let source = SelectedSourceCompatibility {
        lc_spec_version: "0.1.0".to_owned(),
        profile: profile.clone(),
        geometry: SignalGeometry {
            dtype: TensorDtype::Fp32,
            device: TensorDevice::Cpu,
            batch: 1,
            channels: 4,
            temporal: 1,
            height: 2,
            width: 3,
        },
        decoded_height: 4,
        decoded_width: 6,
        frame_rate_numerator: 24,
        frame_rate_denominator: 1,
        timing_contract: "synthetic_ticks".to_owned(),
        timing_contract_version: "0.1.0".to_owned(),
    };

    let decision = resolve_selected_compatibility(
        &deck,
        &codec,
        "0.1.0",
        true,
        Some(&profile),
        TensorDevice::Cpu,
        &[source],
        SelectedSourceScope::CompleteSet,
    );

    assert_eq!(decision.reason, CompatibilityReason::Compatible);
    assert_eq!(decision.compatible_profiles, vec![profile]);
}
