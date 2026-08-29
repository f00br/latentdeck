"""H3 cadence adapter for the native RGB Ring ABI 1 producer."""

from __future__ import annotations

import time
import uuid
from collections.abc import Mapping, Sequence
from typing import Protocol

MAX_MAPPING_BYTES = 256 * 1024 * 1024


class H3RingError(RuntimeError):
    """The H3 cycle or native ring binding violated its closed contract."""

    code = "h3_ring_contract"

    def __init__(self, detail: str) -> None:
        super().__init__(detail)
        self.detail = detail


class NativeProducer(Protocol):
    write_sequence: int
    read_sequence: int
    occupancy: int

    def can_publish(self, frame_count: int) -> bool: ...
    def publish_cycle(self, frames: Sequence[bytes], timestamp_ns: int) -> tuple[int, int]: ...
    def set_generation(self, stream_generation: int) -> None: ...
    def close(self) -> None: ...


class NativeProducerType(Protocol):
    @classmethod
    def open(
        cls,
        mapping_handle: int,
        frames_ready_event_handle: int,
        mapping_bytes: int,
        expected_generation: int,
        expected_width: int,
        expected_height: int,
    ) -> NativeProducer: ...


def _u64(value: object, label: str, *, nonzero: bool = False) -> int:
    valid = (
        isinstance(value, int)
        and not isinstance(value, bool)
        and 0 <= value <= 0xFFFF_FFFF_FFFF_FFFF
    )
    if not valid or (nonzero and value == 0):
        raise H3RingError(f"{label} is not a valid unsigned handle or bound")
    return value


def _u32(value: object, label: str, *, nonzero: bool = False) -> int:
    integer = _u64(value, label, nonzero=nonzero)
    if integer > 0xFFFF_FFFF:
        raise H3RingError(f"{label} exceeds the u32 range")
    return integer


def _ring_id(value: object) -> str:
    if not isinstance(value, str):
        raise H3RingError("ring_id is not a canonical UUID")
    try:
        parsed = uuid.UUID(value)
    except ValueError as error:
        raise H3RingError("ring_id is not a canonical UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise H3RingError("ring_id is not a canonical non-nil UUID")
    return value


class WindowsRingProducer:
    """Codec-valid H3 publication over the native Rust ring producer."""

    def __init__(
        self,
        native: NativeProducer,
        *,
        ring_id: str,
        stream_generation: int,
    ) -> None:
        self._native = native
        self._ring_id = ring_id
        self._stream_generation = stream_generation
        self._closed = False

    @classmethod
    def bind(
        cls,
        payload: Mapping[str, object],
        width: int,
        height: int,
        stream_generation: int,
        *,
        native_producer_type: NativeProducerType | None = None,
    ) -> WindowsRingProducer:
        """Validate RingBind and claim its worker-local duplicated handles."""

        if set(payload) != {
            "layout_version",
            "mapping_handle",
            "mapping_bytes",
            "frames_ready_event_handle",
            "ring_id",
        }:
            raise H3RingError("ring binding fields do not match Worker Protocol 1")
        if _u32(payload["layout_version"], "layout_version") != 1:
            raise H3RingError("RGB Ring ABI 1 was not selected")
        mapping_handle = _u64(payload["mapping_handle"], "mapping_handle", nonzero=True)
        event_handle = _u64(
            payload["frames_ready_event_handle"],
            "frames_ready_event_handle",
            nonzero=True,
        )
        mapping_bytes = _u64(payload["mapping_bytes"], "mapping_bytes", nonzero=True)
        if not 4096 <= mapping_bytes <= MAX_MAPPING_BYTES:
            raise H3RingError("mapping_bytes is outside the RGB Ring ABI 1 bound")
        checked_width = _u32(width, "width", nonzero=True)
        checked_height = _u32(height, "height", nonzero=True)
        generation = _u64(stream_generation, "stream_generation", nonzero=True)
        ring_id = _ring_id(payload["ring_id"])

        if native_producer_type is None:
            from latentdeck_rgb_ring import WindowsRgbRingProducer

            native_producer_type = WindowsRgbRingProducer
        native = native_producer_type.open(
            mapping_handle,
            event_handle,
            mapping_bytes,
            generation,
            checked_width,
            checked_height,
        )
        return cls(native, ring_id=ring_id, stream_generation=generation)

    @property
    def write_sequence(self) -> int:
        return int(self._native.write_sequence)

    @property
    def read_sequence(self) -> int:
        return int(self._native.read_sequence)

    @property
    def occupancy(self) -> int:
        return int(self._native.occupancy)

    @property
    def presentation_skipped_total(self) -> int:
        return 0

    def can_publish(self, frame_count: int) -> bool:
        if self._closed:
            raise H3RingError("RGB ring producer is closed")
        count = _u32(frame_count, "frame_count")
        return bool(self._native.can_publish(count))

    def publish_cycle(
        self,
        frames: Sequence[bytes],
        *,
        stream_generation: int,
        cycle_index: int,
        decoded_start_frame: int,
    ) -> tuple[int, int]:
        """Publish one complete H3 prime or steady cycle without partial writes."""

        if self._closed:
            raise H3RingError("RGB ring producer is closed")
        generation = _u64(stream_generation, "stream_generation", nonzero=True)
        if generation != self._stream_generation:
            raise H3RingError("H3 cycle generation does not match the bound ring")
        cycle = _u32(cycle_index, "cycle_index")
        decoded_start = _u64(decoded_start_frame, "decoded_start_frame")
        expected_count = 5 if cycle == 0 else 17
        expected_start = 0 if cycle == 0 else 5 + (cycle - 1) * 17
        if len(frames) != expected_count or decoded_start != expected_start:
            raise H3RingError("frame batch is not a codec-valid H3 decode cycle")
        return self._publish(frames, expected_count)

    def publish_frames(
        self,
        frames: Sequence[bytes],
        *,
        stream_generation: int,
    ) -> tuple[int, int]:
        """Publish one D2/Q4 post-operator decoder slot (one to four frames)."""

        if self._closed:
            raise H3RingError("RGB ring producer is closed")
        generation = _u64(stream_generation, "stream_generation", nonzero=True)
        if generation != self._stream_generation:
            raise H3RingError("latent deck slot generation does not match the bound ring")
        count = len(frames)
        if not 1 <= count <= 4:
            raise H3RingError("latent deck slot must contain one to four H3 frames")
        return self._publish(frames, count)

    def _publish(self, frames: Sequence[bytes], expected_count: int) -> tuple[int, int]:
        if not self.can_publish(expected_count):
            raise H3RingError("RGB ring cannot publish the complete frame batch")
        first_expected = self.write_sequence + 1
        first, last_exclusive = self._native.publish_cycle(frames, time.monotonic_ns())
        if first != first_expected or last_exclusive != first + expected_count:
            raise H3RingError("native RGB ring returned an invalid sequence range")
        return first, last_exclusive

    def set_generation(self, stream_generation: int) -> None:
        if self._closed:
            raise H3RingError("RGB ring producer is closed")
        generation = _u64(stream_generation, "stream_generation", nonzero=True)
        if generation <= self._stream_generation:
            raise H3RingError("RGB ring generation must increase")
        self._native.set_generation(generation)
        self._stream_generation = generation

    def close(self) -> None:
        if self._closed:
            return
        self._native.close()
        self._closed = True


__all__ = ["H3RingError", "WindowsRingProducer"]
