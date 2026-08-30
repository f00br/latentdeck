"""ComfyUI node declaration for deterministic Toolkit research reports."""

from __future__ import annotations

import json
from collections.abc import Callable
from pathlib import Path

from .cartridge_io import ToolkitIOError
from .io_nodes import _resolve_comfy_output_path
from .research_report import export_research_report
from .workflow_metadata import derive_research_report_inputs


def _json(value: object) -> str:
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    )


class LatentDeckToolkitResearchReport:
    RETURN_TYPES = ("STRING", "STRING", "STRING")
    RETURN_NAMES = ("report_json", "report_markdown", "receipt_json")
    FUNCTION = "export"
    OUTPUT_NODE = True
    CATEGORY = "LatentDeck/Toolkit/Research"

    def __init__(self, directory_resolver: Callable[[str], str] | None = None) -> None:
        self._directory_resolver = directory_resolver or self._resolve_directory

    @staticmethod
    def _resolve_directory(relative_directory: str) -> str:
        sentinel = "report.json"
        resolved = Path(
            _resolve_comfy_output_path(
                str(Path(relative_directory) / sentinel),
                subdirectory="reports",
                suffix=".json",
            )
        )
        return str(resolved.parent)

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "output_directory": (
                    "STRING",
                    {"default": "session", "multiline": False},
                ),
                "report_name": (
                    "STRING",
                    {"default": "latentdeck-research-report", "multiline": False},
                ),
                "overwrite": ("BOOLEAN", {"default": False}),
            }
        }

    def export(self, **inputs: object) -> dict[str, object]:
        latent = inputs.pop("latent")
        output_directory = inputs.get("output_directory")
        if not isinstance(output_directory, str):
            raise ToolkitIOError(
                "output.path_invalid", "research report output_directory must be text"
            )
        inputs["output_directory"] = self._directory_resolver(output_directory)
        collected = derive_research_report_inputs(latent)
        result = export_research_report(
            **inputs,  # type: ignore[arg-type]
            versions_json=_json(collected["versions"]),
            cartridges_json=_json(collected["cartridges"]),
            sources_json=_json(collected["raw_sources"]),
            operators_json=_json(collected["operators"]),
            measurements_json=_json({"records": collected["measurements"]}),
            outputs_json=_json(collected["outputs"]),
        )
        return {
            "ui": {
                "text": [
                    f"Saved {result.json_path.resolve()}",
                    f"Saved {result.markdown_path.resolve()}",
                ]
            },
            "result": (result.json_text, result.markdown_text, _json(result.receipt)),
        }


REPORT_NODE_CLASS_MAPPINGS: dict[str, type] = {
    "LatentDeckToolkitResearchReport": LatentDeckToolkitResearchReport,
}

REPORT_NODE_DISPLAY_NAME_MAPPINGS: dict[str, str] = {
    "LatentDeckToolkitResearchReport": "LatentDeck One-click Research Report",
}


__all__ = [
    "REPORT_NODE_CLASS_MAPPINGS",
    "REPORT_NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckToolkitResearchReport",
]
