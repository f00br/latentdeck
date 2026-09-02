//! Exact Codec Pack v2 selection and CPU-only Raw→LC Protocol 2 runtime.

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use latentdeck_control::v2::{
    Ack, Capability, CodecDescriptor, CodecDescriptorRequest, Command, LimitedVec,
    MAX_CAPABILITIES, MAX_FRAME_BYTES, MAX_RAW_IMPORT_SOURCE_BYTES, PROTOCOL_VERSION, ProfileKey,
    RawImportAbort, RawImportArtifact, RawImportPreflight, RawImportPreflightRequest,
    RawImportStage, SessionConfigure, ShutdownReason,
};
use latentdeck_core::{
    diagnostics::{LogLevel, record_global},
    raw_import::{
        RawImportAuthoring, RawImportError, RawImportExpectedAuthority, RawImportFinalizeRequest,
        RawImportStagingRoot, finalize_raw_import_atomic,
    },
    worker_client_v2::{WorkerClientV2, WorkerClientV2Error},
    worker_supervisor::{ValidatedWorkerLaunch, WorkerSupervisorError, spawn_worker_v2},
};
use latentdeck_extension_manager::{
    ActiveInstalledPackage, ActivePackageCache, CodecCapability, CodecPackManifest, ExtensionError,
    ExtensionRoots, PackageKind, PackageManifest, PackageReference,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::conversion::{
    ConversionCoordinator, ConversionError, ConversionPlan, ConversionPlanRequest,
    plan_conversion_inventory,
};

const CODEC_HOST_API_VERSION: &str = "2.0";
const RAW_IMPORT_COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const RAW_IMPORT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawImportProfileView {
    pub codec_family: String,
    pub profile: String,
    pub profile_version: String,
}

impl From<&ProfileKey> for RawImportProfileView {
    fn from(value: &ProfileKey) -> Self {
        Self {
            codec_family: value.codec_family.clone(),
            profile: value.profile.clone(),
            profile_version: value.profile_version.clone(),
        }
    }
}

impl From<&latentdeck_extension_manager::ProfileKey> for RawImportProfileView {
    fn from(value: &latentdeck_extension_manager::ProfileKey) -> Self {
        Self {
            codec_family: value.codec_family.clone(),
            profile: value.profile.clone(),
            profile_version: value.profile_version.clone(),
        }
    }
}

impl From<&RawImportProfileView> for ProfileKey {
    fn from(value: &RawImportProfileView) -> Self {
        Self {
            codec_family: value.codec_family.clone(),
            profile: value.profile.clone(),
            profile_version: value.profile_version.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawImportSelectionRequest {
    pub package_id: String,
    pub package_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub profile: RawImportProfileView,
}

impl RawImportSelectionRequest {
    fn package_reference(&self) -> PackageReference {
        PackageReference {
            kind: PackageKind::CodecPack,
            package_id: self.package_id.clone(),
            package_version: self.package_version.clone(),
        }
    }

    fn profile_key(&self) -> ProfileKey {
        (&self.profile).into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawImportCodecOptions {
    pub package_id: String,
    pub package_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
    pub display_name: String,
    pub profiles: Vec<RawImportProfileView>,
}

#[derive(Debug, Error)]
pub enum RawImportRuntimeError {
    #[error("no exact Codec Pack is selected for raw import")]
    SelectionMissing,
    #[error("the raw import request differs from the exact selected Codec Pack")]
    SelectionMismatch,
    #[error("the exact selected Codec Pack is not active and trusted")]
    PackageUnavailable,
    #[error("the exact selected package is not a compatible Codec Pack v2")]
    PackageInvalid,
    #[error("the exact selected Codec Pack does not provide raw import")]
    UnsupportedCapability,
    #[error("the exact selected Codec Pack does not provide the selected profile")]
    UnsupportedProfile,
    #[error("the exact selected Codec Pack does not support this LatentPlayer version")]
    UnsupportedAppVersion,
    #[error(
        "the worker reply does not match host-selected package, adapter, profile, or source authority"
    )]
    AuthorityMismatch,
    #[error("the raw import Protocol 2 lifecycle returned an unexpected acknowledgement")]
    UnexpectedReply,
    #[error("the raw import worker failed")]
    Worker,
    #[error("the adapter rejected this raw source during bounded CPU preflight")]
    SourceInvalid,
    #[error("the host-owned raw import staging root is unavailable")]
    StagingUnavailable,
    #[error("Core could not finalize the staged raw import")]
    Finalize,
}

impl RawImportRuntimeError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SelectionMissing => "raw_import.selection_missing",
            Self::SelectionMismatch => "raw_import.selection_mismatch",
            Self::PackageUnavailable => "raw_import.package_unavailable",
            Self::PackageInvalid => "raw_import.package_invalid",
            Self::UnsupportedCapability => "raw_import.unsupported_capability",
            Self::UnsupportedProfile => "raw_import.unsupported_profile",
            Self::UnsupportedAppVersion => "raw_import.unsupported_app_version",
            Self::AuthorityMismatch => "raw_import.authority_mismatch",
            Self::UnexpectedReply => "raw_import.protocol_invalid",
            Self::Worker => "raw_import.worker_failed",
            Self::SourceInvalid => "raw_import.source_invalid",
            Self::StagingUnavailable => "raw_import.staging_root_unavailable",
            Self::Finalize => "raw_import.finalize_failed",
        }
    }

    #[must_use]
    pub const fn user_message(&self) -> &'static str {
        match self {
            Self::SelectionMissing => {
                "Select an exact raw-import-capable Codec Pack version in Extensions first."
            }
            Self::SelectionMismatch => {
                "The requested package or adapter is not the exact version selected in Extensions."
            }
            Self::PackageUnavailable => {
                "The exact Codec Pack version is disabled, untrusted, missing, or corrupt."
            }
            Self::PackageInvalid => {
                "The exact selected package is not a compatible Protocol 2 Codec Pack."
            }
            Self::UnsupportedCapability => {
                "The exact selected Codec Pack does not declare the optional raw import capability."
            }
            Self::UnsupportedProfile => {
                "The exact selected Codec Pack does not declare the requested raw import profile."
            }
            Self::UnsupportedAppVersion => {
                "The exact selected Codec Pack does not support this LatentPlayer version."
            }
            Self::AuthorityMismatch => {
                "The adapter receipt did not match the exact selected package, profile, or source bytes."
            }
            Self::UnexpectedReply => {
                "The selected Codec Pack violated the closed Raw Import Protocol 2 lifecycle."
            }
            Self::Worker => "The isolated raw import worker stopped or rejected the operation.",
            Self::SourceInvalid => {
                "The selected codec adapter rejected this raw source during bounded CPU preflight."
            }
            Self::StagingUnavailable => {
                "LatentPlayer could not create its host-owned raw import staging directory."
            }
            Self::Finalize => {
                "Core rejected the staged payload or could not commit a validated no-clobber cartridge."
            }
        }
    }
}

