"""Diagnostics and repeatability evaluation for Toolkit research operators."""

from __future__ import annotations

import json
from collections.abc import Callable
from dataclasses import dataclass
from time import perf_counter_ns
from typing import Any

import torch

from .decoder_compare import ToolkitContractError
from .research_ops import ResearchResult, _surface

MAX_BENCHMARK_RUNS = 100
MAX_DETERMINISM_RUNS = 16


@dataclass(frozen=True, slots=True)
class BenchmarkResult:
    output: object
    report: dict[str, Any]


@dataclass(frozen=True, slots=True)
class DeterminismResult:
    output: object
    report: dict[str, Any]


@dataclass(frozen=True, slots=True)
class StreamingCompatibilityResult:
    full_output: torch.Tensor
    streamed_output: torch.Tensor
    report: dict[str, Any]


def _dtype_name(dtype: torch.dtype) -> str:
    return {torch.float16: "F16", torch.float32: "F32"}[dtype]


def _energy(value: torch.Tensor) -> float | None:
    if not bool(torch.isfinite(value).all().item()):
        return None
    return float(torch.sqrt(value.float().square().mean()).item())


@torch.inference_mode()
def latent_scopes(latent: object) -> dict[str, Any]:
    """Return bounded JSON-safe statistics without rejecting NaN/Inf first."""

    visual = _surface(latent, "latent", allow_non_finite=True).visual
    finite_mask = torch.isfinite(visual)
    nan_count = int(torch.isnan(visual).sum().item())
    positive_inf_count = int(torch.isposinf(visual).sum().item())
    negative_inf_count = int(torch.isneginf(visual).sum().item())
    finite_values = visual[finite_mask]
    if finite_values.numel():
        finite_f32 = finite_values.float()
        mean: float | None = float(finite_f32.mean().item())
        std: float | None = float(finite_f32.std(correction=0).item())
        minimum: float | None = float(finite_f32.min().item())
        maximum: float | None = float(finite_f32.max().item())
    else:
        mean = std = minimum = maximum = None

    channel_energy = [_energy(visual[:, channel]) for channel in range(24)]
    temporal_energy = [_energy(visual[:, :, slot]) for slot in range(visual.shape[2])]
    temporal_delta_energy = [
        _energy(visual[:, :, slot] - visual[:, :, slot - 1])
        for slot in range(1, visual.shape[2])
    ]
    report: dict[str, Any] = {
        "schema_version": "0.1.0",
        "shape": list(visual.shape),
        "dtype": _dtype_name(visual.dtype),
        "device": visual.device.type,
        "finite": bool(finite_mask.all().item()),
        "nan_count": nan_count,
        "positive_inf_count": positive_inf_count,
        "negative_inf_count": negative_inf_count,
        "mean": mean,
        "std": std,
        "min": minimum,
        "max": maximum,
        "channel_energy": channel_energy,
        "temporal_energy": temporal_energy,
        "temporal_delta_energy": temporal_delta_energy,
        "full_grid": True,
        "visual_only": True,
    }
    json.dumps(report, allow_nan=False, separators=(",", ":"))
    return report


def _operator_output(operator: Callable[[object], object], latent: object) -> object:
    result = operator(latent)
    return result.output if isinstance(result, ResearchResult) else result


def _fresh_input(latent: object) -> object:
    surface = _surface(latent, "benchmark.input")
    return surface.repack(surface.visual.clone())


def _percentile(values: list[float], quantile: float) -> float:
    ordered = sorted(values)
    index = max(0, min(len(ordered) - 1, int((len(ordered) * quantile) + 0.999999) - 1))
    return ordered[index]


