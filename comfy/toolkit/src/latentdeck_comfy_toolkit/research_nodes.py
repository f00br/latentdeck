"""Comfy declarations for the complete LatentDeck 0.1 operator research surface."""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Any

import torch

from .decoder_compare import ToolkitContractError
from .mixer_labs import dual_mixer_lab, quad_mixer_lab, route_carrier_donors
from .research_evaluation import (
    benchmark_operator,
    evaluate_determinism,
    evaluate_streaming_compatibility,
    latent_scopes,
)
from .research_labs import (
    channel_lab,
    channel_rotation_matrix,
    feedback_lab,
    temporal_lab,
)
from .research_ops import (
    visual_latent,
    xs1_channel_mixer,
    xs2_spatial_graft,
    xs3_frequency_cross_synthesis,
    xs4_statistics_transfer,
    xs5_affinity_transport,
)
from .workflow_metadata import annotate_evaluation

MAX_JSON_INPUT_BYTES = 1_048_576
MAX_SAFE_SEED = 9_007_199_254_740_991


def _json(value: object) -> str:
    return json.dumps(
        value, ensure_ascii=False, allow_nan=False, separators=(",", ":"), sort_keys=True
    )


def _duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
    output: dict[str, object] = {}
    for key, value in pairs:
        if key in output:
            raise ToolkitContractError("node.json_duplicate_key", f"duplicate JSON key: {key}")
        output[key] = value
    return output


def _json_value(value: str, *, expected: type, label: str) -> Any:
    if not isinstance(value, str) or len(value.encode("utf-8")) > MAX_JSON_INPUT_BYTES:
        raise ToolkitContractError("node.json_invalid", f"{label} exceeds its byte bound")
    try:
        parsed = json.loads(value, object_pairs_hook=_duplicates)
    except (TypeError, ValueError) as error:
        raise ToolkitContractError("node.json_invalid", f"{label} is not valid JSON") from error
    if not isinstance(parsed, expected):
        raise ToolkitContractError(
            "node.json_invalid", f"{label} must be a JSON {expected.__name__}"
        )
    return parsed


def _result(value: object, provenance: Mapping[str, object]) -> tuple[object, str]:
    return value, _json(dict(provenance))


class LatentDeckToolkitXS1:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/XS"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor": ("LATENT",),
                "channel_mix_json": (
                    "STRING",
                    {"default": _json([0.5] * 24), "multiline": True},
                ),
            }
        }

    def process(self, carrier: object, donor: object, channel_mix_json: str) -> tuple[object, str]:
        weights = _json_value(channel_mix_json, expected=list, label="channel_mix_json")
        operation = xs1_channel_mixer(carrier, donor, channel_mix=weights)
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitXS2:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/XS"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {"required": {"carrier": ("LATENT",), "donor": ("LATENT",), "mask": ("MASK",)}}

    def process(self, carrier: object, donor: object, mask: torch.Tensor) -> tuple[object, str]:
        operation = xs2_spatial_graft(carrier, donor, mask=mask)
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitXS3:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/XS"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor": ("LATENT",),
                "cutoff": ("FLOAT", {"default": 0.25, "min": 0.0, "max": 1.0, "step": 0.01}),
                "donor_band": (["LOW", "HIGH"],),
                "strength": ("FLOAT", {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}),
            }
        }

    def process(
        self, carrier: object, donor: object, cutoff: float, donor_band: str, strength: float
    ) -> tuple[object, str]:
        operation = xs3_frequency_cross_synthesis(
            carrier, donor, cutoff=cutoff, donor_band=donor_band, strength=strength
        )
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitXS4:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/XS"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor": ("LATENT",),
                "strength": ("FLOAT", {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}),
                "scope": (["SPATIAL", "TEMPORAL", "SEQUENCE"],),
                "epsilon": ("FLOAT", {"default": 1e-6, "min": 1e-8, "max": 1e-2, "step": 1e-6}),
            }
        }

    def process(
        self, carrier: object, donor: object, strength: float, scope: str, epsilon: float
    ) -> tuple[object, str]:
        operation = xs4_statistics_transfer(
            carrier, donor, strength=strength, scope=scope, epsilon=epsilon
        )
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitXS5:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/XS"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {"required": _xs5_inputs()}

    def process(self, carrier: object, donor: object, **controls: object) -> tuple[object, str]:
        seed = int(controls.pop("seed"))
        operation = xs5_affinity_transport(carrier, donor, controls=controls, seed=seed)
        return _result(operation.output, operation.provenance)