impl From<ExtensionError> for RawImportRuntimeError {
    fn from(_: ExtensionError) -> Self {
        Self::PackageUnavailable
    }
}

impl From<WorkerSupervisorError> for RawImportRuntimeError {
    fn from(_: WorkerSupervisorError) -> Self {
        Self::Worker
    }
}

impl From<WorkerClientV2Error> for RawImportRuntimeError {
    fn from(error: WorkerClientV2Error) -> Self {
        match error {
            WorkerClientV2Error::Remote(remote)
                if !remote.fatal
                    && matches!(
                        remote.code,
                        latentdeck_control::v2::ErrorCode::SourceInvalid
                            | latentdeck_control::v2::ErrorCode::ProfileInvalid
                            | latentdeck_control::v2::ErrorCode::ProfileIncompatible
                    ) =>
            {
                Self::SourceInvalid
            }
            WorkerClientV2Error::Remote(remote)
                if !remote.fatal
                    && remote.code
                        == latentdeck_control::v2::ErrorCode::CodecCapabilityUnsupported =>
            {
                Self::UnsupportedCapability
            }
            _ => Self::Worker,
        }
    }
}

impl From<RawImportError> for RawImportRuntimeError {
    fn from(error: RawImportError) -> Self {
        match error {
            RawImportError::AuthorityMismatch | RawImportError::ReceiptMismatch => {
                Self::AuthorityMismatch
            }
            RawImportError::StagingRootUnavailable | RawImportError::StagingCreate(_) => {
                Self::StagingUnavailable
            }
            RawImportError::StagedPathUntrusted
            | RawImportError::RawSourceUntrusted
            | RawImportError::MetadataInvalid
            | RawImportError::Protocol(_)
            | RawImportError::Cartridge(_) => Self::Finalize,
        }
    }
}

pub struct PreparedRawImportCodec {
    package: ActiveInstalledPackage,
    selection: RawImportSelectionRequest,
    heartbeat_interval_ms: u32,
    heartbeat_hard_timeout_ms: u32,
    app_version: String,
}

pub fn raw_import_options_for(
    cache: &ActivePackageCache,
    roots: &ExtensionRoots,
    selected_package: Option<&PackageReference>,
    app_version: &str,
) -> Result<RawImportCodecOptions, RawImportRuntimeError> {
    let reference = selected_package.ok_or(RawImportRuntimeError::SelectionMissing)?;
    let package = cache.resolve_active(roots, reference)?;
    let manifest = codec_manifest(&package)?;
    validate_manifest_base(manifest, app_version)?;
    if !manifest.capabilities.contains(&CodecCapability::RawImport) {
        return Err(RawImportRuntimeError::UnsupportedCapability);
    }
    Ok(RawImportCodecOptions {
        package_id: manifest.pack_id.clone(),
        package_version: manifest.pack_version.clone(),
        adapter_id: manifest.adapter.adapter_id.clone(),
        adapter_version: manifest.adapter.adapter_version.clone(),
        display_name: manifest.display_name.clone(),
        profiles: manifest
            .compatibility
            .profiles
            .iter()
            .map(Into::into)
            .collect(),
    })
}

