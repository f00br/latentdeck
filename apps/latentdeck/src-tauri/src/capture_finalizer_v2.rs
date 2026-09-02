//! Host-owned staging and codec-neutral LC finalization for Protocol 2 capture.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use latentdeck_cartridge::{
    LC_SPEC_VERSION,
    hash::{Sha256Hash, hash_path},
    limits::ValidationLimits,
    manifest::{
        AudioDisposition, AudioOmissionReason, CartridgeId, Identifier, ManifestV0_1,
        OperationRecord, ParentCartridge, PayloadDescriptor, ProducerDescriptor, Provenance,
        Rational, Sha256Digest, SourceCartridgeRef, SpecVersion, TensorStream,
    },
    resample::{PayloadExpectation, ProfileResampleRequest, pack_profile_resample_atomic},
    writer::WriteOptions,
};
use latentdeck_control::v2::{CaptureMode, ControlBinding, ControlValue, RoleBinding};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::library_state::LibraryImporter;

const CAPTURE_DIRECTORY: &str = "capture-spool-v2";

#[derive(Debug)]
pub(crate) struct CaptureStagingRoot {
    capture_id: Uuid,
    container: PathBuf,
    root: PathBuf,
}

#[derive(Debug)]
pub(crate) struct BoundCapturePayload {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) byte_length: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct CaptureSourceEvidence {
    pub(crate) physical_slot: u8,
    pub(crate) archive_sha256: String,
    pub(crate) manifest: ManifestV0_1,
}