def _xs5_inputs() -> dict[str, object]:
    return {
        "carrier": ("LATENT",),
        "donor": ("LATENT",),
        "mix": ("FLOAT", {"default": 0.5, "min": 0.0, "max": 1.0, "step": 0.01}),
        "mode": (["HYBRIDIZE", "INTERACT"],),
        "routing": (["A", "B"],),
        "interaction": ("FLOAT", {"default": 0.7, "min": 0.0, "max": 1.0, "step": 0.01}),
        "preserve": ("FLOAT", {"default": 0.55, "min": 0.0, "max": 1.0, "step": 0.01}),
        "chaos": ("FLOAT", {"default": 0.0, "min": 0.0, "max": 1.0, "step": 0.01}),
        "xs5_routing": (["TOPK", "SINKHORN"],),
        "temperature": ("FLOAT", {"default": 0.12, "min": 0.02, "max": 1.0, "step": 0.01}),
        "top_k": ("INT", {"default": 8, "min": 1, "max": 64}),
        "sinkhorn_iterations": ("INT", {"default": 5, "min": 2, "max": 12}),
        "seed": ("INT", {"default": 0, "min": 0, "max": MAX_SAFE_SEED}),
    }


class LatentDeckToolkitDualMixerLab:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/Labs"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor": ("LATENT",),
                "operator": (["LINEAR", "XS1", "XS2", "XS3", "XS4", "XS5"],),
                "controls_json": ("STRING", {"default": '{"mix":0.5}', "multiline": True}),
                "seed": ("INT", {"default": 0, "min": 0, "max": MAX_SAFE_SEED}),
            },
            "optional": {"mask": ("MASK",)},
        }

    def process(
        self,
        carrier: object,
        donor: object,
        operator: str,
        controls_json: str,
        seed: int,
        mask: torch.Tensor | None = None,
    ) -> tuple[object, str]:
        controls = _json_value(controls_json, expected=dict, label="controls_json")
        result = dual_mixer_lab(
            carrier, donor, operator=operator, controls=controls, seed=seed, mask=mask
        )
        return _result(result.output, result.provenance)


class LatentDeckToolkitCarrierDonorRouter:
    RETURN_TYPES = (
        "LATENT",
        "LATENT",
        "LATENT",
        "LATENT",
        "FLOAT",
        "FLOAT",
        "FLOAT",
        "STRING",
    )
    RETURN_NAMES = (
        "carrier",
        "donor_b",
        "donor_c",
        "donor_d",
        "weight_b",
        "weight_c",
        "weight_d",
        "routing_json",
    )
    FUNCTION = "route"
    CATEGORY = "LatentDeck/Toolkit/Labs"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        weight = {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor_b": ("LATENT",),
                "donor_c": ("LATENT",),
                "donor_d": ("LATENT",),
                "weight_b": ("FLOAT", weight),
                "weight_c": ("FLOAT", weight),
                "weight_d": ("FLOAT", weight),
                "order": (["B,C,D", "B,D,C", "C,B,D", "C,D,B", "D,B,C", "D,C,B"],),
            }
        }

    def route(
        self,
        carrier: object,
        donor_b: object,
        donor_c: object,
        donor_d: object,
        weight_b: float,
        weight_c: float,
        weight_d: float,
        order: str,
    ) -> tuple[object, object, object, object, float, float, float, str]:
        routed = route_carrier_donors(
            carrier,
            donor_b,
            donor_c,
            donor_d,
            donor_weights=(weight_b, weight_c, weight_d),
            order=tuple(order.split(",")),
        )
        return (
            routed.carrier,
            *routed.donors,
            *routed.weights,
            _json(routed.provenance),
        )


