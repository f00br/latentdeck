"""Authoritative LC and raw-H3 I/O adapters for the Comfy research surface.

This module deliberately delegates every archive and Safetensors read to the
Rust-backed :mod:`latentdeck_cartridge` SDK.  It does not contain a ZIP or
Safetensors parser and never extracts an LC payload to disk.
"""

from __future__ import annotations

import json
import math
import re
import tempfile
import uuid
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

import torch

from .workflow_metadata import (
    derive_resample_inputs,
    initialize_lc_metadata,
    initialize_raw_metadata,
)

TOOLKIT_IO_VERSION = "0.1.0"
LATENTDECK_METADATA_KEY = "latentdeck"
MAX_TOOLKIT_VISUAL_VALUES = 50_331_648
MAX_TOOLKIT_TENSOR_BYTES = MAX_TOOLKIT_VISUAL_VALUES * 4


class ToolkitIOError(ValueError):
    """Stable preflight error raised at the Toolkit I/O boundary."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


class CartridgeSdk(Protocol):
    """Narrow public SDK surface consumed by the Toolkit."""

    def read_h3(
        self,
        path: str | Path,
        *,
        max_visual_values: int | None = None,
        max_tensor_bytes: int | None = None,
    ) -> dict[str, object]: ...

    def inspect(self, path: str | Path) -> dict[str, object]: ...

    def validate(self, path: str | Path) -> dict[str, object]: ...

    def inspect_raw_h3(self, path: str | Path) -> dict[str, object]: ...

    def read_raw_h3(
        self,
        path: str | Path,
        *,
        max_visual_values: int | None = None,
        max_tensor_bytes: int | None = None,
    ) -> dict[str, object]: ...

    def pack_raw_h3(
        self,
        payload_path: str | Path,
        output_path: str | Path,
        preview_path: str | Path | None = None,
        *,
        cartridge_id: str | None = None,
        provenance: Mapping[str, object] | None = None,
        overwrite: bool = False,
    ) -> dict[str, object]: ...

    def pack(
        self,
        manifest: dict[str, object],
        payload_path: str | Path,
        output_path: str | Path,
        preview_path: str | Path | None = None,
        *,
        overwrite: bool = False,
    ) -> dict[str, object]: ...


class H3AVSamples:
    """Comfy NestedTensor-compatible carrier for H3 video and audio streams."""

    is_nested = True

    def __init__(self, streams: tuple[torch.Tensor, torch.Tensor]) -> None:
        if not isinstance(streams, tuple) or len(streams) != 2:
            raise ToolkitIOError(
                "latent.av_streams_invalid", "H3 AV samples require (video, audio)"
            )
        self._streams = streams

    def unbind(self) -> tuple[torch.Tensor, torch.Tensor]:
        """Return video first and audio second, matching H3 NestedTensor."""

        return self._streams


def _make_av_samples(video: torch.Tensor, audio: torch.Tensor) -> object:
    """Use Comfy's native carrier when hosted, with a test/library fallback."""

    try:
        from comfy.nested_tensor import NestedTensor
    except ImportError:
        return H3AVSamples((video, audio))
    try:
        return NestedTensor((video, audio))
    except Exception as error:
        raise ToolkitIOError(
            "latent.nested_create_failed", "Comfy NestedTensor rejected H3 video/audio streams"
        ) from error


@dataclass(frozen=True, slots=True)
class LoadedH3Latent:
    """One workflow LATENT plus a JSON-safe human/research report."""

    latent: dict[str, object]
    report: dict[str, object]


@dataclass(frozen=True, slots=True)
class SavedCartridge:
    output_path: Path
    receipt: dict[str, object]
    manifest: dict[str, object]


