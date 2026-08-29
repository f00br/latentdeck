from __future__ import annotations

from collections.abc import Sequence

import pytest

from latentdeck_codec_h3.ring import H3RingError, WindowsRingProducer

RING_ID = "bbfb89cc-0739-423f-9474-d03e01bc34aa"


class FakeNativeProducer:
    open_args: tuple[int, int, int, int, int, int] | None = None
    last_instance: FakeNativeProducer | None = None

    def __init__(self, width: int, height: int, generation: int) -> None:
        self.width = width
        self.height = height
        self.generation = generation
        self.write_sequence = 0
        self.read_sequence = 0
        self.occupancy = 0
        self.available_capacity = 24
        self.closed = False

    @classmethod
    def open(
        cls,
        mapping_handle: int,
        frames_ready_event_handle: int,
        mapping_bytes: int,
        expected_generation: int,
        expected_width: int,
        expected_height: int,
    ) -> FakeNativeProducer:
        cls.open_args = (
            mapping_handle,
            frames_ready_event_handle,
            mapping_bytes,
            expected_generation,
            expected_width,
            expected_height,
        )
        instance = cls(expected_width, expected_height, expected_generation)
        cls.last_instance = instance
        return instance

    def can_publish(self, frame_count: int) -> bool:
        return frame_count <= self.available_capacity

    def publish_cycle(self, frames: Sequence[bytes], timestamp_ns: int) -> tuple[int, int]:
        assert timestamp_ns > 0
        if not self.can_publish(len(frames)):
            raise RuntimeError("native full-cycle backpressure")
        first = self.write_sequence + 1
        self.write_sequence += len(frames)
        self.occupancy += len(frames)
        self.available_capacity -= len(frames)
        return first, self.write_sequence + 1

    def set_generation(self, generation: int) -> None:
        assert generation > self.generation
        self.generation = generation
        self.write_sequence = 0
        self.read_sequence = 0
        self.occupancy = 0
        self.available_capacity = 24

    def close(self) -> None:
        self.closed = True


def binding_payload() -> dict[str, object]:
    return {
        "layout_version": 1,
        "mapping_handle": 101,
        "mapping_bytes": 1_052_672,
        "frames_ready_event_handle": 202,
        "ring_id": RING_ID,
    }


def frames(count: int) -> tuple[bytes, ...]:
    return tuple(bytes(16 * 16 * 4) for _ in range(count))


def test_bind_passes_only_handles_bounds_generation_and_geometry_to_native() -> None:
    ring = WindowsRingProducer.bind(
        binding_payload(),
        width=16,
        height=16,
        stream_generation=7,
        native_producer_type=FakeNativeProducer,
    )

    assert FakeNativeProducer.open_args == (101, 202, 1_052_672, 7, 16, 16)
    assert ring.write_sequence == 0
    assert ring.read_sequence == 0
    assert ring.occupancy == 0
    assert ring.presentation_skipped_total == 0


def test_prime_and_steady_cycles_publish_as_one_preflighted_batch() -> None:
    ring = WindowsRingProducer.bind(
        binding_payload(),
        16,
        16,
        1,
        native_producer_type=FakeNativeProducer,
    )

    assert ring.can_publish(5) is True
    assert ring.publish_cycle(
        frames(5), stream_generation=1, cycle_index=0, decoded_start_frame=0
    ) == (1, 6)
    assert ring.publish_cycle(
        frames(17), stream_generation=1, cycle_index=1, decoded_start_frame=5
    ) == (6, 23)
    assert ring.write_sequence == 22
    assert ring.occupancy == 22
    assert ring.can_publish(5) is False


@pytest.mark.parametrize(
    ("frame_count", "cycle_index", "decoded_start"),
    [(4, 0, 0), (17, 0, 0), (5, 1, 5), (17, 2, 5)],
)
def test_non_h3_cycle_shapes_are_rejected_before_native_publication(
    frame_count: int, cycle_index: int, decoded_start: int
) -> None:
    ring = WindowsRingProducer.bind(
        binding_payload(),
        16,
        16,
        1,
        native_producer_type=FakeNativeProducer,
    )

    with pytest.raises(H3RingError):
        ring.publish_cycle(
            frames(frame_count),
            stream_generation=1,
            cycle_index=cycle_index,
            decoded_start_frame=decoded_start,
        )
    assert ring.write_sequence == 0


def test_generation_reset_and_close_delegate_to_native_owner() -> None:
    ring = WindowsRingProducer.bind(
        binding_payload(),
        16,
        16,
        1,
        native_producer_type=FakeNativeProducer,
    )
    native = FakeNativeProducer.last_instance
    assert native is not None
    ring.publish_cycle(frames(5), stream_generation=1, cycle_index=0, decoded_start_frame=0)

    ring.set_generation(2)
    assert ring.write_sequence == 0
    assert ring.occupancy == 0
    ring.close()
    assert native.closed is True
