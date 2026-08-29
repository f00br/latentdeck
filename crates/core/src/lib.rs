//! UI-independent runtime coordination for `LatentDeck`.

/// Returns the version shared by all first-party workspace packages.
#[must_use]
pub const fn product_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
