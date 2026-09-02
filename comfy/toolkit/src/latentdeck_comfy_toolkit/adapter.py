"""Stable clean-room adapters over the reviewed LD-D2 operator surface."""

from __future__ import annotations

import json
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

import torch
from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding
from latentdeck_operator_d2 import (
    MAX_SPATIAL_TOKENS,
    OPERATOR_ID,
    OPERATOR_VERSION,
    Algorithm,
    D2ContractError,
    D2Controls,
    Routing,
    process_sources,
)

MAX_TEMPORAL_SLOTS = 512
MAX_SEQUENCE_VALUES = 50_331_648
TOOLKIT_ADAPTER_VERSION = "0.1.0"


@dataclass(frozen=True, slots=True)
class XsSequenceResult:
    """A full H3 sequence and bounded JSON-safe research provenance."""

    output: torch.Tensor
    provenance: dict[str, Any]


def _validate_sequence(name: str, value: object) -> torch.Tensor:
    if not isinstance(value, torch.Tensor):
        raise D2ContractError("tensor.type", f"{name} must be a torch.Tensor")
    if value.ndim != 5 or value.shape[0] != 1 or value.shape[1] != 24:
        raise D2ContractError("tensor.shape", f"{name} must have layout [1,24,T,H,W]")
    if not 1 <= value.shape[2] <= MAX_TEMPORAL_SLOTS:
        raise D2ContractError(
            "tensor.temporal_bound",
            f"{name} temporal slots must be in [1, {MAX_TEMPORAL_SLOTS}]",
        )
    if value.shape[3] < 1 or value.shape[4] < 1:
        raise D2ContractError("tensor.shape", f"{name} must have layout [1,24,T,H,W]")
    if value.shape[3] * value.shape[4] > MAX_SPATIAL_TOKENS:
        raise D2ContractError(
            "tensor.too_large",
            f"{name} exceeds the {MAX_SPATIAL_TOKENS}-token full-grid bound",
        )
    if value.numel() > MAX_SEQUENCE_VALUES:
        raise D2ContractError(
            "tensor.too_large",
            f"{name} exceeds the {MAX_SEQUENCE_VALUES}-value sequence bound",
        )
    if value.layout is not torch.strided:
        raise D2ContractError("tensor.layout", f"{name} must use dense strided storage")
    if value.device.type not in {"cpu", "cuda"}:
        raise D2ContractError("tensor.device", f"{name} must use CPU or CUDA")
    if value.dtype != torch.float16:
        raise D2ContractError("tensor.dtype", f"{name} runtime dtype must be F16")
    if not bool(torch.isfinite(value).all().item()):
        raise D2ContractError("tensor.non_finite", f"{name} contains NaN or Inf")
    return value


def _parse_algorithm(value: object) -> Algorithm:
    if not isinstance(value, str) or value == Algorithm.LINEAR.value:
        raise D2ContractError("control.enum", "algorithm must be one of XS1, XS2, XS3, XS4, XS5")
    try:
        algorithm = Algorithm(value)
    except ValueError as error:
        raise D2ContractError(
            "control.enum", "algorithm must be one of XS1, XS2, XS3, XS4, XS5"
        ) from error
    return algorithm


@torch.inference_mode()
def process_xs_sequence(
    a: torch.Tensor,
    b: torch.Tensor,
    *,
    algorithm: str,
    controls: Mapping[str, object] | None = None,
    seed: int = 0,
) -> XsSequenceResult:
    """Process a complete H3 sequence through the authoritative Deck SDK boundary.

    The adapter owns only sequence iteration and provenance aggregation. All
    XS1--XS5 math and slot-level validation remain in
    :func:`latentdeck_operator_d2.process_sources`.
    """

    slot_a = _validate_sequence("A", a)
    slot_b = _validate_sequence("B", b)
    if slot_a.shape != slot_b.shape:
        raise D2ContractError("tensor.incompatible_shape", "A and B shapes must match exactly")
    if slot_a.device != slot_b.device:
        raise D2ContractError("tensor.incompatible_device", "A and B must use the same device")

    selected = _parse_algorithm(algorithm)
    if controls is not None and not isinstance(controls, Mapping):
        raise D2ContractError("control.type", "controls must be an object")
    raw_controls = dict(controls or {})
    if "algorithm" in raw_controls:
        raise D2ContractError("control.conflict", "algorithm is selected by the adapter")
    parsed_controls = D2Controls.from_mapping({"algorithm": selected.value, **raw_controls})
    role_slots = (
        (RoleBinding("carrier", 1), RoleBinding("donor", 2))
        if parsed_controls.routing is Routing.A
        else (RoleBinding("carrier", 2), RoleBinding("donor", 1))
    )

    output_slots: list[torch.Tensor] = []
    for slot_index in range(slot_a.shape[2]):
        current_a = slot_a[:, :, slot_index : slot_index + 1].contiguous()
        current_b = slot_b[:, :, slot_index : slot_index + 1].contiguous()
        previous_a = (
            None
            if slot_index == 0
            else slot_a[:, :, slot_index - 1 : slot_index].contiguous()
        )
        previous_b = (
            None
            if slot_index == 0
            else slot_b[:, :, slot_index - 1 : slot_index].contiguous()
        )
        context = DeckOperatorContext(
            codec_family="minimax_h3",
            profile="h3_av_latent",
            profile_version="0.1.0",
            timing_contract="minimax_h3_causal",
            timing_contract_version="0.1.0",
            frame_rate_numerator=24,
            frame_rate_denominator=1,
            generation=1,
            sequence=slot_index + 1,
            seed=seed,
            playheads=(slot_index, slot_index),
            physical_slots=(1, 2),
            roles=role_slots,
            previous_sources=(previous_a, previous_b),
        )
        output_slots.append(
            process_sources(
                (current_a, current_b),
                parsed_controls.as_dict(),
                context,
            ).output
        )

    output = torch.cat(output_slots, dim=2).contiguous()
    provenance: dict[str, Any] = {
        "schema_version": TOOLKIT_ADAPTER_VERSION,
        "kind": "latentdeck.toolkit.xs_sequence",
        "execution_surface": "comfy_research",
        "operation": {
            "operator_id": OPERATOR_ID,
            "operator_version": OPERATOR_VERSION,
            "adapter_version": TOOLKIT_ADAPTER_VERSION,
            "algorithm": selected.value,
            "seed": seed,
            "controls": parsed_controls.as_dict(),
        },
        "profile": {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
        },
        "sequence": {
            "layout": "[1,24,T,H,W]",
            "shape": list(output.shape),
            "runtime_dtype": "F16",
            "slots": output.shape[2],
            "spatial_tokens": output.shape[3] * output.shape[4],
            "full_grid": True,
        },
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return XsSequenceResult(output=output, provenance=provenance)


__all__ = [
    "MAX_SEQUENCE_VALUES",
    "MAX_TEMPORAL_SLOTS",
    "TOOLKIT_ADAPTER_VERSION",
    "XsSequenceResult",
    "process_xs_sequence",
]
