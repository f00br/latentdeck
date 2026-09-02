"""Generic, codec-neutral Worker Protocol 2 runtime.

The wire protocol carries only bounded control data. Validated cartridge handles,
latent tensors, and decoded RGBA batches are deliberately injected or delivered
out of band by the native host.
"""

from __future__ import annotations

import hashlib
import importlib
import math
import os
import struct
import sys
import threading
import time
import uuid
from collections.abc import Callable, Mapping
from contextlib import suppress
from dataclasses import dataclass, field
from pathlib import Path
from typing import BinaryIO, Protocol, runtime_checkable

import msgpack
from latentdeck_codec_sdk import (
    PROTOCOL,
    PROTOCOL_VERSION,
    Capability,
    CapturePayload,
    CaptureRequest,
    CaptureState,
    CartridgeAccess,
    CodecAdapter,
    CodecDescriptor,
    CodecLoadRequest,
    CodecSdkError,
    CodecState,
    DeckState,
    DecodedBatch,
    ErrorCode,
    ExternalAsset,
    PlayerState,
    ProfileInspection,
    ProfileKey,
    ProfileReceipt,
    ProtocolError,
    RawImportAdapter,
    RawImportArtifact,
    RawImportPreflight,
    RawImportPreflightRequest,
    RawImportStageRequest,
    SessionState,
    SourceHandle,
    decode_messagepack,
    encode_messagepack,
    validate_codec_v2_descriptor,
    validate_envelope,
    validate_profile_receipt,
)
from latentdeck_deck_sdk import (
    DeckContractError,
    DeckOperatorContext,
    DeckOperatorResult,
    RoleBinding,
    process_sources_checked,
)

MAX_ERROR_MESSAGE_BYTES = 4_096
MAX_ERROR_DETAILS = 16
MAX_SOURCES = 16
MAX_CONTROLS = 64
MAX_CAPTURE_EVENTS = 32
MAX_ENTRYPOINT_BYTES = 512
MAX_PATH_BYTES = 32_768
MAX_RESULT_RECORDS = 256
MAX_BOOTSTRAP_BYTES = 4_096
MAX_TRANSPORT_MESSAGES = 65_536
PIPE_NAME_PREFIX = r"\\.\pipe\latentdeck-worker-"


@dataclass(slots=True)
class Protocol2Bootstrap:
    """Single-use P2 bootstrap with an explicitly clearable secret buffer."""

    session_id: uuid.UUID
    pipe_name: str
    auth_token: bytearray

    def clear_secret(self) -> None:
        self.auth_token[:] = b"\x00" * len(self.auth_token)


@dataclass(frozen=True, slots=True)
class StreamConnection:
    """Injectable full-duplex byte stream used by the P2 service loop."""

    reader: BinaryIO
    writer: BinaryIO
    close_callback: Callable[[], None] | None = None

    def close(self) -> None:
        if self.close_callback is not None:
            self.close_callback()


@runtime_checkable
class WorkerPipeConnector(Protocol):
    def connect(self, pipe_name: str) -> StreamConnection: ...


class WindowsNamedPipeConnector:
    """Open only a supervisor-created local LatentDeck worker pipe."""

    def connect(self, pipe_name: str) -> StreamConnection:
        if os.name != "nt":
            raise OSError("Protocol 2 Named Pipes are available only on Windows")
        if not pipe_name.startswith(PIPE_NAME_PREFIX) or len(pipe_name) > 512:
            raise OSError("Protocol 2 pipe name is invalid")
        stream = open(pipe_name, "r+b", buffering=0)  # noqa: SIM115
        return StreamConnection(stream, stream, stream.close)


def read_protocol2_bootstrap(stream: BinaryIO) -> Protocol2Bootstrap:
    """Read one bounded, closed P2 bootstrap frame from inherited stdin."""

    prefix = _read_exact_transport(stream, 4, "bootstrap length")
    byte_length = struct.unpack("<I", prefix)[0]
    if not 1 <= byte_length <= MAX_BOOTSTRAP_BYTES:
        raise ProtocolError("bootstrap frame length is outside its bound")
    encoded = _read_exact_transport(stream, byte_length, "bootstrap payload")
    try:
        value = msgpack.unpackb(
            encoded,
            raw=False,
            strict_map_key=True,
            object_pairs_hook=_unique_transport_map,
            max_str_len=512,
            max_bin_len=64,
            max_array_len=0,
            max_map_len=8,
            max_ext_len=0,
        )
    except (msgpack.ExtraData, msgpack.FormatError, msgpack.StackError, ValueError) as error:
        raise ProtocolError("bootstrap MessagePack is invalid") from error
    if not isinstance(value, dict):
        raise ProtocolError("bootstrap must be a map")
    expected = {
        "bootstrap_version",
        "protocol_version",
        "session_id",
        "pipe_name",
        "auth_token",
    }
    if set(value) != expected:
        raise ProtocolError("bootstrap fields do not match the closed contract")
    if value["bootstrap_version"] != 2 or value["protocol_version"] != PROTOCOL_VERSION:
        raise ProtocolError("bootstrap version is unsupported")
    session_text = value["session_id"]
    if not isinstance(session_text, str):
        raise ProtocolError("bootstrap session_id must be canonical UUID text")
    try:
        session_id = uuid.UUID(session_text)
    except ValueError as error:
        raise ProtocolError("bootstrap session_id must be canonical UUID text") from error
    if session_id.int == 0 or str(session_id) != session_text:
        raise ProtocolError("bootstrap session_id must be canonical non-nil UUID text")
    pipe_name = value["pipe_name"]
    if not isinstance(pipe_name, str) or not pipe_name or len(pipe_name.encode()) > 512:
        raise ProtocolError("bootstrap pipe name is invalid")
    token = value["auth_token"]
    if (
        not isinstance(token, str)
        or len(token) != 64
        or any(character not in "0123456789abcdef" for character in token)
    ):
        raise ProtocolError("bootstrap auth token must be 64 lowercase hex characters")
    return Protocol2Bootstrap(session_id, pipe_name, bytearray.fromhex(token))


class _CoreCommandStreamValidator:
    def __init__(self, session_id: uuid.UUID) -> None:
        self._session_id = str(session_id)
        self._next_sequence = 1
        self._message_ids: set[str] = set()

    def validate(self, envelope: Mapping[str, object]) -> dict[str, object]:
        validated = validate_envelope(envelope)
        if validated["session_id"] != self._session_id:
            raise ProtocolError("command belongs to another session")
        if validated["sequence"] != self._next_sequence:
            raise ProtocolError("command sequence is not contiguous")
        message_id = str(validated["message_id"])
        if message_id in self._message_ids:
            raise ProtocolError("command message_id was already used")
        if len(self._message_ids) >= MAX_TRANSPORT_MESSAGES:
            raise ProtocolError("command message budget is exhausted")
        message = validated["message"]
        if not isinstance(message, Mapping) or message["kind"] != "command":
            raise ProtocolError("worker transport accepts commands only")
        self._message_ids.add(message_id)
        self._next_sequence += 1
        return validated


class _TransportWriter:
    def __init__(self, stream: BinaryIO, session_id: uuid.UUID) -> None:
        self._stream = stream
        self._session_id = str(session_id)
        self._sequence = 1
        self._started_ns = time.monotonic_ns()
        self._lock = threading.Lock()

    def send(self, message: Mapping[str, object], *, sensitive: bool = False) -> None:
        with self._lock:
            envelope = {
                "protocol": PROTOCOL,
                "protocol_version": PROTOCOL_VERSION,
                "session_id": self._session_id,
                "sequence": self._sequence,
                "message_id": str(uuid.uuid4()),
                "sender_uptime_ns": time.monotonic_ns() - self._started_ns,
                "message": dict(message),
            }
            payload = encode_messagepack(envelope)
            framed = bytearray(struct.pack("<I", len(payload)))
            framed.extend(payload)
            try:
                _write_all_transport(self._stream, framed)
                self._stream.flush()
            finally:
                if sensitive:
                    framed[:] = b"\x00" * len(framed)
            self._sequence += 1

    def event(self, name: str, payload: Mapping[str, object], *, sensitive: bool = False) -> None:
        self.send(
            {
                "kind": "event",
                "body": {
                    "caused_by": None,
                    "event": {"name": name, "payload": dict(payload)},
                },
            },
            sensitive=sensitive,
        )


class _HeartbeatPump:
    def __init__(self, worker: Protocol2Worker, writer: _TransportWriter, interval_ms: int) -> None:
        self._worker = worker
        self._writer = writer
        self._interval = interval_ms / 1000
        self._stop = threading.Event()
        self._thread = threading.Thread(
            target=self._run, name="latentdeck-p2-heartbeat", daemon=True
        )
        self.error: Exception | None = None

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._thread.join(timeout=max(1.0, self._interval * 2))

    def _run(self) -> None:
        while not self._stop.wait(self._interval):
            try:
                self._writer.event("worker.heartbeat", self._worker.status())
            except Exception as error:  # The main loop converts this into process failure.
                self.error = error
                self._stop.set()
                return


def run_protocol2_service(
    stdin: BinaryIO,
    *,
    worker_factory: Callable[[uuid.UUID], Protocol2Worker],
    connector: WorkerPipeConnector | None = None,
    worker_identity: str = "org.latentdeck.codec-host",
    runtime_identity: str | None = None,
) -> int:
    """Run one authenticated P2-only worker service until typed shutdown."""

    bootstrap: Protocol2Bootstrap | None = None
    connection: StreamConnection | None = None
    heartbeat: _HeartbeatPump | None = None
    worker: Protocol2Worker | None = None
    clean_shutdown = False
    try:
        bootstrap = read_protocol2_bootstrap(stdin)
        worker = worker_factory(bootstrap.session_id)
        if worker.session_id != bootstrap.session_id:
            raise ProtocolError("worker factory returned another session")
        connection = (connector or WindowsNamedPipeConnector()).connect(bootstrap.pipe_name)
        writer = _TransportWriter(connection.writer, bootstrap.session_id)
        validator = _CoreCommandStreamValidator(bootstrap.session_id)

        token_text = bootstrap.auth_token.hex()
        writer.event(
            "worker.hello",
            {
                "auth_token": token_text,
                "worker_pid": os.getpid(),
                "worker_identity": worker_identity,
                "runtime_identity": runtime_identity
                or f"cpython-{sys.version_info.major}.{sys.version_info.minor}",
                "protocol_min": PROTOCOL_VERSION,
                "protocol_max": PROTOCOL_VERSION,
            },
            sensitive=True,
        )
        del token_text
        bootstrap.clear_secret()

        while True:
            if heartbeat is not None and heartbeat.error is not None:
                raise heartbeat.error
            encoded = _read_transport_frame(connection.reader)
            command = validator.validate(decode_messagepack(encoded))
            message = command["message"]
            body = message["body"]
            name = str(body["name"])
            if name == "session.shutdown" and heartbeat is not None:
                heartbeat.stop()
                heartbeat = None

            reply = decode_messagepack(worker.handle_messagepack(encoded))
            _validate_runtime_reply(command, reply)
            writer.send(reply["message"])

            reply_kind = reply["message"]["kind"]
            if name == "session.configure" and reply_kind == "ack" and heartbeat is None:
                interval_ms = int(body["payload"]["heartbeat_interval_ms"])
                heartbeat = _HeartbeatPump(worker, writer, interval_ms)
                heartbeat.start()
            if name == "session.shutdown":
                clean_shutdown = reply_kind == "ack"
                return 0 if clean_shutdown else 2
    except Exception:
        return 2
    finally:
        if heartbeat is not None:
            heartbeat.stop()
        if bootstrap is not None:
            bootstrap.clear_secret()
        if worker is not None and not clean_shutdown:
            worker.abort_transport()
        if connection is not None:
            connection.close()


def _read_transport_frame(stream: BinaryIO) -> bytes:
    prefix = _read_exact_transport(stream, 4, "control frame length")
    byte_length = struct.unpack("<I", prefix)[0]
    if not 1 <= byte_length <= 262_144:
        raise ProtocolError("control frame length is outside its bound")
    return _read_exact_transport(stream, byte_length, "control frame payload")


def _read_exact_transport(stream: BinaryIO, size: int, label: str) -> bytes:
    value = bytearray()
    while len(value) < size:
        chunk = stream.read(size - len(value))
        if not chunk:
            raise ProtocolError(f"{label} is truncated")
        value.extend(chunk)
    return bytes(value)


