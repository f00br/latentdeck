"""Strict JSON and named-MessagePack mirror of Worker Protocol 2."""

from __future__ import annotations

import hmac
import json
import math
import os
import uuid
from collections.abc import Mapping, Sequence
from enum import StrEnum

import msgpack

PROTOCOL = "latentdeck.worker"
PROTOCOL_VERSION = 2
MAX_FRAME_BYTES = 262_144
MAX_DEPTH = 16
MAX_MAP_FIELDS = 64
MAX_ARRAY_ITEMS = 256
MAX_STRING_BYTES = 65_536
MAX_BINARY_BYTES = 64
MAX_PROFILES = 64
MAX_MESSAGES_PER_SESSION = 65_536
MAX_PATH_BYTES = 32_768
MAX_ENTRYPOINT_BYTES = 512
MAX_RAW_IMPORT_SOURCE_BYTES = 64 * 1024**3

WORKER_HELLO_FIELDS = frozenset(
    {
        "auth_token",
        "worker_pid",
        "worker_identity",
        "runtime_identity",
        "protocol_min",
        "protocol_max",
    }
)


class Capability(StrEnum):
    PLAYER = "player"
    REALTIME = "realtime"
    RESAMPLE = "resample"
    SNAPSHOT_CAPTURE = "snapshot_capture"
    LIVE_CAPTURE = "live_capture"
    RAW_IMPORT = "raw_import"


class ErrorCode(StrEnum):
    PROTOCOL_INVALID_MESSAGE = "protocol.invalid_message"
    PROTOCOL_UNSUPPORTED_VERSION = "protocol.unsupported_version"
    PROTOCOL_BOUND_EXCEEDED = "protocol.bound_exceeded"
    SESSION_NOT_CONFIGURED = "session.not_configured"
    SESSION_CAPACITY_EXCEEDED = "session.capacity_exceeded"
    SESSION_OUTPUT_LEASE_BUSY = "session.output_lease_busy"
    SESSION_OUTPUT_LEASE_PINNED = "session.output_lease_pinned"
    CODEC_NOT_LOADED = "codec.not_loaded"
    CODEC_UNTRUSTED = "codec.untrusted"
    CODEC_CAPABILITY_UNSUPPORTED = "codec.capability_unsupported"
    PROFILE_INVALID = "profile.invalid"
    PROFILE_INCOMPATIBLE = "profile.incompatible"
    SOURCE_INVALID = "source.invalid"
    SOURCE_NOT_LOADED = "source.not_loaded"
    DECK_INVALID = "deck.invalid"
    DECK_INCOMPATIBLE = "deck.incompatible"
    CAPTURE_INVALID_STATE = "capture.invalid_state"
    CAPTURE_NOT_READY = "capture.not_ready"
    CAPTURE_LIMIT_EXCEEDED = "capture.limit_exceeded"
    STATE_BUSY = "state.busy"
    WORKER_INTERNAL = "worker.internal"


class SessionState(StrEnum):
    UNCONFIGURED = "unconfigured"
    READY = "ready"
    BUSY = "busy"
    FAULTED = "faulted"
    STOPPING = "stopping"
    STOPPED = "stopped"


class CodecState(StrEnum):
    UNLOADED = "unloaded"
    LOADING = "loading"
    READY = "ready"
    FAULTED = "faulted"


class PlayerState(StrEnum):
    EMPTY = "empty"
    LOADING = "loading"
    READY = "ready"
    PLAYING = "playing"
    PAUSED = "paused"
    END_OF_STREAM = "end_of_stream"
    FAULTED = "faulted"


class DeckState(StrEnum):
    EMPTY = "empty"
    LOADING = "loading"
    READY = "ready"
    PLAYING = "playing"
    PAUSED = "paused"
    CAPTURING = "capturing"
    FAULTED = "faulted"


class CaptureState(StrEnum):
    IDLE = "idle"
    STARTING = "starting"
    CAPTURING = "capturing"
    FINALIZING = "finalizing"
    COMPLETED = "completed"
    ABORTED = "aborted"
    FAULTED = "faulted"


class ProtocolError(ValueError):
    """A Protocol 2 value violated the closed wire contract."""


class WorkerStreamValidator:
    """Require authenticated hello as the exact first ordered worker frame."""

    def __init__(self, session_id: str, expected_auth_token: str) -> None:
        self._session_id = _uuid(session_id, "session_id")
        self._expected_auth_token = bytearray.fromhex(
            _auth_token_text(expected_auth_token, "expected auth token")
        )
        self._next_sequence = 1
        self._message_ids: set[str] = set()
        self._hello_received = False

    def validate(self, raw: Mapping[str, object]) -> dict[str, object]:
        """Validate and consume one worker-originated Protocol 2 envelope."""

        envelope = validate_envelope(raw)
        if envelope["session_id"] != self._session_id:
            raise ProtocolError("worker frame belongs to another session")
        if envelope["sequence"] != self._next_sequence:
            raise ProtocolError("worker sequence is not contiguous")
        message_id = str(envelope["message_id"])
        if message_id in self._message_ids:
            raise ProtocolError("worker message_id was already used")
        if len(self._message_ids) >= MAX_MESSAGES_PER_SESSION:
            raise ProtocolError("worker session message budget is exhausted")

        message = _mapping(envelope["message"], "message")
        body = _mapping(message["body"], "message body")
        is_hello = (
            message["kind"] == "event"
            and _mapping(body["event"], "event")["name"] == "worker.hello"
        )
        if not self._hello_received:
            if not is_hello or body["caused_by"] is not None:
                raise ProtocolError("worker.hello must be the first worker frame")
            hello = _mapping(_mapping(body["event"], "event")["payload"], "worker hello")
            candidate = bytearray.fromhex(str(hello["auth_token"]))
            authenticated = hmac.compare_digest(self._expected_auth_token, candidate)
            candidate[:] = b"\x00" * len(candidate)
            self._expected_auth_token[:] = b"\x00" * len(self._expected_auth_token)
            if not authenticated:
                raise ProtocolError("worker authentication failed")
            self._hello_received = True
        elif is_hello:
            raise ProtocolError("worker.hello is allowed exactly once")

        self._message_ids.add(message_id)
        self._next_sequence += 1
        return envelope


DECK_LOAD_FIELDS = frozenset(
    {
        "deck_session_id",
        "deck_id",
        "deck_version",
        "operator_id",
        "operator_version",
        "sources",
        "roles",
        "controls",
        "seed",
        "stream_generation",
    }
)

DECK_RUNTIME_BINDING_FIELDS = frozenset(
    {
        "deck_id",
        "deck_version",
        "operator_id",
        "operator_version",
        "python_root",
        "entrypoint",
        "package_manifest_sha256",
        "integrity_catalog_sha256",
    }
)
RAW_IMPORT_METADATA_FIELDS = frozenset(
    {
        "profile_key",
        "payload_entry",
        "payload_media_type",
        "tensors",
        "timing_contract",
        "timing_contract_version",
        "decoded_width",
        "decoded_height",
        "decoded_frame_count",
        "frame_rate_numerator",
        "frame_rate_denominator",
        "duration_numerator",
        "duration_denominator",
        "audio_policy",
    }
)
RAW_IMPORT_TENSOR_FIELDS = frozenset({"stream", "name", "storage_dtype", "runtime_dtype", "shape"})


