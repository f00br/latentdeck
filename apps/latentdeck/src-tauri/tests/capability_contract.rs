use serde_json::Value;

#[test]
fn release_binary_embeds_the_frontend_with_the_tauri_custom_protocol() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("default = [\"custom-protocol\"]"));
    assert!(manifest.contains("custom-protocol = [\"tauri/custom-protocol\"]"));
}

#[test]
fn main_window_can_import_cartridges_and_roundtrip_presets_without_broad_file_access() {
    let capability: Value = serde_json::from_str(include_str!("../capabilities/main.json"))
        .expect("LatentDeck capability JSON");

    assert_eq!(capability["identifier"], "main-window");
    assert_eq!(capability["windows"], serde_json::json!(["main"]));
    assert_eq!(
        capability["permissions"],
        serde_json::json!(["core:default", "dialog:allow-open"])
    );
}

#[test]
fn release_binary_uses_the_windows_gui_subsystem() {
    let composition_root = include_str!("../src/main.rs");
    assert!(
        composition_root
            .starts_with("#![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]"),
        "the release application must not create a second console window"
    );
}

#[test]
fn fullscreen_commands_are_host_level_not_runtime_gated() {
    for (source, prefix) in [
        (include_str!("../src/d2_state.rs"), "deck_d2"),
        (include_str!("../src/q4_state.rs"), "deck_q4"),
    ] {
        for suffix in ["fullscreen_status_get", "fullscreen_set"] {
            let command = format!("{prefix}_{suffix}");
            let start = source
                .find(&format!("async fn {command}("))
                .expect("fullscreen command exists");
            let tail = &source[start..];
            let end = tail
                .find("\n}\n")
                .map(|index| index + 3)
                .expect("fullscreen command closes");
            let body = &tail[..end];
            assert!(body.contains("State<'_, HostFullscreenController>"));
            assert!(body.contains("main_window(&app)?"));
            assert!(!body.contains("State<'_, D2AppState>"));
            assert!(!body.contains("State<'_, Q4AppState>"));
            assert!(!body.contains("clone_slot"));
            assert!(!body.contains("runtime_inactive"));
        }
    }
}
