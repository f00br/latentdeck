//! Generic multi-source Deck Protocol 2 startup and owning runtime session.
//!
//! The host resolves exact active Codec and Deck package versions before this
//! boundary. Startup opens already integrity-validated LC handles, negotiates
//! one receipt per physical source, loads the codec, binds RGB Ring ABI 2, and
//! finally sends the hash-bound dynamic `deck.load` built by
//! [`crate::deck_runtime_v2::ActiveDeckRuntime`].

use std::{collections::HashSet, time::Duration};

use latentdeck_control::v2::{
    Capability, ControlBinding, DecodedAbi, DeviceKind, ProfileKey, RoleBinding, SignalGeometry,
    SourceTransportBinding, TensorAbi,
};
use semver::Version;
use thiserror::Error;
use uuid::Uuid;

use crate::deck_runtime_v2::DeckRuntimeError;

const MAX_DECODE_BATCH: u8 = 24;

/// Exact codec-neutral host ABI and bounded runtime policy for one Deck.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckSessionV2HostContract {
    pub app_version: String,
    pub deck_session_id: Uuid,
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
    pub heartbeat_interval_ms: u32,
    pub heartbeat_hard_timeout_ms: u32,
    pub command_timeout: Duration,
}

/// User/session state supplied to the exact active Deck runtime after source
/// receipts have been negotiated.
#[derive(Clone, Debug, PartialEq)]
pub struct DeckSessionV2LoadRequest {
    pub roles: Vec<RoleBinding>,
    pub controls: Vec<ControlBinding>,
    pub source_transport: Vec<SourceTransportBinding>,
    pub seed: u64,
}