COMMAND_FIELDS: dict[str, frozenset[str]] = {
    "session.configure": frozenset(
        {
            "selected_protocol_version",
            "app_version",
            "heartbeat_interval_ms",
            "heartbeat_hard_timeout_ms",
            "max_frame_bytes",
            "max_inflight_batches",
            "requested_capabilities",
        }
    ),
    "session.status": frozenset(),
    "session.shutdown": frozenset({"reason"}),
    "codec.descriptor": frozenset({"pack_id", "pack_version", "adapter_id"}),
    "codec.load": frozenset(
        {
            "pack_id",
            "pack_version",
            "adapter_id",
            "adapter_version",
            "device",
            "device_ordinal",
            "external_assets",
        }
    ),
    "codec.unload": frozenset({"pack_id", "pack_version"}),
    "source.open": frozenset(
        {
            "source_id",
            "cartridge_id",
            "archive_sha256",
            "archive_bytes",
            "retained_native_handle",
            "integrity_access_receipt",
        }
    ),
    "source.close": frozenset({"source_id"}),
    "ring.configure": frozenset(
        {
            "ring_id",
            "kind",
            "mapping_handle",
            "ready_event_handle",
            "consumed_event_handle",
            "slot_count",
            "slot_bytes",
        }
    ),
    "ring.release": frozenset({"ring_id"}),
    "profile.inspect": frozenset({"source_id", "cartridge_id", "archive_sha256"}),
    "profile.validate": frozenset({"source_id", "expected_profile", "required_capabilities"}),
    "raw_import.preflight": frozenset({"import_id", "source_path", "maximum_source_bytes"}),
    "raw_import.stage": frozenset({"import_id", "receipt_id", "staging_root"}),
    "raw_import.abort": frozenset({"import_id", "receipt_id"}),
    "player.open": frozenset({"player_session_id", "source", "stream_generation"}),
    "player.step": frozenset({"player_session_id", "stream_generation", "maximum_decoded_frames"}),
    "player.reset": frozenset({"player_session_id", "new_stream_generation"}),
    "player.status": frozenset(),
    "deck.load": DECK_LOAD_FIELDS,
    "deck.process": frozenset({"deck_session_id", "deck_revision", "stream_generation"}),
    "deck.controls.set": frozenset({"deck_session_id", "deck_revision", "controls"}),
    "deck.roles.set": frozenset({"deck_session_id", "deck_revision", "roles"}),
    "deck.transport.set": frozenset({"deck_session_id", "deck_revision", "sources"}),
    "deck.seed.set": frozenset({"deck_session_id", "deck_revision", "seed"}),
    "deck.reset": frozenset(
        {
            "deck_session_id",
            "deck_revision",
            "new_stream_generation",
            "preserve_playheads",
        }
    ),
    "deck.restart": frozenset({"deck_session_id", "deck_revision"}),
    "deck.status": frozenset(),
    "capture.start": frozenset(
        {
            "deck_session_id",
            "deck_revision",
            "capture_id",
            "mode",
            "staging_root",
            "maximum_latent_slots",
            "maximum_visual_bytes",
            "maximum_reset_events",
        }
    ),
    "capture.stop": frozenset({"deck_session_id", "deck_revision", "capture_id"}),
    "capture.status": frozenset({"deck_session_id", "deck_revision", "capture_id"}),
    "metrics.get": frozenset(),
}

STATUS_FIELDS = frozenset(
    {
        "session",
        "codec",
        "player",
        "deck",
        "capture",
        "open_session_count",
        "foreground_output_session",
        "output_lease_pinned",
    }
)
PROFILE_KEY_FIELDS = frozenset({"codec_family", "profile", "profile_version"})
SOURCE_FIELDS = frozenset(
    {
        "physical_slot",
        "source_id",
        "cartridge_id",
        "archive_sha256",
        "profile_receipt_id",
        "loop_enabled",
    }
)
SIGNAL_GEOMETRY_FIELDS = frozenset(
    {
        "channels",
        "latent_height",
        "latent_width",
        "decoded_height",
        "decoded_width",
        "frame_rate_numerator",
        "frame_rate_denominator",
        "timing_contract",
        "timing_contract_version",
    }
)
TENSOR_ABI_FIELDS = frozenset(
    {
        "python_major",
        "python_minor",
        "torch_version",
        "dtype",
        "shape",
        "contiguous",
        "device",
    }
)
DECODED_ABI_FIELDS = frozenset({"pixel_format", "maximum_batch"})
PROFILE_RECEIPT_FIELDS = frozenset(
    {
        "receipt_id",
        "cartridge_id",
        "archive_sha256",
        "payload_sha256",
        "pack_id",
        "pack_version",
        "adapter_id",
        "adapter_version",
        "profile_key",
        "signal_geometry",
        "tensor_abi",
        "decoded_abi",
        "capabilities",
        "estimated_host_bytes",
        "estimated_device_bytes",
    }
)
PLAYER_STATUS_FIELDS = frozenset(
    {
        "player_session_id",
        "state",
        "stream_generation",
        "stream_sequence",
        "playhead_slot",
        "end_of_stream",
        "decoded_ring_id",
    }
)
DECK_STATUS_FIELDS = frozenset(
    {
        "deck_session_id",
        "state",
        "deck_revision",
        "stream_generation",
        "stream_sequence",
        "playheads",
        "roles",
        "controls",
        "source_transport",
        "seed",
        "capture_state",
    }
)
CAPTURE_ARTIFACT_FIELDS = frozenset(
    {
        "staged_payload_path",
        "payload_sha256",
        "payload_byte_length",
        "latent_slots",
        "decoded_frame_count",
    }
)
CAPTURE_STATUS_FIELDS = frozenset(
    {
        "deck_session_id",
        "deck_revision",
        "capture_id",
        "state",
        "mode",
        "latent_slots",
        "reset_events",
        "artifact",
    }
)
ACK_FIELDS: dict[str, frozenset[str]] = {
    "session.configure": frozenset(
        {"selected_protocol_version", "maximum_frame_bytes", "accepted_capabilities"}
    ),
    "session.status": STATUS_FIELDS,
    "session.shutdown": frozenset({"reason"}),
    "codec.descriptor": frozenset(
        {
            "pack_id",
            "pack_version",
            "adapter_id",
            "adapter_version",
            "host_api_version",
            "capabilities",
            "profiles",
        }
    ),
    "codec.load": frozenset(
        {
            "pack_id",
            "pack_version",
            "adapter_id",
            "adapter_version",
            "device",
            "device_ordinal",
        }
    ),
    "codec.unload": frozenset({"pack_id", "pack_version"}),
    "source.open": frozenset({"source_id", "cartridge_id", "archive_sha256"}),
    "source.close": frozenset({"source_id"}),
    "ring.configure": frozenset({"ring_id", "kind", "slot_count", "slot_bytes"}),
    "ring.release": frozenset({"ring_id"}),
    "profile.inspect": frozenset(
        {
            "source_id",
            "cartridge_id",
            "archive_sha256",
            "payload_sha256",
            "profile_key",
            "signal_geometry",
        }
    ),
    "profile.validate": PROFILE_RECEIPT_FIELDS,
    "raw_import.preflight": frozenset(
        {
            "receipt_id",
            "import_id",
            "pack_id",
            "pack_version",
            "adapter_id",
            "adapter_version",
            "source_sha256",
            "source_byte_length",
            "metadata",
        }
    ),
    "raw_import.stage": frozenset(
        {
            "receipt_id",
            "import_id",
            "staged_payload_path",
            "payload_sha256",
            "payload_byte_length",
        }
    ),
    "raw_import.abort": frozenset({"receipt_id", "import_id"}),
    "player.open": PLAYER_STATUS_FIELDS,
    "player.step": frozenset(
        {"status", "output_ring_id", "output_slot_sequence", "decoded_frames"}
    ),
    "player.reset": PLAYER_STATUS_FIELDS,
    "player.status": PLAYER_STATUS_FIELDS,
    "deck.load": DECK_STATUS_FIELDS,
    "deck.process": frozenset({"status", "output_ring_id", "output_slot_sequence", "provenance"}),
    "deck.controls.set": DECK_STATUS_FIELDS,
    "deck.roles.set": DECK_STATUS_FIELDS,
    "deck.transport.set": DECK_STATUS_FIELDS,
    "deck.seed.set": DECK_STATUS_FIELDS,
    "deck.reset": DECK_STATUS_FIELDS,
    "deck.restart": DECK_STATUS_FIELDS,
    "deck.status": DECK_STATUS_FIELDS,
    "capture.start": CAPTURE_STATUS_FIELDS,
    "capture.stop": CAPTURE_STATUS_FIELDS,
    "capture.status": CAPTURE_STATUS_FIELDS,
    "metrics.get": frozenset(
        {
            "worker_uptime_ns",
            "commands_total",
            "commands_failed_total",
            "player_steps_total",
            "deck_process_total",
            "capture_slots_total",
            "decoded_frames_total",
        }
    ),
}


