//! Trusted LD-Q4 launch inputs and the single-owner realtime host.
//!
//! The webview supplies only immutable Library cartridge identities and
//! bounded controls. Paths, worker launch, stream clock, causal generations,
//! decoded RGB transport, and native presentation remain owned here.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use latentdeck_cartridge::{
    profile::h3::ValidatedH3Profile,
    reader::{ValidationOptions, open_validated},
};
use latentdeck_control::{
    FiniteF64, MAX_Q4_SAFE_INTEGER, Q4Algorithm, Q4Controls, Q4InfluenceMode, Q4Mode,
    Q4ResetReason, Q4Roles, Q4Slot, Q4SourceStatus, Q4Status, Q4Transport, Q4Xs5Routing,
};
use latentdeck_core::{
    codec_pack::{
        ValidatedCodecPack, ValidatedExternalAsset, default_codec_pack_roots, discover_codec_packs,
        validate_external_asset,
    },
    signal_geometry::{SignalCompatibilityPolicy, SignalGeometry, check_signal_compatibility},
};
use latentdeck_library::ResolvedDeckSource;
#[cfg(not(target_os = "windows"))]
use latentdeck_native_output::NativeSpoutStatus;
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::library_state::{DeckSessionLease, LibraryImporter};

#[cfg(not(target_os = "windows"))]
use crate::q4_capture_host::Q4CaptureView;
#[cfg(not(target_os = "windows"))]
use latentdeck_control::Q4CaptureMode;

pub(crate) const Q4_OUTPUT_WINDOW_LABEL: &str = "latentdeck-q4-output";
const Q4_OUTPUT_WINDOW_TITLE: &str = "LatentDeck LD-Q4 Output";
const H3_PACK_ID: &str = "org.latentdeck.h3";
const H3_ASSET_ID: &str = "taeh3";
const Q4_DECK_ID: &str = "main-q4";
const Q4_OPERATOR_ID: &str = "org.latentdeck.builtin.ld_q4";
const Q4_OPERATOR_VERSION: &str = "0.1.0";
const INITIAL_GENERATION: u64 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Q4ControlsInput {
    algorithm: Q4Algorithm,
    interaction: f64,
    mode: Q4Mode,
    preserve: f64,
    influence_mode: Q4InfluenceMode,
    donor_weight_b: f64,
    donor_weight_c: f64,
    donor_weight_d: f64,
    triangle_x: f64,
    triangle_y: f64,
    xs5_routing: Q4Xs5Routing,
    temperature: f64,
    top_k: u8,
    sinkhorn_iterations: u8,
    chaos: f64,
}

