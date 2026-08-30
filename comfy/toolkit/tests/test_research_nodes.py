from __future__ import annotations

import json

import pytest
import torch

from latentdeck_comfy_toolkit.research_nodes import (
    RESEARCH_NODE_CLASS_MAPPINGS,
    RESEARCH_NODE_DISPLAY_NAME_MAPPINGS,
    LatentDeckResearchOperatorHook,
)


def latent(offset: float = 0.0, *, slots: int = 3) -> dict[str, object]:
    values = torch.arange(24 * slots * 2 * 2, dtype=torch.float32)
    video = torch.sin(values.reshape(1, 24, slots, 2, 2) * 0.02 + offset).half()
    return {"samples": video.contiguous(), "source_id": f"source-{offset}"}


def test_research_node_registry_exposes_every_requested_operator_lab_surface() -> None:
    expected = {
        "LatentDeckToolkitXS1",
        "LatentDeckToolkitXS2",
        "LatentDeckToolkitXS3",
        "LatentDeckToolkitXS4",
        "LatentDeckToolkitXS5",
        "LatentDeckToolkitDualMixerLab",
        "LatentDeckToolkitCarrierDonorRouter",
        "LatentDeckToolkitQuadMixerLab",
        "LatentDeckToolkitTemporalLab",
        "LatentDeckToolkitFeedbackLab",
        "LatentDeckToolkitChannelLab",
        "LatentDeckToolkitOperatorChainReceipt",
        "LatentDeckToolkitLatentScopes",
        "LatentDeckToolkitDualOperatorHook",
        "LatentDeckToolkitOperatorBenchmark",
        "LatentDeckToolkitDeterminismTest",
        "LatentDeckToolkitStreamingCompatibilityTest",
    }
    assert set(RESEARCH_NODE_CLASS_MAPPINGS) == expected
    assert set(RESEARCH_NODE_DISPLAY_NAME_MAPPINGS) == expected
    assert "Frequency" in RESEARCH_NODE_DISPLAY_NAME_MAPPINGS["LatentDeckToolkitXS3"]
    assert (
        "Carrier + 3 Donors" in RESEARCH_NODE_DISPLAY_NAME_MAPPINGS["LatentDeckToolkitQuadMixerLab"]
    )


def test_xs_nodes_execute_the_actual_new_operator_meanings() -> None:
    a, b = latent(0.0), latent(0.7)
    xs1 = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitXS1"]()
    output, receipt = xs1.process(a, b, json.dumps([0.0] * 12 + [1.0] * 12))
    output_tensor = output["samples"]

    assert torch.equal(output_tensor[:, :12], a["samples"][:, :12])
    assert torch.equal(output_tensor[:, 12:], b["samples"][:, 12:])
    assert json.loads(receipt)["operation"] == "XS1_CHANNEL_MIXER"

    xs3 = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitXS3"]()
    _, xs3_receipt = xs3.process(a, b, 0.25, "LOW", 1.0)
    assert json.loads(xs3_receipt)["parameters"]["domain"] == "FFT2_SPATIAL"


def test_quad_node_and_router_accept_duplicate_donors_with_visible_provenance() -> None:
    a, donor = latent(0.0, slots=2), latent(0.9, slots=2)
    router = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitCarrierDonorRouter"]()
    carrier, b, c, d, weight_b, weight_c, weight_d, router_json = router.route(
        a,
        donor,
        donor,
        donor,
        1.0,
        0.5,
        0.25,
        "D,B,C",
    )
    assert (weight_b, weight_c, weight_d) == pytest.approx((0.25 / 1.75, 1.0 / 1.75, 0.5 / 1.75))
    quad = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitQuadMixerLab"]()
    output, report_json = quad.process(
        carrier,
        b,
        c,
        d,
        "LINEAR",
        0.7,
        "HYBRIDIZE",
        0.5,
        "MANUAL",
        1.0,
        0.5,
        0.25,
        0.5,
        1.0 / 3.0,
        "TOPK",
        0.12,
        2,
        3,
        0.0,
        17,
        "A,DUP,DUP,DUP",
    )

    assert output["samples"].shape == a["samples"].shape
    assert json.loads(router_json)["order"] == ["D", "B", "C"]
    report = json.loads(report_json)
    assert report["duplicate_source_test"] is True
    assert report["acceptance_scope"] == "functional_not_source_diversity"


def test_diagnostics_and_evaluation_nodes_use_an_explicit_operator_hook() -> None:
    carrier, donor = latent(0.0, slots=3), latent(0.5, slots=3)
    hook_node = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitDualOperatorHook"]()
    (hook,) = hook_node.build(
        donor,
        "LINEAR",
        '{"mix":0.25}',
        11,
        True,
    )
    assert isinstance(hook, LatentDeckResearchOperatorHook)

    benchmark = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitOperatorBenchmark"]()
    bench_output, bench_json = benchmark.run(carrier, hook, 0, 2)
    determinism = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitDeterminismTest"]()
    deterministic_output, deterministic_json = determinism.run(carrier, hook, 3)
    streaming = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitStreamingCompatibilityTest"]()
    full, chunks, streaming_json = streaming.run(carrier, hook, 1, 0.0, 0.0)

    assert bench_output["samples"].shape == carrier["samples"].shape
    assert json.loads(bench_json)["execution_ms"]["runs"] == 2
    assert deterministic_output["samples"].shape == carrier["samples"].shape
    assert json.loads(deterministic_json)["deterministic"] is True
    assert torch.equal(full["samples"], chunks["samples"])
    assert json.loads(streaming_json)["compatible"] is True


def test_chain_receipt_and_scopes_are_json_safe_output_nodes() -> None:
    value = latent()
    scopes = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitLatentScopes"]()
    scoped, scope_json = scopes.inspect(value)
    chain = RESEARCH_NODE_CLASS_MAPPINGS["LatentDeckToolkitOperatorChainReceipt"]()
    passed, chain_json = chain.collect(
        value,
        '{"operation":"XS5"}',
        '{"operation":"FREQUENCY"}',
        "",
        "",
    )

    assert json.loads(scope_json)["finite"] is True
    assert scoped["latentdeck"]["measurements"][0]["kind"] == "latent_scopes"
    assert passed is value
    assert [step["operation"] for step in json.loads(chain_json)["chain"]] == [
        "XS5",
        "FREQUENCY",
    ]
