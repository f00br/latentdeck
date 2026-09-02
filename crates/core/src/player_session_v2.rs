//! Strict Protocol 2 Player startup from trusted installed package bytes.
//!
//! The control path carries identities, bounded metadata, and duplicated
//! native handles only. LC payloads and decoded RGBA bytes never enter the
//! authenticated Named Pipe frames.

use std::{collections::HashSet, future::Future, time::Duration};

#[cfg(windows)]
use std::collections::HashMap;

use latentdeck_control::v2::{
    Ack, Capability, CodecDescriptorRequest, CodecLoad, Command, CommandName, DecodedAbi,
    DeviceKind, ExternalAssetBinding, LimitedVec, MAX_CAPABILITIES, MAX_DECODE_BATCH,
    MAX_EXTERNAL_ASSETS, MAX_FRAME_BYTES, PROTOCOL_VERSION, PlayerOpen, PlayerState,
    ProfileInspect, ProfileKey, ProfileReceipt, ProfileValidate, RingKind, SessionConfigure,
    SignalGeometry, SourceBinding, TensorAbi,
};
#[cfg(windows)]
use latentdeck_extension_manager::{
    ActiveInstalledPackage, CodecCapability, CodecPackManifest, PackageManifest,
};
use latentdeck_gpu::{ring::RingError, ring_v2::RingV2Descriptor};
use semver::Version;
use thiserror::Error;
use uuid::Uuid;

#[cfg(windows)]
use crate::external_asset_v2::{
    IntegrityValidatedExternalAsset, RetainedExternalAssetError, retain_exact_external_asset,
};
use crate::{
    worker_client_v2::WorkerClientV2Error, worker_source_v2::WorkerSourceV2Error,
    worker_supervisor::WorkerSupervisorError,
};

const HOST_API_VERSION: &str = "2.0";
const MAX_INFLIGHT_BATCHES: u8 = 1;

/// Exact trusted Codec Pack and adapter identity for one startup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerCodecSelectionV2 {
    pub pack_id: String,
    pub pack_version: String,
    pub adapter_id: String,
    pub adapter_version: String,
}

/// Codec-neutral identity of one already integrity-validated retained LC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSessionV2SourceIdentity {
    pub source_id: Uuid,
    pub cartridge_id: Uuid,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub payload_sha256: String,
}

/// Exact host ABI, signal, timing, memory, and startup policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerSessionV2HostContract {
    pub app_version: String,
    pub player_session_id: Uuid,
    pub ring_id: Uuid,
    pub profile_key: ProfileKey,
    pub signal_geometry: SignalGeometry,
    pub tensor_abi: TensorAbi,
    pub decoded_abi: DecodedAbi,
    pub maximum_estimated_host_bytes: u64,
    pub maximum_estimated_device_bytes: u64,
    pub device_ordinal: u8,
    pub ring_slot_count: u8,
    pub stream_generation: u64,
    pub loop_enabled: bool,
    pub heartbeat_interval_ms: u32,
    pub heartbeat_hard_timeout_ms: u32,
    pub command_timeout: Duration,
}

