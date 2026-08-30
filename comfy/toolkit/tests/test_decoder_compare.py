from __future__ import annotations

import math

import pytest
import torch

from latentdeck_comfy_toolkit import (
    MAX_DECODED_VALUES,
    DecoderHook,
    ToolkitContractError,
    compare_decoder_hooks,
)


def test_decoder_comparison_uses_only_explicit_hooks_and_assets() -> None:
    latent = torch.linspace(-1.0, 1.0, 24, dtype=torch.float32).reshape(1, 3, 2, 2, 2)
    fast_asset = object()
    hq_asset = object()
    observed_assets: list[object] = []

    def fast_decoder(value: torch.Tensor, asset: object) -> torch.Tensor:
        observed_assets.append(asset)
        return value * 0.75

    def hq_decoder(value: torch.Tensor, asset: object) -> torch.Tensor:
        observed_assets.append(asset)
        return value * 0.8 + 0.01

    comparison = compare_decoder_hooks(
        latent,
        DecoderHook(
            decoder_id="org.example.fast",
            decoder_version="1.2.3",
            decode=fast_decoder,
            asset=fast_asset,
            asset_sha256="1" * 64,
        ),
        DecoderHook(
            decoder_id="org.example.hq",
            decoder_version="2.0.0",
            decode=hq_decoder,
            asset=hq_asset,
            asset_sha256="2" * 64,
        ),
    )

    assert observed_assets == [fast_asset, hq_asset]
    assert torch.equal(comparison.fast_output, latent * 0.75)
    assert torch.equal(comparison.hq_output, latent * 0.8 + 0.01)
    assert comparison.metrics["mean_absolute_error"] > 0.0
    assert math.isfinite(comparison.metrics["root_mean_squared_error"])
    assert comparison.provenance["fast"] == {
        "decoder_id": "org.example.fast",
        "decoder_version": "1.2.3",
        "asset_sha256": "1" * 64,
    }
    assert comparison.provenance["hq"]["asset_sha256"] == "2" * 64


def test_decoder_asset_requires_an_explicit_provenance_hash() -> None:
    latent = torch.zeros((1, 1, 1, 1), dtype=torch.float32)

    with pytest.raises(ToolkitContractError) as caught:
        compare_decoder_hooks(
            latent,
            DecoderHook("org.example.fast", "1.0.0", lambda value, _asset: value, object()),
            DecoderHook("org.example.hq", "1.0.0", lambda value, _asset: value),
        )

    assert caught.value.code == "decoder.asset_hash_required"


def test_decoder_outputs_must_match_and_failures_are_path_free() -> None:
    latent = torch.zeros((1, 1, 2, 2), dtype=torch.float32)

    with pytest.raises(ToolkitContractError) as incompatible:
        compare_decoder_hooks(
            latent,
            DecoderHook("org.example.fast", "1.0.0", lambda value, _asset: value),
            DecoderHook(
                "org.example.hq",
                "1.0.0",
                lambda value, _asset: value[..., :1].contiguous(),
            ),
        )
    assert incompatible.value.code == "decoder.output_incompatible"

    def fail(_value: torch.Tensor, _asset: object) -> torch.Tensor:
        raise RuntimeError("private-machine-path")

    with pytest.raises(ToolkitContractError) as failed:
        compare_decoder_hooks(
            latent,
            DecoderHook("org.example.fast", "1.0.0", fail),
            DecoderHook("org.example.hq", "1.0.0", lambda value, _asset: value),
        )
    assert failed.value.code == "decoder.execution_failed"
    assert "private-machine-path" not in failed.value.detail


def test_hooks_receive_isolated_latent_snapshots() -> None:
    latent = torch.linspace(-1.0, 1.0, 24, dtype=torch.float32).reshape(1, 3, 2, 2, 2)
    original = latent.clone()
    hq_observation: list[torch.Tensor] = []

    def mutating_fast(value: torch.Tensor, _asset: object) -> torch.Tensor:
        value.zero_()
        return value

    def observing_hq(value: torch.Tensor, _asset: object) -> torch.Tensor:
        hq_observation.append(value.clone())
        return value

    compare_decoder_hooks(
        latent,
        DecoderHook("org.example.fast", "1.0.0", mutating_fast),
        DecoderHook("org.example.hq", "1.0.0", observing_hq),
    )

    assert torch.equal(latent, original)
    assert torch.equal(hq_observation[0], original)


def test_decoded_bound_covers_the_long_release_profile_rgb_case() -> None:
    assert MAX_DECODED_VALUES >= 243 * 448 * 800 * 4


def test_float64_decoder_outputs_are_rejected_before_metrics() -> None:
    latent = torch.zeros((1, 1, 2, 2), dtype=torch.float32)
    extreme = torch.full_like(latent, torch.finfo(torch.float64).max, dtype=torch.float64)

    with pytest.raises(ToolkitContractError) as caught:
        compare_decoder_hooks(
            latent,
            DecoderHook("org.example.fast", "1.0.0", lambda _value, _asset: extreme),
            DecoderHook("org.example.hq", "1.0.0", lambda _value, _asset: -extreme),
        )

    assert caught.value.code == "decoder.tensor_dtype"
