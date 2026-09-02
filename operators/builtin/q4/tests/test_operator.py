from __future__ import annotations

import pytest
import torch
from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding

from latentdeck_operator_q4 import Q4ContractError, process_sources, triangular_influence_weights


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
        playheads=(0, 0, 0, 0),
        physical_slots=(1, 2, 3, 4),
        roles=(
            RoleBinding("carrier", 1),
            RoleBinding("donor_b", 2),
            RoleBinding("donor_c", 3),
            RoleBinding("donor_d", 4),
        ),
        previous_sources=(None, None, None, None),
    )


def test_barycentric_vertex_is_an_exact_donor_through_deck_sdk() -> None:
    sources = tuple(torch.full((1, 8, 1, 2, 3), float(index)) for index in range(1, 5))
    result = process_sources(
        sources,
        {
            "algorithm": "linear",
            "interaction": 1.0,
            "influence_mode": "triangle",
            "triangle_x": 0.5,
            "triangle_y": 1.0,
        },
        _context(),
    )
    assert triangular_influence_weights(0.5, 1.0) == (0.0, 0.0, 1.0)
    assert torch.equal(result.output, sources[3])


def test_barycentric_controls_reject_outside_triangle_without_clamping() -> None:
    sources = tuple(torch.zeros((1, 8, 1, 2, 3)) for _ in range(4))
    with pytest.raises(Q4ContractError, match="control.outside_triangle"):
        process_sources(
            sources,
            {"influence_mode": "triangle", "triangle_x": 0.1, "triangle_y": 0.9},
            _context(),
        )
