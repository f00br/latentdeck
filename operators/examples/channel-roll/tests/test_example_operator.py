from __future__ import annotations

import json

import torch
from latentdeck_comfy_toolkit import OperatorContext, TrustedOperatorRegistry

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
    assert set(NODE_CLASS_MAPPINGS) == {"LatentDeckExampleChannelRoll"}
    assert set(NODE_DISPLAY_NAME_MAPPINGS) == set(NODE_CLASS_MAPPINGS)
    node = NODE_CLASS_MAPPINGS["LatentDeckExampleChannelRoll"]()
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