#[derive(Debug, Error)]
pub enum DeckSessionV2Error {
    #[error("invalid Protocol 2 Deck host contract: {0}")]
    InvalidHostContract(&'static str),
    #[error("the trusted package is not an exact compatible Protocol 2 package: {0}")]
    IncompatiblePackage(&'static str),
    #[error("the retained cartridge identity or source set is invalid")]
    InvalidSource,
    #[error("the worker acknowledgement does not match the exact startup request")]
    ProtocolMismatch,
    #[error("the worker or profile is missing realtime capability")]
    CapabilityMismatch,
    #[error("the codec profile, signal, tensor ABI, or memory receipt is incompatible")]
    ProfileMismatch,
    #[error("the host-created retained-handle or shared-ring transfer is invalid")]
    InvalidNativeTransfer,
    #[error("external codec asset validation failed")]
    ExternalAssetInvalid,
    #[error(transparent)]
    DeckRuntime(#[from] DeckRuntimeError),
    #[error(transparent)]
    Supervisor(#[from] crate::worker_supervisor::WorkerSupervisorError),
    #[error(transparent)]
    Client(#[from] crate::worker_client_v2::WorkerClientV2Error),
    #[error(transparent)]
    Source(#[from] crate::worker_source_v2::WorkerSourceV2Error),
    #[error(transparent)]
    Ring(#[from] latentdeck_gpu::ring::RingError),
}

/// Validate the common host contract before any worker or GPU allocation.
///
/// # Errors
///
/// Returns a closed contract error for malformed identities, versions,
/// memory, heartbeat, tensor, signal, or ring bounds.
pub fn validate_deck_host_contract(
    host: &DeckSessionV2HostContract,
) -> Result<(), DeckSessionV2Error> {
    if Version::parse(&host.app_version).is_err()
        || host.deck_session_id.is_nil()
        || host.ring_id.is_nil()
        || host.maximum_estimated_host_bytes == 0
        || (host.tensor_abi.device == DeviceKind::Cuda && host.maximum_estimated_device_bytes == 0)
        || host.stream_generation == 0
        || host.command_timeout.is_zero()
    {
        return Err(DeckSessionV2Error::InvalidHostContract(
            "identity, version, memory, generation, or timeout",
        ));
    }
    if !(2..=MAX_DECODE_BATCH).contains(&host.ring_slot_count)
        || !(1..=MAX_DECODE_BATCH).contains(&host.decoded_abi.maximum_batch)
        || host.decoded_abi.pixel_format != "rgba8"
    {
        return Err(DeckSessionV2Error::InvalidHostContract("decoded ring ABI"));
    }
    if host.heartbeat_interval_ms < 250
        || host.heartbeat_interval_ms > 60_000
        || host.heartbeat_hard_timeout_ms < host.heartbeat_interval_ms.saturating_mul(3)
    {
        return Err(DeckSessionV2Error::InvalidHostContract("heartbeat"));
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
        return Err(DeckSessionV2Error::InvalidHostContract("tensor ABI"));
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
        return Err(DeckSessionV2Error::InvalidHostContract("signal geometry"));
    }
    Ok(())
}

fn require_capabilities(
    actual: &[Capability],
    required: &[Capability],
) -> Result<(), DeckSessionV2Error> {
    let unique: HashSet<_> = actual.iter().copied().collect();
    if unique.len() != actual.len()
        || !required
            .iter()
            .all(|capability| unique.contains(capability))
    {
        return Err(DeckSessionV2Error::CapabilityMismatch);
    }
    Ok(())
}

#[cfg(windows)]
mod windows_runtime {
    use std::{collections::HashMap, fs::File};

    use super::{
        Capability, DeckSessionV2Error, DeckSessionV2HostContract, DeckSessionV2LoadRequest,
        DeviceKind, HashSet, SourceTransportBinding, Uuid, Version, require_capabilities,
        validate_deck_host_contract,
    };
    use crate::{
        deck_runtime_v2::{ActiveDeckRuntime, DeckLoadRequest},
        external_asset_v2::retain_exact_external_asset,
        worker_client_v2::WorkerClientV2,
        worker_source_v2::prepare_source_open,
        worker_supervisor::{ValidatedWorkerLaunch, spawn_worker_v2},
    };
    use latentdeck_cartridge::reader::IntegrityValidatedCartridge;
    use latentdeck_control::v2::{
        Ack, CodecDescriptorRequest, CodecLoad, Command, CommandName, DeckState, DeckTransportSet,
        ExternalAssetBinding, LimitedVec, MAX_CAPABILITIES, MAX_EXTERNAL_ASSETS, MAX_FRAME_BYTES,
        MAX_SOURCES, PROTOCOL_VERSION, ProfileInspect, ProfileReceipt, ProfileValidate,
        RingConfigure, RingKind, SessionConfigure, SourceBinding,
    };
    use latentdeck_extension_manager::{
        ActiveInstalledPackage, CodecCapability, CodecPackManifest, PackageManifest, TensorDevice,
        TensorDtype,
    };
    use latentdeck_gpu::{
        ring_v2::RingV2Descriptor,
        windows_ring_v2::{WindowsRgbRingV2Consumer, WindowsRgbRingV2Owner},
    };

    const MAX_INFLIGHT_BATCHES: u8 = 1;

    #[derive(Clone, Debug)]
    struct CodecSelection {
        pack_id: String,
        pack_version: String,
        adapter_id: String,
        adapter_version: String,
    }

    struct SourceIdentity {
        physical_slot: u8,
        source_id: Uuid,
        cartridge_id: Uuid,
        archive_sha256: String,
        archive_bytes: u64,
        payload_sha256: String,
        loop_enabled: bool,
    }

    /// Live owning generic Deck Protocol 2 session.
    pub struct DeckSessionV2 {
        client: WorkerClientV2,
        codec_package: ActiveInstalledPackage,
        deck_runtime: ActiveDeckRuntime,
        cartridges: Vec<IntegrityValidatedCartridge>,
        _external_asset_handles: Vec<File>,
        ring_owner: WindowsRgbRingV2Owner,
        ring_consumer: WindowsRgbRingV2Consumer,
        profile_receipts: Vec<ProfileReceipt>,
        initial_status: latentdeck_control::v2::DeckStatusSnapshot,
    }

    impl DeckSessionV2 {
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
        pub const fn deck_runtime(&self) -> &ActiveDeckRuntime {
            &self.deck_runtime
        }

        #[must_use]
        pub fn cartridges(&self) -> &[IntegrityValidatedCartridge] {
            &self.cartridges
        }

        #[must_use]
        pub fn profile_receipts(&self) -> &[ProfileReceipt] {
            &self.profile_receipts
        }

        #[must_use]
        pub const fn initial_status(&self) -> &latentdeck_control::v2::DeckStatusSnapshot {
            &self.initial_status
        }

        #[must_use]
        pub const fn ring_owner(&self) -> &WindowsRgbRingV2Owner {
            &self.ring_owner
        }

        #[must_use]
        pub fn ring_consumer_mut(&mut self) -> &mut WindowsRgbRingV2Consumer {
            &mut self.ring_consumer
        }

        /// Adopt the generation already reset by the worker producer.
        ///
        /// # Errors
        ///
        /// Returns unless both Core views observe the exact empty new mapping.
        pub fn adopt_ring_generation(
            &mut self,
            new_generation: u64,
        ) -> Result<(), latentdeck_gpu::ring::RingError> {
            self.ring_owner.adopt_generation(new_generation)?;
            self.ring_consumer.adopt_generation(new_generation)
        }
    }

    /// Spawn and fully load one generic multi-source Deck Protocol 2 session.
    ///
    /// # Errors
    ///
    /// Returns without Protocol 1 fallback on any package, cartridge, receipt,
    /// worker, ring, or dynamic Deck binding mismatch.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn start_deck_session_v2(
        codec_package: ActiveInstalledPackage,
        deck_runtime: ActiveDeckRuntime,
        cartridges: Vec<IntegrityValidatedCartridge>,
        host: DeckSessionV2HostContract,
        external_assets: Vec<ExternalAssetBinding>,
        load: DeckSessionV2LoadRequest,
    ) -> Result<DeckSessionV2, DeckSessionV2Error> {
        validate_deck_host_contract(&host)?;
        validate_load_shape(&load, cartridges.len())?;
        let (selection, external_asset_handles) =
            validate_codec_package(&codec_package, &host, &external_assets)?;
        validate_deck_package(&deck_runtime, &host, cartridges.len())?;
        let identities = source_identities(&cartridges, &load.source_transport)?;

        let launch = ValidatedWorkerLaunch::from_installed_codec_v2(&codec_package)?;
        let worker = spawn_worker_v2(launch).await?.connect().await?;
        let mut client = WorkerClientV2::new(worker);

        let requested = limited_capabilities(vec![Capability::Realtime])?;
        let ack = client
            .call(
                Command::SessionConfigure(SessionConfigure {
                    selected_protocol_version: PROTOCOL_VERSION,
                    app_version: host.app_version.clone(),
                    heartbeat_interval_ms: host.heartbeat_interval_ms,
                    heartbeat_hard_timeout_ms: host.heartbeat_hard_timeout_ms,
                    max_frame_bytes: u32::try_from(MAX_FRAME_BYTES)
                        .map_err(|_| DeckSessionV2Error::InvalidHostContract("frame bound"))?,
                    max_inflight_batches: MAX_INFLIGHT_BATCHES,
                    requested_capabilities: requested,
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
            return Err(DeckSessionV2Error::ProtocolMismatch);
        }
        require_capabilities(
            configured.accepted_capabilities.as_slice(),
            &[Capability::Realtime],
        )?;

        let ack = client
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
            || descriptor.host_api_version != "2.0"
            || !descriptor.profiles.as_slice().contains(&host.profile_key)
        {
            return Err(DeckSessionV2Error::ProtocolMismatch);
        }
        require_capabilities(
            descriptor.capabilities.as_slice(),
            &Capability::REQUIRED_CODEC_V2,
        )?;

        let mut receipts = Vec::with_capacity(cartridges.len());
        for (cartridge, source) in cartridges.iter().zip(&identities) {
            let command = prepare_source_open(&client, cartridge, source.source_id)?;
            validate_source_open(&command, source)?;
            let ack = client.call(command, host.command_timeout).await?;
            let Ack::SourceOpen(opened) = ack else {
                return Err(unexpected(CommandName::SourceOpen, &ack));
            };
            if opened.source_id != source.source_id
                || opened.cartridge_id != source.cartridge_id
                || opened.archive_sha256 != source.archive_sha256
            {
                return Err(DeckSessionV2Error::ProtocolMismatch);
            }

            let ack = client
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
                || inspection.profile_key != host.profile_key
                || inspection.signal_geometry != host.signal_geometry
            {
                return Err(DeckSessionV2Error::ProfileMismatch);
            }

            let ack = client
                .call(
                    Command::ProfileValidate(ProfileValidate {
                        source_id: source.source_id,
                        expected_profile: host.profile_key.clone(),
                        required_capabilities: limited_capabilities(vec![Capability::Realtime])?,
                    }),
                    host.command_timeout,
                )
                .await?;
            let Ack::ProfileValidate(receipt) = ack else {
                return Err(unexpected(CommandName::ProfileValidate, &ack));
            };
            let receipt = *receipt;
            validate_profile_receipt(&receipt, &selection, source, &host)?;
            receipts.push(receipt);
        }

        let external_assets = LimitedVec::<_, MAX_EXTERNAL_ASSETS>::try_from_vec(external_assets)
            .map_err(|_| DeckSessionV2Error::ExternalAssetInvalid)?;
        let ack = client
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
            return Err(DeckSessionV2Error::ProtocolMismatch);
        }

        let descriptor = RingV2Descriptor::new(
            host.signal_geometry.decoded_width,
            host.signal_geometry.decoded_height,
            u32::from(host.decoded_abi.maximum_batch),
            u32::from(host.ring_slot_count),
            host.stream_generation,
        )?;
        let ring_owner = WindowsRgbRingV2Owner::create(descriptor)?;
        let ring_consumer = ring_owner.open_consumer()?;
        let binding =
            client.with_process_handle(|process| ring_owner.duplicate_into(process))??;
        let slot_count = u8::try_from(binding.slot_count())
            .map_err(|_| DeckSessionV2Error::InvalidNativeTransfer)?;
        let ring_command = Command::RingConfigure(RingConfigure {
            ring_id: host.ring_id,
            kind: RingKind::DecodedRgba,
            mapping_handle: binding.mapping_handle(),
            ready_event_handle: binding.ready_event_handle(),
            consumed_event_handle: binding.consumed_event_handle(),
            slot_count,
            slot_bytes: binding.slot_bytes(),
        });
        validate_ring_command(&ring_command, descriptor, host.ring_id)?;
        let ack = client.call(ring_command, host.command_timeout).await?;
        let Ack::RingConfigure(configured) = ack else {
            return Err(unexpected(CommandName::RingConfigure, &ack));
        };
        if configured.ring_id != host.ring_id
            || configured.kind != RingKind::DecodedRgba
            || configured.slot_count != slot_count
            || configured.slot_bytes != descriptor.layout().slot_bytes()
        {
            return Err(DeckSessionV2Error::ProtocolMismatch);
        }

        let sources = identities
            .iter()
            .zip(&receipts)
            .map(|(source, receipt)| SourceBinding {
                physical_slot: source.physical_slot,
                source_id: source.source_id,
                cartridge_id: source.cartridge_id,
                archive_sha256: source.archive_sha256.clone(),
                profile_receipt_id: receipt.receipt_id,
                loop_enabled: source.loop_enabled,
            })
            .collect();
        let command = deck_runtime.build_load_command(DeckLoadRequest {
            deck_session_id: host.deck_session_id,
            sources,
            roles: load.roles.clone(),
            controls: load.controls.clone(),
            seed: load.seed,
            stream_generation: host.stream_generation,
        })?;
        let ack = client.call(command, host.command_timeout).await?;
        let Ack::DeckLoad(status) = ack else {
            return Err(unexpected(CommandName::DeckLoad, &ack));
        };
        let status = *status;
        validate_deck_status(&status, &host, &load, 1, false)?;

        let source_transport =
            LimitedVec::<_, MAX_SOURCES>::try_from_vec(load.source_transport.clone())
                .map_err(|_| DeckSessionV2Error::InvalidSource)?;
        let ack = client
            .call(
                Command::DeckTransportSet(DeckTransportSet {
                    deck_session_id: host.deck_session_id,
                    deck_revision: status.deck_revision,
                    sources: source_transport,
                }),
                host.command_timeout,
            )
            .await?;
        let Ack::DeckTransportSet(status) = ack else {
            return Err(unexpected(CommandName::DeckTransportSet, &ack));
        };
        let status = *status;
        validate_deck_status(&status, &host, &load, 1, true)?;

        Ok(DeckSessionV2 {
            client,
            codec_package,
            deck_runtime,
            cartridges,
            _external_asset_handles: external_asset_handles,
            ring_owner,
            ring_consumer,
            profile_receipts: receipts,
            initial_status: status,
        })
    }

    fn unexpected(expected: CommandName, actual: &Ack) -> DeckSessionV2Error {
        let _ = (expected, actual.name());
        DeckSessionV2Error::ProtocolMismatch
    }

    fn limited_capabilities(
        capabilities: Vec<Capability>,
    ) -> Result<LimitedVec<Capability, MAX_CAPABILITIES>, DeckSessionV2Error> {
        LimitedVec::try_from_vec(capabilities)
            .map_err(|_| DeckSessionV2Error::InvalidHostContract("capability bound"))
    }

    fn validate_load_shape(
        load: &DeckSessionV2LoadRequest,
        source_count: usize,
    ) -> Result<(), DeckSessionV2Error> {
        if source_count == 0
            || source_count > MAX_SOURCES
            || load.source_transport.len() != source_count
        {
            return Err(DeckSessionV2Error::InvalidSource);
        }
        let expected: Vec<u8> = (1..=u8::try_from(source_count)
            .map_err(|_| DeckSessionV2Error::InvalidSource)?)
            .collect();
        let mut actual: Vec<u8> = load
            .source_transport
            .iter()
            .map(|source| source.physical_slot)
            .collect();
        actual.sort_unstable();
        if actual != expected {
            return Err(DeckSessionV2Error::InvalidSource);
        }
        Ok(())
    }

    fn source_identities(
        cartridges: &[IntegrityValidatedCartridge],
        transport: &[SourceTransportBinding],
    ) -> Result<Vec<SourceIdentity>, DeckSessionV2Error> {
        cartridges
            .iter()
            .enumerate()
            .map(|(index, cartridge)| {
                let physical_slot =
                    u8::try_from(index + 1).map_err(|_| DeckSessionV2Error::InvalidSource)?;
                let cartridge_text = &cartridge.manifest().cartridge_id.0;
                let cartridge_id = Uuid::parse_str(cartridge_text)
                    .ok()
                    .filter(|value| {
                        !value.is_nil() && value.hyphenated().to_string() == *cartridge_text
                    })
                    .ok_or(DeckSessionV2Error::InvalidSource)?;
                let source_transport = transport
                    .iter()
                    .find(|source| source.physical_slot == physical_slot)
                    .ok_or(DeckSessionV2Error::InvalidSource)?;
                Ok(SourceIdentity {
                    physical_slot,
                    source_id: Uuid::new_v4(),
                    cartridge_id,
                    archive_sha256: cartridge.receipt().archive_sha256.to_string(),
                    archive_bytes: cartridge.receipt().archive_bytes,
                    payload_sha256: cartridge.receipt().payload_sha256.to_string(),
                    loop_enabled: source_transport.loop_enabled,
                })
            })
            .collect()
    }

    fn validate_source_open(
        command: &Command,
        source: &SourceIdentity,
    ) -> Result<(), DeckSessionV2Error> {
        let Command::SourceOpen(open) = command else {
            return Err(DeckSessionV2Error::InvalidNativeTransfer);
        };
        if open.source_id != source.source_id
            || open.cartridge_id != source.cartridge_id
            || open.archive_sha256 != source.archive_sha256
            || open.archive_bytes != source.archive_bytes
            || open.retained_native_handle == 0
            || open.integrity_access_receipt.is_empty()
        {
            return Err(DeckSessionV2Error::InvalidNativeTransfer);
        }
        Ok(())
    }

    fn validate_ring_command(
        command: &Command,
        descriptor: RingV2Descriptor,
        ring_id: Uuid,
    ) -> Result<(), DeckSessionV2Error> {
        let Command::RingConfigure(configure) = command else {
            return Err(DeckSessionV2Error::InvalidNativeTransfer);
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
            return Err(DeckSessionV2Error::InvalidNativeTransfer);
        }
        Ok(())
    }

    fn validate_profile_receipt(
        receipt: &ProfileReceipt,
        selection: &CodecSelection,
        source: &SourceIdentity,
        host: &DeckSessionV2HostContract,
    ) -> Result<(), DeckSessionV2Error> {
        receipt
            .validate()
            .map_err(|_| DeckSessionV2Error::ProfileMismatch)?;
        if receipt.cartridge_id != source.cartridge_id
            || receipt.archive_sha256 != source.archive_sha256
            || receipt.payload_sha256 != source.payload_sha256
            || receipt.pack_id != selection.pack_id
            || receipt.pack_version != selection.pack_version
            || receipt.adapter_id != selection.adapter_id
            || receipt.adapter_version != selection.adapter_version
            || receipt.profile_key != host.profile_key
            || receipt.signal_geometry != host.signal_geometry
            || receipt.tensor_abi != host.tensor_abi
            || receipt.decoded_abi != host.decoded_abi
            || receipt.estimated_host_bytes > host.maximum_estimated_host_bytes
            || receipt.estimated_device_bytes > host.maximum_estimated_device_bytes
            || (host.tensor_abi.device == DeviceKind::Cpu && receipt.estimated_device_bytes != 0)
            || (host.tensor_abi.device == DeviceKind::Cuda && receipt.estimated_device_bytes == 0)
        {
            return Err(DeckSessionV2Error::ProfileMismatch);
        }
        require_capabilities(receipt.capabilities.as_slice(), &[Capability::Realtime])
    }

    fn validate_deck_status(
        status: &latentdeck_control::v2::DeckStatusSnapshot,
        host: &DeckSessionV2HostContract,
        load: &DeckSessionV2LoadRequest,
        expected_revision: u64,
        require_transport: bool,
    ) -> Result<(), DeckSessionV2Error> {
        if status.deck_session_id != host.deck_session_id
            || status.deck_revision != expected_revision
            || status.stream_generation != host.stream_generation
            || status.stream_sequence != 0
            || status.playheads.len() != load.source_transport.len()
            || status.roles.as_slice() != load.roles
            || status.controls.as_slice() != load.controls
            || (require_transport && status.source_transport.as_slice() != load.source_transport)
            || status.seed != load.seed
            || !matches!(
                status.state,
                DeckState::Ready | DeckState::Paused | DeckState::Playing
            )
        {
            return Err(DeckSessionV2Error::ProtocolMismatch);
        }
        Ok(())
    }

    fn validate_codec_package(
        package: &ActiveInstalledPackage,
        host: &DeckSessionV2HostContract,
        assets: &[ExternalAssetBinding],
    ) -> Result<(CodecSelection, Vec<File>), DeckSessionV2Error> {
        if !package.trust_receipt().enabled {
            return Err(DeckSessionV2Error::IncompatiblePackage("disabled codec"));
        }
        let PackageManifest::Codec(manifest) = package.manifest() else {
            return Err(DeckSessionV2Error::IncompatiblePackage("codec kind"));
        };
        if manifest.manifest_version != "2.0.0"
            || manifest.compatibility.worker_protocol != PROTOCOL_VERSION
            || manifest.compatibility.codec_adapter_api != 1
            || manifest.compatibility.tensor_abi != "latentdeck.tensor.v1"
            || manifest.compatibility.python.version != "3.13"
            || manifest.compatibility.torch_exact_build != host.tensor_abi.torch_version
            || !manifest.capabilities.contains(&CodecCapability::Realtime)
            || !manifest.compatibility.profiles.iter().any(|profile| {
                profile.codec_family == host.profile_key.codec_family
                    && profile.profile == host.profile_key.profile
                    && profile.profile_version == host.profile_key.profile_version
            })
        {
            return Err(DeckSessionV2Error::IncompatiblePackage("codec ABI"));
        }
        validate_version_range(
            &host.app_version,
            &manifest.compatibility.app_min_inclusive,
            &manifest.compatibility.app_max_exclusive,
        )?;
        let external_asset_handles = validate_external_assets(manifest, assets)?;
        Ok((
            CodecSelection {
                pack_id: manifest.pack_id.clone(),
                pack_version: manifest.pack_version.clone(),
                adapter_id: manifest.adapter.adapter_id.clone(),
                adapter_version: manifest.adapter.adapter_version.clone(),
            },
            external_asset_handles,
        ))
    }

    fn validate_deck_package(
        runtime: &ActiveDeckRuntime,
        host: &DeckSessionV2HostContract,
        source_count: usize,
    ) -> Result<(), DeckSessionV2Error> {
        let PackageManifest::Deck(manifest) = runtime.active_package().manifest() else {
            return Err(DeckSessionV2Error::IncompatiblePackage("Deck kind"));
        };
        let signal = &manifest.signal;
        let timing = &signal.timing;
        let exact_geometry = signal.geometry_allowlist.iter().any(|geometry| {
            let dtype_matches = matches!(
                (geometry.dtype, host.tensor_abi.dtype),
                (
                    TensorDtype::Fp16,
                    latentdeck_control::v2::TensorDtype::Float16
                ) | (
                    TensorDtype::Fp32,
                    latentdeck_control::v2::TensorDtype::Float32
                )
            );
            let device_matches = matches!(
                (geometry.device, host.tensor_abi.device),
                (TensorDevice::Cpu, DeviceKind::Cpu) | (TensorDevice::Cuda, DeviceKind::Cuda)
            );
            geometry.batch == 1
                && geometry.temporal == 1
                && u32::from(geometry.channels) == host.signal_geometry.channels
                && geometry.height == host.signal_geometry.latent_height
                && geometry.width == host.signal_geometry.latent_width
                && dtype_matches
                && device_matches
        });
        if manifest.manifest_version != "1.0.0"
            || manifest.compatibility.worker_protocol != PROTOCOL_VERSION
            || manifest.compatibility.deck_host_api != 1
            || manifest.compatibility.deck_operator_api != 1
            || manifest.compatibility.tensor_abi != "latentdeck.tensor.v1"
            || manifest.compatibility.python.version != "3.13"
            || manifest.compatibility.torch_exact_build != host.tensor_abi.torch_version
            || usize::from(signal.slots) != source_count
            || !exact_geometry
            || timing.frames_per_second_numerator != host.signal_geometry.frame_rate_numerator
            || timing.frames_per_second_denominator != host.signal_geometry.frame_rate_denominator
            || !signal
                .required_capabilities
                .contains(&CodecCapability::Realtime)
            || signal.profile_allowlist.as_ref().is_some_and(|profiles| {
                !profiles.iter().any(|profile| {
                    profile.codec_family == host.profile_key.codec_family
                        && profile.profile == host.profile_key.profile
                        && profile.profile_version == host.profile_key.profile_version
                })
            })
        {
            return Err(DeckSessionV2Error::IncompatiblePackage(
                "Deck ABI or signal",
            ));
        }
        validate_version_range(
            &host.app_version,
            &manifest.compatibility.app_min_inclusive,
            &manifest.compatibility.app_max_exclusive,
        )
    }

    fn validate_version_range(
        app: &str,
        minimum: &str,
        maximum: &str,
    ) -> Result<(), DeckSessionV2Error> {
        let app = Version::parse(app)
            .map_err(|_| DeckSessionV2Error::InvalidHostContract("app version"))?;
        let minimum = Version::parse(minimum)
            .map_err(|_| DeckSessionV2Error::IncompatiblePackage("app range"))?;
        let maximum = Version::parse(maximum)
            .map_err(|_| DeckSessionV2Error::IncompatiblePackage("app range"))?;
        if app < minimum || app >= maximum {
            return Err(DeckSessionV2Error::IncompatiblePackage("app range"));
        }
        Ok(())
    }

    fn validate_external_assets(
        manifest: &CodecPackManifest,
        bindings: &[ExternalAssetBinding],
    ) -> Result<Vec<File>, DeckSessionV2Error> {
        if bindings.len() > MAX_EXTERNAL_ASSETS {
            return Err(DeckSessionV2Error::ExternalAssetInvalid);
        }
        let declared: HashMap<_, _> = manifest
            .external_assets
            .iter()
            .map(|asset| (asset.asset_id.as_str(), asset))
            .collect();
        let mut seen = HashSet::new();
        let mut retained_handles = Vec::with_capacity(bindings.len());
        for binding in bindings {
            if !seen.insert(binding.asset_id.as_str()) {
                return Err(DeckSessionV2Error::ExternalAssetInvalid);
            }
            let Some(expected) = declared.get(binding.asset_id.as_str()) else {
                return Err(DeckSessionV2Error::ExternalAssetInvalid);
            };
            if binding.sha256 != expected.sha256
                || binding.byte_length != expected.byte_length
                || !is_sha256(&binding.sha256)
            {
                return Err(DeckSessionV2Error::ExternalAssetInvalid);
            }
            retained_handles.push(validate_external_asset_file(binding)?);
        }
        if manifest
            .external_assets
            .iter()
            .any(|asset| asset.required && !seen.contains(asset.asset_id.as_str()))
        {
            return Err(DeckSessionV2Error::ExternalAssetInvalid);
        }
        Ok(retained_handles)
    }

    fn validate_external_asset_file(
        binding: &ExternalAssetBinding,
    ) -> Result<File, DeckSessionV2Error> {
        retain_exact_external_asset(binding).map_err(|_| DeckSessionV2Error::ExternalAssetInvalid)
    }

    fn is_sha256(value: &str) -> bool {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

#[cfg(windows)]
pub use windows_runtime::{DeckSessionV2, start_deck_session_v2};

#[cfg(test)]
mod tests {
    use latentdeck_control::v2::{DecodedAbi, TensorDtype};

    use super::{
        DeckSessionV2Error, DeckSessionV2HostContract, DeviceKind, Duration, ProfileKey,
        SignalGeometry, TensorAbi, Uuid, validate_deck_host_contract,
    };

    fn host() -> DeckSessionV2HostContract {
        DeckSessionV2HostContract {
            app_version: "0.2.0".to_owned(),
            deck_session_id: Uuid::new_v4(),
            ring_id: Uuid::new_v4(),
            profile_key: ProfileKey {
                codec_family: "synthetic".to_owned(),
                profile: "latent".to_owned(),
                profile_version: "1.0.0".to_owned(),
            },
            signal_geometry: SignalGeometry {
                channels: 4,
                latent_height: 8,
                latent_width: 8,
                decoded_height: 64,
                decoded_width: 64,
                frame_rate_numerator: 24,
                frame_rate_denominator: 1,
                timing_contract: "synthetic_ticks".to_owned(),
                timing_contract_version: "1.0.0".to_owned(),
            },
            tensor_abi: TensorAbi {
                python_major: 3,
                python_minor: 13,
                torch_version: "2.13.0+cu130".to_owned(),
                dtype: TensorDtype::Float16,
                shape: [1, 4, 1, 8, 8],
                contiguous: true,
                device: DeviceKind::Cuda,
            },
            decoded_abi: DecodedAbi {
                pixel_format: "rgba8".to_owned(),
                maximum_batch: 24,
            },
            maximum_estimated_host_bytes: 1024,
            maximum_estimated_device_bytes: 2048,
            device_ordinal: 0,
            ring_slot_count: 2,
            stream_generation: 1,
            heartbeat_interval_ms: 1_000,
            heartbeat_hard_timeout_ms: 5_000,
            command_timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn deck_host_contract_closes_tensor_signal_and_heartbeat_before_startup() {
        validate_deck_host_contract(&host()).expect("valid host contract");

        let mut invalid = host();
        invalid.tensor_abi.shape[1] = 5;
        assert!(matches!(
            validate_deck_host_contract(&invalid),
            Err(DeckSessionV2Error::InvalidHostContract("tensor ABI"))
        ));

        let mut invalid = host();
        invalid.heartbeat_hard_timeout_ms = 2_000;
        assert!(matches!(
            validate_deck_host_contract(&invalid),
            Err(DeckSessionV2Error::InvalidHostContract("heartbeat"))
        ));
    }
}
