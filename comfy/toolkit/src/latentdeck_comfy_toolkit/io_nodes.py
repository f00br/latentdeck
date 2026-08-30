"""Comfy declarations for explicit Toolkit cartridge I/O and alignment."""

from __future__ import annotations

import json
from collections.abc import Callable

from .alignment import AlignmentResult, PairAlignmentResult, align_h3_pair, crop_h3_latent
from .cartridge_io import (
    LoadedH3Latent,
    SavedCartridge,
    ToolkitIOError,
    import_raw_h3,
    load_lc,
    save_resampled_lc,
)
from .compatibility import check_h3_compatibility
from .workflow_metadata import derive_resample_inputs, record_saved_output


def _json(value: object) -> str:
    return json.dumps(value, allow_nan=False, separators=(",", ":"), sort_keys=True)


class LatentDeckToolkitLCLoadInspect:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "inspection_json")
    FUNCTION = "load"
    CATEGORY = "LatentDeck/Toolkit/Cartridge"

    def __init__(
        self, loader: Callable[[str], LoadedH3Latent] | None = None
    ) -> None:
        self._loader = loader or load_lc

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "lc_path": ("STRING", {"default": "", "multiline": False}),
            }
        }

    def load(self, lc_path: str) -> tuple[dict[str, object], str]:
        loaded = self._loader(lc_path)
        return loaded.latent, _json(loaded.report)


class LatentDeckToolkitRawH3Import:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "inspection_json")
    FUNCTION = "load"
    CATEGORY = "LatentDeck/Toolkit/Cartridge"

    def __init__(
        self, loader: Callable[[str], LoadedH3Latent] | None = None
    ) -> None:
        self._loader = loader or import_raw_h3

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "safetensors_path": ("STRING", {"default": "", "multiline": False}),
            }
        }

    def load(self, safetensors_path: str) -> tuple[dict[str, object], str]:
        loaded = self._loader(safetensors_path)
        return loaded.latent, _json(loaded.report)


class LatentDeckToolkitLCSaveResample:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "receipt_json")
    FUNCTION = "save"
    OUTPUT_NODE = True
    CATEGORY = "LatentDeck/Toolkit/Cartridge"

    def __init__(self, saver: Callable[..., SavedCartridge] | None = None) -> None:
        self._saver = saver or save_resampled_lc

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "output_path": ("STRING", {"default": "resampled.lc", "multiline": False}),
                "overwrite": ("BOOLEAN", {"default": False}),
            }
        }

    def save(
        self,
        latent: object,
        output_path: str,
        overwrite: bool,
    ) -> dict[str, object]:
        derived = derive_resample_inputs(latent)
        saved = self._saver(
            latent,
            output_path,
            overwrite=overwrite,
        )
        validation = saved.receipt.get("validation")
        cartridge_id = saved.manifest.get("cartridge_id")
        archive_sha256 = (
            validation.get("archive_sha256") if isinstance(validation, dict) else None
        )
        if not isinstance(cartridge_id, str) or not isinstance(archive_sha256, str):
            raise ToolkitIOError(
                "sdk.response_invalid",
                "saved LC receipt omitted cartridge identity or archive SHA-256",
            )
        annotated = record_saved_output(
            latent,
            cartridge_id=cartridge_id,
            archive_sha256=archive_sha256,
            file_name=saved.output_path.name,
        )
        report = {
            "output_name": saved.output_path.name,
            "receipt": saved.receipt,
            "cartridge_id": cartridge_id,
            "genealogy": {
                "parents": list(derived.parent_cartridges),
                "operations": list(derived.operation_history),
                "audio": derived.audio_disposition,
            },
        }
        return {
            "ui": {"text": [f"Saved {saved.output_path.name}"]},
            "result": (annotated, _json(report)),
        }


