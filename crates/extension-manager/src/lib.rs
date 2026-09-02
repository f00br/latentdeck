//! Hash-bound lifecycle management for `.ld` Deck packages and `.ldcodec`
//! Codec packages.

mod archive;
mod error;
mod lifecycle;
mod model;
mod schema;

pub use archive::{PackRequest, inspect, pack};
pub use error::{ErrorCode, ExtensionError, Result};
pub use lifecycle::{
    ExtensionRoots, InstallRequest, RemoveOptions, compatibility_matrix, disable, enable,
    enable_if_only_installed_version, install, install_from_bundled_index, list, remove, repair,
    repair_from_bundled_index, resolve_active, resolve_installed, verify,
};
pub use model::*;
