//! Bounded raw-H3 conversion planning and execution for `LatentPlayer`.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard},
};

use latentdeck_cartridge::{
    authoring::{RawH3AuthoringOptions, inspect_raw_h3, pack_raw_h3_atomic},
    hash::Sha256Hash,
    safetensor::SafetensorDType,
};
use serde::Serialize;

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
pub struct ConversionMetadata {
    pub payload_bytes: u64,
    pub payload_sha256: String,
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
    expected_payload_sha256: Option<Sha256Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionPlan {
    pub items: Vec<ConversionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionSnapshot {
    pub phase: ConversionPhase,
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

#[derive(Debug)]
pub struct ConversionCoordinator {
    state: Mutex<ConversionState>,
    idle: Condvar,
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
        }
    }

    pub fn snapshot(&self) -> Result<ConversionSnapshot, ConversionError> {
        let state = self.lock_state()?;
        Ok(snapshot_from_state(&state))
    }

    #[cfg(test)]
    #[allow(dead_code)] // Integration contracts call this; the binary test target does not.
    pub fn run_to_completion(&self) -> Result<ConversionSnapshot, ConversionError> {
        self.run_to_completion_with(convert_one)
    }

    pub(crate) fn begin(&self) -> Result<(), ConversionError> {
        let mut state = self.lock_state()?;
        if state.phase != ConversionPhase::Planned {
            return Err(ConversionError::new(
                "conversion.invalid_transition",
                "Prepare a new conversion before starting it.",
            ));
        }
        if !state
            .items
            .iter()
            .any(|item| item.status == ConversionStatus::Ready)
        {
            return Err(ConversionError::new(
                "conversion.no_ready_inputs",
                "No preflighted raw H3 files are ready to convert.",
            ));
        }
        state.phase = ConversionPhase::Running;
        Ok(())
    }

    #[allow(dead_code)] // The path-included integration contract has no Tauri command layer.
    pub(crate) fn finish_started(&self) -> Result<ConversionSnapshot, ConversionError> {
        self.run_started_with(convert_one)
    }

    pub fn request_stop(&self) -> Result<ConversionSnapshot, ConversionError> {
        let mut state = self.lock_state()?;
        match state.phase {
            ConversionPhase::Planned => {
                for item in &mut state.items {
                    if item.status == ConversionStatus::Ready {
                        item.status = ConversionStatus::Cancelled;
                    }
                }
                state.phase = ConversionPhase::Stopped;
            }
            ConversionPhase::Running => {
                state.stop_requested = true;
                state.phase = ConversionPhase::Stopping;
            }
            ConversionPhase::Stopping => {
                state.stop_requested = true;
            }
            ConversionPhase::Complete | ConversionPhase::Stopped => {}
        }
        let snapshot = snapshot_from_state(&state);
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

    pub(crate) fn wait_until_idle(&self) -> Result<ConversionSnapshot, ConversionError> {
        let mut state = self.lock_state()?;
        while matches!(
            state.phase,
            ConversionPhase::Running | ConversionPhase::Stopping
        ) {
            state = self.idle.wait(state).map_err(|_| {
                ConversionError::new(
                    "conversion.state_unavailable",
                    "Conversion state is unavailable; restart LatentPlayer.",
                )
            })?;
        }
        Ok(snapshot_from_state(&state))
    }

    pub fn completed_output(&self, index: usize) -> Result<PathBuf, ConversionError> {
        let state = self.lock_state()?;
        let item = state.items.get(index).ok_or_else(|| {
            ConversionError::new(
                "conversion.output_unavailable",
                "Choose a completed converted item to open in Player.",
            )
        })?;
        if item.status != ConversionStatus::Complete {
            return Err(ConversionError::new(
                "conversion.output_unavailable",
                "Choose a completed converted item to open in Player.",
            ));
        }
        Ok(item.output_path.clone())
    }

    #[cfg(test)]
    fn run_to_completion_with<F>(&self, converter: F) -> Result<ConversionSnapshot, ConversionError>
    where
        F: FnMut(&Path, &Path, Option<Sha256Hash>) -> Result<String, ConversionError>,
    {
        self.begin()?;
        self.run_started_with(converter)
    }

    fn run_started_with<F>(&self, mut converter: F) -> Result<ConversionSnapshot, ConversionError>
    where
        F: FnMut(&Path, &Path, Option<Sha256Hash>) -> Result<String, ConversionError>,
    {
        loop {
            let work = {
                let mut state = self.lock_state()?;
                if state.stop_requested {
                    for item in &mut state.items {
                        if item.status == ConversionStatus::Ready {
                            item.status = ConversionStatus::Cancelled;
                        }
                    }
                    state.active_index = None;
                    state.phase = ConversionPhase::Stopped;
                    let snapshot = snapshot_from_state(&state);
                    drop(state);
                    self.idle.notify_all();
                    return Ok(snapshot);
                }
                let Some(index) = state
                    .items
                    .iter()
                    .position(|item| item.status == ConversionStatus::Ready)
                else {
                    state.active_index = None;
                    state.phase = ConversionPhase::Complete;
                    let snapshot = snapshot_from_state(&state);
                    drop(state);
                    self.idle.notify_all();
                    return Ok(snapshot);
                };
                state.active_index = Some(index);
                state.items[index].status = ConversionStatus::Converting;
                (
                    index,
                    state.items[index].source_path.clone(),
                    state.items[index].output_path.clone(),
                    state.items[index].expected_payload_sha256,
                )
            };

            let result = converter(&work.1, &work.2, work.3);
            let mut state = self.lock_state()?;
            state.active_index = None;
            match result {
                Ok(archive_sha256) => {
                    state.items[work.0].status = ConversionStatus::Complete;
                    state.items[work.0].archive_sha256 = Some(archive_sha256);
                }
                Err(error) => {
                    state.items[work.0].status = ConversionStatus::Failed;
                    state.items[work.0].error = Some(error);
                }
            }
        }
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, ConversionState>, ConversionError> {
        self.state.lock().map_err(|_| {
            ConversionError::new(
                "conversion.state_unavailable",
                "Conversion state is unavailable; restart LatentPlayer.",
            )
        })
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
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            recoverable: true,
        }
    }
}

pub fn plan_conversion(request: ConversionPlanRequest) -> Result<ConversionPlan, ConversionError> {
    validate_output_directory(&request.output_directory)?;
    if request.inputs.is_empty() || request.inputs.len() > MAX_CONVERSION_INPUTS {
        return Err(ConversionError::new(
            "conversion.invalid_inputs",
            format!("Choose between one and {MAX_CONVERSION_INPUTS} raw H3 files or folders."),
        ));
    }
    let mut inventory = Vec::new();
    for provided in request.inputs {
        if provided.is_file() {
            if !has_safetensors_extension(&provided) {
                return Err(ConversionError::new(
                    "conversion.invalid_input",
                    "Every selected input file must use the .safetensors extension.",
                ));
            }
            let relative = PathBuf::from(
                provided
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("raw-h3.safetensors"),
            );
            inventory.push((provided, relative));
        } else if provided.is_dir() {
            collect_directory_inputs(&provided, &provided, request.recursive, &mut inventory)?;
        } else {
            return Err(ConversionError::new(
                "conversion.invalid_input",
                "A selected raw H3 input does not exist or is not readable.",
            ));
        }
        if inventory.len() > MAX_CONVERSION_INPUTS {
            return Err(ConversionError::new(
                "conversion.input_limit_exceeded",
                format!("A conversion may contain at most {MAX_CONVERSION_INPUTS} files."),
            ));
        }
    }
    if inventory.is_empty() {
        return Err(ConversionError::new(
            "conversion.no_inputs",
            "The selected folder contains no .safetensors files.",
        ));
    }
    inventory.sort_by_key(|(_, relative)| relative.to_string_lossy().to_lowercase());
    let mut outputs = HashSet::with_capacity(inventory.len());
    for (_, relative) in &inventory {
        let output = relative.with_extension("lc");
        let key = output.to_string_lossy().replace('\\', "/").to_lowercase();
        if !outputs.insert(key) {
            return Err(ConversionError::new(
                "conversion.output_collision",
                "Multiple selected raw H3 files resolve to the same LC output name.",
            ));
        }
    }
    if inventory.iter().any(|(_, relative)| {
        request
            .output_directory
            .join(relative.with_extension("lc"))
            .exists()
    }) {
        return Err(ConversionError::new(
            "conversion.output_exists",
            "One or more LC outputs already exist. Choose another output folder or remove the conflict.",
        ));
    }
    let items = inventory
        .into_iter()
        .map(|(source, relative)| preflight_item(source, &relative, &request.output_directory))
        .collect();
    Ok(ConversionPlan { items })
}

fn preflight_item(
    source: PathBuf,
    relative_source: &Path,
    output_directory: &Path,
) -> ConversionItem {
    let source_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("raw-h3.safetensors")
        .to_owned();
    let relative_output = relative_source.with_extension("lc");
    let output_path = output_directory.join(&relative_output);
    match inspect_raw_h3(&source) {
        Ok(inspection) => {
            let expected_payload_sha256 = inspection.payload_sha256;
            ConversionItem {
                source_name,
                relative_output,
                status: ConversionStatus::Ready,
                metadata: Some(ConversionMetadata {
                    payload_bytes: inspection.payload_bytes,
                    payload_sha256: inspection.payload_sha256.to_string(),
                    storage_dtype: dtype_name(inspection.safetensors.video.dtype).to_owned(),
                    latent_slots: inspection.profile.visual.latent_slots,
                    latent_height: inspection.profile.visual.latent_height,
                    latent_width: inspection.profile.visual.latent_width,
                    decoded_width: inspection.profile.visual.decoded_width,
                    decoded_height: inspection.profile.visual.decoded_height,
                    decoded_frames: inspection.profile.visual.decoded_frame_count,
                    audio_present: inspection.profile.audio.is_some(),
                }),
                error: None,
                archive_sha256: None,
                source_path: source,
                output_path,
                expected_payload_sha256: Some(expected_payload_sha256),
            }
        }
        Err(error) => ConversionItem {
            source_name,
            relative_output,
            status: ConversionStatus::Failed,
            metadata: None,
            error: Some(ConversionError {
                code: error.code().to_owned(),
                message: error.detail,
                recoverable: true,
            }),
            archive_sha256: None,
            source_path: source,
            output_path,
            expected_payload_sha256: None,
        },
    }
}

fn snapshot_from_state(state: &ConversionState) -> ConversionSnapshot {
    ConversionSnapshot {
        phase: state.phase,
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

fn convert_one(
    source: &Path,
    output: &Path,
    expected_payload_sha256: Option<Sha256Hash>,
) -> Result<String, ConversionError> {
    let expected_payload_sha256 = expected_payload_sha256.ok_or_else(|| {
        ConversionError::new(
            "conversion.preflight_identity_missing",
            "Validate this raw H3 source again before converting it.",
        )
    })?;
    let parent = output.parent().ok_or_else(|| {
        ConversionError::new(
            "conversion.output_directory_invalid",
            "The prepared output location is invalid.",
        )
    })?;
    fs::create_dir_all(parent).map_err(|_| {
        ConversionError::new(
            "conversion.output_create_failed",
            "LatentPlayer could not create a prepared output folder.",
        )
    })?;
    let receipt = pack_raw_h3_atomic(
        source,
        output,
        &RawH3AuthoringOptions::new("latentplayer", env!("CARGO_PKG_VERSION"))
            .with_expected_payload_sha256(expected_payload_sha256),
    )
    .map_err(|error| ConversionError {
        code: error.code().to_owned(),
        message: error.detail,
        recoverable: true,
    })?;
    Ok(receipt.validation.archive_sha256.to_string())
}

fn collect_directory_inputs(
    root: &Path,
    current: &Path,
    recursive: bool,
    inventory: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), ConversionError> {
    let entries = fs::read_dir(current).map_err(|_| {
        ConversionError::new(
            "conversion.input_read_failed",
            "The selected raw H3 folder could not be read.",
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|_| {
            ConversionError::new(
                "conversion.input_read_failed",
                "An entry in the selected raw H3 folder could not be read.",
            )
        })?;
        let file_type = entry.file_type().map_err(|_| {
            ConversionError::new(
                "conversion.input_read_failed",
                "An input type in the selected raw H3 folder could not be inspected.",
            )
        })?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if recursive {
                collect_directory_inputs(root, &path, true, inventory)?;
            }
            continue;
        }
        if !file_type.is_file() || !has_safetensors_extension(&path) {
            continue;
        }
        if inventory.len() >= MAX_CONVERSION_INPUTS {
            return Err(ConversionError::new(
                "conversion.input_limit_exceeded",
                format!("A conversion may contain at most {MAX_CONVERSION_INPUTS} files."),
            ));
        }
        let relative = path.strip_prefix(root).map_err(|_| {
            ConversionError::new(
                "conversion.input_outside_root",
                "A discovered input escaped the selected folder.",
            )
        })?;
        inventory.push((path.clone(), relative.to_path_buf()));
    }
    Ok(())
}

fn validate_output_directory(path: &Path) -> Result<(), ConversionError> {
    if !path.is_absolute() || !path.is_dir() {
        return Err(ConversionError::new(
            "conversion.output_directory_invalid",
            "Choose an existing local output folder.",
        ));
    }
    Ok(())
}

fn has_safetensors_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("safetensors"))
}

const fn dtype_name(dtype: SafetensorDType) -> &'static str {
    match dtype {
        SafetensorDType::F16 => "F16",
        SafetensorDType::F32 => "F32",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    use super::*;

    fn pending_item(name: &str) -> ConversionItem {
        ConversionItem {
            source_name: format!("{name}.safetensors"),
            relative_output: PathBuf::from(format!("{name}.lc")),
            status: ConversionStatus::Ready,
            metadata: None,
            error: None,
            archive_sha256: None,
            source_path: PathBuf::from(format!("{name}.safetensors")),
            output_path: PathBuf::from(format!("{name}.lc")),
            expected_payload_sha256: None,
        }
    }

    #[test]
    fn stop_request_finishes_the_current_file_and_cancels_the_remaining_queue() {
        let coordinator = Arc::new(ConversionCoordinator::from_plan(ConversionPlan {
            items: vec![pending_item("current"), pending_item("queued")],
        }));
        let worker_coordinator = Arc::clone(&coordinator);
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let worker = thread::spawn(move || {
            worker_coordinator.run_to_completion_with(|_, _, _| {
                started_tx.send(()).expect("announce current file");
                release_rx.recv().expect("finish current file");
                Ok("a".repeat(64))
            })
        });
        started_rx.recv().expect("current file started");

        let stopping = coordinator.request_stop().expect("stop request");

        assert_eq!(stopping.phase, ConversionPhase::Stopping);
        assert!(stopping.stop_requested);
        assert_eq!(stopping.items[0].status, ConversionStatus::Converting);
        assert_eq!(stopping.items[1].status, ConversionStatus::Ready);
        release_tx.send(()).expect("release current file");
        let stopped = worker
            .join()
            .expect("worker thread")
            .expect("stopped batch");
        assert_eq!(stopped.phase, ConversionPhase::Stopped);
        assert_eq!(stopped.items[0].status, ConversionStatus::Complete);
        assert_eq!(stopped.items[1].status, ConversionStatus::Cancelled);
    }

    #[test]
    fn idle_wait_does_not_finish_until_the_current_atomic_write_finishes() {
        let coordinator = Arc::new(ConversionCoordinator::from_plan(ConversionPlan {
            items: vec![pending_item("current")],
        }));
        let worker_coordinator = Arc::clone(&coordinator);
        let (started_tx, started_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::sync_channel(0);
        let worker = thread::spawn(move || {
            worker_coordinator.run_to_completion_with(|_, _, _| {
                started_tx.send(()).expect("announce current file");
                release_rx.recv().expect("finish current file");
                Ok("a".repeat(64))
            })
        });
        started_rx.recv().expect("current file started");
        coordinator.request_stop().expect("stop request");
        let waiting_coordinator = Arc::clone(&coordinator);
        let (idle_tx, idle_rx) = mpsc::sync_channel(0);
        let waiter = thread::spawn(move || {
            idle_tx
                .send(waiting_coordinator.wait_until_idle())
                .expect("idle snapshot");
        });

        assert!(matches!(
            idle_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_tx.send(()).expect("release current file");
        let idle = idle_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("idle result")
            .expect("idle snapshot");
        assert_eq!(idle.phase, ConversionPhase::Stopped);
        worker
            .join()
            .expect("worker thread")
            .expect("stopped batch");
        waiter.join().expect("waiter thread");
    }
}
