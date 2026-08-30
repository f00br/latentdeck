from __future__ import annotations

import json
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
from types import ModuleType

import torch

from latentdeck_comfy_toolkit import validate_external_descriptor
from latentdeck_comfy_toolkit.research_nodes import (
    LatentDeckToolkitDeterminismTest,
    LatentDeckToolkitOperatorBenchmark,
    LatentDeckToolkitStreamingCompatibilityTest,
)


def load_template() -> ModuleType:
    path = Path(__file__).parents[1] / "templates" / "MyLatentOperator.py"
    spec = spec_from_file_location("latentdeck_test_my_latent_operator", path)
    assert spec is not None and spec.loader is not None
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def synthetic_pair() -> tuple[dict[str, object], dict[str, object]]:
    index = torch.arange(24 * 3 * 3 * 4, dtype=torch.float32).reshape(1, 24, 3, 3, 4)
    carrier = {
        "samples": torch.sin(index * 0.03).half(),
        "marker": "carrier metadata survives",
    }
    donor = {"samples": torch.cos(index * 0.05).half()}
    return carrier, donor


def test_single_file_template_is_directly_discoverable_and_contract_valid() -> None:
    module = load_template()

    assert set(module.NODE_CLASS_MAPPINGS) == {
        "MyLatentOperator",
        "MyLatentOperatorTestHook",
    }
    assert set(module.NODE_CLASS_MAPPINGS) == set(module.NODE_DISPLAY_NAME_MAPPINGS)
    descriptor = validate_external_descriptor(module.DESCRIPTOR)
    assert descriptor.operator_id == "org.example.my_latent_operator"
    assert descriptor.entrypoint == "MyLatentOperator:process_sources"
    assert descriptor.topology == "dual_source"
    assert descriptor.input_count == 2
    assert descriptor.capabilities.deterministic is True
    assert module._installed_operator().descriptor == descriptor


def test_single_file_process_node_has_exact_bypass_and_preserves_metadata() -> None:
    module = load_template()
    carrier, donor = synthetic_pair()
    node = module.NODE_CLASS_MAPPINGS["MyLatentOperator"]()

    output, receipt_json = node.process(carrier, donor, 0.75, 41)
    bypass, _ = node.process(carrier, donor, 0.0, 41)

    assert output["samples"].shape == carrier["samples"].shape
    assert output["samples"].dtype == torch.float16
    assert output["marker"] == "carrier metadata survives"
    assert not torch.equal(output["samples"], carrier["samples"])
    assert torch.equal(bypass["samples"], carrier["samples"])
    receipt = json.loads(receipt_json)
    assert receipt["operator_id"] == "org.example.my_latent_operator"
    assert receipt["topology"] == "dual_source"
    assert receipt["controls"] == {"amount": 0.75}
    assert receipt["seed"] == 41


def test_single_file_test_hook_runs_all_toolkit_evaluations() -> None:
    module = load_template()
    carrier, donor = synthetic_pair()
    hook_node = module.NODE_CLASS_MAPPINGS["MyLatentOperatorTestHook"]()
    (hook,) = hook_node.build(donor, 0.75, 41)

    benchmark_output, benchmark_json = LatentDeckToolkitOperatorBenchmark().run(carrier, hook, 0, 2)
    deterministic_output, deterministic_json = LatentDeckToolkitDeterminismTest().run(
        carrier, hook, 3
    )
    full, chunked, streaming_json = LatentDeckToolkitStreamingCompatibilityTest().run(
        carrier, hook, 1, 0.0, 0.0
    )

    assert benchmark_output["samples"].shape == carrier["samples"].shape
    assert json.loads(benchmark_json)["operator"]["topology"] == "dual_source"
    assert deterministic_output["samples"].shape == carrier["samples"].shape
    assert json.loads(deterministic_json)["deterministic"] is True
    assert torch.equal(full["samples"], chunked["samples"])
    assert json.loads(streaming_json)["compatible"] is True
