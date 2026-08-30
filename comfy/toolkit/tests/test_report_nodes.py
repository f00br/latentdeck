from __future__ import annotations

import json

import torch

from latentdeck_comfy_toolkit.report_nodes import LatentDeckToolkitResearchReport
from latentdeck_comfy_toolkit.workflow_metadata import (
    annotate_evaluation,
    annotate_operation,
    initialize_lc_metadata,
    record_saved_output,
)


def test_research_report_node_exports_both_formats_and_returns_a_safe_receipt(tmp_path) -> None:
    node = LatentDeckToolkitResearchReport(directory_resolver=lambda path: path)
    source = initialize_lc_metadata(
        {"samples": torch.zeros((1, 24, 2, 1, 1), dtype=torch.float16)},
        manifest={"cartridge_id": "source-id", "codec": {}, "timing": {}},
        validation={"archive_sha256": "a" * 64},
    )
    operated = annotate_operation(
        source,
        sources=(("carrier", source),),
        structural_role="carrier",
        provenance={
            "operation": {
                "operator_id": "org.latentdeck.xs3",
                "operator_version": "0.1.0",
                "seed": 0,
                "controls": {},
            }
        },
    )
    measured = annotate_evaluation(
        operated,
        kind="benchmark",
        report={"execution_ms": 4.25, "vram_delta_bytes": 1024},
    )
    latent = record_saved_output(
        measured,
        cartridge_id="output-id",
        archive_sha256="b" * 64,
        file_name="study.lc",
    )

    response = node.export(
        latent=latent,
        output_directory=str(tmp_path),
        report_name="operator-study",
        overwrite=False,
    )

    report_json, report_markdown, receipt_json = response["result"]
    receipt = json.loads(receipt_json)
    assert json.loads(report_json)["operators"][0]["operator_id"] == "org.latentdeck.xs3"
    assert json.loads(report_json)["measurements"]["records"][0]["kind"] == "benchmark"
    assert "# LatentDeck Research Report" in report_markdown
    assert receipt["json_file"] == "operator-study.json"
    assert receipt["markdown_file"] == "operator-study.md"
    assert receipt["json_path"] == str((tmp_path / "operator-study.json").resolve())
    assert receipt["markdown_path"] == str((tmp_path / "operator-study.md").resolve())
    assert response["ui"]["text"] == [
        f"Saved {(tmp_path / 'operator-study.json').resolve()}",
        f"Saved {(tmp_path / 'operator-study.md').resolve()}",
    ]
    assert LatentDeckToolkitResearchReport.OUTPUT_NODE is True
