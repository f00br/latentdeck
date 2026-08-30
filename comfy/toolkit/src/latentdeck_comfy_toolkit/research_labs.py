"""Bounded low-level laboratories that compose the Toolkit operators."""

from __future__ import annotations

import json
import math
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import torch

from .decoder_compare import ToolkitContractError
from .research_ops import ResearchResult, _provenance, _research_result, _surface

MAX_TEMPORAL_LOOPS = 16
MAX_FEEDBACK_ITERATIONS = 16
MAX_OPERATOR_CHAIN_STEPS = 16


@dataclass(frozen=True, slots=True)
class OperatorStep:
    """One explicitly named callable in a bounded research chain."""

    name: str
    apply: Callable[[object], ResearchResult]


@torch.inference_mode()
def temporal_lab(
    latent: object,
    *,
    crop_start: int = 0,
    crop_length: int | None = None,
    reverse: bool = False,
    offset: int = 0,
    loop_count: int = 1,
    audio_policy: str = "REJECT",
) -> ResearchResult:
    """Apply explicit crop, reverse, circular offset, and loop transforms."""

    surface = _surface(latent, "latent")
    slots = surface.visual.shape[2]
    length = slots - crop_start if crop_length is None else crop_length
    if not isinstance(crop_start, int) or not isinstance(length, int):
        raise ToolkitContractError("temporal.type", "crop_start and crop_length must be integers")
    if crop_start < 0 or length < 1 or crop_start + length > slots:
        raise ToolkitContractError("temporal.crop", "crop range must be inside the visual sequence")
    if not isinstance(offset, int):
        raise ToolkitContractError("temporal.type", "offset must be an integer")
    if not isinstance(loop_count, int) or not 1 <= loop_count <= MAX_TEMPORAL_LOOPS:
        raise ToolkitContractError(
            "temporal.loop", f"loop_count must be in [1,{MAX_TEMPORAL_LOOPS}]"
        )
    if length * loop_count > 512:
        raise ToolkitContractError("temporal.bound", "temporal output exceeds 512 slots")
    if audio_policy not in {"REJECT", "DROP"}:
        raise ToolkitContractError("audio.policy", "audio_policy must be REJECT or DROP")

    changes_mapping = (
        crop_start != 0
        or length != slots
        or bool(reverse)
        or offset % length != 0
        or loop_count != 1
    )
    if surface.audio and changes_mapping and audio_policy == "REJECT":
        raise ToolkitContractError(
            "audio.mapping",
            "visual temporal transforms require explicit audio_policy=DROP in Toolkit 0.1",
        )

    output = surface.visual[:, :, crop_start : crop_start + length]
    if reverse:
        output = torch.flip(output, dims=(2,))
    if offset:
        output = torch.roll(output, shifts=offset, dims=2)
    if loop_count > 1:
        output = output.repeat(1, 1, loop_count, 1, 1)
    output = output.contiguous()
    resolved_audio_policy = (
        "NONE" if not surface.audio else ("DROP" if changes_mapping else "PRESERVE")
    )
    provenance = _provenance(
            "TEMPORAL_LAB",
            output,
            crop_start=crop_start,
            crop_length=length,
            reverse=bool(reverse),
            offset=offset,
            loop_count=loop_count,
            order=["CROP", "REVERSE", "OFFSET", "LOOP"],
            audio_policy=resolved_audio_policy,
        )
    return _research_result(
        surface.repack(output, keep_audio=not (surface.audio and changes_mapping)),
        provenance,
        sources=(("source", latent),),
        structural_role="source",
    )


@torch.inference_mode()
def feedback_lab(
    latent: object,
    *,
    amount: float,
    delay: int = 1,
    iterations: int = 1,
) -> ResearchResult:
    """Apply a bounded causal visual-latent feedback experiment."""

    surface = _surface(latent, "latent")
    if not 0.0 <= amount <= 1.0:
        raise ToolkitContractError("feedback.amount", "amount must be in [0,1]")
    if not isinstance(delay, int) or not 1 <= delay < surface.visual.shape[2]:
        raise ToolkitContractError("feedback.delay", "delay must be inside the temporal sequence")
    if not isinstance(iterations, int) or not 1 <= iterations <= MAX_FEEDBACK_ITERATIONS:
        raise ToolkitContractError(
            "feedback.iterations",
            f"iterations must be in [1,{MAX_FEEDBACK_ITERATIONS}]",
        )
    source = surface.visual
    if amount == 0.0:
        output = source.clone()
    else:
        source_f32 = source.float()
        current = source_f32.clone()
        for _ in range(iterations):
            delayed = torch.empty_like(current)
            delayed[:, :, :delay] = source_f32[:, :, :delay]
            delayed[:, :, delay:] = current[:, :, :-delay]
            current = torch.lerp(source_f32, delayed, amount)
        output = current.to(dtype=source.dtype).contiguous()
    if not bool(torch.isfinite(output).all().item()):
        raise ToolkitContractError("feedback.non_finite", "feedback produced NaN or Inf")
    provenance = _provenance(
            "FEEDBACK_LAB",
            output,
            amount=float(amount),
            delay=delay,
            iterations=iterations,
            causal=True,
            wraparound=False,
            max_iterations=MAX_FEEDBACK_ITERATIONS,
            compute_dtype="F32",
            output_dtype_preserved=True,
        )
    return _research_result(
        surface.repack(output),
        provenance,
        sources=(("source", latent),),
        structural_role="source",
    )


