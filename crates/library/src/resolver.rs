use std::{
    fs,
    path::{Path, PathBuf},
};

use latentdeck_cartridge::reader::{ValidationOptions, open_integrity_validated};
use rusqlite::{OptionalExtension as _, params};

use crate::{
    CartridgeKey, ErrorCode, Library, LibraryError, Result,
    db::{invalid, usize_to_i64, validate_cartridge_key},
};

/// Maximum number of registered `present` paths that one resolution attempt
/// will revalidate, in ascending `SQLite` `path_id` order.
pub const MAX_DECK_SOURCE_PATH_CANDIDATES: usize = 16;

/// Immutable cartridge identity accepted by the backend Deck source resolver.
/// It deliberately contains no caller-selected filesystem path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckSourceIdentity {
    cartridge_id: String,
    archive_sha256: CartridgeKey,
}

impl DeckSourceIdentity {
    /// Builds a canonical Deck source identity.
    ///
    /// # Errors
    ///
    /// Rejects a non-canonical UUID or SHA-256 identity.
    pub fn new(cartridge_id: impl Into<String>, archive_sha256: CartridgeKey) -> Result<Self> {
        let cartridge_id = cartridge_id.into();
        validate_cartridge_id(&cartridge_id)?;
        validate_cartridge_key(&archive_sha256)?;
        Ok(Self {
            cartridge_id,
            archive_sha256,
        })
    }

    #[must_use]
    pub fn cartridge_id(&self) -> &str {
        &self.cartridge_id
    }

    #[must_use]
    pub fn archive_sha256(&self) -> &CartridgeKey {
        &self.archive_sha256
    }
}

/// A backend-only source selected from the library index and revalidated from
/// disk. This type is intentionally not serializable so the machine path
/// cannot accidentally cross the public UI command boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDeckSource {
    identity: DeckSourceIdentity,
    path: PathBuf,
}

impl ResolvedDeckSource {
    #[must_use]
    pub fn identity(&self) -> &DeckSourceIdentity {
        &self.identity
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Library {
    /// Resolves one Deck slot source only through registered `present` paths,
    /// then performs full LC validation before returning the local path.
    ///
    /// # Errors
    ///
    /// Returns only path-free typed library errors. No filesystem path supplied
    /// by a UI caller is accepted by this API.
    pub fn resolve_deck_source(&self, identity: &DeckSourceIdentity) -> Result<ResolvedDeckSource> {
        let indexed_cartridge_id = self
            .connection
            .query_row(
                "SELECT cartridge_id FROM cartridges WHERE archive_sha256 = ?1",
                [identity.archive_sha256.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(LibraryError::database)?
            .ok_or_else(|| {
                LibraryError::new(ErrorCode::NotFound, "Deck source is not available")
            })?;

        if indexed_cartridge_id != identity.cartridge_id {
            return Err(LibraryError::new(
                ErrorCode::Conflict,
                "Deck source identity does not match the library index",
            ));
        }

        let mut statement = self
            .connection
            .prepare(
                "SELECT path_text FROM cartridge_paths \
                 WHERE archive_sha256 = ?1 AND state = 'present' \
                 ORDER BY path_id LIMIT ?2",
            )
            .map_err(LibraryError::database)?;
        let candidates = statement
            .query_map(
                params![
                    identity.archive_sha256.as_str(),
                    usize_to_i64(MAX_DECK_SOURCE_PATH_CANDIDATES)?
                ],
                |row| row.get::<_, String>(0).map(PathBuf::from),
            )
            .map_err(LibraryError::database)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::database)?;

        let mut first_error = None;
        for path in candidates {
            match validate_registered_path(&path, identity) {
                Ok(()) => {
                    return Ok(ResolvedDeckSource {
                        identity: identity.clone(),
                        path,
                    });
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        Err(first_error.unwrap_or_else(source_unavailable))
    }
}

fn validate_registered_path(path: &Path, identity: &DeckSourceIdentity) -> Result<()> {
    let source_metadata = fs::symlink_metadata(path).map_err(|_error| source_unavailable())?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err(source_unavailable());
    }

    let validated = open_integrity_validated(path, &ValidationOptions::default())
        .map_err(|error| LibraryError::cartridge(&error))?;
    if validated.receipt().archive_sha256.to_string() != identity.archive_sha256.as_str()
        || validated.manifest().cartridge_id.0 != identity.cartridge_id
    {
        return Err(LibraryError::new(
            ErrorCode::Conflict,
            "registered Deck source content changed",
        ));
    }
    let current_metadata = fs::metadata(path).map_err(|_error| source_unavailable())?;
    if current_metadata.len() != validated.receipt().archive_bytes {
        return Err(LibraryError::new(
            ErrorCode::Conflict,
            "registered Deck source changed during validation",
        ));
    }
    Ok(())
}

fn validate_cartridge_id(value: &str) -> Result<()> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| invalid("cartridge identity is not a canonical UUID"))?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err(invalid("cartridge identity is not a canonical UUID"));
    }
    Ok(())
}

fn source_unavailable() -> LibraryError {
    LibraryError::new(ErrorCode::NotFound, "Deck source is not available")
}
