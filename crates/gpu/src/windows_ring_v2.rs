//! Anonymous Windows shared-memory transport for Protocol 2 RGB Ring ABI 2.
//!
//! The two auto-reset events have distinct directions: the worker producer
//! signals `ready` after publishing a complete batch, and the Core consumer
//! signals `consumed` only after copying and releasing that batch's slot.

use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::time::Duration;

use crate::ring::RingError;
use crate::ring_v2::{
    ReadV2Status, RingV2Descriptor, RingV2State, WriteV2Status, adopt_mapping_generation,
    claim_consumer, claim_producer, initialize_mapping, mapping_len, mapping_state,
    read_mapping_batch, release_consumer, release_producer, reset_mapping_generation,
    validate_mapping_header, write_mapping_batch,
};
use crate::windows_ring::{FramesReady, win32};

/// Wire-safe handle values and exact mapped-header facts for one target process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsRingV2Binding {
    mapping_handle: u64,
    ready_event_handle: u64,
    consumed_event_handle: u64,
    descriptor: RingV2Descriptor,
}

impl WindowsRingV2Binding {
    #[must_use]
    pub const fn mapping_handle(self) -> u64 {
        self.mapping_handle
    }

    #[must_use]
    pub const fn ready_event_handle(self) -> u64 {
        self.ready_event_handle
    }

    #[must_use]
    pub const fn consumed_event_handle(self) -> u64 {
        self.consumed_event_handle
    }

    /// Exact anonymous mapping byte length (`byte_length` in host metadata).
    #[must_use]
    pub const fn byte_length(self) -> u64 {
        self.descriptor.layout().mapping_bytes()
    }

    #[must_use]
    pub const fn slot_count(self) -> u32 {
        self.descriptor.layout().slot_count()
    }

    #[must_use]
    pub const fn slot_bytes(self) -> u64 {
        self.descriptor.layout().slot_bytes()
    }

    #[must_use]
    pub const fn slot_stride_bytes(self) -> u64 {
        self.descriptor.layout().slot_stride_bytes()
    }

    #[must_use]
    pub const fn frame_stride_bytes(self) -> u64 {
        self.descriptor.layout().frame_stride_bytes()
    }

    #[must_use]
    pub const fn descriptor(self) -> RingV2Descriptor {
        self.descriptor
    }
}

/// Owns the anonymous ABI2 mapping and both directional events in Core.
pub struct WindowsRgbRingV2Owner {
    mapping_handle: OwnedHandle,
    ready_event: OwnedHandle,
    consumed_event: OwnedHandle,
    mapping: win32::MappedView,
    descriptor: RingV2Descriptor,
}

impl WindowsRgbRingV2Owner {
    /// Allocates and initializes one unnamed mapping and two unnamed events.
    ///
    /// # Errors
    ///
    /// Returns a checked layout, Windows allocation, or initialization error.
    pub fn create(descriptor: RingV2Descriptor) -> Result<Self, RingError> {
        let length = mapping_len(descriptor.layout())?;
        let (mapping_handle, mut mapping) = win32::create_anonymous_mapping(length)?;
        initialize_mapping(&mut mapping, descriptor)?;
        let ready_event = win32::create_auto_reset_event()?;
        let consumed_event = win32::create_auto_reset_event()?;
        Ok(Self {
            mapping_handle,
            ready_event,
            consumed_event,
            mapping,
            descriptor,
        })
    }

    #[must_use]
    pub const fn descriptor(&self) -> RingV2Descriptor {
        self.descriptor
    }

    #[must_use]
    pub const fn byte_length(&self) -> u64 {
        self.descriptor.layout().mapping_bytes()
    }

    /// Returns an internally consistent queue snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale generation or corrupt shared state.
    pub fn state(&self) -> Result<RingV2State, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Opens the sole Core-side consumer from independent local duplicates.
    ///
    /// # Errors
    ///
    /// Returns an error for handle, header, generation, or claim failure.
    pub fn open_consumer(&self) -> Result<WindowsRgbRingV2Consumer, RingError> {
        WindowsRgbRingV2Consumer::open_from_handles(
            self.mapping_handle.as_handle(),
            self.ready_event.as_handle(),
            self.consumed_event.as_handle(),
            self.byte_length(),
            self.descriptor.generation(),
        )
    }

    /// Duplicates all three handles into exactly `target_process`.
    ///
    /// Every partial failure closes earlier target-process duplicates before
    /// returning. Successful values are owned by the target and must be
    /// consumed exactly once by its ring transport.
    ///
    /// # Errors
    ///
    /// Returns a Windows duplication error and reclaims partial duplicates.
    pub fn duplicate_into(
        &self,
        target_process: BorrowedHandle<'_>,
    ) -> Result<WindowsRingV2Binding, RingError> {
        let mapping =
            win32::duplicate_into_target(self.mapping_handle.as_handle(), target_process)?;
        let ready = win32::duplicate_into_target(self.ready_event.as_handle(), target_process)?;
        let consumed =
            win32::duplicate_into_target(self.consumed_event.as_handle(), target_process)?;
        let binding = WindowsRingV2Binding {
            mapping_handle: mapping.raw_value()?,
            ready_event_handle: ready.raw_value()?,
            consumed_event_handle: consumed.raw_value()?,
            descriptor: self.descriptor,
        };
        mapping.release();
        ready.release();
        consumed.release();
        Ok(binding)
    }