def _write_all_transport(stream: BinaryIO, value: bytes | bytearray) -> None:
    view = memoryview(value)
    written = 0
    while written < len(view):
        count = stream.write(view[written:])
        if count is None or count <= 0:
            raise OSError("worker pipe write did not make progress")
        written += count


def _unique_transport_map(pairs: list[tuple[object, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if not isinstance(key, str) or key in result:
            raise ProtocolError("bootstrap map keys must be unique text")
        result[key] = value
    return result


def _validate_runtime_reply(command: Mapping[str, object], reply: Mapping[str, object]) -> None:
    if reply["session_id"] != command["session_id"]:
        raise ProtocolError("runtime reply belongs to another session")
    command_body = command["message"]["body"]
    reply_message = reply["message"]
    if reply_message["kind"] not in {"ack", "error"}:
        raise ProtocolError("runtime did not produce a terminal command reply")
    reply_body = reply_message["body"]
    if reply_body["reply_to"] != command["message_id"]:
        raise ProtocolError("runtime reply_to does not match the command")
    actual_name = (
        reply_body["ack"]["name"] if reply_message["kind"] == "ack" else reply_body["name"]
    )
    if actual_name != command_body["name"]:
        raise ProtocolError("runtime reply name does not match the command")


class WorkerRuntimeError(RuntimeError):
    """A bounded, stable Protocol 2 runtime failure."""

    def __init__(
        self,
        code: ErrorCode,
        detail: str,
        *,
        retryable: bool = False,
        fatal: bool = False,
        details: Mapping[str, str] | None = None,
    ) -> None:
        safe_detail = _bounded_error_text(detail)
        safe_details = _bounded_details(details or {})
        super().__init__(f"{code.value}: {safe_detail}")
        self.code = code
        self.detail = safe_detail
        self.retryable = retryable
        self.fatal = fatal
        self.details = safe_details


@dataclass(frozen=True, slots=True)
class TrustedCodecEntrypoint:
    pack_id: str
    pack_version: str
    adapter_id: str
    adapter_version: str
    entrypoint: str

    def validate(self) -> None:
        _identifier(self.pack_id, "pack_id")
        _version(self.pack_version, "pack_version")
        _identifier(self.adapter_id, "adapter_id")
        _version(self.adapter_version, "adapter_version")
        _entrypoint(self.entrypoint)


@dataclass(frozen=True, slots=True)
class TrustedDeckEntrypoint:
    deck_id: str
    deck_version: str
    operator_id: str
    operator_version: str
    entrypoint: str

    def validate(self) -> None:
        _identifier(self.deck_id, "deck_id")
        _version(self.deck_version, "deck_version")
        _identifier(self.operator_id, "operator_id")
        _version(self.operator_version, "operator_version")
        _entrypoint(self.entrypoint)


@runtime_checkable
class CartridgeAccessFactory(Protocol):
    """Consume a duplicated native handle into Core-retained cartridge access.

    ``open`` owns the target-process handle as soon as it is called and must
    close that handle itself if opening fails before an access object is
    returned. The worker closes every returned access that it cannot register.
    """

    def open(
        self,
        *,
        retained_native_handle: int,
        archive_bytes: int,
        cartridge_id: uuid.UUID,
        archive_sha256: str,
        integrity_access_receipt: str,
    ) -> CartridgeAccess: ...

    def close(self, access: CartridgeAccess) -> None: ...


@runtime_checkable
class SharedRingTransport(Protocol):
    """Native shared-ring boundary; bulk bytes never enter a P2 frame.

    ``configure`` takes ownership of all three target-process handles on entry
    and must reclaim them if native validation or binding fails.
    """

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
    ) -> None:
        """Close a valid transferred handle triple rejected before configure."""
        ...

    def release(self, ring_id: uuid.UUID) -> None: ...

    def set_generation(self, ring_id: uuid.UUID, new_generation: int) -> None: ...

    def publish(
        self,
        *,
        ring_id: uuid.UUID,
        session_id: uuid.UUID,
        stream_generation: int,
        sequence: int,
        batch: DecodedBatch,
    ) -> int: ...


@dataclass(frozen=True, slots=True)
class ProcessReceipt:
    """Bounded metadata for one out-of-band tensor/decode operation."""

    session_id: uuid.UUID
    stream_generation: int
    sequence: int
    output_ring_id: uuid.UUID
    output_slot_sequence: int
    latent_shape: tuple[int, int, int, int, int]
    latent_dtype: str
    latent_device: str
    decoded_shape: tuple[int, int, int, int]
    provenance: Mapping[str, object]


@dataclass(frozen=True, slots=True)
class CommandResult:
    """Non-byte command result retained for the native host bridge."""

    name: str
    value: object


@dataclass(slots=True)
class _BoundCartridge:
    source_id: uuid.UUID
    access: CartridgeAccess
    inspection: ProfileInspection | None = None
    receipt: ProfileReceipt | None = None
    handle: SourceHandle | None = None


@dataclass(frozen=True, slots=True)
class _RingBinding:
    ring_id: uuid.UUID
    kind: str
    slot_count: int
    slot_bytes: int


@dataclass(frozen=True, slots=True)
class _SourceBinding:
    physical_slot: int
    source_id: uuid.UUID
    cartridge_id: uuid.UUID
    archive_sha256: str
    profile_receipt_id: uuid.UUID
    loop_enabled: bool


@dataclass(slots=True)
class _SourceTransport:
    physical_slot: int
    playing: bool
    loop_enabled: bool


@dataclass(slots=True)
class _PlayerSession:
    session_id: uuid.UUID
    source: _SourceBinding
    stream_generation: int
    playhead: int = 0
    sequence: int = 0
    state: PlayerState = PlayerState.READY


@dataclass(slots=True)
class _CaptureSession:
    capture_id: uuid.UUID
    mode: str
    maximum_latent_slots: int
    maximum_visual_bytes: int
    maximum_reset_events: int
    writer: object
    state: CaptureState = CaptureState.CAPTURING
    reset_events: int = 0
    latent_slots: int = 0
    pending_reset_event: Mapping[str, object] | None = None
    payload: CapturePayload | None = None


@dataclass(slots=True)
class _DeckSession:
    session_id: uuid.UUID
    deck_id: str
    deck_version: str
    operator_id: str
    operator_version: str
    operator: Callable[..., DeckOperatorResult]
    sources: tuple[_SourceBinding, ...]
    roles: tuple[RoleBinding, ...]
    controls: dict[str, object]
    seed: int
    stream_generation: int
    deck_revision: int = 1
    sequence: int = 0
    playheads: list[int] = field(default_factory=list)
    previous_sources: list[object | None] = field(default_factory=list)
    source_transport: list[_SourceTransport] = field(default_factory=list)
    state: DeckState = DeckState.READY
    capture: _CaptureSession | None = None


class Protocol2Worker:
    """One isolated warm session with a closed P2-only dispatch surface.

    The Rust host broker owns the four-worker capacity and the single foreground
    output lease. A worker never evicts or absorbs a second warm session.
    """

    def __init__(
        self,
        *,
        session_id: uuid.UUID,
        codec_entrypoints: tuple[TrustedCodecEntrypoint, ...],
        deck_entrypoints: tuple[TrustedDeckEntrypoint, ...],
        cartridge_access_factory: CartridgeAccessFactory,
        ring_transport: SharedRingTransport,
    ) -> None:
        if not isinstance(session_id, uuid.UUID) or session_id.int == 0:
            raise ValueError("session_id must be a non-nil UUID")
        if not isinstance(cartridge_access_factory, CartridgeAccessFactory):
            raise TypeError("cartridge_access_factory must implement CartridgeAccessFactory")
        if not isinstance(ring_transport, SharedRingTransport):
            raise TypeError("ring_transport must implement SharedRingTransport")
        self._session_id = session_id
        self._codec_registry = _codec_registry(codec_entrypoints)
        self._deck_registry = _deck_registry(deck_entrypoints)
        self._cartridge_access_factory = cartridge_access_factory
        self._ring_transport = ring_transport
        self._adapter: CodecAdapter | None = None
        self._descriptor: CodecDescriptor | None = None
        self._codec_load_request: CodecLoadRequest | None = None
        self._codec_state = CodecState.UNLOADED
        self._session_state = SessionState.UNCONFIGURED
        self._player: _PlayerSession | None = None
        self._deck: _DeckSession | None = None
        self._capture_state = CaptureState.IDLE
        self._bound: dict[uuid.UUID, _BoundCartridge] = {}
        self._rings: dict[uuid.UUID, _RingBinding] = {}
        self._receipt_sources: dict[uuid.UUID, uuid.UUID] = {}
        self._accepted_capabilities: frozenset[Capability] = frozenset()
        self._raw_imports: dict[uuid.UUID, RawImportPreflight] = {}
        self._raw_import_artifacts: dict[uuid.UUID, tuple[RawImportArtifact, Path]] = {}
        self._incoming_sequence = 1
        self._outgoing_sequence = 1
        self._seen_message_ids: set[uuid.UUID] = set()
        self._started_ns = time.monotonic_ns()
        self._command_results: dict[uuid.UUID, CommandResult] = {}
        self._shutdown = False
        self._metrics = {
            "commands_total": 0,
            "commands_failed_total": 0,
            "player_steps_total": 0,
            "deck_process_total": 0,
            "capture_slots_total": 0,
            "decoded_frames_total": 0,
        }

    @property
    def session_id(self) -> uuid.UUID:
        return self._session_id

    def bind_cartridge(self, source_id: uuid.UUID, access: CartridgeAccess) -> None:
        """Bind a Core-retained validated handle; paths are intentionally unsupported."""

        if not isinstance(source_id, uuid.UUID) or source_id.int == 0:
            raise ValueError("source_id must be a non-nil UUID")
        if not isinstance(access, CartridgeAccess):
            raise TypeError("access must implement CartridgeAccess")
        if source_id in self._bound:
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "source ID is already bound")
        _validate_cartridge_access(access)
        self._bound[source_id] = _BoundCartridge(source_id, access)

    def take_command_result(self, message_id: uuid.UUID) -> CommandResult | None:
        """Take bounded non-tensor metadata retained for the native host bridge."""

        return self._command_results.pop(message_id, None)

    def abort_transport(self) -> None:
        """Best-effort cleanup after framing, authentication, or pipe failure."""

        if self._shutdown:
            return
        if self._deck is not None and self._deck.capture is not None:
            capture = self._deck.capture
            if capture.state not in {CaptureState.COMPLETED, CaptureState.ABORTED}:
                with suppress(Exception):
                    capture.writer.abort()
                capture.state = CaptureState.ABORTED
                self._capture_state = CaptureState.ABORTED
        self._abort_all_raw_imports()
        for bound in tuple(self._bound.values()):
            if bound.handle is not None:
                with suppress(Exception):
                    bound.handle.close()
                bound.handle = None
            with suppress(Exception):
                self._cartridge_access_factory.close(bound.access)
        self._bound.clear()
        for ring_id in tuple(self._rings):
            with suppress(Exception):
                self._ring_transport.release(ring_id)
        self._rings.clear()
        self._shutdown = True
        self._session_state = SessionState.STOPPED

    def status(self) -> dict[str, object]:
        foreground: uuid.UUID | None = None
        if self._deck is not None:
            foreground = self._deck.session_id
        elif self._player is not None:
            foreground = self._player.session_id
        return {
            "session": self._session_state.value,
            "codec": self._codec_state.value,
            "player": (self._player.state.value if self._player else PlayerState.EMPTY.value),
            "deck": (self._deck.state.value if self._deck else DeckState.EMPTY.value),
            "capture": self._capture_state.value,
            "open_session_count": int(self._player is not None) + int(self._deck is not None),
            "foreground_output_session": None if foreground is None else str(foreground),
            "output_lease_pinned": self._capture_state
            in {CaptureState.STARTING, CaptureState.CAPTURING, CaptureState.FINALIZING},
        }

    def handle_messagepack(self, encoded: bytes) -> bytes:
        """Handle exactly one P2 frame; P1 is rejected and never retried."""

        envelope = decode_messagepack(encoded)
        reply = self.handle_envelope(envelope)
        return encode_messagepack(reply)

    def handle_envelope(self, raw: Mapping[str, object]) -> dict[str, object]:
        """Validate, dispatch, and return one bounded P2 ack or error envelope."""

        envelope = validate_envelope(raw)
        message_id = uuid.UUID(str(envelope["message_id"]))
        command_name = "session.status"
        try:
            self._validate_incoming(envelope, message_id)
            message = _mapping(envelope["message"], "message")
            if message["kind"] != "command":
                raise WorkerRuntimeError(
                    ErrorCode.PROTOCOL_INVALID_MESSAGE,
                    "worker accepts command envelopes only",
                )
            command = _mapping(message["body"], "command")
            command_name = str(command["name"])
            payload = _mapping(command["payload"], "command payload")
            self._metrics["commands_total"] += 1
            result = self._dispatch(command_name, payload)
            if result is not None:
                self._remember_result(message_id, CommandResult(command_name, result))
            return self._reply(
                {
                    "kind": "ack",
                    "body": {
                        "reply_to": str(message_id),
                        "ack": {
                            "name": command_name,
                            "payload": self._ack_payload(command_name, payload, result),
                        },
                        "status": self.status(),
                    },
                }
            )
        except WorkerRuntimeError as error:
            self._metrics["commands_failed_total"] += 1
            if error.fatal:
                self._session_state = SessionState.FAULTED
            return self._error_reply(message_id, command_name, error)
        except (CodecSdkError, DeckContractError) as error:
            self._metrics["commands_failed_total"] += 1
            return self._error_reply(
                message_id,
                command_name,
                WorkerRuntimeError(
                    _sdk_error_code(command_name),
                    "trusted extension rejected the requested operation",
                    details={"extension_code": error.code},
                ),
            )
        except Exception:
            self._metrics["commands_failed_total"] += 1
            return self._error_reply(
                message_id,
                command_name,
                WorkerRuntimeError(
                    ErrorCode.WORKER_INTERNAL,
                    "trusted runtime operation failed",
                    fatal=True,
                ),
            )

    def _ack_payload(
        self, name: str, command: Mapping[str, object], result: object | None
    ) -> dict[str, object]:
        if name == "session.configure":
            return {
                "selected_protocol_version": PROTOCOL_VERSION,
                "maximum_frame_bytes": 262_144,
                "accepted_capabilities": list(command["requested_capabilities"]),
            }
        if name == "session.status":
            return self.status()
        if name == "session.shutdown":
            return {"reason": command["reason"]}
        if name == "codec.descriptor":
            assert isinstance(result, CodecDescriptor)
            return _descriptor_wire(result)
        if name == "codec.load":
            assert isinstance(result, CodecDescriptor)
            return {
                "pack_id": result.pack_id,
                "pack_version": result.pack_version,
                "adapter_id": result.adapter_id,
                "adapter_version": result.adapter_version,
                "device": command["device"],
                "device_ordinal": command["device_ordinal"],
            }
        if name == "codec.unload":
            return {
                "pack_id": command["pack_id"],
                "pack_version": command["pack_version"],
            }
        if name == "source.open":
            assert isinstance(result, _BoundCartridge)
            return {
                "source_id": str(result.source_id),
                "cartridge_id": str(result.access.cartridge_id),
                "archive_sha256": result.access.archive_sha256,
            }
        if name == "source.close":
            assert isinstance(result, uuid.UUID)
            return {"source_id": str(result)}
        if name == "ring.configure":
            assert isinstance(result, _RingBinding)
            return {
                "ring_id": str(result.ring_id),
                "kind": result.kind,
                "slot_count": result.slot_count,
                "slot_bytes": result.slot_bytes,
            }
        if name == "ring.release":
            assert isinstance(result, uuid.UUID)
            return {"ring_id": str(result)}
        if name == "profile.inspect":
            assert isinstance(result, ProfileInspection)
            return _inspection_wire(uuid.UUID(str(command["source_id"])), result)
        if name == "profile.validate":
            assert isinstance(result, ProfileReceipt)
            return _receipt_wire(result)
        if name == "raw_import.preflight":
            assert isinstance(result, RawImportPreflight)
            return _raw_import_preflight_wire(result)
        if name == "raw_import.stage":
            assert isinstance(result, RawImportArtifact)
            return _raw_import_artifact_wire(result)
        if name == "raw_import.abort":
            assert isinstance(result, tuple) and len(result) == 2
            return {"import_id": str(result[0]), "receipt_id": str(result[1])}
        if name in {"player.open", "player.reset", "player.status"}:
            assert isinstance(result, _PlayerSession)
            return self._player_status_wire(result)
        if name == "player.step":
            assert isinstance(result, ProcessReceipt)
            assert self._player is not None
            return {
                "status": self._player_status_wire(self._player),
                "output_ring_id": str(result.output_ring_id),
                "output_slot_sequence": result.output_slot_sequence,
                "decoded_frames": result.decoded_shape[0],
            }
        if name in {
            "deck.load",
            "deck.controls.set",
            "deck.roles.set",
            "deck.transport.set",
            "deck.seed.set",
            "deck.reset",
            "deck.restart",
            "deck.status",
        }:
            assert isinstance(result, _DeckSession)
            return self._deck_status_wire(result)
        if name == "deck.process":
            assert isinstance(result, ProcessReceipt)
            assert self._deck is not None
            return {
                "status": self._deck_status_wire(self._deck),
                "output_ring_id": str(result.output_ring_id),
                "output_slot_sequence": result.output_slot_sequence,
                "provenance": _provenance_wire(result.provenance),
            }
        if name in {"capture.start", "capture.stop", "capture.status"}:
            assert isinstance(result, _CaptureSession | CapturePayload)
            assert self._deck is not None and self._deck.capture is not None
            return self._capture_status_wire(self._deck, self._deck.capture)
        if name == "metrics.get":
            assert isinstance(result, Mapping)
            return dict(result)
        raise WorkerRuntimeError(
            ErrorCode.PROTOCOL_INVALID_MESSAGE, "ack payload is not implemented"
        )

    def _player_status_wire(self, player: _PlayerSession) -> dict[str, object]:
        bound = self._require_bound(player.source.source_id)
        handle, _receipt = self._source_handle_and_receipt(bound, player.source)
        ring = self._decoded_ring()
        return {
            "player_session_id": str(player.session_id),
            "state": player.state.value,
            "stream_generation": player.stream_generation,
            "stream_sequence": player.sequence,
            "playhead_slot": player.playhead,
            "end_of_stream": player.playhead >= handle.slot_count
            and not player.source.loop_enabled,
            "decoded_ring_id": None if ring is None else str(ring.ring_id),
        }

    def _deck_status_wire(self, deck: _DeckSession) -> dict[str, object]:
        playheads: list[dict[str, object]] = []
        for index, source in enumerate(deck.sources):
            bound = self._require_bound(source.source_id)
            handle, _receipt = self._source_handle_and_receipt(bound, source)
            playhead = deck.playheads[index]
            transport = deck.source_transport[index]
            playheads.append(
                {
                    "physical_slot": source.physical_slot,
                    "latent_slot": playhead,
                    "loop_enabled": transport.loop_enabled,
                    "end_of_stream": (
                        not transport.playing
                        and not transport.loop_enabled
                        and playhead + 1 >= handle.slot_count
                    ),
                }
            )
        capture_state = CaptureState.IDLE
        if deck.capture is not None:
            capture_state = deck.capture.state
        return {
            "deck_session_id": str(deck.session_id),
            "state": deck.state.value,
            "deck_revision": deck.deck_revision,
            "stream_generation": deck.stream_generation,
            "stream_sequence": deck.sequence,
            "playheads": playheads,
            "roles": [
                {"role": role.role, "physical_slot": role.physical_slot} for role in deck.roles
            ],
            "controls": [
                {"name": name, "value": _control_value_wire(value)}
                for name, value in sorted(deck.controls.items())
            ],
            "source_transport": [
                {
                    "physical_slot": source.physical_slot,
                    "playing": source.playing,
                    "loop_enabled": source.loop_enabled,
                }
                for source in deck.source_transport
            ],
            "seed": deck.seed,
            "capture_state": capture_state.value,
        }

    def _capture_status_wire(
        self, deck: _DeckSession, capture: _CaptureSession
    ) -> dict[str, object]:
        artifact = None
        if capture.payload is not None:
            artifact = {
                "staged_payload_path": capture.payload.payload_path,
                "payload_sha256": capture.payload.payload_sha256,
                "payload_byte_length": capture.payload.payload_byte_length,
                "latent_slots": capture.payload.latent_slots,
                "decoded_frame_count": capture.payload.decoded_frame_count,
            }
        return {
            "deck_session_id": str(deck.session_id),
            "deck_revision": deck.deck_revision,
            "capture_id": str(capture.capture_id),
            "state": capture.state.value,
            "mode": capture.mode,
            "latent_slots": capture.latent_slots,
            "reset_events": capture.reset_events,
            "artifact": artifact,
        }

    def _metrics_snapshot(self) -> dict[str, int]:
        return {
            "worker_uptime_ns": time.monotonic_ns() - self._started_ns,
            **self._metrics,
        }

    def _validate_incoming(self, envelope: Mapping[str, object], message_id: uuid.UUID) -> None:
        if uuid.UUID(str(envelope["session_id"])) != self._session_id:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "command belongs to another session"
            )
        if envelope["sequence"] != self._incoming_sequence:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "command sequence is not contiguous"
            )
        if message_id in self._seen_message_ids:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "message ID was already consumed"
            )
        self._seen_message_ids.add(message_id)
        self._incoming_sequence += 1

    def _dispatch(self, name: str, payload: Mapping[str, object]) -> object | None:
        if self._shutdown:
            raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "worker is stopped")
        if name == "session.configure":
            return self._configure(payload)
        if self._session_state is SessionState.UNCONFIGURED:
            raise WorkerRuntimeError(
                ErrorCode.SESSION_NOT_CONFIGURED, "session.configure must run first"
            )
        dispatch: dict[str, Callable[[Mapping[str, object]], object | None]] = {
            "session.status": lambda _payload: self.status(),
            "session.shutdown": self._shutdown_worker,
            "codec.descriptor": self._codec_descriptor,
            "codec.load": self._codec_load,
            "codec.unload": self._codec_unload,
            "source.open": self._source_open,
            "source.close": self._source_close,
            "ring.configure": self._ring_configure,
            "ring.release": self._ring_release,
            "profile.inspect": self._profile_inspect,
            "profile.validate": self._profile_validate,
            "raw_import.preflight": self._raw_import_preflight,
            "raw_import.stage": self._raw_import_stage,
            "raw_import.abort": self._raw_import_abort,
            "player.open": self._player_open,
            "player.step": self._player_step,
            "player.reset": self._player_reset,
            "player.status": lambda _payload: self._player,
            "deck.load": self._deck_load,
            "deck.process": self._deck_process,
            "deck.controls.set": self._deck_controls_set,
            "deck.roles.set": self._deck_roles_set,
            "deck.transport.set": self._deck_transport_set,
            "deck.seed.set": self._deck_seed_set,
            "deck.reset": self._deck_reset,
            "deck.restart": self._deck_restart,
            "deck.status": lambda _payload: self._deck,
            "capture.start": self._capture_start,
            "capture.stop": self._capture_stop,
            "capture.status": self._capture_status,
            "metrics.get": lambda _payload: self._metrics_snapshot(),
        }
        operation = dispatch.get(name)
        if operation is None:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "command is not implemented by P2"
            )
        return operation(payload)

    def _configure(self, payload: Mapping[str, object]) -> dict[str, object]:
        if self._session_state is not SessionState.UNCONFIGURED:
            raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "session is already configured")
        requested = {Capability(value) for value in payload["requested_capabilities"]}
        if not requested:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "at least one capability is required"
            )
        self._session_state = SessionState.READY
        self._accepted_capabilities = frozenset(requested)
        return {"protocol_version": PROTOCOL_VERSION, "capabilities": tuple(sorted(requested))}

    def _codec_descriptor(self, payload: Mapping[str, object]) -> CodecDescriptor:
        key = (str(payload["pack_id"]), str(payload["pack_version"]), str(payload["adapter_id"]))
        if self._descriptor is not None and self._adapter is not None:
            selected = (
                self._descriptor.pack_id,
                self._descriptor.pack_version,
                self._descriptor.adapter_id,
            )
            if selected != key:
                raise WorkerRuntimeError(
                    ErrorCode.STATE_BUSY,
                    "another codec descriptor is already selected",
                )
            return self._descriptor
        trusted = self._codec_registry.get(key)
        if trusted is None:
            raise WorkerRuntimeError(ErrorCode.CODEC_UNTRUSTED, "codec entrypoint is not trusted")
        adapter = _load_adapter(trusted)
        descriptor = validate_codec_v2_descriptor(adapter.descriptor())
        if (
            descriptor.pack_id,
            descriptor.pack_version,
            descriptor.adapter_id,
            descriptor.adapter_version,
        ) != (
            trusted.pack_id,
            trusted.pack_version,
            trusted.adapter_id,
            trusted.adapter_version,
        ):
            raise WorkerRuntimeError(
                ErrorCode.CODEC_UNTRUSTED, "codec descriptor does not match its trust receipt"
            )
        self._adapter = adapter
        self._descriptor = descriptor
        return descriptor

    def _codec_load(self, payload: Mapping[str, object]) -> CodecDescriptor:
        expected = (
            str(payload["pack_id"]),
            str(payload["pack_version"]),
            str(payload["adapter_id"]),
            str(payload["adapter_version"]),
        )
        adapter, descriptor = self._selected_codec()
        actual = (
            descriptor.pack_id,
            descriptor.pack_version,
            descriptor.adapter_id,
            descriptor.adapter_version,
        )
        if actual != expected:
            raise WorkerRuntimeError(ErrorCode.CODEC_UNTRUSTED, "codec identity is not exact")
        if not self._receipt_sources:
            raise WorkerRuntimeError(
                ErrorCode.PROFILE_INVALID,
                "codec load requires a cross-checked profile receipt",
            )
        assets = tuple(
            ExternalAsset(
                asset_id=str(raw["asset_id"]),
                path=str(raw["path"]),
                sha256=str(raw["sha256"]),
                byte_length=int(raw["byte_length"]),
            )
            for raw in payload["external_assets"]
        )
        request = CodecLoadRequest(
            descriptor=descriptor,
            assets=assets,
            device=str(payload["device"]),
            device_ordinal=int(payload["device_ordinal"]),
        )
        request.validate()
        if request.device == "cpu" and request.device_ordinal != 0:
            raise WorkerRuntimeError(
                ErrorCode.PROFILE_INVALID,
                "CPU codec loads require device ordinal zero",
            )
        if any(
            bound.receipt is not None
            and bound.receipt.tensor_abi.device != request.device
            for bound in self._bound.values()
        ):
            raise WorkerRuntimeError(
                ErrorCode.PROFILE_INVALID,
                "codec load device does not match every validated profile receipt",
            )
        self._codec_state = CodecState.LOADING
        try:
            adapter.load(request)
        except Exception:
            self._codec_load_request = None
            self._codec_state = CodecState.FAULTED
            raise
        self._codec_load_request = request
        self._codec_state = CodecState.READY
        return descriptor

    def _codec_unload(self, payload: Mapping[str, object]) -> None:
        if self._descriptor is not None and (
            str(payload["pack_id"]),
            str(payload["pack_version"]),
        ) != (self._descriptor.pack_id, self._descriptor.pack_version):
            raise WorkerRuntimeError(ErrorCode.CODEC_NOT_LOADED, "codec identity is not loaded")
        self._abort_all_raw_imports()
        self._close_handles()
        self._player = None
        self._deck = None
        self._capture_state = CaptureState.IDLE
        self._adapter = None
        self._descriptor = None
        self._codec_load_request = None
        self._codec_state = CodecState.UNLOADED
        self._receipt_sources.clear()
        for bound in self._bound.values():
            bound.inspection = None
            bound.receipt = None

    def _source_open(self, payload: Mapping[str, object]) -> _BoundCartridge:
        source_id = uuid.UUID(str(payload["source_id"]))
        cartridge_id = uuid.UUID(str(payload["cartridge_id"]))
        archive_sha256 = str(payload["archive_sha256"])
        archive_bytes = int(payload["archive_bytes"])
        access = self._cartridge_access_factory.open(
            retained_native_handle=int(payload["retained_native_handle"]),
            archive_bytes=archive_bytes,
            cartridge_id=cartridge_id,
            archive_sha256=archive_sha256,
            integrity_access_receipt=str(payload["integrity_access_receipt"]),
        )
        registered = False
        try:
            if source_id in self._bound:
                raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "source ID is already open")
            if not isinstance(access, CartridgeAccess):
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_INVALID, "retained handle access is invalid"
                )
            try:
                _validate_cartridge_access(access)
            except (TypeError, ValueError) as error:
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_INVALID, "retained handle access is invalid"
                ) from error
            if access.cartridge_id != cartridge_id or access.archive_sha256 != archive_sha256:
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_INVALID, "retained handle identity is not exact"
                )
            bound = _BoundCartridge(source_id, access)
            self._bound[source_id] = bound
            registered = True
            return bound
        finally:
            if not registered:
                self._cartridge_access_factory.close(access)

    def _source_close(self, payload: Mapping[str, object]) -> uuid.UUID:
        source_id = uuid.UUID(str(payload["source_id"]))
        if self._source_is_active(source_id):
            raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "source belongs to an active session")
        bound = self._bound.pop(source_id, None)
        if bound is None:
            raise WorkerRuntimeError(ErrorCode.SOURCE_NOT_LOADED, "source is not open")
        if bound.handle is not None:
            bound.handle.close()
        if bound.receipt is not None:
            self._receipt_sources.pop(bound.receipt.receipt_id, None)
        self._cartridge_access_factory.close(bound.access)
        return source_id

    def _ring_configure(self, payload: Mapping[str, object]) -> _RingBinding:
        ring_id = uuid.UUID(str(payload["ring_id"]))
        kind = str(payload["kind"])
        transferred_handles = {
            "mapping_handle": int(payload["mapping_handle"]),
            "ready_event_handle": int(payload["ready_event_handle"]),
            "consumed_event_handle": int(payload["consumed_event_handle"]),
        }
        try:
            if ring_id in self._rings:
                raise WorkerRuntimeError(
                    ErrorCode.PROTOCOL_INVALID_MESSAGE, "ring ID is already bound"
                )
            if any(ring.kind == kind for ring in self._rings.values()):
                raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "ring kind is already configured")
            binding = _RingBinding(
                ring_id=ring_id,
                kind=kind,
                slot_count=int(payload["slot_count"]),
                slot_bytes=int(payload["slot_bytes"]),
            )
        except Exception:
            self._ring_transport.discard_transferred_handles(**transferred_handles)
            raise
        self._ring_transport.configure(
            ring_id=ring_id,
            kind=kind,
            **transferred_handles,
            slot_count=binding.slot_count,
            slot_bytes=binding.slot_bytes,
        )
        try:
            self._rings[ring_id] = binding
        except Exception:
            self._ring_transport.release(ring_id)
            raise
        return binding

    def _ring_release(self, payload: Mapping[str, object]) -> uuid.UUID:
        ring_id = uuid.UUID(str(payload["ring_id"]))
        binding = self._rings.pop(ring_id, None)
        if binding is None:
            raise WorkerRuntimeError(ErrorCode.PROTOCOL_INVALID_MESSAGE, "ring is not configured")
        self._ring_transport.release(ring_id)
        return ring_id

    def _profile_inspect(self, payload: Mapping[str, object]) -> ProfileInspection:
        adapter, _descriptor = self._selected_codec()
        source_id = uuid.UUID(str(payload["source_id"]))
        bound = self._require_bound(source_id)
        if (
            str(bound.access.cartridge_id) != str(payload["cartridge_id"])
            or bound.access.archive_sha256 != payload["archive_sha256"]
        ):
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "source identity is not exact")
        inspection = adapter.inspect(bound.access)
        inspection.validate()
        if (
            inspection.cartridge_id != bound.access.cartridge_id
            or inspection.archive_sha256 != bound.access.archive_sha256
        ):
            raise WorkerRuntimeError(
                ErrorCode.PROFILE_INVALID, "profile inspection changed cartridge identity"
            )
        bound.inspection = inspection
        return inspection

    def _profile_validate(self, payload: Mapping[str, object]) -> ProfileReceipt:
        adapter, descriptor = self._selected_codec()
        source_id = uuid.UUID(str(payload["source_id"]))
        bound = self._require_bound(source_id)
        if bound.inspection is None:
            raise WorkerRuntimeError(ErrorCode.PROFILE_INVALID, "profile must be inspected first")
        expected = _profile_key(payload["expected_profile"])
        if bound.inspection.profile_key != expected:
            raise WorkerRuntimeError(
                ErrorCode.PROFILE_INCOMPATIBLE, "profile identity does not match selection"
            )
        required = tuple(Capability(value) for value in payload["required_capabilities"])
        receipt = validate_profile_receipt(
            adapter.validate_profile(bound.access, bound.inspection), descriptor
        )
        _crosscheck_receipt(receipt, bound.inspection)
        if not set(required).issubset(receipt.capabilities):
            raise WorkerRuntimeError(
                ErrorCode.PROFILE_INCOMPATIBLE, "profile lacks a required capability"
            )
        if receipt.receipt_id in self._receipt_sources:
            raise WorkerRuntimeError(ErrorCode.PROFILE_INVALID, "receipt ID was already issued")
        bound.receipt = receipt
        self._receipt_sources[receipt.receipt_id] = source_id
        return receipt

    def _raw_import_preflight(self, payload: Mapping[str, object]) -> RawImportPreflight:
        adapter, descriptor = self._raw_import_adapter()
        request = RawImportPreflightRequest(
            import_id=uuid.UUID(str(payload["import_id"])),
            source_path=str(payload["source_path"]),
            maximum_source_bytes=int(payload["maximum_source_bytes"]),
        )
        request.validate()
        if request.import_id in self._raw_imports:
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "raw import ID is already active")
        preflight = adapter.preflight_raw_import(request)
        if not isinstance(preflight, RawImportPreflight):
            raise WorkerRuntimeError(
                ErrorCode.SOURCE_INVALID, "raw import adapter returned an invalid preflight"
            )
        preflight.validate()
        if (
            preflight.import_id != request.import_id
            or preflight.source_byte_length > request.maximum_source_bytes
            or (
                preflight.pack_id,
                preflight.pack_version,
                preflight.adapter_id,
                preflight.adapter_version,
            )
            != (
                descriptor.pack_id,
                descriptor.pack_version,
                descriptor.adapter_id,
                descriptor.adapter_version,
            )
        ):
            with suppress(Exception):
                adapter.abort_raw_import(request.import_id)
            raise WorkerRuntimeError(
                ErrorCode.SOURCE_INVALID, "raw import preflight identity is not exact"
            )
        if any(item.receipt_id == preflight.receipt_id for item in self._raw_imports.values()):
            with suppress(Exception):
                adapter.abort_raw_import(request.import_id)
            raise WorkerRuntimeError(
                ErrorCode.SOURCE_INVALID, "raw import receipt ID was already issued"
            )
        self._raw_imports[request.import_id] = preflight
        return preflight

    def _raw_import_stage(self, payload: Mapping[str, object]) -> RawImportArtifact:
        adapter, _descriptor = self._raw_import_adapter()
        import_id = uuid.UUID(str(payload["import_id"]))
        receipt_id = uuid.UUID(str(payload["receipt_id"]))
        preflight = self._raw_imports.get(import_id)
        if preflight is None or preflight.receipt_id != receipt_id:
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "raw import receipt is not active")
        if import_id in self._raw_import_artifacts:
            raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "raw import is already staged")
        try:
            staging_root = Path(str(payload["staging_root"]))
            if staging_root.is_symlink():
                raise OSError("staging root is a link")
            staging_root = staging_root.resolve(strict=True)
            if not staging_root.is_dir():
                raise OSError("staging root is not a directory")
        except OSError as error:
            raise WorkerRuntimeError(
                ErrorCode.SOURCE_INVALID, "Core staging root is not retained"
            ) from error
        request = RawImportStageRequest(preflight, str(staging_root))
        request.validate()
        artifact: RawImportArtifact | None = None
        try:
            artifact = adapter.stage_raw_import(request)
            if not isinstance(artifact, RawImportArtifact):
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_INVALID, "raw import adapter returned an invalid artifact"
                )
            artifact.validate()
            if artifact.import_id != import_id or artifact.receipt_id != receipt_id:
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_INVALID, "staged raw import identity is not exact"
                )
            staged = _admit_staged_raw_import_path(artifact, staging_root)
            self._raw_import_artifacts[import_id] = (artifact, staging_root)
            # Keep the canonical path that Core will independently re-admit.
            if staged != Path(artifact.staged_payload_path):
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_INVALID, "staged raw import path is not canonical"
                )
            return artifact
        except Exception:
            with suppress(Exception):
                adapter.abort_raw_import(import_id)
            if artifact is not None:
                _cleanup_admitted_raw_import_artifact(artifact, staging_root)
            self._raw_import_artifacts.pop(import_id, None)
            raise

    def _raw_import_abort(self, payload: Mapping[str, object]) -> tuple[uuid.UUID, uuid.UUID]:
        adapter, _descriptor = self._raw_import_adapter()
        import_id = uuid.UUID(str(payload["import_id"]))
        receipt_id = uuid.UUID(str(payload["receipt_id"]))
        preflight = self._raw_imports.get(import_id)
        if preflight is None or preflight.receipt_id != receipt_id:
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "raw import receipt is not active")
        with suppress(Exception):
            adapter.abort_raw_import(import_id)
        admitted = self._raw_import_artifacts.pop(import_id, None)
        if admitted is not None:
            _cleanup_admitted_raw_import_artifact(*admitted)
        self._raw_imports.pop(import_id, None)
        return import_id, receipt_id

    def _raw_import_adapter(self) -> tuple[RawImportAdapter, CodecDescriptor]:
        adapter, descriptor = self._selected_codec()
        if (
            Capability.RAW_IMPORT not in self._accepted_capabilities
            or Capability.RAW_IMPORT not in descriptor.capabilities
        ):
            raise WorkerRuntimeError(
                ErrorCode.CODEC_CAPABILITY_UNSUPPORTED,
                "selected codec does not support raw import",
            )
        if not isinstance(adapter, RawImportAdapter):
            raise WorkerRuntimeError(
                ErrorCode.CODEC_UNTRUSTED,
                "codec declares raw import without the optional adapter API",
            )
        if (
            self._codec_state is not CodecState.UNLOADED
            or self._player is not None
            or self._deck is not None
        ):
            raise WorkerRuntimeError(
                ErrorCode.STATE_BUSY, "raw import requires an unloaded CPU-only codec session"
            )
        return adapter, descriptor

    def _player_open(self, payload: Mapping[str, object]) -> _PlayerSession:
        self._ready_codec()
        session_id = uuid.UUID(str(payload["player_session_id"]))
        self._admit_worker_session(session_id)
        source = _source_binding(payload["source"])
        self._open_source(source, Capability.PLAYER)
        player = _PlayerSession(session_id, source, int(payload["stream_generation"]))
        self._player = player
        return player

    def _player_step(self, payload: Mapping[str, object]) -> ProcessReceipt:
        adapter, _descriptor = self._ready_codec()
        player = self._require_player(payload)
        maximum = int(payload["maximum_decoded_frames"])
        bound = self._require_bound(player.source.source_id)
        handle, receipt = self._source_handle_and_receipt(bound, player.source)
        if player.playhead >= handle.slot_count:
            if player.source.loop_enabled:
                player.playhead = 0
            else:
                player.state = PlayerState.END_OF_STREAM
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_NOT_LOADED, "player reached end of stream"
                )
        tensor = adapter.read_slot(handle, player.playhead)
        load_request = self._loaded_codec_request()
        _validate_tensor(tensor, receipt, load_request)
        batch = adapter.decode_slot(tensor, maximum)
        batch.validate()
        player.sequence += 1
        ring_id, slot_sequence = self._publish(
            player.session_id, player.stream_generation, player.sequence, batch
        )
        player.playhead += 1
        player.state = PlayerState.PLAYING
        self._metrics["player_steps_total"] += 1
        self._metrics["decoded_frames_total"] += batch.batch
        return _process_receipt(
            player.session_id,
            player.stream_generation,
            player.sequence,
            ring_id,
            slot_sequence,
            tensor,
            batch,
            {},
        )

    def _player_reset(self, payload: Mapping[str, object]) -> _PlayerSession:
        adapter, _descriptor = self._ready_codec()
        player = self._require_player(payload, generation_field=None)
        generation = int(payload["new_stream_generation"])
        if generation <= player.stream_generation:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "stream generation must increase"
            )
        ring = self._decoded_ring()
        if ring is None:
            raise WorkerRuntimeError(
                ErrorCode.STATE_BUSY, "exactly one decoded RGBA ring must be configured"
            )
        adapter.reset_decoder(generation)
        self._ring_transport.set_generation(ring.ring_id, generation)
        player.stream_generation = generation
        player.playhead = 0
        player.sequence = 0
        player.state = PlayerState.READY
        return player

    def _deck_load(self, payload: Mapping[str, object]) -> _DeckSession:
        self._ready_codec()
        session_id = uuid.UUID(str(payload["deck_session_id"]))
        self._admit_worker_session(session_id)
        key = (
            str(payload["deck_id"]),
            str(payload["deck_version"]),
            str(payload["operator_id"]),
            str(payload["operator_version"]),
        )
        runtime = payload.get("runtime")
        if runtime is None:
            trusted = self._deck_registry.get(key)
            if trusted is None:
                raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "deck entrypoint is not trusted")
            operator = _load_operator(trusted)
        else:
            operator = _load_bound_operator(_mapping(runtime, "Deck runtime binding"), key)
        sources = tuple(
            sorted(
                (_source_binding(raw) for raw in payload["sources"]), key=lambda s: s.physical_slot
            )
        )
        if tuple(source.physical_slot for source in sources) != tuple(range(1, len(sources) + 1)):
            raise WorkerRuntimeError(
                ErrorCode.DECK_INVALID, "physical slots must be the exact range 1..N"
            )
        receipts = tuple(self._open_source(source, Capability.REALTIME) for source in sources)
        _compatible_receipts(receipts)
        roles = tuple(
            RoleBinding(str(raw["role"]), int(raw["physical_slot"])) for raw in payload["roles"]
        )
        for role in roles:
            role.validate(len(sources))
        controls = _controls(payload["controls"])
        deck = _DeckSession(
            session_id=session_id,
            deck_id=key[0],
            deck_version=key[1],
            operator_id=key[2],
            operator_version=key[3],
            operator=operator,
            sources=sources,
            roles=roles,
            controls=controls,
            seed=int(payload["seed"]),
            stream_generation=int(payload["stream_generation"]),
            playheads=[0] * len(sources),
            previous_sources=[None] * len(sources),
            source_transport=[
                _SourceTransport(source.physical_slot, True, source.loop_enabled)
                for source in sources
            ],
        )
        self._deck = deck
        return deck

    def _deck_process(self, payload: Mapping[str, object]) -> ProcessReceipt:
        adapter, _descriptor = self._ready_codec()
        deck = self._require_deck(payload, generation_field="stream_generation")
        tensors: list[object] = []
        receipt: ProfileReceipt | None = None
        for index, source in enumerate(deck.sources):
            bound = self._require_bound(source.source_id)
            handle, source_receipt = self._source_handle_and_receipt(bound, source)
            receipt = source_receipt
            playhead = deck.playheads[index]
            if not 0 <= playhead < handle.slot_count:
                raise WorkerRuntimeError(
                    ErrorCode.SOURCE_NOT_LOADED, "a Deck source playhead is outside its handle"
                )
            tensor = adapter.read_slot(handle, playhead)
            _validate_tensor(tensor, source_receipt, self._loaded_codec_request())
            tensors.append(tensor)
        assert receipt is not None
        deck.sequence += 1
        context = DeckOperatorContext(
            codec_family=receipt.profile_key.codec_family,
            profile=receipt.profile_key.profile,
            profile_version=receipt.profile_key.profile_version,
            timing_contract=receipt.signal_geometry.timing_contract,
            timing_contract_version=receipt.signal_geometry.timing_contract_version,
            frame_rate_numerator=receipt.signal_geometry.frame_rate_numerator,
            frame_rate_denominator=receipt.signal_geometry.frame_rate_denominator,
            generation=deck.stream_generation,
            sequence=deck.sequence,
            seed=deck.seed,
            playheads=tuple(deck.playheads),
            physical_slots=tuple(source.physical_slot for source in deck.sources),
            roles=deck.roles,
            previous_sources=tuple(deck.previous_sources),
        )
        result = process_sources_checked(
            deck.operator,
            tuple(tensors),
            deck.controls,
            context,
        )
        _validate_tensor(result.output, receipt, self._loaded_codec_request())
        # Capture is deliberately before decode: the adapter receives the exact
        # post-operator latent tensor and Core later finalizes the LC container.
        self._capture_append(deck, result.output)
        batch = adapter.decode_slot(result.output, receipt.decoded_abi.maximum_batch)
        batch.validate()
        ring_id, slot_sequence = self._publish(
            deck.session_id, deck.stream_generation, deck.sequence, batch
        )
        deck.previous_sources = list(tensors)
        for index, source in enumerate(deck.sources):
            transport = deck.source_transport[index]
            if not transport.playing:
                continue
            bound = self._require_bound(source.source_id)
            handle, _source_receipt = self._source_handle_and_receipt(bound, source)
            next_playhead = deck.playheads[index] + 1
            if next_playhead < handle.slot_count:
                deck.playheads[index] = next_playhead
            elif transport.loop_enabled:
                # A later bounded slice owns causal decoder/ring generation
                # rollover. This slice pins independent scheduler state only.
                deck.playheads[index] = 0
            else:
                transport.playing = False
        if deck.capture is None or deck.capture.state is CaptureState.COMPLETED:
            deck.state = (
                DeckState.PLAYING
                if any(source.playing for source in deck.source_transport)
                else DeckState.PAUSED
            )
        self._metrics["deck_process_total"] += 1
        self._metrics["decoded_frames_total"] += batch.batch
        return _process_receipt(
            deck.session_id,
            deck.stream_generation,
            deck.sequence,
            ring_id,
            slot_sequence,
            result.output,
            batch,
            result.provenance,
        )

    def _deck_controls_set(self, payload: Mapping[str, object]) -> _DeckSession:
        deck = self._require_deck(payload)
        deck.controls = _controls(payload["controls"])
        return deck

    def _deck_roles_set(self, payload: Mapping[str, object]) -> _DeckSession:
        deck = self._require_deck(payload)
        roles = tuple(
            RoleBinding(str(raw["role"]), int(raw["physical_slot"])) for raw in payload["roles"]
        )
        for role in roles:
            role.validate(len(deck.sources))
        deck.roles = roles
        return deck

    def _deck_transport_set(self, payload: Mapping[str, object]) -> _DeckSession:
        deck = self._require_deck(payload)
        sources = sorted(
            (_source_transport_binding(raw) for raw in payload["sources"]),
            key=lambda source: source.physical_slot,
        )
        if [source.physical_slot for source in sources] != [
            source.physical_slot for source in deck.sources
        ]:
            raise WorkerRuntimeError(
                ErrorCode.DECK_INVALID,
                "source transport must bind every loaded physical slot exactly once",
            )
        deck.source_transport = sources
        deck.state = (
            DeckState.PLAYING if any(source.playing for source in sources) else DeckState.PAUSED
        )
        return deck

    def _deck_seed_set(self, payload: Mapping[str, object]) -> _DeckSession:
        deck = self._require_deck(payload)
        deck.seed = int(payload["seed"])
        return deck

    def _deck_reset(self, payload: Mapping[str, object]) -> _DeckSession:
        adapter, _descriptor = self._ready_codec()
        deck = self._require_deck(payload)
        generation = int(payload["new_stream_generation"])
        if generation <= deck.stream_generation:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_INVALID_MESSAGE, "stream generation must increase"
            )
        capture = deck.capture
        if capture is not None and capture.state in {
            CaptureState.CAPTURING,
            CaptureState.FINALIZING,
        }:
            if capture.reset_events >= capture.maximum_reset_events:
                raise WorkerRuntimeError(
                    ErrorCode.CAPTURE_LIMIT_EXCEEDED, "capture reset-event bound was reached"
                )
            capture.reset_events += 1
            capture.pending_reset_event = {
                "generation": generation,
                "sequence": deck.sequence,
            }
        elif capture is not None and capture.state in {
            CaptureState.COMPLETED,
            CaptureState.ABORTED,
            CaptureState.FAULTED,
        }:
            deck.capture = None
            self._capture_state = CaptureState.IDLE
        elif capture is not None:
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE,
                "Deck reset cannot clear a nonterminal capture state",
            )
        ring = self._decoded_ring()
        if ring is None:
            raise WorkerRuntimeError(
                ErrorCode.STATE_BUSY, "exactly one decoded RGBA ring must be configured"
            )
        adapter.reset_decoder(generation)
        self._ring_transport.set_generation(ring.ring_id, generation)
        deck.stream_generation = generation
        deck.sequence = 0
        if not bool(payload["preserve_playheads"]):
            deck.playheads = [0] * len(deck.sources)
        deck.previous_sources = [None] * len(deck.sources)
        deck.state = (
            DeckState.CAPTURING
            if deck.capture is not None
            and deck.capture.state in {CaptureState.CAPTURING, CaptureState.FINALIZING}
            else DeckState.READY
        )
        return deck

    def _deck_restart(self, payload: Mapping[str, object]) -> _DeckSession:
        deck = self._require_deck(payload)
        if deck.capture is not None and deck.capture.state in {
            CaptureState.CAPTURING,
            CaptureState.FINALIZING,
        }:
            raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "capture prevents Deck restart")
        adapter, _descriptor = self._ready_codec()
        deck.deck_revision += 1
        deck.stream_generation += 1
        ring = self._decoded_ring()
        if ring is None:
            raise WorkerRuntimeError(
                ErrorCode.STATE_BUSY, "exactly one decoded RGBA ring must be configured"
            )
        adapter.reset_decoder(deck.stream_generation)
        self._ring_transport.set_generation(ring.ring_id, deck.stream_generation)
        deck.sequence = 0
        deck.playheads = [0] * len(deck.sources)
        deck.previous_sources = [None] * len(deck.sources)
        deck.state = DeckState.READY
        return deck

    def _capture_start(self, payload: Mapping[str, object]) -> _CaptureSession:
        adapter, _descriptor = self._ready_codec()
        deck = self._require_deck(payload)
        if deck.capture is not None and deck.capture.state in {
            CaptureState.STARTING,
            CaptureState.CAPTURING,
            CaptureState.FINALIZING,
        }:
            raise WorkerRuntimeError(ErrorCode.CAPTURE_INVALID_STATE, "capture is already active")
        request = CaptureRequest(
            capture_id=uuid.UUID(str(payload["capture_id"])),
            mode=str(payload["mode"]),
            staging_root=str(payload["staging_root"]),
            maximum_latent_slots=int(payload["maximum_latent_slots"]),
            maximum_visual_bytes=int(payload["maximum_visual_bytes"]),
            maximum_reset_events=int(payload["maximum_reset_events"]),
        )
        request.validate()
        capture = _CaptureSession(
            capture_id=request.capture_id,
            mode=request.mode,
            maximum_latent_slots=request.maximum_latent_slots,
            maximum_visual_bytes=request.maximum_visual_bytes,
            maximum_reset_events=request.maximum_reset_events,
            writer=adapter.create_capture_writer(request),
        )
        deck.capture = capture
        deck.state = DeckState.CAPTURING
        self._capture_state = CaptureState.CAPTURING
        return capture

    def _capture_stop(self, payload: Mapping[str, object]) -> _CaptureSession | CapturePayload:
        deck, capture = self._require_capture(payload)
        if capture.payload is not None:
            return capture.payload
        if capture.mode != "live_capture":
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE,
                "snapshot capture completes automatically at its first valid boundary",
            )
        if capture.state not in {CaptureState.CAPTURING, CaptureState.FINALIZING}:
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE, "capture cannot be stopped from this state"
            )
        capture.state = CaptureState.FINALIZING
        self._capture_state = CaptureState.FINALIZING
        if capture.latent_slots > 0 and self._capture_try_finish(deck, capture):
            assert capture.payload is not None
            return capture.payload
        return capture

    def _capture_status(self, payload: Mapping[str, object]) -> _CaptureSession:
        _deck, capture = self._require_capture(payload)
        return capture

    def _capture_append(self, deck: _DeckSession, tensor: object) -> None:
        capture = deck.capture
        if capture is None or capture.state not in {
            CaptureState.CAPTURING,
            CaptureState.FINALIZING,
        }:
            return
        if capture.latent_slots >= capture.maximum_latent_slots:
            self._capture_fault(deck, capture)
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_LIMIT_EXCEEDED, "capture latent-slot bound was reached"
            )
        try:
            capture.writer.append(tensor, reset_event=capture.pending_reset_event)
        except Exception as error:
            self._capture_fault(deck, capture)
            details = {"extension_code": error.code} if isinstance(error, CodecSdkError) else None
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE,
                "capture writer rejected the post-operator latent slot",
                details=details,
            ) from error
        capture.latent_slots += 1
        self._metrics["capture_slots_total"] += 1
        capture.pending_reset_event = None
        if capture.mode == "snapshot" or capture.state is CaptureState.FINALIZING:
            self._capture_try_finish(deck, capture)

    def _capture_try_finish(self, deck: _DeckSession, capture: _CaptureSession) -> bool:
        try:
            completed = capture.writer.finish()
            if not isinstance(completed, CapturePayload):
                raise CodecSdkError(
                    "capture.payload", "capture writer returned an invalid payload object"
                )
            completed.validate()
            _validate_capture_payload(capture, completed)
        except CodecSdkError as error:
            if error.code == ErrorCode.CAPTURE_NOT_READY.value:
                return False
            self._capture_fault(deck, capture)
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE,
                "capture writer failed while finalizing its staged payload",
                details={"extension_code": error.code},
            ) from error
        except WorkerRuntimeError:
            self._capture_fault(deck, capture)
            raise
        except Exception as error:
            self._capture_fault(deck, capture)
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE,
                "capture writer failed while finalizing its staged payload",
            ) from error
        capture.payload = completed
        capture.state = CaptureState.COMPLETED
        self._capture_state = CaptureState.COMPLETED
        deck.state = (
            DeckState.PLAYING
            if any(source.playing for source in deck.source_transport)
            else DeckState.PAUSED
        )
        return True

    def _capture_fault(self, deck: _DeckSession, capture: _CaptureSession) -> None:
        with suppress(Exception):
            capture.writer.abort()
        capture.payload = None
        capture.pending_reset_event = None
        capture.state = CaptureState.FAULTED
        self._capture_state = CaptureState.FAULTED
        deck.state = (
            DeckState.PLAYING
            if any(source.playing for source in deck.source_transport)
            else DeckState.PAUSED
        )

    def _require_capture(
        self, payload: Mapping[str, object]
    ) -> tuple[_DeckSession, _CaptureSession]:
        deck = self._require_deck(payload)
        capture = deck.capture
        if capture is None or capture.capture_id != uuid.UUID(str(payload["capture_id"])):
            raise WorkerRuntimeError(
                ErrorCode.CAPTURE_INVALID_STATE, "capture identity is not active"
            )
        return deck, capture

    def _require_player(
        self, payload: Mapping[str, object], *, generation_field: str | None = "stream_generation"
    ) -> _PlayerSession:
        player = self._player
        if player is None or player.session_id != uuid.UUID(str(payload["player_session_id"])):
            raise WorkerRuntimeError(ErrorCode.SOURCE_NOT_LOADED, "player session is not open")
        if (
            generation_field is not None
            and int(payload[generation_field]) != player.stream_generation
        ):
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "player generation is stale")
        return player

    def _require_deck(
        self, payload: Mapping[str, object], *, generation_field: str | None = None
    ) -> _DeckSession:
        deck = self._deck
        if deck is None or deck.session_id != uuid.UUID(str(payload["deck_session_id"])):
            raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck session is not loaded")
        if int(payload["deck_revision"]) != deck.deck_revision:
            raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck revision is stale")
        if (
            generation_field is not None
            and int(payload[generation_field]) != deck.stream_generation
        ):
            raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck generation is stale")
        return deck

    def _open_source(self, source: _SourceBinding, capability: Capability) -> ProfileReceipt:
        adapter, _descriptor = self._ready_codec()
        bound = self._require_bound(source.source_id)
        if (
            bound.access.cartridge_id != source.cartridge_id
            or bound.access.archive_sha256 != source.archive_sha256
            or bound.receipt is None
            or bound.receipt.receipt_id != source.profile_receipt_id
            or capability not in bound.receipt.capabilities
        ):
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "source binding is not exact")
        if bound.handle is None:
            bound.handle = adapter.open_source(bound.access, bound.receipt, source.source_id)
            _validate_source_handle(bound.handle, source.source_id)
        return bound.receipt

    def _source_handle_and_receipt(
        self, bound: _BoundCartridge, source: _SourceBinding
    ) -> tuple[SourceHandle, ProfileReceipt]:
        if bound.handle is None or bound.receipt is None:
            raise WorkerRuntimeError(ErrorCode.SOURCE_NOT_LOADED, "source handle is not open")
        if bound.receipt.receipt_id != source.profile_receipt_id:
            raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "source receipt changed")
        return bound.handle, bound.receipt

    def _require_bound(self, source_id: uuid.UUID) -> _BoundCartridge:
        bound = self._bound.get(source_id)
        if bound is None:
            raise WorkerRuntimeError(ErrorCode.SOURCE_NOT_LOADED, "source handle is not bound")
        return bound

    def _ready_codec(self) -> tuple[CodecAdapter, CodecDescriptor]:
        adapter, descriptor = self._selected_codec()
        if self._codec_state is not CodecState.READY:
            raise WorkerRuntimeError(ErrorCode.CODEC_NOT_LOADED, "codec is not ready")
        return adapter, descriptor

    def _loaded_codec_request(self) -> CodecLoadRequest:
        request = self._codec_load_request
        if request is None or self._codec_state is not CodecState.READY:
            raise WorkerRuntimeError(
                ErrorCode.CODEC_NOT_LOADED,
                "exact codec load binding is not retained",
            )
        return request

    def _selected_codec(self) -> tuple[CodecAdapter, CodecDescriptor]:
        if self._adapter is None or self._descriptor is None:
            raise WorkerRuntimeError(
                ErrorCode.CODEC_NOT_LOADED,
                "codec descriptor is not selected",
            )
        return self._adapter, self._descriptor

    def _admit_worker_session(self, session_id: uuid.UUID) -> None:
        sessions = {
            session.session_id for session in (self._player, self._deck) if session is not None
        }
        if session_id in sessions:
            raise WorkerRuntimeError(ErrorCode.STATE_BUSY, "output session is already open")
        if sessions:
            raise WorkerRuntimeError(
                ErrorCode.STATE_BUSY,
                "isolated worker already owns one warm session",
            )

    def _source_is_active(self, source_id: uuid.UUID) -> bool:
        if self._player is not None and self._player.source.source_id == source_id:
            return True
        return self._deck is not None and any(
            source.source_id == source_id for source in self._deck.sources
        )

    def _decoded_ring(self) -> _RingBinding | None:
        rings = [ring for ring in self._rings.values() if ring.kind == "decoded_rgba"]
        return rings[0] if len(rings) == 1 else None

    def _publish(
        self,
        session_id: uuid.UUID,
        stream_generation: int,
        sequence: int,
        batch: DecodedBatch,
    ) -> tuple[uuid.UUID, int]:
        ring = self._decoded_ring()
        if ring is None:
            raise WorkerRuntimeError(
                ErrorCode.STATE_BUSY, "exactly one decoded RGBA ring must be configured"
            )
        if batch.pixels.nbytes > ring.slot_bytes:
            raise WorkerRuntimeError(
                ErrorCode.PROTOCOL_BOUND_EXCEEDED, "decoded output exceeds the shared ring slot"
            )
        slot_sequence = self._ring_transport.publish(
            ring_id=ring.ring_id,
            session_id=session_id,
            stream_generation=stream_generation,
            sequence=sequence,
            batch=batch,
        )
        if (
            isinstance(slot_sequence, bool)
            or not isinstance(slot_sequence, int)
            or slot_sequence <= 0
        ):
            raise WorkerRuntimeError(
                ErrorCode.WORKER_INTERNAL, "shared ring returned an invalid slot sequence"
            )
        return ring.ring_id, slot_sequence

    def _shutdown_worker(self, _payload: Mapping[str, object]) -> None:
        self._session_state = SessionState.STOPPING
        if self._deck is not None and self._deck.capture is not None:
            capture = self._deck.capture
            if capture.state not in {
                CaptureState.COMPLETED,
                CaptureState.ABORTED,
                CaptureState.FAULTED,
            }:
                with suppress(Exception):
                    capture.writer.abort()
                capture.state = CaptureState.ABORTED
                self._capture_state = CaptureState.ABORTED
        self._abort_all_raw_imports()
        self._close_handles()
        for bound in tuple(self._bound.values()):
            self._cartridge_access_factory.close(bound.access)
        self._bound.clear()
        for ring_id in tuple(self._rings):
            self._ring_transport.release(ring_id)
        self._rings.clear()
        self._shutdown = True
        self._session_state = SessionState.STOPPED

    def _abort_all_raw_imports(self) -> None:
        adapter = self._adapter
        for import_id in tuple(self._raw_imports):
            if isinstance(adapter, RawImportAdapter):
                with suppress(Exception):
                    adapter.abort_raw_import(import_id)
            admitted = self._raw_import_artifacts.pop(import_id, None)
            if admitted is not None:
                _cleanup_admitted_raw_import_artifact(*admitted)
        self._raw_imports.clear()

    def _close_handles(self) -> None:
        for bound in self._bound.values():
            if bound.handle is not None:
                try:
                    bound.handle.close()
                finally:
                    bound.handle = None

    def _remember_result(self, message_id: uuid.UUID, result: CommandResult) -> None:
        if len(self._command_results) >= MAX_RESULT_RECORDS:
            oldest = next(iter(self._command_results))
            self._command_results.pop(oldest)
        self._command_results[message_id] = result

    def _reply(self, message: Mapping[str, object]) -> dict[str, object]:
        envelope = {
            "protocol": PROTOCOL,
            "protocol_version": PROTOCOL_VERSION,
            "session_id": str(self._session_id),
            "sequence": self._outgoing_sequence,
            "message_id": str(uuid.uuid4()),
            "sender_uptime_ns": time.monotonic_ns() - self._started_ns,
            "message": dict(message),
        }
        self._outgoing_sequence += 1
        return validate_envelope(envelope)

    def _error_reply(
        self, reply_to: uuid.UUID, name: str, error: WorkerRuntimeError
    ) -> dict[str, object]:
        details = [{"key": key, "value": value} for key, value in error.details.items()]
        return self._reply(
            {
                "kind": "error",
                "body": {
                    "reply_to": str(reply_to),
                    "name": name,
                    "error": {
                        "code": error.code.value,
                        "message": error.detail,
                        "retryable": error.retryable,
                        "fatal": error.fatal,
                        "status": self.status(),
                        "diagnostic_id": str(uuid.uuid4()),
                        "details": details,
                    },
                },
            }
        )


