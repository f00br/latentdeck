"""Typed LD-D2 wrapper over the dependency-free trusted Operator API."""

from __future__ import annotations

import json
from collections.abc import Callable, Mapping
from dataclasses import dataclass

import torch
from latentdeck_codec_host.operator_api import (
    BuiltinOperatorRegistry as GenericBuiltinRegistry,
)
from latentdeck_codec_host.operator_api import (
    ControlDescriptor,
    OperatorDescriptor,
    OperatorLoadError,
    ProfileSupport,
    validate_descriptor,
)
from latentdeck_codec_host.operator_api import (
    LoadedOperator as GenericLoadedOperator,
)

from .contract import D2Context, D2ContractError, D2Controls, ProcessResult
from .descriptor import get_descriptor
from .operator import process_slot

OperatorCallable = Callable[
    [
        torch.Tensor,
        torch.Tensor,
        D2Controls | Mapping[str, object] | None,
        D2Context | Mapping[str, object] | None,
    ],
    ProcessResult,
]


@dataclass(frozen=True, slots=True)
class LoadedOperator:
    """A registered operator whose D2 result contract is checked on every call."""

    _registered: GenericLoadedOperator

    @property
    def descriptor(self) -> OperatorDescriptor:
        return self._registered.descriptor

    def process_slot(
        self,
        a: torch.Tensor,
        b: torch.Tensor,
        controls: D2Controls | Mapping[str, object] | None = None,
        context: D2Context | Mapping[str, object] | None = None,
    ) -> ProcessResult:
        try:
            result = self._registered.invoke(a, b, controls, context)
        except (D2ContractError, OperatorLoadError):
            raise
        except Exception as error:
            raise OperatorLoadError(
                "operator.process_failed", "trusted operator execution failed"
            ) from error
        if not isinstance(result, ProcessResult):
            raise OperatorLoadError("operator.result_invalid", "operator returned the wrong type")
        if not isinstance(result.output, torch.Tensor):
            raise OperatorLoadError("operator.result_invalid", "operator returned no tensor")
        if (
            result.output.shape != a.shape
            or result.output.dtype != a.dtype
            or result.output.device != a.device
            or result.output.layout is not torch.strided
            or not result.output.is_contiguous()
        ):
            raise OperatorLoadError(
                "operator.result_invalid", "operator changed the tensor contract"
            )
        if not bool(torch.isfinite(result.output).all().item()):
            raise OperatorLoadError("operator.result_invalid", "operator returned NaN or Inf")
        if not isinstance(result.provenance, dict):
            raise OperatorLoadError(
                "operator.provenance_invalid", "operator provenance must be an object"
            )
        try:
            json.dumps(result.provenance, allow_nan=False, separators=(",", ":"))
        except (TypeError, ValueError) as error:
            raise OperatorLoadError(
                "operator.provenance_invalid", "operator provenance is not JSON-safe"
            ) from error
        operation = result.provenance.get("operation")
        if not isinstance(operation, dict) or (
            operation.get("operator_id") != self.descriptor.operator_id
            or operation.get("operator_version") != self.descriptor.operator_version
        ):
            raise OperatorLoadError(
                "operator.provenance_invalid", "operator provenance identity changed"
            )
        return result


class BuiltinOperatorRegistry:
    """D2-typed view of the application-populated generic builtin allowlist."""

    def __init__(self) -> None:
        self._registry = GenericBuiltinRegistry()

    def register(
        self,
        raw_descriptor: Mapping[str, object],
        implementation: OperatorCallable,
        *,
        exported_entrypoint: str,
    ) -> None:
        self._registry.register(
            raw_descriptor,
            implementation,
            exported_entrypoint=exported_entrypoint,
        )

    def load(self, operator_id: str, operator_version: str) -> LoadedOperator:
        return LoadedOperator(self._registry.load(operator_id, operator_version))

    def descriptors(self) -> tuple[OperatorDescriptor, ...]:
        return self._registry.descriptors()


def builtin_registry() -> BuiltinOperatorRegistry:
    """Build the explicit public 0.1 D2 builtin allowlist."""

    registry = BuiltinOperatorRegistry()
    registry.register(
        get_descriptor(),
        process_slot,
        exported_entrypoint="latentdeck_operator_d2:process_slot",
    )
    return registry


__all__ = [
    "BuiltinOperatorRegistry",
    "ControlDescriptor",
    "LoadedOperator",
    "OperatorDescriptor",
    "OperatorLoadError",
    "ProfileSupport",
    "builtin_registry",
    "validate_descriptor",
]
