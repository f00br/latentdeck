from __future__ import annotations

import sys
import threading
import uuid

import pytest
from latentdeck_rgb_ring import (
    BINDING_ABI_VERSION,
    PROTOCOL2_BINDING_ABI_VERSION,
    RingError,
    WindowsRgbRingProducer,
    WindowsSharedRingTransport,
)


def test_native_surface_is_abi_1_and_requires_the_open_factory() -> None:
    assert BINDING_ABI_VERSION == "1"
    assert PROTOCOL2_BINDING_ABI_VERSION == "2"
    with pytest.raises(TypeError):
        WindowsRgbRingProducer()


def test_open_reports_a_stable_native_error() -> None:
    with pytest.raises(RingError) as captured:
        WindowsRgbRingProducer.open(0, 0, 4096, 1, 1, 1)

    expected_code = (
        "ring_invalid_handle" if sys.platform == "win32" else "ring_unsupported_platform"
    )
    assert captured.value.code == expected_code
    assert captured.value.detail
    assert str(captured.value).startswith(f"{expected_code}: ")


def test_protocol2_transport_rejects_invalid_transferred_handles_stably() -> None:
    transport = WindowsSharedRingTransport()
    with pytest.raises(RingError) as captured:
        transport.configure(
            ring_id=uuid.uuid4(),
            kind="decoded_rgba",
            mapping_handle=0,
            ready_event_handle=0,
            consumed_event_handle=0,
            slot_count=4,
            slot_bytes=4096,
        )

    expected_code = (
        "ring_invalid_handle" if sys.platform == "win32" else "ring_unsupported_platform"
    )
    assert captured.value.code == expected_code
    transport.close()


def test_protocol2_python_adapter_passes_only_metadata_and_contiguous_pixels_to_native() -> None:
    class NativeSpy:
        def __init__(self) -> None:
            self.calls: list[tuple[object, ...]] = []

        def configure(self, *args: object) -> None:
            self.calls.append(("configure", *args))

        def discard_transferred_handles(self, *args: object) -> None:
            self.calls.append(("discard", *args))

        def release(self, *args: object) -> None:
            self.calls.append(("release", *args))

        def publish(self, *args: object) -> int:
            self.calls.append(("publish", *args))
            return 17

        def close(self) -> None:
            self.calls.append(("close",))

    class Batch:
        pixels = memoryview(bytes(range(32)))
        batch = 2
        height = 2
        width = 2

        def validate(self) -> None:
            assert self.pixels.c_contiguous

    ring_id = uuid.uuid4()
    session_id = uuid.uuid4()
    native = NativeSpy()
    transport = WindowsSharedRingTransport()
    transport._native = native
    transport.configure(
        ring_id=ring_id,
        kind="decoded_rgba",
        mapping_handle=101,
        ready_event_handle=102,
        consumed_event_handle=103,
        slot_count=4,
        slot_bytes=4096,
    )
    assert (
        transport.publish(
            ring_id=ring_id,
            session_id=session_id,
            stream_generation=5,
            sequence=9,
            batch=Batch(),
        )
        == 17
    )
    transport.release(ring_id)
    transport.discard_transferred_handles(
        mapping_handle=201,
        ready_event_handle=202,
        consumed_event_handle=203,
    )
    transport.close()

    publish = native.calls[1]
    assert publish[:8] == (
        "publish",
        str(ring_id),
        str(session_id),
        5,
        9,
        2,
        2,
        2,
    )
    assert publish[8] == bytes(range(32))


def test_protocol2_native_transport_can_close_on_heartbeat_thread() -> None:
    transport = WindowsSharedRingTransport()
    thread = threading.Thread(target=transport.close, name="latentdeck-p2-heartbeat")
    thread.start()
    thread.join(timeout=2)
    assert not thread.is_alive()
