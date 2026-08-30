"""Comfy declarations for explicit Toolkit cartridge I/O and alignment."""

from __future__ import annotations

import json
import os
from collections.abc import Callable
from pathlib import Path

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


_NO_LC_INPUT = "Select or upload an .lc file"
_NO_RAW_INPUT = "Select or upload an H3 .safetensors file"
_MAX_INPUT_CHOICES = 4_096
_MAX_INPUT_SCAN_ENTRIES = 32_768


def _comfy_input_choices(suffix: str, empty_label: str) -> list[str]:
    """List bounded data files from Comfy's explicit input directory only."""

    try:
        import folder_paths  # type: ignore[import-not-found]
    except ImportError:
        return [empty_label]

    input_root = Path(folder_paths.get_input_directory())
    if not input_root.is_dir():
        return [empty_label]

    choices: list[str] = []
    scanned_entries = 0
    for directory, child_directories, file_names in os.walk(input_root, followlinks=False):
        child_directories[:] = [
            name for name in child_directories if not (Path(directory) / name).is_symlink()
        ]
        scanned_entries += len(child_directories)
        if scanned_entries > _MAX_INPUT_SCAN_ENTRIES:
            return sorted(choices) or [empty_label]
        for file_name in file_names:
            scanned_entries += 1
            if scanned_entries > _MAX_INPUT_SCAN_ENTRIES:
                return sorted(choices) or [empty_label]
            candidate = Path(directory) / file_name
            if candidate.is_symlink() or candidate.suffix.lower() != suffix:
                continue
            choices.append(candidate.relative_to(input_root).as_posix())
            if len(choices) >= _MAX_INPUT_CHOICES:
                return sorted(choices)
    return sorted(choices) or [empty_label]


def _resolve_comfy_input_file(selection: str, suffix: str, empty_label: str) -> str:
    """Resolve an uploaded/selected file without accepting arbitrary host paths."""

    if not isinstance(selection, str) or not selection.strip() or selection == empty_label:
        raise ToolkitIOError("input.file_required", empty_label)
    if Path(selection).suffix.lower() != suffix:
        raise ToolkitIOError(
            "input.extension_invalid", f"selected Comfy input must use the {suffix} extension"
        )

    try:
        import folder_paths  # type: ignore[import-not-found]
    except ImportError as error:
        raise ToolkitIOError(
            "input.comfy_unavailable", "safe file selection requires a running ComfyUI host"
        ) from error

    try:
        input_root = Path(folder_paths.get_input_directory()).resolve(strict=True)
        annotated = Path(folder_paths.get_annotated_filepath(selection))
        if annotated.is_symlink():
            raise ToolkitIOError(
                "input.symlink_forbidden", "selected Comfy input cannot be a symlink"
            )
        resolved = annotated.resolve(strict=True)
        resolved.relative_to(input_root)
    except ToolkitIOError:
        raise
    except (FileNotFoundError, OSError, ValueError) as error:
        raise ToolkitIOError(
            "input.path_invalid", "selected file must exist inside ComfyUI's input directory"
        ) from error
    if not resolved.is_file():
        raise ToolkitIOError("input.file_invalid", "selected Comfy input is not a regular file")
    return str(resolved)


def _resolve_comfy_output_path(relative_path: str, *, subdirectory: str, suffix: str) -> str:
    """Resolve a visible relative output below Comfy's dedicated LatentDeck folder."""

    if not isinstance(relative_path, str) or not relative_path.strip():
        raise ToolkitIOError("output.path_required", "output path cannot be empty")
    requested = Path(relative_path)
    if requested.is_absolute() or requested.drive or ".." in requested.parts:
        raise ToolkitIOError(
            "output.path_invalid", "output path must be relative to the LatentDeck output folder"
        )
    if requested.suffix.lower() != suffix:
        raise ToolkitIOError("output.extension_invalid", f"output path must use {suffix}")
    try:
        import folder_paths  # type: ignore[import-not-found]
    except ImportError as error:
        raise ToolkitIOError(
            "output.comfy_unavailable", "safe output selection requires a running ComfyUI host"
        ) from error

    root = (Path(folder_paths.get_output_directory()) / "latentdeck" / subdirectory).resolve()
    target = (root / requested).resolve()
    try:
        target.relative_to(root)
    except ValueError as error:
        raise ToolkitIOError(
            "output.path_invalid", "output path escaped the LatentDeck output folder"
        ) from error
    return str(target)


