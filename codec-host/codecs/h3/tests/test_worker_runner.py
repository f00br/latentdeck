from __future__ import annotations

import io
import time
import uuid

from latentdeck_codec_host.protocol import (
    Bootstrap,
    encode_bootstrap,
    read_frame,
    write_frame,
)

from latentdeck_codec_h3.worker import StreamConnection, run_worker
from latentdeck_codec_h3.worker_state import H3WorkerState

SESSION_ID = "9ca8c228-04c7-4b59-909f-6fbef591a43e"
PIPE_NAME = rf"\\.\pipe\LatentDeck.Worker.{SESSION_ID}"
AUTH_TOKEN = b"a" * 32


def _command(sequence: int, name: str, payload: dict[str, object]) -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 1,
        "session_id": SESSION_ID,
        "sequence": sequence,
        "message_id": str(uuid.uuid4()),
        "sender_uptime_ns": sequence,
        "message": {
            "kind": "command",
            "body": {"name": name, "payload": payload},
        },
    }


def _configure() -> dict[str, object]:
    return {
        "selected_protocol_version": 1,
        "app_version": "0.1.0",
        "heartbeat_interval_ms": 100,
        "heartbeat_hard_timeout_ms": 300,
        "max_frame_bytes": 262_144,
        "max_inflight_decode_batches": 1,
    }


class MemoryConnector:
    def __init__(self, inbound: io.BytesIO, outbound: io.BytesIO) -> None:
        class TrackingConnection(StreamConnection):
            def __init__(self) -> None:
                super().__init__(inbound, outbound)
                self.closed = False

            def close(self) -> None:
                self.closed = True
                super().close()

        self._connection = TrackingConnection()
        self.connected_pipe: str | None = None

    @property
    def closed(self) -> bool:
        return self._connection.closed

    def connect(self, pipe_name: str) -> StreamConnection:
        self.connected_pipe = pipe_name
        return self._connection


def _frames(stream: io.BytesIO) -> list[dict[str, object]]:
    stream.seek(0)
    result: list[dict[str, object]] = []
    while stream.tell() < len(stream.getvalue()):
        result.append(read_frame(stream))
    return result


def test_bootstrap_hello_status_and_shutdown_complete_one_session() -> None:
    inbound = io.BytesIO()
    write_frame(inbound, _command(1, "session.configure", _configure()))
    write_frame(inbound, _command(2, "worker.status", {}))
    write_frame(inbound, _command(3, "worker.shutdown", {"reason": "user_request"}))
    inbound.seek(0)
    outbound = io.BytesIO()
    connector = MemoryConnector(inbound, outbound)
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, AUTH_TOKEN)))

    assert run_worker(bootstrap, connector=connector) == 0
    assert connector.connected_pipe == PIPE_NAME
    assert connector.closed

    frames = _frames(outbound)
    assert [frame["sequence"] for frame in frames] == list(range(1, len(frames) + 1))
    hello = frames[0]["message"]
    assert hello["kind"] == "event"  # type: ignore[index]
    assert hello["body"]["event"]["name"] == "worker.hello"  # type: ignore[index]
    assert hello["body"]["event"]["payload"]["auth_token"] == AUTH_TOKEN  # type: ignore[index]

    replies = [
        frame["message"]["body"]  # type: ignore[index]
        for frame in frames
        if frame["message"]["kind"] == "ack"  # type: ignore[index]
    ]
    assert [reply["ack"]["name"] for reply in replies] == [  # type: ignore[index]
        "session.configure",
        "worker.status",
        "worker.shutdown",
    ]
    assert replies[1]["ack"]["payload"]["worker_state"] == "ready"  # type: ignore[index]
    assert replies[2]["ack"]["payload"] == {"accepted": True}  # type: ignore[index]
    states = [
        frame["message"]["body"]["event"]["payload"]["status"]  # type: ignore[index]
        for frame in frames
        if frame["message"]["kind"] == "event"  # type: ignore[index]
        and frame["message"]["body"]["event"]["name"]  # type: ignore[index]
        == "worker.state_changed"
    ]
    assert [state["worker_state"] for state in states] == ["ready", "stopping"]


