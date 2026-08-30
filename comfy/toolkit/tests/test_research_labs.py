from __future__ import annotations

import torch

from latentdeck_comfy_toolkit.research_labs import (
    OperatorStep,
    channel_lab,
    channel_rotation_matrix,
    feedback_lab,
    run_operator_chain,
    temporal_lab,
)


def test_temporal_lab_applies_visible_crop_reverse_offset_and_loop_in_order() -> None:
    source = torch.arange(4, dtype=torch.float32).reshape(1, 1, 4, 1, 1)
    source = source.expand(1, 24, 4, 2, 2).contiguous()

    result = temporal_lab(
        source,
        crop_start=1,
        crop_length=2,
        reverse=True,
        offset=1,
        loop_count=2,
    )

    assert result.output[0, 0, :, 0, 0].tolist() == [1.0, 2.0, 1.0, 2.0]
    assert result.output.shape == source.shape
    assert result.output.dtype == source.dtype
    assert result.provenance["parameters"]["order"] == ["CROP", "REVERSE", "OFFSET", "LOOP"]
    assert result.provenance["parameters"]["audio_policy"] == "NONE"


def test_feedback_lab_is_causal_and_iteration_bounded_without_wraparound() -> None:
    source = torch.zeros((1, 24, 4, 1, 1), dtype=torch.float32)
    source[:, :, 0] = 1.0

    result = feedback_lab(source, amount=0.5, delay=1, iterations=1)

    assert result.output[0, 0, :, 0, 0].tolist() == [1.0, 0.5, 0.0, 0.0]
    assert result.output.dtype == source.dtype
    assert result.provenance["parameters"]["causal"] is True
    assert result.provenance["parameters"]["iterations"] == 1


def test_channel_lab_applies_an_explicit_24_by_24_rotation_matrix() -> None:
    source = torch.zeros((1, 24, 1, 2, 2), dtype=torch.float32)
    source[:, 0] = 1.0
    source[:, 1] = 2.0
    matrix = channel_rotation_matrix(0, 1, angle_degrees=90.0)

    result = channel_lab(source, matrix=matrix, strength=1.0)

    assert torch.allclose(result.output[:, 0], torch.full_like(source[:, 0], -2.0), atol=1e-6)
    assert torch.allclose(result.output[:, 1], torch.full_like(source[:, 1], 1.0), atol=1e-6)
    assert torch.equal(result.output[:, 2:], source[:, 2:])
    assert result.provenance["parameters"]["matrix_shape"] == [24, 24]


def test_operator_chain_preserves_explicit_step_order_and_provenance() -> None:
    source = torch.zeros((1, 24, 3, 1, 1), dtype=torch.float32)
    source[:, 0, 0] = 1.0
    matrix = channel_rotation_matrix(0, 1, angle_degrees=90.0)
    steps = (
        OperatorStep("channel-rotate", lambda value: channel_lab(value, matrix=matrix)),
        OperatorStep("bounded-feedback", lambda value: feedback_lab(value, amount=0.5)),
    )

    result = run_operator_chain(source, steps)

    expected = feedback_lab(channel_lab(source, matrix=matrix).output, amount=0.5).output
    assert torch.equal(result.output, expected)
    assert [step["name"] for step in result.provenance["chain"]] == [
        "channel-rotate",
        "bounded-feedback",
    ]
    assert result.provenance["parameters"]["step_count"] == 2
