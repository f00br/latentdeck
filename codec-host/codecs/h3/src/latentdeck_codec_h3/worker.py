"""Isolated H3 worker process entry point for Worker Protocol 1."""

from __future__ import annotations

import os
import platform
import sys
import tempfile
import threading
import time
import uuid
from collections.abc import Callable, Mapping
from contextlib import suppress
from pathlib import Path
from typing import BinaryIO, Protocol

from latentdeck_codec_host.protocol import (
    EnvelopeWriter,
    ProtocolError,
    SequenceValidator,
    read_bootstrap,
    read_frame,
)

from .worker_state import ADAPTER_ID, WORKER_VERSION, H3WorkerState, WorkerCommandError

EXIT_OK = 0
EXIT_WORKER_ERROR = 2
MAX_COMMANDS_PER_SESSION = 65_536
DIAGNOSTIC_SCHEMA_VERSION = 1
MAX_DIAGNOSTIC_BYTES = 1024 * 1024


def _record_diagnostic(
    event: str,
    *,
    code: str | None = None,
    error: BaseException | None = None,
) -> None:
    """Best-effort bounded path-free worker lifecycle evidence."""

    try:
        import json

        directory = Path(tempfile.gettempdir()) / "LatentDeck" / "worker-diagnostics"
        directory.mkdir(parents=True, exist_ok=True)
        path = directory / f"worker-{os.getpid()}.jsonl"
        if path.exists() and path.stat().st_size >= MAX_DIAGNOSTIC_BYTES:
            return
        record: dict[str, object] = {
            "schema_version": DIAGNOSTIC_SCHEMA_VERSION,
            "timestamp_ns": time.time_ns(),
            "pid": os.getpid(),
            "event": event,
        }
        if code is not None:
            record["code"] = code
        if error is not None:
            record["error_type"] = type(error).__name__
            if isinstance(error, (ProtocolError, WorkerCommandError)):
                detail = str(error).replace("\0", "")[:256]
                if detail:
                    record["detail"] = detail
            if isinstance(error, WorkerCommandError):
                diagnostic_code = error.diagnostic_code
                diagnostic_detail = error.diagnostic_detail
                if diagnostic_code is not None:
                    safe_code = diagnostic_code.replace("\0", "")[:64]
                    if safe_code and all(
                        character.isascii()
                        and (character.isalnum() or character in "._-")
                        for character in safe_code
                    ):
                        record["cause_code"] = safe_code
                if diagnostic_detail is not None:
                    safe_detail = diagnostic_detail.replace("\0", "")[:256]
                    if safe_detail:
                        record["cause_detail"] = safe_detail
        with path.open("a", encoding="utf-8", newline="\n") as stream:
            stream.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")
    except Exception:
        pass


class Connection(Protocol):
    """One connected duplex control channel."""

    reader: BinaryIO
    writer: BinaryIO

    def close(self) -> None: ...


class Connector(Protocol):
    """Connect to the supervisor-created local control transport."""

    def connect(self, pipe_name: str) -> Connection: ...


class StreamConnection:
    """A duplex connection composed from binary input and output streams."""

    def __init__(
        self,
        reader: BinaryIO,
        writer: BinaryIO,
        *,
        owns_streams: bool = False,
    ) -> None:
        self.reader = reader
        self.writer = writer
        self._owns_streams = owns_streams
        self._closed = False

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        if not self._owns_streams:
            return
        if self.reader is self.writer:
            self.reader.close()
            return
        try:
            self.reader.close()
        finally:
            self.writer.close()


class WindowsNamedPipeConnector:
    """Open the exact local Windows Named Pipe supplied through stdin."""

    _PREFIX = "\\\\.\\pipe\\"

    def connect(self, pipe_name: str) -> StreamConnection:
        if os.name != "nt":
            raise OSError("Windows Named Pipes are unavailable on this platform")
        if not pipe_name.startswith(self._PREFIX):
            raise OSError("worker control transport is not a local Named Pipe")
        stream = open(pipe_name, "r+b", buffering=0)  # noqa: SIM115
        return StreamConnection(stream, stream, owns_streams=True)


class _SessionProgress:
    def __init__(self) -> None:
        self._last_completed = 0
        self._lock = threading.Lock()

    def get(self) -> int:
        with self._lock:
            return self._last_completed

    def complete(self, sequence: int) -> None:
        with self._lock:
            self._last_completed = sequence