@torch.inference_mode()
def benchmark_operator(
    operator: Callable[[object], object],
    latent: object,
    *,
    warmup_runs: int = 1,
    measured_runs: int = 5,
    streaming_compatible: bool | None = None,
) -> BenchmarkResult:
    """Measure an explicitly supplied operator without guessing its capabilities."""

    if not callable(operator):
        raise ToolkitContractError("benchmark.operator", "operator must be callable")
    if not isinstance(warmup_runs, int) or not 0 <= warmup_runs <= 10:
        raise ToolkitContractError("benchmark.warmup", "warmup_runs must be in [0,10]")
    if not isinstance(measured_runs, int) or not 1 <= measured_runs <= MAX_BENCHMARK_RUNS:
        raise ToolkitContractError(
            "benchmark.runs", f"measured_runs must be in [1,{MAX_BENCHMARK_RUNS}]"
        )
    if streaming_compatible not in {None, True, False}:
        raise ToolkitContractError(
            "benchmark.streaming", "streaming_compatible must be true, false, or unknown"
        )
    input_surface = _surface(latent, "benchmark.input")
    device = input_surface.visual.device

    def synchronize() -> None:
        if device.type == "cuda":
            torch.cuda.synchronize(device)

    for _ in range(warmup_runs):
        warmup_output = _operator_output(operator, _fresh_input(latent))
        _surface(warmup_output, "benchmark.warmup_output")
    synchronize()

    before_allocated: int | None = None
    if device.type == "cuda":
        before_allocated = int(torch.cuda.memory_allocated(device))
        torch.cuda.reset_peak_memory_stats(device)

    durations: list[float] = []
    output: object | None = None
    for _ in range(measured_runs):
        run_input = _fresh_input(latent)
        synchronize()
        started = perf_counter_ns()
        output = _operator_output(operator, run_input)
        synchronize()
        durations.append((perf_counter_ns() - started) / 1_000_000.0)
        _surface(output, "benchmark.output")
    assert output is not None
    output_surface = _surface(output, "benchmark.output")

    if device.type == "cuda" and before_allocated is not None:
        after_allocated = int(torch.cuda.memory_allocated(device))
        vram_delta: int | None = after_allocated - before_allocated
        vram_peak_delta: int | None = max(
            0, int(torch.cuda.max_memory_allocated(device)) - before_allocated
        )
    else:
        vram_delta = None
        vram_peak_delta = None

    report: dict[str, Any] = {
        "schema_version": "0.1.0",
        "shape": list(output_surface.visual.shape),
        "dtype": _dtype_name(output_surface.visual.dtype),
        "device": device.type,
        "execution_ms": {
            "runs": measured_runs,
            "warmup_runs": warmup_runs,
            "mean": sum(durations) / len(durations),
            "minimum": min(durations),
            "maximum": max(durations),
            "p50": _percentile(durations, 0.50),
            "p95": _percentile(durations, 0.95),
        },
        "vram_delta_bytes": vram_delta,
        "vram_peak_delta_bytes": vram_peak_delta,
        "streaming_compatible": streaming_compatible,
        "full_grid": True,
    }
    json.dumps(report, allow_nan=False, separators=(",", ":"))
    return BenchmarkResult(output=output, report=report)


@torch.inference_mode()
def evaluate_determinism(
    operator: Callable[[object], object], latent: object, *, runs: int = 3
) -> DeterminismResult:
    """Repeat an operator over isolated clones and require exact tensor equality."""

    if not callable(operator):
        raise ToolkitContractError("determinism.operator", "operator must be callable")
    if not isinstance(runs, int) or not 2 <= runs <= MAX_DETERMINISM_RUNS:
        raise ToolkitContractError(
            "determinism.runs", f"runs must be in [2,{MAX_DETERMINISM_RUNS}]"
        )
    baseline_output = _operator_output(operator, _fresh_input(latent))
    baseline = _surface(baseline_output, "determinism.baseline").visual
    mismatch_runs: list[int] = []
    incompatible_runs: list[int] = []
    max_abs_difference = 0.0
    for run in range(1, runs):
        candidate_output = _operator_output(operator, _fresh_input(latent))
        candidate = _surface(candidate_output, f"determinism.run[{run}]").visual
        if (
            candidate.shape != baseline.shape
            or candidate.dtype != baseline.dtype
            or candidate.device != baseline.device
        ):
            mismatch_runs.append(run)
            incompatible_runs.append(run)
            continue
        if not torch.equal(candidate, baseline):
            mismatch_runs.append(run)
            difference = float((candidate.float() - baseline.float()).abs().max().item())
            max_abs_difference = max(max_abs_difference, difference)
    report: dict[str, Any] = {
        "schema_version": "0.1.0",
        "runs": runs,
        "comparison": "EXACT_TENSOR_EQUALITY",
        "deterministic": not mismatch_runs,
        "mismatch_runs": mismatch_runs,
        "incompatible_runs": incompatible_runs,
        "max_abs_difference": max_abs_difference,
        "shape": list(baseline.shape),
        "dtype": _dtype_name(baseline.dtype),
        "device": baseline.device.type,
    }
    json.dumps(report, allow_nan=False, separators=(",", ":"))
    return DeterminismResult(output=baseline_output, report=report)


