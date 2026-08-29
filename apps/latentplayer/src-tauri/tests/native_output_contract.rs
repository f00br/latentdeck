// Compile and exercise the Player identity adapter independently from the
// Tauri command entrypoint. The DX12 implementation itself lives in the shared
// `latentdeck-native-output` workspace crate.
#[allow(dead_code)]
#[path = "../src/native_output.rs"]
mod native_output;

#[test]
fn player_adapter_reexports_the_shared_runtime_contract() {
    fn assert_send<T: Send>() {}

    assert_send::<native_output::NativeOutput>();
    assert_eq!(
        native_output::NativeOutputError::FrameRejected.code(),
        "output.frame_rejected"
    );
    assert_ne!(
        native_output::PresentOutcome::SkippedTimeout,
        native_output::PresentOutcome::Presented
    );
    assert_ne!(
        native_output::ResizeOutcome::Suspended,
        native_output::ResizeOutcome::Configured
    );
}
