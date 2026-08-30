"""Ready dual and carrier-plus-donors research topologies for H3 latents."""

from __future__ import annotations

import json
import math
from collections import Counter
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from typing import Any

import torch
from latentdeck_operator_q4 import Q4ContractError
from latentdeck_operator_q4 import process_slot as process_q4_slot

from .decoder_compare import ToolkitContractError
from .research_ops import (
    ResearchResult,
    _provenance,
    _research_result,
    visual_latent,
    xs1_channel_mixer,
    xs2_spatial_graft,
    xs3_frequency_cross_synthesis,
    xs4_statistics_transfer,
    xs5_affinity_transport,
)

MIXER_LABS_VERSION = "0.1.0"
MAX_SAFE_SEED = 9_007_199_254_740_991
_DONOR_NAMES = ("B", "C", "D")


@dataclass(frozen=True, slots=True)
class MixerLabResult:
    output: object
    provenance: dict[str, Any]


@dataclass(frozen=True, slots=True)
class RoutedCarrierDonors:
    carrier: object
    donors: tuple[object, object, object]
    weights: tuple[float, float, float]
    provenance: dict[str, Any]


def _weights(values: Sequence[float]) -> tuple[float, float, float]:
    if isinstance(values, (str, bytes)) or len(values) != 3:
        raise ToolkitContractError("router.weights", "donor_weights must contain B, C, D")
    try:
        parsed = tuple(float(value) for value in values)
    except (TypeError, ValueError) as error:
        raise ToolkitContractError("router.weights", "donor weights must be numeric") from error
    if any(not math.isfinite(value) or value < 0.0 for value in parsed):
        raise ToolkitContractError(
            "router.weights", "donor weights must be finite and non-negative"
        )
    total = sum(parsed)
    if total <= 0.0:
        raise ToolkitContractError("router.weights", "at least one donor weight must be positive")
    return tuple(value / total for value in parsed)  # type: ignore[return-value]


def _order(values: Sequence[str]) -> tuple[str, str, str]:
    if isinstance(values, (str, bytes)) or tuple(sorted(values)) != _DONOR_NAMES:
        raise ToolkitContractError("router.order", "order must contain B, C, D exactly once")
    return tuple(values)  # type: ignore[return-value]


def _compatible_latents(values: Sequence[object]) -> None:
    surfaces = [visual_latent(value, f"input[{index}]") for index, value in enumerate(values)]
    reference = surfaces[0].visual
    for surface in surfaces[1:]:
        if surface.visual.shape != reference.shape:
            raise ToolkitContractError(
                "mixer.shape_incompatible", "all mixer visual shapes must match exactly"
            )
        if surface.visual.dtype != reference.dtype:
            raise ToolkitContractError(
                "mixer.dtype_incompatible", "all mixer visual dtypes must match exactly"
            )
        if surface.visual.device != reference.device:
            raise ToolkitContractError(
                "mixer.device_incompatible", "all mixer visuals must use the same device"
            )


def route_carrier_donors(
    carrier: object,
    donor_b: object,
    donor_c: object,
    donor_d: object,
    *,
    donor_weights: Sequence[float],
    order: Sequence[str] = _DONOR_NAMES,
) -> RoutedCarrierDonors:
    """Assign an immutable carrier and three explicitly ordered donor roles."""

    _compatible_latents((carrier, donor_b, donor_c, donor_d))
    normalized = _weights(donor_weights)
    parsed_order = _order(order)
    donor_by_name = dict(zip(_DONOR_NAMES, (donor_b, donor_c, donor_d), strict=True))
    weight_by_name = dict(zip(_DONOR_NAMES, normalized, strict=True))
    routed = tuple(donor_by_name[name] for name in parsed_order)
    routed_weights = tuple(weight_by_name[name] for name in parsed_order)
    provenance: dict[str, Any] = {
        "schema_version": MIXER_LABS_VERSION,
        "operation": "CARRIER_DONOR_ROUTER",
        "carrier_immutable": True,
        "order": list(parsed_order),
        "normalized_weights": list(routed_weights),
        "deterministic": True,
    }
    return RoutedCarrierDonors(
        carrier=carrier,
        donors=routed,  # type: ignore[arg-type]
        weights=routed_weights,  # type: ignore[arg-type]
        provenance=provenance,
    )


