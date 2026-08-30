"""Comfy node for explicit CPU/CUDA latent staging."""

from __future__ import annotations

import json
from collections.abc import Mapping

from .device_transfer import MAX_CUDA_DEVICE_INDEX, transfer_latent_device
from .workflow_metadata import annotate_operation


class LatentDeckToolkitExplicitDeviceTransfer:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "device_receipt_json")
    FUNCTION = "transfer"
    CATEGORY = "LatentDeck/Toolkit/Utilities"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "target": (["CPU", "CUDA"],),
                "cuda_index": (
                    "INT",
                    {"default": 0, "min": 0, "max": MAX_CUDA_DEVICE_INDEX},
                ),
                "cuda_unavailable_policy": (["ERROR", "FALLBACK_TO_CPU"],),
            }
        }

    def transfer(
        self,
        latent: object,
        target: str,
        cuda_index: int,
        cuda_unavailable_policy: str,
    ) -> tuple[object, str]:
        result = transfer_latent_device(
            latent,
            target=target,
            cuda_index=cuda_index,
            cuda_unavailable_policy=cuda_unavailable_policy,
        )
        output = result.output
        if isinstance(output, Mapping):
            output = annotate_operation(
                output,
                sources=(("source", latent),),
                structural_role="source",
                provenance=result.provenance,
            )
        receipt = json.dumps(
            result.provenance,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )
        return output, receipt


DEVICE_NODE_CLASS_MAPPINGS = {
    "LatentDeckToolkitExplicitDeviceTransfer": LatentDeckToolkitExplicitDeviceTransfer,
}

DEVICE_NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckToolkitExplicitDeviceTransfer": "LatentDeck Explicit Device Transfer — CPU / CUDA",
}


__all__ = [
    "DEVICE_NODE_CLASS_MAPPINGS",
    "DEVICE_NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckToolkitExplicitDeviceTransfer",
]
