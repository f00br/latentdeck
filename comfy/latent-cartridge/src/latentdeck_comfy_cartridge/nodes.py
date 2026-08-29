"""ComfyUI node declarations for the lightweight cartridge recorder."""

from __future__ import annotations

from typing import Any

from .recorder import H3Recorder


class SaveLatentCartridge:
    """Record an existing H3 latent while preserving the workflow value."""

    RETURN_TYPES = ("LATENT",)
    RETURN_NAMES = ("latent",)
    FUNCTION = "save"
    OUTPUT_NODE = True
    CATEGORY = "LatentDeck/Cartridge"

    def __init__(self, recorder: H3Recorder | None = None) -> None:
        self._recorder = recorder or H3Recorder()

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "filename_prefix": (
                    "STRING",
                    {"default": "cartridge", "multiline": False},
                ),
            },
            "hidden": {"prompt": "PROMPT"},
        }

    def save(
        self,
        latent: object,
        filename_prefix: str,
        prompt: object = None,
    ) -> dict[str, Any]:
        recording = self._recorder.record(latent, filename_prefix, prompt=prompt)
        return {
            "ui": {"text": [f"Saved {recording.output_path.name}"]},
            "result": (latent,),
        }


NODE_CLASS_MAPPINGS = {"LatentDeckSaveLatentCartridge": SaveLatentCartridge}
NODE_DISPLAY_NAME_MAPPINGS = {"LatentDeckSaveLatentCartridge": "Save Latent Cartridge (.lc)"}
