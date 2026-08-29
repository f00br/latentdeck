//! Trusted application-side boundary for LD-Q4 resample capture artifacts.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use latentdeck_cartridge::{
    manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, DType, Identifier, ParentCartridge,
        Sha256Digest, SourceCartridgeRef, TensorStream,
    },
    reader::{ValidationOptions, open_validated},
    resample::{
        CaptureMode, PayloadExpectation, ResampleManifestRequest, build_resample_manifest,
        pack_resample_atomic,
    },
    writer::WriteOptions,
};
use latentdeck_control::{
    Q4CaptureAudioDescriptor, Q4CaptureAudioDtype, Q4CaptureAudioPolicy,
    Q4CaptureAudioPolicyReason, Q4CaptureMode, Q4CaptureParent, Q4CaptureReceipt, Q4CaptureState,
    Q4CaptureStatus, Q4CaptureVisualDtype, Q4Roles, Q4Slot, WireUuid,
};
use serde::Serialize;
use serde_json::Value;

use crate::library_state::LibraryImporter;

pub(crate) const APP_Q4_CAPTURE_MAX_LATENT_SLOTS: u64 = 16_382;
pub(crate) const APP_Q4_CAPTURE_MAX_VISUAL_BYTES: u64 = 1024 * 1024 * 1024;
const CAPTURE_DIRECTORY: &str = "q4-capture-spool";
const Q4_OPERATOR_ID: &str = "org.latentdeck.builtin.ld_q4";
const Q4_OPERATOR_VERSION: &str = "0.1.0";
const H3_CODEC_FAMILY: &str = "minimax_h3";
const H3_PROFILE: &str = "h3_av_latent";
const H3_PROFILE_VERSION: &str = "0.1.0";
const H3_TIMING_CONTRACT: &str = "minimax_h3_causal";
const H3_TIMING_CONTRACT_VERSION: &str = "0.1.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Q4CaptureView {
    pub(crate) capture_id: Option<String>,
    pub(crate) mode: Option<Q4CaptureMode>,
    pub(crate) state: String,
    pub(crate) latent_slots: String,
    pub(crate) target_latent_slots: Option<String>,
    pub(crate) cartridge_id: Option<String>,
    pub(crate) archive_sha256: Option<String>,
    pub(crate) detail: Option<String>,
}

