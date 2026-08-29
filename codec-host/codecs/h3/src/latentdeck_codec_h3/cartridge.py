"""Read a Rust-validated H3 cartridge without extracting it to disk."""

from __future__ import annotations

import hashlib
import json
import math
import struct
import zipfile
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path

MAX_ARCHIVE_BYTES = 16_123_953_152
MAX_MANIFEST_BYTES = 1024 * 1024
MAX_SAFETENSORS_HEADER_BYTES = 1024 * 1024
H3_PAYLOAD = "payloads/h3.safetensors"
REQUIRED_ENTRIES = frozenset({"manifest.json", H3_PAYLOAD})
OPTIONAL_ENTRIES = frozenset({"preview.webp"})


class CartridgeLoadError(ValueError):
    """The worker could not bind the exact Rust-validated cartridge bytes."""


@dataclass(frozen=True)
class H3Cycle:
    """One codec-valid decode request."""

    cycle_index: int
    latent_start: int
    latent_count: int
    decoded_start_frame: int
    decoded_frame_count: int
    end_of_stream: bool


@dataclass(frozen=True)
class H3VideoSource:
    """Only the visual tensor bytes and validated presentation metadata."""

    cartridge_id: str
    archive_sha256: str
    storage_dtype: str
    shape: tuple[int, int, int, int, int]
    video_bytes: bytes
    width: int
    height: int
    frame_count: int
    frame_rate_numerator: int
    frame_rate_denominator: int

    @property
    def latent_slot_count(self) -> int:
        return self.shape[2]

    @property
    def cycle_count(self) -> int:
        return 1 + (self.latent_slot_count - 2) // 5

    def cycle(self, cycle_index: int) -> H3Cycle:
        """Return exact latent/frame ranges for one sequential cycle."""

        if not 0 <= cycle_index < self.cycle_count:
            raise CartridgeLoadError("decode cycle is outside the H3 clip")
        if cycle_index == 0:
            latent_start, latent_count = 0, 2
            decoded_start, decoded_count = 0, 5
        else:
            latent_start, latent_count = 2 + 5 * (cycle_index - 1), 5
            decoded_start, decoded_count = 5 + 17 * (cycle_index - 1), 17
        return H3Cycle(
            cycle_index=cycle_index,
            latent_start=latent_start,
            latent_count=latent_count,
            decoded_start_frame=decoded_start,
            decoded_frame_count=decoded_count,
            end_of_stream=cycle_index + 1 == self.cycle_count,
        )


def load_video_source(path: str | Path, expected_archive_sha256: str) -> H3VideoSource:
    """Bind one exact archive and read only its visual tensor entry range."""

    archive_path = Path(path)
    _validate_sha256(expected_archive_sha256)
    archive_size = archive_path.stat().st_size
    if not 1 <= archive_size <= MAX_ARCHIVE_BYTES:
        raise CartridgeLoadError("cartridge archive is outside the LC 0.1 byte limit")
    actual_archive_sha256 = _hash_path(archive_path)
    if actual_archive_sha256 != expected_archive_sha256:
        raise CartridgeLoadError("cartridge archive hash changed after validation")

    with zipfile.ZipFile(archive_path, "r") as archive:
        entries = archive.infolist()
        _validate_entries(entries)
        manifest = _read_json_entry(archive, "manifest.json", MAX_MANIFEST_BYTES)
        visual = _visual_manifest_descriptor(manifest)
        payload_info = archive.getinfo(H3_PAYLOAD)
        with archive.open(payload_info, "r") as payload:
            header_length = struct.unpack("<Q", _read_exact(payload, 8))[0]
            if not 1 <= header_length <= MAX_SAFETENSORS_HEADER_BYTES or header_length % 8:
                raise CartridgeLoadError("Safetensors header length is invalid")
            header = _strict_json(_read_exact(payload, header_length), "Safetensors header")
            tensor = _object(header.get("video"), "Safetensors video descriptor")
            dtype = tensor.get("dtype")
            shape = _shape(tensor.get("shape"))
            offsets = _offsets(tensor.get("data_offsets"))
            if dtype not in {"F16", "F32"}:
                raise CartridgeLoadError("Safetensors video dtype is unsupported")
            byte_width = 2 if dtype == "F16" else 4
            expected_bytes = math.prod(shape) * byte_width
            if offsets[1] - offsets[0] != expected_bytes:
                raise CartridgeLoadError("Safetensors video range disagrees with its shape")
            if visual["storage_dtype"] != dtype or tuple(visual["shape"]) != shape:
                raise CartridgeLoadError("manifest and Safetensors video descriptors disagree")
            payload.seek(8 + header_length + offsets[0])
            video_bytes = _read_exact(payload, expected_bytes)

    timing = _object(manifest.get("timing"), "timing")
    decoded = _object(timing.get("decoded_video"), "decoded video")
    frame_rate = _object(decoded.get("frame_rate"), "frame rate")
    source = H3VideoSource(
        cartridge_id=_text(manifest.get("cartridge_id"), "cartridge_id"),
        archive_sha256=actual_archive_sha256,
        storage_dtype=str(visual["storage_dtype"]),
        shape=shape,
        video_bytes=video_bytes,
        width=_positive_int(decoded.get("width"), "decoded width"),
        height=_positive_int(decoded.get("height"), "decoded height"),
        frame_count=_positive_int(decoded.get("frame_count"), "decoded frame count"),
        frame_rate_numerator=_positive_int(frame_rate.get("numerator"), "frame-rate numerator"),
        frame_rate_denominator=_positive_int(
            frame_rate.get("denominator"), "frame-rate denominator"
        ),
    )
    _validate_h3_source(source)
    return source


