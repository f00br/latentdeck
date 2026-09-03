"""Deterministic, full-grid LD-D2 latent synthesis math."""

from __future__ import annotations

import math
from collections.abc import Mapping
from dataclasses import dataclass

import torch
import torch.nn.functional as functional
from latentdeck_deck_sdk import (
    DeckOperatorContext,
    DeckOperatorResult,
    validate_process_call,
    validate_process_result,
)

from .contract import (
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    ArtisticMode,
    D2ContractError,
    D2Controls,
    Routing,
    Xs5Routing,
)


@dataclass(frozen=True, slots=True)
class _D2RuntimeContext:
    codec_family: str
    profile: str
    profile_version: str
    timing_contract: str
    timing_contract_version: str
    frame_rate_numerator: int
    frame_rate_denominator: int
    playhead_a: int
    playhead_b: int
    seed: int
    previous_a: torch.Tensor | None
    previous_b: torch.Tensor | None


def _linear(a: torch.Tensor, b: torch.Tensor, mix: float) -> torch.Tensor:
    if mix == 0.0:
        return a.clone()
    if mix == 1.0:
        return b.clone()
    return torch.lerp(a.float(), b.float(), mix).to(dtype=a.dtype)


def _xs1(donor: torch.Tensor, controls: D2Controls) -> torch.Tensor:
    # Keep one private F32 materialization for both the output and the rotated
    # channel reads. ``copy=True`` also preserves the source for generic F32
    # profiles, where ``donor.float()`` alone would alias the caller's tensor.
    output = donor.to(dtype=torch.float32, copy=True)
    angle = math.radians(controls.xs1_angle_degrees)
    cosine = math.cos(angle)
    sine = math.sin(angle)
    first = output[:, controls.xs1_channel_a]
    second = output[:, controls.xs1_channel_b]
    rotated_first = cosine * first - sine * second
    rotated_second = sine * first + cosine * second
    output[:, controls.xs1_channel_a] = rotated_first
    output[:, controls.xs1_channel_b] = rotated_second
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
    tokens = _tokens(slot)
    centered = tokens - tokens.mean(dim=-1, keepdim=True)
    return functional.normalize(centered, dim=-1, eps=1e-6)


def _tokens(slot: torch.Tensor) -> torch.Tensor:
    channels = slot.shape[1]
    return slot[0, :, 0].permute(1, 2, 0).reshape(-1, channels).float()