    /// Advances an unclaimed mapping generation for pre-bind recovery.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale generation, claim, event, or mapping state.
    pub fn set_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        require_new_generation(self.descriptor, new_generation)?;
        claim_producer(&self.mapping)?;
        let result = reset_both_events(&self.ready_event, &self.consumed_event).and_then(|()| {
            reset_mapping_generation(&mut self.mapping, &mut self.descriptor, new_generation)
        });
        release_producer(&self.mapping);
        result
    }

    /// Adopts a generation already reset and acknowledged by the worker.
    ///
    /// # Errors
    ///
    /// Returns an error unless the exact newer reset state is visible.
    pub fn adopt_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        adopt_mapping_generation(&self.mapping, &mut self.descriptor, new_generation)
    }
}

/// Sole worker-side producer opened from target-owned ABI2 handles.
pub struct WindowsRgbRingV2Producer {
    _mapping_handle: OwnedHandle,
    ready_event: OwnedHandle,
    consumed_event: OwnedHandle,
    mapping: win32::MappedView,
    descriptor: RingV2Descriptor,
    claimed: bool,
}

impl WindowsRgbRingV2Producer {
    /// Duplicates borrowed current-process handles and opens the producer.
    ///
    /// # Errors
    ///
    /// Returns an error for duplication, mapping, validation, or claim failure.
    pub fn open_from_handles(
        mapping_handle: BorrowedHandle<'_>,
        ready_event: BorrowedHandle<'_>,
        consumed_event: BorrowedHandle<'_>,
        mapping_bytes: u64,
        expected_generation: u64,
    ) -> Result<Self, RingError> {
        Self::open_from_owned_handles(
            win32::duplicate_local(mapping_handle)?,
            win32::duplicate_local(ready_event)?,
            win32::duplicate_local(consumed_event)?,
            mapping_bytes,
            expected_generation,
        )
    }

    /// Consumes three target-owned handles. Every error path closes all three.
    ///
    /// # Errors
    ///
    /// Returns an error for mapping, validation, generation, event, or claim failure.
    pub fn open_from_owned_handles(
        mapping_handle: OwnedHandle,
        ready_event: OwnedHandle,
        consumed_event: OwnedHandle,
        mapping_bytes: u64,
        expected_generation: u64,
    ) -> Result<Self, RingError> {
        Self::open_owned(
            mapping_handle,
            ready_event,
            consumed_event,
            mapping_bytes,
            Some(expected_generation),
        )
    }

    /// Consumes target-owned handles and adopts the validated mapped generation.
    ///
    /// This is the Python Protocol 2 seam because the current typed
    /// `ring.configure` surface carries slot count/bytes but no generation.
    /// Every publish still cross-checks its command generation before writing.
    ///
    /// # Errors
    ///
    /// Returns an error for mapping, validation, event, or claim failure.
    pub fn open_from_owned_handles_discovered_generation(
        mapping_handle: OwnedHandle,
        ready_event: OwnedHandle,
        consumed_event: OwnedHandle,
        mapping_bytes: u64,
    ) -> Result<Self, RingError> {
        Self::open_owned(
            mapping_handle,
            ready_event,
            consumed_event,
            mapping_bytes,
            None,
        )
    }

