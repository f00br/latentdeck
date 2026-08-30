from __future__ import annotations

import pytest
import torch

from latentdeck_comfy_toolkit.alignment import align_h3_pair, crop_h3_latent
from latentdeck_comfy_toolkit.cartridge_io import H3AVSamples, ToolkitIOError


def _latent(video: torch.Tensor, samples: object | None = None) -> dict[str, object]:
    return {
        "samples": video if samples is None else samples,
        "latentdeck": {
            "source_kind": "raw_h3_safetensors",
            "profile": {
                "codec_family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_version": "0.1.0",
            },
            "operation_chain": [],
        },
    }


def test_explicit_crop_materializes_only_the_selected_h3_region() -> None:
    video = torch.arange(24 * 7 * 4 * 5, dtype=torch.float16).reshape(1, 24, 7, 4, 5)

    result = crop_h3_latent(
        _latent(video),
        temporal_start=0,
        temporal_slots=2,
        spatial_top=1,
        spatial_left=2,
        spatial_height=2,
        spatial_width=3,
        audio_policy="PRESERVE_EXACT",
    )

    output = result.latent["samples"]
    assert torch.equal(output, video[:, :, 0:2, 1:3, 2:5])
    assert output.is_contiguous()
    assert result.report["before_shape"] == [1, 24, 7, 4, 5]
    assert result.report["after_shape"] == [1, 24, 2, 2, 3]
    assert result.report["conversion"] == {
        "crop": "explicit",
        "resize": False,
        "reencode": False,
        "dtype_cast": False,
        "materialized_contiguous": True,
    }
    assert result.latent["latentdeck"]["operation_chain"][-1] == result.report


def test_temporal_crop_requires_a_visible_audio_drop_policy() -> None:
    video = torch.zeros((1, 24, 7, 2, 2), dtype=torch.float16)
    audio = torch.zeros((1, 32, 2, 37), dtype=torch.float32)
    av = _latent(video, H3AVSamples((video, audio)))
    controls = {
        "temporal_start": 0,
        "temporal_slots": 2,
        "spatial_top": 0,
        "spatial_left": 0,
        "spatial_height": 2,
        "spatial_width": 2,
    }

    with pytest.raises(ToolkitIOError) as caught:
        crop_h3_latent(av, **controls, audio_policy="PRESERVE_EXACT")
    assert caught.value.code == "align.audio_timing_changed"

    dropped = crop_h3_latent(av, **controls, audio_policy="DROP_EXPLICIT")
    assert isinstance(dropped.latent["samples"], torch.Tensor)
    assert dropped.report["audio_action"] == "dropped_explicitly"


def test_pair_align_uses_only_the_selected_visible_crop_policies() -> None:
    first = torch.arange(24 * 7 * 4 * 5, dtype=torch.float16).reshape(1, 24, 7, 4, 5)
    second = torch.arange(24 * 2 * 2 * 3, dtype=torch.float16).reshape(1, 24, 2, 2, 3)

    result = align_h3_pair(
        _latent(first),
        _latent(second),
        temporal_policy="CROP_END_TO_SHORTEST",
        spatial_policy="CENTER_TO_SMALLEST",
        audio_policy="PRESERVE_EXACT",
    )

    assert torch.equal(result.latent_a["samples"], first[:, :, :2, 1:3, 1:4])
    assert torch.equal(result.latent_b["samples"], second)
    assert result.report["target_shape"] == [1, 24, 2, 2, 3]
    assert result.report["policies"] == {
        "temporal": "CROP_END_TO_SHORTEST",
        "spatial": "CENTER_TO_SMALLEST",
        "audio": "PRESERVE_EXACT",
    }
    assert result.report["conversion_performed"] == "explicit_crop_only"
    assert result.report["compatibility"]["compatible"] is True
