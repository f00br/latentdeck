#![cfg(windows)]

use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use latentdeck_control::v2::{
    Ack, Capability, CodecDescriptor, CodecLoaded, Command, CommandName, DecodedAbi, DeviceKind,
    LimitedVec, MAX_CAPABILITIES, MAX_FRAME_BYTES, MAX_PROFILES, PROTOCOL_VERSION, PlayerState,
    PlayerStatusSnapshot, ProfileInspection, ProfileKey, ProfileReceipt, RingConfigured, RingKind,
    SessionConfigured, SignalGeometry, SourceOpened, TensorAbi, TensorDtype,
};
use latentdeck_core::player_session_v2::{
    PlayerCodecSelectionV2, PlayerSessionV2HostContract, PlayerSessionV2PreparedRing,
    PlayerSessionV2SourceIdentity, PlayerSessionV2StartupIo, orchestrate_player_session_v2_startup,
};
use latentdeck_gpu::ring_v2::RingV2Descriptor;
use uuid::Uuid;

const SOURCE_ID: Uuid = Uuid::from_u128(0x10);
const CARTRIDGE_ID: Uuid = Uuid::from_u128(0x20);
const PLAYER_ID: Uuid = Uuid::from_u128(0x30);
const RING_ID: Uuid = Uuid::from_u128(0x40);
const RECEIPT_ID: Uuid = Uuid::from_u128(0x50);
const ARCHIVE_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PAYLOAD_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct FakeIo {
    calls: Vec<CommandName>,
    selection: PlayerCodecSelectionV2,
    source: PlayerSessionV2SourceIdentity,
    host: PlayerSessionV2HostContract,
    corrupt_receipt: bool,
    codec_load_reply_device_ordinal: Option<u8>,
    fail_player_open: bool,
    dropped_ring_endpoints: Arc<AtomicUsize>,
    dropped_io: Arc<AtomicUsize>,
}

impl Drop for FakeIo {
    fn drop(&mut self) {
        self.dropped_io.fetch_add(1, Ordering::SeqCst);
    }
}

struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

impl PlayerSessionV2StartupIo for FakeIo {
    type RingOwner = DropProbe;
    type RingConsumer = DropProbe;

