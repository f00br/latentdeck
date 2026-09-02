//! Checked layout and state machine for Protocol 2 decoded-RGBA Ring ABI 2.
//!
//! One queue slot contains one complete contiguous CPU `uint8 [N,H,W,4]`
//! batch. Control IPC carries only duplicated handle values and bounded
//! metadata; pixel bytes remain in this mapping.

use std::sync::atomic::Ordering;

use crate::ring::{MAX_MAPPING_BYTES, RingError, mapped};

#[cfg(not(target_endian = "little"))]
compile_error!("LatentDeck RGB Ring ABI 2 requires a little-endian target");
#[cfg(not(target_has_atomic = "64"))]
compile_error!("LatentDeck RGB Ring ABI 2 requires lock-free-width 64-bit atomics");

/// Stable mapping ABI generation used by Protocol 2.
pub const ABI_VERSION: u32 = 2;
/// Bytes reserved for the mapping-wide header.
pub const MAPPING_HEADER_BYTES: u64 = 4096;
/// Bytes reserved before each decoded batch payload.
pub const SLOT_HEADER_BYTES: u64 = 128;
/// Minimum number of queued decoded batches.
pub const MIN_SLOT_COUNT: u32 = 2;
/// Maximum number of queued decoded batches.
pub const MAX_SLOT_COUNT: u32 = 24;
/// Maximum decoded frames in one ABI batch.
pub const MAX_BATCH_FRAMES: u32 = 24;
/// Alignment of each complete queue slot.
pub const SLOT_STRIDE_ALIGNMENT: u64 = 4096;

const BYTES_PER_PIXEL: u64 = 4;
const MAPPING_HEADER_LEN: usize = 4096;
const SLOT_HEADER_LEN: usize = 128;
const MAPPING_HEADER_FIELD: u32 = 4096;
const SLOT_HEADER_FIELD: u32 = 128;
const STATE_READY: u64 = 1;
const STATE_RESETTING: u64 = 2;

/// Public byte offsets for independent ABI 2 implementations.
pub mod abi2 {
    /// Eight-byte mapping signature.
    pub const MAGIC: [u8; 8] = *b"LDRGBR02";
    pub const HEADER_MAGIC_OFFSET: usize = 0;
    pub const HEADER_ABI_OFFSET: usize = 8;
    pub const HEADER_BYTES_OFFSET: usize = 12;
    pub const HEADER_GENERATION_OFFSET: usize = 16;
    pub const HEADER_STATE_OFFSET: usize = 24;
    pub const HEADER_SLOT_COUNT_OFFSET: usize = 32;
    pub const HEADER_SLOT_HEADER_BYTES_OFFSET: usize = 36;
    pub const HEADER_WIDTH_OFFSET: usize = 40;
    pub const HEADER_HEIGHT_OFFSET: usize = 44;
    pub const HEADER_FRAME_STRIDE_OFFSET: usize = 48;
    pub const HEADER_SLOT_BYTES_OFFSET: usize = 56;
    pub const HEADER_SLOT_STRIDE_OFFSET: usize = 64;
    pub const HEADER_MAPPING_BYTES_OFFSET: usize = 72;
    pub const HEADER_MAX_BATCH_OFFSET: usize = 80;
    pub const HEADER_PIXEL_FORMAT_OFFSET: usize = 84;
    pub const HEADER_PRODUCER_SEQUENCE_OFFSET: usize = 88;
    pub const HEADER_CONSUMER_SEQUENCE_OFFSET: usize = 96;
    pub const HEADER_PRODUCER_CLAIM_OFFSET: usize = 104;
    pub const HEADER_CONSUMER_CLAIM_OFFSET: usize = 112;
    pub const HEADER_RESERVED_OFFSET: usize = 120;

    pub const SLOT_SEQUENCE_OFFSET: usize = 0;
    pub const SLOT_GENERATION_OFFSET: usize = 8;
    pub const SLOT_LOGICAL_SEQUENCE_OFFSET: usize = 16;
    pub const SLOT_SESSION_ID_OFFSET: usize = 24;
    pub const SLOT_BATCH_OFFSET: usize = 40;
    pub const SLOT_WIDTH_OFFSET: usize = 44;
    pub const SLOT_HEIGHT_OFFSET: usize = 48;
    pub const SLOT_FRAME_STRIDE_OFFSET: usize = 56;
    pub const SLOT_PAYLOAD_BYTES_OFFSET: usize = 64;
    pub const SLOT_RESERVED_OFFSET: usize = 72;

    /// CPU RGBA8 wire value.
    pub const PIXEL_FORMAT_RGBA8: u32 = 1;
}

