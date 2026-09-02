//! Checked layout and shared-memory core for `LatentDeck` RGB Ring ABI 1.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

use memmap2::MmapMut;
use thiserror::Error;

#[cfg(not(target_endian = "little"))]
compile_error!("LatentDeck RGB Ring ABI 1 requires a little-endian target");
#[cfg(not(target_has_atomic = "64"))]
compile_error!("LatentDeck RGB Ring ABI 1 requires lock-free-width 64-bit atomics");

/// The stable RGB ring ABI version.
pub const ABI_VERSION: u32 = 1;
/// Bytes reserved for the mapping-wide header.
pub const MAPPING_HEADER_BYTES: u64 = 4096;
/// Bytes reserved before every frame payload.
pub const SLOT_HEADER_BYTES: u64 = 128;
/// The fixed number of bounded frame slots.
pub const RING_SLOT_COUNT: u32 = 24;
/// Required row alignment for RGBA8 frame data.
pub const ROW_STRIDE_ALIGNMENT: u32 = 256;
/// Required alignment of every complete slot in the mapping.
pub const SLOT_STRIDE_ALIGNMENT: u64 = 4096;
/// Maximum accepted file mapping size.
pub const MAX_MAPPING_BYTES: u64 = 256 * 1024 * 1024;

const RGBA8_BYTES_PER_PIXEL: u32 = 4;
const MAPPING_HEADER_LEN: usize = 4096;
const MAPPING_HEADER_FIELD: u32 = 4096;
const SLOT_HEADER_LEN: usize = 128;
const SLOT_HEADER_FIELD: u32 = 128;

/// Stable byte-level constants for non-Rust ABI 1 implementations.
pub mod abi1 {
    /// Eight-byte file/mapping signature.
    pub const MAGIC: [u8; 8] = *b"LDRGBR01";
    /// `PixelFormat::Rgba8` wire value.
    pub const PIXEL_FORMAT_RGBA8: u32 = 1;
    /// `FrameOrigin::TopLeft` wire value.
    pub const ORIGIN_TOP_LEFT: u32 = 1;

    pub const HEADER_MAGIC_OFFSET: usize = 0;
    pub const HEADER_ABI_OFFSET: usize = 8;
    pub const HEADER_BYTES_OFFSET: usize = 12;
    pub const HEADER_GENERATION_OFFSET: usize = 16;
    pub const HEADER_SLOT_COUNT_OFFSET: usize = 24;
    pub const HEADER_SLOT_BYTES_OFFSET: usize = 28;
    pub const HEADER_PIXEL_FORMAT_OFFSET: usize = 32;
    pub const HEADER_ORIGIN_OFFSET: usize = 36;
    pub const HEADER_WIDTH_OFFSET: usize = 40;
    pub const HEADER_HEIGHT_OFFSET: usize = 44;
    pub const HEADER_ROW_STRIDE_OFFSET: usize = 48;
    pub const HEADER_PAYLOAD_BYTES_OFFSET: usize = 52;
    pub const HEADER_SLOT_STRIDE_OFFSET: usize = 56;
    pub const HEADER_MAPPING_BYTES_OFFSET: usize = 64;
    pub const HEADER_PRODUCER_SEQUENCE_OFFSET: usize = 72;
    pub const HEADER_CONSUMER_SEQUENCE_OFFSET: usize = 80;
    pub const HEADER_CONSUMER_CLAIM_OFFSET: usize = 88;
    pub const HEADER_PRODUCER_CLAIM_OFFSET: usize = 96;
    pub const HEADER_RESERVED_OFFSET: usize = 104;

    pub const SLOT_SEQUENCE_OFFSET: usize = 0;
    pub const SLOT_GENERATION_OFFSET: usize = 8;
    pub const SLOT_TIMESTAMP_OFFSET: usize = 16;
    pub const SLOT_PAYLOAD_BYTES_OFFSET: usize = 24;
    pub const SLOT_WIDTH_OFFSET: usize = 28;
    pub const SLOT_HEIGHT_OFFSET: usize = 32;
    pub const SLOT_ROW_STRIDE_OFFSET: usize = 36;
    pub const SLOT_PIXEL_FORMAT_OFFSET: usize = 40;
    pub const SLOT_ORIGIN_OFFSET: usize = 44;
    pub const SLOT_RESERVED_OFFSET: usize = 48;
}

use abi1::{
    HEADER_ABI_OFFSET, HEADER_BYTES_OFFSET, HEADER_CONSUMER_CLAIM_OFFSET,
    HEADER_CONSUMER_SEQUENCE_OFFSET, HEADER_GENERATION_OFFSET, HEADER_HEIGHT_OFFSET,
    HEADER_MAGIC_OFFSET, HEADER_MAPPING_BYTES_OFFSET, HEADER_ORIGIN_OFFSET,
    HEADER_PAYLOAD_BYTES_OFFSET, HEADER_PIXEL_FORMAT_OFFSET, HEADER_PRODUCER_CLAIM_OFFSET,
    HEADER_PRODUCER_SEQUENCE_OFFSET, HEADER_RESERVED_OFFSET, HEADER_ROW_STRIDE_OFFSET,
    HEADER_SLOT_BYTES_OFFSET, HEADER_SLOT_COUNT_OFFSET, HEADER_SLOT_STRIDE_OFFSET,
    HEADER_WIDTH_OFFSET, MAGIC, ORIGIN_TOP_LEFT, PIXEL_FORMAT_RGBA8, SLOT_GENERATION_OFFSET,
    SLOT_HEIGHT_OFFSET, SLOT_ORIGIN_OFFSET, SLOT_PAYLOAD_BYTES_OFFSET, SLOT_PIXEL_FORMAT_OFFSET,
    SLOT_RESERVED_OFFSET, SLOT_ROW_STRIDE_OFFSET, SLOT_SEQUENCE_OFFSET, SLOT_TIMESTAMP_OFFSET,
    SLOT_WIDTH_OFFSET,
};

