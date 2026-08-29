//! Trusted application-side boundary for LD-D2 resample capture artifacts.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use latentdeck_cartridge::{
    manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, Identifier, ParentCartridge,
        Sha256Digest, SourceCartridgeRef,
    },
    resample::{CaptureMode, PayloadExpectation, ResampleManifestRequest},
};
use latentdeck_control::{
    D2CaptureAudioPolicy, D2CaptureAudioPolicyReason, D2CaptureMode, D2CaptureReceipt,
    D2CaptureState, D2CaptureStatus, D2Routing, WireUuid,
};
use serde::Serialize;
use serde_json::Value;

pub(crate) const APP_CAPTURE_MAX_LATENT_SLOTS: u64 = 16_382;
pub(crate) const APP_CAPTURE_MAX_VISUAL_BYTES: u64 = 1024 * 1024 * 1024;
const CAPTURE_DIRECTORY: &str = "capture-spool";
const D2_OPERATOR_ID: &str = "org.latentdeck.builtin.ld_d2";
const D2_OPERATOR_VERSION: &str = "0.1.0";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct D2CaptureView {
    pub(crate) capture_id: Option<String>,
    pub(crate) mode: Option<D2CaptureMode>,
    pub(crate) state: String,
    pub(crate) latent_slots: String,
    pub(crate) target_latent_slots: Option<String>,
    pub(crate) cartridge_id: Option<String>,
    pub(crate) archive_sha256: Option<String>,
    pub(crate) detail: Option<String>,
}

impl Default for D2CaptureView {
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
    mode: D2CaptureMode,
    phase: HostCapturePhase,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CaptureCoordinator {
    active: Option<ActiveCaptureIdentity>,
    view: D2CaptureView,
}

impl CaptureCoordinator {
    pub(crate) fn begin(
        &mut self,
        capture_id: WireUuid,
        mode: D2CaptureMode,
    ) -> Result<(), CaptureHostError> {
        if self.active.is_some() {
            return Err(CaptureHostError::new(
                "capture.already_active",
                "Only one LD-D2 capture may be active.",
            ));
        }
        self.active = Some(ActiveCaptureIdentity {
            capture_id,
            mode,
            phase: HostCapturePhase::AwaitingReset,
        });
        self.view = D2CaptureView {
            capture_id: Some(capture_id.to_string()),
            mode: Some(mode),
            state: "awaiting_reset".to_owned(),
            ..D2CaptureView::default()
        };
        Ok(())
    }

