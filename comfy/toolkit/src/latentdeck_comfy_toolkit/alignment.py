"""Visible, explicit H3 crop/alignment transforms for Comfy node graphs."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass

import torch

from .cartridge_io import LATENTDECK_METADATA_KEY, H3AVSamples, ToolkitIOError
from .compatibility import check_h3_compatibility
from .workflow_metadata import annotate_operation

ALIGNMENT_VERSION = "0.1.0"
_AUDIO_POLICIES = {"PRESERVE_EXACT", "DROP_EXPLICIT"}


@dataclass(frozen=True, slots=True)
class AlignmentResult:
    latent: dict[str, object]
    report: dict[str, object]


@dataclass(frozen=True, slots=True)
class PairAlignmentResult:
    latent_a: dict[str, object]
    latent_b: dict[str, object]
    report: dict[str, object]


def _mapping(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise ToolkitIOError("align.latent_invalid", f"{label} must be an object")
    return value


def _streams(samples: object) -> tuple[torch.Tensor, torch.Tensor | None]:
    if bool(getattr(samples, "is_nested", False)):
        unbind = getattr(samples, "unbind", None)
        if not callable(unbind):
            raise ToolkitIOError("align.latent_invalid", "nested H3 samples must expose unbind()")
        streams = tuple(unbind())
        if len(streams) != 2 or not all(isinstance(item, torch.Tensor) for item in streams):
            raise ToolkitIOError(
                "align.latent_invalid", "nested H3 samples must contain video and audio tensors"
            )
        return streams[0], streams[1]
    if not isinstance(samples, torch.Tensor):
        raise ToolkitIOError("align.latent_invalid", "LATENT samples must be a tensor")
    return samples, None


def _axis(value: object, label: str, *, allow_zero: bool) -> int:
    minimum = 0 if allow_zero else 1
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        comparison = "non-negative" if allow_zero else "positive"
        raise ToolkitIOError("align.range_invalid", f"{label} must be a {comparison} integer")
    return value


def _bounded_window(start: int, length: int, maximum: int, label: str) -> slice:
    end = start + length
    if start >= maximum or end > maximum:
        raise ToolkitIOError(
            "align.range_invalid", f"{label} crop [{start}, {end}) exceeds axis length {maximum}"
        )
    return slice(start, end)


def _repack_av(
    original: object, video: torch.Tensor, audio: torch.Tensor
) -> object:
    if isinstance(original, H3AVSamples):
        return H3AVSamples((video, audio))
    try:
        return type(original)((video, audio))
    except Exception as error:
        raise ToolkitIOError(
            "align.nested_repack_failed",
            "could not reconstruct the source H3 NestedTensor type",
        ) from error


def crop_h3_latent(
    latent: object,
    *,
    temporal_start: int,
    temporal_slots: int,
    spatial_top: int,
    spatial_left: int,
    spatial_height: int,
    spatial_width: int,
    audio_policy: str,
) -> AlignmentResult:
    """Crop exact requested axes; never resize, pad, re-encode, or cast."""

    workflow = _mapping(latent, "LATENT")
    video, audio = _streams(workflow.get("samples"))
    if video.ndim != 5 or video.shape[0] != 1 or video.shape[1] != 24:
        raise ToolkitIOError("align.latent_invalid", "H3 video must have layout [1,24,T,H,W]")
    if audio_policy not in _AUDIO_POLICIES:
        raise ToolkitIOError(
            "align.audio_policy_invalid",
            "audio_policy must be PRESERVE_EXACT or DROP_EXPLICIT",
        )
    temporal_start = _axis(temporal_start, "temporal_start", allow_zero=True)
    temporal_slots = _axis(temporal_slots, "temporal_slots", allow_zero=False)
    spatial_top = _axis(spatial_top, "spatial_top", allow_zero=True)
    spatial_left = _axis(spatial_left, "spatial_left", allow_zero=True)
    spatial_height = _axis(spatial_height, "spatial_height", allow_zero=False)
    spatial_width = _axis(spatial_width, "spatial_width", allow_zero=False)
    temporal = _bounded_window(temporal_start, temporal_slots, video.shape[2], "temporal")
    vertical = _bounded_window(spatial_top, spatial_height, video.shape[3], "vertical")
    horizontal = _bounded_window(spatial_left, spatial_width, video.shape[4], "horizontal")
    temporal_changed = temporal_start != 0 or temporal_slots != video.shape[2]
    if audio is not None and temporal_changed and audio_policy == "PRESERVE_EXACT":
        raise ToolkitIOError(
            "align.audio_timing_changed",
            "temporal crop cannot preserve H3 audio; select DROP_EXPLICIT in the node graph",
        )

    selected = video[:, :, temporal, vertical, horizontal]
    materialized = not selected.is_contiguous()
    output_video = selected.contiguous()
    if audio is None:
        output_samples: object = output_video
        audio_action = "source_absent"
    elif audio_policy == "DROP_EXPLICIT":
        output_samples = output_video
        audio_action = "dropped_explicitly"
    else:
        output_samples = _repack_av(workflow.get("samples"), output_video, audio)
        audio_action = "preserved_timing_exact"

    report: dict[str, object] = {
        "schema_version": ALIGNMENT_VERSION,
        "kind": "latentdeck.toolkit.explicit_crop",
        "before_shape": list(video.shape),
        "after_shape": list(output_video.shape),
        "controls": {
            "temporal_start": temporal_start,
            "temporal_slots": temporal_slots,
            "spatial_top": spatial_top,
            "spatial_left": spatial_left,
            "spatial_height": spatial_height,
            "spatial_width": spatial_width,
            "audio_policy": audio_policy,
        },
        "audio_action": audio_action,
        "conversion": {
            "crop": "explicit",
            "resize": False,
            "reencode": False,
            "dtype_cast": False,
            "materialized_contiguous": materialized,
        },
    }
    output = dict(workflow)
    output["samples"] = output_samples
    existing_metadata = workflow.get(LATENTDECK_METADATA_KEY, {})
    metadata = dict(_mapping(existing_metadata, "LatentDeck metadata"))
    chain = metadata.get("operation_chain", [])
    if not isinstance(chain, list):
        raise ToolkitIOError(
            "align.metadata_invalid", "LatentDeck operation_chain must be an array"
        )
    metadata["operation_chain"] = [*chain, report]
    output[LATENTDECK_METADATA_KEY] = metadata
    output = annotate_operation(
        output,
        sources=(("source", latent),),
        structural_role="source",
        provenance={
            "operation": {
                "operator_id": "org.latentdeck.toolkit.explicit_crop",
                "operator_version": ALIGNMENT_VERSION,
                "seed": 0,
                "controls": report["controls"],
            },
            "audio_action": audio_action,
        },
    )
    return AlignmentResult(latent=output, report=report)


def align_h3_pair(
    latent_a: object,
    latent_b: object,
    *,
    temporal_policy: str,
    spatial_policy: str,
    audio_policy: str,
) -> PairAlignmentResult:
    """Explicitly crop two H3 latents to shared axes under named policies."""

    workflow_a = _mapping(latent_a, "latent_a")
    workflow_b = _mapping(latent_b, "latent_b")
    video_a, _ = _streams(workflow_a.get("samples"))
    video_b, _ = _streams(workflow_b.get("samples"))
    for label, video in (("A", video_a), ("B", video_b)):
        if video.ndim != 5 or video.shape[0] != 1 or video.shape[1] != 24:
            raise ToolkitIOError(
                "align.latent_invalid", f"H3 video {label} must have layout [1,24,T,H,W]"
            )

    if temporal_policy == "ERROR":
        if video_a.shape[2] != video_b.shape[2]:
            raise ToolkitIOError(
                "align.temporal_mismatch", "temporal axes differ and temporal_policy is ERROR"
            )
        temporal_target = video_a.shape[2]
    elif temporal_policy in {"CROP_END_TO_SHORTEST", "CROP_START_TO_SHORTEST"}:
        temporal_target = min(video_a.shape[2], video_b.shape[2])
    else:
        raise ToolkitIOError(
            "align.policy_invalid",
            "temporal_policy must be ERROR, CROP_END_TO_SHORTEST, or CROP_START_TO_SHORTEST",
        )

    same_spatial = video_a.shape[3:] == video_b.shape[3:]
    if spatial_policy == "ERROR":
        if not same_spatial:
            raise ToolkitIOError(
                "align.spatial_mismatch", "spatial axes differ and spatial_policy is ERROR"
            )
        target_height, target_width = video_a.shape[3:]
    elif spatial_policy in {"CENTER_TO_SMALLEST", "TOP_LEFT_TO_SMALLEST"}:
        target_height = min(video_a.shape[3], video_b.shape[3])
        target_width = min(video_a.shape[4], video_b.shape[4])
    else:
        raise ToolkitIOError(
            "align.policy_invalid",
            "spatial_policy must be ERROR, CENTER_TO_SMALLEST, or TOP_LEFT_TO_SMALLEST",
        )

    def starts(video: torch.Tensor) -> tuple[int, int, int]:
        temporal_start = (
            video.shape[2] - temporal_target
            if temporal_policy == "CROP_START_TO_SHORTEST"
            else 0
        )
        if spatial_policy == "CENTER_TO_SMALLEST":
            top = (video.shape[3] - target_height) // 2
            left = (video.shape[4] - target_width) // 2
        else:
            top = 0
            left = 0
        return temporal_start, top, left

    def align_one(workflow: object, video: torch.Tensor) -> AlignmentResult:
        temporal_start, top, left = starts(video)
        return crop_h3_latent(
            workflow,
            temporal_start=temporal_start,
            temporal_slots=temporal_target,
            spatial_top=top,
            spatial_left=left,
            spatial_height=target_height,
            spatial_width=target_width,
            audio_policy=audio_policy,
        )

    aligned_a = align_one(workflow_a, video_a)
    aligned_b = align_one(workflow_b, video_b)
    report: dict[str, object] = {
        "schema_version": ALIGNMENT_VERSION,
        "kind": "latentdeck.toolkit.explicit_pair_alignment",
        "target_shape": [1, 24, temporal_target, target_height, target_width],
        "policies": {
            "temporal": temporal_policy,
            "spatial": spatial_policy,
            "audio": audio_policy,
        },
        "conversion_performed": "explicit_crop_only",
        "input_a": aligned_a.report,
        "input_b": aligned_b.report,
        "compatibility": check_h3_compatibility([aligned_a.latent, aligned_b.latent]),
    }
    return PairAlignmentResult(
        latent_a=aligned_a.latent,
        latent_b=aligned_b.latent,
        report=report,
    )


__all__ = [
    "ALIGNMENT_VERSION",
    "AlignmentResult",
    "PairAlignmentResult",
    "align_h3_pair",
    "crop_h3_latent",
]
