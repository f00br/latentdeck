"""Comfy nodes for explicit H3 FAST/HQ decode and native projection."""

from __future__ import annotations

import json
from dataclasses import dataclass, replace

from .decoder_compare import ToolkitContractError
from .vae_research import (
    H3VaeIdentity,
    compare_h3_vaes,
    compare_projected_h3,
    decode_h3_vae,
    project_h3_native,
)


def _json(value: object) -> str:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, separators=(",", ":"), sort_keys=True
    )


@dataclass(frozen=True, slots=True)
class DeclaredH3Vae:
    """One caller-selected Comfy VAE plus explicit external-asset identity."""

    vae: object
    identity: H3VaeIdentity


def _declared(value: object, role: str) -> DeclaredH3Vae:
    if not isinstance(value, DeclaredH3Vae):
        raise ToolkitContractError(
            "vae.declaration_required", "use Declare H3 VAE Asset before decode"
        )
    value.identity.validate(required_role=role)
    return value


class LatentDeckToolkitDeclareH3Vae:
    RETURN_TYPES = ("LATENTDECK_H3_VAE",)
    RETURN_NAMES = ("declared_vae",)
    FUNCTION = "declare"
    CATEGORY = "LatentDeck/Toolkit/Decode"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "vae": ("VAE",),
                "role": (["FAST", "HQ"],),
                "decoder_id": (
                    "STRING",
                    {"default": "org.comfy.taeh3", "multiline": False},
                ),
                "decoder_version": (
                    "STRING",
                    {"default": "0.1.0", "multiline": False},
                ),
                "source": (
                    "STRING",
                    {"default": "explicit Comfy VAE input", "multiline": False},
                ),
                "license": (
                    "STRING",
                    {"default": "verify external asset license", "multiline": False},
                ),
                "asset_sha256": (
                    "STRING",
                    {"default": "REPLACE_WITH_LOWERCASE_SHA256", "multiline": False},
                ),
            }
        }

    def declare(
        self,
        vae: object,
        role: str,
        decoder_id: str,
        decoder_version: str,
        source: str,
        license: str,
        asset_sha256: str,
    ) -> tuple[DeclaredH3Vae]:
        identity = H3VaeIdentity(
            role=role,
            decoder_id=decoder_id,
            decoder_version=decoder_version,
            source=source,
            license=license,
            asset_sha256=asset_sha256,
        )
        identity.validate(required_role=role)
        return (DeclaredH3Vae(vae=vae, identity=identity),)


class LatentDeckToolkitFastDecode:
    RETURN_TYPES = ("IMAGE", "STRING")
    RETURN_NAMES = ("fast_image", "decode_json")
    FUNCTION = "decode"
    CATEGORY = "LatentDeck/Toolkit/Decode"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {"required": {"latent": ("LATENT",), "fast_vae": ("LATENTDECK_H3_VAE",)}}

    def decode(self, latent: object, fast_vae: object):  # type: ignore[no-untyped-def]
        selected = _declared(fast_vae, "FAST")
        result = decode_h3_vae(latent, selected.vae, selected.identity, required_role="FAST")
        return result.image, _json(result.provenance)


class LatentDeckToolkitHQDecode:
    RETURN_TYPES = ("IMAGE", "STRING")
    RETURN_NAMES = ("hq_image", "decode_json")
    FUNCTION = "decode"
    CATEGORY = "LatentDeck/Toolkit/Decode"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {"required": {"latent": ("LATENT",), "hq_vae": ("LATENTDECK_H3_VAE",)}}

    def decode(self, latent: object, hq_vae: object):  # type: ignore[no-untyped-def]
        selected = _declared(hq_vae, "HQ")
        result = decode_h3_vae(latent, selected.vae, selected.identity, required_role="HQ")
        return result.image, _json(result.provenance)


