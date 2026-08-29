"""Strict MessagePack framing shared by isolated codec workers."""

from __future__ import annotations

import math
import struct
import threading
import time
import uuid
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from typing import BinaryIO

import msgpack

PROTOCOL = "latentdeck.worker"
PROTOCOL_VERSION = 1
MAX_CONTROL_FRAME_BYTES = 262_144
MAX_BOOTSTRAP_BYTES = 4096
# Closed Worker Protocol 1 responses legitimately reach ten levels when a
# codec inspection contains adapters and profile descriptors. Sixteen remains
# a strict bound while leaving those typed envelopes representable.
MAX_DEPTH = 16
MAX_MAP_FIELDS = 64
MAX_ARRAY_ITEMS = 256
MAX_STRING_BYTES = 32_768
MAX_BINARY_BYTES = 64
MAX_D2_CAPTURE_LATENT_SLOTS = 1_048_576
MAX_D2_CAPTURE_VISUAL_BYTES = 15 * 1024 * 1024 * 1024
MAX_Q4_CAPTURE_LATENT_SLOTS = 1_048_576
MAX_Q4_CAPTURE_VISUAL_BYTES = 15 * 1024 * 1024 * 1024

D2_CONTROL_FIELDS = {
    "algorithm",
    "mix",
    "mode",
    "routing",
    "interaction",
    "preserve",
    "chaos",
    "xs1_channel_a",
    "xs1_channel_b",
    "xs1_angle_degrees",
    "xs2_radius",
    "xs3_high_gain",
    "xs4_epsilon",
    "xs5_routing",
    "temperature",
    "top_k",
    "sinkhorn_iterations",
}

Q4_CONTROL_FIELDS = {
    "algorithm",
    "interaction",
    "mode",
    "preserve",
    "influence_mode",
    "donor_weight_b",
    "donor_weight_c",
    "donor_weight_d",
    "triangle_x",
    "triangle_y",
    "xs5_routing",
    "temperature",
    "top_k",
    "sinkhorn_iterations",
    "chaos",
}


class ProtocolError(ValueError):
    """A bootstrap or control frame violated Worker Protocol 1."""


@dataclass(frozen=True)
class Bootstrap:
    """Single-use secret delivered over inherited stdin."""

    session_id: str
    pipe_name: str
    auth_token: bytes


class SequenceValidator:
    """Validate one ordered inbound peer stream."""

    def __init__(self, session_id: str) -> None:
        self._session_id = _uuid(session_id, "session_id")
        self._next_sequence = 1
        self._message_ids: set[str] = set()

    def validate_command(self, envelope: Mapping[str, object]) -> dict[str, object]:
        """Validate and consume exactly the next core command."""

        command = validate_command_envelope(envelope, self._session_id)
        sequence = _positive_int(envelope.get("sequence"), "sequence")
        if sequence != self._next_sequence:
            raise ProtocolError("command sequence is not contiguous")
        message_id = _uuid(envelope.get("message_id"), "message_id")
        if message_id in self._message_ids:
            raise ProtocolError("command message_id was already used")
        self._message_ids.add(message_id)
        self._next_sequence += 1
        return command


