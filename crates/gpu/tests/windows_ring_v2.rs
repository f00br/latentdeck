#![cfg(target_os = "windows")]
#![allow(
    unsafe_code,
    reason = "tests reclaim target-process DuplicateHandle values exactly once"
)]

use std::os::windows::io::{BorrowedHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use latentdeck_gpu::ring::RingError;
use latentdeck_gpu::ring_v2::{ReadV2Status, RingV2Descriptor, WriteV2Status};
use latentdeck_gpu::windows_ring::FramesReady;
use latentdeck_gpu::windows_ring_v2::{WindowsRgbRingV2Owner, WindowsRgbRingV2Producer};
use windows::Win32::Foundation::{GetHandleInformation, HANDLE};
use windows::Win32::System::Threading::GetCurrentProcess;

#[test]
fn exact_binding_round_trips_one_batch_with_directional_event_handshake() {
    let descriptor = RingV2Descriptor::new(3, 2, 4, 3, 101).expect("valid descriptor");
    let owner = WindowsRgbRingV2Owner::create(descriptor).expect("create ABI2 owner");
    let mut consumer = owner.open_consumer().expect("open Core consumer");
    let process = current_process();
    let binding = owner
        .duplicate_into(process)
        .expect("duplicate three target handles");
    assert_eq!(binding.slot_count(), 3);
    assert_eq!(binding.frame_stride_bytes(), 24);
    assert_eq!(binding.slot_bytes(), 96);
    assert_eq!(binding.slot_stride_bytes(), 4096);
    assert_eq!(binding.byte_length(), 16_384);

    let mapping = binding.mapping_handle();
    let ready = binding.ready_event_handle();
    let consumed_handle = binding.consumed_event_handle();
    let mut producer = WindowsRgbRingV2Producer::open_from_owned_handles(
        owned_handle(mapping),
        owned_handle(ready),
        owned_handle(consumed_handle),
        binding.byte_length(),
        descriptor.generation(),
    )
    .expect("worker opens exact target handles");

    assert_eq!(
        consumer.wait_ready(Duration::ZERO).expect("ready wait"),
        FramesReady::Timeout
    );
    assert_eq!(
        producer
            .wait_consumed(Duration::ZERO)
            .expect("consumed wait"),
        FramesReady::Timeout
    );
    let pixels: Vec<u8> = (0..48).collect();
    let WriteV2Status::Written(metadata) = producer
        .try_write_batch([7; 16], 55, 2, 3, 2, &pixels)
        .expect("publish decoded batch")
    else {
        panic!("empty ring must accept one batch");
    };
    assert_eq!(metadata.slot_sequence(), 1);
    assert_eq!(metadata.logical_sequence(), 55);
    assert_eq!(
        consumer
            .wait_ready(Duration::from_secs(1))
            .expect("producer ready event"),
        FramesReady::Signaled
    );
    assert_eq!(
        producer
            .wait_consumed(Duration::ZERO)
            .expect("not consumed yet"),
        FramesReady::Timeout
    );
    let ReadV2Status::Batch(batch) = consumer.try_read().expect("consume decoded batch") else {
        panic!("ready slot must contain one batch");
    };
    assert_eq!(batch.metadata(), metadata);
    assert_eq!(batch.pixels(), pixels);
    assert_eq!(
        producer
            .wait_consumed(Duration::from_secs(1))
            .expect("consumer consumed event"),
        FramesReady::Signaled
    );

    drop(producer);
    assert!(!handle_is_valid(mapping));
    assert!(!handle_is_valid(ready));
    assert!(!handle_is_valid(consumed_handle));
}

#[test]
fn generation_reset_rejects_stale_consumer_and_clears_both_events() {
    let descriptor = RingV2Descriptor::new(1, 1, 2, 2, 201).expect("valid descriptor");
    let mut owner = WindowsRgbRingV2Owner::create(descriptor).expect("create ABI2 owner");
    let mut consumer = owner.open_consumer().expect("open Core consumer");
    let binding = owner
        .duplicate_into(current_process())
        .expect("duplicate handles");
    let mut producer = WindowsRgbRingV2Producer::open_from_owned_handles(
        owned_handle(binding.mapping_handle()),
        owned_handle(binding.ready_event_handle()),
        owned_handle(binding.consumed_event_handle()),
        binding.byte_length(),
        201,
    )
    .expect("open worker producer");
    producer
        .try_write_batch([1; 16], 1, 1, 1, 1, &[1, 2, 3, 255])
        .expect("publish old generation");
    producer.set_generation(202).expect("strict reset");
    assert_eq!(
        consumer.wait_ready(Duration::ZERO).expect("ready cleared"),
        FramesReady::Timeout
    );
    assert_eq!(
        producer
            .wait_consumed(Duration::ZERO)
            .expect("consumed cleared"),
        FramesReady::Timeout
    );
    assert!(matches!(
        consumer.try_read(),
        Err(RingError::GenerationChanged {
            expected: 201,
            actual: 202
        })
    ));
    consumer
        .adopt_generation(202)
        .expect("adopt reset acknowledgement");
    owner
        .adopt_generation(202)
        .expect("owner adopts reset acknowledgement");
    assert_eq!(
        consumer.try_read().expect("reset ring is empty"),
        ReadV2Status::Empty
    );
    assert!(matches!(
        producer.set_generation(202),
        Err(RingError::GenerationNotIncreasing { .. })
    ));
}

fn current_process() -> BorrowedHandle<'static> {
    // SAFETY: The current-process pseudo-handle is valid for process lifetime.
    unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess().0) }
}

fn owned_handle(value: u64) -> OwnedHandle {
    let address = usize::try_from(value).expect("target handle fits pointer width");
    // SAFETY: Each value is a nonzero target-process DuplicateHandle result
    // transferred to one OwnedHandle exactly once.
    unsafe { OwnedHandle::from_raw_handle(std::ptr::without_provenance_mut(address)) }
}

fn handle_is_valid(value: u64) -> bool {
    let address = usize::try_from(value).expect("target handle fits pointer width");
    let handle = HANDLE(std::ptr::without_provenance_mut(address));
    let mut flags = 0;
    // SAFETY: GetHandleInformation only probes the numeric current-process
    // handle and writes one initialized DWORD.
    unsafe { GetHandleInformation(handle, &raw mut flags) }.is_ok()
}
