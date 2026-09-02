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

#[test]
fn production_startup_is_protocol2_only_without_hidden_protocol1_fallback() {
    let composition_root = include_str!("../src/main.rs");
    let start = composition_root
        .find("async fn start_runtime(")
        .expect("production startup exists");
    let tail = &composition_root[start..];
    let end = tail
        .find("\n}\n")
        .map(|index| index + 3)
        .expect("production startup closes");
    let body = &tail[..end];

    assert!(body.contains("PlaybackRuntime::start_protocol2"));
    assert!(!body.contains("start_protocol1_h3"));
    assert!(!body.contains("fallback"));
}

#[test]
fn player_boot_does_not_auto_select_a_newest_codec_pack() {
    let composition_root = include_str!("../src/main.rs");
    let start = composition_root
        .find("fn discover() -> Self")
        .expect("application discovery exists");
    let tail = &composition_root[start..];
    let end = tail
        .find("\n    }\n}")
        .map(|index| index + 7)
        .expect("application discovery closes");
    let body = &tail[..end];

    assert!(body.contains("PlayerCoordinator::without_codec()"));
    assert!(!body.contains("discover_visible"));
    assert!(!body.contains("newest"));
}

#[test]
fn player_execution_boundaries_share_one_process_active_package_cache() {
    let composition_root = include_str!("../src/main.rs");

    assert!(composition_root.contains("active_packages: ActivePackageCache"));
    assert_eq!(
        composition_root
            .matches("ActivePackageCache::new()")
            .count(),
        1,
        "the application must construct one shared process cache"
    );
    for command in [
        "player_raw_import_options",
        "player_conversion_plan",
        "player_conversion_start",
        "player_select_decoder",
        "player_select_codec_exact",
        "player_play",
        "player_restart",
    ] {
        let start = composition_root
            .find(&format!("fn {command}("))
            .expect("Player command exists");
        let tail = &composition_root[start..];
        let end = tail.find("\n#[tauri::command]").unwrap_or(tail.len());
        assert!(
            tail[..end].contains("state.active_packages"),
            "{command} must use the shared process cache"
        );
    }

    let snapshot = composition_root
        .split("fn extension_snapshot_for(")
        .nth(1)
        .expect("extension snapshot helper exists")
        .split("\n}")
        .next()
        .expect("extension snapshot helper closes");
    assert!(snapshot.contains("active_packages"));
    assert!(snapshot.contains(".runtime_inventory(roots)"));

    let command = composition_root
        .split("async fn extensions_snapshot(")
        .nth(1)
        .expect("Extensions snapshot command exists")
        .split("\n#[tauri::command]")
        .next()
        .expect("Extensions snapshot command closes");
    assert!(command.contains("state.active_packages.clone()"));
    assert!(command.contains("extension_snapshot_for(&roots, &cache)"));
    assert!(composition_root.contains(".enable_and_prime(&roots, &package)"));
    assert!(composition_root.contains(".disable(&roots, &package)"));
}

#[test]
fn player_loop_is_a_host_controlled_generation_reset() {
    let runtime = include_str!("../src/playback_runtime_v2.rs");

    assert!(runtime.contains("if self.loop_enabled"));
    assert!(runtime.contains("self.reset().await?"));
    assert!(runtime.contains("adopt_ring_generation(new_generation)"));
}