impl Default for Q4CaptureView {
    fn default() -> Self {
        Self {
            capture_id: None,
            mode: None,
            state: "idle".to_owned(),
            latent_slots: "0".to_owned(),
            target_latent_slots: None,
            cartridge_id: None,
            archive_sha256: None,
            detail: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HostCapturePhase {
    AwaitingReset,
    Capturing,
    StopArmed,
    Finalizing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveCaptureIdentity {
    capture_id: WireUuid,
    mode: Q4CaptureMode,
    phase: HostCapturePhase,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Q4CaptureCoordinator {
    active: Option<ActiveCaptureIdentity>,
    view: Q4CaptureView,
}

impl Q4CaptureCoordinator {
    pub(crate) fn begin(
        &mut self,
        capture_id: WireUuid,
        mode: Q4CaptureMode,
    ) -> Result<(), Q4CaptureHostError> {
        if self.active.is_some() || capture_id.is_nil() {
            return Err(Q4CaptureHostError::new(
                "capture.already_active",
                "Only one valid LD-Q4 capture may be active.",
            ));
        }
        self.active = Some(ActiveCaptureIdentity {
            capture_id,
            mode,
            phase: HostCapturePhase::AwaitingReset,
        });
        self.view = Q4CaptureView {
            capture_id: Some(capture_id.to_string()),
            mode: Some(mode),
            state: "awaiting_reset".to_owned(),
            ..Q4CaptureView::default()
        };
        Ok(())
    }

    pub(crate) fn observe(
        &mut self,
        status: &Q4CaptureStatus,
    ) -> Result<Q4CaptureView, Q4CaptureHostError> {
        let active = self.active.as_mut().ok_or_else(|| {
            Q4CaptureHostError::new(
                "capture.not_active",
                "No host capture is available for this worker status.",
            )
        })?;
        if active.capture_id != status.capture_id || active.mode != status.mode {
            return Err(Q4CaptureHostError::new(
                "capture.id_mismatch",
                "The worker capture status does not match the active host capture.",
            ));
        }
        active.phase = next_phase(active.phase, status.state, active.mode)?;
        self.view = Q4CaptureView {
            capture_id: Some(active.capture_id.to_string()),
            mode: Some(active.mode),
            state: match active.phase {
                HostCapturePhase::AwaitingReset => "awaiting_reset",
                HostCapturePhase::Capturing => "capturing",
                HostCapturePhase::StopArmed => "stop_armed",
                HostCapturePhase::Finalizing => "finalizing",
            }
            .to_owned(),
            latent_slots: status.latent_slots.to_string(),
            target_latent_slots: status.target_latent_slots.map(|value| value.to_string()),
            cartridge_id: None,
            archive_sha256: None,
            detail: None,
        };
        if status.state == Q4CaptureState::Aborted {
            self.active = None;
            "aborted".clone_into(&mut self.view.state);
            self.view.detail = Some("The worker aborted capture safely.".to_owned());
        }
        Ok(self.view.clone())
    }

    pub(crate) fn complete(
        &mut self,
        finalized: &Q4FinalizedCapture,
    ) -> Result<Q4CaptureView, Q4CaptureHostError> {
        let active = self.active.as_ref().ok_or_else(|| {
            Q4CaptureHostError::new(
                "capture.not_active",
                "No finalized host capture is available.",
            )
        })?;
        if active.phase != HostCapturePhase::Finalizing {
            return Err(Q4CaptureHostError::new(
                "capture.state_invalid",
                "Capture cannot complete before its finished receipt is bound.",
            ));
        }
        self.active = None;
        "finished".clone_into(&mut self.view.state);
        self.view.cartridge_id = Some(finalized.cartridge_id.clone());
        self.view.archive_sha256 = Some(finalized.archive_sha256.clone());
        self.view.detail =
            Some("Validated cartridge saved and imported into the Library.".to_owned());
        Ok(self.view.clone())
    }

    pub(crate) fn fail(&mut self) -> Q4CaptureView {
        self.active = None;
        "error".clone_into(&mut self.view.state);
        self.view.detail = Some("Capture finalization failed safely.".to_owned());
        self.view.clone()
    }

    pub(crate) fn view(&self) -> Q4CaptureView {
        self.view.clone()
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active.is_some()
    }
}

fn next_phase(
    current: HostCapturePhase,
    worker: Q4CaptureState,
    mode: Q4CaptureMode,
) -> Result<HostCapturePhase, Q4CaptureHostError> {
    let next = match (current, worker, mode) {
        (HostCapturePhase::AwaitingReset, Q4CaptureState::AwaitingReset, _)
        | (_, Q4CaptureState::Aborted, _) => current,
        (
            HostCapturePhase::AwaitingReset | HostCapturePhase::Capturing,
            Q4CaptureState::Capturing,
            _,
        ) => HostCapturePhase::Capturing,
        (
            HostCapturePhase::Capturing | HostCapturePhase::StopArmed,
            Q4CaptureState::StopArmed,
            Q4CaptureMode::LiveCapture,
        ) => HostCapturePhase::StopArmed,
        (HostCapturePhase::Capturing, Q4CaptureState::Finished, _)
        | (HostCapturePhase::StopArmed, Q4CaptureState::Finished, Q4CaptureMode::LiveCapture) => {
            HostCapturePhase::Finalizing
        }
        _ => {
            return Err(Q4CaptureHostError::new(
                "capture.state_invalid",
                "The worker capture state transition is invalid.",
            ));
        }
    };
    Ok(next)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Q4CaptureHostError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl Q4CaptureHostError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Q4CaptureSpoolBinding {
    capture_id: WireUuid,
    root: PathBuf,
    payload: PathBuf,
}

impl Drop for Q4CaptureSpoolBinding {
    fn drop(&mut self) {
        self.cleanup();
    }
}

impl Q4CaptureSpoolBinding {
    pub(crate) fn create(
        app_local_data: &Path,
        capture_id: WireUuid,
    ) -> Result<Self, Q4CaptureHostError> {
        if !app_local_data.is_absolute() || capture_id.is_nil() {
            return Err(Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "Capture storage must use an absolute app-local root and canonical identity.",
            ));
        }
        reject_reparse(app_local_data)?;
        let app_local_data = fs::canonicalize(app_local_data).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "The app-local capture storage root is unavailable.",
            )
        })?;
        let container = app_local_data.join(CAPTURE_DIRECTORY);
        fs::create_dir_all(&container).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "The app-local capture storage root could not be created.",
            )
        })?;
        reject_reparse(&container)?;
        let container = fs::canonicalize(&container).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "The app-local capture storage root is unavailable.",
            )
        })?;
        if !container.starts_with(&app_local_data) {
            return Err(Q4CaptureHostError::new(
                "capture.spool_root_escape",
                "Capture storage escaped the application data directory.",
            ));
        }

        let root = container.join(capture_id.to_string());
        fs::create_dir(&root).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "A fresh capture storage directory could not be created.",
            )
        })?;
        reject_reparse(&root)?;
        let root = fs::canonicalize(&root).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "The fresh capture storage directory is unavailable.",
            )
        })?;
        if root.parent() != Some(container.as_path()) {
            return Err(Q4CaptureHostError::new(
                "capture.spool_root_escape",
                "Capture storage escaped its bounded application directory.",
            ));
        }
        let payload = root.join(format!("{capture_id}.safetensors.partial"));
        Ok(Self {
            capture_id,
            root,
            payload,
        })
    }

    pub(crate) const fn capture_id(&self) -> WireUuid {
        self.capture_id
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn cleanup(&self) {
        if self.payload.parent() == Some(self.root.as_path()) {
            let _ = fs::remove_file(&self.payload);
        }
        let _ = fs::remove_dir(&self.root);
    }

    pub(crate) fn bind_finished_receipt(
        &self,
        receipt: &Q4CaptureReceipt,
    ) -> Result<PathBuf, Q4CaptureHostError> {
        if receipt.capture_id != self.capture_id {
            return Err(Q4CaptureHostError::new(
                "capture.id_mismatch",
                "The worker capture receipt does not match the active host capture.",
            ));
        }
        let reported = PathBuf::from(&receipt.payload_path);
        if reported != self.payload {
            return Err(Q4CaptureHostError::new(
                "capture.spool_path_mismatch",
                "The worker capture receipt did not bind the exact expected spool path.",
            ));
        }
        reject_reparse(&self.root)?;
        reject_reparse(&reported)?;
        let metadata = fs::metadata(&reported).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_missing",
                "The finalized capture spool is unavailable.",
            )
        })?;
        if !metadata.is_file() {
            return Err(Q4CaptureHostError::new(
                "capture.spool_invalid",
                "The finalized capture spool is not a regular file.",
            ));
        }
        if metadata.len() != receipt.payload_bytes {
            return Err(Q4CaptureHostError::new(
                "capture.spool_size_mismatch",
                "The finalized capture spool size does not match its receipt.",
            ));
        }
        let canonical_root = fs::canonicalize(&self.root).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_root_invalid",
                "The retained capture storage root is unavailable.",
            )
        })?;
        let canonical_payload = fs::canonicalize(&reported).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.spool_missing",
                "The finalized capture spool is unavailable.",
            )
        })?;
        if canonical_root != self.root || canonical_payload.parent() != Some(self.root.as_path()) {
            return Err(Q4CaptureHostError::new(
                "capture.spool_path_mismatch",
                "The finalized capture spool escaped its retained root.",
            ));
        }
        Ok(reported)
    }
}

pub(crate) fn validate_q4_output_path(selected: PathBuf) -> Result<PathBuf, Q4CaptureHostError> {
    if !selected.is_absolute() {
        return Err(Q4CaptureHostError::new(
            "capture.output_path_invalid",
            "The native save dialog did not return an absolute output path.",
        ));
    }
    let mut output = selected;
    match output.extension().and_then(|value| value.to_str()) {
        None => {
            output.set_extension("lc");
        }
        Some(extension) if extension.eq_ignore_ascii_case("lc") => {
            output.set_extension("lc");
        }
        Some(_) => {
            return Err(Q4CaptureHostError::new(
                "capture.output_path_invalid",
                "Resampled cartridges must use the .lc extension.",
            ));
        }
    }
    if output.exists() {
        return Err(Q4CaptureHostError::new(
            "target.exists",
            "The selected cartridge output already exists; capture never overwrites files.",
        ));
    }
    let parent = output.parent().ok_or_else(|| {
        Q4CaptureHostError::new(
            "capture.output_path_invalid",
            "The selected cartridge output has no parent directory.",
        )
    })?;
    if !parent.is_dir() {
        return Err(Q4CaptureHostError::new(
            "capture.output_path_invalid",
            "The selected cartridge output directory is unavailable.",
        ));
    }
    Ok(output)
}

