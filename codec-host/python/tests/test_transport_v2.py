from __future__ import annotations

import io
import struct
import threading
import time
import uuid
from dataclasses import dataclass

import msgpack
import pytest
from latentdeck_codec_host.runtime_v2 import (
    Protocol2Worker,
    StreamConnection,
    read_protocol2_bootstrap,
    run_protocol2_service,
)
from latentdeck_codec_sdk import decode_messagepack, encode_messagepack
from latentdeck_codec_sdk.protocol import MAX_FRAME_BYTES, ProtocolError

SESSION_ID = uuid.UUID("9ca8c228-04c7-4b59-909f-6fbef591a43e")
TOKEN = "ab" * 32


def _framed(value: object) -> bytes:
    payload = msgpack.packb(value, use_bin_type=True, strict_types=True)
    return struct.pack("<I", len(payload)) + payload


def _bootstrap(**updates: object) -> bytes:
    value: dict[str, object] = {
        "bootstrap_version": 2,
        "protocol_version": 2,
        "session_id": str(SESSION_ID),
        "pipe_name": rf"\\.\pipe\latentdeck-worker-{SESSION_ID}",
        "auth_token": TOKEN,
    }
    value.update(updates)
    return _framed(value)


def _command(sequence: int, name: str, payload: dict[str, object]) -> bytes:
    envelope = {
        "protocol": "latentdeck.worker",
        "protocol_version": 2,
        "session_id": str(SESSION_ID),
        "sequence": sequence,
        "message_id": str(uuid.UUID(int=sequence + 100)),
        "sender_uptime_ns": sequence,
        "message": {"kind": "command", "body": {"name": name, "payload": payload}},
    }
    encoded = encode_messagepack(envelope)
    return struct.pack("<I", len(encoded)) + encoded


def _configure(sequence: int = 1) -> bytes:
    return _command(
        sequence,
        "session.configure",
        {
            "selected_protocol_version": 2,
            "app_version": "0.2.0",
            "heartbeat_interval_ms": 250,
            "heartbeat_hard_timeout_ms": 750,
            "max_frame_bytes": MAX_FRAME_BYTES,
            "max_inflight_batches": 1,
            "requested_capabilities": ["player"],
        },
    )


def _shutdown(sequence: int = 2) -> bytes:
    return _command(sequence, "session.shutdown", {"reason": "host_exit"})


def _frames(encoded: bytes) -> list[dict[str, object]]:
    stream = io.BytesIO(encoded)
    values: list[dict[str, object]] = []
    while prefix := stream.read(4):
        assert len(prefix) == 4
        size = struct.unpack("<I", prefix)[0]
        values.append(decode_messagepack(stream.read(size)))
    return values


class AccessFactory:
    def open(self, **_values: object) -> object:
        raise AssertionError("source.open is outside this transport test")

    def close(self, _access: object) -> None:
        return None


class RingTransport:
    def configure(self, **_values: object) -> None:
        raise AssertionError("ring.configure is outside this transport test")

    def discard_transferred_handles(self, **_values: object) -> None:
        return None

    def release(self, _ring_id: uuid.UUID) -> None:
        return None

    def set_generation(self, ring_id: uuid.UUID, new_generation: int) -> None:
        del ring_id, new_generation
        return None

    def publish(self, **_values: object) -> int:
        raise AssertionError("publish is outside this transport test")


def _worker(session_id: uuid.UUID) -> Protocol2Worker:
    return Protocol2Worker(
        session_id=session_id,
        codec_entrypoints=(),
        deck_entrypoints=(),
        cartridge_access_factory=AccessFactory(),
        ring_transport=RingTransport(),
    )


@dataclass
class Connector:
    reader: object
    writer: io.BytesIO

    def connect(self, pipe_name: str) -> StreamConnection:
        assert pipe_name == rf"\\.\pipe\latentdeck-worker-{SESSION_ID}"
        return StreamConnection(self.reader, self.writer)


class BlockingReader:
    def __init__(self, initial: bytes = b"") -> None:
        self._buffer = bytearray(initial)
        self._condition = threading.Condition()

    def feed(self, value: bytes) -> None:
        with self._condition:
            self._buffer.extend(value)
            self._condition.notify_all()

    def read(self, size: int) -> bytes:
        with self._condition:
            while len(self._buffer) < size:
                self._condition.wait(timeout=1)
            value = bytes(self._buffer[:size])
            del self._buffer[:size]
            return value


