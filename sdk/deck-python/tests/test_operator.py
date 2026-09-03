from __future__ import annotations

from dataclasses import replace

import pytest

from latentdeck_deck_sdk import (
    DeckContractError,
    DeckOperatorContext,
    DeckOperatorResult,
    RoleBinding,
    process_sources_checked,
    validate_process_call,
)


class FakeScalar:
    def __init__(self, value: bool) -> None:
        self._value = value

    def item(self) -> bool:
        return self._value


class FakeFinite:
    def __init__(self, value: bool) -> None:
        self._value = value

    def all(self) -> FakeScalar:
        return FakeScalar(self._value)


class FakeTensor:
    def __init__(
        self,
        *,
        shape: tuple[int, ...] = (1, 4, 1, 8, 8),
        dtype: str = "float16",
        device: str = "cpu",
        contiguous: bool = True,
        finite: bool = True,
    ) -> None:
        self.shape = shape
        self.dtype = dtype
        self.device = device
        self._contiguous = contiguous
        self.finite = finite

    def is_contiguous(self) -> bool:
        return self._contiguous


class FakeTorch:
    float16 = "float16"
    bfloat16 = "bfloat16"
    float32 = "float32"

    @staticmethod
    def is_tensor(value: object) -> bool:
        return isinstance(value, FakeTensor)

    @staticmethod
    def isfinite(value: FakeTensor) -> FakeFinite:
        return FakeFinite(value.finite)

    @staticmethod
    def stack(values: tuple[FakeScalar, ...]) -> FakeFinite:
        return FakeFinite(all(value._value for value in values))


def context(source_count: int = 2) -> DeckOperatorContext:
    return DeckOperatorContext(
        codec_family="synthetic",
        profile="test_latent",
        profile_version="0.1.0",
        timing_contract="synthetic_causal",
        timing_contract_version="0.1.0",
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        generation=1,
        sequence=1,
        seed=7,
        playheads=tuple(range(source_count)),
        physical_slots=tuple(range(1, source_count + 1)),
        roles=(RoleBinding("carrier", 1), RoleBinding("donor", 2)),
        previous_sources=(None,) * source_count,
    )


def test_checked_operator_preserves_the_exact_tensor_abi() -> None:
    sources = (FakeTensor(), FakeTensor())

    def operator(
        received: tuple[object, ...],
        controls: dict[str, object],
        _context: DeckOperatorContext,
    ) -> DeckOperatorResult:
        assert controls == {"mix": 0.5}
        return DeckOperatorResult(
            output=received[0],
            provenance={"operator_id": "org.example.synthetic", "slot": 1},
        )

    result = process_sources_checked(
        operator,
        sources,
        {"mix": 0.5},
        context(),
        torch_module=FakeTorch,
    )
    assert result.output is sources[0]


@pytest.mark.parametrize(
    ("source", "code"),
    [
        (FakeTensor(shape=(1, 4, 8, 8)), "tensor.shape"),
        (FakeTensor(contiguous=False), "tensor.non_contiguous"),
        (FakeTensor(finite=False), "tensor.non_finite"),
        (FakeTensor(dtype="int8"), "tensor.dtype"),
    ],
)
def test_invalid_tensor_contracts_fail_without_repair(source: FakeTensor, code: str) -> None:
    with pytest.raises(DeckContractError, match=code):
        validate_process_call(
            (source, FakeTensor()),
            {},
            context(),
            torch_module=FakeTorch,
        )


def test_non_finite_history_is_rejected_by_the_aggregate_input_gate() -> None:
    invalid = replace(
        context(),
        previous_sources=(FakeTensor(finite=False), FakeTensor()),
    )
    with pytest.raises(DeckContractError, match="tensor.non_finite"):
        validate_process_call(
            (FakeTensor(), FakeTensor()),
            {},
            invalid,
            torch_module=FakeTorch,
        )


def test_incompatible_sources_are_not_cast_or_resized() -> None:
    with pytest.raises(DeckContractError, match="tensor.incompatible"):
        validate_process_call(
            (FakeTensor(), FakeTensor(shape=(1, 4, 1, 4, 8))),
            {},
            context(),
            torch_module=FakeTorch,
        )


def test_operator_output_must_preserve_source_shape_dtype_and_device() -> None:
    def operator(
        _sources: tuple[object, ...],
        _controls: dict[str, object],
        _context: DeckOperatorContext,
    ) -> DeckOperatorResult:
        return DeckOperatorResult(
            FakeTensor(shape=(1, 4, 1, 4, 8)),
            {"operator_id": "org.example.synthetic"},
        )

    with pytest.raises(DeckContractError, match="result.tensor_abi"):
        process_sources_checked(
            operator,
            (FakeTensor(), FakeTensor()),
            {},
            context(),
            torch_module=FakeTorch,
        )


def test_context_history_and_roles_are_bounded_by_physical_sources() -> None:
    invalid = replace(context(), previous_sources=(None,), roles=(RoleBinding("carrier", 1),))
    with pytest.raises(DeckContractError, match="context.source_count"):
        validate_process_call(
            (FakeTensor(), FakeTensor()),
            {},
            invalid,
            torch_module=FakeTorch,
        )


def test_non_finite_controls_and_provenance_are_rejected() -> None:
    with pytest.raises(DeckContractError, match="control.non_finite"):
        validate_process_call(
            (FakeTensor(), FakeTensor()),
            {"mix": float("nan")},
            context(),
            torch_module=FakeTorch,
        )

    def operator(
        received: tuple[object, ...],
        _controls: dict[str, object],
        _context: DeckOperatorContext,
    ) -> DeckOperatorResult:
        return DeckOperatorResult(received[0], {"metric": float("inf")})

    with pytest.raises(DeckContractError, match="provenance.non_finite"):
        process_sources_checked(
            operator,
            (FakeTensor(), FakeTensor()),
            {},
            context(),
            torch_module=FakeTorch,
        )


def test_source_count_is_strictly_one_to_sixteen() -> None:
    with pytest.raises(DeckContractError, match="tensor.source_count"):
        validate_process_call((), {}, context(), torch_module=FakeTorch)
    with pytest.raises(DeckContractError, match="tensor.source_count"):
        validate_process_call(
            (FakeTensor(),) * 17,
            {},
            context(17),
            torch_module=FakeTorch,
        )