class EnvelopeWriter:
    """Serialize complete envelopes so heartbeat and command replies cannot interleave."""

    def __init__(self, stream: BinaryIO, session_id: str) -> None:
        self._stream = stream
        self._session_id = _uuid(session_id, "session_id")
        self._sequence = 1
        self._started = time.monotonic_ns()
        self._lock = threading.Lock()

    def event(
        self,
        name: str,
        payload: Mapping[str, object],
        *,
        caused_by: str | None = None,
    ) -> str:
        body: dict[str, object] = {"event": {"name": name, "payload": dict(payload)}}
        if caused_by is not None:
            body["caused_by"] = _uuid(caused_by, "caused_by")
        return self._write({"kind": "event", "body": body})

    def ack(self, reply_to: str, name: str, payload: Mapping[str, object]) -> str:
        body = {
            "reply_to": _uuid(reply_to, "reply_to"),
            "ack": {"name": name, "payload": dict(payload)},
        }
        return self._write({"kind": "ack", "body": body})

    def error(
        self,
        reply_to: str,
        name: str,
        *,
        code: str,
        message: str,
        retryable: bool,
        fatal: bool,
        worker_state: str,
        details: Iterable[Mapping[str, str]] = (),
    ) -> str:
        body = {
            "reply_to": _uuid(reply_to, "reply_to"),
            "name": name,
            "error": {
                "code": code,
                "message": message,
                "retryable": retryable,
                "fatal": fatal,
                "worker_state": worker_state,
                "diagnostic_id": str(uuid.uuid4()),
                "details": [dict(detail) for detail in details],
            },
        }
        return self._write({"kind": "error", "body": body})

    def _write(self, message: Mapping[str, object]) -> str:
        with self._lock:
            message_id = str(uuid.uuid4())
            envelope = {
                "protocol": PROTOCOL,
                "protocol_version": PROTOCOL_VERSION,
                "session_id": self._session_id,
                "sequence": self._sequence,
                "message_id": message_id,
                "sender_uptime_ns": time.monotonic_ns() - self._started,
                "message": dict(message),
            }
            write_frame(self._stream, envelope)
            self._sequence += 1
            return message_id


def encode_bootstrap(bootstrap: Bootstrap) -> bytes:
    """Encode the bounded stdin bootstrap record for a supervisor."""

    value = {
        "bootstrap_version": 1,
        "session_id": _uuid(bootstrap.session_id, "session_id"),
        "pipe_name": _text(bootstrap.pipe_name, "pipe_name", 512),
        "auth_token": _binary(bootstrap.auth_token, "auth_token", exact=32),
    }
    return _encode_length_prefixed(value, MAX_BOOTSTRAP_BYTES)


def read_bootstrap(stream: BinaryIO) -> Bootstrap:
    """Read and validate the single bounded worker bootstrap record."""

    value = _mapping(read_frame(stream, maximum=MAX_BOOTSTRAP_BYTES), "bootstrap")
    _exact_keys(
        value,
        {"bootstrap_version", "session_id", "pipe_name", "auth_token"},
        "bootstrap",
    )
    if value["bootstrap_version"] != 1:
        raise ProtocolError("unsupported bootstrap version")
    return Bootstrap(
        session_id=_uuid(value["session_id"], "session_id"),
        pipe_name=_text(value["pipe_name"], "pipe_name", 512),
        auth_token=_binary(value["auth_token"], "auth_token", exact=32),
    )


def write_frame(stream: BinaryIO, value: Mapping[str, object]) -> None:
    """Write one bounded u32-LE MessagePack map."""

    encoded = _encode_length_prefixed(value, MAX_CONTROL_FRAME_BYTES)
    stream.write(encoded)
    stream.flush()


def read_frame(stream: BinaryIO, *, maximum: int = MAX_CONTROL_FRAME_BYTES) -> dict[str, object]:
    """Read one bounded u32-LE MessagePack map and reject malformed values."""

    prefix = _read_exact(stream, 4, "frame length")
    byte_length = struct.unpack("<I", prefix)[0]
    if not 1 <= byte_length <= maximum:
        raise ProtocolError("control frame length is outside its bound")
    encoded = _read_exact(stream, byte_length, "frame payload")
    try:
        value = msgpack.unpackb(
            encoded,
            raw=False,
            strict_map_key=True,
            object_pairs_hook=_unique_map,
            max_str_len=MAX_STRING_BYTES,
            max_bin_len=MAX_BINARY_BYTES,
            max_array_len=MAX_ARRAY_ITEMS,
            max_map_len=MAX_MAP_FIELDS,
            max_ext_len=0,
        )
    except (msgpack.ExtraData, msgpack.FormatError, msgpack.StackError, ValueError) as error:
        raise ProtocolError("control frame contains invalid MessagePack") from error
    _validate_value(value)
    return _mapping(value, "control frame")


