//! UI-independent `SQLite` index for validated Latent Cartridges and flat
//! many-to-many collections.

mod backup;
mod collections;
mod db;
mod error;
mod import;
mod migrations;
mod model;
mod resolver;

pub use error::{ErrorCode, LibraryError, Result};
pub use migrations::SCHEMA_VERSION;
pub use model::*;
pub use resolver::{DeckSourceIdentity, MAX_DECK_SOURCE_PATH_CANDIDATES, ResolvedDeckSource};

use rusqlite::Connection;

/// One local library database. The connection is intentionally not shared
/// internally; applications put this UI-independent value behind their own
/// command/actor boundary.
#[derive(Debug)]
pub struct Library {
    pub(crate) connection: Connection,
}
