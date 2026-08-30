from __future__ import annotations

import torch

from latentdeck_comfy_toolkit.compatibility import check_h3_compatibility


def _latent(temporal: int, *, height: int = 3, width: int = 4) -> dict[str, object]:
    shape = [1, 24, temporal, height, width]
    return {
        "samples": torch.zeros(shape, dtype=torch.float16),
        "latentdeck": {
            "source_kind": "latent_cartridge",
            "manifest": {
                "codec": {
                    "family": "minimax_h3",
                    "profile": "h3_av_latent",
                    "profile_version": "0.1.0",
                },
                "tensors": [
                    {
                        "stream": "visual",
                        "name": "video",
                        "runtime_dtype": "F16",
                        "shape": shape,
                    }
                ],
                "timing": {
                    "contract": "minimax_h3_causal",
                    "contract_version": "0.1.0",
                    "decoded_video": {"frame_rate": {"numerator": 24, "denominator": 1}},
                },
            },
        },
    }


def test_compatibility_requires_explicit_temporal_alignment() -> None:
    report = check_h3_compatibility([_latent(32), _latent(72)])

    assert report["compatible"] is False
    assert report["mismatches"] == [
        {
            "input_index": 1,
            "field": "temporal_slots",
            "reference": 32,
            "actual": 72,
        }
    ]
    assert report["inputs"][0]["temporal_slots"] == 32
    assert report["inputs"][1]["temporal_slots"] == 72
    assert report["compatibility_key"]["temporal_slots"] == 32
    assert report["conversion_performed"] is False


def test_compatibility_reports_each_mismatched_contract_field_without_conversion() -> None:
    report = check_h3_compatibility([_latent(32, height=3), _latent(32, height=5)])

    assert report["compatible"] is False
    assert report["mismatches"] == [
        {
            "input_index": 1,
            "field": "latent_height",
            "reference": 3,
            "actual": 5,
        }
    ]
    assert report["conversion_performed"] is False