def validate_command_envelope(
    envelope: Mapping[str, object],
    session_id: str,
) -> dict[str, object]:
    """Validate static envelope fields and the closed command payload schema."""

    _exact_keys(
        envelope,
        {
            "protocol",
            "protocol_version",
            "session_id",
            "sequence",
            "message_id",
            "sender_uptime_ns",
            "message",
        },
        "envelope",
    )
    if envelope["protocol"] != PROTOCOL or envelope["protocol_version"] != PROTOCOL_VERSION:
        raise ProtocolError("worker protocol marker or version is unsupported")
    if _uuid(envelope["session_id"], "session_id") != session_id:
        raise ProtocolError("command belongs to another worker session")
    _positive_int(envelope["sequence"], "sequence")
    _uuid(envelope["message_id"], "message_id")
    _nonnegative_int(envelope["sender_uptime_ns"], "sender_uptime_ns")

    message = _mapping(envelope["message"], "message")
    _exact_keys(message, {"kind", "body"}, "message")
    if message["kind"] != "command":
        raise ProtocolError("worker accepts commands only")
    command = _mapping(message["body"], "command")
    _exact_keys(command, {"name", "payload"}, "command")
    name = _text(command["name"], "command name", 128)
    payload = _mapping(command["payload"], "command payload")
    _validate_command_payload(name, payload)
    return {"name": name, "payload": payload, "message_id": envelope["message_id"]}


