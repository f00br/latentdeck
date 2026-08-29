//! Native Python bridge for the worker-side RGB Ring ABI 1 producer.

use std::{error::Error, fmt};

use latentdeck_gpu::ring::RingError as CoreRingError;
use pyo3::{exceptions::PyException, prelude::*, pybacked::PyBackedBytes};

const BINDING_ABI_VERSION: &str = "1";

#[derive(Debug)]
#[cfg_attr(
    not(target_os = "windows"),
    allow(
        dead_code,
        reason = "Windows diagnostics remain part of one cross-platform exception type"
    )
)]
enum BridgeError {
    Core(Box<CoreRingError>),
    InvalidHandle {
        field: &'static str,
        value: u64,
    },
    AliasedHandles,
    GeometryMismatch {
        expected_width: u32,
        expected_height: u32,
        actual_width: u32,
        actual_height: u32,
    },
    EmptyBatch,
    BatchTooLarge {
        actual: usize,
    },
    Backpressure {
        requested: u32,
        available: u32,
    },
    BackpressureAfterPreflight,
    SequenceMismatch {
        expected: u64,
        actual: u64,
    },
    Closed,
    #[cfg(not(target_os = "windows"))]
    UnsupportedPlatform,
}

impl BridgeError {
    const fn code(&self) -> &'static str {
        match self {
            Self::Core(error) => error.code(),
            Self::InvalidHandle { .. } => "ring_invalid_handle",
            Self::AliasedHandles => "ring_aliased_handles",
            Self::GeometryMismatch { .. } => "ring_geometry_mismatch",
            Self::EmptyBatch => "ring_batch_empty",
            Self::BatchTooLarge { .. } => "ring_batch_too_large",
            Self::Backpressure { .. } => "ring_backpressure",
            Self::BackpressureAfterPreflight => "ring_backpressure_after_preflight",
            Self::SequenceMismatch { .. } => "ring_sequence_mismatch",
            Self::Closed => "ring_closed",
            #[cfg(not(target_os = "windows"))]
            Self::UnsupportedPlatform => "ring_unsupported_platform",
        }
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => error.fmt(formatter),
            Self::InvalidHandle { field, value } => {
                write!(
                    formatter,
                    "{field} is not a valid owned Windows handle: {value}"
                )
            }
            Self::AliasedHandles => formatter.write_str(
                "mapping_handle and frames_ready_event_handle must be distinct owned handles",
            ),
            Self::GeometryMismatch {
                expected_width,
                expected_height,
                actual_width,
                actual_height,
            } => write!(
                formatter,
                "RGB ring geometry mismatch: expected {expected_width}x{expected_height}, found {actual_width}x{actual_height}"
            ),
            Self::EmptyBatch => formatter.write_str("RGB ring frame batch must not be empty"),
            Self::BatchTooLarge { actual } => write!(
                formatter,
                "RGB ring frame batch length {actual} does not fit the ABI counter"
            ),
            Self::Backpressure {
                requested,
                available,
            } => write!(
                formatter,
                "RGB ring cannot publish all {requested} frames; {available} slots are available"
            ),
            Self::BackpressureAfterPreflight => formatter.write_str(
                "RGB ring reported backpressure after a successful full-batch preflight",
            ),
            Self::SequenceMismatch { expected, actual } => write!(
                formatter,
                "RGB ring committed sequence {actual}, expected {expected}"
            ),
            Self::Closed => formatter.write_str("RGB ring producer is closed"),
            #[cfg(not(target_os = "windows"))]
            Self::UnsupportedPlatform => {
                formatter.write_str("RGB Ring handle transport is available only on Windows")
            }
        }
    }
}