def _validate_entries(entries: list[zipfile.ZipInfo]) -> None:
    names = [entry.filename for entry in entries]
    if len(names) != len(set(names)):
        raise CartridgeLoadError("cartridge contains duplicate entries")
    actual = set(names)
    if not actual >= REQUIRED_ENTRIES or actual - REQUIRED_ENTRIES - OPTIONAL_ENTRIES:
        raise CartridgeLoadError("cartridge entry set is not LC 0.1")
    for entry in entries:
        if entry.compress_type != zipfile.ZIP_STORED or entry.flag_bits & 1:
            raise CartridgeLoadError("cartridge entries must be stored and unencrypted")


def _read_json_entry(archive: zipfile.ZipFile, name: str, maximum: int) -> dict[str, object]:
    entry = archive.getinfo(name)
    if not 1 <= entry.file_size <= maximum:
        raise CartridgeLoadError(f"{name} is outside its byte limit")
    with archive.open(entry, "r") as stream:
        return _strict_json(_read_exact(stream, entry.file_size), name)


def _strict_json(encoded: bytes, label: str) -> dict[str, object]:
    def unique_object(pairs: Iterable[tuple[str, object]]) -> dict[str, object]:
        value: dict[str, object] = {}
        for key, item in pairs:
            if key in value:
                raise CartridgeLoadError(f"{label} contains a duplicate JSON key")
            value[key] = item
        return value

    try:
        value = json.loads(encoded, object_pairs_hook=unique_object)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise CartridgeLoadError(f"{label} is not valid UTF-8 JSON") from error
    return _object(value, label)


def _visual_manifest_descriptor(manifest: dict[str, object]) -> dict[str, object]:
    tensors = manifest.get("tensors")
    if not isinstance(tensors, list):
        raise CartridgeLoadError("manifest tensors must be an array")
    visual = [
        _object(tensor, "tensor descriptor")
        for tensor in tensors
        if isinstance(tensor, dict) and tensor.get("name") == "video"
    ]
    if len(visual) != 1:
        raise CartridgeLoadError("manifest must describe exactly one video tensor")
    descriptor = visual[0]
    if descriptor.get("stream") != "visual" or descriptor.get("payload") != H3_PAYLOAD:
        raise CartridgeLoadError("manifest video tensor does not reference the H3 payload")
    return descriptor


def _shape(value: object) -> tuple[int, int, int, int, int]:
    if not isinstance(value, list) or len(value) != 5:
        raise CartridgeLoadError("H3 video shape must have five axes")
    axes = tuple(_positive_int(axis, "video shape axis") for axis in value)
    if axes[0] != 1 or axes[1] != 24 or axes[2] < 2 or (axes[2] - 2) % 5:
        raise CartridgeLoadError("H3 video shape must be [1,24,2+5n,H,W]")
    return axes  # type: ignore[return-value]


def _offsets(value: object) -> tuple[int, int]:
    if (
        not isinstance(value, list)
        or len(value) != 2
        or not all(isinstance(item, int) and not isinstance(item, bool) for item in value)
        or value[0] < 0
        or value[1] < value[0]
    ):
        raise CartridgeLoadError("Safetensors video offsets are invalid")
    return value[0], value[1]


def _validate_h3_source(source: H3VideoSource) -> None:
    expected_frames = 5 + 17 * ((source.latent_slot_count - 2) // 5)
    if source.frame_count != expected_frames:
        raise CartridgeLoadError("manifest decoded frame count disagrees with H3 cadence")
    if source.width != source.shape[4] * 16 or source.height != source.shape[3] * 16:
        raise CartridgeLoadError("manifest decoded geometry disagrees with H3 spatial cadence")
    if source.frame_rate_numerator != 24 or source.frame_rate_denominator != 1:
        raise CartridgeLoadError("LatentDeck H3 playback requires the 24 fps profile")


def _hash_path(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(64 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _read_exact(stream: object, byte_count: int) -> bytes:
    read = getattr(stream, "read", None)
    if not callable(read):
        raise CartridgeLoadError("cartridge entry is not readable")
    encoded = read(byte_count)
    if not isinstance(encoded, bytes) or len(encoded) != byte_count:
        raise CartridgeLoadError("cartridge entry ended before its declared byte range")
    return encoded


def _validate_sha256(value: str) -> None:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise CartridgeLoadError("expected archive SHA-256 is not canonical")


def _object(value: object, label: str) -> dict[str, object]:
    if not isinstance(value, dict) or not all(isinstance(key, str) for key in value):
        raise CartridgeLoadError(f"{label} must be an object")
    return value


def _text(value: object, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise CartridgeLoadError(f"{label} must be non-empty text")
    return value


def _positive_int(value: object, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
        raise CartridgeLoadError(f"{label} must be a positive integer")
    return value


__all__ = ["CartridgeLoadError", "H3Cycle", "H3VideoSource", "load_video_source"]
