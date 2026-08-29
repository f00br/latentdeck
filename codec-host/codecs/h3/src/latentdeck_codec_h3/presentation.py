"""MiniMax H3 presentation cadence over a causal streaming decoder."""

from __future__ import annotations

from typing import Protocol


class StreamingDecoder[SlotT, FrameT](Protocol):
    """Decoder surface required by :class:`H3PresentationCadence`."""

    def decode(self, slot: SlotT | None = None) -> FrameT | None:
        """Feed one slot or drain one pending raw output."""

    def reset(self) -> None:
        """Clear all causal memory and pending raw outputs."""


class H3CadenceError(RuntimeError):
    """The streaming decoder violated the H3 slot/output contract."""


class H3PresentationCadence[SlotT, FrameT]:
    """Expose H3's usable frames without resetting causal decoder memory.

    The wrapped decoder must already suppress the three causal warm-up outputs
    at stream startup. Every later group of five H3 slots starts with three
    additional non-presentation outputs, which this adapter consumes while
    preserving the decoder's causal state.
    """

    SLOTS_PER_GROUP = 5
    RAW_OUTPUTS_PER_SLOT = 4
    SUPPRESSED_OUTPUTS_PER_GROUP = 3

    def __init__(self, decoder: StreamingDecoder[SlotT, FrameT]) -> None:
        self._decoder = decoder
        self._slots_seen = 0
        self._pending_outputs = 0

    def feed_slot(self, slot: SlotT) -> FrameT:
        """Feed one slot and return its first usable presentation frame."""

        if self._pending_outputs:
            raise H3CadenceError("pending outputs must be drained before feeding another slot")

        frame = self._required_decode(slot)
        at_later_group_boundary = (
            self._slots_seen > 0 and self._slots_seen % self.SLOTS_PER_GROUP == 0
        )
        if at_later_group_boundary:
            for _ in range(self.SUPPRESSED_OUTPUTS_PER_GROUP):
                frame = self._required_decode()
        elif self._slots_seen > 0:
            self._pending_outputs = self.RAW_OUTPUTS_PER_SLOT - 1

        self._slots_seen += 1
        return frame

    def pop_pending(self) -> FrameT | None:
        """Return the next usable output for the current slot, if one remains."""

        if self._pending_outputs == 0:
            return None
        frame = self._required_decode()
        self._pending_outputs -= 1
        return frame

    def reset(self) -> None:
        """Clear cadence counters and all causal/pending decoder state."""

        self._decoder.reset()
        self._slots_seen = 0
        self._pending_outputs = 0

    def restart(self) -> None:
        """Start a new H3 stream from its causal boundary."""

        self.reset()

    def _required_decode(self, slot: SlotT | None = None) -> FrameT:
        frame = self._decoder.decode(slot)
        if frame is None:
            raise H3CadenceError("decoder produced no frame for a complete H3 slot")
        return frame


__all__ = ["H3CadenceError", "H3PresentationCadence", "StreamingDecoder"]