def test_configured_worker_emits_serialized_heartbeats_while_waiting() -> None:
    encoded = io.BytesIO()
    write_frame(encoded, _command(1, "session.configure", _configure()))
    second_frame_offset = encoded.tell()
    write_frame(encoded, _command(2, "worker.shutdown", {"reason": "application_exit"}))

    class DelayedReader(io.BytesIO):
        def __init__(self, raw: bytes) -> None:
            super().__init__(raw)
            self._delayed = False

        def read(self, size: int = -1) -> bytes:
            if self.tell() == second_frame_offset and not self._delayed:
                self._delayed = True
                time.sleep(0.16)
            return super().read(size)

    outbound = io.BytesIO()
    connector = MemoryConnector(DelayedReader(encoded.getvalue()), outbound)
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, AUTH_TOKEN)))

    assert run_worker(bootstrap, connector=connector) == 0

    frames = _frames(outbound)
    heartbeats = [
        frame["message"]["body"]["event"]["payload"]  # type: ignore[index]
        for frame in frames
        if frame["message"]["kind"] == "event"  # type: ignore[index]
        and frame["message"]["body"]["event"]["name"]  # type: ignore[index]
        == "worker.heartbeat"
    ]
    assert heartbeats
    assert heartbeats[0]["last_completed_core_sequence"] == 1
    assert [frame["sequence"] for frame in frames] == list(range(1, len(frames) + 1))


def test_transport_closes_even_when_state_teardown_fails() -> None:
    inbound = io.BytesIO()
    write_frame(inbound, _command(1, "session.configure", _configure()))
    write_frame(inbound, _command(2, "worker.shutdown", {"reason": "recovery"}))
    inbound.seek(0)
    connector = MemoryConnector(inbound, io.BytesIO())
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, AUTH_TOKEN)))

    class FailingCloseState(H3WorkerState):
        def close(self) -> None:
            super().close()
            raise RuntimeError("synthetic teardown failure")

    assert run_worker(bootstrap, connector=connector, state_factory=FailingCloseState) == 2
    assert connector.closed


def test_transport_closes_when_worker_state_cannot_start() -> None:
    connector = MemoryConnector(io.BytesIO(), io.BytesIO())
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, AUTH_TOKEN)))

    def fail_to_start() -> H3WorkerState:
        raise RuntimeError("synthetic startup failure")

    assert run_worker(bootstrap, connector=connector, state_factory=fail_to_start) == 2
    assert connector.closed


def test_invalid_heartbeat_bounds_fail_before_starting_background_work() -> None:
    invalid = _configure()
    invalid["heartbeat_interval_ms"] = 0
    inbound = io.BytesIO()
    write_frame(inbound, _command(1, "session.configure", invalid))
    inbound.seek(0)
    outbound = io.BytesIO()
    connector = MemoryConnector(inbound, outbound)
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, AUTH_TOKEN)))

    assert run_worker(bootstrap, connector=connector) == 2

    replies = [
        frame["message"]  # type: ignore[index]
        for frame in _frames(outbound)
        if frame["message"]["kind"] in {"ack", "error"}  # type: ignore[index]
    ]
    assert len(replies) == 1
    assert replies[0]["kind"] == "error"  # type: ignore[index]
    error = replies[0]["body"]["error"]  # type: ignore[index]
    assert error["code"] == "protocol.schema_invalid"
    assert error["fatal"] is True
    assert connector.closed


def test_sequence_gap_emits_specific_fatal_fault_and_closes() -> None:
    inbound = io.BytesIO()
    write_frame(inbound, _command(2, "session.configure", _configure()))
    inbound.seek(0)
    outbound = io.BytesIO()
    connector = MemoryConnector(inbound, outbound)
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, AUTH_TOKEN)))

    assert run_worker(bootstrap, connector=connector) == 2

    faults = [
        frame["message"]["body"]["event"]["payload"]  # type: ignore[index]
        for frame in _frames(outbound)
        if frame["message"]["kind"] == "event"  # type: ignore[index]
        and frame["message"]["body"]["event"]["name"] == "worker.fault"  # type: ignore[index]
    ]
    assert len(faults) == 1
    assert faults[0]["code"] == "protocol.sequence_invalid"
    assert faults[0]["fatal"] is True
    assert connector.closed
