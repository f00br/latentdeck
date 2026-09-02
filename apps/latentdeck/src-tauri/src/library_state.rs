use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

#[cfg(target_os = "windows")]
use std::collections::BTreeMap;

use latentdeck_cartridge::{
    limits::ValidationLimits, manifest::parse_manifest_json,
    signal::validate_codec_neutral_signal_geometry,
};
use latentdeck_core::{
    diagnostics::{LogLevel, record_global},
    signal_geometry::{
        SignalCompatibilityPolicy, SignalCompatibilityReport, SignalGeometry, SignalPresentation,
        SignalWorkload, check_signal_compatibility,
    },
};
use latentdeck_library::{
    ALL_CARTRIDGES_ID, Availability, CartridgeKey, CartridgeRecord, CollectionId, CollectionRecord,
    DeckSourceIdentity, FolderImportOptions, IndexedDeckSource, Library, LibraryError, PathState,
    QueryOptions, ReindexDisposition, ResolvedDeckSource,
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
// Four warm sessions can each own the maximum 16 exact physical sources. Only
// launch-selected identities enter this Windows LRU; Library eligibility never
// pins a cartridge file. Other platforms revalidate because an open read handle
// does not prevent same-length in-place mutation there.
#[cfg(target_os = "windows")]
const MAX_RETAINED_DECK_SOURCES: usize = 64;

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
}

impl Default for DeckSessionState {
    fn default() -> Self {
        Self {
            active_collection_id: CollectionId::all_cartridges(),
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
            loaded_slots: Vec::new(),
        }
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
    let visual = validate_codec_neutral_signal_geometry(&manifest).map_err(|error| {
        CommandError::new(
            error.code(),
            "Indexed cartridge signal geometry is invalid; reimport the cartridge.",
        )
    })?;
    Ok(SignalGeometry {
        codec_family: manifest.codec.family.0.clone(),
        profile: manifest.codec.profile.0.clone(),
        profile_version: manifest.codec.profile_version.0.clone(),
        runtime_dtype: visual.runtime_dtype,
        batch: visual.batch,
        latent_channels: visual.latent_channels,
        latent_slots: visual.latent_slots,
        latent_height: visual.latent_height,
        latent_width: visual.latent_width,
        decoded_frame_count: visual.decoded_frame_count,
        decoded_height: visual.decoded_height,
        decoded_width: visual.decoded_width,
        timing_contract: manifest.timing.contract.0.clone(),
        timing_contract_version: manifest.timing.contract_version.0.clone(),
        frame_rate: visual.frame_rate,
    })
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

    fn indexed_deck_source_manifests(
        &self,
        identities: &[DeckSourceIdentity],
    ) -> Result<Vec<Result<latentdeck_cartridge::manifest::ManifestV0_1, CommandError>>, CommandError>
    {
        Ok(self
            .library
            .indexed_deck_sources(identities)?
            .into_iter()
            .map(|indexed| {
                indexed
                    .map_err(CommandError::from)
                    .and_then(|indexed| parse_indexed_deck_source_manifest(&indexed))
            })
            .collect())
    }
}

fn parse_indexed_deck_source_manifest(
    indexed: &IndexedDeckSource,
) -> Result<latentdeck_cartridge::manifest::ManifestV0_1, CommandError> {
    let manifest = parse_manifest_json(
        indexed.manifest_json().as_bytes(),
        &ValidationLimits::default(),
    )
    .map_err(|error| {
        CommandError::new(
            error.code(),
            "Indexed cartridge metadata failed validation; reimport the cartridge.",
        )
    })?;
    if manifest.cartridge_id.0 != indexed.identity().cartridge_id() {
        return Err(CommandError::new(
            "package_invalid",
            "The exact indexed Deck source identity is inconsistent.",
        ));
    }
    Ok(manifest)
}

pub(crate) struct AppState {
    controller: Arc<Mutex<LibraryController>>,
    deck_sources: Arc<Mutex<DeckSourceCache>>,
}

#[derive(Clone)]
pub(crate) struct LibraryImporter {
    controller: Arc<Mutex<LibraryController>>,
    deck_sources: Arc<Mutex<DeckSourceCache>>,
}

#[derive(Default)]
struct DeckSourceCache {
    #[cfg(target_os = "windows")]
    entries: BTreeMap<(String, String, PathBuf), DeckSourceCacheEntry>,
    #[cfg(target_os = "windows")]
    use_sequence: u64,
    indexed_compatibility_checks: u64,
    full_validations: u64,
    cached_checkouts: u64,
}

#[cfg(target_os = "windows")]
struct DeckSourceCacheEntry {
    source: ResolvedDeckSource,
    last_used: u64,
}

impl DeckSourceCache {
    fn clear_retained(&mut self) {
        #[cfg(target_os = "windows")]
        self.entries.clear();
    }