def _profile_key_wire(value: ProfileKey) -> dict[str, str]:
    return {
        "codec_family": value.codec_family,
        "profile": value.profile,
        "profile_version": value.profile_version,
    }


def _signal_geometry_wire(value: object) -> dict[str, object]:
    return {
        "channels": value.channels,
        "latent_height": value.latent_height,
        "latent_width": value.latent_width,
        "decoded_height": value.decoded_height,
        "decoded_width": value.decoded_width,
        "frame_rate_numerator": value.frame_rate_numerator,
        "frame_rate_denominator": value.frame_rate_denominator,
        "timing_contract": value.timing_contract,
        "timing_contract_version": value.timing_contract_version,
    }


def _descriptor_wire(value: CodecDescriptor) -> dict[str, object]:
    return {
        "pack_id": value.pack_id,
        "pack_version": value.pack_version,
        "adapter_id": value.adapter_id,
        "adapter_version": value.adapter_version,
        "host_api_version": value.host_api_version,
        "capabilities": [capability.value for capability in value.capabilities],
        "profiles": [_profile_key_wire(profile) for profile in value.profiles],
    }


def _inspection_wire(source_id: uuid.UUID, value: ProfileInspection) -> dict[str, object]:
    return {
        "source_id": str(source_id),
        "cartridge_id": str(value.cartridge_id),
        "archive_sha256": value.archive_sha256,
        "payload_sha256": value.payload_sha256,
        "profile_key": _profile_key_wire(value.profile_key),
        "signal_geometry": _signal_geometry_wire(value.signal_geometry),
    }