impl Error for BridgeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Core(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<CoreRingError> for BridgeError {
    fn from(error: CoreRingError) -> Self {
        Self::Core(Box::new(error))
    }
}

#[derive(Clone, Copy)]
struct StateSnapshot {
    write_sequence: u64,
    read_sequence: u64,
    occupancy: u32,
    available_capacity: u32,
}

#[cfg(target_os = "windows")]
mod platform {
    #![allow(
        unsafe_code,
        reason = "audited ownership transfer for target-valid Windows RingBind handles"
    )]

    use std::ffi::c_void;
    use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};

    use latentdeck_gpu::{ring::WriteStatus, windows_ring::WindowsRgbRingProducer as CoreProducer};

    use super::{BridgeError, CoreRingError, StateSnapshot};

    pub(super) struct ProducerEndpoint {
        inner: CoreProducer,
    }

    impl ProducerEndpoint {
        pub(super) fn open(
            mapping_handle: u64,
            frames_ready_event_handle: u64,
            mapping_bytes: u64,
            expected_generation: u64,
            expected_width: u32,
            expected_height: u32,
        ) -> Result<Self, BridgeError> {
            // Validate both integer representations before taking ownership of
            // either handle. Once converted, every following error path drops
            // both OwnedHandles or the CoreProducer that received them.
            let mapping_raw = checked_raw_handle(mapping_handle, "mapping_handle")?;
            let event_raw =
                checked_raw_handle(frames_ready_event_handle, "frames_ready_event_handle")?;
            if mapping_raw == event_raw {
                // A duplicated handle may be transferred only once. Close the
                // single malformed target handle without manufacturing two
                // OwnedHandles for the same numeric value.
                drop(take_target_handle(mapping_raw));
                return Err(BridgeError::AliasedHandles);
            }
            let mapping_owned = take_target_handle(mapping_raw);
            let event_owned = take_target_handle(event_raw);
            let inner = CoreProducer::open_from_owned_handles(
                mapping_owned,
                event_owned,
                mapping_bytes,
                expected_generation,
            )?;
            let descriptor = inner.descriptor();
            let layout = descriptor.layout();
            if layout.width() != expected_width || layout.height() != expected_height {
                return Err(BridgeError::GeometryMismatch {
                    expected_width,
                    expected_height,
                    actual_width: layout.width(),
                    actual_height: layout.height(),
                });
            }
            Ok(Self { inner })
        }

        pub(super) fn width(&self) -> u32 {
            self.inner.descriptor().layout().width()
        }

        pub(super) fn height(&self) -> u32 {
            self.inner.descriptor().layout().height()
        }

        pub(super) fn row_stride(&self) -> u32 {
            self.inner.descriptor().layout().row_stride()
        }

        pub(super) fn mapping_bytes(&self) -> u64 {
            self.inner.descriptor().layout().mapping_bytes()
        }

        pub(super) fn generation(&self) -> u64 {
            self.inner.descriptor().generation()
        }

        pub(super) fn state(&self) -> Result<StateSnapshot, BridgeError> {
            let state = self.inner.state()?;
            Ok(StateSnapshot {
                write_sequence: state.producer_sequence(),
                read_sequence: state.consumer_sequence(),
                occupancy: state.occupancy(),
                available_capacity: state.available_capacity(),
            })
        }

        pub(super) fn can_publish(&self, frame_count: u32) -> Result<bool, BridgeError> {
            self.inner.can_publish(frame_count).map_err(Into::into)
        }

        pub(super) fn publish_cycle<B: AsRef<[u8]>>(
            &mut self,
            frames: &[B],
            timestamp_ns: u64,
        ) -> Result<(u64, u64), BridgeError> {
            if frames.is_empty() {
                return Err(BridgeError::EmptyBatch);
            }
            let frame_count =
                u32::try_from(frames.len()).map_err(|_| BridgeError::BatchTooLarge {
                    actual: frames.len(),
                })?;
            let expected_frame_bytes = tight_frame_bytes(self.width(), self.height())?;
            // Validate every frame before reading queue capacity or committing
            // the first slot. This makes a bad later frame a zero-write error.
            for frame in frames {
                let actual = frame.as_ref().len();
                if actual != expected_frame_bytes {
                    return Err(CoreRingError::FrameLengthMismatch {
                        expected: expected_frame_bytes,
                        actual,
                    }
                    .into());
                }
            }

            let state = self.state()?;
            if !self.inner.can_publish(frame_count)? {
                return Err(BridgeError::Backpressure {
                    requested: frame_count,
                    available: state.available_capacity,
                });
            }
            let first = state
                .write_sequence
                .checked_add(1)
                .ok_or(CoreRingError::SequenceExhausted)?;
            let last_exclusive = first
                .checked_add(u64::from(frame_count))
                .ok_or(CoreRingError::SequenceExhausted)?;

            for (offset, frame) in frames.iter().enumerate() {
                let expected_sequence = first
                    .checked_add(
                        u64::try_from(offset).map_err(|_| BridgeError::BatchTooLarge {
                            actual: frames.len(),
                        })?,
                    )
                    .ok_or(CoreRingError::SequenceExhausted)?;
                match self.inner.try_write(frame.as_ref(), timestamp_ns)? {
                    WriteStatus::Written(metadata) => {
                        if metadata.sequence() != expected_sequence {
                            return Err(BridgeError::SequenceMismatch {
                                expected: expected_sequence,
                                actual: metadata.sequence(),
                            });
                        }
                    }
                    WriteStatus::Backpressure(_) => {
                        return Err(BridgeError::BackpressureAfterPreflight);
                    }
                }
            }
            Ok((first, last_exclusive))
        }

        pub(super) fn set_generation(&mut self, new_generation: u64) -> Result<(), BridgeError> {
            self.inner
                .set_generation(new_generation)
                .map_err(Into::into)
        }
    }

    fn tight_frame_bytes(width: u32, height: u32) -> Result<usize, BridgeError> {
        usize::try_from(
            u64::from(width)
                .checked_mul(u64::from(height))
                .and_then(|pixels| pixels.checked_mul(4))
                .ok_or(CoreRingError::LayoutOverflow)?,
        )
        .map_err(|_| CoreRingError::LayoutOverflow.into())
    }

    fn checked_raw_handle(value: u64, field: &'static str) -> Result<RawHandle, BridgeError> {
        let address =
            usize::try_from(value).map_err(|_| BridgeError::InvalidHandle { field, value })?;
        if address == 0 || address == usize::MAX {
            return Err(BridgeError::InvalidHandle { field, value });
        }
        Ok(std::ptr::with_exposed_provenance_mut::<c_void>(address))
    }

    fn take_target_handle(raw: RawHandle) -> OwnedHandle {
        // SAFETY: RingBind values are duplicated into this exact worker
        // process, checked non-null/non-INVALID_HANDLE_VALUE above, and are
        // transferred exactly once into OwnedHandle. CoreProducer consumes
        // them and closes them on all success/error/drop paths.
        unsafe { OwnedHandle::from_raw_handle(raw) }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{BridgeError, StateSnapshot};

    pub(super) struct ProducerEndpoint;

    impl ProducerEndpoint {
        #[allow(clippy::too_many_arguments)]
        pub(super) fn open(
            _mapping_handle: u64,
            _frames_ready_event_handle: u64,
            _mapping_bytes: u64,
            _expected_generation: u64,
            _expected_width: u32,
            _expected_height: u32,
        ) -> Result<Self, BridgeError> {
            Err(BridgeError::UnsupportedPlatform)
        }

        pub(super) const fn width(&self) -> u32 {
            0
        }

        pub(super) const fn height(&self) -> u32 {
            0
        }

        pub(super) const fn row_stride(&self) -> u32 {
            0
        }

        pub(super) const fn mapping_bytes(&self) -> u64 {
            0
        }

        pub(super) const fn generation(&self) -> u64 {
            0
        }

        pub(super) fn state(&self) -> Result<StateSnapshot, BridgeError> {
            Err(BridgeError::UnsupportedPlatform)
        }

        pub(super) fn can_publish(&self, _frame_count: u32) -> Result<bool, BridgeError> {
            Err(BridgeError::UnsupportedPlatform)
        }

        pub(super) fn publish_cycle<B: AsRef<[u8]>>(
            &mut self,
            _frames: &[B],
            _timestamp_ns: u64,
        ) -> Result<(u64, u64), BridgeError> {
            Err(BridgeError::UnsupportedPlatform)
        }

        pub(super) fn set_generation(&mut self, _new_generation: u64) -> Result<(), BridgeError> {
            Err(BridgeError::UnsupportedPlatform)
        }
    }
}

