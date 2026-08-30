"""Small deterministic operator used only as an external SDK example."""

from __future__ import annotations

import torch
from latentdeck_comfy_toolkit import OperatorContext, ToolkitOperatorResult

OPERATOR_ID = "org.latentdeck.example.channel_roll"
OPERATOR_VERSION = "0.1.0"


@torch.inference_mode()
def process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: OperatorContext,
) -> ToolkitOperatorResult:
    """Blend the carrier with a deterministic seed-offset donor channel roll."""

    carrier, donor = sources
    amount = float(controls["amount"])
    channel_shift = int(controls["channel_shift"])
    effective_shift = ((channel_shift + context.seed % 23 - 1) % 23) + 1
    routed = torch.roll(donor.float(), shifts=effective_shift, dims=1)
    output = torch.lerp(carrier.float(), routed, amount).to(torch.float16).contiguous()
    return ToolkitOperatorResult(
        output=output,
        provenance={
            "operation": {
                "operator_id": OPERATOR_ID,
                "operator_version": OPERATOR_VERSION,
                "seed": context.seed,
                "controls": controls,
            },
            "profile": {
                "codec_family": context.codec_family,
                "profile": context.profile,
                "profile_version": context.profile_version,
                "timing_contract": context.timing_contract,
                "timing_contract_version": context.timing_contract_version,
            },
            "processing_mode": context.processing_mode,
            "slot_index": context.slot_index,
            "effective_channel_shift": effective_shift,
            "full_grid": True,
        },
    )


__all__ = ["OPERATOR_ID", "OPERATOR_VERSION", "process_sources"]