class LatentDeckToolkitQuadMixerLab:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/Labs"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        unit = {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}
        return {
            "required": {
                "carrier": ("LATENT",),
                "donor_b": ("LATENT",),
                "donor_c": ("LATENT",),
                "donor_d": ("LATENT",),
                "algorithm": (["LINEAR", "XS5"],),
                "interaction": ("FLOAT", {**unit, "default": 0.7}),
                "mode": (["HYBRIDIZE", "INTERACT"],),
                "preserve": ("FLOAT", {**unit, "default": 0.55}),
                "influence_mode": (["MANUAL", "TRIANGLE"],),
                "donor_weight_b": ("FLOAT", unit),
                "donor_weight_c": ("FLOAT", unit),
                "donor_weight_d": ("FLOAT", unit),
                "triangle_x": ("FLOAT", {**unit, "default": 0.5}),
                "triangle_y": ("FLOAT", {**unit, "default": 1.0 / 3.0}),
                "xs5_routing": (["TOPK", "SINKHORN"],),
                "temperature": ("FLOAT", {"default": 0.12, "min": 0.02, "max": 1.0, "step": 0.01}),
                "top_k": ("INT", {"default": 8, "min": 1, "max": 64}),
                "sinkhorn_iterations": ("INT", {"default": 5, "min": 2, "max": 12}),
                "chaos": ("FLOAT", {**unit, "default": 0.0}),
                "seed": ("INT", {"default": 0, "min": 0, "max": MAX_SAFE_SEED}),
                "source_identities": (
                    "STRING",
                    {"default": "A,B,C,D", "multiline": False},
                ),
            }
        }

    def process(
        self,
        carrier: object,
        donor_b: object,
        donor_c: object,
        donor_d: object,
        algorithm: str,
        interaction: float,
        mode: str,
        preserve: float,
        influence_mode: str,
        donor_weight_b: float,
        donor_weight_c: float,
        donor_weight_d: float,
        triangle_x: float,
        triangle_y: float,
        xs5_routing: str,
        temperature: float,
        top_k: int,
        sinkhorn_iterations: int,
        chaos: float,
        seed: int,
        source_identities: str,
    ) -> tuple[object, str]:
        controls = {
            "algorithm": algorithm,
            "interaction": interaction,
            "mode": mode,
            "preserve": preserve,
            "influence_mode": influence_mode,
            "donor_weight_b": donor_weight_b,
            "donor_weight_c": donor_weight_c,
            "donor_weight_d": donor_weight_d,
            "triangle_x": triangle_x,
            "triangle_y": triangle_y,
            "xs5_routing": xs5_routing,
            "temperature": temperature,
            "top_k": top_k,
            "sinkhorn_iterations": sinkhorn_iterations,
            "chaos": chaos,
        }
        result = quad_mixer_lab(
            carrier,
            donor_b,
            donor_c,
            donor_d,
            controls=controls,
            seed=seed,
            source_identities=tuple(value.strip() for value in source_identities.split(",")),
        )
        return _result(result.output, result.provenance)


class LatentDeckToolkitTemporalLab:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/Labs"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "crop_start": ("INT", {"default": 0, "min": 0, "max": 511}),
                "crop_length": ("INT", {"default": 0, "min": 0, "max": 512}),
                "reverse": ("BOOLEAN", {"default": False}),
                "offset": ("INT", {"default": 0, "min": -511, "max": 511}),
                "loop_count": ("INT", {"default": 1, "min": 1, "max": 16}),
                "audio_policy": (["REJECT", "DROP"],),
            }
        }

    def process(self, latent: object, **controls: object) -> tuple[object, str]:
        length = int(controls.pop("crop_length"))
        operation = temporal_lab(latent, crop_length=None if length == 0 else length, **controls)
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitFeedbackLab:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/Labs"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "amount": ("FLOAT", {"default": 0.25, "min": 0.0, "max": 1.0, "step": 0.01}),
                "delay": ("INT", {"default": 1, "min": 1, "max": 511}),
                "iterations": ("INT", {"default": 1, "min": 1, "max": 8}),
            }
        }

    def process(
        self, latent: object, amount: float, delay: int, iterations: int
    ) -> tuple[object, str]:
        operation = feedback_lab(latent, amount=amount, delay=delay, iterations=iterations)
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitChannelLab:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "operation_json")
    FUNCTION = "process"
    CATEGORY = "LatentDeck/Toolkit/Labs"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "mode": (["ROTATION", "MATRIX_JSON"],),
                "channel_a": ("INT", {"default": 0, "min": 0, "max": 23}),
                "channel_b": ("INT", {"default": 1, "min": 0, "max": 23}),
                "angle_degrees": (
                    "FLOAT",
                    {"default": 30.0, "min": -180.0, "max": 180.0, "step": 1.0},
                ),
                "matrix_json": ("STRING", {"default": "[]", "multiline": True}),
                "strength": ("FLOAT", {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}),
            }
        }

    def process(
        self,
        latent: object,
        mode: str,
        channel_a: int,
        channel_b: int,
        angle_degrees: float,
        matrix_json: str,
        strength: float,
    ) -> tuple[object, str]:
        surface = visual_latent(latent)
        if mode == "ROTATION":
            matrix = channel_rotation_matrix(
                channel_a,
                channel_b,
                angle_degrees=angle_degrees,
                device=surface.visual.device,
            )
        else:
            values = _json_value(matrix_json, expected=list, label="matrix_json")
            try:
                matrix = torch.tensor(values, dtype=torch.float32, device=surface.visual.device)
            except (TypeError, ValueError) as error:
                raise ToolkitContractError(
                    "node.matrix_invalid", "matrix_json must be numeric"
                ) from error
        operation = channel_lab(latent, matrix=matrix, strength=strength)
        return _result(operation.output, operation.provenance)


