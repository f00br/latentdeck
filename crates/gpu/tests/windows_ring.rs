#![cfg(target_os = "windows")]
#![allow(
    unsafe_code,
    reason = "the test reclaims DuplicateHandle values targeted to this process"
)]

use std::os::windows::io::{BorrowedHandle, FromRawHandle, OwnedHandle};
use std::time::Duration;

use latentdeck_gpu::ring::{ReadStatus, RingDescriptor, RingError, WriteStatus};
use latentdeck_gpu::windows_ring::{FramesReady, WindowsRgbRingOwner, WindowsRgbRingProducer};
use windows::Win32::System::Threading::GetCurrentProcess;

#[test]
fn anonymous_mapping_duplicates_into_target_and_signals_committed_frames() {
    let descriptor = RingDescriptor::new(3, 2, 101).expect("valid descriptor");
    let owner = WindowsRgbRingOwner::create(descriptor).expect("create anonymous ring");
    let mut consumer = owner.open_consumer().expect("open owner consumer");

    // SAFETY: GetCurrentProcess returns a process-lifetime pseudo-handle. This
    // test uses the current process as the DuplicateHandle target so the
    // returned target handles can be owned and closed locally.
    let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess().0) };
    let binding = owner
        .duplicate_into(process)
        .expect("duplicate mapping and event into target");
    assert_eq!(binding.mapping_bytes(), descriptor.layout().mapping_bytes());

    let target_mapping = owned_current_process_handle(binding.mapping_handle());
    let target_event = owned_current_process_handle(binding.frames_ready_event_handle());
    let mut producer = WindowsRgbRingProducer::open_from_owned_handles(
        target_mapping,
        target_event,
        binding.mapping_bytes(),
        101,
    )
    .expect("worker opens duplicated handles");

    assert_eq!(
        consumer
            .wait_frames_ready(Duration::ZERO)
            .expect("event wait"),
        FramesReady::Timeout
    );
    let tight_rgba: Vec<u8> = (0..24).collect();
    assert!(matches!(
        producer.try_write(&tight_rgba, 55).expect("publish"),
        WriteStatus::Written(_)
    ));
    assert_eq!(
        consumer
            .wait_frames_ready(Duration::from_secs(1))
            .expect("event wait"),
        FramesReady::Signaled
    );
    let ReadStatus::Frame(frame) = consumer.try_read().expect("consume") else {
        panic!("signaled committed frame must be readable");
    };
    assert_eq!(frame.sequence(), 1);
    assert_eq!(&frame.padded_rgba()[..12], &tight_rgba[..12]);
}

#[test]
fn generation_reset_clears_event_sequences_and_old_commits() {
    let descriptor = RingDescriptor::new(1, 1, 201).expect("valid descriptor");
    let mut owner = WindowsRgbRingOwner::create(descriptor).expect("create anonymous ring");
    owner
        .set_generation(202)
        .expect("owner advances unclaimed pre-bind generation");
    assert!(matches!(
        owner.set_generation(202),
        Err(RingError::GenerationNotIncreasing { .. })
    ));
    let mut consumer = owner.open_consumer().expect("open owner consumer");

    // SAFETY: See the first test; duplication targets this same process.
    let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess().0) };
    let binding = owner.duplicate_into(process).expect("duplicate handles");
    let target_mapping = owned_current_process_handle(binding.mapping_handle());
    let target_event = owned_current_process_handle(binding.frames_ready_event_handle());
    let mut producer = WindowsRgbRingProducer::open_from_owned_handles(
        target_mapping,
        target_event,
        binding.mapping_bytes(),
        202,
    )
    .expect("worker opens target handles");

    producer
        .try_write(&[1, 2, 3, 255], 1)
        .expect("publish stale frame");
    assert!(matches!(
        owner.set_generation(203),
        Err(RingError::ProducerAlreadyClaimed)
    ));
    producer.set_generation(203).expect("worker resets ring");
    assert_eq!(
        consumer
            .wait_frames_ready(Duration::ZERO)
            .expect("reset event state"),
        FramesReady::Timeout
    );
    assert!(matches!(
        consumer.try_read(),
        Err(RingError::GenerationChanged {
            expected: 202,
            actual: 203
        })
    ));
    consumer
        .adopt_generation(203)
        .expect("adopt reset acknowledgement");
    owner
        .adopt_generation(203)
        .expect("owner adopts remote reset acknowledgement");
    assert_eq!(owner.descriptor().generation(), 203);
    let state = producer.state().expect("reset state");
    assert_eq!(state.producer_sequence(), 0);
    assert_eq!(state.consumer_sequence(), 0);
    assert_eq!(state.occupancy(), 0);
    assert_eq!(state.available_capacity(), 24);
    assert!(producer.can_publish(17).expect("cycle capacity"));

    producer
        .try_write(&[9, 8, 7, 255], 2)
        .expect("publish new generation");
    assert_eq!(
        consumer
            .wait_frames_ready(Duration::from_secs(1))
            .expect("new generation event"),
        FramesReady::Signaled
    );
    let ReadStatus::Frame(frame) = consumer.try_read().expect("read new frame") else {
        panic!("new generation frame must be visible");
    };
    assert_eq!(frame.generation(), 203);
    assert_eq!(&frame.padded_rgba()[..4], &[9, 8, 7, 255]);
}

