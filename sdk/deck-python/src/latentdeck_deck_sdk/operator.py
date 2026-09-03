"""Generic one-to-sixteen-source Deck operator boundary."""

from __future__ import annotations

import importlib
import math
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from typing import Protocol, runtime_checkable

MAX_SOURCES = 16
MAX_CONTROLS = 64
MAX_SAFE_SEED = 9_007_199_254_740_991
MAX_IDENTIFIER_BYTES = 128
MAX_PROVENANCE_DEPTH = 8
MAX_PROVENANCE_ITEMS = 256
MAX_PROVENANCE_FIELDS = 64
MAX_PROVENANCE_STRING_BYTES = 4_096
MAX_PROVENANCE_NODES = 2_048


class DeckContractError(ValueError):
    """A stable path-free Deck operator contract failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


def _identifier(value: object, field: str) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > MAX_IDENTIFIER_BYTES:
        raise DeckContractError("context.identity", f"{field} is not a bounded identifier")
    allowed = frozenset("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:-")
    if any(character not in allowed for character in value):
        raise DeckContractError("context.identity", f"{field} contains an invalid character")
    return value


def _integer(value: object, field: str, minimum: int, maximum: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not minimum <= value <= maximum:
        raise DeckContractError("context.integer", f"{field} is outside [{minimum}, {maximum}]")
    return value


@dataclass(frozen=True, slots=True)
class RoleBinding:
    role: str
    physical_slot: int

    def validate(self, source_count: int) -> None:
        _identifier(self.role, "role")
        _integer(self.physical_slot, "physical_slot", 1, source_count)


@dataclass(frozen=True, slots=True)
class DeckOperatorContext:
    codec_family: str
    profile: str
    profile_version: str
    timing_contract: str
    timing_contract_version: str
    frame_rate_numerator: int
    frame_rate_denominator: int
    generation: int
    sequence: int
    seed: int
    playheads: tuple[int, ...]
    physical_slots: tuple[int, ...]
    roles: tuple[RoleBinding, ...]
    previous_sources: tuple[object | None, ...]

    def validate(self, source_count: int) -> None:
        for field in (
            "codec_family",
            "profile",
            "profile_version",
            "timing_contract",
            "timing_contract_version",
        ):
            _identifier(getattr(self, field), field)
        _integer(self.frame_rate_numerator, "frame_rate_numerator", 1, 2**32 - 1)
        _integer(self.frame_rate_denominator, "frame_rate_denominator", 1, 2**32 - 1)
        _integer(self.generation, "generation", 1, 2**64 - 1)
        _integer(self.sequence, "sequence", 1, 2**64 - 1)
        _integer(self.seed, "seed", 0, MAX_SAFE_SEED)
        if not (
            len(self.playheads)
            == len(self.physical_slots)
            == len(self.previous_sources)
            == source_count
        ):
            raise DeckContractError(
                "context.source_count", "playheads, slots, and history must match sources"
            )
        if tuple(sorted(self.physical_slots)) != tuple(range(1, source_count + 1)):
            raise DeckContractError(
                "context.physical_slots", "physical slots must be one permutation of 1..N"
            )
        for playhead in self.playheads:
            _integer(playhead, "playhead", 0, MAX_SAFE_SEED)
        role_names: set[str] = set()
        for role in self.roles:
            role.validate(source_count)
            if role.role in role_names:
                raise DeckContractError("context.role_duplicate", "role names must be unique")
            role_names.add(role.role)


@dataclass(frozen=True, slots=True)
class DeckOperatorResult:
    output: object
    provenance: Mapping[str, object]


@runtime_checkable
class DeckOperator(Protocol):
    def __call__(
        self,
        sources: tuple[object, ...],
        controls: dict[str, object],
        context: DeckOperatorContext,
    ) -> DeckOperatorResult: ...


def _torch(torch_module: object | None) -> object:
    if torch_module is not None:
        return torch_module
    try:
        return importlib.import_module("torch")
    except ModuleNotFoundError as error:
        raise DeckContractError(
            "tensor.torch_unavailable",
            "tensor validation requires the codec runtime's declared Torch build",
        ) from error


def _validate_tensor(
    value: object,
    label: str,
    torch_module: object,
    *,
    check_finite: bool = True,
) -> None:
    if not torch_module.is_tensor(value):
        raise DeckContractError("tensor.type", f"{label} must be a torch.Tensor")
    shape = tuple(value.shape)
    if (
        len(shape) != 5
        or shape[0] != 1
        or shape[2] != 1
        or any(
            isinstance(dimension, bool) or not isinstance(dimension, int) or dimension <= 0
            for dimension in shape
        )
    ):
        raise DeckContractError("tensor.shape", f"{label} must have shape [1,C,1,H,W]")
    allowed_dtypes = {
        torch_module.float16,
        torch_module.bfloat16,
        torch_module.float32,
    }
    if value.dtype not in allowed_dtypes:
        raise DeckContractError("tensor.dtype", f"{label} dtype is outside the tensor ABI")
    if str(value.device).split(":", maxsplit=1)[0] not in {"cpu", "cuda"}:
        raise DeckContractError("tensor.device", f"{label} device is outside the tensor ABI")
    if not value.is_contiguous():
        raise DeckContractError("tensor.non_contiguous", f"{label} must be contiguous")
    if check_finite and not bool(torch_module.isfinite(value).all().item()):
        raise DeckContractError("tensor.non_finite", f"{label} contains NaN or Inf")


def _validate_controls(controls: Mapping[str, object]) -> dict[str, object]:
    if not isinstance(controls, Mapping) or len(controls) > MAX_CONTROLS:
        raise DeckContractError(
            "control.count", "controls must be an object with at most 64 fields"
        )
    parsed: dict[str, object] = {}
    for name, value in controls.items():
        _identifier(name, "control name")
        if isinstance(value, bool | int):
            parsed[name] = value
        elif isinstance(value, float):
            if not math.isfinite(value):
                raise DeckContractError("control.non_finite", f"{name} must be finite")
            parsed[name] = value
        elif isinstance(value, str):
            if len(value.encode()) > MAX_PROVENANCE_STRING_BYTES or "\0" in value:
                raise DeckContractError("control.text", f"{name} exceeds the text bound")
            parsed[name] = value
        else:
            raise DeckContractError("control.type", f"{name} must be a scalar JSON value")
    return parsed


def _validate_provenance(value: object) -> None:
    nodes = 0

    def visit(item: object, depth: int) -> None:
        nonlocal nodes
        nodes += 1
        if nodes > MAX_PROVENANCE_NODES:
            raise DeckContractError("provenance.nodes", "provenance exceeds its node bound")
        if depth > MAX_PROVENANCE_DEPTH:
            raise DeckContractError("provenance.depth", "provenance exceeds its depth bound")
        if item is None or isinstance(item, bool | int):
            return
        if isinstance(item, float):
            if not math.isfinite(item):
                raise DeckContractError("provenance.non_finite", "provenance must be finite")
            return
        if isinstance(item, str):
            if len(item.encode()) > MAX_PROVENANCE_STRING_BYTES or "\0" in item:
                raise DeckContractError("provenance.text", "provenance text exceeds its bound")
            return
        if isinstance(item, Mapping):
            if len(item) > MAX_PROVENANCE_FIELDS:
                raise DeckContractError("provenance.fields", "provenance object is too large")
            for key, child in item.items():
                _identifier(key, "provenance key")
                visit(child, depth + 1)
            return
        if isinstance(item, Sequence) and not isinstance(item, bytes | bytearray | memoryview):
            if len(item) > MAX_PROVENANCE_ITEMS:
                raise DeckContractError("provenance.items", "provenance array is too large")
            for child in item:
                visit(child, depth + 1)
            return
        raise DeckContractError("provenance.type", "provenance must be bounded JSON data")

    visit(value, 0)


def validate_process_call(
    sources: tuple[object, ...],
    controls: Mapping[str, object],
    context: DeckOperatorContext,
    *,
    torch_module: object | None = None,
) -> dict[str, object]:
    """Validate inputs without cast, resize, copy, or device movement."""

    if not isinstance(sources, tuple) or not 1 <= len(sources) <= MAX_SOURCES:
        raise DeckContractError(
            "tensor.source_count", "sources must be a tuple containing 1..16 tensors"
        )
    torch_runtime = _torch(torch_module)
    context.validate(len(sources))
    finite_checks: list[object] = []
    for index, source in enumerate(sources, start=1):
        _validate_tensor(source, f"source {index}", torch_runtime, check_finite=False)
        finite_checks.append(torch_runtime.isfinite(source).all())
    reference = sources[0]
    for index, source in enumerate(sources[1:], start=2):
        if (
            tuple(source.shape) != tuple(reference.shape)
            or source.dtype != reference.dtype
            or source.device != reference.device
        ):
            raise DeckContractError(
                "tensor.incompatible", f"source {index} does not match source 1 exactly"
            )
    for index, previous in enumerate(context.previous_sources, start=1):
        if previous is None:
            continue
        _validate_tensor(
            previous,
            f"previous source {index}",
            torch_runtime,
            check_finite=False,
        )
        # An independently supplied operator may mutate a tensor retained as
        # history. Include history in the same asynchronous aggregate gate as
        # current sources so modular operators remain untrusted without adding
        # another host synchronization per slot.
        finite_checks.append(torch_runtime.isfinite(previous).all())
        if (
            tuple(previous.shape) != tuple(reference.shape)
            or previous.dtype != reference.dtype
            or previous.device != reference.device
        ):
            raise DeckContractError(
                "tensor.previous_incompatible",
                f"previous source {index} does not match current sources",
            )
    # All tensors share one device by contract. Kernel launches remain
    # asynchronous; the complete current+history set crosses the host boundary
    # exactly once here.
    finite_sources = torch_runtime.stack(tuple(finite_checks))
    if not bool(finite_sources.all().item()):
        raise DeckContractError(
            "tensor.non_finite", "a current or previous source contains NaN or Inf"
        )
    return _validate_controls(controls)


def validate_process_result(
    result: DeckOperatorResult,
    sources: tuple[object, ...],
    *,
    torch_module: object | None = None,
) -> DeckOperatorResult:
    """Enforce exact output ABI and bounded data-only provenance."""

    if not isinstance(result, DeckOperatorResult):
        raise DeckContractError("result.type", "operator must return DeckOperatorResult")
    if not sources:
        raise DeckContractError("tensor.source_count", "result validation requires source 1")
    torch_runtime = _torch(torch_module)
    _validate_tensor(result.output, "output", torch_runtime)
    reference = sources[0]
    if (
        tuple(result.output.shape) != tuple(reference.shape)
        or result.output.dtype != reference.dtype
        or result.output.device != reference.device
    ):
        raise DeckContractError(
            "result.tensor_abi", "output must preserve source shape, dtype, and device exactly"
        )
    if not isinstance(result.provenance, Mapping):
        raise DeckContractError("provenance.type", "provenance must be a JSON object")
    _validate_provenance(result.provenance)
    return result


def process_sources_checked(
    operator: Callable[
        [tuple[object, ...], dict[str, object], DeckOperatorContext], DeckOperatorResult
    ],
    sources: tuple[object, ...],
    controls: Mapping[str, object],
    context: DeckOperatorContext,
    *,
    torch_module: object | None = None,
) -> DeckOperatorResult:
    """Run one operator call with identical pre- and post-contract gates."""

    parsed_controls = validate_process_call(sources, controls, context, torch_module=torch_module)
    result = operator(sources, parsed_controls, context)
    return validate_process_result(result, sources, torch_module=torch_module)
