//! Bounded codec-neutral Raw→LC planning and queue state for `LatentPlayer`.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard},
};

use latentdeck_control::v2::{
    RawImportAudioPolicy, RawImportPreflight, RawImportStorageDtype, RawImportTensorStream,
};
use latentdeck_core::raw_import::RawImportExpectedAuthority;
use serde::Serialize;

use crate::raw_import_runtime::{
    RawImportProfileView, RawImportRuntimeError, RawImportSelectionRequest,
};

pub const MAX_CONVERSION_INPUTS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionPlanRequest {
    pub inputs: Vec<PathBuf>,
    pub output_directory: PathBuf,
    pub recursive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversionStatus {
    Ready,
    Converting,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversionPhase {
    Planned,
    Running,
    Stopping,
    Complete,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionSelection {
    pub package_id: String,
    pub package_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub profile: RawImportProfileView,
}

impl From<&RawImportSelectionRequest> for ConversionSelection {
    fn from(value: &RawImportSelectionRequest) -> Self {
        Self {
            package_id: value.package_id.clone(),
            package_version: value.package_version.clone(),
            adapter_id: value.adapter_id.clone(),
            adapter_version: value.adapter_version.clone(),
            profile: value.profile.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionMetadata {
    pub source_bytes: u64,
    pub source_sha256: String,
    pub storage_dtype: String,
    pub latent_slots: u64,
    pub latent_height: u64,
    pub latent_width: u64,
    pub decoded_width: u32,
    pub decoded_height: u32,
    pub decoded_frames: u64,
    pub audio_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionItem {
    pub source_name: String,
    pub relative_output: PathBuf,
    pub status: ConversionStatus,
    pub metadata: Option<ConversionMetadata>,
    pub error: Option<ConversionError>,
    pub archive_sha256: Option<String>,
    #[serde(skip)]
    source_path: PathBuf,
    #[serde(skip)]
    output_path: PathBuf,
    #[serde(skip)]
    expected: Option<RawImportExpectedAuthority>,
    #[serde(skip)]
    preflight: Option<RawImportPreflight>,
}

#[derive(Debug, Clone)]
pub struct ConversionPlan {
    pub items: Vec<ConversionItem>,
    pub selection: RawImportSelectionRequest,
    output_root: PathBuf,
}

impl ConversionPlan {
    pub fn accept_preflight(
        &mut self,
        index: usize,
        expected: RawImportExpectedAuthority,
        preflight: RawImportPreflight,
    ) -> Result<(), ConversionError> {
        expected
            .validate_preflight(&preflight)
            .map_err(ConversionError::from)?;
        let metadata = conversion_metadata(&preflight)?;
        let item = self.items.get_mut(index).ok_or_else(|| {
            ConversionError::new(
                "conversion.item_unavailable",
                "The raw import queue item is unavailable; prepare the batch again.",
            )
        })?;
        item.status = ConversionStatus::Ready;
        item.metadata = Some(metadata);
        item.error = None;
        item.expected = Some(expected);
        item.preflight = Some(preflight);
        Ok(())
    }

    pub fn reject_preflight(
        &mut self,
        index: usize,
        error: ConversionError,
    ) -> Result<(), ConversionError> {
        let item = self.items.get_mut(index).ok_or_else(|| {
            ConversionError::new(
                "conversion.item_unavailable",
                "The raw import queue item is unavailable; prepare the batch again.",
            )
        })?;
        item.status = ConversionStatus::Failed;
        item.metadata = None;
        item.error = Some(error);
        item.expected = None;
        item.preflight = None;
        Ok(())
    }

    #[must_use]
    pub fn source_path(&self, index: usize) -> Option<&Path> {
        self.items.get(index).map(|item| item.source_path.as_path())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionSnapshot {
    pub phase: ConversionPhase,
    pub selection: ConversionSelection,
    pub items: Vec<ConversionItem>,
    pub completed: usize,
    pub failed: usize,
    pub active_index: Option<usize>,
    pub stop_requested: bool,
}

#[derive(Debug)]
struct ConversionState {
    phase: ConversionPhase,
    items: Vec<ConversionItem>,
    active_index: Option<usize>,
    stop_requested: bool,
}

#[derive(Debug, Clone)]
pub struct ConversionWork {
    pub index: usize,
    pub source_path: PathBuf,
    pub output_path: PathBuf,
    pub output_root: PathBuf,
    pub expected: RawImportExpectedAuthority,
    pub planned_preflight: RawImportPreflight,
}

#[derive(Debug)]
pub struct ConversionCoordinator {
    state: Mutex<ConversionState>,
    idle: Condvar,
    selection: RawImportSelectionRequest,
    output_root: PathBuf,
}

impl ConversionCoordinator {
    #[must_use]
    pub fn from_plan(plan: ConversionPlan) -> Self {
        Self {
            state: Mutex::new(ConversionState {
                phase: ConversionPhase::Planned,
                items: plan.items,
                active_index: None,
                stop_requested: false,
            }),
            idle: Condvar::new(),
            selection: plan.selection,
            output_root: plan.output_root,
        }
    }

    #[must_use]
    pub fn selection(&self) -> &RawImportSelectionRequest {
        &self.selection
    }

    pub fn snapshot(&self) -> Result<ConversionSnapshot, ConversionError> {
        let state = self.lock_state()?;
        Ok(self.snapshot_from_state(&state))
    }

    pub(crate) fn begin(&self) -> Result<(), ConversionError> {
        let mut state = self.lock_state()?;
        if state.phase != ConversionPhase::Planned {
            return Err(ConversionError::new(
                "conversion.invalid_transition",
                "Prepare a new raw import batch before starting it.",
            ));
        }
        if !state
            .items
            .iter()
            .any(|item| item.status == ConversionStatus::Ready)
        {
            return Err(ConversionError::new(
                "conversion.no_ready_inputs",
                "No codec-preflighted raw files are ready to import.",
            ));
        }
        state.phase = ConversionPhase::Running;
        Ok(())
    }

    pub fn request_stop(&self) -> Result<ConversionSnapshot, ConversionError> {
        let mut state = self.lock_state()?;
        match state.phase {
            ConversionPhase::Planned => {
                cancel_ready(&mut state.items);
                state.phase = ConversionPhase::Stopped;
            }
            ConversionPhase::Running => {
                state.stop_requested = true;
                state.phase = ConversionPhase::Stopping;
            }
            ConversionPhase::Stopping => state.stop_requested = true,
            ConversionPhase::Complete | ConversionPhase::Stopped => {}
        }
        let snapshot = self.snapshot_from_state(&state);
        let terminal = matches!(
            state.phase,
            ConversionPhase::Complete | ConversionPhase::Stopped
        );
        drop(state);
        if terminal {
            self.idle.notify_all();
        }
        Ok(snapshot)
    }

    pub(crate) fn next_work(&self) -> Result<Option<ConversionWork>, ConversionError> {
        let mut state = self.lock_state()?;
        if !matches!(
            state.phase,
            ConversionPhase::Running | ConversionPhase::Stopping
        ) {
            return Err(ConversionError::new(
                "conversion.invalid_transition",
                "The raw import batch is not running.",
            ));
        }
        if state.stop_requested {
            cancel_ready(&mut state.items);
            state.active_index = None;
            state.phase = ConversionPhase::Stopped;
            drop(state);
            self.idle.notify_all();
            return Ok(None);
        }
        let Some(index) = state
            .items
            .iter()
            .position(|item| item.status == ConversionStatus::Ready)
        else {
            state.active_index = None;
            state.phase = ConversionPhase::Complete;
            drop(state);
            self.idle.notify_all();
            return Ok(None);
        };
        let item = &state.items[index];
        let expected = item.expected.clone().ok_or_else(|| {
            ConversionError::new(
                "conversion.preflight_identity_missing",
                "Validate this raw source with the selected codec again before importing it.",
            )
        })?;
        let planned_preflight = item.preflight.clone().ok_or_else(|| {
            ConversionError::new(
                "conversion.preflight_identity_missing",
                "Validate this raw source with the selected codec again before importing it.",
            )
        })?;
        let source_path = item.source_path.clone();
        let output_path = item.output_path.clone();
        state.items[index].status = ConversionStatus::Converting;
        state.active_index = Some(index);
        Ok(Some(ConversionWork {
            index,
            source_path,
            output_path,
            output_root: self.output_root.clone(),
            expected,
            planned_preflight,
        }))
    }

    pub(crate) fn settle(
        &self,
        index: usize,
        result: Result<String, ConversionError>,
    ) -> Result<(), ConversionError> {
        let mut state = self.lock_state()?;
        if state.active_index != Some(index) {
            return Err(ConversionError::new(
                "conversion.invalid_transition",
                "The raw import queue acknowledgement is out of order.",
            ));
        }
        let item = state.items.get_mut(index).ok_or_else(|| {
            ConversionError::new(
                "conversion.item_unavailable",
                "The raw import queue item is unavailable; prepare the batch again.",
            )
        })?;
        match result {
            Ok(archive_sha256) => {
                item.status = ConversionStatus::Complete;
                item.archive_sha256 = Some(archive_sha256);
                item.error = None;
            }
            Err(error) => {
                item.status = ConversionStatus::Failed;
                item.error = Some(error);
            }
        }
        state.active_index = None;
        Ok(())
    }

    pub(crate) fn fail_remaining(&self, error: &ConversionError) -> Result<(), ConversionError> {
        let mut state = self.lock_state()?;
        for item in &mut state.items {
            if matches!(
                item.status,
                ConversionStatus::Ready | ConversionStatus::Converting
            ) {
                item.status = ConversionStatus::Failed;
                item.error = Some(error.clone());
            }
        }
        state.active_index = None;
        state.phase = ConversionPhase::Complete;
        drop(state);
        self.idle.notify_all();
        Ok(())
    }

    pub(crate) fn wait_until_idle(&self) -> Result<ConversionSnapshot, ConversionError> {
        let mut state = self.lock_state()?;
        while matches!(
            state.phase,
            ConversionPhase::Running | ConversionPhase::Stopping
        ) {
            state = self.idle.wait(state).map_err(|_| {
                ConversionError::new(
                    "conversion.state_unavailable",
                    "Raw import state is unavailable; restart LatentPlayer.",
                )
            })?;
        }
        Ok(self.snapshot_from_state(&state))
    }

    pub fn completed_output(&self, index: usize) -> Result<PathBuf, ConversionError> {
        let state = self.lock_state()?;
        let item = state.items.get(index).ok_or_else(output_unavailable)?;
        if item.status != ConversionStatus::Complete {
            return Err(output_unavailable());
        }
        Ok(item.output_path.clone())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, ConversionState>, ConversionError> {
        self.state.lock().map_err(|_| {
            ConversionError::new(
                "conversion.state_unavailable",
                "Raw import state is unavailable; restart LatentPlayer.",
            )
        })
    }

    fn snapshot_from_state(&self, state: &ConversionState) -> ConversionSnapshot {
        ConversionSnapshot {
            phase: state.phase,
            selection: (&self.selection).into(),
            items: state.items.clone(),
            completed: state
                .items
                .iter()
                .filter(|item| item.status == ConversionStatus::Complete)
                .count(),
            failed: state
                .items
                .iter()
                .filter(|item| item.status == ConversionStatus::Failed)
                .count(),
            active_index: state.active_index,
            stop_requested: state.stop_requested,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

impl ConversionError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            recoverable: true,
        }
    }
}

impl From<RawImportRuntimeError> for ConversionError {
    fn from(error: RawImportRuntimeError) -> Self {
        Self::new(error.code(), error.user_message())
    }
}

impl From<latentdeck_core::raw_import::RawImportError> for ConversionError {
    fn from(error: latentdeck_core::raw_import::RawImportError) -> Self {
        let code = error.stable_code();
        let message = match code {
            "raw_import.authority_mismatch" | "raw_import.receipt_mismatch" => {
                "The adapter receipt did not match the exact selected package, profile, or source bytes."
            }
            "raw_import.source_untrusted" => {
                "The raw source changed, is linked, or exceeds the bounded import contract."
            }
            "raw_import.staging_root_unavailable" => {
                "LatentPlayer could not create its host-owned raw import staging directory."
            }
            _ => "Core rejected the adapter-staged raw import.",
        };
        Self::new(code, message)
    }
}

pub fn plan_conversion_inventory(
    request: ConversionPlanRequest,
    selection: RawImportSelectionRequest,
) -> Result<ConversionPlan, ConversionError> {
    let ConversionPlanRequest {
        inputs,
        output_directory,
        recursive,
    } = request;
    validate_output_directory(&output_directory)?;
    if inputs.is_empty() || inputs.len() > MAX_CONVERSION_INPUTS {
        return Err(ConversionError::new(
            "conversion.invalid_inputs",
            format!("Choose between one and {MAX_CONVERSION_INPUTS} raw files or folders."),
        ));
    }
    let output_root = output_directory
        .canonicalize()
        .map_err(|_| invalid_output_directory())?;
    let mut inventory = inventory_inputs(inputs, &output_root, recursive)?;
    inventory.sort_by_key(|(_, relative)| relative.to_string_lossy().to_lowercase());
    validate_inventory_outputs(&inventory, &output_root)?;
    let items = inventory
        .into_iter()
        .map(|(source_path, relative_source)| {
            let source_name = source_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("raw-input")
                .to_owned();
            let relative_output = relative_source.with_extension("lc");
            let output_path = output_root.join(&relative_output);
            ConversionItem {
                source_name,
                relative_output,
                status: ConversionStatus::Ready,
                metadata: None,
                error: None,
                archive_sha256: None,
                source_path,
                output_path,
                expected: None,
                preflight: None,
            }
        })
        .collect();
    Ok(ConversionPlan {
        items,
        selection,
        output_root,
    })
}

fn inventory_inputs(
    inputs: Vec<PathBuf>,
    output_root: &Path,
    recursive: bool,
) -> Result<Vec<(PathBuf, PathBuf)>, ConversionError> {
    let mut inventory = Vec::new();
    for provided in inputs {
        let metadata = fs::symlink_metadata(&provided).map_err(|_| {
            ConversionError::new(
                "conversion.invalid_input",
                "A selected raw input does not exist or is not readable.",
            )
        })?;
        if metadata_is_reparse(&metadata) {
            return Err(ConversionError::new(
                "conversion.invalid_input",
                "Linked or reparse-point raw inputs are not accepted.",
            ));
        }
        if metadata.is_file() {
            let relative = PathBuf::from(
                provided
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("raw-input"),
            );
            inventory.push((provided, relative));
        } else if metadata.is_dir() {
            let root = provided
                .canonicalize()
                .map_err(|_| invalid_input_directory())?;
            if output_root.starts_with(&root) {
                return Err(ConversionError::new(
                    "conversion.output_inside_input",
                    "Choose an output folder outside every selected raw input folder.",
                ));
            }
            collect_directory_inputs(&root, &root, recursive, &mut inventory)?;
        } else {
            return Err(ConversionError::new(
                "conversion.invalid_input",
                "A selected raw input must be a regular local file or directory.",
            ));
        }
        if inventory.len() > MAX_CONVERSION_INPUTS {
            return Err(input_limit_exceeded());
        }
    }
    if inventory.is_empty() {
        return Err(ConversionError::new(
            "conversion.no_inputs",
            "The selected folder contains no regular raw files.",
        ));
    }
    Ok(inventory)
}

fn validate_inventory_outputs(
    inventory: &[(PathBuf, PathBuf)],
    output_root: &Path,
) -> Result<(), ConversionError> {
    let mut outputs = HashSet::with_capacity(inventory.len());
    for (_, relative) in inventory {
        let output = relative.with_extension("lc");
        let key = output.to_string_lossy().replace('\\', "/").to_lowercase();
        if !outputs.insert(key) {
            return Err(ConversionError::new(
                "conversion.output_collision",
                "Multiple selected raw files resolve to the same LC output name.",
            ));
        }
    }
    if inventory
        .iter()
        .any(|(_, relative)| output_root.join(relative.with_extension("lc")).exists())
    {
        return Err(ConversionError::new(
            "conversion.output_exists",
            "One or more LC outputs already exist. Choose another output folder or remove the conflict.",
        ));
    }
    Ok(())
}

fn conversion_metadata(
    preflight: &RawImportPreflight,
) -> Result<ConversionMetadata, ConversionError> {
    let visual = preflight
        .metadata
        .tensors
        .as_slice()
        .iter()
        .find(|tensor| tensor.stream == RawImportTensorStream::Visual)
        .ok_or_else(metadata_invalid)?;
    let [batch, _channels, latent_slots, latent_height, latent_width]: [u64; 5] = visual
        .shape
        .as_slice()
        .try_into()
        .map_err(|_| metadata_invalid())?;
    if batch != 1 || latent_slots == 0 || latent_height == 0 || latent_width == 0 {
        return Err(metadata_invalid());
    }
    Ok(ConversionMetadata {
        source_bytes: preflight.source_byte_length,
        source_sha256: preflight.source_sha256.clone(),
        storage_dtype: match visual.storage_dtype {
            RawImportStorageDtype::F16 => "F16",
            RawImportStorageDtype::F32 => "F32",
        }
        .to_owned(),
        latent_slots,
        latent_height,
        latent_width,
        decoded_width: preflight.metadata.decoded_width,
        decoded_height: preflight.metadata.decoded_height,
        decoded_frames: preflight.metadata.decoded_frame_count,
        audio_present: preflight.metadata.audio_policy == RawImportAudioPolicy::PreservedSource,
    })
}

fn collect_directory_inputs(
    root: &Path,
    current: &Path,
    recursive: bool,
    inventory: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), ConversionError> {
    let entries = fs::read_dir(current).map_err(|_| invalid_input_directory())?;
    for entry in entries {
        let entry = entry.map_err(|_| invalid_input_directory())?;
        let file_type = entry.file_type().map_err(|_| invalid_input_directory())?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| invalid_input_directory())?;
        if metadata_is_reparse(&metadata) {
            continue;
        }
        if file_type.is_dir() {
            if recursive {
                collect_directory_inputs(root, &path, true, inventory)?;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if inventory.len() >= MAX_CONVERSION_INPUTS {
            return Err(input_limit_exceeded());
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            ConversionError::new(
                "conversion.input_outside_root",
                "A discovered raw input escaped the selected folder.",
            )
        })?;
        inventory.push((path.clone(), relative.to_path_buf()));
    }
    Ok(())
}

fn validate_output_directory(path: &Path) -> Result<(), ConversionError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid_output_directory())?;
    if !path.is_absolute() || !metadata.is_dir() || metadata_is_reparse(&metadata) {
        return Err(invalid_output_directory());
    }
    Ok(())
}

fn cancel_ready(items: &mut [ConversionItem]) {
    for item in items {
        if item.status == ConversionStatus::Ready {
            item.status = ConversionStatus::Cancelled;
        }
    }
}

fn output_unavailable() -> ConversionError {
    ConversionError::new(
        "conversion.output_unavailable",
        "Choose a completed imported item to open in Player.",
    )
}

fn invalid_output_directory() -> ConversionError {
    ConversionError::new(
        "conversion.output_directory_invalid",
        "Choose an existing local non-linked output folder.",
    )
}

fn invalid_input_directory() -> ConversionError {
    ConversionError::new(
        "conversion.input_read_failed",
        "The selected raw input folder could not be read safely.",
    )
}

fn input_limit_exceeded() -> ConversionError {
    ConversionError::new(
        "conversion.input_limit_exceeded",
        format!("A raw import batch may contain at most {MAX_CONVERSION_INPUTS} files."),
    )
}

fn metadata_invalid() -> ConversionError {
    ConversionError::new(
        "raw_import.metadata_invalid",
        "The adapter returned an invalid visual tensor layout for LC authoring.",
    )
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use latentdeck_control::v2::{LimitedVec, ProfileKey, RawImportMetadata, RawImportTensor};
    use uuid::Uuid;

    use super::*;

    fn selection() -> RawImportSelectionRequest {
        RawImportSelectionRequest {
            package_id: "org.example.codec".to_owned(),
            package_version: "0.2.0".to_owned(),
            adapter_id: "org.example.codec.adapter".to_owned(),
            adapter_version: "0.2.0".to_owned(),
            profile: RawImportProfileView {
                codec_family: "example_codec".to_owned(),
                profile: "example_latent".to_owned(),
                profile_version: "0.1.0".to_owned(),
            },
        }
    }

    #[cfg(windows)]
    #[test]
    fn nonverbatim_output_selection_plans_inside_the_same_canonical_root() {
        let directory = tempfile::tempdir().expect("temp");
        let source = directory.path().join("clip.raw");
        let selected_output = directory.path().join("prepared");
        fs::write(&source, b"bounded raw bytes").expect("source");
        fs::create_dir(&selected_output).expect("output");
        assert!(
            !selected_output.to_string_lossy().starts_with(r"\\?\"),
            "the UI-style selected path must begin in the non-verbatim namespace"
        );

        let plan = plan_conversion_inventory(
            ConversionPlanRequest {
                inputs: vec![source],
                output_directory: selected_output.clone(),
                recursive: false,
            },
            selection(),
        )
        .expect("inventory");

        assert_eq!(
            plan.output_root,
            selected_output.canonicalize().expect("canonical output")
        );
        assert_ne!(plan.output_root, selected_output);
        assert_eq!(plan.items[0].relative_output, PathBuf::from("clip.lc"));
        assert_eq!(
            plan.items[0].output_path,
            plan.output_root.join(&plan.items[0].relative_output),
            "the private output and root must use the same Windows path namespace"
        );
    }

    #[test]
    fn canonical_output_authority_rejects_an_existing_relative_output() {
        let directory = tempfile::tempdir().expect("temp");
        let source = directory.path().join("clip.raw");
        let selected_output = directory.path().join("prepared");
        fs::write(&source, b"bounded raw bytes").expect("source");
        fs::create_dir(&selected_output).expect("output");
        fs::write(selected_output.join("clip.lc"), b"existing cartridge").expect("existing output");

        let error = plan_conversion_inventory(
            ConversionPlanRequest {
                inputs: vec![source],
                output_directory: selected_output,
                recursive: false,
            },
            selection(),
        )
        .expect_err("planning never overwrites an existing relative output");

        assert_eq!(error.code, "conversion.output_exists");
    }

    fn preflight(source: &Path, expected: &RawImportExpectedAuthority) -> RawImportPreflight {
        let measured = latentdeck_cartridge::hash::hash_path(source).expect("source hash");
        let value = RawImportPreflight {
            receipt_id: Uuid::new_v4(),
            import_id: Uuid::new_v4(),
            pack_id: selection().package_id,
            pack_version: selection().package_version,
            adapter_id: selection().adapter_id,
            adapter_version: selection().adapter_version,
            source_sha256: measured.sha256.to_string(),
            source_byte_length: measured.byte_length,
            metadata: RawImportMetadata {
                profile_key: ProfileKey::from(&selection().profile),
                payload_entry: "payloads/example.safetensors".to_owned(),
                payload_media_type: "application/vnd.safetensors".to_owned(),
                tensors: LimitedVec::try_from_vec(vec![RawImportTensor {
                    stream: RawImportTensorStream::Visual,
                    name: "video".to_owned(),
                    storage_dtype: RawImportStorageDtype::F16,
                    runtime_dtype: RawImportStorageDtype::F16,
                    shape: LimitedVec::try_from_vec(vec![1, 24, 2, 1, 1]).expect("shape"),
                }])
                .expect("tensors"),
                timing_contract: "example_timing".to_owned(),
                timing_contract_version: "0.1.0".to_owned(),
                decoded_width: 16,
                decoded_height: 16,
                decoded_frame_count: 5,
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
                duration_numerator: 5,
                duration_denominator: 24,
                audio_policy: RawImportAudioPolicy::SourceAbsent,
            },
        };
        expected.validate_preflight(&value).expect("host authority");
        value
    }

    #[test]
    fn serialized_plan_is_path_free_and_binds_exact_codec_adapter_and_profile() {
        let directory = tempfile::tempdir().expect("temp");
        let source = directory.path().join("clip.raw");
        let output = directory.path().join("prepared");
        fs::write(&source, b"bounded raw bytes").expect("source");
        fs::create_dir(&output).expect("output");
        let mut plan = plan_conversion_inventory(
            ConversionPlanRequest {
                inputs: vec![source.clone()],
                output_directory: output,
                recursive: false,
            },
            selection(),
        )
        .expect("inventory");
        let expected = RawImportExpectedAuthority::measure_source(
            "org.example.codec",
            "0.2.0",
            "org.example.codec.adapter",
            "0.2.0",
            &source,
            ProfileKey::from(&selection().profile),
        )
        .expect("authority");
        let receipt = preflight(&source, &expected);
        plan.accept_preflight(0, expected, receipt)
            .expect("accepted preflight");
        let coordinator = ConversionCoordinator::from_plan(plan);

        let json =
            serde_json::to_string(&coordinator.snapshot().expect("snapshot")).expect("serialize");

        assert!(json.contains("org.example.codec.adapter"));
        assert!(json.contains("example_latent"));
        assert!(!json.contains(directory.path().to_string_lossy().as_ref()));
        assert!(!json.contains("sourcePath"));
        assert!(!json.contains("outputPath"));
    }

    #[test]
    fn malicious_preflight_identity_is_rejected_before_metadata_reaches_snapshot() {
        let directory = tempfile::tempdir().expect("temp");
        let source = directory.path().join("clip.raw");
        let output = directory.path().join("prepared");
        fs::write(&source, b"bounded raw bytes").expect("source");
        fs::create_dir(&output).expect("output");
        let mut plan = plan_conversion_inventory(
            ConversionPlanRequest {
                inputs: vec![source.clone()],
                output_directory: output,
                recursive: false,
            },
            selection(),
        )
        .expect("inventory");
        let expected = RawImportExpectedAuthority::measure_source(
            "org.example.codec",
            "0.2.0",
            "org.example.codec.adapter",
            "0.2.0",
            &source,
            ProfileKey::from(&selection().profile),
        )
        .expect("authority");
        let mut malicious = preflight(&source, &expected);
        malicious.adapter_version = "9.9.9".to_owned();

        let error = plan
            .accept_preflight(0, expected, malicious)
            .expect_err("malicious identity");

        assert_eq!(error.code, "raw_import.authority_mismatch");
        assert!(plan.items[0].metadata.is_none());
    }
}
