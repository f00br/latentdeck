"""Full-grid latent research operators for the Comfy Toolkit.

The functions in this module are deliberately independent from ComfyUI node
registration.  They accept either a visual tensor or a Comfy ``LATENT``
mapping and preserve an H3 audio stream by identity whenever tensor geometry
does not change.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import torch

from .adapter import process_xs_sequence
from .decoder_compare import ToolkitContractError
from .workflow_metadata import annotate_operation

MAX_TEMPORAL_SLOTS = 512
MAX_SPATIAL_TOKENS = 4096
MAX_VISUAL_VALUES = 50_331_648
MAX_AUDIO_SLOTS = 1_048_576


@dataclass(frozen=True, slots=True)
class ResearchResult:
    """One latent result and JSON-safe operation provenance."""

    output: object
    provenance: dict[str, Any]


@dataclass(frozen=True, slots=True)
class VisualLatent:
    """Validated visual/audio view with a non-mutating Comfy repack operation."""

    visual: torch.Tensor
    audio: tuple[object, ...]
    mapping: dict[str, object] | None
    samples: object

    def repack(self, visual: torch.Tensor, *, keep_audio: bool = True) -> object:
        audio = self.audio if keep_audio else ()
        return self.repack_streams(visual, audio)

    def repack_streams(
        self, visual: torch.Tensor, audio: tuple[object, ...]
    ) -> object:
        streams = (visual, *audio)
        if self.mapping is None:
            if bool(getattr(self.samples, "is_nested", False)):
                return _rebuild_nested(self.samples, streams)
            return visual
        output = dict(self.mapping)
        if not bool(getattr(self.samples, "is_nested", False)):
            output["samples"] = visual
            return output
        output["samples"] = _rebuild_nested(self.samples, streams)
        return output


_LatentSurface = VisualLatent


def _rebuild_nested(template: object, streams: tuple[object, ...]) -> object:
    for method_name in ("with_streams", "replace_streams"):
        method = getattr(template, method_name, None)
        if callable(method):
            return method(streams)
    nested_type = type(template)
    for arguments in (streams, (streams,)):
        try:
            rebuilt = nested_type(*arguments)
        except (TypeError, ValueError, RuntimeError):
            continue
        unbind = getattr(rebuilt, "unbind", None)
        if callable(unbind) and len(tuple(unbind())) == len(streams):
            return rebuilt
    raise ToolkitContractError(
        "latent.repack_unsupported",
        "nested LATENT type must expose with_streams()/replace_streams() or a stream constructor",
    )


def _surface(
    value: object, label: str, *, allow_non_finite: bool = False
) -> _LatentSurface:
    mapping: dict[str, object] | None = None
    samples = value
    if isinstance(value, Mapping):
        if not all(isinstance(key, str) for key in value) or "samples" not in value:
            raise ToolkitContractError(
                "latent.invalid", f"{label} must be a tensor or LATENT mapping with samples"
            )
        mapping = dict(value)
        samples = value["samples"]

    if bool(getattr(samples, "is_nested", False)):
        unbind = getattr(samples, "unbind", None)
        if not callable(unbind):
            raise ToolkitContractError("latent.nested_invalid", f"{label} does not expose unbind()")
        try:
            streams = tuple(unbind())
        except Exception as error:
            raise ToolkitContractError(
                "latent.nested_invalid", f"{label} streams could not be unpacked"
            ) from error
        if not 1 <= len(streams) <= 2:
            raise ToolkitContractError(
                "latent.nested_invalid", f"{label} must contain visual and optional audio"
            )
        visual = _validate_visual(streams[0], label, allow_non_finite=allow_non_finite)
        audio: tuple[object, ...] = ()
        if len(streams) == 2:
            audio = (
                _validate_audio(
                    streams[1],
                    label,
                    visual,
                    allow_non_finite=allow_non_finite,
                ),
            )
        return _LatentSurface(visual, audio, mapping, samples)

    visual = _validate_visual(samples, label, allow_non_finite=allow_non_finite)
    return _LatentSurface(visual, (), mapping, samples)


def visual_latent(
    value: object,
    label: str = "latent",
    *,
    allow_non_finite: bool = False,
) -> VisualLatent:
    """Extract a validated visual stream and an AV-safe ``repack`` handle."""

    return _surface(value, label, allow_non_finite=allow_non_finite)


def _validate_visual(
    value: object, label: str, *, allow_non_finite: bool = False
) -> torch.Tensor:
    if not isinstance(value, torch.Tensor):
        raise ToolkitContractError("tensor.type", f"{label} visual stream must be a tensor")
    if value.ndim != 5 or value.shape[0] != 1 or value.shape[1] != 24:
        raise ToolkitContractError("tensor.shape", f"{label} must have layout [1,24,T,H,W]")
    if not 1 <= value.shape[2] <= MAX_TEMPORAL_SLOTS:
        raise ToolkitContractError(
            "tensor.temporal_bound",
            f"{label} temporal slots must be in [1, {MAX_TEMPORAL_SLOTS}]",
        )
    if value.shape[3] < 1 or value.shape[4] < 1:
        raise ToolkitContractError("tensor.shape", f"{label} must have layout [1,24,T,H,W]")
    if value.shape[3] * value.shape[4] > MAX_SPATIAL_TOKENS:
        raise ToolkitContractError(
            "tensor.spatial_bound", f"{label} exceeds the full-grid spatial bound"
        )
    if value.numel() > MAX_VISUAL_VALUES:
        raise ToolkitContractError("tensor.value_bound", f"{label} exceeds the value bound")
    if value.layout is not torch.strided:
        raise ToolkitContractError("tensor.layout", f"{label} must use dense strided storage")
    if value.device.type not in {"cpu", "cuda"}:
        raise ToolkitContractError("tensor.device", f"{label} must use CPU or CUDA")
    if value.dtype not in {torch.float16, torch.float32}:
        raise ToolkitContractError("tensor.dtype", f"{label} must use F16 or F32")
    if not allow_non_finite and not bool(torch.isfinite(value).all().item()):
        raise ToolkitContractError("tensor.non_finite", f"{label} contains NaN or Inf")
    return value


def _validate_audio(
    value: object,
    label: str,
    visual: torch.Tensor,
    *,
    allow_non_finite: bool = False,
) -> torch.Tensor:
    if not isinstance(value, torch.Tensor):
        raise ToolkitContractError("tensor.audio_type", f"{label} audio stream must be a tensor")
    if (
        value.ndim != 4
        or value.shape[0] != 1
        or value.shape[1] != 32
        or value.shape[2] != 2
        or not 1 <= value.shape[3] <= MAX_AUDIO_SLOTS
    ):
        raise ToolkitContractError(
            "tensor.audio_shape", f"{label} audio must have layout [1,32,2,T_audio]"
        )
    if value.layout is not torch.strided:
        raise ToolkitContractError("tensor.audio_layout", f"{label} audio must be dense strided")
    if value.dtype not in {torch.float16, torch.float32}:
        raise ToolkitContractError("tensor.audio_dtype", f"{label} audio must use F16 or F32")
    if value.device != visual.device:
        raise ToolkitContractError(
            "tensor.audio_device", f"{label} audio and visual must use the same device"
        )
    if not allow_non_finite and not bool(torch.isfinite(value).all().item()):
        raise ToolkitContractError(
            "tensor.audio_non_finite", f"{label} audio contains NaN or Inf"
        )
    return value


def _compatible(carrier: _LatentSurface, donor: _LatentSurface) -> None:
    if carrier.visual.shape != donor.visual.shape:
        raise ToolkitContractError(
            "tensor.incompatible_shape", "carrier and donor shapes must match exactly"
        )
    if carrier.visual.dtype != donor.visual.dtype:
        raise ToolkitContractError(
            "tensor.incompatible_dtype", "carrier and donor dtypes must match exactly"
        )
    if carrier.visual.device != donor.visual.device:
        raise ToolkitContractError(
            "tensor.incompatible_device", "carrier and donor devices must match exactly"
        )


def _provenance(operation: str, output: torch.Tensor, **parameters: object) -> dict[str, Any]:
    return {
        "schema_version": "0.1.0",
        "operation": operation,
        "parameters": parameters,
        "shape": list(output.shape),
        "dtype": "F16" if output.dtype == torch.float16 else "F32",
        "device": output.device.type,
        "full_grid": True,
        "hidden_resize": False,
        "hidden_crop": False,
        "hidden_reencode": False,
        "hidden_output_dtype_conversion": False,
    }


def _research_result(
    output: object,
    provenance: dict[str, Any],
    *,
    sources: Sequence[tuple[str, object]],
    structural_role: str,
) -> ResearchResult:
    if isinstance(output, Mapping):
        output = annotate_operation(
            output,
            sources=sources,
            structural_role=structural_role,
            provenance=provenance,
        )
    return ResearchResult(output=output, provenance=provenance)


@torch.inference_mode()
def xs1_channel_mixer(
    carrier: object,
    donor: object,
    *,
    channel_mix: Sequence[float],
) -> ResearchResult:
    """Mix carrier/donor independently for all 24 H3 visual channels."""

    carrier_surface = _surface(carrier, "carrier")
    donor_surface = _surface(donor, "donor")
    _compatible(carrier_surface, donor_surface)
    if isinstance(channel_mix, (str, bytes)) or len(channel_mix) != 24:
        raise ToolkitContractError("control.shape", "channel_mix must contain 24 values")
    try:
        weights = tuple(float(value) for value in channel_mix)
    except (TypeError, ValueError) as error:
        raise ToolkitContractError("control.type", "channel_mix values must be numbers") from error
    if any(not 0.0 <= value <= 1.0 for value in weights):
        raise ToolkitContractError("control.range", "channel_mix values must be in [0,1]")
    weight_tensor = carrier_surface.visual.new_tensor(weights).reshape(1, 24, 1, 1, 1)
    output = torch.lerp(carrier_surface.visual, donor_surface.visual, weight_tensor).contiguous()
    provenance = _provenance("XS1_CHANNEL_MIXER", output, channel_mix=list(weights))
    return _research_result(
        carrier_surface.repack(output),
        provenance,
        sources=(("carrier", carrier), ("donor", donor)),
        structural_role="carrier",
    )


def _mask_grid(mask: object, reference: torch.Tensor) -> torch.Tensor:
    if not isinstance(mask, torch.Tensor):
        raise ToolkitContractError("mask.type", "mask must be a tensor")
    if mask.device != reference.device:
        raise ToolkitContractError("mask.device", "mask and latent must use the same device")
    if mask.dtype not in {torch.bool, reference.dtype}:
        raise ToolkitContractError(
            "mask.dtype", "mask must be bool or use the exact latent dtype"
        )
    expected = reference.shape[2:]
    if tuple(mask.shape) == tuple(expected) or tuple(mask.shape) == (1, *expected):
        grid = mask.reshape(1, 1, *expected)
    elif tuple(mask.shape) == (1, 1, *expected):
        grid = mask
    else:
        raise ToolkitContractError(
            "mask.shape", "mask must exactly match [T,H,W], [1,T,H,W], or [1,1,T,H,W]"
        )
    if grid.dtype is not torch.bool:
        if not bool(torch.isfinite(grid).all().item()):
            raise ToolkitContractError("mask.non_finite", "mask contains NaN or Inf")
        if bool(((grid < 0) | (grid > 1)).any().item()):
            raise ToolkitContractError("mask.range", "mask values must be in [0,1]")
    return grid


@torch.inference_mode()
def xs2_spatial_graft(carrier: object, donor: object, *, mask: torch.Tensor) -> ResearchResult:
    """Graft a donor over the exact latent grid selected by an explicit mask."""

    carrier_surface = _surface(carrier, "carrier")
    donor_surface = _surface(donor, "donor")
    _compatible(carrier_surface, donor_surface)
    grid = _mask_grid(mask, carrier_surface.visual)
    if grid.dtype is torch.bool:
        output = torch.where(grid, donor_surface.visual, carrier_surface.visual)
    else:
        output = torch.lerp(carrier_surface.visual, donor_surface.visual, grid)
    output = output.contiguous()
    provenance = _provenance(
            "XS2_SPATIAL_GRAFT",
            output,
            mask_shape=list(mask.shape),
            soft_mask=mask.dtype != torch.bool,
        )
    return _research_result(
        carrier_surface.repack(output),
        provenance,
        sources=(("carrier", carrier), ("donor", donor)),
        structural_role="carrier",
    )


@torch.inference_mode()
def xs3_frequency_cross_synthesis(
    carrier: object,
    donor: object,
    *,
    cutoff: float,
    donor_band: str = "LOW",
    strength: float = 1.0,
) -> ResearchResult:
    """Exchange explicit spatial-frequency bands on the complete latent grid."""

    carrier_surface = _surface(carrier, "carrier")
    donor_surface = _surface(donor, "donor")
    _compatible(carrier_surface, donor_surface)
    if not 0.0 <= cutoff <= 1.0:
        raise ToolkitContractError("control.range", "cutoff must be in [0,1]")
    if donor_band not in {"LOW", "HIGH"}:
        raise ToolkitContractError("control.enum", "donor_band must be LOW or HIGH")
    if not 0.0 <= strength <= 1.0:
        raise ToolkitContractError("control.range", "strength must be in [0,1]")
    source = carrier_surface.visual
    if strength == 0.0:
        output = source.clone()
    else:
        carrier_spectrum = torch.fft.rfft2(source.float(), dim=(-2, -1))
        donor_spectrum = torch.fft.rfft2(donor_surface.visual.float(), dim=(-2, -1))
        height, width = source.shape[-2:]
        frequencies_y = torch.fft.fftfreq(height, device=source.device).reshape(height, 1)
        frequencies_x = torch.fft.rfftfreq(width, device=source.device).reshape(1, -1)
        radius = torch.sqrt(frequencies_y.square() + frequencies_x.square())
        radius = radius / (2.0**-0.5)
        low_band = radius <= cutoff
        donor_mask = low_band if donor_band == "LOW" else ~low_band
        combined = torch.where(donor_mask, donor_spectrum, carrier_spectrum)
        routed = torch.fft.irfft2(combined, s=(height, width), dim=(-2, -1))
        output = torch.lerp(source.float(), routed, strength).to(dtype=source.dtype).contiguous()
    provenance = _provenance(
            "XS3_FREQUENCY_CROSS_SYNTHESIS",
            output,
            cutoff=float(cutoff),
            donor_band=donor_band,
            strength=float(strength),
            domain="FFT2_SPATIAL",
            compute_dtype="F32",
            output_dtype_preserved=True,
        )
    return _research_result(
        carrier_surface.repack(output),
        provenance,
        sources=(("carrier", carrier), ("donor", donor)),
        structural_role="carrier",
    )


@torch.inference_mode()
def xs4_statistics_transfer(
    carrier: object,
    donor: object,
    *,
    strength: float = 1.0,
    scope: str = "SPATIAL",
    epsilon: float = 1e-6,
) -> ResearchResult:
    """Transfer donor mean/std while retaining the carrier's normalized structure."""

    carrier_surface = _surface(carrier, "carrier")
    donor_surface = _surface(donor, "donor")
    _compatible(carrier_surface, donor_surface)
    if not 0.0 <= strength <= 1.0:
        raise ToolkitContractError("control.range", "strength must be in [0,1]")
    if scope not in {"SPATIAL", "TEMPORAL", "SEQUENCE"}:
        raise ToolkitContractError(
            "control.enum", "scope must be SPATIAL, TEMPORAL, or SEQUENCE"
        )
    if not 0.0 < epsilon <= 1e-2:
        raise ToolkitContractError("control.range", "epsilon must be in (0,1e-2]")
    dimensions = {
        "SPATIAL": (-2, -1),
        "TEMPORAL": (-3,),
        "SEQUENCE": (-3, -2, -1),
    }[scope]
    source = carrier_surface.visual
    if strength == 0.0:
        output = source.clone()
    else:
        carrier_f32 = source.float()
        donor_f32 = donor_surface.visual.float()
        carrier_mean = carrier_f32.mean(dim=dimensions, keepdim=True)
        carrier_std = carrier_f32.std(dim=dimensions, keepdim=True, correction=0)
        donor_mean = donor_f32.mean(dim=dimensions, keepdim=True)
        donor_std = donor_f32.std(dim=dimensions, keepdim=True, correction=0)
        normalized = (carrier_f32 - carrier_mean) / carrier_std.clamp_min(epsilon)
        transferred = normalized * donor_std + donor_mean
        output = torch.lerp(carrier_f32, transferred, strength).to(dtype=source.dtype).contiguous()
    provenance = _provenance(
            "XS4_STATISTICS_TRANSFER",
            output,
            strength=float(strength),
            scope=scope,
            epsilon=float(epsilon),
            compute_dtype="F32",
            output_dtype_preserved=True,
        )
    return _research_result(
        carrier_surface.repack(output),
        provenance,
        sources=(("carrier", carrier), ("donor", donor)),
        structural_role="carrier",
    )