def _receipt_wire(value: ProfileReceipt) -> dict[str, object]:
    python_major, python_minor = (int(part) for part in value.tensor_abi.python_version.split("."))
    return {
        "receipt_id": str(value.receipt_id),
        "cartridge_id": str(value.cartridge_id),
        "archive_sha256": value.archive_sha256,
        "payload_sha256": value.payload_sha256,
        "pack_id": value.pack_id,
        "pack_version": value.pack_version,
        "adapter_id": value.adapter_id,
        "adapter_version": value.adapter_version,
        "profile_key": _profile_key_wire(value.profile_key),
        "signal_geometry": _signal_geometry_wire(value.signal_geometry),
        "tensor_abi": {
            "python_major": python_major,
            "python_minor": python_minor,
            "torch_version": value.tensor_abi.torch_version,
            "dtype": value.tensor_abi.dtype,
            "shape": list(value.tensor_abi.shape),
            "contiguous": value.tensor_abi.contiguous,
            "device": value.tensor_abi.device,
        },
        "decoded_abi": {
            "pixel_format": value.decoded_abi.pixel_format,
            "maximum_batch": value.decoded_abi.maximum_batch,
        },
        "capabilities": [capability.value for capability in value.capabilities],
        "estimated_host_bytes": value.estimated_host_bytes,
        "estimated_device_bytes": value.estimated_device_bytes,
    }


