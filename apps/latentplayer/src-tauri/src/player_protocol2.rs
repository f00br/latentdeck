//! Strict host-side Player bridge for Worker Protocol 2.
//!
//! The bridge is deliberately transport-independent. The authenticated Named
//! Pipe supervisor remains the process boundary, while tensor and decoded
//! frame bytes remain in shared handles/rings. This module owns negotiation,
//! exact receipt cross-checking, and lifecycle ordering before a command that
//! may allocate GPU memory can be emitted.

#![allow(
    dead_code,
    reason = "the Protocol 2 bridge is compiled and contract-tested before the typed wire replies are connected to the Windows supervisor"
)]

use std::collections::HashSet;

use latentdeck_control::v2::{
    Ack, Capability, CodecDescriptor, CodecDescriptorRequest, CodecLoad, Command, DecodedAbi,
    DeviceKind, ExternalAssetBinding, LimitedVec, MAX_CAPABILITIES, MAX_DECODE_BATCH,
    MAX_EXTERNAL_ASSETS, MAX_FRAME_BYTES, PROTOCOL_VERSION, PlayerOpen, PlayerReset, PlayerState,
    PlayerStatusSnapshot, PlayerStep, PlayerStepAck, ProfileInspect, ProfileInspection, ProfileKey,
    ProfileReceipt, ProfileValidate, RingConfigure, RingConfigured, RingKind, SessionConfigure,
    SignalGeometry, SourceBinding, SourceOpen, TensorAbi,
};
use thiserror::Error;
use uuid::Uuid;

const PLAYER_HOST_API_VERSION: &str = "2.0";
const PLAYER_HEARTBEAT_INTERVAL_MS: u32 = 1_000;
const PLAYER_HEARTBEAT_HARD_TIMEOUT_MS: u32 = 10_000;
const PLAYER_MAX_INFLIGHT_BATCHES: u8 = 1;

/// Explicit runtime bridge selection. There is intentionally no `Auto` form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerBridgeSelection {
    /// Accepted Protocol 1 H3 Player bridge.
    Protocol1H3,
    /// Generic capability-negotiated Protocol 2 bridge.
    Protocol2,
}

/// Dispatch surface used by the runtime while the two protocols coexist.
pub(crate) trait ExplicitPlayerBridgeLauncher {
    type Output;
    type Error;

    fn launch_protocol1_h3(&mut self) -> Result<Self::Output, Self::Error>;
    fn launch_protocol2(&mut self) -> Result<Self::Output, Self::Error>;
}

/// Launch exactly the selected bridge. A Protocol 2 failure is returned to the
/// caller and never invokes the Protocol 1 path.
pub(crate) fn launch_explicit_player_bridge<L: ExplicitPlayerBridgeLauncher>(
    selection: PlayerBridgeSelection,
    launcher: &mut L,
) -> Result<L::Output, L::Error> {
    match selection {
        PlayerBridgeSelection::Protocol1H3 => launcher.launch_protocol1_h3(),
        PlayerBridgeSelection::Protocol2 => launcher.launch_protocol2(),
    }
}

/// Stable compatibility and lifecycle failures surfaced by the Player host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerProtocol2ErrorCode {
    UnsupportedProtocol,
    UnsupportedHostApi,
    UnsupportedTensorAbi,
    UnsupportedProfile,
    UnsupportedSignal,
    UnsupportedCapability,
    PackageInvalid,
    InvalidLifecycle,
}

impl PlayerProtocol2ErrorCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedProtocol => "unsupported_protocol",
            Self::UnsupportedHostApi => "unsupported_host_api",
            Self::UnsupportedTensorAbi => "unsupported_tensor_abi",
            Self::UnsupportedProfile => "unsupported_profile",
            Self::UnsupportedSignal => "unsupported_signal",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::PackageInvalid => "package_invalid",
            Self::InvalidLifecycle => "protocol.invalid_lifecycle",
        }
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
#[error("{code}: {message}", code = .code.as_str())]
pub(crate) struct PlayerProtocol2Error {
    pub(crate) code: PlayerProtocol2ErrorCode,
    pub(crate) message: &'static str,
}

impl PlayerProtocol2Error {
    const fn new(code: PlayerProtocol2ErrorCode, message: &'static str) -> Self {
        Self { code, message }
    }

    const fn protocol() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::UnsupportedProtocol,
            "The selected worker does not support exact Worker Protocol 2.",
        )
    }

    const fn host_api() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::UnsupportedHostApi,
            "The codec adapter targets a different host API.",
        )
    }

    const fn tensor_abi() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::UnsupportedTensorAbi,
            "The codec tensor ABI does not exactly match the negotiated host ABI.",
        )
    }

    const fn profile() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::UnsupportedProfile,
            "The codec does not support the cartridge profile exactly.",
        )
    }

    const fn signal() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::UnsupportedSignal,
            "The codec receipt does not match the validated cartridge signal.",
        )
    }

    const fn capability() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::UnsupportedCapability,
            "The codec or profile is missing a required capability.",
        )
    }

    const fn package() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::PackageInvalid,
            "The typed codec reply does not match the selected trusted package.",
        )
    }

    const fn lifecycle() -> Self {
        Self::new(
            PlayerProtocol2ErrorCode::InvalidLifecycle,
            "The Protocol 2 Player lifecycle command is out of order.",
        )
    }
}

/// Exact trusted Codec Pack and adapter selected by the Extensions Manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerCodecSelection {
    pub(crate) pack_id: String,
    pub(crate) pack_version: String,
    pub(crate) adapter_id: String,
    pub(crate) adapter_version: String,
}

/// Host ABI and bounded memory policy checked before codec load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerHostContract {
    pub(crate) protocol_version: u16,
    pub(crate) host_api_version: String,
    pub(crate) tensor_abi: TensorAbi,
    pub(crate) decoded_abi: DecodedAbi,
    pub(crate) maximum_estimated_host_bytes: u64,
    pub(crate) maximum_estimated_device_bytes: u64,
}