impl Q4ControlsInput {
    pub(crate) fn into_wire(self) -> Result<Q4Controls, Q4RuntimeError> {
        let controls = Q4Controls {
            algorithm: self.algorithm,
            interaction: finite(self.interaction)?,
            mode: self.mode,
            preserve: finite(self.preserve)?,
            influence_mode: self.influence_mode,
            donor_weight_b: finite(self.donor_weight_b)?,
            donor_weight_c: finite(self.donor_weight_c)?,
            donor_weight_d: finite(self.donor_weight_d)?,
            triangle_x: finite(self.triangle_x)?,
            triangle_y: finite(self.triangle_y)?,
            xs5_routing: self.xs5_routing,
            temperature: finite(self.temperature)?,
            top_k: self.top_k,
            sinkhorn_iterations: self.sinkhorn_iterations,
            chaos: finite(self.chaos)?,
        };
        controls
            .validate()
            .map_err(|_| Q4RuntimeError::invalid_controls())?;
        Ok(controls)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Q4RolesInput {
    carrier: Q4Slot,
    donor_b: Q4Slot,
    donor_c: Q4Slot,
    donor_d: Q4Slot,
}

impl Q4RolesInput {
    pub(crate) fn into_wire(self) -> Result<Q4Roles, Q4RuntimeError> {
        let roles = Q4Roles {
            carrier: self.carrier,
            donor_b: self.donor_b,
            donor_c: self.donor_c,
            donor_d: self.donor_d,
        };
        roles
            .validate()
            .map_err(|_| Q4RuntimeError::invalid_roles())?;
        Ok(roles)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Exact eight-flag frontend transport schema.
pub(crate) struct Q4TransportInput {
    playing_a: bool,
    playing_b: bool,
    playing_c: bool,
    playing_d: bool,
    loop_a: bool,
    loop_b: bool,
    loop_c: bool,
    loop_d: bool,
}

impl From<Q4TransportInput> for Q4Transport {
    fn from(value: Q4TransportInput) -> Self {
        Self {
            playing_a: value.playing_a,
            playing_b: value.playing_b,
            playing_c: value.playing_c,
            playing_d: value.playing_d,
            loop_a: value.loop_a,
            loop_b: value.loop_b,
            loop_c: value.loop_c,
            loop_d: value.loop_d,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4ControlsView {
    algorithm: Q4Algorithm,
    interaction: f64,
    mode: Q4Mode,
    preserve: f64,
    influence_mode: Q4InfluenceMode,
    donor_weight_b: f64,
    donor_weight_c: f64,
    donor_weight_d: f64,
    triangle_x: f64,
    triangle_y: f64,
    xs5_routing: Q4Xs5Routing,
    temperature: f64,
    top_k: u8,
    sinkhorn_iterations: u8,
    chaos: f64,
}

impl From<&Q4Controls> for Q4ControlsView {
    fn from(value: &Q4Controls) -> Self {
        Self {
            algorithm: value.algorithm,
            interaction: value.interaction.get(),
            mode: value.mode,
            preserve: value.preserve.get(),
            influence_mode: value.influence_mode,
            donor_weight_b: value.donor_weight_b.get(),
            donor_weight_c: value.donor_weight_c.get(),
            donor_weight_d: value.donor_weight_d.get(),
            triangle_x: value.triangle_x.get(),
            triangle_y: value.triangle_y.get(),
            xs5_routing: value.xs5_routing,
            temperature: value.temperature.get(),
            top_k: value.top_k,
            sinkhorn_iterations: value.sinkhorn_iterations,
            chaos: value.chaos.get(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4RolesView {
    carrier: Q4Slot,
    donor_b: Q4Slot,
    donor_c: Q4Slot,
    donor_d: Q4Slot,
}

impl From<Q4Roles> for Q4RolesView {
    fn from(value: Q4Roles) -> Self {
        Self {
            carrier: value.carrier,
            donor_b: value.donor_b,
            donor_c: value.donor_c,
            donor_d: value.donor_d,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Exact eight-flag frontend transport schema.
pub(crate) struct Q4TransportView {
    playing_a: bool,
    playing_b: bool,
    playing_c: bool,
    playing_d: bool,
    loop_a: bool,
    loop_b: bool,
    loop_c: bool,
    loop_d: bool,
}

impl From<Q4Transport> for Q4TransportView {
    fn from(value: Q4Transport) -> Self {
        Self {
            playing_a: value.playing_a,
            playing_b: value.playing_b,
            playing_c: value.playing_c,
            playing_d: value.playing_d,
            loop_a: value.loop_a,
            loop_b: value.loop_b,
            loop_c: value.loop_c,
            loop_d: value.loop_d,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4SourceStatusView {
    cartridge_id: String,
    archive_sha256: String,
    latent_slot_count: String,
}

impl From<&Q4SourceStatus> for Q4SourceStatusView {
    fn from(value: &Q4SourceStatus) -> Self {
        Self {
            cartridge_id: value.cartridge_id.to_string(),
            archive_sha256: value.archive_sha256.clone(),
            latent_slot_count: value.latent_slot_count.to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Q4LoadedSourcesView {
    #[serde(rename = "sourceA")]
    a: Q4SourceStatusView,
    #[serde(rename = "sourceB")]
    b: Q4SourceStatusView,
    #[serde(rename = "sourceC")]
    c: Q4SourceStatusView,
    #[serde(rename = "sourceD")]
    d: Q4SourceStatusView,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4StatusView {
    pub(crate) loaded: bool,
    sources: Option<Q4LoadedSourcesView>,
    stream_generation: String,
    stream_sequence: String,
    playhead_a: u64,
    playhead_b: u64,
    playhead_c: u64,
    playhead_d: u64,
    roles: Q4RolesView,
    transport: Q4TransportView,
    controls: Q4ControlsView,
    seed: u64,
    pending_reset: bool,
    pending_reset_reasons: Vec<String>,
}

impl Default for Q4StatusView {
    fn default() -> Self {
        let controls = Q4Controls::default();
        Self {
            loaded: false,
            sources: None,
            stream_generation: "0".to_owned(),
            stream_sequence: "0".to_owned(),
            playhead_a: 0,
            playhead_b: 0,
            playhead_c: 0,
            playhead_d: 0,
            roles: Q4Roles::default().into(),
            transport: Q4Transport::default().into(),
            controls: (&controls).into(),
            seed: 0,
            pending_reset: false,
            pending_reset_reasons: Vec::new(),
        }
    }
}

impl Q4StatusView {
    fn from_status(status: &Q4Status) -> Self {
        Self {
            loaded: true,
            sources: Some(Q4LoadedSourcesView {
                a: (&status.source_a).into(),
                b: (&status.source_b).into(),
                c: (&status.source_c).into(),
                d: (&status.source_d).into(),
            }),
            stream_generation: status.stream_generation.to_string(),
            stream_sequence: status.stream_sequence.to_string(),
            playhead_a: status.playhead_a,
            playhead_b: status.playhead_b,
            playhead_c: status.playhead_c,
            playhead_d: status.playhead_d,
            roles: status.roles.into(),
            transport: status.transport.into(),
            controls: (&status.controls).into(),
            seed: status.seed,
            pending_reset: status.pending_reset,
            pending_reset_reasons: status
                .pending_reset_reasons
                .iter()
                .map(|reason| reset_reason_name(*reason).to_owned())
                .collect(),
        }
    }

    fn stopped_from(status: &Q4Status) -> Self {
        let mut view = Self::from_status(status);
        view.loaded = false;
        view.transport.playing_a = false;
        view.transport.playing_b = false;
        view.transport.playing_c = false;
        view.transport.playing_d = false;
        view.pending_reset = false;
        view.pending_reset_reasons.clear();
        view
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4ControlsAckView {
    controls: Q4ControlsView,
    requires_causal_reset: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4RolesAckView {
    roles: Q4RolesView,
    requires_causal_reset: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4TransportAckView {
    transport: Q4TransportView,
    requires_causal_reset: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4SeedAckView {
    seed: u64,
    requires_causal_reset: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4ErrorEvent {
    pub(crate) code: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4DecoderView {
    asset_id: String,
    variant_id: String,
    sha256: String,
    byte_length: u64,
    source_url: String,
    license_label: String,
    license_url: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4BackendView {
    state: String,
    pack_id: Option<String>,
    pack_version: Option<String>,
    display_name: Option<String>,
    q4_entrypoint_available: bool,
    decoder: Option<Q4DecoderView>,
    detail: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Q4LaunchBackend {
    codec_pack: ValidatedCodecPack,
    decoder_asset: ValidatedExternalAsset,
}

pub(crate) struct Q4BackendController {
    codec_pack: Option<ValidatedCodecPack>,
    decoder_asset: Option<ValidatedExternalAsset>,
    discovery_fault: Option<Q4RuntimeError>,
}

impl Q4BackendController {
    pub(crate) fn discover_default() -> Self {
        Self::discover(&default_codec_pack_roots())
    }

    fn discover(roots: &[PathBuf]) -> Self {
        match discover_codec_packs(roots, latentdeck_core::product_version()) {
            Ok(packs) => Self {
                codec_pack: newest_h3_pack(packs),
                decoder_asset: None,
                discovery_fault: None,
            },
            Err(error) => Self {
                codec_pack: None,
                decoder_asset: None,
                discovery_fault: Some(Q4RuntimeError::new(
                    error.code,
                    "Installed Codec Pack validation failed.",
                    true,
                    false,
                )),
            },
        }
    }

    pub(crate) fn view(&self) -> Q4BackendView {
        if let Some(error) = &self.discovery_fault {
            return Q4BackendView {
                state: "error".to_owned(),
                pack_id: None,
                pack_version: None,
                display_name: None,
                q4_entrypoint_available: false,
                decoder: None,
                detail: Some(error.message.to_owned()),
            };
        }
        let Some(pack) = &self.codec_pack else {
            return Q4BackendView {
                state: "missing".to_owned(),
                pack_id: None,
                pack_version: None,
                display_name: None,
                q4_entrypoint_available: false,
                decoder: None,
                detail: Some(
                    "Install a compatible H3 Codec Pack with an LD-Q4 entrypoint.".to_owned(),
                ),
            };
        };
        let q4_available = pack.manifest.worker.q4_arguments.is_some();
        let decoder = self
            .decoder_asset
            .as_ref()
            .and_then(|asset| decoder_view(pack, asset));
        let (state, detail) = if !q4_available {
            (
                "incompatible",
                Some("The installed Codec Pack does not declare an LD-Q4 worker.".to_owned()),
            )
        } else if decoder.is_none() {
            (
                "decoder_missing",
                Some("Select a compatible TAEH3 decoder weight.".to_owned()),
            )
        } else {
            ("ready", None)
        };
        Q4BackendView {
            state: state.to_owned(),
            pack_id: Some(pack.manifest.pack_id.clone()),
            pack_version: Some(pack.manifest.pack_version.clone()),
            display_name: Some(pack.manifest.display_name.clone()),
            q4_entrypoint_available: q4_available,
            decoder,
            detail,
        }
    }

    pub(crate) fn pack_for_selection(&self) -> Result<ValidatedCodecPack, Q4RuntimeError> {
        if let Some(error) = &self.discovery_fault {
            return Err(error.clone());
        }
        self.codec_pack
            .clone()
            .ok_or_else(Q4RuntimeError::codec_missing)
    }

    pub(crate) fn accept_decoder(&mut self, asset: ValidatedExternalAsset) -> Q4BackendView {
        self.decoder_asset = Some(asset);
        self.view()
    }

    pub(crate) fn launch_backend(&self) -> Result<Q4LaunchBackend, Q4RuntimeError> {
        if let Some(error) = &self.discovery_fault {
            return Err(error.clone());
        }
        let codec_pack = self
            .codec_pack
            .clone()
            .ok_or_else(Q4RuntimeError::codec_missing)?;
        if codec_pack.manifest.worker.q4_arguments.is_none() {
            return Err(Q4RuntimeError::q4_entrypoint_missing());
        }
        let decoder_asset = self
            .decoder_asset
            .clone()
            .ok_or_else(Q4RuntimeError::decoder_missing)?;
        Ok(Q4LaunchBackend {
            codec_pack,
            decoder_asset,
        })
    }
}

pub(crate) fn validate_selected_decoder(
    pack: &ValidatedCodecPack,
    path: &Path,
) -> Result<ValidatedExternalAsset, Q4RuntimeError> {
    validate_external_asset(pack, H3_ASSET_ID, path).map_err(|error| {
        Q4RuntimeError::new(
            error.code,
            "The selected decoder weight is not an accepted Codec Pack asset.",
            true,
            false,
        )
    })
}

#[derive(Clone)]
struct TrustedQ4Source {
    path: PathBuf,
    cartridge_id: String,
    archive_sha256: String,
    profile: ValidatedH3Profile,
}

#[derive(Clone)]
pub(crate) struct Q4LaunchConfig {
    backend: Q4LaunchBackend,
    source_a: TrustedQ4Source,
    source_b: TrustedQ4Source,
    source_c: TrustedQ4Source,
    source_d: TrustedQ4Source,
    roles: Q4Roles,
    controls: Q4Controls,
    transport: Q4Transport,
    seed: u64,
    app_local_data: PathBuf,
    library_importer: LibraryImporter,
}

#[derive(Clone)]
pub(crate) struct Q4CaptureHostServices {
    app_local_data: PathBuf,
    library_importer: LibraryImporter,
}

impl Q4CaptureHostServices {
    pub(crate) fn new(app_local_data: PathBuf, library_importer: LibraryImporter) -> Self {
        Self {
            app_local_data,
            library_importer,
        }
    }
}

impl Q4LaunchConfig {
    #[allow(clippy::too_many_arguments)] // Closed four-source launch contract.
    pub(crate) fn build(
        backend: Q4LaunchBackend,
        source_a: &ResolvedDeckSource,
        source_b: &ResolvedDeckSource,
        source_c: &ResolvedDeckSource,
        source_d: &ResolvedDeckSource,
        roles: Q4Roles,
        controls: Q4Controls,
        transport: Q4Transport,
        seed: u64,
        capture_host: Q4CaptureHostServices,
    ) -> Result<Self, Q4RuntimeError> {
        if seed > MAX_Q4_SAFE_INTEGER {
            return Err(Q4RuntimeError::invalid_seed());
        }
        roles
            .validate()
            .map_err(|_| Q4RuntimeError::invalid_roles())?;
        controls
            .validate()
            .map_err(|_| Q4RuntimeError::invalid_controls())?;
        let source_a = inspect_source(source_a)?;
        let source_b = inspect_source(source_b)?;
        let source_c = inspect_source(source_c)?;
        let source_d = inspect_source(source_d)?;
        require_compatible_sources([
            &source_a.profile,
            &source_b.profile,
            &source_c.profile,
            &source_d.profile,
        ])?;
        Ok(Self {
            backend,
            source_a,
            source_b,
            source_c,
            source_d,
            roles,
            controls,
            transport,
            seed,
            app_local_data: capture_host.app_local_data,
            library_importer: capture_host.library_importer,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Q4RuntimeError {
    pub(crate) code: String,
    pub(crate) message: &'static str,
    pub(crate) recoverable: bool,
    terminal: bool,
}

impl Q4RuntimeError {
    fn new(code: &'static str, message: &'static str, recoverable: bool, terminal: bool) -> Self {
        Self {
            code: code.to_owned(),
            message,
            recoverable,
            terminal,
        }
    }

    fn owned(
        code: impl Into<String>,
        message: &'static str,
        recoverable: bool,
        terminal: bool,
    ) -> Self {
        Self {
            code: code.into(),
            message,
            recoverable,
            terminal,
        }
    }

    fn invalid_controls() -> Self {
        Self::owned(
            "deck.controls_invalid",
            "LD-Q4 controls are outside the supported finite bounds.",
            true,
            false,
        )
    }

    fn invalid_roles() -> Self {
        Self::owned(
            "deck.roles_invalid",
            "LD-Q4 Carrier and Donor B/C/D roles must be an exact A/B/C/D permutation.",
            true,
            false,
        )
    }

    pub(crate) fn invalid_seed() -> Self {
        Self::owned(
            "deck.seed_invalid",
            "LD-Q4 seed must be a non-negative exact u53 integer.",
            true,
            false,
        )
    }

    pub(crate) fn source_invalid() -> Self {
        Self::owned(
            "deck.source_invalid",
            "A selected LD-Q4 source failed full cartridge validation.",
            true,
            false,
        )
    }

    fn source_incompatible() -> Self {
        Self::owned(
            "deck.source_incompatible",
            "LD-Q4 sources differ in codec profile, latent grid, geometry, or timing contract.",
            true,
            false,
        )
    }

    fn codec_missing() -> Self {
        Self::owned(
            "codec.pack_missing",
            "Install a compatible H3 Codec Pack before opening LD-Q4.",
            true,
            false,
        )
    }

    fn q4_entrypoint_missing() -> Self {
        Self::owned(
            "codec.q4_entrypoint_missing",
            "The installed Codec Pack does not provide the trusted LD-Q4 worker entrypoint.",
            true,
            false,
        )
    }

    fn decoder_missing() -> Self {
        Self::owned(
            "codec.asset_missing",
            "Select a compatible TAEH3 decoder weight before opening LD-Q4.",
            true,
            false,
        )
    }

    fn runtime_unavailable() -> Self {
        Self::owned(
            "deck.runtime_unavailable",
            "The LD-Q4 runtime is unavailable; open the Deck again.",
            true,
            false,
        )
    }

    fn runtime_timeout() -> Self {
        Self::owned(
            "deck.runtime_timeout",
            "The LD-Q4 runtime did not answer within its bounded deadline.",
            true,
            false,
        )
    }

    fn runtime_cleanup() -> Self {
        Self::owned(
            "deck.runtime_cleanup_failed",
            "The LD-Q4 runtime stopped before its owned resources were cleaned up.",
            false,
            true,
        )
    }

    pub(crate) fn state_poisoned() -> Self {
        Self::owned(
            "deck.state_unavailable",
            "LD-Q4 state is unavailable; restart LatentDeck.",
            false,
            true,
        )
    }

    fn worker_start() -> Self {
        Self::owned(
            "worker.start_failed",
            "The isolated H3 LD-Q4 worker could not be started.",
            true,
            true,
        )
    }

    fn worker_protocol() -> Self {
        Self::owned(
            "worker.protocol_failed",
            "The isolated H3 LD-Q4 worker violated its typed contract.",
            true,
            true,
        )
    }

    fn worker_shutdown() -> Self {
        Self::owned(
            "worker.shutdown_failed",
            "The isolated H3 LD-Q4 worker could not be stopped safely.",
            false,
            true,
        )
    }

    fn worker_process_exited() -> Self {
        Self::owned(
            "worker.process_exited",
            "The isolated H3 LD-Q4 worker exited; open the Deck again to restart it.",
            true,
            true,
        )
    }

    fn session_rotation_required() -> Self {
        Self::owned(
            "worker.session_rotation_required",
            "The bounded worker session is near its message limit; open the Deck again.",
            true,
            true,
        )
    }

    fn codec_runtime() -> Self {
        Self::owned(
            "codec.runtime_incompatible",
            "The Codec Pack does not expose the required CUDA H3 adapter.",
            true,
            true,
        )
    }

    fn input_contract() -> Self {
        Self::owned(
            "deck.input_contract_invalid",
            "Trusted LD-Q4 inputs cannot be represented by Worker Protocol 1.",
            true,
            true,
        )
    }

    fn ring() -> Self {
        Self::owned(
            "ring.runtime_failed",
            "The bounded decoded-frame transport failed validation.",
            true,
            true,
        )
    }

    fn reset() -> Self {
        Self::owned(
            "deck.reset_failed",
            "The causal decoder reset handshake failed.",
            true,
            true,
        )
    }

    fn capture_host(error: crate::q4_capture_host::Q4CaptureHostError) -> Self {
        Self::owned(error.code, error.message, true, false)
    }

    fn capture_finalize() -> Self {
        Self::owned(
            "capture.finalize_failed",
            "The Q4 capture could not be validated, saved, and imported safely.",
            true,
            false,
        )
    }

    fn output(code: &'static str) -> Self {
        Self::owned(
            code,
            "Native DX12 output failed and LD-Q4 was stopped.",
            true,
            true,
        )
    }

    #[cfg(not(target_os = "windows"))]
    fn unsupported() -> Self {
        Self::owned(
            "output.platform_unsupported",
            "LD-Q4 native realtime output requires Windows and DirectX 12.",
            false,
            false,
        )
    }

    pub(crate) fn event(&self) -> Q4ErrorEvent {
        Q4ErrorEvent {
            code: self.code.clone(),
            detail: self.message.to_owned(),
        }
    }
}

impl fmt::Display for Q4RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for Q4RuntimeError {}

fn finite(value: f64) -> Result<FiniteF64, Q4RuntimeError> {
    FiniteF64::new(value).ok_or_else(Q4RuntimeError::invalid_controls)
}

fn newest_h3_pack(packs: Vec<ValidatedCodecPack>) -> Option<ValidatedCodecPack> {
    packs
        .into_iter()
        .filter(|pack| pack.manifest.pack_id == H3_PACK_ID)
        .max_by(|left, right| {
            let left = Version::parse(&left.manifest.pack_version)
                .expect("validated Codec Pack version is canonical SemVer");
            let right = Version::parse(&right.manifest.pack_version)
                .expect("validated Codec Pack version is canonical SemVer");
            left.cmp(&right)
        })
}

fn decoder_view(
    pack: &ValidatedCodecPack,
    asset: &ValidatedExternalAsset,
) -> Option<Q4DecoderView> {
    let descriptor = pack
        .manifest
        .external_assets
        .iter()
        .find(|candidate| candidate.asset_id == asset.asset_id)?;
    let variant = descriptor
        .accepted_variants
        .iter()
        .find(|candidate| candidate.variant_id == asset.variant_id)?;
    Some(Q4DecoderView {
        asset_id: asset.asset_id.clone(),
        variant_id: asset.variant_id.clone(),
        sha256: asset.sha256.clone(),
        byte_length: asset.byte_length,
        source_url: variant.source_url.clone(),
        license_label: variant.license_label.clone(),
        license_url: variant.license_url.clone(),
    })
}

fn inspect_source(resolved: &ResolvedDeckSource) -> Result<TrustedQ4Source, Q4RuntimeError> {
    let validated = open_validated(resolved.path(), &ValidationOptions::default())
        .map_err(|_| Q4RuntimeError::source_invalid())?;
    let identity = resolved.identity();
    if validated.manifest().cartridge_id.0 != identity.cartridge_id()
        || validated.receipt().archive_sha256.to_string() != identity.archive_sha256().as_str()
    {
        return Err(Q4RuntimeError::source_invalid());
    }
    Ok(TrustedQ4Source {
        path: resolved.path().to_path_buf(),
        cartridge_id: identity.cartridge_id().to_owned(),
        archive_sha256: identity.archive_sha256().as_str().to_owned(),
        profile: validated.h3_profile().clone(),
    })
}

fn require_compatible_sources(sources: [&ValidatedH3Profile; 4]) -> Result<(), Q4RuntimeError> {
    let reference = SignalGeometry::from_h3(sources[0]);
    let candidates = sources[1..]
        .iter()
        .map(|source| SignalGeometry::from_h3(source))
        .collect::<Vec<_>>();
    let report = check_signal_compatibility(
        SignalCompatibilityPolicy::SpatialSynthesis,
        &reference,
        &candidates,
    );
    if !report.compatible {
        return Err(Q4RuntimeError::source_incompatible());
    }
    Ok(())
}

const fn reset_reason_name(reason: Q4ResetReason) -> &'static str {
    match reason {
        Q4ResetReason::SlotALoop => "slot_a.loop",
        Q4ResetReason::SlotBLoop => "slot_b.loop",
        Q4ResetReason::SlotCLoop => "slot_c.loop",
        Q4ResetReason::SlotDLoop => "slot_d.loop",
        Q4ResetReason::TransportRestart => "transport.restart",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CausalResetPlan {
    current_generation: u64,
    new_generation: u64,
    reasons: Vec<Q4ResetReason>,
}

impl CausalResetPlan {
    fn from_barrier(
        expected_generation: u64,
        barrier_generation: u64,
        minimum_new_generation: u64,
        reasons: &[Q4ResetReason],
    ) -> Result<Self, Q4RuntimeError> {
        if barrier_generation != expected_generation
            || minimum_new_generation <= barrier_generation
            || reasons.is_empty()
            || reasons.len() > 5
        {
            return Err(Q4RuntimeError::reset());
        }
        Ok(Self {
            current_generation: barrier_generation,
            new_generation: minimum_new_generation,
            reasons: reasons.to_vec(),
        })
    }

    fn validate_ack(
        &self,
        generation: u64,
        reasons: &[Q4ResetReason],
        causal_state_cleared: bool,
    ) -> Result<(), Q4RuntimeError> {
        if generation != self.new_generation
            || reasons != self.reasons
            || !causal_state_cleared
            || self.new_generation <= self.current_generation
        {
            return Err(Q4RuntimeError::reset());
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::{
        future::pending,
        sync::atomic::{AtomicBool, Ordering},
    };

    use latentdeck_control::{
        Ack, BoundedVec, CodecInspection, CodecLoad, Command, EmptyPayload, ErrorCode,
        ExternalAssetBinding, MAX_CONTROL_FRAME_BYTES, ProfileRef, Q4CaptureMode, Q4CaptureParent,
        Q4CaptureStart, Q4CaptureState, Q4CaptureStatus, Q4CaptureStatusRequest, Q4CaptureStop,
        Q4ControlsSet, Q4Load, Q4ProcessSlot, Q4ProcessSlotAck, Q4Reset, Q4Restart, Q4RolesSet,
        Q4SeedSet, Q4SourceBinding, Q4TransportSet, RingBind, SessionConfigure, ShutdownReason,
        WORKER_PROTOCOL_VERSION, WireUuid,
    };
    use latentdeck_core::{
        worker_client::{WorkerClient, WorkerClientError},
        worker_supervisor::{ValidatedWorkerLaunch, spawn_worker},
    };
    use latentdeck_gpu::{
        ring::{ReadStatus, RingDescriptor, RingState},
        windows_ring::{WindowsRgbRingConsumer, WindowsRgbRingOwner},
    };
    use latentdeck_native_output::{
        NativeOutput, NativeOutputConfig, NativeOutputError, NativeSpoutStatus, PresentOutcome,
        ResizeOutcome,
    };
    use serde::Deserialize as _;
    use tauri::{Emitter as _, async_runtime::JoinHandle as TauriJoinHandle};
    use tokio::{
        sync::{mpsc, oneshot, watch},
        task::JoinHandle,
        time::{Instant, sleep_until, timeout},
    };

    use crate::q4_capture_host::{
        APP_Q4_CAPTURE_MAX_LATENT_SLOTS, APP_Q4_CAPTURE_MAX_VISUAL_BYTES, Q4CaptureCoordinator,
        Q4CaptureHostError, Q4CaptureSpoolBinding, Q4CaptureView, Q4FinalizedCapture,
        Q4StructuralCarrierEvidence, finalize_q4_capture, validate_q4_output_path,
    };

    use super::{
        AppHandle, Arc, CausalResetPlan, DeckSessionLease, Duration, INITIAL_GENERATION,
        LibraryImporter, MAX_Q4_SAFE_INTEGER, Mutex, Path, PathBuf, Q4_DECK_ID, Q4_OPERATOR_ID,
        Q4_OPERATOR_VERSION, Q4_OUTPUT_WINDOW_LABEL, Q4_OUTPUT_WINDOW_TITLE, Q4Controls,
        Q4ControlsAckView, Q4LaunchBackend, Q4LaunchConfig, Q4ResetReason, Q4Roles, Q4RolesAckView,
        Q4RuntimeError, Q4SeedAckView, Q4Slot, Q4Status, Q4StatusView, Q4Transport,
        Q4TransportAckView, TrustedQ4Source, ValidatedCodecPack,
    };

    const CHANNEL_CAPACITY: usize = 8;
    const ACTOR_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
    const SCHEDULER_POLL: Duration = Duration::from_millis(2);
    const MAX_FRAMES_PER_Q4_SLOT: u32 = 4;
    // Preserve space for the final authenticated reply and orderly shutdown.
    // Reopening the Deck is the explicit session-rotation boundary in 0.1.
    const SESSION_SHUTDOWN_MESSAGE_RESERVE: usize = 1_024;
    const CODEC_FAMILY: &str = "minimax_h3";
    const PROFILE_ID: &str = "h3_av_latent";
    const PROFILE_VERSION: &str = "0.1.0";

    pub(crate) struct Q4Runtime {
        sender: mpsc::Sender<RuntimeCommand>,
        closed: Arc<AtomicBool>,
        cleanup_complete: watch::Receiver<bool>,
        _task: TauriJoinHandle<()>,
    }

    impl Q4Runtime {
        pub(crate) async fn start(
            app: AppHandle,
            shared_status: Arc<Mutex<Q4StatusView>>,
            shared_capture_status: Arc<Mutex<Q4CaptureView>>,
            config: Q4LaunchConfig,
            deck_session: DeckSessionLease,
        ) -> Result<Self, Q4RuntimeError> {
            let launch = ValidatedWorkerLaunch::from_codec_pack_q4(&config.backend.codec_pack)
                .map_err(|_| Q4RuntimeError::q4_entrypoint_missing())?;
            let pending = spawn_worker(launch)
                .await
                .map_err(|_| Q4RuntimeError::worker_start())?;
            let session = pending
                .connect()
                .await
                .map_err(|_| Q4RuntimeError::worker_start())?;
            let mut client = WorkerClient::new(session);
            let initialized = initialize_session(&app, &config, &mut client).await;
            let InitializedSession {
                status,
                owner,
                consumer,
                output,
            } = match initialized {
                Ok(value) => value,
                Err(error) => {
                    let _ = stop_worker(&mut client, ShutdownReason::Recovery).await;
                    return Err(error);
                }
            };

            let frame_clock = match FrameClock::new(
                config
                    .source_a
                    .profile
                    .compatibility_key
                    .frame_rate
                    .numerator,
                config
                    .source_a
                    .profile
                    .compatibility_key
                    .frame_rate
                    .denominator,
            ) {
                Ok(value) => value,
                Err(error) => {
                    cleanup_pre_actor_start(output, &mut client).await;
                    return Err(error);
                }
            };
            let view = Q4StatusView::from_status(&status);
            if let Err(error) = replace_shared_status(&shared_status, view.clone()) {
                cleanup_pre_actor_start(output, &mut client).await;
                return Err(error);
            }
            let _ = app.emit("deck-q4-status", view);
            let capture_view = Q4CaptureView::default();
            if let Err(error) =
                replace_shared_capture_status(&shared_capture_status, capture_view.clone())
            {
                cleanup_pre_actor_start(output, &mut client).await;
                return Err(error);
            }
            let _ = app.emit("deck-q4-capture", capture_view);

            let closed = Arc::new(AtomicBool::new(false));
            let (sender, receiver) = mpsc::channel(CHANNEL_CAPACITY);
            let actor = RuntimeActor {
                app,
                client,
                owner,
                consumer,
                output,
                status,
                shared_status,
                shared_capture_status,
                deck_session,
                closed: Arc::clone(&closed),
                pending_frame: None,
                presented_sequence: 0,
                frame_clock,
                sources: [
                    config.source_a,
                    config.source_b,
                    config.source_c,
                    config.source_d,
                ],
                app_local_data: config.app_local_data,
                library_importer: config.library_importer,
                capture: None,
                capture_finalizer: None,
                capture_coordinator: Q4CaptureCoordinator::default(),
            };
            let (cleanup_sender, cleanup_complete) = watch::channel(false);
            let task = tauri::async_runtime::spawn(async move {
                actor.run(receiver).await;
                cleanup_sender.send_replace(true);
            });
            Ok(Self {
                sender,
                closed,
                cleanup_complete,
                _task: task,
            })
        }

        pub(crate) async fn controls_set(
            &self,
            controls: Q4Controls,
        ) -> Result<Q4ControlsAckView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::ControlsSet { controls, reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn roles_set(
            &self,
            roles: Q4Roles,
        ) -> Result<Q4RolesAckView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::RolesSet { roles, reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn transport_set(
            &self,
            transport: Q4Transport,
        ) -> Result<Q4TransportAckView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::TransportSet { transport, reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn seed_set(&self, seed: u64) -> Result<Q4SeedAckView, Q4RuntimeError> {
            self.ensure_open()?;
            if seed > MAX_Q4_SAFE_INTEGER {
                return Err(Q4RuntimeError::invalid_seed());
            }
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::SeedSet { seed, reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn restart(&self) -> Result<Q4StatusView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::Restart { reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn capture_start(
            &self,
            mode: Q4CaptureMode,
            output: PathBuf,
        ) -> Result<Q4CaptureView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::CaptureStart {
                    mode,
                    output,
                    reply,
                },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn capture_stop(&self) -> Result<Q4CaptureView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::CaptureStop { reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn capture_status(&self) -> Result<Q4CaptureView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::CaptureStatus { reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn status(&self) -> Result<Q4StatusView, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::Status { reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn resize(
            &self,
            width: u32,
            height: u32,
        ) -> Result<ResizeOutcome, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::Resize {
                    width,
                    height,
                    reply,
                },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn toggle_fullscreen(&self) -> Result<bool, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::ToggleFullscreen { reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn spout_status(&self) -> Result<NativeSpoutStatus, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::SpoutStatus { reply },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn configure_spout(
            &self,
            name: Option<String>,
            enabled: Option<bool>,
        ) -> Result<NativeSpoutStatus, Q4RuntimeError> {
            self.ensure_open()?;
            let (reply, receiver) = oneshot::channel();
            send_bounded(
                &self.sender,
                RuntimeCommand::ConfigureSpout {
                    name,
                    enabled,
                    reply,
                },
                ACTOR_REPLY_TIMEOUT,
            )
            .await?;
            receive_owned(receiver).await?
        }

        pub(crate) async fn shutdown(&self) -> Result<(), Q4RuntimeError> {
            let command_result = if self.closed.load(Ordering::Acquire) {
                Ok(())
            } else {
                let (reply, receiver) = oneshot::channel();
                // Shutdown is an ownership barrier. A full bounded command
                // queue delays it; it never abandons actor-owned cleanup.
                match self.sender.send(RuntimeCommand::Shutdown { reply }).await {
                    Ok(()) => match receiver.await {
                        Ok(result) => result,
                        Err(_) => Err(Q4RuntimeError::runtime_unavailable()),
                    },
                    Err(_) => Err(Q4RuntimeError::runtime_unavailable()),
                }
            };
            let cleanup_result = wait_for_actor_cleanup(self.cleanup_complete.clone()).await;
            self.closed.store(true, Ordering::Release);
            cleanup_result?;
            command_result
        }

        fn ensure_open(&self) -> Result<(), Q4RuntimeError> {
            if self.closed.load(Ordering::Acquire) {
                Err(Q4RuntimeError::runtime_unavailable())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for Q4Runtime {
        fn drop(&mut self) {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            let (reply, _receiver) = oneshot::channel();
            // Do not pre-close the actor. If the queue is full, dropping the
            // last sender still disconnects it and transfers stop ownership.
            let _ = self.sender.try_send(RuntimeCommand::Shutdown { reply });
        }
    }

    struct InitializedSession {
        status: Q4Status,
        owner: WindowsRgbRingOwner,
        consumer: WindowsRgbRingConsumer,
        output: NativeOutput,
    }

    async fn initialize_session(
        app: &AppHandle,
        config: &Q4LaunchConfig,
        client: &mut WorkerClient,
    ) -> Result<InitializedSession, Q4RuntimeError> {
        configure_session(client).await?;
        let profile = h3_profile();
        let inspection = inspect_codec(client).await?;
        validate_inspection(&inspection, &config.backend.codec_pack, &profile)?;
        load_codec(client, &config.backend, &profile).await?;
        let status = load_deck(client, config).await?;
        validate_loaded_status(&status, config)?;

        // All four sources already passed the exact compatibility gate, so A
        // is only a geometry representative; it is not an implicit carrier.
        let width = config.source_a.profile.visual.decoded_width;
        let height = config.source_a.profile.visual.decoded_height;
        let descriptor = RingDescriptor::new(width, height, INITIAL_GENERATION)
            .map_err(|_| Q4RuntimeError::ring())?;
        let owner = WindowsRgbRingOwner::create(descriptor).map_err(|_| Q4RuntimeError::ring())?;
        let consumer = owner.open_consumer().map_err(|_| Q4RuntimeError::ring())?;
        let owner = bind_ring(client, owner).await?;
        let output = NativeOutput::new(
            app,
            NativeOutputConfig::new(
                width,
                height,
                Q4_OUTPUT_WINDOW_LABEL,
                Q4_OUTPUT_WINDOW_TITLE,
            ),
        )
        .await
        .map_err(|error| Q4RuntimeError::output(error.code()))?;
        if output.frame_dimensions() != (width, height) {
            let error = Q4RuntimeError::output("output.contract_invalid");
            let _ = destroy_output(&output);
            return Err(error);
        }
        if let Err(error) = output.show() {
            let error = Q4RuntimeError::output(error.code());
            let _ = destroy_output(&output);
            return Err(error);
        }
        Ok(InitializedSession {
            status,
            owner,
            consumer,
            output,
        })
    }

    async fn cleanup_pre_actor_start(output: NativeOutput, client: &mut WorkerClient) {
        let _ = destroy_output(&output);
        drop(output);
        let _ = stop_worker(client, ShutdownReason::Recovery).await;
    }

    async fn configure_session(client: &mut WorkerClient) -> Result<(), Q4RuntimeError> {
        let request = SessionConfigure {
            selected_protocol_version: WORKER_PROTOCOL_VERSION,
            app_version: latentdeck_core::product_version().to_owned(),
            heartbeat_interval_ms: 1_000,
            heartbeat_hard_timeout_ms: 5_000,
            max_frame_bytes: MAX_CONTROL_FRAME_BYTES,
            max_inflight_decode_batches: 1,
        };
        let ack = client
            .call(Command::SessionConfigure(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::SessionConfigure(configured) = ack else {
            return Err(Q4RuntimeError::worker_protocol());
        };
        if configured.selected_protocol_version != request.selected_protocol_version
            || configured.heartbeat_interval_ms != request.heartbeat_interval_ms
            || configured.heartbeat_hard_timeout_ms != request.heartbeat_hard_timeout_ms
            || configured.max_frame_bytes != request.max_frame_bytes
            || configured.max_inflight_decode_batches != request.max_inflight_decode_batches
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn inspect_codec(client: &mut WorkerClient) -> Result<CodecInspection, Q4RuntimeError> {
        match client
            .call(Command::CodecInspect(EmptyPayload {}), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?
        {
            Ack::CodecInspect(inspection) => Ok(inspection),
            _ => Err(Q4RuntimeError::worker_protocol()),
        }
    }

    fn validate_inspection(
        inspection: &CodecInspection,
        pack: &ValidatedCodecPack,
        profile: &ProfileRef,
    ) -> Result<(), Q4RuntimeError> {
        if !inspection.cuda_available
            || !inspection.devices.iter().any(|device| device.ordinal == 0)
        {
            return Err(Q4RuntimeError::codec_runtime());
        }
        let adapter = inspection
            .adapters
            .iter()
            .find(|adapter| adapter.adapter_id == pack.manifest.adapter.adapter_id)
            .ok_or_else(Q4RuntimeError::codec_runtime)?;
        if adapter.adapter_version != pack.manifest.adapter.adapter_version
            || !adapter
                .profiles
                .iter()
                .any(|candidate| candidate == profile)
        {
            return Err(Q4RuntimeError::codec_runtime());
        }
        let declared = pack
            .manifest
            .compatibility
            .profiles
            .iter()
            .any(|candidate| {
                candidate.codec_family == profile.codec_family
                    && candidate.profile == profile.profile
                    && candidate
                        .profile_versions
                        .iter()
                        .any(|version| version == &profile.profile_version)
            });
        if !declared {
            return Err(Q4RuntimeError::codec_runtime());
        }
        Ok(())
    }

    async fn load_codec(
        client: &mut WorkerClient,
        backend: &Q4LaunchBackend,
        profile: &ProfileRef,
    ) -> Result<(), Q4RuntimeError> {
        let asset = ExternalAssetBinding {
            asset_id: backend.decoder_asset.asset_id.clone(),
            path: path_for_protocol(&backend.decoder_asset.path)?,
            sha256: backend.decoder_asset.sha256.clone(),
            byte_length: backend.decoder_asset.byte_length,
        };
        let request = CodecLoad {
            pack_id: backend.codec_pack.manifest.pack_id.clone(),
            pack_version: backend.codec_pack.manifest.pack_version.clone(),
            adapter_id: backend.codec_pack.manifest.adapter.adapter_id.clone(),
            profile: profile.clone(),
            device_ordinal: 0,
            assets: BoundedVec::try_from_vec(vec![asset])
                .map_err(|_| Q4RuntimeError::input_contract())?,
        };
        let ack = client
            .call(Command::CodecLoad(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::CodecLoad(loaded) = ack else {
            return Err(Q4RuntimeError::worker_protocol());
        };
        if loaded.pack_id != request.pack_id
            || loaded.pack_version != request.pack_version
            || loaded.adapter_id != request.adapter_id
            || loaded.adapter_version != backend.codec_pack.manifest.adapter.adapter_version
            || loaded.profile != request.profile
            || loaded.device.ordinal != request.device_ordinal
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn load_deck(
        client: &mut WorkerClient,
        config: &Q4LaunchConfig,
    ) -> Result<Q4Status, Q4RuntimeError> {
        client
            .deck_q4_load(
                Q4Load {
                    deck_id: Q4_DECK_ID.to_owned(),
                    operator_id: Q4_OPERATOR_ID.to_owned(),
                    operator_version: Q4_OPERATOR_VERSION.to_owned(),
                    source_a: source_binding(&config.source_a)?,
                    source_b: source_binding(&config.source_b)?,
                    source_c: source_binding(&config.source_c)?,
                    source_d: source_binding(&config.source_d)?,
                    roles: config.roles,
                    controls: config.controls.clone(),
                    transport: config.transport,
                    seed: config.seed,
                    stream_generation: INITIAL_GENERATION,
                },
                COMMAND_TIMEOUT,
            )
            .await
            .map_err(map_worker_error)
    }

    fn source_binding(source: &TrustedQ4Source) -> Result<Q4SourceBinding, Q4RuntimeError> {
        Ok(Q4SourceBinding {
            cartridge_path: path_for_protocol(&source.path)?,
            cartridge_id: parse_wire_uuid(&source.cartridge_id)?,
            expected_archive_sha256: source.archive_sha256.clone(),
        })
    }

    fn validate_loaded_status(
        status: &Q4Status,
        config: &Q4LaunchConfig,
    ) -> Result<(), Q4RuntimeError> {
        let expected_ids = [
            parse_wire_uuid(&config.source_a.cartridge_id)?,
            parse_wire_uuid(&config.source_b.cartridge_id)?,
            parse_wire_uuid(&config.source_c.cartridge_id)?,
            parse_wire_uuid(&config.source_d.cartridge_id)?,
        ];
        let expected = [
            (&status.source_a, &config.source_a, expected_ids[0]),
            (&status.source_b, &config.source_b, expected_ids[1]),
            (&status.source_c, &config.source_c, expected_ids[2]),
            (&status.source_d, &config.source_d, expected_ids[3]),
        ];
        if status.deck_id != Q4_DECK_ID
            || status.deck_revision == 0
            || status.operator_id != Q4_OPERATOR_ID
            || status.operator_version != Q4_OPERATOR_VERSION
            || status.stream_generation != INITIAL_GENERATION
            || status.stream_sequence != 0
            || status.playhead_a != 0
            || status.playhead_b != 0
            || status.playhead_c != 0
            || status.playhead_d != 0
            || status.roles != config.roles
            || status.transport != config.transport
            || status.controls != config.controls
            || status.seed != config.seed
            || status.pending_reset
            || !status.pending_reset_reasons.is_empty()
            || status.decoded_start_frame != 0
            || expected.into_iter().any(|(actual, trusted, expected_id)| {
                actual.cartridge_id != expected_id
                    || actual.archive_sha256 != trusted.archive_sha256
                    || actual.latent_slot_count != trusted.profile.visual.latent_slots
            })
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    fn validate_capture_start_status(
        capture: &Q4CaptureStatus,
        deck: &Q4Status,
        mode: Q4CaptureMode,
        capture_id: WireUuid,
    ) -> Result<(), Q4RuntimeError> {
        let expected_target = match mode {
            Q4CaptureMode::Snapshot => source_latent_slots(deck, deck.roles.carrier),
            Q4CaptureMode::LiveCapture => 0,
        };
        if capture.capture_id != capture_id
            || capture.mode != mode
            || capture.state != Q4CaptureState::AwaitingReset
            || capture.structural_carrier != deck.roles.carrier
            || capture.latent_slots != 0
            || capture.current_generation != Some(deck.stream_generation)
            || capture.minimum_new_generation != deck.stream_generation.checked_add(1)
            || capture.target_latent_slots != Some(expected_target)
            || capture.stream_generation.is_some()
            || capture.finalize_after_latent_slots.is_some()
            || capture.reason.is_some()
            || capture.receipt.is_some()
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    fn validate_active_capture_status(
        capture: &Q4CaptureStatus,
        deck: &Q4Status,
        mode: Q4CaptureMode,
        capture_id: WireUuid,
    ) -> Result<(), Q4RuntimeError> {
        if capture.capture_id != capture_id
            || capture.mode != mode
            || capture.state != Q4CaptureState::Capturing
            || capture.structural_carrier != deck.roles.carrier
            || capture.latent_slots != 0
            || capture.current_generation.is_some()
            || capture.minimum_new_generation.is_some()
            || capture.target_latent_slots.is_some()
            || capture.stream_generation != Some(deck.stream_generation)
            || capture.finalize_after_latent_slots.is_some()
            || capture.reason.is_some()
            || capture.receipt.is_some()
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    fn validate_capture_identity(
        status: &Q4CaptureStatus,
        active: &ActiveCapture,
    ) -> Result<(), Q4RuntimeError> {
        if status.capture_id != active.binding.capture_id()
            || status.mode != active.mode
            || status.structural_carrier != active.structural_parent.slot
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    const fn source_latent_slots(status: &Q4Status, slot: Q4Slot) -> u64 {
        match slot {
            Q4Slot::A => status.source_a.latent_slot_count,
            Q4Slot::B => status.source_b.latent_slot_count,
            Q4Slot::C => status.source_c.latent_slot_count,
            Q4Slot::D => status.source_d.latent_slot_count,
        }
    }

    async fn bind_ring(
        client: &mut WorkerClient,
        owner: WindowsRgbRingOwner,
    ) -> Result<WindowsRgbRingOwner, Q4RuntimeError> {
        ensure_zero_ring(owner.state().map_err(|_| Q4RuntimeError::ring())?)?;
        let binding = client
            .with_process_handle(|process| owner.duplicate_into(process))
            .map_err(|_| Q4RuntimeError::ring())?
            .map_err(|_| Q4RuntimeError::ring())?;
        let request = RingBind {
            layout_version: 1,
            mapping_handle: binding.mapping_handle(),
            mapping_bytes: binding.mapping_bytes(),
            frames_ready_event_handle: binding.frames_ready_event_handle(),
            ring_id: WireUuid::new_v4(),
        };
        let ack = client
            .call(Command::RingBind(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::RingBind(bound) = ack else {
            return Err(Q4RuntimeError::worker_protocol());
        };
        if bound.layout_version != request.layout_version
            || bound.mapping_bytes != request.mapping_bytes
            || bound.ring_id != request.ring_id
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        ensure_zero_ring(owner.state().map_err(|_| Q4RuntimeError::ring())?)?;
        Ok(owner)
    }

    struct RuntimeActor {
        app: AppHandle,
        client: WorkerClient,
        owner: WindowsRgbRingOwner,
        consumer: WindowsRgbRingConsumer,
        output: NativeOutput,
        status: Q4Status,
        shared_status: Arc<Mutex<Q4StatusView>>,
        shared_capture_status: Arc<Mutex<Q4CaptureView>>,
        deck_session: DeckSessionLease,
        closed: Arc<AtomicBool>,
        pending_frame: Option<latentdeck_gpu::ring::RgbaFrame>,
        presented_sequence: u64,
        frame_clock: FrameClock,
        sources: [TrustedQ4Source; 4],
        app_local_data: PathBuf,
        library_importer: LibraryImporter,
        capture: Option<ActiveCapture>,
        capture_finalizer: Option<CaptureFinalizer>,
        capture_coordinator: Q4CaptureCoordinator,
    }

    struct ActiveCapture {
        mode: Q4CaptureMode,
        binding: Q4CaptureSpoolBinding,
        output: PathBuf,
        structural_parent: Q4CaptureParent,
        structural_path: PathBuf,
    }

    impl ActiveCapture {
        fn cleanup(&self) {
            self.binding.cleanup();
        }
    }

    struct CaptureFinalizer {
        capture_id: WireUuid,
        task: JoinHandle<Result<Q4FinalizedCapture, Q4RuntimeError>>,
    }

    struct CaptureFinalizerCompletion {
        capture_id: WireUuid,
        result: Result<Q4FinalizedCapture, Q4RuntimeError>,
    }

    impl RuntimeActor {
        async fn run(mut self, mut receiver: mpsc::Receiver<RuntimeCommand>) {
            let mut next_schedule = Instant::now();
            loop {
                if !transport_active(self.status.transport) {
                    if self.wait_while_paused(&mut receiver).await {
                        break;
                    }
                    continue;
                }

                let frame_deadline = match self.frame_clock.next_deadline() {
                    Ok(value) => value,
                    Err(error) => {
                        self.fail(error).await;
                        break;
                    }
                };
                let Ok(state) = self.owner.state() else {
                    self.fail(Q4RuntimeError::ring()).await;
                    break;
                };
                let now = Instant::now();
                match due_work(
                    capture_finalizer_readiness(self.capture_finalizer.as_ref()),
                    now >= frame_deadline,
                    now >= next_schedule,
                    decode_watermark_allows(state, self.pending_frame.is_some()),
                ) {
                    DueWork::Present => {
                        self.frame_clock.advance_past(now);
                        if let Err(error) = self.present_once() {
                            self.fail(error).await;
                            break;
                        }
                        continue;
                    }
                    DueWork::FinalizeCapture => {
                        let completion = wait_capture_finalizer(&mut self.capture_finalizer).await;
                        if self.finish_capture_finalizer(completion).await {
                            break;
                        }
                        continue;
                    }
                    DueWork::Schedule => {
                        match receiver.try_recv() {
                            Ok(command) => {
                                if self.handle_command(command).await {
                                    break;
                                }
                                continue;
                            }
                            Err(mpsc::error::TryRecvError::Disconnected) => {
                                let _ = self.stop(ShutdownReason::ApplicationExit).await;
                                break;
                            }
                            Err(mpsc::error::TryRecvError::Empty) => {}
                        }
                        if let Err(error) = self.schedule_once().await {
                            self.fail(error).await;
                            break;
                        }
                        next_schedule = Instant::now() + SCHEDULER_POLL;
                        continue;
                    }
                    DueWork::Wait => {}
                }
                tokio::select! {
                    biased;
                    _exit = self.client.wait_for_exit() => break self.fail_worker_exit().await,
                    () = sleep_until(frame_deadline) => {
                        self.frame_clock.advance_past(Instant::now());
                        if let Err(error) = self.present_once() {
                            self.fail(error).await;
                            break;
                        }
                    }
                    command = receiver.recv() => {
                        let Some(command) = command else {
                            let _ = self.stop(ShutdownReason::ApplicationExit).await;
                            break;
                        };
                        if self.handle_command(command).await {
                            break;
                        }
                    }
                    completion = wait_capture_finalizer(&mut self.capture_finalizer) => {
                        if self.finish_capture_finalizer(completion).await {
                            break;
                        }
                    }
                    () = sleep_until(next_schedule), if decode_watermark_allows(state, self.pending_frame.is_some()) => {
                        if let Err(error) = self.schedule_once().await {
                            self.fail(error).await;
                            break;
                        }
                        next_schedule = Instant::now() + SCHEDULER_POLL;
                    }
                }
            }
        }

        async fn wait_while_paused(
            &mut self,
            receiver: &mut mpsc::Receiver<RuntimeCommand>,
        ) -> bool {
            tokio::select! {
                biased;
                _exit = self.client.wait_for_exit() => {
                    self.fail(Q4RuntimeError::worker_process_exited()).await;
                    true
                }
                completion = wait_capture_finalizer(&mut self.capture_finalizer) => {
                    self.finish_capture_finalizer(completion).await
                }
                command = receiver.recv() => {
                    let Some(command) = command else {
                        let _ = self.stop(ShutdownReason::ApplicationExit).await;
                        return true;
                    };
                    self.handle_command(command).await
                }
            }
        }

        async fn fail_worker_exit(&mut self) {
            self.fail(Q4RuntimeError::worker_process_exited()).await;
        }

        async fn finish_capture_finalizer(
            &mut self,
            completion: CaptureFinalizerCompletion,
        ) -> bool {
            self.capture_finalizer.take();
            if let Err(error) = self.accept_capture_finalizer(completion) {
                self.fail(error).await;
                true
            } else {
                false
            }
        }

        async fn handle_command(&mut self, command: RuntimeCommand) -> bool {
            if command.reply_is_closed() {
                return false;
            }
            match command {
                RuntimeCommand::ControlsSet { controls, reply } => {
                    let result = self.controls_set(controls).await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::RolesSet { roles, reply } => {
                    let result = self.roles_set(roles).await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::TransportSet { transport, reply } => {
                    let result = self.transport_set(transport).await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::SeedSet { seed, reply } => {
                    let result = self.seed_set(seed).await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::Restart { reply } => {
                    let result = self.restart().await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::CaptureStart {
                    mode,
                    output,
                    reply,
                } => self.capture_start_command(mode, output, reply).await,
                RuntimeCommand::CaptureStop { reply } => self.capture_stop_command(reply).await,
                RuntimeCommand::CaptureStatus { reply } => {
                    let result = self.capture_status_command().await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::Status { reply } => {
                    let result = self.refresh_status().await;
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::Resize {
                    width,
                    height,
                    reply,
                } => {
                    let result = self
                        .output
                        .resize(width, height)
                        .map_err(|error| Q4RuntimeError::output(error.code()));
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::ToggleFullscreen { reply } => {
                    let result = self
                        .output
                        .toggle_fullscreen()
                        .map_err(|error| Q4RuntimeError::output(error.code()));
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::SpoutStatus { reply } => {
                    let result = Ok(self.output.spout_status());
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::ConfigureSpout {
                    name,
                    enabled,
                    reply,
                } => {
                    // Spout is an optional native output. Control failures are
                    // represented in the sanitized status and never stop Q4.
                    if let Some(name) = name {
                        let _ = self.output.set_spout_name(name);
                    }
                    if let Some(enabled) = enabled {
                        let _ = self.output.set_spout_enabled(enabled);
                    }
                    let result = Ok(self.output.spout_status());
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::Shutdown { reply } => {
                    let result = self.stop(ShutdownReason::ApplicationExit).await;
                    let _ = reply.send(result);
                    true
                }
            }
        }

        async fn finish_command<T>(
            &mut self,
            result: Result<T, Q4RuntimeError>,
            reply: oneshot::Sender<Result<T, Q4RuntimeError>>,
        ) -> bool {
            let terminal = result.as_ref().err().is_some_and(|error| error.terminal);
            let failure = result.as_ref().err().cloned();
            let _ = reply.send(result);
            if terminal {
                if let Some(error) = failure {
                    self.fail(error).await;
                }
                true
            } else {
                false
            }
        }

        async fn controls_set(
            &mut self,
            controls: Q4Controls,
        ) -> Result<Q4ControlsAckView, Q4RuntimeError> {
            controls
                .validate()
                .map_err(|_| Q4RuntimeError::invalid_controls())?;
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_q4_controls_set(
                    Q4ControlsSet {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        controls: controls.clone(),
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            if ack.deck_id != self.status.deck_id
                || ack.deck_revision != self.status.deck_revision
                || ack.controls != controls
                || ack.requires_causal_reset
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            self.status.controls = ack.controls.clone();
            self.publish_status()?;
            Ok(Q4ControlsAckView {
                controls: (&ack.controls).into(),
                requires_causal_reset: false,
            })
        }

        async fn roles_set(&mut self, roles: Q4Roles) -> Result<Q4RolesAckView, Q4RuntimeError> {
            roles
                .validate()
                .map_err(|_| Q4RuntimeError::invalid_roles())?;
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_q4_roles_set(
                    Q4RolesSet {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        roles,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            if ack.deck_id != self.status.deck_id
                || ack.deck_revision != self.status.deck_revision
                || ack.roles != roles
                || ack.requires_causal_reset
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            self.status.roles = ack.roles;
            self.publish_status()?;
            Ok(Q4RolesAckView {
                roles: ack.roles.into(),
                requires_causal_reset: false,
            })
        }

        async fn transport_set(
            &mut self,
            transport: Q4Transport,
        ) -> Result<Q4TransportAckView, Q4RuntimeError> {
            let was_active = transport_active(self.status.transport);
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_q4_transport_set(
                    Q4TransportSet {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        transport,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            if ack.deck_id != self.status.deck_id
                || ack.deck_revision != self.status.deck_revision
                || ack.transport != transport
                || ack.requires_causal_reset
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            self.status.transport = ack.transport;
            if !was_active && transport_active(ack.transport) {
                self.frame_clock.restart();
            }
            self.publish_status()?;
            Ok(Q4TransportAckView {
                transport: ack.transport.into(),
                requires_causal_reset: false,
            })
        }

        async fn seed_set(&mut self, seed: u64) -> Result<Q4SeedAckView, Q4RuntimeError> {
            if seed > MAX_Q4_SAFE_INTEGER {
                return Err(Q4RuntimeError::invalid_seed());
            }
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_q4_seed_set(
                    Q4SeedSet {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        seed,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            if ack.deck_id != self.status.deck_id
                || ack.deck_revision != self.status.deck_revision
                || ack.seed != seed
                || ack.requires_causal_reset
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            self.status.seed = ack.seed;
            self.publish_status()?;
            Ok(Q4SeedAckView {
                seed: ack.seed,
                requires_causal_reset: false,
            })
        }

        async fn restart(&mut self) -> Result<Q4StatusView, Q4RuntimeError> {
            self.ensure_worker_session_budget()?;
            let barrier = self
                .client
                .deck_q4_restart(
                    Q4Restart {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            if barrier.deck_id != self.status.deck_id
                || barrier.deck_revision != self.status.deck_revision
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            self.status.pending_reset = true;
            self.status.pending_reset_reasons = barrier.reasons.clone();
            self.publish_status()?;
            let plan = CausalResetPlan::from_barrier(
                self.status.stream_generation,
                barrier.current_generation,
                barrier.minimum_new_generation,
                &barrier.reasons,
            )?;
            self.apply_reset(plan).await?;
            Ok(Q4StatusView::from_status(&self.status))
        }

        async fn capture_start_command(
            &mut self,
            mode: Q4CaptureMode,
            output: PathBuf,
            reply: oneshot::Sender<Result<Q4CaptureView, Q4RuntimeError>>,
        ) -> bool {
            let had_active_capture = self.capture.is_some()
                || self.capture_finalizer.is_some()
                || self.capture_coordinator.is_active();
            match self.begin_capture(mode, output).await {
                Ok(view) => {
                    let _ = reply.send(Ok(view));
                    false
                }
                Err(error) => {
                    // Once the host binding is installed, worker ownership is
                    // uncertain until shutdown. Retain it and route every
                    // subsequent start failure through stop_worker before
                    // cleaning the spool.
                    let terminal = capture_start_failure_requires_shutdown(
                        error.terminal,
                        had_active_capture,
                        self.capture.is_some(),
                    );
                    let _ = reply.send(Err(error.clone()));
                    if terminal {
                        self.fail(error).await;
                    } else {
                        self.publish_capture_error(&error);
                    }
                    terminal
                }
            }
        }

        async fn begin_capture(
            &mut self,
            mode: Q4CaptureMode,
            output: PathBuf,
        ) -> Result<Q4CaptureView, Q4RuntimeError> {
            let result = self.begin_capture_inner(mode, output).await;
            if result.is_err() && self.capture.is_none() && self.capture_coordinator.is_active() {
                let failed = self.capture_coordinator.fail();
                let _ = self.publish_capture_view(failed);
            }
            result
        }

        async fn begin_capture_inner(
            &mut self,
            mode: Q4CaptureMode,
            output: PathBuf,
        ) -> Result<Q4CaptureView, Q4RuntimeError> {
            if self.capture.is_some()
                || self.capture_finalizer.is_some()
                || self.capture_coordinator.is_active()
            {
                return Err(Q4RuntimeError::capture_host(Q4CaptureHostError {
                    code: "capture.already_active",
                    message: "Only one LD-Q4 capture may be active.",
                }));
            }
            let output = validate_q4_output_path(output).map_err(Q4RuntimeError::capture_host)?;
            self.ensure_worker_session_budget()?;
            let capture_id = WireUuid::new_v4();
            let binding = Q4CaptureSpoolBinding::create(&self.app_local_data, capture_id)
                .map_err(Q4RuntimeError::capture_host)?;
            let structural_slot = self.status.roles.carrier;
            let structural_source = self.trusted_source(structural_slot);
            let structural_parent = Q4CaptureParent {
                slot: structural_slot,
                cartridge_id: parse_wire_uuid(&structural_source.cartridge_id)?,
                archive_sha256: structural_source.archive_sha256.clone(),
            };
            let structural_path = structural_source.path.clone();
            self.capture_coordinator
                .begin(capture_id, mode)
                .map_err(Q4RuntimeError::capture_host)?;
            self.capture = Some(ActiveCapture {
                mode,
                binding,
                output,
                structural_parent,
                structural_path,
            });

            let temporary_root = path_for_protocol(
                self.capture
                    .as_ref()
                    .ok_or_else(Q4RuntimeError::worker_protocol)?
                    .binding
                    .root(),
            )?;
            let started = self
                .client
                .deck_q4_capture_start(
                    Q4CaptureStart {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        capture_id,
                        mode,
                        temporary_root,
                        max_latent_slots: APP_Q4_CAPTURE_MAX_LATENT_SLOTS,
                        max_visual_bytes: APP_Q4_CAPTURE_MAX_VISUAL_BYTES,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            validate_capture_start_status(&started, &self.status, mode, capture_id)?;
            let awaiting = self
                .capture_coordinator
                .observe(&started)
                .map_err(|_| Q4RuntimeError::worker_protocol())?;
            self.publish_capture_view(awaiting)?;

            let plan = CausalResetPlan::from_barrier(
                self.status.stream_generation,
                started
                    .current_generation
                    .ok_or_else(Q4RuntimeError::worker_protocol)?,
                started
                    .minimum_new_generation
                    .ok_or_else(Q4RuntimeError::worker_protocol)?,
                &[Q4ResetReason::TransportRestart],
            )?;
            self.apply_reset(plan).await?;
            self.ensure_worker_session_budget()?;
            let active_status = self
                .client
                .deck_q4_capture_status(
                    Q4CaptureStatusRequest {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        capture_id,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            validate_active_capture_status(&active_status, &self.status, mode, capture_id)?;
            let view = self
                .capture_coordinator
                .observe(&active_status)
                .map_err(|_| Q4RuntimeError::worker_protocol())?;
            self.publish_capture_view(view.clone())?;
            Ok(view)
        }

        async fn capture_stop_command(
            &mut self,
            reply: oneshot::Sender<Result<Q4CaptureView, Q4RuntimeError>>,
        ) -> bool {
            let Some(capture) = self.capture.as_ref() else {
                let error = Q4RuntimeError::capture_host(Q4CaptureHostError {
                    code: "capture.not_active",
                    message: "No Live Capture is active.",
                });
                let _ = reply.send(Err(error));
                return false;
            };
            if capture.mode != Q4CaptureMode::LiveCapture {
                let error = Q4RuntimeError::capture_host(Q4CaptureHostError {
                    code: "capture.mode_invalid",
                    message: "Snapshot capture stops automatically.",
                });
                let _ = reply.send(Err(error));
                return false;
            }
            let capture_id = capture.binding.capture_id();
            if let Err(error) = self.ensure_worker_session_budget() {
                let _ = reply.send(Err(error.clone()));
                self.fail(error).await;
                return true;
            }
            let stopped = self
                .client
                .deck_q4_capture_stop(
                    Q4CaptureStop {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        capture_id,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error);
            match stopped {
                Ok(status) => {
                    if !matches!(
                        status.state,
                        Q4CaptureState::StopArmed | Q4CaptureState::Finished
                    ) {
                        let error = Q4RuntimeError::worker_protocol();
                        let _ = reply.send(Err(error.clone()));
                        self.fail(error).await;
                        return true;
                    }
                    match self.observe_capture_status(status) {
                        Ok(view) => {
                            let _ = reply.send(Ok(view));
                            false
                        }
                        Err(error) if error.terminal => {
                            let _ = reply.send(Err(error.clone()));
                            self.fail(error).await;
                            true
                        }
                        Err(error) => {
                            let _ = reply.send(Err(error.clone()));
                            self.publish_capture_error(&error);
                            false
                        }
                    }
                }
                Err(error) => {
                    let _ = reply.send(Err(error.clone()));
                    if error.terminal {
                        self.fail(error).await;
                        true
                    } else {
                        self.publish_capture_error(&error);
                        false
                    }
                }
            }
        }

        async fn capture_status_command(&mut self) -> Result<Q4CaptureView, Q4RuntimeError> {
            let Some(capture) = self.capture.as_ref() else {
                return Ok(self.capture_coordinator.view());
            };
            self.ensure_worker_session_budget()?;
            let capture_id = capture.binding.capture_id();
            let status = self
                .client
                .deck_q4_capture_status(
                    Q4CaptureStatusRequest {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        capture_id,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            self.observe_capture_status(status)
        }

        fn observe_capture_status(
            &mut self,
            status: Q4CaptureStatus,
        ) -> Result<Q4CaptureView, Q4RuntimeError> {
            let capture = self
                .capture
                .as_ref()
                .ok_or_else(Q4RuntimeError::worker_protocol)?;
            validate_capture_identity(&status, capture)?;
            let view = self
                .capture_coordinator
                .observe(&status)
                .map_err(|_| Q4RuntimeError::worker_protocol())?;
            self.publish_capture_view(view.clone())?;
            match status.state {
                Q4CaptureState::Finished => {
                    self.start_capture_finalizer(status)?;
                    Ok(view)
                }
                Q4CaptureState::Aborted => {
                    let capture = self
                        .capture
                        .take()
                        .ok_or_else(Q4RuntimeError::worker_protocol)?;
                    capture.cleanup();
                    let error = Q4RuntimeError::capture_host(Q4CaptureHostError {
                        code: "capture.aborted",
                        message: "The worker aborted Q4 capture safely.",
                    });
                    self.publish_capture_error(&error);
                    Ok(view)
                }
                _ => Ok(view),
            }
        }

        fn start_capture_finalizer(
            &mut self,
            status: Q4CaptureStatus,
        ) -> Result<Q4CaptureView, Q4RuntimeError> {
            if self.capture_finalizer.is_some() {
                return Err(Q4RuntimeError::capture_host(Q4CaptureHostError {
                    code: "capture.already_finalizing",
                    message: "The LD-Q4 capture finalizer is already running.",
                }));
            }
            let capture = self
                .capture
                .take()
                .ok_or_else(Q4RuntimeError::worker_protocol)?;
            let capture_id = capture.binding.capture_id();
            let library_importer = self.library_importer.clone();
            let task = tokio::spawn(async move {
                let ActiveCapture {
                    binding,
                    output,
                    structural_parent,
                    structural_path,
                    ..
                } = capture;
                let evidence = tauri::async_runtime::spawn_blocking(move || {
                    Q4StructuralCarrierEvidence::inspect(&structural_parent, &structural_path)
                })
                .await
                .map_err(|_| Q4RuntimeError::capture_finalize())?
                .map_err(Q4RuntimeError::capture_host)?;
                finalize_q4_capture(binding, &status, output, &evidence, library_importer)
                    .await
                    .map_err(Q4RuntimeError::capture_host)
            });
            self.capture_finalizer = Some(CaptureFinalizer { capture_id, task });
            Ok(self.capture_coordinator.view())
        }

        fn accept_capture_finalizer(
            &mut self,
            completion: CaptureFinalizerCompletion,
        ) -> Result<(), Q4RuntimeError> {
            let capture_id = completion.capture_id.to_string();
            if self.capture_coordinator.view().capture_id.as_deref() != Some(&capture_id) {
                return Err(Q4RuntimeError::worker_protocol());
            }
            match completion.result {
                Ok(finalized) => {
                    let view = self
                        .capture_coordinator
                        .complete(&finalized)
                        .map_err(Q4RuntimeError::capture_host)?;
                    self.publish_capture_view(view)?;
                }
                Err(error) => {
                    let view = self.capture_coordinator.fail();
                    self.publish_capture_view(view)?;
                    self.publish_capture_error(&error);
                }
            }
            Ok(())
        }

        fn trusted_source(&self, slot: Q4Slot) -> &TrustedQ4Source {
            match slot {
                Q4Slot::A => &self.sources[0],
                Q4Slot::B => &self.sources[1],
                Q4Slot::C => &self.sources[2],
                Q4Slot::D => &self.sources[3],
            }
        }

        async fn refresh_status(&mut self) -> Result<Q4StatusView, Q4RuntimeError> {
            self.ensure_worker_session_budget()?;
            let status = self
                .client
                .deck_q4_status(COMMAND_TIMEOUT)
                .await
                .map_err(map_worker_error)?;
            if status.deck_id != self.status.deck_id
                || status.deck_revision != self.status.deck_revision
                || status.stream_generation != self.status.stream_generation
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            self.status = status;
            self.publish_status()
        }

        async fn schedule_once(&mut self) -> Result<(), Q4RuntimeError> {
            if !transport_active(self.status.transport) {
                return Ok(());
            }
            let before = self.owner.state().map_err(|_| Q4RuntimeError::ring())?;
            if !before.can_publish(MAX_FRAMES_PER_Q4_SLOT) {
                return Ok(());
            }
            self.ensure_worker_session_budget()?;
            let result = self
                .client
                .deck_q4_process_slot(
                    Q4ProcessSlot {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        stream_generation: self.status.stream_generation,
                    },
                    COMMAND_TIMEOUT,
                )
                .await;
            let ack = match result {
                Ok(value) => value,
                Err(WorkerClientError::Remote(remote))
                    if remote.code == ErrorCode::RingBackpressure
                        && remote.retryable
                        && !remote.fatal =>
                {
                    return Ok(());
                }
                Err(error) => return Err(map_worker_error(error)),
            };
            // Live Capture survives automatic source-loop resets. Adopt the
            // causal generation before polling the still host-owned capture;
            // only an explicit Stop or the bounded spool limit may finish it.
            self.handle_process_ack(ack, before).await?;
            if let Some(capture) = self.capture.as_ref() {
                self.ensure_worker_session_budget()?;
                let capture_id = capture.binding.capture_id();
                let status = self
                    .client
                    .deck_q4_capture_status(
                        Q4CaptureStatusRequest {
                            deck_id: self.status.deck_id.clone(),
                            deck_revision: self.status.deck_revision,
                            capture_id,
                        },
                        COMMAND_TIMEOUT,
                    )
                    .await
                    .map_err(map_worker_error)?;
                if let Err(error) = self.observe_capture_status(status) {
                    if error.terminal {
                        return Err(error);
                    }
                    self.publish_capture_error(&error);
                }
            }
            Ok(())
        }

        fn ensure_worker_session_budget(&self) -> Result<(), Q4RuntimeError> {
            if session_rotation_required(
                self.client.remaining_inbound_message_budget(),
                self.client.remaining_outbound_message_budget(),
            ) {
                Err(Q4RuntimeError::session_rotation_required())
            } else {
                Ok(())
            }
        }

        async fn handle_process_ack(
            &mut self,
            ack: Q4ProcessSlotAck,
            before: RingState,
        ) -> Result<(), Q4RuntimeError> {
            match ack {
                Q4ProcessSlotAck::DecodedSlot {
                    deck_id,
                    deck_revision,
                    stream_generation,
                    stream_sequence,
                    playhead_a,
                    playhead_b,
                    playhead_c,
                    playhead_d,
                    roles,
                    transport,
                    decoded_start_frame,
                    decoded_frame_count,
                    ring_first_sequence,
                    ring_last_sequence_exclusive,
                    provenance_json: _,
                } => {
                    let after = self.owner.state().map_err(|_| Q4RuntimeError::ring())?;
                    validate_decoded_slot(&DecodedSlotReceipt {
                        status: &self.status,
                        deck_id: &deck_id,
                        deck_revision,
                        stream_generation,
                        stream_sequence,
                        roles,
                        decoded_start_frame,
                        decoded_frame_count,
                        ring_first_sequence,
                        ring_last_sequence_exclusive,
                        before,
                        after,
                    })?;
                    adopt_decoded_progress(
                        &mut self.status,
                        stream_sequence,
                        playhead_a,
                        playhead_b,
                        playhead_c,
                        playhead_d,
                        roles,
                        transport,
                        decoded_start_frame,
                        decoded_frame_count,
                    )?;
                    self.status.pending_reset = false;
                    self.status.pending_reset_reasons = BoundedVec::default();
                    self.publish_status()?;
                    Ok(())
                }
                Q4ProcessSlotAck::ResetBarrier {
                    deck_id,
                    deck_revision,
                    current_generation,
                    minimum_new_generation,
                    reasons,
                } => {
                    if deck_id != self.status.deck_id || deck_revision != self.status.deck_revision
                    {
                        return Err(Q4RuntimeError::worker_protocol());
                    }
                    self.status.pending_reset = true;
                    self.status.pending_reset_reasons = reasons.clone();
                    self.publish_status()?;
                    let plan = CausalResetPlan::from_barrier(
                        self.status.stream_generation,
                        current_generation,
                        minimum_new_generation,
                        &reasons,
                    )?;
                    self.apply_reset(plan).await
                }
                Q4ProcessSlotAck::Paused {
                    deck_id,
                    deck_revision,
                    stream_generation,
                    playhead_a,
                    playhead_b,
                    playhead_c,
                    playhead_d,
                    roles,
                    transport,
                } => {
                    adopt_paused_progress(
                        &mut self.status,
                        &deck_id,
                        deck_revision,
                        stream_generation,
                        playhead_a,
                        playhead_b,
                        playhead_c,
                        playhead_d,
                        roles,
                        transport,
                    )?;
                    self.publish_status().map(|_| ())
                }
            }
        }

        async fn apply_reset(&mut self, plan: CausalResetPlan) -> Result<(), Q4RuntimeError> {
            // No stale decoded frame may survive the causal reset handshake.
            self.pending_frame = None;
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_q4_reset(
                    Q4Reset {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        new_stream_generation: plan.new_generation,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error)?;
            if ack.deck_id != self.status.deck_id || ack.deck_revision != self.status.deck_revision
            {
                return Err(Q4RuntimeError::worker_protocol());
            }
            plan.validate_ack(
                ack.stream_generation,
                &ack.reasons,
                ack.causal_state_cleared,
            )?;
            self.owner
                .adopt_generation(plan.new_generation)
                .map_err(|_| Q4RuntimeError::ring())?;
            self.consumer
                .adopt_generation(plan.new_generation)
                .map_err(|_| Q4RuntimeError::ring())?;
            ensure_zero_ring(self.owner.state().map_err(|_| Q4RuntimeError::ring())?)?;
            ensure_zero_ring(self.consumer.state().map_err(|_| Q4RuntimeError::ring())?)?;
            self.presented_sequence = 0;
            self.frame_clock.restart();
            self.status.stream_generation = ack.stream_generation;
            self.status.stream_sequence = 0;
            self.status.playhead_a = ack.playhead_a;
            self.status.playhead_b = ack.playhead_b;
            self.status.playhead_c = ack.playhead_c;
            self.status.playhead_d = ack.playhead_d;
            self.status.pending_reset = false;
            self.status.pending_reset_reasons = BoundedVec::default();
            self.status.decoded_start_frame = 0;
            self.publish_status().map(|_| ())
        }

        fn present_once(&mut self) -> Result<(), Q4RuntimeError> {
            if self.pending_frame.is_none() {
                match self
                    .consumer
                    .try_read()
                    .map_err(|_| Q4RuntimeError::ring())?
                {
                    ReadStatus::Frame(frame) => self.pending_frame = Some(frame),
                    ReadStatus::Empty => return Ok(()),
                }
            }
            let frame = self
                .pending_frame
                .as_ref()
                .ok_or_else(Q4RuntimeError::ring)?;
            let expected = self
                .presented_sequence
                .checked_add(1)
                .ok_or_else(Q4RuntimeError::ring)?;
            if frame.generation() != self.status.stream_generation || frame.sequence() != expected {
                return Err(Q4RuntimeError::ring());
            }
            let outcome = self
                .output
                .present_padded_rgba(
                    frame.width(),
                    frame.height(),
                    frame.row_stride(),
                    frame.padded_rgba(),
                )
                .map_err(|error| Q4RuntimeError::output(error.code()))?;
            if matches!(
                outcome,
                PresentOutcome::Presented | PresentOutcome::PresentedAndReconfigured
            ) {
                self.presented_sequence = expected;
                self.pending_frame = None;
            }
            Ok(())
        }

        fn publish_status(&self) -> Result<Q4StatusView, Q4RuntimeError> {
            let view = Q4StatusView::from_status(&self.status);
            replace_shared_status(&self.shared_status, view.clone())?;
            let _ = self.app.emit("deck-q4-status", view.clone());
            Ok(view)
        }

        fn publish_capture_view(
            &self,
            view: Q4CaptureView,
        ) -> Result<Q4CaptureView, Q4RuntimeError> {
            replace_shared_capture_status(&self.shared_capture_status, view.clone())?;
            let _ = self.app.emit("deck-q4-capture", view.clone());
            Ok(view)
        }

        fn publish_capture_error(&self, error: &Q4RuntimeError) {
            let _ = self.app.emit("deck-q4-capture-error", error.event());
        }

        async fn settle_capture_for_shutdown(&mut self, error: &Q4RuntimeError) {
            if self.capture_finalizer.is_some() {
                let completion = wait_capture_finalizer(&mut self.capture_finalizer).await;
                self.capture_finalizer.take();
                if let Err(finalizer_error) = self.accept_capture_finalizer(completion) {
                    self.abort_active_capture(&finalizer_error);
                    return;
                }
            }
            self.abort_active_capture(error);
        }

        fn abort_active_capture(&mut self, error: &Q4RuntimeError) {
            let mut aborted = false;
            if let Some(capture) = self.capture.take() {
                capture.cleanup();
                aborted = true;
            }
            if self.capture_coordinator.is_active() {
                let view = self.capture_coordinator.fail();
                let _ = self.publish_capture_view(view);
                aborted = true;
            }
            if aborted {
                self.publish_capture_error(error);
            }
        }

        async fn fail(&mut self, error: Q4RuntimeError) {
            if self.closed.swap(true, Ordering::AcqRel) {
                return;
            }
            self.deck_session.close();
            stop_transport(&mut self.status.transport);
            let stopped = Q4StatusView::stopped_from(&self.status);
            let _ = replace_shared_status(&self.shared_status, stopped.clone());
            let _ = self.app.emit("deck-q4-status", stopped);
            let _ = self.app.emit("deck-q4-error", error.event());
            let _ = destroy_output(&self.output);
            let _ = stop_worker(&mut self.client, ShutdownReason::Recovery).await;
            self.settle_capture_for_shutdown(&error).await;
        }

        async fn stop(&mut self, reason: ShutdownReason) -> Result<(), Q4RuntimeError> {
            if self.closed.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            self.deck_session.close();
            stop_transport(&mut self.status.transport);
            let output_result = destroy_output(&self.output);
            let worker_result = stop_worker(&mut self.client, reason).await;
            self.settle_capture_for_shutdown(&Q4RuntimeError::runtime_unavailable())
                .await;
            let stopped = Q4StatusView::stopped_from(&self.status);
            let _ = replace_shared_status(&self.shared_status, stopped.clone());
            let _ = self.app.emit("deck-q4-status", stopped);
            output_result?;
            worker_result
        }
    }

    enum RuntimeCommand {
        ControlsSet {
            controls: Q4Controls,
            reply: oneshot::Sender<Result<Q4ControlsAckView, Q4RuntimeError>>,
        },
        RolesSet {
            roles: Q4Roles,
            reply: oneshot::Sender<Result<Q4RolesAckView, Q4RuntimeError>>,
        },
        TransportSet {
            transport: Q4Transport,
            reply: oneshot::Sender<Result<Q4TransportAckView, Q4RuntimeError>>,
        },
        SeedSet {
            seed: u64,
            reply: oneshot::Sender<Result<Q4SeedAckView, Q4RuntimeError>>,
        },
        Restart {
            reply: oneshot::Sender<Result<Q4StatusView, Q4RuntimeError>>,
        },
        CaptureStart {
            mode: Q4CaptureMode,
            output: PathBuf,
            reply: oneshot::Sender<Result<Q4CaptureView, Q4RuntimeError>>,
        },
        CaptureStop {
            reply: oneshot::Sender<Result<Q4CaptureView, Q4RuntimeError>>,
        },
        CaptureStatus {
            reply: oneshot::Sender<Result<Q4CaptureView, Q4RuntimeError>>,
        },
        Status {
            reply: oneshot::Sender<Result<Q4StatusView, Q4RuntimeError>>,
        },
        Resize {
            width: u32,
            height: u32,
            reply: oneshot::Sender<Result<ResizeOutcome, Q4RuntimeError>>,
        },
        ToggleFullscreen {
            reply: oneshot::Sender<Result<bool, Q4RuntimeError>>,
        },
        SpoutStatus {
            reply: oneshot::Sender<Result<NativeSpoutStatus, Q4RuntimeError>>,
        },
        ConfigureSpout {
            name: Option<String>,
            enabled: Option<bool>,
            reply: oneshot::Sender<Result<NativeSpoutStatus, Q4RuntimeError>>,
        },
        Shutdown {
            reply: oneshot::Sender<Result<(), Q4RuntimeError>>,
        },
    }

    impl RuntimeCommand {
        fn reply_is_closed(&self) -> bool {
            match self {
                Self::ControlsSet { reply, .. } => reply.is_closed(),
                Self::RolesSet { reply, .. } => reply.is_closed(),
                Self::TransportSet { reply, .. } => reply.is_closed(),
                Self::SeedSet { reply, .. } => reply.is_closed(),
                Self::Restart { reply } | Self::Status { reply } => reply.is_closed(),
                Self::CaptureStart { reply, .. }
                | Self::CaptureStop { reply }
                | Self::CaptureStatus { reply } => reply.is_closed(),
                Self::Resize { reply, .. } => reply.is_closed(),
                Self::ToggleFullscreen { reply } => reply.is_closed(),
                Self::SpoutStatus { reply } | Self::ConfigureSpout { reply, .. } => {
                    reply.is_closed()
                }
                // Shutdown owns cleanup even if its original waiter vanished.
                Self::Shutdown { .. } => false,
            }
        }
    }

    async fn wait_capture_finalizer(
        finalizer: &mut Option<CaptureFinalizer>,
    ) -> CaptureFinalizerCompletion {
        let Some(finalizer) = finalizer.as_mut() else {
            return pending().await;
        };
        let capture_id = finalizer.capture_id;
        let result = (&mut finalizer.task)
            .await
            .map_err(|_| Q4RuntimeError::capture_finalize())
            .and_then(|result| result);
        CaptureFinalizerCompletion { capture_id, result }
    }

    async fn send_bounded<T>(
        sender: &mpsc::Sender<T>,
        command: T,
        deadline: Duration,
    ) -> Result<(), Q4RuntimeError> {
        timeout(deadline, sender.send(command))
            .await
            .map_err(|_| Q4RuntimeError::runtime_timeout())?
            .map_err(|_| Q4RuntimeError::runtime_unavailable())
    }

    async fn wait_for_actor_cleanup(
        mut completion: watch::Receiver<bool>,
    ) -> Result<(), Q4RuntimeError> {
        if *completion.borrow() {
            return Ok(());
        }
        completion
            .changed()
            .await
            .map_err(|_| Q4RuntimeError::runtime_cleanup())?;
        if *completion.borrow() {
            Ok(())
        } else {
            Err(Q4RuntimeError::runtime_cleanup())
        }
    }

    const fn session_rotation_required(
        inbound_remaining: usize,
        outbound_remaining: usize,
    ) -> bool {
        inbound_remaining <= SESSION_SHUTDOWN_MESSAGE_RESERVE
            || outbound_remaining <= SESSION_SHUTDOWN_MESSAGE_RESERVE
    }

    async fn receive_owned<T>(receiver: oneshot::Receiver<T>) -> Result<T, Q4RuntimeError> {
        receiver
            .await
            .map_err(|_| Q4RuntimeError::runtime_unavailable())
    }

    struct DecodedSlotReceipt<'a> {
        status: &'a Q4Status,
        deck_id: &'a str,
        deck_revision: u64,
        stream_generation: u64,
        stream_sequence: u64,
        roles: Q4Roles,
        decoded_start_frame: u64,
        decoded_frame_count: u32,
        ring_first_sequence: u64,
        ring_last_sequence_exclusive: u64,
        before: RingState,
        after: RingState,
    }

    fn validate_decoded_slot(receipt: &DecodedSlotReceipt<'_>) -> Result<(), Q4RuntimeError> {
        let expected_ring_first = receipt
            .before
            .producer_sequence()
            .checked_add(1)
            .ok_or_else(Q4RuntimeError::ring)?;
        let expected_ring_last_exclusive = expected_ring_first
            .checked_add(u64::from(receipt.decoded_frame_count))
            .ok_or_else(Q4RuntimeError::ring)?;
        let expected_occupancy = receipt
            .before
            .occupancy()
            .checked_add(receipt.decoded_frame_count)
            .ok_or_else(Q4RuntimeError::ring)?;
        let expected_available_capacity = receipt
            .before
            .available_capacity()
            .checked_sub(receipt.decoded_frame_count)
            .ok_or_else(Q4RuntimeError::ring)?;
        if receipt.deck_id != receipt.status.deck_id
            || receipt.deck_revision != receipt.status.deck_revision
            || receipt.stream_generation != receipt.status.stream_generation
            || receipt.stream_sequence != receipt.status.stream_sequence
            || receipt.roles != receipt.status.roles
            || receipt.decoded_start_frame != receipt.status.decoded_start_frame
            || !(1..=MAX_FRAMES_PER_Q4_SLOT).contains(&receipt.decoded_frame_count)
            || receipt.ring_first_sequence != expected_ring_first
            || receipt.ring_last_sequence_exclusive != expected_ring_last_exclusive
            || receipt.after.producer_sequence()
                != receipt
                    .ring_last_sequence_exclusive
                    .checked_sub(1)
                    .ok_or_else(Q4RuntimeError::ring)?
            || receipt.after.consumer_sequence() != receipt.before.consumer_sequence()
            || receipt.after.occupancy() != expected_occupancy
            || receipt.after.available_capacity() != expected_available_capacity
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the closed decoded-slot ack.
    fn adopt_decoded_progress(
        status: &mut Q4Status,
        stream_sequence: u64,
        playhead_a: u64,
        playhead_b: u64,
        playhead_c: u64,
        playhead_d: u64,
        roles: Q4Roles,
        transport: Q4Transport,
        decoded_start_frame: u64,
        decoded_frame_count: u32,
    ) -> Result<(), Q4RuntimeError> {
        if stream_sequence != status.stream_sequence
            || roles != status.roles
            || decoded_start_frame != status.decoded_start_frame
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        status.stream_sequence = stream_sequence
            .checked_add(1)
            .ok_or_else(Q4RuntimeError::worker_protocol)?;
        status.playhead_a = playhead_a;
        status.playhead_b = playhead_b;
        status.playhead_c = playhead_c;
        status.playhead_d = playhead_d;
        status.transport = transport;
        status.decoded_start_frame = decoded_start_frame
            .checked_add(u64::from(decoded_frame_count))
            .ok_or_else(Q4RuntimeError::worker_protocol)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the closed paused ack.
    fn adopt_paused_progress(
        status: &mut Q4Status,
        deck_id: &str,
        deck_revision: u64,
        stream_generation: u64,
        playhead_a: u64,
        playhead_b: u64,
        playhead_c: u64,
        playhead_d: u64,
        roles: Q4Roles,
        transport: Q4Transport,
    ) -> Result<(), Q4RuntimeError> {
        if deck_id != status.deck_id
            || deck_revision != status.deck_revision
            || stream_generation != status.stream_generation
            || roles != status.roles
            || transport_active(transport)
        {
            return Err(Q4RuntimeError::worker_protocol());
        }
        status.playhead_a = playhead_a;
        status.playhead_b = playhead_b;
        status.playhead_c = playhead_c;
        status.playhead_d = playhead_d;
        status.transport = transport;
        Ok(())
    }

    fn ensure_zero_ring(state: RingState) -> Result<(), Q4RuntimeError> {
        if state.producer_sequence() == 0
            && state.consumer_sequence() == 0
            && state.occupancy() == 0
        {
            Ok(())
        } else {
            Err(Q4RuntimeError::ring())
        }
    }

    fn destroy_output(output: &NativeOutput) -> Result<(), Q4RuntimeError> {
        let _ = output.hide();
        output
            .window()
            .destroy()
            .map_err(|_| Q4RuntimeError::output(NativeOutputError::WindowVisibility.code()))
    }

    async fn stop_worker(
        client: &mut WorkerClient,
        reason: ShutdownReason,
    ) -> Result<(), Q4RuntimeError> {
        if client
            .request_shutdown(reason, SHUTDOWN_TIMEOUT)
            .await
            .is_ok()
        {
            return Ok(());
        }
        client
            .force_kill()
            .await
            .map(|_| ())
            .map_err(|_| Q4RuntimeError::worker_shutdown())
    }

    fn map_worker_error(error: WorkerClientError) -> Q4RuntimeError {
        match error {
            WorkerClientError::Remote(remote) => Q4RuntimeError::owned(
                wire_error_code(remote.code),
                "The isolated H3 LD-Q4 worker rejected a typed request.",
                remote.retryable,
                remote.fatal,
            ),
            WorkerClientError::Supervisor(_)
            | WorkerClientError::CommandTimeout(_)
            | WorkerClientError::HeartbeatTimeout(_)
            | WorkerClientError::UnexpectedAck { .. }
            | WorkerClientError::UnexpectedReply => Q4RuntimeError::worker_protocol(),
        }
    }

    fn wire_error_code(code: ErrorCode) -> String {
        serde_json::to_value(code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "worker.command_failed".to_owned())
    }

    fn h3_profile() -> ProfileRef {
        ProfileRef {
            codec_family: CODEC_FAMILY.to_owned(),
            profile: PROFILE_ID.to_owned(),
            profile_version: PROFILE_VERSION.to_owned(),
        }
    }

    fn path_for_protocol(path: &Path) -> Result<String, Q4RuntimeError> {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(Q4RuntimeError::input_contract)
    }

    fn parse_wire_uuid(value: &str) -> Result<WireUuid, Q4RuntimeError> {
        let deserializer = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value);
        WireUuid::deserialize(deserializer).map_err(|_| Q4RuntimeError::input_contract())
    }

    fn replace_shared_status(
        shared: &Arc<Mutex<Q4StatusView>>,
        view: Q4StatusView,
    ) -> Result<(), Q4RuntimeError> {
        let mut guard = shared
            .lock()
            .map_err(|_| Q4RuntimeError::state_poisoned())?;
        *guard = view;
        Ok(())
    }

    fn replace_shared_capture_status(
        shared: &Arc<Mutex<Q4CaptureView>>,
        view: Q4CaptureView,
    ) -> Result<(), Q4RuntimeError> {
        let mut guard = shared
            .lock()
            .map_err(|_| Q4RuntimeError::state_poisoned())?;
        *guard = view;
        Ok(())
    }

    const fn transport_active(transport: Q4Transport) -> bool {
        transport.playing_a || transport.playing_b || transport.playing_c || transport.playing_d
    }

    fn stop_transport(transport: &mut Q4Transport) {
        transport.playing_a = false;
        transport.playing_b = false;
        transport.playing_c = false;
        transport.playing_d = false;
    }

    fn decode_watermark_allows(state: RingState, pending_frame: bool) -> bool {
        decode_watermark(state.occupancy(), pending_frame, state.available_capacity())
    }

    const fn decode_watermark(
        occupancy: u32,
        pending_frame: bool,
        available_capacity: u32,
    ) -> bool {
        occupancy == 0 && !pending_frame && available_capacity >= MAX_FRAMES_PER_Q4_SLOT
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum CaptureFinalizerReadiness {
        Absent,
        Pending,
        Ready,
    }

    fn capture_finalizer_readiness(
        finalizer: Option<&CaptureFinalizer>,
    ) -> CaptureFinalizerReadiness {
        match finalizer {
            None => CaptureFinalizerReadiness::Absent,
            Some(finalizer) if finalizer.task.is_finished() => CaptureFinalizerReadiness::Ready,
            Some(_) => CaptureFinalizerReadiness::Pending,
        }
    }

    const fn capture_start_failure_requires_shutdown(
        error_terminal: bool,
        had_active_capture: bool,
        owns_capture_after_failure: bool,
    ) -> bool {
        error_terminal || (!had_active_capture && owns_capture_after_failure)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum DueWork {
        Present,
        FinalizeCapture,
        Schedule,
        Wait,
    }

    const fn due_work(
        finalizer: CaptureFinalizerReadiness,
        presentation_due: bool,
        scheduling_due: bool,
        watermark_allows_decode: bool,
    ) -> DueWork {
        if presentation_due {
            DueWork::Present
        } else if matches!(finalizer, CaptureFinalizerReadiness::Ready) {
            DueWork::FinalizeCapture
        } else if scheduling_due && watermark_allows_decode {
            DueWork::Schedule
        } else {
            DueWork::Wait
        }
    }

    struct FrameClock {
        numerator: u64,
        denominator: u64,
        epoch: Instant,
        next_tick: u64,
    }

    impl FrameClock {
        fn new(numerator: u64, denominator: u64) -> Result<Self, Q4RuntimeError> {
            if numerator == 0
                || denominator == 0
                || frame_offset_ns(numerator, denominator, 1)? == 0
            {
                return Err(Q4RuntimeError::input_contract());
            }
            Ok(Self {
                numerator,
                denominator,
                epoch: Instant::now(),
                next_tick: 1,
            })
        }

        fn restart(&mut self) {
            self.epoch = Instant::now();
            self.next_tick = 1;
        }

        fn next_deadline(&self) -> Result<Instant, Q4RuntimeError> {
            let offset = frame_offset_ns(self.numerator, self.denominator, self.next_tick)?;
            self.epoch
                .checked_add(Duration::from_nanos(offset))
                .ok_or_else(Q4RuntimeError::worker_protocol)
        }

        fn advance_past(&mut self, now: Instant) {
            self.next_tick = self.next_tick.saturating_add(1);
            while self.next_deadline().is_ok_and(|deadline| deadline <= now) {
                self.next_tick = self.next_tick.saturating_add(1);
            }
        }
    }

    fn frame_offset_ns(numerator: u64, denominator: u64, tick: u64) -> Result<u64, Q4RuntimeError> {
        if numerator == 0 || denominator == 0 || tick == 0 {
            return Err(Q4RuntimeError::input_contract());
        }
        let value = u128::from(tick)
            .checked_mul(u128::from(denominator))
            .and_then(|value| value.checked_mul(1_000_000_000))
            .ok_or_else(Q4RuntimeError::worker_protocol)?
            / u128::from(numerator);
        u64::try_from(value).map_err(|_| Q4RuntimeError::worker_protocol())
    }

    #[cfg(test)]
    mod tests {
        use latentdeck_control::{CommandName, Q4Slot, Q4SourceStatus};

        use super::*;

        #[test]
        fn rational_frame_clock_uses_absolute_offsets() {
            assert_eq!(frame_offset_ns(24, 1, 1).expect("first"), 41_666_666);
            assert_eq!(frame_offset_ns(24, 1, 2).expect("second"), 83_333_333);
            assert_eq!(frame_offset_ns(24, 1, 3).expect("third"), 125_000_000);
        }

        #[test]
        fn worker_errors_are_path_free_and_preserve_closed_codes() {
            let error = map_worker_error(WorkerClientError::CommandTimeout(
                CommandName::DeckQ4ProcessSlot,
            ));
            assert_eq!(error.code, "worker.protocol_failed");
            assert!(error.terminal);
            assert!(!error.message.contains('\\'));
            assert!(!error.message.contains(':'));
        }

        #[test]
        fn scheduling_is_bounded_to_one_complete_q4_slot() {
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Ready, true, true, true),
                DueWork::Present
            );
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Ready, false, true, true),
                DueWork::FinalizeCapture
            );
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Absent, false, true, true),
                DueWork::Schedule
            );
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Pending, false, true, false),
                DueWork::Wait
            );
            assert!(decode_watermark(0, false, MAX_FRAMES_PER_Q4_SLOT));
            assert!(!decode_watermark(1, false, MAX_FRAMES_PER_Q4_SLOT));
            assert!(!decode_watermark(0, true, MAX_FRAMES_PER_Q4_SLOT));
            assert!(!decode_watermark(0, false, MAX_FRAMES_PER_Q4_SLOT - 1));
        }

        #[test]
        fn capture_status_validators_pin_carrier_target_and_generation() {
            let mut deck = actor_status();
            deck.source_a.latent_slot_count = 7;
            deck.source_b.latent_slot_count = 12;
            deck.source_c.latent_slot_count = 17;
            deck.source_d.latent_slot_count = 22;

            for (carrier, target) in [
                (Q4Slot::A, 7),
                (Q4Slot::B, 12),
                (Q4Slot::C, 17),
                (Q4Slot::D, 22),
            ] {
                deck.roles = roles_for_carrier(carrier);
                let capture_id = WireUuid::new_v4();
                let awaiting = Q4CaptureStatus {
                    capture_id,
                    mode: Q4CaptureMode::Snapshot,
                    state: Q4CaptureState::AwaitingReset,
                    structural_carrier: carrier,
                    latent_slots: 0,
                    current_generation: Some(deck.stream_generation),
                    minimum_new_generation: deck.stream_generation.checked_add(1),
                    target_latent_slots: Some(target),
                    stream_generation: None,
                    finalize_after_latent_slots: None,
                    reason: None,
                    receipt: None,
                };
                validate_capture_start_status(
                    &awaiting,
                    &deck,
                    Q4CaptureMode::Snapshot,
                    capture_id,
                )
                .expect("exact carrier target");

                let mut drifted_target = awaiting.clone();
                drifted_target.target_latent_slots = Some(target + 5);
                assert!(
                    validate_capture_start_status(
                        &drifted_target,
                        &deck,
                        Q4CaptureMode::Snapshot,
                        capture_id,
                    )
                    .is_err()
                );

                let mut active = awaiting;
                active.state = Q4CaptureState::Capturing;
                active.current_generation = None;
                active.minimum_new_generation = None;
                active.target_latent_slots = None;
                active.stream_generation = Some(deck.stream_generation);
                validate_active_capture_status(&active, &deck, Q4CaptureMode::Snapshot, capture_id)
                    .expect("active generation");
                active.stream_generation = deck.stream_generation.checked_add(1);
                assert!(
                    validate_active_capture_status(
                        &active,
                        &deck,
                        Q4CaptureMode::Snapshot,
                        capture_id,
                    )
                    .is_err()
                );
            }
        }

        #[test]
        fn capture_commands_observe_cancelled_callers() {
            let (start_reply, start_receiver) = oneshot::channel();
            drop(start_receiver);
            assert!(
                RuntimeCommand::CaptureStart {
                    mode: Q4CaptureMode::Snapshot,
                    output: PathBuf::from("unused.lc"),
                    reply: start_reply,
                }
                .reply_is_closed()
            );

            let (stop_reply, stop_receiver) = oneshot::channel();
            drop(stop_receiver);
            assert!(RuntimeCommand::CaptureStop { reply: stop_reply }.reply_is_closed());

            let (status_reply, status_receiver) = oneshot::channel();
            drop(status_receiver);
            assert!(
                RuntimeCommand::CaptureStatus {
                    reply: status_reply,
                }
                .reply_is_closed()
            );
        }

        #[test]
        fn spout_commands_observe_cancelled_callers() {
            let (status_reply, status_receiver) = oneshot::channel();
            drop(status_receiver);
            assert!(
                RuntimeCommand::SpoutStatus {
                    reply: status_reply
                }
                .reply_is_closed()
            );

            let (configure_reply, configure_receiver) = oneshot::channel();
            drop(configure_receiver);
            assert!(
                RuntimeCommand::ConfigureSpout {
                    name: Some("LatentDeck LD-Q4 Output".to_owned()),
                    enabled: Some(true),
                    reply: configure_reply,
                }
                .reply_is_closed()
            );
        }

        #[test]
        fn capture_start_failure_retains_new_worker_ownership_for_shutdown() {
            assert!(capture_start_failure_requires_shutdown(false, false, true));
            assert!(!capture_start_failure_requires_shutdown(false, true, true));
            assert!(!capture_start_failure_requires_shutdown(
                false, false, false
            ));
            assert!(capture_start_failure_requires_shutdown(true, true, false));
        }

        #[test]
        fn session_rotation_preserves_orderly_shutdown_reserve() {
            let above = SESSION_SHUTDOWN_MESSAGE_RESERVE + 1;
            assert!(!session_rotation_required(above, above));
            assert!(session_rotation_required(
                SESSION_SHUTDOWN_MESSAGE_RESERVE,
                above
            ));
            assert!(session_rotation_required(
                above,
                SESSION_SHUTDOWN_MESSAGE_RESERVE
            ));
        }

        #[tokio::test]
        async fn actor_cleanup_signal_is_distinct_from_closed_state() {
            let (completion_sender, completion) = watch::channel(false);
            let mut waiting = Box::pin(wait_for_actor_cleanup(completion));
            assert!(
                timeout(Duration::from_millis(10), waiting.as_mut())
                    .await
                    .is_err()
            );
            completion_sender.send_replace(true);
            waiting.await.expect("actor cleanup barrier");
        }

        #[tokio::test]
        async fn runtime_shutdown_waits_to_enqueue_behind_a_full_queue() {
            let (sender, mut receiver) = mpsc::channel(1);
            let (queued_reply, _queued_receiver) = oneshot::channel();
            sender
                .send(RuntimeCommand::Status {
                    reply: queued_reply,
                })
                .await
                .expect("fill actor queue");
            let closed = Arc::new(AtomicBool::new(false));
            let (cleanup_sender, cleanup_complete) = watch::channel(false);
            let runtime = Q4Runtime {
                sender,
                closed: Arc::clone(&closed),
                cleanup_complete,
                _task: tauri::async_runtime::spawn(async {}),
            };
            let mut shutdown = tokio::spawn(async move { runtime.shutdown().await });
            assert!(
                timeout(Duration::from_millis(10), &mut shutdown)
                    .await
                    .is_err(),
                "shutdown must retain ownership while the actor queue is full"
            );

            assert!(matches!(
                receiver.recv().await,
                Some(RuntimeCommand::Status { .. })
            ));
            let Some(RuntimeCommand::Shutdown { reply }) = receiver.recv().await else {
                panic!("shutdown must enqueue after capacity is released");
            };
            closed.store(true, Ordering::Release);
            reply.send(Ok(())).expect("actor shutdown reply");
            cleanup_sender.send_replace(true);
            shutdown
                .await
                .expect("shutdown task")
                .expect("owned cleanup");
        }

        #[tokio::test]
        async fn runtime_drop_queues_shutdown_without_preclosing_cleanup() {
            let (sender, mut receiver) = mpsc::channel(1);
            let closed = Arc::new(AtomicBool::new(false));
            let (_cleanup_sender, cleanup_complete) = watch::channel(false);
            let runtime = Q4Runtime {
                sender,
                closed: Arc::clone(&closed),
                cleanup_complete,
                _task: tauri::async_runtime::spawn(async {}),
            };
            drop(runtime);
            assert!(!closed.load(Ordering::Acquire));
            assert!(matches!(
                receiver.recv().await,
                Some(RuntimeCommand::Shutdown { .. })
            ));
        }

        #[test]
        fn decoded_progress_requires_current_roles_and_advances_all_playheads() {
            let mut status = actor_status();
            let transport = status.transport;
            let roles = status.roles;
            adopt_decoded_progress(&mut status, 0, 1, 2, 3, 4, roles, transport, 0, 4)
                .expect("first slot");
            assert_eq!(status.stream_sequence, 1);
            assert_eq!(status.decoded_start_frame, 4);
            assert_eq!(
                [
                    status.playhead_a,
                    status.playhead_b,
                    status.playhead_c,
                    status.playhead_d,
                ],
                [1, 2, 3, 4]
            );

            let wrong_roles = Q4Roles {
                carrier: Q4Slot::B,
                donor_b: Q4Slot::A,
                donor_c: Q4Slot::C,
                donor_d: Q4Slot::D,
            };
            assert!(
                adopt_decoded_progress(&mut status, 1, 2, 3, 4, 5, wrong_roles, transport, 4, 4,)
                    .is_err()
            );
        }

        #[tokio::test]
        async fn bounded_sender_times_out_instead_of_growing_a_queue() {
            let (sender, _receiver) = mpsc::channel(1);
            sender.send(1_u8).await.expect("fill bounded queue");
            let error = send_bounded(&sender, 2_u8, Duration::from_millis(10))
                .await
                .expect_err("full queue must time out");
            assert_eq!(error.code, "deck.runtime_timeout");
        }

        fn actor_status() -> Q4Status {
            let source = Q4SourceStatus {
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "a".repeat(64),
                latent_slot_count: 12,
            };
            Q4Status {
                deck_id: Q4_DECK_ID.to_owned(),
                deck_revision: 1,
                operator_id: Q4_OPERATOR_ID.to_owned(),
                operator_version: Q4_OPERATOR_VERSION.to_owned(),
                stream_generation: 1,
                stream_sequence: 0,
                playhead_a: 0,
                playhead_b: 0,
                playhead_c: 0,
                playhead_d: 0,
                roles: Q4Roles::default(),
                transport: Q4Transport::default(),
                controls: Q4Controls::default(),
                seed: 42,
                pending_reset: false,
                pending_reset_reasons: BoundedVec::default(),
                decoded_start_frame: 0,
                source_a: source.clone(),
                source_b: source.clone(),
                source_c: source.clone(),
                source_d: source,
            }
        }

        const fn roles_for_carrier(carrier: Q4Slot) -> Q4Roles {
            match carrier {
                Q4Slot::A => Q4Roles {
                    carrier,
                    donor_b: Q4Slot::B,
                    donor_c: Q4Slot::C,
                    donor_d: Q4Slot::D,
                },
                Q4Slot::B => Q4Roles {
                    carrier,
                    donor_b: Q4Slot::A,
                    donor_c: Q4Slot::C,
                    donor_d: Q4Slot::D,
                },
                Q4Slot::C => Q4Roles {
                    carrier,
                    donor_b: Q4Slot::A,
                    donor_c: Q4Slot::B,
                    donor_d: Q4Slot::D,
                },
                Q4Slot::D => Q4Roles {
                    carrier,
                    donor_b: Q4Slot::A,
                    donor_c: Q4Slot::B,
                    donor_d: Q4Slot::C,
                },
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use platform::Q4Runtime;

#[cfg(not(target_os = "windows"))]
pub(crate) struct Q4Runtime;

#[cfg(not(target_os = "windows"))]
impl Q4Runtime {
    pub(crate) async fn start(
        _app: AppHandle,
        _shared_status: Arc<Mutex<Q4StatusView>>,
        _shared_capture_status: Arc<Mutex<Q4CaptureView>>,
        _config: Q4LaunchConfig,
        _deck_session: DeckSessionLease,
    ) -> Result<Self, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn controls_set(
        &self,
        _controls: Q4Controls,
    ) -> Result<Q4ControlsAckView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn roles_set(
        &self,
        _roles: Q4Roles,
    ) -> Result<Q4RolesAckView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn transport_set(
        &self,
        _transport: Q4Transport,
    ) -> Result<Q4TransportAckView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn seed_set(&self, _seed: u64) -> Result<Q4SeedAckView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn restart(&self) -> Result<Q4StatusView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn capture_start(
        &self,
        _mode: Q4CaptureMode,
        _output: PathBuf,
    ) -> Result<Q4CaptureView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn capture_stop(&self) -> Result<Q4CaptureView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn capture_status(&self) -> Result<Q4CaptureView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn status(&self) -> Result<Q4StatusView, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn resize(&self, _width: u32, _height: u32) -> Result<(), Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), Q4RuntimeError> {
        Ok(())
    }

    pub(crate) async fn toggle_fullscreen(&self) -> Result<bool, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn spout_status(&self) -> Result<NativeSpoutStatus, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }

    pub(crate) async fn configure_spout(
        &self,
        _name: Option<String>,
        _enabled: Option<bool>,
    ) -> Result<NativeSpoutStatus, Q4RuntimeError> {
        Err(Q4RuntimeError::unsupported())
    }
}

#[cfg(test)]
mod common_tests {
    use latentdeck_cartridge::{
        manifest::{DType, Rational},
        profile::h3::H3CompatibilityKey,
    };
    use latentdeck_control::{BoundedVec, Q4SourceStatus, WireUuid};

    use super::*;

    #[test]
    fn ui_controls_and_roles_reject_invalid_values() {
        let mut controls = controls_input();
        controls.interaction = f64::NAN;
        assert_eq!(
            controls.into_wire().expect_err("NaN must fail").code,
            "deck.controls_invalid"
        );

        let roles = Q4RolesInput {
            carrier: Q4Slot::A,
            donor_b: Q4Slot::A,
            donor_c: Q4Slot::C,
            donor_d: Q4Slot::D,
        };
        assert_eq!(
            roles
                .into_wire()
                .expect_err("duplicate role must fail")
                .code,
            "deck.roles_invalid"
        );
    }

    #[test]
    fn status_view_stringifies_lossless_stream_counters() {
        let source = Q4SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: "a".repeat(64),
            latent_slot_count: 7,
        };
        let status = Q4Status {
            deck_id: Q4_DECK_ID.to_owned(),
            deck_revision: 1,
            operator_id: Q4_OPERATOR_ID.to_owned(),
            operator_version: Q4_OPERATOR_VERSION.to_owned(),
            stream_generation: u64::MAX,
            stream_sequence: u64::MAX - 1,
            playhead_a: 1,
            playhead_b: 2,
            playhead_c: 3,
            playhead_d: 4,
            roles: Q4Roles::default(),
            transport: Q4Transport::default(),
            controls: Q4Controls::default(),
            seed: 42,
            pending_reset: false,
            pending_reset_reasons: BoundedVec::default(),
            decoded_start_frame: 0,
            source_a: source.clone(),
            source_b: source.clone(),
            source_c: source.clone(),
            source_d: source,
        };
        let view = Q4StatusView::from_status(&status);
        assert_eq!(view.stream_generation, u64::MAX.to_string());
        assert_eq!(view.stream_sequence, (u64::MAX - 1).to_string());
    }

    #[test]
    fn status_view_preserves_explicit_duplicate_source_identity() {
        let source_a = Q4SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: "a".repeat(64),
            latent_slot_count: 72,
        };
        let source_b = Q4SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: "b".repeat(64),
            latent_slot_count: 32,
        };
        let source_c = Q4SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: "c".repeat(64),
            latent_slot_count: 32,
        };
        let status = Q4Status {
            deck_id: Q4_DECK_ID.to_owned(),
            deck_revision: 1,
            operator_id: Q4_OPERATOR_ID.to_owned(),
            operator_version: Q4_OPERATOR_VERSION.to_owned(),
            stream_generation: 1,
            stream_sequence: 0,
            playhead_a: 0,
            playhead_b: 0,
            playhead_c: 0,
            playhead_d: 0,
            roles: Q4Roles::default(),
            transport: Q4Transport::default(),
            controls: Q4Controls::default(),
            seed: 42,
            pending_reset: false,
            pending_reset_reasons: BoundedVec::default(),
            decoded_start_frame: 0,
            source_a,
            source_b: source_b.clone(),
            source_c,
            source_d: source_b,
        };

        let sources = Q4StatusView::from_status(&status)
            .sources
            .expect("loaded source identities");
        assert_eq!(sources.b, sources.d);
        assert_ne!(sources.a.archive_sha256, sources.b.archive_sha256);
        assert_ne!(sources.c.archive_sha256, sources.b.archive_sha256);
    }

    #[test]
    fn causal_reset_plan_rejects_stale_barriers_and_false_clear_acks() {
        assert!(
            CausalResetPlan::from_barrier(2, 1, 3, &[Q4ResetReason::TransportRestart]).is_err()
        );
        let plan = CausalResetPlan::from_barrier(2, 2, 3, &[Q4ResetReason::TransportRestart])
            .expect("fresh barrier");
        assert!(
            plan.validate_ack(3, &[Q4ResetReason::TransportRestart], false)
                .is_err()
        );
        plan.validate_ack(3, &[Q4ResetReason::TransportRestart], true)
            .expect("exact cleared ack");
    }

    #[test]
    fn four_way_compatibility_gate_includes_grid_geometry_and_timing() {
        let source_a = synthetic_profile(28, 50);
        let source_b = synthetic_profile(28, 50);
        let source_c = synthetic_profile(28, 50);
        let mut source_d = synthetic_profile(28, 50);
        require_compatible_sources([&source_a, &source_b, &source_c, &source_d])
            .expect("same contract");
        source_d.compatibility_key.latent_width = 51;
        assert!(require_compatible_sources([&source_a, &source_b, &source_c, &source_d]).is_err());
    }

    fn controls_input() -> Q4ControlsInput {
        Q4ControlsInput {
            algorithm: Q4Algorithm::Linear,
            interaction: 0.0,
            mode: Q4Mode::Hybridize,
            preserve: 0.55,
            influence_mode: Q4InfluenceMode::Manual,
            donor_weight_b: 1.0,
            donor_weight_c: 1.0,
            donor_weight_d: 1.0,
            triangle_x: 0.5,
            triangle_y: 1.0 / 3.0,
            xs5_routing: Q4Xs5Routing::TopK,
            temperature: 0.12,
            top_k: 8,
            sinkhorn_iterations: 5,
            chaos: 0.0,
        }
    }

    fn synthetic_profile(latent_height: u64, latent_width: u64) -> ValidatedH3Profile {
        ValidatedH3Profile {
            visual: latentdeck_cartridge::profile::h3::ValidatedVisual {
                latent_slots: 7,
                latent_height,
                latent_width,
                decoded_frame_count: 22,
                decoded_height: u32::try_from(latent_height * 16).expect("test height"),
                decoded_width: u32::try_from(latent_width * 16).expect("test width"),
            },
            audio: None,
            compatibility_key: H3CompatibilityKey {
                codec_family: "minimax_h3",
                profile: "h3_av_latent",
                profile_version: "0.1.0",
                runtime_dtype: DType::F16,
                batch: 1,
                latent_channels: 24,
                latent_height,
                latent_width,
                timing_contract: "minimax_h3_causal",
                timing_contract_version: "0.1.0",
                frame_rate: Rational {
                    numerator: 24,
                    denominator: 1,
                },
            },
        }
    }
}
