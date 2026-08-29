"""UI-independent LD-D2 latent stream state and causal reset barriers."""

from __future__ import annotations

import copy
import json
import uuid
from collections.abc import Callable, Mapping
from dataclasses import dataclass
from typing import Literal, Protocol

import torch

from .contract import (
    MAX_SAFE_SEED,
    D2Context,
    D2ContractError,
    D2Controls,
)
from .trusted import LoadedOperator, OperatorLoadError

H3_PROFILE = {
    "codec_family": "minimax_h3",
    "profile": "h3_av_latent",
    "profile_version": "0.1.0",
    "timing_contract": "minimax_h3_causal",
    "timing_contract_version": "0.1.0",
    "frame_rate_numerator": 24,
    "frame_rate_denominator": 1,
    "runtime_dtype": "F16",
}

MAX_STREAM_GENERATION = 0xFFFF_FFFF_FFFF_FFFF


class D2StreamError(RuntimeError):
    """A stable stream transition or trusted-source failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


@dataclass(frozen=True, slots=True)
class D2Source:
    """A validated source identity plus a trusted H3 slot reader."""

    cartridge_id: str
    archive_sha256: str
    shape: tuple[int, int, int, int, int]
    read_slot: Callable[[int], torch.Tensor]
    codec_family: str = "minimax_h3"
    profile: str = "h3_av_latent"
    profile_version: str = "0.1.0"
    timing_contract: str = "minimax_h3_causal"
    timing_contract_version: str = "0.1.0"
    frame_rate_numerator: int = 24
    frame_rate_denominator: int = 1
    runtime_dtype: str = "F16"

    @property
    def latent_slot_count(self) -> int:
        return self.shape[2]


@dataclass(frozen=True, slots=True)
class D2Transport:
    playing_a: bool = True
    playing_b: bool = True
    loop_a: bool = True
    loop_b: bool = True

    def validate(self) -> None:
        for name in ("playing_a", "playing_b", "loop_a", "loop_b"):
            if not isinstance(getattr(self, name), bool):
                raise D2StreamError("deck.transport_invalid", f"{name} must be boolean")


@dataclass(frozen=True, slots=True)
class D2ProcessedSlot:
    kind: Literal["processed_slot"]
    stream_generation: int
    stream_sequence: int
    playhead_a: int
    playhead_b: int
    output: torch.Tensor
    provenance: dict[str, object]


@dataclass(frozen=True, slots=True)
class D2ResetBarrier:
    kind: Literal["reset_barrier"]
    current_generation: int
    minimum_new_generation: int
    reasons: tuple[str, ...]

    def as_dict(self) -> dict[str, object]:
        return {
            "kind": self.kind,
            "current_generation": self.current_generation,
            "minimum_new_generation": self.minimum_new_generation,
            "reasons": list(self.reasons),
        }


@dataclass(frozen=True, slots=True)
class D2Paused:
    kind: Literal["paused"]
    stream_generation: int
    playhead_a: int
    playhead_b: int

    def as_dict(self) -> dict[str, object]:
        return {
            "kind": self.kind,
            "stream_generation": self.stream_generation,
            "playhead_a": self.playhead_a,
            "playhead_b": self.playhead_b,
        }


D2Step = D2ProcessedSlot | D2ResetBarrier | D2Paused


class CausalSlotDecoder(Protocol):
    """The only decoder surface used by the D2 pre-decode pump."""

    def decode_slot(self, slot: torch.Tensor) -> object: ...

    def reset(self) -> None: ...


@dataclass(frozen=True, slots=True)
class D2DecodedSlot:
    kind: Literal["decoded_slot"]
    latent: D2ProcessedSlot
    decoded: object


def _canonical_uuid(value: str, label: str) -> None:
    try:
        parsed = uuid.UUID(value)
    except (AttributeError, ValueError) as error:
        raise D2StreamError("deck.source_invalid", f"{label} is not a UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise D2StreamError("deck.source_invalid", f"{label} is not canonical")


def _validate_source(source: D2Source, *, max_spatial_tokens: int) -> None:
    _canonical_uuid(source.cartridge_id, "cartridge_id")
    if len(source.archive_sha256) != 64 or any(
        character not in "0123456789abcdef" for character in source.archive_sha256
    ):
        raise D2StreamError("deck.source_invalid", "archive_sha256 is not canonical")
    if not callable(source.read_slot):
        raise D2StreamError("deck.source_invalid", "source slot reader is not callable")
    if (
        len(source.shape) != 5
        or any(isinstance(axis, bool) or not isinstance(axis, int) for axis in source.shape)
        or any(axis <= 0 for axis in source.shape)
        or source.shape[0] != 1
        or source.shape[1] != 24
        or source.shape[2] < 2
        or (source.shape[2] - 2) % 5
    ):
        raise D2StreamError("deck.source_invalid", "source shape is not H3 [1,24,2+5n,H,W]")
    if source.shape[3] * source.shape[4] > max_spatial_tokens:
        raise D2StreamError("deck.source_too_large", "source exceeds operator full-grid bound")
    for name, expected in H3_PROFILE.items():
        if getattr(source, name) != expected:
            raise D2StreamError("deck.source_incompatible", f"source {name} is incompatible")


class D2StreamEngine:
    """Produce one post-operator H3 slot at a time, before TAEH3 decode.

    Source wrapping never happens inside :meth:`step`. A loop or explicit
    restart first yields a typed reset barrier. The caller must reset the
    causal decoder successfully through :meth:`apply_reset_barrier` before any
    slot from the new generation can be produced.
    """

    def __init__(
        self,
        operator: LoadedOperator,
        source_a: D2Source,
        source_b: D2Source,
        *,
        controls: D2Controls | Mapping[str, object] | None = None,
        transport: D2Transport | None = None,
        seed: int = 0,
        stream_generation: int = 1,
    ) -> None:
        if (
            isinstance(stream_generation, bool)
            or not isinstance(stream_generation, int)
            or not 1 <= stream_generation <= MAX_STREAM_GENERATION
        ):
            raise D2StreamError("deck.generation_invalid", "generation must be a nonzero u64")
        self._validate_operator(operator)
        _validate_source(
            source_a,
            max_spatial_tokens=operator.descriptor.limit("max_spatial_tokens"),
        )
        _validate_source(
            source_b,
            max_spatial_tokens=operator.descriptor.limit("max_spatial_tokens"),
        )
        if source_a.shape[3:] != source_b.shape[3:]:
            raise D2StreamError(
                "deck.source_incompatible", "A and B latent spatial geometry differs"
            )
        self._operator = operator
        self._sources = (source_a, source_b)
        self._controls = (
            controls if isinstance(controls, D2Controls) else D2Controls.from_mapping(controls)
        )
        self._controls.validate()
        self._transport = transport or D2Transport()
        self._transport.validate()
        self._seed = self._validated_seed(seed)
        self._stream_generation = stream_generation
        self._stream_sequence = 0
        self._positions = [0, 0]
        self._previous: list[torch.Tensor | None] = [None, None]
        self._pending_reasons: tuple[str, ...] = ()
        self._restart_pending = False

    @staticmethod
    def _validate_operator(operator: LoadedOperator) -> None:
        if not isinstance(operator, LoadedOperator):
            raise D2StreamError("operator.not_trusted", "a registered trusted operator is required")
        expected = {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
            "timing_contract": "minimax_h3_causal",
            "timing_contract_version": "0.1.0",
            "layout": "[1,24,1,H,W]",
            "runtime_dtype": "F16",
        }
        if not any(
            all(getattr(profile, name) == value for name, value in expected.items())
            for profile in operator.descriptor.supported_profiles
        ):
            raise D2StreamError("operator.profile_incompatible", "operator lacks H3 0.1 support")

    @staticmethod
    def _validated_seed(seed: int) -> int:
        if isinstance(seed, bool) or not isinstance(seed, int) or not 0 <= seed <= MAX_SAFE_SEED:
            raise D2StreamError("deck.seed_invalid", "seed is outside the safe integer range")
        return seed

    @property
    def controls(self) -> D2Controls:
        return self._controls

    @property
    def transport(self) -> D2Transport:
        return self._transport

    @property
    def stream_generation(self) -> int:
        return self._stream_generation

    def update_controls(self, controls: D2Controls | Mapping[str, object]) -> dict[str, object]:
        parsed = controls if isinstance(controls, D2Controls) else D2Controls.from_mapping(controls)
        parsed.validate()
        self._controls = parsed
        return {"controls": parsed.as_dict(), "requires_causal_reset": False}

    def update_transport(self, transport: D2Transport) -> dict[str, object]:
        transport.validate()
        self._transport = transport
        return {"transport": self._transport_dict(), "requires_causal_reset": False}

    def update_seed(self, seed: int) -> dict[str, object]:
        self._seed = self._validated_seed(seed)
        return {"seed": self._seed, "requires_causal_reset": False}

    def request_restart(self) -> D2ResetBarrier:
        if self._stream_generation == MAX_STREAM_GENERATION:
            raise D2StreamError("deck.generation_exhausted", "stream generation is exhausted")
        self._restart_pending = True
        self._pending_reasons = ("transport.restart",)
        return self._barrier()

    def step(self) -> D2Step:
        if self._pending_reasons:
            return self._barrier()
        self._settle_ends()
        if self._pending_reasons:
            return self._barrier()

        playing = (self._transport.playing_a, self._transport.playing_b)
        if not any(playing):
            return D2Paused(
                "paused",
                self._stream_generation,
                self._positions[0],
                self._positions[1],
            )
        slot_a = self._read_slot(0)
        slot_b = self._read_slot(1)
        context = D2Context(
            playhead_a=self._positions[0],
            playhead_b=self._positions[1],
            seed=self._seed,
            previous_a=self._previous[0],
            previous_b=self._previous[1],
        )
        try:
            result = self._operator.process_slot(slot_a, slot_b, self._controls, context)
        except (D2ContractError, OperatorLoadError) as error:
            raise D2StreamError("deck.process_failed", error.code) from error

        sequence = self._stream_sequence
        positions = tuple(self._positions)
        provenance = copy.deepcopy(result.provenance)
        provenance["stream"] = {
            "generation": self._stream_generation,
            "sequence": sequence,
            "sources": {
                "a": self._source_provenance(0, positions[0]),
                "b": self._source_provenance(1, positions[1]),
            },
        }
        json.dumps(provenance, allow_nan=False, separators=(",", ":"))
        self._previous = [slot_a, slot_b]
        if playing[0]:
            self._positions[0] += 1
        if playing[1]:
            self._positions[1] += 1
        self._stream_sequence += 1
        return D2ProcessedSlot(
            "processed_slot",
            self._stream_generation,
            sequence,
            positions[0],
            positions[1],
            result.output,
            provenance,
        )

    def apply_reset_barrier(
        self,
        new_stream_generation: int,
        causal_decoder_reset: Callable[[], None],
    ) -> dict[str, object]:
        if not self._pending_reasons:
            raise D2StreamError("deck.reset_not_required", "no causal reset barrier is pending")
        if (
            isinstance(new_stream_generation, bool)
            or not isinstance(new_stream_generation, int)
            or new_stream_generation <= self._stream_generation
        ):
            raise D2StreamError("deck.generation_stale", "new generation must be strictly greater")
        if new_stream_generation > MAX_STREAM_GENERATION:
            raise D2StreamError("deck.generation_invalid", "new generation exceeds u64")
        if not callable(causal_decoder_reset):
            raise D2StreamError("deck.reset_invalid", "causal decoder reset is not callable")

        reasons = self._pending_reasons
        try:
            causal_decoder_reset()
        except Exception as error:
            raise D2StreamError("deck.reset_failed", "causal decoder reset failed") from error
        if self._restart_pending:
            self._positions = [0, 0]
        else:
            loops = (self._transport.loop_a, self._transport.loop_b)
            for index, source in enumerate(self._sources):
                if loops[index] and self._positions[index] >= source.latent_slot_count:
                    self._positions[index] = 0
        self._previous = [None, None]
        self._stream_generation = new_stream_generation
        self._stream_sequence = 0
        self._pending_reasons = ()
        self._restart_pending = False
        result = {
            "kind": "reset_applied",
            "stream_generation": self._stream_generation,
            "playhead_a": self._positions[0],
            "playhead_b": self._positions[1],
            "reasons": list(reasons),
            "causal_state_cleared": True,
        }
        json.dumps(result, allow_nan=False, separators=(",", ":"))
        return result

    def status(self) -> dict[str, object]:
        result = {
            "stream_generation": self._stream_generation,
            "stream_sequence": self._stream_sequence,
            "playhead_a": self._positions[0],
            "playhead_b": self._positions[1],
            "transport": self._transport_dict(),
            "controls": self._controls.as_dict(),
            "seed": self._seed,
            "pending_reset": bool(self._pending_reasons),
            "pending_reset_reasons": list(self._pending_reasons),
        }
        json.dumps(result, allow_nan=False, separators=(",", ":"))
        return result

    def _settle_ends(self) -> None:
        playing = [self._transport.playing_a, self._transport.playing_b]
        loops = (self._transport.loop_a, self._transport.loop_b)
        reasons: list[str] = []
        for index, source in enumerate(self._sources):
            if not playing[index] or self._positions[index] < source.latent_slot_count:
                continue
            if loops[index]:
                reasons.append(f"slot_{'ab'[index]}.loop")
            else:
                playing[index] = False
                self._positions[index] = source.latent_slot_count - 1
        if reasons:
            self._pending_reasons = tuple(reasons)
        if tuple(playing) != (self._transport.playing_a, self._transport.playing_b):
            self._transport = D2Transport(
                playing_a=playing[0],
                playing_b=playing[1],
                loop_a=self._transport.loop_a,
                loop_b=self._transport.loop_b,
            )

    def _read_slot(self, source_index: int) -> torch.Tensor:
        source = self._sources[source_index]
        position = self._positions[source_index]
        try:
            slot = source.read_slot(position)
        except Exception as error:
            raise D2StreamError("deck.source_read_failed", "trusted source read failed") from error
        if not isinstance(slot, torch.Tensor):
            raise D2StreamError("deck.source_read_failed", "trusted source returned no tensor")
        return slot

    def _barrier(self) -> D2ResetBarrier:
        if self._stream_generation == MAX_STREAM_GENERATION:
            raise D2StreamError("deck.generation_exhausted", "stream generation is exhausted")
        return D2ResetBarrier(
            "reset_barrier",
            self._stream_generation,
            self._stream_generation + 1,
            self._pending_reasons,
        )

    def _transport_dict(self) -> dict[str, bool]:
        return {
            "playing_a": self._transport.playing_a,
            "playing_b": self._transport.playing_b,
            "loop_a": self._transport.loop_a,
            "loop_b": self._transport.loop_b,
        }

    def _source_provenance(self, source_index: int, playhead: int) -> dict[str, object]:
        source = self._sources[source_index]
        return {
            "cartridge_id": source.cartridge_id,
            "archive_sha256": source.archive_sha256,
            "playhead": playhead,
        }


class D2DecodePump:
    """Guarantee operator processing occurs before the causal H3 decoder."""

    def __init__(self, engine: D2StreamEngine, decoder: CausalSlotDecoder) -> None:
        if not isinstance(engine, D2StreamEngine):
            raise D2StreamError("deck.engine_invalid", "D2 stream engine is required")
        if not callable(getattr(decoder, "decode_slot", None)) or not callable(
            getattr(decoder, "reset", None)
        ):
            raise D2StreamError("deck.decoder_invalid", "causal slot decoder is invalid")
        self._engine = engine
        self._decoder = decoder

    def step(
        self,
        before_decode: Callable[[D2ProcessedSlot], None] | None = None,
    ) -> D2DecodedSlot | D2ResetBarrier | D2Paused:
        step = self._engine.step()
        if not isinstance(step, D2ProcessedSlot):
            return step
        if before_decode is not None:
            if not callable(before_decode):
                raise D2StreamError("deck.sink_invalid", "pre-decode sink is not callable")
            before_decode(step)
        try:
            decoded = self._decoder.decode_slot(step.output)
        except Exception as error:
            raise D2StreamError("deck.decode_failed", "causal H3 decode failed") from error
        return D2DecodedSlot("decoded_slot", step, decoded)

    def apply_reset_barrier(
        self,
        new_stream_generation: int,
        after_decoder_reset: Callable[[], None] | None = None,
    ) -> dict[str, object]:
        if after_decoder_reset is not None and not callable(after_decoder_reset):
            raise D2StreamError("deck.reset_invalid", "post-reset callback is not callable")

        def reset() -> None:
            self._decoder.reset()
            if after_decoder_reset is not None:
                after_decoder_reset()

        return self._engine.apply_reset_barrier(new_stream_generation, reset)


__all__ = [
    "D2Paused",
    "D2DecodePump",
    "D2DecodedSlot",
    "D2ProcessedSlot",
    "D2ResetBarrier",
    "D2Source",
    "D2Step",
    "D2StreamEngine",
    "D2StreamError",
    "D2Transport",
    "H3_PROFILE",
    "MAX_STREAM_GENERATION",
]