pub fn prepare_exact_raw_import(
    cache: &ActivePackageCache,
    roots: &ExtensionRoots,
    request: RawImportSelectionRequest,
    selected_package: Option<&PackageReference>,
    app_version: &str,
) -> Result<PreparedRawImportCodec, RawImportRuntimeError> {
    let selected = selected_package.ok_or(RawImportRuntimeError::SelectionMissing)?;
    if selected != &request.package_reference() {
        return Err(RawImportRuntimeError::SelectionMismatch);
    }
    let package = cache.resolve_active(roots, selected)?;
    let manifest = codec_manifest(&package)?;
    validate_manifest_base(manifest, app_version)?;
    validate_exact_request(manifest, &request)?;
    let (heartbeat_interval_ms, heartbeat_hard_timeout_ms) =
        negotiated_raw_import_heartbeat(manifest.worker.heartbeat_timeout_ms);
    Ok(PreparedRawImportCodec {
        package,
        selection: request,
        heartbeat_interval_ms,
        heartbeat_hard_timeout_ms,
        app_version: app_version.to_owned(),
    })
}

fn codec_manifest(
    package: &ActiveInstalledPackage,
) -> Result<&CodecPackManifest, RawImportRuntimeError> {
    let PackageManifest::Codec(manifest) = package.manifest() else {
        return Err(RawImportRuntimeError::PackageInvalid);
    };
    Ok(manifest)
}

fn validate_manifest_base(
    manifest: &CodecPackManifest,
    app_version: &str,
) -> Result<(), RawImportRuntimeError> {
    if manifest.manifest_version != "2.0.0"
        || manifest.kind != PackageKind::CodecPack
        || manifest.compatibility.worker_protocol != PROTOCOL_VERSION
        || manifest.compatibility.codec_adapter_api != 1
    {
        return Err(RawImportRuntimeError::PackageInvalid);
    }
    let current =
        Version::parse(app_version).map_err(|_| RawImportRuntimeError::UnsupportedAppVersion)?;
    let minimum = Version::parse(&manifest.compatibility.app_min_inclusive)
        .map_err(|_| RawImportRuntimeError::PackageInvalid)?;
    let maximum = Version::parse(&manifest.compatibility.app_max_exclusive)
        .map_err(|_| RawImportRuntimeError::PackageInvalid)?;
    if current < minimum || current >= maximum {
        return Err(RawImportRuntimeError::UnsupportedAppVersion);
    }
    Ok(())
}

fn validate_exact_request(
    manifest: &CodecPackManifest,
    request: &RawImportSelectionRequest,
) -> Result<(), RawImportRuntimeError> {
    if manifest.pack_id != request.package_id
        || manifest.pack_version != request.package_version
        || manifest.adapter.adapter_id != request.adapter_id
        || manifest.adapter.adapter_version != request.adapter_version
    {
        return Err(RawImportRuntimeError::SelectionMismatch);
    }
    if !manifest.capabilities.contains(&CodecCapability::RawImport) {
        return Err(RawImportRuntimeError::UnsupportedCapability);
    }
    if !manifest.compatibility.profiles.iter().any(|profile| {
        profile.codec_family == request.profile.codec_family
            && profile.profile == request.profile.profile
            && profile.profile_version == request.profile.profile_version
    }) {
        return Err(RawImportRuntimeError::UnsupportedProfile);
    }
    Ok(())
}

pub struct RawImportWorkerSession {
    client: WorkerClientV2,
    _package: ActiveInstalledPackage,
    selection: RawImportSelectionRequest,
}

/// Run adapter CPU preflight for every inventory item under one exact package
/// lease and authenticated Raw Import Protocol 2 worker.
pub async fn preflight_conversion_plan(
    request: ConversionPlanRequest,
    selection: RawImportSelectionRequest,
    prepared: PreparedRawImportCodec,
) -> Result<ConversionPlan, ConversionError> {
    let mut plan = plan_conversion_inventory(request, selection)?;
    let mut session = RawImportWorkerSession::start(prepared)
        .await
        .map_err(ConversionError::from)?;
    for index in 0..plan.items.len() {
        let source = plan.source_path(index).ok_or_else(|| {
            ConversionError::new(
                "conversion.item_unavailable",
                "The raw import queue item is unavailable; prepare the batch again.",
            )
        })?;
        let inspected = match session.measure_source(source) {
            Ok(expected) => match session.preflight(source, &expected).await {
                Ok(preflight) => Ok((expected, preflight)),
                Err(RawImportRuntimeError::SourceInvalid) => {
                    Err(ConversionError::from(RawImportRuntimeError::SourceInvalid))
                }
                Err(error) => {
                    session.shutdown(ShutdownReason::ProtocolFault).await;
                    return Err(error.into());
                }
            },
            Err(error) => Err(error.into()),
        };
        match inspected {
            Ok((expected, preflight)) => {
                let accepted = plan.accept_preflight(index, expected, preflight.clone());
                let aborted = session.abort(&preflight).await;
                if let Err(error) = accepted {
                    session.shutdown(ShutdownReason::ProtocolFault).await;
                    return Err(error);
                }
                if let Err(error) = aborted {
                    session.shutdown(ShutdownReason::ProtocolFault).await;
                    return Err(error.into());
                }
            }
            Err(error) => plan.reject_preflight(index, error)?,
        }
    }
    session.shutdown(ShutdownReason::UserRequest).await;
    Ok(plan)
}

