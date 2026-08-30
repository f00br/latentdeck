"""Copyable ComfyUI wrapper for the example dual-source operator."""

from __future__ import annotations

import json

import torch
from latentdeck_comfy_toolkit import OperatorContext, TrustedOperatorRegistry
from latentdeck_comfy_toolkit.decoder_compare import ToolkitContractError
from latentdeck_comfy_toolkit.research_ops import visual_latent
from latentdeck_comfy_toolkit.workflow_metadata import annotate_operation

from .descriptor import get_descriptor
from .operator import process_sources


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
        if (
            donor_surface.visual.shape != carrier_surface.visual.shape
            or donor_surface.visual.dtype != carrier_surface.visual.dtype
            or donor_surface.visual.device != carrier_surface.visual.device
        ):
            raise ToolkitContractError(
                "example.tensor_incompatible",
                "carrier and donor must match shape, dtype, and device exactly",
            )

        registry = TrustedOperatorRegistry()
        registry.install(
            get_descriptor(),
            process_sources,
            exported_entrypoint="latentdeck_example_channel_roll:process_sources",
        )
        installed = registry.load("org.latentdeck.example.channel_roll", "0.1.0")
        controls = {"amount": amount, "channel_shift": channel_shift}
        slots: list[torch.Tensor] = []
        receipts: list[dict[str, object]] = []
        for slot in range(carrier_surface.visual.shape[2]):
            result = installed.process_dual(
                carrier_surface.visual[:, :, slot : slot + 1].contiguous(),
                donor_surface.visual[:, :, slot : slot + 1].contiguous(),
                controls,
                OperatorContext(seed=seed, slot_index=slot, processing_mode="full_clip"),
            )
            slots.append(result.output)
            receipts.append(result.provenance)
        output = torch.cat(slots, dim=2).contiguous()
        report = {
            "schema_version": "0.1.0",
            "operation": receipts[0]["operation"],
            "topology": "dual_source",
            "sequence": {
                "slots": len(slots),
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


NODE_CLASS_MAPPINGS = {"LatentDeckExampleChannelRoll": LatentDeckExampleChannelRoll}
NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckExampleChannelRoll": "LatentDeck Example — MyLatentOperator Channel Roll"
}


__all__ = [
    "NODE_CLASS_MAPPINGS",
    "NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckExampleChannelRoll",
]