    #[cfg(test)]
    fn retained_len(&self) -> usize {
        #[cfg(target_os = "windows")]
        {
            self.entries.len()
        }
        #[cfg(not(target_os = "windows"))]
        {
            0
        }
    }
}

#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeckSourceCacheStats {
    pub(crate) indexed_compatibility_checks: u64,
    pub(crate) full_validations: u64,
    pub(crate) cached_checkouts: u64,
    pub(crate) retained_entries: usize,
}

impl AppState {
    pub(crate) fn new(library: Library) -> Self {
        Self {
            controller: Arc::new(Mutex::new(LibraryController::new(library))),
            deck_sources: Arc::new(Mutex::new(DeckSourceCache::default())),
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
        let deck_sources = Arc::clone(&self.deck_sources);
        tauri::async_runtime::spawn_blocking(move || {
            // Keep this lock through the miss path. This is a deliberately
            // small selected-source singleflight: concurrent callers for one
            // exact LC cannot launch duplicate full-byte validation.
            let mut cache = lock_deck_sources(&deck_sources)?;
            #[cfg(target_os = "windows")]
            let cached_key = cache
                .entries
                .keys()
                .find(|key| {
                    key.0 == identity.cartridge_id() && key.1 == identity.archive_sha256().as_str()
                })
                .cloned();
            #[cfg(target_os = "windows")]
            if let Some(key) = cached_key {
                let cloned = cache
                    .entries
                    .get(&key)
                    .map(|entry| entry.source.try_clone_retained());
                match cloned {
                    Some(Ok(resolved)) => {
                        cache.use_sequence = cache.use_sequence.saturating_add(1);
                        let used = cache.use_sequence;
                        if let Some(entry) = cache.entries.get_mut(&key) {
                            entry.last_used = used;
                        }
                        cache.cached_checkouts = cache.cached_checkouts.saturating_add(1);
                        return Ok(resolved);
                    }
                    Some(Err(_)) => {
                        // A failed handle clone cannot remain reusable evidence.
                        cache.entries.remove(&key);
                    }
                    None => {}
                }
            }

            cache.full_validations = cache.full_validations.saturating_add(1);
            let resolved = lock_controller(&controller)?.resolve_deck_source(&identity)?;
            #[cfg(target_os = "windows")]
            if let Ok(retained) = resolved.try_clone_retained() {
                if cache.entries.len() >= MAX_RETAINED_DECK_SOURCES
                    && let Some(lru_key) = cache
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_used)
                        .map(|(key, _)| key.clone())
                {
                    cache.entries.remove(&lru_key);
                }
                cache.use_sequence = cache.use_sequence.saturating_add(1);
                let used = cache.use_sequence;
                cache.entries.insert(
                    deck_source_cache_key(&resolved),
                    DeckSourceCacheEntry {
                        source: retained,
                        last_used: used,
                    },
                );
            }
            Ok(resolved)
        })
        .await
        .map_err(|_| CommandError::task_stopped())?
    }