def channel_rotation_matrix(
    channel_a: int,
    channel_b: int,
    *,
    angle_degrees: float,
    dtype: torch.dtype = torch.float32,
    device: torch.device | str = "cpu",
) -> torch.Tensor:
    """Construct an explicit two-channel rotation inside a 24-channel matrix."""

    if not isinstance(channel_a, int) or not isinstance(channel_b, int):
        raise ToolkitContractError("channel.type", "channel indices must be integers")
    if not 0 <= channel_a < 24 or not 0 <= channel_b < 24 or channel_a == channel_b:
        raise ToolkitContractError("channel.range", "channels must be distinct indices in [0,23]")
    if dtype not in {torch.float16, torch.float32}:
        raise ToolkitContractError("channel.dtype", "matrix dtype must be F16 or F32")
    angle = math.radians(float(angle_degrees))
    cosine = math.cos(angle)
    sine = math.sin(angle)
    matrix = torch.eye(24, dtype=dtype, device=device)
    matrix[channel_a, channel_a] = cosine
    matrix[channel_a, channel_b] = -sine
    matrix[channel_b, channel_a] = sine
    matrix[channel_b, channel_b] = cosine
    return matrix


@torch.inference_mode()
def channel_lab(
    latent: object,
    *,
    matrix: torch.Tensor,
    strength: float = 1.0,
) -> ResearchResult:
    """Apply an explicit dense 24x24 matrix over every full-grid visual token."""

    surface = _surface(latent, "latent")
    if not isinstance(matrix, torch.Tensor) or tuple(matrix.shape) != (24, 24):
        raise ToolkitContractError("channel.matrix_shape", "matrix must have shape [24,24]")
    if matrix.layout is not torch.strided or matrix.dtype not in {torch.float16, torch.float32}:
        raise ToolkitContractError("channel.matrix_dtype", "matrix must be dense F16 or F32")
    if matrix.device != surface.visual.device:
        raise ToolkitContractError(
            "channel.matrix_device", "matrix and latent must use the same explicit device"
        )
    if not bool(torch.isfinite(matrix).all().item()):
        raise ToolkitContractError("channel.matrix_non_finite", "matrix contains NaN or Inf")
    if not 0.0 <= strength <= 1.0:
        raise ToolkitContractError("channel.strength", "strength must be in [0,1]")
    source = surface.visual
    if strength == 0.0:
        output = source.clone()
    else:
        transformed = torch.einsum("oc,bcthw->bothw", matrix.float(), source.float())
        output = (
            torch.lerp(source.float(), transformed, strength)
            .to(dtype=source.dtype)
            .contiguous()
        )
    provenance = _provenance(
            "CHANNEL_LAB",
            output,
            matrix_shape=[24, 24],
            strength=float(strength),
            compute_dtype="F32",
            output_dtype_preserved=True,
        )
    return _research_result(
        surface.repack(output),
        provenance,
        sources=(("source", latent),),
        structural_role="source",
    )


@torch.inference_mode()
def run_operator_chain(latent: object, steps: Sequence[OperatorStep]) -> ResearchResult:
    """Run explicitly supplied research steps and aggregate their provenance."""

    if isinstance(steps, (str, bytes)) or not 1 <= len(steps) <= MAX_OPERATOR_CHAIN_STEPS:
        raise ToolkitContractError(
            "chain.length", f"operator chain must contain 1 to {MAX_OPERATOR_CHAIN_STEPS} steps"
        )
    current = latent
    chain: list[dict[str, Any]] = []
    for index, step in enumerate(steps):
        if not isinstance(step, OperatorStep) or not callable(step.apply):
            raise ToolkitContractError("chain.step", f"step {index} is not an OperatorStep")
        if not step.name or len(step.name) > 96 or any(ord(char) < 32 for char in step.name):
            raise ToolkitContractError("chain.name", f"step {index} has an invalid name")
        result = step.apply(current)
        if not isinstance(result, ResearchResult):
            raise ToolkitContractError(
                "chain.result", f"step {index} must return ResearchResult"
            )
        _surface(result.output, f"step[{index}].output")
        current = result.output
        chain.append({"name": step.name, "provenance": result.provenance})

    final_surface = _surface(current, "chain.output")
    provenance = _provenance(
        "OPERATOR_CHAIN",
        final_surface.visual,
        step_count=len(chain),
        explicit_order=True,
    )
    provenance["chain"] = chain
    try:
        json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    except (TypeError, ValueError) as error:
        raise ToolkitContractError(
            "chain.provenance", "chain provenance must be JSON-safe"
        ) from error
    if isinstance(current, Mapping):
        return _research_result(
            current,
            provenance,
            sources=(("source", current),),
            structural_role="source",
        )
    return ResearchResult(output=current, provenance=provenance)


__all__ = [
    "MAX_FEEDBACK_ITERATIONS",
    "MAX_OPERATOR_CHAIN_STEPS",
    "MAX_TEMPORAL_LOOPS",
    "OperatorStep",
    "channel_lab",
    "channel_rotation_matrix",
    "feedback_lab",
    "run_operator_chain",
    "temporal_lab",
]
