//! Generic installed Deck/Codec Protocol 2 session coordination.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use latentdeck_cartridge::hash::hash_path;
use latentdeck_control::{
    DeckPresetDocument,
    v2::{
        CaptureMode, ControlBinding, DeviceKind, ProfileKey, RoleBinding, SourceTransportBinding,
    },
};
use latentdeck_core::{
    deck_selection_v2::{
        DeckPackageSelectionV2, DeckSelectionV2Error, DeckSourceSelectionV2,
        PreparedDeckSelectionV2, prepare_exact_deck_selection,
    },
    deck_session_v2::DeckSessionV2LoadRequest,
};
use latentdeck_deck_runtime_contracts::{
    BrokerError, ContractId, ForegroundLease, MAX_WARM_SESSIONS, OutputPinKind, OutputPinToken,
    PackageIdentity, SessionBroker, SessionId, WarmSession, WorkerId,
};
use latentdeck_extension_manager::{
    CompatibilityReason, ExtensionRoots, PackageKind, PackageManifest, PackageReference,
    compatibility_matrix, resolve_active,
};
use latentdeck_library::{CartridgeKey, DeckSourceIdentity, ResolvedDeckSource};
use latentdeck_native_output::{HostFullscreenController, NativeSpoutStatus};
use latentdeck_output_mp4::{RecorderState, RecorderStatus};
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager as _, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt as _;
use uuid::Uuid;

use crate::{
    decoded_recording::normalize_mp4_destination,
    embedded_viewport::{
        EmbeddedViewportStore, ViewportBoundsRequest, ViewportSessionAck, validate_viewport_bounds,
        viewport_error,
    },
    extension_commands::ExtensionManagerState,
    generic_deck_runtime::{
        GenericCaptureView, GenericDeckRuntime, GenericDeckRuntimeDiagnostics,
        GenericDeckRuntimeError, GenericDeckRuntimeView,
    },
    library_state::{AppState, CommandError},
    preset_state::{PresetSaveView, deck_preset_load, deck_preset_save},
};

const MAX_RECENT_FAULTS: usize = 32;
const MAX_JS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_SOURCE_OPTIONS: usize = 256;

#[derive(Debug, Default)]
struct GenericSessionRegistry {
    broker: SessionBroker,
    pending: BTreeSet<SessionId>,
}

impl GenericSessionRegistry {
    fn reserve(&mut self, session_id: SessionId) -> Result<(), BrokerError> {
        if self.broker.contains_session(&session_id) || self.pending.contains(&session_id) {
            return Err(BrokerError::SessionAlreadyExists);
        }
        if self.broker.len().saturating_add(self.pending.len()) >= MAX_WARM_SESSIONS {
            return Err(BrokerError::SessionCapacityExceeded);
        }
        self.pending.insert(session_id);
        Ok(())
    }

    fn cancel_reservation(&mut self, session_id: &SessionId) {
        self.pending.remove(session_id);
    }

    fn commit(&mut self, session: WarmSession) -> Result<(), BrokerError> {
        if !self.pending.remove(&session.session_id) {
            return Err(BrokerError::SessionNotFound);
        }
        if let Err(error) = self.broker.open_session(session.clone()) {
            self.pending.insert(session.session_id);
            return Err(error);
        }
        Ok(())
    }

    fn close(&mut self, session_id: &SessionId) -> Result<WarmSession, BrokerError> {
        self.broker.close_session(session_id)
    }

    #[cfg(test)]
    fn switch_foreground(
        &mut self,
        session_id: &SessionId,
    ) -> Result<ForegroundLease, BrokerError> {
        self.broker.switch_foreground(session_id)
    }

    fn pin_foreground(
        &mut self,
        session_id: &SessionId,
        kind: OutputPinKind,
    ) -> Result<OutputPinToken, BrokerError> {
        self.broker.pin_foreground(session_id, kind)
    }

    fn release_output_pin(&mut self, token: &OutputPinToken) -> Result<(), BrokerError> {
        self.broker.release_output_pin(token)
    }

    fn worker_fault(&mut self, worker_id: &WorkerId) -> Result<WarmSession, BrokerError> {
        self.broker.handle_worker_fault(worker_id)
    }

