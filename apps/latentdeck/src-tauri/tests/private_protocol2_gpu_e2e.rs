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
