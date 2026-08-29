use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use latentdeck_library::{
    ALL_CARTRIDGES_ID, Availability, CartridgeKey, CartridgeRecord, CollectionId, CollectionRecord,
    FolderImportOptions, Library, LibraryError, PathState, QueryOptions, ReindexDisposition,
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
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
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
    loaded_slots: BTreeMap<String, CartridgeKey>,
}

impl Default for DeckSessionState {
    fn default() -> Self {
        Self {
            active_collection_id: CollectionId::all_cartridges(),
            loaded_slots: BTreeMap::new(),
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
                .map(|(slot, key)| SlotAssignmentView {
                    slot: slot.clone(),
                    archive_sha256: key.as_str().to_owned(),
                })
                .collect(),
        }
    }

    #[cfg(test)]
    fn assign_slot(&mut self, slot: &str, key: CartridgeKey) {
        self.loaded_slots.insert(slot.to_owned(), key);
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
}

pub(crate) struct AppState {
    controller: Arc<Mutex<LibraryController>>,
}

impl AppState {
    pub(crate) fn new(library: Library) -> Self {
        Self {
            controller: Arc::new(Mutex::new(LibraryController::new(library))),
        }
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
        controller.deck_session.assign_slot("A", fake_key('a'));
        controller.deck_session.assign_slot("B", fake_key('b'));
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
