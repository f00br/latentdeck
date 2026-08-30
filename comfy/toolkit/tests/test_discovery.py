from __future__ import annotations

import comfy.toolkit as discovery
import torch

from latentdeck_comfy_toolkit import DecoderHook


def test_comfy_discovery_shim_uses_the_canonical_installed_package_identity() -> None:
    node_type = discovery.NODE_CLASS_MAPPINGS["LatentDeckToolkitCompareDecoders"]
    latent = {"samples": torch.zeros((1, 24, 1, 2, 2), dtype=torch.float16)}

    def decode(value: torch.Tensor, _asset: object) -> torch.Tensor:
        return value[0, :3].permute(1, 2, 3, 0).float().contiguous()

    hook = DecoderHook("org.example.decoder", "1.0.0", decode)
    fast, hq, _provenance = node_type().compare(latent, hook, hook)

    assert fast.shape == (1, 2, 2, 3)
    assert torch.equal(fast, hq)