fn reject_reparse(path: &Path) -> Result<(), Q4CaptureHostError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        Q4CaptureHostError::new(
            "capture.spool_root_invalid",
            "Capture storage metadata is unavailable.",
        )
    })?;
    if is_reparse(&metadata) {
        return Err(Q4CaptureHostError::new(
            "capture.spool_reparse_forbidden",
            "Capture storage cannot use symbolic links or reparse points.",
        ));
    }
    Ok(())
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Q4StructuralCarrierEvidence {
    slot: Q4Slot,
    cartridge_id: WireUuid,
    archive_sha256: String,
    codec_family: String,
    profile: String,
    profile_version: String,
    timing_contract: String,
    timing_contract_version: String,
    decoded_frame_count: u64,
    frame_rate_numerator: u64,
    frame_rate_denominator: u64,
    audio_descriptor: Option<Q4CaptureAudioDescriptor>,
}

impl Q4StructuralCarrierEvidence {
    pub(crate) fn inspect(
        parent: &Q4CaptureParent,
        registered_path: &Path,
    ) -> Result<Self, Q4CaptureHostError> {
        validate_parent(parent)?;
        let metadata = fs::symlink_metadata(registered_path).map_err(|_| {
            Q4CaptureHostError::new(
                "capture.carrier_unavailable",
                "The structural carrier is unavailable.",
            )
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(Q4CaptureHostError::new(
                "capture.carrier_unavailable",
                "The structural carrier is unavailable.",
            ));
        }
        let validated =
            open_validated(registered_path, &ValidationOptions::default()).map_err(|_| {
                Q4CaptureHostError::new(
                    "capture.carrier_invalid",
                    "The structural carrier failed full LC validation.",
                )
            })?;
        let manifest = validated.manifest();
        if manifest.cartridge_id.0 != parent.cartridge_id.to_string()
            || validated.receipt().archive_sha256.to_string() != parent.archive_sha256
        {
            return Err(Q4CaptureHostError::new(
                "capture.carrier_identity_mismatch",
                "The structural carrier identity changed.",
            ));
        }
        let audio_descriptor = manifest
            .tensors
            .iter()
            .find(|tensor| tensor.stream == TensorStream::Audio)
            .map(audio_descriptor_from_manifest)
            .transpose()?;
        Ok(Self {
            slot: parent.slot,
            cartridge_id: parent.cartridge_id,
            archive_sha256: parent.archive_sha256.clone(),
            codec_family: manifest.codec.family.0.clone(),
            profile: manifest.codec.profile.0.clone(),
            profile_version: manifest.codec.profile_version.0.clone(),
            timing_contract: manifest.timing.contract.0.clone(),
            timing_contract_version: manifest.timing.contract_version.0.clone(),
            decoded_frame_count: manifest.timing.decoded_video.frame_count,
            frame_rate_numerator: manifest.timing.decoded_video.frame_rate.numerator,
            frame_rate_denominator: manifest.timing.decoded_video.frame_rate.denominator,
            audio_descriptor,
        })
    }
}

fn audio_descriptor_from_manifest(
    tensor: &latentdeck_cartridge::manifest::TensorDescriptor,
) -> Result<Q4CaptureAudioDescriptor, Q4CaptureHostError> {
    let shape: [u64; 4] = tensor.shape.clone().try_into().map_err(|_| {
        Q4CaptureHostError::new(
            "capture.carrier_invalid",
            "The structural carrier audio descriptor is invalid.",
        )
    })?;
    let (storage_dtype, element_bytes) = match tensor.storage_dtype {
        DType::F16 => (Q4CaptureAudioDtype::F16, 2_u64),
        DType::F32 => (Q4CaptureAudioDtype::F32, 4_u64),
        _ => {
            return Err(Q4CaptureHostError::new(
                "capture.carrier_invalid",
                "The structural carrier audio dtype is invalid.",
            ));
        }
    };
    let byte_length = shape.iter().try_fold(element_bytes, |total, axis| {
        total.checked_mul(*axis).ok_or_else(|| {
            Q4CaptureHostError::new(
                "capture.carrier_invalid",
                "The structural carrier audio size overflows.",
            )
        })
    })?;
    Ok(Q4CaptureAudioDescriptor {
        storage_dtype,
        shape,
        byte_length,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Q4FinalizedCapture {
    pub(crate) cartridge_id: String,
    pub(crate) archive_sha256: String,
}

pub(crate) async fn finalize_q4_capture(
    binding: Q4CaptureSpoolBinding,
    status: &Q4CaptureStatus,
    output: PathBuf,
    structural_carrier: &Q4StructuralCarrierEvidence,
    library_importer: LibraryImporter,
) -> Result<Q4FinalizedCapture, Q4CaptureHostError> {
    finalize_q4_capture_with_id(
        binding,
        status,
        output,
        structural_carrier,
        library_importer,
        WireUuid::new_v4(),
    )
    .await
}

async fn finalize_q4_capture_with_id(
    binding: Q4CaptureSpoolBinding,
    status: &Q4CaptureStatus,
    output: PathBuf,
    structural_carrier: &Q4StructuralCarrierEvidence,
    library_importer: LibraryImporter,
    cartridge_id: WireUuid,
) -> Result<Q4FinalizedCapture, Q4CaptureHostError> {
    validate_finished_status(status, binding.capture_id())?;
    let receipt = status
        .receipt
        .as_deref()
        .ok_or_else(invalid_finished_status)?
        .clone();
    let output = validate_q4_output_path(output)?;
    let payload = binding.bind_finished_receipt(&receipt)?;
    let request = resample_request_from_receipt(&receipt, cartridge_id, structural_carrier)?;
    validate_payload_against_receipt(&receipt, &request, &payload)?;

    let packed = tauri::async_runtime::spawn_blocking(move || {
        pack_resample_atomic(&request, payload, output, &WriteOptions::default())
    })
    .await
    .map_err(|_| capture_finalize_error())?
    .map_err(|_| capture_finalize_error())?;
    binding.cleanup();

    let archive_sha256 = packed.validation.archive_sha256.to_string();
    let imported = library_importer
        .import_generated(packed.output_path)
        .await
        .map_err(|_| capture_import_error())?;
    if imported.as_str() != archive_sha256 {
        return Err(capture_import_error());
    }
    Ok(Q4FinalizedCapture {
        cartridge_id: cartridge_id.to_string(),
        archive_sha256,
    })
}

fn validate_finished_status(
    status: &Q4CaptureStatus,
    capture_id: WireUuid,
) -> Result<(), Q4CaptureHostError> {
    if status.state != Q4CaptureState::Finished || status.capture_id != capture_id {
        return Err(invalid_finished_status());
    }
    let receipt = status
        .receipt
        .as_deref()
        .ok_or_else(invalid_finished_status)?;
    if receipt.capture_id != status.capture_id
        || receipt.mode != status.mode
        || receipt.structural_carrier != status.structural_carrier
        || receipt.visual_shape[2] != status.latent_slots
        || status.current_generation.is_some()
        || status.minimum_new_generation.is_some()
        || status.target_latent_slots.is_some()
        || status.finalize_after_latent_slots.is_some()
        || status.reason.is_some()
        || status
            .stream_generation
            .is_none_or(|generation| generation == 0)
    {
        return Err(invalid_finished_status());
    }
    Ok(())
}

const fn invalid_finished_status() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.status_invalid",
        "The finished Q4 capture status is internally inconsistent.",
    )
}

const fn capture_finalize_error() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.finalize_failed",
        "The Q4 cartridge could not be packed and fully validated.",
    )
}