def _unique_object(pairs: Sequence[tuple[object, object]]) -> dict[object, object]:
    value: dict[object, object] = {}
    for key, item in pairs:
        if key in value:
            raise ProtocolError("map contains a duplicate key")
        value[key] = item
    return value


def _mapping(value: object, field: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise ProtocolError(f"{field} must be a string-keyed object")
    return value


def _exact(value: Mapping[str, object], fields: frozenset[str], label: str) -> None:
    if set(value) != fields:
        raise ProtocolError(f"{label} fields do not match the closed schema")


def _uuid(value: object, field: str) -> str:
    if not isinstance(value, str):
        raise ProtocolError(f"{field} must be a UUID")
    try:
        parsed = uuid.UUID(value)
    except (ValueError, AttributeError) as error:
        raise ProtocolError(f"{field} must be a UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise ProtocolError(f"{field} must be a canonical non-nil UUID")
    return value


def _integer(value: object, field: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise ProtocolError(f"{field} is outside its integer bound")
    return value


def _text(value: object, field: str, maximum: int = 128) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > maximum or "\0" in value:
        raise ProtocolError(f"{field} is outside its text bound")
    return value


def _canonical_json_object(value: object, field: str, maximum: int) -> Mapping[str, object]:
    text = _text(value, field, maximum)
    try:
        parsed = json.loads(
            text,
            object_pairs_hook=_unique_object,
            parse_constant=lambda _value: (_ for _ in ()).throw(
                ProtocolError(f"{field} contains a non-finite value")
            ),
        )
    except (json.JSONDecodeError, UnicodeError, ProtocolError) as error:
        raise ProtocolError(f"{field} must be canonical JSON") from error
    parsed = _mapping(parsed, field)
    _validate_value(parsed)
    try:
        canonical = json.dumps(
            parsed,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        )
    except (TypeError, ValueError) as error:
        raise ProtocolError(f"{field} must be canonical JSON") from error
    if canonical != text:
        raise ProtocolError(f"{field} must be canonical JSON")
    return parsed


def _identifier(value: object, field: str) -> str:
    text = _text(value, field)
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:-")
    if any(character not in allowed for character in text):
        raise ProtocolError(f"{field} contains an invalid identifier character")
    return text


def _version(value: object, field: str) -> str:
    text = _text(value, field, 64)
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.+-_")
    if any(character not in allowed for character in text):
        raise ProtocolError(f"{field} contains an invalid version character")
    return text


def _sha(value: object, field: str) -> str:
    text = _text(value, field, 64)
    if len(text) != 64 or any(character not in "0123456789abcdef" for character in text):
        raise ProtocolError(f"{field} must be a canonical SHA-256")
    return text


def _absolute_path(value: object, field: str) -> str:
    path = _text(value, field, MAX_PATH_BYTES)
    if not os.path.isabs(path):
        raise ProtocolError(f"{field} must be an absolute path")
    return path


def _archive_entry(value: object, field: str) -> str:
    entry = _text(value, field, 512)
    if (
        "\\" in entry
        or entry.startswith("/")
        or entry.endswith("/")
        or ":" in entry
        or any(part in {"", ".", ".."} for part in entry.split("/"))
    ):
        raise ProtocolError(f"{field} is not a safe relative payload entry")
    return entry


def _entrypoint(value: object, field: str) -> str:
    entrypoint = _text(value, field, MAX_ENTRYPOINT_BYTES)
    if entrypoint.count(":") != 1:
        raise ProtocolError(f"{field} is not an exact Python entrypoint")
    module_name, attribute = entrypoint.split(":")
    parts = (*module_name.split("."), attribute)
    if any(not part.isidentifier() for part in parts):
        raise ProtocolError(f"{field} is not an exact Python entrypoint")
    return entrypoint


def _validate_value(value: object, depth: int = 0) -> None:
    if depth > MAX_DEPTH:
        raise ProtocolError("value nesting exceeds the protocol bound")
    if value is None or isinstance(value, bool):
        return
    if isinstance(value, int):
        if not -(2**63) <= value <= 2**64 - 1:
            raise ProtocolError("integer exceeds the protocol numeric bound")
        return
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ProtocolError("non-finite numbers are forbidden")
        return
    if isinstance(value, str):
        if len(value.encode()) > MAX_STRING_BYTES or "\0" in value:
            raise ProtocolError("string exceeds the protocol bound")
        return
    if isinstance(value, bytes):
        if len(value) > MAX_BINARY_BYTES:
            raise ProtocolError("binary value exceeds the protocol bound")
        return
    if isinstance(value, Mapping):
        if len(value) > MAX_MAP_FIELDS:
            raise ProtocolError("object exceeds the protocol field bound")
        for key, item in value.items():
            if not isinstance(key, str):
                raise ProtocolError("object key must be text")
            _validate_value(key, depth + 1)
            _validate_value(item, depth + 1)
        return
    if isinstance(value, list | tuple):
        if len(value) > MAX_ARRAY_ITEMS:
            raise ProtocolError("array exceeds the protocol item bound")
        for item in value:
            _validate_value(item, depth + 1)
        return
    raise ProtocolError("value contains an unsupported type")


def _validate_profile_key(raw: object) -> None:
    value = _mapping(raw, "profile key")
    _exact(value, PROFILE_KEY_FIELDS, "profile key")
    _identifier(value["codec_family"], "codec_family")
    _identifier(value["profile"], "profile")
    _version(value["profile_version"], "profile_version")


def _validate_raw_import_metadata(raw: object) -> None:
    metadata = _mapping(raw, "raw import metadata")
    _exact(metadata, RAW_IMPORT_METADATA_FIELDS, "raw import metadata")
    _validate_profile_key(metadata["profile_key"])
    _archive_entry(metadata["payload_entry"], "raw import payload entry")
    if metadata["payload_media_type"] != "application/vnd.safetensors":
        raise ProtocolError("raw import payload media type must be Safetensors")
    tensors = metadata["tensors"]
    if not isinstance(tensors, list) or not 1 <= len(tensors) <= 64:
        raise ProtocolError("raw import tensors are outside their bound")
    names: set[str] = set()
    visual_count = 0
    audio_count = 0
    for raw_tensor in tensors:
        tensor = _mapping(raw_tensor, "raw import tensor")
        _exact(tensor, RAW_IMPORT_TENSOR_FIELDS, "raw import tensor")
        if tensor["stream"] not in {"visual", "audio"}:
            raise ProtocolError("raw import tensor stream is unknown")
        name = _identifier(tensor["name"], "raw import tensor name")
        if name in names:
            raise ProtocolError("raw import tensor names must be unique")
        names.add(name)
        visual_count += tensor["stream"] == "visual"
        audio_count += tensor["stream"] == "audio"
        if tensor["storage_dtype"] not in {"F16", "F32"} or tensor["runtime_dtype"] not in {
            "F16",
            "F32",
        }:
            raise ProtocolError("raw import tensor dtype is unsupported")
        shape = tensor["shape"]
        if not isinstance(shape, list) or not 1 <= len(shape) <= 8:
            raise ProtocolError("raw import tensor shape is outside its bound")
        values = 1
        for axis in shape:
            values *= _integer(axis, "raw import tensor axis", 1, 2**32 - 1)
            if values > MAX_RAW_IMPORT_SOURCE_BYTES:
                raise ProtocolError("raw import tensor shape is unbounded")
    if visual_count != 1 or audio_count > 1:
        raise ProtocolError("raw import requires one visual and at most one audio tensor")
    _identifier(metadata["timing_contract"], "raw import timing contract")
    _version(metadata["timing_contract_version"], "raw import timing contract version")
    for field in (
        "decoded_width",
        "decoded_height",
        "decoded_frame_count",
        "frame_rate_numerator",
        "frame_rate_denominator",
        "duration_numerator",
        "duration_denominator",
    ):
        _integer(metadata[field], f"raw import {field}", 1, 2**64 - 1)
    if metadata["audio_policy"] not in {"source_absent", "preserved_source"}:
        raise ProtocolError("raw import audio policy is unknown")
    if (audio_count == 1) != (metadata["audio_policy"] == "preserved_source"):
        raise ProtocolError("raw import audio policy does not match tensors")


def _validate_source(raw: object) -> None:
    value = _mapping(raw, "source")
    _exact(value, SOURCE_FIELDS, "source")
    _integer(value["physical_slot"], "physical_slot", 1, 16)
    for field in ("source_id", "cartridge_id", "profile_receipt_id"):
        _uuid(value[field], field)
    _sha(value["archive_sha256"], "archive_sha256")
    if not isinstance(value["loop_enabled"], bool):
        raise ProtocolError("loop_enabled must be boolean")


def _validate_source_transport(raw: object, *, allow_empty: bool = False) -> set[int]:
    if not isinstance(raw, list) or len(raw) > 16 or (not allow_empty and not raw):
        raise ProtocolError("source transport must contain between 1 and 16 entries")
    physical_slots: set[int] = set()
    for raw_source in raw:
        source = _mapping(raw_source, "source transport")
        _exact(
            source,
            frozenset({"physical_slot", "playing", "loop_enabled"}),
            "source transport",
        )
        physical_slot = _integer(source["physical_slot"], "physical_slot", 1, 16)
        if physical_slot in physical_slots:
            raise ProtocolError("source transport physical slots must be unique")
        physical_slots.add(physical_slot)
        if not isinstance(source["playing"], bool) or not isinstance(source["loop_enabled"], bool):
            raise ProtocolError("source transport flags must be boolean")
    return physical_slots


def _validate_controls(raw: object) -> None:
    if not isinstance(raw, list) or len(raw) > 64:
        raise ProtocolError("controls must contain at most 64 entries")
    names: set[str] = set()
    for raw_control in raw:
        control = _mapping(raw_control, "control")
        _exact(control, frozenset({"name", "value"}), "control")
        name = _identifier(control["name"], "control name")
        if name in names:
            raise ProtocolError("control names must be unique")
        names.add(name)
        value = _mapping(control["value"], "control value")
        _exact(value, frozenset({"kind", "value"}), "control value")
        kind = value["kind"]
        if kind not in {"boolean", "integer", "number", "text"}:
            raise ProtocolError("control kind is unsupported")
        payload = value["value"]
        if kind == "boolean" and not isinstance(payload, bool):
            raise ProtocolError("boolean control must contain a boolean")
        if kind == "integer" and (isinstance(payload, bool) or not isinstance(payload, int)):
            raise ProtocolError("integer control must contain an integer")
        if kind == "integer" and not -(2**63) <= payload <= 2**63 - 1:
            raise ProtocolError("integer control exceeds signed 64-bit range")
        if kind == "number" and (
            isinstance(payload, bool)
            or not isinstance(payload, int | float)
            or not math.isfinite(float(payload))
        ):
            raise ProtocolError("number control must contain a finite number")
        if kind == "text":
            _text(payload, "control text", 4096)


def _validate_roles(raw: object) -> None:
    if not isinstance(raw, list) or len(raw) > 16:
        raise ProtocolError("roles must contain at most 16 entries")
    names: set[str] = set()
    for raw_role in raw:
        role = _mapping(raw_role, "role")
        _exact(role, frozenset({"role", "physical_slot"}), "role")
        name = _identifier(role["role"], "role")
        if name in names:
            raise ProtocolError("role names must be unique")
        names.add(name)
        _integer(role["physical_slot"], "physical_slot", 1, 16)


def _validate_assets(raw: object) -> None:
    if not isinstance(raw, list) or len(raw) > 16:
        raise ProtocolError("external assets must contain at most 16 entries")
    identifiers: set[str] = set()
    fields = frozenset({"asset_id", "path", "sha256", "byte_length"})
    for raw_asset in raw:
        asset = _mapping(raw_asset, "external asset")
        _exact(asset, fields, "external asset")
        asset_id = _identifier(asset["asset_id"], "asset_id")
        if asset_id in identifiers:
            raise ProtocolError("external asset IDs must be unique")
        identifiers.add(asset_id)
        _text(asset["path"], "asset path", MAX_STRING_BYTES)
        _sha(asset["sha256"], "asset sha256")
        _integer(asset["byte_length"], "asset byte_length", 1, 2**64 - 1)


def _validate_capabilities(raw: object, field: str) -> list[Capability]:
    if not isinstance(raw, list) or len(raw) > 16:
        raise ProtocolError(f"{field} exceeds the capability bound")
    try:
        parsed = [Capability(capability) for capability in raw]
    except ValueError as error:
        raise ProtocolError(f"{field} contains an unknown capability") from error
    if len(set(parsed)) != len(parsed):
        raise ProtocolError(f"{field} capabilities must be unique")
    return parsed


def _validate_signal_geometry(raw: object) -> Mapping[str, object]:
    geometry = _mapping(raw, "signal geometry")
    _exact(geometry, SIGNAL_GEOMETRY_FIELDS, "signal geometry")
    for field in (
        "channels",
        "latent_height",
        "latent_width",
        "decoded_height",
        "decoded_width",
        "frame_rate_numerator",
        "frame_rate_denominator",
    ):
        _integer(geometry[field], field, 1, 2**32 - 1)
    _identifier(geometry["timing_contract"], "timing_contract")
    _version(geometry["timing_contract_version"], "timing_contract_version")
    return geometry


def _validate_tensor_abi(raw: object) -> Mapping[str, object]:
    tensor = _mapping(raw, "tensor ABI")
    _exact(tensor, TENSOR_ABI_FIELDS, "tensor ABI")
    if tensor["python_major"] != 3 or tensor["python_minor"] != 13:
        raise ProtocolError("tensor ABI requires CPython 3.13")
    _version(tensor["torch_version"], "torch_version")
    if tensor["dtype"] not in {"float16", "bfloat16", "float32"}:
        raise ProtocolError("tensor ABI dtype is unknown")
    shape = tensor["shape"]
    if not isinstance(shape, list) or len(shape) != 5:
        raise ProtocolError("tensor ABI shape must contain five dimensions")
    for index, dimension in enumerate(shape):
        _integer(dimension, f"shape[{index}]", 1, 2**32 - 1)
    if shape[0] != 1 or shape[2] != 1:
        raise ProtocolError("tensor ABI shape must be [1,C,1,H,W]")
    if tensor["contiguous"] is not True:
        raise ProtocolError("tensor ABI must be contiguous")
    if tensor["device"] not in {"cpu", "cuda"}:
        raise ProtocolError("tensor ABI device is unknown")
    return tensor


def _validate_decoded_abi(raw: object) -> None:
    decoded = _mapping(raw, "decoded ABI")
    _exact(decoded, DECODED_ABI_FIELDS, "decoded ABI")
    if decoded["pixel_format"] != "rgba8":
        raise ProtocolError("decoded ABI pixel format must be rgba8")
    _integer(decoded["maximum_batch"], "maximum_batch", 1, 24)


def _validate_profile_receipt(raw: object) -> None:
    receipt = _mapping(raw, "profile receipt")
    _exact(receipt, PROFILE_RECEIPT_FIELDS, "profile receipt")
    for field in ("receipt_id", "cartridge_id"):
        _uuid(receipt[field], field)
    for field in ("archive_sha256", "payload_sha256"):
        _sha(receipt[field], field)
    for field in ("pack_id", "adapter_id"):
        _identifier(receipt[field], field)
    for field in ("pack_version", "adapter_version"):
        _version(receipt[field], field)
    _validate_profile_key(receipt["profile_key"])
    geometry = _validate_signal_geometry(receipt["signal_geometry"])
    tensor = _validate_tensor_abi(receipt["tensor_abi"])
    _validate_decoded_abi(receipt["decoded_abi"])
    if [tensor["shape"][1], tensor["shape"][3], tensor["shape"][4]] != [
        geometry["channels"],
        geometry["latent_height"],
        geometry["latent_width"],
    ]:
        raise ProtocolError("profile receipt tensor ABI does not match signal geometry")
    if not _validate_capabilities(receipt["capabilities"], "receipt capabilities"):
        raise ProtocolError("profile receipt must declare capabilities")
    for field in ("estimated_host_bytes", "estimated_device_bytes"):
        _integer(receipt[field], field, 0, 2**64 - 1)


def _validate_player_status(raw: object) -> None:
    status = _mapping(raw, "player status")
    _exact(status, PLAYER_STATUS_FIELDS, "player status")
    _uuid(status["player_session_id"], "player_session_id")
    try:
        state = PlayerState(status["state"])
    except (TypeError, ValueError) as error:
        raise ProtocolError("player state is unknown") from error
    generation = _integer(status["stream_generation"], "stream_generation", 0, 2**64 - 1)
    if state is not PlayerState.EMPTY and generation == 0:
        raise ProtocolError("non-empty player status requires a stream generation")
    for field in ("stream_sequence", "playhead_slot"):
        _integer(status[field], field, 0, 2**64 - 1)
    if not isinstance(status["end_of_stream"], bool):
        raise ProtocolError("end_of_stream must be boolean")
    if status["decoded_ring_id"] is not None:
        _uuid(status["decoded_ring_id"], "decoded_ring_id")


def _validate_deck_status(raw: object) -> None:
    status = _mapping(raw, "Deck status")
    _exact(status, DECK_STATUS_FIELDS, "Deck status")
    _uuid(status["deck_session_id"], "deck_session_id")
    try:
        state = DeckState(status["state"])
    except (TypeError, ValueError) as error:
        raise ProtocolError("Deck state is unknown") from error
    revision = _integer(status["deck_revision"], "deck_revision", 0, 2**64 - 1)
    generation = _integer(status["stream_generation"], "stream_generation", 0, 2**64 - 1)
    if state is not DeckState.EMPTY and (revision == 0 or generation == 0):
        raise ProtocolError("non-empty Deck status requires revision and generation")
    _integer(status["stream_sequence"], "stream_sequence", 0, 2**64 - 1)
    playheads = status["playheads"]
    if not isinstance(playheads, list) or len(playheads) > 16:
        raise ProtocolError("Deck playheads exceed the source bound")
    physical_slots: set[int] = set()
    for raw_playhead in playheads:
        playhead = _mapping(raw_playhead, "playhead")
        _exact(
            playhead,
            frozenset({"physical_slot", "latent_slot", "loop_enabled", "end_of_stream"}),
            "playhead",
        )
        slot = _integer(playhead["physical_slot"], "physical_slot", 1, 16)
        if slot in physical_slots:
            raise ProtocolError("playhead physical slots must be unique")
        physical_slots.add(slot)
        _integer(playhead["latent_slot"], "latent_slot", 0, 2**64 - 1)
        if not isinstance(playhead["loop_enabled"], bool) or not isinstance(
            playhead["end_of_stream"], bool
        ):
            raise ProtocolError("playhead flags must be boolean")
    source_transport_slots = _validate_source_transport(
        status["source_transport"], allow_empty=state is DeckState.EMPTY
    )
    if source_transport_slots != physical_slots:
        raise ProtocolError("source transport must match Deck playhead physical slots")
    _validate_roles(status["roles"])
    _validate_controls(status["controls"])
    _integer(status["seed"], "seed", 0, 2**64 - 1)
    try:
        CaptureState(status["capture_state"])
    except (TypeError, ValueError) as error:
        raise ProtocolError("Deck capture state is unknown") from error


def _validate_capture_status(raw: object) -> None:
    status = _mapping(raw, "capture status")
    _exact(status, CAPTURE_STATUS_FIELDS, "capture status")
    _uuid(status["deck_session_id"], "deck_session_id")
    _integer(status["deck_revision"], "deck_revision", 1, 2**64 - 1)
    _uuid(status["capture_id"], "capture_id")
    try:
        state = CaptureState(status["state"])
    except (TypeError, ValueError) as error:
        raise ProtocolError("capture state is unknown") from error
    if status["mode"] not in {"snapshot", "live_capture"}:
        raise ProtocolError("capture mode is unknown")
    if state is CaptureState.IDLE:
        raise ProtocolError("per-capture status cannot be idle")
    latent_slots = _integer(status["latent_slots"], "latent_slots", 0, 1_048_576)
    _integer(status["reset_events"], "reset_events", 0, 32)
    artifact = status["artifact"]
    if (state is CaptureState.COMPLETED) != (artifact is not None):
        raise ProtocolError("capture artifact presence is inconsistent with capture state")
    if artifact is not None:
        artifact = _mapping(artifact, "capture artifact")
        _exact(artifact, CAPTURE_ARTIFACT_FIELDS, "capture artifact")
        _absolute_path(artifact["staged_payload_path"], "staged_payload_path")
        _sha(artifact["payload_sha256"], "payload_sha256")
        _integer(artifact["payload_byte_length"], "payload_byte_length", 1, 2**64 - 1)
        artifact_slots = _integer(artifact["latent_slots"], "artifact latent_slots", 1, 1_048_576)
        _integer(artifact["decoded_frame_count"], "decoded_frame_count", 1, 2**64 - 1)
        if state is not CaptureState.COMPLETED or artifact_slots != latent_slots:
            raise ProtocolError("capture artifact is inconsistent with capture status")


def _validate_ack(raw: object) -> None:
    ack = _mapping(raw, "typed ack")
    _exact(ack, frozenset({"name", "payload"}), "typed ack")
    name = _text(ack["name"], "ack command name")
    fields = ACK_FIELDS.get(name)
    if fields is None:
        raise ProtocolError("ack command name is unknown")
    payload = _mapping(ack["payload"], "ack payload")
    _exact(payload, fields, f"{name} ack payload")

    if name == "session.configure":
        if payload["selected_protocol_version"] != PROTOCOL_VERSION:
            raise ProtocolError("configured protocol must be 2")
        if payload["maximum_frame_bytes"] != MAX_FRAME_BYTES:
            raise ProtocolError("configured frame bound must equal Protocol 2")
        if not _validate_capabilities(payload["accepted_capabilities"], "accepted capabilities"):
            raise ProtocolError("accepted capabilities must not be empty")
    elif name == "session.status":
        _validate_status(payload)
    elif name == "session.shutdown":
        if payload["reason"] not in {"user_request", "host_exit", "protocol_fault"}:
            raise ProtocolError("shutdown reason is unknown")
    elif name == "codec.descriptor":
        for field in ("pack_id", "adapter_id"):
            _identifier(payload[field], field)
        for field in ("pack_version", "adapter_version", "host_api_version"):
            _version(payload[field], field)
        if payload["host_api_version"] != "2.0":
            raise ProtocolError("codec host API must be 2.0")
        capabilities = _validate_capabilities(payload["capabilities"], "codec capabilities")
        required = {
            Capability.PLAYER,
            Capability.REALTIME,
            Capability.RESAMPLE,
            Capability.SNAPSHOT_CAPTURE,
            Capability.LIVE_CAPTURE,
        }
        if not required.issubset(capabilities):
            raise ProtocolError("Codec Pack v2 is missing a required capability")
        profiles = payload["profiles"]
        if not isinstance(profiles, list) or not 1 <= len(profiles) <= MAX_PROFILES:
            raise ProtocolError("codec profiles are outside their bound")
        identities: set[tuple[object, object, object]] = set()
        for profile in profiles:
            _validate_profile_key(profile)
            mapping = _mapping(profile, "profile key")
            identity = (
                mapping["codec_family"],
                mapping["profile"],
                mapping["profile_version"],
            )
            if identity in identities:
                raise ProtocolError("codec profiles must be unique")
            identities.add(identity)
    elif name == "codec.load":
        for field in ("pack_id", "adapter_id"):
            _identifier(payload[field], field)
        for field in ("pack_version", "adapter_version"):
            _version(payload[field], field)
        if payload["device"] not in {"cpu", "cuda"}:
            raise ProtocolError("codec device is unknown")
        _integer(payload["device_ordinal"], "device_ordinal", 0, 255)
    elif name == "codec.unload":
        _identifier(payload["pack_id"], "pack_id")
        _version(payload["pack_version"], "pack_version")
    elif name == "source.open":
        _uuid(payload["source_id"], "source_id")
        _uuid(payload["cartridge_id"], "cartridge_id")
        _sha(payload["archive_sha256"], "archive_sha256")
    elif name == "source.close":
        _uuid(payload["source_id"], "source_id")
    elif name == "ring.configure":
        _uuid(payload["ring_id"], "ring_id")
        if payload["kind"] not in {"latent_tensor", "decoded_rgba"}:
            raise ProtocolError("ring kind is unknown")
        _integer(payload["slot_count"], "slot_count", 2, 24)
        _integer(payload["slot_bytes"], "slot_bytes", 1, 2**64 - 1)
    elif name == "ring.release":
        _uuid(payload["ring_id"], "ring_id")
    elif name == "profile.inspect":
        _uuid(payload["source_id"], "source_id")
        _uuid(payload["cartridge_id"], "cartridge_id")
        _sha(payload["archive_sha256"], "archive_sha256")
        _sha(payload["payload_sha256"], "payload_sha256")
        _validate_profile_key(payload["profile_key"])
        _validate_signal_geometry(payload["signal_geometry"])
    elif name == "profile.validate":
        _validate_profile_receipt(payload)
    elif name == "raw_import.preflight":
        for field in ("receipt_id", "import_id"):
            _uuid(payload[field], field)
        for field in ("pack_id", "adapter_id"):
            _identifier(payload[field], field)
        for field in ("pack_version", "adapter_version"):
            _version(payload[field], field)
        _sha(payload["source_sha256"], "raw import source SHA-256")
        _integer(
            payload["source_byte_length"],
            "raw import source byte length",
            1,
            MAX_RAW_IMPORT_SOURCE_BYTES,
        )
        _validate_raw_import_metadata(payload["metadata"])
    elif name == "raw_import.stage":
        for field in ("receipt_id", "import_id"):
            _uuid(payload[field], field)
        _absolute_path(payload["staged_payload_path"], "raw import staged payload path")
        _sha(payload["payload_sha256"], "raw import payload SHA-256")
        _integer(
            payload["payload_byte_length"],
            "raw import payload byte length",
            1,
            MAX_RAW_IMPORT_SOURCE_BYTES,
        )
    elif name == "raw_import.abort":
        _uuid(payload["receipt_id"], "receipt_id")
        _uuid(payload["import_id"], "import_id")
    elif name in {"player.open", "player.reset", "player.status"}:
        _validate_player_status(payload)
    elif name == "player.step":
        _validate_player_status(payload["status"])
        if payload["output_ring_id"] is not None:
            _uuid(payload["output_ring_id"], "output_ring_id")
        _integer(payload["output_slot_sequence"], "output_slot_sequence", 0, 2**64 - 1)
        decoded_frames = _integer(payload["decoded_frames"], "decoded_frames", 0, 24)
        if decoded_frames and payload["output_ring_id"] is None:
            raise ProtocolError("decoded frames require an output ring")
    elif name in {
        "deck.load",
        "deck.controls.set",
        "deck.roles.set",
        "deck.transport.set",
        "deck.seed.set",
        "deck.reset",
        "deck.restart",
        "deck.status",
    }:
        _validate_deck_status(payload)
    elif name == "deck.process":
        _validate_deck_status(payload["status"])
        _uuid(payload["output_ring_id"], "output_ring_id")
        _integer(payload["output_slot_sequence"], "output_slot_sequence", 1, 2**64 - 1)
        provenance = payload["provenance"]
        if not isinstance(provenance, list) or len(provenance) > 64:
            raise ProtocolError("provenance exceeds its bound")
        keys: set[str] = set()
        for raw_entry in provenance:
            entry = _mapping(raw_entry, "provenance entry")
            _exact(entry, frozenset({"key", "value"}), "provenance entry")
            key = _identifier(entry["key"], "provenance key")
            if key in keys:
                raise ProtocolError("provenance keys must be unique")
            keys.add(key)
            _validate_controls([{"name": key, "value": entry["value"]}])
    elif name in {"capture.start", "capture.stop", "capture.status"}:
        _validate_capture_status(payload)
    elif name == "metrics.get":
        for field in ACK_FIELDS["metrics.get"]:
            _integer(payload[field], field, 0, 2**64 - 1)


def _validate_deck_identity(payload: Mapping[str, object]) -> None:
    _uuid(payload["deck_session_id"], "deck_session_id")
    _integer(payload["deck_revision"], "deck_revision", 1, 2**64 - 1)


def _validate_capture_identity(payload: Mapping[str, object]) -> None:
    _validate_deck_identity(payload)
    _uuid(payload["capture_id"], "capture_id")


def _validate_deck_runtime_binding(raw: object, deck_load: Mapping[str, object]) -> None:
    runtime = _mapping(raw, "Deck runtime binding")
    _exact(runtime, DECK_RUNTIME_BINDING_FIELDS, "Deck runtime binding")
    _identifier(runtime["deck_id"], "runtime deck_id")
    _version(runtime["deck_version"], "runtime deck_version")
    _identifier(runtime["operator_id"], "runtime operator_id")
    _version(runtime["operator_version"], "runtime operator_version")
    _absolute_path(runtime["python_root"], "runtime python_root")
    _entrypoint(runtime["entrypoint"], "runtime entrypoint")
    _sha(runtime["package_manifest_sha256"], "package_manifest_sha256")
    _sha(runtime["integrity_catalog_sha256"], "integrity_catalog_sha256")
    for field in ("deck_id", "deck_version", "operator_id", "operator_version"):
        if runtime[field] != deck_load[field]:
            raise ProtocolError("Deck runtime identity does not match deck.load")


def _validate_command(raw: object) -> None:
    command = _mapping(raw, "command")
    _exact(command, frozenset({"name", "payload"}), "command")
    name = _text(command["name"], "command name")
    fields = COMMAND_FIELDS.get(name)
    if fields is None:
        raise ProtocolError("command name is unknown")
    payload = _mapping(command["payload"], "command payload")
    if name == "deck.load":
        actual_fields = frozenset(payload)
        if actual_fields not in (DECK_LOAD_FIELDS, DECK_LOAD_FIELDS | {"runtime"}):
            raise ProtocolError("deck.load payload does not match its closed schema")
    else:
        _exact(payload, fields, f"{name} payload")
    if name == "session.configure":
        if payload["selected_protocol_version"] != PROTOCOL_VERSION:
            raise ProtocolError("session selected protocol must be 2")
        _version(payload["app_version"], "app_version")
        interval = _integer(payload["heartbeat_interval_ms"], "heartbeat interval", 250, 60_000)
        _integer(payload["heartbeat_hard_timeout_ms"], "heartbeat timeout", interval * 3, 2**32 - 1)
        if payload["max_frame_bytes"] != MAX_FRAME_BYTES:
            raise ProtocolError("max_frame_bytes must equal the Protocol 2 bound")
        _integer(payload["max_inflight_batches"], "max_inflight_batches", 1, 24)
        _validate_capabilities(payload["requested_capabilities"], "requested capabilities")
    elif name == "session.shutdown":
        if payload["reason"] not in {"user_request", "host_exit", "protocol_fault"}:
            raise ProtocolError("shutdown reason is unknown")
    elif name == "codec.descriptor":
        _identifier(payload["pack_id"], "pack_id")
        _version(payload["pack_version"], "pack_version")
        _identifier(payload["adapter_id"], "adapter_id")
    elif name == "codec.load":
        _identifier(payload["pack_id"], "pack_id")
        _version(payload["pack_version"], "pack_version")
        _identifier(payload["adapter_id"], "adapter_id")
        _version(payload["adapter_version"], "adapter_version")
        if payload["device"] not in {"cpu", "cuda"}:
            raise ProtocolError("codec device is unknown")
        _integer(payload["device_ordinal"], "device_ordinal", 0, 255)
        _validate_assets(payload["external_assets"])
    elif name == "codec.unload":
        _identifier(payload["pack_id"], "pack_id")
        _version(payload["pack_version"], "pack_version")
    elif name == "source.open":
        _uuid(payload["source_id"], "source_id")
        _uuid(payload["cartridge_id"], "cartridge_id")
        _sha(payload["archive_sha256"], "archive_sha256")
        _integer(payload["archive_bytes"], "archive_bytes", 1, 2**64 - 1)
        _integer(payload["retained_native_handle"], "retained_native_handle", 1, 2**64 - 1)
        _canonical_json_object(
            payload["integrity_access_receipt"],
            "integrity_access_receipt",
            65_536,
        )
    elif name == "source.close":
        _uuid(payload["source_id"], "source_id")
    elif name == "ring.configure":
        _uuid(payload["ring_id"], "ring_id")
        if payload["kind"] not in {"latent_tensor", "decoded_rgba"}:
            raise ProtocolError("ring kind is unknown")
        for field in ("mapping_handle", "ready_event_handle", "consumed_event_handle"):
            _integer(payload[field], field, 1, 2**64 - 1)
        _integer(payload["slot_count"], "slot_count", 2, 24)
        _integer(payload["slot_bytes"], "slot_bytes", 1, 2**64 - 1)
    elif name == "ring.release":
        _uuid(payload["ring_id"], "ring_id")
    elif name == "profile.inspect":
        _uuid(payload["source_id"], "source_id")
        _uuid(payload["cartridge_id"], "cartridge_id")
        _sha(payload["archive_sha256"], "archive_sha256")
    elif name == "profile.validate":
        _uuid(payload["source_id"], "source_id")
        _validate_profile_key(payload["expected_profile"])
        _validate_capabilities(payload["required_capabilities"], "required capabilities")
    elif name == "raw_import.preflight":
        _uuid(payload["import_id"], "import_id")
        _absolute_path(payload["source_path"], "raw import source path")
        _integer(
            payload["maximum_source_bytes"],
            "raw import maximum source bytes",
            1,
            MAX_RAW_IMPORT_SOURCE_BYTES,
        )
    elif name == "raw_import.stage":
        _uuid(payload["import_id"], "import_id")
        _uuid(payload["receipt_id"], "receipt_id")
        _absolute_path(payload["staging_root"], "raw import staging root")
    elif name == "raw_import.abort":
        _uuid(payload["import_id"], "import_id")
        _uuid(payload["receipt_id"], "receipt_id")
    elif name == "player.open":
        _uuid(payload["player_session_id"], "player_session_id")
        _validate_source(payload["source"])
        _integer(payload["stream_generation"], "stream_generation", 1, 2**64 - 1)
    elif name == "player.step":
        _uuid(payload["player_session_id"], "player_session_id")
        _integer(payload["stream_generation"], "stream_generation", 1, 2**64 - 1)
        _integer(payload["maximum_decoded_frames"], "maximum_decoded_frames", 1, 24)
    elif name == "player.reset":
        _uuid(payload["player_session_id"], "player_session_id")
        _integer(payload["new_stream_generation"], "new_stream_generation", 1, 2**64 - 1)
    elif name == "deck.load":
        _uuid(payload["deck_session_id"], "deck_session_id")
        _identifier(payload["deck_id"], "deck_id")
        _version(payload["deck_version"], "deck_version")
        _identifier(payload["operator_id"], "operator_id")
        _version(payload["operator_version"], "operator_version")
        if "runtime" in payload:
            _validate_deck_runtime_binding(payload["runtime"], payload)
        sources = payload["sources"]
        if not isinstance(sources, list) or not 1 <= len(sources) <= 16:
            raise ProtocolError("deck sources must contain 1..16 entries")
        for source in sources:
            _validate_source(source)
        physical_slots = [source["physical_slot"] for source in sources]
        if len(set(physical_slots)) != len(physical_slots):
            raise ProtocolError("source physical slots must be unique")
        _validate_roles(payload["roles"])
        if any(role["physical_slot"] not in physical_slots for role in payload["roles"]):
            raise ProtocolError("roles must reference loaded physical slots")
        _validate_controls(payload["controls"])
        _integer(payload["seed"], "seed", 0, 2**64 - 1)
        _integer(payload["stream_generation"], "stream_generation", 1, 2**64 - 1)
    elif name == "deck.process":
        _validate_deck_identity(payload)
        _integer(payload["stream_generation"], "stream_generation", 1, 2**64 - 1)
    elif name == "deck.controls.set":
        _validate_deck_identity(payload)
        _validate_controls(payload["controls"])
    elif name == "deck.roles.set":
        _validate_deck_identity(payload)
        _validate_roles(payload["roles"])
    elif name == "deck.transport.set":
        _validate_deck_identity(payload)
        _validate_source_transport(payload["sources"])
    elif name == "deck.seed.set":
        _validate_deck_identity(payload)
        _integer(payload["seed"], "seed", 0, 2**64 - 1)
    elif name == "deck.reset":
        _validate_deck_identity(payload)
        _integer(payload["new_stream_generation"], "new_stream_generation", 1, 2**64 - 1)
        if not isinstance(payload["preserve_playheads"], bool):
            raise ProtocolError("preserve_playheads must be boolean")
    elif name == "deck.restart":
        _validate_deck_identity(payload)
    elif name == "capture.start":
        _validate_capture_identity(payload)
        if payload["mode"] not in {"snapshot", "live_capture"}:
            raise ProtocolError("capture mode is unknown")
        _absolute_path(payload["staging_root"], "staging_root")
        _integer(payload["maximum_latent_slots"], "maximum_latent_slots", 1, 1_048_576)
        _integer(
            payload["maximum_visual_bytes"],
            "maximum_visual_bytes",
            1,
            15 * 1024**3,
        )
        _integer(payload["maximum_reset_events"], "maximum_reset_events", 1, 32)
    elif name in {"capture.stop", "capture.status"}:
        _validate_capture_identity(payload)


def _validate_status(raw: object) -> None:
    status = _mapping(raw, "status")
    _exact(status, STATUS_FIELDS, "status")
    for field, enum_type in (
        ("session", SessionState),
        ("codec", CodecState),
        ("player", PlayerState),
        ("deck", DeckState),
        ("capture", CaptureState),
    ):
        try:
            enum_type(status[field])
        except (TypeError, ValueError) as error:
            raise ProtocolError(f"{field} status is unknown") from error
    _integer(status["open_session_count"], "open_session_count", 0, 4)
    foreground = status["foreground_output_session"]
    if foreground is not None:
        _uuid(foreground, "foreground_output_session")
    if not isinstance(status["output_lease_pinned"], bool):
        raise ProtocolError("output_lease_pinned must be boolean")
    if status["output_lease_pinned"] and foreground is None:
        raise ProtocolError("a pinned output lease requires a foreground session")


def _validate_error_payload(raw: object) -> None:
    error = _mapping(raw, "error")
    _exact(
        error,
        frozenset({"code", "message", "retryable", "fatal", "status", "diagnostic_id", "details"}),
        "error",
    )
    try:
        ErrorCode(error["code"])
    except (TypeError, ValueError) as exception:
        raise ProtocolError("error code is unknown") from exception
    _text(error["message"], "error message", 4096)
    if not isinstance(error["retryable"], bool) or not isinstance(error["fatal"], bool):
        raise ProtocolError("error flags must be boolean")
    _uuid(error["diagnostic_id"], "diagnostic_id")
    _validate_status(error["status"])
    details = error["details"]
    if not isinstance(details, list) or len(details) > 16:
        raise ProtocolError("error details exceed the bound")
    keys: set[str] = set()
    for raw_detail in details:
        detail = _mapping(raw_detail, "error detail")
        _exact(detail, frozenset({"key", "value"}), "error detail")
        key = _identifier(detail["key"], "error detail key")
        _text(detail["value"], "error detail value", 4096)
        if key in keys:
            raise ProtocolError("error detail keys must be unique")
        keys.add(key)


def _validate_worker_hello(raw: object) -> None:
    hello = _mapping(raw, "worker hello")
    _exact(hello, WORKER_HELLO_FIELDS, "worker hello")
    _auth_token_text(hello["auth_token"], "worker hello auth token")
    _integer(hello["worker_pid"], "worker_pid", 1, 2**32 - 1)
    _identifier(hello["worker_identity"], "worker_identity")
    _text(hello["runtime_identity"], "runtime_identity", 4096)
    if hello["protocol_min"] != PROTOCOL_VERSION or hello["protocol_max"] != PROTOCOL_VERSION:
        raise ProtocolError("worker hello protocol range must be exactly Protocol 2")


def _auth_token_text(raw: object, label: str) -> str:
    if (
        not isinstance(raw, str)
        or len(raw) != 64
        or any(character not in "0123456789abcdef" for character in raw)
    ):
        raise ProtocolError(f"{label} must be 64 lowercase hex characters")
    return raw


def validate_envelope(raw: Mapping[str, object]) -> dict[str, object]:
    """Validate one closed Protocol 2 envelope without repairing or truncating it."""

    envelope = _mapping(raw, "envelope")
    _validate_value(envelope)
    _exact(
        envelope,
        frozenset(
            {
                "protocol",
                "protocol_version",
                "session_id",
                "sequence",
                "message_id",
                "sender_uptime_ns",
                "message",
            }
        ),
        "envelope",
    )
    if envelope["protocol"] != PROTOCOL or envelope["protocol_version"] != PROTOCOL_VERSION:
        raise ProtocolError("protocol marker or version is unsupported")
    _uuid(envelope["session_id"], "session_id")
    _integer(envelope["sequence"], "sequence", 1, 2**64 - 1)
    _uuid(envelope["message_id"], "message_id")
    _integer(envelope["sender_uptime_ns"], "sender_uptime_ns", 0, 2**64 - 1)
    message = _mapping(envelope["message"], "message")
    _exact(message, frozenset({"kind", "body"}), "message")
    if message["kind"] == "command":
        _validate_command(message["body"])
    elif message["kind"] == "ack":
        body = _mapping(message["body"], "ack")
        _exact(body, frozenset({"reply_to", "ack", "status"}), "ack")
        _uuid(body["reply_to"], "reply_to")
        _validate_ack(body["ack"])
        _validate_status(body["status"])
    elif message["kind"] == "error":
        body = _mapping(message["body"], "error reply")
        _exact(body, frozenset({"reply_to", "name", "error"}), "error reply")
        _uuid(body["reply_to"], "reply_to")
        if body["name"] not in COMMAND_FIELDS:
            raise ProtocolError("error command name is unknown")
        _validate_error_payload(body["error"])
    elif message["kind"] == "event":
        body = _mapping(message["body"], "event")
        _exact(body, frozenset({"caused_by", "event"}), "event")
        if body["caused_by"] is not None:
            _uuid(body["caused_by"], "caused_by")
        event = _mapping(body["event"], "event body")
        _exact(event, frozenset({"name", "payload"}), "event body")
        if event["name"] == "worker.hello":
            _validate_worker_hello(event["payload"])
        elif event["name"] in {"status.changed", "worker.heartbeat"}:
            _validate_status(event["payload"])
        elif event["name"] == "worker.fault":
            _validate_error_payload(event["payload"])
        else:
            raise ProtocolError("event name is unknown")
    else:
        raise ProtocolError("message kind is unknown")
    return dict(envelope)


def encode_json(value: Mapping[str, object]) -> bytes:
    validated = validate_envelope(value)
    encoded = json.dumps(validated, ensure_ascii=True, separators=(",", ":")).encode()
    if not 1 <= len(encoded) <= MAX_FRAME_BYTES:
        raise ProtocolError("JSON frame length is outside its bound")
    return encoded


def decode_json(encoded: bytes) -> dict[str, object]:
    if not 1 <= len(encoded) <= MAX_FRAME_BYTES:
        raise ProtocolError("JSON frame length is outside its bound")
    try:
        value = json.loads(encoded, object_pairs_hook=_unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError, ProtocolError) as error:
        raise ProtocolError("JSON frame is invalid") from error
    return validate_envelope(_mapping(value, "JSON frame"))


def encode_messagepack(value: Mapping[str, object]) -> bytes:
    validated = validate_envelope(value)
    encoded = msgpack.packb(validated, use_bin_type=True, strict_types=True)
    if not 1 <= len(encoded) <= MAX_FRAME_BYTES:
        raise ProtocolError("MessagePack frame length is outside its bound")
    return encoded


def decode_messagepack(encoded: bytes) -> dict[str, object]:
    if not 1 <= len(encoded) <= MAX_FRAME_BYTES:
        raise ProtocolError("MessagePack frame length is outside its bound")
    try:
        value = msgpack.unpackb(
            encoded,
            raw=False,
            strict_map_key=True,
            object_pairs_hook=_unique_object,
            max_str_len=MAX_STRING_BYTES,
            max_bin_len=MAX_BINARY_BYTES,
            max_array_len=MAX_ARRAY_ITEMS,
            max_map_len=MAX_MAP_FIELDS,
            max_ext_len=0,
        )
    except (msgpack.ExtraData, msgpack.FormatError, msgpack.StackError, ValueError) as error:
        raise ProtocolError("MessagePack frame is invalid") from error
    return validate_envelope(_mapping(value, "MessagePack frame"))


def make_conformance_envelope() -> dict[str, object]:
    """Return the deterministic fixture also emitted by the Rust helper."""

    return {
        "protocol": PROTOCOL,
        "protocol_version": PROTOCOL_VERSION,
        "session_id": "9ca8c228-04c7-4b59-909f-6fbef591a43e",
        "sequence": 1,
        "message_id": "10000000-0000-4000-8000-000000000002",
        "sender_uptime_ns": 123_456,
        "message": {
            "kind": "command",
            "body": {
                "name": "session.configure",
                "payload": {
                    "selected_protocol_version": PROTOCOL_VERSION,
                    "app_version": "0.2.0",
                    "heartbeat_interval_ms": 1_000,
                    "heartbeat_hard_timeout_ms": 10_000,
                    "max_frame_bytes": MAX_FRAME_BYTES,
                    "max_inflight_batches": 4,
                    "requested_capabilities": [
                        "player",
                        "realtime",
                        "resample",
                        "snapshot_capture",
                        "live_capture",
                    ],
                },
            },
        },
    }


def make_conformance_ack_envelope() -> dict[str, object]:
    """Return the deterministic typed-Ack fixture also emitted by Rust."""

    return {
        "protocol": PROTOCOL,
        "protocol_version": PROTOCOL_VERSION,
        "session_id": "9ca8c228-04c7-4b59-909f-6fbef591a43e",
        "sequence": 3,
        "message_id": "10000000-0000-4000-8000-000000000007",
        "sender_uptime_ns": 323_456,
        "message": {
            "kind": "ack",
            "body": {
                "reply_to": "10000000-0000-4000-8000-000000000002",
                "ack": {
                    "name": "codec.descriptor",
                    "payload": {
                        "pack_id": "org.example.synthetic",
                        "pack_version": "0.2.0",
                        "adapter_id": "org.example.synthetic.adapter",
                        "adapter_version": "0.2.0",
                        "host_api_version": "2.0",
                        "capabilities": [
                            "player",
                            "realtime",
                            "resample",
                            "snapshot_capture",
                            "live_capture",
                        ],
                        "profiles": [
                            {
                                "codec_family": "synthetic",
                                "profile": "test_latent",
                                "profile_version": "0.1.0",
                            }
                        ],
                    },
                },
                "status": {
                    "session": "ready",
                    "codec": "ready",
                    "player": "empty",
                    "deck": "empty",
                    "capture": "idle",
                    "open_session_count": 0,
                    "foreground_output_session": None,
                    "output_lease_pinned": False,
                },
            },
        },
    }


def make_conformance_error_envelope() -> dict[str, object]:
    """Return the deterministic typed error/status fixture emitted by Rust."""

    return {
        "protocol": PROTOCOL,
        "protocol_version": PROTOCOL_VERSION,
        "session_id": "9ca8c228-04c7-4b59-909f-6fbef591a43e",
        "sequence": 2,
        "message_id": "10000000-0000-4000-8000-000000000003",
        "sender_uptime_ns": 223_456,
        "message": {
            "kind": "error",
            "body": {
                "reply_to": "10000000-0000-4000-8000-000000000002",
                "name": "capture.start",
                "error": {
                    "code": "session.output_lease_pinned",
                    "message": "foreground output is pinned by capture",
                    "retryable": True,
                    "fatal": False,
                    "status": {
                        "session": "busy",
                        "codec": "ready",
                        "player": "paused",
                        "deck": "capturing",
                        "capture": "finalizing",
                        "open_session_count": 4,
                        "foreground_output_session": ("10000000-0000-4000-8000-000000000004"),
                        "output_lease_pinned": True,
                    },
                    "diagnostic_id": "10000000-0000-4000-8000-000000000005",
                    "details": [
                        {
                            "key": "capture_id",
                            "value": "10000000-0000-4000-8000-000000000006",
                        }
                    ],
                },
            },
        },
    }