use platform::ProducerEndpoint;

/// Native exception with a stable RGB Ring diagnostic category.
#[pyclass(extends=PyException, module = "latentdeck_rgb_ring._native")]
struct RingError {
    #[pyo3(get)]
    code: String,
    #[pyo3(get)]
    detail: String,
}

#[pymethods]
impl RingError {
    #[new]
    fn new(code: String, detail: String) -> Self {
        Self { code, detail }
    }

    fn __str__(&self) -> String {
        format!("{}: {}", self.code, self.detail)
    }
}

/// Sole worker-side native producer for one RGB Ring ABI 1 mapping.
#[pyclass(module = "latentdeck_rgb_ring._native", unsendable)]
struct WindowsRgbRingProducer {
    endpoint: Option<ProducerEndpoint>,
}

#[pymethods]
impl WindowsRgbRingProducer {
    #[staticmethod]
    #[pyo3(signature = (
        mapping_handle,
        frames_ready_event_handle,
        mapping_bytes,
        expected_generation,
        expected_width,
        expected_height
    ))]
    fn open(
        py: Python<'_>,
        mapping_handle: u64,
        frames_ready_event_handle: u64,
        mapping_bytes: u64,
        expected_generation: u64,
        expected_width: u32,
        expected_height: u32,
    ) -> PyResult<Self> {
        ProducerEndpoint::open(
            mapping_handle,
            frames_ready_event_handle,
            mapping_bytes,
            expected_generation,
            expected_width,
            expected_height,
        )
        .map(|endpoint| Self {
            endpoint: Some(endpoint),
        })
        .map_err(|error| into_py_error(py, &error))
    }

    #[getter]
    fn width(&self, py: Python<'_>) -> PyResult<u32> {
        self.endpoint(py).map(ProducerEndpoint::width)
    }

    #[getter]
    fn height(&self, py: Python<'_>) -> PyResult<u32> {
        self.endpoint(py).map(ProducerEndpoint::height)
    }

    #[getter]
    fn row_stride(&self, py: Python<'_>) -> PyResult<u32> {
        self.endpoint(py).map(ProducerEndpoint::row_stride)
    }

    #[getter]
    fn mapping_bytes(&self, py: Python<'_>) -> PyResult<u64> {
        self.endpoint(py).map(ProducerEndpoint::mapping_bytes)
    }

    #[getter]
    fn generation(&self, py: Python<'_>) -> PyResult<u64> {
        self.endpoint(py).map(ProducerEndpoint::generation)
    }

    #[getter]
    fn write_sequence(&self, py: Python<'_>) -> PyResult<u64> {
        self.state(py).map(|state| state.write_sequence)
    }

    #[getter]
    fn read_sequence(&self, py: Python<'_>) -> PyResult<u64> {
        self.state(py).map(|state| state.read_sequence)
    }

    #[getter]
    fn occupancy(&self, py: Python<'_>) -> PyResult<u32> {
        self.state(py).map(|state| state.occupancy)
    }

    #[getter]
    fn available_capacity(&self, py: Python<'_>) -> PyResult<u32> {
        self.state(py).map(|state| state.available_capacity)
    }

    fn can_publish(&self, py: Python<'_>, frame_count: u32) -> PyResult<bool> {
        self.endpoint(py)?
            .can_publish(frame_count)
            .map_err(|error| into_py_error(py, &error))
    }

    #[pyo3(signature = (frames, timestamp_ns))]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "PyO3 owns the extracted Python sequence and its byte-buffer references"
    )]
    fn publish_cycle(
        &mut self,
        py: Python<'_>,
        frames: Vec<PyBackedBytes>,
        timestamp_ns: u64,
    ) -> PyResult<(u64, u64)> {
        self.endpoint_mut(py)?
            .publish_cycle(&frames, timestamp_ns)
            .map_err(|error| into_py_error(py, &error))
    }

    fn set_generation(&mut self, py: Python<'_>, new_generation: u64) -> PyResult<()> {
        self.endpoint_mut(py)?
            .set_generation(new_generation)
            .map_err(|error| into_py_error(py, &error))
    }

    fn close(&mut self) {
        self.endpoint.take();
    }
}