/// Pixel representation fixed by ABI 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum PixelFormat {
    /// Eight unorm bits each in red, green, blue, alpha byte order.
    Rgba8 = PIXEL_FORMAT_RGBA8,
}

/// Pixel coordinate origin fixed by ABI 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FrameOrigin {
    /// The first stored row is the top row of the image.
    TopLeft = ORIGIN_TOP_LEFT,
}

/// A fully checked ABI 1 mapping layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingLayout {
    width: u32,
    height: u32,
    row_stride: u32,
    payload_bytes: u64,
    slot_stride: u64,
    mapping_bytes: u64,
}

impl RingLayout {
    /// Computes the exact ABI layout for an RGBA8, top-left-origin frame.
    ///
    /// # Errors
    ///
    /// Returns an error for zero dimensions, arithmetic overflow, or a mapping
    /// larger than [`MAX_MAPPING_BYTES`].
    pub fn new(width: u32, height: u32) -> Result<Self, RingError> {
        if width == 0 || height == 0 {
            return Err(RingError::InvalidDimensions { width, height });
        }

        let tight_row = width
            .checked_mul(RGBA8_BYTES_PER_PIXEL)
            .ok_or(RingError::LayoutOverflow)?;
        let row_stride = tight_row
            .checked_add(ROW_STRIDE_ALIGNMENT - 1)
            .map(|value| value & !(ROW_STRIDE_ALIGNMENT - 1))
            .ok_or(RingError::LayoutOverflow)?;
        let payload_bytes = u64::from(row_stride)
            .checked_mul(u64::from(height))
            .ok_or(RingError::LayoutOverflow)?;
        let slot_used_bytes = SLOT_HEADER_BYTES
            .checked_add(payload_bytes)
            .ok_or(RingError::LayoutOverflow)?;
        let slot_stride = slot_used_bytes
            .checked_add(SLOT_STRIDE_ALIGNMENT - 1)
            .map(|value| value & !(SLOT_STRIDE_ALIGNMENT - 1))
            .ok_or(RingError::LayoutOverflow)?;
        let mapping_bytes = u64::from(RING_SLOT_COUNT)
            .checked_mul(slot_stride)
            .and_then(|slots| MAPPING_HEADER_BYTES.checked_add(slots))
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
            row_stride,
            payload_bytes,
            slot_stride,
            mapping_bytes,
        })
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// Padded bytes per row.
    #[must_use]
    pub const fn row_stride(self) -> u32 {
        self.row_stride
    }

    /// Bytes occupied by one padded pixel payload.
    #[must_use]
    pub const fn payload_bytes(self) -> u64 {
        self.payload_bytes
    }

    /// Bytes occupied by a slot header and its payload.
    #[must_use]
    pub const fn slot_stride(self) -> u64 {
        self.slot_stride
    }

    /// Exact mapping/file length.
    #[must_use]
    pub const fn mapping_bytes(self) -> u64 {
        self.mapping_bytes
    }

    const fn tight_row_bytes(self) -> u32 {
        self.width * RGBA8_BYTES_PER_PIXEL
    }

    fn tight_payload_bytes(self) -> Result<usize, RingError> {
        usize::try_from(
            u64::from(self.tight_row_bytes())
                .checked_mul(u64::from(self.height))
                .ok_or(RingError::LayoutOverflow)?,
        )
        .map_err(|_| RingError::LayoutOverflow)
    }
}

/// Immutable description exchanged through control IPC before opening a ring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingDescriptor {
    layout: RingLayout,
    generation: u64,
}

impl RingDescriptor {
    /// Builds a checked ABI 1 descriptor.
    ///
    /// # Errors
    ///
    /// Returns an error when `generation` is zero or the dimensions cannot be
    /// represented under the ABI limits.
    pub fn new(width: u32, height: u32, generation: u64) -> Result<Self, RingError> {
        if generation == 0 {
            return Err(RingError::InvalidGeneration);
        }
        Ok(Self {
            layout: RingLayout::new(width, height)?,
            generation,
        })
    }

    /// Checked memory layout.
    #[must_use]
    pub const fn layout(self) -> RingLayout {
        self.layout
    }

    /// Non-zero lifecycle generation.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// ABI pixel representation.
    #[must_use]
    pub const fn pixel_format(self) -> PixelFormat {
        PixelFormat::Rgba8
    }

    /// ABI image origin.
    #[must_use]
    pub const fn origin(self) -> FrameOrigin {
        FrameOrigin::TopLeft
    }
}

/// Metadata committed with one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameMetadata {
    generation: u64,
    sequence: u64,
    timestamp_ns: u64,
}

impl FrameMetadata {
    /// Ring lifecycle generation.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Monotonically increasing sequence, starting at one.
    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// Producer-supplied monotonic timestamp in nanoseconds.
    #[must_use]
    pub const fn timestamp_ns(self) -> u64 {
        self.timestamp_ns
    }
}

/// A copied frame whose rows retain ABI padding for direct GPU upload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RgbaFrame {
    metadata: FrameMetadata,
    layout: RingLayout,
    padded_rgba: Vec<u8>,
}

impl RgbaFrame {
    /// Ring lifecycle generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.metadata.generation
    }

    /// Monotonically increasing frame sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.metadata.sequence
    }

    /// Producer-supplied monotonic timestamp in nanoseconds.
    #[must_use]
    pub const fn timestamp_ns(&self) -> u64 {
        self.metadata.timestamp_ns
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.layout.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.layout.height
    }

    /// Padded bytes per row.
    #[must_use]
    pub const fn row_stride(&self) -> u32 {
        self.layout.row_stride
    }

    /// RGBA8 bytes including zeroed row padding.
    #[must_use]
    pub fn padded_rgba(&self) -> &[u8] {
        &self.padded_rgba
    }
}

