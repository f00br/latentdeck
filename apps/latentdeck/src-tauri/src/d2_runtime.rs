//! Trusted LD-D2 launch inputs and the single-owner realtime host.
//!
//! The webview supplies only immutable cartridge identities and bounded control
//! values. Local paths, the worker command, stream clock, generation, operator
//! identity, RGB ring, and native output remain owned by this Rust boundary.

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
    D2Algorithm, D2Controls, D2Mode, D2ResetReason, D2Routing, D2Status, D2Transport, D2Xs5Routing,
    FiniteF64, MAX_D2_SAFE_INTEGER,
};
use latentdeck_core::codec_pack::{
    ValidatedCodecPack, ValidatedExternalAsset, default_codec_pack_roots, discover_codec_packs,
    validate_external_asset,
};
use latentdeck_library::ResolvedDeckSource;
#[cfg(not(target_os = "windows"))]
use latentdeck_native_output::NativeSpoutStatus;
use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::{d2_capture_host::CaptureHostError, library_state::LibraryImporter};

#[cfg(not(target_os = "windows"))]
use crate::d2_capture_host::D2CaptureView;
#[cfg(not(target_os = "windows"))]
use latentdeck_control::D2CaptureMode;

pub(crate) const D2_OUTPUT_WINDOW_LABEL: &str = "latentdeck-d2-output";
const D2_OUTPUT_WINDOW_TITLE: &str = "LatentDeck LD-D2 Output";
const H3_PACK_ID: &str = "org.latentdeck.h3";
const H3_ASSET_ID: &str = "taeh3";
const D2_DECK_ID: &str = "main-d2";
const D2_OPERATOR_ID: &str = "org.latentdeck.builtin.ld_d2";
const D2_OPERATOR_VERSION: &str = "0.1.0";
const INITIAL_GENERATION: u64 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct D2ControlsInput {
    algorithm: D2Algorithm,
    mix: f64,
    mode: D2Mode,
    routing: D2Routing,
    interaction: f64,
    preserve: f64,
    chaos: f64,
    xs1_channel_a: u8,
    xs1_channel_b: u8,
    xs1_angle_degrees: f64,
    xs2_radius: u8,
    xs3_high_gain: f64,
    xs4_epsilon: f64,
    xs5_routing: D2Xs5Routing,
    temperature: f64,
    top_k: u8,
    sinkhorn_iterations: u8,
}

