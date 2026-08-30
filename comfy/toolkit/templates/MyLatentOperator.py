"""One-file LatentDeck Operator API 0.1 template for ComfyUI.

Copy this file into ComfyUI/custom_nodes, change the AUTHOR EDIT values and
``process_sources()``, then restart ComfyUI. Installing this Python file is an
explicit trust decision; a data-only .lc cartridge never loads it.
"""

from __future__ import annotations

import json
from collections.abc import Mapping

import torch

from latentdeck_comfy_toolkit import (
    InstalledOperator,
    LatentDeckResearchOperatorHook,
    OperatorContext,
    ToolkitOperatorResult,
    TrustedOperatorRegistry,
    build_installed_operator_research_hook,
)

# AUTHOR EDIT: keep these identities stable once cartridges cite this operator.
OPERATOR_ID = "org.example.my_latent_operator"
OPERATOR_VERSION = "0.1.0"
ENTRYPOINT = "MyLatentOperator:process_sources"

# AUTHOR EDIT: this default template is dual-source: (carrier, donor).
# The closed descriptor is validated before trusted code is called.
DESCRIPTOR: dict[str, object] = {
    "schema_version": "0.1.0",
    "operator_id": OPERATOR_ID,
    "operator_version": OPERATOR_VERSION,
    "trust": "explicit_install",
    "entrypoint": ENTRYPOINT,
    "topology": "dual_source",
    "input_count": 2,
    "capabilities": {
        "full_clip": True,
        "streaming": True,
        "chunk": True,
        "deterministic": True,
    },
    "supported_profiles": [
        {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
            "layout": "[1,24,1,H,W]",
            "runtime_dtype": "F16",
        }
    ],
    "controls": {
        "amount": {
            "type": "float",
            "default": 0.5,
            "minimum": 0.0,
            "maximum": 1.0,
        }
    },
    "bypass": {"controls": {"amount": 0.0}, "output_source": 0},
    "limits": {"max_spatial_tokens": 4096},
}


# AUTHOR EDIT: replace this small PyTorch function with your operator.
# The host has already checked input count, profile, controls, F16 slot shape,
# matching device/geometry, finiteness, bypass, and the declared token bound.
@torch.inference_mode()
def process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: Mapping[str, object],
    context: OperatorContext,
) -> ToolkitOperatorResult:
    """Blend one carrier slot with one donor slot on the complete latent grid."""

    carrier, donor = sources
    amount = float(controls["amount"])

    # --- AUTHOR ALGORITHM START ---
    # Replace these two lines with roughly 30-50 lines of bounded PyTorch.
    mixed = torch.lerp(carrier.float(), donor.float(), amount)
    output = mixed.to(torch.float16).contiguous()
    # --- AUTHOR ALGORITHM END ---

    return ToolkitOperatorResult(
        output=output,
        provenance={
            "operation": {
                "operator_id": OPERATOR_ID,
                "operator_version": OPERATOR_VERSION,
                "controls": dict(controls),
                "seed": context.seed,
            },
            "processing_mode": context.processing_mode,
            "slot_index": context.slot_index,
            "full_grid": True,
        },
    )


def _installed_operator() -> InstalledOperator:
    """Explicitly install the already imported callable into a private registry."""

    registry = TrustedOperatorRegistry()
    registry.install(DESCRIPTOR, process_sources, exported_entrypoint=ENTRYPOINT)
    return registry.load(OPERATOR_ID, OPERATOR_VERSION)


def _build_hook(donor: object, amount: float, seed: int) -> LatentDeckResearchOperatorHook:
    return build_installed_operator_research_hook(
        _installed_operator(),
        captured_sources=(donor,),
        controls={"amount": amount},
        seed=seed,
        name=f"external/{OPERATOR_ID}",
    )


class MyLatentOperator:
    """Normal Comfy node: carrier + donor -> manipulated latent."""

    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operator_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Community Operators"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor": ("LATENT",),
                "amount": ("FLOAT", {"default": 0.5, "min": 0.0, "max": 1.0, "step": 0.01}),
                "seed": ("INT", {"default": 0, "min": 0, "max": 9_007_199_254_740_991}),
            }
        }

    def process(
        self, carrier: object, donor: object, amount: float, seed: int
    ) -> tuple[object, str]:
        hook = _build_hook(donor, amount, seed)
        output = hook.full(carrier)
        return output, json.dumps(
            hook.descriptor,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )


class MyLatentOperatorTestHook:
    """Connect the same operator to Toolkit benchmark/determinism/streaming nodes."""

    RETURN_TYPES = ("LATENTDECK_OPERATOR_HOOK",)
    RETURN_NAMES = ("operator_hook",)
    FUNCTION = "build"
    CATEGORY = "LatentDeck/Community Operators"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "donor": ("LATENT",),
                "amount": ("FLOAT", {"default": 0.5, "min": 0.0, "max": 1.0, "step": 0.01}),
                "seed": ("INT", {"default": 0, "min": 0, "max": 9_007_199_254_740_991}),
            }
        }

    def build(
        self, donor: object, amount: float, seed: int
    ) -> tuple[LatentDeckResearchOperatorHook]:
        return (_build_hook(donor, amount, seed),)


# AUTHOR EDIT: use unique mapping keys/display names for each copied operator.
NODE_CLASS_MAPPINGS = {
    "MyLatentOperator": MyLatentOperator,
    "MyLatentOperatorTestHook": MyLatentOperatorTestHook,
}
NODE_DISPLAY_NAME_MAPPINGS = {
    "MyLatentOperator": "LatentDeck Example - MyLatentOperator",
    "MyLatentOperatorTestHook": "LatentDeck Example - MyLatentOperator Test Hook",
}

__all__ = ["NODE_CLASS_MAPPINGS", "NODE_DISPLAY_NAME_MAPPINGS"]