    pub(crate) fn observe(
        &mut self,
        status: &D2CaptureStatus,
    ) -> Result<D2CaptureView, CaptureHostError> {
        let active = self.active.as_mut().ok_or_else(|| {
            CaptureHostError::new(
                "capture.not_active",
                "No host capture is available for this worker status.",
            )
        })?;
        if active.capture_id != status.capture_id || active.mode != status.mode {
            return Err(CaptureHostError::new(
                "capture.id_mismatch",
                "The worker capture status does not match the active host capture.",
            ));
        }
        active.phase = next_phase(active.phase, status.state, active.mode)?;
        self.view = D2CaptureView {
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
        if status.state == D2CaptureState::Aborted {
            self.active = None;
            "aborted".clone_into(&mut self.view.state);
            self.view.detail = Some("The worker aborted capture safely.".to_owned());
        }
        Ok(self.view.clone())
    }

    pub(crate) fn complete(
        &mut self,
        cartridge_id: String,
        archive_sha256: String,
    ) -> Result<D2CaptureView, CaptureHostError> {
        let active = self.active.as_ref().ok_or_else(|| {
            CaptureHostError::new(
                "capture.not_active",
                "No finalized host capture is available.",
            )
        })?;
        if active.phase != HostCapturePhase::Finalizing {
            return Err(CaptureHostError::new(
                "capture.state_invalid",
                "Capture cannot complete before its finished receipt is bound.",
            ));
        }
        self.active = None;
        "finished".clone_into(&mut self.view.state);
        self.view.cartridge_id = Some(cartridge_id);
        self.view.archive_sha256 = Some(archive_sha256);
        self.view.detail =
            Some("Validated cartridge saved and imported into the Library.".to_owned());
        Ok(self.view.clone())
    }

    pub(crate) fn fail(&mut self) -> D2CaptureView {
        self.active = None;
        "error".clone_into(&mut self.view.state);
        self.view.detail = Some("Capture finalization failed safely.".to_owned());
        self.view.clone()
    }

    pub(crate) fn view(&self) -> D2CaptureView {
        self.view.clone()
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.is_some()
    }
}

fn next_phase(
    current: HostCapturePhase,
    worker: D2CaptureState,
    mode: D2CaptureMode,
) -> Result<HostCapturePhase, CaptureHostError> {
    let next = match (current, worker, mode) {
        (HostCapturePhase::AwaitingReset, D2CaptureState::AwaitingReset, _)
        | (_, D2CaptureState::Aborted, _) => current,
        (
            HostCapturePhase::AwaitingReset | HostCapturePhase::Capturing,
            D2CaptureState::Capturing,
            _,
        ) => HostCapturePhase::Capturing,
        (
            HostCapturePhase::Capturing | HostCapturePhase::StopArmed,
            D2CaptureState::StopArmed,
            D2CaptureMode::LiveCapture,
        ) => HostCapturePhase::StopArmed,
        (HostCapturePhase::Capturing, D2CaptureState::Finished, _)
        | (HostCapturePhase::StopArmed, D2CaptureState::Finished, D2CaptureMode::LiveCapture) => {
            HostCapturePhase::Finalizing
        }
        _ => {
            return Err(CaptureHostError::new(
                "capture.state_invalid",
                "The worker capture state transition is invalid.",
            ));
        }
    };
    Ok(next)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureHostError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl CaptureHostError {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CaptureSpoolBinding {
    capture_id: WireUuid,
    root: PathBuf,
    payload: PathBuf,
}

impl Drop for CaptureSpoolBinding {
    fn drop(&mut self) {
        self.cleanup();
    }
}

impl CaptureSpoolBinding {
    pub(crate) fn create(
        app_local_data: &Path,
        capture_id: WireUuid,
    ) -> Result<Self, CaptureHostError> {
        if !app_local_data.is_absolute() || capture_id.is_nil() {
            return Err(CaptureHostError::new(
                "capture.spool_root_invalid",
                "Capture storage must use an absolute app-local root and canonical identity.",
            ));
        }
        reject_reparse(app_local_data)?;
        let app_local_data = fs::canonicalize(app_local_data).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_root_invalid",
                "The app-local capture storage root is unavailable.",
            )
        })?;
        let container = app_local_data.join(CAPTURE_DIRECTORY);
        fs::create_dir_all(&container).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_root_invalid",
                "The app-local capture storage root could not be created.",
            )
        })?;
        reject_reparse(&container)?;
        let container = fs::canonicalize(&container).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_root_invalid",
                "The app-local capture storage root is unavailable.",
            )
        })?;
        if !container.starts_with(&app_local_data) {
            return Err(CaptureHostError::new(
                "capture.spool_root_escape",
                "Capture storage escaped the application data directory.",
            ));
        }

        let root = container.join(capture_id.to_string());
        fs::create_dir(&root).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_root_invalid",
                "A fresh capture storage directory could not be created.",
            )
        })?;
        reject_reparse(&root)?;
        let root = fs::canonicalize(&root).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_root_invalid",
                "The fresh capture storage directory is unavailable.",
            )
        })?;
        if root.parent() != Some(container.as_path()) {
            return Err(CaptureHostError::new(
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
        receipt: &D2CaptureReceipt,
    ) -> Result<PathBuf, CaptureHostError> {
        if receipt.capture_id != self.capture_id {
            return Err(CaptureHostError::new(
                "capture.id_mismatch",
                "The worker capture receipt does not match the active host capture.",
            ));
        }
        let reported = PathBuf::from(&receipt.payload_path);
        if reported != self.payload {
            return Err(CaptureHostError::new(
                "capture.spool_path_mismatch",
                "The worker capture receipt did not bind the exact expected spool path.",
            ));
        }
        reject_reparse(&self.root)?;
        reject_reparse(&reported)?;
        let metadata = fs::metadata(&reported).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_missing",
                "The finalized capture spool is unavailable.",
            )
        })?;
        if !metadata.is_file() {
            return Err(CaptureHostError::new(
                "capture.spool_invalid",
                "The finalized capture spool is not a regular file.",
            ));
        }
        if metadata.len() != receipt.payload_bytes {
            return Err(CaptureHostError::new(
                "capture.spool_size_mismatch",
                "The finalized capture spool size does not match its receipt.",
            ));
        }
        let canonical_root = fs::canonicalize(&self.root).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_root_invalid",
                "The retained capture storage root is unavailable.",
            )
        })?;
        let canonical_payload = fs::canonicalize(&reported).map_err(|_| {
            CaptureHostError::new(
                "capture.spool_missing",
                "The finalized capture spool is unavailable.",
            )
        })?;
        if canonical_root != self.root || canonical_payload.parent() != Some(self.root.as_path()) {
            return Err(CaptureHostError::new(
                "capture.spool_path_mismatch",
                "The finalized capture spool escaped its retained root.",
            ));
        }
        Ok(reported)
    }
}

