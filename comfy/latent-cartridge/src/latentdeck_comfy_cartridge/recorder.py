"""H3-specific LC 0.1 recording without a ComfyUI import at module load."""

from __future__ import annotations

import hashlib
import json
import operator
import re
import tempfile
import uuid
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Protocol

SPEC_VERSION = "0.1.0"
PROFILE = "h3_av_latent"
PROFILE_VERSION = "0.1.0"
MAX_PAYLOAD_BYTES = 16_106_127_360
MAX_TEMPORAL_AXIS = 1_048_576
MAX_DECODED_AXIS = 4_096
MAX_DECODED_PIXELS = 16_777_216
MAX_SAFE_INTEGER = 9_007_199_254_740_991
SAFETENSORS_LENGTH_PREFIX_BYTES = 8
SAFETENSORS_MAX_HEADER_BYTES = 1024 * 1024
WINDOWS_RESERVED_BASENAMES = {
    "AUX",
    "CON",
    "NUL",
    "PRN",
    *(f"COM{index}" for index in range(1, 10)),
    *(f"LPT{index}" for index in range(1, 10)),
}


class RecorderError(ValueError):
    """Raised before the native SDK when an input cannot be H3 LC 0.1."""


class PackCallable(Protocol):
    def __call__(
        self,
        payload_path: Path,
        output_path: Path,
        preview_path: Path | None = None,
        *,
        cartridge_id: str | None = None,
        provenance: dict[str, object] | None = None,
        overwrite: bool = False,
    ) -> dict[str, object]: ...


class TensorWriter(Protocol):
    def __call__(self, path: Path, tensors: dict[str, object]) -> None: ...


@dataclass(frozen=True)
class TensorInfo:
    stream: str
    name: str
    value: object
    shape: tuple[int, ...]
    storage_dtype: str
    byte_width: int


@dataclass(frozen=True)
class InferredH3:
    tensors: tuple[TensorInfo, ...]
    decoded_width: int
    decoded_height: int
    frame_count: int


@dataclass(frozen=True)
class RecordingResult:
    output_path: Path
    receipt: dict[str, object]


class H3Recorder:
    """Create one validated LC archive from an existing Comfy H3 latent."""

    def __init__(
        self,
        *,
        pack: PackCallable | None = None,
        tensor_writer: TensorWriter | None = None,
        output_directory: Callable[[], str | Path] | None = None,
        cartridge_id_factory: Callable[[], str | uuid.UUID] | None = None,
        clock: Callable[[], datetime] | None = None,
    ) -> None:
        self._pack = pack or _sdk_pack
        self._tensor_writer = tensor_writer or _write_safetensors
        self._output_directory = output_directory or _comfy_output_directory
        self._cartridge_id_factory = cartridge_id_factory or uuid.uuid4
        self._clock = clock or (lambda: datetime.now(UTC))

    def record(
        self,
        latent: object,
        filename_prefix: str,
        *,
        prompt: object = None,
    ) -> RecordingResult:
        """Record an H3 latent and return the committed path and SDK receipt."""

        inferred = _infer_h3(latent)
        cartridge_id = _canonical_cartridge_id(self._cartridge_id_factory())
        output_path = _output_path(Path(self._output_directory()), filename_prefix, cartridge_id)
        if output_path.exists():
            raise RecorderError("refusing to overwrite an existing cartridge")

        output_path.parent.mkdir(parents=True, exist_ok=True)
        temporary_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                dir=output_path.parent,
                prefix=f".{output_path.stem}.",
                suffix=".safetensors",
                delete=False,
            ) as temporary:
                temporary_path = Path(temporary.name)

            tensors = {tensor.name: tensor.value for tensor in inferred.tensors}
            self._tensor_writer(temporary_path, tensors)
            _validate_payload_file(temporary_path)
            provenance = _provenance(prompt, _created_at(self._clock()))

            if output_path.exists():
                raise RecorderError("refusing to overwrite an existing cartridge")
            receipt = self._pack(
                temporary_path,
                output_path,
                None,
                cartridge_id=cartridge_id,
                provenance=provenance,
                overwrite=False,
            )
            if not isinstance(receipt, dict) or receipt.get("status") != "ok":
                raise RecorderError("Cartridge SDK returned an invalid pack receipt")
            if not output_path.is_file():
                raise RecorderError("Cartridge SDK reported success without an output file")
            return RecordingResult(output_path=output_path, receipt=receipt)
        finally:
            if temporary_path is not None:
                temporary_path.unlink(missing_ok=True)


