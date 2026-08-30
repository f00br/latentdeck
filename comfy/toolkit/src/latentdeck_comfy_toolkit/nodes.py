"""ComfyUI declarations for the public LatentDeck research Toolkit."""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import ClassVar

import torch

from .adapter import process_xs_sequence
from .decoder_compare import DecoderHook, ToolkitContractError, compare_decoder_hooks
from .projector import preflight_projector_input, project_offline

_MAX_SAFE_SEED = 9_007_199_254_740_991


def _latent(value: object, label: str) -> tuple[dict[str, object], torch.Tensor]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise ToolkitContractError("node.latent_invalid", f"{label} must be a LATENT object")
    samples = value.get("samples")
    if not isinstance(samples, torch.Tensor):
        raise ToolkitContractError("node.latent_invalid", f"{label} must contain tensor samples")
    return dict(value), samples


def _provenance(value: dict[str, object]) -> str:
    return json.dumps(value, allow_nan=False, separators=(",", ":"), sort_keys=True)


def _image(value: torch.Tensor, label: str) -> torch.Tensor:
    if value.ndim != 4 or value.shape[-1] not in {1, 3, 4}:
        raise ToolkitContractError(
            "node.decoder_output_invalid",
            f"{label} hook must return Comfy IMAGE layout [N,H,W,C]",
        )
    return value


class _XsNode:
    ALGORITHM: ClassVar[str]
    EXTRA_INPUTS: ClassVar[dict[str, object]] = {}
    CONTROL_KEYS: ClassVar[tuple[str, ...]] = ()
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "provenance_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/XS"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        required: dict[str, object] = {
            "latent_a": ("LATENT",),
            "latent_b": ("LATENT",),
            "mix": ("FLOAT", {"default": 0.5, "minimum": 0.0, "maximum": 1.0, "step": 0.01}),
            "mode": (["HYBRIDIZE", "INTERACT"],),
            "routing": (["A", "B"],),
            "interaction": (
                "FLOAT",
                {"default": 0.7, "minimum": 0.0, "maximum": 1.0, "step": 0.01},
            ),
            "preserve": (
                "FLOAT",
                {"default": 0.55, "minimum": 0.0, "maximum": 1.0, "step": 0.01},
            ),
            "chaos": (
                "FLOAT",
                {"default": 0.0, "minimum": 0.0, "maximum": 1.0, "step": 0.01},
            ),
            "seed": (
                "INT",
                {"default": 0, "minimum": 0, "maximum": _MAX_SAFE_SEED},
            ),
        }
        required.update(cls.EXTRA_INPUTS)
        return {"required": required}

    def process(
        self,
        latent_a: object,
        latent_b: object,
        mix: float,
        mode: str,
        routing: str,
        interaction: float,
        preserve: float,
        chaos: float,
        seed: int,
        **advanced: object,
    ) -> tuple[dict[str, object], str]:
        mapping_a, samples_a = _latent(latent_a, "latent_a")
        mapping_b, samples_b = _latent(latent_b, "latent_b")
        unknown = sorted(set(advanced) - set(self.CONTROL_KEYS))
        if unknown:
            raise ToolkitContractError(
                "node.control_invalid", f"unknown advanced controls: {', '.join(unknown)}"
            )
        controls: dict[str, object] = {
            "mix": mix,
            "mode": mode,
            "routing": routing,
            "interaction": interaction,
            "preserve": preserve,
            "chaos": chaos,
            **advanced,
        }
        result = process_xs_sequence(
            samples_a,
            samples_b,
            algorithm=self.ALGORITHM,
            controls=controls,
            seed=seed,
        )
        output = mapping_a if routing == "A" else mapping_b
        output["samples"] = result.output
        return output, _provenance(result.provenance)


class LatentDeckToolkitXS1(_XsNode):
    ALGORITHM = "XS1"
    CONTROL_KEYS = ("xs1_channel_a", "xs1_channel_b", "xs1_angle_degrees")
    EXTRA_INPUTS = {
        "xs1_channel_a": ("INT", {"default": 0, "minimum": 0, "maximum": 23}),
        "xs1_channel_b": ("INT", {"default": 1, "minimum": 0, "maximum": 23}),
        "xs1_angle_degrees": (
            "FLOAT",
            {"default": 30.0, "minimum": -180.0, "maximum": 180.0, "step": 1.0},
        ),
    }


class LatentDeckToolkitXS2(_XsNode):
    ALGORITHM = "XS2"
    CONTROL_KEYS = ("xs2_radius",)
    EXTRA_INPUTS = {
        "xs2_radius": ("INT", {"default": 1, "minimum": 1, "maximum": 8}),
    }


