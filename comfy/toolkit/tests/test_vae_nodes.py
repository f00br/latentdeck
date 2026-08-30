from __future__ import annotations

import json

import torch

from latentdeck_comfy_toolkit.vae_nodes import (
    VAE_NODE_CLASS_MAPPINGS,
    VAE_NODE_DISPLAY_NAME_MAPPINGS,
    DeclaredH3Vae,
)


class FakeVae:
    def decode(self, latent: torch.Tensor) -> torch.Tensor:
        return latent[:, :3].movedim(1, -1).float().contiguous()

    def encode(self, image: torch.Tensor) -> torch.Tensor:
        visual = image.movedim(-1, 1)
        return visual.repeat(1, 8, 1, 1, 1).half().contiguous()


def latent() -> dict[str, torch.Tensor]:
    return {"samples": torch.linspace(-1.0, 1.0, 24 * 2 * 2 * 2).reshape(1, 24, 2, 2, 2).half()}


def declare(role: str, marker: str) -> DeclaredH3Vae:
    node = VAE_NODE_CLASS_MAPPINGS["LatentDeckToolkitDeclareH3Vae"]()
    (declared,) = node.declare(
        FakeVae(),
        role,
        f"org.example.{role.lower()}",
        "1.0.0",
        "explicit test asset",
        "test-only",
        marker * 64,
    )
    return declared


def test_vae_node_registry_is_a_ready_fast_hq_projector_surface() -> None:
    expected = {
        "LatentDeckToolkitDeclareH3Vae",
        "LatentDeckToolkitFastDecode",
        "LatentDeckToolkitHQDecode",
        "LatentDeckToolkitFastHQComparator",
        "LatentDeckToolkitManifoldProjector",
        "LatentDeckToolkitProjectorComparison",
    }
    assert set(VAE_NODE_CLASS_MAPPINGS) == expected
    assert set(VAE_NODE_DISPLAY_NAME_MAPPINGS) == expected
    assert "TAEHV / taeh3" in VAE_NODE_DISPLAY_NAME_MAPPINGS["LatentDeckToolkitFastDecode"]
    assert "Native H3" in VAE_NODE_DISPLAY_NAME_MAPPINGS["LatentDeckToolkitHQDecode"]


def test_fast_hq_decode_and_comparator_use_declared_external_assets() -> None:
    source = latent()
    fast = declare("FAST", "a")
    hq = declare("HQ", "b")

    fast_image, fast_json = VAE_NODE_CLASS_MAPPINGS["LatentDeckToolkitFastDecode"]().decode(
        source, fast
    )
    hq_image, hq_json = VAE_NODE_CLASS_MAPPINGS["LatentDeckToolkitHQDecode"]().decode(source, hq)
    compared_fast, compared_hq, comparison_json = VAE_NODE_CLASS_MAPPINGS[
        "LatentDeckToolkitFastHQComparator"
    ]().compare(source, fast, hq)

    assert fast_image.shape == hq_image.shape == (2, 2, 2, 3)
    assert torch.equal(compared_fast, fast_image)
    assert torch.equal(compared_hq, hq_image)
    assert json.loads(fast_json)["decoder"]["role"] == "FAST"
    assert json.loads(hq_json)["decoder"]["role"] == "HQ"
    assert json.loads(comparison_json)["kind"].endswith("h3_fast_hq_comparison")


def test_projector_and_raw_projected_comparison_are_directly_wired() -> None:
    source = latent()
    fast = declare("FAST", "c")
    hq = declare("HQ", "d")
    projected, native_image, projector_json = VAE_NODE_CLASS_MAPPINGS[
        "LatentDeckToolkitManifoldProjector"
    ]().project(source, hq)
    raw_fast, projected_fast, raw_hq, projected_hq, report_json = VAE_NODE_CLASS_MAPPINGS[
        "LatentDeckToolkitProjectorComparison"
    ]().compare(source, projected, fast, hq)

    assert projected["samples"].shape == source["samples"].shape
    assert native_image.shape == (2, 2, 2, 3)
    assert raw_fast.shape == projected_fast.shape
    assert raw_hq.shape == projected_hq.shape
    assert json.loads(projector_json)["explicit_native_reencode"] is True
    assert json.loads(report_json)["same_decoder_pair"] is True