def _controls_mapping(controls: Mapping[str, object]) -> dict[str, object]:
    if not isinstance(controls, Mapping) or not all(isinstance(key, str) for key in controls):
        raise ToolkitContractError("mixer.controls", "controls must be an object")
    return dict(controls)


@torch.inference_mode()
def dual_mixer_lab(
    carrier: object,
    donor: object,
    *,
    operator: str,
    controls: Mapping[str, object],
    seed: int = 0,
    mask: torch.Tensor | None = None,
) -> MixerLabResult:
    """Ready A+B→operator research path covering Linear and XS1–XS5."""

    _compatible_latents((carrier, donor))
    if isinstance(seed, bool) or not isinstance(seed, int) or not 0 <= seed <= MAX_SAFE_SEED:
        raise ToolkitContractError("mixer.seed", "seed is outside the deterministic bound")
    parsed = _controls_mapping(controls)
    result: ResearchResult
    if operator == "LINEAR":
        mix = float(parsed.get("mix", 0.5))
        if not math.isfinite(mix) or not 0.0 <= mix <= 1.0:
            raise ToolkitContractError("mixer.control", "LINEAR mix must be in [0,1]")
        carrier_surface = visual_latent(carrier, "carrier")
        donor_surface = visual_latent(donor, "donor")
        output = torch.lerp(carrier_surface.visual, donor_surface.visual, mix).contiguous()
        linear_provenance = _provenance("LINEAR", output, mix=mix, seed=seed)
        result = _research_result(
            carrier_surface.repack(output),
            linear_provenance,
            sources=(("carrier", carrier), ("donor", donor)),
            structural_role="carrier",
        )
    elif operator == "XS1":
        values = parsed.get("channel_mix", [parsed.get("mix", 0.5)] * 24)
        if not isinstance(values, Sequence):
            raise ToolkitContractError("mixer.control", "XS1 channel_mix must be an array")
        result = xs1_channel_mixer(carrier, donor, channel_mix=values)
    elif operator == "XS2":
        if mask is None:
            raise ToolkitContractError("mixer.mask_required", "XS2 requires an explicit mask")
        result = xs2_spatial_graft(carrier, donor, mask=mask)
    elif operator == "XS3":
        result = xs3_frequency_cross_synthesis(
            carrier,
            donor,
            cutoff=float(parsed.get("cutoff", 0.25)),
            donor_band=str(parsed.get("donor_band", "LOW")),
            strength=float(parsed.get("strength", 1.0)),
        )
    elif operator == "XS4":
        result = xs4_statistics_transfer(
            carrier,
            donor,
            strength=float(parsed.get("strength", 1.0)),
            scope=str(parsed.get("scope", "SPATIAL")),
            epsilon=float(parsed.get("epsilon", 1e-6)),
        )
    elif operator == "XS5":
        result = xs5_affinity_transport(carrier, donor, controls=parsed, seed=seed)
    else:
        raise ToolkitContractError(
            "mixer.operator", "operator must be LINEAR or one of XS1, XS2, XS3, XS4, XS5"
        )
    provenance: dict[str, Any] = {
        "schema_version": MIXER_LABS_VERSION,
        "operation": "DUAL_MIXER_LAB",
        "operator": operator,
        "mode": parsed.get("mode"),
        "seed": seed,
        "operator_provenance": result.provenance,
        "full_grid": True,
        "hidden_resize": False,
        "hidden_reencode": False,
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    return MixerLabResult(output=result.output, provenance=provenance)


def _identity_duplicates(identities: Sequence[str]) -> list[str]:
    counts = Counter(identities)
    return sorted(identity for identity, count in counts.items() if count > 1)


@torch.inference_mode()
def quad_mixer_lab(
    carrier: object,
    donor_b: object,
    donor_c: object,
    donor_d: object,
    *,
    controls: Mapping[str, object],
    seed: int = 0,
    source_identities: Sequence[str] = ("A", "B", "C", "D"),
) -> MixerLabResult:
    """Process 1 immutable carrier + 3 donors over every H3 temporal slot."""

    values = (carrier, donor_b, donor_c, donor_d)
    _compatible_latents(values)
    if isinstance(seed, bool) or not isinstance(seed, int) or not 0 <= seed <= MAX_SAFE_SEED:
        raise ToolkitContractError("mixer.seed", "seed is outside the deterministic bound")
    if (
        isinstance(source_identities, (str, bytes))
        or len(source_identities) != 4
        or not all(isinstance(value, str) and value for value in source_identities)
    ):
        raise ToolkitContractError(
            "mixer.identities", "source_identities must contain four non-empty strings"
        )
    parsed = _controls_mapping(controls)
    carrier_surface = visual_latent(carrier, "carrier")
    donor_surfaces = tuple(
        visual_latent(value, f"donor_{name}")
        for name, value in zip(_DONOR_NAMES, values[1:], strict=True)
    )
    slots: list[torch.Tensor] = []
    slot_provenance: list[dict[str, Any]] = []
    for index in range(carrier_surface.visual.shape[2]):
        try:
            result = process_q4_slot(
                carrier_surface.visual[:, :, index : index + 1],
                donor_surfaces[0].visual[:, :, index : index + 1],
                donor_surfaces[1].visual[:, :, index : index + 1],
                donor_surfaces[2].visual[:, :, index : index + 1],
                parsed,
                {
                    "carrier_identity": source_identities[0],
                    "donor_b_identity": source_identities[1],
                    "donor_c_identity": source_identities[2],
                    "donor_d_identity": source_identities[3],
                    "carrier_playhead": index,
                    "donor_b_playhead": index,
                    "donor_c_playhead": index,
                    "donor_d_playhead": index,
                    "seed": seed,
                },
            )
        except Q4ContractError as error:
            raise ToolkitContractError(f"q4.{error.code}", error.detail) from error
        slots.append(result.output)
        slot_provenance.append(result.provenance)
    output = torch.cat(slots, dim=2).contiguous()
    duplicates = _identity_duplicates(source_identities)
    provenance: dict[str, Any] = {
        "schema_version": MIXER_LABS_VERSION,
        "operation": "QUAD_MIXER_LAB",
        "operator": "org.latentdeck.builtin.ld_q4/0.1.0",
        "controls": parsed,
        "seed": seed,
        "source_identities": list(source_identities),
        "duplicate_source_test": bool(duplicates),
        "duplicate_identities": duplicates,
        "acceptance_scope": (
            "functional_not_source_diversity" if duplicates else "independent_identity_path"
        ),
        "slot_order": ["B", "C", "D"],
        "processed_slots": len(slots),
        "full_grid": True,
        "hidden_downscale": False,
        "hidden_resize": False,
        "hidden_reencode": False,
        "slot_provenance": slot_provenance,
    }
    json.dumps(provenance, allow_nan=False, separators=(",", ":"))
    routed_output = carrier_surface.repack(output)
    if isinstance(routed_output, Mapping):
        routed_output = _research_result(
            routed_output,
            {
                "operation": {
                    "operator_id": "org.latentdeck.builtin.ld_q4",
                    "operator_version": MIXER_LABS_VERSION,
                    "seed": seed,
                    "controls": parsed,
                }
            },
            sources=(
                ("carrier", carrier),
                ("donor_b", donor_b),
                ("donor_c", donor_c),
                ("donor_d", donor_d),
            ),
            structural_role="carrier",
        ).output
    return MixerLabResult(output=routed_output, provenance=provenance)


__all__ = [
    "MAX_SAFE_SEED",
    "MIXER_LABS_VERSION",
    "MixerLabResult",
    "RoutedCarrierDonors",
    "dual_mixer_lab",
    "quad_mixer_lab",
    "route_carrier_donors",
]
