"""Deterministic, full-grid LD-D2 latent synthesis math."""

from __future__ import annotations

import math
from collections.abc import Mapping

import torch
import torch.nn.functional as functional

from .contract import (
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    ArtisticMode,
    D2Context,
    D2ContractError,
    D2Controls,
    ProcessResult,
    Routing,
    Xs5Routing,
)


def _validate_slot(name: str, tensor: object) -> torch.Tensor:
    if not isinstance(tensor, torch.Tensor):
        raise D2ContractError("tensor.type", f"{name} must be a torch.Tensor")
    if tensor.ndim != 5 or tensor.shape[0] != 1 or tensor.shape[1] != 24:
        raise D2ContractError("tensor.shape", f"{name} must have layout [1,24,1,H,W]")
    if tensor.shape[2] != 1 or tensor.shape[3] < 1 or tensor.shape[4] < 1:
        raise D2ContractError("tensor.shape", f"{name} must have layout [1,24,1,H,W]")
    if tensor.layout is not torch.strided:
        raise D2ContractError("tensor.layout", f"{name} must use dense strided storage")
    if tensor.device.type not in {"cpu", "cuda"}:
        raise D2ContractError("tensor.device", f"{name} must use CPU or CUDA")
    if tensor.dtype != torch.float16:
        raise D2ContractError("tensor.dtype", f"{name} runtime dtype must be F16")
    if tensor.numel() // 24 > MAX_SPATIAL_TOKENS:
        raise D2ContractError(
            "tensor.too_large",
            f"{name} exceeds the {MAX_SPATIAL_TOKENS}-token full-grid bound",
        )
    if not bool(torch.isfinite(tensor).all().item()):
        raise D2ContractError("tensor.non_finite", f"{name} contains NaN or Inf")
    return tensor


def _validate_inputs(
    a: object,
    b: object,
    controls: D2Controls,
    context: D2Context,
) -> tuple[torch.Tensor, torch.Tensor]:
    context.validate()
    slot_a = _validate_slot("A", a)
    slot_b = _validate_slot("B", b)
    if slot_a.shape != slot_b.shape:
        raise D2ContractError("tensor.incompatible_shape", "A and B shapes must match exactly")
    if slot_a.device != slot_b.device:
        raise D2ContractError("tensor.incompatible_device", "A and B must use the same device")
    tokens = slot_a.shape[-2] * slot_a.shape[-1]
    if (
        controls.algorithm is Algorithm.XS5
        and controls.xs5_routing is Xs5Routing.TOPK
        and controls.top_k > tokens
    ):
        raise D2ContractError(
            "control.out_of_range", "top_k cannot exceed the complete spatial grid"
        )
    for name, previous in (("previous_a", context.previous_a), ("previous_b", context.previous_b)):
        if previous is None:
            continue
        checked = _validate_slot(name, previous)
        if checked.shape != slot_a.shape or checked.device != slot_a.device:
            raise D2ContractError(
                "tensor.incompatible_history",
                f"{name} must match the current slot shape and device exactly",
            )
    return slot_a, slot_b


def _linear(a: torch.Tensor, b: torch.Tensor, mix: float) -> torch.Tensor:
    if mix == 0.0:
        return a.clone()
    if mix == 1.0:
        return b.clone()
    return torch.lerp(a.float(), b.float(), mix).to(dtype=a.dtype)


def _xs1(donor: torch.Tensor, controls: D2Controls) -> torch.Tensor:
    output = donor.float().clone()
    angle = math.radians(controls.xs1_angle_degrees)
    cosine = math.cos(angle)
    sine = math.sin(angle)
    first = donor[:, controls.xs1_channel_a].float()
    second = donor[:, controls.xs1_channel_b].float()
    output[:, controls.xs1_channel_a] = cosine * first - sine * second
    output[:, controls.xs1_channel_b] = sine * first + cosine * second
    return output


def _xs2(donor: torch.Tensor, controls: D2Controls) -> torch.Tensor:
    source = donor.float()
    radius = controls.xs2_radius
    return 0.25 * (
        torch.roll(source, shifts=radius, dims=-2)
        + torch.roll(source, shifts=-radius, dims=-2)
        + torch.roll(source, shifts=radius, dims=-1)
        + torch.roll(source, shifts=-radius, dims=-1)
    )