def _validate_command_payload(name: str, payload: Mapping[str, object]) -> None:
    fields: dict[str, set[str]] = {
        "session.configure": {
            "selected_protocol_version",
            "app_version",
            "heartbeat_interval_ms",
            "heartbeat_hard_timeout_ms",
            "max_frame_bytes",
            "max_inflight_decode_batches",
        },
        "codec.inspect": set(),
        "codec.load": {
            "pack_id",
            "pack_version",
            "adapter_id",
            "profile",
            "device_ordinal",
            "assets",
        },
        "slot.load": {
            "slot_id",
            "cartridge_path",
            "cartridge_id",
            "expected_archive_sha256",
            "stream_generation",
        },
        "slot.reset": {"slot_id", "slot_revision", "new_stream_generation", "reason"},
        "slot.decode_cycle": {
            "slot_id",
            "slot_revision",
            "stream_generation",
            "cycle_index",
        },
        "ring.bind": {
            "layout_version",
            "mapping_handle",
            "mapping_bytes",
            "frames_ready_event_handle",
            "ring_id",
        },
        "deck.d2.load": {
            "deck_id",
            "operator_id",
            "operator_version",
            "source_a",
            "source_b",
            "controls",
            "transport",
            "seed",
            "stream_generation",
        },
        "deck.d2.process_slot": {"deck_id", "deck_revision", "stream_generation"},
        "deck.d2.reset": {"deck_id", "deck_revision", "new_stream_generation"},
        "deck.d2.restart": {"deck_id", "deck_revision"},
        "deck.d2.controls.set": {"deck_id", "deck_revision", "controls"},
        "deck.d2.transport.set": {"deck_id", "deck_revision", "transport"},
        "deck.d2.seed.set": {"deck_id", "deck_revision", "seed"},
        "deck.d2.status": set(),
        "deck.d2.capture.start": {
            "deck_id",
            "deck_revision",
            "capture_id",
            "mode",
            "temporary_root",
            "max_latent_slots",
            "max_visual_bytes",
        },
        "deck.d2.capture.stop": {"deck_id", "deck_revision", "capture_id"},
        "deck.d2.capture.status": {"deck_id", "deck_revision", "capture_id"},
        "deck.q4.load": {
            "deck_id",
            "operator_id",
            "operator_version",
            "source_a",
            "source_b",
            "source_c",
            "source_d",
            "roles",
            "controls",
            "transport",
            "seed",
            "stream_generation",
        },
        "deck.q4.process_slot": {"deck_id", "deck_revision", "stream_generation"},
        "deck.q4.reset": {"deck_id", "deck_revision", "new_stream_generation"},
        "deck.q4.restart": {"deck_id", "deck_revision"},
        "deck.q4.controls.set": {"deck_id", "deck_revision", "controls"},
        "deck.q4.roles.set": {"deck_id", "deck_revision", "roles"},
        "deck.q4.transport.set": {"deck_id", "deck_revision", "transport"},
        "deck.q4.seed.set": {"deck_id", "deck_revision", "seed"},
        "deck.q4.status": set(),
        "deck.q4.capture.start": {
            "deck_id",
            "deck_revision",
            "capture_id",
            "mode",
            "temporary_root",
            "max_latent_slots",
            "max_visual_bytes",
        },
        "deck.q4.capture.stop": {"deck_id", "deck_revision", "capture_id"},
        "deck.q4.capture.status": {"deck_id", "deck_revision", "capture_id"},
        "worker.status": set(),
        "metrics.get": set(),
        "worker.shutdown": {"reason"},
    }
    expected = fields.get(name)
    if expected is None:
        raise ProtocolError("worker command name is unknown")
    _exact_keys(payload, expected, f"{name} payload")
    if name == "codec.load":
        _exact_keys(
            _mapping(payload["profile"], "codec profile"),
            {"codec_family", "profile", "profile_version"},
            "codec profile",
        )
        assets = payload["assets"]
        if not isinstance(assets, list) or not 1 <= len(assets) <= 8:
            raise ProtocolError("codec assets must contain one to eight bindings")
        for asset in assets:
            _exact_keys(
                _mapping(asset, "codec asset"),
                {"asset_id", "path", "sha256", "byte_length"},
                "codec asset",
            )
    elif name == "deck.d2.load":
        _text(payload["deck_id"], "deck_id", 128)
        _text(payload["operator_id"], "operator_id", 128)
        _text(payload["operator_version"], "operator_version", 128)
        _validate_d2_source(payload["source_a"], "source_a")
        _validate_d2_source(payload["source_b"], "source_b")
        _validate_d2_controls(payload["controls"])
        _validate_d2_transport(payload["transport"])
        _safe_seed(payload["seed"], "seed")
        _positive_int(payload["stream_generation"], "stream_generation")
    elif name == "deck.d2.process_slot":
        _validate_d2_identity(payload, generation_field="stream_generation")
    elif name == "deck.d2.reset":
        _validate_d2_identity(payload, generation_field="new_stream_generation")
    elif name == "deck.d2.restart":
        _validate_d2_identity(payload)
    elif name == "deck.d2.controls.set":
        _validate_d2_identity(payload)
        _validate_d2_controls(payload["controls"])
    elif name == "deck.d2.transport.set":
        _validate_d2_identity(payload)
        _validate_d2_transport(payload["transport"])
    elif name == "deck.d2.seed.set":
        _validate_d2_identity(payload)
        _safe_seed(payload["seed"], "seed")
    elif name == "deck.d2.capture.start":
        _validate_d2_capture_identity(payload)
        _enum(payload["mode"], "capture mode", {"snapshot", "live_capture"})
        _text(payload["temporary_root"], "temporary_root", MAX_STRING_BYTES)
        _bounded_int(
            payload["max_latent_slots"],
            "max_latent_slots",
            2,
            MAX_D2_CAPTURE_LATENT_SLOTS,
        )
        _bounded_int(
            payload["max_visual_bytes"],
            "max_visual_bytes",
            1,
            MAX_D2_CAPTURE_VISUAL_BYTES,
        )
    elif name in {"deck.d2.capture.stop", "deck.d2.capture.status"}:
        _validate_d2_capture_identity(payload)
    elif name == "deck.q4.load":
        _text(payload["deck_id"], "deck_id", 128)
        _text(payload["operator_id"], "operator_id", 128)
        _text(payload["operator_version"], "operator_version", 128)
        for slot in "abcd":
            label = f"source_{slot}"
            _validate_d2_source(payload[label], label)
        _validate_q4_roles(payload["roles"])
        _validate_q4_controls(payload["controls"])
        _validate_q4_transport(payload["transport"])
        _safe_seed(payload["seed"], "seed")
        _positive_int(payload["stream_generation"], "stream_generation")
    elif name == "deck.q4.process_slot":
        _validate_q4_identity(payload, generation_field="stream_generation")
    elif name == "deck.q4.reset":
        _validate_q4_identity(payload, generation_field="new_stream_generation")
    elif name == "deck.q4.restart":
        _validate_q4_identity(payload)
    elif name == "deck.q4.controls.set":
        _validate_q4_identity(payload)
        _validate_q4_controls(payload["controls"])
    elif name == "deck.q4.roles.set":
        _validate_q4_identity(payload)
        _validate_q4_roles(payload["roles"])
    elif name == "deck.q4.transport.set":
        _validate_q4_identity(payload)
        _validate_q4_transport(payload["transport"])
    elif name == "deck.q4.seed.set":
        _validate_q4_identity(payload)
        _safe_seed(payload["seed"], "seed")
    elif name == "deck.q4.capture.start":
        _validate_q4_capture_identity(payload)
        _enum(payload["mode"], "capture mode", {"snapshot", "live_capture"})
        _text(payload["temporary_root"], "temporary_root", MAX_STRING_BYTES)
        _bounded_int(
            payload["max_latent_slots"],
            "max_latent_slots",
            2,
            MAX_Q4_CAPTURE_LATENT_SLOTS,
        )
        _bounded_int(
            payload["max_visual_bytes"],
            "max_visual_bytes",
            1,
            MAX_Q4_CAPTURE_VISUAL_BYTES,
        )
    elif name in {"deck.q4.capture.stop", "deck.q4.capture.status"}:
        _validate_q4_capture_identity(payload)


