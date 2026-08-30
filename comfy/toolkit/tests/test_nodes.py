from __future__ import annotations

import json

import pytest
import torch

from latentdeck_comfy_toolkit import (
    NODE_CLASS_MAPPINGS,
    DecoderHook,
    ToolkitContractError,
)


def latent_pair() -> tuple[dict[str, object], dict[str, object]]:
    index = torch.arange(24 * 2 * 3 * 4, dtype=torch.float32).reshape(1, 24, 2, 3, 4)
    return (
        {"samples": torch.sin(index * 0.03).half(), "batch_index": [0]},
        {"samples": torch.cos(index * 0.05).half(), "batch_index": [1]},
    )


def test_comfy_surface_exposes_all_five_xs_families_through_the_adapter() -> None:
    expected = {
        "LatentDeckToolkitXS1": "XS1",
        "LatentDeckToolkitXS2": "XS2",
        "LatentDeckToolkitXS3": "XS3",
        "LatentDeckToolkitXS4": "XS4",
        "LatentDeckToolkitXS5": "XS5",
    }
    assert {node_id: NODE_CLASS_MAPPINGS[node_id].ALGORITHM for node_id in expected} == expected

    latent_a, latent_b = latent_pair()
    node = NODE_CLASS_MAPPINGS["LatentDeckToolkitXS2"]()
    output, provenance_json = node.process(
        latent_a,
        latent_b,
        mix=0.4,
        mode="HYBRIDIZE",
        routing="A",
        interaction=0.8,
        preserve=0.5,
        chaos=0.0,
        seed=9,
        xs2_radius=1,
    )
    provenance = json.loads(provenance_json)

    assert output is not latent_a
    assert output["batch_index"] == [0]
    assert output["samples"].shape == latent_a["samples"].shape
    assert provenance["operation"]["algorithm"] == "XS2"


def test_compare_and_projector_nodes_keep_assets_external_and_projector_offline() -> None:
    compare_type = NODE_CLASS_MAPPINGS["LatentDeckToolkitCompareDecoders"]
    projector_type = NODE_CLASS_MAPPINGS["LatentDeckToolkitOfflineProjector"]
    latent_a, _ = latent_pair()

    def decode_image(value: torch.Tensor, _asset: object) -> torch.Tensor:
        return value[0, :3].permute(1, 2, 3, 0).float().contiguous()

    hook_fast = DecoderHook("org.example.fast", "1.0.0", decode_image)
    hook_hq = DecoderHook(
        "org.example.hq", "1.0.0", lambda value, asset: decode_image(value, asset) + 0.01
    )
    fast, hq, metrics_json = compare_type().compare(latent_a, hook_fast, hook_hq)
    projected, provenance_json = projector_type().project(latent_a, components=3)

    assert fast.shape == hq.shape
    assert json.loads(metrics_json)["kind"] == "latentdeck.toolkit.decoder_comparison"
    assert projected["samples"].device.type == "cpu"
    assert json.loads(provenance_json)["realtime_eligible"] is False


def test_compare_node_rejects_non_image_hook_outputs() -> None:
    compare_type = NODE_CLASS_MAPPINGS["LatentDeckToolkitCompareDecoders"]
    latent_a, _ = latent_pair()
    latent_output = DecoderHook("org.example.fast", "1.0.0", lambda value, _asset: value.float())

    with pytest.raises(ToolkitContractError) as caught:
        compare_type().compare(latent_a, latent_output, latent_output)

    assert caught.value.code == "node.decoder_output_invalid"


@pytest.mark.parametrize(
    ("node_id", "advanced"),
    [
        (
            "LatentDeckToolkitXS1",
            {"xs1_channel_a": 0, "xs1_channel_b": 1, "xs1_angle_degrees": 20.0},
        ),
        ("LatentDeckToolkitXS2", {"xs2_radius": 1}),
        ("LatentDeckToolkitXS3", {"xs3_high_gain": 0.5}),
        ("LatentDeckToolkitXS4", {"xs4_epsilon": 1e-6}),
        (
            "LatentDeckToolkitXS5",
            {
                "xs5_routing": "TOPK",
                "temperature": 0.12,
                "top_k": 4,
                "sinkhorn_iterations": 4,
            },
        ),
    ],
)
def test_each_xs_node_executes_the_public_adapter(
    node_id: str, advanced: dict[str, object]
) -> None:
    latent_a, latent_b = latent_pair()
    output, provenance_json = NODE_CLASS_MAPPINGS[node_id]().process(
        latent_a,
        latent_b,
        mix=0.5,
        mode="INTERACT",
        routing="A",
        interaction=0.7,
        preserve=0.4,
        chaos=0.0,
        seed=12,
        **advanced,
    )

    assert torch.isfinite(output["samples"]).all()
    assert json.loads(provenance_json)["operation"]["algorithm"] == node_id[-3:]