    fn call(
        &mut self,
        command: Command,
        _timeout: Duration,
    ) -> impl Future<Output = Result<Ack, latentdeck_core::player_session_v2::PlayerSessionV2Error>>
    {
        if let Command::PlayerOpen(open) = &command {
            assert!(
                !open.source.loop_enabled,
                "Player source binding must keep worker auto-loop disabled"
            );
        }
        self.calls.push(command.name());
        let reply = match command {
            Command::SessionConfigure(_) => Ack::SessionConfigure(SessionConfigured {
                selected_protocol_version: PROTOCOL_VERSION,
                maximum_frame_bytes: u32::try_from(MAX_FRAME_BYTES).expect("frame bound"),
                accepted_capabilities: capabilities(),
            }),
            Command::CodecDescriptor(_) => Ack::CodecDescriptor(CodecDescriptor {
                pack_id: self.selection.pack_id.clone(),
                pack_version: self.selection.pack_version.clone(),
                adapter_id: self.selection.adapter_id.clone(),
                adapter_version: self.selection.adapter_version.clone(),
                host_api_version: "2.0".to_owned(),
                capabilities: capabilities(),
                profiles: LimitedVec::<_, MAX_PROFILES>::try_from_vec(vec![
                    self.host.profile_key.clone(),
                ])
                .expect("bounded profiles"),
            }),
            Command::SourceOpen(open) => {
                assert_eq!(open.source_id, SOURCE_ID);
                assert_eq!(open.retained_native_handle, 99);
                Ack::SourceOpen(SourceOpened {
                    source_id: self.source.source_id,
                    cartridge_id: self.source.cartridge_id,
                    archive_sha256: self.source.archive_sha256.clone(),
                })
            }
            Command::ProfileInspect(_) => Ack::ProfileInspect(ProfileInspection {
                source_id: self.source.source_id,
                cartridge_id: self.source.cartridge_id,
                archive_sha256: self.source.archive_sha256.clone(),
                payload_sha256: self.source.payload_sha256.clone(),
                profile_key: self.host.profile_key.clone(),
                signal_geometry: self.host.signal_geometry.clone(),
            }),
            Command::ProfileValidate(_) => Ack::ProfileValidate(Box::new(ProfileReceipt {
                receipt_id: RECEIPT_ID,
                cartridge_id: self.source.cartridge_id,
                archive_sha256: self.source.archive_sha256.clone(),
                payload_sha256: if self.corrupt_receipt {
                    "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_owned()
                } else {
                    self.source.payload_sha256.clone()
                },
                pack_id: self.selection.pack_id.clone(),
                pack_version: self.selection.pack_version.clone(),
                adapter_id: self.selection.adapter_id.clone(),
                adapter_version: self.selection.adapter_version.clone(),
                profile_key: self.host.profile_key.clone(),
                signal_geometry: self.host.signal_geometry.clone(),
                tensor_abi: self.host.tensor_abi.clone(),
                decoded_abi: self.host.decoded_abi.clone(),
                capabilities: capabilities(),
                estimated_host_bytes: 1024,
                estimated_device_bytes: 0,
            })),
            Command::CodecLoad(load) => Ack::CodecLoad(CodecLoaded {
                pack_id: load.pack_id,
                pack_version: load.pack_version,
                adapter_id: load.adapter_id,
                adapter_version: load.adapter_version,
                device: load.device,
                device_ordinal: self
                    .codec_load_reply_device_ordinal
                    .unwrap_or(load.device_ordinal),
            }),
            Command::RingConfigure(configure) => Ack::RingConfigure(RingConfigured {
                ring_id: configure.ring_id,
                kind: configure.kind,
                slot_count: configure.slot_count,
                slot_bytes: configure.slot_bytes,
            }),
            Command::PlayerOpen(open) => Ack::PlayerOpen(PlayerStatusSnapshot {
                player_session_id: open.player_session_id,
                state: PlayerState::Ready,
                stream_generation: open.stream_generation,
                stream_sequence: 0,
                playhead_slot: 0,
                end_of_stream: false,
                decoded_ring_id: Some(if self.fail_player_open {
                    Uuid::from_u128(0x41)
                } else {
                    RING_ID
                }),
            }),
            other => panic!("unexpected startup command {:?}", other.name()),
        };
        async move { Ok(reply) }
    }

    fn prepare_source_open(
        &self,
        source: &PlayerSessionV2SourceIdentity,
    ) -> Result<Command, latentdeck_core::player_session_v2::PlayerSessionV2Error> {
        Ok(Command::SourceOpen(latentdeck_control::v2::SourceOpen {
            source_id: source.source_id,
            cartridge_id: source.cartridge_id,
            archive_sha256: source.archive_sha256.clone(),
            archive_bytes: source.archive_bytes,
            retained_native_handle: 99,
            integrity_access_receipt: "native-receipt".to_owned(),
        }))
    }

    fn prepare_decoded_ring(
        &self,
        descriptor: RingV2Descriptor,
        ring_id: Uuid,
    ) -> Result<
        PlayerSessionV2PreparedRing<Self::RingOwner, Self::RingConsumer>,
        latentdeck_core::player_session_v2::PlayerSessionV2Error,
    > {
        let layout = descriptor.layout();
        Ok(PlayerSessionV2PreparedRing::new(
            Command::RingConfigure(latentdeck_control::v2::RingConfigure {
                ring_id,
                kind: RingKind::DecodedRgba,
                mapping_handle: 101,
                ready_event_handle: 102,
                consumed_event_handle: 103,
                slot_count: u8::try_from(layout.slot_count()).expect("slot count"),
                slot_bytes: layout.slot_bytes(),
            }),
            DropProbe(Arc::clone(&self.dropped_ring_endpoints)),
            DropProbe(Arc::clone(&self.dropped_ring_endpoints)),
        ))
    }
}

