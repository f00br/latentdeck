from __future__ import annotations

import json

import pytest

from latentdeck_comfy_toolkit.research_report import (
    MAX_SECTION_JSON_BYTES,
    ResearchReportError,
    export_research_report,
)


def test_export_research_report_writes_deterministic_json_and_markdown_atomically(
    tmp_path,
) -> None:
    result = export_research_report(
        tmp_path,
        "session-001",
        versions_json='{"toolkit":"0.1.0","torch":"2.13.0"}',
        cartridges_json=(
            '[{"cartridge_id":"0190-test","payload_sha256":"'
            + "a" * 64
            + '"}]'
        ),
        operators_json=(
            '[{"operator_id":"org.latentdeck.xs5","parameters":{"mode":"HYBRIDIZE"}}]'
        ),
        measurements_json='{"execution_ms":12.5,"vram_delta_bytes":0}',
        outputs_json='[{"name":"result.lc","sha256":"' + "b" * 64 + '"}]',
    )

    json_text = (tmp_path / "session-001.json").read_text(encoding="utf-8")
    markdown_text = (tmp_path / "session-001.md").read_text(encoding="utf-8")

    assert json.loads(json_text) == result.report
    assert result.json_text == json_text
    assert result.markdown_text == markdown_text
    assert result.receipt["json_sha256"]
    assert result.receipt["markdown_sha256"]
    assert result.receipt["json_file"] == "session-001.json"
    assert result.receipt["markdown_file"] == "session-001.md"
    assert "# LatentDeck Research Report" in markdown_text
    assert "org.latentdeck.xs5" in markdown_text
    assert not list(tmp_path.glob("*.partial"))


def test_export_research_report_rejects_duplicate_json_keys_before_writing(tmp_path) -> None:
    with pytest.raises(ResearchReportError) as caught:
        export_research_report(
            tmp_path,
            "duplicate",
            versions_json='{"toolkit":"0.1.0","toolkit":"9.9.9"}',
            cartridges_json="[]",
            operators_json="[]",
            measurements_json="{}",
            outputs_json="[]",
        )

    assert caught.value.code == "report.json_duplicate_key"
    assert not list(tmp_path.iterdir())


def test_export_research_report_rejects_non_finite_json_numbers(tmp_path) -> None:
    with pytest.raises(ResearchReportError) as caught:
        export_research_report(
            tmp_path,
            "non-finite",
            versions_json='{"toolkit":"0.1.0"}',
            cartridges_json="[]",
            operators_json="[]",
            measurements_json='{"execution_ms":NaN}',
            outputs_json="[]",
        )

    assert caught.value.code == "report.json_non_finite"
    assert not list(tmp_path.iterdir())


def test_export_research_report_bounds_each_json_receipt(tmp_path) -> None:
    oversized = '{"toolkit":"' + "x" * MAX_SECTION_JSON_BYTES + '"}'

    with pytest.raises(ResearchReportError) as caught:
        export_research_report(
            tmp_path,
            "oversized",
            versions_json=oversized,
            cartridges_json="[]",
            operators_json="[]",
            measurements_json="{}",
            outputs_json="[]",
        )

    assert caught.value.code == "report.json_too_large"
    assert not list(tmp_path.iterdir())


def test_export_research_report_rejects_machine_absolute_paths_in_receipts(tmp_path) -> None:
    with pytest.raises(ResearchReportError) as caught:
        export_research_report(
            tmp_path,
            "unsafe-path",
            versions_json='{"toolkit":"0.1.0"}',
            cartridges_json="[]",
            operators_json="[]",
            measurements_json="{}",
            outputs_json='[{"path":"C:\\\\private\\\\result.lc"}]',
        )

    assert caught.value.code == "report.path_unsafe"
    assert not list(tmp_path.iterdir())


def test_export_research_report_rejects_absolute_paths_hidden_under_generic_keys(tmp_path) -> None:
    with pytest.raises(ResearchReportError) as caught:
        export_research_report(
            tmp_path,
            "hidden-path",
            versions_json='{"toolkit":"0.1.0"}',
            cartridges_json='[{"source":"D:\\\\private\\\\raw.safetensors"}]',
            operators_json="[]",
            measurements_json="{}",
            outputs_json="[]",
        )

    assert caught.value.code == "report.path_unsafe"
    assert not list(tmp_path.iterdir())


def test_export_research_report_translates_parser_recursion_into_a_bounded_error(tmp_path) -> None:
    nested = '{"nested":' + "[" * 1200 + "0" + "]" * 1200 + "}"

    with pytest.raises(ResearchReportError) as caught:
        export_research_report(
            tmp_path,
            "deep-json",
            versions_json=nested,
            cartridges_json="[]",
            operators_json="[]",
            measurements_json="{}",
            outputs_json="[]",
        )

    assert caught.value.code == "report.json_depth"
    assert not list(tmp_path.iterdir())


def test_export_research_report_requires_explicit_overwrite(tmp_path) -> None:
    common = {
        "versions_json": '{"toolkit":"0.1.0"}',
        "cartridges_json": "[]",
        "operators_json": "[]",
        "measurements_json": "{}",
        "outputs_json": "[]",
    }
    export_research_report(tmp_path, "stable", **common)
    original = (tmp_path / "stable.json").read_bytes()

    changed = {**common, "versions_json": '{"toolkit":"0.1.1"}'}
    with pytest.raises(ResearchReportError) as caught:
        export_research_report(tmp_path, "stable", **changed)

    assert caught.value.code == "report.output_exists"
    assert (tmp_path / "stable.json").read_bytes() == original

    export_research_report(tmp_path, "stable", overwrite=True, **changed)
    assert (tmp_path / "stable.json").read_bytes() != original
    assert not list(tmp_path.glob("*.partial"))
