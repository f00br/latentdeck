//! Private opt-in proof of the real H3 worker and anonymous RGB ring.
//!
//! No fixture path or payload is embedded in the public test. Run explicitly:
//! `LATENTDECK_PRIVATE_CODEC_ROOT`, `LATENTDECK_PRIVATE_CARTRIDGE`, and
//! `LATENTDECK_PRIVATE_TAEH3` must point to owner-controlled local data.

use std::{env, path::PathBuf, time::Duration};

use latentdeck_control::{
    Ack, BoundedVec, CodecLoad, Command, EmptyPayload, ExternalAssetBinding, ProfileRef, RingBind,
    SessionConfigure, ShutdownReason, SlotLoad, WORKER_PROTOCOL_VERSION, WireUuid,
};
use latentdeck_core::{
    playback_schedule::PlaybackSchedule,
    player::PlayerCoordinator,
    worker_client::WorkerClient,
    worker_supervisor::{ValidatedWorkerLaunch, spawn_worker},
};
use latentdeck_gpu::{
    ring::{ReadStatus, RingDescriptor},
    windows_ring::{FramesReady, WindowsRgbRingOwner},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SLOT_ID: &str = "private-player-proof";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires private LC/weight, installed linked Codec Pack, CUDA, and an NVIDIA GPU"]
#[allow(
    clippy::too_many_lines,
    reason = "linear private end-to-end contract proof"
)]
async fn real_h3_worker_decodes_resets_and_republishes_rgba() {
    let codec_root = required_path("LATENTDECK_PRIVATE_CODEC_ROOT");
    let cartridge_path = required_path("LATENTDECK_PRIVATE_CARTRIDGE");
    let decoder_path = required_path("LATENTDECK_PRIVATE_TAEH3");

    let mut coordinator =
        PlayerCoordinator::discover(&[codec_root], latentdeck_core::product_version())
            .expect("private Codec Pack must validate");
    coordinator
        .select_decoder_asset(&decoder_path)
        .expect("private decoder selection must match pack receipt");
    coordinator
        .open_cartridge(&cartridge_path)
        .expect("private cartridge must pass the full LC validator");
    let launch = coordinator.launch_inputs().expect("trusted launch inputs");
    let pack = launch.codec_pack.clone();
    let asset = launch.decoder_asset.clone();
    let cartridge = launch.cartridge.clone();
    let cartridge_path = launch.cartridge_path.to_path_buf();

    let pending = spawn_worker(ValidatedWorkerLaunch::from_codec_pack(&pack))
        .await
        .expect("worker spawn");
    let session = pending.connect().await.expect("authenticated worker hello");
    let mut client = WorkerClient::new(session);

    expect_ack(
        &client
            .call(
                Command::SessionConfigure(SessionConfigure {
                    selected_protocol_version: WORKER_PROTOCOL_VERSION,
                    app_version: latentdeck_core::product_version().to_owned(),
                    heartbeat_interval_ms: 500,
                    heartbeat_hard_timeout_ms: 2_000,
                    max_frame_bytes: latentdeck_control::MAX_CONTROL_FRAME_BYTES,
                    max_inflight_decode_batches: 1,
                }),
                Duration::from_secs(5),
            )
            .await
            .expect("session.configure"),
        "session.configure",
        |ack| matches!(ack, Ack::SessionConfigure(_)),
    );

    let inspection = client
        .call(
            Command::CodecInspect(EmptyPayload::default()),
            Duration::from_secs(10),
        )
        .await
        .expect("codec.inspect");
    let Ack::CodecInspect(inspection) = inspection else {
        panic!("codec.inspect returned wrong acknowledgement");
    };
    assert!(inspection.cuda_available, "private H3 proof requires CUDA");
    assert!(inspection.devices.iter().any(|device| device.ordinal == 0));

    let profile = ProfileRef {
        codec_family: "minimax_h3".to_owned(),
        profile: "h3_av_latent".to_owned(),
        profile_version: "0.1.0".to_owned(),
    };
    let assets = BoundedVec::try_from_vec(vec![ExternalAssetBinding {
        asset_id: asset.asset_id.clone(),
        path: asset.path.to_string_lossy().into_owned(),
        sha256: asset.sha256.clone(),
        byte_length: asset.byte_length,
    }])
    .expect("one bounded external asset");
    expect_ack(
        &client
            .call(
                Command::CodecLoad(CodecLoad {
                    pack_id: pack.manifest.pack_id.clone(),
                    pack_version: pack.manifest.pack_version.clone(),
                    adapter_id: pack.manifest.adapter.adapter_id.clone(),
                    profile,
                    device_ordinal: 0,
                    assets,
                }),
                COMMAND_TIMEOUT,
            )
            .await
            .expect("codec.load"),
        "codec.load",
        |ack| matches!(ack, Ack::CodecLoad(_)),
    );

    let cartridge_uuid = uuid::Uuid::parse_str(&cartridge.cartridge_id)
        .map(WireUuid::from_uuid)
        .expect("validated LC cartridge ID is a UUID");
    let slot = client
        .call(
            Command::SlotLoad(SlotLoad {
                slot_id: SLOT_ID.to_owned(),
                cartridge_path: cartridge_path.to_string_lossy().into_owned(),
                cartridge_id: cartridge_uuid,
                expected_archive_sha256: cartridge.archive_sha256.clone(),
                stream_generation: 1,
            }),
            Duration::from_secs(20),
        )
        .await
        .expect("slot.load");
    let Ack::SlotLoad(slot) = slot else {
        panic!("slot.load returned wrong acknowledgement");
    };
    assert_eq!(
        (slot.width, slot.height),
        (cartridge.width, cartridge.height)
    );
    assert_eq!(slot.timing.decoded_frame_count, cartridge.frame_count);

    let descriptor = RingDescriptor::new(slot.width, slot.height, 1).expect("bounded RGB ring");
    let mut owner = WindowsRgbRingOwner::create(descriptor).expect("anonymous RGB ring");
    let mut consumer = owner.open_consumer().expect("sole native consumer");
    let binding = client
        .with_process_handle(|process| owner.duplicate_into(process))
        .expect("live authenticated process handle")
        .expect("duplicate anonymous mapping and event into worker");
    let ring_id = WireUuid::new_v4();
    let bound = client
        .call(
            Command::RingBind(RingBind {
                layout_version: 1,
                mapping_handle: binding.mapping_handle(),
                mapping_bytes: binding.mapping_bytes(),
                frames_ready_event_handle: binding.frames_ready_event_handle(),
                ring_id,
            }),
            Duration::from_secs(10),
        )
        .await
        .expect("ring.bind");
    let Ack::RingBind(bound) = bound else {
        panic!("ring.bind returned wrong acknowledgement");
    };
    assert_eq!(bound.ring_id, ring_id);
    assert_eq!(bound.mapping_bytes, owner.mapping_bytes());

    let mut schedule = PlaybackSchedule::new(slot, 1).expect("fresh playback schedule");
    decode_next(&mut client, &mut schedule).await;
    assert_eq!(
        consumer
            .wait_frames_ready(Duration::from_secs(10))
            .expect("frames-ready wait"),
        FramesReady::Signaled
    );
    drain_exact_prime(&mut consumer, 1);

    let reset_command = schedule
        .begin_reset(latentdeck_control::ResetReason::Restart)
        .expect("restart command");
    let reset = client
        .call(reset_command, Duration::from_secs(10))
        .await
        .expect("slot.reset");
    let Ack::SlotReset(reset) = reset else {
        panic!("slot.reset returned wrong acknowledgement");
    };
    schedule
        .accept_reset(&reset)
        .expect("exact cleared reset ack");
    owner
        .adopt_generation(reset.stream_generation)
        .expect("owner adopts worker generation");
    consumer
        .adopt_generation(reset.stream_generation)
        .expect("consumer adopts worker generation");

    decode_next(&mut client, &mut schedule).await;
    assert_eq!(
        consumer
            .wait_frames_ready(Duration::from_secs(10))
            .expect("second frames-ready wait"),
        FramesReady::Signaled
    );
    drain_exact_prime(&mut consumer, 2);

    client
        .request_shutdown(ShutdownReason::UserRequest, Duration::from_secs(5))
        .await
        .expect("typed worker shutdown and process exit");
}

