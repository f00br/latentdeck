"""Strict public contract for the trusted LD-D2 built-in operator."""

from __future__ import annotations

import math
from collections.abc import Mapping
from dataclasses import dataclass, fields
from enum import StrEnum
from typing import Any

import torch

OPERATOR_ID = "org.latentdeck.builtin.ld_d2"
OPERATOR_VERSION = "0.1.0"
MAX_SAFE_SEED = 9_007_199_254_740_991
MAX_SPATIAL_TOKENS = 4096


class D2ContractError(ValueError):
    """A stable, path-free operator contract failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


class Algorithm(StrEnum):
    LINEAR = "LINEAR"
    XS1 = "XS1"
    XS2 = "XS2"
    XS3 = "XS3"
    XS4 = "XS4"
    XS5 = "XS5"


class ArtisticMode(StrEnum):
    HYBRIDIZE = "HYBRIDIZE"
    INTERACT = "INTERACT"


class Routing(StrEnum):
    A = "A"
    B = "B"


class Xs5Routing(StrEnum):
    TOPK = "TOPK"
    SINKHORN = "SINKHORN"


def _enum_value(enum_type: type[StrEnum], name: str, value: object) -> StrEnum:
    if not isinstance(value, str):
        raise D2ContractError("control.type", f"{name} must be a string enum")
    try:
        return enum_type(value)
    except ValueError as exc:
        allowed = ", ".join(member.value for member in enum_type)
        raise D2ContractError("control.enum", f"{name} must be one of {allowed}") from exc


def _float_value(name: str, value: object, minimum: float, maximum: float) -> float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise D2ContractError("control.type", f"{name} must be a finite number")
    parsed = float(value)
    if not math.isfinite(parsed):
        raise D2ContractError("control.non_finite", f"{name} must be finite")
    if parsed < minimum or parsed > maximum:
        raise D2ContractError("control.out_of_range", f"{name} must be in [{minimum}, {maximum}]")
    return parsed


def _int_value(name: str, value: object, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise D2ContractError("control.type", f"{name} must be an integer")
    if value < minimum or value > maximum:
        raise D2ContractError("control.out_of_range", f"{name} must be in [{minimum}, {maximum}]")
    return value


@dataclass(frozen=True, slots=True)
class D2Controls:
    algorithm: Algorithm = Algorithm.LINEAR
    mix: float = 0.5
    mode: ArtisticMode = ArtisticMode.HYBRIDIZE
    routing: Routing = Routing.A
    interaction: float = 0.0
    preserve: float = 0.55
    chaos: float = 0.0
    xs1_channel_a: int = 0
    xs1_channel_b: int = 1
    xs1_angle_degrees: float = 30.0
    xs2_radius: int = 1
    xs3_high_gain: float = 0.5
    xs4_epsilon: float = 1e-6
    xs5_routing: Xs5Routing = Xs5Routing.TOPK
    temperature: float = 0.12
    top_k: int = 8
    sinkhorn_iterations: int = 5

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> D2Controls:
        if raw is None:
            return cls()
        if not isinstance(raw, Mapping):
            raise D2ContractError("control.type", "controls must be an object")
        if any(not isinstance(name, str) for name in raw):
            raise D2ContractError("control.type", "control names must be strings")
        allowed = {field.name for field in fields(cls)}
        unknown = sorted(set(raw) - allowed)
        if unknown:
            raise D2ContractError("control.unknown", f"unknown controls: {', '.join(unknown)}")

        values: dict[str, object] = {}
        if "algorithm" in raw:
            values["algorithm"] = _enum_value(Algorithm, "algorithm", raw["algorithm"])
        if "mode" in raw:
            values["mode"] = _enum_value(ArtisticMode, "mode", raw["mode"])
        if "routing" in raw:
            values["routing"] = _enum_value(Routing, "routing", raw["routing"])
        if "xs5_routing" in raw:
            values["xs5_routing"] = _enum_value(Xs5Routing, "xs5_routing", raw["xs5_routing"])

        float_ranges = {
            "mix": (0.0, 1.0),
            "interaction": (0.0, 1.0),
            "preserve": (0.0, 1.0),
            "chaos": (0.0, 1.0),
            "xs1_angle_degrees": (-180.0, 180.0),
            "xs3_high_gain": (-2.0, 2.0),
            "xs4_epsilon": (1e-8, 1e-3),
            "temperature": (0.02, 1.0),
        }
        for name, (minimum, maximum) in float_ranges.items():
            if name in raw:
                values[name] = _float_value(name, raw[name], minimum, maximum)

        int_ranges = {
            "xs1_channel_a": (0, 23),
            "xs1_channel_b": (0, 23),
            "xs2_radius": (1, 8),
            "top_k": (1, 64),
            "sinkhorn_iterations": (2, 12),
        }
        for name, (minimum, maximum) in int_ranges.items():
            if name in raw:
                values[name] = _int_value(name, raw[name], minimum, maximum)

        controls = cls(**values)
        controls.validate()
        return controls

    def validate(self) -> None:
        enum_fields = {
            "algorithm": Algorithm,
            "mode": ArtisticMode,
            "routing": Routing,
            "xs5_routing": Xs5Routing,
        }
        for name, enum_type in enum_fields.items():
            if not isinstance(getattr(self, name), enum_type):
                raise D2ContractError("control.enum", f"{name} has an invalid enum value")
        for name, (minimum, maximum) in {
            "mix": (0.0, 1.0),
            "interaction": (0.0, 1.0),
            "preserve": (0.0, 1.0),
            "chaos": (0.0, 1.0),
            "xs1_angle_degrees": (-180.0, 180.0),
            "xs3_high_gain": (-2.0, 2.0),
            "xs4_epsilon": (1e-8, 1e-3),
            "temperature": (0.02, 1.0),
        }.items():
            _float_value(name, getattr(self, name), minimum, maximum)
        for name, (minimum, maximum) in {
            "xs1_channel_a": (0, 23),
            "xs1_channel_b": (0, 23),
            "xs2_radius": (1, 8),
            "top_k": (1, 64),
            "sinkhorn_iterations": (2, 12),
        }.items():
            _int_value(name, getattr(self, name), minimum, maximum)
        if self.xs1_channel_a == self.xs1_channel_b:
            raise D2ContractError("control.conflict", "xs1_channel_a and xs1_channel_b must differ")

    def as_dict(self) -> dict[str, str | int | float]:
        return {
            "algorithm": self.algorithm.value,
            "mix": self.mix,
            "mode": self.mode.value,
            "routing": self.routing.value,
            "interaction": self.interaction,
            "preserve": self.preserve,
            "chaos": self.chaos,
            "xs1_channel_a": self.xs1_channel_a,
            "xs1_channel_b": self.xs1_channel_b,
            "xs1_angle_degrees": self.xs1_angle_degrees,
            "xs2_radius": self.xs2_radius,
            "xs3_high_gain": self.xs3_high_gain,
            "xs4_epsilon": self.xs4_epsilon,
            "xs5_routing": self.xs5_routing.value,
            "temperature": self.temperature,
            "top_k": self.top_k,
            "sinkhorn_iterations": self.sinkhorn_iterations,
        }


@dataclass(frozen=True, slots=True)
class D2Context:
    codec_family: str = "minimax_h3"
    profile: str = "h3_av_latent"
    profile_version: str = "0.1.0"
    timing_contract: str = "minimax_h3_causal"
    timing_contract_version: str = "0.1.0"
    frame_rate_numerator: int = 24
    frame_rate_denominator: int = 1
    playhead_a: int = 0
    playhead_b: int = 0
    seed: int = 0
    previous_a: torch.Tensor | None = None
    previous_b: torch.Tensor | None = None

    @classmethod
    def from_mapping(cls, raw: Mapping[str, Any] | None) -> D2Context:
        if raw is None:
            return cls()
        if not isinstance(raw, Mapping):
            raise D2ContractError("context.type", "context must be an object")
        if any(not isinstance(name, str) for name in raw):
            raise D2ContractError("context.type", "context names must be strings")
        allowed = {field.name for field in fields(cls)}
        unknown = sorted(set(raw) - allowed)
        if unknown:
            raise D2ContractError("context.unknown", f"unknown context: {', '.join(unknown)}")
        return cls(**raw)

    def validate(self) -> None:
        expected = {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
        }
        for name, value in expected.items():
            if getattr(self, name) != value:
                raise D2ContractError("profile.incompatible", f"unsupported {name}")
        if self.frame_rate_numerator != 24 or self.frame_rate_denominator != 1:
            raise D2ContractError("timing.incompatible", "H3 0.1 requires frame rate 24/1")
        _int_value("playhead_a", self.playhead_a, 0, MAX_SAFE_SEED)
        _int_value("playhead_b", self.playhead_b, 0, MAX_SAFE_SEED)
        _int_value("seed", self.seed, 0, MAX_SAFE_SEED)


@dataclass(frozen=True, slots=True)
class ProcessResult:
    output: torch.Tensor
    provenance: dict[str, Any]