def _raw_import_metadata_wire(value: object) -> dict[str, object]:
    return {
        "profile_key": _profile_key_wire(value.profile_key),
        "payload_entry": value.payload_entry,
        "payload_media_type": value.payload_media_type,
        "tensors": [
            {
                "stream": tensor.stream,
                "name": tensor.name,
                "storage_dtype": tensor.storage_dtype,
                "runtime_dtype": tensor.runtime_dtype,
                "shape": list(tensor.shape),
            }
            for tensor in value.tensors
        ],
        "timing_contract": value.timing_contract,
        "timing_contract_version": value.timing_contract_version,
        "decoded_width": value.decoded_width,
        "decoded_height": value.decoded_height,
        "decoded_frame_count": value.decoded_frame_count,
        "frame_rate_numerator": value.frame_rate_numerator,
        "frame_rate_denominator": value.frame_rate_denominator,
        "duration_numerator": value.duration_numerator,
        "duration_denominator": value.duration_denominator,
        "audio_policy": value.audio_policy,
    }


def _raw_import_preflight_wire(value: RawImportPreflight) -> dict[str, object]:
    return {
        "receipt_id": str(value.receipt_id),
        "import_id": str(value.import_id),
        "pack_id": value.pack_id,
        "pack_version": value.pack_version,
        "adapter_id": value.adapter_id,
        "adapter_version": value.adapter_version,
        "source_sha256": value.source_sha256,
        "source_byte_length": value.source_byte_length,
        "metadata": _raw_import_metadata_wire(value.metadata),
    }


