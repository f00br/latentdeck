from __future__ import annotations

import torch

from latentdeck_comfy_toolkit.research_evaluation import (
    benchmark_operator,
    evaluate_determinism,
    evaluate_streaming_compatibility,
    latent_scopes,
)
from latentdeck_comfy_toolkit.research_ops import ResearchResult


def test_latent_scopes_reports_finite_statistics_and_non_finite_counts_json_safely() -> None:
    latent = torch.arange(24 * 2 * 2 * 2, dtype=torch.float32).reshape(1, 24, 2, 2, 2)
    latent[0, 0, 0, 0, 0] = float("nan")
    latent[0, 1, 1, 1, 1] = float("inf")

    report = latent_scopes(latent)

    assert report["shape"] == [1, 24, 2, 2, 2]
    assert report["finite"] is False
    assert report["nan_count"] == 1
    assert report["positive_inf_count"] == 1
    assert report["negative_inf_count"] == 0
    assert report["channel_energy"][0] is None
    assert report["channel_energy"][1] is None
    assert len(report["channel_energy"]) == 24
    assert len(report["temporal_energy"]) == 2
    assert isinstance(report["mean"], float)


def test_operator_benchmark_reports_time_shape_and_cpu_vram_semantics() -> None:
    latent = torch.ones((1, 24, 2, 2, 2), dtype=torch.float32)

    def double(value: object) -> ResearchResult:
        assert isinstance(value, torch.Tensor)
        return ResearchResult(value * 2, {"operation": "DOUBLE"})

    benchmark = benchmark_operator(
        double,
        latent,
        warmup_runs=1,
        measured_runs=3,
        streaming_compatible=True,
    )

    assert torch.equal(benchmark.output, latent * 2)
    assert benchmark.report["shape"] == [1, 24, 2, 2, 2]
    assert benchmark.report["execution_ms"]["runs"] == 3
    assert benchmark.report["execution_ms"]["mean"] >= 0.0
    assert benchmark.report["vram_delta_bytes"] is None
    assert benchmark.report["streaming_compatible"] is True


def test_determinism_evaluation_detects_exact_repeatability_and_divergence() -> None:
    latent = torch.ones((1, 24, 2, 2, 2), dtype=torch.float32)
    stable = evaluate_determinism(lambda value: value * 2, latent, runs=3)
    calls = 0

    def changing(value: object) -> object:
        nonlocal calls
        calls += 1
        assert isinstance(value, torch.Tensor)
        return value + calls

    unstable = evaluate_determinism(changing, latent, runs=3)

    assert stable.report["deterministic"] is True
    assert stable.report["mismatch_runs"] == []
    assert unstable.report["deterministic"] is False
    assert unstable.report["mismatch_runs"] == [1, 2]
    assert unstable.report["max_abs_difference"] == 2.0


def test_streaming_evaluation_compares_full_clip_with_stateful_chunks() -> None:
    latent = torch.arange(24 * 5 * 2 * 2, dtype=torch.float32).reshape(1, 24, 5, 2, 2)

    compatible = evaluate_streaming_compatibility(
        lambda value: value * 2,
        lambda chunk, state, offset: (chunk * 2, state),
        latent,
        chunk_slots=2,
    )
    incompatible = evaluate_streaming_compatibility(
        lambda value: value + value.mean(dim=2, keepdim=True),
        lambda chunk, state, offset: (chunk + chunk.mean(dim=2, keepdim=True), state),
        latent,
        chunk_slots=2,
    )

    assert compatible.report["compatible"] is True
    assert torch.equal(compatible.full_output, compatible.streamed_output)
    assert compatible.report["chunks"] == 3
    assert incompatible.report["compatible"] is False
    assert incompatible.report["max_abs_difference"] > 0.0
