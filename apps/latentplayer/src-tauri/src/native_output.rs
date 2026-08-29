//! Player-specific identity adapter over the reusable native-output crate.

use latentdeck_native_output::NativeOutputConfig;
pub use latentdeck_native_output::{
    NativeOutput, NativeOutputError, PresentOutcome, ResizeOutcome,
};

/// Stable Tauri label for the Player's decoded-frame output window.
pub const NATIVE_OUTPUT_WINDOW_LABEL: &str = "latentplayer-native-output";

const NATIVE_OUTPUT_WINDOW_TITLE: &str = "LatentPlayer Output";

pub fn native_output_config(frame_width: u32, frame_height: u32) -> NativeOutputConfig {
    NativeOutputConfig::new(
        frame_width,
        frame_height,
        NATIVE_OUTPUT_WINDOW_LABEL,
        NATIVE_OUTPUT_WINDOW_TITLE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_window_identity_is_preserved() {
        let config = native_output_config(800, 448);
        assert_eq!(config.frame_width, 800);
        assert_eq!(config.frame_height, 448);
        assert_eq!(config.window_label(), "latentplayer-native-output");
        assert_eq!(config.window_title(), "LatentPlayer Output");
    }
}
