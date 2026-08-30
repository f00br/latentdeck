"""Direct Comfy VAE research paths for H3 FAST, HQ, and projection work.

The caller supplies already loaded Comfy ``VAE`` objects.  This module never
discovers, downloads, or bundles weights.  H3 audio is not decoded; an AV
``LATENT`` contributes only its validated visual stream to the VAE.
"""

from __future__ import annotations

import json
import re
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

import torch

from .decoder_compare import (
    MAX_DECODED_VALUES,
    DecoderHook,
    ToolkitContractError,
    compare_decoder_hooks,
)
from .research_ops import _research_result, visual_latent

VAE_RESEARCH_VERSION = "0.1.0"
_TOKEN = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_ROLES = frozenset({"FAST", "HQ", "HQ_PROJECTOR"})


@dataclass(frozen=True, slots=True)
class H3VaeIdentity:
    """Explicit provenance for a caller-selected external decoder asset."""

    role: str
    decoder_id: str
    decoder_version: str
    source: str
    license: str
    asset_sha256: str

    def validate(self, *, required_role: str | None = None) -> None:
        if self.role not in _ROLES:
            raise ToolkitContractError("vae.role_invalid", "VAE role is unsupported")
        if required_role is not None and self.role != required_role:
            raise ToolkitContractError(
                "vae.role_mismatch", f"this operation requires the {required_role} VAE role"
            )
        if _TOKEN.fullmatch(self.decoder_id) is None:
            raise ToolkitContractError("vae.id_invalid", "decoder_id must be a bounded token")
        if _VERSION.fullmatch(self.decoder_version) is None:
            raise ToolkitContractError(
                "vae.version_invalid", "decoder_version must use MAJOR.MINOR.PATCH"
            )
        for value, label in ((self.source, "source"), (self.license, "license")):
            if not isinstance(value, str) or not value or len(value.encode("utf-8")) > 4096:
                raise ToolkitContractError(
                    "vae.identity_invalid", f"{label} must be non-empty bounded text"
                )
        if _SHA256.fullmatch(self.asset_sha256) is None:
            raise ToolkitContractError(
                "vae.asset_hash_invalid", "asset_sha256 must be lowercase SHA-256"
            )

    def as_dict(self) -> dict[str, str]:
        return {
            "role": self.role,
            "decoder_id": self.decoder_id,
            "decoder_version": self.decoder_version,
            "source": self.source,
            "license": self.license,
            "asset_sha256": self.asset_sha256,
        }


@dataclass(frozen=True, slots=True)
class H3VaeDecodeResult:
    image: torch.Tensor
    provenance: dict[str, Any]


@dataclass(frozen=True, slots=True)
class H3VaeComparison:
    fast_image: torch.Tensor
    hq_image: torch.Tensor
    metrics: dict[str, float | None]
    provenance: dict[str, Any]


@dataclass(frozen=True, slots=True)
class H3ProjectionResult:
    latent: object
    decoded_image: torch.Tensor
    provenance: dict[str, Any]


@dataclass(frozen=True, slots=True)
class H3ProjectionComparison:
    raw_fast: torch.Tensor
    projected_fast: torch.Tensor
    raw_hq: torch.Tensor
    projected_hq: torch.Tensor
    provenance: dict[str, Any]


def _vae_method(vae: object, name: str):  # type: ignore[no-untyped-def]
    method = getattr(vae, name, None)
    if not callable(method):
        raise ToolkitContractError(
            "vae.callable_missing", f"selected Comfy VAE must expose {name}()"
        )
    return method


def _validate_raw_image(value: object, label: str) -> torch.Tensor:
    if not isinstance(value, torch.Tensor):
        raise ToolkitContractError("vae.image_type", f"{label} must be a torch.Tensor")
    if value.ndim not in {4, 5} or value.shape[-1] not in {1, 3, 4}:
        raise ToolkitContractError(
            "vae.image_shape", f"{label} must use Comfy IMAGE or video IMAGE layout"
        )
    if value.numel() < 1 or value.numel() > MAX_DECODED_VALUES:
        raise ToolkitContractError("vae.image_bound", f"{label} exceeds the decoded value bound")
    if value.layout is not torch.strided or value.device.type not in {"cpu", "cuda"}:
        raise ToolkitContractError("vae.image_layout", f"{label} must be dense on CPU or CUDA")
    if value.dtype not in {torch.float16, torch.bfloat16, torch.float32}:
        raise ToolkitContractError("vae.image_dtype", f"{label} must use F16, BF16, or F32")
    if not bool(torch.isfinite(value).all().item()):
        raise ToolkitContractError("vae.image_non_finite", f"{label} contains NaN or Inf")
    return value