/// Convert the already preflighted queue sequentially. Each item is measured
/// and preflighted again before staging, so no receipt or source authority is
/// inherited across worker processes or source mutations.
pub async fn run_conversion_batch(
    coordinator: Arc<ConversionCoordinator>,
    prepared: PreparedRawImportCodec,
    staging_parent: PathBuf,
) -> Result<crate::conversion::ConversionSnapshot, ConversionError> {
    let staging_ready = fs::create_dir_all(&staging_parent)
        .and_then(|()| fs::symlink_metadata(&staging_parent))
        .is_ok_and(|metadata| metadata.is_dir() && !metadata_is_reparse(&metadata));
    if !staging_ready {
        let error = ConversionError::from(RawImportRuntimeError::StagingUnavailable);
        coordinator.fail_remaining(&error)?;
        return coordinator.snapshot();
    }
    let mut session = match RawImportWorkerSession::start(prepared).await {
        Ok(session) => session,
        Err(error) => {
            let conversion_error = ConversionError::from(error);
            coordinator.fail_remaining(&conversion_error)?;
            return coordinator.snapshot();
        }
    };
    loop {
        let Some(work) = coordinator.next_work()? else {
            session.shutdown(ShutdownReason::UserRequest).await;
            return coordinator.snapshot();
        };
        let imported = import_one(&mut session, &work, &staging_parent).await;
        let cleanup_fault = imported
            .as_ref()
            .ok()
            .and_then(|success| success.cleanup_fault.clone());
        let result = imported.map(|success| success.archive_sha256);
        let fatal = cleanup_fault.or_else(|| {
            result
                .as_ref()
                .err()
                .filter(|error| raw_import_session_is_invalid(error))
                .cloned()
        });
        coordinator.settle(work.index, result)?;
        if let Some(fatal) = fatal {
            record_global(
                LogLevel::Warn,
                "raw_import.session_invalid",
                Some(fatal.code.as_str()),
            );
            let error = ConversionError::new(
                fatal.code,
                "The Raw Import Protocol 2 session became invalid; remaining queue items were not started.",
            );
            coordinator.fail_remaining(&error)?;
            session.shutdown(ShutdownReason::ProtocolFault).await;
            return coordinator.snapshot();
        }
    }
}

struct RawImportSuccess {
    archive_sha256: String,
    cleanup_fault: Option<ConversionError>,
}

fn preserve_committed_success(
    archive_sha256: String,
    cleanup: Result<(), RawImportRuntimeError>,
) -> RawImportSuccess {
    RawImportSuccess {
        archive_sha256,
        cleanup_fault: cleanup.err().map(ConversionError::from),
    }
}

async fn import_one(
    session: &mut RawImportWorkerSession,
    work: &crate::conversion::ConversionWork,
    staging_parent: &Path,
) -> Result<RawImportSuccess, ConversionError> {
    let current = session
        .measure_source(&work.source_path)
        .map_err(ConversionError::from)?;
    if current != work.expected {
        return Err(ConversionError::new(
            "raw_import.source_changed",
            "The raw source changed after preflight; validate the batch again.",
        ));
    }
    let preflight = session
        .preflight(&work.source_path, &work.expected)
        .await
        .map_err(ConversionError::from)?;
    if !same_preflight_authority_and_metadata(&preflight, &work.planned_preflight) {
        if let Err(error) = session.abort(&preflight).await {
            return Err(error.into());
        }
        return Err(ConversionError::new(
            "raw_import.preflight_changed",
            "The codec preflight result changed after validation; prepare the batch again.",
        ));
    }
    let staging = RawImportStagingRoot::create_in(staging_parent).map_err(ConversionError::from)?;
    let artifact = match session.stage(&preflight, &staging).await {
        Ok(artifact) => artifact,
        Err(error) => {
            if let Err(abort_error) = session.abort(&preflight).await {
                return Err(abort_error.into());
            }
            return Err(error.into());
        }
    };
    if work
        .expected
        .validate_source_unchanged(&work.source_path)
        .is_err()
    {
        if let Err(error) = session.abort(&preflight).await {
            return Err(error.into());
        }
        return Err(ConversionError::new(
            "raw_import.source_changed",
            "The raw source changed while the codec staged it; no cartridge was committed.",
        ));
    }
    prepare_output_parent(&work.output_root, &work.output_path)?;
    let finalized = finalize_raw_import_atomic(
        &staging,
        &RawImportFinalizeRequest {
            expected: work.expected.clone(),
            preflight: preflight.clone(),
            artifact,
            authoring: RawImportAuthoring::new("latentplayer", env!("CARGO_PKG_VERSION")),
        },
        &work.output_path,
    )
    .map_err(ConversionError::from);
    let cleanup = session.abort(&preflight).await;
    match finalized {
        Ok(receipt) => Ok(preserve_committed_success(
            receipt.validation.archive_sha256.to_string(),
            cleanup,
        )),
        Err(error) => {
            if let Err(cleanup_error) = cleanup {
                return Err(cleanup_error.into());
            }
            Err(error)
        }
    }
}