    /// Return immutable metadata imported into the Library for lightweight UI
    /// compatibility display. This never opens an LC file or retains a file
    /// handle; exact selected sources are fully validated by
    /// [`Self::resolve_deck_source`] before launch.
    pub(crate) async fn indexed_deck_source_manifests(
        &self,
        identities: Vec<DeckSourceIdentity>,
    ) -> Result<Vec<Result<latentdeck_cartridge::manifest::ManifestV0_1, CommandError>>, CommandError>
    {
        let controller = Arc::clone(&self.controller);
        let deck_sources = Arc::clone(&self.deck_sources);
        tauri::async_runtime::spawn_blocking(move || {
            let results = {
                let controller = lock_controller(&controller)?;
                controller.indexed_deck_source_manifests(&identities)?
            };
            let mut cache = lock_deck_sources(&deck_sources)?;
            cache.indexed_compatibility_checks = cache
                .indexed_compatibility_checks
                .saturating_add(u64::try_from(identities.len()).unwrap_or(u64::MAX));
            Ok(results)
        })
        .await
        .map_err(|_| CommandError::task_stopped())?
    }

    pub(crate) fn importer(&self) -> LibraryImporter {
        LibraryImporter {
            controller: Arc::clone(&self.controller),
            deck_sources: Arc::clone(&self.deck_sources),
        }
    }

    fn invalidate_deck_sources(&self) -> Result<(), CommandError> {
        lock_deck_sources(&self.deck_sources)?.clear_retained();
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn deck_source_cache_stats(&self) -> DeckSourceCacheStats {
        self.deck_sources.lock().map_or_else(
            |_| DeckSourceCacheStats {
                indexed_compatibility_checks: 0,
                full_validations: 0,
                cached_checkouts: 0,
                retained_entries: 0,
            },
            |cache| DeckSourceCacheStats {
                indexed_compatibility_checks: cache.indexed_compatibility_checks,
                full_validations: cache.full_validations,
                cached_checkouts: cache.cached_checkouts,
                retained_entries: cache.retained_len(),
            },
        )
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
        let deck_sources = Arc::clone(&self.deck_sources);
        tauri::async_runtime::spawn_blocking(move || {
            let mut controller = lock_controller(&controller)?;
            let result = controller
                .library
                .import_file(path)
                .map(|result| result.key)
                .map_err(Into::into);
            drop(controller);
            if result.is_ok() {
                lock_deck_sources(&deck_sources)?.clear_retained();
            }
            result
        })
        .await
        .map_err(|_| CommandError::task_stopped())?
    }
}

#[cfg(target_os = "windows")]
fn deck_source_cache_key(source: &ResolvedDeckSource) -> (String, String, PathBuf) {
    (
        source.identity().cartridge_id().to_owned(),
        source.identity().archive_sha256().as_str().to_owned(),
        source.path().to_path_buf(),
    )
}

fn lock_deck_sources(
    cache: &Arc<Mutex<DeckSourceCache>>,
) -> Result<MutexGuard<'_, DeckSourceCache>, CommandError> {
    cache.lock().map_err(|_| {
        CommandError::new(
            "library.source_cache_poisoned",
            "Deck source validation cache is unavailable; restart LatentDeck.",
        )
    })
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
    let deck_sources = Arc::clone(&state.deck_sources);
    tauri::async_runtime::spawn_blocking(move || {
        let result = lock_controller(&controller)?.import_files(paths);
        if result.is_ok() {
            lock_deck_sources(&deck_sources)?.clear_retained();
        }
        result
    })
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
    let deck_sources = Arc::clone(&state.deck_sources);
    tauri::async_runtime::spawn_blocking(move || {
        let result = lock_controller(&controller)?.import_folder(path, recursive);
        if result.is_ok() {
            lock_deck_sources(&deck_sources)?.clear_retained();
        }
        result
    })
    .await
    .map_err(|_| CommandError::task_stopped())?
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn library_reindex(
    state: State<'_, AppState>,
) -> Result<ReindexSummary, CommandError> {
    state.invalidate_deck_sources()?;
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
        writer::{PackRequest, WriteOptions, pack_atomic, pack_integrity_atomic},
    };
    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn exact_deck_source_resolution_reuses_one_full_validation() {
        let root = tempdir().expect("temporary library root");
        let path = write_synthetic_lc(
            root.path(),
            "cached-source.lc",
            "550e8400-e29b-41d4-a716-446655440077",
            7,
            2,
            2,
        );
        let mut library = Library::in_memory().expect("in-memory Library");
        let imported = library.import_file(&path).expect("import source");
        let identity =
            DeckSourceIdentity::new("550e8400-e29b-41d4-a716-446655440077", imported.key)
                .expect("exact identity");
        let state = AppState::new(library);

        let indexed = state
            .indexed_deck_source_manifests(vec![identity.clone()])
            .await
            .expect("indexed compatibility batch");
        assert_eq!(indexed.len(), 1);
        assert!(indexed[0].is_ok());
        assert_eq!(
            state.deck_source_cache_stats(),
            DeckSourceCacheStats {
                indexed_compatibility_checks: 1,
                full_validations: 0,
                cached_checkouts: 0,
                retained_entries: 0,
            }
        );

        let (first, second) = tokio::join!(
            state.resolve_deck_source(identity.clone()),
            state.resolve_deck_source(identity.clone()),
        );
        let first = first.expect("first concurrent exact resolve");
        let second = second.expect("second concurrent exact resolve");
        #[cfg(target_os = "windows")]
        assert_eq!(
            state.deck_source_cache_stats(),
            DeckSourceCacheStats {
                indexed_compatibility_checks: 1,
                full_validations: 1,
                cached_checkouts: 1,
                retained_entries: 1,
            }
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            state.deck_source_cache_stats(),
            DeckSourceCacheStats {
                indexed_compatibility_checks: 1,
                full_validations: 2,
                cached_checkouts: 0,
                retained_entries: 0,
            }
        );
        assert_eq!(first.path(), second.path());
        assert_eq!(
            first.validated_cartridge().receipt(),
            second.validated_cartridge().receipt()
        );
        drop(first);
        drop(second);
        state
            .invalidate_deck_sources()
            .expect("library mutation invalidates retained source cache");
        let _revalidated = state
            .resolve_deck_source(identity)
            .await
            .expect("resolve again after invalidation");
        #[cfg(target_os = "windows")]
        assert_eq!(state.deck_source_cache_stats().full_validations, 2);
        #[cfg(not(target_os = "windows"))]
        assert_eq!(state.deck_source_cache_stats().full_validations, 3);
    }