#[tokio::test]
async fn synthetic_non_h3_startup_is_strictly_ordered_and_gpu_load_is_receipt_gated() {
    let selection = selection();
    let source = source();
    let host = host();
    let dropped_ring_endpoints = Arc::new(AtomicUsize::new(0));
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: false,
        codec_load_reply_device_ordinal: None,
        fail_player_open: false,
        dropped_ring_endpoints: Arc::clone(&dropped_ring_endpoints),
        dropped_io: Arc::new(AtomicUsize::new(0)),
    };

    let negotiated =
        orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[])
            .await
            .expect("synthetic codec startup");

    assert_eq!(
        io.calls,
        [
            CommandName::SessionConfigure,
            CommandName::CodecDescriptor,
            CommandName::SourceOpen,
            CommandName::ProfileInspect,
            CommandName::ProfileValidate,
            CommandName::CodecLoad,
            CommandName::RingConfigure,
            CommandName::PlayerOpen,
        ]
    );
    assert_eq!(negotiated.profile_receipt().receipt_id, RECEIPT_ID);
    assert_eq!(dropped_ring_endpoints.load(Ordering::SeqCst), 0);
    drop(negotiated.into_ring_parts());
    assert_eq!(dropped_ring_endpoints.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn codec_load_rejects_a_worker_ack_for_another_device_ordinal() {
    let selection = selection();
    let source = source();
    let host = host();
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: false,
        codec_load_reply_device_ordinal: Some(1),
        fail_player_open: false,
        dropped_ring_endpoints: Arc::new(AtomicUsize::new(0)),
        dropped_io: Arc::new(AtomicUsize::new(0)),
    };

    let Err(error) =
        orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[]).await
    else {
        panic!("a codec.load acknowledgement for another device ordinal must fail closed");
    };

    assert!(matches!(
        error,
        latentdeck_core::player_session_v2::PlayerSessionV2Error::PackageMismatch
    ));
    assert_eq!(io.calls.last(), Some(&CommandName::CodecLoad));
    assert!(!io.calls.contains(&CommandName::RingConfigure));
}

#[tokio::test]
async fn codec_load_is_never_emitted_after_a_malicious_profile_receipt() {
    let selection = selection();
    let source = source();
    let host = host();
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: true,
        codec_load_reply_device_ordinal: None,
        fail_player_open: false,
        dropped_ring_endpoints: Arc::new(AtomicUsize::new(0)),
        dropped_io: Arc::new(AtomicUsize::new(0)),
    };

    let Err(error) =
        orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[]).await
    else {
        panic!("mismatched receipt must be rejected");
    };

    assert!(matches!(
        error,
        latentdeck_core::player_session_v2::PlayerSessionV2Error::PackageMismatch
    ));
    assert_eq!(
        io.calls,
        [
            CommandName::SessionConfigure,
            CommandName::CodecDescriptor,
            CommandName::SourceOpen,
            CommandName::ProfileInspect,
            CommandName::ProfileValidate,
        ]
    );
}

#[tokio::test]
async fn cpu_codec_accepts_zero_device_budget_and_zero_device_estimate() {
    let selection = selection();
    let source = source();
    let mut host = host();
    host.maximum_estimated_device_bytes = 0;
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: false,
        codec_load_reply_device_ordinal: None,
        fail_player_open: false,
        dropped_ring_endpoints: Arc::new(AtomicUsize::new(0)),
        dropped_io: Arc::new(AtomicUsize::new(0)),
    };

    orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[])
        .await
        .expect("CPU-only codec uses no device memory");

    assert!(io.calls.contains(&CommandName::CodecLoad));
}

#[tokio::test]
async fn cuda_codec_rejects_zero_device_budget_before_any_worker_command() {
    let selection = selection();
    let source = source();
    let mut host = host();
    host.tensor_abi.device = DeviceKind::Cuda;
    host.maximum_estimated_device_bytes = 0;
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: false,
        codec_load_reply_device_ordinal: None,
        fail_player_open: false,
        dropped_ring_endpoints: Arc::new(AtomicUsize::new(0)),
        dropped_io: Arc::new(AtomicUsize::new(0)),
    };

    let Err(error) =
        orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[]).await
    else {
        panic!("CUDA with a zero device budget must fail closed");
    };
    assert!(matches!(
        error,
        latentdeck_core::player_session_v2::PlayerSessionV2Error::InvalidHostContract(_)
    ));
    assert!(io.calls.is_empty());
}

#[tokio::test]
async fn cuda_codec_rejects_zero_device_estimate_before_codec_load() {
    let selection = selection();
    let source = source();
    let mut host = host();
    host.tensor_abi.device = DeviceKind::Cuda;
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: false,
        codec_load_reply_device_ordinal: None,
        fail_player_open: false,
        dropped_ring_endpoints: Arc::new(AtomicUsize::new(0)),
        dropped_io: Arc::new(AtomicUsize::new(0)),
    };

    let Err(error) =
        orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[]).await
    else {
        panic!("CUDA with a zero device estimate must fail closed");
    };
    assert!(matches!(
        error,
        latentdeck_core::player_session_v2::PlayerSessionV2Error::SignalMismatch
    ));
    assert_eq!(io.calls.last(), Some(&CommandName::ProfileValidate));
    assert!(!io.calls.contains(&CommandName::CodecLoad));
}