    fn reap_terminal_output_pin(
        &mut self,
        is_terminal: impl FnOnce(&OutputPinToken) -> bool,
    ) -> bool {
        let Some(token) = self.broker.output_pin().cloned() else {
            return false;
        };
        if !is_terminal(&token) {
            return false;
        }
        self.broker.release_output_pin(&token).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ExternalAssetKey {
    codec_id: String,
    codec_version: String,
    asset_id: String,
}

#[derive(Clone, Debug)]
struct BoundExternalAsset {
    path: PathBuf,
    sha256: String,
    byte_length: u64,
}

struct GenericSessionRecord {
    runtime: Arc<GenericDeckRuntime>,
    worker_id: WorkerId,
    deck: PackageIdentity,
    codec: PackageIdentity,
    negotiated: GenericNegotiatedIdentity,
    sources: Vec<GenericDeckSourceView>,
}

struct ForegroundTransition {
    generation: u64,
    candidate: SessionBroker,
    previous_runtime: Option<Arc<GenericDeckRuntime>>,
    target_runtime: Option<Arc<GenericDeckRuntime>>,
}

enum PreparedForegroundTransition {
    Unchanged(GenericDeckSessionsView),
    Pending(ForegroundTransition),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericDeckFaultView {
    session_id: String,
    worker_id: String,
    code: String,
}

#[derive(Default)]
struct GenericDeckController {
    registry: GenericSessionRegistry,
    sessions: BTreeMap<SessionId, GenericSessionRecord>,
    external_assets: BTreeMap<ExternalAssetKey, BoundExternalAsset>,
    recent_faults: VecDeque<GenericDeckFaultView>,
    closing: BTreeSet<SessionId>,
    lifecycle_transition: Option<u64>,
    next_lifecycle_generation: u64,
}

impl GenericDeckController {
    fn reserve(&mut self, session_id: SessionId) -> Result<(), CommandError> {
        self.reap_closed();
        self.ensure_lifecycle_idle()?;
        if self
            .registry
            .broker
            .len()
            .saturating_add(self.registry.pending.len())
            .saturating_add(self.closing.len())
            >= MAX_WARM_SESSIONS
        {
            return Err(broker_command_error(BrokerError::SessionCapacityExceeded));
        }
        self.registry
            .reserve(session_id)
            .map_err(broker_command_error)
    }

    fn cancel_reservation(&mut self, session_id: &SessionId) {
        self.registry.cancel_reservation(session_id);
    }

    fn asset_paths(&self, codec_id: &str, codec_version: &str) -> Vec<(String, PathBuf)> {
        self.external_assets
            .iter()
            .filter(|(key, _)| key.codec_id == codec_id && key.codec_version == codec_version)
            .map(|(key, asset)| (key.asset_id.clone(), asset.path.clone()))
            .collect()
    }

    fn bind_asset(&mut self, key: ExternalAssetKey, asset: BoundExternalAsset) {
        self.external_assets.insert(key, asset);
    }

    fn clear_asset(&mut self, key: &ExternalAssetKey) -> bool {
        self.external_assets.remove(key).is_some()
    }

    fn asset_view(&self, key: &ExternalAssetKey) -> GenericExternalAssetView {
        self.external_assets.get(key).map_or_else(
            || GenericExternalAssetView {
                codec_id: key.codec_id.clone(),
                codec_version: key.codec_version.clone(),
                asset_id: key.asset_id.clone(),
                bound: false,
                sha256: None,
                byte_length: None,
            },
            |asset| GenericExternalAssetView {
                codec_id: key.codec_id.clone(),
                codec_version: key.codec_version.clone(),
                asset_id: key.asset_id.clone(),
                bound: true,
                sha256: Some(asset.sha256.clone()),
                byte_length: Some(asset.byte_length),
            },
        )
    }

    fn commit(
        &mut self,
        session_id: &SessionId,
        record: GenericSessionRecord,
    ) -> Result<(), CommandError> {
        self.ensure_lifecycle_idle()?;
        let warm = WarmSession {
            session_id: session_id.clone(),
            worker_id: record.worker_id.clone(),
            deck: record.deck.clone(),
            codec: record.codec.clone(),
        };
        self.registry.commit(warm).map_err(broker_command_error)?;
        if self.sessions.insert(session_id.clone(), record).is_some() {
            let _ = self.registry.close(session_id);
            return Err(CommandError::new(
                "session.already_exists",
                "The exact generic Deck session identity is already active.",
            ));
        }
        Ok(())
    }

    fn ensure_lifecycle_idle(&self) -> Result<(), CommandError> {
        if self.lifecycle_transition.is_some() {
            return Err(CommandError::new(
                "session.lifecycle_busy",
                "A foreground output lease transition is already in progress.",
            ));
        }
        Ok(())
    }

    fn begin_lifecycle_transition(&mut self) -> Result<u64, CommandError> {
        self.ensure_lifecycle_idle()?;
        self.next_lifecycle_generation =
            self.next_lifecycle_generation
                .checked_add(1)
                .ok_or_else(|| {
                    CommandError::new(
                        "session.lease_generation_exhausted",
                        "The foreground output lease generation is exhausted.",
                    )
                })?;
        self.lifecycle_transition = Some(self.next_lifecycle_generation);
        Ok(self.next_lifecycle_generation)
    }

    fn abort_lifecycle_transition(&mut self, generation: u64) {
        if self.lifecycle_transition == Some(generation) {
            self.lifecycle_transition = None;
        }
    }

    fn runtime(&mut self, session_id: &SessionId) -> Result<Arc<GenericDeckRuntime>, CommandError> {
        self.reap_closed();
        if self.closing.contains(session_id) {
            return Err(session_not_found());
        }
        self.sessions
            .get(session_id)
            .map(|record| Arc::clone(&record.runtime))
            .ok_or_else(session_not_found)
    }

    fn snapshot(&mut self) -> Result<GenericDeckSessionsView, CommandError> {
        self.reap_closed();
        let foreground = self.registry.broker.foreground_output().cloned();
        let sessions = self
            .sessions
            .iter()
            .filter(|(session_id, _)| !self.closing.contains(*session_id))
            .map(|(session_id, record)| session_view(session_id, record, foreground.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(GenericDeckSessionsView {
            sessions,
            foreground_output: foreground.map(ForegroundLeaseView::from),
            output_pin: self.registry.broker.output_pin().cloned(),
            recent_faults: self.recent_faults.iter().cloned().collect(),
        })
    }

    fn prepare_switch_foreground(
        &mut self,
        target: &SessionId,
    ) -> Result<PreparedForegroundTransition, CommandError> {
        self.reap_closed();
        self.ensure_lifecycle_idle()?;
        let target_runtime = self
            .sessions
            .get(target)
            .filter(|_| !self.closing.contains(target))
            .map(|record| Arc::clone(&record.runtime))
            .ok_or_else(session_not_found)?;
        let previous_id = self
            .registry
            .broker
            .foreground_output()
            .map(|lease| lease.session_id.clone());
        let previous_runtime = previous_id
            .as_ref()
            .and_then(|session_id| self.sessions.get(session_id))
            .map(|record| Arc::clone(&record.runtime));

        if previous_id.as_ref() == Some(target) {
            return self.snapshot().map(PreparedForegroundTransition::Unchanged);
        }
        let mut candidate = self.registry.broker.clone();
        candidate
            .switch_foreground(target)
            .map_err(broker_command_error)?;
        let generation = self.begin_lifecycle_transition()?;
        Ok(PreparedForegroundTransition::Pending(
            ForegroundTransition {
                generation,
                candidate,
                previous_runtime,
                target_runtime: Some(target_runtime),
            },
        ))
    }

    fn prepare_clear_foreground(&mut self) -> Result<PreparedForegroundTransition, CommandError> {
        self.reap_closed();
        self.ensure_lifecycle_idle()?;
        let current_id = self
            .registry
            .broker
            .foreground_output()
            .map(|lease| lease.session_id.clone());
        if current_id.is_none() {
            return self.snapshot().map(PreparedForegroundTransition::Unchanged);
        }
        let previous_runtime = current_id
            .as_ref()
            .and_then(|session_id| self.sessions.get(session_id))
            .map(|record| Arc::clone(&record.runtime));
        let mut candidate = self.registry.broker.clone();
        candidate.clear_foreground().map_err(broker_command_error)?;
        let generation = self.begin_lifecycle_transition()?;
        Ok(PreparedForegroundTransition::Pending(
            ForegroundTransition {
                generation,
                candidate,
                previous_runtime,
                target_runtime: None,
            },
        ))
    }

    fn complete_foreground_transition(
        &mut self,
        transition: &ForegroundTransition,
    ) -> Result<GenericDeckSessionsView, CommandError> {
        if self.lifecycle_transition != Some(transition.generation) {
            return Err(CommandError::new(
                "session.lifecycle_changed",
                "The foreground output lease changed while the actor was responding.",
            ));
        }
        let current = self
            .registry
            .broker
            .sessions()
            .map(|session| session.session_id.clone())
            .collect::<BTreeSet<_>>();
        let candidate = transition
            .candidate
            .sessions()
            .map(|session| session.session_id.clone())
            .collect::<BTreeSet<_>>();
        if current != candidate {
            self.lifecycle_transition = None;
            return Err(CommandError::new(
                "session.lifecycle_changed",
                "A worker lifecycle changed during the foreground output transition.",
            ));
        }
        self.registry.broker = transition.candidate.clone();
        self.lifecycle_transition = None;
        self.snapshot()
    }

    fn pin(
        &mut self,
        session_id: &SessionId,
        kind: OutputPinKind,
    ) -> Result<OutputPinToken, CommandError> {
        self.reap_closed();
        self.ensure_lifecycle_idle()?;
        self.registry
            .pin_foreground(session_id, kind)
            .map_err(broker_command_error)
    }

    fn unpin(&mut self, token: &OutputPinToken) -> Result<(), CommandError> {
        self.reap_closed();
        self.registry
            .release_output_pin(token)
            .map_err(broker_command_error)
    }

    fn prepare_close(
        &mut self,
        session_id: &SessionId,
    ) -> Result<Arc<GenericDeckRuntime>, CommandError> {
        self.reap_closed();
        self.ensure_lifecycle_idle()?;
        if self.closing.contains(session_id) {
            return Err(session_not_found());
        }
        let runtime = self
            .sessions
            .get(session_id)
            .map(|record| Arc::clone(&record.runtime))
            .ok_or_else(session_not_found)?;
        let mut candidate = self.registry.broker.clone();
        candidate
            .close_session(session_id)
            .map_err(broker_command_error)?;
        self.registry.broker = candidate;
        self.closing.insert(session_id.clone());
        Ok(runtime)
    }

    fn finish_close(&mut self, session_id: &SessionId) {
        self.sessions.remove(session_id);
        self.closing.remove(session_id);
    }

    fn detach_all_for_shutdown(&mut self) -> Vec<Arc<GenericDeckRuntime>> {
        let sessions = self
            .sessions
            .values()
            .map(|record| Arc::clone(&record.runtime))
            .collect::<Vec<_>>();
        self.sessions.clear();
        self.registry = GenericSessionRegistry::default();
        self.closing.clear();
        self.lifecycle_transition = None;
        sessions
    }

    fn reap_closed(&mut self) {
        let closed = self
            .sessions
            .iter()
            .filter(|(_, record)| record.runtime.is_closed())
            .map(|(session_id, record)| {
                (
                    session_id.clone(),
                    record.worker_id.clone(),
                    record.runtime.view().ok().and_then(|view| view.fault_code),
                )
            })
            .collect::<Vec<_>>();
        for (session_id, worker_id, fault_code) in closed {
            if !self.closing.remove(&session_id) {
                let _ = self.registry.worker_fault(&worker_id);
            }
            self.sessions.remove(&session_id);
            if let Some(code) = fault_code {
                if self.recent_faults.len() == MAX_RECENT_FAULTS {
                    self.recent_faults.pop_front();
                }
                self.recent_faults.push_back(GenericDeckFaultView {
                    session_id: session_id.as_str().to_owned(),
                    worker_id: worker_id.as_str().to_owned(),
                    code,
                });
            }
        }
        let terminal = self.registry.broker.output_pin().and_then(|token| {
            self.sessions
                .get(token.session_id())
                .map(|record| match token.kind() {
                    OutputPinKind::Capture => record
                        .runtime
                        .cached_capture_status()
                        .is_ok_and(|view| capture_state_terminal(&view.state)),
                    OutputPinKind::Mp4 => {
                        recording_state_terminal(record.runtime.recording_status().state)
                    }
                })
        });
        if terminal == Some(true) {
            let _ = self.registry.reap_terminal_output_pin(|_| true);
        }
    }
}

pub(crate) struct GenericDeckAppState {
    controller: tokio::sync::Mutex<GenericDeckController>,
    viewport: EmbeddedViewportStore,
    app_local_data: PathBuf,
}

impl GenericDeckAppState {
    #[must_use]
    pub(crate) fn new(app_local_data: PathBuf) -> Self {
        Self {
            controller: tokio::sync::Mutex::new(GenericDeckController::default()),
            viewport: EmbeddedViewportStore::new(),
            app_local_data,
        }
    }

    pub(crate) async fn shutdown_all(&self) {
        let runtimes = self.controller.lock().await.detach_all_for_shutdown();
        for runtime in runtimes {
            let _ = runtime.shutdown().await;
        }
    }

    pub(crate) async fn foreground_diagnostics(
        &self,
    ) -> Result<(Option<GenericDeckRuntimeDiagnostics>, Option<&'static str>), CommandError> {
        let (runtime, last_error) = {
            let mut controller = self.controller.lock().await;
            controller.reap_closed();
            let runtime = controller
                .registry
                .broker
                .foreground_output()
                .and_then(|lease| controller.sessions.get(&lease.session_id))
                .map(|record| Arc::clone(&record.runtime));
            let last_error = controller
                .recent_faults
                .back()
                .map(|fault| fault.code.as_str());
            let stable_error = match last_error {
                Some("deck.protocol_fault") => Some("deck.protocol_fault"),
                Some("deck.worker_fault") => Some("deck.worker_fault"),
                Some("deck.worker_timeout") => Some("deck.worker_timeout"),
                Some("output.unavailable") => Some("output.unavailable"),
                Some("output.ring_fault") => Some("output.ring_fault"),
                Some("diagnostics.contract_invalid") => Some("diagnostics.contract_invalid"),
                _ => None,
            };
            (runtime, stable_error)
        };
        let diagnostics = match runtime {
            Some(runtime) => Some(runtime.diagnostics().await.map_err(runtime_command_error)?),
            None => None,
        };
        Ok((diagnostics, last_error))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenericDeckSourceInput {
    cartridge_id: String,
    archive_sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenericProfileKeyInput {
    codec_family: String,
    profile: String,
    profile_version: String,
}

impl GenericProfileKeyInput {
    fn matches_wire(&self, value: &ProfileKey) -> bool {
        self.codec_family == value.codec_family
            && self.profile == value.profile
            && self.profile_version == value.profile_version
    }
}

impl From<&latentdeck_extension_manager::ProfileKey> for GenericProfileKeyInput {
    fn from(value: &latentdeck_extension_manager::ProfileKey) -> Self {
        Self {
            codec_family: value.codec_family.clone(),
            profile: value.profile.clone(),
            profile_version: value.profile_version.clone(),
        }
    }
}

impl From<&ProfileKey> for GenericProfileKeyInput {
    fn from(value: &ProfileKey) -> Self {
        Self {
            codec_family: value.codec_family.clone(),
            profile: value.profile.clone(),
            profile_version: value.profile_version.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenericRuntimeOptionsRequest {
    deck_id: String,
    deck_version: String,
    codec_id: String,
    codec_version: String,
    profile_key: Option<GenericProfileKeyInput>,
    device: DeviceKind,
    device_ordinal: u8,
    #[serde(default)]
    sources: Vec<GenericDeckSourceInput>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenericDeckOpenRequest {
    deck_id: String,
    deck_version: String,
    codec_id: String,
    codec_version: String,
    profile_key: GenericProfileKeyInput,
    device: DeviceKind,
    device_ordinal: u8,
    sources: Vec<GenericDeckSourceInput>,
    roles: Vec<RoleBinding>,
    controls: Vec<ControlBinding>,
    source_transport: Vec<SourceTransportBinding>,
    seed: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GenericExternalAssetRequest {
    codec_id: String,
    codec_version: String,
    asset_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExactPackageView {
    package_id: String,
    package_version: String,
}

impl From<&PackageIdentity> for ExactPackageView {
    fn from(value: &PackageIdentity) -> Self {
        Self {
            package_id: value.package_id.as_str().to_owned(),
            package_version: value.version.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericDeckSourceView {
    cartridge_id: String,
    archive_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericSessionExternalAssetView {
    asset_id: String,
    sha256: String,
    byte_length: u64,
}

#[derive(Clone, Debug)]
struct GenericNegotiatedIdentity {
    profile_key: GenericProfileKeyInput,
    device: DeviceKind,
    device_ordinal: u8,
    external_assets: Vec<GenericSessionExternalAssetView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericNegotiatedIdentityView {
    profile_key: GenericProfileKeyInput,
    device: DeviceKind,
    device_ordinal: u8,
    external_assets: Vec<GenericSessionExternalAssetView>,
}

impl GenericNegotiatedIdentity {
    fn from_prepared(prepared: &PreparedDeckSelectionV2) -> Self {
        Self {
            profile_key: GenericProfileKeyInput::from(&prepared.host.profile_key),
            device: prepared.host.tensor_abi.device,
            device_ordinal: prepared.host.device_ordinal,
            external_assets: prepared
                .external_assets
                .iter()
                .map(|asset| GenericSessionExternalAssetView {
                    asset_id: asset.asset_id.clone(),
                    sha256: asset.sha256.clone(),
                    byte_length: asset.byte_length,
                })
                .collect(),
        }
    }

    fn view(&self) -> GenericNegotiatedIdentityView {
        GenericNegotiatedIdentityView {
            profile_key: self.profile_key.clone(),
            device: self.device,
            device_ordinal: self.device_ordinal,
            external_assets: self.external_assets.clone(),
        }
    }
}

impl From<&GenericDeckSourceInput> for GenericDeckSourceView {
    fn from(value: &GenericDeckSourceInput) -> Self {
        Self {
            cartridge_id: value.cartridge_id.clone(),
            archive_sha256: value.archive_sha256.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericDeckSessionView {
    session_id: String,
    worker_id: String,
    deck: ExactPackageView,
    codec: ExactPackageView,
    #[serde(flatten)]
    negotiated: GenericNegotiatedIdentityView,
    sources: Vec<GenericDeckSourceView>,
    runtime: GenericDeckRuntimeView,
    foreground: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ForegroundLeaseView {
    session_id: String,
    generation: u64,
}

impl From<ForegroundLease> for ForegroundLeaseView {
    fn from(value: ForegroundLease) -> Self {
        Self {
            session_id: value.session_id.as_str().to_owned(),
            generation: value.generation,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericDeckSessionsView {
    sessions: Vec<GenericDeckSessionView>,
    foreground_output: Option<ForegroundLeaseView>,
    output_pin: Option<OutputPinToken>,
    recent_faults: Vec<GenericDeckFaultView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericExternalAssetView {
    codec_id: String,
    codec_version: String,
    asset_id: String,
    bound: bool,
    sha256: Option<String>,
    byte_length: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericExternalAssetOptionView {
    asset_id: String,
    display_name: String,
    required_sha256: String,
    byte_length: u64,
    required: bool,
    bound: bool,
    bound_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GenericSourceOptionView {
    cartridge_id: String,
    archive_sha256: String,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericRuntimeOptionsView {
    deck: ExactPackageView,
    codec: ExactPackageView,
    reason: String,
    profiles: Vec<GenericProfileKeyInput>,
    device: DeviceKind,
    slots: u8,
    external_assets: Vec<GenericExternalAssetOptionView>,
    sources: Vec<GenericSourceOptionView>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericSessionDiagnosticsView {
    session_id: String,
    deck: ExactPackageView,
    codec: ExactPackageView,
    #[serde(flatten)]
    negotiated: GenericNegotiatedIdentityView,
    runtime: GenericDeckRuntimeView,
    diagnostics: GenericDeckRuntimeDiagnostics,
    capture: GenericCaptureView,
    recording: RecorderStatus,
    spout: NativeSpoutStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericCaptureSessionView {
    session_id: String,
    capture_id: Option<String>,
    mode: Option<CaptureMode>,
    state: String,
    latent_slots: String,
    reset_events: u32,
    cartridge_id: Option<String>,
    archive_sha256: Option<String>,
    detail: Option<String>,
}

impl GenericCaptureSessionView {
    fn new(session_id: &SessionId, capture: GenericCaptureView) -> Self {
        Self {
            session_id: session_id.as_str().to_owned(),
            capture_id: capture.capture_id,
            mode: capture.mode,
            state: capture.state,
            latent_slots: capture.latent_slots,
            reset_events: capture.reset_events,
            cartridge_id: capture.cartridge_id,
            archive_sha256: capture.archive_sha256,
            detail: capture.detail,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GenericRecordingSessionView {
    session_id: String,
    state: RecorderState,
    frames_accepted: u64,
    frames_written: u64,
    width: Option<u32>,
    height: Option<u32>,
    error_code: Option<&'static str>,
}

impl GenericRecordingSessionView {
    fn new(session_id: &SessionId, status: &RecorderStatus) -> Self {
        Self {
            session_id: session_id.as_str().to_owned(),
            state: status.state,
            frames_accepted: status.frames_accepted,
            frames_written: status.frames_written,
            width: status.width,
            height: status.height,
            error_code: status.error_code,
        }
    }
}

fn session_view(
    session_id: &SessionId,
    record: &GenericSessionRecord,
    foreground: Option<&ForegroundLease>,
) -> Result<GenericDeckSessionView, CommandError> {
    Ok(GenericDeckSessionView {
        session_id: session_id.as_str().to_owned(),
        worker_id: record.worker_id.as_str().to_owned(),
        deck: ExactPackageView::from(&record.deck),
        codec: ExactPackageView::from(&record.codec),
        negotiated: record.negotiated.view(),
        sources: record.sources.clone(),
        runtime: record.runtime.view().map_err(runtime_command_error)?,
        foreground: foreground.is_some_and(|lease| &lease.session_id == session_id),
    })
}

fn session_not_found() -> CommandError {
    CommandError::new(
        "session.not_found",
        "The exact generic Deck session is not active.",
    )
}

fn broker_command_error(error: BrokerError) -> CommandError {
    CommandError::new(
        error.code(),
        "The generic Deck session broker rejected the requested lifecycle transition.",
    )
}

fn runtime_command_error(error: GenericDeckRuntimeError) -> CommandError {
    CommandError::new(error.code, error.message)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub(crate) async fn deck_generic_runtime_options(
    extensions: State<'_, ExtensionManagerState>,
    library: State<'_, AppState>,
    state: State<'_, GenericDeckAppState>,
    request: GenericRuntimeOptionsRequest,
) -> Result<GenericRuntimeOptionsView, CommandError> {
    if request.sources.len() > MAX_SOURCE_OPTIONS
        || (request.profile_key.is_none() && !request.sources.is_empty())
        || (request.device == DeviceKind::Cpu && request.device_ordinal != 0)
    {
        return Err(CommandError::new(
            "deck.input_invalid",
            "Runtime discovery requires a bounded exact profile and Library identity set.",
        ));
    }
    let deck_reference = PackageReference {
        kind: PackageKind::DeckPack,
        package_id: request.deck_id.clone(),
        package_version: request.deck_version.clone(),
    };
    let codec_reference = PackageReference {
        kind: PackageKind::CodecPack,
        package_id: request.codec_id.clone(),
        package_version: request.codec_version.clone(),
    };
    let matrix = compatibility_matrix(extensions.roots()).map_err(extension_command_error)?;
    let mut reason = matrix
        .iter()
        .find(|pair| pair.deck == deck_reference && pair.codec == codec_reference)
        .map_or(CompatibilityReason::Untrusted, |pair| pair.reason);

    let deck = resolve_active(extensions.roots(), &deck_reference).ok();
    let codec = resolve_active(extensions.roots(), &codec_reference).ok();
    let (deck_manifest, codec_manifest) = match (
        deck.as_ref()
            .map(latentdeck_extension_manager::ActiveInstalledPackage::manifest),
        codec
            .as_ref()
            .map(latentdeck_extension_manager::ActiveInstalledPackage::manifest),
    ) {
        (Some(PackageManifest::Deck(deck)), Some(PackageManifest::Codec(codec))) => {
            reason = discovery_reason(deck, codec, request.device, reason);
            (Some(deck.clone()), Some(codec.clone()))
        }
        _ => (None, None),
    };
    let profiles = deck_manifest
        .as_ref()
        .zip(codec_manifest.as_ref())
        .map_or_else(Vec::new, |(deck, codec)| compatible_profiles(deck, codec));
    if let Some(selected) = &request.profile_key
        && !profiles.iter().any(|profile| profile == selected)
    {
        reason = CompatibilityReason::UnsupportedProfile;
    }

    let bound_assets = {
        let mut controller = state.controller.lock().await;
        controller.reap_closed();
        controller
            .external_assets
            .iter()
            .filter(|(key, _)| {
                key.codec_id == request.codec_id && key.codec_version == request.codec_version
            })
            .map(|(key, value)| (key.asset_id.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>()
    };
    let external_assets = codec_manifest.as_ref().map_or_else(Vec::new, |manifest| {
        manifest
            .external_assets
            .iter()
            .map(|asset| {
                let bound = bound_assets.get(&asset.asset_id);
                GenericExternalAssetOptionView {
                    asset_id: asset.asset_id.clone(),
                    display_name: asset.display_name.clone(),
                    required_sha256: asset.sha256.clone(),
                    byte_length: asset.byte_length,
                    required: asset.required,
                    bound: bound.is_some(),
                    bound_sha256: bound.map(|value| value.sha256.clone()),
                }
            })
            .collect()
    });
    if reason == CompatibilityReason::Compatible
        && codec_manifest.as_ref().is_some_and(|manifest| {
            manifest
                .external_assets
                .iter()
                .any(|asset| asset.required && !bound_assets.contains_key(&asset.asset_id))
        })
    {
        reason = CompatibilityReason::MissingAsset;
    }

    let slots = deck_manifest
        .as_ref()
        .map_or(0, |manifest| manifest.signal.slots);
    let mut source_views = Vec::with_capacity(request.sources.len());
    for source in &request.sources {
        let source_reason = if reason == CompatibilityReason::Compatible {
            source_option_reason(
                extensions.roots().clone(),
                &library,
                &request,
                source,
                slots,
                &bound_assets,
            )
            .await
        } else {
            compatibility_reason_code(reason).to_owned()
        };
        source_views.push(GenericSourceOptionView {
            cartridge_id: source.cartridge_id.clone(),
            archive_sha256: source.archive_sha256.clone(),
            reason: source_reason,
        });
    }
    Ok(GenericRuntimeOptionsView {
        deck: exact_package_view(&request.deck_id, &request.deck_version),
        codec: exact_package_view(&request.codec_id, &request.codec_version),
        reason: compatibility_reason_code(reason).to_owned(),
        profiles,
        device: request.device,
        slots,
        external_assets,
        sources: source_views,
    })
}

async fn source_option_reason(
    roots: ExtensionRoots,
    library: &AppState,
    request: &GenericRuntimeOptionsRequest,
    source: &GenericDeckSourceInput,
    slots: u8,
    assets: &BTreeMap<String, BoundExternalAsset>,
) -> String {
    let Some(profile) = request.profile_key.clone() else {
        return "unsupported_profile".to_owned();
    };
    if slots == 0 || slots > 16 {
        return "unsupported_signal".to_owned();
    }
    let Ok(identity) = source_identity(source) else {
        return "package_invalid".to_owned();
    };
    let Ok(resolved) = library.resolve_deck_source(identity).await else {
        return "package_invalid".to_owned();
    };
    let deck_id = request.deck_id.clone();
    let deck_version = request.deck_version.clone();
    let codec_id = request.codec_id.clone();
    let codec_version = request.codec_version.clone();
    let device = request.device;
    let device_ordinal = request.device_ordinal;
    let assets = assets
        .iter()
        .map(|(asset_id, asset)| (asset_id.clone(), asset.path.clone()))
        .collect::<Vec<_>>();
    tauri::async_runtime::spawn_blocking(move || {
        let mut selection =
            DeckPackageSelectionV2::new(deck_id, deck_version, codec_id, codec_version, device);
        selection.set_device_ordinal(device_ordinal);
        for (asset_id, path) in assets {
            selection.bind_external_asset(asset_id, path);
        }
        let source_inputs = (0..slots)
            .map(|_| DeckSourceSelectionV2 {
                path: resolved.path(),
                cartridge_id: resolved.identity().cartridge_id(),
                archive_sha256: resolved.identity().archive_sha256().as_str(),
            })
            .collect::<Vec<_>>();
        match prepare_exact_deck_selection(
            &roots,
            &selection,
            &source_inputs,
            latentdeck_core::product_version(),
        ) {
            Ok(prepared) if profile.matches_wire(&prepared.host.profile_key) => {
                "compatible".to_owned()
            }
            Ok(_) => "unsupported_profile".to_owned(),
            Err(error) => error.code().to_owned(),
        }
    })
    .await
    .unwrap_or_else(|_| "package_invalid".to_owned())
}

fn compatible_profiles(
    deck: &latentdeck_extension_manager::DeckPackManifest,
    codec: &latentdeck_extension_manager::CodecPackManifest,
) -> Vec<GenericProfileKeyInput> {
    codec
        .compatibility
        .profiles
        .iter()
        .filter(|profile| {
            deck.signal
                .profile_allowlist
                .as_ref()
                .is_none_or(|allowlist| allowlist.contains(profile))
        })
        .map(GenericProfileKeyInput::from)
        .collect()
}

fn discovery_reason(
    deck: &latentdeck_extension_manager::DeckPackManifest,
    codec: &latentdeck_extension_manager::CodecPackManifest,
    device: DeviceKind,
    base: CompatibilityReason,
) -> CompatibilityReason {
    if base != CompatibilityReason::Compatible {
        return base;
    }
    if deck.manifest_version != "1.0.0"
        || codec.manifest_version != "2.0.0"
        || deck.compatibility.worker_protocol != 2
        || codec.compatibility.worker_protocol != 2
    {
        return CompatibilityReason::UnsupportedProtocol;
    }
    let Ok(app) = Version::parse(latentdeck_core::product_version()) else {
        return CompatibilityReason::UnsupportedHostApi;
    };
    if !version_in_range(
        &app,
        &deck.compatibility.app_min_inclusive,
        &deck.compatibility.app_max_exclusive,
    ) || !version_in_range(
        &app,
        &codec.compatibility.app_min_inclusive,
        &codec.compatibility.app_max_exclusive,
    ) {
        return CompatibilityReason::UnsupportedHostApi;
    }
    if deck.compatibility.deck_host_api != 1
        || deck.compatibility.deck_operator_api != 1
        || codec.compatibility.codec_adapter_api != 1
        || deck.compatibility.tensor_abi != "latentdeck.tensor.v1"
        || codec.compatibility.tensor_abi != "latentdeck.tensor.v1"
        || deck.compatibility.python != codec.compatibility.python
        || deck.compatibility.torch_exact_build != codec.compatibility.torch_exact_build
    {
        return CompatibilityReason::UnsupportedTensorAbi;
    }
    if !geometry_allowlist_supports_device(deck, device) {
        return CompatibilityReason::UnsupportedTensorAbi;
    }
    if deck
        .signal
        .required_capabilities
        .iter()
        .any(|required| !codec.capabilities.contains(required))
    {
        return CompatibilityReason::UnsupportedCapability;
    }
    CompatibilityReason::Compatible
}

fn geometry_allowlist_supports_device(
    deck: &latentdeck_extension_manager::DeckPackManifest,
    device: DeviceKind,
) -> bool {
    deck.signal.geometry_allowlist.iter().any(|geometry| {
        matches!(
            (geometry.device, device),
            (
                latentdeck_extension_manager::TensorDevice::Cpu,
                DeviceKind::Cpu
            ) | (
                latentdeck_extension_manager::TensorDevice::Cuda,
                DeviceKind::Cuda
            )
        )
    })
}

fn version_in_range(version: &Version, minimum: &str, maximum: &str) -> bool {
    Version::parse(minimum)
        .ok()
        .zip(Version::parse(maximum).ok())
        .is_some_and(|(minimum, maximum)| version >= &minimum && version < &maximum)
}

const fn compatibility_reason_code(reason: CompatibilityReason) -> &'static str {
    match reason {
        CompatibilityReason::Compatible => "compatible",
        CompatibilityReason::Untrusted => "untrusted",
        CompatibilityReason::MissingAsset => "missing_asset",
        CompatibilityReason::PackageInvalid => "package_invalid",
        CompatibilityReason::UnsupportedProtocol => "unsupported_protocol",
        CompatibilityReason::UnsupportedHostApi => "unsupported_host_api",
        CompatibilityReason::UnsupportedTensorAbi => "unsupported_tensor_abi",
        CompatibilityReason::UnsupportedProfile => "unsupported_profile",
        CompatibilityReason::UnsupportedSignal => "unsupported_signal",
        CompatibilityReason::UnsupportedTiming => "unsupported_timing",
        CompatibilityReason::UnsupportedCapability => "unsupported_capability",
    }
}

fn exact_package_view(id: &str, version: &str) -> ExactPackageView {
    ExactPackageView {
        package_id: id.to_owned(),
        package_version: version.to_owned(),
    }
}

fn source_identity(input: &GenericDeckSourceInput) -> Result<DeckSourceIdentity, CommandError> {
    DeckSourceIdentity::new(
        input.cartridge_id.clone(),
        CartridgeKey::new_unchecked(input.archive_sha256.clone()),
    )
    .map_err(|_| {
        CommandError::new(
            "deck.source_identity_invalid",
            "A generic Deck source requires a canonical cartridge UUID and lowercase SHA-256.",
        )
    })
}

fn extension_command_error(_error: latentdeck_extension_manager::ExtensionError) -> CommandError {
    CommandError::new(
        "package_invalid",
        "LatentDeck could not inspect the exact installed extension set safely.",
    )
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_external_asset_select(
    app: AppHandle,
    extensions: State<'_, ExtensionManagerState>,
    state: State<'_, GenericDeckAppState>,
    codec_id: String,
    codec_version: String,
    asset_id: String,
) -> Result<Option<GenericExternalAssetView>, CommandError> {
    let request = GenericExternalAssetRequest {
        codec_id,
        codec_version,
        asset_id,
    };
    let reference = PackageReference {
        kind: PackageKind::CodecPack,
        package_id: request.codec_id.clone(),
        package_version: request.codec_version.clone(),
    };
    let package = resolve_active(extensions.roots(), &reference).map_err(|_| {
        CommandError::new(
            "untrusted",
            "The exact Codec Pack version is not active and trusted.",
        )
    })?;
    let PackageManifest::Codec(manifest) = package.manifest() else {
        return Err(CommandError::new(
            "package_invalid",
            "The exact selected package is not a Codec Pack.",
        ));
    };
    let descriptor = manifest
        .external_assets
        .iter()
        .find(|asset| asset.asset_id == request.asset_id)
        .cloned()
        .ok_or_else(|| {
            CommandError::new(
                "package_invalid",
                "The Codec Pack does not declare the requested external asset identity.",
            )
        })?;
    let selected = app.dialog().file().blocking_pick_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let selected = selected.into_path().map_err(|_| {
        CommandError::new(
            "missing_asset",
            "The native picker did not return a usable external asset file.",
        )
    })?;
    let selected = validate_external_asset_path(&selected)?;
    let measurement = hash_path(&selected).map_err(|_| {
        CommandError::new(
            "missing_asset",
            "LatentDeck could not measure the selected external asset safely.",
        )
    })?;
    let sha256 = measurement.sha256.to_string();
    if measurement.byte_length != descriptor.byte_length || sha256 != descriptor.sha256 {
        return Err(CommandError::new(
            "missing_asset",
            "The selected external asset does not match the Codec Pack's exact hash and length.",
        ));
    }
    let key = ExternalAssetKey {
        codec_id: request.codec_id,
        codec_version: request.codec_version,
        asset_id: request.asset_id,
    };
    let mut controller = state.controller.lock().await;
    controller.bind_asset(
        key.clone(),
        BoundExternalAsset {
            path: selected,
            sha256,
            byte_length: measurement.byte_length,
        },
    );
    Ok(Some(controller.asset_view(&key)))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_external_asset_clear(
    state: State<'_, GenericDeckAppState>,
    codec_id: String,
    codec_version: String,
    asset_id: String,
) -> Result<GenericExternalAssetView, CommandError> {
    let request = GenericExternalAssetRequest {
        codec_id,
        codec_version,
        asset_id,
    };
    let key = ExternalAssetKey {
        codec_id: request.codec_id,
        codec_version: request.codec_version,
        asset_id: request.asset_id,
    };
    let mut controller = state.controller.lock().await;
    controller.clear_asset(&key);
    Ok(controller.asset_view(&key))
}

fn validate_external_asset_path(path: &Path) -> Result<PathBuf, CommandError> {
    if !path.is_absolute() {
        return Err(CommandError::new(
            "missing_asset",
            "External assets must be selected through an absolute native file identity.",
        ));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CommandError::new(
            "missing_asset",
            "The selected external asset is unavailable.",
        )
    })?;
    if !metadata.is_file() || is_reparse(&metadata) {
        return Err(CommandError::new(
            "missing_asset",
            "External assets cannot be symbolic links, reparse points, or non-files.",
        ));
    }
    fs::canonicalize(path).map_err(|_| {
        CommandError::new(
            "missing_asset",
            "The selected external asset identity cannot be retained safely.",
        )
    })
}

#[cfg(target_os = "windows")]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(target_os = "windows"))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value, clippy::too_many_lines)]
pub(crate) async fn deck_generic_open(
    app: AppHandle,
    extensions: State<'_, ExtensionManagerState>,
    library: State<'_, AppState>,
    state: State<'_, GenericDeckAppState>,
    request: GenericDeckOpenRequest,
) -> Result<GenericDeckSessionView, CommandError> {
    if request.sources.is_empty()
        || request.sources.len() > 16
        || request.seed > MAX_JS_SAFE_INTEGER
        || (request.device == DeviceKind::Cpu && request.device_ordinal != 0)
    {
        return Err(CommandError::new(
            "deck.input_invalid",
            "Generic Deck open requires 1-16 exact sources and bounded controls, transport, and seed.",
        ));
    }
    let session_id =
        SessionId::new(format!("generic-{}", Uuid::new_v4().simple())).map_err(|_| {
            CommandError::new(
                "session.identity_invalid",
                "LatentDeck could not allocate a generic Deck session identity.",
            )
        })?;
    {
        let mut controller = state.controller.lock().await;
        controller.reserve(session_id.clone())?;
    }

    let result = Box::pin(async {
        let mut resolved = Vec::with_capacity(request.sources.len());
        for source in &request.sources {
            resolved.push(
                library
                    .resolve_deck_source(source_identity(source)?)
                    .await?,
            );
        }
        let asset_paths = {
            let controller = state.controller.lock().await;
            controller.asset_paths(&request.codec_id, &request.codec_version)
        };
        let roots = extensions.roots().clone();
        let deck_id = request.deck_id.clone();
        let deck_version = request.deck_version.clone();
        let codec_id = request.codec_id.clone();
        let codec_version = request.codec_version.clone();
        let profile = request.profile_key.clone();
        let device = request.device;
        let device_ordinal = request.device_ordinal;
        let prepared = tauri::async_runtime::spawn_blocking(move || {
            prepare_open_selection(
                &roots,
                deck_id,
                deck_version,
                codec_id,
                codec_version,
                &profile,
                device,
                device_ordinal,
                asset_paths,
                &resolved,
            )
        })
        .await
        .map_err(|_| CommandError::new("package_invalid", "Generic Deck preflight stopped."))??;
        let viewport = state.viewport.current_visible()?;
        let parent = main_window(&app)?;
        let load = DeckSessionV2LoadRequest {
            roles: request.roles.clone(),
            controls: request.controls.clone(),
            source_transport: request.source_transport.clone(),
            seed: request.seed,
        };
        let negotiated = GenericNegotiatedIdentity::from_prepared(&prepared);
        let runtime = Arc::new(
            Box::pin(GenericDeckRuntime::start(
                app.clone(),
                parent,
                viewport,
                prepared,
                load,
                state.app_local_data.clone(),
                library.importer(),
            ))
            .await
            .map_err(runtime_command_error)?,
        );
        let worker_id = WorkerId::new(format!(
            "worker-{}-{}",
            runtime.worker_pid(),
            Uuid::new_v4().simple()
        ))
        .map_err(|_| {
            CommandError::new(
                "session.identity_invalid",
                "LatentDeck could not bind the generic worker identity.",
            )
        })?;
        let record = GenericSessionRecord {
            runtime: Arc::clone(&runtime),
            worker_id,
            deck: package_identity(&request.deck_id, &request.deck_version)?,
            codec: package_identity(&request.codec_id, &request.codec_version)?,
            negotiated,
            sources: request
                .sources
                .iter()
                .map(GenericDeckSourceView::from)
                .collect(),
        };
        {
            let mut controller = state.controller.lock().await;
            if let Err(error) = controller.commit(&session_id, record) {
                drop(controller);
                let _ = runtime.shutdown().await;
                return Err(error);
            }
        }
        let latest = state.viewport.current()?;
        if let Err(error) = runtime.set_viewport(latest).await {
            let _ = close_generic_session(&state, &session_id).await;
            return Err(runtime_command_error(error));
        }
        let mut controller = state.controller.lock().await;
        let snapshot = controller.snapshot()?;
        snapshot
            .sessions
            .into_iter()
            .find(|session| session.session_id == session_id.as_str())
            .ok_or_else(session_not_found)
    })
    .await;
    if result.is_err() {
        state
            .controller
            .lock()
            .await
            .cancel_reservation(&session_id);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn prepare_open_selection(
    roots: &ExtensionRoots,
    deck_id: String,
    deck_version: String,
    codec_id: String,
    codec_version: String,
    profile: &GenericProfileKeyInput,
    device: DeviceKind,
    device_ordinal: u8,
    asset_paths: Vec<(String, PathBuf)>,
    sources: &[ResolvedDeckSource],
) -> Result<latentdeck_core::deck_selection_v2::PreparedDeckSelectionV2, CommandError> {
    let mut selection =
        DeckPackageSelectionV2::new(deck_id, deck_version, codec_id, codec_version, device);
    selection.set_device_ordinal(device_ordinal);
    for (asset_id, path) in asset_paths {
        selection.bind_external_asset(asset_id, path);
    }
    let source_inputs = sources
        .iter()
        .map(|source| DeckSourceSelectionV2 {
            path: source.path(),
            cartridge_id: source.identity().cartridge_id(),
            archive_sha256: source.identity().archive_sha256().as_str(),
        })
        .collect::<Vec<_>>();
    let prepared = prepare_exact_deck_selection(
        roots,
        &selection,
        &source_inputs,
        latentdeck_core::product_version(),
    )
    .map_err(selection_command_error)?;
    if !profile.matches_wire(&prepared.host.profile_key) {
        return Err(selection_command_error(
            DeckSelectionV2Error::UnsupportedProfile,
        ));
    }
    Ok(prepared)
}

fn package_identity(id: &str, version: &str) -> Result<PackageIdentity, CommandError> {
    let id = ContractId::new(id.to_owned()).map_err(|_| {
        CommandError::new(
            "package_invalid",
            "The exact extension package identity is invalid.",
        )
    })?;
    let version = Version::parse(version).map_err(|_| {
        CommandError::new(
            "package_invalid",
            "The exact extension package version is invalid.",
        )
    })?;
    Ok(PackageIdentity::new(id, version))
}

fn selection_command_error(error: DeckSelectionV2Error) -> CommandError {
    CommandError::new(
        error.code(),
        "The exact Deck, Codec, profile, assets, and Library source set are not compatible.",
    )
}

#[tauri::command]
pub(crate) async fn deck_generic_sessions_get(
    state: State<'_, GenericDeckAppState>,
) -> Result<GenericDeckSessionsView, CommandError> {
    state.controller.lock().await.snapshot()
}

#[tauri::command]
pub(crate) async fn deck_generic_status_get(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericDeckSessionView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let mut controller = state.controller.lock().await;
    let snapshot = controller.snapshot()?;
    snapshot
        .sessions
        .into_iter()
        .find(|session| session.session_id == session_id.as_str())
        .ok_or_else(session_not_found)
}

#[tauri::command]
pub(crate) async fn deck_generic_process_once(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericDeckRuntimeView, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .process_once()
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_controls_set(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    controls: Vec<ControlBinding>,
) -> Result<GenericDeckRuntimeView, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .controls_set(controls)
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_roles_set(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    roles: Vec<RoleBinding>,
) -> Result<GenericDeckRuntimeView, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .roles_set(roles)
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_transport_set(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    source_transport: Vec<SourceTransportBinding>,
) -> Result<GenericDeckRuntimeView, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .transport_set(source_transport)
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_seed_set(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    seed: u64,
) -> Result<GenericDeckRuntimeView, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .seed_set(seed)
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_reset(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    preserve_playheads: bool,
) -> Result<GenericDeckRuntimeView, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .reset(preserve_playheads)
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_close(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<(), CommandError> {
    let session_id = parse_session_id(session_id)?;
    close_generic_session(&state, &session_id).await
}

#[tauri::command]
pub(crate) async fn deck_generic_foreground_set(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericDeckSessionsView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let prepared = state
        .controller
        .lock()
        .await
        .prepare_switch_foreground(&session_id)?;
    execute_foreground_transition(&state, prepared).await
}

#[tauri::command]
pub(crate) async fn deck_generic_foreground_clear(
    state: State<'_, GenericDeckAppState>,
) -> Result<GenericDeckSessionsView, CommandError> {
    let prepared = state.controller.lock().await.prepare_clear_foreground()?;
    execute_foreground_transition(&state, prepared).await
}

async fn execute_foreground_transition(
    state: &GenericDeckAppState,
    prepared: PreparedForegroundTransition,
) -> Result<GenericDeckSessionsView, CommandError> {
    let transition = match prepared {
        PreparedForegroundTransition::Unchanged(snapshot) => return Ok(snapshot),
        PreparedForegroundTransition::Pending(transition) => transition,
    };
    if let Some(previous) = &transition.previous_runtime
        && let Err(error) = previous.set_foreground(false).await
    {
        state
            .controller
            .lock()
            .await
            .abort_lifecycle_transition(transition.generation);
        return Err(runtime_command_error(error));
    }
    if let Some(target) = &transition.target_runtime
        && let Err(error) = target.set_foreground(true).await
    {
        if let Some(previous) = &transition.previous_runtime {
            let _ = previous.set_foreground(true).await;
        }
        state
            .controller
            .lock()
            .await
            .abort_lifecycle_transition(transition.generation);
        return Err(runtime_command_error(error));
    }
    let completion = state
        .controller
        .lock()
        .await
        .complete_foreground_transition(&transition);
    if completion.is_err() {
        if let Some(target) = &transition.target_runtime {
            let _ = target.set_foreground(false).await;
        }
        if let Some(previous) = &transition.previous_runtime {
            let _ = previous.set_foreground(true).await;
        }
    }
    completion
}

async fn close_generic_session(
    state: &GenericDeckAppState,
    session_id: &SessionId,
) -> Result<(), CommandError> {
    let runtime = state.controller.lock().await.prepare_close(session_id)?;
    let shutdown = runtime.shutdown().await;
    state.controller.lock().await.finish_close(session_id);
    shutdown.map_err(runtime_command_error)
}

async fn runtime_for(
    state: &GenericDeckAppState,
    session_id: String,
) -> Result<Arc<GenericDeckRuntime>, CommandError> {
    let session_id = parse_session_id(session_id)?;
    state.controller.lock().await.runtime(&session_id)
}

fn parse_session_id(value: String) -> Result<SessionId, CommandError> {
    SessionId::new(value).map_err(|_| {
        CommandError::new(
            "session.not_found",
            "The generic Deck session identity is invalid or no longer active.",
        )
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_viewport_session_begin(
    app: AppHandle,
    state: State<'_, GenericDeckAppState>,
) -> Result<ViewportSessionAck, CommandError> {
    let _parent = main_window(&app)?;
    let (session, hidden) = state.viewport.begin_session()?;
    let runtimes = active_runtimes(&state).await;
    for runtime in runtimes {
        runtime
            .set_viewport(hidden)
            .await
            .map_err(runtime_command_error)?;
    }
    state.viewport.confirm_session(session, hidden)?;
    Ok(session)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_viewport_set_bounds(
    app: AppHandle,
    state: State<'_, GenericDeckAppState>,
    bounds: ViewportBoundsRequest,
) -> Result<(), CommandError> {
    let window = main_window(&app)?;
    let scale_factor = window.scale_factor().map_err(|_| {
        CommandError::new(
            "output.viewport_scale_unavailable",
            "LatentDeck could not read the main-window display scale.",
        )
    })?;
    let client = window.inner_size().map_err(|_| {
        CommandError::new(
            "output.viewport_client_unavailable",
            "LatentDeck could not read the main-window client size.",
        )
    })?;
    let request = validate_viewport_bounds(bounds, scale_factor, client.width, client.height)
        .map_err(viewport_error)?;
    let viewport = state.viewport.apply(request)?;
    for runtime in active_runtimes(&state).await {
        runtime
            .set_viewport(viewport)
            .await
            .map_err(runtime_command_error)?;
    }
    state.viewport.confirm_applied(request, viewport)?;
    Ok(())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_fullscreen_status_get(
    app: AppHandle,
    fullscreen: State<'_, HostFullscreenController>,
) -> Result<bool, CommandError> {
    fullscreen
        .status(&main_window(&app)?)
        .await
        .map_err(|_| fullscreen_error())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_fullscreen_set(
    app: AppHandle,
    fullscreen: State<'_, HostFullscreenController>,
    enabled: bool,
) -> Result<bool, CommandError> {
    fullscreen
        .set(&main_window(&app)?, enabled)
        .await
        .map_err(|_| fullscreen_error())
}

#[tauri::command]
pub(crate) async fn deck_generic_spout_status_get(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<NativeSpoutStatus, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .spout_status()
        .await
        .map_err(runtime_command_error)
}

#[tauri::command]
pub(crate) async fn deck_generic_spout_configure(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    name: Option<String>,
    enabled: Option<bool>,
) -> Result<NativeSpoutStatus, CommandError> {
    runtime_for(&state, session_id)
        .await?
        .configure_spout(name, enabled)
        .await
        .map_err(runtime_command_error)
}

async fn active_runtimes(state: &GenericDeckAppState) -> Vec<Arc<GenericDeckRuntime>> {
    let mut controller = state.controller.lock().await;
    controller.reap_closed();
    controller
        .sessions
        .values()
        .map(|record| Arc::clone(&record.runtime))
        .collect()
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow, CommandError> {
    app.get_webview_window("main").ok_or_else(|| {
        CommandError::new(
            "output.main_window_unavailable",
            "The LatentDeck main window is unavailable.",
        )
    })
}

fn fullscreen_error() -> CommandError {
    CommandError::new(
        "output.window_fullscreen_failed",
        "LatentDeck could not change or confirm the main-window fullscreen state.",
    )
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_capture_start(
    app: AppHandle,
    state: State<'_, GenericDeckAppState>,
    session_id: String,
    mode: CaptureMode,
) -> Result<Option<GenericCaptureSessionView>, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let suggested = match mode {
        CaptureMode::Snapshot => "LatentDeck Snapshot.lc",
        CaptureMode::LiveCapture => "LatentDeck Live Capture.lc",
    };
    let Some(output) = select_capture_output(&app, suggested)? else {
        return Ok(None);
    };
    let runtime = {
        let mut controller = state.controller.lock().await;
        controller.runtime(&session_id)?
    };
    let token = {
        let mut controller = state.controller.lock().await;
        controller.pin(&session_id, OutputPinKind::Capture)?
    };
    let capture = match runtime.capture_start(mode, output).await {
        Ok(capture) => capture,
        Err(error) => {
            let _ = state.controller.lock().await.unpin(&token);
            return Err(runtime_command_error(error));
        }
    };
    spawn_capture_pin_monitor(app, session_id.clone(), token, runtime);
    Ok(Some(GenericCaptureSessionView::new(&session_id, capture)))
}

#[tauri::command]
pub(crate) async fn deck_generic_capture_stop(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericCaptureSessionView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let runtime = {
        let mut controller = state.controller.lock().await;
        controller.runtime(&session_id)?
    };
    let capture = runtime
        .capture_stop()
        .await
        .map_err(runtime_command_error)?;
    state.controller.lock().await.reap_closed();
    Ok(GenericCaptureSessionView::new(&session_id, capture))
}

#[tauri::command]
pub(crate) async fn deck_generic_capture_status_get(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericCaptureSessionView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let runtime = {
        let mut controller = state.controller.lock().await;
        controller.runtime(&session_id)?
    };
    let capture = runtime
        .capture_status()
        .await
        .map_err(runtime_command_error)?;
    state.controller.lock().await.reap_closed();
    Ok(GenericCaptureSessionView::new(&session_id, capture))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) async fn deck_generic_recording_start(
    app: AppHandle,
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<Option<GenericRecordingSessionView>, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let selected = app
        .dialog()
        .file()
        .add_filter("H.264 MP4 Video", &["mp4"])
        .set_file_name("LatentDeck Output.mp4")
        .blocking_save_file();
    let Some(selected) = selected else {
        return Ok(None);
    };
    let destination = selected.into_path().map_err(|_| {
        CommandError::new(
            "recording.destination_invalid",
            "The native save dialog did not return a usable MP4 destination.",
        )
    })?;
    let destination = normalize_mp4_destination(destination).map_err(recording_command_error)?;
    let runtime = {
        let mut controller = state.controller.lock().await;
        controller.runtime(&session_id)?
    };
    let token = {
        let mut controller = state.controller.lock().await;
        controller.pin(&session_id, OutputPinKind::Mp4)?
    };
    let status = match runtime.recording_start(destination).await {
        Ok(status) => status,
        Err(error) => {
            let _ = state.controller.lock().await.unpin(&token);
            return Err(runtime_command_error(error));
        }
    };
    spawn_recording_pin_monitor(app, session_id.clone(), token, Arc::clone(&runtime));
    Ok(Some(GenericRecordingSessionView::new(&session_id, &status)))
}

#[tauri::command]
pub(crate) async fn deck_generic_recording_stop(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericRecordingSessionView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let runtime = {
        let mut controller = state.controller.lock().await;
        controller.runtime(&session_id)?
    };
    let status = runtime
        .recording_stop()
        .await
        .map_err(runtime_command_error)?;
    state.controller.lock().await.reap_closed();
    Ok(GenericRecordingSessionView::new(&session_id, &status))
}

#[tauri::command]
pub(crate) async fn deck_generic_recording_status_get(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericRecordingSessionView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let runtime = {
        let mut controller = state.controller.lock().await;
        controller.runtime(&session_id)?
    };
    let status = runtime.recording_status();
    state.controller.lock().await.reap_closed();
    Ok(GenericRecordingSessionView::new(&session_id, &status))
}

#[tauri::command]
pub(crate) async fn deck_generic_diagnostics_get(
    state: State<'_, GenericDeckAppState>,
    session_id: String,
) -> Result<GenericSessionDiagnosticsView, CommandError> {
    let session_id = parse_session_id(session_id)?;
    let (runtime, deck, codec, negotiated) = {
        let mut controller = state.controller.lock().await;
        controller.reap_closed();
        let record = controller
            .sessions
            .get(&session_id)
            .ok_or_else(session_not_found)?;
        (
            Arc::clone(&record.runtime),
            record.deck.clone(),
            record.codec.clone(),
            record.negotiated.view(),
        )
    };
    let diagnostics = runtime.diagnostics().await.map_err(runtime_command_error)?;
    let spout = runtime
        .spout_status()
        .await
        .map_err(runtime_command_error)?;
    Ok(GenericSessionDiagnosticsView {
        session_id: session_id.as_str().to_owned(),
        deck: ExactPackageView::from(&deck),
        codec: ExactPackageView::from(&codec),
        negotiated,
        runtime: runtime.view().map_err(runtime_command_error)?,
        diagnostics,
        capture: runtime
            .cached_capture_status()
            .map_err(runtime_command_error)?,
        recording: runtime.recording_status(),
        spout,
    })
}

fn select_capture_output(
    app: &AppHandle,
    suggested: &str,
) -> Result<Option<PathBuf>, CommandError> {
    let selected = app
        .dialog()
        .file()
        .add_filter("Latent Cartridge", &["lc"])
        .set_file_name(suggested)
        .blocking_save_file();
    selected
        .map(|selected| {
            let path = selected.into_path().map_err(|_| {
                CommandError::new(
                    "capture.output_path_invalid",
                    "The native save dialog did not return a usable cartridge path.",
                )
            })?;
            validate_capture_output_path(path)
        })
        .transpose()
}

fn validate_capture_output_path(mut output: PathBuf) -> Result<PathBuf, CommandError> {
    if !output.is_absolute() {
        return Err(CommandError::new(
            "capture.output_path_invalid",
            "The native save dialog did not return an absolute cartridge path.",
        ));
    }
    match output.extension().and_then(|value| value.to_str()) {
        None => {
            output.set_extension("lc");
        }
        Some(extension) if extension.eq_ignore_ascii_case("lc") => {
            output.set_extension("lc");
        }
        Some(_) => {
            return Err(CommandError::new(
                "capture.output_path_invalid",
                "Latent captures must use the .lc extension.",
            ));
        }
    }
    if output.exists() {
        return Err(CommandError::new(
            "target.exists",
            "Latent capture never overwrites an existing cartridge.",
        ));
    }
    if !output.parent().is_some_and(Path::is_dir) {
        return Err(CommandError::new(
            "capture.output_path_invalid",
            "The selected latent capture directory is unavailable.",
        ));
    }
    Ok(output)
}

fn spawn_capture_pin_monitor(
    app: AppHandle,
    session_id: SessionId,
    token: OutputPinToken,
    runtime: Arc<GenericDeckRuntime>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            let terminal = match runtime.capture_status().await {
                Ok(capture) => capture_state_terminal(&capture.state),
                Err(_) => true,
            };
            if terminal || runtime.is_closed() {
                release_monitored_pin(&app, &session_id, &token).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
}

fn spawn_recording_pin_monitor(
    app: AppHandle,
    session_id: SessionId,
    token: OutputPinToken,
    runtime: Arc<GenericDeckRuntime>,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            if recording_state_terminal(runtime.recording_status().state) || runtime.is_closed() {
                release_monitored_pin(&app, &session_id, &token).await;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
}

async fn release_monitored_pin(app: &AppHandle, session_id: &SessionId, token: &OutputPinToken) {
    let state = app.state::<GenericDeckAppState>();
    let mut controller = state.controller.lock().await;
    controller.reap_closed();
    if controller
        .registry
        .broker
        .output_pin()
        .is_some_and(|active| active == token && active.session_id() == session_id)
    {
        let _ = controller.unpin(token);
    }
}

fn capture_state_terminal(state: &str) -> bool {
    matches!(state, "idle" | "finished" | "aborted" | "error")
}

const fn recording_state_terminal(state: RecorderState) -> bool {
    matches!(
        state,
        RecorderState::Idle
            | RecorderState::Finished
            | RecorderState::Cancelled
            | RecorderState::Failed
    )
}

fn recording_command_error(error: crate::decoded_recording::DecodedRecordingError) -> CommandError {
    CommandError::new(error.code(), error.message())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn deck_generic_preset_save(
    app: AppHandle,
    preset: DeckPresetDocument,
) -> Result<Option<PresetSaveView>, CommandError> {
    deck_preset_save(app, preset)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn deck_generic_preset_load(
    app: AppHandle,
) -> Result<Option<DeckPresetDocument>, CommandError> {
    deck_preset_load(app)
}

#[cfg(test)]
mod tests {
    use latentdeck_deck_runtime_contracts::{ContractId, PackageIdentity};
    use semver::Version;

    use super::*;

    mod protocol2_session_registry_e2e {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/protocol2_session_registry_e2e.rs"
        ));
    }

    fn session(number: usize) -> SessionId {
        SessionId::new(format!("session-{number}")).expect("session id")
    }

    fn warm(number: usize) -> WarmSession {
        WarmSession {
            session_id: session(number),
            worker_id: WorkerId::new(format!("worker-{number}")).expect("worker id"),
            deck: PackageIdentity::new(
                ContractId::new("org.example.deck").expect("deck id"),
                Version::parse("1.2.3").expect("deck version"),
            ),
            codec: PackageIdentity::new(
                ContractId::new("org.example.codec").expect("codec id"),
                Version::parse("2.3.4").expect("codec version"),
            ),
        }
    }

    #[test]
    fn ui_discovery_uses_any_exact_allowlisted_geometry_for_the_selected_device() {
        let deck: latentdeck_extension_manager::DeckPackManifest =
            serde_json::from_str(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../operators/builtin/d2/package/deck-pack.json"
            )))
            .expect("bundled D2 manifest");

        assert_eq!(deck.signal.geometry_allowlist.len(), 4);
        assert!(geometry_allowlist_supports_device(&deck, DeviceKind::Cuda));
        assert!(!geometry_allowlist_supports_device(&deck, DeviceKind::Cpu));
    }

    #[test]
    fn pending_starts_count_toward_the_four_session_capacity_without_eviction() {
        let mut registry = GenericSessionRegistry::default();
        for number in 1..=4 {
            registry.reserve(session(number)).expect("reservation");
        }

        let error = registry.reserve(session(5)).expect_err("fifth rejected");

        assert_eq!(error, BrokerError::SessionCapacityExceeded);
        assert_eq!(registry.pending.len(), 4);
        assert!(registry.broker.is_empty());
    }

    #[tokio::test]
    async fn stalled_foreground_actor_does_not_hold_the_global_controller_lock() {
        let controller = Arc::new(tokio::sync::Mutex::new(GenericDeckController::default()));
        let generation = controller
            .lock()
            .await
            .begin_lifecycle_transition()
            .expect("transition");
        let (release, stalled) = tokio::sync::oneshot::channel::<()>();
        let stalled_controller = Arc::clone(&controller);
        let actor = tokio::spawn(async move {
            let _ = stalled.await;
            stalled_controller
                .lock()
                .await
                .abort_lifecycle_transition(generation);
        });

        tokio::time::timeout(std::time::Duration::from_millis(100), async {
            let controller = controller.lock().await;
            let error = controller
                .ensure_lifecycle_idle()
                .expect_err("lease transition stays explicit");
            let value = serde_json::to_value(error).expect("serialize error");
            assert_eq!(value["code"], "session.lifecycle_busy");
        })
        .await
        .expect("another session can acquire controller state while actor stalls");

        release.send(()).expect("release stalled actor");
        actor.await.expect("actor task");
    }

    #[test]
    fn foreground_pin_and_worker_fault_apply_to_exact_committed_sessions() {
        let mut registry = GenericSessionRegistry::default();
        for number in 1..=3 {
            registry.reserve(session(number)).expect("reservation");
            registry.commit(warm(number)).expect("commit");
        }

        registry.switch_foreground(&session(1)).expect("foreground");
        let pin = registry
            .pin_foreground(&session(1), OutputPinKind::Capture)
            .expect("capture pin");
        assert_eq!(
            registry.switch_foreground(&session(2)).expect_err("pinned"),
            BrokerError::SessionOutputLeasePinned
        );

        let removed = registry
            .worker_fault(&WorkerId::new("worker-3").expect("worker"))
            .expect("background fault");
        assert_eq!(removed.session_id, session(3));
        assert!(registry.broker.contains_session(&session(1)));
        assert!(registry.broker.contains_session(&session(2)));
        assert_eq!(registry.broker.output_pin(), Some(&pin));
    }

    #[test]
    fn snapshot_auto_completion_is_reaped_before_the_next_foreground_switch() {
        let mut registry = GenericSessionRegistry::default();
        for number in 1..=2 {
            registry.reserve(session(number)).expect("reservation");
            registry.commit(warm(number)).expect("commit");
        }
        registry.switch_foreground(&session(1)).expect("foreground");
        registry
            .pin_foreground(&session(1), OutputPinKind::Capture)
            .expect("capture pin");

        assert!(
            registry.reap_terminal_output_pin(|token| { token.kind() == OutputPinKind::Capture })
        );
        registry
            .switch_foreground(&session(2))
            .expect("terminal snapshot no longer pins output");
    }

    #[test]
    fn asynchronous_recorder_failure_is_reaped_on_the_next_broker_operation() {
        let mut registry = GenericSessionRegistry::default();
        for number in 1..=2 {
            registry.reserve(session(number)).expect("reservation");
            registry.commit(warm(number)).expect("commit");
        }
        registry.switch_foreground(&session(1)).expect("foreground");
        registry
            .pin_foreground(&session(1), OutputPinKind::Mp4)
            .expect("recording pin");

        assert!(registry.reap_terminal_output_pin(|token| { token.kind() == OutputPinKind::Mp4 }));
        registry
            .switch_foreground(&session(2))
            .expect("failed recorder no longer pins output");
    }

    #[test]
    fn active_or_finalizing_output_work_remains_pinned() {
        let mut registry = GenericSessionRegistry::default();
        for number in 1..=2 {
            registry.reserve(session(number)).expect("reservation");
            registry.commit(warm(number)).expect("commit");
        }
        registry.switch_foreground(&session(1)).expect("foreground");
        registry
            .pin_foreground(&session(1), OutputPinKind::Capture)
            .expect("capture pin");

        assert!(!registry.reap_terminal_output_pin(|_| false));
        assert_eq!(
            registry.switch_foreground(&session(2)),
            Err(BrokerError::SessionOutputLeasePinned)
        );
    }

    #[test]
    fn negotiated_identity_is_exact_path_free_and_stable_across_warm_switches() {
        let identity = GenericNegotiatedIdentity {
            profile_key: GenericProfileKeyInput {
                codec_family: "synthetic".to_owned(),
                profile: "latent".to_owned(),
                profile_version: "1.2.3".to_owned(),
            },
            device: DeviceKind::Cuda,
            device_ordinal: 2,
            external_assets: vec![GenericSessionExternalAssetView {
                asset_id: "decoder".to_owned(),
                sha256: "ab".repeat(32),
                byte_length: 1_024,
            }],
        };

        let before = identity.view();
        let after = identity.view();
        assert_eq!(before, after);
        let wire = serde_json::to_value(after).expect("serialize negotiated identity");
        assert_eq!(wire["profileKey"]["codecFamily"], "synthetic");
        assert_eq!(wire["profileKey"]["profile"], "latent");
        assert_eq!(wire["profileKey"]["profileVersion"], "1.2.3");
        assert_eq!(wire["device"], "cuda");
        assert_eq!(wire["deviceOrdinal"], 2);
        assert_eq!(wire["externalAssets"][0]["assetId"], "decoder");
        assert_eq!(wire["externalAssets"][0]["sha256"], "ab".repeat(32));
        assert_eq!(wire["externalAssets"][0]["byteLength"], 1_024);
        assert!(!wire.to_string().contains("path"));
    }
}
