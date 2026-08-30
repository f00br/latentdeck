from __future__ import annotations

import json

import pytest
import torch

import latentdeck_comfy_toolkit.device_transfer as device_transfer_module
from latentdeck_comfy_toolkit.decoder_compare import ToolkitContractError
from latentdeck_comfy_toolkit.device_nodes import (
    DEVICE_NODE_CLASS_MAPPINGS,
    DEVICE_NODE_DISPLAY_NAME_MAPPINGS,
)
from latentdeck_comfy_toolkit.device_transfer import transfer_latent_device
from latentdeck_comfy_toolkit.mixer_labs import dual_mixer_lab
from latentdeck_comfy_toolkit.research_evaluation import benchmark_operator


class NestedSamples:
    is_nested = True

    def __init__(self, streams: tuple[torch.Tensor, ...]) -> None:
        self._streams = streams

    def unbind(self) -> tuple[torch.Tensor, ...]:
        return self._streams

    def with_streams(self, streams: tuple[torch.Tensor, ...]) -> NestedSamples:
        return NestedSamples(streams)


def _visual() -> torch.Tensor:
    return torch.arange(24 * 2 * 3 * 4, dtype=torch.float32).reshape(1, 24, 2, 3, 4)


def test_explicit_cpu_transfer_preserves_shape_dtype_metadata_and_records_policy() -> None:
    metadata = {"identity": "public-synthetic"}
    latent = {"samples": _visual().half(), "metadata": metadata}

    result = transfer_latent_device(
        latent,
        target="CPU",
        cuda_index=0,
        cuda_unavailable_policy="ERROR",
    )

    assert isinstance(result.output, dict)
    output = result.output["samples"]
    assert isinstance(output, torch.Tensor)
    assert output.device.type == "cpu"
    assert output.dtype is torch.float16
    assert output.shape == latent["samples"].shape
    assert torch.equal(output, latent["samples"])
    assert result.output["metadata"] is metadata
    assert result.provenance["requested_device"] == "cpu"
    assert result.provenance["resolved_device"] == "cpu"
    assert result.provenance["fallback_used"] is False
    assert result.provenance["transfer_performed"] is False
    assert result.provenance["hidden_dtype_conversion"] is False


def test_explicit_transfer_moves_every_av_stream_together() -> None:
    visual = _visual().half()
    audio = torch.arange(32 * 2 * 7, dtype=torch.float32).reshape(1, 32, 2, 7).half()
    latent = {"samples": NestedSamples((visual, audio))}

    result = transfer_latent_device(
        latent,
        target="CPU",
        cuda_index=0,
        cuda_unavailable_policy="ERROR",
    )

    assert isinstance(result.output, dict)
    samples = result.output["samples"]
    assert isinstance(samples, NestedSamples)
    output_visual, output_audio = samples.unbind()
    assert output_visual.device.type == output_audio.device.type == "cpu"
    assert torch.equal(output_visual, visual)
    assert torch.equal(output_audio, audio)
    assert result.provenance["stream_count"] == 2
    assert result.provenance["byte_length"] == (
        visual.numel() * visual.element_size() + audio.numel() * audio.element_size()
    )


def test_cuda_absence_requires_an_explicit_error_or_cpu_fallback_policy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(torch.cuda, "is_available", lambda: False)
    latent = {"samples": _visual().half()}

    with pytest.raises(ToolkitContractError) as unavailable:
        transfer_latent_device(
            latent,
            target="CUDA",
            cuda_index=0,
            cuda_unavailable_policy="ERROR",
        )
    assert unavailable.value.code == "device.cuda_unavailable"

    fallback = transfer_latent_device(
        latent,
        target="CUDA",
        cuda_index=0,
        cuda_unavailable_policy="FALLBACK_TO_CPU",
    )
    assert fallback.provenance["requested_device"] == "cuda:0"
    assert fallback.provenance["resolved_device"] == "cpu"
    assert fallback.provenance["fallback_used"] is True
    assert fallback.provenance["fallback_reason"] == "cuda_unavailable"