use abi2::{
    HEADER_ABI_OFFSET, HEADER_BYTES_OFFSET, HEADER_CONSUMER_CLAIM_OFFSET,
    HEADER_CONSUMER_SEQUENCE_OFFSET, HEADER_FRAME_STRIDE_OFFSET, HEADER_GENERATION_OFFSET,
    HEADER_HEIGHT_OFFSET, HEADER_MAGIC_OFFSET, HEADER_MAPPING_BYTES_OFFSET,
    HEADER_MAX_BATCH_OFFSET, HEADER_PIXEL_FORMAT_OFFSET, HEADER_PRODUCER_CLAIM_OFFSET,
    HEADER_PRODUCER_SEQUENCE_OFFSET, HEADER_RESERVED_OFFSET, HEADER_SLOT_BYTES_OFFSET,
    HEADER_SLOT_COUNT_OFFSET, HEADER_SLOT_HEADER_BYTES_OFFSET, HEADER_SLOT_STRIDE_OFFSET,
    HEADER_STATE_OFFSET, HEADER_WIDTH_OFFSET, MAGIC, PIXEL_FORMAT_RGBA8, SLOT_BATCH_OFFSET,
    SLOT_FRAME_STRIDE_OFFSET, SLOT_GENERATION_OFFSET, SLOT_HEIGHT_OFFSET,
    SLOT_LOGICAL_SEQUENCE_OFFSET, SLOT_PAYLOAD_BYTES_OFFSET, SLOT_RESERVED_OFFSET,
    SLOT_SEQUENCE_OFFSET, SLOT_SESSION_ID_OFFSET, SLOT_WIDTH_OFFSET,
};

/// Fully recomputed fixed geometry and queue capacity for ABI 2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingV2Layout {
    width: u32,
    height: u32,
    maximum_batch: u32,
    slot_count: u32,
    frame_stride_bytes: u64,
    slot_bytes: u64,
    slot_stride_bytes: u64,
    mapping_bytes: u64,
}

impl RingV2Layout {
    /// Computes an exact tight-RGBA batch layout under the shared-memory cap.
    ///
    /// # Errors
    ///
    /// Returns a bounded-layout error for invalid counts, dimensions,
    /// arithmetic overflow, or a mapping larger than [`MAX_MAPPING_BYTES`].
    pub fn new(
        width: u32,
        height: u32,
        maximum_batch: u32,
        slot_count: u32,
    ) -> Result<Self, RingError> {
        if width == 0 || height == 0 {
            return Err(RingError::InvalidDimensions { width, height });
        }
        if !(1..=MAX_BATCH_FRAMES).contains(&maximum_batch) {
            return Err(RingError::InvalidBatchCount {
                actual: maximum_batch,
            });
        }
        if !(MIN_SLOT_COUNT..=MAX_SLOT_COUNT).contains(&slot_count) {
            return Err(RingError::InvalidSlotCount { actual: slot_count });
        }
        let frame_stride_bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(BYTES_PER_PIXEL))
            .ok_or(RingError::LayoutOverflow)?;
        let slot_bytes = frame_stride_bytes
            .checked_mul(u64::from(maximum_batch))
            .ok_or(RingError::LayoutOverflow)?;
        let used = SLOT_HEADER_BYTES
            .checked_add(slot_bytes)
            .ok_or(RingError::LayoutOverflow)?;
        let slot_stride_bytes = align_up(used, SLOT_STRIDE_ALIGNMENT)?;
        let mapping_bytes = MAPPING_HEADER_BYTES
            .checked_add(
                slot_stride_bytes
                    .checked_mul(u64::from(slot_count))
                    .ok_or(RingError::LayoutOverflow)?,
            )
            .ok_or(RingError::LayoutOverflow)?;
        if mapping_bytes > MAX_MAPPING_BYTES {
            return Err(RingError::MappingTooLarge {
                requested: mapping_bytes,
                maximum: MAX_MAPPING_BYTES,
            });
        }
        Ok(Self {
            width,
            height,
            maximum_batch,
            slot_count,
            frame_stride_bytes,
            slot_bytes,
            slot_stride_bytes,
            mapping_bytes,
        })
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn maximum_batch(self) -> u32 {
        self.maximum_batch
    }

    #[must_use]
    pub const fn slot_count(self) -> u32 {
        self.slot_count
    }

    /// Exact tight bytes in one `[H,W,4]` frame.
    #[must_use]
    pub const fn frame_stride_bytes(self) -> u64 {
        self.frame_stride_bytes
    }

    /// Maximum payload bytes in one queued `[N,H,W,4]` batch.
    #[must_use]
    pub const fn slot_bytes(self) -> u64 {
        self.slot_bytes
    }

    /// Header, payload capacity, and zeroed alignment padding per slot.
    #[must_use]
    pub const fn slot_stride_bytes(self) -> u64 {
        self.slot_stride_bytes
    }

