from __future__ import annotations

import math

import pytest
import torch
from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding
from torch.utils._python_dispatch import TorchDispatchMode

from latentdeck_operator_d2 import D2ContractError, process_sources
from latentdeck_operator_d2.contract import Algorithm, D2Controls
from latentdeck_operator_d2.operator import _xs1


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


@pytest.mark.parametrize("dtype", [torch.float16, torch.float32])
def test_xs1_uses_one_f32_materialization_without_mutating_the_donor(
    dtype: torch.dtype,
) -> None:
    donor = torch.linspace(-2.0, 2.0, 96, dtype=torch.float32).reshape(1, 8, 1, 3, 4).to(dtype)
    original = donor.clone()
    controls = D2Controls(
        algorithm=Algorithm.XS1,
        xs1_channel_a=1,
        xs1_channel_b=6,
        xs1_angle_degrees=37.0,
    )

    recorder = _OperationRecorder()
    with recorder:
        result = _xs1(donor, controls)

    angle = math.radians(controls.xs1_angle_degrees)
    cosine = math.cos(angle)
    sine = math.sin(angle)
    expected = original.float().clone()
    first = original[:, controls.xs1_channel_a].float()
    second = original[:, controls.xs1_channel_b].float()
    expected[:, controls.xs1_channel_a] = cosine * first - sine * second
    expected[:, controls.xs1_channel_b] = sine * first + cosine * second

    assert torch.equal(donor, original)
    assert torch.equal(result, expected)
    assert recorder.operations.count("aten._to_copy.default") == 1
    assert "aten.clone.default" not in recorder.operations