    #[tokio::test]
    async fn q4_selected_sources_are_the_only_retained_library_entries() {
        let root = tempdir().expect("temporary Q4 Library root");
        let ids = [
            "550e8400-e29b-41d4-a716-446655440081",
            "550e8400-e29b-41d4-a716-446655440082",
            "550e8400-e29b-41d4-a716-446655440083",
            "550e8400-e29b-41d4-a716-446655440084",
        ];
        let mut library = Library::in_memory().expect("in-memory Library");
        let identities = ids
            .iter()
            .enumerate()
            .map(|(index, cartridge_id)| {
                let path = write_synthetic_lc(
                    root.path(),
                    &format!("q4-source-{index}.lc"),
                    cartridge_id,
                    7,
                    2,
                    2,
                );
                let imported = library.import_file(path).expect("import Q4 source");
                DeckSourceIdentity::new(*cartridge_id, imported.key).expect("exact Q4 identity")
            })
            .collect::<Vec<_>>();
        let state = AppState::new(library);

        let indexed = state
            .indexed_deck_source_manifests(identities.clone())
            .await
            .expect("metadata-only Q4 eligibility batch");
        assert_eq!(indexed.len(), 4);
        assert!(indexed.iter().all(Result::is_ok));
        assert_eq!(
            state.deck_source_cache_stats(),
            DeckSourceCacheStats {
                indexed_compatibility_checks: 4,
                full_validations: 0,
                cached_checkouts: 0,
                retained_entries: 0,
            }
        );

        let mut selected = Vec::new();
        for identity in &identities {
            selected.push(
                state
                    .resolve_deck_source(identity.clone())
                    .await
                    .expect("full-validate one exact selected Q4 source"),
            );
        }
        let mut reopened = Vec::new();
        for identity in identities {
            reopened.push(
                state
                    .resolve_deck_source(identity)
                    .await
                    .expect("repeat exact Q4 source checkout"),
            );
        }

        #[cfg(target_os = "windows")]
        assert_eq!(
            state.deck_source_cache_stats(),
            DeckSourceCacheStats {
                indexed_compatibility_checks: 4,
                full_validations: 4,
                cached_checkouts: 4,
                retained_entries: 4,
            }
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            state.deck_source_cache_stats(),
            DeckSourceCacheStats {
                indexed_compatibility_checks: 4,
                full_validations: 8,
                cached_checkouts: 0,
                retained_entries: 0,
            }
        );
        drop(selected);
        drop(reopened);
    }

