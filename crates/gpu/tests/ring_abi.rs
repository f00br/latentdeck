use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

use latentdeck_gpu::ring::abi1;
use latentdeck_gpu::ring::{
    ABI_VERSION, MAPPING_HEADER_BYTES, MAX_MAPPING_BYTES, RING_SLOT_COUNT, ReadStatus,
    RingDescriptor, RingError, RingLayout, SLOT_HEADER_BYTES, SLOT_STRIDE_ALIGNMENT,
    TestFileRgbRingConsumer, TestFileRgbRingProducer, WriteStatus,
};

#[test]
fn abi_one_layout_is_fixed_and_rows_are_256_byte_aligned() {
    let layout = RingLayout::new(3, 2).expect("3x2 RGBA8 must fit the ABI limits");

    assert_eq!(ABI_VERSION, 1);
    assert_eq!(MAPPING_HEADER_BYTES, 4096);
    assert_eq!(SLOT_HEADER_BYTES, 128);
    assert_eq!(SLOT_STRIDE_ALIGNMENT, 4096);
    assert_eq!(RING_SLOT_COUNT, 24);
    assert_eq!(layout.row_stride(), 256);
    assert_eq!(layout.payload_bytes(), 512);
    assert_eq!(layout.slot_stride(), 4096);
    assert_eq!(layout.mapping_bytes(), 102_400);
}

#[test]
fn layout_rejects_zero_overflowing_and_over_cap_dimensions() {
    assert!(matches!(
        RingLayout::new(0, 1),
        Err(RingError::InvalidDimensions { .. })
    ));
    assert!(matches!(
        RingLayout::new(u32::MAX, 1),
        Err(RingError::LayoutOverflow)
    ));
    assert!(matches!(
        RingLayout::new(3840, 2160),
        Err(RingError::MappingTooLarge { .. })
    ));
}

#[test]
fn file_backed_ring_roundtrips_rgba_with_zeroed_row_padding() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("frames.rgb-ring");
    let descriptor = RingDescriptor::new(3, 2, 7).expect("valid ring descriptor");
    let mut producer = TestFileRgbRingProducer::create(&path, descriptor).expect("create producer");
    let mut consumer = TestFileRgbRingConsumer::open(&path, 7).expect("open consumer");
    let tight_rgba: Vec<u8> = (0..24).collect();

    let written = producer
        .try_write(&tight_rgba, 123_456)
        .expect("write frame");
    let WriteStatus::Written(written) = written else {
        panic!("an empty ring must accept its first frame");
    };
    assert_eq!(written.sequence(), 1);
    assert_eq!(written.generation(), 7);

    let ReadStatus::Frame(frame) = consumer.try_read().expect("read frame") else {
        panic!("the published frame must be visible");
    };
    assert_eq!(frame.sequence(), 1);
    assert_eq!(frame.generation(), 7);
    assert_eq!(frame.timestamp_ns(), 123_456);
    assert_eq!(frame.row_stride(), 256);
    assert_eq!(frame.padded_rgba().len(), 512);
    assert_eq!(&frame.padded_rgba()[..12], &tight_rgba[..12]);
    assert!(frame.padded_rgba()[12..256].iter().all(|byte| *byte == 0));
    assert_eq!(&frame.padded_rgba()[256..268], &tight_rgba[12..]);
    assert!(frame.padded_rgba()[268..].iter().all(|byte| *byte == 0));
}

#[test]
fn rejected_input_does_not_publish_or_consume_a_sequence() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("bad-input.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 8).expect("valid descriptor");
    let mut producer = TestFileRgbRingProducer::create(&path, descriptor).expect("create producer");
    let mut consumer = TestFileRgbRingConsumer::open(&path, 8).expect("open consumer");

    assert!(matches!(
        producer.try_write(&[1, 2, 3], 1),
        Err(RingError::FrameLengthMismatch {
            expected: 4,
            actual: 3
        })
    ));
    assert_eq!(
        consumer.try_read().expect("empty result"),
        ReadStatus::Empty
    );

    let WriteStatus::Written(metadata) = producer.try_write(&[1, 2, 3, 4], 2).expect("valid write")
    else {
        panic!("valid frame must publish");
    };
    assert_eq!(metadata.sequence(), 1);
}

