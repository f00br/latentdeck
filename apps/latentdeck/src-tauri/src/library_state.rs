use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use latentdeck_core::diagnostics::{LogLevel, record_global};
use latentdeck_library::{
    ALL_CARTRIDGES_ID, Availability, CartridgeKey, CartridgeRecord, CollectionId, CollectionRecord,
    DeckSourceIdentity, FolderImportOptions, Library, LibraryError, PathState, QueryOptions,
    ReindexDisposition, ResolvedDeckSource,
};
use serde::Serialize;
use tauri::State;

const UI_QUERY_LIMIT: usize = 1_000;
const RECENT_LIMIT: usize = 8;
const MAX_EXPLICIT_FILES: usize = 1_024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    code: String,
    message: String,
}

impl CommandError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        record_global(LogLevel::Error, "app.command_failed", Some(&code));
        Self {
            code,
            message: message.into(),
        }
    }

    fn task_stopped() -> Self {
        Self::new(
            "library.task_stopped",
            "The local library task stopped unexpectedly.",
        )
    }
}

impl From<LibraryError> for CommandError {
    fn from(error: LibraryError) -> Self {
        let code = error
            .cartridge_code
            .unwrap_or_else(|| error.code.as_str().to_owned());
        Self::new(code, error.detail)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SlotAssignmentView {
    deck_type: &'static str,
    slot: String,
    archive_sha256: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeckSessionView {
    active_collection_id: String,
    loaded_slots: Vec<SlotAssignmentView>,
}

#[derive(Debug)]
struct DeckSessionState {
    active_collection_id: CollectionId,
    loaded_slots: BTreeMap<(DeckKind, String), CartridgeKey>,
    runtime_sessions: BTreeMap<DeckKind, DeckRuntimeSession>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeckKind {
    D2,
    Q4,
}

impl DeckKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::D2 => "d2",
            Self::Q4 => "q4",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DeckRuntimeSession {
    generation: u64,
    open: bool,
}

#[derive(Debug)]
struct DeckSessionClosed;

impl Default for DeckSessionState {
    fn default() -> Self {
        Self {
            active_collection_id: CollectionId::all_cartridges(),
            loaded_slots: BTreeMap::new(),
            runtime_sessions: BTreeMap::new(),
        }
    }
}

impl DeckSessionState {
    fn select_collection(&mut self, collection_id: CollectionId) {
        self.active_collection_id = collection_id;
    }

    fn view(&self) -> DeckSessionView {
        DeckSessionView {
            active_collection_id: self.active_collection_id.as_str().to_owned(),
            loaded_slots: self
                .loaded_slots
                .iter()
                .map(|((deck, slot), key)| SlotAssignmentView {
                    deck_type: deck.as_str(),
                    slot: slot.clone(),
                    archive_sha256: key.as_str().to_owned(),
                })
                .collect(),
        }
    }

    fn begin_deck(&mut self, deck: DeckKind) -> u64 {
        self.loaded_slots
            .retain(|(loaded_deck, _), _| *loaded_deck != deck);
        let session = self.runtime_sessions.entry(deck).or_default();
        session.generation = session.generation.wrapping_add(1).max(1);
        session.open = true;
        session.generation
    }

    fn publish_deck_slots<const N: usize>(
        &mut self,
        deck: DeckKind,
        generation: u64,
        slots: [(&str, CartridgeKey); N],
    ) -> Result<(), DeckSessionClosed> {
        let current = self
            .runtime_sessions
            .get(&deck)
            .filter(|session| session.open && session.generation == generation)
            .ok_or(DeckSessionClosed)?;
        debug_assert_eq!(current.generation, generation);
        self.loaded_slots
            .retain(|(loaded_deck, _), _| *loaded_deck != deck);
        self.loaded_slots.extend(
            slots
                .into_iter()
                .map(|(slot, key)| ((deck, slot.to_owned()), key)),
        );
        Ok(())
    }

    fn close_deck(&mut self, deck: DeckKind, generation: u64) -> bool {
        let Some(session) = self.runtime_sessions.get_mut(&deck) else {
            return false;
        };
        if session.generation != generation || !session.open {
            return false;
        }
        session.open = false;
        self.loaded_slots
            .retain(|(loaded_deck, _), _| *loaded_deck != deck);
        true
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectionView {
    id: String,
    name: String,
    position: Option<u32>,
    is_virtual: bool,
    member_count: u64,
}

impl From<CollectionRecord> for CollectionView {
    fn from(record: CollectionRecord) -> Self {
        Self {
            id: record.id.as_str().to_owned(),
            name: record.name,
            position: record.position,
            is_virtual: record.is_virtual,
            member_count: record.member_count,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CartridgePathView {
    path: String,
    file_name: String,
    state: PathState,
    warning_code: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CartridgeView {
    archive_sha256: String,
    cartridge_id: String,
    codec_family: String,
    codec_profile: String,
    decoded_width: u32,
    decoded_height: u32,
    decoded_frame_count: u64,
    duration_numerator: u64,
    duration_denominator: u64,
    favorite: bool,
    tags: Vec<String>,
    availability: Availability,
    paths: Vec<CartridgePathView>,
}

impl From<CartridgeRecord> for CartridgeView {
    fn from(record: CartridgeRecord) -> Self {
        let metadata = record.metadata;
        Self {
            archive_sha256: record.key.as_str().to_owned(),
            cartridge_id: metadata.cartridge_id,
            codec_family: metadata.codec_family,
            codec_profile: metadata.codec_profile,
            decoded_width: metadata.decoded_width,
            decoded_height: metadata.decoded_height,
            decoded_frame_count: metadata.decoded_frame_count,
            duration_numerator: metadata.duration_numerator,
            duration_denominator: metadata.duration_denominator,
            favorite: record.favorite,
            tags: record.tags,
            availability: record.availability,
            paths: record
                .paths
                .into_iter()
                .map(|path| CartridgePathView {
                    file_name: path
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("cartridge.lc")
                        .to_owned(),
                    path: path.path.to_string_lossy().into_owned(),
                    state: path.state,
                    warning_code: path.warning_code,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LibraryView {
    deck_session: DeckSessionView,
    collections: Vec<CollectionView>,
    cartridges: Vec<CartridgeView>,
    recent: Vec<CartridgeView>,
    search: String,
    total_indexed: u64,
    active_member_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportFailureView {
    path: String,
    code: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportSummary {
    accepted: usize,
    rejected: Vec<ImportFailureView>,
    ignored_non_cartridges: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReindexSummary {
    unchanged: usize,
    present: usize,
    missing: usize,
    invalid: usize,
    content_changed: usize,
}

pub(crate) struct LibraryController {
    library: Library,
    deck_session: DeckSessionState,
}

impl LibraryController {
    pub(crate) fn new(library: Library) -> Self {
        Self {
            library,
            deck_session: DeckSessionState::default(),
        }
    }

    fn snapshot(&mut self, search: Option<String>) -> Result<LibraryView, CommandError> {
        let collections = self.library.list_collections()?;
        if !collections
            .iter()
            .any(|collection| collection.id == self.deck_session.active_collection_id)
        {
            self.deck_session
                .select_collection(CollectionId::all_cartridges());
        }
        let normalized_search = search.unwrap_or_default();
        let cartridges = self.library.query_collection(
            &self.deck_session.active_collection_id,
            &QueryOptions {
                search: (!normalized_search.trim().is_empty()).then_some(normalized_search.clone()),
                limit: UI_QUERY_LIMIT,
            },
        )?;
        let recent = self.library.recent(RECENT_LIMIT)?;
        let total_indexed = collections
            .iter()
            .find(|collection| collection.id.as_str() == ALL_CARTRIDGES_ID)
            .map_or(0, |collection| collection.member_count);
        let active_member_count = collections
            .iter()
            .find(|collection| collection.id == self.deck_session.active_collection_id)
            .map_or(0, |collection| collection.member_count);
        Ok(LibraryView {
            deck_session: self.deck_session.view(),
            collections: collections.into_iter().map(Into::into).collect(),
            cartridges: cartridges.into_iter().map(Into::into).collect(),
            recent: recent.into_iter().map(Into::into).collect(),
            search: normalized_search,
            total_indexed,
            active_member_count,
        })
    }

    fn select_collection(
        &mut self,
        collection_id: CollectionId,
    ) -> Result<DeckSessionView, CommandError> {
        let exists = self
            .library
            .list_collections()?
            .iter()
            .any(|collection| collection.id == collection_id);
        if !exists {
            return Err(CommandError::new(
                "not_found",
                "The selected collection does not exist.",
            ));
        }
        self.deck_session.select_collection(collection_id);
        Ok(self.deck_session.view())
    }

    fn create_collection(&mut self, name: &str) -> Result<DeckSessionView, CommandError> {
        let collection = self.library.create_collection(name)?;
        self.deck_session.select_collection(collection.id);
        Ok(self.deck_session.view())
    }

    fn delete_collection(&mut self, collection_id: &CollectionId) -> Result<(), CommandError> {
        self.library.delete_collection(collection_id)?;
        if self.deck_session.active_collection_id == *collection_id {
            self.deck_session
                .select_collection(CollectionId::all_cartridges());
        }
        Ok(())
    }

    fn import_files(&mut self, paths: Vec<String>) -> Result<ImportSummary, CommandError> {
        if paths.len() > MAX_EXPLICIT_FILES {
            return Err(CommandError::new(
                "invalid_input",
                "Too many files were selected for one explicit import.",
            ));
        }
        let mut accepted = 0_usize;
        let mut rejected = Vec::new();
        for path in paths {
            match self.library.import_file(&path) {
                Ok(_) => accepted = accepted.saturating_add(1),
                Err(error) => {
                    let command_error = CommandError::from(error);
                    rejected.push(ImportFailureView {
                        path,
                        code: command_error.code,
                        message: command_error.message,
                    });
                }
            }
        }
        Ok(ImportSummary {
            accepted,
            rejected,
            ignored_non_cartridges: 0,
        })
    }

    fn import_folder(
        &mut self,
        path: String,
        recursive: bool,
    ) -> Result<ImportSummary, CommandError> {
        let report = self.library.import_folder(
            path,
            &FolderImportOptions {
                recursive,
                ..FolderImportOptions::default()
            },
        )?;
        Ok(ImportSummary {
            accepted: report.accepted.len(),
            rejected: report
                .rejected
                .into_iter()
                .map(|rejected| ImportFailureView {
                    path: rejected.path.to_string_lossy().into_owned(),
                    code: rejected.code,
                    message: rejected
                        .cartridge_code
                        .unwrap_or_else(|| "Cartridge import was rejected.".to_owned()),
                })
                .collect(),
            ignored_non_cartridges: report.ignored_non_cartridges,
        })
    }

    fn reindex(&mut self) -> Result<ReindexSummary, CommandError> {
        let mut summary = ReindexSummary {
            unchanged: 0,
            present: 0,
            missing: 0,
            invalid: 0,
            content_changed: 0,
        };
        for result in self.library.reindex_registered()? {
            let counter = match result.disposition {
                ReindexDisposition::Unchanged => &mut summary.unchanged,
                ReindexDisposition::Present => &mut summary.present,
                ReindexDisposition::Missing => &mut summary.missing,
                ReindexDisposition::Invalid => &mut summary.invalid,
                ReindexDisposition::ContentChanged => &mut summary.content_changed,
            };
            *counter = counter.saturating_add(1);
        }
        Ok(summary)
    }

    fn resolve_deck_source(
        &self,
        identity: &DeckSourceIdentity,
    ) -> Result<ResolvedDeckSource, CommandError> {
        self.library
            .resolve_deck_source(identity)
            .map_err(Into::into)
    }
}

pub(crate) struct AppState {
    controller: Arc<Mutex<LibraryController>>,
}

/// Generation-scoped writer for one running Deck. A terminal worker failure or
/// explicit shutdown can only clear the slots that belong to its own runtime;
/// a late cleanup from an older runtime cannot erase a replacement session.
#[derive(Clone)]
pub(crate) struct DeckSessionLease {
    controller: Arc<Mutex<LibraryController>>,
    deck: DeckKind,
    generation: u64,
}

#[derive(Clone)]
pub(crate) struct LibraryImporter {
    controller: Arc<Mutex<LibraryController>>,
}

impl AppState {
    pub(crate) fn new(library: Library) -> Self {
        Self {
            controller: Arc::new(Mutex::new(LibraryController::new(library))),
        }
    }

    /// Resolve a Deck source through the registered library path only. Full LC
    /// validation runs on a blocking worker thread and the resulting local path
    /// remains a backend-only, non-serializable value.
    pub(crate) async fn resolve_deck_source(
        &self,
        identity: DeckSourceIdentity,
    ) -> Result<ResolvedDeckSource, CommandError> {
        let controller = Arc::clone(&self.controller);
        tauri::async_runtime::spawn_blocking(move || {
            lock_controller(&controller)?.resolve_deck_source(&identity)
        })
        .await
        .map_err(|_| CommandError::task_stopped())?
    }

    pub(crate) fn importer(&self) -> LibraryImporter {
        LibraryImporter {
            controller: Arc::clone(&self.controller),
        }
    }

    /// Begin a replacement runtime session. Existing slots for this Deck are
    /// removed immediately because every open attempt first shuts down the
    /// previous runtime; other concurrently running Deck types are untouched.
    pub(crate) fn begin_deck_session(
        &self,
        deck: DeckKind,
    ) -> Result<DeckSessionLease, CommandError> {
        let generation = lock_controller(&self.controller)?
            .deck_session
            .begin_deck(deck);
        Ok(DeckSessionLease {
            controller: Arc::clone(&self.controller),
            deck,
            generation,
        })
    }
}

impl DeckSessionLease {
    /// Publish identities only after the worker has started and answered its
    /// first status request. A lease already closed by actor recovery rejects
    /// the publication, preventing a false loaded state.
    pub(crate) fn publish<const N: usize>(
        &self,
        slots: [(&str, CartridgeKey); N],
    ) -> Result<(), CommandError> {
        lock_controller(&self.controller)?
            .deck_session
            .publish_deck_slots(self.deck, self.generation, slots)
            .map_err(|_| {
                CommandError::new(
                    "deck.runtime_unavailable",
                    "The Deck stopped before its loaded slots could be retained.",
                )
            })
    }

    /// Close this runtime's view. Stale generations are deliberately ignored.
    pub(crate) fn close(&self) {
        match lock_controller(&self.controller) {
            Ok(mut controller) => {
                controller
                    .deck_session
                    .close_deck(self.deck, self.generation);
            }
            Err(error) => record_global(
                LogLevel::Error,
                "library.deck_session_close_failed",
                Some(&error.code),
            ),
        }
    }
}

impl LibraryImporter {
    /// Import one already validated application-generated cartridge. The path
    /// remains inside the native host and only the content identity crosses
    /// back to the capture actor.
    pub(crate) async fn import_generated(
        &self,
        path: PathBuf,
    ) -> Result<CartridgeKey, CommandError> {
        let controller = Arc::clone(&self.controller);
        tauri::async_runtime::spawn_blocking(move || {
            let mut controller = lock_controller(&controller)?;
            controller
                .library
                .import_file(path)
                .map(|result| result.key)
                .map_err(Into::into)
        })
        .await
        .map_err(|_| CommandError::task_stopped())?
    }
}

fn lock_controller(
    controller: &Arc<Mutex<LibraryController>>,
) -> Result<MutexGuard<'_, LibraryController>, CommandError> {
    controller.lock().map_err(|_| {
        CommandError::new(
            "library.state_poisoned",
            "Library state is unavailable; restart LatentDeck.",
        )
    })
}

fn to_collection_id(value: String) -> CollectionId {
    CollectionId::new_unchecked(value)
}

fn cartridge_key(value: String) -> CartridgeKey {
    CartridgeKey::new_unchecked(value)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_snapshot(
    state: State<'_, AppState>,
    search: Option<String>,
) -> Result<LibraryView, CommandError> {
    lock_controller(&state.controller)?.snapshot(search)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_set_active_collection(
    state: State<'_, AppState>,
    collection_id: String,
) -> Result<DeckSessionView, CommandError> {
    lock_controller(&state.controller)?.select_collection(to_collection_id(collection_id))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn library_import_files(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<ImportSummary, CommandError> {
    let controller = Arc::clone(&state.controller);
    tauri::async_runtime::spawn_blocking(move || lock_controller(&controller)?.import_files(paths))
        .await
        .map_err(|_| CommandError::task_stopped())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn library_import_folder(
    state: State<'_, AppState>,
    path: String,
    recursive: bool,
) -> Result<ImportSummary, CommandError> {
    let controller = Arc::clone(&state.controller);
    tauri::async_runtime::spawn_blocking(move || {
        lock_controller(&controller)?.import_folder(path, recursive)
    })
    .await
    .map_err(|_| CommandError::task_stopped())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn library_reindex(
    state: State<'_, AppState>,
) -> Result<ReindexSummary, CommandError> {
    let controller = Arc::clone(&state.controller);
    tauri::async_runtime::spawn_blocking(move || lock_controller(&controller)?.reindex())
        .await
        .map_err(|_| CommandError::task_stopped())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_create_collection(
    state: State<'_, AppState>,
    name: String,
) -> Result<DeckSessionView, CommandError> {
    lock_controller(&state.controller)?.create_collection(&name)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_rename_collection(
    state: State<'_, AppState>,
    collection_id: String,
    name: String,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?
        .library
        .rename_collection(&to_collection_id(collection_id), &name)
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_delete_collection(
    state: State<'_, AppState>,
    collection_id: String,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?.delete_collection(&to_collection_id(collection_id))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_reorder_collections(
    state: State<'_, AppState>,
    collection_ids: Vec<String>,
) -> Result<(), CommandError> {
    let ids = collection_ids
        .into_iter()
        .map(to_collection_id)
        .collect::<Vec<_>>();
    lock_controller(&state.controller)?
        .library
        .reorder_collections(&ids)
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_add_membership(
    state: State<'_, AppState>,
    collection_id: String,
    archive_sha256: String,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?
        .library
        .add_to_collection(
            &to_collection_id(collection_id),
            &cartridge_key(archive_sha256),
        )
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_remove_membership(
    state: State<'_, AppState>,
    collection_id: String,
    archive_sha256: String,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?
        .library
        .remove_from_collection(
            &to_collection_id(collection_id),
            &cartridge_key(archive_sha256),
        )
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_reorder_members(
    state: State<'_, AppState>,
    collection_id: String,
    archive_sha256_order: Vec<String>,
) -> Result<(), CommandError> {
    let keys = archive_sha256_order
        .into_iter()
        .map(cartridge_key)
        .collect::<Vec<_>>();
    lock_controller(&state.controller)?
        .library
        .reorder_collection(&to_collection_id(collection_id), &keys)
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_set_favorite(
    state: State<'_, AppState>,
    archive_sha256: String,
    favorite: bool,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?
        .library
        .set_favorite(&cartridge_key(archive_sha256), favorite)
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_set_tags(
    state: State<'_, AppState>,
    archive_sha256: String,
    tags: Vec<String>,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?
        .library
        .set_tags(&cartridge_key(archive_sha256), &tags)
        .map_err(Into::into)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_mark_recent(
    state: State<'_, AppState>,
    archive_sha256: String,
) -> Result<(), CommandError> {
    lock_controller(&state.controller)?
        .library
        .mark_recent(&cartridge_key(archive_sha256))
        .map_err(Into::into)
}

pub(crate) fn database_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("library.sqlite3")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_key(byte: char) -> CartridgeKey {
        CartridgeKey::new_unchecked(std::iter::repeat_n(byte, 64).collect::<String>())
    }

    #[test]
    fn changing_or_deleting_active_bank_never_mutates_loaded_slots() {
        let library = Library::in_memory().expect("in-memory library");
        let mut controller = LibraryController::new(library);
        let first = controller
            .library
            .create_collection("First")
            .expect("first collection");
        let second = controller
            .library
            .create_collection("Second")
            .expect("second collection");
        let d2 = controller.deck_session.begin_deck(DeckKind::D2);
        controller
            .deck_session
            .publish_deck_slots(
                DeckKind::D2,
                d2,
                [("A", fake_key('a')), ("B", fake_key('b'))],
            )
            .expect("current D2 session accepts its slots");
        let slots_before = controller.deck_session.view().loaded_slots;

        controller
            .select_collection(first.id.clone())
            .expect("select first");
        assert_eq!(controller.deck_session.view().loaded_slots, slots_before);
        controller
            .select_collection(second.id.clone())
            .expect("select second");
        assert_eq!(controller.deck_session.view().loaded_slots, slots_before);

        controller
            .delete_collection(&second.id)
            .expect("delete active collection");
        let after_delete = controller.deck_session.view();
        assert_eq!(after_delete.active_collection_id, ALL_CARTRIDGES_ID);
        assert_eq!(after_delete.loaded_slots, slots_before);
    }

    #[test]
    fn deck_slots_are_namespaced_and_replaced_per_runtime() {
        let mut session = DeckSessionState::default();
        let d2 = session.begin_deck(DeckKind::D2);
        session
            .publish_deck_slots(
                DeckKind::D2,
                d2,
                [("A", fake_key('a')), ("B", fake_key('b'))],
            )
            .expect("publish D2");
        let q4 = session.begin_deck(DeckKind::Q4);
        session
            .publish_deck_slots(
                DeckKind::Q4,
                q4,
                [
                    ("A", fake_key('c')),
                    ("B", fake_key('d')),
                    ("C", fake_key('e')),
                    ("D", fake_key('f')),
                ],
            )
            .expect("publish Q4");

        let view = session.view();
        assert_eq!(view.loaded_slots.len(), 6);
        assert_eq!(view.loaded_slots[0].deck_type, "d2");
        assert_eq!(view.loaded_slots[0].slot, "A");
        assert_eq!(view.loaded_slots[2].deck_type, "q4");
        assert_eq!(view.loaded_slots[2].slot, "A");

        let replacement = session.begin_deck(DeckKind::D2);
        assert_eq!(session.view().loaded_slots.len(), 4);
        session
            .publish_deck_slots(
                DeckKind::D2,
                replacement,
                [("A", fake_key('1')), ("B", fake_key('2'))],
            )
            .expect("replace D2 only");
        let view = session.view();
        assert_eq!(view.loaded_slots.len(), 6);
        assert!(view.loaded_slots.iter().any(|slot| {
            slot.deck_type == "q4" && slot.slot == "D" && slot.archive_sha256 == "f".repeat(64)
        }));
    }

    #[test]
    fn closed_or_stale_runtime_cannot_publish_or_clear_another_session() {
        let mut session = DeckSessionState::default();
        let stale = session.begin_deck(DeckKind::D2);
        assert!(session.close_deck(DeckKind::D2, stale));
        assert!(
            session
                .publish_deck_slots(
                    DeckKind::D2,
                    stale,
                    [("A", fake_key('a')), ("B", fake_key('b'))],
                )
                .is_err()
        );
        assert!(session.view().loaded_slots.is_empty());

        let current = session.begin_deck(DeckKind::D2);
        session
            .publish_deck_slots(
                DeckKind::D2,
                current,
                [("A", fake_key('c')), ("B", fake_key('d'))],
            )
            .expect("publish replacement");
        assert!(!session.close_deck(DeckKind::D2, stale));
        assert_eq!(session.view().loaded_slots.len(), 2);
        assert!(session.close_deck(DeckKind::D2, current));
        assert!(session.view().loaded_slots.is_empty());
    }

    #[test]
    fn active_collection_is_validated_at_the_controller_boundary() {
        let library = Library::in_memory().expect("in-memory library");
        let mut controller = LibraryController::new(library);
        controller
            .select_collection(CollectionId::unassigned())
            .expect("virtual Unassigned is selectable");
        let missing = CollectionId::new_unchecked("01900000-0000-7000-8000-000000000000");
        let error = controller
            .select_collection(missing)
            .expect_err("unknown collection");
        assert_eq!(error.code, "not_found");
    }
}
