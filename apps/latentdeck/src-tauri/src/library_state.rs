use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use latentdeck_cartridge::{limits::ValidationLimits, manifest::parse_manifest_json, profile::h3};
use latentdeck_core::{
    diagnostics::{LogLevel, record_global},
    signal_geometry::{
        SignalCompatibilityPolicy, SignalCompatibilityReport, SignalGeometry, SignalPresentation,
        SignalWorkload, check_signal_compatibility,
    },
};
use latentdeck_library::{
    ALL_CARTRIDGES_ID, Availability, CartridgeKey, CartridgeRecord, CollectionId, CollectionRecord,
    DeckSourceIdentity, FolderImportOptions, Library, LibraryError, PathState, QueryOptions,
    ReindexDisposition, ResolvedDeckSource,
};
use serde::{Deserialize, Serialize};
use tauri::State;

const UI_QUERY_LIMIT: usize = 1_000;
const RECENT_LIMIT: usize = 8;
const MAX_EXPLICIT_FILES: usize = 1_024;
const MAX_PRESET_SOURCE_IDENTITIES: usize = 4;
// A full Bank can expose `UI_QUERY_LIMIT` rows, while a loaded D2/Q4 preset may
// retain up to four exact sources that are outside that Bank. The compatibility
// preflight must accept the same closed set the faceplate can render.
const MAX_COMPATIBILITY_CANDIDATES: usize = UI_QUERY_LIMIT + MAX_PRESET_SOURCE_IDENTITIES;

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
    codec_profile_version: String,
    timing_contract: String,
    timing_contract_version: String,
    frame_rate_numerator: u64,
    frame_rate_denominator: u64,
    decoded_width: u32,
    decoded_height: u32,
    decoded_frame_count: u64,
    duration_numerator: u64,
    duration_denominator: u64,
    signal_geometry: SignalGeometry,
    signal_presentation: SignalPresentation,
    signal_workload: SignalWorkload,
    favorite: bool,
    tags: Vec<String>,
    availability: Availability,
    paths: Vec<CartridgePathView>,
}

impl TryFrom<CartridgeRecord> for CartridgeView {
    type Error = CommandError;

    fn try_from(record: CartridgeRecord) -> Result<Self, Self::Error> {
        let metadata = record.metadata;
        let signal_geometry = signal_geometry_from_manifest(&metadata.manifest_json)?;
        let signal_presentation = signal_geometry.presentation();
        let signal_workload = signal_geometry.workload();
        Ok(Self {
            archive_sha256: record.key.as_str().to_owned(),
            cartridge_id: metadata.cartridge_id,
            codec_family: metadata.codec_family,
            codec_profile: metadata.codec_profile,
            codec_profile_version: metadata.codec_profile_version,
            timing_contract: metadata.timing_contract,
            timing_contract_version: metadata.timing_contract_version,
            frame_rate_numerator: metadata.frame_rate_numerator,
            frame_rate_denominator: metadata.frame_rate_denominator,
            decoded_width: metadata.decoded_width,
            decoded_height: metadata.decoded_height,
            decoded_frame_count: metadata.decoded_frame_count,
            duration_numerator: metadata.duration_numerator,
            duration_denominator: metadata.duration_denominator,
            signal_geometry,
            signal_presentation,
            signal_workload,
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
        })
    }
}

