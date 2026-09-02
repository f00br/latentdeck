from __future__ import annotations

import pytest
import torch
from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding

from latentdeck_operator_d2 import D2ContractError, process_sources


def _context() -> DeckOperatorContext:
    return DeckOperatorContext(
        codec_family="test",
        profile="generic",
        profile_version="1.0.0",
        timing_contract="ticks",
        timing_contract_version="1.0.0",
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        generation=1,
        sequence=1,
        seed=17,
        playheads=(0, 0),
        physical_slots=(1, 2),
        roles=(RoleBinding("carrier", 1), RoleBinding("donor", 2)),
        previous_sources=(None, None),
    )


def test_linear_endpoints_remain_exact_through_deck_sdk() -> None:
    index = torch.arange(96, dtype=torch.float32).reshape(1, 8, 1, 3, 4)
    sources = (torch.sin(index).contiguous(), torch.cos(index).contiguous())
    at_a = process_sources(sources, {"algorithm": "linear", "mix": 0.0}, _context())
    at_b = process_sources(sources, {"algorithm": "linear", "mix": 1.0}, _context())
    assert torch.equal(at_a.output, sources[0])
    assert torch.equal(at_b.output, sources[1])


def test_controls_reject_instead_of_clamping_or_falling_back() -> None:
    sources = tuple(torch.zeros((1, 8, 1, 3, 4)) for _ in range(2))
    with pytest.raises(D2ContractError, match="control.out_of_range"):
        process_sources(sources, {"mix": 1.01}, _context())