def test_bootstrap_reader_is_closed_bounded_and_returns_clearable_secret() -> None:
    bootstrap = read_protocol2_bootstrap(io.BytesIO(_bootstrap()))
    assert bootstrap.session_id == SESSION_ID
    assert bootstrap.auth_token == bytearray.fromhex(TOKEN)
    bootstrap.clear_secret()
    assert bootstrap.auth_token == bytearray(32)

    for invalid in ("AB" * 32, "ab" * 31, b"\xab" * 32):
        with pytest.raises(ProtocolError, match="auth token"):
            read_protocol2_bootstrap(io.BytesIO(_bootstrap(auth_token=invalid)))

    with pytest.raises(ProtocolError):
        read_protocol2_bootstrap(io.BytesIO(_bootstrap(hidden_fallback=True)))


def test_service_emits_authenticated_hello_first_then_ordered_acks_and_exits() -> None:
    writer = io.BytesIO()
    connector = Connector(io.BytesIO(_configure() + _shutdown()), writer)
    exit_code = run_protocol2_service(
        io.BytesIO(_bootstrap()), worker_factory=_worker, connector=connector
    )
    assert exit_code == 0

    outgoing = _frames(writer.getvalue())
    assert [frame["sequence"] for frame in outgoing] == [1, 2, 3]
    hello = outgoing[0]["message"]
    assert hello["kind"] == "event"
    assert hello["body"]["caused_by"] is None
    hello_event = hello["body"]["event"]
    assert hello_event["name"] == "worker.hello"
    hello_payload = hello_event["payload"]
    assert hello_payload == {
        "auth_token": TOKEN,
        "worker_pid": hello_payload["worker_pid"],
        "worker_identity": "org.latentdeck.codec-host",
        "runtime_identity": "cpython-3.13",
        "protocol_min": 2,
        "protocol_max": 2,
    }
    assert hello_payload["worker_pid"] > 0
    assert outgoing[1]["message"]["body"]["ack"]["name"] == "session.configure"
    assert outgoing[2]["message"]["body"]["ack"]["name"] == "session.shutdown"
    assert writer.getvalue().count(TOKEN.encode()) == 1
    assert b"tensor_bytes" not in writer.getvalue()
    assert b"rgba_bytes" not in writer.getvalue()


def test_configure_starts_ordered_heartbeats_and_shutdown_stops_them() -> None:
    reader = BlockingReader(_configure())
    writer = io.BytesIO()
    connector = Connector(reader, writer)
    result: list[int] = []
    service = threading.Thread(
        target=lambda: result.append(
            run_protocol2_service(
                io.BytesIO(_bootstrap()), worker_factory=_worker, connector=connector
            )
        )
    )
    service.start()

    deadline = time.monotonic() + 2
    while len(_frames(writer.getvalue())) < 3 and time.monotonic() < deadline:
        time.sleep(0.02)
    observed = _frames(writer.getvalue())
    assert observed[0]["message"]["body"]["event"]["name"] == "worker.hello"
    assert observed[1]["message"]["body"]["ack"]["name"] == "session.configure"
    assert observed[2]["message"]["body"]["event"]["name"] == "worker.heartbeat"

    reader.feed(_shutdown())
    service.join(timeout=2)
    assert result == [0]
    completed = _frames(writer.getvalue())
    assert [frame["sequence"] for frame in completed] == list(range(1, len(completed) + 1))
    assert completed[-1]["message"]["body"]["ack"]["name"] == "session.shutdown"


@pytest.mark.parametrize(
    "commands",
    [
        _configure(sequence=2),
        struct.pack("<I", MAX_FRAME_BYTES + 1),
    ],
)
def test_sequence_gap_and_oversized_frame_terminate_without_a_protocol_fallback(
    commands: bytes,
) -> None:
    writer = io.BytesIO()
    connector = Connector(io.BytesIO(commands), writer)
    assert (
        run_protocol2_service(io.BytesIO(_bootstrap()), worker_factory=_worker, connector=connector)
        == 2
    )
    outgoing = _frames(writer.getvalue())
    assert len(outgoing) == 1
    assert outgoing[0]["message"]["body"]["event"]["name"] == "worker.hello"