pub(crate) fn validate_output_path(selected: PathBuf) -> Result<PathBuf, CaptureHostError> {
    if !selected.is_absolute() {
        return Err(CaptureHostError::new(
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
            return Err(CaptureHostError::new(
                "capture.output_path_invalid",
                "Resampled cartridges must use the .lc extension.",
            ));
        }
    }
    if output.exists() {
        return Err(CaptureHostError::new(
            "target.exists",
            "The selected cartridge output already exists; capture never overwrites files.",
        ));
    }
    let parent = output.parent().ok_or_else(|| {
        CaptureHostError::new(
            "capture.output_path_invalid",
            "The selected cartridge output has no parent directory.",
        )
    })?;
    if !parent.is_dir() {
        return Err(CaptureHostError::new(
            "capture.output_path_invalid",
            "The selected cartridge output directory is unavailable.",
        ));
    }
    Ok(output)
}

fn reject_reparse(path: &Path) -> Result<(), CaptureHostError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| {
        CaptureHostError::new(
            "capture.spool_root_invalid",
            "Capture storage metadata is unavailable.",
        )
    })?;
    if is_reparse(&metadata) {
        return Err(CaptureHostError::new(
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

pub(crate) fn resample_request_from_receipt(
    receipt: &D2CaptureReceipt,
    cartridge_id: WireUuid,
) -> Result<ResampleManifestRequest, CaptureHostError> {
    if cartridge_id.is_nil() {
        return Err(CaptureHostError::new(
            "capture.cartridge_id_invalid",
            "The host-generated output cartridge identity is invalid.",
        ));
    }
    let (controls, seed) = controls_and_seed(receipt)?;
    Ok(ResampleManifestRequest {
        cartridge_id: CartridgeId(cartridge_id.to_string()),
        expected_payload: PayloadExpectation {
            byte_length: receipt.payload_bytes,
            sha256: Sha256Digest(receipt.payload_sha256.clone()),
        },
        capture_mode: mode(receipt.mode),
        audio: audio_disposition(receipt)?,
        parent_cartridges: parent_cartridges(receipt),
        operator_id: Identifier(D2_OPERATOR_ID.to_owned()),
        operator_version: D2_OPERATOR_VERSION.to_owned(),
        seed,
        controls,
    })
}

const fn mode(mode: D2CaptureMode) -> CaptureMode {
    match mode {
        D2CaptureMode::Snapshot => CaptureMode::Snapshot,
        D2CaptureMode::LiveCapture => CaptureMode::LiveCapture,
    }
}

fn audio_disposition(receipt: &D2CaptureReceipt) -> Result<AudioDisposition, CaptureHostError> {
    let carrier = receipt
        .parents
        .iter()
        .find(|parent| parent.slot == receipt.structural_carrier)
        .ok_or_else(|| {
            CaptureHostError::new(
                "capture.receipt_invalid",
                "The capture receipt does not identify its structural carrier parent.",
            )
        })?;
    let source_cartridge = SourceCartridgeRef {
        cartridge_id: CartridgeId(carrier.cartridge_id.to_string()),
        archive_sha256: Sha256Digest(carrier.archive_sha256.clone()),
    };
    match receipt.audio_policy {
        D2CaptureAudioPolicy::SourceAbsent => {
            if receipt.audio_policy_reason.is_some() || receipt.audio_descriptor.is_some() {
                return Err(invalid_audio_policy());
            }
            Ok(AudioDisposition::SourceAbsent)
        }
        D2CaptureAudioPolicy::CopiedFromCarrierExact => {
            if receipt.audio_policy_reason.is_some() || receipt.audio_descriptor.is_none() {
                return Err(invalid_audio_policy());
            }
            Ok(AudioDisposition::CopiedFromCarrierExact { source_cartridge })
        }
        D2CaptureAudioPolicy::OmittedTimingMismatch => {
            if receipt.audio_descriptor.is_some() {
                return Err(invalid_audio_policy());
            }
            let reason = match receipt.audio_policy_reason {
                Some(D2CaptureAudioPolicyReason::DurationMismatch) => {
                    AudioOmissionReason::DurationMismatch
                }
                Some(D2CaptureAudioPolicyReason::TemporalMappingMismatch) => {
                    AudioOmissionReason::TemporalMappingMismatch
                }
                Some(D2CaptureAudioPolicyReason::DurationAndMappingMismatch) => {
                    AudioOmissionReason::DurationAndMappingMismatch
                }
                None => return Err(invalid_audio_policy()),
            };
            Ok(AudioDisposition::OmittedTimingMismatch {
                source_cartridge,
                reason,
            })
        }
    }
}

const fn invalid_audio_policy() -> CaptureHostError {
    CaptureHostError::new(
        "capture.audio_policy_invalid",
        "The capture receipt audio policy is internally inconsistent.",
    )
}

fn parent_cartridges(receipt: &D2CaptureReceipt) -> Vec<ParentCartridge> {
    receipt
        .parents
        .iter()
        .map(|parent| {
            let carrier = parent.slot == receipt.structural_carrier;
            let slot = match parent.slot {
                D2Routing::A => 'a',
                D2Routing::B => 'b',
            };
            ParentCartridge {
                cartridge_id: CartridgeId(parent.cartridge_id.to_string()),
                archive_sha256: Sha256Digest(parent.archive_sha256.clone()),
                role: Identifier(format!(
                    "{}_{}",
                    if carrier { "carrier" } else { "donor" },
                    slot
                )),
            }
        })
        .collect()
}

fn controls_and_seed(
    receipt: &D2CaptureReceipt,
) -> Result<(BTreeMap<String, Value>, u64), CaptureHostError> {
    match receipt.mode {
        D2CaptureMode::Snapshot => {
            let controls = receipt.frozen_controls.as_ref().ok_or_else(|| {
                CaptureHostError::new(
                    "capture.receipt_invalid",
                    "Snapshot receipt is missing its frozen controls.",
                )
            })?;
            let seed = receipt.frozen_seed.ok_or_else(|| {
                CaptureHostError::new(
                    "capture.receipt_invalid",
                    "Snapshot receipt is missing its frozen seed.",
                )
            })?;
            if receipt.control_events.is_some() {
                return Err(CaptureHostError::new(
                    "capture.receipt_invalid",
                    "Snapshot receipt cannot contain live control events.",
                ));
            }
            let Value::Object(object) = serde_json::to_value(controls).map_err(|_| {
                CaptureHostError::new(
                    "capture.provenance_invalid",
                    "Snapshot controls are not JSON-safe.",
                )
            })?
            else {
                return Err(CaptureHostError::new(
                    "capture.provenance_invalid",
                    "Snapshot controls are not a closed JSON object.",
                ));
            };
            Ok((object.into_iter().collect(), seed))
        }
        D2CaptureMode::LiveCapture => {
            if receipt.frozen_seed.is_some() || receipt.frozen_controls.is_some() {
                return Err(CaptureHostError::new(
                    "capture.receipt_invalid",
                    "Live Capture receipt cannot contain snapshot-frozen state.",
                ));
            }
            let events = receipt.control_events.as_ref().ok_or_else(|| {
                CaptureHostError::new(
                    "capture.receipt_invalid",
                    "Live Capture receipt is missing its bounded control events.",
                )
            })?;
            let first = events.first().ok_or_else(|| {
                CaptureHostError::new(
                    "capture.receipt_invalid",
                    "Live Capture receipt has no initial control event.",
                )
            })?;
            let mut controls = BTreeMap::new();
            controls.insert(
                "control_events".to_owned(),
                serde_json::to_value(events).map_err(|_| {
                    CaptureHostError::new(
                        "capture.provenance_invalid",
                        "Live Capture control events are not JSON-safe.",
                    )
                })?,
            );
            controls.insert(
                "structural_carrier".to_owned(),
                serde_json::to_value(receipt.structural_carrier).map_err(|_| {
                    CaptureHostError::new(
                        "capture.provenance_invalid",
                        "Live Capture structural carrier is not JSON-safe.",
                    )
                })?,
            );
            Ok((controls, first.seed))
        }
    }
}

#[cfg(test)]
mod tests {
    use latentdeck_control::{
        BoundedVec, D2CaptureAudioDescriptor, D2CaptureAudioDtype, D2CaptureControlEvent,
        D2CaptureParent, D2CaptureVisualDtype, D2Controls,
    };
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn exact_receipt_path_is_bound_to_its_host_created_capture_root() {
        let directory = tempdir().expect("temporary app data");
        let capture_id = WireUuid::new_v4();
        let binding = CaptureSpoolBinding::create(directory.path(), capture_id).expect("binding");
        fs::write(&binding.payload, b"bounded-spool").expect("synthetic spool marker");
        let mut receipt =
            snapshot_receipt(capture_id, binding.payload.to_string_lossy().into_owned());
        receipt.payload_bytes = 13;

        assert_eq!(
            binding
                .bind_finished_receipt(&receipt)
                .expect("exact receipt"),
            binding.payload
        );
        assert_eq!(
            binding.root(),
            binding.payload.parent().expect("payload parent")
        );
        assert!(binding.root().is_absolute());
    }

    #[test]
    fn receipt_cannot_escape_to_a_sibling_or_rebind_another_capture() {
        let directory = tempdir().expect("temporary app data");
        let capture_id = WireUuid::new_v4();
        let binding = CaptureSpoolBinding::create(directory.path(), capture_id).expect("binding");
        let sibling = binding
            .root()
            .parent()
            .expect("capture container")
            .join(format!("{}.safetensors.partial", WireUuid::new_v4()));
        fs::write(&sibling, b"wrong-spool").expect("sibling marker");
        let receipt = snapshot_receipt(capture_id, sibling.to_string_lossy().into_owned());

        let error = binding
            .bind_finished_receipt(&receipt)
            .expect_err("sibling path must fail");
        assert_eq!(error.code, "capture.spool_path_mismatch");

        let other = snapshot_receipt(
            WireUuid::new_v4(),
            binding.payload.to_string_lossy().into_owned(),
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
    fn snapshot_receipt_becomes_exact_genealogy_controls_seed_and_audio_policy() {
        let capture_id = WireUuid::new_v4();
        let mut receipt = snapshot_receipt(capture_id, "ignored-by-conversion".to_owned());
        receipt.audio_policy = D2CaptureAudioPolicy::CopiedFromCarrierExact;
        receipt.audio_descriptor = Some(D2CaptureAudioDescriptor {
            storage_dtype: D2CaptureAudioDtype::F16,
            shape: [1, 32, 2, 37],
            byte_length: 4_736,
        });
        let output_id = WireUuid::new_v4();

        let request =
            resample_request_from_receipt(&receipt, output_id).expect("snapshot conversion");

        assert_eq!(request.cartridge_id.0, output_id.to_string());
        assert_eq!(request.capture_mode, CaptureMode::Snapshot);
        assert_eq!(request.seed, 77);
        assert_eq!(request.controls["algorithm"], serde_json::json!("LINEAR"));
        assert_eq!(request.parent_cartridges[0].role.0, "carrier_a");
        assert_eq!(request.parent_cartridges[1].role.0, "donor_b");
        let AudioDisposition::CopiedFromCarrierExact { source_cartridge } = request.audio else {
            panic!("exact carrier audio policy");
        };
        assert_eq!(
            source_cartridge.cartridge_id.0,
            receipt.parents[0].cartridge_id.to_string()
        );
    }

    #[test]
    fn live_receipt_preserves_bounded_control_event_history() {
        let capture_id = WireUuid::new_v4();
        let initial = D2CaptureControlEvent {
            slot_offset: 0,
            controls: D2Controls::default(),
            seed: 19,
        };
        let receipt = D2CaptureReceipt {
            capture_id,
            mode: D2CaptureMode::LiveCapture,
            payload_path: "ignored-by-conversion".to_owned(),
            payload_sha256: "c".repeat(64),
            payload_bytes: 1_000_000,
            storage_dtype: D2CaptureVisualDtype::F16,
            visual_shape: [1, 24, 7, 28, 50],
            decoded_frame_count: 22,
            audio_policy: D2CaptureAudioPolicy::OmittedTimingMismatch,
            audio_policy_reason: Some(D2CaptureAudioPolicyReason::DurationMismatch),
            audio_descriptor: None,
            structural_carrier: D2Routing::A,
            parents: parents(),
            frozen_seed: None,
            frozen_controls: None,
            control_events: Some(BoundedVec::try_from_vec(vec![initial]).expect("event")),
        };

        let request =
            resample_request_from_receipt(&receipt, WireUuid::new_v4()).expect("live conversion");

        assert_eq!(request.capture_mode, CaptureMode::LiveCapture);
        assert_eq!(request.seed, 19);
        assert_eq!(
            request.controls["control_events"][0]["slot_offset"],
            serde_json::json!(0)
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
    fn coordinator_allows_one_capture_and_waits_for_live_stop_boundary() {
        let capture_id = WireUuid::new_v4();
        let mut coordinator = CaptureCoordinator::default();
        coordinator
            .begin(capture_id, D2CaptureMode::LiveCapture)
            .expect("begin live");
        assert_eq!(
            coordinator
                .begin(WireUuid::new_v4(), D2CaptureMode::Snapshot)
                .expect_err("one capture only")
                .code,
            "capture.already_active"
        );

        let capturing = capture_status(
            capture_id,
            D2CaptureMode::LiveCapture,
            D2CaptureState::Capturing,
            3,
        );
        assert_eq!(
            coordinator.observe(&capturing).expect("capturing").state,
            "capturing"
        );
        let mut armed = capture_status(
            capture_id,
            D2CaptureMode::LiveCapture,
            D2CaptureState::StopArmed,
            3,
        );
        armed.finalize_after_latent_slots = Some(7);
        assert_eq!(
            coordinator.observe(&armed).expect("stop armed").state,
            "stop_armed"
        );
    }

    #[test]
    fn capture_view_is_path_free_even_when_finished_status_contains_a_spool_path() {
        let capture_id = WireUuid::new_v4();
        let mut coordinator = CaptureCoordinator::default();
        coordinator
            .begin(capture_id, D2CaptureMode::Snapshot)
            .expect("begin snapshot");
        let capturing = capture_status(
            capture_id,
            D2CaptureMode::Snapshot,
            D2CaptureState::Capturing,
            2,
        );
        coordinator.observe(&capturing).expect("capturing");
        let mut finished = capture_status(
            capture_id,
            D2CaptureMode::Snapshot,
            D2CaptureState::Finished,
            7,
        );
        finished.receipt = Some(snapshot_receipt(
            capture_id,
            r"W:\private\capture.safetensors.partial".to_owned(),
        ));
        let view = coordinator.observe(&finished).expect("finalizing");
        let json = serde_json::to_string(&view).expect("serialize path-free view");
        assert_eq!(view.state, "finalizing");
        assert!(!json.contains("payload"));
        assert!(!json.contains("W:"));
    }

    fn snapshot_receipt(capture_id: WireUuid, payload_path: String) -> D2CaptureReceipt {
        D2CaptureReceipt {
            capture_id,
            mode: D2CaptureMode::Snapshot,
            payload_path,
            payload_sha256: "c".repeat(64),
            payload_bytes: 1_000_000,
            storage_dtype: D2CaptureVisualDtype::F16,
            visual_shape: [1, 24, 7, 28, 50],
            decoded_frame_count: 22,
            audio_policy: D2CaptureAudioPolicy::SourceAbsent,
            audio_policy_reason: None,
            audio_descriptor: None,
            structural_carrier: D2Routing::A,
            parents: parents(),
            frozen_seed: Some(77),
            frozen_controls: Some(D2Controls::default()),
            control_events: None,
        }
    }

    fn parents() -> [D2CaptureParent; 2] {
        [
            D2CaptureParent {
                slot: D2Routing::A,
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "a".repeat(64),
            },
            D2CaptureParent {
                slot: D2Routing::B,
                cartridge_id: WireUuid::new_v4(),
                archive_sha256: "b".repeat(64),
            },
        ]
    }

    fn capture_status(
        capture_id: WireUuid,
        mode: D2CaptureMode,
        state: D2CaptureState,
        latent_slots: u64,
    ) -> D2CaptureStatus {
        D2CaptureStatus {
            capture_id,
            mode,
            state,
            structural_carrier: D2Routing::A,
            latent_slots,
            current_generation: None,
            minimum_new_generation: None,
            target_latent_slots: match mode {
                D2CaptureMode::Snapshot => Some(7),
                D2CaptureMode::LiveCapture => Some(0),
            },
            stream_generation: Some(2),
            finalize_after_latent_slots: None,
            reason: None,
            receipt: None,
        }
    }
}
