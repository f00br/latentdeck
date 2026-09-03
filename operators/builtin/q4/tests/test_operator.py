from __future__ import annotations

import pytest
import torch
from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding
from torch.utils._python_dispatch import TorchDispatchMode

from latentdeck_operator_q4 import Q4ContractError, process_sources, triangular_influence_weights
from latentdeck_operator_q4.contract import Algorithm, ArtisticMode, Q4Controls
from latentdeck_operator_q4.operator import _accumulate_routed


class _OperationRecorder(TorchDispatchMode):
    def __init__(self) -> None:
        self.operations: list[str] = []

    def __torch_dispatch__(
        self,
        function: object,
        types: tuple[type, ...],
        args: tuple[object, ...] = (),
        kwargs: dict[str, object] | None = None,
    ) -> object:
        del types
        self.operations.append(str(function))
        return function(*args, **(kwargs or {}))  # type: ignore[operator]


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


def test_xs5_accumulator_converts_the_carrier_to_f32_once() -> None:
    index = torch.arange(96, dtype=torch.float32).reshape(1, 8, 1, 3, 4)
    carrier = torch.sin(index * 0.07).half()
    donors = tuple(torch.cos(index * scale).half() for scale in (0.03, 0.05, 0.09))
    routed = torch.stack([donor.squeeze(0).float() for donor in donors])
    weights = (0.2, 0.3, 0.5)
    controls = Q4Controls(
        algorithm=Algorithm.XS5,
        interaction=0.8,
        mode=ArtisticMode.HYBRIDIZE,
        preserve=0.35,
    )

    recorder = _OperationRecorder()
    with recorder:
        result = _accumulate_routed(carrier, donors, routed, weights, controls)

    expected = carrier.float().clone()
    structural = carrier.float()
    for donor_index in range(3):
        routed_donor = routed[donor_index].unsqueeze(0)
        target = controls.preserve * structural + (1.0 - controls.preserve) * routed_donor
        expected.add_(
            target - structural,
            alpha=controls.interaction * weights[donor_index],
        )

    assert torch.equal(result, expected)
    # One carrier conversion plus one for each of the three donors. The former
    # implementation converted the complete carrier twice.
    assert recorder.operations.count("aten._to_copy.default") == 4