/// Stable failures from package preflight, negotiation, or native transport.
#[derive(Debug, Error)]
pub enum PlayerSessionV2Error {
    #[error("invalid Protocol 2 Player host contract: {0}")]
    InvalidHostContract(&'static str),
    #[error("the retained cartridge identity is not a canonical non-nil UUID")]
    InvalidCartridgeIdentity,
    #[error("the trusted package is not an exact compatible Codec Pack v2: {0}")]
    IncompatiblePackage(&'static str),
    #[error("the selected external asset binding is invalid: {0}")]
    InvalidExternalAsset(String),
    #[error("Protocol 2 startup expected {expected:?}, received {actual:?}")]
    UnexpectedAck {
        expected: CommandName,
        actual: CommandName,
    },
    #[error("the worker reply does not match the trusted codec package")]
    PackageMismatch,
    #[error("the worker reply does not match the requested codec profile")]
    ProfileMismatch,
    #[error("the worker reply does not match the requested signal geometry or memory bounds")]
    SignalMismatch,
    #[error("the worker reply does not match the exact tensor/decoded ABI")]
    TensorAbiMismatch,
    #[error("the worker or profile is missing a required capability")]
    CapabilityMismatch,
    #[error("the native startup seam returned a malformed source or ring transfer")]
    InvalidNativeTransfer,
    #[error("external asset I/O failed")]
    ExternalAssetIo(#[source] std::io::Error),
    #[error(transparent)]
    Supervisor(#[from] WorkerSupervisorError),
    #[error(transparent)]
    Client(#[from] WorkerClientV2Error),
    #[error(transparent)]
    Source(#[from] WorkerSourceV2Error),
    #[error(transparent)]
    Ring(#[from] RingError),
}

/// Host-created ring transfer plus the two endpoints that must outlive it.
pub struct PlayerSessionV2PreparedRing<Owner, Consumer> {
    command: Command,
    owner: Owner,
    consumer: Consumer,
}

impl<Owner, Consumer> PlayerSessionV2PreparedRing<Owner, Consumer> {
    #[must_use]
    pub fn new(command: Command, owner: Owner, consumer: Consumer) -> Self {
        Self {
            command,
            owner,
            consumer,
        }
    }
}

/// Injectable control/handle seam for an exact ordered startup.
///
/// Production implements this with the authenticated worker client, a
/// duplicated retained LC handle, and an anonymous RGB Ring ABI 2 mapping.
/// Tests can implement it without a subprocess while observing every command.
pub trait PlayerSessionV2StartupIo {
    type RingOwner;
    type RingConsumer;

    /// Send one typed command and await its correlated acknowledgement.
    fn call(
        &mut self,
        command: Command,
        timeout: Duration,
    ) -> impl Future<Output = Result<Ack, PlayerSessionV2Error>>;

    /// Duplicate the retained LC handle and build the exact path-free command.
    ///
    /// # Errors
    ///
    /// Returns when the retained handle cannot be duplicated or the transfer
    /// metadata cannot be constructed.
    fn prepare_source_open(
        &self,
        source: &PlayerSessionV2SourceIdentity,
    ) -> Result<Command, PlayerSessionV2Error>;

    /// Allocate the anonymous decoded ring and duplicate its three handles.
    ///
    /// # Errors
    ///
    /// Returns on layout, mapping, event, claim, or handle-duplication failure.
    fn prepare_decoded_ring(
        &self,
        descriptor: RingV2Descriptor,
        ring_id: Uuid,
    ) -> Result<
        PlayerSessionV2PreparedRing<Self::RingOwner, Self::RingConsumer>,
        PlayerSessionV2Error,
    >;
}

/// Receipt-gated result of the complete startup orchestration.
pub struct NegotiatedPlayerSessionV2<Owner, Consumer> {
    profile_receipt: ProfileReceipt,
    ring_owner: Owner,
    ring_consumer: Consumer,
}

impl<Owner, Consumer> NegotiatedPlayerSessionV2<Owner, Consumer> {
    #[must_use]
    pub const fn profile_receipt(&self) -> &ProfileReceipt {
        &self.profile_receipt
    }

    #[must_use]
    pub fn into_ring_parts(self) -> (Owner, Consumer) {
        (self.ring_owner, self.ring_consumer)
    }
}

/// Run the exact closed Player startup over an injectable transport.
///
/// The `codec.load` command is constructed only after a `ProfileReceipt` has
/// passed every package, cartridge, profile, signal, ABI, capability, and
/// memory cross-check. There is no Protocol 1 retry or fallback.
///
/// # Errors
///
/// Returns the first contract, transport, remote, or acknowledgement mismatch.
#[allow(clippy::too_many_lines)]
pub async fn orchestrate_player_session_v2_startup<I: PlayerSessionV2StartupIo>(
    io: &mut I,
    selection: &PlayerCodecSelectionV2,
    source: &PlayerSessionV2SourceIdentity,
    host: &PlayerSessionV2HostContract,
    external_assets: &[ExternalAssetBinding],
) -> Result<NegotiatedPlayerSessionV2<I::RingOwner, I::RingConsumer>, PlayerSessionV2Error> {
    validate_host_contract(host)?;
    validate_source_identity(source)?;

    let requested_capabilities = limited_capabilities(vec![Capability::Player])?;
    let ack = io
        .call(
            Command::SessionConfigure(SessionConfigure {
                selected_protocol_version: PROTOCOL_VERSION,
                app_version: host.app_version.clone(),
                heartbeat_interval_ms: host.heartbeat_interval_ms,
                heartbeat_hard_timeout_ms: host.heartbeat_hard_timeout_ms,
                max_frame_bytes: u32::try_from(MAX_FRAME_BYTES)
                    .map_err(|_| PlayerSessionV2Error::InvalidHostContract("frame bound"))?,
                max_inflight_batches: MAX_INFLIGHT_BATCHES,
                requested_capabilities,
            }),
            host.command_timeout,
        )
        .await?;
    let Ack::SessionConfigure(configured) = ack else {
        return Err(unexpected(CommandName::SessionConfigure, &ack));
    };
    if configured.selected_protocol_version != PROTOCOL_VERSION
        || usize::try_from(configured.maximum_frame_bytes).ok() != Some(MAX_FRAME_BYTES)
    {
        return Err(PlayerSessionV2Error::IncompatiblePackage(
            "worker protocol or control frame bound",
        ));
    }
    require_capabilities(
        configured.accepted_capabilities.as_slice(),
        &[Capability::Player],
    )?;

    let ack = io
        .call(
            Command::CodecDescriptor(CodecDescriptorRequest {
                pack_id: selection.pack_id.clone(),
                pack_version: selection.pack_version.clone(),
                adapter_id: selection.adapter_id.clone(),
            }),
            host.command_timeout,
        )
        .await?;
    let Ack::CodecDescriptor(descriptor) = ack else {
        return Err(unexpected(CommandName::CodecDescriptor, &ack));
    };
    if descriptor.pack_id != selection.pack_id
        || descriptor.pack_version != selection.pack_version
        || descriptor.adapter_id != selection.adapter_id
        || descriptor.adapter_version != selection.adapter_version
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }
    if descriptor.host_api_version != HOST_API_VERSION {
        return Err(PlayerSessionV2Error::IncompatiblePackage("host API"));
    }
    require_capabilities(
        descriptor.capabilities.as_slice(),
        &Capability::REQUIRED_CODEC_V2,
    )?;
    if !descriptor.profiles.as_slice().contains(&host.profile_key) {
        return Err(PlayerSessionV2Error::ProfileMismatch);
    }

    let source_open = io.prepare_source_open(source)?;
    validate_source_open_transfer(&source_open, source)?;
    let ack = io.call(source_open, host.command_timeout).await?;
    let Ack::SourceOpen(opened) = ack else {
        return Err(unexpected(CommandName::SourceOpen, &ack));
    };
    if opened.source_id != source.source_id
        || opened.cartridge_id != source.cartridge_id
        || opened.archive_sha256 != source.archive_sha256
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }

    let ack = io
        .call(
            Command::ProfileInspect(ProfileInspect {
                source_id: source.source_id,
                cartridge_id: source.cartridge_id,
                archive_sha256: source.archive_sha256.clone(),
            }),
            host.command_timeout,
        )
        .await?;
    let Ack::ProfileInspect(inspection) = ack else {
        return Err(unexpected(CommandName::ProfileInspect, &ack));
    };
    if inspection.source_id != source.source_id
        || inspection.cartridge_id != source.cartridge_id
        || inspection.archive_sha256 != source.archive_sha256
        || inspection.payload_sha256 != source.payload_sha256
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }
    if inspection.profile_key != host.profile_key {
        return Err(PlayerSessionV2Error::ProfileMismatch);
    }
    if inspection.signal_geometry != host.signal_geometry {
        return Err(PlayerSessionV2Error::SignalMismatch);
    }

    let ack = io
        .call(
            Command::ProfileValidate(ProfileValidate {
                source_id: source.source_id,
                expected_profile: host.profile_key.clone(),
                required_capabilities: limited_capabilities(vec![Capability::Player])?,
            }),
            host.command_timeout,
        )
        .await?;
    let Ack::ProfileValidate(receipt) = ack else {
        return Err(unexpected(CommandName::ProfileValidate, &ack));
    };
    let receipt = *receipt;
    validate_profile_receipt(&receipt, selection, source, host)?;

    let external_assets =
        LimitedVec::<_, MAX_EXTERNAL_ASSETS>::try_from_vec(external_assets.to_vec())
            .map_err(|_| PlayerSessionV2Error::InvalidHostContract("external asset bound"))?;
    let ack = io
        .call(
            Command::CodecLoad(CodecLoad {
                pack_id: selection.pack_id.clone(),
                pack_version: selection.pack_version.clone(),
                adapter_id: selection.adapter_id.clone(),
                adapter_version: selection.adapter_version.clone(),
                device: host.tensor_abi.device,
                device_ordinal: host.device_ordinal,
                external_assets,
            }),
            host.command_timeout,
        )
        .await?;
    let Ack::CodecLoad(loaded) = ack else {
        return Err(unexpected(CommandName::CodecLoad, &ack));
    };
    if loaded.pack_id != selection.pack_id
        || loaded.pack_version != selection.pack_version
        || loaded.adapter_id != selection.adapter_id
        || loaded.adapter_version != selection.adapter_version
        || loaded.device != host.tensor_abi.device
        || loaded.device_ordinal != host.device_ordinal
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }

    let ring_descriptor = RingV2Descriptor::new(
        host.signal_geometry.decoded_width,
        host.signal_geometry.decoded_height,
        u32::from(host.decoded_abi.maximum_batch),
        u32::from(host.ring_slot_count),
        host.stream_generation,
    )?;
    let prepared_ring = io.prepare_decoded_ring(ring_descriptor, host.ring_id)?;
    validate_ring_transfer(&prepared_ring.command, ring_descriptor, host.ring_id)?;
    let PlayerSessionV2PreparedRing {
        command,
        owner,
        consumer,
    } = prepared_ring;
    let ack = io.call(command, host.command_timeout).await?;
    let Ack::RingConfigure(configured) = ack else {
        return Err(unexpected(CommandName::RingConfigure, &ack));
    };
    let layout = ring_descriptor.layout();
    if configured.ring_id != host.ring_id
        || configured.kind != RingKind::DecodedRgba
        || u32::from(configured.slot_count) != layout.slot_count()
        || configured.slot_bytes != layout.slot_bytes()
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }

    let ack = io
        .call(
            Command::PlayerOpen(PlayerOpen {
                player_session_id: host.player_session_id,
                source: SourceBinding {
                    physical_slot: 1,
                    source_id: source.source_id,
                    cartridge_id: source.cartridge_id,
                    archive_sha256: source.archive_sha256.clone(),
                    profile_receipt_id: receipt.receipt_id,
                    // Player looping is a host transport intent. Core always
                    // performs an explicit generation-increasing reset after
                    // the final decoded batch; the worker must never wrap a
                    // causal decoder invisibly inside `player.step`.
                    loop_enabled: false,
                },
                stream_generation: host.stream_generation,
            }),
            host.command_timeout,
        )
        .await?;
    let Ack::PlayerOpen(status) = ack else {
        return Err(unexpected(CommandName::PlayerOpen, &ack));
    };
    if status.player_session_id != host.player_session_id
        || status.stream_generation != host.stream_generation
        || status.stream_sequence != 0
        || status.playhead_slot != 0
        || status.end_of_stream
        || status.decoded_ring_id != Some(host.ring_id)
        || !matches!(status.state, PlayerState::Ready | PlayerState::Paused)
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }

    Ok(NegotiatedPlayerSessionV2 {
        profile_receipt: receipt,
        ring_owner: owner,
        ring_consumer: consumer,
    })
}

fn unexpected(expected: CommandName, actual: &Ack) -> PlayerSessionV2Error {
    PlayerSessionV2Error::UnexpectedAck {
        expected,
        actual: actual.name(),
    }
}

fn limited_capabilities(
    values: Vec<Capability>,
) -> Result<LimitedVec<Capability, MAX_CAPABILITIES>, PlayerSessionV2Error> {
    LimitedVec::try_from_vec(values)
        .map_err(|_| PlayerSessionV2Error::InvalidHostContract("capability bound"))
}

fn require_capabilities(
    actual: &[Capability],
    required: &[Capability],
) -> Result<(), PlayerSessionV2Error> {
    let unique: HashSet<_> = actual.iter().copied().collect();
    if unique.len() != actual.len()
        || !required
            .iter()
            .all(|capability| unique.contains(capability))
    {
        return Err(PlayerSessionV2Error::CapabilityMismatch);
    }
    Ok(())
}

fn validate_host_contract(host: &PlayerSessionV2HostContract) -> Result<(), PlayerSessionV2Error> {
    if Version::parse(&host.app_version).is_err() {
        return Err(PlayerSessionV2Error::InvalidHostContract("app version"));
    }
    if host.player_session_id.is_nil()
        || host.ring_id.is_nil()
        || host.maximum_estimated_host_bytes == 0
        || (host.tensor_abi.device == DeviceKind::Cuda && host.maximum_estimated_device_bytes == 0)
        || host.stream_generation == 0
        || host.command_timeout.is_zero()
    {
        return Err(PlayerSessionV2Error::InvalidHostContract(
            "identity, memory, generation, or timeout",
        ));
    }
    if !(2..=MAX_DECODE_BATCH).contains(&host.ring_slot_count)
        || !(1..=MAX_DECODE_BATCH).contains(&host.decoded_abi.maximum_batch)
        || host.decoded_abi.pixel_format != "rgba8"
    {
        return Err(PlayerSessionV2Error::InvalidHostContract(
            "decoded ring ABI",
        ));
    }
    if host.heartbeat_interval_ms < 250
        || host.heartbeat_interval_ms > 60_000
        || host.heartbeat_hard_timeout_ms < host.heartbeat_interval_ms.saturating_mul(3)
    {
        return Err(PlayerSessionV2Error::InvalidHostContract("heartbeat"));
    }
    let tensor = &host.tensor_abi;
    if tensor.python_major != 3
        || tensor.python_minor != 13
        || tensor.torch_version.is_empty()
        || tensor.shape[0] != 1
        || tensor.shape[2] != 1
        || tensor.shape.contains(&0)
        || !tensor.contiguous
        || tensor.shape[1] != host.signal_geometry.channels
        || tensor.shape[3] != host.signal_geometry.latent_height
        || tensor.shape[4] != host.signal_geometry.latent_width
    {
        return Err(PlayerSessionV2Error::InvalidHostContract("tensor ABI"));
    }
    let signal = &host.signal_geometry;
    if signal.channels == 0
        || signal.latent_height == 0
        || signal.latent_width == 0
        || signal.decoded_height == 0
        || signal.decoded_width == 0
        || signal.frame_rate_numerator == 0
        || signal.frame_rate_denominator == 0
        || signal.timing_contract.is_empty()
        || signal.timing_contract_version.is_empty()
    {
        return Err(PlayerSessionV2Error::InvalidHostContract("signal geometry"));
    }
    Ok(())
}

fn validate_source_identity(
    source: &PlayerSessionV2SourceIdentity,
) -> Result<(), PlayerSessionV2Error> {
    if source.source_id.is_nil()
        || source.cartridge_id.is_nil()
        || source.archive_bytes == 0
        || !is_sha256(&source.archive_sha256)
        || !is_sha256(&source.payload_sha256)
    {
        return Err(PlayerSessionV2Error::InvalidNativeTransfer);
    }
    Ok(())
}

fn validate_source_open_transfer(
    command: &Command,
    source: &PlayerSessionV2SourceIdentity,
) -> Result<(), PlayerSessionV2Error> {
    let Command::SourceOpen(open) = command else {
        return Err(PlayerSessionV2Error::InvalidNativeTransfer);
    };
    if open.source_id != source.source_id
        || open.cartridge_id != source.cartridge_id
        || open.archive_sha256 != source.archive_sha256
        || open.archive_bytes != source.archive_bytes
        || open.retained_native_handle == 0
        || open.integrity_access_receipt.is_empty()
    {
        return Err(PlayerSessionV2Error::InvalidNativeTransfer);
    }
    Ok(())
}

fn validate_ring_transfer(
    command: &Command,
    descriptor: RingV2Descriptor,
    ring_id: Uuid,
) -> Result<(), PlayerSessionV2Error> {
    let Command::RingConfigure(configure) = command else {
        return Err(PlayerSessionV2Error::InvalidNativeTransfer);
    };
    let layout = descriptor.layout();
    if configure.ring_id != ring_id
        || configure.kind != RingKind::DecodedRgba
        || configure.mapping_handle == 0
        || configure.ready_event_handle == 0
        || configure.consumed_event_handle == 0
        || u32::from(configure.slot_count) != layout.slot_count()
        || configure.slot_bytes != layout.slot_bytes()
    {
        return Err(PlayerSessionV2Error::InvalidNativeTransfer);
    }
    Ok(())
}

fn validate_profile_receipt(
    receipt: &ProfileReceipt,
    selection: &PlayerCodecSelectionV2,
    source: &PlayerSessionV2SourceIdentity,
    host: &PlayerSessionV2HostContract,
) -> Result<(), PlayerSessionV2Error> {
    receipt
        .validate()
        .map_err(|_| PlayerSessionV2Error::PackageMismatch)?;
    if receipt.cartridge_id != source.cartridge_id
        || receipt.archive_sha256 != source.archive_sha256
        || receipt.payload_sha256 != source.payload_sha256
        || receipt.pack_id != selection.pack_id
        || receipt.pack_version != selection.pack_version
        || receipt.adapter_id != selection.adapter_id
        || receipt.adapter_version != selection.adapter_version
    {
        return Err(PlayerSessionV2Error::PackageMismatch);
    }
    if receipt.profile_key != host.profile_key {
        return Err(PlayerSessionV2Error::ProfileMismatch);
    }
    if receipt.signal_geometry != host.signal_geometry
        || receipt.estimated_host_bytes > host.maximum_estimated_host_bytes
        || receipt.estimated_device_bytes > host.maximum_estimated_device_bytes
        || (host.tensor_abi.device == DeviceKind::Cpu && receipt.estimated_device_bytes != 0)
        || (host.tensor_abi.device == DeviceKind::Cuda && receipt.estimated_device_bytes == 0)
    {
        return Err(PlayerSessionV2Error::SignalMismatch);
    }
    if receipt.tensor_abi != host.tensor_abi || receipt.decoded_abi != host.decoded_abi {
        return Err(PlayerSessionV2Error::TensorAbiMismatch);
    }
    require_capabilities(receipt.capabilities.as_slice(), &[Capability::Player])
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(windows)]
fn codec_selection(manifest: &CodecPackManifest) -> PlayerCodecSelectionV2 {
    PlayerCodecSelectionV2 {
        pack_id: manifest.pack_id.clone(),
        pack_version: manifest.pack_version.clone(),
        adapter_id: manifest.adapter.adapter_id.clone(),
        adapter_version: manifest.adapter.adapter_version.clone(),
    }
}

#[cfg(windows)]
fn validate_package_contract(
    package: &ActiveInstalledPackage,
    host: &PlayerSessionV2HostContract,
    external_assets: &[ExternalAssetBinding],
    retained_external_assets: &[IntegrityValidatedExternalAsset],
) -> Result<(PlayerCodecSelectionV2, Vec<IntegrityValidatedExternalAsset>), PlayerSessionV2Error> {
    if !package.trust_receipt().enabled {
        return Err(PlayerSessionV2Error::IncompatiblePackage("disabled"));
    }
    let PackageManifest::Codec(manifest) = package.manifest() else {
        return Err(PlayerSessionV2Error::IncompatiblePackage("package kind"));
    };
    if manifest.manifest_version != "2.0.0"
        || manifest.compatibility.worker_protocol != PROTOCOL_VERSION
        || manifest.compatibility.codec_adapter_api != 1
        || manifest.compatibility.tensor_abi != "latentdeck.tensor.v1"
        || manifest.compatibility.python.version != "3.13"
        || manifest.compatibility.torch_exact_build != host.tensor_abi.torch_version
    {
        return Err(PlayerSessionV2Error::IncompatiblePackage(
            "protocol, adapter, Python, or Torch ABI",
        ));
    }
    let app = Version::parse(&host.app_version)
        .map_err(|_| PlayerSessionV2Error::InvalidHostContract("app version"))?;
    let minimum = Version::parse(&manifest.compatibility.app_min_inclusive)
        .map_err(|_| PlayerSessionV2Error::IncompatiblePackage("app range"))?;
    let maximum = Version::parse(&manifest.compatibility.app_max_exclusive)
        .map_err(|_| PlayerSessionV2Error::IncompatiblePackage("app range"))?;
    if app < minimum || app >= maximum {
        return Err(PlayerSessionV2Error::IncompatiblePackage("app range"));
    }
    if !manifest.compatibility.profiles.iter().any(|profile| {
        profile.codec_family == host.profile_key.codec_family
            && profile.profile == host.profile_key.profile
            && profile.profile_version == host.profile_key.profile_version
    }) {
        return Err(PlayerSessionV2Error::ProfileMismatch);
    }
    let declared_capabilities: HashSet<_> = manifest.capabilities.iter().copied().collect();
    if ![
        CodecCapability::Player,
        CodecCapability::Realtime,
        CodecCapability::Resample,
        CodecCapability::SnapshotCapture,
        CodecCapability::LiveCapture,
    ]
    .iter()
    .all(|capability| declared_capabilities.contains(capability))
    {
        return Err(PlayerSessionV2Error::CapabilityMismatch);
    }
    let retained_assets =
        validate_external_assets(manifest, external_assets, retained_external_assets)?;
    Ok((codec_selection(manifest), retained_assets))
}

#[cfg(windows)]
fn validate_external_assets(
    manifest: &CodecPackManifest,
    bindings: &[ExternalAssetBinding],
    retained_assets: &[IntegrityValidatedExternalAsset],
) -> Result<Vec<IntegrityValidatedExternalAsset>, PlayerSessionV2Error> {
    if bindings.len() > MAX_EXTERNAL_ASSETS {
        return Err(PlayerSessionV2Error::InvalidExternalAsset(
            "too many bindings".to_owned(),
        ));
    }
    let declared: HashMap<_, _> = manifest
        .external_assets
        .iter()
        .map(|asset| (asset.asset_id.as_str(), asset))
        .collect();
    let mut seen = HashSet::new();
    let retained: HashMap<_, _> = retained_assets
        .iter()
        .map(|asset| (asset.binding().asset_id.as_str(), asset))
        .collect();
    if retained.len() != retained_assets.len()
        || retained
            .keys()
            .any(|asset_id| !bindings.iter().any(|binding| &binding.asset_id == asset_id))
    {
        return Err(PlayerSessionV2Error::InvalidExternalAsset(
            "retained asset mismatch".to_owned(),
        ));
    }
    let mut retained_assets = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if !seen.insert(binding.asset_id.as_str()) {
            return Err(PlayerSessionV2Error::InvalidExternalAsset(
                binding.asset_id.clone(),
            ));
        }
        let Some(expected) = declared.get(binding.asset_id.as_str()) else {
            return Err(PlayerSessionV2Error::InvalidExternalAsset(
                binding.asset_id.clone(),
            ));
        };
        if binding.sha256 != expected.sha256
            || binding.byte_length != expected.byte_length
            || !is_sha256(&binding.sha256)
        {
            return Err(PlayerSessionV2Error::InvalidExternalAsset(
                binding.asset_id.clone(),
            ));
        }
        if let Some(retained) = retained.get(binding.asset_id.as_str()) {
            if retained.binding() != binding {
                return Err(PlayerSessionV2Error::InvalidExternalAsset(
                    binding.asset_id.clone(),
                ));
            }
            retained_assets.push(retained.clone_retained());
        } else {
            retained_assets.push(validate_external_asset_file(binding)?);
        }
    }
    if manifest
        .external_assets
        .iter()
        .any(|asset| asset.required && !seen.contains(asset.asset_id.as_str()))
    {
        return Err(PlayerSessionV2Error::InvalidExternalAsset(
            "required asset missing".to_owned(),
        ));
    }
    Ok(retained_assets)
}