class LatentDeckToolkitLCLoadInspect:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "inspection_json")
    FUNCTION = "load"
    CATEGORY = "LatentDeck/Toolkit/Cartridge"

    def __init__(
        self,
        loader: Callable[[str], LoadedH3Latent] | None = None,
        path_resolver: Callable[[str], str] | None = None,
    ) -> None:
        self._loader = loader or load_lc
        self._path_resolver = path_resolver or (
            lambda selection: _resolve_comfy_input_file(selection, ".lc", _NO_LC_INPUT)
        )

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "lc_file": (
                    _comfy_input_choices(".lc", _NO_LC_INPUT),
                    {"tooltip": "Select from Comfy input or use Upload .lc below."},
                ),
            }
        }

    def load(self, lc_file: str) -> tuple[dict[str, object], str]:
        loaded = self._loader(self._path_resolver(lc_file))
        return loaded.latent, _json(loaded.report)


class LatentDeckToolkitRawH3Import:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "inspection_json")
    FUNCTION = "load"
    CATEGORY = "LatentDeck/Toolkit/Cartridge"

    def __init__(
        self,
        loader: Callable[[str], LoadedH3Latent] | None = None,
        path_resolver: Callable[[str], str] | None = None,
    ) -> None:
        self._loader = loader or import_raw_h3
        self._path_resolver = path_resolver or (
            lambda selection: _resolve_comfy_input_file(
                selection, ".safetensors", _NO_RAW_INPUT
            )
        )

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "safetensors_file": (
                    _comfy_input_choices(".safetensors", _NO_RAW_INPUT),
                    {"tooltip": "Select from Comfy input or use Upload .safetensors below."},
                ),
            }
        }

    def load(self, safetensors_file: str) -> tuple[dict[str, object], str]:
        loaded = self._loader(self._path_resolver(safetensors_file))
        return loaded.latent, _json(loaded.report)


class LatentDeckToolkitLCSaveResample:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "receipt_json")
    FUNCTION = "save"
    OUTPUT_NODE = True
    CATEGORY = "LatentDeck/Toolkit/Cartridge"

    def __init__(
        self,
        saver: Callable[..., SavedCartridge] | None = None,
        output_resolver: Callable[[str], str] | None = None,
    ) -> None:
        self._saver = saver or save_resampled_lc
        self._output_resolver = output_resolver or (
            lambda path: _resolve_comfy_output_path(
                path, subdirectory="cartridges", suffix=".lc"
            )
        )

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
        resolved_output = self._output_resolver(output_path)
        saved = self._saver(
            latent,
            resolved_output,
            overwrite=overwrite,
        )
        saved_path = saved.output_path.resolve()
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
            file_name=saved_path.name,
        )
        report = {
            "output_name": saved_path.name,
            "output_path": str(saved_path),
            "receipt": saved.receipt,
            "cartridge_id": cartridge_id,
            "genealogy": {
                "parents": list(derived.parent_cartridges),
                "operations": list(derived.operation_history),
                "audio": derived.audio_disposition,
            },
        }
        return {
            "ui": {"text": [f"Saved {saved_path}"]},
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
        temporal_slots = {"default": 2, "min": 2, "max": 512, "step": 5}
        offset = {"default": 0, "min": 0, "max": 1_048_575}
        return {
            "required": {
                "latent": ("LATENT",),
                "temporal_start": ("INT", offset),
                "temporal_slots": ("INT", temporal_slots),
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
