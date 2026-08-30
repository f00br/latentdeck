"""Deterministic, bounded research-report export for the Comfy Toolkit."""

from __future__ import annotations

import hashlib
import json
import os
import re
from contextlib import suppress
from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Any

REPORT_SCHEMA_VERSION = "0.1.0"
MAX_SECTION_JSON_BYTES = 262_144
_REPORT_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_PATH_KEY = re.compile(r"(?:^path$|_path$|^file$|_file$|^filename$|_filename$)")


class ResearchReportError(ValueError):
    """Stable validation/write failure for a Toolkit research report."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


@dataclass(frozen=True, slots=True)
class ResearchReportResult:
    report: dict[str, Any]
    json_text: str
    markdown_text: str
    receipt: dict[str, Any]
    json_path: Path
    markdown_path: Path


def _parse_json(value: str, *, expected: type, label: str) -> Any:
    if not isinstance(value, str):
        raise ResearchReportError("report.json_invalid", f"{label} must be JSON text")
    if len(value.encode("utf-8")) > MAX_SECTION_JSON_BYTES:
        raise ResearchReportError("report.json_too_large", f"{label} exceeds its byte bound")

    def reject_duplicate_keys(pairs: list[tuple[str, object]]) -> dict[str, object]:
        result: dict[str, object] = {}
        for key, item in pairs:
            if key in result:
                raise ResearchReportError(
                    "report.json_duplicate_key", f"{label} contains duplicate key: {key}"
                )
            result[key] = item
        return result

    def reject_non_finite(_value: str) -> object:
        raise ResearchReportError(
            "report.json_non_finite", f"{label} contains NaN or Infinity"
        )

    try:
        parsed = json.loads(
            value,
            object_pairs_hook=reject_duplicate_keys,
            parse_constant=reject_non_finite,
        )
    except ResearchReportError:
        raise
    except (TypeError, ValueError) as error:
        raise ResearchReportError("report.json_invalid", f"{label} is not valid JSON") from error
    if not isinstance(parsed, expected):
        raise ResearchReportError(
            "report.section_type", f"{label} must be a JSON {expected.__name__}"
        )
    return parsed


def _canonical_json(value: object) -> str:
    return json.dumps(
        value,
        allow_nan=False,
        ensure_ascii=False,
        indent=2,
        sort_keys=True,
    ) + "\n"


def _validate_receipt_tree(value: object, *, key: str = "", depth: int = 0) -> None:
    if depth > 16:
        raise ResearchReportError("report.json_depth", "receipt nesting exceeds 16 levels")
    if isinstance(value, dict):
        for child_key, child in value.items():
            _validate_receipt_tree(child, key=child_key, depth=depth + 1)
        return
    if isinstance(value, list):
        for child in value:
            _validate_receipt_tree(child, key=key, depth=depth + 1)
        return
    if not isinstance(value, str):
        return
    windows_path = PureWindowsPath(value)
    posix_path = PurePosixPath(value)
    normalized_parts = value.replace("\\", "/").split("/")
    if (
        "\x00" in value
        or windows_path.drive
        or windows_path.root
        or posix_path.is_absolute()
        or value.lower().startswith("file:")
        or (_PATH_KEY.search(key.lower()) is not None and ".." in normalized_parts)
    ):
        raise ResearchReportError(
            "report.path_unsafe", f"{key} must be a safe relative report reference"
        )


def _markdown(report: dict[str, Any]) -> str:
    sections = (
        ("Versions", report["versions"]),
        ("Cartridges", report["cartridges"]),
        ("Raw sources", report["sources"]),
        ("Operators and parameters", report["operators"]),
        ("Timing, benchmark, and VRAM", report["measurements"]),
        ("Outputs", report["outputs"]),
    )
    lines = [
        "# LatentDeck Research Report",
        "",
        f"Schema version: `{REPORT_SCHEMA_VERSION}`",
        "",
    ]
    for title, value in sections:
        lines.extend((f"## {title}", "", "```json", _canonical_json(value).rstrip(), "```", ""))
    return "\n".join(lines)


def _write_new(path: Path, text: str) -> None:
    with path.open("x", encoding="utf-8", newline="\n") as handle:
        handle.write(text)
        handle.flush()
        os.fsync(handle.fileno())


def export_research_report(
    output_directory: str | Path,
    report_name: str,
    *,
    versions_json: str,
    cartridges_json: str,
    operators_json: str,
    measurements_json: str,
    outputs_json: str,
    sources_json: str = "[]",
    overwrite: bool = False,
) -> ResearchReportResult:
    """Validate receipts and export a deterministic JSON/Markdown report pair."""

    if not isinstance(report_name, str) or _REPORT_NAME.fullmatch(report_name) is None:
        raise ResearchReportError("report.name_invalid", "report_name must be a safe basename")

    report: dict[str, Any] = {
        "schema_version": REPORT_SCHEMA_VERSION,
        "kind": "latentdeck.toolkit.research_report",
        "versions": _parse_json(versions_json, expected=dict, label="versions_json"),
        "cartridges": _parse_json(cartridges_json, expected=list, label="cartridges_json"),
        "sources": _parse_json(sources_json, expected=list, label="sources_json"),
        "operators": _parse_json(operators_json, expected=list, label="operators_json"),
        "measurements": _parse_json(
            measurements_json, expected=dict, label="measurements_json"
        ),
        "outputs": _parse_json(outputs_json, expected=list, label="outputs_json"),
    }
    for section in report.values():
        _validate_receipt_tree(section)
    json_text = _canonical_json(report)
    markdown_text = _markdown(report)

    directory = Path(output_directory)
    directory.mkdir(parents=True, exist_ok=True)
    json_path = directory / f"{report_name}.json"
    markdown_path = directory / f"{report_name}.md"
    json_partial = json_path.with_suffix(json_path.suffix + ".partial")
    markdown_partial = markdown_path.with_suffix(markdown_path.suffix + ".partial")

    if not overwrite and (json_path.exists() or markdown_path.exists()):
        raise ResearchReportError("report.output_exists", "report output already exists")

    created: list[Path] = []
    try:
        _write_new(json_partial, json_text)
        created.append(json_partial)
        _write_new(markdown_partial, markdown_text)
        created.append(markdown_partial)
        replace = os.replace if overwrite else os.rename
        replace(json_partial, json_path)
        created.remove(json_partial)
        replace(markdown_partial, markdown_path)
        created.remove(markdown_partial)
    except OSError as error:
        raise ResearchReportError(
            "report.write_failed", "research report could not be written"
        ) from error
    finally:
        for partial in created:
            with suppress(FileNotFoundError):
                partial.unlink()

    receipt: dict[str, Any] = {
        "schema_version": REPORT_SCHEMA_VERSION,
        "status": "ok",
        "json_file": json_path.name,
        "markdown_file": markdown_path.name,
        "json_sha256": hashlib.sha256(json_text.encode("utf-8")).hexdigest(),
        "markdown_sha256": hashlib.sha256(markdown_text.encode("utf-8")).hexdigest(),
    }
    return ResearchReportResult(
        report=report,
        json_text=json_text,
        markdown_text=markdown_text,
        receipt=receipt,
        json_path=json_path,
        markdown_path=markdown_path,
    )


__all__ = [
    "MAX_SECTION_JSON_BYTES",
    "REPORT_SCHEMA_VERSION",
    "ResearchReportError",
    "ResearchReportResult",
    "export_research_report",
]