def _validate_d2_identity(
    payload: Mapping[str, object], *, generation_field: str | None = None
) -> None:
    _text(payload["deck_id"], "deck_id", 128)
    _positive_int(payload["deck_revision"], "deck_revision")
    if generation_field is not None:
        _positive_int(payload[generation_field], generation_field)


def _validate_d2_capture_identity(payload: Mapping[str, object]) -> None:
    _validate_d2_identity(payload)
    _uuid(payload["capture_id"], "capture_id")


def _validate_q4_identity(
    payload: Mapping[str, object], *, generation_field: str | None = None
) -> None:
    _text(payload["deck_id"], "deck_id", 128)
    _positive_int(payload["deck_revision"], "deck_revision")
    if generation_field is not None:
        _positive_int(payload[generation_field], generation_field)


def _validate_q4_capture_identity(payload: Mapping[str, object]) -> None:
    _validate_q4_identity(payload)
    _uuid(payload["capture_id"], "capture_id")


def _validate_d2_source(raw: object, label: str) -> None:
    source = _mapping(raw, label)
    _exact_keys(
        source,
        {"cartridge_path", "cartridge_id", "expected_archive_sha256"},
        label,
    )
    _text(source["cartridge_path"], f"{label}.cartridge_path", MAX_STRING_BYTES)
    _uuid(source["cartridge_id"], f"{label}.cartridge_id")
    digest = _text(source["expected_archive_sha256"], f"{label}.sha256", 64)
    if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
        raise ProtocolError(f"{label}.sha256 is not canonical")


def _validate_d2_controls(raw: object) -> None:
    controls = _mapping(raw, "D2 controls")
    _exact_keys(controls, D2_CONTROL_FIELDS, "D2 controls")
    _enum(controls["algorithm"], "algorithm", {"LINEAR", "XS1", "XS2", "XS3", "XS4", "XS5"})
    _number(controls["mix"], "mix", 0.0, 1.0)
    _enum(controls["mode"], "mode", {"HYBRIDIZE", "INTERACT"})
    _enum(controls["routing"], "routing", {"A", "B"})
    _number(controls["interaction"], "interaction", 0.0, 1.0)
    _number(controls["preserve"], "preserve", 0.0, 1.0)
    _number(controls["chaos"], "chaos", 0.0, 1.0)
    _bounded_int(controls["xs1_channel_a"], "xs1_channel_a", 0, 23)
    _bounded_int(controls["xs1_channel_b"], "xs1_channel_b", 0, 23)
    if controls["xs1_channel_a"] == controls["xs1_channel_b"]:
        raise ProtocolError("D2 XS1 channels must differ")
    _number(controls["xs1_angle_degrees"], "xs1_angle_degrees", -180.0, 180.0)
    _bounded_int(controls["xs2_radius"], "xs2_radius", 1, 8)
    _number(controls["xs3_high_gain"], "xs3_high_gain", -2.0, 2.0)
    _number(controls["xs4_epsilon"], "xs4_epsilon", 1e-8, 1e-3)
    _enum(controls["xs5_routing"], "xs5_routing", {"TOPK", "SINKHORN"})
    _number(controls["temperature"], "temperature", 0.02, 1.0)
    _bounded_int(controls["top_k"], "top_k", 1, 64)
    _bounded_int(controls["sinkhorn_iterations"], "sinkhorn_iterations", 2, 12)


