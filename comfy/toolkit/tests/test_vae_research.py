from __future__ import annotations

import pytest
import torch

from latentdeck_comfy_toolkit.decoder_compare import ToolkitContractError
from latentdeck_comfy_toolkit.vae_research import (
    H3VaeIdentity,
    compare_h3_vaes,
    compare_projected_h3,
    decode_h3_vae,
    project_h3_native,
)


class NestedSamples:
    is_nested = True

    def __init__(self, streams: tuple[torch.Tensor, ...]) -> None:
        self.streams = streams

    def unbind(self) -> tuple[torch.Tensor, ...]:
        return self.streams


class FakeVae:
    def __init__(self, *, decode_scale: float, encode_scale: float = 1.0) -> None:
        self.decode_scale = decode_scale
        self.encode_scale = encode_scale
        self.decoded_inputs: list[torch.Tensor] = []
        self.encoded_inputs: list[torch.Tensor] = []

    def decode(self, latent: torch.Tensor) -> torch.Tensor:
        self.decoded_inputs.append(latent.clone())
        # Comfy video IMAGE layout before VAEDecode flattens B/T.
        return latent[:, :3].movedim(1, -1).float().mul(self.decode_scale).contiguous()

    def encode(self, image: torch.Tensor) -> torch.Tensor:
        self.encoded_inputs.append(image.clone())
        visual = image.movedim(-1, 1)
        repeat = (24 + visual.shape[1] - 1) // visual.shape[1]
        return visual.repeat(1, repeat, 1, 1, 1)[:, :24].mul(self.encode_scale).half().contiguous()


def identity(role: str, sha: str) -> H3VaeIdentity:
    return H3VaeIdentity(
        role=role,
        decoder_id=f"org.example.{role.lower()}",
        decoder_version="1.0.0",
        source="explicit Comfy VAE input",
        license="test-only",
        asset_sha256=sha * 64,
    )


def av_latent() -> tuple[dict[str, object], torch.Tensor, torch.Tensor]:
    video = torch.linspace(-1.0, 1.0, 24 * 2 * 3 * 4).reshape(1, 24, 2, 3, 4).half()
    audio = torch.linspace(-0.5, 0.5, 32 * 2 * 9).reshape(1, 32, 2, 9).half()
    return {"samples": NestedSamples((video, audio)), "marker": "preserve"}, video, audio


def test_fast_and_hq_decode_receive_only_an_isolated_h3_visual_stream() -> None:
    latent, video, _audio = av_latent()
    source = video.clone()
    fast = FakeVae(decode_scale=0.5)

    decoded = decode_h3_vae(latent, fast, identity("FAST", "1"))

    assert len(fast.decoded_inputs) == 1
    assert torch.equal(fast.decoded_inputs[0], source)
    assert fast.decoded_inputs[0].data_ptr() != video.data_ptr()
    assert torch.equal(video, source)
    assert decoded.image.shape == (2, 3, 4, 3)
    assert decoded.provenance["decoder"]["role"] == "FAST"
    assert decoded.provenance["input"]["audio_disposition"] == "ignored_visual_decode"


def test_ready_fast_hq_comparator_uses_same_latent_and_reports_metrics() -> None:
    latent, _video, _audio = av_latent()
    result = compare_h3_vaes(
        latent,
        FakeVae(decode_scale=0.5),
        FakeVae(decode_scale=0.75),
        fast_identity=identity("FAST", "2"),
        hq_identity=identity("HQ", "3"),
    )

    assert result.fast_image.shape == result.hq_image.shape == (2, 3, 4, 3)
    assert result.metrics["mean_absolute_error"] > 0.0
    assert result.provenance["kind"] == "latentdeck.toolkit.h3_fast_hq_comparison"
    assert result.provenance["fast"]["asset_sha256"] == "2" * 64
    assert result.provenance["hq"]["asset_sha256"] == "3" * 64


def test_native_projector_is_explicit_decode_encode_and_preserves_exact_timing_audio() -> None:
    latent, video, audio = av_latent()
    native = FakeVae(decode_scale=0.8, encode_scale=1.25)

    result = project_h3_native(latent, native, identity("HQ_PROJECTOR", "4"))

    assert len(native.decoded_inputs) == len(native.encoded_inputs) == 1
    assert torch.equal(native.decoded_inputs[0], video)
    projected_samples = result.latent["samples"]
    projected_video, projected_audio = projected_samples.unbind()
    assert projected_video.shape == video.shape
    assert torch.equal(projected_audio, audio)
    assert projected_audio.data_ptr() == audio.data_ptr()
    assert result.provenance["operation"] == "H3_NATIVE_DECODE_ENCODE_PROJECTOR"
    assert result.provenance["audio_policy"] == "copied_exact_temporal_geometry"
    assert result.provenance["hidden_resize"] is False


def test_projection_comparison_decodes_raw_and_projected_through_the_same_two_vaes() -> None:
    raw, _video, _audio = av_latent()
    projected = project_h3_native(
        raw,
        FakeVae(decode_scale=1.0, encode_scale=0.9),
        identity("HQ_PROJECTOR", "5"),
    ).latent
    fast = FakeVae(decode_scale=0.5)
    hq = FakeVae(decode_scale=0.75)

    comparison = compare_projected_h3(
        raw,
        projected,
        fast,
        hq,
        fast_identity=identity("FAST", "6"),
        hq_identity=identity("HQ", "7"),
    )

    assert len(fast.decoded_inputs) == len(hq.decoded_inputs) == 2
    assert comparison.raw_fast.shape == comparison.projected_fast.shape
    assert comparison.raw_hq.shape == comparison.projected_hq.shape
    assert comparison.provenance["same_decoder_pair"] is True


def test_vae_surfaces_reject_wrong_roles_missing_methods_and_non_finite_outputs() -> None:
    latent, _video, _audio = av_latent()

    with pytest.raises(ToolkitContractError) as wrong_role:
        decode_h3_vae(latent, FakeVae(decode_scale=1.0), identity("HQ", "8"), required_role="FAST")
    assert wrong_role.value.code == "vae.role_mismatch"

    with pytest.raises(ToolkitContractError) as missing_encode:
        project_h3_native(
            latent,
            object(),
            identity("HQ_PROJECTOR", "9"),
        )
    assert missing_encode.value.code == "vae.callable_missing"

    class NonFiniteVae(FakeVae):
        def decode(self, latent: torch.Tensor) -> torch.Tensor:
            image = super().decode(latent)
            image[0, 0, 0, 0, 0] = float("nan")
            return image

    with pytest.raises(ToolkitContractError) as non_finite:
        decode_h3_vae(latent, NonFiniteVae(decode_scale=1.0), identity("FAST", "a"))
    assert non_finite.value.code == "vae.image_non_finite"
