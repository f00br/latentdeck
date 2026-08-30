"""Bounded, JSON-safe workflow lineage for Toolkit LATENT values.

The ledger travels with a Comfy ``LATENT`` mapping.  It is deliberately
path-free: source identities are content hashes/cartridge ids, while saved
outputs retain only the file name.  This lets LC Save/Resample and the research
report derive their inputs from the graph instead of asking users to retype
genealogy and operator parameters.
"""

from __future__ import annotations

import json
import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

WORKFLOW_METADATA_VERSION = "0.1.0"
LATENTDECK_METADATA_KEY = "latentdeck"
MAX_LEDGER_BYTES = 1_048_576
MAX_PARENTS = 256
MAX_OPERATIONS = 1_024
MAX_MEASUREMENTS = 256
MAX_OUTPUTS = 256


class WorkflowMetadataError(ValueError):
    """Stable error raised when graph metadata is unsafe or malformed."""


@dataclass(frozen=True, slots=True)
class ResampleInputs:
    parent_cartridges: tuple[dict[str, object], ...]
    operation_history: tuple[dict[str, object], ...]
    audio_disposition: dict[str, object]
    provenance_sources: tuple[dict[str, object], ...]


def _json_copy(value: object, *, label: str = "metadata") -> Any:
    try:
        encoded = json.dumps(
            value,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise WorkflowMetadataError(f"{label} must be finite JSON") from error
    if len(encoded) > MAX_LEDGER_BYTES:
        raise WorkflowMetadataError(f"{label} exceeds the workflow metadata limit")
    return json.loads(encoded)


def _latent_copy(latent: object) -> dict[str, object]:
    if not isinstance(latent, Mapping) or not all(isinstance(key, str) for key in latent):
        raise WorkflowMetadataError("LATENT must be a string-keyed mapping")
    # Tensors and Comfy carriers must retain identity; only the metadata ledger
    # is deep-copied/normalised below.
    return dict(latent)


def _ledger(latent: object) -> dict[str, object]:
    if not isinstance(latent, Mapping):
        return _empty_ledger()
    value = latent.get(LATENTDECK_METADATA_KEY)
    if not isinstance(value, Mapping):
        return _empty_ledger()
    copied = _json_copy(dict(value), label="LatentDeck workflow metadata")
    if not isinstance(copied, dict):
        raise WorkflowMetadataError("LatentDeck workflow metadata must be an object")
    copied.setdefault("schema_version", WORKFLOW_METADATA_VERSION)
    copied.setdefault("parents", [])
    copied.setdefault("provenance_sources", [])
    copied.setdefault("operation_history", [])
    copied.setdefault("measurements", [])
    copied.setdefault("outputs", [])
    return copied


def _empty_ledger() -> dict[str, object]:
    return {
        "schema_version": WORKFLOW_METADATA_VERSION,
        "parents": [],
        "provenance_sources": [],
        "operation_history": [],
        "measurements": [],
        "outputs": [],
        "audio": {"policy": "source_absent"},
    }


def _with_ledger(latent: object, ledger: Mapping[str, object]) -> dict[str, object]:
    result = _latent_copy(latent)
    result[LATENTDECK_METADATA_KEY] = _json_copy(
        dict(ledger), label="LatentDeck workflow metadata"
    )
    return result


def _require_sha256(value: object, *, label: str) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise WorkflowMetadataError(f"{label} must be a lowercase SHA-256")
    return value


def _dedupe(
    values: Sequence[Mapping[str, object]], keys: tuple[str, ...]
) -> list[dict[str, object]]:
    result: list[dict[str, object]] = []
    seen: set[tuple[object, ...]] = set()
    for value in values:
        copied = _json_copy(dict(value))
        identity = tuple(copied.get(key) for key in keys)
        if identity in seen:
            continue
        seen.add(identity)
        result.append(copied)
    return result


def _direct_parent(ledger: Mapping[str, object], role: str) -> dict[str, object] | None:
    identity = ledger.get("source_cartridge")
    if not isinstance(identity, Mapping):
        return None
    cartridge_id = identity.get("cartridge_id")
    archive_sha256 = identity.get("archive_sha256")
    if not isinstance(cartridge_id, str):
        raise WorkflowMetadataError("source cartridge id is missing")
    _require_sha256(archive_sha256, label="source cartridge hash")
    return {
        "cartridge_id": cartridge_id,
        "archive_sha256": archive_sha256,
        "role": role,
    }


def initialize_lc_metadata(
    latent: object,
    *,
    manifest: Mapping[str, object],
    validation: Mapping[str, object],
) -> dict[str, object]:
    """Attach the immediate LC identity without inheriting its old graph ledger."""

    cartridge_id = manifest.get("cartridge_id")
    archive_sha256 = validation.get("archive_sha256")
    if not isinstance(cartridge_id, str) or not cartridge_id:
        raise WorkflowMetadataError("LC manifest cartridge_id is missing")
    _require_sha256(archive_sha256, label="LC archive hash")
    codec = manifest.get("codec")
    timing = manifest.get("timing")
    ledger = _ledger(latent)
    ledger.update(
        {
            "source_kind": "latent_cartridge",
            "source_cartridge": {
                "cartridge_id": cartridge_id,
                "archive_sha256": archive_sha256,
            },
            "codec": _json_copy(codec if isinstance(codec, Mapping) else {}),
            "timing": _json_copy(timing if isinstance(timing, Mapping) else {}),
            "manifest_provenance": _json_copy(
                manifest.get("provenance")
                if isinstance(manifest.get("provenance"), Mapping)
                else {}
            ),
            # Retained for the public parent_cartridge_ref helper and for
            # inspection nodes that need the complete validated identity.
            "manifest": _json_copy(dict(manifest), label="LC manifest"),
            "validation": _json_copy(dict(validation), label="LC validation"),
        }
    )
    tensor_descriptors = manifest.get("tensors")
    has_audio = isinstance(tensor_descriptors, Sequence) and any(
        isinstance(descriptor, Mapping)
        and (descriptor.get("stream") == "audio" or descriptor.get("name") == "audio")
        for descriptor in tensor_descriptors
    )
    ledger["audio"] = (
        {
            "policy": "copied_from_carrier_exact",
            "source_cartridge": {
                "cartridge_id": cartridge_id,
                "archive_sha256": archive_sha256,
            },
        }
        if has_audio
        else {"policy": "source_absent"}
    )
    return _with_ledger(latent, ledger)


def initialize_raw_metadata(
    latent: object,
    *,
    profile: Mapping[str, object],
    source: Mapping[str, object],
) -> dict[str, object]:
    """Attach content-addressed provenance to an old raw H3 Safetensors input."""

    sha256 = _require_sha256(source.get("sha256"), label="raw source hash")
    byte_length = source.get("byte_length")
    if isinstance(byte_length, bool) or not isinstance(byte_length, int) or byte_length <= 0:
        raise WorkflowMetadataError("raw source byte_length must be a positive integer")
    ledger = _ledger(latent)
    ledger.update(
        {
            "source_kind": "raw_h3_safetensors",
            "codec": _json_copy(dict(profile)),
            "profile": _json_copy(dict(profile)),
            "source": _json_copy(dict(source)),
            "provenance_sources": [
                {
                    "kind": "raw_h3_safetensors",
                    "sha256": sha256,
                    "metadata": {"byte_length": byte_length},
                }
            ],
        }
    )
    ledger["audio"] = (
        {"policy": "preserved_source"}
        if bool(
            getattr(
                latent.get("samples") if isinstance(latent, Mapping) else None,
                "is_nested",
                False,
            )
        )
        else {"policy": "source_absent"}
    )
    return _with_ledger(latent, ledger)


def _operation_record(provenance: Mapping[str, object]) -> dict[str, object]:
    operation = provenance.get("operation")
    if isinstance(operation, Mapping):
        operator_id = operation.get("operator_id")
        operator_version = operation.get("operator_version")
        seed = operation.get("seed", 0)
        controls = operation.get("controls", {})
    elif isinstance(operation, str) and operation:
        slug = re.sub(r"[^a-z0-9]+", "_", operation.lower()).strip("_")
        if not slug:
            raise WorkflowMetadataError("operation name cannot be normalised")
        operator_id = f"org.latentdeck.toolkit.{slug}"
        operator_version = WORKFLOW_METADATA_VERSION
        parameters = provenance.get("parameters", {})
        if not isinstance(parameters, Mapping):
            raise WorkflowMetadataError("operation parameters must be an object")
        seed = parameters.get("seed", 0)
        controls = {key: value for key, value in parameters.items() if key != "seed"}
    else:
        raise WorkflowMetadataError("operation provenance must declare an operation")
    if not isinstance(operator_id, str) or not operator_id:
        raise WorkflowMetadataError("operator_id is missing")
    if not isinstance(operator_version, str) or not operator_version:
        raise WorkflowMetadataError("operator_version is missing")
    if isinstance(seed, bool) or not isinstance(seed, int):
        raise WorkflowMetadataError("operator seed must be an integer")
    if not isinstance(controls, Mapping):
        raise WorkflowMetadataError("operator controls must be an object")
    record = {
        "operator_id": operator_id,
        "operator_version": operator_version,
        "seed": seed,
        "controls": _json_copy(dict(controls), label="operator controls"),
    }
    return _json_copy(record, label="operation record")


def annotate_operation(
    output: object,
    *,
    sources: Sequence[tuple[str, object]],
    structural_role: str,
    provenance: Mapping[str, object],
) -> dict[str, object]:
    """Accumulate source genealogy and one normalised LC operation record."""

    if not sources:
        raise WorkflowMetadataError("an operation requires at least one source")
    if not isinstance(structural_role, str) or not structural_role:
        raise WorkflowMetadataError("structural_role is missing")
    source_ledgers: list[tuple[str, dict[str, object]]] = []
    for role, latent in sources:
        if not isinstance(role, str) or not role:
            raise WorkflowMetadataError("source role is missing")
        source_ledgers.append((role, _ledger(latent)))

    base = _ledger(output)
    parents: list[Mapping[str, object]] = []
    provenance_sources: list[Mapping[str, object]] = []
    operations: list[Mapping[str, object]] = []
    measurements: list[Mapping[str, object]] = []
    outputs: list[Mapping[str, object]] = []
    for role, ledger in source_ledgers:
        direct = _direct_parent(ledger, role)
        if direct is not None:
            parents.append(direct)
        parents.extend(value for value in ledger.get("parents", []) if isinstance(value, Mapping))
        provenance_sources.extend(
            value for value in ledger.get("provenance_sources", []) if isinstance(value, Mapping)
        )
        operations.extend(
            value for value in ledger.get("operation_history", []) if isinstance(value, Mapping)
        )
        measurements.extend(
            value for value in ledger.get("measurements", []) if isinstance(value, Mapping)
        )
        outputs.extend(value for value in ledger.get("outputs", []) if isinstance(value, Mapping))

    operations.append(_operation_record(provenance))
    base["parents"] = _dedupe(parents, ("cartridge_id", "archive_sha256", "role"))
    base["provenance_sources"] = _dedupe(
        provenance_sources, ("kind", "sha256")
    )
    base["operation_history"] = _json_copy(operations)
    base["measurements"] = _json_copy(measurements)
    base["outputs"] = _json_copy(outputs)
    if len(base["parents"]) > MAX_PARENTS:
        raise WorkflowMetadataError("too many parent cartridges")
    if len(base["operation_history"]) > MAX_OPERATIONS:
        raise WorkflowMetadataError("too many operation records")

    structural = next(
        (ledger for role, ledger in source_ledgers if role == structural_role), None
    )
    if structural is None:
        raise WorkflowMetadataError("structural_role does not identify an input")
    audio = structural.get("audio")
    base["audio"] = _json_copy(audio if isinstance(audio, Mapping) else {"policy": "source_absent"})
    parameters = provenance.get("parameters")
    parameter_audio = parameters.get("audio_policy") if isinstance(parameters, Mapping) else None
    explicit_audio = provenance.get("audio_policy")
    audio_action = provenance.get("audio_action")
    if (
        parameter_audio == "DROP"
        or audio_action == "dropped_explicitly"
        or (
            isinstance(explicit_audio, str)
            and explicit_audio.startswith("omitted_")
        )
    ):
        base["audio"] = {"policy": "omitted_timing_mismatch"}
    # Once an operator has run, the graph represents a derived latent rather
    # than the originally loaded cartridge itself.
    base.pop("source_cartridge", None)
    base["source_kind"] = "toolkit_post_operator_h3"
    base["last_operation"] = _json_copy(dict(provenance), label="operation provenance")
    return _with_ledger(output, base)


def annotate_evaluation(
    latent: object, *, kind: str, report: Mapping[str, object]
) -> dict[str, object]:
    if not isinstance(kind, str) or not kind:
        raise WorkflowMetadataError("evaluation kind is missing")
    ledger = _ledger(latent)
    values = [value for value in ledger.get("measurements", []) if isinstance(value, Mapping)]
    values.append({"kind": kind, "report": _json_copy(dict(report), label="evaluation report")})
    if len(values) > MAX_MEASUREMENTS:
        raise WorkflowMetadataError("too many evaluation measurements")
    ledger["measurements"] = _json_copy(values)
    return _with_ledger(latent, ledger)


def record_saved_output(
    latent: object,
    *,
    cartridge_id: str,
    archive_sha256: str,
    file_name: str,
) -> dict[str, object]:
    if not isinstance(cartridge_id, str) or not cartridge_id:
        raise WorkflowMetadataError("saved cartridge_id is missing")
    _require_sha256(archive_sha256, label="saved archive hash")
    if not isinstance(file_name, str) or not file_name or "/" in file_name or "\\" in file_name:
        raise WorkflowMetadataError("saved output must contain a path-free file name")
    ledger = _ledger(latent)
    values = [value for value in ledger.get("outputs", []) if isinstance(value, Mapping)]
    values.append(
        {
            "cartridge_id": cartridge_id,
            "archive_sha256": archive_sha256,
            "file_name": file_name,
        }
    )
    if len(values) > MAX_OUTPUTS:
        raise WorkflowMetadataError("too many saved outputs")
    ledger["outputs"] = _json_copy(values)
    return _with_ledger(latent, ledger)


def derive_resample_inputs(latent: object) -> ResampleInputs:
    ledger = _ledger(latent)
    parents = tuple(
        _json_copy(value)
        for value in ledger.get("parents", [])
        if isinstance(value, Mapping)
    )
    operations = tuple(
        _json_copy(value)
        for value in ledger.get("operation_history", [])
        if isinstance(value, Mapping)
    )
    audio = ledger.get("audio")
    sources = tuple(
        _json_copy(value)
        for value in ledger.get("provenance_sources", [])
        if isinstance(value, Mapping)
    )
    return ResampleInputs(
        parent_cartridges=parents,
        operation_history=operations,
        audio_disposition=_json_copy(
            audio if isinstance(audio, Mapping) else {"policy": "source_absent"}
        ),
        provenance_sources=sources,
    )


def derive_research_report_inputs(latent: object) -> dict[str, object]:
    ledger = _ledger(latent)
    cartridges: list[dict[str, object]] = []
    identity = ledger.get("source_cartridge")
    if isinstance(identity, Mapping):
        cartridges.append(_json_copy(identity))
    for parent in ledger.get("parents", []):
        if not isinstance(parent, Mapping):
            continue
        cartridges.append(
            {
                "cartridge_id": parent.get("cartridge_id"),
                "archive_sha256": parent.get("archive_sha256"),
                "role": parent.get("role"),
            }
        )
    cartridges = _dedupe(cartridges, ("cartridge_id", "archive_sha256", "role"))
    result = {
        "versions": {"toolkit": WORKFLOW_METADATA_VERSION, "ledger": WORKFLOW_METADATA_VERSION},
        "cartridges": cartridges,
        "raw_sources": _json_copy(ledger.get("provenance_sources", [])),
        "operators": _json_copy(ledger.get("operation_history", [])),
        "measurements": _json_copy(ledger.get("measurements", [])),
        "outputs": _json_copy(ledger.get("outputs", [])),
    }
    return _json_copy(result, label="research report inputs")


__all__ = [
    "LATENTDECK_METADATA_KEY",
    "ResampleInputs",
    "WORKFLOW_METADATA_VERSION",
    "WorkflowMetadataError",
    "annotate_evaluation",
    "annotate_operation",
    "derive_research_report_inputs",
    "derive_resample_inputs",
    "initialize_lc_metadata",
    "initialize_raw_metadata",
    "record_saved_output",
]