def _xs3(
    carrier: torch.Tensor,
    donor: torch.Tensor,
    previous_carrier: torch.Tensor,
    previous_donor: torch.Tensor,
    controls: D2Controls,
) -> torch.Tensor:
    carrier_now = carrier.float()
    donor_now = donor.float()
    carrier_previous = previous_carrier.float()
    donor_previous = previous_donor.float()
    donor_low = 0.5 * (donor_now + donor_previous)
    donor_high = donor_now - donor_previous
    carrier_high = carrier_now - carrier_previous
    return donor_low + controls.xs3_high_gain * (donor_high - carrier_high)


def _xs4(carrier: torch.Tensor, donor: torch.Tensor, controls: D2Controls) -> torch.Tensor:
    source = donor.float()
    structural = carrier.float()
    dimensions = (-2, -1)
    donor_mean = source.mean(dim=dimensions, keepdim=True)
    donor_std = source.std(dim=dimensions, keepdim=True, correction=0)
    carrier_mean = structural.mean(dim=dimensions, keepdim=True)
    carrier_std = structural.std(dim=dimensions, keepdim=True, correction=0)
    normalized = (source - donor_mean) / donor_std.clamp_min(controls.xs4_epsilon)
    return normalized * carrier_std + carrier_mean


def _normalized_tokens(slot: torch.Tensor) -> torch.Tensor:
    tokens = slot[0, :, 0].permute(1, 2, 0).reshape(-1, 24).float()
    centered = tokens - tokens.mean(dim=-1, keepdim=True)
    return functional.normalize(centered, dim=-1, eps=1e-6)


def _tokens(slot: torch.Tensor) -> torch.Tensor:
    return slot[0, :, 0].permute(1, 2, 0).reshape(-1, 24).float()


def _restore_tokens(tokens: torch.Tensor, reference: torch.Tensor) -> torch.Tensor:
    height, width = reference.shape[-2:]
    return tokens.reshape(height, width, 24).permute(2, 0, 1).unsqueeze(0).unsqueeze(2)


def _xs5_topk(carrier: torch.Tensor, donor: torch.Tensor, controls: D2Controls) -> torch.Tensor:
    query = _normalized_tokens(carrier)
    key = _normalized_tokens(donor)
    values, indices = torch.topk(query @ key.transpose(0, 1), k=controls.top_k, dim=-1, sorted=True)
    weights = torch.softmax(values / controls.temperature, dim=-1)
    selected = _tokens(donor)[indices]
    transported = (weights.unsqueeze(-1) * selected).sum(dim=-2)
    return _restore_tokens(transported, carrier)


def _xs5_sinkhorn(carrier: torch.Tensor, donor: torch.Tensor, controls: D2Controls) -> torch.Tensor:
    query = _normalized_tokens(carrier)
    key = _normalized_tokens(donor)
    log_plan = ((query @ key.transpose(0, 1)) / controls.temperature).clamp(-20.0, 20.0)
    for _ in range(controls.sinkhorn_iterations):
        log_plan = log_plan - torch.logsumexp(log_plan, dim=1, keepdim=True)
        log_plan = log_plan - torch.logsumexp(log_plan, dim=0, keepdim=True)
    weights = torch.softmax(log_plan, dim=1)
    transported = weights @ _tokens(donor)
    return _restore_tokens(transported, carrier)


def _effect(
    carrier: torch.Tensor,
    donor: torch.Tensor,
    previous_carrier: torch.Tensor,
    previous_donor: torch.Tensor,
    controls: D2Controls,
) -> torch.Tensor:
    if controls.algorithm is Algorithm.XS1:
        return _xs1(donor, controls)
    if controls.algorithm is Algorithm.XS2:
        return _xs2(donor, controls)
    if controls.algorithm is Algorithm.XS3:
        return _xs3(carrier, donor, previous_carrier, previous_donor, controls)
    if controls.algorithm is Algorithm.XS4:
        return _xs4(carrier, donor, controls)
    if controls.algorithm is Algorithm.XS5:
        if controls.xs5_routing is Xs5Routing.SINKHORN:
            return _xs5_sinkhorn(carrier, donor, controls)
        return _xs5_topk(carrier, donor, controls)
    return donor.float()