/// Non-blocking producer result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteStatus {
    /// The frame was committed and is visible to the consumer.
    Written(FrameMetadata),
    /// All 24 slots are outstanding; the caller must drop or retry the frame.
    Backpressure(Backpressure),
}

/// Observable bounded-queue state returned instead of blocking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Backpressure {
    queued: u32,
    capacity: u32,
}

/// One internally consistent snapshot of the bounded queue counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingState {
    producer_sequence: u64,
    consumer_sequence: u64,
    occupancy: u32,
    available_capacity: u32,
}

impl RingState {
    /// Latest sequence committed by the producer, or zero after reset.
    #[must_use]
    pub const fn producer_sequence(self) -> u64 {
        self.producer_sequence
    }

    /// Latest sequence released by the consumer, or zero after reset.
    #[must_use]
    pub const fn consumer_sequence(self) -> u64 {
        self.consumer_sequence
    }

    /// Published frames not yet released by the consumer.
    #[must_use]
    pub const fn occupancy(self) -> u32 {
        self.occupancy
    }

    /// Slots available without overwriting an unread frame.
    #[must_use]
    pub const fn available_capacity(self) -> u32 {
        self.available_capacity
    }

    /// Whether an entire decode cycle fits without partial publication.
    #[must_use]
    pub const fn can_publish(self, frame_count: u32) -> bool {
        frame_count <= self.available_capacity
    }
}

impl Backpressure {
    /// Frames published but not yet released by the consumer.
    #[must_use]
    pub const fn queued(self) -> u32 {
        self.queued
    }

    /// Fixed ABI slot capacity.
    #[must_use]
    pub const fn capacity(self) -> u32 {
        self.capacity
    }
}

/// Non-blocking consumer result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadStatus {
    /// One frame was copied and its slot released to the producer.
    Frame(RgbaFrame),
    /// No published frame is available.
    Empty,
}

/// Test-only file-backed producer.
///
/// Production runtime uses anonymous handles from
/// `crate::windows_ring::WindowsRgbRingProducer`. This endpoint exists solely
/// for deterministic corruption tests and never appears in `ring.bind`.
pub struct TestFileRgbRingProducer {
    _file: File,
    mapping: MmapMut,
    descriptor: RingDescriptor,
    claimed: bool,
}

