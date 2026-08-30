from __future__ import annotations

import pytest
import torch

from latentdeck_comfy_toolkit.adapter import process_xs_sequence
from latentdeck_comfy_toolkit.decoder_compare import ToolkitContractError
from latentdeck_comfy_toolkit.research_ops import (
    visual_latent,
    xs1_channel_mixer,
    xs2_spatial_graft,
    xs3_frequency_cross_synthesis,
    xs4_statistics_transfer,
    xs5_affinity_transport,
)


class NestedStreams:
    is_nested = True

    def __init__(self, *streams: torch.Tensor) -> None:
        self._streams = streams

    def unbind(self) -> tuple[torch.Tensor, ...]:
        return self._streams


def visual(seed: int = 0, *, slots: int = 3) -> torch.Tensor:
    generator = torch.Generator().manual_seed(seed)
    return torch.randn((1, 24, slots, 4, 5), generator=generator, dtype=torch.float16)


def test_xs1_mixes_channels_without_dropping_av_audio() -> None:
    carrier = visual(1)
    donor = visual(2)
    audio = torch.randn((1, 32, 2, 18), dtype=torch.float16)
    latent = {"samples": NestedStreams(carrier, audio), "batch_index": [4]}

    result = xs1_channel_mixer(latent, donor, channel_mix=[0.0] * 12 + [1.0] * 12)

    output_streams = result.output["samples"].unbind()
    assert torch.equal(output_streams[0][:, :12], carrier[:, :12])
    assert torch.equal(output_streams[0][:, 12:], donor[:, 12:])
    assert output_streams[0].shape == carrier.shape
    assert output_streams[0].dtype == carrier.dtype
    assert output_streams[1] is audio
    assert result.output["batch_index"] == [4]
    assert result.provenance["operation"] == "XS1_CHANNEL_MIXER"
    assert result.provenance["full_grid"] is True


def test_visual_latent_is_a_public_av_safe_extract_and_repack_hook() -> None:
    source = visual(21)
    audio = torch.randn((1, 32, 2, 18), dtype=torch.float16)
    latent = {"samples": NestedStreams(source, audio), "tag": "keep"}

    surface = visual_latent(latent)
    replacement = source + 1
    output = surface.repack(replacement)

    assert surface.visual is source
    assert surface.audio == (audio,)
    assert output["samples"].unbind()[0] is replacement
    assert output["samples"].unbind()[1] is audio
    assert output["tag"] == "keep"


def test_visual_latent_rejects_a_malformed_audio_stream_before_an_operator_runs() -> None:
    malformed_audio = torch.zeros((1, 31, 2, 18), dtype=torch.float16)

    with pytest.raises(ToolkitContractError) as error:
        visual_latent({"samples": NestedStreams(visual(22), malformed_audio)})

    assert error.value.code == "tensor.audio_shape"


def test_xs2_grafts_the_donor_only_where_the_explicit_grid_mask_selects() -> None:
    carrier = torch.zeros((1, 24, 2, 3, 4), dtype=torch.float32)
    donor = torch.ones_like(carrier)
    mask = torch.zeros((2, 3, 4), dtype=torch.float32)
    mask[:, :, 2:] = 1.0

    result = xs2_spatial_graft(carrier, donor, mask=mask)

    assert torch.equal(result.output[..., :2], carrier[..., :2])
    assert torch.equal(result.output[..., 2:], donor[..., 2:])
    assert result.output.shape == carrier.shape
    assert result.output.dtype == carrier.dtype
    assert result.provenance["parameters"]["mask_shape"] == [2, 3, 4]


def test_xs3_routes_low_frequency_donor_content_through_an_fft_mask() -> None:
    carrier = torch.zeros((1, 24, 1, 8, 8), dtype=torch.float32)
    donor = torch.full_like(carrier, 3.0)

    result = xs3_frequency_cross_synthesis(
        carrier,
        donor,
        cutoff=0.25,
        donor_band="LOW",
        strength=1.0,
    )

    assert torch.allclose(result.output, donor, atol=1e-6, rtol=0.0)
    assert result.output.shape == carrier.shape
    assert result.output.dtype == carrier.dtype
    assert result.provenance["operation"] == "XS3_FREQUENCY_CROSS_SYNTHESIS"
    assert result.provenance["parameters"]["domain"] == "FFT2_SPATIAL"


def test_xs4_transfers_donor_spatial_mean_and_standard_deviation() -> None:
    carrier = visual(7, slots=2).float()
    donor = visual(8, slots=2).float() * 2.5 + 4.0

    result = xs4_statistics_transfer(
        carrier,
        donor,
        strength=1.0,
        scope="SPATIAL",
        epsilon=1e-6,
    )

    output_mean = result.output.mean(dim=(-2, -1))
    output_std = result.output.std(dim=(-2, -1), correction=0)
    donor_mean = donor.mean(dim=(-2, -1))
    donor_std = donor.std(dim=(-2, -1), correction=0)
    assert torch.allclose(output_mean, donor_mean, atol=1e-5, rtol=1e-5)
    assert torch.allclose(output_std, donor_std, atol=1e-5, rtol=1e-5)
    assert result.provenance["parameters"]["scope"] == "SPATIAL"


@pytest.mark.parametrize(
    ("mode", "transport"), (("HYBRIDIZE", "TOPK"), ("INTERACT", "SINKHORN"))
)
def test_xs5_preserves_the_reviewed_hybridize_and_interact_transport_modes(
    mode: str, transport: str
) -> None:
    carrier = visual(11)
    donor = visual(12)
    controls = {
        "mix": 0.4,
        "mode": mode,
        "routing": "A",
        "interaction": 0.7,
        "preserve": 0.5,
        "chaos": 0.0,
        "xs5_routing": transport,
        "temperature": 0.2,
        "top_k": 4,
        "sinkhorn_iterations": 3,
    }

    result = xs5_affinity_transport(carrier, donor, controls=controls, seed=33)
    expected = process_xs_sequence(
        carrier, donor, algorithm="XS5", controls=controls, seed=33
    )

    assert torch.equal(result.output, expected.output)
    assert result.provenance["parameters"]["mode"] == mode
    assert result.provenance["parameters"]["transport"] == transport
