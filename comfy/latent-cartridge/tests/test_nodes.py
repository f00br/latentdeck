from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

from latentdeck_comfy_cartridge.nodes import SaveLatentCartridge
from latentdeck_comfy_cartridge.recorder import RecordingResult


class RecorderStub:
    def __init__(self, output_path: Path) -> None:
        self.output_path = output_path
        self.calls: list[tuple[object, str, object]] = []

    def record(
        self, latent: object, filename_prefix: str, *, prompt: object = None
    ) -> RecordingResult:
        self.calls.append((latent, filename_prefix, prompt))
        return RecordingResult(
            output_path=self.output_path,
            receipt={
                "status": "ok",
                "validation": {"archive_bytes": 12, "archive_sha256": "a" * 64},
            },
        )


def test_node_contract_is_small_nontechnical_and_output_only() -> None:
    inputs = SaveLatentCartridge.INPUT_TYPES()

    assert set(inputs["required"]) == {"latent", "filename_prefix"}
    assert inputs["required"]["latent"] == ("LATENT",)
    assert inputs["required"]["filename_prefix"][1]["default"] == "cartridge"
    assert inputs["hidden"] == {"prompt": "PROMPT"}
    assert SaveLatentCartridge.RETURN_TYPES == ("LATENT",)
    assert SaveLatentCartridge.RETURN_NAMES == ("latent",)
    assert SaveLatentCartridge.OUTPUT_NODE is True
    assert SaveLatentCartridge.FUNCTION == "save"


def test_node_returns_the_exact_latent_object_and_a_ui_report(tmp_path: Path) -> None:
    output = tmp_path / "recording.lc"
    recorder = RecorderStub(output)
    node = SaveLatentCartridge(recorder=recorder)
    latent = {"samples": object(), "custom_extra": object()}
    prompt = {"synthetic": True}

    result = node.save(latent, "cartridge", prompt=prompt)

    assert result["result"][0] is latent
    assert result["ui"] == {"text": ["Saved recording.lc"]}
    assert recorder.calls == [(latent, "cartridge", prompt)]


def test_root_comfy_discovery_loader_exports_the_node_mappings() -> None:
    package_root = Path(__file__).resolve().parents[1]
    module_name = "_latentdeck_comfy_discovery_test"
    specification = importlib.util.spec_from_file_location(
        module_name,
        package_root / "__init__.py",
        submodule_search_locations=[str(package_root)],
    )
    assert specification is not None
    assert specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    sys.modules[module_name] = module
    try:
        specification.loader.exec_module(module)
        assert set(module.NODE_CLASS_MAPPINGS) == {"LatentDeckSaveLatentCartridge"}
        assert module.NODE_DISPLAY_NAME_MAPPINGS == {
            "LatentDeckSaveLatentCartridge": "Save Latent Cartridge (.lc)"
        }
    finally:
        for loaded_name in tuple(sys.modules):
            if loaded_name == module_name or loaded_name.startswith(f"{module_name}."):
                del sys.modules[loaded_name]