class _HeartbeatPump:
    def __init__(
        self,
        *,
        writer: EnvelopeWriter,
        writer_lock: threading.RLock,
        state: H3WorkerState,
        progress: _SessionProgress,
        interval_ms: int,
        on_failure: Callable[[], None],
    ) -> None:
        self._writer = writer
        self._writer_lock = writer_lock
        self._state = state
        self._progress = progress
        self._interval_seconds = interval_ms / 1000
        self._on_failure = on_failure
        self._stop = threading.Event()
        self._thread = threading.Thread(
            target=self._run,
            name="latentdeck-h3-heartbeat",
            daemon=True,
        )

    def start(self) -> None:
        self._thread.start()

    def request_stop(self) -> None:
        self._stop.set()

    def join(self) -> bool:
        self._thread.join(timeout=1)
        return not self._thread.is_alive()

    def _run(self) -> None:
        while not self._stop.wait(self._interval_seconds):
            try:
                with self._writer_lock:
                    self._writer.event(
                        "worker.heartbeat",
                        self._state.heartbeat(self._progress.get()),
                    )
            except Exception:
                with suppress(Exception):
                    self._on_failure()
                return


def _hello_payload(auth_token: bytes) -> dict[str, object]:
    return {
        "auth_token": auth_token,
        "worker_version": WORKER_VERSION,
        "protocol_min": 1,
        "protocol_max": 1,
        "pid": os.getpid(),
        "os": platform.system().lower(),
        "arch": platform.machine().lower(),
        "python_version": platform.python_version(),
        "available_adapters": [ADAPTER_ID],
    }


def _fault_payload(state: H3WorkerState, code: str, message: str) -> dict[str, object]:
    return {
        "code": code,
        "message": message,
        "retryable": False,
        "fatal": True,
        "worker_state": state.status()["worker_state"],
        "diagnostic_id": str(uuid.uuid4()),
        "details": [],
    }


def _u32(value: object) -> int | None:
    if isinstance(value, bool) or not isinstance(value, int):
        return None
    if not 0 <= value <= 0xFFFF_FFFF:
        return None
    return value


def _validate_session_configure(payload: Mapping[str, object]) -> None:
    selected = _u32(payload.get("selected_protocol_version"))
    interval = _u32(payload.get("heartbeat_interval_ms"))
    timeout = _u32(payload.get("heartbeat_hard_timeout_ms"))
    max_frame = _u32(payload.get("max_frame_bytes"))
    max_inflight = _u32(payload.get("max_inflight_decode_batches"))
    app_version = payload.get("app_version")
    valid_app_version = (
        isinstance(app_version, str)
        and 1 <= len(app_version.encode()) <= 4096
        and "\0" not in app_version
    )
    valid = (
        selected == 1
        and interval is not None
        and 100 <= interval <= 10_000
        and timeout is not None
        and timeout >= interval * 3
        and max_frame == 262_144
        and max_inflight == 1
        and valid_app_version
    )
    if not valid:
        raise WorkerCommandError(
            "protocol.schema_invalid",
            "session configuration is outside Worker Protocol 1 bounds",
            fatal=True,
        )