@torch.inference_mode()
def evaluate_streaming_compatibility(
    full_operator: Callable[[torch.Tensor], object],
    chunk_operator: Callable[[torch.Tensor, object | None, int], object],
    latent: object,
    *,
    chunk_slots: int,
    atol: float = 0.0,
    rtol: float = 0.0,
) -> StreamingCompatibilityResult:
    """Compare full visual processing with explicit stateful temporal chunks."""

    if not callable(full_operator) or not callable(chunk_operator):
        raise ToolkitContractError(
            "streaming.operator", "full and chunk operators must be callable"
        )
    source = _surface(latent, "streaming.input").visual
    if not isinstance(chunk_slots, int) or not 1 <= chunk_slots <= source.shape[2]:
        raise ToolkitContractError(
            "streaming.chunk_slots", "chunk_slots must be inside the temporal sequence"
        )
    if atol < 0.0 or rtol < 0.0:
        raise ToolkitContractError("streaming.tolerance", "atol and rtol must be non-negative")

    full_value = _operator_output(full_operator, source.clone())
    full_output = _surface(full_value, "streaming.full_output").visual
    if (
        full_output.shape != source.shape
        or full_output.dtype != source.dtype
        or full_output.device != source.device
    ):
        raise ToolkitContractError(
            "streaming.full_contract", "full operator must preserve shape, dtype, and device"
        )

    state: object | None = None
    chunks: list[torch.Tensor] = []
    for offset in range(0, source.shape[2], chunk_slots):
        chunk = source[:, :, offset : offset + chunk_slots].clone()
        result = chunk_operator(chunk, state, offset)
        if isinstance(result, tuple):
            if len(result) != 2:
                raise ToolkitContractError(
                    "streaming.chunk_result", "chunk tuple must contain output and state"
                )
            chunk_value, state = result
        else:
            chunk_value = result
        chunk_value = (
            chunk_value.output if isinstance(chunk_value, ResearchResult) else chunk_value
        )
        chunk_output = _surface(chunk_value, f"streaming.chunk[{offset}]").visual
        if (
            chunk_output.shape != chunk.shape
            or chunk_output.dtype != source.dtype
            or chunk_output.device != source.device
        ):
            raise ToolkitContractError(
                "streaming.chunk_contract",
                "chunk operator must preserve chunk shape, dtype, and device",
            )
        chunks.append(chunk_output)
    streamed_output = torch.cat(chunks, dim=2).contiguous()
    difference = (streamed_output.float() - full_output.float()).abs()
    tolerance = atol + rtol * full_output.float().abs()
    mismatched_values = int((difference > tolerance).sum().item())
    max_abs_difference = float(difference.max().item())
    report: dict[str, Any] = {
        "schema_version": "0.1.0",
        "compatible": mismatched_values == 0,
        "comparison": "FULL_CLIP_VS_TEMPORAL_CHUNKS",
        "chunk_slots": chunk_slots,
        "chunks": len(chunks),
        "atol": float(atol),
        "rtol": float(rtol),
        "mismatched_values": mismatched_values,
        "max_abs_difference": max_abs_difference,
        "shape": list(source.shape),
        "dtype": _dtype_name(source.dtype),
        "device": source.device.type,
        "visual_only": True,
        "stateful_chunk_callable": True,
    }
    json.dumps(report, allow_nan=False, separators=(",", ":"))
    return StreamingCompatibilityResult(full_output, streamed_output, report)


__all__ = [
    "MAX_BENCHMARK_RUNS",
    "MAX_DETERMINISM_RUNS",
    "BenchmarkResult",
    "DeterminismResult",
    "StreamingCompatibilityResult",
    "benchmark_operator",
    "evaluate_determinism",
    "evaluate_streaming_compatibility",
    "latent_scopes",
]