fn raw_import_session_is_invalid(error: &ConversionError) -> bool {
    matches!(
        error.code.as_str(),
        "raw_import.worker_failed"
            | "raw_import.protocol_invalid"
            | "raw_import.authority_mismatch"
            | "raw_import.receipt_mismatch"
            | "raw_import.staged_path_untrusted"
            | "payload_hash_mismatch"
            | "raw_import.unsupported_capability"
            | "raw_import.unsupported_profile"
            | "raw_import.package_invalid"
            | "raw_import.preflight_changed"
    )
}

fn same_preflight_authority_and_metadata(
    current: &RawImportPreflight,
    planned: &RawImportPreflight,
) -> bool {
    current.pack_id == planned.pack_id
        && current.pack_version == planned.pack_version
        && current.adapter_id == planned.adapter_id
        && current.adapter_version == planned.adapter_version
        && current.source_sha256 == planned.source_sha256
        && current.source_byte_length == planned.source_byte_length
        && current.metadata == planned.metadata
}

fn prepare_output_parent(root: &Path, output: &Path) -> Result<(), ConversionError> {
    let root_metadata = fs::symlink_metadata(root).map_err(|_| {
        ConversionError::new(
            "conversion.output_directory_invalid",
            "The selected output folder is no longer available.",
        )
    })?;
    if !root_metadata.is_dir() || metadata_is_reparse(&root_metadata) {
        return Err(ConversionError::new(
            "conversion.output_directory_invalid",
            "The selected output folder is no longer a regular local directory.",
        ));
    }
    let canonical_root = root.canonicalize().map_err(|_| {
        ConversionError::new(
            "conversion.output_directory_invalid",
            "The selected output folder is no longer available.",
        )
    })?;
    let parent = output.parent().ok_or_else(|| {
        ConversionError::new(
            "conversion.output_directory_invalid",
            "The prepared output location is invalid.",
        )
    })?;
    let relative = parent.strip_prefix(root).map_err(|_| {
        ConversionError::new(
            "conversion.output_outside_root",
            "The prepared output escaped the selected output folder.",
        )
    })?;
    let mut current = canonical_root.clone();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(ConversionError::new(
                "conversion.output_outside_root",
                "The prepared output contains an unsafe path component.",
            ));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata_is_reparse(&metadata) => {}
            Ok(_) => {
                return Err(ConversionError::new(
                    "conversion.output_directory_invalid",
                    "A prepared output directory is linked or not a directory.",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(|_| {
                    ConversionError::new(
                        "conversion.output_create_failed",
                        "LatentPlayer could not create a prepared output folder.",
                    )
                })?;
                let metadata = fs::symlink_metadata(&current).map_err(|_| {
                    ConversionError::new(
                        "conversion.output_create_failed",
                        "LatentPlayer could not verify a prepared output folder.",
                    )
                })?;
                if !metadata.is_dir() || metadata_is_reparse(&metadata) {
                    return Err(ConversionError::new(
                        "conversion.output_directory_invalid",
                        "A prepared output directory is linked or not a directory.",
                    ));
                }
            }
            Err(_) => {
                return Err(ConversionError::new(
                    "conversion.output_create_failed",
                    "LatentPlayer could not inspect a prepared output folder.",
                ));
            }
        }
    }
    let canonical_parent = parent.canonicalize().map_err(|_| {
        ConversionError::new(
            "conversion.output_directory_invalid",
            "LatentPlayer could not verify the prepared output folder.",
        )
    })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(ConversionError::new(
            "conversion.output_outside_root",
            "The prepared output escaped the selected output folder.",
        ));
    }
    Ok(())
}

