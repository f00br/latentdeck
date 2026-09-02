"""Bounded disk spool for post-operator H3 F16 resampling.

The realtime operator emits one ``[1,24,1,H,W]`` slot at a time.  This module
never accumulates those slots in RAM and never transports them through control
IPC.  It first writes a capture-owned slot-major partial, then streams the data
into canonical ``[1,24,T,H,W]`` Safetensors order at a codec-valid boundary.
"""

from __future__ import annotations

import hashlib
import json
import os
import struct
import uuid
from collections.abc import Callable
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO

import torch

MAX_H3_PAYLOAD_BYTES = 15 * 1024 * 1024 * 1024
MAX_H3_TEMPORAL_AXIS = 1_048_576
MAX_H3_LATENT_AXIS = 256
H3_CHANNELS = 24
F16_BYTES = 2


class ResampleSpoolError(RuntimeError):
    """A capture violated its bounded pre-decode resample contract."""


@dataclass(frozen=True, slots=True)
class ResampleAudioSource:
    """Exact validated carrier-audio bytes streamed into a resample payload."""

    storage_dtype: str
    shape: tuple[int, int, int, int]
    byte_length: int
    copy_to: Callable[[BinaryIO], int]

    def __post_init__(self) -> None:
        if self.storage_dtype not in {"F16", "F32"}:
            raise ResampleSpoolError("audio storage dtype must be F16 or F32")
        if (
            len(self.shape) != 4
            or self.shape[:3] != (1, 32, 2)
            or isinstance(self.shape[3], bool)
            or not isinstance(self.shape[3], int)
            or self.shape[3] <= 0
        ):
            raise ResampleSpoolError("audio shape must be [1,32,2,T]")
        byte_width = F16_BYTES if self.storage_dtype == "F16" else 4
        expected_bytes = 1 * 32 * 2 * self.shape[3] * byte_width
        if self.byte_length != expected_bytes:
            raise ResampleSpoolError("audio byte length disagrees with dtype and shape")
        if not callable(self.copy_to):
            raise ResampleSpoolError("audio source copy callback is invalid")


@dataclass(frozen=True, slots=True)
class ResampleAudioReceipt:
    """Audio tensor descriptor committed to a resample Safetensors spool."""

    storage_dtype: str
    shape: tuple[int, int, int, int]
    byte_length: int


@dataclass(frozen=True, slots=True)
class ResampleSpoolReceipt:
    """Trusted local description of one completed visual Safetensors spool."""

    capture_id: str
    payload_path: Path
    byte_length: int
    sha256: str
    storage_dtype: str
    shape: tuple[int, int, int, int, int]
    decoded_frame_count: int
    audio: ResampleAudioReceipt | None