fn signal_geometry_from_manifest(manifest_json: &str) -> Result<SignalGeometry, CommandError> {
    let limits = ValidationLimits::default();
    let manifest = parse_manifest_json(manifest_json.as_bytes(), &limits).map_err(|error| {
        CommandError::new(
            error.code(),
            "Indexed cartridge metadata failed validation; reimport the cartridge.",
        )
    })?;
    let profile = h3::validate(&manifest, &limits).map_err(|error| {
        CommandError::new(
            error.code(),
            "Indexed cartridge profile failed validation; reimport the cartridge.",
        )
    })?;
    Ok(SignalGeometry::from_h3(&profile))
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

#[derive(Debug, Deserialize)]
pub(crate) struct PresetCartridgeIdentityInput {
    cartridge_id: String,
    archive_sha256: String,
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
            cartridges: cartridges
                .into_iter()
                .map(CartridgeView::try_from)
                .collect::<Result<Vec<_>, _>>()?,
            recent: recent
                .into_iter()
                .map(CartridgeView::try_from)
                .collect::<Result<Vec<_>, _>>()?,
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

    fn activate_collection_snapshot(
        &mut self,
        collection_id: CollectionId,
        search: Option<String>,
    ) -> Result<LibraryView, CommandError> {
        let previous = self.deck_session.active_collection_id.clone();
        self.select_collection(collection_id)?;
        match self.snapshot(search) {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                self.deck_session.select_collection(previous);
                Err(error)
            }
        }
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

    fn resolve_preset_sources(
        &self,
        identities: &[PresetCartridgeIdentityInput],
    ) -> Result<Vec<Option<CartridgeView>>, CommandError> {
        if identities.len() > MAX_PRESET_SOURCE_IDENTITIES {
            return Err(CommandError::new(
                "invalid_input",
                "A Deck preset source lookup accepts at most four identities.",
            ));
        }
        identities
            .iter()
            .map(|requested| {
                let identity = DeckSourceIdentity::new(
                    requested.cartridge_id.clone(),
                    cartridge_key(requested.archive_sha256.clone()),
                )?;
                let record = self.library.get_cartridge(identity.archive_sha256())?;
                match record {
                    Some(record) if record.metadata.cartridge_id == requested.cartridge_id => {
                        CartridgeView::try_from(record).map(Some)
                    }
                    Some(_) | None => Ok(None),
                }
            })
            .collect()
    }

    fn signal_compatibility(
        &self,
        reference_archive_sha256: String,
        candidate_archive_sha256s: Vec<String>,
        policy: SignalCompatibilityPolicy,
    ) -> Result<SignalCompatibilityReport, CommandError> {
        if candidate_archive_sha256s.len() > MAX_COMPATIBILITY_CANDIDATES {
            return Err(CommandError::new(
                "invalid_input",
                "A signal compatibility query exceeds the bounded Bank plus preset-source set.",
            ));
        }
        let reference = self.signal_geometry(&cartridge_key(reference_archive_sha256))?;
        let candidates = candidate_archive_sha256s
            .into_iter()
            .map(|archive_sha256| self.signal_geometry(&cartridge_key(archive_sha256)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(check_signal_compatibility(policy, &reference, &candidates))
    }

    fn signal_geometry(&self, key: &CartridgeKey) -> Result<SignalGeometry, CommandError> {
        let record = self.library.get_cartridge(key)?.ok_or_else(|| {
            CommandError::new(
                "not_found",
                "The requested cartridge is not present in the Library index.",
            )
        })?;
        signal_geometry_from_manifest(&record.metadata.manifest_json)
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
pub(crate) fn library_resolve_preset_sources(
    state: State<'_, AppState>,
    identities: Vec<PresetCartridgeIdentityInput>,
) -> Result<Vec<Option<CartridgeView>>, CommandError> {
    lock_controller(&state.controller)?.resolve_preset_sources(&identities)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn library_signal_compatibility(
    state: State<'_, AppState>,
    reference_archive_sha256: String,
    candidate_archive_sha256s: Vec<String>,
    policy: SignalCompatibilityPolicy,
) -> Result<SignalCompatibilityReport, CommandError> {
    lock_controller(&state.controller)?.signal_compatibility(
        reference_archive_sha256,
        candidate_archive_sha256s,
        policy,
    )
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
pub(crate) fn library_activate_collection_snapshot(
    state: State<'_, AppState>,
    collection_id: String,
    search: Option<String>,
) -> Result<LibraryView, CommandError> {
    lock_controller(&state.controller)?
        .activate_collection_snapshot(to_collection_id(collection_id), search)
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
    use std::{fs, io::Cursor};

    use latentdeck_cartridge::{
        hash::hash_reader,
        writer::{PackRequest, WriteOptions, pack_atomic},
    };
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn fake_key(byte: char) -> CartridgeKey {
        CartridgeKey::new_unchecked(std::iter::repeat_n(byte, 64).collect::<String>())
    }

    fn write_synthetic_lc(
        root: &Path,
        name: &str,
        cartridge_id: &str,
        latent_slots: u64,
        latent_height: u64,
        latent_width: u64,
    ) -> PathBuf {
        let element_count = 24_u64
            .checked_mul(latent_slots)
            .and_then(|value| value.checked_mul(latent_height))
            .and_then(|value| value.checked_mul(latent_width))
            .expect("small synthetic tensor");
        let tensor_bytes = vec![0_u8; usize::try_from(element_count * 2).expect("small payload")];
        let mut header = format!(
            concat!(
                r#"{{"video":{{"data_offsets":[0,{}],"dtype":"F16","#,
                r#""shape":[1,24,{},{},{}]}}}}"#
            ),
            tensor_bytes.len(),
            latent_slots,
            latent_height,
            latent_width,
        )
        .into_bytes();
        while !header.len().is_multiple_of(8) {
            header.push(b' ');
        }
        let mut payload = Vec::with_capacity(8 + header.len() + tensor_bytes.len());
        payload.extend_from_slice(
            &u64::try_from(header.len())
                .expect("small header")
                .to_le_bytes(),
        );
        payload.extend_from_slice(&header);
        payload.extend_from_slice(&tensor_bytes);
        let payload_hash = hash_reader(&mut Cursor::new(&payload)).expect("payload hash");
        let decoded_frame_count = ((latent_slots - 2) / 5) * 17 + 5;
        let duration_divisor = greatest_common_divisor(decoded_frame_count, 24);
        let duration_numerator = decoded_frame_count / duration_divisor;
        let duration_denominator = 24 / duration_divisor;
        let manifest = parse_manifest_json(
            &serde_json::to_vec(&json!({
                "spec_version": "0.1.0",
                "cartridge_id": cartridge_id,
                "codec": {
                    "family": "minimax_h3",
                    "profile": "h3_av_latent",
                    "profile_version": "0.1.0"
                },
                "payloads": [{
                    "path": "payloads/h3.safetensors",
                    "media_type": "application/vnd.safetensors",
                    "byte_length": payload_hash.byte_length,
                    "sha256": payload_hash.sha256.to_string()
                }],
                "tensors": [{
                    "stream": "visual",
                    "name": "video",
                    "payload": "payloads/h3.safetensors",
                    "storage_dtype": "F16",
                    "runtime_dtype": "F16",
                    "shape": [1, 24, latent_slots, latent_height, latent_width]
                }],
                "timing": {
                    "contract": "minimax_h3_causal",
                    "contract_version": "0.1.0",
                    "decoded_video": {
                        "width": latent_width * 16,
                        "height": latent_height * 16,
                        "frame_count": decoded_frame_count,
                        "frame_rate": {"numerator": 24, "denominator": 1},
                        "duration": {
                            "numerator": duration_numerator,
                            "denominator": duration_denominator
                        }
                    }
                },
                "audio": {"policy": "source_absent"},
                "provenance": {
                    "created_by": {"name": "latentdeck-app-tests", "version": "0.1.0"},
                    "sources": []
                },
                "parent_cartridges": [],
                "operation_history": []
            }))
            .expect("manifest JSON"),
            &ValidationLimits::default(),
        )
        .expect("synthetic manifest");
        let payload_path = root.join(format!("{name}.safetensors"));
        let output_path = root.join(format!("{name}.lc"));
        fs::write(&payload_path, payload).expect("synthetic payload");
        pack_atomic(
            &PackRequest::new(manifest, &payload_path),
            &output_path,
            &WriteOptions::default(),
        )
        .expect("synthetic LC");
        output_path
    }

    const fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
        while right != 0 {
            let remainder = left % right;
            left = right;
            right = remainder;
        }
        if left == 0 { 1 } else { left }
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

    #[test]
    fn preset_collection_activation_and_snapshot_are_one_recoverable_transition() {
        let library = Library::in_memory().expect("in-memory library");
        let mut controller = LibraryController::new(library);
        let target = controller
            .library
            .create_collection("Preset Bank")
            .expect("target collection");

        let snapshot = controller
            .activate_collection_snapshot(target.id.clone(), None)
            .expect("atomic activation snapshot");
        assert_eq!(
            snapshot.deck_session.active_collection_id,
            target.id.as_str()
        );

        let error = controller
            .activate_collection_snapshot(CollectionId::new_unchecked("missing"), None)
            .expect_err("missing target must not partially change active collection");
        assert_eq!(error.code, "not_found");
        assert_eq!(controller.deck_session.active_collection_id, target.id);
    }

    #[test]
    fn preset_source_lookup_is_not_scoped_to_the_active_collection_query() {
        let temporary = tempdir().expect("temporary directory");
        let path = write_synthetic_lc(
            temporary.path(),
            "global-source",
            "550e8400-e29b-41d4-a716-446655440000",
            2,
            2,
            1,
        );
        let mut library = Library::in_memory().expect("in-memory library");
        let imported = library.import_file(path).expect("import source");
        let mut controller = LibraryController::new(library);
        let empty_bank = controller
            .library
            .create_collection("Empty preset Bank")
            .expect("empty Bank");
        controller
            .select_collection(empty_bank.id.clone())
            .expect("select empty Bank");
        let requested = PresetCartridgeIdentityInput {
            cartridge_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
            archive_sha256: imported.key.as_str().to_owned(),
        };

        let resolved = controller
            .resolve_preset_sources(&[requested])
            .expect("global exact lookup");

        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].as_ref().map(|view| view.cartridge_id.as_str()),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(
            resolved[0]
                .as_ref()
                .map(|view| view.signal_presentation.aspect_ratio),
            Some(latentdeck_core::signal_geometry::AspectRatio {
                width: 1,
                height: 2
            })
        );
        assert_eq!(controller.deck_session.active_collection_id, empty_bank.id);

        let mismatch = controller
            .resolve_preset_sources(&[PresetCartridgeIdentityInput {
                cartridge_id: "550e8400-e29b-41d4-a716-446655440099".to_owned(),
                archive_sha256: imported.key.as_str().to_owned(),
            }])
            .expect("identity mismatch is a missing exact source");
        assert!(mismatch[0].is_none());
    }

    #[test]
    fn compatibility_report_comes_from_validated_core_geometry() {
        let temporary = tempdir().expect("temporary directory");
        let portrait = write_synthetic_lc(
            temporary.path(),
            "portrait",
            "550e8400-e29b-41d4-a716-446655440010",
            2,
            2,
            1,
        );
        let landscape = write_synthetic_lc(
            temporary.path(),
            "landscape",
            "550e8400-e29b-41d4-a716-446655440011",
            7,
            1,
            2,
        );
        let mut library = Library::in_memory().expect("in-memory library");
        let portrait = library.import_file(portrait).expect("portrait").key;
        let landscape = library.import_file(landscape).expect("landscape").key;
        let controller = LibraryController::new(library);

        let report = controller
            .signal_compatibility(
                portrait.as_str().to_owned(),
                vec![portrait.as_str().to_owned(), landscape.as_str().to_owned()],
                SignalCompatibilityPolicy::SpatialSynthesis,
            )
            .expect("Core compatibility report");

        assert!(!report.compatible);
        assert!(
            report
                .mismatches
                .iter()
                .all(|item| item.candidate_index == 1)
        );
        assert_eq!(
            report
                .mismatches
                .iter()
                .map(|item| item.code)
                .collect::<Vec<_>>(),
            vec![
                latentdeck_core::signal_geometry::SignalGeometryMismatchCode::LatentHeight,
                latentdeck_core::signal_geometry::SignalGeometryMismatchCode::LatentWidth,
                latentdeck_core::signal_geometry::SignalGeometryMismatchCode::DecodedHeight,
                latentdeck_core::signal_geometry::SignalGeometryMismatchCode::DecodedWidth,
            ]
        );
    }

    #[test]
    fn preset_source_lookup_rejects_more_than_one_deck_can_address() {
        let controller = LibraryController::new(Library::in_memory().expect("in-memory library"));
        let requested = (0..5)
            .map(|_| PresetCartridgeIdentityInput {
                cartridge_id: "550e8400-e29b-41d4-a716-446655440000".to_owned(),
                archive_sha256: "a".repeat(64),
            })
            .collect::<Vec<_>>();

        let error = controller
            .resolve_preset_sources(&requested)
            .expect_err("five preset slots exceed the D2/Q4 boundary");

        assert_eq!(error.code, "invalid_input");
    }

    #[test]
    fn compatibility_bound_includes_a_full_bank_and_four_global_preset_sources() {
        let controller = LibraryController::new(Library::in_memory().expect("in-memory library"));
        let allowed = vec!["a".repeat(64); MAX_COMPATIBILITY_CANDIDATES];
        let allowed_error = controller
            .signal_compatibility(
                "b".repeat(64),
                allowed,
                SignalCompatibilityPolicy::SpatialSynthesis,
            )
            .expect_err("empty test Library has no reference source");
        let candidates = vec!["a".repeat(64); MAX_COMPATIBILITY_CANDIDATES + 1];

        let error = controller
            .signal_compatibility(
                "b".repeat(64),
                candidates,
                SignalCompatibilityPolicy::SpatialSynthesis,
            )
            .expect_err("candidate set above the rendered maximum must be rejected first");

        assert_eq!(MAX_COMPATIBILITY_CANDIDATES, 1_004);
        assert_eq!(allowed_error.code, "not_found");
        assert_eq!(error.code, "invalid_input");
    }
}