impl RawImportWorkerSession {
    pub async fn start(prepared: PreparedRawImportCodec) -> Result<Self, RawImportRuntimeError> {
        let launch = ValidatedWorkerLaunch::from_installed_codec_v2(&prepared.package)?;
        let worker = spawn_worker_v2(launch).await?.connect().await?;
        let mut client = WorkerClientV2::new(worker);
        let requested_capabilities =
            LimitedVec::<Capability, MAX_CAPABILITIES>::try_from_vec(vec![Capability::RawImport])
                .map_err(|_| RawImportRuntimeError::PackageInvalid)?;
        let ack = client
            .call(
                Command::SessionConfigure(SessionConfigure {
                    selected_protocol_version: PROTOCOL_VERSION,
                    app_version: prepared.app_version,
                    heartbeat_interval_ms: prepared.heartbeat_interval_ms,
                    heartbeat_hard_timeout_ms: prepared.heartbeat_hard_timeout_ms,
                    max_frame_bytes: u32::try_from(MAX_FRAME_BYTES)
                        .map_err(|_| RawImportRuntimeError::PackageInvalid)?,
                    max_inflight_batches: 1,
                    requested_capabilities,
                }),
                RAW_IMPORT_COMMAND_TIMEOUT,
            )
            .await?;
        let Ack::SessionConfigure(configured) = ack else {
            return Err(RawImportRuntimeError::UnexpectedReply);
        };
        if configured.selected_protocol_version != PROTOCOL_VERSION
            || usize::try_from(configured.maximum_frame_bytes).ok() != Some(MAX_FRAME_BYTES)
            || !raw_import_session_capabilities_are_exact(
                configured.accepted_capabilities.as_slice(),
            )
        {
            return Err(RawImportRuntimeError::UnsupportedCapability);
        }
        let ack = client
            .call(
                Command::CodecDescriptor(CodecDescriptorRequest {
                    pack_id: prepared.selection.package_id.clone(),
                    pack_version: prepared.selection.package_version.clone(),
                    adapter_id: prepared.selection.adapter_id.clone(),
                }),
                RAW_IMPORT_COMMAND_TIMEOUT,
            )
            .await?;
        let Ack::CodecDescriptor(descriptor) = ack else {
            return Err(RawImportRuntimeError::UnexpectedReply);
        };
        validate_descriptor(&descriptor, &prepared.selection)?;
        Ok(Self {
            client,
            _package: prepared.package,
            selection: prepared.selection,
        })
    }

    pub fn measure_source(
        &self,
        source: &Path,
    ) -> Result<RawImportExpectedAuthority, RawImportRuntimeError> {
        RawImportExpectedAuthority::measure_source(
            self.selection.package_id.clone(),
            self.selection.package_version.clone(),
            self.selection.adapter_id.clone(),
            self.selection.adapter_version.clone(),
            source,
            self.selection.profile_key(),
        )
        .map_err(Into::into)
    }

    pub async fn preflight(
        &mut self,
        source: &Path,
        expected: &RawImportExpectedAuthority,
    ) -> Result<RawImportPreflight, RawImportRuntimeError> {
        let source_path = source
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or(RawImportRuntimeError::AuthorityMismatch)?;
        let import_id = Uuid::new_v4();
        let ack = self
            .client
            .call(
                Command::RawImportPreflight(RawImportPreflightRequest {
                    import_id,
                    source_path: source_path.to_owned(),
                    maximum_source_bytes: MAX_RAW_IMPORT_SOURCE_BYTES,
                }),
                RAW_IMPORT_COMMAND_TIMEOUT,
            )
            .await?;
        let Ack::RawImportPreflight(preflight) = ack else {
            return Err(RawImportRuntimeError::UnexpectedReply);
        };
        if preflight.import_id != import_id {
            return Err(RawImportRuntimeError::AuthorityMismatch);
        }
        expected.validate_preflight(&preflight)?;
        Ok(*preflight)
    }

    pub async fn stage(
        &mut self,
        preflight: &RawImportPreflight,
        staging: &RawImportStagingRoot,
    ) -> Result<RawImportArtifact, RawImportRuntimeError> {
        let staging_root = staging
            .path()
            .to_str()
            .filter(|value| !value.is_empty())
            .ok_or(RawImportRuntimeError::StagingUnavailable)?;
        let ack = self
            .client
            .call(
                Command::RawImportStage(RawImportStage {
                    import_id: preflight.import_id,
                    receipt_id: preflight.receipt_id,
                    staging_root: staging_root.to_owned(),
                }),
                RAW_IMPORT_COMMAND_TIMEOUT,
            )
            .await?;
        let Ack::RawImportStage(artifact) = ack else {
            return Err(RawImportRuntimeError::UnexpectedReply);
        };
        if artifact.import_id != preflight.import_id || artifact.receipt_id != preflight.receipt_id
        {
            return Err(RawImportRuntimeError::AuthorityMismatch);
        }
        Ok(artifact)
    }

