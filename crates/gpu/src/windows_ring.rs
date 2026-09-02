//! Anonymous Windows shared-memory transport for RGB Ring ABI 1.
//!
//! Production control exchanges only handles duplicated into the worker
//! process. No filesystem path participates in the runtime contract.

use std::os::windows::io::{AsHandle, BorrowedHandle, OwnedHandle};
use std::time::Duration;

use crate::ring::{
    ReadStatus, RingDescriptor, RingError, RingState, WriteStatus, adopt_mapping_generation,
    claim_consumer, claim_producer, initialize_mapping, mapping_len, mapping_state,
    read_mapping_frame, release_consumer, release_producer, reset_mapping_generation,
    validate_mapping_header, write_mapping_frame,
};

/// Wire-safe handle values already duplicated into one target process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowsRingBinding {
    mapping_handle: u64,
    mapping_bytes: u64,
    frames_ready_event_handle: u64,
}

impl WindowsRingBinding {
    /// File-mapping handle value valid only in the duplication target process.
    #[must_use]
    pub const fn mapping_handle(self) -> u64 {
        self.mapping_handle
    }

    /// Exact byte length that the target must map and validate.
    #[must_use]
    pub const fn mapping_bytes(self) -> u64 {
        self.mapping_bytes
    }

    /// Auto-reset event handle value valid only in the target process.
    #[must_use]
    pub const fn frames_ready_event_handle(self) -> u64 {
        self.frames_ready_event_handle
    }
}

/// Result of one bounded frames-ready event wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramesReady {
    /// At least one publish occurred since the event was last consumed.
    Signaled,
    /// No notification arrived within the requested duration.
    Timeout,
}

/// Owns one anonymous pagefile-backed mapping and its frames-ready event.
///
/// The owner stays in Core. It can open the sole native consumer and duplicate
/// mapping/event handles into an already-created worker process.
pub struct WindowsRgbRingOwner {
    mapping_handle: OwnedHandle,
    frames_ready_event: OwnedHandle,
    mapping: win32::MappedView,
    descriptor: RingDescriptor,
}

impl WindowsRgbRingOwner {
    /// Creates and initializes an unnamed Windows file-mapping object plus an
    /// unnamed auto-reset event. No on-disk file is created.
    ///
    /// # Errors
    ///
    /// Returns an error when allocation, mapping, event creation, or checked
    /// ABI initialization fails.
    pub fn create(descriptor: RingDescriptor) -> Result<Self, RingError> {
        let mapping_len = mapping_len(descriptor.layout())?;
        let (mapping_handle, mut mapping) = win32::create_anonymous_mapping(mapping_len)?;
        initialize_mapping(&mut mapping, descriptor)?;
        let frames_ready_event = win32::create_auto_reset_event()?;
        Ok(Self {
            mapping_handle,
            frames_ready_event,
            mapping,
            descriptor,
        })
    }