impl D2ControlsInput {
    pub(crate) fn into_wire(self) -> Result<D2Controls, D2RuntimeError> {
        let controls = D2Controls {
            algorithm: self.algorithm,
            mix: finite(self.mix)?,
            mode: self.mode,
            routing: self.routing,
            interaction: finite(self.interaction)?,
            preserve: finite(self.preserve)?,
            chaos: finite(self.chaos)?,
            xs1_channel_a: self.xs1_channel_a,
            xs1_channel_b: self.xs1_channel_b,
            xs1_angle_degrees: finite(self.xs1_angle_degrees)?,
            xs2_radius: self.xs2_radius,
            xs3_high_gain: finite(self.xs3_high_gain)?,
            xs4_epsilon: finite(self.xs4_epsilon)?,
            xs5_routing: self.xs5_routing,
            temperature: finite(self.temperature)?,
            top_k: self.top_k,
            sinkhorn_iterations: self.sinkhorn_iterations,
        };
        controls
            .validate()
            .map_err(|_| D2RuntimeError::invalid_controls())?;
        Ok(controls)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Exact four-flag frontend transport schema.
pub(crate) struct D2TransportInput {
    playing_a: bool,
    playing_b: bool,
    loop_a: bool,
    loop_b: bool,
}

impl From<D2TransportInput> for D2Transport {
    fn from(value: D2TransportInput) -> Self {
        Self {
            playing_a: value.playing_a,
            playing_b: value.playing_b,
            loop_a: value.loop_a,
            loop_b: value.loop_b,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2ControlsView {
    algorithm: D2Algorithm,
    mix: f64,
    mode: D2Mode,
    routing: D2Routing,
    interaction: f64,
    preserve: f64,
    chaos: f64,
    xs1_channel_a: u8,
    xs1_channel_b: u8,
    xs1_angle_degrees: f64,
    xs2_radius: u8,
    xs3_high_gain: f64,
    xs4_epsilon: f64,
    xs5_routing: D2Xs5Routing,
    temperature: f64,
    top_k: u8,
    sinkhorn_iterations: u8,
}

impl From<&D2Controls> for D2ControlsView {
    fn from(value: &D2Controls) -> Self {
        Self {
            algorithm: value.algorithm,
            mix: value.mix.get(),
            mode: value.mode,
            routing: value.routing,
            interaction: value.interaction.get(),
            preserve: value.preserve.get(),
            chaos: value.chaos.get(),
            xs1_channel_a: value.xs1_channel_a,
            xs1_channel_b: value.xs1_channel_b,
            xs1_angle_degrees: value.xs1_angle_degrees.get(),
            xs2_radius: value.xs2_radius,
            xs3_high_gain: value.xs3_high_gain.get(),
            xs4_epsilon: value.xs4_epsilon.get(),
            xs5_routing: value.xs5_routing,
            temperature: value.temperature.get(),
            top_k: value.top_k,
            sinkhorn_iterations: value.sinkhorn_iterations,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // Exact four-flag frontend transport schema.
pub(crate) struct D2TransportView {
    playing_a: bool,
    playing_b: bool,
    loop_a: bool,
    loop_b: bool,
}

impl From<D2Transport> for D2TransportView {
    fn from(value: D2Transport) -> Self {
        Self {
            playing_a: value.playing_a,
            playing_b: value.playing_b,
            loop_a: value.loop_a,
            loop_b: value.loop_b,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2StatusView {
    pub(crate) loaded: bool,
    stream_generation: String,
    stream_sequence: String,
    playhead_a: u64,
    playhead_b: u64,
    transport: D2TransportView,
    controls: D2ControlsView,
    seed: u64,
    pending_reset: bool,
    pending_reset_reasons: Vec<String>,
}

impl Default for D2StatusView {
    fn default() -> Self {
        let controls = D2Controls::default();
        Self {
            loaded: false,
            stream_generation: "0".to_owned(),
            stream_sequence: "0".to_owned(),
            playhead_a: 0,
            playhead_b: 0,
            transport: D2Transport::default().into(),
            controls: (&controls).into(),
            seed: 0,
            pending_reset: false,
            pending_reset_reasons: Vec::new(),
        }
    }
}

impl D2StatusView {
    fn from_status(status: &D2Status) -> Self {
        Self {
            loaded: true,
            stream_generation: status.stream_generation.to_string(),
            stream_sequence: status.stream_sequence.to_string(),
            playhead_a: status.playhead_a,
            playhead_b: status.playhead_b,
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

    fn stopped_from(status: &D2Status) -> Self {
        let mut view = Self::from_status(status);
        view.loaded = false;
        view.transport.playing_a = false;
        view.transport.playing_b = false;
        view.pending_reset = false;
        view.pending_reset_reasons.clear();
        view
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2ControlsAckView {
    controls: D2ControlsView,
    requires_causal_reset: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2TransportAckView {
    transport: D2TransportView,
    requires_causal_reset: bool,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2SeedAckView {
    seed: u64,
    requires_causal_reset: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2ErrorEvent {
    pub(crate) code: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2DecoderView {
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
pub(crate) struct D2BackendView {
    state: String,
    pack_id: Option<String>,
    pack_version: Option<String>,
    display_name: Option<String>,
    d2_entrypoint_available: bool,
    decoder: Option<D2DecoderView>,
    detail: Option<String>,
}

#[derive(Clone)]
pub(crate) struct D2LaunchBackend {
    codec_pack: ValidatedCodecPack,
    decoder_asset: ValidatedExternalAsset,
}

pub(crate) struct D2BackendController {
    codec_pack: Option<ValidatedCodecPack>,
    decoder_asset: Option<ValidatedExternalAsset>,
    discovery_fault: Option<D2RuntimeError>,
}

impl D2BackendController {
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
                discovery_fault: Some(D2RuntimeError::new(
                    error.code,
                    "Installed Codec Pack validation failed.",
                    true,
                    false,
                )),
            },
        }
    }

    pub(crate) fn view(&self) -> D2BackendView {
        if let Some(error) = &self.discovery_fault {
            return D2BackendView {
                state: "error".to_owned(),
                pack_id: None,
                pack_version: None,
                display_name: None,
                d2_entrypoint_available: false,
                decoder: None,
                detail: Some(error.message.to_owned()),
            };
        }
        let Some(pack) = &self.codec_pack else {
            return D2BackendView {
                state: "missing".to_owned(),
                pack_id: None,
                pack_version: None,
                display_name: None,
                d2_entrypoint_available: false,
                decoder: None,
                detail: Some("Install a compatible H3 Codec Pack.".to_owned()),
            };
        };
        let d2_available = pack.manifest.worker.d2_arguments.is_some();
        let decoder = self
            .decoder_asset
            .as_ref()
            .and_then(|asset| decoder_view(pack, asset));
        let (state, detail) = if !d2_available {
            (
                "incompatible",
                Some("The installed Codec Pack does not declare an LD-D2 worker.".to_owned()),
            )
        } else if decoder.is_none() {
            (
                "decoder_missing",
                Some("Select a compatible TAEH3 decoder weight.".to_owned()),
            )
        } else {
            ("ready", None)
        };
        D2BackendView {
            state: state.to_owned(),
            pack_id: Some(pack.manifest.pack_id.clone()),
            pack_version: Some(pack.manifest.pack_version.clone()),
            display_name: Some(pack.manifest.display_name.clone()),
            d2_entrypoint_available: d2_available,
            decoder,
            detail,
        }
    }

    pub(crate) fn pack_for_selection(&self) -> Result<ValidatedCodecPack, D2RuntimeError> {
        if let Some(error) = &self.discovery_fault {
            return Err(error.clone());
        }
        self.codec_pack
            .clone()
            .ok_or_else(D2RuntimeError::codec_missing)
    }

    pub(crate) fn accept_decoder(&mut self, asset: ValidatedExternalAsset) -> D2BackendView {
        self.decoder_asset = Some(asset);
        self.view()
    }

    pub(crate) fn launch_backend(&self) -> Result<D2LaunchBackend, D2RuntimeError> {
        if let Some(error) = &self.discovery_fault {
            return Err(error.clone());
        }
        let codec_pack = self
            .codec_pack
            .clone()
            .ok_or_else(D2RuntimeError::codec_missing)?;
        if codec_pack.manifest.worker.d2_arguments.is_none() {
            return Err(D2RuntimeError::d2_entrypoint_missing());
        }
        let decoder_asset = self
            .decoder_asset
            .clone()
            .ok_or_else(D2RuntimeError::decoder_missing)?;
        Ok(D2LaunchBackend {
            codec_pack,
            decoder_asset,
        })
    }
}

pub(crate) fn validate_selected_decoder(
    pack: &ValidatedCodecPack,
    path: &Path,
) -> Result<ValidatedExternalAsset, D2RuntimeError> {
    validate_external_asset(pack, H3_ASSET_ID, path).map_err(|error| {
        D2RuntimeError::new(
            error.code,
            "The selected decoder weight is not an accepted Codec Pack asset.",
            true,
            false,
        )
    })
}

#[derive(Clone)]
struct TrustedD2Source {
    path: PathBuf,
    cartridge_id: String,
    archive_sha256: String,
    profile: ValidatedH3Profile,
}

#[derive(Clone)]
pub(crate) struct D2LaunchConfig {
    backend: D2LaunchBackend,
    source_a: TrustedD2Source,
    source_b: TrustedD2Source,
    controls: D2Controls,
    transport: D2Transport,
    seed: u64,
    app_local_data: PathBuf,
    library_importer: LibraryImporter,
}

#[derive(Clone)]
pub(crate) struct D2CaptureHostServices {
    app_local_data: PathBuf,
    library_importer: LibraryImporter,
}

impl D2CaptureHostServices {
    pub(crate) fn new(app_local_data: PathBuf, library_importer: LibraryImporter) -> Self {
        Self {
            app_local_data,
            library_importer,
        }
    }
}

impl D2LaunchConfig {
    pub(crate) fn build(
        backend: D2LaunchBackend,
        source_a: &ResolvedDeckSource,
        source_b: &ResolvedDeckSource,
        controls: D2Controls,
        transport: D2Transport,
        seed: u64,
        capture_host: D2CaptureHostServices,
    ) -> Result<Self, D2RuntimeError> {
        if seed > MAX_D2_SAFE_INTEGER {
            return Err(D2RuntimeError::invalid_seed());
        }
        controls
            .validate()
            .map_err(|_| D2RuntimeError::invalid_controls())?;
        let source_a = inspect_source(source_a)?;
        let source_b = inspect_source(source_b)?;
        require_compatible_sources(&source_a.profile, &source_b.profile)?;
        Ok(Self {
            backend,
            source_a,
            source_b,
            controls,
            transport,
            seed,
            app_local_data: capture_host.app_local_data,
            library_importer: capture_host.library_importer,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct D2RuntimeError {
    pub(crate) code: String,
    pub(crate) message: &'static str,
    pub(crate) recoverable: bool,
    terminal: bool,
}

impl D2RuntimeError {
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
            "LD-D2 controls are outside the supported finite bounds.",
            true,
            false,
        )
    }

    pub(crate) fn invalid_seed() -> Self {
        Self::owned(
            "deck.seed_invalid",
            "LD-D2 seed must be a non-negative exact u53 integer.",
            true,
            false,
        )
    }

    pub(crate) fn source_invalid() -> Self {
        Self::owned(
            "deck.source_invalid",
            "A selected LD-D2 source failed full cartridge validation.",
            true,
            false,
        )
    }

    fn source_incompatible() -> Self {
        Self::owned(
            "deck.source_incompatible",
            "LD-D2 sources differ in codec profile, latent grid, geometry, or timing contract.",
            true,
            false,
        )
    }

    fn codec_missing() -> Self {
        Self::owned(
            "codec.pack_missing",
            "Install a compatible H3 Codec Pack before opening LD-D2.",
            true,
            false,
        )
    }

    fn d2_entrypoint_missing() -> Self {
        Self::owned(
            "codec.d2_entrypoint_missing",
            "The installed Codec Pack does not provide the trusted LD-D2 worker entrypoint.",
            true,
            false,
        )
    }

    fn decoder_missing() -> Self {
        Self::owned(
            "codec.asset_missing",
            "Select a compatible TAEH3 decoder weight before opening LD-D2.",
            true,
            false,
        )
    }

    fn runtime_unavailable() -> Self {
        Self::owned(
            "deck.runtime_unavailable",
            "The LD-D2 runtime is unavailable; open the Deck again.",
            true,
            false,
        )
    }

    fn runtime_timeout() -> Self {
        Self::owned(
            "deck.runtime_timeout",
            "The LD-D2 runtime did not answer within its bounded deadline.",
            true,
            false,
        )
    }

    fn runtime_cleanup() -> Self {
        Self::owned(
            "deck.runtime_cleanup_failed",
            "The LD-D2 runtime stopped before its owned resources were cleaned up.",
            false,
            true,
        )
    }

    pub(crate) fn state_poisoned() -> Self {
        Self::owned(
            "deck.state_unavailable",
            "LD-D2 state is unavailable; restart LatentDeck.",
            false,
            true,
        )
    }

    fn worker_start() -> Self {
        Self::owned(
            "worker.start_failed",
            "The isolated H3 LD-D2 worker could not be started.",
            true,
            true,
        )
    }

    fn worker_protocol() -> Self {
        Self::owned(
            "worker.protocol_failed",
            "The isolated H3 LD-D2 worker violated its typed contract.",
            true,
            true,
        )
    }

    fn worker_shutdown() -> Self {
        Self::owned(
            "worker.shutdown_failed",
            "The isolated H3 LD-D2 worker could not be stopped safely.",
            false,
            true,
        )
    }

    fn worker_process_exited() -> Self {
        Self::owned(
            "worker.process_exited",
            "The isolated H3 LD-D2 worker exited; open the Deck again to restart it.",
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
            "Trusted LD-D2 inputs cannot be represented by Worker Protocol 1.",
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

    fn capture_host(error: CaptureHostError) -> Self {
        Self::owned(error.code, error.message, true, false)
    }

    fn capture_finalize() -> Self {
        Self::owned(
            "capture.finalize_failed",
            "The capture could not be validated, saved, and imported safely.",
            true,
            false,
        )
    }

    fn output(code: &'static str) -> Self {
        Self::owned(
            code,
            "Native DX12 output failed and LD-D2 was stopped.",
            true,
            true,
        )
    }

    #[cfg(not(target_os = "windows"))]
    fn unsupported() -> Self {
        Self::owned(
            "output.platform_unsupported",
            "LD-D2 native realtime output requires Windows and DirectX 12.",
            false,
            false,
        )
    }

    pub(crate) fn event(&self) -> D2ErrorEvent {
        D2ErrorEvent {
            code: self.code.clone(),
            detail: self.message.to_owned(),
        }
    }
}

impl fmt::Display for D2RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for D2RuntimeError {}

fn finite(value: f64) -> Result<FiniteF64, D2RuntimeError> {
    FiniteF64::new(value).ok_or_else(D2RuntimeError::invalid_controls)
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
) -> Option<D2DecoderView> {
    let descriptor = pack
        .manifest
        .external_assets
        .iter()
        .find(|candidate| candidate.asset_id == asset.asset_id)?;
    let variant = descriptor
        .accepted_variants
        .iter()
        .find(|candidate| candidate.variant_id == asset.variant_id)?;
    Some(D2DecoderView {
        asset_id: asset.asset_id.clone(),
        variant_id: asset.variant_id.clone(),
        sha256: asset.sha256.clone(),
        byte_length: asset.byte_length,
        source_url: variant.source_url.clone(),
        license_label: variant.license_label.clone(),
        license_url: variant.license_url.clone(),
    })
}

fn inspect_source(resolved: &ResolvedDeckSource) -> Result<TrustedD2Source, D2RuntimeError> {
    let validated = open_validated(resolved.path(), &ValidationOptions::default())
        .map_err(|_| D2RuntimeError::source_invalid())?;
    let identity = resolved.identity();
    if validated.manifest().cartridge_id.0 != identity.cartridge_id()
        || validated.receipt().archive_sha256.to_string() != identity.archive_sha256().as_str()
    {
        return Err(D2RuntimeError::source_invalid());
    }
    Ok(TrustedD2Source {
        path: resolved.path().to_path_buf(),
        cartridge_id: identity.cartridge_id().to_owned(),
        archive_sha256: identity.archive_sha256().as_str().to_owned(),
        profile: validated.h3_profile().clone(),
    })
}

fn require_compatible_sources(
    source_a: &ValidatedH3Profile,
    source_b: &ValidatedH3Profile,
) -> Result<(), D2RuntimeError> {
    let a = &source_a.compatibility_key;
    let b = &source_b.compatibility_key;
    if a != b
        || source_a.visual.decoded_width != source_b.visual.decoded_width
        || source_a.visual.decoded_height != source_b.visual.decoded_height
    {
        return Err(D2RuntimeError::source_incompatible());
    }
    Ok(())
}

const fn reset_reason_name(reason: D2ResetReason) -> &'static str {
    match reason {
        D2ResetReason::SlotALoop => "slot_a.loop",
        D2ResetReason::SlotBLoop => "slot_b.loop",
        D2ResetReason::TransportRestart => "transport.restart",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CausalResetPlan {
    current_generation: u64,
    new_generation: u64,
    reasons: Vec<D2ResetReason>,
}

impl CausalResetPlan {
    fn from_barrier(
        expected_generation: u64,
        barrier_generation: u64,
        minimum_new_generation: u64,
        reasons: &[D2ResetReason],
    ) -> Result<Self, D2RuntimeError> {
        if barrier_generation != expected_generation
            || minimum_new_generation <= barrier_generation
            || reasons.is_empty()
            || reasons.len() > 2
        {
            return Err(D2RuntimeError::reset());
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
        reasons: &[D2ResetReason],
        causal_state_cleared: bool,
    ) -> Result<(), D2RuntimeError> {
        if generation != self.new_generation
            || reasons != self.reasons
            || !causal_state_cleared
            || self.new_generation <= self.current_generation
        {
            return Err(D2RuntimeError::reset());
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

    use latentdeck_cartridge::{resample::pack_resample_atomic, writer::WriteOptions};
    use latentdeck_control::{
        Ack, BoundedVec, CodecInspection, CodecLoad, Command, D2CaptureMode, D2CaptureStart,
        D2CaptureState, D2CaptureStatus, D2CaptureStatusRequest, D2CaptureStop, D2ControlsSet,
        D2Load, D2ProcessSlot, D2ProcessSlotAck, D2Reset, D2Restart, D2SeedSet, D2SourceBinding,
        D2TransportSet, EmptyPayload, ErrorCode, ExternalAssetBinding, MAX_CONTROL_FRAME_BYTES,
        ProfileRef, RingBind, SessionConfigure, ShutdownReason, WORKER_PROTOCOL_VERSION, WireUuid,
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

    use crate::d2_capture_host::{
        APP_CAPTURE_MAX_LATENT_SLOTS, APP_CAPTURE_MAX_VISUAL_BYTES, CaptureCoordinator,
        CaptureHostError, CaptureSpoolBinding, D2CaptureView, resample_request_from_receipt,
        validate_output_path,
    };

    use super::{
        AppHandle, Arc, CausalResetPlan, D2_DECK_ID, D2_OPERATOR_ID, D2_OPERATOR_VERSION,
        D2_OUTPUT_WINDOW_LABEL, D2_OUTPUT_WINDOW_TITLE, D2Controls, D2ControlsAckView,
        D2LaunchBackend, D2LaunchConfig, D2ResetReason, D2RuntimeError, D2SeedAckView, D2Status,
        D2StatusView, D2Transport, D2TransportAckView, Duration, INITIAL_GENERATION,
        MAX_D2_SAFE_INTEGER, Mutex, Path, PathBuf, TrustedD2Source, ValidatedCodecPack,
    };

    const CHANNEL_CAPACITY: usize = 8;
    const ACTOR_REPLY_TIMEOUT: Duration = Duration::from_secs(5);
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
    const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
    const SCHEDULER_POLL: Duration = Duration::from_millis(2);
    const MAX_FRAMES_PER_D2_SLOT: u32 = 4;
    // Leave enough authenticated envelopes for a final in-flight reply,
    // capture settlement, and orderly worker shutdown. Reopening the Deck is
    // the explicit causal reset and session-rotation boundary in v0.1.
    // A 64 KiB named-pipe buffer cannot hold 1,024 authenticated protocol
    // envelopes (the fixed MessagePack envelope alone exceeds 64 bytes). This
    // reserve therefore covers every queued idle heartbeat plus final command
    // replies and an orderly shutdown, even before Core has read those frames.
    const SESSION_SHUTDOWN_MESSAGE_RESERVE: usize = 1_024;
    const CODEC_FAMILY: &str = "minimax_h3";
    const PROFILE_ID: &str = "h3_av_latent";
    const PROFILE_VERSION: &str = "0.1.0";

    pub(crate) struct D2Runtime {
        sender: mpsc::Sender<RuntimeCommand>,
        closed: Arc<AtomicBool>,
        cleanup_complete: watch::Receiver<bool>,
        _task: TauriJoinHandle<()>,
    }

    impl D2Runtime {
        pub(crate) async fn start(
            app: AppHandle,
            shared_status: Arc<Mutex<D2StatusView>>,
            shared_capture_status: Arc<Mutex<D2CaptureView>>,
            config: D2LaunchConfig,
        ) -> Result<Self, D2RuntimeError> {
            let launch = ValidatedWorkerLaunch::from_codec_pack_d2(&config.backend.codec_pack)
                .map_err(|_| D2RuntimeError::d2_entrypoint_missing())?;
            let pending = spawn_worker(launch)
                .await
                .map_err(|_| D2RuntimeError::worker_start())?;
            let session = pending
                .connect()
                .await
                .map_err(|_| D2RuntimeError::worker_start())?;
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
            let view = D2StatusView::from_status(&status);
            if let Err(error) = replace_shared_status(&shared_status, view.clone()) {
                cleanup_pre_actor_start(output, &mut client).await;
                return Err(error);
            }
            let _ = app.emit("deck-d2-status", view);
            let capture_view = D2CaptureView::default();
            if let Err(error) =
                replace_shared_capture_status(&shared_capture_status, capture_view.clone())
            {
                cleanup_pre_actor_start(output, &mut client).await;
                return Err(error);
            }
            let _ = app.emit("deck-d2-capture", capture_view);

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
                closed: Arc::clone(&closed),
                pending_frame: None,
                presented_sequence: 0,
                frame_clock,
                app_local_data: config.app_local_data,
                library_importer: config.library_importer,
                capture: None,
                capture_finalizer: None,
                capture_coordinator: CaptureCoordinator::default(),
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
            controls: D2Controls,
        ) -> Result<D2ControlsAckView, D2RuntimeError> {
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

        pub(crate) async fn transport_set(
            &self,
            transport: D2Transport,
        ) -> Result<D2TransportAckView, D2RuntimeError> {
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

        pub(crate) async fn seed_set(&self, seed: u64) -> Result<D2SeedAckView, D2RuntimeError> {
            self.ensure_open()?;
            if seed > MAX_D2_SAFE_INTEGER {
                return Err(D2RuntimeError::invalid_seed());
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

        pub(crate) async fn restart(&self) -> Result<D2StatusView, D2RuntimeError> {
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
            mode: D2CaptureMode,
            output: PathBuf,
        ) -> Result<D2CaptureView, D2RuntimeError> {
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

        pub(crate) async fn capture_stop(&self) -> Result<D2CaptureView, D2RuntimeError> {
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

        pub(crate) async fn capture_status(&self) -> Result<D2CaptureView, D2RuntimeError> {
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

        pub(crate) async fn status(&self) -> Result<D2StatusView, D2RuntimeError> {
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
        ) -> Result<ResizeOutcome, D2RuntimeError> {
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

        pub(crate) async fn toggle_fullscreen(&self) -> Result<bool, D2RuntimeError> {
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

        pub(crate) async fn spout_status(&self) -> Result<NativeSpoutStatus, D2RuntimeError> {
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
        ) -> Result<NativeSpoutStatus, D2RuntimeError> {
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

        pub(crate) async fn shutdown(&self) -> Result<(), D2RuntimeError> {
            let command_result = if self.closed.load(Ordering::Acquire) {
                Ok(())
            } else {
                let (reply, receiver) = oneshot::channel();
                // Shutdown is an ownership barrier, not an ordinary UI
                // command. It must remain pending until the bounded queue has
                // room instead of timing out before the actor ever sees it.
                match self.sender.send(RuntimeCommand::Shutdown { reply }).await {
                    Ok(()) => match receiver.await {
                        Ok(result) => result,
                        Err(_) => Err(D2RuntimeError::runtime_unavailable()),
                    },
                    Err(_) => Err(D2RuntimeError::runtime_unavailable()),
                }
            };
            // `closed` rejects new commands but does not mean the actor has
            // finished worker shutdown or joined its capture finalizer. Always
            // wait for the actor-owned cleanup barrier, including after a
            // terminal command already put the runtime into the closed state.
            let cleanup_result = wait_for_actor_cleanup(self.cleanup_complete.clone()).await;
            self.closed.store(true, Ordering::Release);
            cleanup_result?;
            command_result
        }

        fn ensure_open(&self) -> Result<(), D2RuntimeError> {
            if self.closed.load(Ordering::Acquire) {
                Err(D2RuntimeError::runtime_unavailable())
            } else {
                Ok(())
            }
        }
    }

    impl Drop for D2Runtime {
        fn drop(&mut self) {
            if self.closed.load(Ordering::Acquire) {
                return;
            }
            let (reply, _receiver) = oneshot::channel();
            // Do not pre-close the shared actor state here: `stop` owns that
            // transition. If this bounded send fails, dropping the last sender
            // still disconnects the channel and makes the actor stop itself.
            let _ = self.sender.try_send(RuntimeCommand::Shutdown { reply });
        }
    }

    struct InitializedSession {
        status: D2Status,
        owner: WindowsRgbRingOwner,
        consumer: WindowsRgbRingConsumer,
        output: NativeOutput,
    }

    async fn initialize_session(
        app: &AppHandle,
        config: &D2LaunchConfig,
        client: &mut WorkerClient,
    ) -> Result<InitializedSession, D2RuntimeError> {
        configure_session(client).await?;
        let profile = h3_profile();
        let inspection = inspect_codec(client).await?;
        validate_inspection(&inspection, &config.backend.codec_pack, &profile)?;
        load_codec(client, &config.backend, &profile).await?;
        let status = load_deck(client, config).await?;
        validate_loaded_status(&status, config)?;

        let width = config.source_a.profile.visual.decoded_width;
        let height = config.source_a.profile.visual.decoded_height;
        let descriptor = RingDescriptor::new(width, height, INITIAL_GENERATION)
            .map_err(|_| D2RuntimeError::ring())?;
        let owner = WindowsRgbRingOwner::create(descriptor).map_err(|_| D2RuntimeError::ring())?;
        let consumer = owner.open_consumer().map_err(|_| D2RuntimeError::ring())?;
        let owner = bind_ring(client, owner).await?;
        let output = NativeOutput::new(
            app,
            NativeOutputConfig::new(
                width,
                height,
                D2_OUTPUT_WINDOW_LABEL,
                D2_OUTPUT_WINDOW_TITLE,
            ),
        )
        .await
        .map_err(|error| D2RuntimeError::output(error.code()))?;
        if output.frame_dimensions() != (width, height) {
            let error = D2RuntimeError::output("output.contract_invalid");
            let _ = destroy_output(&output);
            return Err(error);
        }
        if let Err(error) = output.show() {
            let error = D2RuntimeError::output(error.code());
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

    async fn configure_session(client: &mut WorkerClient) -> Result<(), D2RuntimeError> {
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
            return Err(D2RuntimeError::worker_protocol());
        };
        if configured.selected_protocol_version != request.selected_protocol_version
            || configured.heartbeat_interval_ms != request.heartbeat_interval_ms
            || configured.heartbeat_hard_timeout_ms != request.heartbeat_hard_timeout_ms
            || configured.max_frame_bytes != request.max_frame_bytes
            || configured.max_inflight_decode_batches != request.max_inflight_decode_batches
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn inspect_codec(client: &mut WorkerClient) -> Result<CodecInspection, D2RuntimeError> {
        match client
            .call(Command::CodecInspect(EmptyPayload {}), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?
        {
            Ack::CodecInspect(inspection) => Ok(inspection),
            _ => Err(D2RuntimeError::worker_protocol()),
        }
    }

    fn validate_inspection(
        inspection: &CodecInspection,
        pack: &ValidatedCodecPack,
        profile: &ProfileRef,
    ) -> Result<(), D2RuntimeError> {
        if !inspection.cuda_available
            || !inspection.devices.iter().any(|device| device.ordinal == 0)
        {
            return Err(D2RuntimeError::codec_runtime());
        }
        let adapter = inspection
            .adapters
            .iter()
            .find(|adapter| adapter.adapter_id == pack.manifest.adapter.adapter_id)
            .ok_or_else(D2RuntimeError::codec_runtime)?;
        if adapter.adapter_version != pack.manifest.adapter.adapter_version
            || !adapter
                .profiles
                .iter()
                .any(|candidate| candidate == profile)
        {
            return Err(D2RuntimeError::codec_runtime());
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
            return Err(D2RuntimeError::codec_runtime());
        }
        Ok(())
    }

    async fn load_codec(
        client: &mut WorkerClient,
        backend: &D2LaunchBackend,
        profile: &ProfileRef,
    ) -> Result<(), D2RuntimeError> {
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
                .map_err(|_| D2RuntimeError::input_contract())?,
        };
        let ack = client
            .call(Command::CodecLoad(request.clone()), COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)?;
        let Ack::CodecLoad(loaded) = ack else {
            return Err(D2RuntimeError::worker_protocol());
        };
        if loaded.pack_id != request.pack_id
            || loaded.pack_version != request.pack_version
            || loaded.adapter_id != request.adapter_id
            || loaded.adapter_version != backend.codec_pack.manifest.adapter.adapter_version
            || loaded.profile != request.profile
            || loaded.device.ordinal != request.device_ordinal
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn load_deck(
        client: &mut WorkerClient,
        config: &D2LaunchConfig,
    ) -> Result<D2Status, D2RuntimeError> {
        let request = D2Load {
            deck_id: D2_DECK_ID.to_owned(),
            operator_id: D2_OPERATOR_ID.to_owned(),
            operator_version: D2_OPERATOR_VERSION.to_owned(),
            source_a: source_binding(&config.source_a)?,
            source_b: source_binding(&config.source_b)?,
            controls: config.controls.clone(),
            transport: config.transport,
            seed: config.seed,
            stream_generation: INITIAL_GENERATION,
        };
        client
            .deck_d2_load(request, COMMAND_TIMEOUT)
            .await
            .map_err(map_worker_error)
    }

    fn source_binding(source: &TrustedD2Source) -> Result<D2SourceBinding, D2RuntimeError> {
        Ok(D2SourceBinding {
            cartridge_path: path_for_protocol(&source.path)?,
            cartridge_id: parse_wire_uuid(&source.cartridge_id)?,
            expected_archive_sha256: source.archive_sha256.clone(),
        })
    }

    fn validate_loaded_status(
        status: &D2Status,
        config: &D2LaunchConfig,
    ) -> Result<(), D2RuntimeError> {
        if status.deck_id != D2_DECK_ID
            || status.deck_revision == 0
            || status.operator_id != D2_OPERATOR_ID
            || status.operator_version != D2_OPERATOR_VERSION
            || status.stream_generation != INITIAL_GENERATION
            || status.stream_sequence != 0
            || status.playhead_a != 0
            || status.playhead_b != 0
            || status.transport != config.transport
            || status.controls != config.controls
            || status.seed != config.seed
            || status.pending_reset
            || !status.pending_reset_reasons.is_empty()
            || status.decoded_start_frame != 0
            || status.source_a.cartridge_id != parse_wire_uuid(&config.source_a.cartridge_id)?
            || status.source_b.cartridge_id != parse_wire_uuid(&config.source_b.cartridge_id)?
            || status.source_a.archive_sha256 != config.source_a.archive_sha256
            || status.source_b.archive_sha256 != config.source_b.archive_sha256
            || status.source_a.latent_slot_count != config.source_a.profile.visual.latent_slots
            || status.source_b.latent_slot_count != config.source_b.profile.visual.latent_slots
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    fn validate_capture_start_status(
        capture: &D2CaptureStatus,
        deck: &D2Status,
        mode: D2CaptureMode,
        capture_id: WireUuid,
    ) -> Result<(), D2RuntimeError> {
        let carrier_slots = match deck.controls.routing {
            super::D2Routing::A => deck.source_a.latent_slot_count,
            super::D2Routing::B => deck.source_b.latent_slot_count,
        };
        let expected_target = match mode {
            D2CaptureMode::Snapshot => carrier_slots,
            D2CaptureMode::LiveCapture => 0,
        };
        if capture.capture_id != capture_id
            || capture.mode != mode
            || capture.state != D2CaptureState::AwaitingReset
            || capture.structural_carrier != deck.controls.routing
            || capture.latent_slots != 0
            || capture.current_generation != Some(deck.stream_generation)
            || capture.minimum_new_generation != deck.stream_generation.checked_add(1)
            || capture.target_latent_slots != Some(expected_target)
            || capture.stream_generation.is_some()
            || capture.finalize_after_latent_slots.is_some()
            || capture.reason.is_some()
            || capture.receipt.is_some()
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    fn validate_active_capture_status(
        capture: &D2CaptureStatus,
        deck: &D2Status,
        mode: D2CaptureMode,
        capture_id: WireUuid,
    ) -> Result<(), D2RuntimeError> {
        if capture.capture_id != capture_id
            || capture.mode != mode
            || capture.state != D2CaptureState::Capturing
            || capture.structural_carrier != deck.controls.routing
            || capture.latent_slots != 0
            || capture.current_generation.is_some()
            || capture.minimum_new_generation.is_some()
            || capture.target_latent_slots.is_some()
            || capture.stream_generation != Some(deck.stream_generation)
            || capture.finalize_after_latent_slots.is_some()
            || capture.reason.is_some()
            || capture.receipt.is_some()
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    fn validate_capture_identity(
        status: &D2CaptureStatus,
        active: &ActiveCapture,
    ) -> Result<(), D2RuntimeError> {
        if status.capture_id != active.binding.capture_id() || status.mode != active.mode {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    async fn bind_ring(
        client: &mut WorkerClient,
        owner: WindowsRgbRingOwner,
    ) -> Result<WindowsRgbRingOwner, D2RuntimeError> {
        ensure_zero_ring(owner.state().map_err(|_| D2RuntimeError::ring())?)?;
        let binding = client
            .with_process_handle(|process| owner.duplicate_into(process))
            .map_err(|_| D2RuntimeError::ring())?
            .map_err(|_| D2RuntimeError::ring())?;
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
            return Err(D2RuntimeError::worker_protocol());
        };
        if bound.layout_version != request.layout_version
            || bound.mapping_bytes != request.mapping_bytes
            || bound.ring_id != request.ring_id
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        ensure_zero_ring(owner.state().map_err(|_| D2RuntimeError::ring())?)?;
        Ok(owner)
    }

    struct RuntimeActor {
        app: AppHandle,
        client: WorkerClient,
        owner: WindowsRgbRingOwner,
        consumer: WindowsRgbRingConsumer,
        output: NativeOutput,
        status: D2Status,
        shared_status: Arc<Mutex<D2StatusView>>,
        shared_capture_status: Arc<Mutex<D2CaptureView>>,
        closed: Arc<AtomicBool>,
        pending_frame: Option<latentdeck_gpu::ring::RgbaFrame>,
        presented_sequence: u64,
        frame_clock: FrameClock,
        app_local_data: PathBuf,
        library_importer: super::LibraryImporter,
        capture: Option<ActiveCapture>,
        capture_finalizer: Option<CaptureFinalizer>,
        capture_coordinator: CaptureCoordinator,
    }

    struct ActiveCapture {
        mode: D2CaptureMode,
        binding: CaptureSpoolBinding,
        output: PathBuf,
    }

    impl ActiveCapture {
        fn cleanup(&self) {
            self.binding.cleanup();
        }
    }

    struct CaptureFinalizer {
        capture_id: WireUuid,
        task: JoinHandle<Result<(String, String), D2RuntimeError>>,
    }

    struct CaptureFinalizerCompletion {
        capture_id: WireUuid,
        result: Result<(String, String), D2RuntimeError>,
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
                    self.fail(D2RuntimeError::ring()).await;
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
                        // Commands win before a long worker call; presentation
                        // was already checked first.
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
                        // A one-shot deadline is re-armed after every attempt;
                        // an overdue periodic interval can never remain ready
                        // and monopolize the actor.
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
                    self.fail(D2RuntimeError::worker_process_exited()).await;
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
            self.fail(D2RuntimeError::worker_process_exited()).await;
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
            // Tauri has no cancellation surface for an invoke already being
            // executed, but a queued command whose caller disappeared must
            // never become a later, unobserved state mutation.
            if command.reply_is_closed() {
                return false;
            }
            match command {
                RuntimeCommand::ControlsSet { controls, reply } => {
                    let result = self.controls_set(controls).await;
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
                        .map_err(|error| D2RuntimeError::output(error.code()));
                    self.finish_command(result, reply).await
                }
                RuntimeCommand::ToggleFullscreen { reply } => {
                    let result = self
                        .output
                        .toggle_fullscreen()
                        .map_err(|error| D2RuntimeError::output(error.code()));
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
            result: Result<T, D2RuntimeError>,
            reply: oneshot::Sender<Result<T, D2RuntimeError>>,
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
            controls: D2Controls,
        ) -> Result<D2ControlsAckView, D2RuntimeError> {
            controls
                .validate()
                .map_err(|_| D2RuntimeError::invalid_controls())?;
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_d2_controls_set(
                    D2ControlsSet {
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
                return Err(D2RuntimeError::worker_protocol());
            }
            self.status.controls = ack.controls.clone();
            self.publish_status()?;
            Ok(D2ControlsAckView {
                controls: (&ack.controls).into(),
                requires_causal_reset: false,
            })
        }

        async fn transport_set(
            &mut self,
            transport: D2Transport,
        ) -> Result<D2TransportAckView, D2RuntimeError> {
            let was_active = transport_active(self.status.transport);
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_d2_transport_set(
                    D2TransportSet {
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
                return Err(D2RuntimeError::worker_protocol());
            }
            self.status.transport = ack.transport;
            if !was_active && transport_active(ack.transport) {
                self.frame_clock.restart();
            }
            self.publish_status()?;
            Ok(D2TransportAckView {
                transport: ack.transport.into(),
                requires_causal_reset: false,
            })
        }

        async fn seed_set(&mut self, seed: u64) -> Result<D2SeedAckView, D2RuntimeError> {
            if seed > MAX_D2_SAFE_INTEGER {
                return Err(D2RuntimeError::invalid_seed());
            }
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_d2_seed_set(
                    D2SeedSet {
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
                return Err(D2RuntimeError::worker_protocol());
            }
            self.status.seed = ack.seed;
            self.publish_status()?;
            Ok(D2SeedAckView {
                seed: ack.seed,
                requires_causal_reset: false,
            })
        }

        async fn restart(&mut self) -> Result<D2StatusView, D2RuntimeError> {
            self.ensure_worker_session_budget()?;
            let barrier = self
                .client
                .deck_d2_restart(
                    D2Restart {
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
                return Err(D2RuntimeError::worker_protocol());
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
            Ok(D2StatusView::from_status(&self.status))
        }

        async fn capture_start_command(
            &mut self,
            mode: D2CaptureMode,
            output: PathBuf,
            reply: oneshot::Sender<Result<D2CaptureView, D2RuntimeError>>,
        ) -> bool {
            match self.begin_capture(mode, output).await {
                Ok(view) => {
                    let _ = reply.send(Ok(view));
                    false
                }
                Err(error) => {
                    let terminal = error.terminal;
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
            mode: D2CaptureMode,
            output: PathBuf,
        ) -> Result<D2CaptureView, D2RuntimeError> {
            let result = self.begin_capture_inner(mode, output).await;
            if result.is_err() && self.capture.is_none() && self.capture_coordinator.is_active() {
                let failed = self.capture_coordinator.fail();
                let _ = self.publish_capture_view(failed);
            }
            result
        }

        async fn begin_capture_inner(
            &mut self,
            mode: D2CaptureMode,
            output: PathBuf,
        ) -> Result<D2CaptureView, D2RuntimeError> {
            if self.capture.is_some() || self.capture_coordinator.is_active() {
                return Err(D2RuntimeError::capture_host(CaptureHostError {
                    code: "capture.already_active",
                    message: "Only one LD-D2 capture may be active.",
                }));
            }
            let output = validate_output_path(output).map_err(D2RuntimeError::capture_host)?;
            self.ensure_worker_session_budget()?;
            let capture_id = WireUuid::new_v4();
            let binding = CaptureSpoolBinding::create(&self.app_local_data, capture_id)
                .map_err(D2RuntimeError::capture_host)?;
            self.capture_coordinator
                .begin(capture_id, mode)
                .map_err(D2RuntimeError::capture_host)?;
            let temporary_root = path_for_protocol(binding.root())?;
            let started = self
                .client
                .deck_d2_capture_start(
                    D2CaptureStart {
                        deck_id: self.status.deck_id.clone(),
                        deck_revision: self.status.deck_revision,
                        capture_id,
                        mode,
                        temporary_root,
                        max_latent_slots: APP_CAPTURE_MAX_LATENT_SLOTS,
                        max_visual_bytes: APP_CAPTURE_MAX_VISUAL_BYTES,
                    },
                    COMMAND_TIMEOUT,
                )
                .await
                .map_err(map_worker_error);
            let started = match started {
                Ok(value) => value,
                Err(error) => {
                    let failed = self.capture_coordinator.fail();
                    let _ = self.publish_capture_view(failed);
                    return Err(error);
                }
            };
            validate_capture_start_status(&started, &self.status, mode, capture_id)?;
            let awaiting = self
                .capture_coordinator
                .observe(&started)
                .map_err(|_| D2RuntimeError::worker_protocol())?;
            self.publish_capture_view(awaiting)?;

            let plan = CausalResetPlan::from_barrier(
                self.status.stream_generation,
                started
                    .current_generation
                    .ok_or_else(D2RuntimeError::worker_protocol)?,
                started
                    .minimum_new_generation
                    .ok_or_else(D2RuntimeError::worker_protocol)?,
                &[D2ResetReason::TransportRestart],
            )?;
            self.apply_reset(plan).await?;
            self.ensure_worker_session_budget()?;
            let active_status = self
                .client
                .deck_d2_capture_status(
                    D2CaptureStatusRequest {
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
                .map_err(|_| D2RuntimeError::worker_protocol())?;
            self.capture = Some(ActiveCapture {
                mode,
                binding,
                output,
            });
            self.publish_capture_view(view.clone())?;
            Ok(view)
        }

        async fn capture_stop_command(
            &mut self,
            reply: oneshot::Sender<Result<D2CaptureView, D2RuntimeError>>,
        ) -> bool {
            let Some(capture) = self.capture.as_ref() else {
                let error = D2RuntimeError::capture_host(CaptureHostError {
                    code: "capture.not_active",
                    message: "No Live Capture is active.",
                });
                let _ = reply.send(Err(error));
                return false;
            };
            if capture.mode != D2CaptureMode::LiveCapture {
                let error = D2RuntimeError::capture_host(CaptureHostError {
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
                .deck_d2_capture_stop(
                    D2CaptureStop {
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
                        D2CaptureState::StopArmed | D2CaptureState::Finished
                    ) {
                        let error = D2RuntimeError::worker_protocol();
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

        async fn capture_status_command(&mut self) -> Result<D2CaptureView, D2RuntimeError> {
            let Some(capture) = self.capture.as_ref() else {
                return Ok(self.capture_coordinator.view());
            };
            self.ensure_worker_session_budget()?;
            let capture_id = capture.binding.capture_id();
            let status = self
                .client
                .deck_d2_capture_status(
                    D2CaptureStatusRequest {
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
            status: D2CaptureStatus,
        ) -> Result<D2CaptureView, D2RuntimeError> {
            let capture = self
                .capture
                .as_ref()
                .ok_or_else(D2RuntimeError::worker_protocol)?;
            validate_capture_identity(&status, capture)?;
            let view = self
                .capture_coordinator
                .observe(&status)
                .map_err(|_| D2RuntimeError::worker_protocol())?;
            self.publish_capture_view(view.clone())?;
            match status.state {
                D2CaptureState::Finished => {
                    self.start_capture_finalizer(status)?;
                    Ok(view)
                }
                D2CaptureState::Aborted => {
                    let capture = self
                        .capture
                        .take()
                        .ok_or_else(D2RuntimeError::worker_protocol)?;
                    capture.cleanup();
                    let error = D2RuntimeError::capture_host(CaptureHostError {
                        code: "capture.aborted",
                        message: "The worker aborted capture safely.",
                    });
                    self.publish_capture_error(&error);
                    Ok(view)
                }
                _ => Ok(view),
            }
        }

        fn start_capture_finalizer(
            &mut self,
            status: D2CaptureStatus,
        ) -> Result<D2CaptureView, D2RuntimeError> {
            if self.capture_finalizer.is_some() {
                return Err(D2RuntimeError::capture_host(CaptureHostError {
                    code: "capture.already_finalizing",
                    message: "The LD-D2 capture finalizer is already running.",
                }));
            }
            let capture = self
                .capture
                .take()
                .ok_or_else(D2RuntimeError::worker_protocol)?;
            let capture_id = capture.binding.capture_id();
            let library_importer = self.library_importer.clone();
            let task = tokio::spawn(async move {
                let result =
                    Self::finalize_capture_inner(library_importer, &capture, &status).await;
                capture.cleanup();
                result
            });
            self.capture_finalizer = Some(CaptureFinalizer { capture_id, task });
            Ok(self.capture_coordinator.view())
        }

        fn accept_capture_finalizer(
            &mut self,
            completion: CaptureFinalizerCompletion,
        ) -> Result<(), D2RuntimeError> {
            let capture_id = completion.capture_id.to_string();
            if self.capture_coordinator.view().capture_id.as_deref() != Some(&capture_id) {
                return Err(D2RuntimeError::worker_protocol());
            }
            match completion.result {
                Ok((cartridge_id, archive_sha256)) => {
                    let view = self
                        .capture_coordinator
                        .complete(cartridge_id, archive_sha256)
                        .map_err(D2RuntimeError::capture_host)?;
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

        async fn finalize_capture_inner(
            library_importer: super::LibraryImporter,
            capture: &ActiveCapture,
            status: &D2CaptureStatus,
        ) -> Result<(String, String), D2RuntimeError> {
            let receipt = status
                .receipt
                .as_ref()
                .ok_or_else(D2RuntimeError::worker_protocol)?;
            let payload = capture
                .binding
                .bind_finished_receipt(receipt)
                .map_err(D2RuntimeError::capture_host)?;
            let cartridge_id = WireUuid::new_v4();
            let request = resample_request_from_receipt(receipt, cartridge_id)
                .map_err(D2RuntimeError::capture_host)?;
            let output = capture.output.clone();
            let packed = tauri::async_runtime::spawn_blocking(move || {
                pack_resample_atomic(&request, payload, output, &WriteOptions::default())
            })
            .await
            .map_err(|_| D2RuntimeError::capture_finalize())?
            .map_err(|_| D2RuntimeError::capture_finalize())?;
            let archive_sha256 = packed.validation.archive_sha256.to_string();
            let imported = library_importer
                .import_generated(packed.output_path)
                .await
                .map_err(|_| D2RuntimeError::capture_finalize())?;
            if imported.as_str() != archive_sha256 {
                return Err(D2RuntimeError::capture_finalize());
            }
            Ok((cartridge_id.to_string(), archive_sha256))
        }

        async fn refresh_status(&mut self) -> Result<D2StatusView, D2RuntimeError> {
            self.ensure_worker_session_budget()?;
            let status = self
                .client
                .deck_d2_status(COMMAND_TIMEOUT)
                .await
                .map_err(map_worker_error)?;
            if status.deck_id != self.status.deck_id
                || status.deck_revision != self.status.deck_revision
                || status.stream_generation != self.status.stream_generation
            {
                return Err(D2RuntimeError::worker_protocol());
            }
            self.status = status;
            self.publish_status()
        }

        async fn schedule_once(&mut self) -> Result<(), D2RuntimeError> {
            if !transport_active(self.status.transport) {
                return Ok(());
            }
            let before = self.owner.state().map_err(|_| D2RuntimeError::ring())?;
            if !before.can_publish(MAX_FRAMES_PER_D2_SLOT) {
                return Ok(());
            }
            self.ensure_worker_session_budget()?;
            let request = D2ProcessSlot {
                deck_id: self.status.deck_id.clone(),
                deck_revision: self.status.deck_revision,
                stream_generation: self.status.stream_generation,
            };
            let result = self
                .client
                .deck_d2_process_slot(request, COMMAND_TIMEOUT)
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
            // A loop boundary may finish Live Capture in the same worker turn
            // that returns a ResetBarrier. Adopt that reset (and its new ring)
            // before observing Finished and allowing any spool finalization.
            self.handle_process_ack(ack, before).await?;
            if let Some(capture) = self.capture.as_ref() {
                self.ensure_worker_session_budget()?;
                let capture_id = capture.binding.capture_id();
                let status = self
                    .client
                    .deck_d2_capture_status(
                        D2CaptureStatusRequest {
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

        fn ensure_worker_session_budget(&self) -> Result<(), D2RuntimeError> {
            if session_rotation_required(
                self.client.remaining_inbound_message_budget(),
                self.client.remaining_outbound_message_budget(),
            ) {
                Err(D2RuntimeError::session_rotation_required())
            } else {
                Ok(())
            }
        }

        async fn handle_process_ack(
            &mut self,
            ack: D2ProcessSlotAck,
            before: RingState,
        ) -> Result<(), D2RuntimeError> {
            match ack {
                D2ProcessSlotAck::DecodedSlot {
                    deck_id,
                    deck_revision,
                    stream_generation,
                    stream_sequence,
                    playhead_a,
                    playhead_b,
                    transport,
                    decoded_start_frame,
                    decoded_frame_count,
                    ring_first_sequence,
                    ring_last_sequence_exclusive,
                    provenance_json: _,
                } => {
                    let after = self.owner.state().map_err(|_| D2RuntimeError::ring())?;
                    validate_decoded_slot(&DecodedSlotReceipt {
                        status: &self.status,
                        deck_id: &deck_id,
                        deck_revision,
                        stream_generation,
                        stream_sequence,
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
                        transport,
                        decoded_start_frame,
                        decoded_frame_count,
                    )?;
                    self.status.pending_reset = false;
                    self.status.pending_reset_reasons = BoundedVec::default();
                    self.publish_status()?;
                    Ok(())
                }
                D2ProcessSlotAck::ResetBarrier {
                    deck_id,
                    deck_revision,
                    current_generation,
                    minimum_new_generation,
                    reasons,
                } => {
                    if deck_id != self.status.deck_id || deck_revision != self.status.deck_revision
                    {
                        return Err(D2RuntimeError::worker_protocol());
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
                D2ProcessSlotAck::Paused {
                    deck_id,
                    deck_revision,
                    stream_generation,
                    playhead_a,
                    playhead_b,
                    transport,
                } => {
                    adopt_paused_progress(
                        &mut self.status,
                        &deck_id,
                        deck_revision,
                        stream_generation,
                        playhead_a,
                        playhead_b,
                        transport,
                    )?;
                    self.publish_status().map(|_| ())
                }
            }
        }

        async fn apply_reset(&mut self, plan: CausalResetPlan) -> Result<(), D2RuntimeError> {
            self.pending_frame = None;
            self.ensure_worker_session_budget()?;
            let ack = self
                .client
                .deck_d2_reset(
                    D2Reset {
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
                return Err(D2RuntimeError::worker_protocol());
            }
            plan.validate_ack(
                ack.stream_generation,
                &ack.reasons,
                ack.causal_state_cleared,
            )?;
            self.owner
                .adopt_generation(plan.new_generation)
                .map_err(|_| D2RuntimeError::ring())?;
            self.consumer
                .adopt_generation(plan.new_generation)
                .map_err(|_| D2RuntimeError::ring())?;
            ensure_zero_ring(self.owner.state().map_err(|_| D2RuntimeError::ring())?)?;
            ensure_zero_ring(self.consumer.state().map_err(|_| D2RuntimeError::ring())?)?;
            self.presented_sequence = 0;
            self.frame_clock.restart();
            self.status.stream_generation = ack.stream_generation;
            self.status.stream_sequence = 0;
            self.status.playhead_a = ack.playhead_a;
            self.status.playhead_b = ack.playhead_b;
            self.status.pending_reset = false;
            self.status.pending_reset_reasons = BoundedVec::default();
            self.status.decoded_start_frame = 0;
            self.publish_status().map(|_| ())
        }

        fn present_once(&mut self) -> Result<(), D2RuntimeError> {
            if self.pending_frame.is_none() {
                match self
                    .consumer
                    .try_read()
                    .map_err(|_| D2RuntimeError::ring())?
                {
                    ReadStatus::Frame(frame) => self.pending_frame = Some(frame),
                    ReadStatus::Empty => return Ok(()),
                }
            }
            let frame = self
                .pending_frame
                .as_ref()
                .ok_or_else(D2RuntimeError::ring)?;
            let expected = self
                .presented_sequence
                .checked_add(1)
                .ok_or_else(D2RuntimeError::ring)?;
            if frame.generation() != self.status.stream_generation || frame.sequence() != expected {
                return Err(D2RuntimeError::ring());
            }
            let outcome = self
                .output
                .present_padded_rgba(
                    frame.width(),
                    frame.height(),
                    frame.row_stride(),
                    frame.padded_rgba(),
                )
                .map_err(|error| D2RuntimeError::output(error.code()))?;
            if matches!(
                outcome,
                PresentOutcome::Presented | PresentOutcome::PresentedAndReconfigured
            ) {
                self.presented_sequence = expected;
                self.pending_frame = None;
            }
            Ok(())
        }

        fn publish_status(&self) -> Result<D2StatusView, D2RuntimeError> {
            let view = D2StatusView::from_status(&self.status);
            replace_shared_status(&self.shared_status, view.clone())?;
            let _ = self.app.emit("deck-d2-status", view.clone());
            Ok(view)
        }

        fn publish_capture_view(
            &self,
            view: D2CaptureView,
        ) -> Result<D2CaptureView, D2RuntimeError> {
            replace_shared_capture_status(&self.shared_capture_status, view.clone())?;
            let _ = self.app.emit("deck-d2-capture", view.clone());
            Ok(view)
        }

        fn publish_capture_error(&self, error: &D2RuntimeError) {
            let _ = self.app.emit("deck-d2-capture-error", error.event());
        }

        async fn settle_capture_for_shutdown(&mut self, error: &D2RuntimeError) {
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

        fn abort_active_capture(&mut self, error: &D2RuntimeError) {
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

        async fn fail(&mut self, error: D2RuntimeError) {
            if self.closed.swap(true, Ordering::AcqRel) {
                return;
            }
            self.status.transport.playing_a = false;
            self.status.transport.playing_b = false;
            let stopped = D2StatusView::stopped_from(&self.status);
            let _ = replace_shared_status(&self.shared_status, stopped.clone());
            let _ = self.app.emit("deck-d2-status", stopped);
            let _ = self.app.emit("deck-d2-error", error.event());
            let _ = destroy_output(&self.output);
            let _ = stop_worker(&mut self.client, ShutdownReason::Recovery).await;
            self.settle_capture_for_shutdown(&error).await;
        }

        async fn stop(&mut self, reason: ShutdownReason) -> Result<(), D2RuntimeError> {
            if self.closed.swap(true, Ordering::AcqRel) {
                return Ok(());
            }
            self.status.transport.playing_a = false;
            self.status.transport.playing_b = false;
            let output_result = destroy_output(&self.output);
            let worker_result = stop_worker(&mut self.client, reason).await;
            self.settle_capture_for_shutdown(&D2RuntimeError::runtime_unavailable())
                .await;
            let stopped = D2StatusView::stopped_from(&self.status);
            let _ = replace_shared_status(&self.shared_status, stopped.clone());
            let _ = self.app.emit("deck-d2-status", stopped);
            output_result?;
            worker_result
        }
    }

    enum RuntimeCommand {
        ControlsSet {
            controls: D2Controls,
            reply: oneshot::Sender<Result<D2ControlsAckView, D2RuntimeError>>,
        },
        TransportSet {
            transport: D2Transport,
            reply: oneshot::Sender<Result<D2TransportAckView, D2RuntimeError>>,
        },
        SeedSet {
            seed: u64,
            reply: oneshot::Sender<Result<D2SeedAckView, D2RuntimeError>>,
        },
        Restart {
            reply: oneshot::Sender<Result<D2StatusView, D2RuntimeError>>,
        },
        CaptureStart {
            mode: D2CaptureMode,
            output: PathBuf,
            reply: oneshot::Sender<Result<D2CaptureView, D2RuntimeError>>,
        },
        CaptureStop {
            reply: oneshot::Sender<Result<D2CaptureView, D2RuntimeError>>,
        },
        CaptureStatus {
            reply: oneshot::Sender<Result<D2CaptureView, D2RuntimeError>>,
        },
        Status {
            reply: oneshot::Sender<Result<D2StatusView, D2RuntimeError>>,
        },
        Resize {
            width: u32,
            height: u32,
            reply: oneshot::Sender<Result<ResizeOutcome, D2RuntimeError>>,
        },
        ToggleFullscreen {
            reply: oneshot::Sender<Result<bool, D2RuntimeError>>,
        },
        SpoutStatus {
            reply: oneshot::Sender<Result<NativeSpoutStatus, D2RuntimeError>>,
        },
        ConfigureSpout {
            name: Option<String>,
            enabled: Option<bool>,
            reply: oneshot::Sender<Result<NativeSpoutStatus, D2RuntimeError>>,
        },
        Shutdown {
            reply: oneshot::Sender<Result<(), D2RuntimeError>>,
        },
    }

    impl RuntimeCommand {
        fn reply_is_closed(&self) -> bool {
            match self {
                Self::ControlsSet { reply, .. } => reply.is_closed(),
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
                // Shutdown owns cleanup even if its original waiter is
                // cancelled; Drop also uses this path without a live waiter.
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
            .map_err(|_| D2RuntimeError::capture_finalize())
            .and_then(|result| result);
        CaptureFinalizerCompletion { capture_id, result }
    }

    async fn send_bounded<T>(
        sender: &mpsc::Sender<T>,
        command: T,
        deadline: Duration,
    ) -> Result<(), D2RuntimeError> {
        timeout(deadline, sender.send(command))
            .await
            .map_err(|_| D2RuntimeError::runtime_timeout())?
            .map_err(|_| D2RuntimeError::runtime_unavailable())
    }

    async fn wait_for_actor_cleanup(
        mut completion: watch::Receiver<bool>,
    ) -> Result<(), D2RuntimeError> {
        if *completion.borrow() {
            return Ok(());
        }
        completion
            .changed()
            .await
            .map_err(|_| D2RuntimeError::runtime_cleanup())?;
        if *completion.borrow() {
            Ok(())
        } else {
            Err(D2RuntimeError::runtime_cleanup())
        }
    }

    const fn session_rotation_required(
        inbound_remaining: usize,
        outbound_remaining: usize,
    ) -> bool {
        inbound_remaining <= SESSION_SHUTDOWN_MESSAGE_RESERVE
            || outbound_remaining <= SESSION_SHUTDOWN_MESSAGE_RESERVE
    }

    async fn receive_owned<T>(receiver: oneshot::Receiver<T>) -> Result<T, D2RuntimeError> {
        receiver
            .await
            .map_err(|_| D2RuntimeError::runtime_unavailable())
    }

    struct DecodedSlotReceipt<'a> {
        status: &'a D2Status,
        deck_id: &'a str,
        deck_revision: u64,
        stream_generation: u64,
        stream_sequence: u64,
        decoded_start_frame: u64,
        decoded_frame_count: u32,
        ring_first_sequence: u64,
        ring_last_sequence_exclusive: u64,
        before: RingState,
        after: RingState,
    }

    fn validate_decoded_slot(receipt: &DecodedSlotReceipt<'_>) -> Result<(), D2RuntimeError> {
        let expected_stream_sequence = receipt.status.stream_sequence;
        let expected_ring_first = receipt
            .before
            .producer_sequence()
            .checked_add(1)
            .ok_or_else(D2RuntimeError::ring)?;
        let expected_ring_last_exclusive = expected_ring_first
            .checked_add(u64::from(receipt.decoded_frame_count))
            .ok_or_else(D2RuntimeError::ring)?;
        let expected_occupancy = receipt
            .before
            .occupancy()
            .checked_add(receipt.decoded_frame_count)
            .ok_or_else(D2RuntimeError::ring)?;
        let expected_available_capacity = receipt
            .before
            .available_capacity()
            .checked_sub(receipt.decoded_frame_count)
            .ok_or_else(D2RuntimeError::ring)?;
        if receipt.deck_id != receipt.status.deck_id
            || receipt.deck_revision != receipt.status.deck_revision
            || receipt.stream_generation != receipt.status.stream_generation
            || receipt.stream_sequence != expected_stream_sequence
            || receipt.decoded_start_frame != receipt.status.decoded_start_frame
            || !(1..=MAX_FRAMES_PER_D2_SLOT).contains(&receipt.decoded_frame_count)
            || receipt.ring_first_sequence != expected_ring_first
            || receipt.ring_last_sequence_exclusive != expected_ring_last_exclusive
            || receipt.after.producer_sequence()
                != receipt
                    .ring_last_sequence_exclusive
                    .checked_sub(1)
                    .ok_or_else(D2RuntimeError::ring)?
            || receipt.after.consumer_sequence() != receipt.before.consumer_sequence()
            || receipt.after.occupancy() != expected_occupancy
            || receipt.after.available_capacity() != expected_available_capacity
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the closed decoded-slot ack fields.
    fn adopt_decoded_progress(
        status: &mut D2Status,
        stream_sequence: u64,
        playhead_a: u64,
        playhead_b: u64,
        transport: D2Transport,
        decoded_start_frame: u64,
        decoded_frame_count: u32,
    ) -> Result<(), D2RuntimeError> {
        if stream_sequence != status.stream_sequence
            || decoded_start_frame != status.decoded_start_frame
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        status.stream_sequence = stream_sequence
            .checked_add(1)
            .ok_or_else(D2RuntimeError::worker_protocol)?;
        status.playhead_a = playhead_a;
        status.playhead_b = playhead_b;
        status.transport = transport;
        status.decoded_start_frame = decoded_start_frame
            .checked_add(u64::from(decoded_frame_count))
            .ok_or_else(D2RuntimeError::worker_protocol)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the closed paused ack fields.
    fn adopt_paused_progress(
        status: &mut D2Status,
        deck_id: &str,
        deck_revision: u64,
        stream_generation: u64,
        playhead_a: u64,
        playhead_b: u64,
        transport: D2Transport,
    ) -> Result<(), D2RuntimeError> {
        if deck_id != status.deck_id
            || deck_revision != status.deck_revision
            || stream_generation != status.stream_generation
            || transport_active(transport)
        {
            return Err(D2RuntimeError::worker_protocol());
        }
        status.playhead_a = playhead_a;
        status.playhead_b = playhead_b;
        status.transport = transport;
        Ok(())
    }

    fn ensure_zero_ring(state: RingState) -> Result<(), D2RuntimeError> {
        if state.producer_sequence() == 0
            && state.consumer_sequence() == 0
            && state.occupancy() == 0
        {
            Ok(())
        } else {
            Err(D2RuntimeError::ring())
        }
    }

    fn destroy_output(output: &NativeOutput) -> Result<(), D2RuntimeError> {
        let _ = output.hide();
        output
            .window()
            .destroy()
            .map_err(|_| D2RuntimeError::output(NativeOutputError::WindowVisibility.code()))
    }

    async fn stop_worker(
        client: &mut WorkerClient,
        reason: ShutdownReason,
    ) -> Result<(), D2RuntimeError> {
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
            .map_err(|_| D2RuntimeError::worker_shutdown())
    }

    fn map_worker_error(error: WorkerClientError) -> D2RuntimeError {
        match error {
            WorkerClientError::Remote(remote) => D2RuntimeError::owned(
                wire_error_code(remote.code),
                "The isolated H3 LD-D2 worker rejected a typed request.",
                remote.retryable,
                remote.fatal,
            ),
            WorkerClientError::Supervisor(_)
            | WorkerClientError::CommandTimeout(_)
            | WorkerClientError::HeartbeatTimeout(_)
            | WorkerClientError::UnexpectedAck { .. }
            | WorkerClientError::UnexpectedReply => D2RuntimeError::worker_protocol(),
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

    fn path_for_protocol(path: &Path) -> Result<String, D2RuntimeError> {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(D2RuntimeError::input_contract)
    }

    fn parse_wire_uuid(value: &str) -> Result<WireUuid, D2RuntimeError> {
        let deserializer = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(value);
        WireUuid::deserialize(deserializer).map_err(|_| D2RuntimeError::input_contract())
    }

    fn replace_shared_status(
        shared: &Arc<Mutex<D2StatusView>>,
        view: D2StatusView,
    ) -> Result<(), D2RuntimeError> {
        let mut guard = shared
            .lock()
            .map_err(|_| D2RuntimeError::state_poisoned())?;
        *guard = view;
        Ok(())
    }

    fn replace_shared_capture_status(
        shared: &Arc<Mutex<D2CaptureView>>,
        view: D2CaptureView,
    ) -> Result<(), D2RuntimeError> {
        let mut guard = shared
            .lock()
            .map_err(|_| D2RuntimeError::state_poisoned())?;
        *guard = view;
        Ok(())
    }

    const fn transport_active(transport: D2Transport) -> bool {
        transport.playing_a || transport.playing_b
    }

    /// Keep at most one decoder slot in flight in the presentation queue. The
    /// D2 worker may publish four frames per latent slot, so requiring a fully
    /// drained queue bounds stale-control latency to one slot and prevents the
    /// scheduler from filling all 24 ABI slots ahead of the native clock.
    fn decode_watermark_allows(state: RingState, pending_frame: bool) -> bool {
        decode_watermark(state.occupancy(), pending_frame, state.available_capacity())
    }

    const fn decode_watermark(
        occupancy: u32,
        pending_frame: bool,
        available_capacity: u32,
    ) -> bool {
        occupancy == 0 && !pending_frame && available_capacity >= MAX_FRAMES_PER_D2_SLOT
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
        fn new(numerator: u64, denominator: u64) -> Result<Self, D2RuntimeError> {
            if numerator == 0
                || denominator == 0
                || frame_offset_ns(numerator, denominator, 1)? == 0
            {
                return Err(D2RuntimeError::input_contract());
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

        fn next_deadline(&self) -> Result<Instant, D2RuntimeError> {
            let offset = frame_offset_ns(self.numerator, self.denominator, self.next_tick)?;
            self.epoch
                .checked_add(Duration::from_nanos(offset))
                .ok_or_else(D2RuntimeError::worker_protocol)
        }

        fn advance_past(&mut self, now: Instant) {
            self.next_tick = self.next_tick.saturating_add(1);
            while self.next_deadline().is_ok_and(|deadline| deadline <= now) {
                self.next_tick = self.next_tick.saturating_add(1);
            }
        }
    }

    fn frame_offset_ns(numerator: u64, denominator: u64, tick: u64) -> Result<u64, D2RuntimeError> {
        if numerator == 0 || denominator == 0 || tick == 0 {
            return Err(D2RuntimeError::input_contract());
        }
        let value = u128::from(tick)
            .checked_mul(u128::from(denominator))
            .and_then(|value| value.checked_mul(1_000_000_000))
            .ok_or_else(D2RuntimeError::worker_protocol)?
            / u128::from(numerator);
        u64::try_from(value).map_err(|_| D2RuntimeError::worker_protocol())
    }

    #[cfg(test)]
    mod tests {
        use latentdeck_control::CommandName;

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
                CommandName::DeckD2ProcessSlot,
            ));
            assert_eq!(error.code, "worker.protocol_failed");
            assert!(error.terminal);
            assert!(!error.message.contains('\\'));
            assert!(!error.message.contains(':'));
        }

        #[test]
        fn session_rotation_preserves_pipe_backlog_and_orderly_shutdown_reserve() {
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

            let error = D2RuntimeError::session_rotation_required();
            assert_eq!(error.code, "worker.session_rotation_required");
            assert!(error.recoverable);
            assert!(error.terminal);
            assert!(!error.message.contains('\\'));
            assert!(!error.message.contains(':'));
        }

        #[tokio::test]
        async fn actor_cleanup_signal_is_distinct_from_the_closed_state() {
            let (completion_sender, completion) = watch::channel(false);
            let mut waiting = Box::pin(wait_for_actor_cleanup(completion));
            assert!(
                timeout(Duration::from_millis(10), waiting.as_mut())
                    .await
                    .is_err(),
                "a closed command surface must not imply completed cleanup"
            );
            completion_sender.send_replace(true);
            waiting.await.expect("actor cleanup barrier");

            let (dropped_sender, dropped_completion) = watch::channel(false);
            drop(dropped_sender);
            let error = wait_for_actor_cleanup(dropped_completion)
                .await
                .expect_err("a lost actor cannot satisfy cleanup ownership");
            assert_eq!(error.code, "deck.runtime_cleanup_failed");
            assert!(error.terminal);
            assert!(!error.message.contains('\\'));
            assert!(!error.message.contains(':'));
        }

        #[tokio::test]
        async fn runtime_shutdown_waits_for_cleanup_after_commands_are_closed() {
            let (sender, _receiver) = mpsc::channel(1);
            let closed = Arc::new(AtomicBool::new(true));
            let (cleanup_sender, cleanup_complete) = watch::channel(false);
            let runtime = D2Runtime {
                sender,
                closed,
                cleanup_complete,
                _task: tauri::async_runtime::spawn(async {}),
            };
            let mut shutdown = Box::pin(runtime.shutdown());
            assert!(
                timeout(Duration::from_millis(10), shutdown.as_mut())
                    .await
                    .is_err(),
                "closed commands must not bypass actor-owned cleanup"
            );
            cleanup_sender.send_replace(true);
            shutdown.await.expect("cleanup-complete shutdown");
        }

        #[tokio::test]
        async fn runtime_shutdown_waits_to_enqueue_behind_a_full_command_queue() {
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
            let runtime = D2Runtime {
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
        async fn runtime_drop_queues_shutdown_without_preclosing_actor_cleanup() {
            let (sender, mut receiver) = mpsc::channel(1);
            let closed = Arc::new(AtomicBool::new(false));
            let (_cleanup_sender, cleanup_complete) = watch::channel(false);
            let runtime = D2Runtime {
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
        fn observed_worker_exit_is_recoverable_terminal_and_path_free() {
            let error = D2RuntimeError::worker_process_exited();
            assert_eq!(error.code, "worker.process_exited");
            assert!(error.recoverable);
            assert!(error.terminal);
            assert!(!error.message.contains('\\'));
            assert!(!error.message.contains(':'));
        }

        #[test]
        fn runtime_error_event_uses_the_frontend_code_detail_contract() {
            let event = serde_json::to_value(D2RuntimeError::runtime_cleanup().event())
                .expect("serializable error event");
            assert_eq!(event["code"], "deck.runtime_cleanup_failed");
            assert!(event["detail"].is_string());
            assert!(event.get("message").is_none());
        }

        #[test]
        fn pending_finalizer_never_blocks_presentation_controls_or_decode_scheduling() {
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Pending, true, true, true),
                DueWork::Present
            );
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Pending, false, true, true),
                DueWork::Schedule
            );
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Ready, false, true, true),
                DueWork::FinalizeCapture
            );
            assert_eq!(
                due_work(CaptureFinalizerReadiness::Absent, false, true, false),
                DueWork::Wait
            );
            assert!(decode_watermark(0, false, MAX_FRAMES_PER_D2_SLOT));
            assert!(!decode_watermark(1, false, MAX_FRAMES_PER_D2_SLOT));
            assert!(!decode_watermark(0, true, MAX_FRAMES_PER_D2_SLOT));
            assert!(!decode_watermark(0, false, MAX_FRAMES_PER_D2_SLOT - 1));
        }

        #[tokio::test]
        async fn shutdown_join_waits_until_finalizer_owned_spool_is_cleaned() {
            let directory = tempfile::tempdir().expect("tempdir");
            let capture_id = WireUuid::new_v4();
            let binding = CaptureSpoolBinding::create(directory.path(), capture_id)
                .expect("host-created spool binding");
            let spool_root = binding.root().to_path_buf();
            let payload = spool_root.join(format!("{capture_id}.safetensors.partial"));
            std::fs::write(&payload, b"bounded synthetic payload").expect("synthetic spool");
            let (release, released) = oneshot::channel();
            let task = tokio::spawn(async move {
                let _ = released.await;
                binding.cleanup();
                Ok((WireUuid::new_v4().to_string(), "a".repeat(64)))
            });
            let mut finalizer = Some(CaptureFinalizer { capture_id, task });
            let mut waiting = Box::pin(wait_capture_finalizer(&mut finalizer));

            assert!(
                timeout(Duration::from_millis(10), waiting.as_mut())
                    .await
                    .is_err(),
                "shutdown must not detach a pending finalizer"
            );
            assert!(spool_root.exists());

            release.send(()).expect("release finalizer");
            let completion = waiting.await;
            assert_eq!(completion.capture_id, capture_id);
            assert!(completion.result.is_ok());
            finalizer.take();
            assert!(!spool_root.exists());
        }

        #[test]
        fn first_decoded_slot_zero_advances_the_host_to_next_sequence_one() {
            let mut status = actor_status();
            let transport = status.transport;

            adopt_decoded_progress(&mut status, 0, 1, 1, transport, 0, 4)
                .expect("the worker's first sequence is zero");

            assert_eq!(status.stream_sequence, 1);
            assert_eq!(status.decoded_start_frame, 4);
            assert!(
                adopt_decoded_progress(&mut status, 0, 2, 2, transport, 4, 4).is_err(),
                "replaying the acknowledged sequence must fail"
            );
        }

        #[test]
        fn worker_eos_transport_is_adopted_for_one_source_and_full_pause() {
            let mut status = actor_status();
            let only_b_playing = D2Transport {
                playing_a: false,
                playing_b: true,
                loop_a: false,
                loop_b: false,
            };
            adopt_decoded_progress(&mut status, 0, 6, 7, only_b_playing, 0, 4)
                .expect("slot A may settle while B keeps playing");
            assert_eq!(status.transport, only_b_playing);

            let paused = D2Transport {
                playing_a: false,
                playing_b: false,
                loop_a: false,
                loop_b: false,
            };
            let deck_id = status.deck_id.clone();
            let deck_revision = status.deck_revision;
            let stream_generation = status.stream_generation;
            adopt_paused_progress(
                &mut status,
                &deck_id,
                deck_revision,
                stream_generation,
                6,
                11,
                paused,
            )
            .expect("both settled sources produce a clean paused state");
            assert_eq!(status.transport, paused);
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

        #[tokio::test]
        async fn accepted_command_keeps_reply_ownership_past_a_ui_wait_interval() {
            let (reply, receiver) = oneshot::channel();
            let mut waiting = Box::pin(receive_owned(receiver));
            assert!(
                timeout(Duration::from_millis(10), waiting.as_mut())
                    .await
                    .is_err(),
                "an accepted command remains owned until the actor answers"
            );
            reply.send(42_u8).expect("actor reply");
            assert_eq!(waiting.await.expect("owned reply"), 42);
        }

        #[test]
        fn queued_command_detects_a_caller_that_disappeared_before_execution() {
            let (reply, receiver) = oneshot::channel();
            let command = RuntimeCommand::Status { reply };
            assert!(!command.reply_is_closed());
            drop(receiver);
            assert!(command.reply_is_closed());
        }

        fn actor_status() -> D2Status {
            let source = latentdeck_control::D2SourceStatus {
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "a".repeat(64),
                latent_slot_count: 12,
            };
            D2Status {
                deck_id: D2_DECK_ID.to_owned(),
                deck_revision: 1,
                operator_id: D2_OPERATOR_ID.to_owned(),
                operator_version: D2_OPERATOR_VERSION.to_owned(),
                stream_generation: 1,
                stream_sequence: 0,
                playhead_a: 0,
                playhead_b: 0,
                transport: D2Transport {
                    playing_a: true,
                    playing_b: true,
                    loop_a: false,
                    loop_b: false,
                },
                controls: D2Controls::default(),
                seed: 42,
                pending_reset: false,
                pending_reset_reasons: BoundedVec::default(),
                decoded_start_frame: 0,
                source_a: source.clone(),
                source_b: source,
            }
        }
    }
}

#[cfg(target_os = "windows")]
pub(crate) use platform::D2Runtime;

#[cfg(not(target_os = "windows"))]
pub(crate) struct D2Runtime;

#[cfg(not(target_os = "windows"))]
impl D2Runtime {
    pub(crate) async fn start(
        _app: AppHandle,
        _shared_status: Arc<Mutex<D2StatusView>>,
        _shared_capture_status: Arc<Mutex<D2CaptureView>>,
        _config: D2LaunchConfig,
    ) -> Result<Self, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn controls_set(
        &self,
        _controls: D2Controls,
    ) -> Result<D2ControlsAckView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn transport_set(
        &self,
        _transport: D2Transport,
    ) -> Result<D2TransportAckView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn seed_set(&self, _seed: u64) -> Result<D2SeedAckView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn restart(&self) -> Result<D2StatusView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn capture_start(
        &self,
        _mode: D2CaptureMode,
        _output: PathBuf,
    ) -> Result<D2CaptureView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn capture_stop(&self) -> Result<D2CaptureView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn capture_status(&self) -> Result<D2CaptureView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn status(&self) -> Result<D2StatusView, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn resize(&self, _width: u32, _height: u32) -> Result<(), D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn shutdown(&self) -> Result<(), D2RuntimeError> {
        Ok(())
    }

    pub(crate) async fn toggle_fullscreen(&self) -> Result<bool, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn spout_status(&self) -> Result<NativeSpoutStatus, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }

    pub(crate) async fn configure_spout(
        &self,
        _name: Option<String>,
        _enabled: Option<bool>,
    ) -> Result<NativeSpoutStatus, D2RuntimeError> {
        Err(D2RuntimeError::unsupported())
    }
}

#[cfg(test)]
mod common_tests {
    use latentdeck_cartridge::{
        manifest::{DType, Rational},
        profile::h3::H3CompatibilityKey,
    };
    use latentdeck_control::WireUuid;

    use super::*;

    #[test]
    fn ui_controls_reject_non_finite_and_out_of_range_values() {
        let mut input = controls_input();
        input.mix = f64::NAN;
        assert_eq!(
            input.into_wire().expect_err("NaN must fail").code,
            "deck.controls_invalid"
        );

        let mut input = controls_input();
        input.top_k = 0;
        assert_eq!(
            input.into_wire().expect_err("zero top-k must fail").code,
            "deck.controls_invalid"
        );
    }

    #[test]
    fn status_view_stringifies_lossless_u64_counters() {
        let source = latentdeck_control::D2SourceStatus {
            cartridge_id: WireUuid::new_v4(),
            archive_sha256: "a".repeat(64),
            latent_slot_count: 7,
        };
        let status = D2Status {
            deck_id: D2_DECK_ID.to_owned(),
            deck_revision: 1,
            operator_id: D2_OPERATOR_ID.to_owned(),
            operator_version: D2_OPERATOR_VERSION.to_owned(),
            stream_generation: u64::MAX,
            stream_sequence: u64::MAX - 1,
            playhead_a: 2,
            playhead_b: 3,
            transport: D2Transport::default(),
            controls: D2Controls::default(),
            seed: 42,
            pending_reset: false,
            pending_reset_reasons: latentdeck_control::BoundedVec::default(),
            decoded_start_frame: 0,
            source_a: source.clone(),
            source_b: source,
        };
        let view = D2StatusView::from_status(&status);
        assert_eq!(view.stream_generation, u64::MAX.to_string());
        assert_eq!(view.stream_sequence, (u64::MAX - 1).to_string());
    }

    #[test]
    fn causal_reset_plan_rejects_stale_barriers_and_false_clear_acks() {
        assert!(
            CausalResetPlan::from_barrier(2, 1, 3, &[D2ResetReason::TransportRestart]).is_err()
        );
        let plan = CausalResetPlan::from_barrier(2, 2, 3, &[D2ResetReason::TransportRestart])
            .expect("fresh barrier");
        assert!(
            plan.validate_ack(3, &[D2ResetReason::TransportRestart], false)
                .is_err()
        );
        plan.validate_ack(3, &[D2ResetReason::TransportRestart], true)
            .expect("exact cleared ack");
    }

    #[test]
    fn full_h3_compatibility_key_includes_grid_geometry_and_timing() {
        let source_a = synthetic_profile(28, 50);
        let mut source_b = synthetic_profile(28, 50);
        require_compatible_sources(&source_a, &source_b).expect("same contract");
        source_b.compatibility_key.latent_width = 51;
        assert!(require_compatible_sources(&source_a, &source_b).is_err());
    }

    fn controls_input() -> D2ControlsInput {
        D2ControlsInput {
            algorithm: D2Algorithm::Linear,
            mix: 0.5,
            mode: D2Mode::Hybridize,
            routing: D2Routing::A,
            interaction: 0.0,
            preserve: 0.55,
            chaos: 0.0,
            xs1_channel_a: 0,
            xs1_channel_b: 1,
            xs1_angle_degrees: 30.0,
            xs2_radius: 1,
            xs3_high_gain: 0.5,
            xs4_epsilon: 0.000_001,
            xs5_routing: D2Xs5Routing::TopK,
            temperature: 0.12,
            top_k: 8,
            sinkhorn_iterations: 5,
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