    /// Exact anonymous mapping byte length.
    #[must_use]
    pub const fn mapping_bytes(self) -> u64 {
        self.mapping_bytes
    }
}

/// Computes the exact mapping length from the two fields currently carried by
/// typed `ring.configure`, before any untrusted mapping bytes are read.
///
/// # Errors
///
/// Returns an error for an invalid slot count, zero/overflowing slot payload,
/// or a result over the shared-memory cap.
pub fn control_mapping_bytes(slot_count: u32, slot_bytes: u64) -> Result<u64, RingError> {
    if !(MIN_SLOT_COUNT..=MAX_SLOT_COUNT).contains(&slot_count) {
        return Err(RingError::InvalidSlotCount { actual: slot_count });
    }
    if slot_bytes == 0 {
        return Err(RingError::BatchLengthMismatch {
            expected: 1,
            actual: 0,
        });
    }
    let slot_stride = align_up(
        SLOT_HEADER_BYTES
            .checked_add(slot_bytes)
            .ok_or(RingError::LayoutOverflow)?,
        SLOT_STRIDE_ALIGNMENT,
    )?;
    let mapping_bytes = MAPPING_HEADER_BYTES
        .checked_add(
            slot_stride
                .checked_mul(u64::from(slot_count))
                .ok_or(RingError::LayoutOverflow)?,
        )
        .ok_or(RingError::LayoutOverflow)?;
    if mapping_bytes > MAX_MAPPING_BYTES {
        return Err(RingError::MappingTooLarge {
            requested: mapping_bytes,
            maximum: MAX_MAPPING_BYTES,
        });
    }
    Ok(mapping_bytes)
}

/// Immutable ABI 2 descriptor retained by every endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingV2Descriptor {
    layout: RingV2Layout,
    generation: u64,
}

impl RingV2Descriptor {
    /// Creates a non-zero generation descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error for generation zero or any invalid bounded layout.
    pub fn new(
        width: u32,
        height: u32,
        maximum_batch: u32,
        slot_count: u32,
        generation: u64,
    ) -> Result<Self, RingError> {
        if generation == 0 {
            return Err(RingError::InvalidGeneration);
        }
        Ok(Self {
            layout: RingV2Layout::new(width, height, maximum_batch, slot_count)?,
            generation,
        })
    }

    #[must_use]
    pub const fn layout(self) -> RingV2Layout {
        self.layout
    }

    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

/// Consistent bounded-queue counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingV2State {
    producer_sequence: u64,
    consumer_sequence: u64,
    occupancy: u32,
    available_capacity: u32,
}

impl RingV2State {
    #[must_use]
    pub const fn producer_sequence(self) -> u64 {
        self.producer_sequence
    }

    #[must_use]
    pub const fn consumer_sequence(self) -> u64 {
        self.consumer_sequence
    }

    #[must_use]
    pub const fn occupancy(self) -> u32 {
        self.occupancy
    }

    #[must_use]
    pub const fn available_capacity(self) -> u32 {
        self.available_capacity
    }
}

/// Metadata bound to one decoded batch publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchV2Metadata {
    generation: u64,
    slot_sequence: u64,
    logical_sequence: u64,
    session_id: [u8; 16],
    batch: u32,
}

impl BatchV2Metadata {
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    #[must_use]
    pub const fn slot_sequence(self) -> u64 {
        self.slot_sequence
    }

    #[must_use]
    pub const fn logical_sequence(self) -> u64 {
        self.logical_sequence
    }

    #[must_use]
    pub const fn session_id(self) -> [u8; 16] {
        self.session_id
    }

    #[must_use]
    pub const fn batch(self) -> u32 {
        self.batch
    }
}

/// One copied contiguous CPU `uint8 [N,H,W,4]` batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaBatchV2 {
    metadata: BatchV2Metadata,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl RgbaBatchV2 {
    #[must_use]
    pub const fn metadata(&self) -> BatchV2Metadata {
        self.metadata
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
}

/// Result of a non-blocking batch publish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteV2Status {
    Written(BatchV2Metadata),
    Backpressure { queued: u32, capacity: u32 },
}

/// Result of a non-blocking batch consume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadV2Status {
    Batch(RgbaBatchV2),
    Empty,
}

pub(crate) fn mapping_len(layout: RingV2Layout) -> Result<usize, RingError> {
    usize::try_from(layout.mapping_bytes).map_err(|_| RingError::LayoutOverflow)
}