/// Codec-neutral source identity already validated and retained by Core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PlayerSourceContract {
    pub(crate) source_id: Uuid,
    pub(crate) cartridge_id: Uuid,
    pub(crate) archive_sha256: String,
    pub(crate) archive_bytes: u64,
    pub(crate) payload_sha256: String,
    pub(crate) profile_key: ProfileKey,
    pub(crate) signal_geometry: SignalGeometry,
}

/// Accepting a validated profile receipt yields the only token that permits
/// construction of `codec.load`, the first lifecycle command allowed to cause
/// GPU allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GpuAllocationPermit {
    receipt_id: Uuid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerProtocol2ReplyOutcome {
    Accepted,
    GpuAllocationPermitted(GpuAllocationPermit),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerProtocol2State {
    New,
    SessionConfigured,
    DescriptorAccepted,
    SourceOpened,
    SourceInspected,
    ProfileValidated,
    CodecLoaded,
    RingConfigured,
    PlayerOpened,
}

/// Protocol 2 transport policy. Control messages carry metadata and duplicated
/// handles only; existing shared tensor mappings and decoded RGBA rings own all
/// bulk bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlayerProtocol2TransportContract {
    pub(crate) maximum_control_frame_bytes: usize,
    pub(crate) tensor_transport: PlayerBulkTransport,
    pub(crate) decoded_transport: PlayerBulkTransport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlayerBulkTransport {
    SharedTensorHandles,
    DecodedRgbaRing,
}

impl PlayerProtocol2TransportContract {
    pub(crate) const SHARED_HANDLES_ONLY: Self = Self {
        maximum_control_frame_bytes: MAX_FRAME_BYTES,
        tensor_transport: PlayerBulkTransport::SharedTensorHandles,
        decoded_transport: PlayerBulkTransport::DecodedRgbaRing,
    };
}

/// Ordered host-side lifecycle for one generic Player session.
pub(crate) struct PlayerProtocol2Bridge {
    app_version: String,
    player_session_id: Uuid,
    stream_generation: u64,
    loop_enabled: bool,
    selection: PlayerCodecSelection,
    host: PlayerHostContract,
    source: PlayerSourceContract,
    state: PlayerProtocol2State,
    descriptor: Option<CodecDescriptor>,
    receipt: Option<ProfileReceipt>,
    pending_codec_device: Option<(DeviceKind, u8)>,
    pending_decoded_ring: Option<RingConfigured>,
    decoded_ring: Option<RingConfigured>,
}

impl PlayerProtocol2Bridge {
    pub(crate) fn new(
        app_version: impl Into<String>,
        player_session_id: Uuid,
        selection: PlayerCodecSelection,
        host: PlayerHostContract,
        source: PlayerSourceContract,
        loop_enabled: bool,
    ) -> Result<Self, PlayerProtocol2Error> {
        if player_session_id.is_nil()
            || source.source_id.is_nil()
            || source.cartridge_id.is_nil()
            || source.archive_bytes == 0
            || host.maximum_estimated_host_bytes == 0
            || host.maximum_estimated_device_bytes == 0
        {
            return Err(PlayerProtocol2Error::package());
        }
        if host.protocol_version != PROTOCOL_VERSION {
            return Err(PlayerProtocol2Error::protocol());
        }
        if host.host_api_version != PLAYER_HOST_API_VERSION {
            return Err(PlayerProtocol2Error::host_api());
        }
        if !tensor_abi_is_protocol2(&host.tensor_abi)
            || !decoded_abi_is_protocol2(&host.decoded_abi)
        {
            return Err(PlayerProtocol2Error::tensor_abi());
        }

        Ok(Self {
            app_version: app_version.into(),
            player_session_id,
            stream_generation: 1,
            loop_enabled,
            selection,
            host,
            source,
            state: PlayerProtocol2State::New,
            descriptor: None,
            receipt: None,
            pending_codec_device: None,
            pending_decoded_ring: None,
            decoded_ring: None,
        })
    }

    pub(crate) const fn state(&self) -> PlayerProtocol2State {
        self.state
    }

    pub(crate) const fn transport_contract() -> PlayerProtocol2TransportContract {
        PlayerProtocol2TransportContract::SHARED_HANDLES_ONLY
    }

    pub(crate) fn session_configure_command(&self) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::New)?;
        Ok(Command::SessionConfigure(SessionConfigure {
            selected_protocol_version: PROTOCOL_VERSION,
            app_version: self.app_version.clone(),
            heartbeat_interval_ms: PLAYER_HEARTBEAT_INTERVAL_MS,
            heartbeat_hard_timeout_ms: PLAYER_HEARTBEAT_HARD_TIMEOUT_MS,
            max_frame_bytes: u32::try_from(MAX_FRAME_BYTES)
                .map_err(|_| PlayerProtocol2Error::package())?,
            max_inflight_batches: PLAYER_MAX_INFLIGHT_BATCHES,
            requested_capabilities: limited_capabilities(vec![Capability::Player])?,
        }))
    }

    pub(crate) fn codec_descriptor_command(&self) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::SessionConfigured)?;
        Ok(Command::CodecDescriptor(CodecDescriptorRequest {
            pack_id: self.selection.pack_id.clone(),
            pack_version: self.selection.pack_version.clone(),
            adapter_id: self.selection.adapter_id.clone(),
        }))
    }

    /// Transfer only the already-retained read-only native handle. The LC
    /// archive and tensor bytes remain outside the Protocol 2 frame.
    pub(crate) fn source_open_command(
        &self,
        retained_native_handle: u64,
        integrity_access_receipt: String,
    ) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::DescriptorAccepted)?;
        if retained_native_handle == 0 || integrity_access_receipt.is_empty() {
            return Err(PlayerProtocol2Error::package());
        }
        Ok(Command::SourceOpen(SourceOpen {
            source_id: self.source.source_id,
            cartridge_id: self.source.cartridge_id,
            archive_sha256: self.source.archive_sha256.clone(),
            archive_bytes: self.source.archive_bytes,
            retained_native_handle,
            integrity_access_receipt,
        }))
    }

    pub(crate) fn profile_inspect_command(&self) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::SourceOpened)?;
        Ok(Command::ProfileInspect(ProfileInspect {
            source_id: self.source.source_id,
            cartridge_id: self.source.cartridge_id,
            archive_sha256: self.source.archive_sha256.clone(),
        }))
    }

    pub(crate) fn profile_validate_command(&self) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::SourceInspected)?;
        Ok(Command::ProfileValidate(ProfileValidate {
            source_id: self.source.source_id,
            expected_profile: self.source.profile_key.clone(),
            required_capabilities: limited_capabilities(vec![Capability::Player])?,
        }))
    }

    pub(crate) fn codec_load_command(
        &mut self,
        permit: GpuAllocationPermit,
        device_ordinal: u8,
        external_assets: Vec<ExternalAssetBinding>,
    ) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::ProfileValidated)?;
        let receipt = self
            .receipt
            .as_ref()
            .ok_or_else(PlayerProtocol2Error::lifecycle)?;
        if permit.receipt_id != receipt.receipt_id {
            return Err(PlayerProtocol2Error::lifecycle());
        }
        let external_assets = LimitedVec::<_, MAX_EXTERNAL_ASSETS>::try_from_vec(external_assets)
            .map_err(|_| PlayerProtocol2Error::package())?;
        self.pending_codec_device = Some((self.host.tensor_abi.device, device_ordinal));
        Ok(Command::CodecLoad(CodecLoad {
            pack_id: self.selection.pack_id.clone(),
            pack_version: self.selection.pack_version.clone(),
            adapter_id: self.selection.adapter_id.clone(),
            adapter_version: self.selection.adapter_version.clone(),
            device: self.host.tensor_abi.device,
            device_ordinal,
            external_assets,
        }))
    }

    /// Bind the existing host-created decoded RGBA ring by duplicated handles.
    /// Inline RGBA bytes are not representable by this command.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn ring_configure_command(
        &mut self,
        ring_id: Uuid,
        mapping_handle: u64,
        ready_event_handle: u64,
        consumed_event_handle: u64,
        slot_count: u8,
        slot_bytes: u64,
    ) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::CodecLoaded)?;
        if ring_id.is_nil()
            || mapping_handle == 0
            || ready_event_handle == 0
            || consumed_event_handle == 0
            || !(2..=MAX_DECODE_BATCH).contains(&slot_count)
            || slot_bytes == 0
        {
            return Err(PlayerProtocol2Error::package());
        }
        let configured = RingConfigured {
            ring_id,
            kind: RingKind::DecodedRgba,
            slot_count,
            slot_bytes,
        };
        self.pending_decoded_ring = Some(configured);
        Ok(Command::RingConfigure(RingConfigure {
            ring_id,
            kind: RingKind::DecodedRgba,
            mapping_handle,
            ready_event_handle,
            consumed_event_handle,
            slot_count,
            slot_bytes,
        }))
    }

    pub(crate) fn player_open_command(&self) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::RingConfigured)?;
        let receipt = self
            .receipt
            .as_ref()
            .ok_or_else(PlayerProtocol2Error::lifecycle)?;
        Ok(Command::PlayerOpen(PlayerOpen {
            player_session_id: self.player_session_id,
            source: SourceBinding {
                physical_slot: 1,
                source_id: self.source.source_id,
                cartridge_id: self.source.cartridge_id,
                archive_sha256: self.source.archive_sha256.clone(),
                profile_receipt_id: receipt.receipt_id,
                loop_enabled: self.loop_enabled,
            },
            stream_generation: self.stream_generation,
        }))
    }

    pub(crate) fn player_step_command(
        &self,
        maximum_decoded_frames: u8,
    ) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::PlayerOpened)?;
        if !(1..=MAX_DECODE_BATCH).contains(&maximum_decoded_frames) {
            return Err(PlayerProtocol2Error::package());
        }
        Ok(Command::PlayerStep(PlayerStep {
            player_session_id: self.player_session_id,
            stream_generation: self.stream_generation,
            maximum_decoded_frames,
        }))
    }

    pub(crate) fn player_reset_command(&mut self) -> Result<Command, PlayerProtocol2Error> {
        self.require_state(PlayerProtocol2State::PlayerOpened)?;
        self.stream_generation = self
            .stream_generation
            .checked_add(1)
            .ok_or_else(PlayerProtocol2Error::lifecycle)?;
        Ok(Command::PlayerReset(PlayerReset {
            player_session_id: self.player_session_id,
            new_stream_generation: self.stream_generation,
        }))
    }

    /// Accept one closed, typed Protocol 2 acknowledgement. Unexpected reply
    /// kinds are lifecycle failures; they never trigger a P1 retry.
    #[allow(
        clippy::too_many_lines,
        reason = "one exhaustive match keeps the closed Ack lifecycle and no-fallback behavior auditable"
    )]
    pub(crate) fn accept_ack(
        &mut self,
        ack: Ack,
    ) -> Result<PlayerProtocol2ReplyOutcome, PlayerProtocol2Error> {
        match ack {
            Ack::SessionConfigure(configured) => {
                self.require_state(PlayerProtocol2State::New)?;
                if configured.selected_protocol_version != PROTOCOL_VERSION
                    || usize::try_from(configured.maximum_frame_bytes).ok() != Some(MAX_FRAME_BYTES)
                {
                    return Err(PlayerProtocol2Error::protocol());
                }
                if !contains_unique_capabilities(
                    configured.accepted_capabilities.as_slice(),
                    &[Capability::Player],
                ) {
                    return Err(PlayerProtocol2Error::capability());
                }
                self.state = PlayerProtocol2State::SessionConfigured;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::CodecDescriptor(descriptor) => {
                self.require_state(PlayerProtocol2State::SessionConfigured)?;
                self.validate_descriptor(&descriptor)?;
                self.descriptor = Some(descriptor);
                self.state = PlayerProtocol2State::DescriptorAccepted;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::SourceOpen(opened) => {
                self.require_state(PlayerProtocol2State::DescriptorAccepted)?;
                self.validate_source_opened(&opened)?;
                self.state = PlayerProtocol2State::SourceOpened;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::ProfileInspect(inspection) => {
                self.require_state(PlayerProtocol2State::SourceOpened)?;
                self.validate_inspection(&inspection)?;
                self.state = PlayerProtocol2State::SourceInspected;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::ProfileValidate(receipt) => {
                self.require_state(PlayerProtocol2State::SourceInspected)?;
                let receipt = *receipt;
                self.validate_receipt(&receipt)?;
                let permit = GpuAllocationPermit {
                    receipt_id: receipt.receipt_id,
                };
                self.receipt = Some(receipt);
                self.state = PlayerProtocol2State::ProfileValidated;
                Ok(PlayerProtocol2ReplyOutcome::GpuAllocationPermitted(permit))
            }
            Ack::CodecLoad(loaded) => {
                self.require_state(PlayerProtocol2State::ProfileValidated)?;
                let expected_device = self
                    .pending_codec_device
                    .ok_or_else(PlayerProtocol2Error::lifecycle)?;
                if !codec_loaded_matches(&loaded, &self.selection)
                    || (loaded.device, loaded.device_ordinal) != expected_device
                {
                    return Err(PlayerProtocol2Error::package());
                }
                self.pending_codec_device = None;
                self.state = PlayerProtocol2State::CodecLoaded;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::RingConfigure(configured) => {
                self.require_state(PlayerProtocol2State::CodecLoaded)?;
                let expected = self
                    .pending_decoded_ring
                    .as_ref()
                    .ok_or_else(PlayerProtocol2Error::lifecycle)?;
                if &configured != expected {
                    return Err(PlayerProtocol2Error::package());
                }
                self.pending_decoded_ring = None;
                self.decoded_ring = Some(configured);
                self.state = PlayerProtocol2State::RingConfigured;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::PlayerOpen(status) => {
                self.require_state(PlayerProtocol2State::RingConfigured)?;
                self.validate_player_status(&status)?;
                self.state = PlayerProtocol2State::PlayerOpened;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::PlayerStep(step) => {
                self.require_state(PlayerProtocol2State::PlayerOpened)?;
                self.validate_player_step(&step)?;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::PlayerReset(status) => {
                self.require_state(PlayerProtocol2State::PlayerOpened)?;
                self.validate_player_status(&status)?;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::PlayerStatus(status) => {
                self.require_state(PlayerProtocol2State::PlayerOpened)?;
                self.validate_player_identity(&status)?;
                Ok(PlayerProtocol2ReplyOutcome::Accepted)
            }
            Ack::SessionStatus(_)
            | Ack::SessionShutdown(_)
            | Ack::CodecUnload(_)
            | Ack::SourceClose(_)
            | Ack::RingRelease(_)
            | Ack::DeckLoad(_)
            | Ack::DeckProcess(_)
            | Ack::DeckControlsSet(_)
            | Ack::DeckRolesSet(_)
            | Ack::DeckTransportSet(_)
            | Ack::DeckSeedSet(_)
            | Ack::DeckReset(_)
            | Ack::DeckRestart(_)
            | Ack::DeckStatus(_)
            | Ack::CaptureStart(_)
            | Ack::CaptureStop(_)
            | Ack::CaptureStatus(_)
            | Ack::RawImportPreflight(_)
            | Ack::RawImportStage(_)
            | Ack::RawImportAbort(_)
            | Ack::MetricsGet(_) => Err(PlayerProtocol2Error::lifecycle()),
        }
    }

    fn validate_descriptor(
        &self,
        descriptor: &CodecDescriptor,
    ) -> Result<(), PlayerProtocol2Error> {
        if descriptor.host_api_version != self.host.host_api_version {
            return Err(PlayerProtocol2Error::host_api());
        }
        if descriptor.pack_id != self.selection.pack_id
            || descriptor.pack_version != self.selection.pack_version
            || descriptor.adapter_id != self.selection.adapter_id
            || descriptor.adapter_version != self.selection.adapter_version
        {
            return Err(PlayerProtocol2Error::package());
        }
        if !contains_unique_capabilities(
            descriptor.capabilities.as_slice(),
            &Capability::REQUIRED_CODEC_V2,
        ) {
            return Err(PlayerProtocol2Error::capability());
        }
        if !descriptor
            .profiles
            .as_slice()
            .iter()
            .any(|profile| profile == &self.source.profile_key)
        {
            return Err(PlayerProtocol2Error::profile());
        }
        Ok(())
    }

    fn validate_source_opened(
        &self,
        opened: &latentdeck_control::v2::SourceOpened,
    ) -> Result<(), PlayerProtocol2Error> {
        if opened.source_id != self.source.source_id
            || opened.cartridge_id != self.source.cartridge_id
            || opened.archive_sha256 != self.source.archive_sha256
        {
            return Err(PlayerProtocol2Error::package());
        }
        Ok(())
    }

    fn validate_inspection(
        &self,
        inspection: &ProfileInspection,
    ) -> Result<(), PlayerProtocol2Error> {
        if inspection.source_id != self.source.source_id
            || inspection.cartridge_id != self.source.cartridge_id
            || inspection.archive_sha256 != self.source.archive_sha256
            || inspection.payload_sha256 != self.source.payload_sha256
        {
            return Err(PlayerProtocol2Error::package());
        }
        if inspection.profile_key != self.source.profile_key {
            return Err(PlayerProtocol2Error::profile());
        }
        if inspection.signal_geometry != self.source.signal_geometry {
            return Err(PlayerProtocol2Error::signal());
        }
        Ok(())
    }

    fn validate_player_status(
        &self,
        status: &PlayerStatusSnapshot,
    ) -> Result<(), PlayerProtocol2Error> {
        self.validate_player_identity(status)?;
        if status.stream_sequence != 0
            || status.playhead_slot != 0
            || status.end_of_stream
            || !matches!(status.state, PlayerState::Ready | PlayerState::Paused)
        {
            return Err(PlayerProtocol2Error::package());
        }
        Ok(())
    }

    fn validate_player_identity(
        &self,
        status: &PlayerStatusSnapshot,
    ) -> Result<(), PlayerProtocol2Error> {
        let ring = self
            .decoded_ring
            .as_ref()
            .ok_or_else(PlayerProtocol2Error::lifecycle)?;
        if status.player_session_id != self.player_session_id
            || status.stream_generation != self.stream_generation
            || status.decoded_ring_id != Some(ring.ring_id)
            || matches!(status.state, PlayerState::Empty | PlayerState::Faulted)
        {
            return Err(PlayerProtocol2Error::package());
        }
        Ok(())
    }

    fn validate_player_step(&self, step: &PlayerStepAck) -> Result<(), PlayerProtocol2Error> {
        self.validate_player_identity(&step.status)?;
        let ring_id = self
            .decoded_ring
            .as_ref()
            .map(|ring| ring.ring_id)
            .ok_or_else(PlayerProtocol2Error::lifecycle)?;
        if step.decoded_frames > MAX_DECODE_BATCH
            || (step.decoded_frames > 0
                && (step.output_ring_id != Some(ring_id) || step.output_slot_sequence == 0))
            || (step.decoded_frames == 0 && step.output_ring_id.is_some())
        {
            return Err(PlayerProtocol2Error::package());
        }
        Ok(())
    }

    fn validate_receipt(&self, receipt: &ProfileReceipt) -> Result<(), PlayerProtocol2Error> {
        receipt
            .validate()
            .map_err(|_| PlayerProtocol2Error::package())?;
        if receipt.cartridge_id != self.source.cartridge_id
            || receipt.archive_sha256 != self.source.archive_sha256
            || receipt.payload_sha256 != self.source.payload_sha256
            || receipt.pack_id != self.selection.pack_id
            || receipt.pack_version != self.selection.pack_version
            || receipt.adapter_id != self.selection.adapter_id
            || receipt.adapter_version != self.selection.adapter_version
        {
            return Err(PlayerProtocol2Error::package());
        }
        if receipt.profile_key != self.source.profile_key {
            return Err(PlayerProtocol2Error::profile());
        }
        if receipt.signal_geometry != self.source.signal_geometry {
            return Err(PlayerProtocol2Error::signal());
        }
        if receipt.tensor_abi != self.host.tensor_abi
            || receipt.decoded_abi != self.host.decoded_abi
        {
            return Err(PlayerProtocol2Error::tensor_abi());
        }
        if !contains_unique_capabilities(receipt.capabilities.as_slice(), &[Capability::Player]) {
            return Err(PlayerProtocol2Error::capability());
        }
        if receipt.estimated_host_bytes > self.host.maximum_estimated_host_bytes
            || receipt.estimated_device_bytes > self.host.maximum_estimated_device_bytes
        {
            return Err(PlayerProtocol2Error::signal());
        }
        Ok(())
    }

    fn require_state(&self, expected: PlayerProtocol2State) -> Result<(), PlayerProtocol2Error> {
        if self.state == expected {
            Ok(())
        } else {
            Err(PlayerProtocol2Error::lifecycle())
        }
    }
}

fn limited_capabilities(
    values: Vec<Capability>,
) -> Result<LimitedVec<Capability, MAX_CAPABILITIES>, PlayerProtocol2Error> {
    LimitedVec::try_from_vec(values).map_err(|_| PlayerProtocol2Error::package())
}

fn codec_loaded_matches(
    loaded: &latentdeck_control::v2::CodecLoaded,
    selection: &PlayerCodecSelection,
) -> bool {
    loaded.pack_id == selection.pack_id
        && loaded.pack_version == selection.pack_version
        && loaded.adapter_id == selection.adapter_id
        && loaded.adapter_version == selection.adapter_version
}

fn contains_unique_capabilities(actual: &[Capability], required: &[Capability]) -> bool {
    let unique = actual.iter().copied().collect::<HashSet<_>>();
    unique.len() == actual.len()
        && required
            .iter()
            .all(|capability| unique.contains(capability))
}

fn tensor_abi_is_protocol2(value: &TensorAbi) -> bool {
    value.python_major == 3
        && value.python_minor == 13
        && !value.torch_version.is_empty()
        && value.shape[0] == 1
        && value.shape[2] == 1
        && !value.shape.contains(&0)
        && value.contiguous
}

fn decoded_abi_is_protocol2(value: &DecodedAbi) -> bool {
    value.pixel_format == "rgba8" && (1..=MAX_DECODE_BATCH).contains(&value.maximum_batch)
}

#[cfg(test)]
mod tests {
    use latentdeck_control::v2::{
        CodecLoaded, CommandName, DeviceKind, SessionConfigured, SourceOpened, TensorDtype,
    };

    use super::*;

    const SYNTHETIC_PACK_ID: &str = "org.example.synthetic";
    const SYNTHETIC_PACK_VERSION: &str = "0.2.0";
    const SYNTHETIC_ADAPTER_ID: &str = "org.example.synthetic.adapter";
    const SYNTHETIC_ADAPTER_VERSION: &str = "0.2.0";

    fn id(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn selection() -> PlayerCodecSelection {
        PlayerCodecSelection {
            pack_id: SYNTHETIC_PACK_ID.to_owned(),
            pack_version: SYNTHETIC_PACK_VERSION.to_owned(),
            adapter_id: SYNTHETIC_ADAPTER_ID.to_owned(),
            adapter_version: SYNTHETIC_ADAPTER_VERSION.to_owned(),
        }
    }

    fn profile() -> ProfileKey {
        ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "test_latent".to_owned(),
            profile_version: "0.1.0".to_owned(),
        }
    }

    fn signal() -> SignalGeometry {
        SignalGeometry {
            channels: 4,
            latent_height: 8,
            latent_width: 8,
            decoded_height: 64,
            decoded_width: 64,
            frame_rate_numerator: 24,
            frame_rate_denominator: 1,
            timing_contract: "synthetic_causal".to_owned(),
            timing_contract_version: "0.1.0".to_owned(),
        }
    }

    fn tensor_abi() -> TensorAbi {
        TensorAbi {
            python_major: 3,
            python_minor: 13,
            torch_version: "2.13.0+cu130".to_owned(),
            dtype: TensorDtype::Float16,
            shape: [1, 4, 1, 8, 8],
            contiguous: true,
            device: DeviceKind::Cuda,
        }
    }

    fn decoded_abi() -> DecodedAbi {
        DecodedAbi {
            pixel_format: "rgba8".to_owned(),
            maximum_batch: MAX_DECODE_BATCH,
        }
    }

    fn host() -> PlayerHostContract {
        PlayerHostContract {
            protocol_version: PROTOCOL_VERSION,
            host_api_version: PLAYER_HOST_API_VERSION.to_owned(),
            tensor_abi: tensor_abi(),
            decoded_abi: decoded_abi(),
            maximum_estimated_host_bytes: 1 << 30,
            maximum_estimated_device_bytes: 1 << 30,
        }
    }

    fn source() -> PlayerSourceContract {
        PlayerSourceContract {
            source_id: id(2),
            cartridge_id: id(3),
            archive_sha256: "a".repeat(64),
            archive_bytes: 16_384,
            payload_sha256: "b".repeat(64),
            profile_key: profile(),
            signal_geometry: signal(),
        }
    }

    fn descriptor() -> CodecDescriptor {
        CodecDescriptor {
            pack_id: SYNTHETIC_PACK_ID.to_owned(),
            pack_version: SYNTHETIC_PACK_VERSION.to_owned(),
            adapter_id: SYNTHETIC_ADAPTER_ID.to_owned(),
            adapter_version: SYNTHETIC_ADAPTER_VERSION.to_owned(),
            host_api_version: PLAYER_HOST_API_VERSION.to_owned(),
            capabilities: limited_capabilities(Capability::REQUIRED_CODEC_V2.to_vec())
                .expect("capabilities"),
            profiles: LimitedVec::try_from_vec(vec![profile()]).expect("profiles"),
        }
    }

    fn inspection() -> ProfileInspection {
        let source = source();
        ProfileInspection {
            source_id: source.source_id,
            cartridge_id: source.cartridge_id,
            archive_sha256: source.archive_sha256,
            payload_sha256: source.payload_sha256,
            profile_key: source.profile_key,
            signal_geometry: source.signal_geometry,
        }
    }

    fn receipt() -> ProfileReceipt {
        let source = source();
        ProfileReceipt {
            receipt_id: id(4),
            cartridge_id: source.cartridge_id,
            archive_sha256: source.archive_sha256,
            payload_sha256: source.payload_sha256,
            pack_id: SYNTHETIC_PACK_ID.to_owned(),
            pack_version: SYNTHETIC_PACK_VERSION.to_owned(),
            adapter_id: SYNTHETIC_ADAPTER_ID.to_owned(),
            adapter_version: SYNTHETIC_ADAPTER_VERSION.to_owned(),
            profile_key: source.profile_key,
            signal_geometry: source.signal_geometry,
            tensor_abi: tensor_abi(),
            decoded_abi: decoded_abi(),
            capabilities: limited_capabilities(vec![Capability::Player]).expect("capabilities"),
            estimated_host_bytes: 4_096,
            estimated_device_bytes: 8_192,
        }
    }

    fn bridge() -> PlayerProtocol2Bridge {
        PlayerProtocol2Bridge::new("0.2.0", id(1), selection(), host(), source(), true)
            .expect("bridge")
    }

    fn accept_session_and_descriptor(
        bridge: &mut PlayerProtocol2Bridge,
        descriptor: CodecDescriptor,
    ) -> Result<(), PlayerProtocol2Error> {
        bridge.accept_ack(Ack::SessionConfigure(configured(PROTOCOL_VERSION)))?;
        bridge.accept_ack(Ack::CodecDescriptor(descriptor))?;
        Ok(())
    }

    fn configured(selected_protocol_version: u16) -> SessionConfigured {
        SessionConfigured {
            selected_protocol_version,
            maximum_frame_bytes: u32::try_from(MAX_FRAME_BYTES).expect("frame bound"),
            accepted_capabilities: limited_capabilities(vec![Capability::Player])
                .expect("capabilities"),
        }
    }

    fn source_opened() -> SourceOpened {
        let source = source();
        SourceOpened {
            source_id: source.source_id,
            cartridge_id: source.cartridge_id,
            archive_sha256: source.archive_sha256,
        }
    }

    fn bridge_at_source_inspected() -> PlayerProtocol2Bridge {
        let mut bridge = bridge();
        accept_session_and_descriptor(&mut bridge, descriptor()).expect("descriptor");
        bridge
            .accept_ack(Ack::SourceOpen(source_opened()))
            .expect("source open");
        bridge
            .accept_ack(Ack::ProfileInspect(inspection()))
            .expect("inspection");
        bridge
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one end-to-end contract test keeps the complete ordered synthetic lifecycle visible"
    )]
    fn synthetic_non_h3_adapter_completes_generic_player_lifecycle() {
        let mut bridge = bridge();
        assert_eq!(
            bridge
                .session_configure_command()
                .expect("configure")
                .name(),
            CommandName::SessionConfigure
        );
        bridge
            .accept_ack(Ack::SessionConfigure(configured(PROTOCOL_VERSION)))
            .expect("session reply");
        assert_eq!(
            bridge
                .codec_descriptor_command()
                .expect("descriptor")
                .name(),
            CommandName::CodecDescriptor
        );
        bridge
            .accept_ack(Ack::CodecDescriptor(descriptor()))
            .expect("descriptor reply");
        assert_eq!(
            bridge
                .source_open_command(0x1000, "{\"access_abi_version\":1}".to_owned())
                .expect("source open")
                .name(),
            CommandName::SourceOpen
        );
        bridge
            .accept_ack(Ack::SourceOpen(source_opened()))
            .expect("source-open reply");
        assert_eq!(
            bridge.profile_inspect_command().expect("inspect").name(),
            CommandName::ProfileInspect
        );
        bridge
            .accept_ack(Ack::ProfileInspect(inspection()))
            .expect("inspection reply");
        assert_eq!(
            bridge.profile_validate_command().expect("validate").name(),
            CommandName::ProfileValidate
        );
        let PlayerProtocol2ReplyOutcome::GpuAllocationPermitted(permit) = bridge
            .accept_ack(Ack::ProfileValidate(Box::new(receipt())))
            .expect("receipt")
        else {
            panic!("validated receipt must create the GPU allocation permit");
        };
        assert_eq!(bridge.state(), PlayerProtocol2State::ProfileValidated);

        let codec_load = bridge
            .codec_load_command(permit, 0, Vec::new())
            .expect("codec load after receipt validation");
        assert_eq!(codec_load.name(), CommandName::CodecLoad);
        bridge
            .accept_ack(Ack::CodecLoad(CodecLoaded {
                pack_id: SYNTHETIC_PACK_ID.to_owned(),
                pack_version: SYNTHETIC_PACK_VERSION.to_owned(),
                adapter_id: SYNTHETIC_ADAPTER_ID.to_owned(),
                adapter_version: SYNTHETIC_ADAPTER_VERSION.to_owned(),
                device: DeviceKind::Cuda,
                device_ordinal: 0,
            }))
            .expect("codec loaded");
        let ring_id = id(5);
        assert_eq!(
            bridge
                .ring_configure_command(ring_id, 0x2000, 0x3000, 0x4000, 4, 64 * 64 * 4)
                .expect("ring")
                .name(),
            CommandName::RingConfigure
        );
        bridge
            .accept_ack(Ack::RingConfigure(RingConfigured {
                ring_id,
                kind: RingKind::DecodedRgba,
                slot_count: 4,
                slot_bytes: 64 * 64 * 4,
            }))
            .expect("ring configured");
        assert_eq!(
            bridge.player_open_command().expect("player open").name(),
            CommandName::PlayerOpen
        );
        bridge
            .accept_ack(Ack::PlayerOpen(PlayerStatusSnapshot {
                player_session_id: id(1),
                state: PlayerState::Ready,
                stream_generation: 1,
                stream_sequence: 0,
                playhead_slot: 0,
                end_of_stream: false,
                decoded_ring_id: Some(ring_id),
            }))
            .expect("player opened");
        assert_eq!(
            bridge.player_step_command(8).expect("step").name(),
            CommandName::PlayerStep
        );
        bridge
            .accept_ack(Ack::PlayerStep(PlayerStepAck {
                status: PlayerStatusSnapshot {
                    player_session_id: id(1),
                    state: PlayerState::Playing,
                    stream_generation: 1,
                    stream_sequence: 1,
                    playhead_slot: 1,
                    end_of_stream: false,
                    decoded_ring_id: Some(ring_id),
                },
                output_ring_id: Some(ring_id),
                output_slot_sequence: 1,
                decoded_frames: 1,
            }))
            .expect("player step reply");
        assert_eq!(
            bridge.player_reset_command().expect("reset").name(),
            CommandName::PlayerReset
        );
        bridge
            .accept_ack(Ack::PlayerReset(PlayerStatusSnapshot {
                player_session_id: id(1),
                state: PlayerState::Ready,
                stream_generation: 2,
                stream_sequence: 0,
                playhead_slot: 0,
                end_of_stream: false,
                decoded_ring_id: Some(ring_id),
            }))
            .expect("player reset reply");
        assert_eq!(bridge.state(), PlayerProtocol2State::PlayerOpened);
    }

    #[test]
    fn unsupported_protocol_is_rejected_without_advancing_lifecycle() {
        let mut bridge = bridge();
        let error = bridge
            .accept_ack(Ack::SessionConfigure(configured(1)))
            .expect_err("P1 reply cannot configure P2");
        assert_eq!(error.code, PlayerProtocol2ErrorCode::UnsupportedProtocol);
        assert_eq!(bridge.state(), PlayerProtocol2State::New);
    }

    #[test]
    fn unsupported_host_api_is_stable_and_does_not_reach_profile_inspection() {
        let mut bridge = bridge();
        bridge
            .accept_ack(Ack::SessionConfigure(configured(PROTOCOL_VERSION)))
            .expect("session");
        let mut incompatible = descriptor();
        incompatible.host_api_version = "1.0".to_owned();
        let error = bridge
            .accept_ack(Ack::CodecDescriptor(incompatible))
            .expect_err("host API mismatch");
        assert_eq!(error.code.as_str(), "unsupported_host_api");
        assert_eq!(bridge.state(), PlayerProtocol2State::SessionConfigured);
    }

    #[test]
    fn unsupported_tensor_abi_is_rejected_before_gpu_allocation() {
        let mut bridge = bridge_at_source_inspected();
        let mut incompatible = receipt();
        incompatible.tensor_abi.torch_version = "2.12.0+cu128".to_owned();
        let error = bridge
            .accept_ack(Ack::ProfileValidate(Box::new(incompatible)))
            .expect_err("Torch build mismatch");
        assert_eq!(error.code.as_str(), "unsupported_tensor_abi");
        assert_eq!(bridge.state(), PlayerProtocol2State::SourceInspected);
    }

    #[test]
    fn unsupported_profile_is_rejected_without_hidden_conversion() {
        let mut bridge = bridge();
        bridge
            .accept_ack(Ack::SessionConfigure(configured(PROTOCOL_VERSION)))
            .expect("session");
        let mut incompatible = descriptor();
        incompatible.profiles = LimitedVec::try_from_vec(vec![ProfileKey {
            codec_family: "other".to_owned(),
            profile: "other".to_owned(),
            profile_version: "1.0.0".to_owned(),
        }])
        .expect("profiles");
        let error = bridge
            .accept_ack(Ack::CodecDescriptor(incompatible))
            .expect_err("profile mismatch");
        assert_eq!(error.code.as_str(), "unsupported_profile");
    }

    #[test]
    fn unsupported_signal_is_rejected_before_codec_load_command_exists() {
        let mut bridge = bridge_at_source_inspected();
        let mut incompatible = receipt();
        incompatible.signal_geometry.frame_rate_numerator = 30;
        incompatible.tensor_abi = tensor_abi();
        let error = bridge
            .accept_ack(Ack::ProfileValidate(Box::new(incompatible)))
            .expect_err("signal mismatch");
        assert_eq!(error.code.as_str(), "unsupported_signal");
        assert_eq!(bridge.state(), PlayerProtocol2State::SourceInspected);
    }

    #[test]
    fn unsupported_capability_is_rejected_at_descriptor_and_receipt_boundaries() {
        let mut bridge = bridge();
        bridge
            .accept_ack(Ack::SessionConfigure(configured(PROTOCOL_VERSION)))
            .expect("session");
        let mut incomplete = descriptor();
        incomplete.capabilities = limited_capabilities(vec![
            Capability::Player,
            Capability::Realtime,
            Capability::Resample,
            Capability::SnapshotCapture,
        ])
        .expect("capabilities");
        let error = bridge
            .accept_ack(Ack::CodecDescriptor(incomplete))
            .expect_err("full Codec Pack v2 capability set is mandatory");
        assert_eq!(error.code.as_str(), "unsupported_capability");

        let mut bridge = bridge_at_source_inspected();
        let mut incomplete = receipt();
        incomplete.capabilities =
            limited_capabilities(vec![Capability::Realtime]).expect("bounded capabilities");
        let error = bridge
            .accept_ack(Ack::ProfileValidate(Box::new(incomplete)))
            .expect_err("profile needs Player capability");
        assert_eq!(error.code.as_str(), "unsupported_capability");
    }

    #[derive(Default)]
    struct CountingLauncher {
        p1_calls: usize,
        p2_calls: usize,
        fail_p2: bool,
    }

    impl ExplicitPlayerBridgeLauncher for CountingLauncher {
        type Output = PlayerBridgeSelection;
        type Error = &'static str;

        fn launch_protocol1_h3(&mut self) -> Result<Self::Output, Self::Error> {
            self.p1_calls += 1;
            Ok(PlayerBridgeSelection::Protocol1H3)
        }

        fn launch_protocol2(&mut self) -> Result<Self::Output, Self::Error> {
            self.p2_calls += 1;
            if self.fail_p2 {
                Err("p2.failed")
            } else {
                Ok(PlayerBridgeSelection::Protocol2)
            }
        }
    }

    #[test]
    fn protocol1_bridge_runs_only_when_selected_explicitly() {
        let mut launcher = CountingLauncher::default();
        let selected =
            launch_explicit_player_bridge(PlayerBridgeSelection::Protocol1H3, &mut launcher)
                .expect("explicit P1");
        assert_eq!(selected, PlayerBridgeSelection::Protocol1H3);
        assert_eq!(launcher.p1_calls, 1);
        assert_eq!(launcher.p2_calls, 0);
    }

    #[test]
    fn protocol2_failure_never_falls_back_to_protocol1() {
        let mut launcher = CountingLauncher {
            fail_p2: true,
            ..CountingLauncher::default()
        };
        let error = launch_explicit_player_bridge(PlayerBridgeSelection::Protocol2, &mut launcher)
            .expect_err("P2 error must surface");
        assert_eq!(error, "p2.failed");
        assert_eq!(launcher.p2_calls, 1);
        assert_eq!(launcher.p1_calls, 0);
    }

    #[test]
    fn control_plane_never_contains_tensor_or_rgba_payload_bytes() {
        let contract = PlayerProtocol2Bridge::transport_contract();
        assert_eq!(contract.maximum_control_frame_bytes, MAX_FRAME_BYTES);
        assert_eq!(
            contract.tensor_transport,
            PlayerBulkTransport::SharedTensorHandles
        );
        assert_eq!(
            contract.decoded_transport,
            PlayerBulkTransport::DecodedRgbaRing
        );

        let commands = [bridge().session_configure_command().expect("configure"), {
            let mut value = bridge();
            value
                .accept_ack(Ack::SessionConfigure(configured(PROTOCOL_VERSION)))
                .expect("session");
            value.codec_descriptor_command().expect("descriptor")
        }];
        let replies = [
            Ack::CodecDescriptor(descriptor()),
            Ack::ProfileInspect(inspection()),
            Ack::ProfileValidate(Box::new(receipt())),
        ];
        let encoded = format!(
            "{}{}",
            serde_json::to_string(&commands).expect("serialize commands"),
            serde_json::to_string(&replies).expect("serialize replies")
        );
        for forbidden in ["tensor_bytes", "rgba_bytes", "pixels", "payload_bytes"] {
            assert!(!encoded.contains(forbidden), "found {forbidden} in IPC");
        }
    }

    #[test]
    fn codec_load_cannot_be_constructed_before_validated_receipt() {
        let mut bridge = bridge_at_source_inspected();
        let forged = GpuAllocationPermit { receipt_id: id(99) };
        let error = bridge
            .codec_load_command(forged, 0, Vec::new())
            .expect_err("pre-receipt GPU load must be impossible");
        assert_eq!(error.code, PlayerProtocol2ErrorCode::InvalidLifecycle);

        let PlayerProtocol2ReplyOutcome::GpuAllocationPermitted(permit) = bridge
            .accept_ack(Ack::ProfileValidate(Box::new(receipt())))
            .expect("receipt")
        else {
            panic!("missing permit");
        };
        let wrong = GpuAllocationPermit { receipt_id: id(99) };
        assert!(bridge.codec_load_command(wrong, 0, Vec::new()).is_err());
        assert!(bridge.codec_load_command(permit, 0, Vec::new()).is_ok());
    }
}