def _infer_h3(latent: object) -> InferredH3:
    if not isinstance(latent, Mapping) or "samples" not in latent:
        raise RecorderError("H3 LATENT must be a mapping containing samples")
    samples = latent["samples"]
    streams = _latent_streams(samples)
    if not 1 <= len(streams) <= 2:
        raise RecorderError("H3 NestedTensor must contain video and optional audio")

    video = _tensor_info("visual", "video", streams[0])
    _validate_visual(video)
    frame_count = _decoded_frames(video.shape[2])
    decoded_height = _decoded_axis(video.shape[3], "height")
    decoded_width = _decoded_axis(video.shape[4], "width")
    if decoded_height * decoded_width > MAX_DECODED_PIXELS:
        raise RecorderError("H3 decoded pixel count exceeds the LC 0.1 ceiling")

    tensors = [video]
    if len(streams) == 2:
        audio = _tensor_info("audio", "audio", streams[1])
        _validate_audio(audio, frame_count)
        tensors.append(audio)
    tensors.sort(key=lambda tensor: tensor.name)
    _validate_tensor_bytes(tensors)
    return InferredH3(tuple(tensors), decoded_width, decoded_height, frame_count)


def _latent_streams(samples: object) -> tuple[object, ...]:
    if not bool(getattr(samples, "is_nested", False)):
        return (samples,)
    unbind = getattr(samples, "unbind", None)
    if not callable(unbind):
        raise RecorderError("H3 NestedTensor does not expose unbind()")
    try:
        streams = tuple(unbind())
    except Exception as error:
        raise RecorderError("could not unpack H3 NestedTensor streams") from error
    return streams


def _tensor_info(stream: str, name: str, value: object) -> TensorInfo:
    raw_shape = getattr(value, "shape", None)
    if raw_shape is None:
        raise RecorderError(f"H3 {name} stream does not expose a tensor shape")
    try:
        shape = tuple(operator.index(axis) for axis in raw_shape)
    except (TypeError, ValueError) as error:
        raise RecorderError(f"H3 {name} shape must contain integer axes") from error
    if any(axis <= 0 for axis in shape):
        raise RecorderError(f"H3 {name} shape axes must be positive")

    dtype, byte_width = _dtype(getattr(value, "dtype", None), name)
    return TensorInfo(stream, name, value, shape, dtype, byte_width)


def _dtype(value: object, tensor_name: str) -> tuple[str, int]:
    normalized = str(value).lower()
    if normalized in {"f16", "float16", "torch.float16"}:
        return "F16", 2
    if normalized in {"f32", "float32", "torch.float32"}:
        return "F32", 4
    raise RecorderError(f"H3 {tensor_name} dtype must be F16 or F32")


def _validate_visual(video: TensorInfo) -> None:
    if len(video.shape) != 5 or video.shape[0] != 1 or video.shape[1] != 24:
        raise RecorderError("H3 video shape must be [1,24,T,H,W]")
    if video.shape[2] > MAX_TEMPORAL_AXIS:
        raise RecorderError("H3 video temporal axis exceeds the LC 0.1 ceiling")


def _validate_audio(audio: TensorInfo, frame_count: int) -> None:
    if len(audio.shape) != 4 or audio.shape[0] != 1 or audio.shape[1] != 32 or audio.shape[2] != 2:
        raise RecorderError("H3 audio shape must be [1,32,2,T_audio]")
    expected_slots = (5 * frame_count + 1) // 3
    if audio.shape[3] != expected_slots:
        raise RecorderError(f"H3 audio T must be {expected_slots}")
    if audio.shape[3] > MAX_TEMPORAL_AXIS:
        raise RecorderError("H3 audio temporal axis exceeds the LC 0.1 ceiling")


