"""Explicit trusted-install boundary for separately distributed research operators."""

from __future__ import annotations

import json
import math
import re
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from importlib.resources import files
from inspect import Parameter, signature
from typing import Any

import torch

from .decoder_compare import ToolkitContractError

OPERATOR_API_VERSION = "0.1.0"
MAX_OPERATOR_PROVENANCE_BYTES = 65_536
MAX_SAFE_SEED = 9_007_199_254_740_991
_TOKEN = re.compile(r"^[a-z0-9][a-z0-9._-]{0,127}$")
_CONTROL_TOKEN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
_VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+$")
_ENTRYPOINT = re.compile(
    r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*:[A-Za-z_][A-Za-z0-9_]*$"
)


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
class OperatorCapabilities:
    full_clip: bool
    streaming: bool
    chunk: bool
    deterministic: bool


@dataclass(frozen=True, slots=True)
class BypassDescriptor:
    controls: tuple[tuple[str, str | int | float], ...]
    output_source: int


@dataclass(frozen=True, slots=True)
class ExternalOperatorDescriptor:
    schema_version: str
    operator_id: str
    operator_version: str
    trust: str
    entrypoint: str
    supported_profiles: tuple[ProfileSupport, ...]
    controls: tuple[ControlDescriptor, ...]
    max_spatial_tokens: int
    topology: str
    input_count: int
    capabilities: OperatorCapabilities
    bypass: BypassDescriptor


@dataclass(frozen=True, slots=True)
class OperatorContext:
    codec_family: str = "minimax_h3"
    profile: str = "h3_av_latent"
    profile_version: str = "0.1.0"
    timing_contract: str = "minimax_h3_causal"
    timing_contract_version: str = "0.1.0"
    seed: int = 0
    slot_index: int = 0
    processing_mode: str = "full_clip"

    def validate(self, descriptor: ExternalOperatorDescriptor) -> None:
        if isinstance(self.seed, bool) or not isinstance(self.seed, int):
            raise ToolkitContractError("operator.context_invalid", "seed must be an integer")
        if not 0 <= self.seed <= MAX_SAFE_SEED:
            raise ToolkitContractError("operator.context_invalid", "seed is out of range")
        if isinstance(self.slot_index, bool) or not isinstance(self.slot_index, int):
            raise ToolkitContractError("operator.context_invalid", "slot_index must be an integer")
        if not 0 <= self.slot_index <= MAX_SAFE_SEED:
            raise ToolkitContractError("operator.context_invalid", "slot_index is out of range")
        if not isinstance(self.processing_mode, str) or self.processing_mode not in {
            "full_clip",
            "streaming",
            "chunk",
        }:
            raise ToolkitContractError("operator.context_invalid", "processing_mode is invalid")
        if not getattr(descriptor.capabilities, self.processing_mode):
            raise ToolkitContractError(
                "operator.processing_mode_unsupported",
                "operator does not support the requested processing mode",
            )
        identity = (
            self.codec_family,
            self.profile,
            self.profile_version,
            self.timing_contract,
            self.timing_contract_version,
            "[1,24,1,H,W]",
            "F16",
        )
        supported = {
            (
                item.codec_family,
                item.profile,
                item.profile_version,
                item.timing_contract,
                item.timing_contract_version,
                item.layout,
                item.runtime_dtype,
            )
            for item in descriptor.supported_profiles
        }
        if identity not in supported:
            raise ToolkitContractError(
                "operator.profile_incompatible", "operator does not support this profile"
            )


@dataclass(frozen=True, slots=True)
class ToolkitOperatorResult:
    output: torch.Tensor
    provenance: dict[str, Any]


OperatorCallable = Callable[
    [tuple[torch.Tensor, ...], dict[str, object], OperatorContext],
    ToolkitOperatorResult,
]
LegacyDualOperatorCallable = Callable[
    [torch.Tensor, torch.Tensor, dict[str, object], OperatorContext], ToolkitOperatorResult
]