    /// Current owner-side descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> RingDescriptor {
        self.descriptor
    }

    /// Exact anonymous mapping byte length.
    #[must_use]
    pub const fn mapping_bytes(&self) -> u64 {
        self.descriptor.layout().mapping_bytes()
    }

    /// Returns a consistent queue snapshot without claiming an endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error for a changed generation or corrupt counters.
    pub fn state(&self) -> Result<RingState, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Opens and claims the sole consumer using independently duplicated local
    /// handles, so its lifetime is not tied to a borrow of the owner.
    ///
    /// # Errors
    ///
    /// Returns an error when handles cannot be duplicated/mapped or another
    /// consumer already owns the claim.
    pub fn open_consumer(&self) -> Result<WindowsRgbRingConsumer, RingError> {
        WindowsRgbRingConsumer::open_from_handles(
            self.mapping_handle.as_handle(),
            self.frames_ready_event.as_handle(),
            self.mapping_bytes(),
            self.descriptor.generation(),
        )
    }

    /// Duplicates both runtime handles into `target_process` using the same
    /// access rights and returns values valid only in that target process.
    ///
    /// `target_process` must grant `PROCESS_DUP_HANDLE`. If the second
    /// duplication fails, the first target handle is closed before returning.
    ///
    /// # Errors
    ///
    /// Returns a Windows error when either handle cannot be duplicated.
    pub fn duplicate_into(
        &self,
        target_process: BorrowedHandle<'_>,
    ) -> Result<WindowsRingBinding, RingError> {
        let mapping =
            win32::duplicate_into_target(self.mapping_handle.as_handle(), target_process)?;
        let event =
            win32::duplicate_into_target(self.frames_ready_event.as_handle(), target_process)?;
        let binding = WindowsRingBinding {
            mapping_handle: mapping.raw_value()?,
            mapping_bytes: self.mapping_bytes(),
            frames_ready_event_handle: event.raw_value()?,
        };
        mapping.release();
        event.release();
        Ok(binding)
    }

    /// Resets an unclaimed ring to a strictly newer generation.
    ///
    /// Runtime reset is normally performed by the worker producer. This owner
    /// path exists for pre-bind initialization/recovery and refuses to race a
    /// claimed producer.
    ///
    /// # Errors
    ///
    /// Returns an error for a claimed producer, stale generation, event reset,
    /// or corrupt shared state.
    pub fn set_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        require_new_generation(self.descriptor, new_generation)?;
        claim_producer(&self.mapping)?;
        let result = win32::reset_event(self.frames_ready_event.as_handle()).and_then(|()| {
            reset_mapping_generation(&mut self.mapping, &mut self.descriptor, new_generation)
        });
        release_producer(&self.mapping);
        result
    }

    /// Adopts the exact generation acknowledged by a remote producer.
    ///
    /// This keeps owner diagnostics and future consumer opens aligned after a
    /// worker-driven reset; it does not mutate the mapping.
    ///
    /// # Errors
    ///
    /// Returns an error unless header generation matches and all sequences and
    /// slot commits are reset to zero.
    pub fn adopt_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        adopt_mapping_generation(&self.mapping, &mut self.descriptor, new_generation)
    }
}

/// Sole producer endpoint opened from handles valid in the worker process.
pub struct WindowsRgbRingProducer {
    _mapping_handle: OwnedHandle,
    frames_ready_event: OwnedHandle,
    mapping: win32::MappedView,
    descriptor: RingDescriptor,
    claimed: bool,
}

impl WindowsRgbRingProducer {
    /// Duplicates valid current-process handles into owned handles, maps and
    /// validates the exact ABI range, and claims the sole producer role.
    ///
    /// This borrowed variant is convenient for Rust owners that retain the
    /// original handles. Native bindings should prefer
    /// [`Self::open_from_owned_handles`] so target handles close on every path.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid handles/length/header/generation or a
    /// producer claim already held by another endpoint.
    pub fn open_from_handles(
        mapping_handle: BorrowedHandle<'_>,
        frames_ready_event: BorrowedHandle<'_>,
        mapping_bytes: u64,
        expected_generation: u64,
    ) -> Result<Self, RingError> {
        let mapping_handle = win32::duplicate_local(mapping_handle)?;
        let frames_ready_event = win32::duplicate_local(frames_ready_event)?;
        Self::open_from_owned_handles(
            mapping_handle,
            frames_ready_event,
            mapping_bytes,
            expected_generation,
        )
    }

