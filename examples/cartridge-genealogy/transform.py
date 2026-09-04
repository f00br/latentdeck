"""Public CPU example for a genealogy-preserving LC transformation."""

from __future__ import annotations

import uuid
from collections.abc import Callable, Mapping
from pathlib import Path

import latentdeck_cartridge
import torch
from latentdeck_comfy_toolkit import (
    load_lc,
    parent_cartridge_ref,
    save_resampled_lc,
)
from latentdeck_comfy_toolkit.workflow_metadata import annotate_operation


def transform_cartridge(
    source_path: str | Path,
    output_path: str | Path,
    *,
    operator: Callable[[torch.Tensor, Mapping[str, object], int], torch.Tensor],
    operator_id: str,
    operator_version: str,
    controls: Mapping[str, object],
    seed: int,
) -> dict[str, object]:
    """Transform visual-only `A.lc` into validated `B.lc` without overwrite."""

    source = load_lc(source_path)
    samples = source.latent.get("samples")
    if not isinstance(samples, torch.Tensor):
        raise ValueError("this example is visual-only and will not silently drop LC audio")
    transformed = operator(samples, dict(controls), seed)
    if not isinstance(transformed, torch.Tensor):
        raise TypeError("operator must return a torch.Tensor")
    operation = {
        "operator_id": operator_id,
        "operator_version": operator_version,
        "seed": seed,
        "controls": dict(controls),
    }
    annotated = annotate_operation(
        {"samples": transformed},
        sources=(("source", source.latent),),
        structural_role="source",
        provenance={"operation": operation},
    )
    parent = parent_cartridge_ref(source.latent, role="source")
    output_id = str(uuid.uuid4())
    saved = save_resampled_lc(
        annotated,
        output_path,
        cartridge_id=output_id,
        overwrite=False,
    )
    validation = latentdeck_cartridge.validate(saved.output_path)
    if validation.get("status") != "ok":
        raise RuntimeError("Cartridge SDK did not return status=ok for B.lc")
    evidence = validation.get("validation")
    if not isinstance(evidence, dict) or evidence.get("validation_level") != "full":
        raise RuntimeError("B.lc did not pass full validation")
    inspection = latentdeck_cartridge.inspect(saved.output_path)
    manifest = inspection.get("manifest")
    if not isinstance(manifest, dict):
        raise RuntimeError("B.lc inspection omitted its manifest")
    if manifest.get("cartridge_id") != output_id or manifest.get("parent_cartridges") != [parent]:
        raise RuntimeError("B.lc genealogy differs from the requested transformation")
    return {
        "output_path": str(saved.output_path),
        "cartridge_id": output_id,
        "archive_sha256": evidence.get("archive_sha256"),
        "parent": parent,
        "operation": operation,
        "validation_level": "full",
    }


__all__ = ["transform_cartridge"]