const fn capture_import_error() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.import_failed",
        "The validated Q4 cartridge could not be imported into the Library.",
    )
}

pub(crate) fn resample_request_from_receipt(
    receipt: &Q4CaptureReceipt,
    cartridge_id: WireUuid,
    structural_carrier: &Q4StructuralCarrierEvidence,
) -> Result<ResampleManifestRequest, Q4CaptureHostError> {
    if cartridge_id.is_nil() {
        return Err(Q4CaptureHostError::new(
            "capture.cartridge_id_invalid",
            "The host-generated output cartridge identity is invalid.",
        ));
    }
    validate_receipt_contract(receipt)?;
    let roles = *initial_roles(receipt)?;
    let (controls, seed) = controls_and_seed(receipt, roles)?;
    Ok(ResampleManifestRequest {
        cartridge_id: CartridgeId(cartridge_id.to_string()),
        expected_payload: PayloadExpectation {
            byte_length: receipt.payload_bytes,
            sha256: Sha256Digest(receipt.payload_sha256.clone()),
        },
        capture_mode: capture_mode(receipt.mode),
        audio: audio_disposition(receipt, roles, structural_carrier)?,
        parent_cartridges: parent_cartridges(receipt, roles),
        operator_id: Identifier(Q4_OPERATOR_ID.to_owned()),
        operator_version: Q4_OPERATOR_VERSION.to_owned(),
        seed,
        controls,
    })
}

const fn capture_mode(mode: Q4CaptureMode) -> CaptureMode {
    match mode {
        Q4CaptureMode::Snapshot => CaptureMode::Snapshot,
        Q4CaptureMode::LiveCapture => CaptureMode::LiveCapture,
    }
}

fn validate_receipt_contract(receipt: &Q4CaptureReceipt) -> Result<(), Q4CaptureHostError> {
    if receipt.capture_id.is_nil()
        || !canonical_sha256(&receipt.payload_sha256)
        || receipt.payload_bytes == 0
        || receipt.payload_bytes > APP_Q4_CAPTURE_MAX_VISUAL_BYTES
        || receipt.storage_dtype != Q4CaptureVisualDtype::F16
    {
        return Err(invalid_receipt());
    }
    let [batch, channels, temporal, height, width] = receipt.visual_shape;
    if batch != 1
        || channels != 24
        || !codec_valid_slots(temporal)
        || height == 0
        || width == 0
        || height.checked_mul(width).is_none_or(|tokens| tokens > 4096)
        || decoded_frames(temporal) != Some(receipt.decoded_frame_count)
    {
        return Err(invalid_receipt());
    }
    for (expected, parent) in [Q4Slot::A, Q4Slot::B, Q4Slot::C, Q4Slot::D]
        .into_iter()
        .zip(&receipt.parents)
    {
        if parent.slot != expected {
            return Err(invalid_receipt());
        }
        validate_parent(parent)?;
    }
    let roles = initial_roles(receipt)?;
    if roles.carrier != receipt.structural_carrier {
        return Err(invalid_receipt());
    }
    Ok(())
}

fn validate_parent(parent: &Q4CaptureParent) -> Result<(), Q4CaptureHostError> {
    if parent.cartridge_id.is_nil() || !canonical_sha256(&parent.archive_sha256) {
        return Err(invalid_receipt());
    }
    Ok(())
}

const fn invalid_receipt() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.receipt_invalid",
        "The worker capture receipt violates the closed Q4 contract.",
    )
}

fn canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn codec_valid_slots(value: u64) -> bool {
    (2..=APP_Q4_CAPTURE_MAX_LATENT_SLOTS).contains(&value) && (value - 2).is_multiple_of(5)
}

fn decoded_frames(latent_slots: u64) -> Option<u64> {
    latent_slots
        .checked_sub(2)
        .and_then(|value| value.checked_div(5))
        .and_then(|blocks| blocks.checked_mul(17))
        .and_then(|frames| frames.checked_add(5))
}

fn initial_roles(receipt: &Q4CaptureReceipt) -> Result<&Q4Roles, Q4CaptureHostError> {
    let roles = match receipt.mode {
        Q4CaptureMode::Snapshot => {
            if receipt.control_events.is_some()
                || receipt.frozen_seed.is_none()
                || receipt.frozen_controls.is_none()
            {
                return Err(invalid_receipt());
            }
            receipt.frozen_roles.as_ref().ok_or_else(invalid_receipt)?
        }
        Q4CaptureMode::LiveCapture => {
            if receipt.frozen_seed.is_some()
                || receipt.frozen_roles.is_some()
                || receipt.frozen_controls.is_some()
            {
                return Err(invalid_receipt());
            }
            let events = receipt
                .control_events
                .as_ref()
                .ok_or_else(invalid_receipt)?;
            let first = events.first().ok_or_else(invalid_receipt)?;
            if first.slot_offset != 0 {
                return Err(invalid_receipt());
            }
            &first.roles
        }
    };
    roles.validate().map_err(|_| invalid_receipt())?;
    Ok(roles)
}