    /// Consumes mapping/event handles already owned by the worker process.
    ///
    /// This is the preferred native Python seam: the binding converts each
    /// target-valid raw value into [`OwnedHandle`] exactly once, then transfers
    /// ownership here. All error paths close both handles.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid handles/length/header/generation or a
    /// producer claim already held by another endpoint.
    pub fn open_from_owned_handles(
        mapping_handle: OwnedHandle,
        frames_ready_event: OwnedHandle,
        mapping_bytes: u64,
        expected_generation: u64,
    ) -> Result<Self, RingError> {
        let mapping_len = checked_mapping_bytes(mapping_bytes)?;
        let mapping = win32::map_view(mapping_handle.as_handle(), mapping_len)?;
        let descriptor = validate_mapping_header(&mapping, mapping_bytes)?;
        if descriptor.generation() != expected_generation {
            return Err(RingError::GenerationMismatch {
                expected: expected_generation,
                actual: descriptor.generation(),
            });
        }
        claim_producer(&mapping)?;
        if let Err(error) = win32::reset_event(frames_ready_event.as_handle()) {
            release_producer(&mapping);
            return Err(error);
        }
        Ok(Self {
            _mapping_handle: mapping_handle,
            frames_ready_event,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    /// Validated mapping descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> RingDescriptor {
        self.descriptor
    }

    /// Returns producer/consumer sequences, occupancy, and available capacity.
    ///
    /// # Errors
    ///
    /// Returns an error for changed generation or corrupt counters.
    pub fn state(&self) -> Result<RingState, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Whether a complete 5/17-frame H3 cycle fits before causal decode.
    ///
    /// # Errors
    ///
    /// Returns an error for changed generation or corrupt counters.
    pub fn can_publish(&self, frame_count: u32) -> Result<bool, RingError> {
        Ok(self.state()?.can_publish(frame_count))
    }

    /// Publishes one frame and signals the frames-ready event only after both
    /// slot and global sequences are committed with release ordering.
    ///
    /// # Errors
    ///
    /// Returns an ABI error or a Windows event error. If signaling itself
    /// fails, the frame is already committed and the session must be reset;
    /// callers must not retry that frame as a new sequence.
    pub fn try_write(
        &mut self,
        tight_rgba: &[u8],
        timestamp_ns: u64,
    ) -> Result<WriteStatus, RingError> {
        let status =
            write_mapping_frame(&mut self.mapping, self.descriptor, tight_rgba, timestamp_ns)?;
        if matches!(status, WriteStatus::Written(_)) {
            win32::set_event(self.frames_ready_event.as_handle())?;
        }
        Ok(status)
    }

    /// Invalidates all old commits and starts a strictly newer generation.
    /// Decode must remain quiescent until Core adopts the acknowledged value.
    ///
    /// # Errors
    ///
    /// Returns an error for non-increasing generation, event reset failure, or
    /// corrupt mapped state.
    pub fn set_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        require_new_generation(self.descriptor, new_generation)?;
        win32::reset_event(self.frames_ready_event.as_handle())?;
        reset_mapping_generation(&mut self.mapping, &mut self.descriptor, new_generation)
    }
}

impl Drop for WindowsRgbRingProducer {
    fn drop(&mut self) {
        if self.claimed {
            release_producer(&self.mapping);
            self.claimed = false;
        }
    }
}

/// Sole Core-side consumer endpoint for one anonymous mapping.
pub struct WindowsRgbRingConsumer {
    _mapping_handle: OwnedHandle,
    frames_ready_event: OwnedHandle,
    mapping: win32::MappedView,
    descriptor: RingDescriptor,
    claimed: bool,
}

impl WindowsRgbRingConsumer {
    fn open_from_handles(
        mapping_handle: BorrowedHandle<'_>,
        frames_ready_event: BorrowedHandle<'_>,
        mapping_bytes: u64,
        expected_generation: u64,
    ) -> Result<Self, RingError> {
        let mapping_len = checked_mapping_bytes(mapping_bytes)?;
        let mapping_handle = win32::duplicate_local(mapping_handle)?;
        let frames_ready_event = win32::duplicate_local(frames_ready_event)?;
        let mapping = win32::map_view(mapping_handle.as_handle(), mapping_len)?;
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
            frames_ready_event,
            mapping,
            descriptor,
            claimed: true,
        })
    }

    /// Validated mapping descriptor.
    #[must_use]
    pub const fn descriptor(&self) -> RingDescriptor {
        self.descriptor
    }

    /// Returns producer/consumer sequences, occupancy, and available capacity.
    ///
    /// # Errors
    ///
    /// Returns an error for changed generation or corrupt counters.
    pub fn state(&self) -> Result<RingState, RingError> {
        mapping_state(&self.mapping, self.descriptor)
    }

    /// Whether an entire frame batch fits in the current snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for changed generation or corrupt counters.
    pub fn can_publish(&self, frame_count: u32) -> Result<bool, RingError> {
        Ok(self.state()?.can_publish(frame_count))
    }

    /// Waits on the coalescing auto-reset event. Sequence counters remain the
    /// source of truth; callers drain `try_read` until empty after a signal.
    ///
    /// # Errors
    ///
    /// Returns a Windows error for a failed or unexpected wait result.
    pub fn wait_frames_ready(&self, timeout: Duration) -> Result<FramesReady, RingError> {
        win32::wait_event(self.frames_ready_event.as_handle(), timeout)
    }

    /// Copies and releases one committed frame without blocking.
    ///
    /// # Errors
    ///
    /// Returns an error for changed generation or corrupt slot/counter state.
    pub fn try_read(&mut self) -> Result<ReadStatus, RingError> {
        read_mapping_frame(&self.mapping, self.descriptor)
    }

    /// Adopts the exact strictly newer generation returned by `slot.reset_ack`.
    ///
    /// # Errors
    ///
    /// Returns an error unless header generation matches and every sequence
    /// and slot commit has been cleared.
    pub fn adopt_generation(&mut self, new_generation: u64) -> Result<(), RingError> {
        adopt_mapping_generation(&self.mapping, &mut self.descriptor, new_generation)
    }
}

