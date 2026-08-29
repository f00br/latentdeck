//! UI-independent runtime coordination for `LatentDeck`.

pub mod codec_pack;
pub mod d2_worker_client;
pub mod playback_schedule;
pub mod player;
pub mod worker_client;
pub mod worker_supervisor;

/// Returns the version shared by all first-party workspace packages.
#[must_use]
pub const fn product_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
