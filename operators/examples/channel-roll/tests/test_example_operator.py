from __future__ import annotations

import torch
from latentdeck_comfy_toolkit import OperatorContext, TrustedOperatorRegistry

from latentdeck_example_channel_roll import OPERATOR_ID, install_into


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
    first = loaded.process_slot(carrier, donor, controls, context)
    repeated = loaded.process_slot(carrier, donor, controls, context)

    assert torch.equal(first.output, repeated.output)
    assert not torch.equal(first.output, carrier)
    assert first.provenance["operation"] == {
        "operator_id": OPERATOR_ID,
        "operator_version": "0.1.0",
        "seed": 41,
        "controls": controls,
    }
    assert loaded.process_slot(carrier, donor, {**controls, "amount": 0.0}).output.equal(carrier)