def run_worker(
    stdin: BinaryIO,
    *,
    connector: Connector | None = None,
    state_factory: Callable[[], H3WorkerState] = H3WorkerState,
) -> int:
    """Run one authenticated worker session until shutdown or fatal failure."""

    try:
        bootstrap = read_bootstrap(stdin)
        connection = (connector or WindowsNamedPipeConnector()).connect(bootstrap.pipe_name)
    except (OSError, ProtocolError) as error:
        _record_diagnostic("worker.bootstrap_failed", error=error)
        return EXIT_WORKER_ERROR

    try:
        state = state_factory()
    except Exception as error:
        _record_diagnostic("worker.state_initialization_failed", error=error)
        with suppress(Exception):
            connection.close()
        return EXIT_WORKER_ERROR
    _record_diagnostic("worker.session_started")
    writer = EnvelopeWriter(connection.writer, bootstrap.session_id)
    writer_lock = threading.RLock()
    validator = SequenceValidator(bootstrap.session_id)
    progress = _SessionProgress()
    heartbeat: _HeartbeatPump | None = None
    expected_core_sequence = 1
    exit_code = EXIT_WORKER_ERROR
    try:
        with writer_lock:
            writer.event("worker.hello", _hello_payload(bootstrap.auth_token))
        for _ in range(MAX_COMMANDS_PER_SESSION):
            try:
                envelope = read_frame(connection.reader)
            except ProtocolError as error:
                _record_diagnostic(
                    "worker.control_read_failed",
                    code="protocol.schema_invalid",
                    error=error,
                )
                with writer_lock:
                    writer.event(
                        "worker.fault",
                        _fault_payload(
                            state,
                            "protocol.schema_invalid",
                            "worker control stream is invalid",
                        ),
                    )
                break
            raw_sequence = envelope.get("sequence")
            if (
                isinstance(raw_sequence, bool)
                or not isinstance(raw_sequence, int)
                or raw_sequence != expected_core_sequence
            ):
                with writer_lock:
                    writer.event(
                        "worker.fault",
                        _fault_payload(
                            state,
                            "protocol.sequence_invalid",
                            "worker command sequence is not contiguous",
                        ),
                    )
                break
            try:
                command = validator.validate_command(envelope)
            except ProtocolError:
                with writer_lock:
                    writer.event(
                        "worker.fault",
                        _fault_payload(
                            state,
                            "protocol.schema_invalid",
                            "worker command envelope is invalid",
                        ),
                    )
                break
            expected_core_sequence += 1

            name = str(command["name"])
            payload = command["payload"]
            message_id = str(command["message_id"])
            core_sequence = int(envelope["sequence"])
            before = state.status()
            try:
                if name == "session.configure":
                    _validate_session_configure(payload)  # type: ignore[arg-type]
                result = state.handle(name, payload)  # type: ignore[arg-type]
            except WorkerCommandError as error:
                _record_diagnostic(
                    "worker.command_rejected",
                    code=error.code,
                    error=error,
                )
                with writer_lock:
                    writer.error(
                        message_id,
                        name,
                        code=error.code,
                        message=error.message,
                        retryable=error.retryable,
                        fatal=error.fatal,
                        worker_state=str(state.status()["worker_state"]),
                    )
                    progress.complete(core_sequence)
                if error.fatal:
                    break
                continue
            except Exception as error:
                _record_diagnostic(
                    "worker.command_failed",
                    code="worker.internal",
                    error=error,
                )
                with writer_lock:
                    writer.error(
                        message_id,
                        name,
                        code="worker.internal",
                        message="worker command failed",
                        retryable=False,
                        fatal=True,
                        worker_state=str(state.status()["worker_state"]),
                    )
                    progress.complete(core_sequence)
                break

            after = state.status()
            with writer_lock:
                writer.ack(message_id, name, result)
                progress.complete(core_sequence)
                if after != before:
                    writer.event(
                        "worker.state_changed",
                        {"status": after, "reason": f"{name} completed"},
                        caused_by=message_id,
                    )
            if name == "session.configure" and heartbeat is None:
                heartbeat = _HeartbeatPump(
                    writer=writer,
                    writer_lock=writer_lock,
                    state=state,
                    progress=progress,
                    interval_ms=int(result["heartbeat_interval_ms"]),
                    on_failure=connection.close,
                )
                heartbeat.start()
                _record_diagnostic("worker.session_configured")
            if state.shutdown_requested:
                exit_code = EXIT_OK
                break
        else:
            with writer_lock:
                writer.event(
                    "worker.fault",
                    _fault_payload(
                        state,
                        "protocol.sequence_invalid",
                        "worker session command limit was reached",
                    ),
                )
    except (OSError, ProtocolError) as error:
        _record_diagnostic("worker.control_write_failed", error=error)
        exit_code = EXIT_WORKER_ERROR
    finally:
        teardown_failed = False
        if heartbeat is not None:
            heartbeat.request_stop()
        try:
            connection.close()
        except Exception:
            teardown_failed = True
        if heartbeat is not None and not heartbeat.join():
            teardown_failed = True
        try:
            state.close()
        except Exception:
            teardown_failed = True
        if teardown_failed:
            _record_diagnostic("worker.teardown_failed")
            exit_code = EXIT_WORKER_ERROR
        _record_diagnostic(
            "worker.session_stopped",
            code="worker.exit_ok" if exit_code == EXIT_OK else "worker.exit_error",
        )
    return exit_code


def main() -> int:
    """Run the H3 worker using a secret bootstrap from inherited stdin."""

    return run_worker(sys.stdin.buffer)


if __name__ == "__main__":
    raise SystemExit(main())


__all__ = [
    "Connection",
    "Connector",
    "StreamConnection",
    "WindowsNamedPipeConnector",
    "run_worker",
]
