# LatentDeck Library

`latentdeck-library` is the UI-independent local index shared by LatentDeck and
LatentPlayer. It stores metadata and organization in SQLite while every `.lc`
remains an ordinary user-owned file.

## Trust and import boundary

- Files enter the index only through `Library::import_file` or an explicitly
  selected `Library::import_folder` root. There is no drive-wide discovery or
  background filesystem scan.
- Folder recursion is opt-in, does not follow symbolic-link directories, and
  is bounded by a caller-visible candidate ceiling.
- Every import uses the Rust Cartridge SDK's full validation path, including
  archive SHA-256, payload hashes, tensor layout, finite-value checks, profile
  compatibility, and size limits.
- Incremental reindex touches only registered paths. It skips unchanged
  size/mtime pairs, retains missing or invalid paths, and reports their state.
- If a registered path now contains a different valid archive hash, reindex
  records `content_changed` but preserves the old cartridge identity and its
  collection memberships. Only another explicit `import_file` accepts the new
  identity. Membership is never silently transferred.
- Public errors contain stable codes and path-free details. Local paths remain
  structured local index data so the application can show and repair them.

## Identity model

The archive SHA-256 is the immutable database key. The manifest
`cartridge_id`, canonical manifest JSON, codec/timing metadata, and import
sequence are cached once for that exact archive. Multiple registered paths may
point to the same archive identity.

Accepting changed content at an existing path creates or selects a different
archive identity and repoints only that path. The previous identity remains in
the index, including favorites, tags, recent state, and collection membership;
without another present path it is visibly unavailable.

## Collections

Collections are flat many-to-many membership sets:

- a cartridge may belong to any number of collections;
- real collections and each collection's members have independent manual
  orders;
- delete removes the collection and its memberships only, never cartridges or
  files;
- `latentdeck.virtual.all` and `latentdeck.virtual.unassigned` are stable
  query-only IDs synthesized by the API and rejected by every mutation;
- `All Cartridges` always queries the complete immutable index;
- `Unassigned` contains identities with no real collection membership.

The product/UI term is **Collection**. A physical-style Deck selector may label
the same active selection **Bank**, but that is not a second persistence model.

## Schema 1

SQLite `PRAGMA user_version = 1` contains:

| Table | Purpose |
|---|---|
| `cartridges` | Immutable archive identity, canonical manifest cache, codec/timing metadata, favorite, import order |
| `cartridge_paths` | Canonical local paths, incremental file facts, visible path state and warning |
| `collections` | Real collection identity, case-insensitive name key, manual order |
| `collection_members` | Many-to-many membership and per-collection manual order |
| `cartridge_tags` | Case-insensitively unique display tags |
| `recent_cartridges` | Deterministic monotonic recent order |

Migrations run in an immediate transaction with foreign keys enabled. A
database created by a newer schema is rejected without mutation. Foreign keys
prevent dangling membership while `ON DELETE CASCADE` is limited to derived
membership/tag/recent rows; cartridge rows and user files have no deletion API
in this crate.

## Determinism and bounds

- All and Unassigned order by immutable import sequence and archive hash.
- Real collection queries follow stored member order and then archive hash.
- Search filters preserve the selected view order and match cartridge ID,
  codec identifiers, tags, and registered filenames.
- Collection names, tags, search text, query results, collection count, and
  folder traversal are explicitly bounded.
- Reorder operations require an exact permutation and run transactionally, so
  malformed requests leave the previous order intact.

## Local verification

Tests build synthetic LC archives only in temporary directories:

```powershell
cargo test -p latentdeck-library
cargo clippy -p latentdeck-library --all-targets -- -D warnings
```