class LatentDeckToolkitCompatibility:
    RETURN_TYPES = ("BOOLEAN", "STRING")
    RETURN_NAMES = ("compatible", "compatibility_json")
    FUNCTION = "check"
    CATEGORY = "LatentDeck/Toolkit/Diagnostics"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {"latent_a": ("LATENT",), "latent_b": ("LATENT",)},
            "optional": {"latent_c": ("LATENT",), "latent_d": ("LATENT",)},
        }

    def check(
        self,
        latent_a: object,
        latent_b: object,
        latent_c: object | None = None,
        latent_d: object | None = None,
    ) -> tuple[bool, str]:
        report = check_h3_compatibility(
            [latent for latent in (latent_a, latent_b, latent_c, latent_d) if latent is not None]
        )
        return bool(report["compatible"]), _json(report)


class LatentDeckToolkitExplicitCrop:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "crop"
    CATEGORY = "LatentDeck/Toolkit/Conversion"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        positive = {"default": 1, "min": 1, "max": 1_048_576}
        offset = {"default": 0, "min": 0, "max": 1_048_575}
        return {
            "required": {
                "latent": ("LATENT",),
                "temporal_start": ("INT", offset),
                "temporal_slots": ("INT", positive),
                "spatial_top": ("INT", offset),
                "spatial_left": ("INT", offset),
                "spatial_height": ("INT", positive),
                "spatial_width": ("INT", positive),
                "audio_policy": (["PRESERVE_EXACT", "DROP_EXPLICIT"],),
            }
        }

    def crop(self, latent: object, **controls: object) -> tuple[dict[str, object], str]:
        result: AlignmentResult = crop_h3_latent(latent, **controls)
        return result.latent, _json(result.report)


class LatentDeckToolkitExplicitAlign:
    RETURN_TYPES = ("LATENT", "LATENT", "STRING")
    RETURN_NAMES = ("latent_a", "latent_b", "alignment_json")
    FUNCTION = "align"
    CATEGORY = "LatentDeck/Toolkit/Conversion"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent_a": ("LATENT",),
                "latent_b": ("LATENT",),
                "temporal_policy": (
                    ["ERROR", "CROP_END_TO_SHORTEST", "CROP_START_TO_SHORTEST"],
                ),
                "spatial_policy": (
                    ["ERROR", "CENTER_TO_SMALLEST", "TOP_LEFT_TO_SMALLEST"],
                ),
                "audio_policy": (["PRESERVE_EXACT", "DROP_EXPLICIT"],),
            }
        }

    def align(self, latent_a: object, latent_b: object, **policies: object) -> tuple[object, ...]:
        result: PairAlignmentResult = align_h3_pair(latent_a, latent_b, **policies)
        return result.latent_a, result.latent_b, _json(result.report)


IO_NODE_CLASS_MAPPINGS: dict[str, type] = {
    "LatentDeckToolkitLCLoadInspect": LatentDeckToolkitLCLoadInspect,
    "LatentDeckToolkitRawH3Import": LatentDeckToolkitRawH3Import,
    "LatentDeckToolkitLCSaveResample": LatentDeckToolkitLCSaveResample,
    "LatentDeckToolkitCompatibility": LatentDeckToolkitCompatibility,
    "LatentDeckToolkitExplicitCrop": LatentDeckToolkitExplicitCrop,
    "LatentDeckToolkitExplicitAlign": LatentDeckToolkitExplicitAlign,
}

IO_NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckToolkitLCLoadInspect": "LatentDeck LC Load / Inspect",
    "LatentDeckToolkitRawH3Import": "LatentDeck Raw H3 Latent Import",
    "LatentDeckToolkitLCSaveResample": "LatentDeck LC Save / Resample",
    "LatentDeckToolkitCompatibility": "LatentDeck Compatibility Checker",
    "LatentDeckToolkitExplicitCrop": "LatentDeck Explicit H3 Crop",
    "LatentDeckToolkitExplicitAlign": "LatentDeck Explicit H3 Pair Align",
}


__all__ = [
    "IO_NODE_CLASS_MAPPINGS",
    "IO_NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckToolkitCompatibility",
    "LatentDeckToolkitExplicitAlign",
    "LatentDeckToolkitExplicitCrop",
    "LatentDeckToolkitLCLoadInspect",
    "LatentDeckToolkitLCSaveResample",
    "LatentDeckToolkitRawH3Import",
]
