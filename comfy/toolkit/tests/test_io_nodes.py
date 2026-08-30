from __future__ import annotations

import json
import sys
import types
from pathlib import Path

import pytest
import torch

from latentdeck_comfy_toolkit.cartridge_io import LoadedH3Latent, SavedCartridge
from latentdeck_comfy_toolkit.io_nodes import (
    IO_NODE_CLASS_MAPPINGS,
    IO_NODE_DISPLAY_NAME_MAPPINGS,
    _comfy_input_choices,
    _resolve_comfy_input_file,
)
from latentdeck_comfy_toolkit.workflow_metadata import annotate_operation, initialize_lc_metadata


def test_io_node_module_exposes_the_complete_explicit_toolkit_surface() -> None:
    assert IO_NODE_DISPLAY_NAME_MAPPINGS == {
        "LatentDeckToolkitLCLoadInspect": "LatentDeck LC Load / Inspect",
        "LatentDeckToolkitRawH3Import": "LatentDeck Raw H3 Latent Import",
        "LatentDeckToolkitLCSaveResample": "LatentDeck LC Save / Resample",
        "LatentDeckToolkitCompatibility": "LatentDeck Compatibility Checker",
        "LatentDeckToolkitExplicitCrop": "LatentDeck Explicit H3 Crop",
        "LatentDeckToolkitExplicitAlign": "LatentDeck Explicit H3 Pair Align",
    }
    assert set(IO_NODE_CLASS_MAPPINGS) == set(IO_NODE_DISPLAY_NAME_MAPPINGS)

    expected = LoadedH3Latent(
        latent={"samples": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)},
        report={"validation": {"validation_level": "full"}},
    )
    node = IO_NODE_CLASS_MAPPINGS["LatentDeckToolkitLCLoadInspect"](
        loader=lambda _path: expected,
        path_resolver=lambda path: path,
    )
    latent, report_json = node.load("X:/explicit/source.lc")

    assert latent is expected.latent
    assert json.loads(report_json) == expected.report
    assert node.CATEGORY == "LatentDeck/Toolkit/Cartridge"
    assert list(node.INPUT_TYPES()["required"]) == ["lc_file"]

    crop_slots = IO_NODE_CLASS_MAPPINGS["LatentDeckToolkitExplicitCrop"].INPUT_TYPES()[
        "required"
    ]["temporal_slots"]
    assert crop_slots == ("INT", {"default": 2, "min": 2, "max": 512, "step": 5})


def test_save_node_derives_genealogy_and_returns_output_receipt_in_the_ledger(
    tmp_path: Path,
) -> None:
    source = initialize_lc_metadata(
        {"samples": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)},
        manifest={"cartridge_id": "source", "codec": {}, "timing": {}},
        validation={"archive_sha256": "a" * 64},
    )
    operated = annotate_operation(
        source,
        sources=(("carrier", source),),
        structural_role="carrier",
        provenance={
            "operation": {
                "operator_id": "org.example.operator",
                "operator_version": "0.1.0",
                "seed": 0,
                "controls": {},
            }
        },
    )
    calls: list[dict[str, object]] = []

    def save(latent: object, output_path: str, **keywords: object) -> SavedCartridge:
        calls.append({"latent": latent, "output_path": output_path, **keywords})
        return SavedCartridge(
            output_path=tmp_path / "study.lc",
            receipt={
                "status": "ok",
                "validation": {"archive_sha256": "b" * 64},
            },
            manifest={"cartridge_id": "output"},
        )

    node = IO_NODE_CLASS_MAPPINGS["LatentDeckToolkitLCSaveResample"](
        saver=save,
        output_resolver=lambda path: str(tmp_path / path),
    )
    response = node.save(operated, "study.lc", False)
    output, report_json = response["result"]

    assert calls == [
        {
            "latent": operated,
            "output_path": str(tmp_path / "study.lc"),
            "overwrite": False,
        }
    ]
    assert output["latentdeck"]["outputs"] == [
        {
            "archive_sha256": "b" * 64,
            "cartridge_id": "output",
            "file_name": "study.lc",
        }
    ]
    assert json.loads(report_json)["genealogy"]["operations"][0]["operator_id"] == (
        "org.example.operator"
    )
    assert set(node.INPUT_TYPES()["required"]) == {"latent", "output_path", "overwrite"}
    assert json.loads(report_json)["output_path"] == str((tmp_path / "study.lc").resolve())
    assert response["ui"]["text"] == [f"Saved {(tmp_path / 'study.lc').resolve()}"]


def test_loaders_offer_only_matching_comfy_inputs_and_resolve_them_inside_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    input_root = tmp_path / "input"
    nested = input_root / "latentdeck" / "raw"
    nested.mkdir(parents=True)
    selected = nested / "sample.safetensors"
    selected.write_bytes(b"synthetic")
    (nested / "ignore.txt").write_text("not a latent", encoding="utf-8")

    folder_paths = types.ModuleType("folder_paths")
    folder_paths.get_input_directory = lambda: str(input_root)  # type: ignore[attr-defined]
    folder_paths.get_annotated_filepath = (  # type: ignore[attr-defined]
        lambda name: str(input_root / Path(name.replace("/", "\\")))
    )
    monkeypatch.setitem(sys.modules, "folder_paths", folder_paths)

    declaration = IO_NODE_CLASS_MAPPINGS["LatentDeckToolkitRawH3Import"].INPUT_TYPES()
    assert declaration["required"]["safetensors_file"][0] == [
        "latentdeck/raw/sample.safetensors"
    ]
    assert _resolve_comfy_input_file(
        "latentdeck/raw/sample.safetensors",
        ".safetensors",
        "Select a file",
    ) == str(selected.resolve())


def test_safe_comfy_input_rejects_host_paths_wrong_extensions_and_escape(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    input_root = tmp_path / "input"
    input_root.mkdir()
    outside = tmp_path / "outside.lc"
    outside.write_bytes(b"not selected")

    folder_paths = types.ModuleType("folder_paths")
    folder_paths.get_input_directory = lambda: str(input_root)  # type: ignore[attr-defined]
    folder_paths.get_annotated_filepath = (  # type: ignore[attr-defined]
        lambda name: str(input_root / Path(name))
    )
    monkeypatch.setitem(sys.modules, "folder_paths", folder_paths)

    with pytest.raises(Exception, match="input.extension_invalid"):
        _resolve_comfy_input_file("bad.txt", ".lc", "Select a file")
    with pytest.raises(Exception, match="input.path_invalid"):
        _resolve_comfy_input_file("../outside.lc", ".lc", "Select a file")
    with pytest.raises(Exception, match="input.path_invalid"):
        _resolve_comfy_input_file(str(outside), ".lc", "Select a file")


def test_comfy_input_choice_scan_is_bounded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    input_root = tmp_path / "input"
    input_root.mkdir()
    folder_paths = types.ModuleType("folder_paths")
    folder_paths.get_input_directory = lambda: str(input_root)  # type: ignore[attr-defined]
    monkeypatch.setitem(sys.modules, "folder_paths", folder_paths)
    monkeypatch.setattr(
        "latentdeck_comfy_toolkit.io_nodes.os.walk",
        lambda *_args, **_kwargs: [
            (str(input_root), [], ["ignored.txt"] * 32_769),
        ],
    )

    assert _comfy_input_choices(".lc", "Select a file") == ["Select a file"]