#[cfg(windows)]
fn validate_external_asset_file(
    binding: &ExternalAssetBinding,
) -> Result<IntegrityValidatedExternalAsset, PlayerSessionV2Error> {
    match retain_exact_external_asset(binding) {
        Ok(file) => Ok(IntegrityValidatedExternalAsset::from_validated_file(
            binding.clone(),
            file,
        )),
        Err(RetainedExternalAssetError::Invalid) => Err(
            PlayerSessionV2Error::InvalidExternalAsset(binding.asset_id.clone()),
        ),
        Err(RetainedExternalAssetError::Io(error)) => {
            Err(PlayerSessionV2Error::ExternalAssetIo(error))
        }
    }
}

#[cfg(windows)]
mod windows_runtime {
    use latentdeck_cartridge::reader::IntegrityValidatedCartridge;
    use latentdeck_control::v2::{Command, ExternalAssetBinding, RingConfigure, RingKind};
    use latentdeck_extension_manager::ActiveInstalledPackage;
    use latentdeck_gpu::{
        ring_v2::RingV2Descriptor,
        windows_ring_v2::{WindowsRgbRingV2Consumer, WindowsRgbRingV2Owner},
    };
    use uuid::Uuid;

    use super::{
        NegotiatedPlayerSessionV2, PlayerSessionV2Error, PlayerSessionV2HostContract,
        PlayerSessionV2PreparedRing, PlayerSessionV2SourceIdentity, PlayerSessionV2StartupIo,
        orchestrate_player_session_v2_startup, validate_package_contract, validate_source_identity,
    };
    use crate::{
        external_asset_v2::IntegrityValidatedExternalAsset,
        worker_client_v2::WorkerClientV2,
        worker_source_v2::prepare_source_open,
        worker_supervisor::{ValidatedWorkerLaunch, spawn_worker_v2},
    };

