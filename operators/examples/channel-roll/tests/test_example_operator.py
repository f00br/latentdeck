from __future__ import annotations

import json
from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path

import torch
from latentdeck_comfy_toolkit import OperatorContext, TrustedOperatorRegistry
from latentdeck_comfy_toolkit.research_nodes import (
    LatentDeckToolkitDeterminismTest,
    LatentDeckToolkitOperatorBenchmark,
    LatentDeckToolkitStreamingCompatibilityTest,
)

from latentdeck_example_channel_roll import (
    NODE_CLASS_MAPPINGS,
    NODE_DISPLAY_NAME_MAPPINGS,
    OPERATOR_ID,
    install_into,
)


def synthetic_pair() -> tuple[torch.Tensor, torch.Tensor]:
    index = torch.arange(24 * 3 * 4, dtype=torch.float32).reshape(1, 24, 1, 3, 4)
    return torch.sin(index * 0.03).half(), torch.cos(index * 0.05).half()


def test_example_is_inert_until_explicitly_installed_then_runs_deterministically() -> None:
    registry = TrustedOperatorRegistry()
    assert registry.descriptors() == ()

    install_into(registry)
    loaded = registry.load(OPERATOR_ID, "0.1.0")
    carrier, donor = synthetic_pair()
    controls = {"amount": 0.75, "channel_shift": 3}
    context = OperatorContext(seed=41, slot_index=8)
    first = loaded.process_dual(carrier, donor, controls, context)
    repeated = loaded.process_dual(carrier, donor, controls, context)

    assert torch.equal(first.output, repeated.output)
    assert not torch.equal(first.output, carrier)
    assert first.provenance["operation"] == {
        "operator_id": OPERATOR_ID,
        "operator_version": "0.1.0",
        "seed": 41,
        "controls": controls,
    }
    assert loaded.descriptor.topology == "dual_source"
    assert loaded.descriptor.capabilities.deterministic is True
    assert loaded.process_slot(carrier, donor, {**controls, "amount": 0.0}).output.equal(carrier)


def test_copyable_example_exports_a_working_comfy_node_for_full_h3_sequences() -> None:
    assert set(NODE_CLASS_MAPPINGS) == {
        "LatentDeckExampleChannelRoll",
        "LatentDeckExampleChannelRollHook",
    }
    assert set(NODE_DISPLAY_NAME_MAPPINGS) == set(NODE_CLASS_MAPPINGS)
    process_node = NODE_CLASS_MAPPINGS["LatentDeckExampleChannelRoll"]
    hook_node = NODE_CLASS_MAPPINGS["LatentDeckExampleChannelRollHook"]
    assert process_node.RETURN_TYPES == ("LATENT", "STRING")
    assert hook_node.RETURN_TYPES == ("LATENTDECK_OPERATOR_HOOK",)
    assert process_node.CATEGORY == hook_node.CATEGORY == "LatentDeck/Examples"
    node = process_node()
    carrier_slot, donor_slot = synthetic_pair()
    carrier = {
        "samples": carrier_slot.repeat(1, 1, 3, 1, 1),
        "marker": "carrier metadata survives",
    }
    donor = {"samples": donor_slot.repeat(1, 1, 3, 1, 1)}

    output, receipt_json = node.process(carrier, donor, 0.75, 3, 41)

    assert output["samples"].shape == (1, 24, 3, 3, 4)
    assert output["samples"].dtype == torch.float16
    assert output["marker"] == "carrier metadata survives"
    receipt = json.loads(receipt_json)
    assert receipt["operation"]["operator_id"] == OPERATOR_ID
    assert receipt["sequence"]["slots"] == 3
    assert receipt["sequence"]["processing"] == "ORDERED_SLOT_CALLS"
    bypass, _ = node.process(carrier, donor, 0.0, 3, 41)
    assert torch.equal(bypass["samples"], carrier["samples"])


def test_external_hook_connects_to_benchmark_determinism_and_streaming_nodes() -> None:
    carrier_slot, donor_slot = synthetic_pair()
    carrier = {"samples": carrier_slot.repeat(1, 1, 3, 1, 1)}
    donor = {"samples": donor_slot.repeat(1, 1, 3, 1, 1)}
    hook_node = NODE_CLASS_MAPPINGS["LatentDeckExampleChannelRollHook"]()
    (hook,) = hook_node.build(donor, 0.75, 3, 41)

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


def test_repository_folder_is_directly_discoverable_by_comfyui() -> None:
    root = Path(__file__).parents[1]
    spec = spec_from_file_location(
        "latentdeck_example_channel_roll_dropin",
        root / "__init__.py",
        submodule_search_locations=[str(root)],
    )
    assert spec is not None and spec.loader is not None
    module = module_from_spec(spec)
    spec.loader.exec_module(module)

    assert set(module.NODE_CLASS_MAPPINGS) == set(NODE_CLASS_MAPPINGS)
    assert set(module.NODE_DISPLAY_NAME_MAPPINGS) == set(NODE_DISPLAY_NAME_MAPPINGS)