#[test]
fn producer_applies_backpressure_then_reuses_only_a_released_slot() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("bounded.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 9).expect("valid descriptor");
    let mut producer = TestFileRgbRingProducer::create(&path, descriptor).expect("create producer");
    let mut consumer = TestFileRgbRingConsumer::open(&path, 9).expect("open consumer");
    let pixel = [1, 2, 3, 4];

    for expected_sequence in 1..=u64::from(RING_SLOT_COUNT) {
        let WriteStatus::Written(metadata) = producer
            .try_write(&pixel, expected_sequence)
            .expect("write within capacity")
        else {
            panic!("slot {expected_sequence} must be available");
        };
        assert_eq!(metadata.sequence(), expected_sequence);
        if expected_sequence == 8 {
            let state = producer.state().expect("consistent capacity snapshot");
            assert_eq!(state.producer_sequence(), 8);
            assert_eq!(state.consumer_sequence(), 0);
            assert_eq!(state.occupancy(), 8);
            assert_eq!(state.available_capacity(), 16);
            assert!(!producer.can_publish(17).expect("17-frame preflight"));
            assert!(producer.can_publish(5).expect("5-frame preflight"));
        }
    }

    let WriteStatus::Backpressure(backpressure) =
        producer.try_write(&pixel, 25).expect("bounded result")
    else {
        panic!("the 25th outstanding frame must not overwrite slot zero");
    };
    assert_eq!(backpressure.queued(), RING_SLOT_COUNT);
    assert_eq!(backpressure.capacity(), RING_SLOT_COUNT);

    let ReadStatus::Frame(first) = consumer.try_read().expect("release first slot") else {
        panic!("first frame must be available");
    };
    assert_eq!(first.sequence(), 1);

    let WriteStatus::Written(wrapped) =
        producer.try_write(&pixel, 25).expect("reuse released slot")
    else {
        panic!("one released slot must accept sequence 25");
    };
    assert_eq!(wrapped.sequence(), 25);

    for expected_sequence in 2..=25 {
        let ReadStatus::Frame(frame) = consumer.try_read().expect("read queued frame") else {
            panic!("sequence {expected_sequence} must remain queued");
        };
        assert_eq!(frame.sequence(), expected_sequence);
    }
    assert_eq!(
        consumer.try_read().expect("empty result"),
        ReadStatus::Empty
    );
}

#[test]
fn generation_and_single_consumer_claim_are_enforced() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("generation.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 41).expect("valid descriptor");
    let _producer = TestFileRgbRingProducer::create(&path, descriptor).expect("create producer");

    let stale = TestFileRgbRingConsumer::open(&path, 42)
        .err()
        .expect("stale generation must fail");
    assert!(matches!(stale, RingError::GenerationMismatch { .. }));

    let first = TestFileRgbRingConsumer::open(&path, 41).expect("first consumer claims endpoint");
    let duplicate = TestFileRgbRingConsumer::open(&path, 41)
        .err()
        .expect("second consumer must not be admitted");
    assert!(matches!(duplicate, RingError::ConsumerAlreadyClaimed));

    drop(first);
    TestFileRgbRingConsumer::open(&path, 41).expect("dropping consumer releases its claim");
}

#[test]
fn initialized_file_can_transfer_the_single_producer_role_to_a_worker() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("producer-claim.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 43).expect("valid descriptor");
    let creator = TestFileRgbRingProducer::create(&path, descriptor).expect("initialize and claim");

    let duplicate = TestFileRgbRingProducer::open(&path, 43)
        .err()
        .expect("second producer must not be admitted");
    assert!(matches!(duplicate, RingError::ProducerAlreadyClaimed));

    drop(creator);
    let worker =
        TestFileRgbRingProducer::open(&path, 43).expect("worker claims released producer role");
    assert_eq!(worker.descriptor(), descriptor);
}