#[tokio::test]
async fn startup_failure_after_ring_transfer_drops_both_host_ring_endpoints() {
    let selection = selection();
    let source = source();
    let host = host();
    let dropped_ring_endpoints = Arc::new(AtomicUsize::new(0));
    let dropped_io = Arc::new(AtomicUsize::new(0));
    let mut io = FakeIo {
        calls: Vec::new(),
        selection: selection.clone(),
        source: source.clone(),
        host: host.clone(),
        corrupt_receipt: false,
        codec_load_reply_device_ordinal: None,
        fail_player_open: true,
        dropped_ring_endpoints: Arc::clone(&dropped_ring_endpoints),
        dropped_io: Arc::clone(&dropped_io),
    };

    let Err(error) =
        orchestrate_player_session_v2_startup(&mut io, &selection, &source, &host, &[]).await
    else {
        panic!("mismatched player.open acknowledgement must fail");
    };

    assert!(matches!(
        error,
        latentdeck_core::player_session_v2::PlayerSessionV2Error::PackageMismatch
    ));
    assert_eq!(dropped_ring_endpoints.load(Ordering::SeqCst), 2);
    assert_eq!(io.calls.last(), Some(&CommandName::PlayerOpen));
    assert_eq!(dropped_io.load(Ordering::SeqCst), 0);
    drop(io);
    assert_eq!(dropped_io.load(Ordering::SeqCst), 1);
}

fn selection() -> PlayerCodecSelectionV2 {
    PlayerCodecSelectionV2 {
        pack_id: "dev.synthetic.codec".to_owned(),
        pack_version: "2.0.0".to_owned(),
        adapter_id: "dev.synthetic.adapter".to_owned(),
        adapter_version: "2.0.0".to_owned(),
    }
}

fn source() -> PlayerSessionV2SourceIdentity {
    PlayerSessionV2SourceIdentity {
        source_id: SOURCE_ID,
        cartridge_id: CARTRIDGE_ID,
        archive_sha256: ARCHIVE_SHA.to_owned(),
        archive_bytes: 4096,
        payload_sha256: PAYLOAD_SHA.to_owned(),
    }
}

fn host() -> PlayerSessionV2HostContract {
    PlayerSessionV2HostContract {
        app_version: "0.1.0".to_owned(),
        player_session_id: PLAYER_ID,
        ring_id: RING_ID,
        profile_key: ProfileKey {
            codec_family: "synthetic".to_owned(),
            profile: "non_h3".to_owned(),
            profile_version: "1.0.0".to_owned(),
        },
        signal_geometry: SignalGeometry {
            channels: 8,
            latent_height: 4,
            latent_width: 6,
            decoded_height: 16,
            decoded_width: 24,
            frame_rate_numerator: 24,
            frame_rate_denominator: 1,
            timing_contract: "synthetic_slots".to_owned(),
            timing_contract_version: "1.0.0".to_owned(),
        },
        tensor_abi: TensorAbi {
            python_major: 3,
            python_minor: 13,
            torch_version: "2.13.0+cpu".to_owned(),
            dtype: TensorDtype::Float32,
            shape: [1, 8, 1, 4, 6],
            contiguous: true,
            device: DeviceKind::Cpu,
        },
        decoded_abi: DecodedAbi {
            pixel_format: "rgba8".to_owned(),
            maximum_batch: 4,
        },
        maximum_estimated_host_bytes: 1 << 20,
        maximum_estimated_device_bytes: 1 << 20,
        device_ordinal: 0,
        ring_slot_count: 3,
        stream_generation: 1,
        loop_enabled: true,
        heartbeat_interval_ms: 1_000,
        heartbeat_hard_timeout_ms: 10_000,
        command_timeout: Duration::from_secs(2),
    }
}

fn capabilities() -> LimitedVec<Capability, MAX_CAPABILITIES> {
    LimitedVec::try_from_vec(Capability::REQUIRED_CODEC_V2.to_vec()).expect("bounded capabilities")
}