class LatentDeckToolkitFastHQComparator:
    RETURN_TYPES = ("IMAGE", "IMAGE", "STRING")
    RETURN_NAMES = ("fast_image", "hq_image", "comparison_json")
    FUNCTION = "compare"
    CATEGORY = "LatentDeck/Toolkit/Decode"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "fast_vae": ("LATENTDECK_H3_VAE",),
                "hq_vae": ("LATENTDECK_H3_VAE",),
            }
        }

    def compare(self, latent: object, fast_vae: object, hq_vae: object):  # type: ignore[no-untyped-def]
        fast = _declared(fast_vae, "FAST")
        hq = _declared(hq_vae, "HQ")
        result = compare_h3_vaes(
            latent,
            fast.vae,
            hq.vae,
            fast_identity=fast.identity,
            hq_identity=hq.identity,
        )
        return result.fast_image, result.hq_image, _json(result.provenance)


class LatentDeckToolkitManifoldProjector:
    RETURN_TYPES = ("LATENT", "IMAGE", "STRING")
    RETURN_NAMES = ("projected_latent", "native_decode", "projector_json")
    FUNCTION = "project"
    CATEGORY = "LatentDeck/Toolkit/Offline"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {"required": {"latent": ("LATENT",), "hq_vae": ("LATENTDECK_H3_VAE",)}}

    def project(self, latent: object, hq_vae: object):  # type: ignore[no-untyped-def]
        selected = _declared(hq_vae, "HQ")
        projector_identity = replace(selected.identity, role="HQ_PROJECTOR")
        result = project_h3_native(latent, selected.vae, projector_identity)
        return result.latent, result.decoded_image, _json(result.provenance)


class LatentDeckToolkitProjectorComparison:
    RETURN_TYPES = ("IMAGE", "IMAGE", "IMAGE", "IMAGE", "STRING")
    RETURN_NAMES = ("raw_fast", "projected_fast", "raw_hq", "projected_hq", "comparison_json")
    FUNCTION = "compare"
    CATEGORY = "LatentDeck/Toolkit/Offline"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "raw_latent": ("LATENT",),
                "projected_latent": ("LATENT",),
                "fast_vae": ("LATENTDECK_H3_VAE",),
                "hq_vae": ("LATENTDECK_H3_VAE",),
            }
        }

    def compare(
        self,
        raw_latent: object,
        projected_latent: object,
        fast_vae: object,
        hq_vae: object,
    ):  # type: ignore[no-untyped-def]
        fast = _declared(fast_vae, "FAST")
        hq = _declared(hq_vae, "HQ")
        result = compare_projected_h3(
            raw_latent,
            projected_latent,
            fast.vae,
            hq.vae,
            fast_identity=fast.identity,
            hq_identity=hq.identity,
        )
        return (
            result.raw_fast,
            result.projected_fast,
            result.raw_hq,
            result.projected_hq,
            _json(result.provenance),
        )


VAE_NODE_CLASS_MAPPINGS: dict[str, type] = {
    "LatentDeckToolkitDeclareH3Vae": LatentDeckToolkitDeclareH3Vae,
    "LatentDeckToolkitFastDecode": LatentDeckToolkitFastDecode,
    "LatentDeckToolkitHQDecode": LatentDeckToolkitHQDecode,
    "LatentDeckToolkitFastHQComparator": LatentDeckToolkitFastHQComparator,
    "LatentDeckToolkitManifoldProjector": LatentDeckToolkitManifoldProjector,
    "LatentDeckToolkitProjectorComparison": LatentDeckToolkitProjectorComparison,
}

VAE_NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckToolkitDeclareH3Vae": "LatentDeck Declare H3 VAE Asset",
    "LatentDeckToolkitFastDecode": "LatentDeck FAST Decode — TAEHV / taeh3",
    "LatentDeckToolkitHQDecode": "LatentDeck HQ Decode — Native H3 VAE",
    "LatentDeckToolkitFastHQComparator": "LatentDeck FAST vs HQ Comparator",
    "LatentDeckToolkitManifoldProjector": "LatentDeck Manifold Projector — Native H3",
    "LatentDeckToolkitProjectorComparison": "LatentDeck RAW vs PROJECTED Comparator",
}


__all__ = [
    "VAE_NODE_CLASS_MAPPINGS",
    "VAE_NODE_DISPLAY_NAME_MAPPINGS",
    "DeclaredH3Vae",
]