fn controls_and_seed(
    receipt: &Q4CaptureReceipt,
    roles: Q4Roles,
) -> Result<(BTreeMap<String, Value>, u64), Q4CaptureHostError> {
    match receipt.mode {
        Q4CaptureMode::Snapshot => {
            let controls = receipt
                .frozen_controls
                .as_ref()
                .ok_or_else(invalid_receipt)?;
            controls.validate().map_err(|_| invalid_receipt())?;
            let seed = receipt.frozen_seed.ok_or_else(invalid_receipt)?;
            validate_seed(seed)?;
            let Value::Object(mut object) =
                serde_json::to_value(controls).map_err(|_| invalid_provenance())?
            else {
                return Err(invalid_provenance());
            };
            object.insert(
                "roles".to_owned(),
                serde_json::to_value(roles).map_err(|_| invalid_provenance())?,
            );
            Ok((object.into_iter().collect(), seed))
        }
        Q4CaptureMode::LiveCapture => {
            let events = receipt
                .control_events
                .as_ref()
                .ok_or_else(invalid_receipt)?;
            let first = events.first().ok_or_else(invalid_receipt)?;
            let mut previous = 0;
            for event in events.iter() {
                if event.slot_offset < previous || event.slot_offset >= receipt.visual_shape[2] {
                    return Err(invalid_receipt());
                }
                event.roles.validate().map_err(|_| invalid_receipt())?;
                event.controls.validate().map_err(|_| invalid_receipt())?;
                validate_seed(event.seed)?;
                previous = event.slot_offset;
            }
            let controls = BTreeMap::from([
                (
                    "control_events".to_owned(),
                    serde_json::to_value(events).map_err(|_| invalid_provenance())?,
                ),
                (
                    "initial_roles".to_owned(),
                    serde_json::to_value(roles).map_err(|_| invalid_provenance())?,
                ),
                (
                    "structural_carrier".to_owned(),
                    serde_json::to_value(roles.carrier).map_err(|_| invalid_provenance())?,
                ),
            ]);
            Ok((controls, first.seed))
        }
    }
}

fn validate_seed(seed: u64) -> Result<(), Q4CaptureHostError> {
    if seed > 9_007_199_254_740_991 {
        return Err(invalid_receipt());
    }
    Ok(())
}

const fn invalid_provenance() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.provenance_invalid",
        "The Q4 capture provenance is not bounded JSON data.",
    )
}

fn parent_cartridges(receipt: &Q4CaptureReceipt, roles: Q4Roles) -> Vec<ParentCartridge> {
    receipt
        .parents
        .iter()
        .map(|parent| ParentCartridge {
            cartridge_id: CartridgeId(parent.cartridge_id.to_string()),
            archive_sha256: Sha256Digest(parent.archive_sha256.clone()),
            role: Identifier(parent_role(parent.slot, roles)),
        })
        .collect()
}

fn parent_role(slot: Q4Slot, roles: Q4Roles) -> String {
    let physical = slot_label(slot);
    if slot == roles.carrier {
        format!("carrier_{physical}")
    } else if slot == roles.donor_b {
        format!("donor_b_{physical}")
    } else if slot == roles.donor_c {
        format!("donor_c_{physical}")
    } else {
        format!("donor_d_{physical}")
    }
}

const fn slot_label(slot: Q4Slot) -> char {
    match slot {
        Q4Slot::A => 'a',
        Q4Slot::B => 'b',
        Q4Slot::C => 'c',
        Q4Slot::D => 'd',
    }
}

fn audio_disposition(
    receipt: &Q4CaptureReceipt,
    roles: Q4Roles,
    evidence: &Q4StructuralCarrierEvidence,
) -> Result<AudioDisposition, Q4CaptureHostError> {
    let parent = receipt
        .parents
        .iter()
        .find(|parent| parent.slot == roles.carrier)
        .ok_or_else(invalid_receipt)?;
    if evidence.slot != roles.carrier
        || evidence.cartridge_id != parent.cartridge_id
        || evidence.archive_sha256 != parent.archive_sha256
    {
        return Err(Q4CaptureHostError::new(
            "capture.carrier_identity_mismatch",
            "The host structural-carrier evidence does not match the frozen Q4 roles.",
        ));
    }
    let source_cartridge = SourceCartridgeRef {
        cartridge_id: CartridgeId(evidence.cartridge_id.to_string()),
        archive_sha256: Sha256Digest(evidence.archive_sha256.clone()),
    };
    let Some(source_audio) = evidence.audio_descriptor.as_ref() else {
        if receipt.audio_policy != Q4CaptureAudioPolicy::SourceAbsent
            || receipt.audio_policy_reason.is_some()
            || receipt.audio_descriptor.is_some()
        {
            return Err(invalid_audio_policy());
        }
        return Ok(AudioDisposition::SourceAbsent);
    };

    let duration_exact = duration_matches(receipt, evidence);
    let mapping_exact = mapping_matches(evidence);
    if duration_exact && mapping_exact {
        if receipt.audio_policy != Q4CaptureAudioPolicy::CopiedFromCarrierExact
            || receipt.audio_policy_reason.is_some()
            || receipt.audio_descriptor.as_ref() != Some(source_audio)
        {
            return Err(invalid_audio_policy());
        }
        return Ok(AudioDisposition::CopiedFromCarrierExact { source_cartridge });
    }

    if receipt.mode == Q4CaptureMode::Snapshot
        || receipt.audio_policy != Q4CaptureAudioPolicy::OmittedTimingMismatch
        || receipt.audio_descriptor.is_some()
    {
        return Err(invalid_audio_policy());
    }
    let (expected_reason, manifest_reason) = match (duration_exact, mapping_exact) {
        (false, false) => (
            Q4CaptureAudioPolicyReason::DurationAndMappingMismatch,
            AudioOmissionReason::DurationAndMappingMismatch,
        ),
        (false, true) => (
            Q4CaptureAudioPolicyReason::DurationMismatch,
            AudioOmissionReason::DurationMismatch,
        ),
        (true, false) => (
            Q4CaptureAudioPolicyReason::TemporalMappingMismatch,
            AudioOmissionReason::TemporalMappingMismatch,
        ),
        (true, true) => unreachable!("exact audio returned above"),
    };
    if receipt.audio_policy_reason != Some(expected_reason) {
        return Err(invalid_audio_policy());
    }
    Ok(AudioDisposition::OmittedTimingMismatch {
        source_cartridge,
        reason: manifest_reason,
    })
}

