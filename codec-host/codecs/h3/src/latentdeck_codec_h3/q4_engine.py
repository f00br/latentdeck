"""H3 source binding for the isolated, pre-decode LD-Q4 stream engine."""

from __future__ import annotations

from collections.abc import Callable, Mapping
from typing import Any

from latentdeck_operator_q4.stream import (
    Q4DecodedSlot,
    Q4DecodePump,
    Q4Paused,
    Q4ProcessedSlot,
    Q4ResetBarrier,
    Q4RoleAssignment,
    Q4Source,
    Q4StreamEngine,
    Q4Transport,
)
from latentdeck_operator_q4.trusted import builtin_registry

from .cartridge import H3VideoSource
from .d2_engine import H3D2SourceError, H3TensorDeckSource


class H3Q4SourceError(RuntimeError):
    """A validated H3 source could not become a finite F16 Q4 source."""


class H3Q4TensorSource:
    """Q4 identity wrapper over the shared strict H3 tensor materializer."""

    def __init__(self, source: H3VideoSource, torch: Any, device: Any) -> None:
        try:
            self._tensor_source = H3TensorDeckSource(source, torch, device)
        except H3D2SourceError as error:
            raise H3Q4SourceError("H3 visual source could not be materialized") from error
        self._source = source

    def descriptor(self) -> Q4Source:
        return Q4Source(
            cartridge_id=self._source.cartridge_id,
            archive_sha256=self._source.archive_sha256,
            shape=self._source.shape,
            read_slot=self._tensor_source.read_slot,
        )

    def close(self) -> None:
        self._tensor_source.close()


class H3Q4StreamEngine:
    """Bind four exact H3 cartridges, synthesize F16 slots, then decode them."""

    def __init__(
        self,
        source_a: H3VideoSource,
        source_b: H3VideoSource,
        source_c: H3VideoSource,
        source_d: H3VideoSource,
        decoder: Any,
        *,
        torch: Any,
        device: Any,
        roles: Q4RoleAssignment | Mapping[str, object] | None = None,
        controls: Mapping[str, object] | None = None,
        transport: Q4Transport | None = None,
        seed: int = 0,
        stream_generation: int = 1,
        operator_id: str = "org.latentdeck.builtin.ld_q4",
        operator_version: str = "0.1.0",
    ) -> None:
        registry = builtin_registry()
        operator = registry.load(operator_id, operator_version)
        source_values = (source_a, source_b, source_c, source_d)
        if any(not isinstance(source, H3VideoSource) for source in source_values):
            raise H3Q4SourceError("Q4 source descriptor is incompatible")
        reference_geometry = (source_a.shape[3:], source_a.width, source_a.height)
        if any(
            (source.shape[3:], source.width, source.height) != reference_geometry
            for source in source_values[1:]
        ):
            raise H3Q4SourceError("Q4 latent or presentation geometry differs")
        materialized: list[H3Q4TensorSource] = []
        try:
            for source in source_values:
                materialized.append(H3Q4TensorSource(source, torch, device))
            self._engine = Q4StreamEngine(
                operator,
                *(source.descriptor() for source in materialized),
                roles=roles,
                controls=controls,
                transport=transport,
                seed=seed,
                stream_generation=stream_generation,
            )
            self._pump = Q4DecodePump(self._engine, decoder)
        except Exception:
            for source in materialized:
                source.close()
            raise
        self._sources = tuple(materialized)

    def step(
        self,
        before_decode: Callable[[Q4ProcessedSlot], None] | None = None,
    ) -> Q4DecodedSlot | Q4ResetBarrier | Q4Paused:
        """Produce/decode one Q4 slot or return a causal reset/paused event."""

        return self._pump.step(before_decode)

    def apply_reset_barrier(
        self,
        new_stream_generation: int,
        after_decoder_reset: Callable[[], None] | None = None,
    ) -> dict[str, object]:
        return self._pump.apply_reset_barrier(new_stream_generation, after_decoder_reset)

    def request_restart(self) -> Q4ResetBarrier:
        return self._engine.request_restart()

    def update_controls(self, controls: Mapping[str, object]) -> dict[str, object]:
        return self._engine.update_controls(controls)

    def update_roles(self, roles: Q4RoleAssignment | Mapping[str, object]) -> dict[str, object]:
        return self._engine.update_roles(roles)

    def update_transport(self, transport: Q4Transport) -> dict[str, object]:
        return self._engine.update_transport(transport)

    def update_seed(self, seed: int) -> dict[str, object]:
        return self._engine.update_seed(seed)

    def status(self) -> dict[str, object]:
        return self._engine.status()

    def close(self) -> None:
        for source in self._sources:
            source.close()


__all__ = [
    "H3Q4SourceError",
    "H3Q4StreamEngine",
    "H3Q4TensorSource",
]
