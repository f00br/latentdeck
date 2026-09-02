import uuid
from collections.abc import Sequence
from typing import Final

BINDING_ABI_VERSION: Final[str]
PROTOCOL2_BINDING_ABI_VERSION: Final[str]
__version__: Final[str]

class RingError(Exception):
    code: str
    detail: str

class WindowsRgbRingProducer:
    @staticmethod
    def open(
        mapping_handle: int,
        frames_ready_event_handle: int,
        mapping_bytes: int,
        expected_generation: int,
        expected_width: int,
        expected_height: int,
    ) -> WindowsRgbRingProducer: ...
    @property
    def width(self) -> int: ...
    @property
    def height(self) -> int: ...
    @property
    def row_stride(self) -> int: ...
    @property
    def mapping_bytes(self) -> int: ...
    @property
    def generation(self) -> int: ...
    @property
    def write_sequence(self) -> int: ...
    @property
    def read_sequence(self) -> int: ...
    @property
    def occupancy(self) -> int: ...
    @property
    def available_capacity(self) -> int: ...
    def can_publish(self, frame_count: int) -> bool: ...
    def publish_cycle(self, frames: Sequence[bytes], timestamp_ns: int) -> tuple[int, int]: ...
    def set_generation(self, new_generation: int) -> None: ...
    def close(self) -> None: ...

class WindowsRgbRingTransportV2:
    def configure(
        self,
        ring_id: str,
        kind: str,
        mapping_handle: int,
        ready_event_handle: int,
        consumed_event_handle: int,
        slot_count: int,
        slot_bytes: int,
    ) -> None: ...
    def discard_transferred_handles(
        self,
        mapping_handle: int,
        ready_event_handle: int,
        consumed_event_handle: int,
    ) -> None: ...
    def release(self, ring_id: str) -> None: ...
    def set_generation(self, ring_id: str, new_generation: int) -> None: ...
    def publish(
        self,
        ring_id: str,
        session_id: str,
        stream_generation: int,
        sequence: int,
        batch: int,
        height: int,
        width: int,
        pixels: bytes,
    ) -> int: ...
    def close(self) -> None: ...

class WindowsSharedRingTransport:
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
    ) -> None: ...
    def discard_transferred_handles(
        self,
        *,
        mapping_handle: int,
        ready_event_handle: int,
        consumed_event_handle: int,
    ) -> None: ...
    def release(self, ring_id: uuid.UUID) -> None: ...
    def set_generation(self, ring_id: uuid.UUID, new_generation: int) -> None: ...
    def publish(
        self,
        *,
        ring_id: uuid.UUID,
        session_id: uuid.UUID,
        stream_generation: int,
        sequence: int,
        batch: object,
    ) -> int: ...
    def close(self) -> None: ...
