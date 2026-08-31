use serde_json::Value;

#[test]
fn release_binary_embeds_the_frontend_with_the_tauri_custom_protocol() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("default = [\"custom-protocol\"]"));
    assert!(manifest.contains("custom-protocol = [\"tauri/custom-protocol\"]"));
}

#[test]
fn main_window_cannot_receive_a_native_diagnostic_destination_path() {
    let capability: Value = serde_json::from_str(include_str!("../capabilities/main.json"))
        .expect("LatentPlayer capability JSON");

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
fn fullscreen_commands_resolve_the_authoritative_main_window() {
    let composition_root = include_str!("../src/main.rs");
    for command in ["player_fullscreen_status", "player_set_fullscreen"] {
        let start = composition_root
            .find(&format!("async fn {command}("))
            .expect("fullscreen command exists");
        let tail = &composition_root[start..];
        let end = tail
            .find("\n}\n")
            .map(|index| index + 3)
            .expect("fullscreen command closes");
        let body = &tail[..end];
        assert!(body.contains("app: AppHandle"));
        assert!(body.contains("let window = main_window(&app)?;"));
        assert!(!body.contains("window: WebviewWindow"));
    }
}