def _decoded_frames(latent_slots: int) -> int:
    if latent_slots < 2 or (latent_slots - 2) % 5 != 0:
        raise RecorderError("H3 video T must be 2 + 5n")
    frame_count = 5 + 17 * ((latent_slots - 2) // 5)
    if frame_count > MAX_SAFE_INTEGER:
        raise RecorderError("H3 frame count exceeds the LC 0.1 integer ceiling")
    return frame_count


def _decoded_axis(latent_axis: int, label: str) -> int:
    decoded = latent_axis * 16
    if decoded > MAX_DECODED_AXIS:
        raise RecorderError(f"H3 decoded {label} exceeds the LC 0.1 ceiling")
    return decoded


def _validate_tensor_bytes(tensors: list[TensorInfo]) -> None:
    total = SAFETENSORS_LENGTH_PREFIX_BYTES + SAFETENSORS_MAX_HEADER_BYTES
    for tensor in tensors:
        elements = 1
        for axis in tensor.shape:
            elements *= axis
            if elements > MAX_PAYLOAD_BYTES:
                raise RecorderError("H3 tensor size exceeds the LC 0.1 payload ceiling")
        total += elements * tensor.byte_width
        if total > MAX_PAYLOAD_BYTES:
            raise RecorderError("H3 tensors exceed the LC 0.1 payload ceiling")


def _validate_payload_file(path: Path) -> None:
    byte_length = path.stat().st_size
    if byte_length <= SAFETENSORS_LENGTH_PREFIX_BYTES or byte_length > MAX_PAYLOAD_BYTES:
        raise RecorderError("temporary Safetensors payload is outside the LC 0.1 ceiling")


def _provenance(prompt: object, created_at: str) -> dict[str, object]:
    provenance: dict[str, object] = {
        "created_by": {"name": "comfyui-latent-cartridge", "version": SPEC_VERSION},
        "created_at": created_at,
        "source_kind": "comfyui_h3_latent",
        "source_metadata": {},
    }
    if prompt is None:
        return provenance
    try:
        canonical = json.dumps(
            prompt,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode()
    except (TypeError, ValueError):
        return provenance
    provenance["source_metadata"] = {"workflow_sha256": hashlib.sha256(canonical).hexdigest()}
    return provenance


def _created_at(value: datetime) -> str:
    if value.tzinfo is None or value.utcoffset() != UTC.utcoffset(value):
        raise RecorderError("recorder clock must return a UTC datetime")
    return value.astimezone(UTC).isoformat().replace("+00:00", "Z")


def _canonical_cartridge_id(value: str | uuid.UUID) -> str:
    text = str(value)
    try:
        parsed = uuid.UUID(text)
    except ValueError as error:
        raise RecorderError("cartridge ID factory returned an invalid UUID") from error
    if parsed.int == 0 or str(parsed) != text:
        raise RecorderError("cartridge ID must be a non-nil canonical lowercase UUID")
    return text


def _output_path(output_directory: Path, filename_prefix: str, cartridge_id: str) -> Path:
    root = output_directory.resolve()
    raw_prefix = filename_prefix.strip()
    prefix = Path(raw_prefix)
    if not raw_prefix or prefix.is_absolute() or bool(prefix.drive) or len(prefix.parts) != 1:
        raise RecorderError("filename_prefix must be a safe relative output prefix")
    if prefix.suffix.lower() == ".lc":
        prefix = prefix.with_suffix("")
    if not prefix.name or prefix.name in {".", ".."}:
        raise RecorderError("filename_prefix must be a safe relative output prefix")
    safe_basename = re.sub(r"[^A-Za-z0-9._-]+", "_", prefix.name).strip("._-")[:96]
    if not safe_basename:
        raise RecorderError("filename_prefix must contain a usable output basename")
    if safe_basename.split(".", maxsplit=1)[0].upper() in WINDOWS_RESERVED_BASENAMES:
        safe_basename = f"_{safe_basename}"
    relative = Path(f"{safe_basename}_{cartridge_id}.lc")
    output_path = (root / relative).resolve()
    if not output_path.is_relative_to(root):
        raise RecorderError("filename_prefix must be a safe relative output prefix")
    return output_path


def _sdk_pack(
    payload_path: Path,
    output_path: Path,
    preview_path: Path | None = None,
    *,
    cartridge_id: str | None = None,
    provenance: dict[str, object] | None = None,
    overwrite: bool = False,
) -> dict[str, object]:
    try:
        import latentdeck_cartridge as cartridge_sdk
    except ImportError as error:
        raise RecorderError("install the latentdeck-cartridge Python SDK") from error
    pack = getattr(cartridge_sdk, "pack_raw_h3", None)
    if not callable(pack):
        raise RecorderError("installed latentdeck-cartridge SDK does not expose pack_raw_h3()")
    return pack(
        payload_path,
        output_path,
        preview_path,
        cartridge_id=cartridge_id,
        provenance=provenance,
        overwrite=overwrite,
    )


def _resolve_safetensors_save_file() -> Callable[..., object]:
    """Resolve the host package first, then the bundle's private fallback."""

    try:
        from safetensors.torch import save_file
    except ImportError as error:
        try:
            from latentdeck_recorder_vendor.safetensors.torch import save_file
        except ImportError:
            raise RecorderError("install safetensors in the ComfyUI environment") from error
    return save_file


def _write_safetensors(path: Path, tensors: dict[str, object]) -> None:
    save_file = _resolve_safetensors_save_file()

    detached_tensors: dict[str, Any] = {}
    for name, tensor in tensors.items():
        detached = tensor.detach() if callable(getattr(tensor, "detach", None)) else tensor
        is_contiguous = getattr(detached, "is_contiguous", None)
        if callable(is_contiguous) and not is_contiguous():
            raise RecorderError(f"H3 {name} tensor must be contiguous for Safetensors")
        detached_tensors[name] = detached
    save_file(
        detached_tensors,
        str(path),
        metadata={"latentdeck_profile": f"{PROFILE}/{PROFILE_VERSION}"},
    )


def _comfy_output_directory() -> Path:
    try:
        import folder_paths
    except ImportError as error:
        raise RecorderError("ComfyUI folder_paths is unavailable") from error
    return Path(folder_paths.get_output_directory()) / "latentdeck" / "cartridges"