def _combine(
    base: torch.Tensor,
    carrier: torch.Tensor,
    donor: torch.Tensor,
    routed: torch.Tensor,
    controls: D2Controls,
) -> torch.Tensor:
    structural = carrier.float()
    if controls.mode is ArtisticMode.INTERACT:
        target = structural + (1.0 - controls.preserve) * (routed - donor.float())
    else:
        target = controls.preserve * structural + (1.0 - controls.preserve) * routed
    return torch.lerp(base.float(), target, controls.interaction)


def _chaos(tensor: torch.Tensor, amount: float, seed: int) -> torch.Tensor:
    if amount == 0.0:
        return tensor
    height, width = tensor.shape[-2:]
    channel_shift = seed % 23 + 1
    y_shift = 0 if height == 1 else (seed // 23) % (height - 1) + 1
    x_shift = 0 if width == 1 else (seed // (23 * max(height, 2))) % (width - 1) + 1
    permuted = torch.roll(
        tensor,
        shifts=(channel_shift, y_shift, x_shift),
        dims=(1, 3, 4),
    )
    return tensor + amount * 0.125 * (permuted - tensor)


def _provenance(
    controls: D2Controls,
    context: D2Context,
    tensor: torch.Tensor,
) -> dict[str, object]:
    height, width = tensor.shape[-2:]
    return {
        "operation": {
            "operator_id": OPERATOR_ID,
            "operator_version": OPERATOR_VERSION,
            "seed": context.seed,
            "controls": controls.as_dict(),
        },
        "profile": {
            "codec_family": context.codec_family,
            "profile": context.profile,
            "profile_version": context.profile_version,
            "timing_contract": context.timing_contract,
            "timing_contract_version": context.timing_contract_version,
            "frame_rate": {
                "numerator": context.frame_rate_numerator,
                "denominator": context.frame_rate_denominator,
            },
        },
        "playheads": {"a": context.playhead_a, "b": context.playhead_b},
        "structural_carrier": controls.routing.value,
        "grid": {"height": height, "width": width, "tokens": height * width, "full": True},
        "history": {
            "previous_a_supplied": context.previous_a is not None,
            "previous_b_supplied": context.previous_b is not None,
        },
    }


@torch.inference_mode()
def process_slot(
    a: torch.Tensor,
    b: torch.Tensor,
    controls: D2Controls | Mapping[str, object] | None = None,
    context: D2Context | Mapping[str, object] | None = None,
) -> ProcessResult:
    """Process one pair of independent H3 playhead slots deterministically."""

    parsed_controls = (
        controls if isinstance(controls, D2Controls) else D2Controls.from_mapping(controls)
    )
    parsed_controls.validate()
    parsed_context = context if isinstance(context, D2Context) else D2Context.from_mapping(context)
    slot_a, slot_b = _validate_inputs(a, b, parsed_controls, parsed_context)
    base = _linear(slot_a, slot_b, parsed_controls.mix)

    if parsed_controls.algorithm is Algorithm.LINEAR or parsed_controls.interaction == 0.0:
        processed = base
    else:
        if parsed_controls.routing is Routing.A:
            carrier, donor = slot_a, slot_b
            previous_carrier = (
                parsed_context.previous_a if parsed_context.previous_a is not None else slot_a
            )
            previous_donor = (
                parsed_context.previous_b if parsed_context.previous_b is not None else slot_b
            )
        else:
            carrier, donor = slot_b, slot_a
            previous_carrier = (
                parsed_context.previous_b if parsed_context.previous_b is not None else slot_b
            )
            previous_donor = (
                parsed_context.previous_a if parsed_context.previous_a is not None else slot_a
            )
        routed = _effect(
            carrier,
            donor,
            previous_carrier,
            previous_donor,
            parsed_controls,
        )
        processed = _combine(base, carrier, donor, routed, parsed_controls)

    output = _chaos(processed.float(), parsed_controls.chaos, parsed_context.seed).to(
        dtype=slot_a.dtype
    )
    output = output.contiguous()
    if not bool(torch.isfinite(output).all().item()):
        raise D2ContractError("tensor.non_finite_output", "operator produced NaN or Inf")
    return ProcessResult(
        output=output, provenance=_provenance(parsed_controls, parsed_context, output)
    )
