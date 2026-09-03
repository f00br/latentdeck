"""Deterministic LD-Q4 synthesis math."""

from __future__ import annotations

import math
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
    DeckSlot,
    Q4ContractError,
    Q4Controls,
    Xs5Routing,
)


@dataclass(frozen=True, slots=True)
class _Q4RuntimeContext:
    codec_family: str
    profile: str
    profile_version: str
    timing_contract: str
    timing_contract_version: str
    frame_rate_numerator: int
    frame_rate_denominator: int
    carrier_slot: DeckSlot
    donor_b_slot: DeckSlot
    donor_c_slot: DeckSlot
    donor_d_slot: DeckSlot
    carrier_identity: str
    donor_b_identity: str
    donor_c_identity: str
    donor_d_identity: str
    carrier_playhead: int
    donor_b_playhead: int
    donor_c_playhead: int
    donor_d_playhead: int
    seed: int


def _tokens(slot: torch.Tensor) -> torch.Tensor:
    channels = slot.shape[1]
    return slot[0, :, 0].permute(1, 2, 0).reshape(-1, channels).float()


def _normalized_tokens(slot: torch.Tensor) -> torch.Tensor:
    tokens = _tokens(slot)
    centered = tokens - tokens.mean(dim=-1, keepdim=True)
    return functional.normalize(centered, dim=-1, eps=1e-6)


def _restore_donor_batch(tokens: torch.Tensor, reference: torch.Tensor) -> torch.Tensor:
    channels = reference.shape[1]
    height, width = reference.shape[-2:]
    return tokens.reshape(3, height, width, channels).permute(0, 3, 1, 2).unsqueeze(2)


def _xs5_batch_inputs(
    carrier: torch.Tensor,
    donors: tuple[torch.Tensor, torch.Tensor, torch.Tensor],
) -> tuple[torch.Tensor, torch.Tensor]:
    carrier_query = _normalized_tokens(carrier)
    donor_keys = torch.stack([_normalized_tokens(donor) for donor in donors], dim=0)
    donor_values = torch.stack([_tokens(donor) for donor in donors], dim=0)
    batched_affinity = torch.einsum("qc,dnc->dqn", carrier_query, donor_keys)
    return batched_affinity, donor_values


def _xs5_topk_batch(
    carrier: torch.Tensor,
    donors: tuple[torch.Tensor, torch.Tensor, torch.Tensor],
    controls: Q4Controls,
) -> torch.Tensor:
    affinity, donor_values = _xs5_batch_inputs(carrier, donors)
    values, indices = torch.topk(
        affinity,
        k=controls.top_k,
        dim=-1,
        sorted=True,
    )
    weights = torch.softmax(values / controls.temperature, dim=-1)
    expanded_values = donor_values.unsqueeze(1).expand(-1, affinity.shape[1], -1, -1)
    selected = torch.gather(
        expanded_values,
        dim=2,
        index=indices.unsqueeze(-1).expand(-1, -1, -1, carrier.shape[1]),
    )
    transported = (weights.unsqueeze(-1) * selected).sum(dim=-2)
    return _restore_donor_batch(transported, carrier)


def _xs5_sinkhorn_batch(
    carrier: torch.Tensor,
    donors: tuple[torch.Tensor, torch.Tensor, torch.Tensor],
    controls: Q4Controls,
) -> torch.Tensor:
    affinity, donor_values = _xs5_batch_inputs(carrier, donors)
    log_plan = (affinity / controls.temperature).clamp(-20.0, 20.0)
    for _ in range(controls.sinkhorn_iterations):
        log_plan = log_plan - torch.logsumexp(log_plan, dim=2, keepdim=True)
        log_plan = log_plan - torch.logsumexp(log_plan, dim=1, keepdim=True)
    weights = torch.softmax(log_plan, dim=2)
    transported = torch.bmm(weights, donor_values)
    return _restore_donor_batch(transported, carrier)


def _xs5_routed_batch(
    carrier: torch.Tensor,
    donors: tuple[torch.Tensor, torch.Tensor, torch.Tensor],
    controls: Q4Controls,
) -> torch.Tensor:
    if controls.xs5_routing is Xs5Routing.TOPK:
        return _xs5_topk_batch(carrier, donors, controls)
    return _xs5_sinkhorn_batch(carrier, donors, controls)


