"""Native worker-side RGB Ring transports for Protocol 1 and Protocol 2."""

from __future__ import annotations

import uuid

from ._native import (
    BINDING_ABI_VERSION,
    PROTOCOL2_BINDING_ABI_VERSION,
    RingError,
    WindowsRgbRingProducer,
    WindowsRgbRingTransportV2,
)

__version__ = "0.1.0"


class WindowsSharedRingTransport:
    """Protocol 2 `SharedRingTransport` backed by target-owned Win32 handles."""

    def __init__(self) -> None:
        self._native = WindowsRgbRingTransportV2()

    def configure(
        self,
        *,
        ring_id: uuid.UUID,
        kind: str,
        mapping_handle: int,
        ready_event_handle: int,
        consumed_event_handle: int,
        slot_count: int,
        slot_bytes: int,
    ) -> None:
        self._native.configure(
            str(ring_id),
            kind,
            mapping_handle,
            ready_event_handle,
            consumed_event_handle,
            slot_count,
            slot_bytes,
        )

    def discard_transferred_handles(
        self,
        *,
        mapping_handle: int,
        ready_event_handle: int,
        consumed_event_handle: int,
    ) -> None:
        self._native.discard_transferred_handles(
            mapping_handle,
            ready_event_handle,
            consumed_event_handle,
        )

    def release(self, ring_id: uuid.UUID) -> None:
        self._native.release(str(ring_id))

    def set_generation(self, ring_id: uuid.UUID, new_generation: int) -> None:
        self._native.set_generation(str(ring_id), new_generation)

    def publish(
        self,
        *,
        ring_id: uuid.UUID,
        session_id: uuid.UUID,
        stream_generation: int,
        sequence: int,
        batch: object,
    ) -> int:
        validate = getattr(batch, "validate", None)
        if not callable(validate):
            raise TypeError("batch must implement the decoded ABI validate method")
        validate()
        pixels = batch.pixels
        return self._native.publish(
            str(ring_id),
            str(session_id),
            stream_generation,
            sequence,
            batch.batch,
            batch.height,
            batch.width,
            bytes(pixels),
        )

    def close(self) -> None:
        self._native.close()


__all__ = [
    "BINDING_ABI_VERSION",
    "PROTOCOL2_BINDING_ABI_VERSION",
    "RingError",
    "WindowsRgbRingProducer",
    "WindowsRgbRingTransportV2",
    "WindowsSharedRingTransport",
    "__version__",
]