    fn open_owned(
        mapping_handle: OwnedHandle,
        ready_event: OwnedHandle,
        consumed_event: OwnedHandle,
        mapping_bytes: u64,
        expected_generation: Option<u64>,
    ) -> Result<Self, RingError> {
        let length = checked_mapping_bytes(mapping_bytes)?;
        let mapping = win32::map_view(mapping_handle.as_handle(), length)?;
        let descriptor = validate_mapping_header(&mapping, mapping_bytes)?;
        if let Some(expected) = expected_generation
            && descriptor.generation() != expected
        {
            return Err(RingError::GenerationMismatch {
                expected,
                actual: descriptor.generation(),
            });
        }
        claim_producer(&mapping)?;
        if let Err(error) = reset_both_events(&ready_event, &consumed_event) {
            release_producer(&mapping);
            return Err(error);
        }
        Ok(Self {
            _mapping_handle: mapping_handle,
            ready_event,
            consumed_event,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    #[must_use]
    pub const fn descriptor(&self) -> RingV2Descriptor {
        self.descriptor
    }

    /// Returns an internally consistent queue snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale generation or corrupt shared state.
    pub fn state(&self) -> Result<RingV2State, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Publishes exactly one contiguous decoded batch and then signals ready.
    ///
    /// # Errors
    ///
    /// Returns a bounds, state, backpressure-adjacent event, or mapping error.
    #[allow(clippy::too_many_arguments)]
    pub fn try_write_batch(
        &mut self,
        session_id: [u8; 16],
        logical_sequence: u64,
        batch: u32,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) -> Result<WriteV2Status, RingError> {
        let status = write_mapping_batch(
            &mut self.mapping,
            self.descriptor,
            session_id,
            logical_sequence,
            batch,
            width,
            height,
            pixels,
        )?;
        if matches!(status, WriteV2Status::Written(_)) {
            win32::set_event(self.ready_event.as_handle())?;
        }
        Ok(status)
    }

    /// Waits until Core reports that at least one queued slot was consumed.
    ///
    /// # Errors
    ///
    /// Returns a Windows wait error.
    pub fn wait_consumed(&self, timeout: Duration) -> Result<FramesReady, RingError> {
        win32::wait_event(self.consumed_event.as_handle(), timeout)
    }

    /// Invalidates old slots and starts one strictly newer generation.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-increasing generation, event, or state failure.
    pub fn set_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        require_new_generation(self.descriptor, new_generation)?;
        reset_both_events(&self.ready_event, &self.consumed_event)?;
        reset_mapping_generation(&mut self.mapping, &mut self.descriptor, new_generation)
    }
}

impl Drop for WindowsRgbRingV2Producer {
    fn drop(&mut self) {
        if self.claimed {
            release_producer(&self.mapping);
            self.claimed = false;
        }
    }
}

/// Sole Core-side consumer and consumed-event signaler.
pub struct WindowsRgbRingV2Consumer {
    _mapping_handle: OwnedHandle,
    ready_event: OwnedHandle,
    consumed_event: OwnedHandle,
    mapping: win32::MappedView,
    descriptor: RingV2Descriptor,
    claimed: bool,
}

impl WindowsRgbRingV2Consumer {
    fn open_from_handles(
        mapping_handle: BorrowedHandle<'_>,
        ready_event: BorrowedHandle<'_>,
        consumed_event: BorrowedHandle<'_>,
        mapping_bytes: u64,
        expected_generation: u64,
    ) -> Result<Self, RingError> {
        let mapping_handle = win32::duplicate_local(mapping_handle)?;
        let ready_event = win32::duplicate_local(ready_event)?;
        let consumed_event = win32::duplicate_local(consumed_event)?;
        let mapping = win32::map_view(
            mapping_handle.as_handle(),
            checked_mapping_bytes(mapping_bytes)?,
        )?;
        let descriptor = validate_mapping_header(&mapping, mapping_bytes)?;
        if descriptor.generation() != expected_generation {
            return Err(RingError::GenerationMismatch {
                expected: expected_generation,
                actual: descriptor.generation(),
            });
        }
        claim_consumer(&mapping)?;
        Ok(Self {
            _mapping_handle: mapping_handle,
            ready_event,
            consumed_event,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    #[must_use]
    pub const fn descriptor(&self) -> RingV2Descriptor {
        self.descriptor
    }

    /// Returns an internally consistent queue snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for a stale generation or corrupt shared state.
    pub fn state(&self) -> Result<RingV2State, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Waits for a producer-ready notification.
    ///
    /// # Errors
    ///
    /// Returns a Windows wait error.
    pub fn wait_ready(&self, timeout: Duration) -> Result<FramesReady, RingError> {
        win32::wait_event(self.ready_event.as_handle(), timeout)
    }

    /// Copies one batch, releases its slot, then signals the consumed event.
    ///
    /// # Errors
    ///
    /// Returns a mapped-state, slot-validation, or event signaling error.
    pub fn try_read(&mut self) -> Result<ReadV2Status, RingError> {
        let status = read_mapping_batch(&self.mapping, self.descriptor)?;
        if matches!(status, ReadV2Status::Batch(_)) {
            win32::set_event(self.consumed_event.as_handle())?;
        }
        Ok(status)
    }

    /// Adopts the exact newer generation after a worker reset acknowledgement.
    ///
    /// # Errors
    ///
    /// Returns an error unless the exact reset state is visible.
    pub fn adopt_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        adopt_mapping_generation(&self.mapping, &mut self.descriptor, new_generation)
    }
}

impl Drop for WindowsRgbRingV2Consumer {
    fn drop(&mut self) {
        if self.claimed {
            release_consumer(&self.mapping);
            self.claimed = false;
        }
    }
}

fn checked_mapping_bytes(mapping_bytes: u64) -> Result<usize, RingError> {
    if !(crate::ring_v2::MAPPING_HEADER_BYTES..=crate::ring::MAX_MAPPING_BYTES)
        .contains(&mapping_bytes)
    {
        return Err(RingError::InvalidMappingLength {
            actual: mapping_bytes,
        });
    }
    usize::try_from(mapping_bytes).map_err(|_| RingError::LayoutOverflow)
}

fn require_new_generation(
    descriptor: RingV2Descriptor,
    new_generation: u64,
) -> Result<(), RingError> {
    if new_generation <= descriptor.generation() {
        Err(RingError::GenerationNotIncreasing {
            current: descriptor.generation(),
            requested: new_generation,
        })
    } else {
        Ok(())
    }
}

fn reset_both_events(
    ready_event: &OwnedHandle,
    consumed_event: &OwnedHandle,
) -> Result<(), RingError> {
    win32::reset_event(ready_event.as_handle())?;
    win32::reset_event(consumed_event.as_handle())
}