def _validate_d2_transport(raw: object) -> None:
    transport = _mapping(raw, "D2 transport")
    _exact_keys(
        transport,
        {"playing_a", "playing_b", "loop_a", "loop_b"},
        "D2 transport",
    )
    if not all(isinstance(transport[name], bool) for name in transport):
        raise ProtocolError("D2 transport fields must be boolean")


def _validate_q4_roles(raw: object) -> None:
    roles = _mapping(raw, "Q4 roles")
    _exact_keys(roles, {"carrier", "donor_b", "donor_c", "donor_d"}, "Q4 roles")
    slots = [
        _enum(roles[name], name, {"A", "B", "C", "D"})
        for name in ("carrier", "donor_b", "donor_c", "donor_d")
    ]
    if len(set(slots)) != 4:
        raise ProtocolError("Q4 roles must be an exact A/B/C/D permutation")


def _validate_q4_controls(raw: object) -> None:
    controls = _mapping(raw, "Q4 controls")
    _exact_keys(controls, Q4_CONTROL_FIELDS, "Q4 controls")
    _enum(controls["algorithm"], "algorithm", {"LINEAR", "XS5"})
    _number(controls["interaction"], "interaction", 0.0, 1.0)
    _enum(controls["mode"], "mode", {"HYBRIDIZE", "INTERACT"})
    _number(controls["preserve"], "preserve", 0.0, 1.0)
    influence_mode = _enum(controls["influence_mode"], "influence_mode", {"MANUAL", "TRIANGLE"})
    for name in (
        "donor_weight_b",
        "donor_weight_c",
        "donor_weight_d",
        "triangle_x",
        "triangle_y",
        "chaos",
    ):
        _number(controls[name], name, 0.0, 1.0)
    _enum(controls["xs5_routing"], "xs5_routing", {"TOPK", "SINKHORN"})
    _number(controls["temperature"], "temperature", 0.02, 1.0)
    _bounded_int(controls["top_k"], "top_k", 1, 64)
    _bounded_int(controls["sinkhorn_iterations"], "sinkhorn_iterations", 2, 12)
    if influence_mode == "MANUAL":
        total = sum(
            float(controls[name]) for name in ("donor_weight_b", "donor_weight_c", "donor_weight_d")
        )
        if total == 0.0:
            raise ProtocolError("at least one Q4 manual donor weight must be positive")
    else:
        x = float(controls["triangle_x"])
        y = float(controls["triangle_y"])
        if min(1.0 - x - 0.5 * y, x - 0.5 * y, y) < -1e-12:
            raise ProtocolError("Q4 triangle point lies outside the influence field")


def _validate_q4_transport(raw: object) -> None:
    transport = _mapping(raw, "Q4 transport")
    _exact_keys(
        transport,
        {
            "playing_a",
            "playing_b",
            "playing_c",
            "playing_d",
            "loop_a",
            "loop_b",
            "loop_c",
            "loop_d",
        },
        "Q4 transport",
    )
    if not all(isinstance(transport[name], bool) for name in transport):
        raise ProtocolError("Q4 transport fields must be boolean")


def _enum(value: object, label: str, allowed: set[str]) -> str:
    text = _text(value, label, 128)
    if text not in allowed:
        raise ProtocolError(f"{label} is outside the closed enum")
    return text


def _number(value: object, label: str, minimum: float, maximum: float) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ProtocolError(f"{label} must be numeric")
    parsed = float(value)
    if not math.isfinite(parsed) or not minimum <= parsed <= maximum:
        raise ProtocolError(f"{label} is outside its finite bound")
    return parsed


def _bounded_int(value: object, label: str, minimum: int, maximum: int) -> int:
    integer = _nonnegative_int(value, label)
    if not minimum <= integer <= maximum:
        raise ProtocolError(f"{label} is outside its integer bound")
    return integer


def _safe_seed(value: object, label: str) -> int:
    return _bounded_int(value, label, 0, 9_007_199_254_740_991)


