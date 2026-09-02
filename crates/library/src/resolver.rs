use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use latentdeck_cartridge::reader::{
    IntegrityValidatedCartridge, ValidationOptions, open_integrity_validated,
};
use rusqlite::{OptionalExtension as _, params, params_from_iter};

use crate::{
    CartridgeKey, ErrorCode, Library, LibraryError, Result,
    db::{invalid, usize_to_i64, validate_cartridge_key},
};

/// Maximum number of registered `present` paths that one resolution attempt
/// will revalidate, in ascending `SQLite` `path_id` order.
pub const MAX_DECK_SOURCE_PATH_CANDIDATES: usize = 16;
/// Maximum exact identities in one metadata-only Deck eligibility request.
pub const MAX_INDEXED_DECK_SOURCES: usize = 1_004;
/// Keep dynamic `VALUES` statements comfortably below `SQLite`'s conservative
/// bind-parameter ceiling: each row binds archive hash plus cartridge ID.
pub const MAX_INDEXED_DECK_SOURCE_QUERY_CHUNK: usize = 250;

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
#[derive(Debug)]
pub struct ResolvedDeckSource {
    identity: DeckSourceIdentity,
    path: PathBuf,
    validated_cartridge: IntegrityValidatedCartridge,
}

/// Backend-only immutable metadata selected from the Library index. It has no
/// filesystem path or retained handle and is therefore suitable only for UI
/// compatibility display, never launch authority.
#[derive(Debug)]
pub struct IndexedDeckSource {
    identity: DeckSourceIdentity,
    manifest_json: String,
}

impl IndexedDeckSource {
    #[must_use]
    pub const fn identity(&self) -> &DeckSourceIdentity {
        &self.identity
    }

    #[must_use]
    pub fn manifest_json(&self) -> &str {
        &self.manifest_json
    }
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

    #[must_use]
    pub const fn validated_cartridge(&self) -> &IntegrityValidatedCartridge {
        &self.validated_cartridge
    }

    /// Duplicate the retained read-only LC handle without repeating full
    /// cartridge validation.
    ///
    /// # Errors
    ///
    /// Returns a path-free library error if the operating system cannot clone
    /// the retained handle.
    pub fn try_clone_retained(&self) -> Result<Self> {
        Ok(Self {
            identity: self.identity.clone(),
            path: self.path.clone(),
            validated_cartridge: self
                .validated_cartridge
                .try_clone_retained()
                .map_err(|error| LibraryError::cartridge(&error))?,
        })
    }
}

impl Library {
    /// Resolve a bounded ordered batch from immutable Library metadata only.
    ///
    /// This never opens a cartridge or returns a machine path. Individual
    /// missing, mismatched, or non-present identities remain aligned with the
    /// input as per-item errors. Exact launch must subsequently call
    /// [`Self::resolve_deck_source`] for selected slots.
    ///
    /// # Errors
    ///
    /// Returns a database or input error for the batch itself. Per-identity
    /// availability errors are returned in the ordered result vector.
    pub fn indexed_deck_sources(
        &self,
        identities: &[DeckSourceIdentity],
    ) -> Result<Vec<Result<IndexedDeckSource>>> {
        if identities.len() > MAX_INDEXED_DECK_SOURCES {
            return Err(invalid("Deck source metadata batch exceeds its bound"));
        }
        let mut output = Vec::with_capacity(identities.len());
        for chunk in identities.chunks(MAX_INDEXED_DECK_SOURCE_QUERY_CHUNK) {
            let mut sql =
                String::from("WITH requested(ordinal, archive_sha256, requested_id) AS (VALUES ");
            let mut bindings = Vec::with_capacity(chunk.len() * 2);
            for (ordinal, identity) in chunk.iter().enumerate() {
                if ordinal > 0 {
                    sql.push(',');
                }
                let first_parameter = bindings.len() + 1;
                let second_parameter = first_parameter + 1;
                write!(sql, "({ordinal},?{first_parameter},?{second_parameter})")
                    .expect("writing to a String cannot fail");
                bindings.push(identity.archive_sha256().as_str());
                bindings.push(identity.cartridge_id());
            }
            sql.push_str(
                ") SELECT r.ordinal, r.requested_id, c.cartridge_id, c.manifest_json, \
                 EXISTS(SELECT 1 FROM cartridge_paths p \
                        WHERE p.archive_sha256 = r.archive_sha256 AND p.state = 'present') \
                 FROM requested r LEFT JOIN cartridges c \
                      ON c.archive_sha256 = r.archive_sha256 ORDER BY r.ordinal",
            );
            let mut statement = self
                .connection
                .prepare(&sql)
                .map_err(LibraryError::database)?;
            let rows = statement
                .query_map(params_from_iter(bindings), |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                })
                .map_err(LibraryError::database)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(LibraryError::database)?;
            if rows.len() != chunk.len() {
                return Err(LibraryError::new(
                    ErrorCode::Database,
                    "Deck source metadata batch returned an incomplete result",
                ));
            }
            for (ordinal, requested_id, indexed_id, manifest_json, present) in rows {
                let ordinal = usize::try_from(ordinal).map_err(|_| {
                    LibraryError::new(
                        ErrorCode::Database,
                        "Deck source metadata batch returned an invalid order",
                    )
                })?;
                let identity = chunk.get(ordinal).ok_or_else(|| {
                    LibraryError::new(
                        ErrorCode::Database,
                        "Deck source metadata batch returned an invalid order",
                    )
                })?;
                let item = match (indexed_id, manifest_json, present) {
                    (Some(indexed_id), Some(manifest_json), true)
                        if indexed_id == requested_id
                            && requested_id == identity.cartridge_id() =>
                    {
                        Ok(IndexedDeckSource {
                            identity: identity.clone(),
                            manifest_json,
                        })
                    }
                    (Some(_), Some(_), _) => Err(LibraryError::new(
                        ErrorCode::Conflict,
                        "Deck source identity does not match available indexed metadata",
                    )),
                    _ => Err(source_unavailable()),
                };
                output.push(item);
            }
        }
        Ok(output)
    }

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
                Ok(validated_cartridge) => {
                    return Ok(ResolvedDeckSource {
                        identity: identity.clone(),
                        path,
                        validated_cartridge,
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

fn validate_registered_path(
    path: &Path,
    identity: &DeckSourceIdentity,
) -> Result<IntegrityValidatedCartridge> {
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
    Ok(validated)
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