    pub async fn abort(
        &mut self,
        preflight: &RawImportPreflight,
    ) -> Result<(), RawImportRuntimeError> {
        let ack = self
            .client
            .call(
                Command::RawImportAbort(RawImportAbort {
                    import_id: preflight.import_id,
                    receipt_id: preflight.receipt_id,
                }),
                RAW_IMPORT_COMMAND_TIMEOUT,
            )
            .await?;
        let Ack::RawImportAbort(aborted) = ack else {
            return Err(RawImportRuntimeError::UnexpectedReply);
        };
        if aborted.import_id != preflight.import_id || aborted.receipt_id != preflight.receipt_id {
            return Err(RawImportRuntimeError::AuthorityMismatch);
        }
        Ok(())
    }

    pub async fn shutdown(&mut self, reason: ShutdownReason) {
        let _ = self
            .client
            .request_shutdown(reason, RAW_IMPORT_SHUTDOWN_TIMEOUT)
            .await;
    }
}

fn validate_descriptor(
    descriptor: &CodecDescriptor,
    selection: &RawImportSelectionRequest,
) -> Result<(), RawImportRuntimeError> {
    if descriptor.pack_id != selection.package_id
        || descriptor.pack_version != selection.package_version
        || descriptor.adapter_id != selection.adapter_id
        || descriptor.adapter_version != selection.adapter_version
    {
        return Err(RawImportRuntimeError::AuthorityMismatch);
    }
    if descriptor.host_api_version != CODEC_HOST_API_VERSION {
        return Err(RawImportRuntimeError::PackageInvalid);
    }
    let mut required = Capability::REQUIRED_CODEC_V2.to_vec();
    required.push(Capability::RawImport);
    if !contains_unique_capabilities(descriptor.capabilities.as_slice(), &required) {
        return Err(RawImportRuntimeError::UnsupportedCapability);
    }
    if !descriptor
        .profiles
        .as_slice()
        .contains(&selection.profile_key())
    {
        return Err(RawImportRuntimeError::UnsupportedProfile);
    }
    Ok(())
}

fn contains_unique_capabilities(actual: &[Capability], required: &[Capability]) -> bool {
    let values: HashSet<_> = actual.iter().copied().collect();
    values.len() == actual.len()
        && required
            .iter()
            .all(|capability| values.contains(capability))
}

fn raw_import_session_capabilities_are_exact(actual: &[Capability]) -> bool {
    actual == [Capability::RawImport]
}

