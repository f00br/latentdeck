"""Explicit, bounded CPU/CUDA staging for Toolkit latent streams."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import torch

from .decoder_compare import ToolkitContractError
from .research_ops import visual_latent

DEVICE_TRANSFER_VERSION = "0.1.0"
MAX_CUDA_DEVICE_INDEX = 63
MAX_DEVICE_TRANSFER_BYTES = 512 * 1024 * 1024
_TARGETS = {"CPU", "CUDA"}
_CUDA_UNAVAILABLE_POLICIES = {"ERROR", "FALLBACK_TO_CPU"}


@dataclass(frozen=True, slots=True)
class DeviceTransferResult:
    """One staged latent plus a bounded, JSON-safe transfer receipt."""

    output: object
    provenance: dict[str, Any]


def _validate_controls(
    target: object,
    cuda_index: object,
    cuda_unavailable_policy: object,
) -> tuple[str, int, str]:
    if target not in _TARGETS:
        raise ToolkitContractError(
            "device.target_invalid", "target must be exactly CPU or CUDA"
        )
    if (
        isinstance(cuda_index, bool)
        or not isinstance(cuda_index, int)
        or not 0 <= cuda_index <= MAX_CUDA_DEVICE_INDEX
    ):
        raise ToolkitContractError(
            "device.cuda_index_invalid",
            f"cuda_index must be an integer in [0,{MAX_CUDA_DEVICE_INDEX}]",
        )
    if cuda_unavailable_policy not in _CUDA_UNAVAILABLE_POLICIES:
        raise ToolkitContractError(
            "device.cuda_policy_invalid",
            "cuda_unavailable_policy must be ERROR or FALLBACK_TO_CPU",
        )
    return str(target), cuda_index, str(cuda_unavailable_policy)


def _resolve_target(
    *, target: str, cuda_index: int, cuda_unavailable_policy: str
) -> tuple[torch.device, int | None, bool]:
    if target == "CPU":
        return torch.device("cpu"), None, False

    try:
        cuda_available = bool(torch.cuda.is_available())
        cuda_device_count = int(torch.cuda.device_count()) if cuda_available else 0
    except Exception as error:
        raise ToolkitContractError(
            "device.cuda_query_failed", "CUDA availability could not be queried safely"
        ) from error

    if not cuda_available or cuda_device_count == 0:
        if cuda_unavailable_policy == "FALLBACK_TO_CPU":
            return torch.device("cpu"), cuda_device_count, True
        raise ToolkitContractError(
            "device.cuda_unavailable",
            "CUDA was requested but no CUDA device is available; "
            "choose FALLBACK_TO_CPU explicitly to continue on CPU",
        )
    if cuda_index >= cuda_device_count:
        raise ToolkitContractError(
            "device.cuda_index_unavailable",
            f"cuda_index {cuda_index} is outside the available device count {cuda_device_count}",
        )
    return torch.device("cuda", cuda_index), cuda_device_count, False


def _device_label(device: torch.device) -> str:
    if device.type == "cpu":
        return "cpu"
    if device.index is None:
        raise ToolkitContractError(
            "device.cuda_index_missing", "CUDA tensors must identify an explicit device index"
        )
    return f"cuda:{device.index}"


def _stream_byte_length(streams: tuple[torch.Tensor, ...]) -> int:
    byte_length = sum(int(stream.numel()) * int(stream.element_size()) for stream in streams)
    if byte_length > MAX_DEVICE_TRANSFER_BYTES:
        raise ToolkitContractError(
            "device.transfer_bound",
            f"latent streams exceed the {MAX_DEVICE_TRANSFER_BYTES}-byte transfer bound",
        )
    return byte_length


def _move_stream(value: torch.Tensor, target: torch.device) -> torch.Tensor:
    try:
        return value.to(device=target, non_blocking=False).contiguous()
    except Exception as error:
        raise ToolkitContractError(
            "device.transfer_failed",
            "the explicit device transfer failed; no automatic retry or fallback was applied",
        ) from error


@torch.inference_mode()
def transfer_latent_device(
    latent: object,
    *,
    target: str,
    cuda_index: int = 0,
    cuda_unavailable_policy: str = "ERROR",
) -> DeviceTransferResult:
    """Move every validated H3 visual/audio stream to one explicit device.

    CPU fallback is permitted only when the caller selects ``FALLBACK_TO_CPU``
    and CUDA is absent. A bad CUDA index, query failure, allocation failure, or
    copy failure remains an error instead of silently changing execution mode.
    """

    parsed_target, parsed_index, parsed_policy = _validate_controls(
        target, cuda_index, cuda_unavailable_policy
    )
    surface = visual_latent(latent, "device_transfer.input")
    source_streams = (surface.visual, *(stream for stream in surface.audio))
    typed_streams = tuple(
        stream
        for stream in source_streams
        if isinstance(stream, torch.Tensor)
    )
    if len(typed_streams) != len(source_streams):
        raise ToolkitContractError(
            "device.stream_invalid", "every validated latent stream must be a tensor"
        )
    byte_length = _stream_byte_length(typed_streams)
    requested_device = "cpu" if parsed_target == "CPU" else f"cuda:{parsed_index}"
    resolved_device, cuda_device_count, fallback_used = _resolve_target(
        target=parsed_target,
        cuda_index=parsed_index,
        cuda_unavailable_policy=parsed_policy,
    )
    moved = tuple(_move_stream(stream, resolved_device) for stream in typed_streams)
    output = surface.repack_streams(moved[0], moved[1:])
    output_surface = visual_latent(output, "device_transfer.output")
    output_streams = (output_surface.visual, *(stream for stream in output_surface.audio))
    if any(
        not isinstance(stream, torch.Tensor)
        or stream.device != resolved_device
        or stream.shape != source.shape
        or stream.dtype != source.dtype
        for source, stream in zip(typed_streams, output_streams, strict=True)
    ):
        raise ToolkitContractError(
            "device.output_invalid",
            "device transfer must preserve every stream shape and dtype on the resolved device",
        )

    source_device = _device_label(surface.visual.device)
    resolved_label = _device_label(resolved_device)
    provenance: dict[str, Any] = {
        "schema_version": DEVICE_TRANSFER_VERSION,
        "operation": {
            "operator_id": "org.latentdeck.toolkit.explicit-device-transfer",
            "operator_version": DEVICE_TRANSFER_VERSION,
            "seed": 0,
            "controls": {
                "target": parsed_target,
                "cuda_index": parsed_index,
                "cuda_unavailable_policy": parsed_policy,
            },
        },
        "source_device": source_device,
        "requested_device": requested_device,
        "resolved_device": resolved_label,
        "cuda_device_count": cuda_device_count,
        "fallback_used": fallback_used,
        "fallback_reason": "cuda_unavailable" if fallback_used else None,
        "transfer_performed": source_device != resolved_label,
        "stream_count": len(typed_streams),
        "byte_length": byte_length,
        "contiguous_output": True,
        "dtype_preserved": True,
        "shape_preserved": True,
        "hidden_transfer": False,
        "hidden_dtype_conversion": False,
        "hidden_resize": False,
        "hidden_reencode": False,
    }
    return DeviceTransferResult(output=output, provenance=provenance)


__all__ = [
    "DEVICE_TRANSFER_VERSION",
    "MAX_CUDA_DEVICE_INDEX",
    "MAX_DEVICE_TRANSFER_BYTES",
    "DeviceTransferResult",
    "transfer_latent_device",
]
