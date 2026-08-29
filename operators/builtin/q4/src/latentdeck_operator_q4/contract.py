"""Public controls and result types for the trusted LD-Q4 operator."""

from __future__ import annotations

import math
from collections.abc import Mapping
from dataclasses import dataclass, fields
from enum import StrEnum
from typing import Any

import torch

OPERATOR_ID = "org.latentdeck.builtin.ld_q4"
OPERATOR_VERSION = "0.1.0"
MAX_SAFE_SEED = 9_007_199_254_740_991
MAX_SPATIAL_TOKENS = 4096


class Q4ContractError(ValueError):
    """A stable, path-free operator contract failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


class Algorithm(StrEnum):
    LINEAR = "LINEAR"
    XS5 = "XS5"


class InfluenceMode(StrEnum):
    MANUAL = "MANUAL"
    TRIANGLE = "TRIANGLE"


class ArtisticMode(StrEnum):
    HYBRIDIZE = "HYBRIDIZE"
    INTERACT = "INTERACT"


class Xs5Routing(StrEnum):
    TOPK = "TOPK"
    SINKHORN = "SINKHORN"


class DeckSlot(StrEnum):
    A = "A"
    B = "B"
    C = "C"
    D = "D"


def _float_value(name: str, value: object, minimum: float, maximum: float) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise Q4ContractError("control.type", f"{name} must be a finite number")
    parsed = float(value)
    if not math.isfinite(parsed):
        raise Q4ContractError("control.non_finite", f"{name} must be finite")
    if parsed < minimum or parsed > maximum:
        raise Q4ContractError("control.out_of_range", f"{name} must be in [{minimum}, {maximum}]")
    return parsed


def triangular_influence_weights(x: object, y: object) -> tuple[float, float, float]:
    """Map a point inside the B/C/D triangle to barycentric donor weights."""

    parsed_x = _float_value("triangle_x", x, 0.0, 1.0)
    parsed_y = _float_value("triangle_y", y, 0.0, 1.0)
    weight_b = 1.0 - parsed_x - 0.5 * parsed_y
    weight_c = parsed_x - 0.5 * parsed_y
    weight_d = parsed_y
    if min(weight_b, weight_c, weight_d) < -1e-12:
        raise Q4ContractError(
            "control.outside_triangle", "triangle point must lie inside the B/C/D field"
        )
    weights = tuple(max(0.0, weight) for weight in (weight_b, weight_c, weight_d))
    total = sum(weights)
    return tuple(weight / total for weight in weights)


def _int_value(name: str, value: object, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise Q4ContractError("control.type", f"{name} must be an integer")
    if value < minimum or value > maximum:
        raise Q4ContractError("control.out_of_range", f"{name} must be in [{minimum}, {maximum}]")
    return value


@dataclass(frozen=True, slots=True)
class Q4Controls:
    algorithm: Algorithm = Algorithm.LINEAR
    interaction: float = 0.0
    mode: ArtisticMode = ArtisticMode.HYBRIDIZE
    preserve: float = 0.55
    influence_mode: InfluenceMode = InfluenceMode.MANUAL
    donor_weight_b: float = 1.0
    donor_weight_c: float = 1.0
    donor_weight_d: float = 1.0
    triangle_x: float = 0.5
    triangle_y: float = 1.0 / 3.0
    xs5_routing: Xs5Routing = Xs5Routing.TOPK
    temperature: float = 0.12
    top_k: int = 8
    sinkhorn_iterations: int = 5
    chaos: float = 0.0

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> Q4Controls:
        if raw is None:
            return cls()
        if not isinstance(raw, Mapping):
            raise Q4ContractError("control.type", "controls must be an object")
        if any(not isinstance(name, str) for name in raw):
            raise Q4ContractError("control.type", "control names must be strings")
        allowed = {field.name for field in fields(cls)}
        unknown = sorted(set(raw) - allowed)
        if unknown:
            raise Q4ContractError("control.unknown", f"unknown controls: {', '.join(unknown)}")
        values = dict(raw)
        if "algorithm" in values:
            try:
                values["algorithm"] = Algorithm(values["algorithm"])
            except (TypeError, ValueError) as exc:
                raise Q4ContractError(
                    "control.enum", "algorithm must be one of LINEAR, XS5"
                ) from exc
        if "influence_mode" in values:
            try:
                values["influence_mode"] = InfluenceMode(values["influence_mode"])
            except (TypeError, ValueError) as exc:
                raise Q4ContractError(
                    "control.enum", "influence_mode must be one of MANUAL, TRIANGLE"
                ) from exc
        if "mode" in values:
            try:
                values["mode"] = ArtisticMode(values["mode"])
            except (TypeError, ValueError) as exc:
                raise Q4ContractError(
                    "control.enum", "mode must be one of HYBRIDIZE, INTERACT"
                ) from exc
        if "xs5_routing" in values:
            try:
                values["xs5_routing"] = Xs5Routing(values["xs5_routing"])
            except (TypeError, ValueError) as exc:
                raise Q4ContractError(
                    "control.enum", "xs5_routing must be one of TOPK, SINKHORN"
                ) from exc
        for name in (
            "interaction",
            "preserve",
            "donor_weight_b",
            "donor_weight_c",
            "donor_weight_d",
            "triangle_x",
            "triangle_y",
            "temperature",
            "chaos",
        ):
            if name in values:
                minimum = 0.02 if name == "temperature" else 0.0
                values[name] = _float_value(name, values[name], minimum, 1.0)
        if "top_k" in values:
            values["top_k"] = _int_value("top_k", values["top_k"], 1, 64)
        if "sinkhorn_iterations" in values:
            values["sinkhorn_iterations"] = _int_value(
                "sinkhorn_iterations", values["sinkhorn_iterations"], 2, 12
            )
        controls = cls(**values)
        controls.validate()
        return controls

    def validate(self) -> None:
        if not isinstance(self.algorithm, Algorithm):
            raise Q4ContractError("control.enum", "algorithm has an invalid enum value")
        if not isinstance(self.influence_mode, InfluenceMode):
            raise Q4ContractError("control.enum", "influence_mode has an invalid enum value")
        if not isinstance(self.mode, ArtisticMode):
            raise Q4ContractError("control.enum", "mode has an invalid enum value")
        if not isinstance(self.xs5_routing, Xs5Routing):
            raise Q4ContractError("control.enum", "xs5_routing has an invalid enum value")
        _float_value("interaction", self.interaction, 0.0, 1.0)
        _float_value("preserve", self.preserve, 0.0, 1.0)
        for name in ("donor_weight_b", "donor_weight_c", "donor_weight_d"):
            _float_value(name, getattr(self, name), 0.0, 1.0)
        _float_value("triangle_x", self.triangle_x, 0.0, 1.0)
        _float_value("triangle_y", self.triangle_y, 0.0, 1.0)
        _float_value("temperature", self.temperature, 0.02, 1.0)
        _float_value("chaos", self.chaos, 0.0, 1.0)
        _int_value("top_k", self.top_k, 1, 64)
        _int_value("sinkhorn_iterations", self.sinkhorn_iterations, 2, 12)
        if self.influence_mode is InfluenceMode.TRIANGLE:
            triangular_influence_weights(self.triangle_x, self.triangle_y)
        elif self.donor_weight_b + self.donor_weight_c + self.donor_weight_d == 0.0:
            raise Q4ContractError(
                "control.zero_distribution", "at least one donor weight must be positive"
            )

    def resolved_weights(self) -> tuple[float, float, float]:
        self.validate()
        if self.influence_mode is InfluenceMode.TRIANGLE:
            return triangular_influence_weights(self.triangle_x, self.triangle_y)
        total = self.donor_weight_b + self.donor_weight_c + self.donor_weight_d
        return (
            self.donor_weight_b / total,
            self.donor_weight_c / total,
            self.donor_weight_d / total,
        )

    def as_dict(self) -> dict[str, str | int | float]:
        return {
            "algorithm": self.algorithm.value,
            "interaction": self.interaction,
            "mode": self.mode.value,
            "preserve": self.preserve,
            "influence_mode": self.influence_mode.value,
            "donor_weight_b": self.donor_weight_b,
            "donor_weight_c": self.donor_weight_c,
            "donor_weight_d": self.donor_weight_d,
            "triangle_x": self.triangle_x,
            "triangle_y": self.triangle_y,
            "xs5_routing": self.xs5_routing.value,
            "temperature": self.temperature,
            "top_k": self.top_k,
            "sinkhorn_iterations": self.sinkhorn_iterations,
            "chaos": self.chaos,
        }


def _context_int(name: str, value: object, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise Q4ContractError("context.type", f"{name} must be an integer")
    if value < minimum or value > maximum:
        raise Q4ContractError("context.out_of_range", f"{name} must be in [{minimum}, {maximum}]")
    return value


def _identity_value(name: str, value: object) -> str:
    if not isinstance(value, str) or not 1 <= len(value) <= 128:
        raise Q4ContractError(
            "context.identity", f"{name} must be a non-empty identity up to 128 characters"
        )
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:-")
    if any(character not in allowed for character in value):
        raise Q4ContractError("context.identity", f"{name} contains an invalid character")
    return value


@dataclass(frozen=True, slots=True)
class Q4Context:
    codec_family: str = "minimax_h3"
    profile: str = "h3_av_latent"
    profile_version: str = "0.1.0"
    timing_contract: str = "minimax_h3_causal"
    timing_contract_version: str = "0.1.0"
    frame_rate_numerator: int = 24
    frame_rate_denominator: int = 1
    carrier_slot: DeckSlot = DeckSlot.A
    donor_b_slot: DeckSlot = DeckSlot.B
    donor_c_slot: DeckSlot = DeckSlot.C
    donor_d_slot: DeckSlot = DeckSlot.D
    carrier_identity: str = "A"
    donor_b_identity: str = "B"
    donor_c_identity: str = "C"
    donor_d_identity: str = "D"
    carrier_playhead: int = 0
    donor_b_playhead: int = 0
    donor_c_playhead: int = 0
    donor_d_playhead: int = 0
    seed: int = 0

    @classmethod
    def from_mapping(cls, raw: Mapping[str, Any] | None) -> Q4Context:
        if raw is None:
            return cls()
        if not isinstance(raw, Mapping):
            raise Q4ContractError("context.type", "context must be an object")
        if any(not isinstance(name, str) for name in raw):
            raise Q4ContractError("context.type", "context names must be strings")
        allowed = {field.name for field in fields(cls)}
        unknown = sorted(set(raw) - allowed)
        if unknown:
            raise Q4ContractError("context.unknown", f"unknown context: {', '.join(unknown)}")
        values = dict(raw)
        for name in ("carrier_slot", "donor_b_slot", "donor_c_slot", "donor_d_slot"):
            if name not in values:
                continue
            try:
                values[name] = DeckSlot(values[name])
            except (TypeError, ValueError) as exc:
                raise Q4ContractError("context.slot", f"{name} must be one of A, B, C, D") from exc
        context = cls(**values)
        context.validate()
        return context

    def validate(self) -> None:
        expected = {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
        }
        for name, expected_value in expected.items():
            if getattr(self, name) != expected_value:
                raise Q4ContractError("profile.incompatible", f"unsupported {name}")
        if self.frame_rate_numerator != 24 or self.frame_rate_denominator != 1:
            raise Q4ContractError("timing.incompatible", "H3 0.1 requires frame rate 24/1")
        slots = (self.carrier_slot, self.donor_b_slot, self.donor_c_slot, self.donor_d_slot)
        if any(not isinstance(slot, DeckSlot) for slot in slots) or len(set(slots)) != 4:
            raise Q4ContractError(
                "context.slot", "carrier and donor roles must reference four distinct deck slots"
            )
        for name in (
            "carrier_identity",
            "donor_b_identity",
            "donor_c_identity",
            "donor_d_identity",
        ):
            _identity_value(name, getattr(self, name))
        for name in (
            "carrier_playhead",
            "donor_b_playhead",
            "donor_c_playhead",
            "donor_d_playhead",
            "seed",
        ):
            _context_int(name, getattr(self, name), 0, MAX_SAFE_SEED)


@dataclass(frozen=True, slots=True)
class ProcessResult:
    output: torch.Tensor
    provenance: dict[str, Any]