#[test]
fn open_rejects_corrupt_or_oversized_mapping_headers() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let bad_magic = directory.path().join("bad-magic.rgb-ring");
    create_then_drop(&bad_magic, 51);
    overwrite(&bad_magic, abi1::HEADER_MAGIC_OFFSET as u64, b"NOTARING");
    assert!(matches!(
        open_error(&bad_magic, 51),
        RingError::InvalidMagic
    ));

    let bad_abi = directory.path().join("bad-abi.rgb-ring");
    create_then_drop(&bad_abi, 52);
    overwrite(
        &bad_abi,
        abi1::HEADER_ABI_OFFSET as u64,
        &99_u32.to_le_bytes(),
    );
    assert!(matches!(
        open_error(&bad_abi, 52),
        RingError::UnsupportedAbi { actual: 99 }
    ));

    let bad_stride = directory.path().join("bad-stride.rgb-ring");
    create_then_drop(&bad_stride, 53);
    overwrite(
        &bad_stride,
        abi1::HEADER_ROW_STRIDE_OFFSET as u64,
        &12_u32.to_le_bytes(),
    );
    assert!(matches!(
        open_error(&bad_stride, 53),
        RingError::CorruptHeader {
            field: "row_stride"
        }
    ));

    let oversized = directory.path().join("oversized.rgb-ring");
    create_then_drop(&oversized, 54);
    OpenOptions::new()
        .write(true)
        .open(&oversized)
        .expect("open oversized fixture")
        .set_len(MAX_MAPPING_BYTES + 1)
        .expect("extend sparse fixture");
    assert!(matches!(
        open_error(&oversized, 54),
        RingError::InvalidMappingLength { .. }
    ));
}