class LatentDeckToolkitOperatorChainReceipt:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "chain_json")
    FUNCTION = "collect"
    OUTPUT_NODE = True
    CATEGORY = "LatentDeck/Toolkit/Diagnostics"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        widget = ("STRING", {"default": "", "multiline": True})
        return {
            "required": {"latent": ("LATENT",), "step_1_json": widget},
            "optional": {"step_2_json": widget, "step_3_json": widget, "step_4_json": widget},
        }

    def collect(
        self,
        latent: object,
        step_1_json: str,
        step_2_json: str = "",
        step_3_json: str = "",
        step_4_json: str = "",
    ) -> tuple[object, str]:
        chain = [
            _json_value(value, expected=dict, label=f"step_{index}_json")
            for index, value in enumerate(
                (step_1_json, step_2_json, step_3_json, step_4_json), start=1
            )
            if value.strip()
        ]
        if not chain:
            raise ToolkitContractError(
                "node.chain_empty", "at least one operation receipt is required"
            )
        report = {
            "schema_version": "0.1.0",
            "operation": "OPERATOR_CHAIN_RECEIPT",
            "chain": chain,
            "step_count": len(chain),
            "execution_order": "COMFY_GRAPH_ORDER",
        }
        return latent, _json(report)


class LatentDeckToolkitLatentScopes:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "scopes_json")
    FUNCTION = "inspect"
    OUTPUT_NODE = True
    CATEGORY = "LatentDeck/Toolkit/Diagnostics"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {"required": {"latent": ("LATENT",)}}

    def inspect(self, latent: object) -> tuple[object, str]:
        report = latent_scopes(latent)
        return annotate_evaluation(latent, kind="latent_scopes", report=report), _json(report)


@dataclass(frozen=True, slots=True)
class LatentDeckResearchOperatorHook:
    name: str
    full: Callable[[object], object]
    chunk: Callable[[torch.Tensor, object | None, int], tuple[object, object | None]]
    streaming_declared: bool
    descriptor: dict[str, object]


class LatentDeckToolkitDualOperatorHook:
    RETURN_TYPES = ("LATENTDECK_OPERATOR_HOOK",)
    RETURN_NAMES = ("operator_hook",)
    FUNCTION = "build"
    CATEGORY = "LatentDeck/Toolkit/Developer"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "donor": ("LATENT",),
                "operator": (["LINEAR", "XS1", "XS3", "XS4", "XS5"],),
                "controls_json": ("STRING", {"default": '{"mix":0.5}', "multiline": True}),
                "seed": ("INT", {"default": 0, "min": 0, "max": MAX_SAFE_SEED}),
                "streaming_declared": ("BOOLEAN", {"default": False}),
            }
        }

    def build(
        self,
        donor: object,
        operator: str,
        controls_json: str,
        seed: int,
        streaming_declared: bool,
    ) -> tuple[LatentDeckResearchOperatorHook]:
        controls = _json_value(controls_json, expected=dict, label="controls_json")
        donor_surface = visual_latent(donor, "donor")

        def full(value: object) -> object:
            return dual_mixer_lab(
                value, donor, operator=operator, controls=controls, seed=seed
            ).output

        def chunk(
            value: torch.Tensor, state: object | None, offset: int
        ) -> tuple[object, object | None]:
            stop = offset + value.shape[2]
            donor_chunk = donor_surface.visual[:, :, offset:stop]
            if donor_chunk.shape[2] != value.shape[2]:
                raise ToolkitContractError(
                    "node.chunk_donor_range", "donor does not cover the requested chunk"
                )
            output = dual_mixer_lab(
                value,
                donor_chunk.contiguous(),
                operator=operator,
                controls=controls,
                seed=seed,
            ).output
            return output, state

        descriptor = {
            "schema_version": "0.1.0",
            "operator": operator,
            "topology": "dual_source",
            "streaming_declared": bool(streaming_declared),
            "deterministic": True,
            "seed": seed,
            "controls": controls,
        }
        return (
            LatentDeckResearchOperatorHook(
                name=f"dual/{operator}",
                full=full,
                chunk=chunk,
                streaming_declared=bool(streaming_declared),
                descriptor=descriptor,
            ),
        )