def _raw_import_artifact_wire(value: RawImportArtifact) -> dict[str, object]:
    return {
        "receipt_id": str(value.receipt_id),
        "import_id": str(value.import_id),
        "staged_payload_path": value.staged_payload_path,
        "payload_sha256": value.payload_sha256,
        "payload_byte_length": value.payload_byte_length,
    }


def _admit_staged_raw_import_path(artifact: RawImportArtifact, staging_root: Path) -> Path:
    raw_path = Path(artifact.staged_payload_path)
    try:
        if raw_path.is_symlink():
            raise OSError("staged payload is a link")
        staged = raw_path.resolve(strict=True)
        if staged.parent != staging_root or not staged.is_file():
            raise OSError("staged payload escaped the Core root")
        before = staged.stat()
        if before.st_size != artifact.payload_byte_length:
            raise OSError("staged payload length changed")
        hasher = hashlib.sha256()
        measured = 0
        with staged.open("rb") as stream:
            while chunk := stream.read(64 * 1024):
                measured += len(chunk)
                if measured > artifact.payload_byte_length:
                    raise OSError("staged payload exceeded its receipt")
                hasher.update(chunk)
        after = staged.stat()
        if (
            measured != artifact.payload_byte_length
            or hasher.hexdigest() != artifact.payload_sha256
            or before.st_size != after.st_size
            or before.st_mtime_ns != after.st_mtime_ns
        ):
            raise OSError("staged payload identity changed")
    except OSError as error:
        raise WorkerRuntimeError(
            ErrorCode.SOURCE_INVALID, "staged raw import is outside the retained Core root"
        ) from error
    return staged


