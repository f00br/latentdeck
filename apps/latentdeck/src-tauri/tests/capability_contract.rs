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
    let source = include_str!("../src/generic_deck_state.rs");
    for suffix in ["fullscreen_status_get", "fullscreen_set"] {
        let command = format!("deck_generic_{suffix}");
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
        assert!(!body.contains("GenericDeckRuntime"));
        assert!(!body.contains("runtime_inactive"));
    }
}

#[test]
fn production_composition_root_registers_only_generic_deck_commands() {
    let main = include_str!("../src/main.rs");
    let handler = main
        .split(".invoke_handler(tauri::generate_handler![")
        .nth(1)
        .expect("production invoke handler")
        .split("])")
        .next()
        .expect("production invoke handler closes");
    assert!(handler.contains("deck_generic_open"));
    assert!(handler.contains("deck_generic_sources_replace"));
    assert!(main.contains("deck_generic_capture_start"));
    assert!(main.contains("deck_generic_recording_start"));
    assert!(!main.contains("mod d2_"));
    assert!(!main.contains("mod q4_"));
    assert!(!main.contains("deck_d2_"));
    assert!(!main.contains("deck_q4_"));
    assert!(!main.contains("D2AppState"));
    assert!(!main.contains("Q4AppState"));
}

#[test]
fn source_replacement_preflights_before_the_old_worker_shutdown_boundary() {
    let source = include_str!("../src/generic_deck_state.rs");
    let command = source
        .split("async fn deck_generic_sources_replace(")
        .nth(1)
        .expect("source replacement command")
        .split("async fn configure_replacement_runtime(")
        .next()
        .expect("source replacement command closes");
    let shutdown = command
        .find("shutdown_for_replacement()")
        .expect("destructive worker boundary");
    for preflight in [
        "resolve_deck_sources(identities)",
        "prepare_open_selection(",
        "prevalidate_load(&prepared, &load)",
        "validate_replacement_identity(",
    ] {
        assert!(
            command
                .find(preflight)
                .is_some_and(|index| index < shutdown),
            "{preflight} must finish before the old worker stops"
        );
    }
    assert!(command.contains("abort_lifecycle_transition(transition.generation)"));
    assert!(
        command
            .find("complete_source_replacement(")
            .is_some_and(|commit| command
                .rfind("state.viewport.current()")
                .is_some_and(|latest_viewport| commit < latest_viewport)),
        "the published replacement must receive an authoritative post-commit viewport re-read"
    );
}

#[test]
fn mp4_pin_monitor_follows_the_shared_recorder_not_a_retired_runtime() {
    let source = include_str!("../src/generic_deck_state.rs");
    let monitor = source
        .split("fn spawn_recording_pin_monitor(")
        .nth(1)
        .expect("recording pin monitor")
        .split("async fn release_monitored_pin(")
        .next()
        .expect("recording pin monitor closes");

    assert!(monitor.contains("recording: DecodedRecordingController"));
    assert!(monitor.contains("recording.status().state"));
    assert!(!monitor.contains("GenericDeckRuntime"));
    assert!(!monitor.contains("runtime.is_closed()"));
}

#[test]
fn source_replacement_preserves_private_spout_intent_without_requiring_spout() {
    let actor = include_str!("../src/generic_deck_runtime.rs");
    let restore = actor
        .rsplit("fn restore_replacement_output(")
        .next()
        .expect("replacement output restore")
        .split("fn publish_status(")
        .next()
        .expect("replacement output restore closes");

    assert!(restore.contains("self.spout_requested_enabled = spout_requested_enabled"));
    assert!(restore.contains("if self.output.spout_status().ready"));
    assert!(restore.contains("let _ = self.output.set_spout_name(name)"));
    assert!(restore.contains("effective_spout_enabled("));
}

#[test]
fn extensions_matrix_and_generic_runtime_share_one_active_package_cache() {
    let extensions = include_str!("../src/extension_commands.rs");
    let runtime = include_str!("../src/generic_deck_state.rs");

    assert!(extensions.contains("active_packages: ActivePackageCache"));
    assert_eq!(
        extensions.matches("ActivePackageCache::new()").count(),
        1,
        "Deck must construct one process cache"
    );
    let snapshot = extensions
        .split("fn extension_snapshot_for(")
        .nth(1)
        .expect("extension snapshot helper exists")
        .split("\n}")
        .next()
        .expect("extension snapshot helper closes");
    assert!(snapshot.contains("active_packages"));
    assert!(snapshot.contains(".runtime_inventory(roots)"));
    assert!(extensions.contains(".runtime_list_kind(roots, PackageKind::DeckPack)"));
    assert!(extensions.contains(".enable_and_prime(&roots, &package)"));
    assert!(extensions.contains(".disable(&roots, &package)"));
    assert!(runtime.contains("extensions.active_packages()"));
    assert!(runtime.contains("prepare_exact_deck_selection_with_cache"));
}

