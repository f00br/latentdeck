"""UI-independent LD-Q4 stream state and causal reset barriers."""

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
    DeckSlot,
    Q4Context,
    Q4ContractError,
    Q4Controls,
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
_PHYSICAL_SLOTS = (DeckSlot.A, DeckSlot.B, DeckSlot.C, DeckSlot.D)


class Q4StreamError(RuntimeError):
    """Stable, path-free stream transition or trusted-source failure."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


@dataclass(frozen=True, slots=True)
class Q4Source:
    """Validated physical source identity plus a trusted H3 slot reader."""

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
class Q4RoleAssignment:
    """Explicit carrier and logical B/C/D donor mapping over physical A-D."""

    carrier: DeckSlot = DeckSlot.A
    donor_b: DeckSlot = DeckSlot.B
    donor_c: DeckSlot = DeckSlot.C
    donor_d: DeckSlot = DeckSlot.D

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> Q4RoleAssignment:
        if raw is None:
            return cls()
        if not isinstance(raw, Mapping) or set(raw) != {
            "carrier",
            "donor_b",
            "donor_c",
            "donor_d",
        }:
            raise Q4StreamError("deck.roles_invalid", "Q4 roles must use the closed role schema")
        try:
            roles = cls(
                carrier=DeckSlot(raw["carrier"]),
                donor_b=DeckSlot(raw["donor_b"]),
                donor_c=DeckSlot(raw["donor_c"]),
                donor_d=DeckSlot(raw["donor_d"]),
            )
        except (TypeError, ValueError) as error:
            raise Q4StreamError(
                "deck.roles_invalid", "Q4 roles must reference physical slots A through D"
            ) from error
        roles.validate()
        return roles

    def validate(self) -> None:
        slots = (self.carrier, self.donor_b, self.donor_c, self.donor_d)
        if any(not isinstance(slot, DeckSlot) for slot in slots) or set(slots) != set(
            _PHYSICAL_SLOTS
        ):
            raise Q4StreamError(
                "deck.roles_invalid", "Q4 roles must be an exact A/B/C/D permutation"
            )

    def as_dict(self) -> dict[str, str]:
        self.validate()
        return {
            "carrier": self.carrier.value,
            "donor_b": self.donor_b.value,
            "donor_c": self.donor_c.value,
            "donor_d": self.donor_d.value,
        }


@dataclass(frozen=True, slots=True)
class Q4Transport:
    playing_a: bool = True
    playing_b: bool = True
    playing_c: bool = True
    playing_d: bool = True
    loop_a: bool = True
    loop_b: bool = True
    loop_c: bool = True
    loop_d: bool = True

    @classmethod
    def from_mapping(cls, raw: Mapping[str, object] | None) -> Q4Transport:
        if raw is None:
            return cls()
        fields = {
            "playing_a",
            "playing_b",
            "playing_c",
            "playing_d",
            "loop_a",
            "loop_b",
            "loop_c",
            "loop_d",
        }
        if not isinstance(raw, Mapping) or set(raw) != fields:
            raise Q4StreamError(
                "deck.transport_invalid", "Q4 transport must use the closed eight-flag schema"
            )
        transport = cls(**{name: raw[name] for name in fields})  # type: ignore[arg-type]
        transport.validate()
        return transport

    def validate(self) -> None:
        if any(not isinstance(value, bool) for value in self.playing_flags() + self.loop_flags()):
            raise Q4StreamError("deck.transport_invalid", "Q4 transport flags must be boolean")

    def playing_flags(self) -> tuple[bool, bool, bool, bool]:
        return (self.playing_a, self.playing_b, self.playing_c, self.playing_d)

    def loop_flags(self) -> tuple[bool, bool, bool, bool]:
        return (self.loop_a, self.loop_b, self.loop_c, self.loop_d)

    def as_dict(self) -> dict[str, bool]:
        self.validate()
        return {
            "playing_a": self.playing_a,
            "playing_b": self.playing_b,
            "playing_c": self.playing_c,
            "playing_d": self.playing_d,
            "loop_a": self.loop_a,
            "loop_b": self.loop_b,
            "loop_c": self.loop_c,
            "loop_d": self.loop_d,
        }


@dataclass(frozen=True, slots=True)
class Q4ProcessedSlot:
    kind: Literal["processed_slot"]
    stream_generation: int
    stream_sequence: int
    playhead_a: int
    playhead_b: int
    playhead_c: int
    playhead_d: int
    roles: Q4RoleAssignment
    output: torch.Tensor
    provenance: dict[str, object]


@dataclass(frozen=True, slots=True)
class Q4ResetBarrier:
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
class Q4Paused:
    kind: Literal["paused"]
    stream_generation: int
    playhead_a: int
    playhead_b: int
    playhead_c: int
    playhead_d: int
    roles: Q4RoleAssignment

    def as_dict(self) -> dict[str, object]:
        return {
            "kind": self.kind,
            "stream_generation": self.stream_generation,
            "playhead_a": self.playhead_a,
            "playhead_b": self.playhead_b,
            "playhead_c": self.playhead_c,
            "playhead_d": self.playhead_d,
            "roles": self.roles.as_dict(),
        }


Q4Step = Q4ProcessedSlot | Q4ResetBarrier | Q4Paused


class CausalSlotDecoder(Protocol):
    def decode_slot(self, slot: torch.Tensor) -> object: ...

    def reset(self) -> None: ...


@dataclass(frozen=True, slots=True)
class Q4DecodedSlot:
    kind: Literal["decoded_slot"]
    latent: Q4ProcessedSlot
    decoded: object


def _canonical_uuid(value: str, label: str) -> None:
    try:
        parsed = uuid.UUID(value)
    except (AttributeError, ValueError) as error:
        raise Q4StreamError("deck.source_invalid", f"{label} is not a UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise Q4StreamError("deck.source_invalid", f"{label} is not canonical")


def _validate_source(source: Q4Source, *, max_spatial_tokens: int) -> None:
    if not isinstance(source, Q4Source):
        raise Q4StreamError("deck.source_invalid", "Q4 source descriptor is invalid")
    _canonical_uuid(source.cartridge_id, "cartridge_id")
    if len(source.archive_sha256) != 64 or any(
        character not in "0123456789abcdef" for character in source.archive_sha256
    ):
        raise Q4StreamError("deck.source_invalid", "archive_sha256 is not canonical")
    if not callable(source.read_slot):
        raise Q4StreamError("deck.source_invalid", "source slot reader is not callable")
    if (
        len(source.shape) != 5
        or any(isinstance(axis, bool) or not isinstance(axis, int) for axis in source.shape)
        or any(axis <= 0 for axis in source.shape)
        or source.shape[0] != 1
        or source.shape[1] != 24
        or source.shape[2] < 2
        or (source.shape[2] - 2) % 5
    ):
        raise Q4StreamError("deck.source_invalid", "source shape is not H3 [1,24,2+5n,H,W]")
    if source.shape[3] * source.shape[4] > max_spatial_tokens:
        raise Q4StreamError("deck.source_too_large", "source exceeds operator full-grid bound")
    for name, expected in H3_PROFILE.items():
        if getattr(source, name) != expected:
            raise Q4StreamError("deck.source_incompatible", f"source {name} is incompatible")


class Q4StreamEngine:
    """Produce one post-operator Q4 H3 slot at a time before decode."""

    def __init__(
        self,
        operator: LoadedOperator,
        source_a: Q4Source,
        source_b: Q4Source,
        source_c: Q4Source,
        source_d: Q4Source,
        *,
        roles: Q4RoleAssignment | Mapping[str, object] | None = None,
        controls: Q4Controls | Mapping[str, object] | None = None,
        transport: Q4Transport | Mapping[str, object] | None = None,
        seed: int = 0,
        stream_generation: int = 1,
    ) -> None:
        if (
            isinstance(stream_generation, bool)
            or not isinstance(stream_generation, int)
            or not 1 <= stream_generation <= MAX_STREAM_GENERATION
        ):
            raise Q4StreamError("deck.generation_invalid", "generation must be a nonzero u64")
        self._validate_operator(operator)
        sources = (source_a, source_b, source_c, source_d)
        maximum = operator.descriptor.limit("max_spatial_tokens")
        for source in sources:
            _validate_source(source, max_spatial_tokens=maximum)
        if any(source.shape[3:] != source_a.shape[3:] for source in sources[1:]):
            raise Q4StreamError("deck.source_incompatible", "Q4 latent spatial geometry differs")
        self._operator = operator
        self._sources = sources
        self._roles = (
            roles if isinstance(roles, Q4RoleAssignment) else Q4RoleAssignment.from_mapping(roles)
        )
        self._roles.validate()
        self._controls = (
            controls if isinstance(controls, Q4Controls) else Q4Controls.from_mapping(controls)
        )
        self._controls.validate()
        self._transport = (
            transport if isinstance(transport, Q4Transport) else Q4Transport.from_mapping(transport)
        )
        self._transport.validate()
        self._seed = self._validated_seed(seed)
        self._stream_generation = stream_generation
        self._stream_sequence = 0
        self._positions = [0, 0, 0, 0]
        self._pending_reasons: tuple[str, ...] = ()
        self._restart_pending = False

    @staticmethod
    def _validate_operator(operator: LoadedOperator) -> None:
        if not isinstance(operator, LoadedOperator):
            raise Q4StreamError("operator.not_trusted", "a registered trusted operator is required")
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
            raise Q4StreamError("operator.profile_incompatible", "operator lacks H3 0.1 support")

    @staticmethod
    def _validated_seed(seed: int) -> int:
        if isinstance(seed, bool) or not isinstance(seed, int) or not 0 <= seed <= MAX_SAFE_SEED:
            raise Q4StreamError("deck.seed_invalid", "seed is outside the safe integer range")
        return seed

    @property
    def controls(self) -> Q4Controls:
        return self._controls

    @property
    def roles(self) -> Q4RoleAssignment:
        return self._roles

    @property
    def transport(self) -> Q4Transport:
        return self._transport

    @property
    def stream_generation(self) -> int:
        return self._stream_generation

    def update_controls(self, controls: Q4Controls | Mapping[str, object]) -> dict[str, object]:
        parsed = controls if isinstance(controls, Q4Controls) else Q4Controls.from_mapping(controls)
        parsed.validate()
        self._controls = parsed
        return {"controls": parsed.as_dict(), "requires_causal_reset": False}

    def update_roles(self, roles: Q4RoleAssignment | Mapping[str, object]) -> dict[str, object]:
        parsed = (
            roles if isinstance(roles, Q4RoleAssignment) else Q4RoleAssignment.from_mapping(roles)
        )
        parsed.validate()
        self._roles = parsed
        return {"roles": parsed.as_dict(), "requires_causal_reset": False}

    def update_transport(self, transport: Q4Transport | Mapping[str, object]) -> dict[str, object]:
        parsed = (
            transport if isinstance(transport, Q4Transport) else Q4Transport.from_mapping(transport)
        )
        parsed.validate()
        self._transport = parsed
        return {"transport": parsed.as_dict(), "requires_causal_reset": False}

    def update_seed(self, seed: int) -> dict[str, object]:
        self._seed = self._validated_seed(seed)
        return {"seed": self._seed, "requires_causal_reset": False}

    def request_restart(self) -> Q4ResetBarrier:
        if self._stream_generation == MAX_STREAM_GENERATION:
            raise Q4StreamError("deck.generation_exhausted", "stream generation is exhausted")
        self._restart_pending = True
        self._pending_reasons = ("transport.restart",)
        return self._barrier()

    def step(self) -> Q4Step:
        if self._pending_reasons:
            return self._barrier()
        self._settle_ends()
        if self._pending_reasons:
            return self._barrier()

        playing = self._transport.playing_flags()
        if not any(playing):
            return Q4Paused(
                "paused",
                self._stream_generation,
                *self._positions,
                self._roles,
            )

        # Snapshot all physical inputs and the complete role assignment before
        # invoking the operator. Realtime updates can only replace these values
        # between complete calls, so one processed slot has one immutable
        # carrier reference and fixed logical donor order B, C, D.
        slots = tuple(self._read_slot(index) for index in range(4))
        roles = self._roles
        positions = tuple(self._positions)
        role_indices = tuple(
            _PHYSICAL_SLOTS.index(slot)
            for slot in (
                roles.carrier,
                roles.donor_b,
                roles.donor_c,
                roles.donor_d,
            )
        )
        carrier_index, donor_b_index, donor_c_index, donor_d_index = role_indices
        context = Q4Context(
            carrier_slot=roles.carrier,
            donor_b_slot=roles.donor_b,
            donor_c_slot=roles.donor_c,
            donor_d_slot=roles.donor_d,
            carrier_identity=self._sources[carrier_index].cartridge_id,
            donor_b_identity=self._sources[donor_b_index].cartridge_id,
            donor_c_identity=self._sources[donor_c_index].cartridge_id,
            donor_d_identity=self._sources[donor_d_index].cartridge_id,
            carrier_playhead=positions[carrier_index],
            donor_b_playhead=positions[donor_b_index],
            donor_c_playhead=positions[donor_c_index],
            donor_d_playhead=positions[donor_d_index],
            seed=self._seed,
        )
        try:
            result = self._operator.process_slot(
                slots[carrier_index],
                slots[donor_b_index],
                slots[donor_c_index],
                slots[donor_d_index],
                self._controls,
                context,
            )
        except (Q4ContractError, OperatorLoadError) as error:
            raise Q4StreamError("deck.process_failed", error.code) from error

        sequence = self._stream_sequence
        provenance = copy.deepcopy(result.provenance)
        provenance["stream"] = {
            "generation": self._stream_generation,
            "sequence": sequence,
            "roles": roles.as_dict(),
            "sources": {
                slot.value.lower(): self._source_provenance(index, positions[index])
                for index, slot in enumerate(_PHYSICAL_SLOTS)
            },
        }
        json.dumps(provenance, allow_nan=False, separators=(",", ":"))
        for index, is_playing in enumerate(playing):
            if is_playing:
                self._positions[index] += 1
        self._stream_sequence += 1
        return Q4ProcessedSlot(
            "processed_slot",
            self._stream_generation,
            sequence,
            *positions,
            roles,
            result.output,
            provenance,
        )

    def apply_reset_barrier(
        self,
        new_stream_generation: int,
        causal_decoder_reset: Callable[[], None],
    ) -> dict[str, object]:
        if not self._pending_reasons:
            raise Q4StreamError("deck.reset_not_required", "no causal reset barrier is pending")
        if (
            isinstance(new_stream_generation, bool)
            or not isinstance(new_stream_generation, int)
            or new_stream_generation <= self._stream_generation
        ):
            raise Q4StreamError("deck.generation_stale", "new generation must be strictly greater")
        if new_stream_generation > MAX_STREAM_GENERATION:
            raise Q4StreamError("deck.generation_invalid", "new generation exceeds u64")
        if not callable(causal_decoder_reset):
            raise Q4StreamError("deck.reset_invalid", "causal decoder reset is not callable")

        reasons = self._pending_reasons
        try:
            causal_decoder_reset()
        except Exception as error:
            raise Q4StreamError("deck.reset_failed", "causal decoder reset failed") from error
        if self._restart_pending:
            self._positions = [0, 0, 0, 0]
        else:
            for index, source in enumerate(self._sources):
                looped = self._transport.loop_flags()[index]
                if looped and self._positions[index] >= source.latent_slot_count:
                    self._positions[index] = 0
        self._stream_generation = new_stream_generation
        self._stream_sequence = 0
        self._pending_reasons = ()
        self._restart_pending = False
        result: dict[str, object] = {
            "kind": "reset_applied",
            "stream_generation": self._stream_generation,
            "playhead_a": self._positions[0],
            "playhead_b": self._positions[1],
            "playhead_c": self._positions[2],
            "playhead_d": self._positions[3],
            "reasons": list(reasons),
            "causal_state_cleared": True,
        }
        json.dumps(result, allow_nan=False, separators=(",", ":"))
        return result

    def status(self) -> dict[str, object]:
        result: dict[str, object] = {
            "stream_generation": self._stream_generation,
            "stream_sequence": self._stream_sequence,
            "playhead_a": self._positions[0],
            "playhead_b": self._positions[1],
            "playhead_c": self._positions[2],
            "playhead_d": self._positions[3],
            "roles": self._roles.as_dict(),
            "transport": self._transport.as_dict(),
            "controls": self._controls.as_dict(),
            "seed": self._seed,
            "pending_reset": bool(self._pending_reasons),
            "pending_reset_reasons": list(self._pending_reasons),
        }
        json.dumps(result, allow_nan=False, separators=(",", ":"))
        return result

    def _settle_ends(self) -> None:
        playing = list(self._transport.playing_flags())
        loops = self._transport.loop_flags()
        reasons: list[str] = []
        for index, source in enumerate(self._sources):
            if self._positions[index] < source.latent_slot_count:
                continue
            if playing[index] and loops[index]:
                reasons.append(f"slot_{_PHYSICAL_SLOTS[index].value.lower()}.loop")
            else:
                playing[index] = False
                self._positions[index] = source.latent_slot_count - 1
        if reasons:
            self._pending_reasons = tuple(reasons)
        if tuple(playing) != self._transport.playing_flags():
            self._transport = Q4Transport(
                *playing,
                *self._transport.loop_flags(),
            )

    def _read_slot(self, source_index: int) -> torch.Tensor:
        source = self._sources[source_index]
        position = self._positions[source_index]
        try:
            slot = source.read_slot(position)
        except Exception as error:
            raise Q4StreamError("deck.source_read_failed", "trusted source read failed") from error
        if not isinstance(slot, torch.Tensor):
            raise Q4StreamError("deck.source_read_failed", "trusted source returned no tensor")
        return slot

    def _barrier(self) -> Q4ResetBarrier:
        if self._stream_generation == MAX_STREAM_GENERATION:
            raise Q4StreamError("deck.generation_exhausted", "stream generation is exhausted")
        return Q4ResetBarrier(
            "reset_barrier",
            self._stream_generation,
            self._stream_generation + 1,
            self._pending_reasons,
        )

    def _source_provenance(self, source_index: int, playhead: int) -> dict[str, object]:
        source = self._sources[source_index]
        return {
            "cartridge_id": source.cartridge_id,
            "archive_sha256": source.archive_sha256,
            "playhead": playhead,
        }


class Q4DecodePump:
    """Guarantee Q4 operator processing occurs before causal H3 decode."""

    def __init__(self, engine: Q4StreamEngine, decoder: CausalSlotDecoder) -> None:
        if not isinstance(engine, Q4StreamEngine):
            raise Q4StreamError("deck.engine_invalid", "Q4 stream engine is required")
        if not callable(getattr(decoder, "decode_slot", None)) or not callable(
            getattr(decoder, "reset", None)
        ):
            raise Q4StreamError("deck.decoder_invalid", "causal slot decoder is invalid")
        self._engine = engine
        self._decoder = decoder

    def step(
        self,
        before_decode: Callable[[Q4ProcessedSlot], None] | None = None,
    ) -> Q4DecodedSlot | Q4ResetBarrier | Q4Paused:
        step = self._engine.step()
        if not isinstance(step, Q4ProcessedSlot):
            return step
        if before_decode is not None:
            if not callable(before_decode):
                raise Q4StreamError("deck.sink_invalid", "pre-decode sink is not callable")
            before_decode(step)
        try:
            decoded = self._decoder.decode_slot(step.output)
        except Exception as error:
            raise Q4StreamError("deck.decode_failed", "causal H3 decode failed") from error
        return Q4DecodedSlot("decoded_slot", step, decoded)

    def apply_reset_barrier(
        self,
        new_stream_generation: int,
        after_decoder_reset: Callable[[], None] | None = None,
    ) -> dict[str, object]:
        if after_decoder_reset is not None and not callable(after_decoder_reset):
            raise Q4StreamError("deck.reset_invalid", "post-reset callback is not callable")

        def reset() -> None:
            self._decoder.reset()
            if after_decoder_reset is not None:
                after_decoder_reset()

        return self._engine.apply_reset_barrier(new_stream_generation, reset)


__all__ = [
    "H3_PROFILE",
    "MAX_STREAM_GENERATION",
    "Q4DecodePump",
    "Q4DecodedSlot",
    "Q4Paused",
    "Q4ProcessedSlot",
    "Q4ResetBarrier",
    "Q4RoleAssignment",
    "Q4Source",
    "Q4Step",
    "Q4StreamEngine",
    "Q4StreamError",
    "Q4Transport",
]