def _cleanup_admitted_raw_import_artifact(artifact: RawImportArtifact, staging_root: Path) -> None:
    with suppress(Exception):
        staged = _admit_staged_raw_import_path(artifact, staging_root)
        staged.unlink()


def _control_value_wire(value: object) -> dict[str, object]:
    if isinstance(value, bool):
        kind = "boolean"
    elif isinstance(value, int):
        kind = "integer"
    elif isinstance(value, float):
        kind = "number"
    elif isinstance(value, str):
        kind = "text"
    else:
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "control value is not scalar")
    return {"kind": kind, "value": value}


def _provenance_wire(value: Mapping[str, object]) -> list[dict[str, object]]:
    entries: list[dict[str, object]] = []
    for key, item in sorted(value.items()):
        if not isinstance(item, bool | int | float | str):
            continue
        entries.append(
            {"key": _identifier(key, "provenance key"), "value": _control_value_wire(item)}
        )
        if len(entries) == MAX_CONTROLS:
            break
    return entries


def _codec_registry(
    entries: tuple[TrustedCodecEntrypoint, ...],
) -> dict[tuple[str, str, str], TrustedCodecEntrypoint]:
    registry: dict[tuple[str, str, str], TrustedCodecEntrypoint] = {}
    for entry in entries:
        entry.validate()
        key = (entry.pack_id, entry.pack_version, entry.adapter_id)
        if key in registry:
            raise ValueError("trusted codec identity is duplicated")
        registry[key] = entry
    return registry


def _deck_registry(
    entries: tuple[TrustedDeckEntrypoint, ...],
) -> dict[tuple[str, str, str, str], TrustedDeckEntrypoint]:
    registry: dict[tuple[str, str, str, str], TrustedDeckEntrypoint] = {}
    for entry in entries:
        entry.validate()
        key = (entry.deck_id, entry.deck_version, entry.operator_id, entry.operator_version)
        if key in registry:
            raise ValueError("trusted Deck identity is duplicated")
        registry[key] = entry
    return registry


def _load_adapter(entry: TrustedCodecEntrypoint) -> CodecAdapter:
    value = _load_entrypoint(entry.entrypoint, ErrorCode.CODEC_UNTRUSTED)
    adapter = value() if callable(value) and not isinstance(value, CodecAdapter) else value
    if not isinstance(adapter, CodecAdapter):
        raise WorkerRuntimeError(ErrorCode.CODEC_UNTRUSTED, "codec entrypoint is not an adapter")
    return adapter


def _load_operator(entry: TrustedDeckEntrypoint) -> Callable[..., DeckOperatorResult]:
    value = _load_entrypoint(entry.entrypoint, ErrorCode.DECK_INVALID)
    if not callable(value):
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck entrypoint is not callable")
    return value