def _encode_length_prefixed(value: Mapping[str, object], maximum: int) -> bytes:
    _validate_value(value)
    encoded = msgpack.packb(value, use_bin_type=True)
    if not 1 <= len(encoded) <= maximum:
        raise ProtocolError("MessagePack record exceeds its byte limit")
    return struct.pack("<I", len(encoded)) + encoded


def _unique_map(pairs: Iterable[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        if not isinstance(key, str) or key in value:
            raise ProtocolError("MessagePack map key is invalid or duplicated")
        value[key] = item
    return value


def _validate_value(value: object, depth: int = 0) -> None:
    if depth > MAX_DEPTH:
        raise ProtocolError("MessagePack nesting exceeds the protocol limit")
    if value is None or isinstance(value, msgpack.ExtType):
        raise ProtocolError("MessagePack nil and extension values are forbidden")
    if isinstance(value, bool):
        return
    if isinstance(value, int):
        if not 0 <= value <= 0xFFFF_FFFF_FFFF_FFFF:
            raise ProtocolError("MessagePack integer is outside the u64 range")
        return
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ProtocolError("MessagePack float must be finite")
        return
    if isinstance(value, str):
        if len(value.encode()) > MAX_STRING_BYTES or "\0" in value:
            raise ProtocolError("MessagePack text exceeds its UTF-8 limit")
        return
    if isinstance(value, bytes):
        if len(value) > MAX_BINARY_BYTES:
            raise ProtocolError("MessagePack binary exceeds its limit")
        return
    if isinstance(value, list):
        if len(value) > MAX_ARRAY_ITEMS:
            raise ProtocolError("MessagePack array exceeds its item limit")
        for item in value:
            _validate_value(item, depth + 1)
        return
    if isinstance(value, dict):
        if len(value) > MAX_MAP_FIELDS:
            raise ProtocolError("MessagePack map exceeds its field limit")
        for key, item in value.items():
            if not isinstance(key, str):
                raise ProtocolError("MessagePack map keys must be UTF-8 strings")
            _validate_value(key, depth + 1)
            _validate_value(item, depth + 1)
        return
    raise ProtocolError("MessagePack value type is forbidden")


def _read_exact(stream: BinaryIO, byte_count: int, label: str) -> bytes:
    chunks = bytearray()
    while len(chunks) < byte_count:
        chunk = stream.read(byte_count - len(chunks))
        if not chunk:
            raise ProtocolError(f"{label} ended early")
        chunks.extend(chunk)
    return bytes(chunks)


def _exact_keys(value: Mapping[str, object], expected: set[str], label: str) -> None:
    if set(value) != expected:
        raise ProtocolError(f"{label} fields do not match the closed schema")


def _mapping(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise ProtocolError(f"{label} must be a string-keyed map")
    return value


def _uuid(value: object, label: str) -> str:
    text = _text(value, label, 36)
    try:
        parsed = uuid.UUID(text)
    except ValueError as error:
        raise ProtocolError(f"{label} is not a UUID") from error
    if parsed.int == 0 or str(parsed) != text:
        raise ProtocolError(f"{label} is not a canonical non-nil UUID")
    return text


def _text(value: object, label: str, maximum: int) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > maximum or "\0" in value:
        raise ProtocolError(f"{label} is not bounded UTF-8 text")
    return value


def _binary(value: object, label: str, *, exact: int) -> bytes:
    if not isinstance(value, bytes) or len(value) != exact:
        raise ProtocolError(f"{label} must contain exactly {exact} bytes")
    return value


def _positive_int(value: object, label: str) -> int:
    integer = _nonnegative_int(value, label)
    if integer == 0:
        raise ProtocolError(f"{label} must be positive")
    return integer


def _nonnegative_int(value: object, label: str) -> int:
    valid = isinstance(value, int) and not isinstance(value, bool)
    if not valid or not 0 <= value <= 0xFFFF_FFFF_FFFF_FFFF:
        raise ProtocolError(f"{label} must be a non-negative u64")
    return value


__all__ = [
    "Bootstrap",
    "EnvelopeWriter",
    "ProtocolError",
    "SequenceValidator",
    "encode_bootstrap",
    "read_bootstrap",
    "read_frame",
    "validate_command_envelope",
    "write_frame",
]