impl Drop for WindowsRgbRingConsumer {
    fn drop(&mut self) {
        if self.claimed {
            release_consumer(&self.mapping);
            self.claimed = false;
        }
    }
}

fn checked_mapping_bytes(mapping_bytes: u64) -> Result<usize, RingError> {
    if !(crate::ring::MAPPING_HEADER_BYTES..=crate::ring::MAX_MAPPING_BYTES)
        .contains(&mapping_bytes)
    {
        return Err(RingError::InvalidMappingLength {
            actual: mapping_bytes,
        });
    }
    usize::try_from(mapping_bytes).map_err(|_| RingError::LayoutOverflow)
}

fn require_new_generation(
    descriptor: RingDescriptor,
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

/// All Win32 calls, raw views, and handle ownership conversion live here.
pub(crate) mod win32 {
    #![allow(
        unsafe_code,
        clippy::as_conversions,
        clippy::cast_ptr_alignment,
        reason = "audited Windows HANDLE and mapped-view ABI boundary"
    )]

    use std::ops::{Deref, DerefMut};
    use std::os::windows::io::{AsHandle, AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};
    use std::ptr::NonNull;
    use std::time::Duration;

    use windows::Win32::Foundation::{
        CloseHandle, DUPLICATE_CLOSE_SOURCE, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, ResetEvent, SetEvent, WaitForSingleObject,
    };
    use windows::core::{Error as WindowsError, PCWSTR};

    use super::{FramesReady, RingError};

    pub struct MappedView {
        pointer: NonNull<u8>,
        len: usize,
    }

    // SAFETY: The view has unique Rust ownership. ABI synchronization governs
    // cross-process access, and moving the view does not move its allocation.
    unsafe impl Send for MappedView {}

    impl Deref for MappedView {
        type Target = [u8];

        fn deref(&self) -> &Self::Target {
            // SAFETY: MapViewOfFile returned this non-null range for `len`; the
            // view remains mapped for self's lifetime.
            unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.len) }
        }
    }

    impl DerefMut for MappedView {
        fn deref_mut(&mut self) -> &mut Self::Target {
            // SAFETY: MappedView has unique Rust ownership and the same valid
            // range/lifetime as described in Deref.
            unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.len) }
        }
    }

    impl Drop for MappedView {
        fn drop(&mut self) {
            // SAFETY: This exact base address came from MapViewOfFile and is
            // unmapped exactly once here.
            let _ = unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.pointer.as_ptr().cast(),
                })
            };
        }
    }

    pub fn create_anonymous_mapping(len: usize) -> Result<(OwnedHandle, MappedView), RingError> {
        let size = u64::try_from(len).map_err(|_| RingError::LayoutOverflow)?;
        let size_high = u32::try_from(size >> 32).map_err(|_| RingError::LayoutOverflow)?;
        let size_low =
            u32::try_from(size & u64::from(u32::MAX)).map_err(|_| RingError::LayoutOverflow)?;
        // SAFETY: INVALID_HANDLE_VALUE requests pagefile backing; attributes
        // and name are null, and the checked nonzero mapping length is split
        // into the documented high/low DWORDs.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                size_high,
                size_low,
                PCWSTR::null(),
            )?
        };
        // SAFETY: CreateFileMappingW returned a newly owned nonzero handle.
        let owned = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        let view = map_view(owned.as_handle(), len)?;
        Ok((owned, view))
    }

    pub fn create_auto_reset_event() -> Result<OwnedHandle, RingError> {
        // SAFETY: Null security/name, auto-reset=false manual-reset flag, and
        // initially nonsignaled are valid CreateEventW inputs.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null())? };
        // SAFETY: CreateEventW returned a newly owned nonzero handle.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
    }

    pub fn map_view(handle: BorrowedHandle<'_>, len: usize) -> Result<MappedView, RingError> {
        // SAFETY: BorrowedHandle is live for this call; offsets are zero and
        // len was checked against the protocol's 256 MiB cap.
        let address = unsafe { MapViewOfFile(as_handle(handle), FILE_MAP_ALL_ACCESS, 0, 0, len) };
        let pointer = NonNull::new(address.Value.cast::<u8>())
            .ok_or_else(|| RingError::Windows(WindowsError::from_thread()))?;
        Ok(MappedView { pointer, len })
    }

    pub fn duplicate_local(handle: BorrowedHandle<'_>) -> Result<OwnedHandle, RingError> {
        let current = unsafe { GetCurrentProcess() };
        let mut duplicate = HANDLE::default();
        // SAFETY: Source handle is borrowed/live; both process pseudo-handles
        // refer to the current process and output points to initialized storage.
        unsafe {
            DuplicateHandle(
                current,
                as_handle(handle),
                current,
                &raw mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
        }
        // SAFETY: DuplicateHandle returned a new handle owned by this process.
        Ok(unsafe { OwnedHandle::from_raw_handle(duplicate.0) })
    }

    pub struct TargetHandle<'a> {
        target_process: BorrowedHandle<'a>,
        handle: HANDLE,
        armed: bool,
    }

    impl TargetHandle<'_> {
        pub fn raw_value(&self) -> Result<u64, RingError> {
            u64::try_from(self.handle.0 as usize).map_err(|_| RingError::LayoutOverflow)
        }

        pub fn release(mut self) {
            self.armed = false;
        }
    }

    impl Drop for TargetHandle<'_> {
        fn drop(&mut self) {
            if self.armed {
                close_target_handle(self.target_process, self.handle);
            }
        }
    }

    pub fn duplicate_into_target<'a>(
        source: BorrowedHandle<'_>,
        target_process: BorrowedHandle<'a>,
    ) -> Result<TargetHandle<'a>, RingError> {
        let mut duplicate = HANDLE::default();
        // SAFETY: Source and target process handles are live borrows. The
        // caller supplies PROCESS_DUP_HANDLE rights for the target.
        unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                as_handle(source),
                as_handle(target_process),
                &raw mut duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS,
            )?;
        }
        Ok(TargetHandle {
            target_process,
            handle: duplicate,
            armed: true,
        })
    }

    pub fn set_event(event: BorrowedHandle<'_>) -> Result<(), RingError> {
        // SAFETY: The owned endpoint keeps this event handle live.
        unsafe { SetEvent(as_handle(event))? };
        Ok(())
    }

    pub fn reset_event(event: BorrowedHandle<'_>) -> Result<(), RingError> {
        // SAFETY: The owned endpoint keeps this event handle live.
        unsafe { ResetEvent(as_handle(event))? };
        Ok(())
    }

    pub fn wait_event(
        event: BorrowedHandle<'_>,
        timeout: Duration,
    ) -> Result<FramesReady, RingError> {
        let timeout_ms = duration_to_millis(timeout);
        // SAFETY: The owned endpoint keeps this event handle live and timeout
        // is finite (INFINITE is never emitted).
        let result = unsafe { WaitForSingleObject(as_handle(event), timeout_ms) };
        if result == WAIT_OBJECT_0 {
            Ok(FramesReady::Signaled)
        } else if result == WAIT_TIMEOUT {
            Ok(FramesReady::Timeout)
        } else if result == WAIT_FAILED {
            Err(RingError::Windows(WindowsError::from_thread()))
        } else {
            Err(RingError::UnexpectedWaitStatus { actual: result.0 })
        }
    }

    fn duration_to_millis(timeout: Duration) -> u32 {
        if timeout.is_zero() {
            return 0;
        }
        let rounded_up = timeout.as_millis().max(1);
        u32::try_from(rounded_up.min(u128::from(u32::MAX - 1)))
            .expect("clamped finite Windows timeout fits u32")
    }

    fn as_handle(handle: BorrowedHandle<'_>) -> HANDLE {
        HANDLE(handle.as_raw_handle())
    }

    fn close_target_handle(target_process: BorrowedHandle<'_>, target_handle: HANDLE) {
        let mut local_duplicate = HANDLE::default();
        // SAFETY: Moving the remote handle back with DUPLICATE_CLOSE_SOURCE is
        // the only general cleanup available after a partially failed pair.
        let moved = unsafe {
            DuplicateHandle(
                as_handle(target_process),
                target_handle,
                GetCurrentProcess(),
                &raw mut local_duplicate,
                0,
                false,
                DUPLICATE_SAME_ACCESS | DUPLICATE_CLOSE_SOURCE,
            )
        };
        if moved.is_ok() && !local_duplicate.is_invalid() {
            // SAFETY: local_duplicate is now owned in the current process and
            // is closed exactly once here.
            let _ = unsafe { CloseHandle(local_duplicate) };
        }
    }
}