def test_bad_device_controls_and_available_index_are_rejected_before_copy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    latent = {"samples": _visual().half()}
    cases = (
        (
            {"target": "AUTO", "cuda_index": 0, "cuda_unavailable_policy": "ERROR"},
            "device.target_invalid",
        ),
        (
            {"target": "CUDA", "cuda_index": True, "cuda_unavailable_policy": "ERROR"},
            "device.cuda_index_invalid",
        ),
        (
            {"target": "CUDA", "cuda_index": 0, "cuda_unavailable_policy": "SILENT"},
            "device.cuda_policy_invalid",
        ),
    )
    for controls, code in cases:
        with pytest.raises(ToolkitContractError) as caught:
            transfer_latent_device(latent, **controls)
        assert caught.value.code == code

    monkeypatch.setattr(torch.cuda, "is_available", lambda: True)
    monkeypatch.setattr(torch.cuda, "device_count", lambda: 1)
    with pytest.raises(ToolkitContractError) as index:
        transfer_latent_device(
            latent,
            target="CUDA",
            cuda_index=1,
            cuda_unavailable_policy="FALLBACK_TO_CPU",
        )
    assert index.value.code == "device.cuda_index_unavailable"


def test_transfer_byte_bound_and_cuda_query_failure_are_stable_errors(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    latent = {"samples": _visual().half()}
    monkeypatch.setattr(device_transfer_module, "MAX_DEVICE_TRANSFER_BYTES", 1)
    with pytest.raises(ToolkitContractError) as oversized:
        transfer_latent_device(
            latent,
            target="CPU",
            cuda_index=0,
            cuda_unavailable_policy="ERROR",
    )
    assert oversized.value.code == "device.transfer_bound"

    monkeypatch.setattr(
        device_transfer_module,
        "MAX_DEVICE_TRANSFER_BYTES",
        512 * 1024 * 1024,
    )
    monkeypatch.setattr(
        torch.cuda,
        "is_available",
        lambda: (_ for _ in ()).throw(RuntimeError("private driver detail")),
    )
    with pytest.raises(ToolkitContractError) as query:
        transfer_latent_device(
            latent,
            target="CUDA",
            cuda_index=0,
            cuda_unavailable_policy="FALLBACK_TO_CPU",
        )
    assert query.value.code == "device.cuda_query_failed"
    assert "private driver detail" not in query.value.detail


def test_comfy_node_is_visible_and_appends_a_device_operation_receipt() -> None:
    assert set(DEVICE_NODE_CLASS_MAPPINGS) == {"LatentDeckToolkitExplicitDeviceTransfer"}
    assert set(DEVICE_NODE_DISPLAY_NAME_MAPPINGS) == set(DEVICE_NODE_CLASS_MAPPINGS)
    node = DEVICE_NODE_CLASS_MAPPINGS["LatentDeckToolkitExplicitDeviceTransfer"]()
    latent = {"samples": _visual().half()}

    output, receipt_json = node.transfer(latent, "CPU", 0, "ERROR")

    receipt = json.loads(receipt_json)
    assert receipt["operation"]["operator_id"].endswith("explicit-device-transfer")
    assert output["latentdeck"]["operation_history"][-1]["controls"] == {
        "cuda_index": 0,
        "cuda_unavailable_policy": "ERROR",
        "target": "CPU",
    }


@pytest.mark.skipif(not torch.cuda.is_available(), reason="CUDA runtime is not available")
def test_explicit_cuda_staging_runs_xs5_and_reports_cuda_benchmark_memory() -> None:
    carrier = {"samples": _visual().half()}
    donor = {"samples": torch.cos(_visual()).half()}
    staged_carrier = transfer_latent_device(
        carrier,
        target="CUDA",
        cuda_index=0,
        cuda_unavailable_policy="ERROR",
    ).output
    staged_donor = transfer_latent_device(
        donor,
        target="CUDA",
        cuda_index=0,
        cuda_unavailable_policy="ERROR",
    ).output
    controls = {
        "mix": 0.5,
        "mode": "HYBRIDIZE",
        "routing": "A",
        "interaction": 0.7,
        "preserve": 0.55,
        "chaos": 0.0,
        "xs5_routing": "TOPK",
        "temperature": 0.12,
        "top_k": 4,
        "sinkhorn_iterations": 3,
    }

    def xs5(value: object) -> object:
        return dual_mixer_lab(
            value,
            staged_donor,
            operator="XS5",
            controls=controls,
            seed=7,
        ).output

    benchmark = benchmark_operator(
        xs5,
        staged_carrier,
        warmup_runs=0,
        measured_runs=1,
        streaming_compatible=True,
    )

    assert benchmark.report["device"] == "cuda"
    assert benchmark.report["vram_delta_bytes"] is not None
    assert benchmark.report["vram_peak_delta_bytes"] is not None
    assert benchmark.output["samples"].device == torch.device("cuda", 0)