def _decode_raw(visual: torch.Tensor, vae: object, label: str) -> torch.Tensor:
    decode = _vae_method(vae, "decode")
    try:
        output = decode(visual.detach().clone(memory_format=torch.contiguous_format))
    except ToolkitContractError:
        raise
    except Exception as error:
        raise ToolkitContractError(
            "vae.decode_failed", f"{label} Comfy VAE decode failed"
        ) from error
    return _validate_raw_image(output, f"{label} output")


def _flatten_video_image(image: torch.Tensor) -> torch.Tensor:
    if image.ndim == 5:
        return image.reshape(-1, image.shape[-3], image.shape[-2], image.shape[-1]).contiguous()
    return image.contiguous()


def _audio_disposition(audio: tuple[object, ...]) -> str:
    return "ignored_visual_decode" if audio else "source_absent"


@torch.inference_mode()
def decode_h3_vae(
    latent: object,
    vae: object,
    identity: H3VaeIdentity,
    *,
    required_role: str | None = None,
) -> H3VaeDecodeResult:
    """Decode the visual stream with one explicitly supplied Comfy VAE."""

    if not isinstance(identity, H3VaeIdentity):
        raise ToolkitContractError("vae.identity_invalid", "H3VaeIdentity is required")
    identity.validate(required_role=required_role)
    surface = visual_latent(latent)
    image = _flatten_video_image(_decode_raw(surface.visual, vae, identity.role))
    provenance: dict[str, Any] = {
        "schema_version": VAE_RESEARCH_VERSION,
        "kind": "latentdeck.toolkit.h3_vae_decode",
        "decoder": identity.as_dict(),
        "input": {
            "shape": list(surface.visual.shape),
            "dtype": str(surface.visual.dtype).removeprefix("torch."),
            "audio_disposition": _audio_disposition(surface.audio),
        },
        "output": {
            "shape": list(image.shape),
            "dtype": str(image.dtype).removeprefix("torch."),
        },
        "external_asset": True,
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return H3VaeDecodeResult(image=image, provenance=provenance)


def _comparison_hook(vae: object, identity: H3VaeIdentity) -> DecoderHook:
    return DecoderHook(
        decoder_id=identity.decoder_id,
        decoder_version=identity.decoder_version,
        decode=lambda value, _asset: _flatten_video_image(_decode_raw(value, vae, identity.role)),
        asset_sha256=identity.asset_sha256,
    )


@torch.inference_mode()
def compare_h3_vaes(
    latent: object,
    fast_vae: object,
    hq_vae: object,
    *,
    fast_identity: H3VaeIdentity,
    hq_identity: H3VaeIdentity,
) -> H3VaeComparison:
    """Ready FAST-vs-HQ comparison using two selected Comfy VAE objects."""

    fast_identity.validate(required_role="FAST")
    hq_identity.validate(required_role="HQ")
    surface = visual_latent(latent)
    comparison = compare_decoder_hooks(
        surface.visual,
        _comparison_hook(fast_vae, fast_identity),
        _comparison_hook(hq_vae, hq_identity),
    )
    provenance = dict(comparison.provenance)
    provenance.update(
        {
            "kind": "latentdeck.toolkit.h3_fast_hq_comparison",
            "fast": fast_identity.as_dict(),
            "hq": hq_identity.as_dict(),
            "audio_disposition": _audio_disposition(surface.audio),
        }
    )
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return H3VaeComparison(
        fast_image=comparison.fast_output,
        hq_image=comparison.hq_output,
        metrics=comparison.metrics,
        provenance=provenance,
    )


@torch.inference_mode()
def project_h3_native(
    latent: object,
    native_vae: object,
    identity: H3VaeIdentity,
) -> H3ProjectionResult:
    """Explicit offline native H3 decode→encode manifold projection."""

    identity.validate(required_role="HQ_PROJECTOR")
    surface = visual_latent(latent)
    raw_image = _decode_raw(surface.visual, native_vae, "HQ_PROJECTOR")
    encode = _vae_method(native_vae, "encode")
    try:
        encoded = encode(raw_image.detach().clone(memory_format=torch.contiguous_format))
    except ToolkitContractError:
        raise
    except Exception as error:
        raise ToolkitContractError(
            "vae.encode_failed", "HQ_PROJECTOR Comfy VAE encode failed"
        ) from error
    projected_surface = visual_latent(encoded, "projected latent")
    projected_visual = projected_surface.visual.contiguous()
    exact_geometry = tuple(projected_visual.shape) == tuple(surface.visual.shape)
    keep_audio = exact_geometry and bool(surface.audio)
    projected = surface.repack(projected_visual, keep_audio=keep_audio)
    audio_policy = (
        "copied_exact_temporal_geometry"
        if keep_audio
        else "source_absent"
        if not surface.audio
        else "omitted_projector_geometry_changed"
    )
    provenance: dict[str, Any] = {
        "schema_version": VAE_RESEARCH_VERSION,
        "operation": "H3_NATIVE_DECODE_ENCODE_PROJECTOR",
        "execution_surface": "comfy_research_offline",
        "vae": identity.as_dict(),
        "input_shape": list(surface.visual.shape),
        "decoded_shape": list(raw_image.shape),
        "output_shape": list(projected_visual.shape),
        "audio_policy": audio_policy,
        "hidden_resize": False,
        "hidden_reencode": False,
        "explicit_native_reencode": True,
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    if isinstance(projected, Mapping):
        projected = _research_result(
            projected,
            {
                "operation": {
                    "operator_id": "org.latentdeck.toolkit.h3_native_projector",
                    "operator_version": VAE_RESEARCH_VERSION,
                    "seed": 0,
                    "controls": {
                        "vae": identity.as_dict(),
                        "audio_policy": audio_policy,
                    },
                },
                "audio_policy": audio_policy,
            },
            sources=(("source", latent),),
            structural_role="source",
        ).output
    return H3ProjectionResult(
        latent=projected,
        decoded_image=_flatten_video_image(raw_image),
        provenance=provenance,
    )


@torch.inference_mode()
def compare_projected_h3(
    raw_latent: object,
    projected_latent: object,
    fast_vae: object,
    hq_vae: object,
    *,
    fast_identity: H3VaeIdentity,
    hq_identity: H3VaeIdentity,
) -> H3ProjectionComparison:
    """Decode RAW and PROJECTED latents through the exact same decoder pair."""

    raw = compare_h3_vaes(
        raw_latent,
        fast_vae,
        hq_vae,
        fast_identity=fast_identity,
        hq_identity=hq_identity,
    )
    projected = compare_h3_vaes(
        projected_latent,
        fast_vae,
        hq_vae,
        fast_identity=fast_identity,
        hq_identity=hq_identity,
    )
    provenance: dict[str, Any] = {
        "schema_version": VAE_RESEARCH_VERSION,
        "kind": "latentdeck.toolkit.h3_projector_comparison",
        "same_decoder_pair": True,
        "fast": fast_identity.as_dict(),
        "hq": hq_identity.as_dict(),
        "raw_fast_hq_metrics": raw.metrics,
        "projected_fast_hq_metrics": projected.metrics,
        "raw_shapes": {
            "fast": list(raw.fast_image.shape),
            "hq": list(raw.hq_image.shape),
        },
        "projected_shapes": {
            "fast": list(projected.fast_image.shape),
            "hq": list(projected.hq_image.shape),
        },
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return H3ProjectionComparison(
        raw_fast=raw.fast_image,
        projected_fast=projected.fast_image,
        raw_hq=raw.hq_image,
        projected_hq=projected.hq_image,
        provenance=provenance,
    )


__all__ = [
    "VAE_RESEARCH_VERSION",
    "H3ProjectionComparison",
    "H3ProjectionResult",
    "H3VaeComparison",
    "H3VaeDecodeResult",
    "H3VaeIdentity",
    "compare_h3_vaes",
    "compare_projected_h3",
    "decode_h3_vae",
    "project_h3_native",
]
