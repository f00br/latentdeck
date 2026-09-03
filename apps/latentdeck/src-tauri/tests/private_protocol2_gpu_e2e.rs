//! Executable, opt-in Protocol 2 H3 GPU acceptance runner.
//!
//! Ordinary workspace tests compile only the closed launcher contract. The
//! ignored test starts the feature-gated real Tauri/DX12 runner in a child
//! process so its native event loop remains on that process' main thread.

#[test]
fn private_protocol2_gpu_runner_is_an_executable_evidence_producer() {
    let source = include_str!("../src/private_protocol2_gpu_e2e_main.rs");
    for required in [
        "start_player_session_v2",
        "start_deck_session_v2",
        "prepare_exact_deck_selection",
        "finalize_capture_with_carrier",
        "DecodedRecordingController",
        "set_spout_enabled",
        "set_spout_name",
        "STABILITY_SECONDS",
        "tokio::task::yield_now",
        "app.run_return",
        "without_windows_verbatim_prefix",
        "write_validated_receipt",
    ] {
        assert!(
            source.contains(required),
            "missing executable seam: {required}"
        );
    }
    for forbidden in [
        "latentdeck_control::protocol",
        "worker_client::WorkerClient",
        "d2_worker_client",
        "q4_worker_client",
        "Command::D2",
        "Command::Q4",
        "app.run(|_, _| {})",
    ] {
        assert!(
            !source.contains(forbidden),
            "legacy Protocol 1 seam remains in private runner: {forbidden}"
        );
    }
    assert!(
        !source.contains("!matches!(initial.state, DeckState::Ready | DeckState::Paused)"),
        "the GPU gate cannot reject Playing after it enables source transport"
    );
    assert!(
        source.contains("DeckState::Ready | DeckState::Paused | DeckState::Playing"),
        "the GPU gate must accept the exact Playing status produced after startup transport"
    );
    assert!(
        !source.contains("let process_count = if mode == CaptureMode::Snapshot { 1 } else { 2 };"),
        "snapshot completion cannot assume that every codec has a one-slot valid boundary"
    );
    assert!(
        source.contains("SNAPSHOT_BOUNDARY_ATTEMPTS")
            && source.contains(
                "mode == CaptureMode::Snapshot && last_process_state == CaptureState::Completed"
            ),
        "the GPU gate must process until the codec reports its first completed snapshot boundary"
    );
    assert!(
        source.contains(r#"const EXTERNAL_DECK_ID: &str = "dev.latentdeck.private.h3-probe";"#),
        "the generated external Deck must use a valid lowercase reverse-DNS package ID"
    );
    for required in [
        r#"const BUNDLED_DECK_VERSION: &str = "0.2.1";"#,
        r#"const EXTERNAL_DECK_VERSION: &str = "0.2.0";"#,
        "bundled_deck_reference(deck_id)",
        "external_deck_reference()",
    ] {
        assert!(
            source.contains(required),
            "the private runner is missing a split Deck version seam: {required}"
        );
    }

    let orchestrator = include_str!("../../../../tools/Run-PrivateProtocol2GpuE2E.ps1");
    for required in [
        "private-protocol2-gpu-e2e",
        "h3_full_matrix_executes_and_emits_receipt",
        "Test-PrivateProtocol2GpuGate.ps1",
        "LATENTDECK_PRIVATE_PROTOCOL2_BASE_ROOT",
        "LATENTDECK_PRIVATE_PROTOCOL2_TAEH3",
        "LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_4",
    ] {
        assert!(
            orchestrator.contains(required),
            "missing private orchestrator seam: {required}"
        );
    }
}

#[test]
fn bundled_gpu_gate_loads_real_descriptor_defaults_in_declaration_order() {
    let source = include_str!("../src/private_protocol2_gpu_e2e_main.rs");
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    for required in [
        "let d2_load = d2_load(&d2_prepared)?; let mut d2 = start_deck(d2_prepared, d2_load).await?;",
        "let q4_load = q4_load(&q4_prepared)?; let mut q4 = start_deck(q4_prepared, q4_load).await?;",
        "let external_load = external_load(&external_prepared)?; let mut external = start_deck(external_prepared, external_load).await?;",
        "controls: operator_default_controls_in_declaration_order(prepared)?",
        ".operator_descriptor() .controls .iter()",
        "\"controls\": []",
    ] {
        assert!(
            normalized.contains(required),
            "bundled Deck load no longer uses descriptor defaults in declaration order: {required}"
        );
    }
    assert!(
        !source.contains("controls: Vec::new()"),
        "the private GPU gate must not bypass real Deck controls with an empty load block"
    );

    for (deck, descriptor) in [
        (
            "D2",
            include_str!("../../../../operators/builtin/d2/package/operator.json"),
        ),
        (
            "Q4",
            include_str!("../../../../operators/builtin/q4/package/operator.json"),
        ),
    ] {
        let document: serde_json::Value =
            serde_json::from_str(descriptor).expect("bundled operator descriptor JSON");
        let controls = document["controls"]
            .as_array()
            .expect("bundled operator controls array");
        let control_ids = controls
            .iter()
            .map(|control| {
                control["control_id"]
                    .as_str()
                    .expect("bundled operator control ID")
            })
            .collect::<Vec<_>>();
        assert!(!control_ids.is_empty(), "{deck} defaults must be non-empty");
        let mut alphabetical = control_ids.clone();
        alphabetical.sort_unstable();
        assert_ne!(
            control_ids, alphabetical,
            "{deck} descriptor must exercise non-alphabetical control ordering"
        );
    }
}

#[test]
#[ignore = "requires exact private H3 Codec Pack v2, LC corpus, TAEH3, CUDA, Media Foundation, and pinned Spout2 SDK"]
fn h3_full_matrix_executes_and_emits_receipt() {
    #[cfg(not(feature = "private-protocol2-gpu-e2e"))]
    panic!("rerun with --features private-protocol2-gpu-e2e");

    #[cfg(feature = "private-protocol2-gpu-e2e")]
    {
        use std::process::Command;

        let executable = env!("CARGO_BIN_EXE_latentdeck-private-protocol2-gpu-e2e");
        let status = Command::new(executable)
            .status()
            .expect("start the dedicated private Protocol 2 GPU runner");
        assert!(status.success(), "private Protocol 2 GPU runner failed");
    }
}