def _linear(
    carrier: torch.Tensor,
    donors: tuple[torch.Tensor, torch.Tensor, torch.Tensor],
    weights: tuple[float, float, float],
    interaction: float,
) -> torch.Tensor:
    if interaction == 0.0:
        return carrier.clone()
    donor_mix = (
        weights[0] * donors[0].float()
        + weights[1] * donors[1].float()
        + weights[2] * donors[2].float()
    )
    if interaction == 1.0:
        return donor_mix
    return torch.lerp(carrier.float(), donor_mix, interaction)


def _accumulate_routed(
    carrier: torch.Tensor,
    donors: tuple[torch.Tensor, torch.Tensor, torch.Tensor],
    routed: torch.Tensor,
    weights: tuple[float, float, float],
    controls: Q4Controls,
) -> torch.Tensor:
    structural = carrier.float()
    accumulator = structural.clone()
    for donor_index in range(3):
        donor = donors[donor_index].float()
        routed_donor = routed[donor_index].unsqueeze(0)
        if controls.mode is ArtisticMode.INTERACT:
            delta = (1.0 - controls.preserve) * (routed_donor - donor)
        else:
            target = controls.preserve * structural + (1.0 - controls.preserve) * routed_donor
            delta = target - structural
        accumulator.add_(delta, alpha=controls.interaction * weights[donor_index])
    return accumulator