    #[tokio::test]
    async fn indexed_eligibility_does_not_hide_strict_open_revalidation() {
        let root = tempdir().expect("temporary stale-source root");
        let cartridge_id = "550e8400-e29b-41d4-a716-446655440085";
        let path = write_synthetic_lc(root.path(), "stale-after-index.lc", cartridge_id, 7, 2, 2);
        let mut library = Library::in_memory().expect("in-memory Library");
        let imported = library.import_file(&path).expect("import exact source");
        let identity = DeckSourceIdentity::new(cartridge_id, imported.key).expect("exact identity");
        let state = AppState::new(library);
        fs::remove_file(path).expect("remove source after indexing");

        let indexed = state
            .indexed_deck_source_manifests(vec![identity.clone()])
            .await
            .expect("indexed eligibility remains metadata-only");
        assert!(indexed[0].is_ok());
        assert_eq!(state.deck_source_cache_stats().full_validations, 0);
        assert_eq!(state.deck_source_cache_stats().retained_entries, 0);

        state
            .resolve_deck_source(identity)
            .await
            .expect_err("exact Open must reject the stale current bytes");
        assert_eq!(state.deck_source_cache_stats().full_validations, 1);
        assert_eq!(state.deck_source_cache_stats().retained_entries, 0);
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

    fn write_synthetic_non_h3_lc(root: &Path, name: &str, cartridge_id: &str) -> PathBuf {
        write_synthetic_non_h3_lc_with_duration(root, name, cartridge_id, 1, 1)
    }

    fn write_synthetic_non_h3_lc_with_duration(
        root: &Path,
        name: &str,
        cartridge_id: &str,
        duration_numerator: u64,
        duration_denominator: u64,
    ) -> PathBuf {
        let tensor_bytes = vec![0_u8; 7 * 3 * 4];
        let mut header = format!(
            r#"{{"latent_state":{{"data_offsets":[0,{}],"dtype":"F32","shape":[1,7,1,3,1]}}}}"#,
            tensor_bytes.len()
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
        let manifest = parse_manifest_json(
            &serde_json::to_vec(&json!({
                "spec_version": "0.1.0",
                "cartridge_id": cartridge_id,
                "codec": {
                    "family": "synthetic_test",
                    "profile": "non_h3_latent",
                    "profile_version": "0.2.0"
                },
                "payloads": [{
                    "path": "payloads/synthetic.safetensors",
                    "media_type": "application/vnd.safetensors",
                    "byte_length": payload_hash.byte_length,
                    "sha256": payload_hash.sha256.to_string()
                }],
                "tensors": [{
                    "stream": "visual",
                    "name": "latent_state",
                    "payload": "payloads/synthetic.safetensors",
                    "storage_dtype": "F32",
                    "runtime_dtype": "F32",
                    "shape": [1, 7, 1, 3, 1]
                }],
                "timing": {
                    "contract": "synthetic_step",
                    "contract_version": "0.2.0",
                    "decoded_video": {
                        "width": 3,
                        "height": 1,
                        "frame_count": 1,
                        "frame_rate": {"numerator": 1, "denominator": 1},
                        "duration": {
                            "numerator": duration_numerator,
                            "denominator": duration_denominator
                        }
                    }
                },
                "audio": {"policy": "source_absent"},
                "provenance": {
                    "created_by": {"name": "latentdeck-app-tests", "version": "0.2.0"},
                    "sources": []
                },
                "parent_cartridges": [],
                "operation_history": []
            }))
            .expect("manifest JSON"),
            &ValidationLimits::default(),
        )
        .expect("synthetic non-H3 manifest");
        let payload_path = root.join(format!("{name}.safetensors"));
        let output_path = root.join(format!("{name}.lc"));
        fs::write(&payload_path, payload).expect("synthetic payload");
        pack_integrity_atomic(
            &PackRequest::new(manifest, &payload_path),
            &output_path,
            &WriteOptions::default(),
        )
        .expect("synthetic non-H3 LC");
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
    fn changing_or_deleting_active_bank_keeps_runtime_slots_owned_by_generic_controller() {
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
    fn library_snapshot_renders_h3_and_codec_neutral_signal_geometry() {
        let temporary = tempdir().expect("temporary directory");
        let h3 = write_synthetic_lc(
            temporary.path(),
            "h3-source",
            "550e8400-e29b-41d4-a716-446655440020",
            2,
            2,
            1,
        );
        let synthetic = write_synthetic_non_h3_lc(
            temporary.path(),
            "synthetic-source",
            "550e8400-e29b-41d4-a716-446655440021",
        );
        let mut library = Library::in_memory().expect("in-memory library");
        library.import_file(h3).expect("H3 import");
        library
            .import_file(synthetic)
            .expect("codec-neutral import");
        let mut controller = LibraryController::new(library);

        let snapshot = controller.snapshot(None).expect("generic Library snapshot");

        assert_eq!(snapshot.cartridges.len(), 2);
        let h3 = snapshot
            .cartridges
            .iter()
            .find(|item| item.codec_family == "minimax_h3")
            .expect("H3 row");
        assert_eq!(
            h3.signal_geometry.runtime_dtype,
            latentdeck_cartridge::manifest::DType::F16
        );
        assert_eq!(h3.signal_geometry.latent_channels, 24);
        assert_eq!(h3.signal_geometry.latent_slots, 2);
        assert_eq!(h3.signal_geometry.decoded_frame_count, 5);
        let synthetic = snapshot
            .cartridges
            .iter()
            .find(|item| item.codec_family == "synthetic_test")
            .expect("synthetic row");
        assert_eq!(
            synthetic.signal_geometry.runtime_dtype,
            latentdeck_cartridge::manifest::DType::F32
        );
        assert_eq!(synthetic.signal_geometry.latent_channels, 7);
        assert_eq!(synthetic.signal_geometry.latent_height, 3);
        assert_eq!(synthetic.signal_geometry.latent_width, 1);
        assert_eq!(synthetic.signal_presentation.decoded_width, 3);
        assert_eq!(synthetic.signal_presentation.decoded_height, 1);
    }

    #[test]
    fn malformed_codec_neutral_import_cannot_poison_library_snapshot() {
        let temporary = tempdir().expect("temporary directory");
        let malformed = write_synthetic_non_h3_lc_with_duration(
            temporary.path(),
            "malformed-source",
            "550e8400-e29b-41d4-a716-446655440022",
            2,
            1,
        );
        let mut library = Library::in_memory().expect("in-memory library");

        let error = library
            .import_file(malformed)
            .expect_err("invalid generic signal must not enter the Library");
        assert_eq!(error.cartridge_code.as_deref(), Some("timing_mismatch"));
        let mut controller = LibraryController::new(library);
        let snapshot = controller
            .snapshot(None)
            .expect("rejected entry cannot poison snapshot");
        assert!(snapshot.cartridges.is_empty());
        assert_eq!(snapshot.total_indexed, 0);
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