fn negotiated_raw_import_heartbeat(declared_hard_timeout_ms: u32) -> (u32, u32) {
    const MINIMUM_INTERVAL_MS: u32 = 250;
    const MAXIMUM_INTERVAL_MS: u32 = 60_000;
    const MINIMUM_HARD_TIMEOUT_MS: u32 = MINIMUM_INTERVAL_MS * 3;

    let hard_timeout_ms = declared_hard_timeout_ms.max(MINIMUM_HARD_TIMEOUT_MS);
    let interval_ms = (hard_timeout_ms / 4).clamp(MINIMUM_INTERVAL_MS, MAXIMUM_INTERVAL_MS);
    (interval_ms, hard_timeout_ms)
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
    use super::*;
    use latentdeck_extension_manager::{
        Architecture, CodecAdapterDescriptor, CodecCompatibility, CodecWorkerDescriptor,
        IntegrityDescriptor, LicenseDescriptor, OperatingSystem, PlatformDescriptor,
        PublisherDescriptor, PublisherIdentityClaim, PythonConstraint, PythonImplementation,
        RuntimeLockDescriptor,
    };

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

    fn descriptor(capabilities: Vec<Capability>) -> CodecDescriptor {
        CodecDescriptor {
            pack_id: "org.example.codec".to_owned(),
            pack_version: "0.2.0".to_owned(),
            adapter_id: "org.example.codec.adapter".to_owned(),
            adapter_version: "0.2.0".to_owned(),
            host_api_version: CODEC_HOST_API_VERSION.to_owned(),
            capabilities: LimitedVec::try_from_vec(capabilities).expect("capabilities"),
            profiles: LimitedVec::try_from_vec(vec![selection().profile_key()]).expect("profiles"),
        }
    }

    fn manifest_without_optional_raw_import() -> CodecPackManifest {
        CodecPackManifest {
            manifest_version: "2.0.0".to_owned(),
            kind: PackageKind::CodecPack,
            pack_id: "org.example.codec".to_owned(),
            pack_version: "0.2.0".to_owned(),
            display_name: "Example Codec".to_owned(),
            summary: "Synthetic test codec".to_owned(),
            publisher: PublisherDescriptor {
                name: "Test Publisher".to_owned(),
                url: None,
                identity_claim: PublisherIdentityClaim::SelfDeclared,
            },
            license: LicenseDescriptor {
                spdx_or_label: "Test-only".to_owned(),
                notice_path: "LICENSE.txt".to_owned(),
            },
            platform: PlatformDescriptor {
                os: OperatingSystem::Windows,
                arch: Architecture::X86_64,
            },
            compatibility: CodecCompatibility {
                app_min_inclusive: "0.1.0".to_owned(),
                app_max_exclusive: "1.0.0".to_owned(),
                worker_protocol: PROTOCOL_VERSION,
                codec_adapter_api: 1,
                tensor_abi: "latentdeck.tensor.v1".to_owned(),
                python: PythonConstraint {
                    implementation: PythonImplementation::Cpython,
                    version: "3.13".to_owned(),
                    platform_tag: "win_amd64".to_owned(),
                },
                torch_exact_build: "test".to_owned(),
                lc_spec_versions: vec!["0.1.0".to_owned()],
                profiles: vec![latentdeck_extension_manager::ProfileKey {
                    codec_family: "example_codec".to_owned(),
                    profile: "example_latent".to_owned(),
                    profile_version: "0.1.0".to_owned(),
                }],
            },
            adapter: CodecAdapterDescriptor {
                adapter_id: "org.example.codec.adapter".to_owned(),
                adapter_version: "0.2.0".to_owned(),
                entrypoint: "example.adapter:create".to_owned(),
            },
            worker: CodecWorkerDescriptor {
                executable: "runtime/python.exe".to_owned(),
                arguments: Vec::new(),
                working_directory: "runtime".to_owned(),
                start_timeout_ms: 1_000,
                heartbeat_timeout_ms: 1_000,
            },
            capabilities: vec![
                CodecCapability::Player,
                CodecCapability::Realtime,
                CodecCapability::Resample,
                CodecCapability::SnapshotCapture,
                CodecCapability::LiveCapture,
            ],
            external_assets: Vec::new(),
            runtime_lock: RuntimeLockDescriptor {
                path: "runtime/runtime.lock".to_owned(),
                sha256: "0".repeat(64),
            },
            integrity: IntegrityDescriptor {
                catalog_path: "integrity.json".to_owned(),
                catalog_sha256: "0".repeat(64),
            },
        }
    }

    #[test]
    fn descriptor_identity_mismatch_is_rejected_before_raw_metadata_is_trusted() {
        let mut malicious = descriptor(
            Capability::REQUIRED_CODEC_V2
                .into_iter()
                .chain([Capability::RawImport])
                .collect(),
        );
        malicious.adapter_version = "9.9.9".to_owned();

        let error = validate_descriptor(&malicious, &selection()).expect_err("identity mismatch");

        assert_eq!(error.code(), "raw_import.authority_mismatch");
    }

    #[test]
    fn descriptor_without_optional_raw_import_capability_is_explicitly_unsupported() {
        let no_raw = descriptor(Capability::REQUIRED_CODEC_V2.to_vec());

        let error = validate_descriptor(&no_raw, &selection()).expect_err("missing raw import");

        assert_eq!(error.code(), "raw_import.unsupported_capability");
    }

    #[test]
    fn raw_import_session_rejects_unrequested_capability_escalation() {
        assert!(raw_import_session_capabilities_are_exact(&[
            Capability::RawImport
        ]));
        assert!(!raw_import_session_capabilities_are_exact(&[
            Capability::RawImport,
            Capability::Player,
        ]));
    }

    #[test]
    fn heartbeat_negotiation_stays_inside_the_closed_protocol_bounds() {
        assert_eq!(negotiated_raw_import_heartbeat(100), (250, 750));
        assert_eq!(negotiated_raw_import_heartbeat(1_000), (250, 1_000));
        assert_eq!(negotiated_raw_import_heartbeat(600_000), (60_000, 600_000));
    }

    #[test]
    fn exact_manifest_without_optional_raw_import_is_explicitly_unsupported() {
        let error = validate_exact_request(&manifest_without_optional_raw_import(), &selection())
            .expect_err("missing raw import");

        assert_eq!(error.code(), "raw_import.unsupported_capability");
    }

    #[test]
    fn cleanup_failure_does_not_retroactively_fail_a_committed_cartridge() {
        let committed =
            preserve_committed_success("a".repeat(64), Err(RawImportRuntimeError::Worker));

        assert_eq!(committed.archive_sha256, "a".repeat(64));
        let cleanup = committed.cleanup_fault.expect("cleanup diagnostic");
        assert_eq!(cleanup.code, "raw_import.worker_failed");
        assert!(raw_import_session_is_invalid(&cleanup));
        assert!(!cleanup.message.contains(['\\', '/']));
    }

    #[test]
    fn malicious_staged_artifact_faults_invalidate_the_worker_session() {
        for code in [
            "raw_import.receipt_mismatch",
            "raw_import.staged_path_untrusted",
            "payload_hash_mismatch",
        ] {
            let error = ConversionError::new(code, "bounded public error");
            assert!(
                raw_import_session_is_invalid(&error),
                "{code} must isolate the worker before another source starts"
            );
        }
    }
}
