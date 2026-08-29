"""Dependency-free trusted Operator Descriptor and explicit builtin registry."""

from __future__ import annotations

import math
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass

_TOKEN = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
_ENTRYPOINT = re.compile(r"^[A-Za-z_][A-Za-z0-9_.]*:[A-Za-z_][A-Za-z0-9_]*$")


class OperatorLoadError(ValueError):
    """A stable trusted-operator descriptor or registry failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


@dataclass(frozen=True, slots=True)
class ProfileSupport:
    codec_family: str
    profile: str
    profile_version: str
    timing_contract: str
    timing_contract_version: str
    layout: str
    runtime_dtype: str


@dataclass(frozen=True, slots=True)
class ControlDescriptor:
    name: str
    kind: str
    default: str | int | float
    minimum: int | float | None = None
    maximum: int | float | None = None
    values: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class OperatorDescriptor:
    schema_version: str
    operator_id: str
    operator_version: str
    trust: str
    entrypoint: str
    supported_profiles: tuple[ProfileSupport, ...]
    algorithms: tuple[str, ...]
    controls: tuple[ControlDescriptor, ...]
    limits: tuple[tuple[str, int], ...]

    def limit(self, name: str) -> int:
        try:
            return dict(self.limits)[name]
        except KeyError as error:
            raise OperatorLoadError("operator.limit_missing", f"missing limit {name}") from error


def _object(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise OperatorLoadError("operator.descriptor_invalid", f"{label} must be an object")
    return value


def _exact(value: Mapping[str, object], expected: set[str], label: str) -> None:
    if set(value) != expected:
        raise OperatorLoadError(
            "operator.descriptor_invalid", f"{label} fields do not match the closed schema"
        )


def _text(value: object, label: str, *, token: bool = False) -> str:
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > 4096:
        raise OperatorLoadError("operator.descriptor_invalid", f"{label} must be bounded text")
    if token and _TOKEN.fullmatch(value) is None:
        raise OperatorLoadError("operator.descriptor_invalid", f"{label} must be a token")
    return value


def _finite_number(value: object, label: str) -> int | float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise OperatorLoadError("operator.descriptor_invalid", f"{label} must be numeric")
    if not math.isfinite(float(value)):
        raise OperatorLoadError("operator.descriptor_invalid", f"{label} must be finite")
    return value


def _control(name: str, raw: object) -> ControlDescriptor:
    spec = _object(raw, f"control {name}")
    kind = _text(spec.get("type"), f"control {name} type")
    if kind == "enum":
        _exact(spec, {"type", "default", "values"}, f"control {name}")
        values = spec["values"]
        if (
            not isinstance(values, list)
            or not 1 <= len(values) <= 64
            or not all(isinstance(item, str) and item for item in values)
            or len(values) != len(set(values))
        ):
            raise OperatorLoadError(
                "operator.descriptor_invalid", f"control {name} enum is invalid"
            )
        default = spec["default"]
        if not isinstance(default, str) or default not in values:
            raise OperatorLoadError(
                "operator.descriptor_invalid", f"control {name} default is invalid"
            )
        return ControlDescriptor(name, kind, default, values=tuple(values))
    if kind not in {"float", "integer"}:
        raise OperatorLoadError(
            "operator.descriptor_invalid", f"control {name} has an unknown type"
        )
    _exact(spec, {"type", "default", "minimum", "maximum"}, f"control {name}")
    default = _finite_number(spec["default"], f"control {name} default")
    minimum = _finite_number(spec["minimum"], f"control {name} minimum")
    maximum = _finite_number(spec["maximum"], f"control {name} maximum")
    if kind == "integer" and any(
        not isinstance(value, int) or isinstance(value, bool)
        for value in (default, minimum, maximum)
    ):
        raise OperatorLoadError(
            "operator.descriptor_invalid", f"control {name} requires integer bounds"
        )
    if minimum > maximum or default < minimum or default > maximum:
        raise OperatorLoadError("operator.descriptor_invalid", f"control {name} range is invalid")
    return ControlDescriptor(name, kind, default, minimum, maximum)


def validate_descriptor(raw: Mapping[str, object]) -> OperatorDescriptor:
    """Validate the closed Operator Descriptor 0.1 surface."""

    descriptor = _object(raw, "operator descriptor")
    _exact(
        descriptor,
        {
            "schema_version",
            "operator_id",
            "operator_version",
            "trust",
            "entrypoint",
            "supported_profiles",
            "algorithms",
            "controls",
            "limits",
        },
        "operator descriptor",
    )
    schema_version = _text(descriptor["schema_version"], "schema_version")
    if schema_version != "0.1.0":
        raise OperatorLoadError("operator.schema_unsupported", "descriptor schema is unsupported")
    operator_id = _text(descriptor["operator_id"], "operator_id", token=True)
    operator_version = _text(descriptor["operator_version"], "operator_version")
    if _VERSION.fullmatch(operator_version) is None:
        raise OperatorLoadError("operator.descriptor_invalid", "operator_version is invalid")
    trust = _text(descriptor["trust"], "trust")
    if trust != "builtin":
        raise OperatorLoadError("operator.not_trusted", "only explicit builtins may be loaded")
    entrypoint = _text(descriptor["entrypoint"], "entrypoint")
    if _ENTRYPOINT.fullmatch(entrypoint) is None:
        raise OperatorLoadError("operator.descriptor_invalid", "entrypoint is invalid")

    profiles_raw = descriptor["supported_profiles"]
    if not isinstance(profiles_raw, list) or not 1 <= len(profiles_raw) <= 16:
        raise OperatorLoadError("operator.descriptor_invalid", "supported_profiles is invalid")
    profiles: list[ProfileSupport] = []
    profile_fields = {
        "codec_family",
        "profile",
        "profile_version",
        "timing_contract",
        "timing_contract_version",
        "layout",
        "runtime_dtype",
    }
    for raw_profile in profiles_raw:
        profile = _object(raw_profile, "supported profile")
        _exact(profile, profile_fields, "supported profile")
        profiles.append(
            ProfileSupport(
                **{
                    name: _text(profile[name], name, token=name not in {"layout", "runtime_dtype"})
                    for name in profile_fields
                }
            )
        )

    algorithms_raw = descriptor["algorithms"]
    if (
        not isinstance(algorithms_raw, list)
        or not 1 <= len(algorithms_raw) <= 64
        or not all(isinstance(value, str) and value for value in algorithms_raw)
        or len(algorithms_raw) != len(set(algorithms_raw))
    ):
        raise OperatorLoadError("operator.descriptor_invalid", "algorithms is invalid")

    controls_raw = _object(descriptor["controls"], "controls")
    if not 1 <= len(controls_raw) <= 128:
        raise OperatorLoadError("operator.descriptor_invalid", "control count is invalid")
    controls = tuple(_control(name, controls_raw[name]) for name in sorted(controls_raw))

    limits_raw = _object(descriptor["limits"], "limits")
    if not 1 <= len(limits_raw) <= 32:
        raise OperatorLoadError("operator.descriptor_invalid", "limits are invalid")
    limits: list[tuple[str, int]] = []
    for name in sorted(limits_raw):
        _text(name, "limit name", token=True)
        value = limits_raw[name]
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise OperatorLoadError(
                "operator.descriptor_invalid", f"limit {name} must be a positive integer"
            )
        limits.append((name, value))

    return OperatorDescriptor(
        schema_version=schema_version,
        operator_id=operator_id,
        operator_version=operator_version,
        trust=trust,
        entrypoint=entrypoint,
        supported_profiles=tuple(profiles),
        algorithms=tuple(algorithms_raw),
        controls=controls,
        limits=tuple(limits),
    )


@dataclass(frozen=True, slots=True)
class LoadedOperator:
    descriptor: OperatorDescriptor
    _implementation: Callable[..., object]

    def invoke(self, *args: object, **kwargs: object) -> object:
        return self._implementation(*args, **kwargs)


class BuiltinOperatorRegistry:
    """An allowlist populated only by trusted application code."""

    def __init__(self) -> None:
        self._operators: dict[str, LoadedOperator] = {}

    def register(
        self,
        raw_descriptor: Mapping[str, object],
        implementation: Callable[..., object],
        *,
        exported_entrypoint: str,
    ) -> None:
        descriptor = validate_descriptor(raw_descriptor)
        if descriptor.entrypoint != exported_entrypoint:
            raise OperatorLoadError(
                "operator.entrypoint_mismatch", "registered builtin does not match its descriptor"
            )
        if not callable(implementation):
            raise OperatorLoadError(
                "operator.implementation_invalid", "registered implementation is not callable"
            )
        if descriptor.operator_id in self._operators:
            raise OperatorLoadError("operator.duplicate", "operator ID is already registered")
        self._operators[descriptor.operator_id] = LoadedOperator(descriptor, implementation)

    def load(self, operator_id: str, operator_version: str) -> LoadedOperator:
        operator = self._operators.get(operator_id)
        if operator is None:
            raise OperatorLoadError(
                "operator.not_installed", "operator is not explicitly installed"
            )
        if operator.descriptor.operator_version != operator_version:
            raise OperatorLoadError(
                "operator.version_mismatch", "installed operator version does not match"
            )
        return operator

    def descriptors(self) -> tuple[OperatorDescriptor, ...]:
        return tuple(self._operators[name].descriptor for name in sorted(self._operators))


__all__ = [
    "BuiltinOperatorRegistry",
    "ControlDescriptor",
    "LoadedOperator",
    "OperatorDescriptor",
    "OperatorLoadError",
    "ProfileSupport",
    "validate_descriptor",
]
