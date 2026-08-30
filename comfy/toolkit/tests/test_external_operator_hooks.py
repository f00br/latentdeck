from __future__ import annotations

import json
import sys

import pytest
import torch

from latentdeck_comfy_toolkit import (
    OperatorContext,
    ToolkitContractError,
    ToolkitOperatorResult,
    TrustedOperatorRegistry,
    build_installed_operator_research_hook,
)
from latentdeck_comfy_toolkit.research_nodes import (
    LatentDeckToolkitDeterminismTest,
    LatentDeckToolkitOperatorBenchmark,
    LatentDeckToolkitStreamingCompatibilityTest,
)


def _descriptor(topology: str, input_count: int) -> dict[str, object]:
    return {
        "schema_version": "0.1.0",
        "operator_id": f"org.example.integration.{topology}",
        "operator_version": "0.1.0",
        "trust": "explicit_install",
        "entrypoint": f"not_a_real_module_{topology}:process_sources",
        "topology": topology,
        "input_count": input_count,
        "capabilities": {
            "full_clip": True,
            "streaming": True,
            "chunk": True,
            "deterministic": True,
        },
        "supported_profiles": [
            {
                "codec_family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_version": "0.1.0",
                "timing_contract": "minimax_h3_causal",
                "timing_contract_version": "0.1.0",
                "layout": "[1,24,1,H,W]",
                "runtime_dtype": "F16",
            }
        ],
        "controls": {
            "amount": {
                "type": "float",
                "default": 0.5,
                "minimum": 0.0,
                "maximum": 1.0,
            }
        },
        "bypass": {"controls": {"amount": 0.0}, "output_source": 0},
        "limits": {"max_spatial_tokens": 4096},
    }


def _install(topology: str, input_count: int):  # type: ignore[no-untyped-def]
    operator_id = f"org.example.integration.{topology}"

    def process_sources(
        sources: tuple[torch.Tensor, ...],
        controls: dict[str, object],
        context: OperatorContext,
    ) -> ToolkitOperatorResult:
        amount = float(controls["amount"])
        output = sources[0].float()
        if len(sources) == 1:
            output = output + amount
        else:
            for index, source in enumerate(sources[1:], start=1):
                output = output + source.float() * (amount * index)
        return ToolkitOperatorResult(
            output.half().contiguous(),
            {
                "operation": {
                    "operator_id": operator_id,
                    "operator_version": "0.1.0",
                    "controls": controls,
                    "seed": context.seed,
                }
            },
        )

    descriptor = _descriptor(topology, input_count)
    entrypoint = str(descriptor["entrypoint"])
    module_name = entrypoint.partition(":")[0]
    assert module_name not in sys.modules
    registry = TrustedOperatorRegistry()
    registry.install(descriptor, process_sources, exported_entrypoint=entrypoint)
    assert module_name not in sys.modules
    return registry.load(operator_id, "0.1.0")


def _sources(input_count: int) -> tuple[dict[str, object], tuple[dict[str, object], ...]]:
    values = torch.arange(24 * 4 * 2 * 3, dtype=torch.float32).reshape(1, 24, 4, 2, 3)
    primary = {"samples": (values / 100).half(), "marker": "primary"}
    captured = tuple(
        {"samples": torch.full_like(primary["samples"], float(index))}
        for index in range(1, input_count)
    )
    return primary, captured


@pytest.mark.parametrize(
    ("topology", "input_count"),
    (("single_source", 1), ("dual_source", 2), ("carrier_donors", 4)),
)
def test_installed_external_topologies_run_through_all_evaluation_nodes(
    topology: str, input_count: int
) -> None:
    installed = _install(topology, input_count)
    primary, captured = _sources(input_count)
    hook = build_installed_operator_research_hook(
        installed,
        captured_sources=captured,
        controls={"amount": 0.25},
        seed=17,
    )

    benchmark_output, benchmark_json = LatentDeckToolkitOperatorBenchmark().run(
        primary, hook, 0, 2
    )
    deterministic_output, deterministic_json = LatentDeckToolkitDeterminismTest().run(
        primary, hook, 3
    )
    full, chunked, streaming_json = LatentDeckToolkitStreamingCompatibilityTest().run(
        primary, hook, 2, 0.0, 0.0
    )

    benchmark = json.loads(benchmark_json)
    determinism = json.loads(deterministic_json)
    streaming = json.loads(streaming_json)
    assert benchmark["operator"]["topology"] == topology
    assert benchmark["operator"]["input_count"] == input_count
    assert benchmark["streaming_compatible"] is True
    assert determinism["deterministic"] is True
    assert streaming["compatible"] is True
    assert streaming["operator"]["topology"] == topology
    assert torch.equal(benchmark_output["samples"], deterministic_output["samples"])
    assert torch.equal(full["samples"], chunked["samples"])
    assert torch.equal(benchmark_output["samples"], full["samples"])
    primary_tensor = primary["samples"]
    assert isinstance(primary_tensor, torch.Tensor)
    expected = primary_tensor.float()
    if not captured:
        expected = expected + 0.25
    else:
        for index, source in enumerate(captured, start=1):
            source_tensor = source["samples"]
            assert isinstance(source_tensor, torch.Tensor)
            expected = expected + source_tensor.float() * (0.25 * index)
    assert torch.equal(benchmark_output["samples"], expected.half())
    assert benchmark_output["marker"] == "primary"


def test_hook_builder_requires_every_explicit_topology_source() -> None:
    installed = _install("carrier_donors", 4)
    _primary, captured = _sources(4)

    with pytest.raises(ToolkitContractError) as caught:
        build_installed_operator_research_hook(
            installed,
            captured_sources=captured[:2],
            controls={"amount": 0.25},
            seed=17,
        )

    assert caught.value.code == "operator_hook.source_count_invalid"