def _object(value: object, label: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or not all(isinstance(key, str) for key in value):
        raise ToolkitContractError("operator.descriptor_invalid", f"{label} must be an object")
    return value


def _exact(value: Mapping[str, object], expected: set[str], label: str) -> None:
    if set(value) != expected:
        raise ToolkitContractError(
            "operator.descriptor_invalid", f"{label} fields do not match the closed schema"
        )


def _text(value: object, label: str, *, token: bool = False) -> str:
    if not isinstance(value, str) or not value:
        raise ToolkitContractError("operator.descriptor_invalid", f"{label} must be bounded text")
    try:
        encoded = value.encode("utf-8")
    except UnicodeEncodeError as error:
        raise ToolkitContractError(
            "operator.descriptor_invalid", f"{label} must be valid Unicode"
        ) from error
    if len(encoded) > 4096:
        raise ToolkitContractError("operator.descriptor_invalid", f"{label} must be bounded text")
    if token and _TOKEN.fullmatch(value) is None:
        raise ToolkitContractError("operator.descriptor_invalid", f"{label} must be a token")
    return value


def _number(value: object, label: str) -> int | float:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise ToolkitContractError("operator.descriptor_invalid", f"{label} must be numeric")
    try:
        finite = math.isfinite(float(value))
    except OverflowError as error:
        raise ToolkitContractError(
            "operator.descriptor_invalid", f"{label} is outside the numeric bound"
        ) from error
    if not finite:
        raise ToolkitContractError("operator.descriptor_invalid", f"{label} must be finite")
    return value


def _control(name: str, raw: object) -> ControlDescriptor:
    if _TOKEN.fullmatch(name) is None:
        raise ToolkitContractError("operator.descriptor_invalid", "control name must be a token")
    value = _object(raw, f"control {name}")
    kind = _text(value.get("type"), f"control {name} type")
    if kind == "enum":
        _exact(value, {"type", "default", "values"}, f"control {name}")
        choices = value["values"]
        if (
            not isinstance(choices, list)
            or not 1 <= len(choices) <= 64
            or not all(
                isinstance(choice, str) and _CONTROL_TOKEN.fullmatch(choice) is not None
                for choice in choices
            )
            or len(choices) != len(set(choices))
        ):
            raise ToolkitContractError(
                "operator.descriptor_invalid", f"control {name} enum is invalid"
            )
        default = value["default"]
        if not isinstance(default, str) or default not in choices:
            raise ToolkitContractError(
                "operator.descriptor_invalid", f"control {name} default is invalid"
            )
        return ControlDescriptor(name, kind, default, values=tuple(choices))
    if kind not in {"float", "integer"}:
        raise ToolkitContractError(
            "operator.descriptor_invalid", f"control {name} type is unsupported"
        )
    _exact(value, {"type", "default", "minimum", "maximum"}, f"control {name}")
    default = _number(value["default"], f"control {name} default")
    minimum = _number(value["minimum"], f"control {name} minimum")
    maximum = _number(value["maximum"], f"control {name} maximum")
    if kind == "integer" and any(
        isinstance(item, bool) or not isinstance(item, int) for item in (default, minimum, maximum)
    ):
        raise ToolkitContractError(
            "operator.descriptor_invalid", f"control {name} requires integer bounds"
        )
    if minimum > maximum or default < minimum or default > maximum:
        raise ToolkitContractError(
            "operator.descriptor_invalid", f"control {name} range is invalid"
        )
    return ControlDescriptor(name, kind, default, minimum, maximum)


def validate_external_descriptor(raw: Mapping[str, object]) -> ExternalOperatorDescriptor:
    """Validate the closed explicit-install Operator API 0.1 descriptor."""

    descriptor = _object(raw, "operator descriptor")
    descriptor_fields = {
        "schema_version",
        "operator_id",
        "operator_version",
        "trust",
        "entrypoint",
        "supported_profiles",
        "controls",
        "limits",
        "topology",
        "input_count",
        "capabilities",
        "bypass",
    }
    _exact(descriptor, descriptor_fields, "operator descriptor")
    schema_version = _text(descriptor["schema_version"], "schema_version")
    if schema_version != OPERATOR_API_VERSION:
        raise ToolkitContractError(
            "operator.schema_unsupported", "operator schema version is unsupported"
        )
    operator_id = _text(descriptor["operator_id"], "operator_id", token=True)
    operator_version = _text(descriptor["operator_version"], "operator_version")
    if _VERSION.fullmatch(operator_version) is None:
        raise ToolkitContractError(
            "operator.descriptor_invalid", "operator_version must use MAJOR.MINOR.PATCH"
        )
    trust = _text(descriptor["trust"], "trust")
    if trust != "explicit_install":
        raise ToolkitContractError(
            "operator.not_trusted", "external operators require an explicit installation"
        )
    entrypoint = _text(descriptor["entrypoint"], "entrypoint")
    if _ENTRYPOINT.fullmatch(entrypoint) is None:
        raise ToolkitContractError("operator.descriptor_invalid", "entrypoint is invalid")

    raw_profiles = descriptor["supported_profiles"]
    if not isinstance(raw_profiles, list) or not 1 <= len(raw_profiles) <= 16:
        raise ToolkitContractError("operator.descriptor_invalid", "supported_profiles is invalid")
    profile_fields = {
        "codec_family",
        "profile",
        "profile_version",
        "timing_contract",
        "timing_contract_version",
        "layout",
        "runtime_dtype",
    }
    profiles: list[ProfileSupport] = []
    for raw_profile in raw_profiles:
        profile = _object(raw_profile, "supported profile")
        _exact(profile, profile_fields, "supported profile")
        parsed_profile = ProfileSupport(
            **{
                name: _text(
                    profile[name],
                    name,
                    token=name not in {"layout", "runtime_dtype"},
                )
                for name in profile_fields
            }
        )
        if parsed_profile.layout != "[1,24,1,H,W]" or parsed_profile.runtime_dtype != "F16":
            raise ToolkitContractError(
                "operator.descriptor_invalid",
                "Operator API 0.1 requires [1,24,1,H,W] F16 slots",
            )
        profiles.append(parsed_profile)

    raw_controls = _object(descriptor["controls"], "controls")
    if len(raw_controls) > 128:
        raise ToolkitContractError("operator.descriptor_invalid", "too many controls")
    controls = tuple(_control(name, raw_controls[name]) for name in sorted(raw_controls))

    topology = _text(descriptor["topology"], "topology")
    if topology not in {"single_source", "dual_source", "carrier_donors"}:
        raise ToolkitContractError("operator.descriptor_invalid", "topology is unsupported")
    input_count = descriptor["input_count"]
    if (
        isinstance(input_count, bool)
        or not isinstance(input_count, int)
        or not 1 <= input_count <= 16
        or (topology == "single_source" and input_count != 1)
        or (topology == "dual_source" and input_count != 2)
        or (topology == "carrier_donors" and input_count < 2)
    ):
        raise ToolkitContractError(
            "operator.descriptor_invalid", "input_count does not match topology"
        )

    raw_capabilities = _object(descriptor["capabilities"], "capabilities")
    _exact(
        raw_capabilities,
        {"full_clip", "streaming", "chunk", "deterministic"},
        "capabilities",
    )
    if not all(isinstance(raw_capabilities[name], bool) for name in raw_capabilities):
        raise ToolkitContractError("operator.descriptor_invalid", "capabilities must be booleans")
    capabilities = OperatorCapabilities(
        full_clip=raw_capabilities["full_clip"],  # type: ignore[arg-type]
        streaming=raw_capabilities["streaming"],  # type: ignore[arg-type]
        chunk=raw_capabilities["chunk"],  # type: ignore[arg-type]
        deterministic=raw_capabilities["deterministic"],  # type: ignore[arg-type]
    )
    if not (capabilities.full_clip or capabilities.streaming or capabilities.chunk):
        raise ToolkitContractError(
            "operator.descriptor_invalid", "operator supports no processing mode"
        )

    raw_bypass = _object(descriptor["bypass"], "bypass")
    _exact(raw_bypass, {"controls", "output_source"}, "bypass")
    bypass_controls = _object(raw_bypass["controls"], "bypass controls")
    if not 1 <= len(bypass_controls) <= 128:
        raise ToolkitContractError(
            "operator.descriptor_invalid", "bypass controls must not be empty"
        )
    control_specs = {control.name: control for control in controls}
    parsed_bypass_controls: list[tuple[str, str | int | float]] = []
    for name in sorted(bypass_controls):
        specification = control_specs.get(name)
        value = bypass_controls[name]
        if specification is None:
            raise ToolkitContractError(
                "operator.descriptor_invalid", "bypass references an unknown control"
            )
        if specification.kind == "enum":
            if not isinstance(value, str) or value not in specification.values:
                raise ToolkitContractError(
                    "operator.descriptor_invalid", "bypass enum value is invalid"
                )
        else:
            value = _number(value, f"bypass control {name}")
            if specification.kind == "integer" and not isinstance(value, int):
                raise ToolkitContractError(
                    "operator.descriptor_invalid", "bypass integer value is invalid"
                )
            if value < specification.minimum or value > specification.maximum:  # type: ignore[operator]
                raise ToolkitContractError(
                    "operator.descriptor_invalid", "bypass value is outside its range"
                )
        parsed_bypass_controls.append((name, value))
    output_source = raw_bypass["output_source"]
    if (
        isinstance(output_source, bool)
        or not isinstance(output_source, int)
        or not 0 <= output_source < input_count
    ):
        raise ToolkitContractError(
            "operator.descriptor_invalid", "bypass output_source is out of range"
        )
    bypass = BypassDescriptor(tuple(parsed_bypass_controls), output_source)

    limits = _object(descriptor["limits"], "limits")
    _exact(limits, {"max_spatial_tokens"}, "limits")
    max_spatial_tokens = limits["max_spatial_tokens"]
    if (
        isinstance(max_spatial_tokens, bool)
        or not isinstance(max_spatial_tokens, int)
        or not 1 <= max_spatial_tokens <= 4096
    ):
        raise ToolkitContractError(
            "operator.descriptor_invalid", "max_spatial_tokens must be in [1, 4096]"
        )
    return ExternalOperatorDescriptor(
        schema_version=schema_version,
        operator_id=operator_id,
        operator_version=operator_version,
        trust=trust,
        entrypoint=entrypoint,
        supported_profiles=tuple(profiles),
        controls=controls,
        max_spatial_tokens=max_spatial_tokens,
        topology=topology,
        input_count=input_count,
        capabilities=capabilities,
        bypass=bypass,
    )


def get_operator_descriptor_schema() -> dict[str, Any]:
    """Return an independent copy of the machine-readable closed schema."""

    resource = files(__package__).joinpath("operator-descriptor.schema.json")
    value = json.loads(resource.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise RuntimeError("packaged operator descriptor schema is not an object")
    return value


def _parse_controls(
    descriptor: ExternalOperatorDescriptor,
    raw: Mapping[str, object] | None,
) -> dict[str, object]:
    if raw is not None and (
        not isinstance(raw, Mapping) or not all(isinstance(name, str) for name in raw)
    ):
        raise ToolkitContractError("operator.control_invalid", "controls must be an object")
    supplied = dict(raw or {})
    specifications = {control.name: control for control in descriptor.controls}
    unknown = sorted(set(supplied) - set(specifications))
    if unknown:
        raise ToolkitContractError(
            "operator.control_invalid", f"unknown controls: {', '.join(unknown)}"
        )
    result: dict[str, object] = {}
    for name in sorted(specifications):
        specification = specifications[name]
        value = supplied.get(name, specification.default)
        if specification.kind == "enum":
            if not isinstance(value, str) or value not in specification.values:
                raise ToolkitContractError(
                    "operator.control_invalid", f"control {name} is outside its enum"
                )
        else:
            try:
                value = _number(value, f"control {name}")
            except ToolkitContractError as error:
                raise ToolkitContractError(
                    "operator.control_invalid", f"control {name} must be finite and numeric"
                ) from error
            if specification.kind == "integer" and (
                isinstance(value, bool) or not isinstance(value, int)
            ):
                raise ToolkitContractError(
                    "operator.control_invalid", f"control {name} must be an integer"
                )
            if value < specification.minimum or value > specification.maximum:  # type: ignore[operator]
                raise ToolkitContractError(
                    "operator.control_invalid", f"control {name} is outside its range"
                )
        result[name] = value
    return result


def _validate_slot(
    value: object,
    label: str,
    descriptor: ExternalOperatorDescriptor,
) -> torch.Tensor:
    if not isinstance(value, torch.Tensor):
        raise ToolkitContractError("operator.tensor_invalid", f"{label} must be a tensor")
    if (
        value.ndim != 5
        or value.shape[0] != 1
        or value.shape[1] != 24
        or value.shape[2] != 1
        or value.shape[3] < 1
        or value.shape[4] < 1
    ):
        raise ToolkitContractError(
            "operator.tensor_invalid", f"{label} must have layout [1,24,1,H,W]"
        )
    if value.shape[3] * value.shape[4] > descriptor.max_spatial_tokens:
        raise ToolkitContractError("operator.tensor_bound", f"{label} exceeds its grid bound")
    if (
        value.dtype != torch.float16
        or value.layout is not torch.strided
        or value.device.type not in {"cpu", "cuda"}
    ):
        raise ToolkitContractError(
            "operator.tensor_invalid", f"{label} must be dense F16 on CPU or CUDA"
        )
    if not bool(torch.isfinite(value).all().item()):
        raise ToolkitContractError("operator.tensor_non_finite", f"{label} contains NaN or Inf")
    return value


@dataclass(frozen=True, slots=True)
class InstalledOperator:
    descriptor: ExternalOperatorDescriptor
    _implementation: OperatorCallable | LegacyDualOperatorCallable
    _legacy_dual: bool

    @torch.inference_mode()
    def process_sources(
        self,
        sources: tuple[torch.Tensor, ...],
        controls: Mapping[str, object] | None = None,
        context: OperatorContext | None = None,
    ) -> ToolkitOperatorResult:
        if not isinstance(sources, tuple) or len(sources) != self.descriptor.input_count:
            raise ToolkitContractError(
                "operator.source_count_invalid",
                "sources must be a tuple matching the declared input_count",
            )
        checked_sources = tuple(
            _validate_slot(source, f"source[{index}]", self.descriptor)
            for index, source in enumerate(sources)
        )
        first = checked_sources[0]
        if any(
            source.shape != first.shape or source.device != first.device
            for source in checked_sources[1:]
        ):
            raise ToolkitContractError(
                "operator.tensor_incompatible", "all sources must match exactly"
            )
        parsed_controls = _parse_controls(self.descriptor, controls)
        parsed_context = context or OperatorContext()
        if not isinstance(parsed_context, OperatorContext):
            raise ToolkitContractError(
                "operator.context_invalid", "context must be an OperatorContext"
            )
        parsed_context.validate(self.descriptor)
        bypass_controls = dict(self.descriptor.bypass.controls)
        bypass_active = all(
            parsed_controls[name] == value for name, value in bypass_controls.items()
        )
        if bypass_active:
            result = ToolkitOperatorResult(
                checked_sources[self.descriptor.bypass.output_source].clone().contiguous(),
                {
                    "operation": {
                        "operator_id": self.descriptor.operator_id,
                        "operator_version": self.descriptor.operator_version,
                        "seed": parsed_context.seed,
                        "controls": parsed_controls,
                        "bypass": True,
                    },
                    "topology": self.descriptor.topology,
                    "processing_mode": parsed_context.processing_mode,
                    "slot_index": parsed_context.slot_index,
                    "output_source": self.descriptor.bypass.output_source,
                },
            )
        else:
            result = self._run_implementation(checked_sources, parsed_controls, parsed_context)
        return self._validate_result(result, checked_sources[0])

    def _run_implementation(
        self,
        checked_sources: tuple[torch.Tensor, ...],
        parsed_controls: dict[str, object],
        parsed_context: OperatorContext,
    ) -> ToolkitOperatorResult:
        try:
            if self._legacy_dual:
                legacy = self._implementation
                result = legacy(  # type: ignore[call-arg]
                    checked_sources[0],
                    checked_sources[1],
                    parsed_controls,
                    parsed_context,
                )
            else:
                implementation = self._implementation
                result = implementation(  # type: ignore[call-arg]
                    checked_sources,
                    parsed_controls,
                    parsed_context,
                )
        except ToolkitContractError:
            raise
        except Exception as error:
            raise ToolkitContractError(
                "operator.execution_failed", "installed operator execution failed"
            ) from error
        return result

    def _validate_result(
        self, result: ToolkitOperatorResult, first: torch.Tensor
    ) -> ToolkitOperatorResult:
        if not isinstance(result, ToolkitOperatorResult):
            raise ToolkitContractError(
                "operator.result_invalid", "operator returned the wrong result type"
            )
        output = _validate_slot(result.output, "output", self.descriptor)
        if (
            output.shape != first.shape
            or output.dtype != first.dtype
            or output.device != first.device
            or not output.is_contiguous()
        ):
            raise ToolkitContractError(
                "operator.result_invalid", "operator changed the tensor contract"
            )
        if not isinstance(result.provenance, dict):
            raise ToolkitContractError(
                "operator.provenance_invalid", "operator provenance must be an object"
            )
        operation = result.provenance.get("operation")
        if not isinstance(operation, dict) or (
            operation.get("operator_id") != self.descriptor.operator_id
            or operation.get("operator_version") != self.descriptor.operator_version
        ):
            raise ToolkitContractError(
                "operator.provenance_invalid", "operator provenance identity changed"
            )
        try:
            encoded = json.dumps(
                result.provenance,
                allow_nan=False,
                separators=(",", ":"),
                sort_keys=True,
            ).encode("utf-8")
        except (TypeError, ValueError) as error:
            raise ToolkitContractError(
                "operator.provenance_invalid", "operator provenance is not JSON-safe"
            ) from error
        if len(encoded) > MAX_OPERATOR_PROVENANCE_BYTES:
            raise ToolkitContractError(
                "operator.provenance_invalid", "operator provenance exceeds its byte bound"
            )
        return result

    def process_single(
        self,
        source: torch.Tensor,
        controls: Mapping[str, object] | None = None,
        context: OperatorContext | None = None,
    ) -> ToolkitOperatorResult:
        """Run a descriptor-declared single-source operator."""

        if self.descriptor.topology != "single_source":
            raise ToolkitContractError(
                "operator.topology_mismatch", "process_single requires single_source topology"
            )
        return self.process_sources((source,), controls, context)

    def process_carrier_donors(
        self,
        carrier: torch.Tensor,
        donors: tuple[torch.Tensor, ...],
        controls: Mapping[str, object] | None = None,
        context: OperatorContext | None = None,
    ) -> ToolkitOperatorResult:
        """Run a carrier followed by its descriptor-declared ordered donors."""

        if self.descriptor.topology != "carrier_donors":
            raise ToolkitContractError(
                "operator.topology_mismatch",
                "process_carrier_donors requires carrier_donors topology",
            )
        if not isinstance(donors, tuple):
            raise ToolkitContractError(
                "operator.source_count_invalid", "donors must be an ordered tuple"
            )
        return self.process_sources((carrier, *donors), controls, context)

    def process_dual(
        self,
        carrier: torch.Tensor,
        donor: torch.Tensor,
        controls: Mapping[str, object] | None = None,
        context: OperatorContext | None = None,
    ) -> ToolkitOperatorResult:
        """Run a descriptor-declared dual-source operator."""

        if self.descriptor.topology != "dual_source":
            raise ToolkitContractError(
                "operator.topology_mismatch", "process_dual requires dual_source topology"
            )
        return self.process_sources((carrier, donor), controls, context)

    def process_slot(
        self,
        carrier: torch.Tensor,
        donor: torch.Tensor,
        controls: Mapping[str, object] | None = None,
        context: OperatorContext | None = None,
    ) -> ToolkitOperatorResult:
        """Compatibility wrapper for the original dual-source 0.1 draft."""

        return self.process_dual(carrier, donor, controls, context)


class TrustedOperatorRegistry:
    """Registry populated only by explicit host calls with already-imported code."""

    def __init__(self) -> None:
        self._operators: dict[str, InstalledOperator] = {}

    def install(
        self,
        raw_descriptor: Mapping[str, object],
        implementation: OperatorCallable | LegacyDualOperatorCallable,
        *,
        exported_entrypoint: str,
    ) -> None:
        descriptor = validate_external_descriptor(raw_descriptor)
        if descriptor.entrypoint != exported_entrypoint:
            raise ToolkitContractError(
                "operator.entrypoint_mismatch",
                "installed callable does not match the declared entrypoint",
            )
        if not callable(implementation):
            raise ToolkitContractError(
                "operator.implementation_invalid", "implementation must be callable"
            )
        try:
            parameters = tuple(signature(implementation).parameters.values())
        except (TypeError, ValueError) as error:
            raise ToolkitContractError(
                "operator.implementation_invalid", "implementation signature is unavailable"
            ) from error
        positional = tuple(
            parameter
            for parameter in parameters
            if parameter.kind in {Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD}
        )
        has_non_positional = any(
            parameter.kind not in {Parameter.POSITIONAL_ONLY, Parameter.POSITIONAL_OR_KEYWORD}
            for parameter in parameters
        )
        if has_non_positional:
            raise ToolkitContractError(
                "operator.implementation_invalid",
                "implementation must use an exact positional callable contract",
            )
        if len(positional) == 3:
            legacy_dual = False
        elif len(positional) == 4 and descriptor.topology == "dual_source":
            legacy_dual = True
        else:
            raise ToolkitContractError(
                "operator.implementation_invalid",
                "implementation must accept sources, controls, context",
            )
        if descriptor.operator_id in self._operators:
            raise ToolkitContractError("operator.duplicate", "operator ID is already installed")
        self._operators[descriptor.operator_id] = InstalledOperator(
            descriptor, implementation, legacy_dual
        )

    def load(self, operator_id: str, operator_version: str) -> InstalledOperator:
        operator = self._operators.get(operator_id)
        if operator is None:
            raise ToolkitContractError(
                "operator.not_installed", "operator is not explicitly installed"
            )
        if operator.descriptor.operator_version != operator_version:
            raise ToolkitContractError(
                "operator.version_mismatch", "installed operator version does not match"
            )
        return operator

    def descriptors(self) -> tuple[ExternalOperatorDescriptor, ...]:
        return tuple(self._operators[name].descriptor for name in sorted(self._operators))


__all__ = [
    "BypassDescriptor",
    "MAX_OPERATOR_PROVENANCE_BYTES",
    "OPERATOR_API_VERSION",
    "ControlDescriptor",
    "ExternalOperatorDescriptor",
    "InstalledOperator",
    "OperatorContext",
    "OperatorCapabilities",
    "ProfileSupport",
    "ToolkitOperatorResult",
    "TrustedOperatorRegistry",
    "get_operator_descriptor_schema",
    "validate_external_descriptor",
]
