"""Explicit H3 synthesis compatibility reporting for Toolkit workflows."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass

import torch

from .cartridge_io import LATENTDECK_METADATA_KEY, ToolkitIOError
from .h3_timing import H3_VISUAL_TEMPORAL_RULE, is_valid_h3_visual_temporal_slots

COMPATIBILITY_VERSION = "0.1.0"
_KEY_FIELDS = (
    "codec_family",
    "profile",
    "profile_version",
    "runtime_dtype",
    "batch",
    "channels",
    "temporal_slots",
    "latent_height",
    "latent_width",
    "timing_contract",
    "timing_contract_version",
    "frame_rate_numerator",
    "frame_rate_denominator",
)


@dataclass(frozen=True, slots=True)
class _Descriptor:
    key: dict[str, object]
    temporal_slots: int
    source_kind: str


def _object(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise ToolkitIOError("compatibility.metadata_invalid", f"{label} must be an object")
    return value


def _video(latent: object) -> torch.Tensor:
    workflow = _object(latent, "LATENT")
    samples = workflow.get("samples")
    if bool(getattr(samples, "is_nested", False)):
        unbind = getattr(samples, "unbind", None)
        if not callable(unbind):
            raise ToolkitIOError(
                "compatibility.latent_invalid", "nested H3 samples must expose unbind()"
            )
        streams = tuple(unbind())
        if len(streams) not in {1, 2}:
            raise ToolkitIOError(
                "compatibility.latent_invalid", "nested H3 samples require video and optional audio"
            )
        samples = streams[0]
    if not isinstance(samples, torch.Tensor):
        raise ToolkitIOError(
            "compatibility.latent_invalid", "LATENT samples must contain a video tensor"
        )
    if samples.ndim != 5 or samples.shape[0] != 1 or samples.shape[1] != 24:
        raise ToolkitIOError(
            "compatibility.latent_invalid", "H3 video must have layout [1,24,T,H,W]"
        )
    return samples


def _visual_manifest_tensor(manifest: Mapping[str, object]) -> Mapping[str, object]:
    tensors = manifest.get("tensors")
    if not isinstance(tensors, list):
        raise ToolkitIOError("compatibility.metadata_invalid", "manifest tensors must be an array")
    visual = [
        tensor
        for tensor in tensors
        if isinstance(tensor, Mapping)
        and tensor.get("stream") == "visual"
        and tensor.get("name") == "video"
    ]
    if len(visual) != 1:
        raise ToolkitIOError(
            "compatibility.metadata_invalid", "manifest must describe exactly one video tensor"
        )
    return visual[0]


def _from_manifest(
    manifest: Mapping[str, object], video: torch.Tensor
) -> tuple[dict[str, object], str]:
    codec = _object(manifest.get("codec"), "manifest codec")
    timing = _object(manifest.get("timing"), "manifest timing")
    decoded = _object(timing.get("decoded_video"), "decoded video timing")
    frame_rate = _object(decoded.get("frame_rate"), "decoded frame rate")
    visual = _visual_manifest_tensor(manifest)
    return (
        {
            "codec_family": codec.get("family"),
            "profile": codec.get("profile"),
            "profile_version": codec.get("profile_version"),
            "runtime_dtype": visual.get("runtime_dtype"),
            "batch": video.shape[0],
            "channels": video.shape[1],
            "temporal_slots": video.shape[2],
            "latent_height": video.shape[3],
            "latent_width": video.shape[4],
            "timing_contract": timing.get("contract"),
            "timing_contract_version": timing.get("contract_version"),
            "frame_rate_numerator": frame_rate.get("numerator"),
            "frame_rate_denominator": frame_rate.get("denominator"),
        },
        "latent_cartridge",
    )


def _from_raw(metadata: Mapping[str, object], video: torch.Tensor) -> tuple[dict[str, object], str]:
    profile = _object(metadata.get("profile"), "raw H3 profile")
    return (
        {
            "codec_family": profile.get("codec_family"),
            "profile": profile.get("profile"),
            "profile_version": profile.get("profile_version"),
            "runtime_dtype": "F16",
            "batch": video.shape[0],
            "channels": video.shape[1],
            "temporal_slots": video.shape[2],
            "latent_height": video.shape[3],
            "latent_width": video.shape[4],
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
            "frame_rate_numerator": 24,
            "frame_rate_denominator": 1,
        },
        "raw_h3_safetensors",
    )


def _descriptor(latent: object) -> _Descriptor:
    workflow = _object(latent, "LATENT")
    video = _video(workflow)
    metadata = _object(workflow.get(LATENTDECK_METADATA_KEY), "LatentDeck metadata")
    manifest = metadata.get("manifest")
    if manifest is not None:
        key, source_kind = _from_manifest(_object(manifest, "LC manifest"), video)
    else:
        key, source_kind = _from_raw(metadata, video)
    if any(key[field] is None for field in _KEY_FIELDS):
        raise ToolkitIOError(
            "compatibility.metadata_incomplete", "LatentDeck compatibility metadata is incomplete"
        )
    return _Descriptor(key=key, temporal_slots=video.shape[2], source_kind=source_kind)


def check_h3_compatibility(latents: Sequence[object]) -> dict[str, object]:
    """Compare the normative H3 synthesis keys without converting any input."""

    if isinstance(latents, (str, bytes)) or not 2 <= len(latents) <= 4:
        raise ToolkitIOError(
            "compatibility.input_count", "compatibility requires two to four H3 LATENT inputs"
        )
    descriptors = [_descriptor(latent) for latent in latents]
    reference = descriptors[0].key
    mismatches: list[dict[str, object]] = []
    temporal_contract_valid = [
        is_valid_h3_visual_temporal_slots(descriptor.temporal_slots)
        for descriptor in descriptors
    ]
    for index, (descriptor, valid) in enumerate(
        zip(descriptors, temporal_contract_valid, strict=True)
    ):
        if not valid:
            mismatches.append(
                {
                    "input_index": index,
                    "field": "temporal_slots_contract",
                    "reference": H3_VISUAL_TEMPORAL_RULE,
                    "actual": descriptor.temporal_slots,
                }
            )
    for index, descriptor in enumerate(descriptors[1:], start=1):
        for field in _KEY_FIELDS:
            if descriptor.key[field] != reference[field]:
                mismatches.append(
                    {
                        "input_index": index,
                        "field": field,
                        "reference": reference[field],
                        "actual": descriptor.key[field],
                    }
                )
    return {
        "schema_version": COMPATIBILITY_VERSION,
        "compatible": not mismatches,
        "compatibility_key": dict(reference),
        "temporal_contract": {
            "rule": H3_VISUAL_TEMPORAL_RULE,
            "all_inputs_valid": all(temporal_contract_valid),
        },
        "inputs": [
            {
                "input_index": index,
                "source_kind": descriptor.source_kind,
                "temporal_slots": descriptor.temporal_slots,
                "temporal_contract_valid": temporal_contract_valid[index],
                "key": dict(descriptor.key),
            }
            for index, descriptor in enumerate(descriptors)
        ],
        "mismatches": mismatches,
        "conversion_performed": False,
    }


__all__ = ["COMPATIBILITY_VERSION", "check_h3_compatibility"]