def _hook(value: object) -> LatentDeckResearchOperatorHook:
    if not isinstance(value, LatentDeckResearchOperatorHook):
        raise ToolkitContractError("node.operator_hook", "an explicit operator hook is required")
    return value


class LatentDeckToolkitOperatorBenchmark:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "benchmark_json")
    FUNCTION = "run"
    CATEGORY = "LatentDeck/Toolkit/Developer"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "operator_hook": ("LATENTDECK_OPERATOR_HOOK",),
                "warmup_runs": ("INT", {"default": 1, "min": 0, "max": 10}),
                "measured_runs": ("INT", {"default": 5, "min": 1, "max": 100}),
            }
        }

    def run(
        self, latent: object, operator_hook: object, warmup_runs: int, measured_runs: int
    ) -> tuple[object, str]:
        selected = _hook(operator_hook)
        result = benchmark_operator(
            selected.full,
            latent,
            warmup_runs=warmup_runs,
            measured_runs=measured_runs,
            streaming_compatible=selected.streaming_declared,
        )
        report = dict(result.report)
        report["operator"] = selected.descriptor
        output = annotate_evaluation(result.output, kind="benchmark", report=report)
        return output, _json(report)


class LatentDeckToolkitDeterminismTest:
    RETURN_TYPES = ("LATENT", "STRING")
    RETURN_NAMES = ("latent", "determinism_json")
    FUNCTION = "run"
    CATEGORY = "LatentDeck/Toolkit/Developer"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "operator_hook": ("LATENTDECK_OPERATOR_HOOK",),
                "runs": ("INT", {"default": 3, "min": 2, "max": 16}),
            }
        }

    def run(self, latent: object, operator_hook: object, runs: int) -> tuple[object, str]:
        selected = _hook(operator_hook)
        result = evaluate_determinism(selected.full, latent, runs=runs)
        report = dict(result.report)
        report["operator"] = selected.descriptor
        output = annotate_evaluation(result.output, kind="determinism", report=report)
        return output, _json(report)


class LatentDeckToolkitStreamingCompatibilityTest:
    RETURN_TYPES = ("LATENT", "LATENT", "STRING")
    RETURN_NAMES = ("full_clip", "chunked", "streaming_json")
    FUNCTION = "run"
    CATEGORY = "LatentDeck/Toolkit/Developer"

    @classmethod
    def INPUT_TYPES(cls) -> dict[str, dict[str, object]]:
        return {
            "required": {
                "latent": ("LATENT",),
                "operator_hook": ("LATENTDECK_OPERATOR_HOOK",),
                "chunk_slots": ("INT", {"default": 1, "min": 1, "max": 512}),
                "atol": ("FLOAT", {"default": 0.0, "min": 0.0, "max": 1.0, "step": 1e-6}),
                "rtol": ("FLOAT", {"default": 0.0, "min": 0.0, "max": 1.0, "step": 1e-6}),
            }
        }

    def run(
        self,
        latent: object,
        operator_hook: object,
        chunk_slots: int,
        atol: float,
        rtol: float,
    ) -> tuple[dict[str, object], dict[str, object], str]:
        selected = _hook(operator_hook)
        result = evaluate_streaming_compatibility(
            selected.full,
            selected.chunk,
            latent,
            chunk_slots=chunk_slots,
            atol=atol,
            rtol=rtol,
        )
        source = dict(latent) if isinstance(latent, Mapping) else {}
        full = {**source, "samples": result.full_output}
        chunks = {**source, "samples": result.streamed_output}
        report = dict(result.report)
        report["operator"] = selected.descriptor
        report["streaming_declared"] = selected.streaming_declared
        full = annotate_evaluation(full, kind="streaming_compatibility/full", report=report)
        chunks = annotate_evaluation(
            chunks, kind="streaming_compatibility/chunked", report=report
        )
        return full, chunks, _json(report)