    struct WindowsPlayerSessionV2StartupIo<'a> {
        client: WorkerClientV2,
        cartridge: &'a IntegrityValidatedCartridge,
    }

    impl WindowsPlayerSessionV2StartupIo<'_> {
        fn into_client(self) -> WorkerClientV2 {
            self.client
        }
    }

    impl PlayerSessionV2StartupIo for WindowsPlayerSessionV2StartupIo<'_> {
        type RingOwner = WindowsRgbRingV2Owner;
        type RingConsumer = WindowsRgbRingV2Consumer;

        async fn call(
            &mut self,
            command: Command,
            timeout: std::time::Duration,
        ) -> Result<latentdeck_control::v2::Ack, PlayerSessionV2Error> {
            self.client.call(command, timeout).await.map_err(Into::into)
        }

        fn prepare_source_open(
            &self,
            source: &PlayerSessionV2SourceIdentity,
        ) -> Result<Command, PlayerSessionV2Error> {
            prepare_source_open(&self.client, self.cartridge, source.source_id).map_err(Into::into)
        }

        fn prepare_decoded_ring(
            &self,
            descriptor: RingV2Descriptor,
            ring_id: Uuid,
        ) -> Result<
            PlayerSessionV2PreparedRing<Self::RingOwner, Self::RingConsumer>,
            PlayerSessionV2Error,
        > {
            let owner = WindowsRgbRingV2Owner::create(descriptor)?;
            let consumer = owner.open_consumer()?;
            let binding = self
                .client
                .with_process_handle(|process| owner.duplicate_into(process))??;
            let slot_count = u8::try_from(binding.slot_count())
                .map_err(|_| PlayerSessionV2Error::InvalidNativeTransfer)?;
            Ok(PlayerSessionV2PreparedRing::new(
                Command::RingConfigure(RingConfigure {
                    ring_id,
                    kind: RingKind::DecodedRgba,
                    mapping_handle: binding.mapping_handle(),
                    ready_event_handle: binding.ready_event_handle(),
                    consumed_event_handle: binding.consumed_event_handle(),
                    slot_count,
                    slot_bytes: binding.slot_bytes(),
                }),
                owner,
                consumer,
            ))
        }
    }

    /// Live owning Player Protocol 2 session.
    ///
    /// The exact active package lease, retained read-only cartridge handle,
    /// authenticated worker, anonymous mapping, and sole Core consumer remain
    /// alive together. Dropping this value terminates the worker Job Object.
    pub struct PlayerSessionV2 {
        client: WorkerClientV2,
        codec_package: ActiveInstalledPackage,
        cartridge: IntegrityValidatedCartridge,
        _external_asset_handles: Vec<IntegrityValidatedExternalAsset>,
        ring_owner: WindowsRgbRingV2Owner,
        ring_consumer: WindowsRgbRingV2Consumer,
        profile_receipt: latentdeck_control::v2::ProfileReceipt,
    }

    impl PlayerSessionV2 {
        #[must_use]
        pub const fn client(&self) -> &WorkerClientV2 {
            &self.client
        }

        #[must_use]
        pub fn client_mut(&mut self) -> &mut WorkerClientV2 {
            &mut self.client
        }

        #[must_use]
        pub const fn codec_package(&self) -> &ActiveInstalledPackage {
            &self.codec_package
        }

        #[must_use]
        pub const fn cartridge(&self) -> &IntegrityValidatedCartridge {
            &self.cartridge
        }

        #[must_use]
        pub const fn ring_owner(&self) -> &WindowsRgbRingV2Owner {
            &self.ring_owner
        }

        /// Adopt the exact generation reset by the worker-side ring producer.
        ///
        /// Both retained Core views must observe the empty acknowledged mapping
        /// before another Player step is admitted.
        ///
        /// # Errors
        ///
        /// Returns when the mapped generation, queue state, or slot state does
        /// not match the worker acknowledgement. Callers must then terminate
        /// the session rather than continuing with split ring identities.
        pub fn adopt_ring_generation(
            &mut self,
            new_generation: u64,
        ) -> Result<(), latentdeck_gpu::ring::RingError> {
            self.ring_owner.adopt_generation(new_generation)?;
            self.ring_consumer.adopt_generation(new_generation)
        }

        #[must_use]
        pub fn ring_consumer_mut(&mut self) -> &mut WindowsRgbRingV2Consumer {
            &mut self.ring_consumer
        }

        #[must_use]
        pub const fn profile_receipt(&self) -> &latentdeck_control::v2::ProfileReceipt {
            &self.profile_receipt
        }
    }

    /// Spawn and fully open one Player Protocol 2 session from exact trusted
    /// installed package and retained cartridge objects.
    ///
    /// # Errors
    ///
    /// Returns without P1 fallback on any package, asset, process, protocol,
    /// receipt, shared-ring, or Player-open failure.
    pub async fn start_player_session_v2(
        package: ActiveInstalledPackage,
        cartridge: IntegrityValidatedCartridge,
        host: PlayerSessionV2HostContract,
        external_assets: Vec<ExternalAssetBinding>,
    ) -> Result<PlayerSessionV2, PlayerSessionV2Error> {
        start_player_session_v2_with_retained_assets(
            package,
            cartridge,
            host,
            external_assets,
            Vec::new(),
        )
        .await
    }

    /// Spawn a Player session while reusing exact external-asset integrity
    /// evidence retained by the host selection UI.
    ///
    /// # Errors
    ///
    /// Rejects evidence that does not exactly match the wire binding and
    /// current Codec Pack declaration before worker or GPU allocation.
    pub async fn start_player_session_v2_with_retained_assets(
        package: ActiveInstalledPackage,
        cartridge: IntegrityValidatedCartridge,
        host: PlayerSessionV2HostContract,
        external_assets: Vec<ExternalAssetBinding>,
        retained_external_assets: Vec<IntegrityValidatedExternalAsset>,
    ) -> Result<PlayerSessionV2, PlayerSessionV2Error> {
        let (selection, external_asset_handles) = validate_package_contract(
            &package,
            &host,
            &external_assets,
            &retained_external_assets,
        )?;
        let manifest_profile = match package.manifest() {
            latentdeck_extension_manager::PackageManifest::Codec(manifest) => {
                &manifest.compatibility
            }
            latentdeck_extension_manager::PackageManifest::Deck(_) => {
                return Err(PlayerSessionV2Error::IncompatiblePackage("package kind"));
            }
        };
        let cartridge_profile = &cartridge.manifest().codec;
        if cartridge_profile.family.0 != host.profile_key.codec_family
            || cartridge_profile.profile.0 != host.profile_key.profile
            || cartridge_profile.profile_version.0 != host.profile_key.profile_version
            || !manifest_profile.profiles.iter().any(|profile| {
                profile.codec_family == cartridge_profile.family.0
                    && profile.profile == cartridge_profile.profile.0
                    && profile.profile_version == cartridge_profile.profile_version.0
            })
        {
            return Err(PlayerSessionV2Error::ProfileMismatch);
        }
        let cartridge_id_text = &cartridge.manifest().cartridge_id.0;
        let cartridge_id = Uuid::parse_str(cartridge_id_text)
            .ok()
            .filter(|value| !value.is_nil() && value.hyphenated().to_string() == *cartridge_id_text)
            .ok_or(PlayerSessionV2Error::InvalidCartridgeIdentity)?;
        let source = PlayerSessionV2SourceIdentity {
            source_id: Uuid::new_v4(),
            cartridge_id,
            archive_sha256: cartridge.receipt().archive_sha256.to_string(),
            archive_bytes: cartridge.receipt().archive_bytes,
            payload_sha256: cartridge.receipt().payload_sha256.to_string(),
        };
        validate_source_identity(&source)?;

        let launch = ValidatedWorkerLaunch::from_installed_codec_v2(&package)?;
        let worker = spawn_worker_v2(launch).await?.connect().await?;
        let mut io = WindowsPlayerSessionV2StartupIo {
            client: WorkerClientV2::new(worker),
            cartridge: &cartridge,
        };
        let negotiated: NegotiatedPlayerSessionV2<_, _> = orchestrate_player_session_v2_startup(
            &mut io,
            &selection,
            &source,
            &host,
            &external_assets,
        )
        .await?;
        let profile_receipt = negotiated.profile_receipt().clone();
        let (ring_owner, ring_consumer) = negotiated.into_ring_parts();
        let client = io.into_client();
        Ok(PlayerSessionV2 {
            client,
            codec_package: package,
            cartridge,
            _external_asset_handles: external_asset_handles,
            ring_owner,
            ring_consumer,
            profile_receipt,
        })
    }
}

#[cfg(windows)]
pub use windows_runtime::{
    PlayerSessionV2, start_player_session_v2, start_player_session_v2_with_retained_assets,
};