class H3ResampleSpool:
    """Incrementally persist exact post-operator F16 H3 visual slots."""

    def __init__(
        self,
        temporary_root: str | Path,
        capture_id: str,
        latent_height: int,
        latent_width: int,
        *,
        max_latent_slots: int = MAX_H3_TEMPORAL_AXIS,
        max_visual_bytes: int = MAX_H3_PAYLOAD_BYTES,
    ) -> None:
        self._capture_id = _canonical_capture_id(capture_id)
        self._height = _bounded_axis(latent_height, "latent height")
        self._width = _bounded_axis(latent_width, "latent width")
        self._max_latent_slots = _positive_limit(
            max_latent_slots,
            "latent-slot limit",
            MAX_H3_TEMPORAL_AXIS,
        )
        self._max_visual_bytes = _positive_limit(
            max_visual_bytes,
            "visual-byte limit",
            MAX_H3_PAYLOAD_BYTES,
        )
        self._plane_bytes = self._height * self._width * F16_BYTES
        self._slot_bytes = H3_CHANNELS * self._plane_bytes
        if self._slot_bytes > self._max_visual_bytes:
            raise ResampleSpoolError("one H3 slot exceeds the visual-byte limit")

        root = Path(temporary_root)
        if root.is_symlink() or not root.is_dir():
            raise ResampleSpoolError("resample temporary root is not a directory")
        self._root = root.resolve(strict=True)
        self._raw_path = self._root / f"{self._capture_id}.visual.f16.partial"
        self._payload_path = self._root / f"{self._capture_id}.safetensors.partial"
        self._raw: BinaryIO | None = self._raw_path.open("x+b")
        self._latent_slots = 0
        self._finished = False

    @property
    def capture_id(self) -> str:
        return self._capture_id

    @property
    def raw_path(self) -> Path:
        return self._raw_path

    @property
    def latent_slots(self) -> int:
        return self._latent_slots

    @property
    def can_finish(self) -> bool:
        """Whether the staged slot count is the next codec-valid H3 boundary."""

        return self._latent_slots >= 2 and (self._latent_slots - 2) % 5 == 0

    def append_slot(self, slot: torch.Tensor) -> None:
        """Append one exact finite F16 operator output without retaining it."""

        raw = self._require_open()
        expected_shape = (1, H3_CHANNELS, 1, self._height, self._width)
        if not isinstance(slot, torch.Tensor):
            raise ResampleSpoolError("post-operator slot must be a torch tensor")
        if tuple(slot.shape) != expected_shape:
            raise ResampleSpoolError(f"post-operator slot shape must be {expected_shape}")
        if slot.dtype != torch.float16:
            raise ResampleSpoolError("post-operator slot storage dtype must remain F16")
        if not bool(torch.isfinite(slot).all().item()):
            raise ResampleSpoolError("post-operator slot must contain only finite values")

        next_slots = self._latent_slots + 1
        next_bytes = next_slots * self._slot_bytes
        if next_slots > self._max_latent_slots or next_bytes > self._max_visual_bytes:
            raise ResampleSpoolError("resample spool limit would be exceeded")

        encoded = slot.detach().to(device="cpu").contiguous().numpy().tobytes(order="C")
        if len(encoded) != self._slot_bytes:
            raise ResampleSpoolError("post-operator slot byte length is invalid")
        try:
            written = raw.write(encoded)
        except OSError as error:
            raise ResampleSpoolError("resample spool write failed") from error
        if written != len(encoded):
            raise ResampleSpoolError("resample spool write was incomplete")
        self._latent_slots = next_slots

    def finish(self, *, audio: ResampleAudioSource | None = None) -> ResampleSpoolReceipt:
        """Finalize a complete ``T=2+5n`` visual Safetensors partial."""

        raw = self._require_open()
        if not self.can_finish:
            raise ResampleSpoolError("H3 resample length must be 2 + 5n latent slots")

        visual_bytes = self._latent_slots * self._slot_bytes
        header = _safetensors_header(
            shape=(1, H3_CHANNELS, self._latent_slots, self._height, self._width),
            visual_bytes=visual_bytes,
            audio=audio,
        )
        audio_bytes = 0 if audio is None else audio.byte_length
        total_bytes = 8 + len(header) + visual_bytes + audio_bytes
        if total_bytes > MAX_H3_PAYLOAD_BYTES:
            raise ResampleSpoolError("completed Safetensors payload exceeds the H3 limit")

        try:
            raw.flush()
            os.fsync(raw.fileno())
            with self._payload_path.open("xb") as payload:
                payload.write(struct.pack("<Q", len(header)))
                payload.write(header)
                self._copy_tensor_order(raw, payload)
                if audio is not None:
                    audio_start = payload.tell()
                    reported_bytes = audio.copy_to(payload)
                    actual_audio_bytes = payload.tell() - audio_start
                    if (
                        reported_bytes != audio.byte_length
                        or actual_audio_bytes != audio.byte_length
                    ):
                        raise ResampleSpoolError("audio source copy was incomplete")
                payload.flush()
                os.fsync(payload.fileno())
        except (OSError, ResampleSpoolError) as error:
            self._remove_payload_partial()
            raise ResampleSpoolError("Safetensors spool finalization failed") from error

        actual_size = self._payload_path.stat().st_size
        if actual_size != total_bytes:
            self._remove_payload_partial()
            raise ResampleSpoolError("Safetensors spool byte length is invalid")
        digest = _hash_path(self._payload_path)
        raw.close()
        self._raw = None
        self._raw_path.unlink()
        self._finished = True
        decoded_frame_count = 5 + 17 * ((self._latent_slots - 2) // 5)
        return ResampleSpoolReceipt(
            capture_id=self._capture_id,
            payload_path=self._payload_path,
            byte_length=actual_size,
            sha256=digest,
            storage_dtype="F16",
            shape=(1, H3_CHANNELS, self._latent_slots, self._height, self._width),
            decoded_frame_count=decoded_frame_count,
            audio=(
                None
                if audio is None
                else ResampleAudioReceipt(
                    storage_dtype=audio.storage_dtype,
                    shape=audio.shape,
                    byte_length=audio.byte_length,
                )
            ),
        )

    def abort(self) -> None:
        """Remove only the two files owned by this exact capture."""

        raw = self._raw
        if raw is not None:
            raw.close()
            self._raw = None
        self._remove_payload_partial()
        with suppress(FileNotFoundError):
            self._raw_path.unlink()

    def _require_open(self) -> BinaryIO:
        if self._finished:
            raise ResampleSpoolError("resample spool is already finalized")
        if self._raw is None or self._raw.closed:
            raise ResampleSpoolError("resample spool is closed")
        return self._raw

    def _copy_tensor_order(self, raw: BinaryIO, payload: BinaryIO) -> None:
        # Captures arrive slot-major (T,C,H,W), while the declared tensor is
        # contiguous channel-major (C,T,H,W).  Seek one spatial plane at a time
        # so the conversion remains bounded regardless of capture duration.
        for channel in range(H3_CHANNELS):
            for latent_slot in range(self._latent_slots):
                source_offset = latent_slot * self._slot_bytes + channel * self._plane_bytes
                raw.seek(source_offset)
                plane = raw.read(self._plane_bytes)
                if len(plane) != self._plane_bytes:
                    raise ResampleSpoolError("resample raw partial ended unexpectedly")
                if payload.write(plane) != len(plane):
                    raise ResampleSpoolError("Safetensors spool write was incomplete")

    def _remove_payload_partial(self) -> None:
        with suppress(FileNotFoundError):
            self._payload_path.unlink()


def _canonical_capture_id(value: str) -> str:
    try:
        parsed = uuid.UUID(value)
    except (AttributeError, ValueError) as error:
        raise ResampleSpoolError("capture_id must be a canonical UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise ResampleSpoolError("capture_id must be a canonical non-nil UUID")
    return value


def _bounded_axis(value: int, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 256:
        raise ResampleSpoolError(f"{label} is outside the H3 profile limit")
    return value


def _positive_limit(value: int, label: str, ceiling: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= ceiling:
        raise ResampleSpoolError(f"{label} is invalid")
    return value


def _safetensors_header(
    *,
    shape: tuple[int, ...],
    visual_bytes: int,
    audio: ResampleAudioSource | None,
) -> bytes:
    value = {
        "video": {
            "dtype": "F16",
            "shape": list(shape),
            "data_offsets": [0, visual_bytes],
        }
    }
    if audio is not None:
        value["audio"] = {
            "dtype": audio.storage_dtype,
            "shape": list(audio.shape),
            "data_offsets": [visual_bytes, visual_bytes + audio.byte_length],
        }
    encoded = json.dumps(
        value,
        ensure_ascii=True,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    padding = (-len(encoded)) % 8
    return encoded + b" " * padding


def _hash_path(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(64 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


__all__ = [
    "H3ResampleSpool",
    "ResampleAudioReceipt",
    "ResampleAudioSource",
    "ResampleSpoolError",
    "ResampleSpoolReceipt",
]
