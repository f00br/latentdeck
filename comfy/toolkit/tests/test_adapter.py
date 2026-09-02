from __future__ import annotations

import pytest
import torch
from latentdeck_deck_sdk import DeckOperatorContext, RoleBinding
from latentdeck_operator_d2 import D2ContractError, process_sources

from latentdeck_comfy_toolkit import process_xs_sequence


def synthetic_sequence() -> tuple[torch.Tensor, torch.Tensor]:
    index = torch.arange(2 * 24 * 3 * 4, dtype=torch.float32).reshape(2, 24, 3, 4)
    a = torch.sin(index * 0.071).permute(1, 0, 2, 3).unsqueeze(0).half()
    b = torch.cos(index * 0.043).permute(1, 0, 2, 3).unsqueeze(0).half()
    return a.contiguous(), b.contiguous()


def test_sequence_adapter_reuses_the_reviewed_d2_slot_contract() -> None:
    a, b = synthetic_sequence()
    controls = {
        "mix": 0.37,
        "interaction": 0.8,
        "preserve": 0.42,
        "xs2_radius": 1,
    }

    result = process_xs_sequence(
        a,
        b,
        algorithm="XS2",
        controls=controls,
        seed=91,
    )
    repeated = process_xs_sequence(
        a,
        b,
        algorithm="XS2",
        controls=controls,
        seed=91,
    )

    expected_slots = []
    for slot_index in range(a.shape[2]):
        expected_slots.append(
            process_sources(
                (
                    a[:, :, slot_index : slot_index + 1].contiguous(),
                    b[:, :, slot_index : slot_index + 1].contiguous(),
                ),
                {"algorithm": "XS2", **controls},
                DeckOperatorContext(
                    codec_family="minimax_h3",
                    profile="h3_av_latent",
                    profile_version="0.1.0",
                    timing_contract="minimax_h3_causal",
                    timing_contract_version="0.1.0",
                    frame_rate_numerator=24,
                    frame_rate_denominator=1,
                    generation=1,
                    sequence=slot_index + 1,
                    seed=91,
                    playheads=(slot_index, slot_index),
                    physical_slots=(1, 2),
                    roles=(RoleBinding("carrier", 1), RoleBinding("donor", 2)),
                    previous_sources=(
                        None
                        if slot_index == 0
                        else a[:, :, slot_index - 1 : slot_index].contiguous(),
                        None
                        if slot_index == 0
                        else b[:, :, slot_index - 1 : slot_index].contiguous(),
                    ),
                ),
            ).output
        )
    expected = torch.cat(expected_slots, dim=2)

    assert torch.equal(result.output, expected)
    assert torch.equal(result.output, repeated.output)
    assert result.output.shape == a.shape
    assert result.output.dtype == torch.float16
    assert result.provenance["operation"]["operator_id"] == "org.latentdeck.builtin.ld_d2"
    assert result.provenance["operation"]["algorithm"] == "XS2"
    assert result.provenance["sequence"]["slots"] == 2


def test_sequence_adapter_rejects_invalid_data_instead_of_converting_it() -> None:
    a, b = synthetic_sequence()
    damaged = a.clone()
    damaged[0, 0, 0, 0, 0] = float("nan")

    with pytest.raises(D2ContractError) as non_finite:
        process_xs_sequence(damaged, b, algorithm="XS1")
    assert non_finite.value.code == "tensor.non_finite"

    with pytest.raises(D2ContractError) as dtype:
        process_xs_sequence(a.float(), b.float(), algorithm="XS1")
    assert dtype.value.code == "tensor.dtype"