async fn decode_next(client: &mut WorkerClient, schedule: &mut PlaybackSchedule) {
    let command = schedule
        .next_decode_command()
        .expect("next decode cycle must exist");
    let acknowledgement = client
        .call(command, COMMAND_TIMEOUT)
        .await
        .expect("real H3 decode cycle");
    let Ack::SlotDecodeCycle(acknowledgement) = acknowledgement else {
        panic!("slot.decode_cycle returned wrong acknowledgement");
    };
    schedule
        .accept_decode(&acknowledgement)
        .expect("worker cadence matches trusted schedule");
}

fn drain_exact_prime(
    consumer: &mut latentdeck_gpu::windows_ring::WindowsRgbRingConsumer,
    expected_generation: u64,
) {
    for expected_sequence in 1..=5 {
        let ReadStatus::Frame(frame) = consumer.try_read().expect("read committed frame") else {
            panic!("prime cycle frame is missing");
        };
        assert_eq!(frame.generation(), expected_generation);
        assert_eq!(frame.sequence(), expected_sequence);
        let tight_row_bytes = usize::try_from(frame.width() * 4).expect("row fits");
        let row_stride = usize::try_from(frame.row_stride()).expect("stride fits");
        for row in frame.padded_rgba().chunks_exact(row_stride) {
            assert!(
                row[..tight_row_bytes]
                    .chunks_exact(4)
                    .all(|pixel| pixel[3] == 255)
            );
            assert!(row[tight_row_bytes..].iter().all(|byte| *byte == 0));
        }
    }
    assert!(matches!(
        consumer.try_read().expect("empty read"),
        ReadStatus::Empty
    ));
}

fn required_path(name: &str) -> PathBuf {
    let value =
        env::var_os(name).unwrap_or_else(|| panic!("{name} is required for this ignored test"));
    let path = PathBuf::from(value);
    assert!(path.exists(), "{name} does not exist");
    path
}

fn expect_ack(ack: &Ack, name: &str, predicate: impl FnOnce(&Ack) -> bool) {
    assert!(predicate(ack), "{name} returned wrong acknowledgement");
}