#[test]
fn producer_rejects_a_non_event_handle_without_leaking_its_claim() {
    let descriptor = RingDescriptor::new(1, 1, 301).expect("valid descriptor");
    let owner = WindowsRgbRingOwner::create(descriptor).expect("create anonymous ring");
    // SAFETY: The pseudo-handle is process-lifetime and duplication targets
    // this same process for deterministic handle ownership in the test.
    let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess().0) };

    let first = owner.duplicate_into(process).expect("first handle pair");
    let second = owner.duplicate_into(process).expect("second handle pair");
    let first_mapping = owned_current_process_handle(first.mapping_handle());
    let first_event = owned_current_process_handle(first.frames_ready_event_handle());
    let second_mapping = owned_current_process_handle(second.mapping_handle());
    let second_event = owned_current_process_handle(second.frames_ready_event_handle());
    drop(first_event);
    drop(second_event);

    let error = WindowsRgbRingProducer::open_from_owned_handles(
        first_mapping,
        second_mapping,
        first.mapping_bytes(),
        301,
    )
    .err()
    .expect("a mapping handle is not an event handle");
    assert!(matches!(error, RingError::Windows(_)));

    let valid = owner
        .duplicate_into(process)
        .expect("valid pair after rejected open");
    let mapping = owned_current_process_handle(valid.mapping_handle());
    let event = owned_current_process_handle(valid.frames_ready_event_handle());
    WindowsRgbRingProducer::open_from_owned_handles(mapping, event, valid.mapping_bytes(), 301)
        .expect("rejected event did not retain the producer claim");
}

#[test]
fn rejected_duplicate_producer_does_not_clear_a_pending_frame_event() {
    let descriptor = RingDescriptor::new(1, 1, 401).expect("valid descriptor");
    let owner = WindowsRgbRingOwner::create(descriptor).expect("create anonymous ring");
    let consumer = owner.open_consumer().expect("open consumer");
    // SAFETY: The pseudo-handle is process-lifetime and target is this process.
    let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess().0) };

    let first = owner.duplicate_into(process).expect("first pair");
    let mut producer = WindowsRgbRingProducer::open_from_owned_handles(
        owned_current_process_handle(first.mapping_handle()),
        owned_current_process_handle(first.frames_ready_event_handle()),
        first.mapping_bytes(),
        401,
    )
    .expect("claim producer");
    producer
        .try_write(&[1, 2, 3, 255], 1)
        .expect("publish pending frame");

    let duplicate = owner.duplicate_into(process).expect("duplicate pair");
    let error = WindowsRgbRingProducer::open_from_owned_handles(
        owned_current_process_handle(duplicate.mapping_handle()),
        owned_current_process_handle(duplicate.frames_ready_event_handle()),
        duplicate.mapping_bytes(),
        401,
    )
    .err()
    .expect("second producer must be rejected");
    assert!(matches!(error, RingError::ProducerAlreadyClaimed));
    assert_eq!(
        consumer
            .wait_frames_ready(Duration::ZERO)
            .expect("pending event remains signaled"),
        FramesReady::Signaled
    );
}

fn owned_current_process_handle(value: u64) -> OwnedHandle {
    let address = usize::try_from(value).expect("current-process handle fits pointer width");
    let raw = std::ptr::without_provenance_mut(address);
    // SAFETY: Every caller passes a nonzero DuplicateHandle result targeted to
    // this current process and transfers ownership exactly once.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}
