from __future__ import annotations

from collections import deque
from dataclasses import dataclass
from typing import Any

import pytest

from latentdeck_codec_h3.presentation import H3PresentationCadence


@dataclass(frozen=True)
class Frame:
    epoch: int
    slot: Any
    raw_index: int


class FakeStreamingDecoder:
    """Small behavioral stand-in for upstream StreamingTAEHV."""

    def __init__(self) -> None:
        self.pending: deque[Frame] = deque()
        self.epoch = 0
        self.is_startup = True
        self.reset_calls = 0
        self.slots_seen: list[Any] = []

    def decode(self, slot: Any | None = None) -> Frame | None:
        if slot is not None:
            if self.pending:
                raise AssertionError("caller fed a slot before draining pending frames")
            self.slots_seen.append(slot)
            generated = [Frame(self.epoch, slot, raw_index) for raw_index in range(4)]
            if self.is_startup:
                generated = generated[3:]
                self.is_startup = False
            self.pending.extend(generated)
        return self.pending.popleft() if self.pending else None

    def reset(self) -> None:
        self.pending.clear()
        self.epoch += 1
        self.is_startup = True
        self.reset_calls += 1


def collect_slot(adapter: H3PresentationCadence[Any, Frame], slot: Any) -> list[Frame]:
    frames = [adapter.feed_slot(slot)]
    while (frame := adapter.pop_pending()) is not None:
        frames.append(frame)
    return frames


def test_suppresses_three_outputs_at_later_five_slot_boundaries_without_reset() -> None:
    decoder = FakeStreamingDecoder()
    adapter = H3PresentationCadence(decoder)

    first_group = [frame for slot in range(5) for frame in collect_slot(adapter, slot)]
    next_group_start = collect_slot(adapter, 5)

    assert len(first_group) == 17
    assert next_group_start == [Frame(epoch=0, slot=5, raw_index=3)]
    assert decoder.reset_calls == 0
    assert decoder.slots_seen == list(range(6))


@pytest.mark.parametrize("operation", ["reset", "restart"])
def test_reset_operations_clear_counters_causal_memory_and_pending_outputs(
    operation: str,
) -> None:
    decoder = FakeStreamingDecoder()
    adapter = H3PresentationCadence(decoder)

    collect_slot(adapter, "startup")
    assert adapter.feed_slot("before-reset") == Frame(0, "before-reset", 0)

    getattr(adapter, operation)()

    assert adapter.pop_pending() is None
    assert collect_slot(adapter, "after-reset") == [Frame(1, "after-reset", 3)]
    assert decoder.reset_calls == 1


@pytest.mark.parametrize(("latent_slots", "expected_frames"), [(32, 107), (72, 243)])
def test_valid_h3_full_clips_emit_the_profile_frame_count(
    latent_slots: int,
    expected_frames: int,
) -> None:
    adapter = H3PresentationCadence(FakeStreamingDecoder())

    frames = [frame for slot in range(latent_slots) for frame in collect_slot(adapter, slot)]

    assert len(frames) == expected_frames