fn duration_matches(receipt: &Q4CaptureReceipt, evidence: &Q4StructuralCarrierEvidence) -> bool {
    u128::from(receipt.decoded_frame_count) * u128::from(evidence.frame_rate_numerator)
        == u128::from(evidence.decoded_frame_count)
            * u128::from(evidence.frame_rate_denominator)
            * 24
}

fn mapping_matches(evidence: &Q4StructuralCarrierEvidence) -> bool {
    evidence.codec_family == H3_CODEC_FAMILY
        && evidence.profile == H3_PROFILE
        && evidence.profile_version == H3_PROFILE_VERSION
        && evidence.timing_contract == H3_TIMING_CONTRACT
        && evidence.timing_contract_version == H3_TIMING_CONTRACT_VERSION
}

const fn invalid_audio_policy() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.audio_policy_invalid",
        "Audio may be copied only from an exact host-validated structural carrier mapping.",
    )
}

fn validate_payload_against_receipt(
    receipt: &Q4CaptureReceipt,
    request: &ResampleManifestRequest,
    payload: &Path,
) -> Result<(), Q4CaptureHostError> {
    let manifest = build_resample_manifest(request, payload).map_err(|_| {
        Q4CaptureHostError::new(
            "capture.payload_invalid",
            "The finalized post-operator payload failed full host validation.",
        )
    })?;
    let visual = manifest
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Visual)
        .ok_or_else(invalid_payload_contract)?;
    if visual.storage_dtype != DType::F16
        || visual.runtime_dtype != DType::F16
        || visual.shape.as_slice() != receipt.visual_shape
        || manifest.timing.decoded_video.frame_count != receipt.decoded_frame_count
    {
        return Err(invalid_payload_contract());
    }
    let actual_audio = manifest
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Audio)
        .map(audio_descriptor_from_manifest)
        .transpose()?;
    if actual_audio != receipt.audio_descriptor {
        return Err(invalid_payload_contract());
    }
    Ok(())
}

