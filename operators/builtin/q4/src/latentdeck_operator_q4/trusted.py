"""Typed LD-Q4 wrapper over the dependency-free trusted Operator API."""

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

from .contract import ProcessResult, Q4Context, Q4ContractError, Q4Controls
from .descriptor import get_descriptor
from .operator import process_slot

OperatorCallable = Callable[
    [
        torch.Tensor,
        torch.Tensor,
        torch.Tensor,
        torch.Tensor,
        Q4Controls | Mapping[str, object] | None,
        Q4Context | Mapping[str, object] | None,
    ],
    ProcessResult,
]


@dataclass(frozen=True, slots=True)
class LoadedOperator:
    """Registered Q4 operator with a checked tensor and provenance result."""

    _registered: GenericLoadedOperator

    @property
    def descriptor(self) -> OperatorDescriptor:
        return self._registered.descriptor

    def process_slot(
        self,
        carrier: torch.Tensor,
        donor_b: torch.Tensor,
        donor_c: torch.Tensor,
        donor_d: torch.Tensor,
        controls: Q4Controls | Mapping[str, object] | None = None,
        context: Q4Context | Mapping[str, object] | None = None,
    ) -> ProcessResult:
        try:
            result = self._registered.invoke(
                carrier,
                donor_b,
                donor_c,
                donor_d,
                controls,
                context,
            )
        except (Q4ContractError, OperatorLoadError):
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
            result.output.shape != carrier.shape
            or result.output.dtype != carrier.dtype
            or result.output.device != carrier.device
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
    """Q4-typed view of the application-populated builtin allowlist."""

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
    """Build the explicit public 0.1 Q4 builtin allowlist."""

    registry = BuiltinOperatorRegistry()
    registry.register(
        get_descriptor(),
        process_slot,
        exported_entrypoint="latentdeck_operator_q4:process_slot",
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
