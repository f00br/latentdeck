from __future__ import annotations

import json
from pathlib import Path

import torch

from latentdeck_comfy_toolkit.cartridge_io import LoadedH3Latent, SavedCartridge
from latentdeck_comfy_toolkit.io_nodes import (
    IO_NODE_CLASS_MAPPINGS,
    IO_NODE_DISPLAY_NAME_MAPPINGS,
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
        loader=lambda _path: expected
    )
    latent, report_json = node.load("X:/explicit/source.lc")

    assert latent is expected.latent
    assert json.loads(report_json) == expected.report
    assert node.CATEGORY == "LatentDeck/Toolkit/Cartridge"


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

    node = IO_NODE_CLASS_MAPPINGS["LatentDeckToolkitLCSaveResample"](saver=save)
    response = node.save(operated, "study.lc", False)
    output, report_json = response["result"]

    assert calls == [
        {"latent": operated, "output_path": "study.lc", "overwrite": False}
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