def _chaos(tensor: torch.Tensor, amount: float, seed: int) -> torch.Tensor:
    if amount == 0.0:
        return tensor
    channels = tensor.shape[1]
    candidate_steps = (1, 5, 7, 11, 13, 17, 19, 23)
    channel_steps = tuple(step for step in candidate_steps if math.gcd(step, channels) == 1)
    step = channel_steps[seed % len(channel_steps)]
    offset = (seed // len(channel_steps)) % channels
    channel_indices = (torch.arange(channels, device=tensor.device) * step + offset) % channels
    permuted = tensor.index_select(1, channel_indices)
    if (seed // 193) & 1:
        permuted = torch.flip(permuted, dims=(-2,))
    if (seed // 389) & 1:
        permuted = torch.flip(permuted, dims=(-1,))
    height, width = tensor.shape[-2:]
    y_shift = 0 if height == 1 else (seed // 769) % height
    x_shift = 0 if width == 1 else (seed // 1543) % width
    permuted = torch.roll(permuted, shifts=(y_shift, x_shift), dims=(-2, -1))
    return tensor + amount * 0.125 * (permuted - tensor)


def _generic_role_slots(context: DeckOperatorContext) -> dict[str, int]:
    role_slots = {binding.role: binding.physical_slot for binding in context.roles}
    required = {"carrier", "donor_b", "donor_c", "donor_d"}
    if set(role_slots) != required or set(role_slots.values()) != {1, 2, 3, 4}:
        raise Q4ContractError(
            "context.roles",
            "Q4 roles must be an exact carrier/donor_b/donor_c/donor_d permutation",
        )
    return role_slots


def _validate_generic_constraints(
    carrier: torch.Tensor,
    controls: Q4Controls,
) -> None:
    tokens = carrier.shape[-2] * carrier.shape[-1]
    if tokens > MAX_SPATIAL_TOKENS:
        raise Q4ContractError(
            "tensor.too_large",
            f"sources exceed the {MAX_SPATIAL_TOKENS}-token full-grid bound",
        )
    if (
        controls.algorithm is Algorithm.XS5
        and controls.xs5_routing is Xs5Routing.TOPK
        and controls.top_k > tokens
    ):
        raise Q4ContractError(
            "control.out_of_range", "top_k cannot exceed the complete spatial grid"
        )


def _provenance(
    controls: Q4Controls,
    context: _Q4RuntimeContext,
    carrier: torch.Tensor,
    weights: tuple[float, float, float],
) -> dict[str, object]:
    tokens = carrier.shape[-2] * carrier.shape[-1]
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
        "roles": {
            "carrier": {
                "slot": context.carrier_slot.value,
                "identity": context.carrier_identity,
                "playhead": context.carrier_playhead,
            },
            "donors": [
                {
                    "role": "B",
                    "slot": context.donor_b_slot.value,
                    "identity": context.donor_b_identity,
                    "playhead": context.donor_b_playhead,
                },
                {
                    "role": "C",
                    "slot": context.donor_c_slot.value,
                    "identity": context.donor_c_identity,
                    "playhead": context.donor_c_playhead,
                },
                {
                    "role": "D",
                    "slot": context.donor_d_slot.value,
                    "identity": context.donor_d_identity,
                    "playhead": context.donor_d_playhead,
                },
            ],
        },
        "resolved_donor_weights": {"B": weights[0], "C": weights[1], "D": weights[2]},
        "influence_mode": controls.influence_mode.value,
        "routing": {
            "method": controls.xs5_routing.value
            if controls.algorithm is Algorithm.XS5
            else "LINEAR",
            "reference": "UNCHANGED_CARRIER",
            "carrier_affinity_reused": controls.algorithm is Algorithm.XS5,
            "donor_batch_size": 3,
            "accumulation_order": ["B", "C", "D"],
        },
        "grid": {
            "height": carrier.shape[-2],
            "width": carrier.shape[-1],
            "tokens": tokens,
            "full": True,
        },
    }


def _process_quad(
    carrier: torch.Tensor,
    donor_b: torch.Tensor,
    donor_c: torch.Tensor,
    donor_d: torch.Tensor,
    parsed: Q4Controls,
    parsed_context: _Q4RuntimeContext,
) -> DeckOperatorResult:
    _validate_generic_constraints(carrier, parsed)
    weight_b, weight_c, weight_d = parsed.resolved_weights()
    weights = (weight_b, weight_c, weight_d)
    donors = (donor_b, donor_c, donor_d)
    if parsed.algorithm is Algorithm.LINEAR:
        processed = _linear(carrier, donors, weights, parsed.interaction)
    elif parsed.interaction == 0.0 or parsed.preserve == 1.0:
        processed = carrier.clone()
    else:
        routed = _xs5_routed_batch(carrier, donors, parsed)
        processed = _accumulate_routed(carrier, donors, routed, weights, parsed)

    output = _chaos(processed.float(), parsed.chaos, parsed_context.seed).to(carrier.dtype)
    output = output.contiguous()
    return DeckOperatorResult(
        output=output,
        provenance=_provenance(parsed, parsed_context, carrier, weights),
    )


@torch.inference_mode()
def process_sources_host(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult:
    """Host entrypoint over an already Deck-SDK-validated call."""

    if len(sources) != 4:
        raise Q4ContractError("tensor.source_count", "Q4 requires exactly four sources")
    role_slots = _generic_role_slots(context)
    normalized_controls = dict(controls)
    # Host-rendered package controls are canonical lower-case identifiers;
    # the mathematical enums remain upper-case internally.
    for name in ("algorithm", "mode", "influence_mode", "xs5_routing"):
        value = normalized_controls.get(name)
        if isinstance(value, str):
            normalized_controls[name] = value.upper()
    parsed = Q4Controls.from_mapping(normalized_controls)
    parsed.validate()
    source_index_by_slot = {
        physical_slot: index for index, physical_slot in enumerate(context.physical_slots)
    }
    role_indices = {
        role: source_index_by_slot[physical_slot] for role, physical_slot in role_slots.items()
    }

    def deck_slot(role: str) -> DeckSlot:
        return DeckSlot("ABCD"[role_slots[role] - 1])

    parsed_context = _Q4RuntimeContext(
        codec_family=context.codec_family,
        profile=context.profile,
        profile_version=context.profile_version,
        timing_contract=context.timing_contract,
        timing_contract_version=context.timing_contract_version,
        frame_rate_numerator=context.frame_rate_numerator,
        frame_rate_denominator=context.frame_rate_denominator,
        carrier_slot=deck_slot("carrier"),
        donor_b_slot=deck_slot("donor_b"),
        donor_c_slot=deck_slot("donor_c"),
        donor_d_slot=deck_slot("donor_d"),
        carrier_identity=deck_slot("carrier").value,
        donor_b_identity=deck_slot("donor_b").value,
        donor_c_identity=deck_slot("donor_c").value,
        donor_d_identity=deck_slot("donor_d").value,
        carrier_playhead=context.playheads[role_indices["carrier"]],
        donor_b_playhead=context.playheads[role_indices["donor_b"]],
        donor_c_playhead=context.playheads[role_indices["donor_c"]],
        donor_d_playhead=context.playheads[role_indices["donor_d"]],
        seed=context.seed,
    )
    return _process_quad(
        sources[role_indices["carrier"]],
        sources[role_indices["donor_b"]],
        sources[role_indices["donor_c"]],
        sources[role_indices["donor_d"]],
        parsed,
        parsed_context,
    )


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