class LatentDeckToolkitXS3(_XsNode):
    ALGORITHM = "XS3"
    CONTROL_KEYS = ("xs3_high_gain",)
    EXTRA_INPUTS = {
        "xs3_high_gain": (
            "FLOAT",
            {"default": 0.5, "minimum": -2.0, "maximum": 2.0, "step": 0.01},
        ),
    }


class LatentDeckToolkitXS4(_XsNode):
    ALGORITHM = "XS4"
    CONTROL_KEYS = ("xs4_epsilon",)
    EXTRA_INPUTS = {
        "xs4_epsilon": (
            "FLOAT",
            {"default": 1e-6, "minimum": 1e-8, "maximum": 1e-3, "step": 1e-6},
        ),
    }


class LatentDeckToolkitXS5(_XsNode):
    ALGORITHM = "XS5"
    CONTROL_KEYS = ("xs5_routing", "temperature", "top_k", "sinkhorn_iterations")
    EXTRA_INPUTS = {
        "xs5_routing": (["TOPK", "SINKHORN"],),
        "temperature": (
            "FLOAT",
            {"default": 0.12, "minimum": 0.02, "maximum": 1.0, "step": 0.01},
        ),
        "top_k": ("INT", {"default": 8, "minimum": 1, "maximum": 64}),
        "sinkhorn_iterations": ("INT", {"default": 5, "minimum": 2, "maximum": 12}),
    }


class LatentDeckToolkitCompareDecoders:
    RETURN_TYPES = ("IMAGE", "IMAGE", "STRING")
    RETURN_NAMES = ("fast_image", "hq_image", "comparison_json")
    FUNCTION = "compare"
    CATEGORY = "LatentDeck/Toolkit/Decode"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "fast_decoder": ("LATENTDECK_DECODER_HOOK",),
                "hq_decoder": ("LATENTDECK_DECODER_HOOK",),
            }
        }

    def compare(
        self,
        latent: object,
        fast_decoder: DecoderHook,
        hq_decoder: DecoderHook,
    ) -> tuple[torch.Tensor, torch.Tensor, str]:
        _, samples = _latent(latent, "latent")
        comparison = compare_decoder_hooks(samples, fast_decoder, hq_decoder)
        return (
            _image(comparison.fast_output, "FAST"),
            _image(comparison.hq_output, "HQ"),
            _provenance(comparison.provenance),
        )


class LatentDeckToolkitOfflineProjector:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "provenance_json")
    FUNCTION = "project"
    CATEGORY = "LatentDeck/Toolkit/Offline"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "components": ("INT", {"default": 8, "minimum": 1, "maximum": 24}),
            }
        }

    def project(self, latent: object, components: int) -> tuple[dict[str, object], str]:
        output, samples = _latent(latent, "latent")
        preflight_projector_input(samples, components=components)
        cpu_samples = samples.detach().to(device="cpu").contiguous()
        result = project_offline(cpu_samples, components=components)
        output["samples"] = result.output
        provenance = dict(result.provenance)
        provenance["comfy_staging"] = {
            "source_device": samples.device.type,
            "output_device": "cpu",
            "explicit_node": "LatentDeckToolkitOfflineProjector",
        }
        return output, _provenance(provenance)


NODE_CLASS_MAPPINGS = {
    "LatentDeckToolkitXS1": LatentDeckToolkitXS1,
    "LatentDeckToolkitXS2": LatentDeckToolkitXS2,
    "LatentDeckToolkitXS3": LatentDeckToolkitXS3,
    "LatentDeckToolkitXS4": LatentDeckToolkitXS4,
    "LatentDeckToolkitXS5": LatentDeckToolkitXS5,
    "LatentDeckToolkitCompareDecoders": LatentDeckToolkitCompareDecoders,
    "LatentDeckToolkitOfflineProjector": LatentDeckToolkitOfflineProjector,
}

NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckToolkitXS1": "LatentDeck XS1 — Channel Rotation",
    "LatentDeckToolkitXS2": "LatentDeck XS2 — Grid Exchange",
    "LatentDeckToolkitXS3": "LatentDeck XS3 — Temporal Interaction",
    "LatentDeckToolkitXS4": "LatentDeck XS4 — Statistics Transfer",
    "LatentDeckToolkitXS5": "LatentDeck XS5 — Affinity Transport",
    "LatentDeckToolkitCompareDecoders": "LatentDeck Compare FAST / HQ Hooks",
    "LatentDeckToolkitOfflineProjector": "LatentDeck Projector (Offline CPU)",
}


__all__ = [
    "NODE_CLASS_MAPPINGS",
    "NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckToolkitCompareDecoders",
    "LatentDeckToolkitOfflineProjector",
    "LatentDeckToolkitXS1",
    "LatentDeckToolkitXS2",
    "LatentDeckToolkitXS3",
    "LatentDeckToolkitXS4",
    "LatentDeckToolkitXS5",
]
