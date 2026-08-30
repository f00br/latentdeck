"""Explicit FAST/HQ decoder hooks with no bundled decoder assets."""

from __future__ import annotations

import json
import math
import re
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any

import torch

MAX_DECODER_INPUT_VALUES = 50_331_648
MAX_DECODED_VALUES = 402_653_184
MAX_METRIC_CHUNK_VALUES = 1_048_576
_TOKEN = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")


class ToolkitContractError(ValueError):
    """Stable, path-free Toolkit contract failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


@dataclass(frozen=True, slots=True)
class DecoderHook:
    """An explicitly supplied decoder callable and opaque caller-owned asset."""

    decoder_id: str
    decoder_version: str
    decode: Callable[[torch.Tensor, object], torch.Tensor]
    asset: object = None
    asset_sha256: str | None = None

    def validate(self) -> None:
        if not isinstance(self.decoder_id, str) or _TOKEN.fullmatch(self.decoder_id) is None:
            raise ToolkitContractError("decoder.id_invalid", "decoder_id must be a bounded token")
        if (
            not isinstance(self.decoder_version, str)
            or _VERSION.fullmatch(self.decoder_version) is None
        ):
            raise ToolkitContractError(
                "decoder.version_invalid", "decoder_version must use MAJOR.MINOR.PATCH"
            )
        if not callable(self.decode):
            raise ToolkitContractError("decoder.callable_invalid", "decode must be callable")
        if self.asset is not None and self.asset_sha256 is None:
            raise ToolkitContractError(
                "decoder.asset_hash_required",
                "an explicitly supplied decoder asset requires asset_sha256",
            )
        if self.asset_sha256 is not None and (
            not isinstance(self.asset_sha256, str) or _SHA256.fullmatch(self.asset_sha256) is None
        ):
            raise ToolkitContractError(
                "decoder.asset_hash_invalid", "asset_sha256 must be lowercase SHA-256"
            )

    def identity(self) -> dict[str, str | None]:
        return {
            "decoder_id": self.decoder_id,
            "decoder_version": self.decoder_version,
            "asset_sha256": self.asset_sha256,
        }


@dataclass(frozen=True, slots=True)
class DecoderComparison:
    fast_output: torch.Tensor
    hq_output: torch.Tensor
    metrics: dict[str, float | None]
    provenance: dict[str, Any]


def _tensor(
    value: object,
    label: str,
    *,
    max_values: int,
    match: torch.Tensor | None = None,
    require_contiguous: bool = False,
) -> torch.Tensor:
    if not isinstance(value, torch.Tensor):
        raise ToolkitContractError("decoder.tensor_type", f"{label} must be a torch.Tensor")
    if value.numel() < 1 or value.numel() > max_values:
        raise ToolkitContractError(
            "decoder.tensor_bound",
            f"{label} must contain between 1 and {max_values} values",
        )
    if value.layout is not torch.strided or value.device.type not in {"cpu", "cuda"}:
        raise ToolkitContractError(
            "decoder.tensor_layout", f"{label} must use dense CPU or CUDA storage"
        )
    if value.dtype not in {torch.float16, torch.bfloat16, torch.float32}:
        raise ToolkitContractError("decoder.tensor_dtype", f"{label} must use F16, BF16, or F32")
    if require_contiguous and not value.is_contiguous():
        raise ToolkitContractError(
            "decoder.tensor_layout", f"{label} must be contiguous for bounded metrics"
        )
    if not bool(torch.isfinite(value).all().item()):
        raise ToolkitContractError("decoder.tensor_non_finite", f"{label} contains NaN or Inf")
    if match is not None and (value.shape != match.shape or value.device != match.device):
        raise ToolkitContractError(
            "decoder.output_incompatible",
            "FAST and HQ decoder outputs must have identical shape and device",
        )
    return value


def _decode(latent: torch.Tensor, hook: DecoderHook, label: str) -> torch.Tensor:
    try:
        output = hook.decode(latent, hook.asset)
    except ToolkitContractError:
        raise
    except Exception as error:
        raise ToolkitContractError(
            "decoder.execution_failed", f"{label} decoder execution failed"
        ) from error
    return _tensor(
        output,
        f"{label} output",
        max_values=MAX_DECODED_VALUES,
        require_contiguous=True,
    )


def _metrics(
    fast_output: torch.Tensor,
    hq_output: torch.Tensor,
) -> dict[str, float | None]:
    fast_values = fast_output.reshape(-1)
    hq_values = hq_output.reshape(-1)
    count = fast_values.numel()
    absolute_sum = 0.0
    squared_sum = 0.0
    max_absolute_error = 0.0
    hq_minimum = math.inf
    hq_maximum = -math.inf

    for start in range(0, count, MAX_METRIC_CHUNK_VALUES):
        stop = min(count, start + MAX_METRIC_CHUNK_VALUES)
        difference = fast_values[start:stop].double()
        difference.sub_(hq_values[start:stop])
        absolute = difference.abs()
        absolute_sum += float(absolute.sum().item())
        squared_sum += float(torch.dot(difference, difference).item())
        max_absolute_error = max(max_absolute_error, float(absolute.max().item()))
        hq_chunk = hq_values[start:stop]
        hq_minimum = min(hq_minimum, float(hq_chunk.min().item()))
        hq_maximum = max(hq_maximum, float(hq_chunk.max().item()))

    mean_absolute_error = absolute_sum / count
    root_mean_squared_error = math.sqrt(squared_sum / count)
    hq_range = hq_maximum - hq_minimum
    values = (mean_absolute_error, max_absolute_error, root_mean_squared_error, hq_range)
    if not all(math.isfinite(value) for value in values):
        raise ToolkitContractError(
            "decoder.metrics_non_finite", "decoder comparison metrics are non-finite"
        )
    psnr_db: float | None = None
    if root_mean_squared_error > 0.0 and hq_range > 0.0:
        candidate = 20.0 * math.log10(hq_range / root_mean_squared_error)
        if math.isfinite(candidate):
            psnr_db = candidate
    return {
        "mean_absolute_error": mean_absolute_error,
        "max_absolute_error": max_absolute_error,
        "root_mean_squared_error": root_mean_squared_error,
        "psnr_db": psnr_db,
    }


@torch.inference_mode()
def compare_decoder_hooks(
    latent: torch.Tensor,
    fast: DecoderHook,
    hq: DecoderHook,
) -> DecoderComparison:
    """Run two caller-provided decoders and return bounded comparison metrics."""

    source = _tensor(latent, "latent", max_values=MAX_DECODER_INPUT_VALUES)
    if not isinstance(fast, DecoderHook) or not isinstance(hq, DecoderHook):
        raise ToolkitContractError("decoder.hook_invalid", "FAST and HQ hooks are required")
    fast.validate()
    hq.validate()
    fast_input = source.detach().clone(memory_format=torch.contiguous_format)
    hq_input = source.detach().clone(memory_format=torch.contiguous_format)
    fast_output = _decode(fast_input, fast, "FAST")
    hq_output = _tensor(
        _decode(hq_input, hq, "HQ"),
        "HQ output",
        max_values=MAX_DECODED_VALUES,
        match=fast_output,
        require_contiguous=True,
    )

    metrics = _metrics(fast_output, hq_output)
    provenance: dict[str, Any] = {
        "schema_version": "0.1.0",
        "kind": "latentdeck.toolkit.decoder_comparison",
        "execution_surface": "comfy_research",
        "fast": fast.identity(),
        "hq": hq.identity(),
        "output": {
            "shape": list(fast_output.shape),
            "fast_dtype": str(fast_output.dtype).removeprefix("torch."),
            "hq_dtype": str(hq_output.dtype).removeprefix("torch."),
            "max_values": MAX_DECODED_VALUES,
        },
        "input_isolation": "independent_clones",
        "metric_chunk_values": MAX_METRIC_CHUNK_VALUES,
        "metrics": metrics,
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return DecoderComparison(fast_output, hq_output, metrics, provenance)


__all__ = [
    "MAX_DECODED_VALUES",
    "MAX_DECODER_INPUT_VALUES",
    "MAX_METRIC_CHUNK_VALUES",
    "DecoderComparison",
    "DecoderHook",
    "ToolkitContractError",
    "compare_decoder_hooks",
]