#[test]
fn consumer_rejects_impossible_counters_and_corrupt_published_slots() {
    let directory = tempfile::tempdir().expect("temporary directory");

    let counters = directory.path().join("bad-counters.rgb-ring");
    create_then_drop(&counters, 61);
    overwrite(
        &counters,
        abi1::HEADER_PRODUCER_SEQUENCE_OFFSET as u64,
        &(u64::from(RING_SLOT_COUNT) + 1).to_le_bytes(),
    );
    assert!(matches!(
        open_error(&counters, 61),
        RingError::CorruptSequences { .. }
    ));

    let slot = directory.path().join("bad-slot.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 62).expect("valid descriptor");
    let mut producer = TestFileRgbRingProducer::create(&slot, descriptor).expect("create ring");
    producer
        .try_write(&[1, 2, 3, 4], 99)
        .expect("publish fixture frame");
    drop(producer);
    overwrite(
        &slot,
        MAPPING_HEADER_BYTES + abi1::SLOT_GENERATION_OFFSET as u64,
        &999_u64.to_le_bytes(),
    );
    let mut consumer = TestFileRgbRingConsumer::open(&slot, 62).expect("header remains valid");
    assert!(matches!(
        consumer.try_read(),
        Err(RingError::CorruptSlot { sequence: 1 })
    ));

    let slot_padding = directory.path().join("bad-slot-padding.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 63).expect("valid descriptor");
    let mut producer =
        TestFileRgbRingProducer::create(&slot_padding, descriptor).expect("create ring");
    producer
        .try_write(&[1, 2, 3, 4], 100)
        .expect("publish fixture frame");
    drop(producer);
    overwrite(
        &slot_padding,
        MAPPING_HEADER_BYTES + SLOT_HEADER_BYTES + 256,
        &[1],
    );
    let mut consumer =
        TestFileRgbRingConsumer::open(&slot_padding, 63).expect("header remains valid");
    assert!(matches!(
        consumer.try_read(),
        Err(RingError::CorruptSlot { sequence: 1 })
    ));

    let row_padding = directory.path().join("bad-row-padding.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 64).expect("valid descriptor");
    let mut producer =
        TestFileRgbRingProducer::create(&row_padding, descriptor).expect("create ring");
    producer
        .try_write(&[1, 2, 3, 4], 101)
        .expect("publish fixture frame");
    drop(producer);
    overwrite(
        &row_padding,
        MAPPING_HEADER_BYTES + SLOT_HEADER_BYTES + 4,
        &[1],
    );
    let mut consumer =
        TestFileRgbRingConsumer::open(&row_padding, 64).expect("header remains valid");
    assert!(matches!(
        consumer.try_read(),
        Err(RingError::CorruptSlot { sequence: 1 })
    ));
}

#[test]
fn spsc_threads_preserve_all_sequences_across_many_wraps() {
    const FRAME_COUNT: u64 = 256;
    const SPIN_LIMIT: u64 = 10_000_000;

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("threaded.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 71).expect("valid descriptor");
    let mut producer = TestFileRgbRingProducer::create(&path, descriptor).expect("create producer");
    let mut consumer = TestFileRgbRingConsumer::open(&path, 71).expect("open consumer");

    let consumer_thread = std::thread::spawn(move || {
        let mut expected = 1;
        let mut spins = 0;
        while expected <= FRAME_COUNT {
            match consumer.try_read().expect("consumer state remains valid") {
                ReadStatus::Frame(frame) => {
                    assert_eq!(frame.sequence(), expected);
                    assert_eq!(frame.timestamp_ns(), expected);
                    assert_eq!(frame.padded_rgba()[0], expected.to_le_bytes()[0]);
                    expected += 1;
                }
                ReadStatus::Empty => {
                    spins += 1;
                    assert!(spins < SPIN_LIMIT, "consumer exceeded bounded spin budget");
                    std::thread::yield_now();
                }
            }
        }
    });

    for sequence in 1..=FRAME_COUNT {
        let pixel = [sequence.to_le_bytes()[0], 2, 3, 255];
        let mut spins = 0;
        loop {
            match producer
                .try_write(&pixel, sequence)
                .expect("producer state remains valid")
            {
                WriteStatus::Written(metadata) => {
                    assert_eq!(metadata.sequence(), sequence);
                    break;
                }
                WriteStatus::Backpressure(_) => {
                    spins += 1;
                    assert!(spins < SPIN_LIMIT, "producer exceeded bounded spin budget");
                    std::thread::yield_now();
                }
            }
        }
    }

    consumer_thread.join().expect("consumer thread completes");
}

#[test]
fn generation_reset_discards_stale_frames_and_restarts_sequences() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("generation-reset.rgb-ring");
    let descriptor = RingDescriptor::new(1, 1, 81).expect("valid descriptor");
    let mut producer = TestFileRgbRingProducer::create(&path, descriptor).expect("create producer");
    let mut consumer = TestFileRgbRingConsumer::open(&path, 81).expect("open consumer");

    producer
        .try_write(&[1, 2, 3, 255], 1)
        .expect("publish stale frame");
    assert_eq!(producer.state().expect("state").occupancy(), 1);
    assert_eq!(producer.state().expect("state").available_capacity(), 23);

    producer.set_generation(82).expect("advance generation");
    assert!(matches!(
        consumer.try_read(),
        Err(RingError::GenerationChanged {
            expected: 81,
            actual: 82
        })
    ));
    consumer
        .adopt_generation(82)
        .expect("consumer adopts acknowledged generation");
    let reset = consumer.state().expect("reset state");
    assert_eq!(reset.producer_sequence(), 0);
    assert_eq!(reset.consumer_sequence(), 0);
    assert_eq!(reset.occupancy(), 0);
    assert_eq!(reset.available_capacity(), RING_SLOT_COUNT);
    assert!(consumer.can_publish(17).expect("capacity snapshot"));
    assert!(matches!(
        producer.set_generation(82),
        Err(RingError::GenerationNotIncreasing { .. })
    ));

    let WriteStatus::Written(frame) = producer
        .try_write(&[9, 8, 7, 255], 2)
        .expect("publish new generation")
    else {
        panic!("reset ring must accept sequence one");
    };
    assert_eq!(frame.generation(), 82);
    assert_eq!(frame.sequence(), 1);
    let ReadStatus::Frame(frame) = consumer.try_read().expect("read new generation") else {
        panic!("new generation frame must be visible");
    };
    assert_eq!(frame.generation(), 82);
    assert_eq!(&frame.padded_rgba()[..4], &[9, 8, 7, 255]);
}

fn create_then_drop(path: &std::path::Path, generation: u64) {
    let descriptor = RingDescriptor::new(1, 1, generation).expect("valid descriptor");
    drop(TestFileRgbRingProducer::create(path, descriptor).expect("create fixture ring"));
}

fn overwrite(path: &std::path::Path, offset: u64, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open fixture for corruption");
    file.seek(SeekFrom::Start(offset)).expect("seek fixture");
    file.write_all(bytes).expect("corrupt fixture");
    file.flush().expect("flush fixture corruption");
}

fn open_error(path: &std::path::Path, generation: u64) -> RingError {
    TestFileRgbRingConsumer::open(path, generation)
        .err()
        .expect("corrupt fixture must be rejected")
}