def _load_bound_operator(
    runtime: Mapping[str, object], identity: tuple[str, str, str, str]
) -> Callable[..., DeckOperatorResult]:
    bound_identity = (
        str(runtime["deck_id"]),
        str(runtime["deck_version"]),
        str(runtime["operator_id"]),
        str(runtime["operator_version"]),
    )
    if bound_identity != identity:
        raise WorkerRuntimeError(
            ErrorCode.DECK_INVALID, "Deck runtime identity does not match deck.load"
        )
    _sha256(str(runtime["package_manifest_sha256"]), "package_manifest_sha256")
    _sha256(str(runtime["integrity_catalog_sha256"]), "integrity_catalog_sha256")
    entrypoint = _entrypoint(str(runtime["entrypoint"]))
    root_text = str(runtime["python_root"])
    if not root_text or len(root_text.encode()) > MAX_PATH_BYTES or not os.path.isabs(root_text):
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck Python root is not absolute")
    try:
        python_root = Path(root_text).resolve(strict=True)
    except OSError as error:
        raise WorkerRuntimeError(
            ErrorCode.DECK_INVALID, "Deck Python root cannot be resolved"
        ) from error
    if not python_root.is_dir():
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck Python root is not a directory")
    value = _load_entrypoint_from_root(python_root, entrypoint)
    if not callable(value):
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "Deck entrypoint is not callable")
    return value


def _load_entrypoint_from_root(python_root: Path, entrypoint: str) -> object:
    module_name, attribute = entrypoint.split(":", maxsplit=1)
    module_names = tuple(
        ".".join(module_name.split(".")[:index])
        for index in range(1, len(module_name.split(".")) + 1)
    )
    for name in module_names:
        loaded = sys.modules.get(name)
        if loaded is not None and not _module_belongs_to_root(loaded, python_root):
            raise WorkerRuntimeError(
                ErrorCode.DECK_INVALID, "Deck module name collides with another runtime"
            )

    root_text = str(python_root)
    sys.path.insert(0, root_text)
    importlib.invalidate_caches()
    try:
        module = importlib.import_module(module_name)
        value = getattr(module, attribute)
    except Exception as error:
        raise WorkerRuntimeError(
            ErrorCode.DECK_INVALID, "bound Deck entrypoint cannot be loaded"
        ) from error
    finally:
        with suppress(ValueError):
            sys.path.remove(root_text)

    for name in module_names:
        loaded = sys.modules.get(name)
        if loaded is None or not _module_belongs_to_root(loaded, python_root):
            raise WorkerRuntimeError(
                ErrorCode.DECK_INVALID, "Deck module resolved outside its bound Python root"
            )
    return value


def _module_belongs_to_root(module: object, python_root: Path) -> bool:
    locations: list[Path] = []
    module_file = getattr(module, "__file__", None)
    if isinstance(module_file, str):
        locations.append(Path(module_file))
    module_paths = getattr(module, "__path__", None)
    if module_paths is not None:
        locations.extend(Path(path) for path in module_paths if isinstance(path, str))
    if not locations:
        return False
    try:
        return all(
            location.resolve(strict=True).is_relative_to(python_root) for location in locations
        )
    except OSError:
        return False


def _load_entrypoint(entrypoint: str, error_code: ErrorCode) -> object:
    module_name, attribute = entrypoint.split(":", maxsplit=1)
    try:
        module = importlib.import_module(module_name)
        return getattr(module, attribute)
    except (ImportError, AttributeError) as error:
        raise WorkerRuntimeError(error_code, "trusted entrypoint cannot be loaded") from error


def _validate_cartridge_access(access: CartridgeAccess) -> None:
    if not isinstance(access.cartridge_id, uuid.UUID) or access.cartridge_id.int == 0:
        raise ValueError("CartridgeAccess.cartridge_id must be a non-nil UUID")
    _sha256(access.archive_sha256, "archive_sha256")
    if not isinstance(access.manifest, Mapping):
        raise ValueError("CartridgeAccess.manifest must be a mapping")


def _validate_source_handle(handle: SourceHandle, source_id: uuid.UUID) -> None:
    if not isinstance(handle, SourceHandle):
        raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "adapter returned an invalid handle")
    if (
        handle.source_id != source_id
        or isinstance(handle.slot_count, bool)
        or handle.slot_count <= 0
    ):
        raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "source handle identity is invalid")


def _profile_key(raw: object) -> ProfileKey:
    value = _mapping(raw, "profile key")
    key = ProfileKey(
        codec_family=str(value["codec_family"]),
        profile=str(value["profile"]),
        profile_version=str(value["profile_version"]),
    )
    key.validate()
    return key


def _source_binding(raw: object) -> _SourceBinding:
    value = _mapping(raw, "source binding")
    return _SourceBinding(
        physical_slot=int(value["physical_slot"]),
        source_id=uuid.UUID(str(value["source_id"])),
        cartridge_id=uuid.UUID(str(value["cartridge_id"])),
        archive_sha256=_sha256(str(value["archive_sha256"]), "archive_sha256"),
        profile_receipt_id=uuid.UUID(str(value["profile_receipt_id"])),
        loop_enabled=bool(value["loop_enabled"]),
    )


def _source_transport_binding(raw: object) -> _SourceTransport:
    value = _mapping(raw, "source transport binding")
    physical_slot = int(value["physical_slot"])
    playing = value["playing"]
    loop_enabled = value["loop_enabled"]
    if (
        not 1 <= physical_slot <= MAX_SOURCES
        or not isinstance(playing, bool)
        or not isinstance(loop_enabled, bool)
    ):
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "source transport binding is invalid")
    return _SourceTransport(physical_slot, playing, loop_enabled)


def _controls(raw: object) -> dict[str, object]:
    if not isinstance(raw, list) or len(raw) > MAX_CONTROLS:
        raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "control bound was exceeded")
    controls: dict[str, object] = {}
    for item in raw:
        value = _mapping(item, "control")
        name = _identifier(str(value["name"]), "control name")
        wire_value = _mapping(value["value"], "control value")
        scalar = wire_value["value"]
        if isinstance(scalar, float) and not math.isfinite(scalar):
            raise WorkerRuntimeError(ErrorCode.DECK_INVALID, "control must be finite")
        controls[name] = scalar
    return controls


def _crosscheck_receipt(receipt: ProfileReceipt, inspection: ProfileInspection) -> None:
    if (
        receipt.cartridge_id != inspection.cartridge_id
        or receipt.archive_sha256 != inspection.archive_sha256
        or receipt.payload_sha256 != inspection.payload_sha256
        or receipt.profile_key != inspection.profile_key
        or receipt.signal_geometry != inspection.signal_geometry
    ):
        raise WorkerRuntimeError(
            ErrorCode.PROFILE_INVALID, "profile receipt does not bind the exact inspection"
        )


def _compatible_receipts(receipts: tuple[ProfileReceipt, ...]) -> None:
    reference = receipts[0]
    for receipt in receipts[1:]:
        if (
            receipt.profile_key != reference.profile_key
            or receipt.signal_geometry != reference.signal_geometry
            or receipt.tensor_abi != reference.tensor_abi
        ):
            raise WorkerRuntimeError(
                ErrorCode.DECK_INCOMPATIBLE,
                "Deck sources have incompatible profile, signal, timing, or tensor ABI",
            )


def _validate_capture_payload(capture: _CaptureSession, payload: CapturePayload) -> None:
    if (
        payload.capture_id != capture.capture_id
        or payload.latent_slots != capture.latent_slots
        or payload.latent_slots > capture.maximum_latent_slots
        or payload.payload_byte_length > capture.maximum_visual_bytes
    ):
        raise WorkerRuntimeError(
            ErrorCode.CAPTURE_LIMIT_EXCEEDED,
            "capture artifact identity or declared bounds are invalid",
        )


def _device_matches_codec_load(
    device: object, expected_device: str, expected_ordinal: int
) -> bool:
    device_type = getattr(device, "type", None)
    device_index = getattr(device, "index", None)
    if device_type != expected_device:
        return False
    if expected_device == "cpu":
        return expected_ordinal == 0 and device_index in {None, 0}
    return expected_device == "cuda" and device_index == expected_ordinal


def _validate_tensor(
    tensor: object, receipt: ProfileReceipt, load_request: CodecLoadRequest
) -> None:
    try:
        torch = importlib.import_module("torch")
    except ModuleNotFoundError as error:
        raise WorkerRuntimeError(
            ErrorCode.PROFILE_INVALID, "declared Torch runtime is missing"
        ) from error
    if torch.__version__ != receipt.tensor_abi.torch_version:
        raise WorkerRuntimeError(ErrorCode.PROFILE_INVALID, "Torch build does not match receipt")
    if not torch.is_tensor(tensor):
        raise WorkerRuntimeError(ErrorCode.SOURCE_INVALID, "adapter returned a non-tensor slot")
    dtype = str(tensor.dtype).removeprefix("torch.")
    if (
        tuple(tensor.shape) != receipt.tensor_abi.shape
        or dtype != receipt.tensor_abi.dtype
        or receipt.tensor_abi.device != load_request.device
        or not _device_matches_codec_load(
            tensor.device,
            load_request.device,
            load_request.device_ordinal,
        )
        or not tensor.is_contiguous()
        or not bool(torch.isfinite(tensor).all().item())
    ):
        raise WorkerRuntimeError(
            ErrorCode.SOURCE_INVALID,
            "adapter tensor violates exact shape, dtype, device, contiguity, or finite ABI",
        )


def _process_receipt(
    session_id: uuid.UUID,
    generation: int,
    sequence: int,
    output_ring_id: uuid.UUID,
    output_slot_sequence: int,
    tensor: object,
    batch: DecodedBatch,
    provenance: Mapping[str, object],
) -> ProcessReceipt:
    return ProcessReceipt(
        session_id=session_id,
        stream_generation=generation,
        sequence=sequence,
        output_ring_id=output_ring_id,
        output_slot_sequence=output_slot_sequence,
        latent_shape=tuple(tensor.shape),
        latent_dtype=str(tensor.dtype).removeprefix("torch."),
        latent_device=str(tensor.device),
        decoded_shape=(batch.batch, batch.height, batch.width, 4),
        provenance=dict(provenance),
    )


def _mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise WorkerRuntimeError(ErrorCode.PROTOCOL_INVALID_MESSAGE, f"{label} must be an object")
    return value


def _identifier(value: str, field: str) -> str:
    if not value or len(value.encode()) > 128:
        raise ValueError(f"{field} is outside its bound")
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:-")
    if any(character not in allowed for character in value):
        raise ValueError(f"{field} contains an invalid character")
    return value


def _version(value: str, field: str) -> str:
    if not value or len(value.encode()) > 64:
        raise ValueError(f"{field} is outside its bound")
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.+-_")
    if any(character not in allowed for character in value):
        raise ValueError(f"{field} contains an invalid character")
    return value


def _entrypoint(value: str) -> str:
    if not value or len(value.encode()) > MAX_ENTRYPOINT_BYTES or value.count(":") != 1:
        raise ValueError("entrypoint is outside its bound")
    module, attribute = value.split(":")
    if (
        not module
        or not attribute
        or any(not part.isidentifier() for part in (*module.split("."), attribute))
    ):
        raise ValueError("entrypoint is invalid")
    return value


def _sha256(value: str, field: str) -> str:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{field} is not a canonical SHA-256")
    return value


def _bounded_error_text(value: str) -> str:
    if not isinstance(value, str) or not value:
        return "runtime operation failed"
    value = value.replace("\0", "")
    encoded = value.encode()
    if len(encoded) <= MAX_ERROR_MESSAGE_BYTES:
        return value
    return encoded[:MAX_ERROR_MESSAGE_BYTES].decode(errors="ignore") or "runtime operation failed"


def _bounded_details(values: Mapping[str, str]) -> dict[str, str]:
    bounded: dict[str, str] = {}
    for key, value in list(values.items())[:MAX_ERROR_DETAILS]:
        try:
            safe_key = _identifier(str(key), "error detail key")
        except ValueError:
            continue
        bounded[safe_key] = _bounded_error_text(str(value))
    return bounded


def _sdk_error_code(command_name: str) -> ErrorCode:
    if command_name.startswith("profile."):
        return ErrorCode.PROFILE_INVALID
    if command_name.startswith("deck."):
        return ErrorCode.DECK_INVALID
    if command_name.startswith("capture."):
        return ErrorCode.CAPTURE_INVALID_STATE
    if command_name.startswith("raw_import."):
        return ErrorCode.SOURCE_INVALID
    if command_name.startswith("player."):
        return ErrorCode.SOURCE_INVALID
    return ErrorCode.CODEC_NOT_LOADED


__all__ = [
    "CartridgeAccessFactory",
    "CommandResult",
    "ProcessReceipt",
    "Protocol2Worker",
    "SharedRingTransport",
    "TrustedCodecEntrypoint",
    "TrustedDeckEntrypoint",
    "WorkerRuntimeError",
]