pub(crate) fn initialize_mapping(
    mapping: &mut [u8],
    descriptor: RingV2Descriptor,
) -> Result<(), RingError> {
    let expected = mapping_len(descriptor.layout)?;
    if mapping.len() != expected {
        return Err(RingError::InvalidMappingLength {
            actual: u64::try_from(mapping.len()).map_err(|_| RingError::LayoutOverflow)?,
        });
    }
    mapping.fill(0);
    mapping[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + MAGIC.len()].copy_from_slice(&MAGIC);
    put_u32(mapping, HEADER_ABI_OFFSET, ABI_VERSION);
    put_u32(mapping, HEADER_BYTES_OFFSET, MAPPING_HEADER_FIELD);
    put_u64(mapping, HEADER_GENERATION_OFFSET, descriptor.generation);
    put_u64(mapping, HEADER_STATE_OFFSET, STATE_READY);
    put_u32(
        mapping,
        HEADER_SLOT_COUNT_OFFSET,
        descriptor.layout.slot_count,
    );
    put_u32(mapping, HEADER_SLOT_HEADER_BYTES_OFFSET, SLOT_HEADER_FIELD);
    put_u32(mapping, HEADER_WIDTH_OFFSET, descriptor.layout.width);
    put_u32(mapping, HEADER_HEIGHT_OFFSET, descriptor.layout.height);
    put_u64(
        mapping,
        HEADER_FRAME_STRIDE_OFFSET,
        descriptor.layout.frame_stride_bytes,
    );
    put_u64(
        mapping,
        HEADER_SLOT_BYTES_OFFSET,
        descriptor.layout.slot_bytes,
    );
    put_u64(
        mapping,
        HEADER_SLOT_STRIDE_OFFSET,
        descriptor.layout.slot_stride_bytes,
    );
    put_u64(
        mapping,
        HEADER_MAPPING_BYTES_OFFSET,
        descriptor.layout.mapping_bytes,
    );
    put_u32(
        mapping,
        HEADER_MAX_BATCH_OFFSET,
        descriptor.layout.maximum_batch,
    );
    put_u32(mapping, HEADER_PIXEL_FORMAT_OFFSET, PIXEL_FORMAT_RGBA8);
    Ok(())
}

pub(crate) fn validate_mapping_header(
    mapping: &[u8],
    mapping_bytes: u64,
) -> Result<RingV2Descriptor, RingError> {
    if mapping.len() < MAPPING_HEADER_LEN {
        return Err(RingError::InvalidMappingLength {
            actual: u64::try_from(mapping.len()).map_err(|_| RingError::LayoutOverflow)?,
        });
    }
    if mapping[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + MAGIC.len()] != MAGIC {
        return Err(RingError::InvalidMagic);
    }
    let abi = get_u32(mapping, HEADER_ABI_OFFSET);
    if abi != ABI_VERSION {
        return Err(RingError::UnsupportedAbi { actual: abi });
    }
    require_header(
        get_u32(mapping, HEADER_BYTES_OFFSET) == MAPPING_HEADER_FIELD,
        "header_bytes",
    )?;
    require_ready(mapping)?;
    let generation = mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).load(Ordering::Acquire);
    require_header(generation != 0, "generation")?;
    let layout = RingV2Layout::new(
        get_u32(mapping, HEADER_WIDTH_OFFSET),
        get_u32(mapping, HEADER_HEIGHT_OFFSET),
        get_u32(mapping, HEADER_MAX_BATCH_OFFSET),
        get_u32(mapping, HEADER_SLOT_COUNT_OFFSET),
    )?;
    require_header(
        get_u32(mapping, HEADER_SLOT_HEADER_BYTES_OFFSET) == SLOT_HEADER_FIELD,
        "slot_header_bytes",
    )?;
    require_header(
        get_u64(mapping, HEADER_FRAME_STRIDE_OFFSET) == layout.frame_stride_bytes,
        "frame_stride_bytes",
    )?;
    require_header(
        get_u64(mapping, HEADER_SLOT_BYTES_OFFSET) == layout.slot_bytes,
        "slot_bytes",
    )?;
    require_header(
        get_u64(mapping, HEADER_SLOT_STRIDE_OFFSET) == layout.slot_stride_bytes,
        "slot_stride_bytes",
    )?;
    require_header(
        get_u64(mapping, HEADER_MAPPING_BYTES_OFFSET) == layout.mapping_bytes,
        "mapping_bytes",
    )?;
    require_header(mapping_bytes == layout.mapping_bytes, "mapped_length")?;
    require_header(
        usize::try_from(mapping_bytes).ok() == Some(mapping.len()),
        "view_length",
    )?;
    require_header(
        get_u32(mapping, HEADER_PIXEL_FORMAT_OFFSET) == PIXEL_FORMAT_RGBA8,
        "pixel_format",
    )?;
    require_header(
        mapping[HEADER_RESERVED_OFFSET..MAPPING_HEADER_LEN]
            .iter()
            .all(|byte| *byte == 0),
        "reserved",
    )?;
    require_header(
        mapped::atomic_u64(mapping, HEADER_PRODUCER_CLAIM_OFFSET).load(Ordering::Acquire) <= 1,
        "producer_claim",
    )?;
    require_header(
        mapped::atomic_u64(mapping, HEADER_CONSUMER_CLAIM_OFFSET).load(Ordering::Acquire) <= 1,
        "consumer_claim",
    )?;
    let descriptor = RingV2Descriptor { layout, generation };
    mapping_state(mapping, descriptor)?;
    Ok(descriptor)
}

