"""Deterministic bounded offline latent projection for Comfy research."""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any

import torch

from .decoder_compare import ToolkitContractError

MAX_PROJECTOR_TOKENS = 262_144
PROJECTOR_VERSION = "0.1.0"


@dataclass(frozen=True, slots=True)
class ProjectionResult:
    output: torch.Tensor
    provenance: dict[str, Any]


def _validate_input(
    value: object,
    components: object,
    *,
    require_cpu: bool,
) -> tuple[torch.Tensor, int, int]:
    if not isinstance(value, torch.Tensor):
        raise ToolkitContractError("projector.tensor_type", "latent must be a torch.Tensor")
    if value.ndim != 5 or value.shape[0] != 1 or value.shape[1] != 24:
        raise ToolkitContractError("projector.tensor_shape", "latent must have layout [1,24,T,H,W]")
    if value.shape[2] < 1 or value.shape[3] < 1 or value.shape[4] < 1:
        raise ToolkitContractError("projector.tensor_shape", "latent must have layout [1,24,T,H,W]")
    if value.layout is not torch.strided or value.device.type not in {"cpu", "cuda"}:
        raise ToolkitContractError(
            "projector.tensor_layout", "offline projector requires dense CPU or CUDA storage"
        )
    if require_cpu and value.device.type != "cpu":
        raise ToolkitContractError(
            "projector.cpu_required", "offline projector requires a dense CPU tensor"
        )
    if value.dtype not in {torch.float16, torch.float32}:
        raise ToolkitContractError(
            "projector.tensor_dtype", "offline projector accepts only F16 or F32"
        )
    tokens = value.shape[2] * value.shape[3] * value.shape[4]
    if tokens > MAX_PROJECTOR_TOKENS:
        raise ToolkitContractError(
            "projector.tensor_bound",
            f"latent exceeds the {MAX_PROJECTOR_TOKENS}-token offline bound",
        )
    if not bool(torch.isfinite(value).all().item()):
        raise ToolkitContractError("projector.tensor_non_finite", "latent contains NaN or Inf")
    if isinstance(components, bool) or not isinstance(components, int):
        raise ToolkitContractError("projector.components_type", "components must be an integer")
    maximum = min(24, tokens)
    if not 1 <= components <= maximum:
        raise ToolkitContractError(
            "projector.components_bound", f"components must be in [1, {maximum}]"
        )
    return value, components, tokens


def preflight_projector_input(latent: torch.Tensor, *, components: int) -> None:
    """Validate all allocation and data bounds before a Comfy CPU transfer."""

    _validate_input(latent, components, require_cpu=False)


@torch.inference_mode()
def project_offline(latent: torch.Tensor, *, components: int) -> ProjectionResult:
    """Reconstruct an H3 latent through a deterministic centered full SVD.

    This API intentionally accepts CPU tensors only. A Comfy node may make the
    CPU staging explicit, but the realtime codec worker must never call this
    function.
    """

    source, rank, tokens = _validate_input(latent, components, require_cpu=True)
    matrix = source[0].permute(1, 2, 3, 0).reshape(tokens, 24).double()
    channel_mean = matrix.mean(dim=0, keepdim=True)
    centered = matrix - channel_mean
    left, singular_values, right = torch.linalg.svd(centered, full_matrices=False)
    reconstructed = (left[:, :rank] * singular_values[:rank]) @ right[:rank, :] + channel_mean
    output = (
        reconstructed.reshape(source.shape[2], source.shape[3], source.shape[4], 24)
        .permute(3, 0, 1, 2)
        .unsqueeze(0)
        .to(dtype=source.dtype)
        .contiguous()
    )
    if not bool(torch.isfinite(output).all().item()):
        raise ToolkitContractError(
            "projector.output_non_finite", "offline projector produced NaN or Inf"
        )

    energy = singular_values.square()
    total_energy = float(energy.sum().item())
    retained_energy = float(energy[:rank].sum().item())
    retained_fraction = 1.0 if total_energy == 0.0 else retained_energy / total_energy
    provenance: dict[str, Any] = {
        "schema_version": PROJECTOR_VERSION,
        "kind": "latentdeck.toolkit.offline_projector",
        "execution": "offline_cpu_only",
        "realtime_eligible": False,
        "method": "centered_full_svd",
        "projector_version": PROJECTOR_VERSION,
        "components": rank,
        "compute_dtype": "F64",
        "storage_dtype": "F16" if source.dtype == torch.float16 else "F32",
        "shape": list(source.shape),
        "tokens": tokens,
        "retained_energy_fraction": retained_fraction,
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return ProjectionResult(output=output, provenance=provenance)


__all__ = [
    "MAX_PROJECTOR_TOKENS",
    "PROJECTOR_VERSION",
    "ProjectionResult",
    "preflight_projector_input",
    "project_offline",
]