def _restore_tokens(tokens: torch.Tensor, reference: torch.Tensor) -> torch.Tensor:
    channels = reference.shape[1]
    height, width = reference.shape[-2:]
    return tokens.reshape(height, width, channels).permute(2, 0, 1).unsqueeze(0).unsqueeze(2)


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
    channels = tensor.shape[1]
    height, width = tensor.shape[-2:]
    channel_shift = 0 if channels == 1 else seed % (channels - 1) + 1
    y_shift = 0 if height == 1 else (seed // 23) % (height - 1) + 1
    x_shift = 0 if width == 1 else (seed // (23 * max(height, 2))) % (width - 1) + 1
    permuted = torch.roll(
        tensor,
        shifts=(channel_shift, y_shift, x_shift),
        dims=(1, 3, 4),
    )
    return tensor + amount * 0.125 * (permuted - tensor)


def _generic_role_slots(context: DeckOperatorContext) -> dict[str, int]:
    role_slots = {binding.role: binding.physical_slot for binding in context.roles}
    if set(role_slots) != {"carrier", "donor"} or set(role_slots.values()) != {1, 2}:
        raise D2ContractError(
            "context.roles",
            "D2 roles must be an exact carrier/donor permutation over physical slots 1 and 2",
        )
    return role_slots


def _generic_controls(
    controls: Mapping[str, object],
    role_slots: Mapping[str, int],
) -> D2Controls:
    values = dict(controls)
    # Declarative faceplates use canonical lower-case identifiers while the
    # mathematical enums remain upper-case internally.
    for name in ("algorithm", "mode", "routing", "xs5_routing"):
        value = values.get(name)
        if isinstance(value, str):
            values[name] = value.upper()
    role_routing = Routing.A if role_slots["carrier"] == 1 else Routing.B
    if "routing" in values and values["routing"] != role_routing.value:
        raise D2ContractError(
            "context.role_conflict",
            "routing control conflicts with the authoritative carrier/donor role binding",
        )
    values["routing"] = role_routing.value
    parsed = D2Controls.from_mapping(values)
    parsed.validate()
    return parsed


def _validate_generic_constraints(
    sources: tuple[torch.Tensor, torch.Tensor],
    controls: D2Controls,
) -> None:
    channels = sources[0].shape[1]
    tokens = sources[0].shape[-2] * sources[0].shape[-1]
    if tokens > MAX_SPATIAL_TOKENS:
        raise D2ContractError(
            "tensor.too_large",
            f"sources exceed the {MAX_SPATIAL_TOKENS}-token full-grid bound",
        )
    if (
        controls.algorithm is Algorithm.XS1
        and max(controls.xs1_channel_a, controls.xs1_channel_b) >= channels
    ):
        raise D2ContractError(
            "control.out_of_range", "XS1 channel controls exceed the negotiated channel count"
        )
    if (
        controls.algorithm is Algorithm.XS5
        and controls.xs5_routing is Xs5Routing.TOPK
        and controls.top_k > tokens
    ):
        raise D2ContractError(
            "control.out_of_range", "top_k cannot exceed the complete spatial grid"
        )


def _provenance(
    controls: D2Controls,
    context: _D2RuntimeContext,
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


def _process_pair(
    slot_a: torch.Tensor,
    slot_b: torch.Tensor,
    parsed_controls: D2Controls,
    parsed_context: _D2RuntimeContext,
) -> DeckOperatorResult:
    _validate_generic_constraints((slot_a, slot_b), parsed_controls)
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
    return DeckOperatorResult(
        output=output, provenance=_provenance(parsed_controls, parsed_context, output)
    )


@torch.inference_mode()
def process_sources_host(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult:
    """Host entrypoint over an already Deck-SDK-validated call.

    Protocol 2 invokes this function through ``process_sources_checked``. Keep
    operator-specific role/control bounds here, while the shared SDK owns the
    single tensor finite gate before and after the operator.
    """

    if len(sources) != 2:
        raise D2ContractError("tensor.source_count", "D2 requires exactly two sources")
    role_slots = _generic_role_slots(context)
    parsed_controls = _generic_controls(controls, role_slots)
    source_index_by_slot = {
        physical_slot: index for index, physical_slot in enumerate(context.physical_slots)
    }
    index_a = source_index_by_slot[1]
    index_b = source_index_by_slot[2]
    slot_a = sources[index_a]
    slot_b = sources[index_b]
    previous_a = context.previous_sources[index_a]
    previous_b = context.previous_sources[index_b]
    parsed_context = _D2RuntimeContext(
        codec_family=context.codec_family,
        profile=context.profile,
        profile_version=context.profile_version,
        timing_contract=context.timing_contract,
        timing_contract_version=context.timing_contract_version,
        frame_rate_numerator=context.frame_rate_numerator,
        frame_rate_denominator=context.frame_rate_denominator,
        playhead_a=context.playheads[index_a],
        playhead_b=context.playheads[index_b],
        seed=context.seed,
        previous_a=previous_a if isinstance(previous_a, torch.Tensor) else None,
        previous_b=previous_b if isinstance(previous_b, torch.Tensor) else None,
    )
    return _process_pair(slot_a, slot_b, parsed_controls, parsed_context)


@torch.inference_mode()
def process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult:
    """Checked standalone entrypoint used by tests and direct SDK callers."""

    parsed_mapping = validate_process_call(sources, controls, context)
    result = process_sources_host(sources, parsed_mapping, context)
    return validate_process_result(result, sources)