RESEARCH_NODE_CLASS_MAPPINGS: dict[str, type] = {
    name: value
    for name, value in {
        "LatentDeckToolkitXS1": LatentDeckToolkitXS1,
        "LatentDeckToolkitXS2": LatentDeckToolkitXS2,
        "LatentDeckToolkitXS3": LatentDeckToolkitXS3,
        "LatentDeckToolkitXS4": LatentDeckToolkitXS4,
        "LatentDeckToolkitXS5": LatentDeckToolkitXS5,
        "LatentDeckToolkitDualMixerLab": LatentDeckToolkitDualMixerLab,
        "LatentDeckToolkitCarrierDonorRouter": LatentDeckToolkitCarrierDonorRouter,
        "LatentDeckToolkitQuadMixerLab": LatentDeckToolkitQuadMixerLab,
        "LatentDeckToolkitTemporalLab": LatentDeckToolkitTemporalLab,
        "LatentDeckToolkitFeedbackLab": LatentDeckToolkitFeedbackLab,
        "LatentDeckToolkitChannelLab": LatentDeckToolkitChannelLab,
        "LatentDeckToolkitOperatorChainReceipt": LatentDeckToolkitOperatorChainReceipt,
        "LatentDeckToolkitLatentScopes": LatentDeckToolkitLatentScopes,
        "LatentDeckToolkitDualOperatorHook": LatentDeckToolkitDualOperatorHook,
        "LatentDeckToolkitOperatorBenchmark": LatentDeckToolkitOperatorBenchmark,
        "LatentDeckToolkitDeterminismTest": LatentDeckToolkitDeterminismTest,
        "LatentDeckToolkitStreamingCompatibilityTest": LatentDeckToolkitStreamingCompatibilityTest,
    }.items()
}

RESEARCH_NODE_DISPLAY_NAME_MAPPINGS = {
    "LatentDeckToolkitXS1": "LatentDeck XS1 — Channel Mixer",
    "LatentDeckToolkitXS2": "LatentDeck XS2 — Spatial Latent Graft",
    "LatentDeckToolkitXS3": "LatentDeck XS3 — Frequency Cross-Synthesis",
    "LatentDeckToolkitXS4": "LatentDeck XS4 — Statistics Transfer",
    "LatentDeckToolkitXS5": "LatentDeck XS5 — Affinity / Sinkhorn Transport",
    "LatentDeckToolkitDualMixerLab": "LatentDeck Dual Mixer Lab",
    "LatentDeckToolkitCarrierDonorRouter": "LatentDeck Carrier / Donor Router",
    "LatentDeckToolkitQuadMixerLab": "LatentDeck Quad Mixer Lab — Carrier + 3 Donors",
    "LatentDeckToolkitTemporalLab": "LatentDeck Temporal Lab",
    "LatentDeckToolkitFeedbackLab": "LatentDeck Feedback Lab (Bounded)",
    "LatentDeckToolkitChannelLab": "LatentDeck Channel Lab — 24x24 Matrix",
    "LatentDeckToolkitOperatorChainReceipt": "LatentDeck Operator Chain Receipt",
    "LatentDeckToolkitLatentScopes": "LatentDeck Latent Scopes / Diagnostics",
    "LatentDeckToolkitDualOperatorHook": "LatentDeck Dual Operator Test Hook",
    "LatentDeckToolkitOperatorBenchmark": "LatentDeck Operator Benchmark",
    "LatentDeckToolkitDeterminismTest": "LatentDeck Determinism Test",
    "LatentDeckToolkitStreamingCompatibilityTest": "LatentDeck Streaming Compatibility Test",
}


__all__ = [
    "RESEARCH_NODE_CLASS_MAPPINGS",
    "RESEARCH_NODE_DISPLAY_NAME_MAPPINGS",
    "LatentDeckResearchOperatorHook",
]
