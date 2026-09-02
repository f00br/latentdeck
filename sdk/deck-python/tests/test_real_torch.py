from __future__ import annotations

import pytest

from latentdeck_deck_sdk import (
    DeckContractError,
    DeckOperatorContext,
    RoleBinding,
    validate_process_call,
)

torch = pytest.importorskip("torch")


def context() -> DeckOperatorContext:
    return DeckOperatorContext(
        codec_family="synthetic",
        profile="test_latent",
        profile_version="0.1.0",
        timing_contract="synthetic_causal",
        timing_contract_version="0.1.0",
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        generation=1,
        sequence=1,
        seed=0,
        playheads=(0, 0),
        physical_slots=(1, 2),
        roles=(RoleBinding("carrier", 1), RoleBinding("donor", 2)),
        previous_sources=(None, None),
    )


def test_tiny_cpu_tensors_use_the_real_torch_contract() -> None:
    sources = (
        torch.zeros((1, 2, 1, 2, 2), dtype=torch.float16),
        torch.ones((1, 2, 1, 2, 2), dtype=torch.float16),
    )
    assert validate_process_call(sources, {}, context()) == {}


def test_real_torch_non_finite_input_is_rejected() -> None:
    source = torch.zeros((1, 2, 1, 2, 2), dtype=torch.float16)
    source[0, 0, 0, 0, 0] = float("nan")
    with pytest.raises(DeckContractError, match="tensor.non_finite"):
        validate_process_call((source, source.clone()), {}, context())
