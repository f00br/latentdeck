//! Hash-bound lifecycle management for `.ld` Deck packages and `.ldcodec`
//! Codec packages.

mod activation;
mod archive;
mod compatibility;
mod error;
mod lifecycle;
mod model;
mod runtime_seal;
mod schema;

pub use activation::{ActivePackageCache, ActivePackageCacheStats};
pub use archive::{PackRequest, inspect, pack};
pub use compatibility::{
    PackageCompatibility, SelectedSourceCompatibility, SelectedSourceScope,
    resolve_package_compatibility, resolve_selected_compatibility,
};
pub use error::{ErrorCode, ExtensionError, Result};
pub use lifecycle::{
    ExtensionRoots, InstallRequest, RemoveOptions, compatibility_matrix, disable, enable,
    enable_if_only_installed_version, install, install_from_bundled_index, inventory, list,
    list_kind, remove, repair, repair_from_bundled_index, resolve_active, resolve_installed,
    verify,
};
pub use model::*;
