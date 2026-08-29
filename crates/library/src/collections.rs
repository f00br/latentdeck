use std::collections::BTreeSet;

use rusqlite::{OptionalExtension as _, Transaction, params};
use uuid::Uuid;

use crate::{
    CartridgeKey, CollectionId, CollectionRecord, ErrorCode, Library, LibraryError, Result,
    db::{
        ensure_cartridge, ensure_collection, invalid, normalize, now_ms, usize_to_i64,
        validate_cartridge_key, validate_collection_id,
    },
};

const MAX_COLLECTIONS: usize = 512;
const MAX_COLLECTION_NAME_BYTES: usize = 128;
const MAX_COLLECTION_MEMBERS: usize = 100_000;

impl Library {
    /// Lists stable virtual views first, followed by persisted collections in
    /// manual order.
    ///
    /// # Errors
    ///
    /// Returns a stable database error if counts or collection rows fail.
    pub fn list_collections(&self) -> Result<Vec<CollectionRecord>> {
        let all_count = count_query(&self.connection, "SELECT COUNT(*) FROM cartridges", [])?;
        let unassigned_count = count_query(
            &self.connection,
            "SELECT COUNT(*) FROM cartridges c WHERE NOT EXISTS \
             (SELECT 1 FROM collection_members cm WHERE cm.archive_sha256 = c.archive_sha256)",
            [],
        )?;
        let mut collections = vec![
            CollectionRecord {
                id: CollectionId::all_cartridges(),
                name: "All Cartridges".to_owned(),
                position: None,
                is_virtual: true,
                member_count: all_count,
            },
            CollectionRecord {
                id: CollectionId::unassigned(),
                name: "Unassigned".to_owned(),
                position: None,
                is_virtual: true,
                member_count: unassigned_count,
            },
        ];
        let mut statement = self
            .connection
            .prepare(
                "SELECT c.collection_id, c.name, c.position, COUNT(cm.archive_sha256) \
                 FROM collections c LEFT JOIN collection_members cm \
                 ON cm.collection_id = c.collection_id \
                 GROUP BY c.collection_id, c.name, c.position \
                 ORDER BY c.position, c.collection_id",
            )
            .map_err(LibraryError::database)?;
        let persisted = statement
            .query_map([], |row| {
                let position = row.get::<_, i64>(2)?;
                let member_count = row.get::<_, i64>(3)?;
                Ok(CollectionRecord {
                    id: CollectionId::new_unchecked(row.get::<_, String>(0)?),
                    name: row.get(1)?,
                    position: Some(u32::try_from(position).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?),
                    is_virtual: false,
                    member_count: u64::try_from(member_count).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    })?,
                })
            })
            .map_err(LibraryError::database)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(LibraryError::database)?;
        collections.extend(persisted);
        Ok(collections)
    }

    /// Creates a persisted flat collection at the end of manual order.
    ///
    /// # Errors
    ///
    /// Rejects reserved/duplicate/bounded names and the collection ceiling.
    pub fn create_collection(&mut self, name: &str) -> Result<CollectionRecord> {
        let (display, normalized) = validate_collection_name(name)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        let count = count_query(&transaction, "SELECT COUNT(*) FROM collections", [])?;
        if count >= 512 {
            return Err(invalid("collection count exceeds the library ceiling"));
        }
        let duplicate = transaction
            .query_row(
                "SELECT 1 FROM collections WHERE normalized_name = ?1",
                [&normalized],
                |_| Ok(()),
            )
            .optional()
            .map_err(LibraryError::database)?
            .is_some();
        if duplicate {
            return Err(LibraryError::new(
                ErrorCode::Conflict,
                "collection name already exists",
            ));
        }
        let id = CollectionId::new_unchecked(Uuid::now_v7().to_string());
        let position = i64::try_from(count)
            .map_err(|_| invalid("collection position exceeds SQLite range"))?;
        let now = now_ms();
        transaction
            .execute(
                "INSERT INTO collections(collection_id, name, normalized_name, position, \
                 created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![id.as_str(), display, normalized, position, now],
            )
            .map_err(LibraryError::database)?;
        let result_position = u32::try_from(position)
            .map_err(|_| invalid("collection position exceeds the public API range"))?;
        transaction.commit().map_err(LibraryError::database)?;
        Ok(CollectionRecord {
            id,
            name: display,
            position: Some(result_position),
            is_virtual: false,
            member_count: 0,
        })
    }

    /// Renames one persisted collection without changing its order.
    ///
    /// # Errors
    ///
    /// Virtual views and duplicate/bounded names are rejected.
    pub fn rename_collection(&mut self, id: &CollectionId, name: &str) -> Result<()> {
        ensure_mutable(id)?;
        let (display, normalized) = validate_collection_name(name)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_collection(&transaction, id)?;
        let conflict = transaction
            .query_row(
                "SELECT 1 FROM collections WHERE normalized_name = ?1 AND collection_id <> ?2",
                params![normalized, id.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(LibraryError::database)?
            .is_some();
        if conflict {
            return Err(LibraryError::new(
                ErrorCode::Conflict,
                "collection name already exists",
            ));
        }
        transaction
            .execute(
                "UPDATE collections SET name = ?2, normalized_name = ?3, updated_at_ms = ?4 \
                 WHERE collection_id = ?1",
                params![id.as_str(), display, normalized, now_ms()],
            )
            .map_err(LibraryError::database)?;
        transaction.commit().map_err(LibraryError::database)
    }

    /// Deletes only the collection and its membership rows. Cartridge records
    /// and filesystem content are never touched.
    ///
    /// # Errors
    ///
    /// Virtual and unknown collections are rejected.
    pub fn delete_collection(&mut self, id: &CollectionId) -> Result<()> {
        ensure_mutable(id)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_collection(&transaction, id)?;
        transaction
            .execute(
                "DELETE FROM collections WHERE collection_id = ?1",
                [id.as_str()],
            )
            .map_err(LibraryError::database)?;
        let remaining = collection_ids(&transaction)?;
        resequence_collections(&transaction, &remaining)?;
        transaction.commit().map_err(LibraryError::database)
    }

    /// Replaces the complete persisted collection order atomically.
    ///
    /// # Errors
    ///
    /// The input must contain every real collection exactly once and no
    /// virtual ID.
    pub fn reorder_collections(&mut self, ordered: &[CollectionId]) -> Result<()> {
        if ordered.len() > MAX_COLLECTIONS
            || ordered.iter().any(CollectionId::is_virtual)
            || ordered.iter().any(|id| validate_collection_id(id).is_err())
            || ordered
                .iter()
                .map(CollectionId::as_str)
                .collect::<BTreeSet<_>>()
                .len()
                != ordered.len()
        {
            return Err(invalid(
                "collection order contains invalid or duplicate IDs",
            ));
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        let current = collection_ids(&transaction)?;
        let current_set = current
            .iter()
            .map(CollectionId::as_str)
            .collect::<BTreeSet<_>>();
        let requested_set = ordered
            .iter()
            .map(CollectionId::as_str)
            .collect::<BTreeSet<_>>();
        if current_set != requested_set {
            return Err(invalid(
                "collection order must include every collection exactly once",
            ));
        }
        resequence_collections(&transaction, ordered)?;
        transaction.commit().map_err(LibraryError::database)
    }

    /// Adds a cartridge to a collection at the end of that collection's
    /// manual order. Repeated adds are idempotent.
    ///
    /// # Errors
    ///
    /// Virtual/unknown collections and unknown cartridges are rejected.
    pub fn add_to_collection(
        &mut self,
        collection_id: &CollectionId,
        key: &CartridgeKey,
    ) -> Result<()> {
        ensure_mutable(collection_id)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_collection(&transaction, collection_id)?;
        ensure_cartridge(&transaction, key)?;
        let exists = transaction
            .query_row(
                "SELECT 1 FROM collection_members WHERE collection_id = ?1 \
                 AND archive_sha256 = ?2",
                params![collection_id.as_str(), key.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(LibraryError::database)?
            .is_some();
        if !exists {
            let position: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM collection_members WHERE collection_id = ?1",
                    [collection_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(LibraryError::database)?;
            if position
                >= i64::try_from(MAX_COLLECTION_MEMBERS)
                    .map_err(|_| invalid("member ceiling exceeds SQLite range"))?
            {
                return Err(invalid(
                    "collection member count exceeds the library ceiling",
                ));
            }
            transaction
                .execute(
                    "INSERT INTO collection_members(collection_id, archive_sha256, position) \
                     VALUES (?1, ?2, ?3)",
                    params![collection_id.as_str(), key.as_str(), position],
                )
                .map_err(LibraryError::database)?;
        }
        transaction.commit().map_err(LibraryError::database)
    }

    /// Removes a membership and compacts the remaining manual order.
    ///
    /// # Errors
    ///
    /// Virtual/unknown collections and unknown cartridges are rejected.
    pub fn remove_from_collection(
        &mut self,
        collection_id: &CollectionId,
        key: &CartridgeKey,
    ) -> Result<()> {
        ensure_mutable(collection_id)?;
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_collection(&transaction, collection_id)?;
        ensure_cartridge(&transaction, key)?;
        transaction
            .execute(
                "DELETE FROM collection_members WHERE collection_id = ?1 \
                 AND archive_sha256 = ?2",
                params![collection_id.as_str(), key.as_str()],
            )
            .map_err(LibraryError::database)?;
        let remaining = member_keys(&transaction, collection_id)?;
        resequence_members(&transaction, collection_id, &remaining)?;
        transaction.commit().map_err(LibraryError::database)
    }

    /// Replaces one collection's complete cartridge order atomically.
    ///
    /// # Errors
    ///
    /// The input must contain every current member exactly once.
    pub fn reorder_collection(
        &mut self,
        collection_id: &CollectionId,
        ordered: &[CartridgeKey],
    ) -> Result<()> {
        ensure_mutable(collection_id)?;
        if ordered.len() > MAX_COLLECTION_MEMBERS
            || ordered
                .iter()
                .any(|key| validate_cartridge_key(key).is_err())
            || ordered
                .iter()
                .map(CartridgeKey::as_str)
                .collect::<BTreeSet<_>>()
                .len()
                != ordered.len()
        {
            return Err(invalid("cartridge order contains duplicate identities"));
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(LibraryError::database)?;
        ensure_collection(&transaction, collection_id)?;
        let current = member_keys(&transaction, collection_id)?;
        let current_set = current
            .iter()
            .map(CartridgeKey::as_str)
            .collect::<BTreeSet<_>>();
        let requested_set = ordered
            .iter()
            .map(CartridgeKey::as_str)
            .collect::<BTreeSet<_>>();
        if current_set != requested_set {
            return Err(invalid(
                "cartridge order must include every member exactly once",
            ));
        }
        resequence_members(&transaction, collection_id, ordered)?;
        transaction.commit().map_err(LibraryError::database)
    }
}

fn validate_collection_name(name: &str) -> Result<(String, String)> {
    let display = name.trim();
    if display.is_empty()
        || display.len() > MAX_COLLECTION_NAME_BYTES
        || display.chars().any(char::is_control)
    {
        return Err(invalid(
            "collection name is empty, too long, or contains controls",
        ));
    }
    let normalized = normalize(display);
    if matches!(normalized.as_str(), "all cartridges" | "unassigned") {
        return Err(LibraryError::new(
            ErrorCode::VirtualCollection,
            "virtual collection names are reserved",
        ));
    }
    Ok((display.to_owned(), normalized))
}

fn ensure_mutable(id: &CollectionId) -> Result<()> {
    if id.is_virtual() {
        return Err(LibraryError::new(
            ErrorCode::VirtualCollection,
            "virtual collections are query-only",
        ));
    }
    Ok(())
}

fn collection_ids(connection: &rusqlite::Connection) -> Result<Vec<CollectionId>> {
    let mut statement = connection
        .prepare("SELECT collection_id FROM collections ORDER BY position, collection_id")
        .map_err(LibraryError::database)?;
    statement
        .query_map([], |row| {
            row.get::<_, String>(0).map(CollectionId::new_unchecked)
        })
        .map_err(LibraryError::database)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::database)
}

fn member_keys(
    connection: &rusqlite::Connection,
    collection_id: &CollectionId,
) -> Result<Vec<CartridgeKey>> {
    let mut statement = connection
        .prepare(
            "SELECT archive_sha256 FROM collection_members WHERE collection_id = ?1 \
             ORDER BY position, archive_sha256",
        )
        .map_err(LibraryError::database)?;
    statement
        .query_map([collection_id.as_str()], |row| {
            row.get::<_, String>(0).map(CartridgeKey::new_unchecked)
        })
        .map_err(LibraryError::database)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(LibraryError::database)
}

fn resequence_collections(transaction: &Transaction<'_>, ordered: &[CollectionId]) -> Result<()> {
    let offset = usize_to_i64(ordered.len().saturating_add(1))?;
    transaction
        .execute("UPDATE collections SET position = position + ?1", [offset])
        .map_err(LibraryError::database)?;
    for (position, id) in ordered.iter().enumerate() {
        transaction
            .execute(
                "UPDATE collections SET position = ?2, updated_at_ms = ?3 \
                 WHERE collection_id = ?1",
                params![id.as_str(), usize_to_i64(position)?, now_ms()],
            )
            .map_err(LibraryError::database)?;
    }
    Ok(())
}

fn resequence_members(
    transaction: &Transaction<'_>,
    collection_id: &CollectionId,
    ordered: &[CartridgeKey],
) -> Result<()> {
    let offset = usize_to_i64(ordered.len().saturating_add(1))?;
    transaction
        .execute(
            "UPDATE collection_members SET position = position + ?2 WHERE collection_id = ?1",
            params![collection_id.as_str(), offset],
        )
        .map_err(LibraryError::database)?;
    for (position, key) in ordered.iter().enumerate() {
        transaction
            .execute(
                "UPDATE collection_members SET position = ?3 \
                 WHERE collection_id = ?1 AND archive_sha256 = ?2",
                params![
                    collection_id.as_str(),
                    key.as_str(),
                    usize_to_i64(position)?
                ],
            )
            .map_err(LibraryError::database)?;
    }
    Ok(())
}

fn count_query<P>(connection: &rusqlite::Connection, sql: &str, parameters: P) -> Result<u64>
where
    P: rusqlite::Params,
{
    let count: i64 = connection
        .query_row(sql, parameters, |row| row.get(0))
        .map_err(LibraryError::database)?;
    u64::try_from(count).map_err(|_| {
        LibraryError::new(
            ErrorCode::Database,
            "library count is outside the valid range",
        )
    })
}
