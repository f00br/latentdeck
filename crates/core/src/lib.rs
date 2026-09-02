//! UI-independent runtime coordination for `LatentDeck`.

pub mod codec_pack;
pub mod deck_runtime_v2;
pub mod deck_selection_v2;
pub mod deck_session_v2;
pub mod diagnostics;
pub mod external_asset_v2;
pub mod playback_schedule;
pub mod player;
pub mod player_session_v2;
pub mod raw_import;
pub mod realtime_diagnostics;
pub mod signal_geometry;
pub mod worker_client;
pub mod worker_client_v2;
pub mod worker_source_v2;
pub mod worker_supervisor;

/// Returns the version shared by all first-party workspace packages.
#[must_use]
pub const fn product_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
