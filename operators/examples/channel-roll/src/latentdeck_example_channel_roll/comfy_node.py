"""Copyable ComfyUI wrapper for the example dual-source operator."""

from __future__ import annotations

import json
from collections.abc import Mapping

import torch
from latentdeck_comfy_toolkit import (
    InstalledOperator,
    LatentDeckResearchOperatorHook,
    OperatorContext,
    TrustedOperatorRegistry,
    build_installed_operator_research_hook,
)
from latentdeck_comfy_toolkit.decoder_compare import ToolkitContractError
from latentdeck_comfy_toolkit.research_ops import visual_latent
from latentdeck_comfy_toolkit.workflow_metadata import annotate_operation

from .descriptor import get_descriptor
from .operator import process_sources


def _installed_operator() -> InstalledOperator:
    registry = TrustedOperatorRegistry()
    registry.install(
        get_descriptor(),
        process_sources,
        exported_entrypoint="latentdeck_example_channel_roll:process_sources",
    )
    return registry.load("org.latentdeck.example.channel_roll", "0.1.0")


def _process_sequence(
    installed: InstalledOperator,
    carrier: torch.Tensor,
    donor: torch.Tensor,
    *,
    controls: Mapping[str, object],
    seed: int,
    processing_mode: str,
    slot_offset: int = 0,
) -> tuple[torch.Tensor, list[dict[str, object]]]:
    if (
        donor.shape != carrier.shape
        or donor.dtype != carrier.dtype
        or donor.device != carrier.device
    ):
        raise ToolkitContractError(
            "example.tensor_incompatible",
            "carrier and donor must match shape, dtype, and device exactly",
        )
    slots: list[torch.Tensor] = []
    receipts: list[dict[str, object]] = []
    for local_slot in range(carrier.shape[2]):
        result = installed.process_dual(
            carrier[:, :, local_slot : local_slot + 1].contiguous(),
            donor[:, :, local_slot : local_slot + 1].contiguous(),
            controls,
            OperatorContext(
                seed=seed,
                slot_index=slot_offset + local_slot,
                processing_mode=processing_mode,
            ),
        )
        slots.append(result.output)
        receipts.append(result.provenance)
    return torch.cat(slots, dim=2).contiguous(), receipts


class LatentDeckExampleChannelRoll:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Examples"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor": ("LATENT",),
                "amount": ("FLOAT", {"default": 0.5, "min": 0.0, "max": 1.0, "step": 0.01}),
                "channel_shift": ("INT", {"default": 1, "min": 1, "max": 23}),
                "seed": ("INT", {"default": 0, "min": 0, "max": 9_007_199_254_740_991}),
            }
        }

    def process(
        self,
        carrier: object,
        donor: object,
        amount: float,
        channel_shift: int,
        seed: int,
    ) -> tuple[object, str]:
        carrier_surface = visual_latent(carrier, "carrier")
        donor_surface = visual_latent(donor, "donor")
        controls = {"amount": amount, "channel_shift": channel_shift}
        output, receipts = _process_sequence(
            _installed_operator(),
            carrier_surface.visual,
            donor_surface.visual,
            controls=controls,
            seed=seed,
            processing_mode="full_clip",
        )
        report = {
            "schema_version": "0.1.0",
            "operation": receipts[0]["operation"],
            "topology": "dual_source",
            "sequence": {
                "slots": len(receipts),
                "processing": "ORDERED_SLOT_CALLS",
                "slot_receipts_identical_except_index": all(
                    receipt.get("operation") == receipts[0].get("operation")
                    for receipt in receipts[1:]
                ),
            },
        }
        annotated = annotate_operation(
            carrier_surface.repack(output),
            sources=(("carrier", carrier), ("donor", donor)),
            structural_role="carrier",
            provenance={"operation": receipts[0]["operation"]},
        )
        return annotated, json.dumps(
            report,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )


class LatentDeckExampleChannelRollHook:
    """Expose this separately installed operator to Toolkit evaluation nodes."""

    RETURN_TYPES = ("LATENTDECK_OPERATOR_HOOK",)
    RETURN_NAMES = ("operator_hook",)
    FUNCTION = "build"
    CATEGORY = "LatentDeck/Examples"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "donor": ("LATENT",),
                "amount": ("FLOAT", {"default": 0.5, "min": 0.0, "max": 1.0, "step": 0.01}),
                "channel_shift": ("INT", {"default": 1, "min": 1, "max": 23}),
                "seed": ("INT", {"default": 0, "min": 0, "max": 9_007_199_254_740_991}),
            }
        }

    def build(
        self,
        donor: object,
        amount: float,
        channel_shift: int,
        seed: int,
    ) -> tuple[LatentDeckResearchOperatorHook]:
        installed = _installed_operator()
        controls: dict[str, object] = {
            "amount": amount,
            "channel_shift": channel_shift,
        }
        return (
            build_installed_operator_research_hook(
                installed,
                captured_sources=(donor,),
                controls=controls,
                seed=seed,
                name="external/org.latentdeck.example.channel_roll",
            ),
        )


NODE_CLASS_MAPPINGS = {
    "LatentDeckExampleChannelRoll": LatentDeckExampleChannelRoll,
    "LatentDeckExampleChannelRollHook": LatentDeckExampleChannelRollHook,
}
NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckExampleChannelRoll": "LatentDeck Example — MyLatentOperator Channel Roll",
    "LatentDeckExampleChannelRollHook": "LatentDeck Example — Channel Roll Test Hook",
}


__all__ = [
    "NODE_CLASS_MAPPINGS",
    "NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckExampleChannelRoll",
    "LatentDeckExampleChannelRollHook",
]