impl WindowsRgbRingProducer {
    fn endpoint(&self, py: Python<'_>) -> PyResult<&ProducerEndpoint> {
        self.endpoint.as_ref().ok_or_else(|| {
            let error = BridgeError::Closed;
            into_py_error(py, &error)
        })
    }

    fn endpoint_mut(&mut self, py: Python<'_>) -> PyResult<&mut ProducerEndpoint> {
        self.endpoint.as_mut().ok_or_else(|| {
            let error = BridgeError::Closed;
            into_py_error(py, &error)
        })
    }

    fn state(&self, py: Python<'_>) -> PyResult<StateSnapshot> {
        self.endpoint(py)?
            .state()
            .map_err(|error| into_py_error(py, &error))
    }
}

fn into_py_error(py: Python<'_>, error: &BridgeError) -> PyErr {
    let exception_type = py.get_type::<RingError>();
    match exception_type.call1((error.code(), error.to_string())) {
        Ok(instance) => PyErr::from_value(instance.into_any()),
        Err(construction_error) => construction_error,
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<RingError>()?;
    module.add_class::<WindowsRgbRingProducer>()?;
    module.add("BINDING_ABI_VERSION", BINDING_ABI_VERSION)?;
    Ok(())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    #![allow(unsafe_code, reason = "real-Windows handle lifecycle test boundary")]

    use std::{
        os::windows::io::{BorrowedHandle, RawHandle},
        sync::Mutex,
        time::Duration,
    };

    use latentdeck_gpu::{
        ring::{ReadStatus, RingDescriptor},
        windows_ring::{FramesReady, WindowsRgbRingOwner},
    };
    use pyo3::{
        Python,
        types::{PyAnyMethods, PyBytes},
    };
    use windows::Win32::{
        Foundation::{GetHandleInformation, HANDLE},
        System::Threading::GetCurrentProcess,
    };

    use super::ProducerEndpoint;

    const WIDTH: u32 = 2;
    const HEIGHT: u32 = 2;
    const FRAME_BYTES: usize = 16;
    // Windows may immediately recycle a closed numeric handle into another
    // parallel test. Serialize lifecycle assertions so `handle_is_valid`
    // observes the handle that this test transferred, not a recycled value.
    static HANDLE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn current_process() -> BorrowedHandle<'static> {
        // SAFETY: GetCurrentProcess returns the immortal pseudo-handle for this
        // test process; BorrowedHandle never closes it.
        let raw: RawHandle = unsafe { GetCurrentProcess() }.0;
        // SAFETY: The pseudo-handle is valid for the entire process lifetime.
        unsafe { BorrowedHandle::borrow_raw(raw) }
    }

    fn raw_handle(value: u64) -> HANDLE {
        let address = usize::try_from(value).expect("test handle fits this process");
        HANDLE(std::ptr::with_exposed_provenance_mut(address))
    }

    fn handle_is_valid(value: u64) -> bool {
        let mut flags = 0_u32;
        // SAFETY: The function only probes the numeric handle in the current
        // process and writes to an initialized local DWORD.
        unsafe { GetHandleInformation(raw_handle(value), &raw mut flags) }.is_ok()
    }

    fn make_owner(generation: u64) -> WindowsRgbRingOwner {
        WindowsRgbRingOwner::create(
            RingDescriptor::new(WIDTH, HEIGHT, generation).expect("valid descriptor"),
        )
        .expect("anonymous ring owner")
    }

    fn duplicate_and_open(
        owner: &WindowsRgbRingOwner,
        generation: u64,
    ) -> (ProducerEndpoint, u64, u64) {
        let binding = owner
            .duplicate_into(current_process())
            .expect("duplicate ring into current test process");
        let mapping_handle = binding.mapping_handle();
        let event_handle = binding.frames_ready_event_handle();
        let endpoint = ProducerEndpoint::open(
            mapping_handle,
            event_handle,
            binding.mapping_bytes(),
            generation,
            WIDTH,
            HEIGHT,
        )
        .expect("open native producer");
        (endpoint, mapping_handle, event_handle)
    }

    fn frames(count: usize, marker: u8) -> Vec<Vec<u8>> {
        vec![vec![marker; FRAME_BYTES]; count]
    }

    #[test]
    fn publishes_prime_and_steady_cycles_with_capacity_preflight_and_reset() {
        let _guard = HANDLE_TEST_LOCK.lock().expect("handle test lock");
        let owner = make_owner(1);
        let mut consumer = owner.open_consumer().expect("open native consumer");
        let (mut producer, mapping_handle, event_handle) = duplicate_and_open(&owner, 1);

        assert_eq!(producer.width(), WIDTH);
        assert_eq!(producer.height(), HEIGHT);
        assert_eq!(producer.generation(), 1);
        assert_eq!(producer.state().expect("initial state").occupancy, 0);
        assert_eq!(
            producer
                .publish_cycle(&frames(5, 0x11), 100)
                .expect("publish prime cycle"),
            (1, 6)
        );
        assert_eq!(
            producer
                .publish_cycle(&frames(17, 0x22), 200)
                .expect("publish steady cycle"),
            (6, 23)
        );

        let state = producer.state().expect("published state");
        assert_eq!(state.write_sequence, 22);
        assert_eq!(state.read_sequence, 0);
        assert_eq!(state.occupancy, 22);
        assert_eq!(state.available_capacity, 2);
        assert!(!producer.can_publish(5).expect("capacity query"));
        assert_eq!(
            producer
                .publish_cycle(&frames(5, 0x33), 300)
                .expect_err("five frames do not fit")
                .code(),
            "ring_backpressure"
        );
        assert_eq!(
            producer.state().expect("no partial batch").write_sequence,
            22
        );
        assert_eq!(
            consumer
                .wait_frames_ready(Duration::ZERO)
                .expect("frames-ready event"),
            FramesReady::Signaled
        );

        for expected_sequence in 1..=22 {
            let ReadStatus::Frame(frame) = consumer.try_read().expect("read committed frame")
            else {
                panic!("expected committed frame {expected_sequence}");
            };
            assert_eq!(frame.sequence(), expected_sequence);
            assert_eq!(frame.generation(), 1);
            assert_eq!(
                frame.timestamp_ns(),
                if expected_sequence <= 5 { 100 } else { 200 }
            );
        }
        assert_eq!(consumer.try_read().expect("empty queue"), ReadStatus::Empty);

        producer.set_generation(2).expect("strict generation reset");
        consumer
            .adopt_generation(2)
            .expect("consumer adopts acknowledged generation");
        let reset = producer.state().expect("reset state");
        assert_eq!(producer.generation(), 2);
        assert_eq!(reset.write_sequence, 0);
        assert_eq!(reset.read_sequence, 0);
        assert_eq!(reset.occupancy, 0);
        assert_eq!(
            producer
                .set_generation(2)
                .expect_err("generation must increase strictly")
                .code(),
            "ring_generation_not_increasing"
        );
        assert_eq!(
            producer
                .publish_cycle(&frames(5, 0x44), 400)
                .expect("publish after reset"),
            (1, 6)
        );

        drop(producer);
        assert!(!handle_is_valid(mapping_handle));
        assert!(!handle_is_valid(event_handle));
    }

    #[test]
    fn rejects_late_bad_frame_before_any_commit_and_releases_producer_claim() {
        let _guard = HANDLE_TEST_LOCK.lock().expect("handle test lock");
        let owner = make_owner(7);
        let (mut producer, _, _) = duplicate_and_open(&owner, 7);
        let mut batch = frames(5, 0x55);
        batch[4].pop();
        let error = producer
            .publish_cycle(&batch, 500)
            .expect_err("late short frame must fail preflight");
        assert_eq!(error.code(), "ring_frame_length");
        assert_eq!(
            producer.state().expect("zero-write failure").write_sequence,
            0
        );

        let second = owner
            .duplicate_into(current_process())
            .expect("duplicate second producer handles");
        let second_mapping = second.mapping_handle();
        let second_event = second.frames_ready_event_handle();
        let Err(error) = ProducerEndpoint::open(
            second_mapping,
            second_event,
            second.mapping_bytes(),
            7,
            WIDTH,
            HEIGHT,
        ) else {
            panic!("sole producer claim must reject a second endpoint");
        };
        assert_eq!(error.code(), "ring_producer_claimed");
        assert!(!handle_is_valid(second_mapping));
        assert!(!handle_is_valid(second_event));

        drop(producer);
        let (replacement, mapping_handle, event_handle) = duplicate_and_open(&owner, 7);
        drop(replacement);
        assert!(!handle_is_valid(mapping_handle));
        assert!(!handle_is_valid(event_handle));
    }

    #[test]
    fn geometry_mismatch_closes_transferred_handles() {
        let _guard = HANDLE_TEST_LOCK.lock().expect("handle test lock");
        let owner = make_owner(9);
        let binding = owner
            .duplicate_into(current_process())
            .expect("duplicate mismatch handles");
        let mapping_handle = binding.mapping_handle();
        let event_handle = binding.frames_ready_event_handle();
        let Err(error) = ProducerEndpoint::open(
            mapping_handle,
            event_handle,
            binding.mapping_bytes(),
            9,
            WIDTH + 1,
            HEIGHT,
        ) else {
            panic!("geometry is part of the worker bind contract");
        };
        assert_eq!(error.code(), "ring_geometry_mismatch");
        assert!(!handle_is_valid(mapping_handle));
        assert!(!handle_is_valid(event_handle));
    }

    #[test]
    fn python_methods_publish_real_bytes_to_the_native_consumer() {
        let _guard = HANDLE_TEST_LOCK.lock().expect("handle test lock");
        let owner = make_owner(11);
        let mut consumer = owner.open_consumer().expect("open native consumer");
        let binding = owner
            .duplicate_into(current_process())
            .expect("duplicate Python bridge handles");
        let mapping_handle = binding.mapping_handle();
        let event_handle = binding.frames_ready_event_handle();

        Python::initialize();
        Python::attach(|py| {
            let producer_type = py.get_type::<super::WindowsRgbRingProducer>();
            let producer = producer_type
                .call_method1(
                    "open",
                    (
                        mapping_handle,
                        event_handle,
                        binding.mapping_bytes(),
                        11_u64,
                        WIDTH,
                        HEIGHT,
                    ),
                )
                .expect("call Python open factory");
            let tight_rgba = vec![0x66; FRAME_BYTES];
            let frames = (0..5)
                .map(|_| PyBytes::new(py, &tight_rgba))
                .collect::<Vec<_>>();
            let sequence_range = producer
                .call_method1("publish_cycle", (frames, 600_u64))
                .expect("call Python publication method")
                .extract::<(u64, u64)>()
                .expect("extract Python sequence range");
            assert_eq!(sequence_range, (1, 6));
            assert_eq!(
                producer
                    .getattr("occupancy")
                    .expect("Python occupancy")
                    .extract::<u32>()
                    .expect("extract Python occupancy"),
                5
            );
            producer
                .call_method0("close")
                .expect("close Python producer");
        });

        for expected_sequence in 1..=5 {
            let ReadStatus::Frame(frame) = consumer.try_read().expect("read Python frame") else {
                panic!("expected Python frame {expected_sequence}");
            };
            assert_eq!(frame.sequence(), expected_sequence);
            assert_eq!(frame.timestamp_ns(), 600);
        }
        assert!(!handle_is_valid(mapping_handle));
        assert!(!handle_is_valid(event_handle));
    }
}