const fn invalid_payload_contract() -> Q4CaptureHostError {
    Q4CaptureHostError::new(
        "capture.payload_mismatch",
        "The finalized payload differs from its bounded Q4 receipt.",
    )
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use latentdeck_cartridge::{
        hash::hash_reader,
        reader::{ValidationOptions, open_validated},
    };
    use latentdeck_control::{BoundedVec, Q4CaptureControlEvent, Q4Controls};
    use latentdeck_library::{CartridgeKey, Library};
    use tempfile::tempdir;

    use super::*;
    use crate::library_state::AppState;

    #[test]
    fn exact_receipt_path_is_bound_and_owned_cleanup_is_narrow() {
        let directory = tempdir().expect("temporary app data");
        let capture_id = WireUuid::new_v4();
        let binding = Q4CaptureSpoolBinding::create(directory.path(), capture_id).expect("binding");
        fs::write(&binding.payload, b"bounded-spool").expect("synthetic spool marker");
        let mut receipt = snapshot_receipt(
            capture_id,
            binding.payload.to_string_lossy().into_owned(),
            "c".repeat(64),
            13,
        );
        receipt.payload_bytes = 13;

        assert_eq!(
            binding
                .bind_finished_receipt(&receipt)
                .expect("exact receipt"),
            binding.payload
        );
        let root = binding.root().to_path_buf();
        drop(binding);
        assert!(!root.exists());
    }

    #[test]
    fn receipt_cannot_escape_to_a_sibling_or_rebind_another_capture() {
        let directory = tempdir().expect("temporary app data");
        let capture_id = WireUuid::new_v4();
        let binding = Q4CaptureSpoolBinding::create(directory.path(), capture_id).expect("binding");
        let sibling = binding
            .root()
            .parent()
            .expect("capture container")
            .join(format!("{}.safetensors.partial", WireUuid::new_v4()));
        fs::write(&sibling, b"wrong-spool").expect("sibling marker");
        let receipt = snapshot_receipt(
            capture_id,
            sibling.to_string_lossy().into_owned(),
            "c".repeat(64),
            11,
        );
        assert_eq!(
            binding
                .bind_finished_receipt(&receipt)
                .expect_err("sibling path must fail")
                .code,
            "capture.spool_path_mismatch"
        );

        let other = snapshot_receipt(
            WireUuid::new_v4(),
            binding.payload.to_string_lossy().into_owned(),
            "c".repeat(64),
            11,
        );
        assert_eq!(
            binding
                .bind_finished_receipt(&other)
                .expect_err("capture identity must fail")
                .code,
            "capture.id_mismatch"
        );
    }

    #[test]
    fn snapshot_freezes_roles_controls_seed_and_four_parent_genealogy() {
        let capture_id = WireUuid::new_v4();
        let roles = Q4Roles {
            carrier: Q4Slot::C,
            donor_b: Q4Slot::A,
            donor_c: Q4Slot::D,
            donor_d: Q4Slot::B,
        };
        let mut receipt = snapshot_receipt(
            capture_id,
            "ignored-by-conversion.safetensors.partial".to_owned(),
            "c".repeat(64),
            1_000_000,
        );
        receipt.frozen_roles = Some(roles);
        receipt.structural_carrier = roles.carrier;
        let evidence = carrier_evidence(&receipt.parents[2], 22, None);
        let output_id = WireUuid::new_v4();

        let request = resample_request_from_receipt(&receipt, output_id, &evidence)
            .expect("snapshot conversion");

        assert_eq!(request.capture_mode, CaptureMode::Snapshot);
        assert_eq!(request.seed, 77);
        assert_eq!(request.controls["algorithm"], serde_json::json!("LINEAR"));
        assert_eq!(request.controls["roles"]["carrier"], serde_json::json!("C"));
        assert_eq!(request.parent_cartridges.len(), 4);
        assert_eq!(request.parent_cartridges[0].role.0, "donor_b_a");
        assert_eq!(request.parent_cartridges[1].role.0, "donor_d_b");
        assert_eq!(request.parent_cartridges[2].role.0, "carrier_c");
        assert_eq!(request.parent_cartridges[3].role.0, "donor_c_d");
    }

    #[test]
    fn audio_copy_requires_exact_host_carrier_duration_mapping_and_descriptor() {
        let capture_id = WireUuid::new_v4();
        let roles = Q4Roles {
            carrier: Q4Slot::C,
            donor_b: Q4Slot::A,
            donor_c: Q4Slot::D,
            donor_d: Q4Slot::B,
        };
        let audio = Q4CaptureAudioDescriptor {
            storage_dtype: Q4CaptureAudioDtype::F16,
            shape: [1, 32, 2, 37],
            byte_length: 4_736,
        };
        let mut receipt = snapshot_receipt(
            capture_id,
            "ignored-by-conversion.safetensors.partial".to_owned(),
            "c".repeat(64),
            1_000_000,
        );
        receipt.frozen_roles = Some(roles);
        receipt.structural_carrier = Q4Slot::C;
        receipt.audio_policy = Q4CaptureAudioPolicy::CopiedFromCarrierExact;
        receipt.audio_descriptor = Some(audio.clone());
        let evidence = carrier_evidence(&receipt.parents[2], 22, Some(audio));

        let request = resample_request_from_receipt(&receipt, WireUuid::new_v4(), &evidence)
            .expect("exact copied audio");
        let AudioDisposition::CopiedFromCarrierExact { source_cartridge } = request.audio else {
            panic!("exact carrier audio policy");
        };
        assert_eq!(
            source_cartridge.cartridge_id.0,
            receipt.parents[2].cartridge_id.to_string()
        );

        let mut mismatched = evidence;
        mismatched.decoded_frame_count = 107;
        let error = resample_request_from_receipt(&receipt, WireUuid::new_v4(), &mismatched)
            .expect_err("snapshot cannot silently copy mismatched audio");
        assert_eq!(error.code, "capture.audio_policy_invalid");
    }

    #[test]
    fn live_capture_preserves_ordered_bounded_role_control_seed_events_and_omission() {
        let capture_id = WireUuid::new_v4();
        let initial_roles = Q4Roles {
            carrier: Q4Slot::C,
            donor_b: Q4Slot::A,
            donor_c: Q4Slot::D,
            donor_d: Q4Slot::B,
        };
        let later_roles = Q4Roles::default();
        let events = BoundedVec::try_from_vec(vec![
            Q4CaptureControlEvent {
                slot_offset: 0,
                roles: initial_roles,
                controls: Q4Controls::default(),
                seed: 19,
            },
            Q4CaptureControlEvent {
                slot_offset: 5,
                roles: later_roles,
                controls: Q4Controls::default(),
                seed: 23,
            },
        ])
        .expect("bounded events");
        let audio = Q4CaptureAudioDescriptor {
            storage_dtype: Q4CaptureAudioDtype::F16,
            shape: [1, 32, 2, 178],
            byte_length: 22_784,
        };
        let mut receipt = snapshot_receipt(
            capture_id,
            "ignored-by-conversion.safetensors.partial".to_owned(),
            "c".repeat(64),
            1_000_000,
        );
        receipt.mode = Q4CaptureMode::LiveCapture;
        receipt.structural_carrier = Q4Slot::C;
        receipt.frozen_seed = None;
        receipt.frozen_roles = None;
        receipt.frozen_controls = None;
        receipt.control_events = Some(events);
        receipt.audio_policy = Q4CaptureAudioPolicy::OmittedTimingMismatch;
        receipt.audio_policy_reason = Some(Q4CaptureAudioPolicyReason::DurationMismatch);
        let evidence = carrier_evidence(&receipt.parents[2], 107, Some(audio));

        let request = resample_request_from_receipt(&receipt, WireUuid::new_v4(), &evidence)
            .expect("live conversion");

        assert_eq!(request.capture_mode, CaptureMode::LiveCapture);
        assert_eq!(request.seed, 19);
        assert_eq!(request.controls["control_events"][0]["seed"], 19);
        assert_eq!(request.controls["control_events"][1]["seed"], 23);
        assert_eq!(
            request.controls["control_events"][0]["roles"]["carrier"],
            "C"
        );
        assert!(matches!(
            request.audio,
            AudioDisposition::OmittedTimingMismatch {
                reason: AudioOmissionReason::DurationMismatch,
                ..
            }
        ));
    }

    #[test]
    fn payload_shape_dtype_size_and_hash_are_revalidated_against_receipt() {
        let directory = tempdir().expect("temporary app data");
        let capture_id = WireUuid::new_v4();
        let binding = Q4CaptureSpoolBinding::create(directory.path(), capture_id).expect("binding");
        let payload = synthetic_video_payload("F16", [1, 24, 7, 2, 3], 2);
        fs::write(&binding.payload, &payload).expect("synthetic payload");
        let measured = hash_reader(&mut Cursor::new(&payload)).expect("payload hash");
        let receipt = snapshot_receipt(
            capture_id,
            binding.payload.to_string_lossy().into_owned(),
            measured.sha256.to_string(),
            measured.byte_length,
        );
        let evidence = carrier_evidence(&receipt.parents[0], 22, None);
        let request = resample_request_from_receipt(&receipt, WireUuid::new_v4(), &evidence)
            .expect("request");
        let bound = binding
            .bind_finished_receipt(&receipt)
            .expect("host-bound payload");
        validate_payload_against_receipt(&receipt, &request, &bound)
            .expect("full payload validation");

        let mut wrong_shape = receipt.clone();
        wrong_shape.visual_shape = [1, 24, 7, 1, 6];
        let request = resample_request_from_receipt(&wrong_shape, WireUuid::new_v4(), &evidence)
            .expect("receipt remains internally shaped");
        assert_eq!(
            validate_payload_against_receipt(&wrong_shape, &request, &bound)
                .expect_err("actual shape must win")
                .code,
            "capture.payload_mismatch"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_packs_validates_atomically_imports_and_cleans_owned_spool() {
        let directory = tempdir().expect("temporary app data");
        let database = directory.path().join("library.sqlite3");
        let app_state = AppState::new(Library::open(&database).expect("library"));
        let capture_id = WireUuid::new_v4();
        let binding = Q4CaptureSpoolBinding::create(directory.path(), capture_id).expect("binding");
        let capture_root = binding.root().to_path_buf();
        let payload = synthetic_video_payload("F16", [1, 24, 7, 2, 3], 2);
        fs::write(&binding.payload, &payload).expect("synthetic payload");
        let measured = hash_reader(&mut Cursor::new(&payload)).expect("payload hash");
        let receipt = snapshot_receipt(
            capture_id,
            binding.payload.to_string_lossy().into_owned(),
            measured.sha256.to_string(),
            measured.byte_length,
        );
        let status = finished_status(receipt.clone());
        let evidence = carrier_evidence(&receipt.parents[0], 22, None);
        let output = directory.path().join("q4-resample.lc");
        let output_id = WireUuid::new_v4();

        let finalized = finalize_q4_capture_with_id(
            binding,
            &status,
            output.clone(),
            &evidence,
            app_state.importer(),
            output_id,
        )
        .await
        .expect("finalize and import");

        assert_eq!(finalized.cartridge_id, output_id.to_string());
        assert!(!capture_root.exists());
        let validated = open_validated(&output, &ValidationOptions::default())
            .expect("committed full validation");
        assert_eq!(
            validated.receipt().archive_sha256.to_string(),
            finalized.archive_sha256
        );
        assert_eq!(validated.manifest().parent_cartridges.len(), 4);
        drop(validated);
        let indexed = Library::open(&database)
            .expect("reopen library")
            .get_cartridge(&CartridgeKey::new_unchecked(
                finalized.archive_sha256.clone(),
            ))
            .expect("library query");
        assert!(indexed.is_some());
        let partial_count = fs::read_dir(directory.path())
            .expect("output directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".partial"))
            .count();
        assert_eq!(partial_count, 0);
    }

    #[test]
    fn coordinator_view_never_exposes_finished_worker_spool_path() {
        let capture_id = WireUuid::new_v4();
        let mut coordinator = Q4CaptureCoordinator::default();
        coordinator
            .begin(capture_id, Q4CaptureMode::Snapshot)
            .expect("begin snapshot");
        coordinator
            .observe(&capture_status(
                capture_id,
                Q4CaptureMode::Snapshot,
                Q4CaptureState::Capturing,
                2,
            ))
            .expect("capturing");
        let mut receipt = snapshot_receipt(
            capture_id,
            r"W:\private\capture.safetensors.partial".to_owned(),
            "c".repeat(64),
            1_000_000,
        );
        receipt.visual_shape[2] = 7;
        let mut finished = capture_status(
            capture_id,
            Q4CaptureMode::Snapshot,
            Q4CaptureState::Finished,
            7,
        );
        finished.receipt = Some(Box::new(receipt));
        let view = coordinator.observe(&finished).expect("finalizing");
        let json = serde_json::to_string(&view).expect("serialize path-free view");
        assert_eq!(view.state, "finalizing");
        assert!(!json.contains("payload"));
        assert!(!json.contains("W:"));
    }

    fn snapshot_receipt(
        capture_id: WireUuid,
        payload_path: String,
        payload_sha256: String,
        payload_bytes: u64,
    ) -> Q4CaptureReceipt {
        Q4CaptureReceipt {
            capture_id,
            mode: Q4CaptureMode::Snapshot,
            payload_path,
            payload_sha256,
            payload_bytes,
            storage_dtype: Q4CaptureVisualDtype::F16,
            visual_shape: [1, 24, 7, 2, 3],
            decoded_frame_count: 22,
            audio_policy: Q4CaptureAudioPolicy::SourceAbsent,
            audio_policy_reason: None,
            audio_descriptor: None,
            structural_carrier: Q4Slot::A,
            parents: parents(),
            frozen_seed: Some(77),
            frozen_roles: Some(Q4Roles::default()),
            frozen_controls: Some(Q4Controls::default()),
            control_events: None,
        }
    }

    fn parents() -> [Q4CaptureParent; 4] {
        [
            Q4CaptureParent {
                slot: Q4Slot::A,
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "a".repeat(64),
            },
            Q4CaptureParent {
                slot: Q4Slot::B,
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "b".repeat(64),
            },
            Q4CaptureParent {
                slot: Q4Slot::C,
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "c".repeat(64),
            },
            Q4CaptureParent {
                slot: Q4Slot::D,
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "d".repeat(64),
            },
        ]
    }

    fn carrier_evidence(
        parent: &Q4CaptureParent,
        decoded_frame_count: u64,
        audio_descriptor: Option<Q4CaptureAudioDescriptor>,
    ) -> Q4StructuralCarrierEvidence {
        Q4StructuralCarrierEvidence {
            slot: parent.slot,
            cartridge_id: parent.cartridge_id,
            archive_sha256: parent.archive_sha256.clone(),
            codec_family: H3_CODEC_FAMILY.to_owned(),
            profile: H3_PROFILE.to_owned(),
            profile_version: H3_PROFILE_VERSION.to_owned(),
            timing_contract: H3_TIMING_CONTRACT.to_owned(),
            timing_contract_version: H3_TIMING_CONTRACT_VERSION.to_owned(),
            decoded_frame_count,
            frame_rate_numerator: 24,
            frame_rate_denominator: 1,
            audio_descriptor,
        }
    }

    fn finished_status(receipt: Q4CaptureReceipt) -> Q4CaptureStatus {
        Q4CaptureStatus {
            capture_id: receipt.capture_id,
            mode: receipt.mode,
            state: Q4CaptureState::Finished,
            structural_carrier: receipt.structural_carrier,
            latent_slots: receipt.visual_shape[2],
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: None,
            stream_generation: Some(2),
            finalize_after_latent_slots: None,
            reason: None,
            receipt: Some(Box::new(receipt)),
        }
    }

    fn capture_status(
        capture_id: WireUuid,
        mode: Q4CaptureMode,
        state: Q4CaptureState,
        latent_slots: u64,
    ) -> Q4CaptureStatus {
        Q4CaptureStatus {
            capture_id,
            mode,
            state,
            structural_carrier: Q4Slot::A,
            latent_slots,
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: None,
            stream_generation: Some(2),
            finalize_after_latent_slots: None,
            reason: None,
            receipt: None,
        }
    }

    fn synthetic_video_payload(dtype: &str, shape: [u64; 5], element_bytes: usize) -> Vec<u8> {
        let elements = shape
            .into_iter()
            .try_fold(1_u64, u64::checked_mul)
            .expect("synthetic tensor size");
        let tensor_bytes =
            vec![0_u8; usize::try_from(elements).expect("usize elements") * element_bytes];
        let mut header = format!(
            r#"{{"video":{{"data_offsets":[0,{}],"dtype":"{}","shape":[{},{},{},{},{}]}}}}"#,
            tensor_bytes.len(),
            dtype,
            shape[0],
            shape[1],
            shape[2],
            shape[3],
            shape[4]
        )
        .into_bytes();
        while !header.len().is_multiple_of(8) {
            header.push(b' ');
        }
        let mut payload = Vec::with_capacity(8 + header.len() + tensor_bytes.len());
        payload.extend_from_slice(
            &u64::try_from(header.len())
                .expect("synthetic header length")
                .to_le_bytes(),
        );
        payload.extend_from_slice(&header);
        payload.extend_from_slice(&tensor_bytes);
        payload
    }
}
