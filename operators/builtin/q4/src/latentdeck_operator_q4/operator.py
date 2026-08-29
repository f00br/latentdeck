"""Deterministic LD-Q4 synthesis math."""

from __future__ import annotations

from collections.abc import Mapping

import torch
import torch.nn.functional as functional

from .contract import (
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    ArtisticMode,
    ProcessResult,
    Q4Context,
    Q4ContractError,
    Q4Controls,
    Xs5Routing,
)


def _validate_slot(name: str, tensor: object) -> torch.Tensor:
    if not isinstance(tensor, torch.Tensor):
        raise Q4ContractError("tensor.type", f"{name} must be a torch.Tensor")
    if (
        tensor.ndim != 5
        or tensor.shape[0] != 1
        or tensor.shape[1] != 24
        or tensor.shape[2] != 1
        or tensor.shape[3] < 1
        or tensor.shape[4] < 1
    ):
        raise Q4ContractError("tensor.shape", f"{name} must have layout [1,24,1,H,W]")
    if tensor.layout is not torch.strided:
        raise Q4ContractError("tensor.layout", f"{name} must use dense strided storage")
    if tensor.device.type not in {"cpu", "cuda"}:
        raise Q4ContractError("tensor.device", f"{name} must use CPU or CUDA")
    if tensor.dtype != torch.float16:
        raise Q4ContractError("tensor.dtype", f"{name} runtime dtype must be F16")
    if tensor.shape[-2] * tensor.shape[-1] > MAX_SPATIAL_TOKENS:
        raise Q4ContractError(
            "tensor.too_large",
            f"{name} exceeds the {MAX_SPATIAL_TOKENS}-token full-grid bound",
        )
    if not bool(torch.isfinite(tensor).all().item()):
        raise Q4ContractError("tensor.non_finite", f"{name} contains NaN or Inf")
    return tensor


def _tokens(slot: torch.Tensor) -> torch.Tensor:
    return slot[0, :, 0].permute(1, 2, 0).reshape(-1, 24).float()


def _normalized_tokens(slot: torch.Tensor) -> torch.Tensor:
    tokens = _tokens(slot)
    centered = tokens - tokens.mean(dim=-1, keepdim=True)
    return functional.normalize(centered, dim=-1, eps=1e-6)


def _restore_donor_batch(tokens: torch.Tensor, reference: torch.Tensor) -> torch.Tensor:
    height, width = reference.shape[-2:]
    return tokens.reshape(3, height, width, 24).permute(0, 3, 1, 2).unsqueeze(2)


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
        index=indices.unsqueeze(-1).expand(-1, -1, -1, 24),
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
    accumulator = carrier.float().clone()
    structural = carrier.float()
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
    channel_steps = (1, 5, 7, 11, 13, 17, 19, 23)
    step = channel_steps[seed % len(channel_steps)]
    offset = (seed // len(channel_steps)) % 24
    channel_indices = (torch.arange(24, device=tensor.device) * step + offset) % 24
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


def _provenance(
    controls: Q4Controls,
    context: Q4Context,
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


@torch.inference_mode()
def process_slot(
    carrier: torch.Tensor,
    donor_b: torch.Tensor,
    donor_c: torch.Tensor,
    donor_d: torch.Tensor,
    controls: Q4Controls | Mapping[str, object] | None = None,
    context: Q4Context | Mapping[str, object] | None = None,
) -> ProcessResult:
    """Process one carrier slot and three donor slots."""

    parsed_context = context if isinstance(context, Q4Context) else Q4Context.from_mapping(context)
    parsed_context.validate()
    carrier = _validate_slot("carrier", carrier)
    donor_b = _validate_slot("donor B", donor_b)
    donor_c = _validate_slot("donor C", donor_c)
    donor_d = _validate_slot("donor D", donor_d)
    for donor in (donor_b, donor_c, donor_d):
        if donor.shape != carrier.shape:
            raise Q4ContractError(
                "tensor.incompatible_shape", "carrier and donor shapes must match exactly"
            )
        if donor.device != carrier.device:
            raise Q4ContractError(
                "tensor.incompatible_device", "carrier and donors must use the same device"
            )
    parsed = controls if isinstance(controls, Q4Controls) else Q4Controls.from_mapping(controls)
    parsed.validate()
    weight_b, weight_c, weight_d = parsed.resolved_weights()
    weights = (weight_b, weight_c, weight_d)
    donors = (donor_b, donor_c, donor_d)
    tokens = carrier.shape[-2] * carrier.shape[-1]
    if (
        parsed.algorithm is Algorithm.XS5
        and parsed.xs5_routing is Xs5Routing.TOPK
        and parsed.top_k > tokens
    ):
        raise Q4ContractError(
            "control.out_of_range", "top_k cannot exceed the complete spatial grid"
        )

    if parsed.algorithm is Algorithm.LINEAR:
        processed = _linear(carrier, donors, weights, parsed.interaction)
    elif parsed.interaction == 0.0 or parsed.preserve == 1.0:
        processed = carrier.clone()
    else:
        routed = _xs5_routed_batch(carrier, donors, parsed)
        processed = _accumulate_routed(carrier, donors, routed, weights, parsed)

    output = _chaos(processed.float(), parsed.chaos, parsed_context.seed).to(carrier.dtype)
    output = output.contiguous()
    if not bool(torch.isfinite(output).all().item()):
        raise Q4ContractError("tensor.non_finite_output", "operator produced NaN or Inf")
    return ProcessResult(
        output=output,
        provenance=_provenance(parsed, parsed_context, carrier, weights),
    )