pub(crate) fn claim_producer(mapping: &[u8]) -> Result<(), RingError> {
    require_ready(mapping)?;
    mapped::atomic_u64(mapping, HEADER_PRODUCER_CLAIM_OFFSET)
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| RingError::ProducerAlreadyClaimed)
}

pub(crate) fn release_producer(mapping: &[u8]) {
    mapped::atomic_u64(mapping, HEADER_PRODUCER_CLAIM_OFFSET).store(0, Ordering::Release);
}

pub(crate) fn claim_consumer(mapping: &[u8]) -> Result<(), RingError> {
    require_ready(mapping)?;
    mapped::atomic_u64(mapping, HEADER_CONSUMER_CLAIM_OFFSET)
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| RingError::ConsumerAlreadyClaimed)
}

pub(crate) fn release_consumer(mapping: &[u8]) {
    mapped::atomic_u64(mapping, HEADER_CONSUMER_CLAIM_OFFSET).store(0, Ordering::Release);
}

pub(crate) fn mapping_state(
    mapping: &[u8],
    descriptor: RingV2Descriptor,
) -> Result<RingV2State, RingError> {
    ensure_live(mapping, descriptor.generation)?;
    let (producer_sequence, consumer_sequence, depth) = load_consistent_sequences(mapping)?;
    ensure_live(mapping, descriptor.generation)?;
    if depth > u64::from(descriptor.layout.slot_count) {
        return Err(RingError::CorruptSequences {
            produced: producer_sequence,
            consumed: consumer_sequence,
        });
    }
    let occupancy = u32::try_from(depth).map_err(|_| RingError::LayoutOverflow)?;
    Ok(RingV2State {
        producer_sequence,
        consumer_sequence,
        occupancy,
        available_capacity: descriptor.layout.slot_count - occupancy,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_mapping_batch(
    mapping: &mut [u8],
    descriptor: RingV2Descriptor,
    session_id: [u8; 16],
    logical_sequence: u64,
    batch: u32,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<WriteV2Status, RingError> {
    ensure_live(mapping, descriptor.generation)?;
    if session_id.iter().all(|byte| *byte == 0) {
        return Err(RingError::InvalidSessionId);
    }
    if logical_sequence == 0 {
        return Err(RingError::InvalidLogicalSequence);
    }
    if !(1..=descriptor.layout.maximum_batch).contains(&batch) {
        return Err(RingError::InvalidBatchCount { actual: batch });
    }
    if width != descriptor.layout.width || height != descriptor.layout.height {
        return Err(RingError::InvalidDimensions { width, height });
    }
    let expected_u64 = descriptor
        .layout
        .frame_stride_bytes
        .checked_mul(u64::from(batch))
        .ok_or(RingError::LayoutOverflow)?;
    let expected = usize::try_from(expected_u64).map_err(|_| RingError::LayoutOverflow)?;
    if pixels.len() != expected {
        return Err(RingError::BatchLengthMismatch {
            expected,
            actual: pixels.len(),
        });
    }
    let state = mapping_state(mapping, descriptor)?;
    if state.occupancy == descriptor.layout.slot_count {
        return Ok(WriteV2Status::Backpressure {
            queued: state.occupancy,
            capacity: descriptor.layout.slot_count,
        });
    }
    let slot_sequence = state
        .producer_sequence
        .checked_add(1)
        .ok_or(RingError::SequenceExhausted)?;
    let base = slot_base(descriptor.layout, slot_sequence)?;
    mapped::atomic_u64(mapping, base + SLOT_SEQUENCE_OFFSET).store(0, Ordering::Release);
    let slot_end = base
        + usize::try_from(descriptor.layout.slot_stride_bytes)
            .map_err(|_| RingError::LayoutOverflow)?;
    mapping[base + 8..slot_end].fill(0);
    put_u64(
        mapping,
        base + SLOT_GENERATION_OFFSET,
        descriptor.generation,
    );
    put_u64(
        mapping,
        base + SLOT_LOGICAL_SEQUENCE_OFFSET,
        logical_sequence,
    );
    mapping[base + SLOT_SESSION_ID_OFFSET..base + SLOT_SESSION_ID_OFFSET + 16]
        .copy_from_slice(&session_id);
    put_u32(mapping, base + SLOT_BATCH_OFFSET, batch);
    put_u32(mapping, base + SLOT_WIDTH_OFFSET, width);
    put_u32(mapping, base + SLOT_HEIGHT_OFFSET, height);
    put_u64(
        mapping,
        base + SLOT_FRAME_STRIDE_OFFSET,
        descriptor.layout.frame_stride_bytes,
    );
    put_u64(mapping, base + SLOT_PAYLOAD_BYTES_OFFSET, expected_u64);
    let payload_start = base + SLOT_HEADER_LEN;
    mapping[payload_start..payload_start + expected].copy_from_slice(pixels);
    mapped::atomic_u64(mapping, base + SLOT_SEQUENCE_OFFSET)
        .store(slot_sequence, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_PRODUCER_SEQUENCE_OFFSET)
        .store(slot_sequence, Ordering::Release);
    Ok(WriteV2Status::Written(BatchV2Metadata {
        generation: descriptor.generation,
        slot_sequence,
        logical_sequence,
        session_id,
        batch,
    }))
}

pub(crate) fn read_mapping_batch(
    mapping: &[u8],
    descriptor: RingV2Descriptor,
) -> Result<ReadV2Status, RingError> {
    let state = mapping_state(mapping, descriptor)?;
    if state.occupancy == 0 {
        return Ok(ReadV2Status::Empty);
    }
    let slot_sequence = state
        .consumer_sequence
        .checked_add(1)
        .ok_or(RingError::SequenceExhausted)?;
    let base = slot_base(descriptor.layout, slot_sequence)?;
    let committed =
        mapped::atomic_u64(mapping, base + SLOT_SEQUENCE_OFFSET).load(Ordering::Acquire);
    if committed != slot_sequence {
        ensure_live(mapping, descriptor.generation)?;
        return Err(RingError::CorruptSlot {
            sequence: slot_sequence,
        });
    }
    let batch = get_u32(mapping, base + SLOT_BATCH_OFFSET);
    let logical_sequence = get_u64(mapping, base + SLOT_LOGICAL_SEQUENCE_OFFSET);
    let payload_bytes = get_u64(mapping, base + SLOT_PAYLOAD_BYTES_OFFSET);
    let expected_payload = descriptor
        .layout
        .frame_stride_bytes
        .checked_mul(u64::from(batch))
        .ok_or(RingError::LayoutOverflow)?;
    let session_id: [u8; 16] = mapping
        [base + SLOT_SESSION_ID_OFFSET..base + SLOT_SESSION_ID_OFFSET + 16]
        .try_into()
        .expect("fixed session ID field is in bounds");
    let valid = get_u64(mapping, base + SLOT_GENERATION_OFFSET) == descriptor.generation
        && logical_sequence != 0
        && session_id.iter().any(|byte| *byte != 0)
        && (1..=descriptor.layout.maximum_batch).contains(&batch)
        && get_u32(mapping, base + SLOT_WIDTH_OFFSET) == descriptor.layout.width
        && get_u32(mapping, base + SLOT_HEIGHT_OFFSET) == descriptor.layout.height
        && get_u64(mapping, base + SLOT_FRAME_STRIDE_OFFSET)
            == descriptor.layout.frame_stride_bytes
        && payload_bytes == expected_payload
        && payload_bytes <= descriptor.layout.slot_bytes
        && mapping[base + SLOT_RESERVED_OFFSET..base + SLOT_HEADER_LEN]
            .iter()
            .all(|byte| *byte == 0);
    if !valid {
        return Err(RingError::CorruptSlot {
            sequence: slot_sequence,
        });
    }
    let payload_start = base + SLOT_HEADER_LEN;
    let payload_len = usize::try_from(payload_bytes).map_err(|_| RingError::LayoutOverflow)?;
    let payload_end = payload_start
        .checked_add(payload_len)
        .ok_or(RingError::LayoutOverflow)?;
    let slot_end = base
        + usize::try_from(descriptor.layout.slot_stride_bytes)
            .map_err(|_| RingError::LayoutOverflow)?;
    if payload_end > slot_end || mapping[payload_end..slot_end].iter().any(|byte| *byte != 0) {
        return Err(RingError::CorruptSlot {
            sequence: slot_sequence,
        });
    }
    let pixels = mapping[payload_start..payload_end].to_vec();
    ensure_live(mapping, descriptor.generation)?;
    mapped::atomic_u64(mapping, HEADER_CONSUMER_SEQUENCE_OFFSET)
        .store(slot_sequence, Ordering::Release);
    Ok(ReadV2Status::Batch(RgbaBatchV2 {
        metadata: BatchV2Metadata {
            generation: descriptor.generation,
            slot_sequence,
            logical_sequence,
            session_id,
            batch,
        },
        width: descriptor.layout.width,
        height: descriptor.layout.height,
        pixels,
    }))
}

pub(crate) fn reset_mapping_generation(
    mapping: &mut [u8],
    descriptor: &mut RingV2Descriptor,
    new_generation: u64,
) -> Result<(), RingError> {
    ensure_live(mapping, descriptor.generation)?;
    if new_generation <= descriptor.generation {
        return Err(RingError::GenerationNotIncreasing {
            current: descriptor.generation,
            requested: new_generation,
        });
    }
    mapped::atomic_u64(mapping, HEADER_STATE_OFFSET)
        .compare_exchange(
            STATE_READY,
            STATE_RESETTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .map_err(|actual| RingError::InvalidLifecycleState { actual })?;
    mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).store(0, Ordering::Release);
    for index in 0..u64::from(descriptor.layout.slot_count) {
        let base = slot_base_for_index(descriptor.layout, index)?;
        mapped::atomic_u64(mapping, base + SLOT_SEQUENCE_OFFSET).store(0, Ordering::Release);
        let slot_end = base
            + usize::try_from(descriptor.layout.slot_stride_bytes)
                .map_err(|_| RingError::LayoutOverflow)?;
        mapping[base + 8..slot_end].fill(0);
    }
    mapped::atomic_u64(mapping, HEADER_PRODUCER_SEQUENCE_OFFSET).store(0, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_CONSUMER_SEQUENCE_OFFSET).store(0, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).store(new_generation, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_STATE_OFFSET).store(STATE_READY, Ordering::Release);
    descriptor.generation = new_generation;
    Ok(())
}

pub(crate) fn adopt_mapping_generation(
    mapping: &[u8],
    descriptor: &mut RingV2Descriptor,
    new_generation: u64,
) -> Result<(), RingError> {
    if new_generation <= descriptor.generation {
        return Err(RingError::GenerationNotIncreasing {
            current: descriptor.generation,
            requested: new_generation,
        });
    }
    require_ready(mapping)?;
    let actual = mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).load(Ordering::Acquire);
    if actual != new_generation {
        return Err(RingError::GenerationMismatch {
            expected: new_generation,
            actual,
        });
    }
    let (produced, consumed, depth) = load_consistent_sequences(mapping)?;
    if produced != 0 || consumed != 0 || depth != 0 {
        return Err(RingError::CorruptSequences { produced, consumed });
    }
    for index in 0..u64::from(descriptor.layout.slot_count) {
        let base = slot_base_for_index(descriptor.layout, index)?;
        if mapped::atomic_u64(mapping, base + SLOT_SEQUENCE_OFFSET).load(Ordering::Acquire) != 0 {
            return Err(RingError::CorruptSlot {
                sequence: index + 1,
            });
        }
    }
    descriptor.generation = new_generation;
    Ok(())
}

fn ensure_live(mapping: &[u8], expected_generation: u64) -> Result<(), RingError> {
    require_ready(mapping)?;
    let actual = mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).load(Ordering::Acquire);
    if actual == expected_generation {
        Ok(())
    } else {
        Err(RingError::GenerationChanged {
            expected: expected_generation,
            actual,
        })
    }
}

fn require_ready(mapping: &[u8]) -> Result<(), RingError> {
    let actual = mapped::atomic_u64(mapping, HEADER_STATE_OFFSET).load(Ordering::Acquire);
    if actual == STATE_READY {
        Ok(())
    } else {
        Err(RingError::InvalidLifecycleState { actual })
    }
}

fn load_consistent_sequences(mapping: &[u8]) -> Result<(u64, u64, u64), RingError> {
    for _ in 0..8 {
        let consumed_before =
            mapped::atomic_u64(mapping, HEADER_CONSUMER_SEQUENCE_OFFSET).load(Ordering::Acquire);
        let produced =
            mapped::atomic_u64(mapping, HEADER_PRODUCER_SEQUENCE_OFFSET).load(Ordering::Acquire);
        let consumed_after =
            mapped::atomic_u64(mapping, HEADER_CONSUMER_SEQUENCE_OFFSET).load(Ordering::Acquire);
        if consumed_before == consumed_after {
            let Some(depth) = produced.checked_sub(consumed_after) else {
                return Err(RingError::CorruptSequences {
                    produced,
                    consumed: consumed_after,
                });
            };
            return Ok((produced, consumed_after, depth));
        }
    }
    Err(RingError::CorruptHeader {
        field: "sequence_snapshot",
    })
}

fn slot_base(layout: RingV2Layout, sequence: u64) -> Result<usize, RingError> {
    let index = (sequence - 1) % u64::from(layout.slot_count);
    slot_base_for_index(layout, index)
}

fn slot_base_for_index(layout: RingV2Layout, index: u64) -> Result<usize, RingError> {
    usize::try_from(
        MAPPING_HEADER_BYTES
            .checked_add(
                layout
                    .slot_stride_bytes
                    .checked_mul(index)
                    .ok_or(RingError::LayoutOverflow)?,
            )
            .ok_or(RingError::LayoutOverflow)?,
    )
    .map_err(|_| RingError::LayoutOverflow)
}

fn align_up(value: u64, alignment: u64) -> Result<u64, RingError> {
    value
        .checked_add(alignment - 1)
        .map(|sum| sum & !(alignment - 1))
        .ok_or(RingError::LayoutOverflow)
}

fn require_header(condition: bool, field: &'static str) -> Result<(), RingError> {
    if condition {
        Ok(())
    } else {
        Err(RingError::CorruptHeader { field })
    }
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed ABI u32 field is in bounds"),
    )
}

fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed ABI u64 field is in bounds"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(generation: u64) -> RingV2Descriptor {
        RingV2Descriptor::new(3, 2, 4, 3, generation).expect("valid descriptor")
    }

    fn mapping(descriptor: RingV2Descriptor) -> Vec<u8> {
        let mut bytes = vec![0; mapping_len(descriptor.layout()).expect("mapping length")];
        initialize_mapping(&mut bytes, descriptor).expect("initialize mapping");
        bytes
    }

    #[test]
    fn exact_descriptor_layout_and_batch_round_trip() {
        let descriptor = descriptor(7);
        let layout = descriptor.layout();
        assert_eq!(layout.frame_stride_bytes(), 24);
        assert_eq!(layout.slot_bytes(), 96);
        assert_eq!(layout.slot_stride_bytes(), 4096);
        assert_eq!(layout.mapping_bytes(), 16_384);
        let mut bytes = mapping(descriptor);
        assert_eq!(
            validate_mapping_header(&bytes, 16_384).expect("valid mapped header"),
            descriptor
        );
        claim_producer(&bytes).expect("producer claim");
        claim_consumer(&bytes).expect("consumer claim");
        let session = [9; 16];
        let pixels = vec![0x5a; 48];
        let WriteV2Status::Written(metadata) =
            write_mapping_batch(&mut bytes, descriptor, session, 42, 2, 3, 2, &pixels)
                .expect("write batch")
        else {
            panic!("empty mapping must accept one batch");
        };
        assert_eq!(metadata.slot_sequence(), 1);
        assert_eq!(metadata.logical_sequence(), 42);
        let ReadV2Status::Batch(batch) =
            read_mapping_batch(&bytes, descriptor).expect("read batch")
        else {
            panic!("committed batch must be visible");
        };
        assert_eq!(batch.metadata(), metadata);
        assert_eq!(batch.pixels(), pixels);
        assert_eq!(
            mapping_state(&bytes, descriptor)
                .expect("state")
                .occupancy(),
            0
        );
    }

    #[test]
    fn bounds_state_and_generation_are_strict() {
        assert!(matches!(
            RingV2Descriptor::new(1, 1, 0, 2, 1),
            Err(RingError::InvalidBatchCount { actual: 0 })
        ));
        assert!(matches!(
            RingV2Descriptor::new(1, 1, 1, 1, 1),
            Err(RingError::InvalidSlotCount { actual: 1 })
        ));
        let mut bytes = mapping(descriptor(11));
        put_u64(&mut bytes, HEADER_STATE_OFFSET, 77);
        assert!(matches!(
            validate_mapping_header(&bytes, bytes.len() as u64),
            Err(RingError::InvalidLifecycleState { actual: 77 })
        ));
        put_u64(&mut bytes, HEADER_STATE_OFFSET, STATE_READY);
        put_u64(&mut bytes, HEADER_GENERATION_OFFSET, 12);
        assert!(matches!(
            mapping_state(&bytes, descriptor(11)),
            Err(RingError::GenerationChanged {
                expected: 11,
                actual: 12
            })
        ));
    }

    #[test]
    fn reset_invalidates_old_endpoint_and_requires_exact_adoption() {
        let mut producer_descriptor = descriptor(20);
        let mut consumer_descriptor = producer_descriptor;
        let mut bytes = mapping(producer_descriptor);
        write_mapping_batch(
            &mut bytes,
            producer_descriptor,
            [1; 16],
            1,
            1,
            3,
            2,
            &[0; 24],
        )
        .expect("write old batch");
        reset_mapping_generation(&mut bytes, &mut producer_descriptor, 21)
            .expect("reset generation");
        assert!(matches!(
            read_mapping_batch(&bytes, consumer_descriptor),
            Err(RingError::GenerationChanged {
                expected: 20,
                actual: 21
            })
        ));
        adopt_mapping_generation(&bytes, &mut consumer_descriptor, 21).expect("adopt exact reset");
        assert_eq!(
            read_mapping_batch(&bytes, consumer_descriptor).expect("empty after reset"),
            ReadV2Status::Empty
        );
    }
}