impl TestFileRgbRingProducer {
    /// Creates a new mapping file and initializes ABI 1. Existing paths are
    /// never truncated or overwritten.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the path already exists or cannot be mapped.
    pub fn create(path: impl AsRef<Path>, descriptor: RingDescriptor) -> Result<Self, RingError> {
        let mapping_len = mapping_len(descriptor.layout)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(RingError::Io)?;
        file.set_len(descriptor.layout.mapping_bytes)
            .map_err(RingError::Io)?;
        let mut mapping = mapped::map_file_mut(&file, mapping_len).map_err(RingError::Io)?;
        initialize_mapping(&mut mapping, descriptor)?;
        claim_producer(&mapping)?;
        mapping
            .flush_range(0, MAPPING_HEADER_LEN)
            .map_err(RingError::Io)?;

        Ok(Self {
            _file: file,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    /// Opens an initialized mapping and atomically claims the single producer
    /// role. This is the worker handoff after the creator drops its endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid mapping, stale generation, or an
    /// already claimed producer endpoint.
    pub fn open(path: impl AsRef<Path>, expected_generation: u64) -> Result<Self, RingError> {
        let (file, mapping, descriptor) = open_mapping(path.as_ref(), expected_generation)?;
        claim_producer(&mapping)?;
        Ok(Self {
            _file: file,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    /// Mapping descriptor advertised to the worker.
    #[must_use]
    pub const fn descriptor(&self) -> RingDescriptor {
        self.descriptor
    }

    /// Returns a consistent sequence/capacity snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the mapped generation or counters are corrupt.
    pub fn state(&self) -> Result<RingState, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Reports whether a complete frame batch fits without partial publish.
    ///
    /// # Errors
    ///
    /// Returns an error when the mapped generation or counters are corrupt.
    pub fn can_publish(&self, frame_count: u32) -> Result<bool, RingError> {
        Ok(self.state()?.can_publish(frame_count))
    }

    /// Starts a strictly newer stream generation and atomically invalidates
    /// every previously committed slot.
    ///
    /// This operation is performed only while decode publication is quiescent;
    /// a new cycle must not start until the consumer acknowledges adoption.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale/non-increasing generation or corrupt map.
    pub fn set_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        reset_mapping_generation(&mut self.mapping, &mut self.descriptor, new_generation)
    }

    /// Attempts to publish one tightly packed RGBA8 frame without blocking.
    ///
    /// Row padding is always overwritten with zeroes so data from an earlier
    /// slot use cannot leak to the consumer.
    ///
    /// # Errors
    ///
    /// Returns an error for a wrong input length, corrupt shared counters, or
    /// exhausted sequence space.
    pub fn try_write(
        &mut self,
        tight_rgba: &[u8],
        timestamp_ns: u64,
    ) -> Result<WriteStatus, RingError> {
        write_mapping_frame(&mut self.mapping, self.descriptor, tight_rgba, timestamp_ns)
    }
}

impl Drop for TestFileRgbRingProducer {
    fn drop(&mut self) {
        if self.claimed {
            release_producer(&self.mapping);
            self.claimed = false;
        }
    }
}

/// Test-only file-backed consumer paired with [`TestFileRgbRingProducer`].
pub struct TestFileRgbRingConsumer {
    _file: File,
    mapping: MmapMut,
    descriptor: RingDescriptor,
    claimed: bool,
}

impl TestFileRgbRingConsumer {
    /// Opens and validates an existing producer mapping, then atomically claims
    /// the single consumer role.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid/corrupt ABI header, a stale generation,
    /// a mapping over the hard cap, or an already claimed consumer endpoint.
    pub fn open(path: impl AsRef<Path>, expected_generation: u64) -> Result<Self, RingError> {
        let (file, mapping, descriptor) = open_mapping(path.as_ref(), expected_generation)?;

        claim_consumer(&mapping)?;

        Ok(Self {
            _file: file,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    /// Validated descriptor read from the mapping.
    #[must_use]
    pub const fn descriptor(&self) -> RingDescriptor {
        self.descriptor
    }

    /// Returns a consistent sequence/capacity snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the mapped generation or counters are corrupt.
    pub fn state(&self) -> Result<RingState, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Reports whether a complete frame batch fits without partial publish.
    ///
    /// # Errors
    ///
    /// Returns an error when the mapped generation or counters are corrupt.
    pub fn can_publish(&self, frame_count: u32) -> Result<bool, RingError> {
        Ok(self.state()?.can_publish(frame_count))
    }

    /// Adopts the exact generation acknowledged by the producer after reset.
    /// No old commit or sequence may remain visible.
    ///
    /// # Errors
    ///
    /// Returns an error for stale generations, an unexpected mapped
    /// generation, or a reset that has not fully cleared its atomic state.
    pub fn adopt_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        adopt_mapping_generation(&self.mapping, &mut self.descriptor, new_generation)
    }

    /// Attempts to copy and release the next sequence without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error when shared counters or slot metadata violate ABI 1.
    pub fn try_read(&mut self) -> Result<ReadStatus, RingError> {
        read_mapping_frame(&self.mapping, self.descriptor)
    }
}

impl Drop for TestFileRgbRingConsumer {
    fn drop(&mut self) {
        if self.claimed {
            release_consumer(&self.mapping);
            self.claimed = false;
        }
    }
}

/// Checked RGB ring construction or access failure.
#[derive(Debug, Error)]
pub enum RingError {
    /// Width and height must both be non-zero.
    #[error("RGB ring dimensions must be non-zero, got {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    /// Protocol 2 rings contain between two and 24 independently queued slots.
    #[error("RGB ring slot count must be in [2, 24], got {actual}")]
    InvalidSlotCount { actual: u32 },
    /// A decoded ABI batch contains between one and 24 frames.
    #[error("RGB ring maximum batch must be in [1, 24], got {actual}")]
    InvalidBatchCount { actual: u32 },
    /// A size calculation exceeded the ABI integer range.
    #[error("RGB ring layout arithmetic overflow")]
    LayoutOverflow,
    /// The checked layout exceeds the hard mapping cap.
    #[error("RGB ring mapping requires {requested} bytes, exceeding the {maximum}-byte cap")]
    MappingTooLarge { requested: u64, maximum: u64 },
    /// Lifecycle generation zero is reserved and invalid.
    #[error("RGB ring generation must be non-zero")]
    InvalidGeneration,
    /// Protocol 2 lifecycle state is not a recognized stable state.
    #[error("RGB ring lifecycle state is invalid: {actual}")]
    InvalidLifecycleState { actual: u64 },
    /// Protocol 2 session identifiers must not be the nil UUID.
    #[error("RGB ring session identifier must not be nil")]
    InvalidSessionId,
    /// Protocol 2 logical command sequences start at one.
    #[error("RGB ring logical sequence must be non-zero")]
    InvalidLogicalSequence,
    /// File creation, open, mapping, or initial header flush failed.
    #[error("RGB ring I/O failed: {0}")]
    Io(#[source] io::Error),
    /// A Windows mapping, handle, event, or wait operation failed.
    #[cfg(target_os = "windows")]
    #[error("RGB ring Windows operation failed: {0}")]
    Windows(#[from] windows::core::Error),
    /// An event wait returned a value impossible for an event object.
    #[cfg(target_os = "windows")]
    #[error("RGB ring event wait returned unexpected status {actual}")]
    UnexpectedWaitStatus { actual: u32 },
    /// A shared mapping has an impossible or unsafe length.
    #[error("RGB ring mapping has invalid length {actual}")]
    InvalidMappingLength { actual: u64 },
    /// The eight-byte ABI magic does not match.
    #[error("RGB ring magic is invalid")]
    InvalidMagic,
    /// The mapping declares an unsupported ABI version.
    #[error("unsupported RGB ring ABI version {actual}")]
    UnsupportedAbi { actual: u32 },
    /// A static header field conflicts with its recomputed checked layout.
    #[error("RGB ring header field is invalid: {field}")]
    CorruptHeader { field: &'static str },
    /// The control-plane generation and mapped generation differ.
    #[error("RGB ring generation mismatch: expected {expected}, found {actual}")]
    GenerationMismatch { expected: u64, actual: u64 },
    /// A live endpoint observed a reset before adopting its new generation.
    #[error("RGB ring generation changed: expected {expected}, found {actual}")]
    GenerationChanged { expected: u64, actual: u64 },
    /// Reset generations must be strictly monotonic.
    #[error("RGB ring generation {requested} is not newer than {current}")]
    GenerationNotIncreasing { current: u64, requested: u64 },
    /// ABI 1 permits exactly one live consumer.
    #[error("RGB ring consumer endpoint is already claimed")]
    ConsumerAlreadyClaimed,
    /// ABI 1 permits exactly one live producer.
    #[error("RGB ring producer endpoint is already claimed")]
    ProducerAlreadyClaimed,
    /// A tightly packed input frame has the wrong byte count.
    #[error("RGBA8 frame has {actual} bytes, expected {expected}")]
    FrameLengthMismatch { expected: usize, actual: usize },
    /// A contiguous decoded batch has the wrong byte count.
    #[error("RGBA8 batch has {actual} bytes, expected {expected}")]
    BatchLengthMismatch { expected: usize, actual: usize },
    /// Shared producer/consumer counters are not a valid bounded queue state.
    #[error("RGB ring sequence counters are corrupt: produced {produced}, consumed {consumed}")]
    CorruptSequences { produced: u64, consumed: u64 },
    /// The next sequence cannot be represented.
    #[error("RGB ring sequence space is exhausted")]
    SequenceExhausted,
    /// A published slot does not match the expected sequence or descriptor.
    #[error("RGB ring slot for sequence {sequence} is corrupt")]
    CorruptSlot { sequence: u64 },
}

impl RingError {
    /// Stable machine-readable category for diagnostics and IPC adapters.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDimensions { .. } => "ring_invalid_dimensions",
            Self::InvalidSlotCount { .. } => "ring_invalid_slot_count",
            Self::InvalidBatchCount { .. } => "ring_invalid_batch_count",
            Self::LayoutOverflow => "ring_layout_overflow",
            Self::MappingTooLarge { .. } => "ring_mapping_too_large",
            Self::InvalidGeneration => "ring_invalid_generation",
            Self::InvalidLifecycleState { .. } => "ring_invalid_state",
            Self::InvalidSessionId => "ring_invalid_session_id",
            Self::InvalidLogicalSequence => "ring_invalid_logical_sequence",
            Self::Io(_) => "ring_io",
            #[cfg(target_os = "windows")]
            Self::Windows(_) => "ring_windows",
            #[cfg(target_os = "windows")]
            Self::UnexpectedWaitStatus { .. } => "ring_wait_status",
            Self::InvalidMappingLength { .. } => "ring_invalid_mapping_length",
            Self::InvalidMagic => "ring_invalid_magic",
            Self::UnsupportedAbi { .. } => "ring_unsupported_abi",
            Self::CorruptHeader { .. } => "ring_corrupt_header",
            Self::GenerationMismatch { .. } => "ring_generation_mismatch",
            Self::GenerationChanged { .. } => "ring_generation_changed",
            Self::GenerationNotIncreasing { .. } => "ring_generation_not_increasing",
            Self::ConsumerAlreadyClaimed => "ring_consumer_claimed",
            Self::ProducerAlreadyClaimed => "ring_producer_claimed",
            Self::FrameLengthMismatch { .. } => "ring_frame_length",
            Self::BatchLengthMismatch { .. } => "ring_batch_length",
            Self::CorruptSequences { .. } => "ring_corrupt_sequences",
            Self::SequenceExhausted => "ring_sequence_exhausted",
            Self::CorruptSlot { .. } => "ring_corrupt_slot",
        }
    }
}

pub(crate) fn initialize_mapping(
    mapping: &mut [u8],
    descriptor: RingDescriptor,
) -> Result<(), RingError> {
    let expected = mapping_len(descriptor.layout)?;
    if mapping.len() != expected {
        return Err(RingError::InvalidMappingLength {
            actual: u64::try_from(mapping.len()).map_err(|_| RingError::LayoutOverflow)?,
        });
    }
    mapping.fill(0);
    write_mapping_header(mapping, descriptor)
}

pub(crate) fn claim_producer(mapping: &[u8]) -> Result<(), RingError> {
    mapped::atomic_u64(mapping, HEADER_PRODUCER_CLAIM_OFFSET)
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| RingError::ProducerAlreadyClaimed)
}

pub(crate) fn release_producer(mapping: &[u8]) {
    mapped::atomic_u64(mapping, HEADER_PRODUCER_CLAIM_OFFSET).store(0, Ordering::Release);
}

pub(crate) fn claim_consumer(mapping: &[u8]) -> Result<(), RingError> {
    mapped::atomic_u64(mapping, HEADER_CONSUMER_CLAIM_OFFSET)
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .map(|_| ())
        .map_err(|_| RingError::ConsumerAlreadyClaimed)
}

pub(crate) fn release_consumer(mapping: &[u8]) {
    mapped::atomic_u64(mapping, HEADER_CONSUMER_CLAIM_OFFSET).store(0, Ordering::Release);
}

pub(crate) fn write_mapping_frame(
    mapping: &mut [u8],
    descriptor: RingDescriptor,
    tight_rgba: &[u8],
    timestamp_ns: u64,
) -> Result<WriteStatus, RingError> {
    ensure_generation(mapping, descriptor.generation)?;
    let layout = descriptor.layout;
    let expected = layout.tight_payload_bytes()?;
    if tight_rgba.len() != expected {
        return Err(RingError::FrameLengthMismatch {
            expected,
            actual: tight_rgba.len(),
        });
    }

    let state = mapping_state(mapping, descriptor)?;
    if state.occupancy == RING_SLOT_COUNT {
        return Ok(WriteStatus::Backpressure(Backpressure {
            queued: RING_SLOT_COUNT,
            capacity: RING_SLOT_COUNT,
        }));
    }

    let sequence = state
        .producer_sequence
        .checked_add(1)
        .ok_or(RingError::SequenceExhausted)?;
    let slot_base = slot_base(layout, sequence)?;
    mapped::atomic_u64(mapping, slot_base + SLOT_SEQUENCE_OFFSET).store(0, Ordering::Release);

    let slot_header_end = slot_base + SLOT_HEADER_LEN;
    let slot_end =
        slot_base + usize::try_from(layout.slot_stride).map_err(|_| RingError::LayoutOverflow)?;
    mapping[slot_base + 8..slot_end].fill(0);
    put_u64(
        mapping,
        slot_base + SLOT_GENERATION_OFFSET,
        descriptor.generation,
    );
    put_u64(mapping, slot_base + SLOT_TIMESTAMP_OFFSET, timestamp_ns);
    put_u32(
        mapping,
        slot_base + SLOT_PAYLOAD_BYTES_OFFSET,
        u32::try_from(layout.payload_bytes).map_err(|_| RingError::LayoutOverflow)?,
    );
    put_u32(mapping, slot_base + SLOT_WIDTH_OFFSET, layout.width);
    put_u32(mapping, slot_base + SLOT_HEIGHT_OFFSET, layout.height);
    put_u32(
        mapping,
        slot_base + SLOT_ROW_STRIDE_OFFSET,
        layout.row_stride,
    );
    put_u32(
        mapping,
        slot_base + SLOT_PIXEL_FORMAT_OFFSET,
        PIXEL_FORMAT_RGBA8,
    );
    put_u32(mapping, slot_base + SLOT_ORIGIN_OFFSET, ORIGIN_TOP_LEFT);

    let payload_start = slot_header_end;
    let payload_end = payload_start
        + usize::try_from(layout.payload_bytes).map_err(|_| RingError::LayoutOverflow)?;
    let payload = &mut mapping[payload_start..payload_end];
    let source_row_bytes = layout.tight_row_bytes() as usize;
    let destination_row_bytes = layout.row_stride as usize;
    for (source, destination) in tight_rgba
        .chunks_exact(source_row_bytes)
        .zip(payload.chunks_exact_mut(destination_row_bytes))
    {
        destination[..source_row_bytes].copy_from_slice(source);
    }

    mapped::atomic_u64(mapping, slot_base + SLOT_SEQUENCE_OFFSET)
        .store(sequence, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_PRODUCER_SEQUENCE_OFFSET).store(sequence, Ordering::Release);

    Ok(WriteStatus::Written(FrameMetadata {
        generation: descriptor.generation,
        sequence,
        timestamp_ns,
    }))
}

pub(crate) fn read_mapping_frame(
    mapping: &[u8],
    descriptor: RingDescriptor,
) -> Result<ReadStatus, RingError> {
    ensure_generation(mapping, descriptor.generation)?;
    let state = mapping_state(mapping, descriptor)?;
    if state.occupancy == 0 {
        return Ok(ReadStatus::Empty);
    }

    let sequence = state
        .consumer_sequence
        .checked_add(1)
        .ok_or(RingError::SequenceExhausted)?;
    let layout = descriptor.layout;
    let slot_base = slot_base(layout, sequence)?;
    let committed =
        mapped::atomic_u64(mapping, slot_base + SLOT_SEQUENCE_OFFSET).load(Ordering::Acquire);
    if committed != sequence {
        ensure_generation(mapping, descriptor.generation)?;
        return Err(RingError::CorruptSlot { sequence });
    }
    validate_slot_header(mapping, slot_base, descriptor, sequence)?;

    let payload_start = slot_base + SLOT_HEADER_LEN;
    let payload_end = payload_start
        + usize::try_from(layout.payload_bytes).map_err(|_| RingError::LayoutOverflow)?;
    let padded_rgba = mapping[payload_start..payload_end].to_vec();
    let timestamp_ns = get_u64(mapping, slot_base + SLOT_TIMESTAMP_OFFSET);

    ensure_generation(mapping, descriptor.generation)?;
    mapped::atomic_u64(mapping, HEADER_CONSUMER_SEQUENCE_OFFSET).store(sequence, Ordering::Release);

    Ok(ReadStatus::Frame(RgbaFrame {
        metadata: FrameMetadata {
            generation: descriptor.generation,
            sequence,
            timestamp_ns,
        },
        layout,
        padded_rgba,
    }))
}

fn open_mapping(
    path: &Path,
    expected_generation: u64,
) -> Result<(File, MmapMut, RingDescriptor), RingError> {
    if expected_generation == 0 {
        return Err(RingError::InvalidGeneration);
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(RingError::Io)?;
    let file_bytes = file.metadata().map_err(RingError::Io)?.len();
    if !(MAPPING_HEADER_BYTES..=MAX_MAPPING_BYTES).contains(&file_bytes) {
        return Err(RingError::InvalidMappingLength { actual: file_bytes });
    }
    let mapping = mapped::map_file_mut(
        &file,
        usize::try_from(file_bytes).map_err(|_| RingError::LayoutOverflow)?,
    )
    .map_err(RingError::Io)?;
    let descriptor = validate_mapping_header(&mapping, file_bytes)?;
    if descriptor.generation != expected_generation {
        return Err(RingError::GenerationMismatch {
            expected: expected_generation,
            actual: descriptor.generation,
        });
    }
    Ok((file, mapping, descriptor))
}

pub(crate) fn mapping_len(layout: RingLayout) -> Result<usize, RingError> {
    usize::try_from(layout.mapping_bytes).map_err(|_| RingError::LayoutOverflow)
}

fn checked_queue_depth(produced: u64, consumed: u64) -> Result<u64, RingError> {
    let Some(depth) = produced.checked_sub(consumed) else {
        return Err(RingError::CorruptSequences { produced, consumed });
    };
    if depth > u64::from(RING_SLOT_COUNT) {
        return Err(RingError::CorruptSequences { produced, consumed });
    }
    Ok(depth)
}

pub(crate) fn mapping_state(
    mapping: &[u8],
    descriptor: RingDescriptor,
) -> Result<RingState, RingError> {
    ensure_generation(mapping, descriptor.generation)?;
    let (producer_sequence, consumer_sequence, depth) = load_consistent_sequences(mapping)?;
    ensure_generation(mapping, descriptor.generation)?;
    let occupancy = u32::try_from(depth).map_err(|_| RingError::LayoutOverflow)?;
    Ok(RingState {
        producer_sequence,
        consumer_sequence,
        occupancy,
        available_capacity: RING_SLOT_COUNT - occupancy,
    })
}

fn ensure_generation(mapping: &[u8], expected: u64) -> Result<(), RingError> {
    let actual = mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).load(Ordering::Acquire);
    if actual == expected {
        Ok(())
    } else {
        Err(RingError::GenerationChanged { expected, actual })
    }
}

pub(crate) fn reset_mapping_generation(
    mapping: &mut [u8],
    descriptor: &mut RingDescriptor,
    new_generation: u64,
) -> Result<(), RingError> {
    ensure_generation(mapping, descriptor.generation)?;
    if new_generation <= descriptor.generation {
        return Err(RingError::GenerationNotIncreasing {
            current: descriptor.generation,
            requested: new_generation,
        });
    }

    mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).store(0, Ordering::Release);
    for slot_index in 0..u64::from(RING_SLOT_COUNT) {
        let slot_base = usize::try_from(
            MAPPING_HEADER_BYTES
                .checked_add(
                    descriptor
                        .layout
                        .slot_stride
                        .checked_mul(slot_index)
                        .ok_or(RingError::LayoutOverflow)?,
                )
                .ok_or(RingError::LayoutOverflow)?,
        )
        .map_err(|_| RingError::LayoutOverflow)?;
        mapped::atomic_u64(mapping, slot_base + SLOT_SEQUENCE_OFFSET).store(0, Ordering::Release);
    }
    mapped::atomic_u64(mapping, HEADER_PRODUCER_SEQUENCE_OFFSET).store(0, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_CONSUMER_SEQUENCE_OFFSET).store(0, Ordering::Release);
    mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).store(new_generation, Ordering::Release);
    descriptor.generation = new_generation;
    Ok(())
}

pub(crate) fn adopt_mapping_generation(
    mapping: &[u8],
    descriptor: &mut RingDescriptor,
    new_generation: u64,
) -> Result<(), RingError> {
    if new_generation <= descriptor.generation {
        return Err(RingError::GenerationNotIncreasing {
            current: descriptor.generation,
            requested: new_generation,
        });
    }
    let actual = mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).load(Ordering::Acquire);
    if actual != new_generation {
        return Err(RingError::GenerationMismatch {
            expected: new_generation,
            actual,
        });
    }
    let (producer_sequence, consumer_sequence, depth) = load_consistent_sequences(mapping)?;
    if producer_sequence != 0 || consumer_sequence != 0 || depth != 0 {
        return Err(RingError::CorruptSequences {
            produced: producer_sequence,
            consumed: consumer_sequence,
        });
    }
    for slot_index in 0..u64::from(RING_SLOT_COUNT) {
        let slot_base = usize::try_from(
            MAPPING_HEADER_BYTES
                .checked_add(
                    descriptor
                        .layout
                        .slot_stride
                        .checked_mul(slot_index)
                        .ok_or(RingError::LayoutOverflow)?,
                )
                .ok_or(RingError::LayoutOverflow)?,
        )
        .map_err(|_| RingError::LayoutOverflow)?;
        if mapped::atomic_u64(mapping, slot_base + SLOT_SEQUENCE_OFFSET).load(Ordering::Acquire)
            != 0
        {
            return Err(RingError::CorruptSlot {
                sequence: slot_index + 1,
            });
        }
    }
    descriptor.generation = new_generation;
    Ok(())
}

fn slot_base(layout: RingLayout, sequence: u64) -> Result<usize, RingError> {
    let slot_index = (sequence - 1) % u64::from(RING_SLOT_COUNT);
    usize::try_from(
        MAPPING_HEADER_BYTES
            .checked_add(
                layout
                    .slot_stride
                    .checked_mul(slot_index)
                    .ok_or(RingError::LayoutOverflow)?,
            )
            .ok_or(RingError::LayoutOverflow)?,
    )
    .map_err(|_| RingError::LayoutOverflow)
}

fn write_mapping_header(mapping: &mut [u8], descriptor: RingDescriptor) -> Result<(), RingError> {
    mapping[HEADER_MAGIC_OFFSET..HEADER_MAGIC_OFFSET + MAGIC.len()].copy_from_slice(&MAGIC);
    put_u32(mapping, HEADER_ABI_OFFSET, ABI_VERSION);
    put_u32(mapping, HEADER_BYTES_OFFSET, MAPPING_HEADER_FIELD);
    put_u64(mapping, HEADER_GENERATION_OFFSET, descriptor.generation);
    put_u32(mapping, HEADER_SLOT_COUNT_OFFSET, RING_SLOT_COUNT);
    put_u32(mapping, HEADER_SLOT_BYTES_OFFSET, SLOT_HEADER_FIELD);
    put_u32(mapping, HEADER_PIXEL_FORMAT_OFFSET, PIXEL_FORMAT_RGBA8);
    put_u32(mapping, HEADER_ORIGIN_OFFSET, ORIGIN_TOP_LEFT);
    put_u32(mapping, HEADER_WIDTH_OFFSET, descriptor.layout.width);
    put_u32(mapping, HEADER_HEIGHT_OFFSET, descriptor.layout.height);
    put_u32(
        mapping,
        HEADER_ROW_STRIDE_OFFSET,
        descriptor.layout.row_stride,
    );
    put_u32(
        mapping,
        HEADER_PAYLOAD_BYTES_OFFSET,
        u32::try_from(descriptor.layout.payload_bytes).map_err(|_| RingError::LayoutOverflow)?,
    );
    put_u64(
        mapping,
        HEADER_SLOT_STRIDE_OFFSET,
        descriptor.layout.slot_stride,
    );
    put_u64(
        mapping,
        HEADER_MAPPING_BYTES_OFFSET,
        descriptor.layout.mapping_bytes,
    );
    Ok(())
}

pub(crate) fn validate_mapping_header(
    mapping: &[u8],
    file_bytes: u64,
) -> Result<RingDescriptor, RingError> {
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
    require_header(
        get_u32(mapping, HEADER_SLOT_COUNT_OFFSET) == RING_SLOT_COUNT,
        "slot_count",
    )?;
    require_header(
        get_u32(mapping, HEADER_SLOT_BYTES_OFFSET) == SLOT_HEADER_FIELD,
        "slot_header_bytes",
    )?;
    require_header(
        get_u32(mapping, HEADER_PIXEL_FORMAT_OFFSET) == PIXEL_FORMAT_RGBA8,
        "pixel_format",
    )?;
    require_header(
        get_u32(mapping, HEADER_ORIGIN_OFFSET) == ORIGIN_TOP_LEFT,
        "origin",
    )?;

    let generation = mapped::atomic_u64(mapping, HEADER_GENERATION_OFFSET).load(Ordering::Acquire);
    require_header(generation != 0, "generation")?;
    let layout = RingLayout::new(
        get_u32(mapping, HEADER_WIDTH_OFFSET),
        get_u32(mapping, HEADER_HEIGHT_OFFSET),
    )?;
    require_header(
        get_u32(mapping, HEADER_ROW_STRIDE_OFFSET) == layout.row_stride,
        "row_stride",
    )?;
    require_header(
        u64::from(get_u32(mapping, HEADER_PAYLOAD_BYTES_OFFSET)) == layout.payload_bytes,
        "payload_bytes",
    )?;
    require_header(
        get_u64(mapping, HEADER_SLOT_STRIDE_OFFSET) == layout.slot_stride,
        "slot_stride",
    )?;
    require_header(
        get_u64(mapping, HEADER_MAPPING_BYTES_OFFSET) == layout.mapping_bytes,
        "mapping_bytes",
    )?;
    require_header(file_bytes == layout.mapping_bytes, "file_length")?;
    require_header(
        mapping[HEADER_RESERVED_OFFSET..MAPPING_HEADER_LEN]
            .iter()
            .all(|byte| *byte == 0),
        "reserved",
    )?;

    load_consistent_queue_depth(mapping)?;
    require_header(
        mapped::atomic_u64(mapping, HEADER_CONSUMER_CLAIM_OFFSET).load(Ordering::Acquire) <= 1,
        "consumer_claim",
    )?;
    require_header(
        mapped::atomic_u64(mapping, HEADER_PRODUCER_CLAIM_OFFSET).load(Ordering::Acquire) <= 1,
        "producer_claim",
    )?;

    Ok(RingDescriptor { layout, generation })
}

fn validate_slot_header(
    mapping: &[u8],
    slot_base: usize,
    descriptor: RingDescriptor,
    sequence: u64,
) -> Result<(), RingError> {
    let layout = descriptor.layout;
    let payload_end = slot_base
        + SLOT_HEADER_LEN
        + usize::try_from(layout.payload_bytes).map_err(|_| RingError::LayoutOverflow)?;
    let slot_end =
        slot_base + usize::try_from(layout.slot_stride).map_err(|_| RingError::LayoutOverflow)?;
    let payload_start = slot_base + SLOT_HEADER_LEN;
    let tight_row_bytes = layout.tight_row_bytes() as usize;
    let row_stride = layout.row_stride as usize;
    let row_padding_is_zero = mapping[payload_start..payload_end]
        .chunks_exact(row_stride)
        .all(|row| row[tight_row_bytes..].iter().all(|byte| *byte == 0));
    let valid = get_u64(mapping, slot_base + SLOT_GENERATION_OFFSET) == descriptor.generation
        && u64::from(get_u32(mapping, slot_base + SLOT_PAYLOAD_BYTES_OFFSET))
            == layout.payload_bytes
        && get_u32(mapping, slot_base + SLOT_WIDTH_OFFSET) == layout.width
        && get_u32(mapping, slot_base + SLOT_HEIGHT_OFFSET) == layout.height
        && get_u32(mapping, slot_base + SLOT_ROW_STRIDE_OFFSET) == layout.row_stride
        && get_u32(mapping, slot_base + SLOT_PIXEL_FORMAT_OFFSET) == PIXEL_FORMAT_RGBA8
        && get_u32(mapping, slot_base + SLOT_ORIGIN_OFFSET) == ORIGIN_TOP_LEFT
        && mapping[slot_base + SLOT_RESERVED_OFFSET..slot_base + SLOT_HEADER_LEN]
            .iter()
            .all(|byte| *byte == 0)
        && row_padding_is_zero
        && mapping[payload_end..slot_end].iter().all(|byte| *byte == 0);
    if valid {
        Ok(())
    } else {
        Err(RingError::CorruptSlot { sequence })
    }
}

fn require_header(condition: bool, field: &'static str) -> Result<(), RingError> {
    if condition {
        Ok(())
    } else {
        Err(RingError::CorruptHeader { field })
    }
}

fn load_consistent_queue_depth(mapping: &[u8]) -> Result<u64, RingError> {
    Ok(load_consistent_sequences(mapping)?.2)
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
            return Ok((
                produced,
                consumed_after,
                checked_queue_depth(produced, consumed_after)?,
            ));
        }
    }
    Err(RingError::CorruptHeader {
        field: "sequence_snapshot",
    })
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

/// Audited unsafe boundary required by memmap2 and atomic access to an
/// externally backed, page-aligned byte region. The mapping file must never be
/// resized while either endpoint is alive. All atomic offsets are compile-time
/// multiples of eight, slot strides are multiples of 128, and a file mapping's
/// base address is page aligned.
pub(crate) mod mapped {
    #![allow(
        unsafe_code,
        clippy::cast_ptr_alignment,
        reason = "memmap2 mapping and AtomicU64::from_ptr are the ABI boundary"
    )]

    use std::fs::File;
    use std::io;
    use std::sync::atomic::AtomicU64;

    use memmap2::{MmapMut, MmapOptions};

    pub fn map_file_mut(file: &File, len: usize) -> io::Result<MmapMut> {
        // SAFETY: The caller validates and fixes the file length before this
        // call and keeps its File alive for the full mapping lifetime. The ring
        // contract forbids resizing the file until both endpoints are dropped.
        unsafe { MmapOptions::new().len(len).map_mut(file) }
    }

    pub fn atomic_u64(mapping: &[u8], offset: usize) -> &AtomicU64 {
        debug_assert!(offset + size_of::<u64>() <= mapping.len());
        let pointer = mapping
            .as_ptr()
            .wrapping_add(offset)
            .cast_mut()
            .cast::<u64>();
        debug_assert_eq!(pointer.align_offset(align_of::<AtomicU64>()), 0);
        // SAFETY: Each supplied offset is naturally aligned and in bounds. The
        // corresponding eight bytes are initialized to zero before publication
        // and are accessed atomically for the remainder of the mapping life.
        unsafe { AtomicU64::from_ptr(pointer) }
    }
}