def _mapping(value: object, *, code: str, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise ToolkitIOError(code, f"{label} must be an object")
    return value


def _positive_shape(value: object, *, tensor_name: str) -> tuple[int, ...]:
    if not isinstance(value, (list, tuple)) or not value:
        raise ToolkitIOError("tensor.shape_invalid", f"{tensor_name} shape must be an array")
    shape: list[int] = []
    for axis in value:
        if isinstance(axis, bool) or not isinstance(axis, int) or axis <= 0:
            raise ToolkitIOError(
                "tensor.shape_invalid", f"{tensor_name} shape axes must be positive integers"
            )
        shape.append(axis)
    return tuple(shape)


def _tensor_from_wire(name: str, descriptor: object) -> torch.Tensor:
    raw = _mapping(descriptor, code="tensor.descriptor_invalid", label=f"{name} descriptor")
    dtype_name = raw.get("dtype")
    if dtype_name == "F16":
        dtype = torch.float16
        byte_width = 2
    elif dtype_name == "F32":
        dtype = torch.float32
        byte_width = 4
    else:
        raise ToolkitIOError("tensor.dtype_forbidden", f"{name} dtype must be F16 or F32")
    shape = _positive_shape(raw.get("shape"), tensor_name=name)
    expected = math.prod(shape) * byte_width
    if expected > MAX_TOOLKIT_TENSOR_BYTES:
        raise ToolkitIOError("runtime_limit_exceeded", f"{name} exceeds the Toolkit load ceiling")
    data = raw.get("data")
    if not isinstance(data, bytes):
        raise ToolkitIOError("tensor.bytes_invalid", f"{name} data must be bytes from the SDK")
    if len(data) != expected:
        raise ToolkitIOError(
            "tensor.byte_length_mismatch",
            f"{name} has {len(data)} bytes but its dtype and shape require {expected}",
        )
    # bytearray gives PyTorch owned writable CPU backing without changing dtype
    # or shape.  The H3 visual runtime cast below is the sole profile-approved
    # conversion.
    return torch.frombuffer(bytearray(data), dtype=dtype).reshape(shape)


def _runtime_casts(video: torch.Tensor) -> tuple[torch.Tensor, list[dict[str, str]]]:
    if video.dtype == torch.float16:
        return video, []
    if video.dtype != torch.float32:
        raise ToolkitIOError("tensor.dtype_forbidden", "video dtype must be F16 or F32")
    return video.to(dtype=torch.float16), [
        {
            "tensor": "video",
            "from": "F32",
            "to": "F16",
            "authority": "minimax_h3/h3_av_latent/0.1.0",
        }
    ]


def _report_from_lc(
    manifest: Mapping[str, object],
    validation: Mapping[str, object],
    runtime_casts: list[dict[str, str]],
) -> dict[str, object]:
    archive_sha256 = validation.get("archive_sha256")
    archive_bytes = validation.get("archive_bytes")
    return {
        "schema_version": TOOLKIT_IO_VERSION,
        "source_kind": "latent_cartridge",
        "validation": dict(validation),
        "archive": {"sha256": archive_sha256, "byte_length": archive_bytes},
        "cartridge_id": manifest.get("cartridge_id"),
        "codec": manifest.get("codec"),
        "tensors": manifest.get("tensors"),
        "timing": manifest.get("timing"),
        "provenance": manifest.get("provenance"),
        "parent_cartridges": manifest.get("parent_cartridges"),
        "operation_history": manifest.get("operation_history"),
        "runtime_casts": runtime_casts,
        "compatibility": {
            "status": "compatible",
            "target": "latentdeck_comfy_toolkit/0.1.0",
            "reasons": [],
        },
    }


def _default_sdk() -> Any:
    try:
        import latentdeck_cartridge as sdk
    except ImportError as error:
        raise ToolkitIOError(
            "sdk.missing", "install the latentdeck-cartridge Python SDK"
        ) from error
    return sdk


def load_lc(path: str | Path, *, sdk: CartridgeSdk | None = None) -> LoadedH3Latent:
    """Fully validate and load one H3 LC through the retained-handle Rust reader."""

    source = Path(path)
    selected_sdk = sdk or _default_sdk()
    reader = getattr(selected_sdk, "read_h3", None)
    if not callable(reader):
        raise ToolkitIOError("sdk.incompatible", "Cartridge SDK does not expose read_h3()")
    response = _mapping(
        reader(
            source,
            max_visual_values=MAX_TOOLKIT_VISUAL_VALUES,
            max_tensor_bytes=MAX_TOOLKIT_TENSOR_BYTES,
        ),
        code="sdk.response_invalid",
        label="read_h3 response",
    )
    if response.get("status") != "ok":
        raise ToolkitIOError("sdk.response_invalid", "read_h3 did not return status=ok")
    manifest = _mapping(
        response.get("manifest"), code="sdk.response_invalid", label="LC manifest"
    )
    validation = _mapping(
        response.get("validation"), code="sdk.response_invalid", label="validation receipt"
    )
    if validation.get("validation_level") != "full":
        raise ToolkitIOError(
            "sdk.validation_incomplete", "LC tensor access requires full validation"
        )
    tensors = _mapping(
        response.get("tensors"), code="sdk.response_invalid", label="tensor mapping"
    )
    video = _tensor_from_wire("video", tensors.get("video"))
    video, runtime_casts = _runtime_casts(video)
    audio_descriptor = tensors.get("audio")
    samples: torch.Tensor | H3AVSamples
    if audio_descriptor is None:
        samples = video
    else:
        samples = _make_av_samples(video, _tensor_from_wire("audio", audio_descriptor))
    report = _report_from_lc(manifest, validation, runtime_casts)
    latent = initialize_lc_metadata(
        {"samples": samples},
        manifest=manifest,
        validation=validation,
    )
    return LoadedH3Latent(latent=latent, report=report)


def inspect_lc(path: str | Path, *, sdk: CartridgeSdk | None = None) -> dict[str, object]:
    """Fully validate LC metadata without materializing its tensor bytes."""

    source = Path(path)
    selected_sdk = sdk or _default_sdk()
    inspect = getattr(selected_sdk, "inspect", None)
    validate = getattr(selected_sdk, "validate", None)
    if not callable(inspect) or not callable(validate):
        raise ToolkitIOError(
            "sdk.incompatible", "Cartridge SDK must expose inspect() and validate()"
        )
    inspection = _mapping(
        inspect(source), code="sdk.response_invalid", label="inspect response"
    )
    if inspection.get("status") != "ok":
        raise ToolkitIOError("sdk.response_invalid", "inspect did not return status=ok")
    manifest = _mapping(
        inspection.get("manifest"), code="sdk.response_invalid", label="LC manifest"
    )
    validation_response = _mapping(
        validate(source), code="sdk.response_invalid", label="validate response"
    )
    validation = _mapping(
        validation_response.get("validation"),
        code="sdk.response_invalid",
        label="validation receipt",
    )
    if validation.get("validation_level") != "full":
        raise ToolkitIOError("sdk.validation_incomplete", "LC inspection requires full validation")
    report = _report_from_lc(manifest, validation, [])
    report["tensor_headers"] = dict(
        _mapping(
            inspection.get("safetensors"),
            code="sdk.response_invalid",
            label="Safetensors headers",
        )
    )
    return report


def _report_from_raw(
    response: Mapping[str, object], runtime_casts: list[dict[str, str]]
) -> dict[str, object]:
    profile = _mapping(
        response.get("profile"), code="sdk.response_invalid", label="raw H3 profile"
    )
    headers = _mapping(
        response.get("safetensors"), code="sdk.response_invalid", label="raw tensor headers"
    )
    return {
        "schema_version": TOOLKIT_IO_VERSION,
        "source_kind": "raw_h3_safetensors",
        "validation": {"validation_level": "full"},
        "source": {
            "sha256": response.get("sha256"),
            "byte_length": response.get("byte_length"),
        },
        "codec": {
            "family": profile.get("codec_family"),
            "profile": profile.get("profile"),
            "profile_version": profile.get("profile_version"),
        },
        "profile": dict(profile),
        "tensor_headers": dict(headers),
        "runtime_casts": runtime_casts,
        "compatibility": {
            "status": "compatible",
            "target": "latentdeck_comfy_toolkit/0.1.0",
            "reasons": [],
        },
    }


def import_raw_h3(path: str | Path, *, sdk: CartridgeSdk | None = None) -> LoadedH3Latent:
    """Load a fully validated raw H3 Safetensors file directly into Comfy LATENT."""

    source = Path(path)
    selected_sdk = sdk or _default_sdk()
    reader = getattr(selected_sdk, "read_raw_h3", None)
    if not callable(reader):
        raise ToolkitIOError("sdk.incompatible", "Cartridge SDK does not expose read_raw_h3()")
    response = _mapping(
        reader(
            source,
            max_visual_values=MAX_TOOLKIT_VISUAL_VALUES,
            max_tensor_bytes=MAX_TOOLKIT_TENSOR_BYTES,
        ),
        code="sdk.response_invalid",
        label="read_raw_h3 response",
    )
    if response.get("status") != "ok":
        raise ToolkitIOError("sdk.response_invalid", "read_raw_h3 did not return status=ok")
    tensors = _mapping(
        response.get("tensors"), code="sdk.response_invalid", label="tensor mapping"
    )
    video = _tensor_from_wire("video", tensors.get("video"))
    video, runtime_casts = _runtime_casts(video)
    audio_descriptor = tensors.get("audio")
    samples: torch.Tensor | H3AVSamples
    if audio_descriptor is None:
        samples = video
    else:
        samples = _make_av_samples(video, _tensor_from_wire("audio", audio_descriptor))
    report = _report_from_raw(response, runtime_casts)
    latent = initialize_raw_metadata(
        {
        "samples": samples,
        LATENTDECK_METADATA_KEY: {
            "schema_version": TOOLKIT_IO_VERSION,
            "source_kind": "raw_h3_safetensors",
            "profile": report["profile"],
            "source": report["source"],
            "tensor_headers": report["tensor_headers"],
        },
        },
        profile=_mapping(report["profile"], code="sdk.response_invalid", label="raw profile"),
        source=_mapping(report["source"], code="sdk.response_invalid", label="raw source"),
    )
    return LoadedH3Latent(latent=latent, report=report)


def inspect_raw_h3(path: str | Path, *, sdk: CartridgeSdk | None = None) -> dict[str, object]:
    """Fully validate and hash raw H3 metadata without materializing tensor bytes."""

    source = Path(path)
    selected_sdk = sdk or _default_sdk()
    inspect = getattr(selected_sdk, "inspect_raw_h3", None)
    if not callable(inspect):
        raise ToolkitIOError(
            "sdk.incompatible", "Cartridge SDK does not expose inspect_raw_h3()"
        )
    response = _mapping(
        inspect(source), code="sdk.response_invalid", label="inspect_raw_h3 response"
    )
    if response.get("status") != "ok":
        raise ToolkitIOError("sdk.response_invalid", "inspect_raw_h3 did not return status=ok")
    return _report_from_raw(response, [])


def parent_cartridge_ref(latent: object, *, role: str) -> dict[str, str]:
    """Build one exact LC genealogy reference from a loaded workflow LATENT."""

    if not isinstance(role, str) or re.fullmatch(
        r"[a-z0-9](?:[a-z0-9._-]{0,126}[a-z0-9])?", role
    ) is None:
        raise ToolkitIOError("genealogy.role_invalid", "parent role must be a lowercase token")
    workflow = _mapping(latent, code="genealogy.latent_invalid", label="LATENT")
    metadata = _mapping(
        workflow.get(LATENTDECK_METADATA_KEY),
        code="genealogy.metadata_missing",
        label="LatentDeck metadata",
    )
    manifest = _mapping(
        metadata.get("manifest"), code="genealogy.not_cartridge", label="LC manifest"
    )
    validation = _mapping(
        metadata.get("validation"),
        code="genealogy.not_cartridge",
        label="LC validation receipt",
    )
    cartridge_id = manifest.get("cartridge_id")
    archive_sha256 = validation.get("archive_sha256")
    if not isinstance(cartridge_id, str) or not isinstance(archive_sha256, str):
        raise ToolkitIOError(
            "genealogy.metadata_invalid", "loaded LC identity or archive hash is missing"
        )
    return {
        "cartridge_id": cartridge_id,
        "archive_sha256": archive_sha256,
        "role": role,
    }


def _resample_streams(latent: object) -> tuple[torch.Tensor, torch.Tensor | None]:
    workflow = _mapping(latent, code="resample.latent_invalid", label="LATENT")
    samples = workflow.get("samples")
    if bool(getattr(samples, "is_nested", False)):
        unbind = getattr(samples, "unbind", None)
        if not callable(unbind):
            raise ToolkitIOError(
                "resample.latent_invalid", "nested H3 samples must expose unbind()"
            )
        streams = tuple(unbind())
        if len(streams) != 2 or not all(isinstance(stream, torch.Tensor) for stream in streams):
            raise ToolkitIOError(
                "resample.latent_invalid", "nested H3 samples must contain video and audio tensors"
            )
        return streams[0], streams[1]
    if not isinstance(samples, torch.Tensor):
        raise ToolkitIOError("resample.latent_invalid", "LATENT samples must be a tensor")
    return samples, None


def _preflight_resample_tensors(
    video: torch.Tensor, audio: torch.Tensor | None
) -> dict[str, torch.Tensor]:
    if video.ndim != 5 or video.shape[0] != 1 or video.shape[1] != 24:
        raise ToolkitIOError(
            "resample.video_shape_invalid", "H3 video must have layout [1,24,T,H,W]"
        )
    if video.dtype != torch.float16:
        raise ToolkitIOError(
            "resample.video_dtype_invalid", "post-operator H3 visual storage must be F16"
        )
    if not video.is_contiguous():
        raise ToolkitIOError(
            "resample.tensor_noncontiguous",
            "H3 video must be contiguous; use an explicit materialization node",
        )
    tensors = {"video": video}
    if audio is not None:
        if audio.ndim != 4 or audio.shape[0] != 1 or audio.shape[1] != 32 or audio.shape[2] != 2:
            raise ToolkitIOError(
                "resample.audio_shape_invalid", "H3 audio must have layout [1,32,2,T_audio]"
            )
        if audio.dtype not in {torch.float16, torch.float32}:
            raise ToolkitIOError(
                "resample.audio_dtype_invalid", "H3 audio storage must be F16 or F32"
            )
        if not audio.is_contiguous():
            raise ToolkitIOError(
                "resample.tensor_noncontiguous",
                "H3 audio must be contiguous; use an explicit materialization node",
            )
        tensors["audio"] = audio
    return {name: tensors[name] for name in sorted(tensors)}


def _json_value(value: object, label: str) -> object:
    try:
        return json.loads(
            json.dumps(
                value,
                ensure_ascii=False,
                allow_nan=False,
                separators=(",", ":"),
            )
        )
    except (TypeError, ValueError) as error:
        raise ToolkitIOError("resample.metadata_invalid", f"{label} must be finite JSON") from error


def _validate_audio_disposition(
    audio: torch.Tensor | None,
    disposition: Mapping[str, object],
    parents: list[object],
    *,
    allow_preserved_raw: bool,
) -> None:
    policy = disposition.get("policy")
    if audio is None:
        if policy not in {"source_absent", "omitted_timing_mismatch"}:
            raise ToolkitIOError(
                "resample.audio_policy_invalid",
                "a visual-only resample requires source_absent or omitted_timing_mismatch",
            )
        return
    if policy == "preserved_source" and allow_preserved_raw:
        return
    if policy != "copied_from_carrier_exact":
        raise ToolkitIOError(
            "resample.audio_policy_invalid",
            "an AV resample requires copied_from_carrier_exact or an exact raw-source preserve",
        )
    source = disposition.get("source_cartridge")
    if not isinstance(source, Mapping):
        raise ToolkitIOError(
            "resample.audio_policy_invalid", "copied audio requires source_cartridge"
        )
    source_key = (source.get("cartridge_id"), source.get("archive_sha256"))
    parent_keys = {
        (parent.get("cartridge_id"), parent.get("archive_sha256"))
        for parent in parents
        if isinstance(parent, Mapping)
    }
    if source_key not in parent_keys:
        raise ToolkitIOError(
            "resample.audio_policy_invalid", "audio source_cartridge must be a declared parent"
        )


def _has_explicit_audio_drop(latent: object) -> bool:
    if not isinstance(latent, Mapping):
        return False
    metadata = latent.get(LATENTDECK_METADATA_KEY)
    if not isinstance(metadata, Mapping):
        return False
    chain = metadata.get("operation_chain")
    if not isinstance(chain, list):
        return False
    return any(
        isinstance(operation, Mapping)
        and operation.get("audio_action") == "dropped_explicitly"
        for operation in chain
    )


def _raw_import_provenance_source(latent: object) -> dict[str, object] | None:
    if not isinstance(latent, Mapping):
        return None
    metadata = latent.get(LATENTDECK_METADATA_KEY)
    if not isinstance(metadata, Mapping) or metadata.get("source_kind") != "raw_h3_safetensors":
        return None
    source = metadata.get("source")
    if not isinstance(source, Mapping):
        raise ToolkitIOError(
            "resample.metadata_invalid", "raw H3 source metadata is missing"
        )
    sha256 = source.get("sha256")
    byte_length = source.get("byte_length")
    if not isinstance(sha256, str) or re.fullmatch(r"[0-9a-f]{64}", sha256) is None:
        raise ToolkitIOError(
            "resample.metadata_invalid", "raw H3 source SHA-256 is invalid"
        )
    if isinstance(byte_length, bool) or not isinstance(byte_length, int) or byte_length <= 0:
        raise ToolkitIOError(
            "resample.metadata_invalid", "raw H3 source byte length is invalid"
        )
    return {
        "kind": "raw_h3_safetensors",
        "sha256": sha256,
        "metadata": {"byte_length": byte_length},
    }


def _write_safetensors(path: Path, tensors: dict[str, torch.Tensor]) -> None:
    try:
        from safetensors.torch import save_file
    except ImportError as error:
        raise ToolkitIOError("dependency.missing", "install safetensors in ComfyUI") from error
    save_file(
        tensors,
        str(path),
        metadata={"latentdeck_profile": "h3_av_latent/0.1.0"},
    )


def save_resampled_lc(
    latent: object,
    output_path: str | Path,
    *,
    parent_cartridges: Sequence[Mapping[str, object]] | None = None,
    operation_history: Sequence[Mapping[str, object]] | None = None,
    audio_disposition: Mapping[str, object] | None = None,
    cartridge_id: str | None = None,
    overwrite: bool = False,
    sdk: CartridgeSdk | None = None,
    tensor_writer: Callable[[Path, dict[str, torch.Tensor]], None] | None = None,
) -> SavedCartridge:
    """Write a validated post-operator H3 LC with explicit genealogy."""

    target = Path(output_path)
    if target.suffix.lower() != ".lc":
        raise ToolkitIOError("resample.output_invalid", "resample output must use the .lc suffix")
    if target.exists() and not overwrite:
        raise ToolkitIOError("resample.target_exists", "refusing to overwrite an existing LC")
    video, audio = _resample_streams(latent)
    tensors = _preflight_resample_tensors(video, audio)
    derived = derive_resample_inputs(latent)
    selected_parents = (
        parent_cartridges
        if parent_cartridges is not None
        else derived.parent_cartridges
    )
    selected_operations = (
        operation_history
        if operation_history is not None
        else derived.operation_history
    )
    selected_audio = (
        audio_disposition
        if audio_disposition is not None
        else derived.audio_disposition
    )
    parents_value = _json_value(list(selected_parents), "parent_cartridges")
    operations_value = _json_value(list(selected_operations), "operation_history")
    audio_value = _json_value(dict(selected_audio), "audio_disposition")
    if not isinstance(parents_value, list) or not isinstance(operations_value, list):
        raise ToolkitIOError("resample.metadata_invalid", "genealogy must be arrays")
    if not operations_value:
        raise ToolkitIOError(
            "resample.operation_history_empty", "resample requires at least one operator record"
        )
    if not isinstance(audio_value, dict):
        raise ToolkitIOError("resample.metadata_invalid", "audio disposition must be an object")
    if (
        audio is None
        and _has_explicit_audio_drop(latent)
        and audio_value.get("policy") != "omitted_timing_mismatch"
    ):
        raise ToolkitIOError(
            "resample.audio_policy_invalid",
            "explicitly dropped upstream audio requires omitted_timing_mismatch",
        )
    raw_source = _raw_import_provenance_source(latent)
    provenance_sources = list(derived.provenance_sources)
    if raw_source is not None and raw_source not in provenance_sources:
        provenance_sources.append(raw_source)
    _validate_audio_disposition(
        audio,
        audio_value,
        parents_value,
        allow_preserved_raw=bool(provenance_sources),
    )

    selected_sdk = sdk or _default_sdk()
    raw_pack = getattr(selected_sdk, "pack_raw_h3", None)
    inspect = getattr(selected_sdk, "inspect", None)
    final_pack = getattr(selected_sdk, "pack", None)
    if not callable(raw_pack) or not callable(inspect) or not callable(final_pack):
        raise ToolkitIOError(
            "sdk.incompatible",
            "Cartridge SDK must expose pack_raw_h3(), inspect(), and pack()",
        )
    selected_writer = tensor_writer or _write_safetensors
    selected_id = cartridge_id or str(uuid.uuid4())
    target.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".latentdeck-toolkit-", dir=target.parent) as temporary:
        temporary_root = Path(temporary)
        payload_path = temporary_root / "post-operator.safetensors"
        base_path = temporary_root / "base.lc"
        selected_writer(payload_path, tensors)
        base_receipt = raw_pack(
            payload_path,
            base_path,
            None,
            cartridge_id=selected_id,
            provenance={
                "created_by": {
                    "name": "latentdeck-comfy-toolkit",
                    "version": TOOLKIT_IO_VERSION,
                },
                "source_kind": "toolkit_post_operator_h3",
                "source_metadata": {
                    "parent_count": len(parents_value),
                    "operation_count": len(operations_value),
                },
            },
            overwrite=False,
        )
        if not isinstance(base_receipt, Mapping) or base_receipt.get("status") != "ok":
            raise ToolkitIOError("sdk.response_invalid", "base H3 pack did not return status=ok")
        inspection = _mapping(
            inspect(base_path), code="sdk.response_invalid", label="base LC inspection"
        )
        manifest_value = _json_value(inspection.get("manifest"), "base manifest")
        if not isinstance(manifest_value, dict):
            raise ToolkitIOError("sdk.response_invalid", "base LC inspection omitted manifest")
        manifest_value["parent_cartridges"] = parents_value
        manifest_value["operation_history"] = operations_value
        manifest_value["audio"] = audio_value
        if provenance_sources:
            provenance = manifest_value.get("provenance")
            if not isinstance(provenance, dict):
                raise ToolkitIOError(
                    "sdk.response_invalid", "base LC manifest omitted provenance"
                )
            provenance["sources"] = _json_value(
                provenance_sources, "provenance_sources"
            )
        receipt = final_pack(
            manifest_value,
            payload_path,
            target,
            None,
            overwrite=overwrite,
        )
        if not isinstance(receipt, dict) or receipt.get("status") != "ok":
            raise ToolkitIOError("sdk.response_invalid", "final LC pack did not return status=ok")
    if not target.is_file():
        raise ToolkitIOError("sdk.response_invalid", "SDK reported success without an LC file")
    return SavedCartridge(output_path=target, receipt=receipt, manifest=manifest_value)


__all__ = [
    "H3AVSamples",
    "LATENTDECK_METADATA_KEY",
    "LoadedH3Latent",
    "MAX_TOOLKIT_TENSOR_BYTES",
    "MAX_TOOLKIT_VISUAL_VALUES",
    "SavedCartridge",
    "TOOLKIT_IO_VERSION",
    "ToolkitIOError",
    "import_raw_h3",
    "inspect_lc",
    "inspect_raw_h3",
    "load_lc",
    "parent_cartridge_ref",
    "save_resampled_lc",
]
