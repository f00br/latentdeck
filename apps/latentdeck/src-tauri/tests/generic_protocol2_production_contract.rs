#[test]
fn generic_deck_production_path_has_no_hardcoded_runtime_fallback() {
    let main = include_str!("../src/main.rs");
    let actor = include_str!("../src/generic_deck_runtime.rs");
    let controller = include_str!("../src/generic_deck_state.rs");

    assert!(main.contains("mod generic_deck_runtime;"));
    assert!(main.contains("mod generic_deck_state;"));
    assert!(!main.contains("deck_d2_"));
    assert!(!main.contains("deck_q4_"));
    assert!(actor.contains("start_deck_session_v2_with_retained_assets("));
    assert!(actor.contains("prepared.retained_external_assets"));
    assert!(controller.contains("prepare_exact_deck_selection_with_cache("));
    assert!(controller.contains("spawn_blocking(move ||"));
    assert!(controller.contains("bind_integrity_validated_external_asset(asset)"));
    assert!(!controller.contains("hash_path(&selected)"));
    assert!(controller.contains("profile.matches_wire(&prepared.host.profile_key)"));
    assert!(!actor.contains("WORKER_PROTOCOL_VERSION"));
    assert!(!controller.contains("newest"));
}
