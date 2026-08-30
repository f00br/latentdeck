from __future__ import annotations

import pytest
import torch

from latentdeck_comfy_toolkit import ToolkitContractError, project_offline


def synthetic_latent() -> torch.Tensor:
    index = torch.arange(24 * 2 * 3 * 4, dtype=torch.float32).reshape(1, 24, 2, 3, 4)
    return (torch.sin(index * 0.019) + 0.2 * torch.cos(index * 0.071)).half()


def test_offline_projector_is_bounded_deterministic_and_shape_preserving() -> None:
    latent = synthetic_latent()

    projected = project_offline(latent, components=3)
    repeated = project_offline(latent, components=3)

    assert torch.equal(projected.output, repeated.output)
    assert projected.output.shape == latent.shape
    assert projected.output.dtype == latent.dtype
    assert projected.output.device.type == "cpu"
    assert torch.isfinite(projected.output).all()
    assert not torch.equal(projected.output, latent)
    assert projected.provenance["execution"] == "offline_cpu_only"
    assert projected.provenance["method"] == "centered_full_svd"
    assert projected.provenance["components"] == 3
    assert projected.provenance["realtime_eligible"] is False


def test_offline_projector_rejects_rank_overflow_and_non_finite_input() -> None:
    latent = synthetic_latent()
    with pytest.raises(ToolkitContractError) as rank:
        project_offline(latent, components=25)
    assert rank.value.code == "projector.components_bound"

    damaged = latent.clone()
    damaged[0, 0, 0, 0, 0] = float("inf")
    with pytest.raises(ToolkitContractError) as non_finite:
        project_offline(damaged, components=3)
    assert non_finite.value.code == "projector.tensor_non_finite"