#[test]
fn production_generic_actor_is_exact_protocol2_without_h3_or_p1_fallback() {
    let actor = include_str!("../src/generic_deck_runtime.rs");
    let state = include_str!("../src/generic_deck_state.rs");
    for required in [
        "start_deck_session_v2",
        "Command::DeckProcess",
        "Command::DeckReset",
        "preserve_playheads: true",
        "adopt_ring_generation",
        "finalize_capture_with_carrier",
        "PreparedProtocol2DeckDiagnosticIdentity",
        "realtime_metrics_v2",
    ] {
        assert!(actor.contains(required), "missing generic seam: {required}");
    }
    for required in [
        "prepare_exact_deck_selection",
        "request.deck_id",
        "request.deck_version",
        "request.codec_id",
        "request.codec_version",
        "request.profile_key",
    ] {
        assert!(
            state.contains(required),
            "missing exact selection seam: {required}"
        );
    }
    for forbidden in [
        "Command::D2Load",
        "Command::Q4Load",
        "d2_worker",
        "q4_worker",
        "WORKER_PROTOCOL_VERSION",
        "minimax_h3",
        "h3_av_latent",
    ] {
        assert!(
            !actor.contains(forbidden) && !state.contains(forbidden),
            "legacy/H3-specific production dependency remains: {forbidden}"
        );
    }
}

#[test]
fn generic_deck_prefetches_on_an_absolute_clock_without_command_reanchoring() {
    let actor = include_str!("../src/generic_deck_runtime.rs");

    assert!(actor.contains("frame_clock: FrameClock"));
    assert!(actor.contains("self.processing_active() && self.pending_frames.is_empty()"));
    assert!(actor.contains("self.process_once().await"));
    assert!(actor.contains("self.frame_clock.advance_past(Instant::now())"));
    assert!(actor.contains("self.restart_clock_on_resume(was_processing)"));
    assert!(!actor.contains("next_frame = Instant::now()"));
    assert!(!actor.contains("Duration::from_secs_f64"));
}

#[test]
fn production_core_does_not_export_hardcoded_deck_worker_clients() {
    let core = include_str!("../../../../crates/core/src/lib.rs");
    assert!(!core.contains("d2_worker_client"));
    assert!(!core.contains("q4_worker_client"));
    assert!(core.contains("pub mod deck_session_v2"));
    assert!(core.contains("pub mod worker_client_v2"));
}

#[test]
fn production_protocol1_surface_is_player_only() {
    let control_root = include_str!("../../../../crates/control/src/lib.rs");
    let control_protocol = include_str!("../../../../crates/control/src/protocol.rs");
    let python_root =
        include_str!("../../../../codec-host/python/src/latentdeck_codec_host/__init__.py");
    let python_protocol =
        include_str!("../../../../codec-host/python/src/latentdeck_codec_host/protocol.py");
    let diagnostics = include_str!("../../../../crates/core/src/realtime_diagnostics.rs");
    let codec_pack = include_str!("../../../../crates/core/src/codec_pack.rs");
    let supervisor = include_str!("../../../../crates/core/src/worker_supervisor.rs");

    for forbidden in ["mod d2;", "mod d2_capture;", "mod q4;"] {
        assert!(
            !control_root.contains(forbidden),
            "legacy Protocol 1 module remains compiled: {forbidden}"
        );
    }
    for forbidden in ["deck.d2.", "deck.q4."] {
        assert!(!control_protocol.contains(forbidden));
        assert!(!python_protocol.contains(forbidden));
    }
    for forbidden in ["operator_api", "BuiltinOperatorRegistry"] {
        assert!(!python_root.contains(forbidden));
    }
    for forbidden in [
        "DeckD2",
        "DeckQ4",
        "D2DiagnosticSession",
        "Q4DiagnosticSession",
    ] {
        assert!(!diagnostics.contains(forbidden));
    }
    for forbidden in ["from_codec_pack_d2", "from_codec_pack_q4"] {
        assert!(!supervisor.contains(forbidden));
    }
    for forbidden in ["d2_arguments", "q4_arguments"] {
        assert!(!codec_pack.contains(forbidden));
    }
}