@torch.inference_mode()
def xs5_affinity_transport(
    carrier: object,
    donor: object,
    *,
    controls: Mapping[str, object],
    seed: int = 0,
) -> ResearchResult:
    """Run the reviewed XS5 TOPK/Sinkhorn implementation without forking its math."""

    carrier_surface = _surface(carrier, "carrier")
    donor_surface = _surface(donor, "donor")
    _compatible(carrier_surface, donor_surface)
    if not isinstance(controls, Mapping):
        raise ToolkitContractError("control.type", "controls must be an object")
    operation = process_xs_sequence(
        carrier_surface.visual,
        donor_surface.visual,
        algorithm="XS5",
        controls=controls,
        seed=seed,
    )
    parsed_controls = operation.provenance["operation"]["controls"]
    structural = carrier_surface if parsed_controls["routing"] == "A" else donor_surface
    provenance = _provenance(
        "XS5_AFFINITY_TRANSPORT",
        operation.output,
        mode=parsed_controls["mode"],
        transport=parsed_controls["xs5_routing"],
        controls=parsed_controls,
        seed=seed,
        reviewed_operator=operation.provenance["operation"]["operator_id"],
        reviewed_operator_version=operation.provenance["operation"]["operator_version"],
    )
    structural_role = "carrier" if structural is carrier_surface else "donor"
    return _research_result(
        structural.repack(operation.output),
        provenance,
        sources=(("carrier", carrier), ("donor", donor)),
        structural_role=structural_role,
    )


__all__ = [
    "MAX_SPATIAL_TOKENS",
    "MAX_TEMPORAL_SLOTS",
    "MAX_VISUAL_VALUES",
    "MAX_AUDIO_SLOTS",
    "ResearchResult",
    "VisualLatent",
    "visual_latent",
    "xs1_channel_mixer",
    "xs2_spatial_graft",
    "xs3_frequency_cross_synthesis",
    "xs4_statistics_transfer",
    "xs5_affinity_transport",
]