#[derive(Clone, Debug)]
pub(crate) struct CaptureFinalizationContext {
    pub(crate) sources: Vec<CaptureSourceEvidence>,
    pub(crate) roles: Vec<RoleBinding>,
    pub(crate) controls: Vec<ControlBinding>,
    pub(crate) operator_id: String,
    pub(crate) operator_version: String,
    pub(crate) seed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaptureArtifactEvidence {
    pub(crate) capture_id: Uuid,
    pub(crate) staged_payload_path: String,
    pub(crate) payload_sha256: String,
    pub(crate) payload_byte_length: u64,
    pub(crate) latent_slots: u64,
    pub(crate) decoded_frame_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FinalizedCapture {
    pub(crate) cartridge_id: String,
    pub(crate) archive_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureFinalizerError {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

impl CaptureFinalizerError {
    /// Only the final Library import is outside the worker/staging trust
    /// boundary. Unknown future codes fail closed.
    pub(crate) fn is_worker_trust_boundary(self) -> bool {
        self.code != "capture.import_failed"
    }
}

impl CaptureStagingRoot {
    pub(crate) fn create(
        app_local_data: &Path,
        capture_id: Uuid,
    ) -> Result<Self, CaptureFinalizerError> {
        if !app_local_data.is_absolute() || capture_id.is_nil() {
            return Err(invalid_root());
        }
        reject_reparse(app_local_data)?;
        let app_local_data = fs::canonicalize(app_local_data).map_err(|_| invalid_root())?;
        let container = app_local_data.join(CAPTURE_DIRECTORY);
        fs::create_dir_all(&container).map_err(|_| invalid_root())?;
        reject_reparse(&container)?;
        let container = fs::canonicalize(container).map_err(|_| invalid_root())?;
        if !container.starts_with(&app_local_data) {
            return Err(root_escape());
        }
        let root = container.join(capture_id.hyphenated().to_string());
        fs::create_dir(&root).map_err(|_| invalid_root())?;
        reject_reparse(&root)?;
        let root = fs::canonicalize(root).map_err(|_| invalid_root())?;
        if root.parent() != Some(container.as_path()) {
            return Err(root_escape());
        }
        Ok(Self {
            capture_id,
            container,
            root,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) const fn capture_id(&self) -> Uuid {
        self.capture_id
    }

    pub(crate) fn bind_artifact(
        &self,
        reported_path: &str,
        reported_sha256: &str,
        reported_byte_length: u64,
        maximum_byte_length: u64,
    ) -> Result<BoundCapturePayload, CaptureFinalizerError> {
        Sha256Hash::parse(reported_sha256).map_err(|_| artifact_mismatch())?;
        if reported_byte_length == 0
            || maximum_byte_length == 0
            || reported_byte_length > maximum_byte_length
        {
            return Err(artifact_mismatch());
        }
        let reported = PathBuf::from(reported_path);
        if !reported.is_absolute() {
            return Err(path_untrusted());
        }
        self.revalidate_root()?;
        reject_path_chain(&self.root, &reported)?;
        let metadata = fs::symlink_metadata(&reported).map_err(|_| path_untrusted())?;
        if !metadata.is_file() || is_reparse(&metadata) {
            return Err(path_untrusted());
        }
        let canonical = fs::canonicalize(&reported).map_err(|_| path_untrusted())?;
        if canonical == self.root || !canonical.starts_with(&self.root) {
            return Err(path_untrusted());
        }
        let measured = hash_path(&canonical).map_err(|_| artifact_mismatch())?;
        let sha256 = measured.sha256.to_string();
        if measured.byte_length != reported_byte_length || sha256 != reported_sha256 {
            return Err(artifact_mismatch());
        }
        Ok(BoundCapturePayload {
            path: canonical,
            sha256,
            byte_length: measured.byte_length,
        })
    }

    pub(crate) fn cleanup(&self) {
        let expected_name = self.capture_id.hyphenated().to_string();
        if self.revalidate_root().is_ok()
            && self.root.parent() == Some(self.container.as_path())
            && self.root.file_name().and_then(|value| value.to_str())
                == Some(expected_name.as_str())
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn revalidate_root(&self) -> Result<(), CaptureFinalizerError> {
        reject_reparse(&self.container)?;
        reject_reparse(&self.root)?;
        let container = fs::canonicalize(&self.container).map_err(|_| invalid_root())?;
        let root = fs::canonicalize(&self.root).map_err(|_| invalid_root())?;
        if container != self.container
            || root != self.root
            || root.parent() != Some(container.as_path())
        {
            return Err(root_escape());
        }
        Ok(())
    }
}

#[cfg(test)]
async fn finalize_capture(
    binding: CaptureStagingRoot,
    artifact: CaptureArtifactEvidence,
    mode: CaptureMode,
    reset_events: u32,
    context: CaptureFinalizationContext,
    output: PathBuf,
    library_importer: LibraryImporter,
) -> Result<FinalizedCapture, CaptureFinalizerError> {
    finalize_capture_with_carrier(
        binding,
        artifact,
        mode,
        reset_events,
        context,
        "carrier",
        latentdeck_control::v2::MAX_CAPTURE_LATENT_SLOTS,
        latentdeck_control::v2::MAX_CAPTURE_VISUAL_BYTES,
        24,
        output,
        library_importer,
    )
    .await
}

/// Finalize a generic Deck capture using the exact structural carrier role
/// declared by the trusted `.ld` manifest. First-party Decks retain the
/// original `carrier` wrapper above for source compatibility.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn finalize_capture_with_carrier(
    binding: CaptureStagingRoot,
    artifact: CaptureArtifactEvidence,
    mode: CaptureMode,
    reset_events: u32,
    context: CaptureFinalizationContext,
    structural_carrier_role: &str,
    maximum_latent_slots: u64,
    maximum_visual_bytes: u64,
    maximum_decoded_frames_per_slot: u64,
    output: PathBuf,
    library_importer: LibraryImporter,
) -> Result<FinalizedCapture, CaptureFinalizerError> {
    let maximum_decoded_frames = artifact
        .latent_slots
        .checked_mul(maximum_decoded_frames_per_slot)
        .ok_or_else(artifact_mismatch)?;
    if artifact.capture_id != binding.capture_id()
        || artifact.latent_slots == 0
        || artifact.latent_slots > maximum_latent_slots
        || artifact.decoded_frame_count == 0
        || artifact.decoded_frame_count < artifact.latent_slots
        || artifact.decoded_frame_count > maximum_decoded_frames
        || artifact.payload_byte_length > maximum_visual_bytes
        || reset_events > 32
    {
        return Err(artifact_mismatch());
    }
    let bound = binding.bind_artifact(
        &artifact.staged_payload_path,
        &artifact.payload_sha256,
        artifact.payload_byte_length,
        maximum_visual_bytes,
    )?;
    let cartridge_id = Uuid::new_v4();
    let request = build_manifest_request(
        &context,
        &artifact,
        mode,
        reset_events,
        cartridge_id,
        &bound,
        structural_carrier_role,
    )?;
    let payload = bound.path.clone();
    let packed = tauri::async_runtime::spawn_blocking(move || {
        pack_profile_resample_atomic(&request, payload, output, &WriteOptions::default())
    })
    .await
    .map_err(|_| finalize_error())?
    .map_err(|_| finalize_error())?;
    drop(binding);

    let archive_sha256 = packed.validation.archive_sha256.to_string();
    let imported = library_importer
        .import_generated(packed.output_path)
        .await
        .map_err(|_| import_error())?;
    if imported.as_str() != archive_sha256 {
        return Err(import_error());
    }
    Ok(FinalizedCapture {
        cartridge_id: cartridge_id.hyphenated().to_string(),
        archive_sha256,
    })
}

#[allow(clippy::too_many_lines)]
fn build_manifest_request(
    context: &CaptureFinalizationContext,
    artifact: &CaptureArtifactEvidence,
    mode: CaptureMode,
    reset_events: u32,
    cartridge_id: Uuid,
    payload: &BoundCapturePayload,
    structural_carrier_role: &str,
) -> Result<ProfileResampleRequest, CaptureFinalizerError> {
    if context.sources.is_empty()
        || context.sources.len() > 16
        || context.seed > 9_007_199_254_740_991
    {
        return Err(context_error());
    }
    let mut slots = BTreeSet::new();
    for source in &context.sources {
        if source.physical_slot == 0
            || !slots.insert(source.physical_slot)
            || Sha256Hash::parse(&source.archive_sha256).is_err()
            || source
                .manifest
                .validate_common(&ValidationLimits::default())
                .is_err()
        {
            return Err(context_error());
        }
    }
    if structural_carrier_role.is_empty() {
        return Err(context_error());
    }
    let carrier_role = context
        .roles
        .iter()
        .find(|role| role.role == structural_carrier_role)
        .ok_or_else(context_error)?;
    let carrier = context
        .sources
        .iter()
        .find(|source| source.physical_slot == carrier_role.physical_slot)
        .ok_or_else(context_error)?;
    let role_by_slot = context
        .roles
        .iter()
        .map(|role| (role.physical_slot, role.role.as_str()))
        .collect::<BTreeMap<_, _>>();
    if role_by_slot.len() != context.sources.len()
        || context
            .sources
            .iter()
            .any(|source| !role_by_slot.contains_key(&source.physical_slot))
    {
        return Err(context_error());
    }

    let payload_descriptor = carrier
        .manifest
        .payloads
        .first()
        .filter(|_| carrier.manifest.payloads.len() == 1)
        .ok_or_else(context_error)?;
    let mut visual = carrier
        .manifest
        .tensors
        .iter()
        .find(|tensor| tensor.stream == TensorStream::Visual)
        .cloned()
        .ok_or_else(context_error)?;
    if carrier
        .manifest
        .tensors
        .iter()
        .filter(|tensor| tensor.stream == TensorStream::Visual)
        .count()
        != 1
        || visual.shape.len() != 5
        || visual.shape[0] != 1
        || visual.shape[2] == 0
        || !visual.runtime_dtype.is_supported()
    {
        return Err(context_error());
    }
    visual.shape[2] = artifact.latent_slots;
    visual.storage_dtype = visual.runtime_dtype;

    let frame_rate = carrier.manifest.timing.decoded_video.frame_rate;
    let duration_numerator = artifact
        .decoded_frame_count
        .checked_mul(frame_rate.denominator)
        .ok_or_else(context_error)?;
    let duration =
        Rational::reduced(duration_numerator, frame_rate.numerator).ok_or_else(context_error)?;
    let carrier_video = &carrier.manifest.timing.decoded_video;
    let carrier_duration_matches = duration == carrier_video.duration;
    let carrier_has_audio = carrier
        .manifest
        .tensors
        .iter()
        .any(|tensor| tensor.stream == TensorStream::Audio);
    let carrier_reference = SourceCartridgeRef {
        cartridge_id: carrier.manifest.cartridge_id.clone(),
        archive_sha256: Sha256Digest(carrier.archive_sha256.clone()),
    };
    let audio = if carrier_has_audio {
        AudioDisposition::OmittedTimingMismatch {
            source_cartridge: carrier_reference,
            reason: if carrier_duration_matches {
                AudioOmissionReason::TemporalMappingMismatch
            } else {
                AudioOmissionReason::DurationAndMappingMismatch
            },
        }
    } else {
        AudioDisposition::SourceAbsent
    };

    let mut controls = control_values(&context.controls)?;
    for reserved in [
        "capture_mode",
        "capture_reset_events",
        "capture_role_bindings",
    ] {
        if controls.contains_key(reserved) {
            return Err(context_error());
        }
    }
    controls.insert(
        "capture_mode".to_owned(),
        Value::String(
            match mode {
                CaptureMode::Snapshot => "snapshot",
                CaptureMode::LiveCapture => "live_capture",
            }
            .to_owned(),
        ),
    );
    controls.insert("capture_reset_events".to_owned(), json!(reset_events));
    let roles = context
        .roles
        .iter()
        .map(|role| {
            Value::Object(Map::from_iter([
                ("role".to_owned(), Value::String(role.role.clone())),
                ("physical_slot".to_owned(), json!(role.physical_slot)),
            ]))
        })
        .collect();
    controls.insert("capture_role_bindings".to_owned(), Value::Array(roles));

    let parent_cartridges = context
        .sources
        .iter()
        .map(|source| ParentCartridge {
            cartridge_id: source.manifest.cartridge_id.clone(),
            archive_sha256: Sha256Digest(source.archive_sha256.clone()),
            role: Identifier(role_by_slot[&source.physical_slot].to_owned()),
        })
        .collect();
    let manifest = ManifestV0_1 {
        spec_version: SpecVersion(LC_SPEC_VERSION.to_owned()),
        cartridge_id: CartridgeId(cartridge_id.hyphenated().to_string()),
        codec: carrier.manifest.codec.clone(),
        payloads: vec![PayloadDescriptor {
            path: payload_descriptor.path.clone(),
            media_type: payload_descriptor.media_type.clone(),
            byte_length: payload.byte_length,
            sha256: Sha256Digest(payload.sha256.clone()),
        }],
        tensors: vec![visual],
        timing: latentdeck_cartridge::manifest::TimingDescriptor {
            contract: carrier.manifest.timing.contract.clone(),
            contract_version: carrier.manifest.timing.contract_version.clone(),
            decoded_video: latentdeck_cartridge::manifest::DecodedVideoDescriptor {
                width: carrier_video.width,
                height: carrier_video.height,
                frame_count: artifact.decoded_frame_count,
                frame_rate,
                duration,
            },
        },
        audio,
        preview: None,
        provenance: Provenance {
            created_by: ProducerDescriptor {
                name: Identifier("latentdeck-resample".to_owned()),
                version: "0.2.0".to_owned(),
            },
            created_at: None,
            sources: Vec::new(),
        },
        parent_cartridges,
        operation_history: vec![OperationRecord {
            operator_id: Identifier(context.operator_id.clone()),
            operator_version: context.operator_version.clone(),
            seed: context.seed,
            controls,
        }],
    };
    manifest
        .validate_common(&ValidationLimits::default())
        .map_err(|_| context_error())?;
    Ok(ProfileResampleRequest {
        manifest,
        expected_payload: PayloadExpectation {
            byte_length: payload.byte_length,
            sha256: Sha256Digest(payload.sha256.clone()),
        },
    })
}

fn control_values(
    values: &[ControlBinding],
) -> Result<BTreeMap<String, Value>, CaptureFinalizerError> {
    let mut controls = BTreeMap::new();
    for binding in values {
        let value = match &binding.value {
            ControlValue::Boolean(value) => Value::Bool(*value),
            ControlValue::Integer(value) => json!(value),
            ControlValue::Number(value) if value.is_finite() => json!(value),
            ControlValue::Text(value) => Value::String(value.clone()),
            ControlValue::Number(_) => return Err(context_error()),
        };
        if binding.name.is_empty() || controls.insert(binding.name.clone(), value).is_some() {
            return Err(context_error());
        }
    }
    Ok(controls)
}

impl Drop for CaptureStagingRoot {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn reject_path_chain(root: &Path, reported: &Path) -> Result<(), CaptureFinalizerError> {
    let relative = reported.strip_prefix(root).map_err(|_| path_untrusted())?;
    if relative.as_os_str().is_empty() {
        return Err(path_untrusted());
    }
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        #[cfg(target_os = "windows")]
        if component.as_os_str().to_string_lossy().contains(':') {
            return Err(path_untrusted());
        }
        cursor.push(component.as_os_str());
        reject_reparse(&cursor)?;
    }
    Ok(())
}

fn reject_reparse(path: &Path) -> Result<(), CaptureFinalizerError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid_root())?;
    if is_reparse(&metadata) {
        return Err(error(
            "capture.staging_reparse_forbidden",
            "Capture staging cannot contain a symbolic link or reparse point.",
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

const fn invalid_root() -> CaptureFinalizerError {
    error(
        "capture.staging_root_invalid",
        "The host-owned capture staging root is unavailable.",
    )
}

const fn root_escape() -> CaptureFinalizerError {
    error(
        "capture.staging_root_escape",
        "The host-owned capture staging root escaped its app-local container.",
    )
}

const fn path_untrusted() -> CaptureFinalizerError {
    error(
        "capture.staged_path_untrusted",
        "The adapter returned a path outside the exact host-owned capture root.",
    )
}

const fn artifact_mismatch() -> CaptureFinalizerError {
    error(
        "capture.staged_payload_mismatch",
        "The staged capture payload does not match its exact measured receipt.",
    )
}

const fn context_error() -> CaptureFinalizerError {
    error(
        "capture.finalization_context_invalid",
        "The host capture genealogy or profile context is invalid.",
    )
}

const fn finalize_error() -> CaptureFinalizerError {
    error(
        "capture.finalize_failed",
        "The captured payload could not be packed and reopened as a valid cartridge.",
    )
}

const fn import_error() -> CaptureFinalizerError {
    error(
        "capture.import_failed",
        "The validated captured cartridge could not be imported into the Library.",
    )
}

const fn error(code: &'static str, message: &'static str) -> CaptureFinalizerError {
    CaptureFinalizerError { code, message }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use latentdeck_cartridge::{
        hash::hash_path,
        manifest::{
            CodecDescriptor, DType, DecodedVideoDescriptor, TensorDescriptor, TimingDescriptor,
        },
        reader::{ValidationOptions, open_integrity_validated},
    };
    use latentdeck_library::{CartridgeKey, DeckSourceIdentity, Library};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn binds_only_a_remeasured_regular_file_below_the_host_owned_root() {
        let app_data = tempdir().expect("app data");
        let capture_id = Uuid::new_v4();
        let binding = CaptureStagingRoot::create(app_data.path(), capture_id).expect("binding");
        let payload = binding.root().join("adapter.safetensors.partial");
        fs::write(&payload, b"trusted bytes").expect("payload");
        let digest = hash_path(&payload).expect("measure").sha256.to_string();

        let bound = binding
            .bind_artifact(
                payload.to_str().expect("UTF-8 test path"),
                &digest,
                b"trusted bytes".len() as u64,
                u64::MAX,
            )
            .expect("bound payload");

        assert_eq!(bound.path, payload);
        assert_eq!(bound.sha256, digest);
        assert_eq!(bound.byte_length, b"trusted bytes".len() as u64);
    }

    #[test]
    fn rejects_worker_path_escape_and_never_deletes_the_reported_file() {
        let app_data = tempdir().expect("app data");
        let outside = app_data.path().join("outside.safetensors.partial");
        fs::write(&outside, b"outside bytes").expect("outside");
        let digest = hash_path(&outside).expect("measure").sha256.to_string();
        let binding = CaptureStagingRoot::create(app_data.path(), Uuid::new_v4()).expect("binding");

        let error = binding
            .bind_artifact(
                outside.to_str().expect("UTF-8 test path"),
                &digest,
                b"outside bytes".len() as u64,
                u64::MAX,
            )
            .expect_err("outside path must be rejected");
        assert_eq!(error.code, "capture.staged_path_untrusted");
        drop(binding);
        assert_eq!(
            fs::read(&outside).expect("outside preserved"),
            b"outside bytes"
        );
    }

    #[test]
    fn rejects_declared_hash_or_length_and_cleans_only_the_owned_root() {
        let app_data = tempdir().expect("app data");
        let capture_id = Uuid::new_v4();
        let binding = CaptureStagingRoot::create(app_data.path(), capture_id).expect("binding");
        let root = binding.root().to_path_buf();
        let payload = root.join("payload.partial");
        fs::write(&payload, b"measured").expect("payload");

        let error = binding
            .bind_artifact(
                payload.to_str().expect("UTF-8 test path"),
                &"0".repeat(64),
                b"measured".len() as u64,
                u64::MAX,
            )
            .expect_err("forged digest must be rejected");
        assert_eq!(error.code, "capture.staged_payload_mismatch");
        drop(binding);
        assert!(
            !root.exists(),
            "only the UUID-owned staging root is removed"
        );
    }

    #[tokio::test]
    async fn finalizes_reopens_and_imports_a_codec_neutral_capture() {
        let app_data = tempdir().expect("app data");
        let capture_id = Uuid::new_v4();
        let binding = CaptureStagingRoot::create(app_data.path(), capture_id).expect("binding");
        let root = binding.root().to_path_buf();
        let payload = synthetic_non_h3_payload();
        let payload_path = root.join("synthetic.safetensors.partial");
        fs::write(&payload_path, &payload).expect("staged payload");
        let measured = hash_path(&payload_path).expect("payload measurement");
        let artifact = capture_artifact(capture_id, &payload_path, &measured);
        let context = capture_context(&payload);
        let output = app_data.path().join("captured.lc");
        let app_state =
            crate::library_state::AppState::new(Library::in_memory().expect("in-memory library"));

        let finalized = finalize_capture(
            binding,
            artifact,
            CaptureMode::Snapshot,
            1,
            context,
            output.clone(),
            app_state.importer(),
        )
        .await
        .expect("codec-neutral finalization");

        assert!(!root.exists(), "host staging is consumed and removed");
        let reopened = open_integrity_validated(&output, &ValidationOptions::default())
            .expect("final LC reopens generically");
        assert_eq!(reopened.manifest().codec.family.0, "synthetic_test");
        assert_eq!(reopened.manifest().timing.decoded_video.frame_count, 1);
        assert_eq!(reopened.manifest().parent_cartridges.len(), 1);
        assert_eq!(reopened.manifest().operation_history.len(), 1);
        assert_eq!(
            reopened.manifest().operation_history[0].controls["capture_reset_events"],
            json!(1)
        );
        let identity = DeckSourceIdentity::new(
            &finalized.cartridge_id,
            CartridgeKey::new_unchecked(finalized.archive_sha256),
        )
        .expect("final identity");
        let resolved = app_state
            .resolve_deck_source(identity)
            .await
            .expect("finalized LC is indexed through the generic Library path");
        assert_eq!(
            resolved.path(),
            output.canonicalize().expect("canonical output")
        );
    }

    #[tokio::test]
    async fn existing_output_is_never_clobbered_and_owned_staging_is_cleaned() {
        let app_data = tempdir().expect("app data");
        let capture_id = Uuid::new_v4();
        let binding = CaptureStagingRoot::create(app_data.path(), capture_id).expect("binding");
        let root = binding.root().to_path_buf();
        let payload = synthetic_non_h3_payload();
        let payload_path = root.join("synthetic.safetensors.partial");
        fs::write(&payload_path, &payload).expect("staged payload");
        let measured = hash_path(&payload_path).expect("payload measurement");
        let artifact = capture_artifact(capture_id, &payload_path, &measured);
        let output = app_data.path().join("already-exists.lc");
        fs::write(&output, b"owner sentinel").expect("existing output");
        let app_state =
            crate::library_state::AppState::new(Library::in_memory().expect("in-memory library"));

        let error = finalize_capture(
            binding,
            artifact,
            CaptureMode::LiveCapture,
            0,
            capture_context(&payload),
            output.clone(),
            app_state.importer(),
        )
        .await
        .expect_err("no-clobber output must fail");

        assert_eq!(error.code, "capture.finalize_failed");
        assert_eq!(
            fs::read(&output).expect("sentinel preserved"),
            b"owner sentinel"
        );
        assert!(!root.exists(), "only host-owned staging is cleaned");
    }

    fn capture_artifact(
        capture_id: Uuid,
        payload_path: &Path,
        measured: &latentdeck_cartridge::hash::MeasuredHash,
    ) -> CaptureArtifactEvidence {
        CaptureArtifactEvidence {
            capture_id,
            staged_payload_path: payload_path.to_string_lossy().into_owned(),
            payload_sha256: measured.sha256.to_string(),
            payload_byte_length: measured.byte_length,
            latent_slots: 1,
            decoded_frame_count: 1,
        }
    }

    fn capture_context(payload: &[u8]) -> CaptureFinalizationContext {
        CaptureFinalizationContext {
            sources: vec![CaptureSourceEvidence {
                physical_slot: 1,
                archive_sha256: "a".repeat(64),
                manifest: synthetic_non_h3_manifest(payload),
            }],
            roles: vec![RoleBinding {
                role: "carrier".to_owned(),
                physical_slot: 1,
            }],
            controls: vec![ControlBinding {
                name: "gain".to_owned(),
                value: ControlValue::Number(0.5),
            }],
            operator_id: "org.example.synthetic_deck".to_owned(),
            operator_version: "0.2.0".to_owned(),
            seed: 7,
        }
    }

    fn synthetic_non_h3_payload() -> Vec<u8> {
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
                .expect("header length")
                .to_le_bytes(),
        );
        payload.extend_from_slice(&header);
        payload.extend_from_slice(&tensor_bytes);
        payload
    }

    fn synthetic_non_h3_manifest(payload: &[u8]) -> ManifestV0_1 {
        let directory = tempdir().expect("measurement directory");
        let payload_path = directory.path().join("payload.safetensors");
        fs::write(&payload_path, payload).expect("measurement payload");
        let measured = hash_path(&payload_path).expect("payload hash");
        ManifestV0_1 {
            spec_version: SpecVersion(LC_SPEC_VERSION.to_owned()),
            cartridge_id: CartridgeId("550e8400-e29b-41d4-a716-446655440002".to_owned()),
            codec: CodecDescriptor {
                family: Identifier("synthetic_test".to_owned()),
                profile: Identifier("non_h3_latent".to_owned()),
                profile_version: SpecVersion("0.2.0".to_owned()),
            },
            payloads: vec![PayloadDescriptor {
                path: "payloads/synthetic.safetensors".to_owned(),
                media_type: "application/vnd.safetensors".to_owned(),
                byte_length: measured.byte_length,
                sha256: Sha256Digest(measured.sha256.to_string()),
            }],
            tensors: vec![TensorDescriptor {
                stream: TensorStream::Visual,
                name: Identifier("latent_state".to_owned()),
                payload: "payloads/synthetic.safetensors".to_owned(),
                storage_dtype: DType::F32,
                runtime_dtype: DType::F32,
                shape: vec![1, 7, 1, 3, 1],
            }],
            timing: TimingDescriptor {
                contract: Identifier("synthetic_step".to_owned()),
                contract_version: SpecVersion("0.2.0".to_owned()),
                decoded_video: DecodedVideoDescriptor {
                    width: 3,
                    height: 1,
                    frame_count: 1,
                    frame_rate: Rational {
                        numerator: 1,
                        denominator: 1,
                    },
                    duration: Rational {
                        numerator: 1,
                        denominator: 1,
                    },
                },
            },
            audio: AudioDisposition::SourceAbsent,
            preview: None,
            provenance: Provenance {
                created_by: ProducerDescriptor {
                    name: Identifier("latentdeck-capture-tests".to_owned()),
                    version: "0.2.0".to_owned(),
                },
                created_at: None,
                sources: Vec::new(),
            },
            parent_cartridges: Vec::new(),
            operation_history: Vec::new(),
        }
    }
}
