"""H3 source binding for the isolated, pre-decode LD-D2 stream engine."""

from __future__ import annotations

import math
import uuid
from collections.abc import Callable, Mapping
from typing import Any

from latentdeck_operator_d2 import (
    MAX_SPATIAL_TOKENS,
    D2DecodedSlot,
    D2DecodePump,
    D2Paused,
    D2ProcessedSlot,
    D2ResetBarrier,
    D2Source,
    D2StreamEngine,
    D2Transport,
    builtin_registry,
)

from .cartridge import H3VideoSource


class H3D2SourceError(RuntimeError):
    """A validated H3 source could not become a finite F16 runtime source."""


def _validate_source_metadata(source: H3VideoSource) -> None:
    if not isinstance(source, H3VideoSource):
        raise H3D2SourceError("H3 source descriptor is incompatible")
    if source.storage_dtype not in {"F16", "F32"}:
        raise H3D2SourceError("H3 source storage dtype is incompatible")
    if (
        not isinstance(source.shape, tuple)
        or len(source.shape) != 5
        or any(isinstance(axis, bool) or not isinstance(axis, int) for axis in source.shape)
        or any(axis <= 0 for axis in source.shape)
        or source.shape[0] != 1
        or source.shape[1] != 24
        or source.shape[2] < 2
        or (source.shape[2] - 2) % 5
    ):
        raise H3D2SourceError("H3 source shape is incompatible")
    if source.shape[3] * source.shape[4] > MAX_SPATIAL_TOKENS:
        raise H3D2SourceError("H3 source exceeds the full-grid operator bound")
    byte_width = 2 if source.storage_dtype == "F16" else 4
    if not isinstance(source.video_bytes, bytes) or len(source.video_bytes) != (
        math.prod(source.shape) * byte_width
    ):
        raise H3D2SourceError("H3 source byte range is incompatible")
    if not isinstance(source.cartridge_id, str):
        raise H3D2SourceError("H3 cartridge ID is not canonical")
    try:
        cartridge_id = uuid.UUID(source.cartridge_id)
    except (AttributeError, ValueError) as error:
        raise H3D2SourceError("H3 cartridge ID is not canonical") from error
    if cartridge_id.int == 0 or str(cartridge_id) != source.cartridge_id:
        raise H3D2SourceError("H3 cartridge ID is not canonical")
    if (
        not isinstance(source.archive_sha256, str)
        or len(source.archive_sha256) != 64
        or any(character not in "0123456789abcdef" for character in source.archive_sha256)
    ):
        raise H3D2SourceError("H3 archive hash is not canonical")
    expected_frames = 5 + 17 * ((source.shape[2] - 2) // 5)
    metadata_numbers = (
        source.width,
        source.height,
        source.frame_count,
        source.frame_rate_numerator,
        source.frame_rate_denominator,
    )
    if (
        any(isinstance(value, bool) or not isinstance(value, int) for value in metadata_numbers)
        or any(value <= 0 for value in metadata_numbers)
        or source.width != source.shape[4] * 16
        or source.height != source.shape[3] * 16
        or source.frame_count != expected_frames
        or source.frame_rate_numerator != 24
        or source.frame_rate_denominator != 1
    ):
        raise H3D2SourceError("H3 presentation metadata is incompatible")


class H3TensorDeckSource:
    """Own one full validated visual tensor on the selected runtime device."""

    def __init__(self, source: H3VideoSource, torch: Any, device: Any) -> None:
        _validate_source_metadata(source)
        storage_dtype = torch.float16 if source.storage_dtype == "F16" else torch.float32
        try:
            # Torch warns (and permits writes) when a tensor views immutable
            # ``bytes``. A short-lived writable copy makes ownership explicit;
            # the runtime tensor below is then an independent allocation.
            stored = torch.frombuffer(bytearray(source.video_bytes), dtype=storage_dtype).reshape(
                source.shape
            )
            # F32 -> F16 is the one profile-authorized runtime cast. Perform it
            # and its finite check on CPU before any GPU allocation.
            runtime_cpu = stored.to(dtype=torch.float16, device="cpu").clone().contiguous()
            if not bool(torch.isfinite(runtime_cpu).all().item()):
                raise H3D2SourceError("H3 runtime cast produced NaN or Inf")
            self._video = runtime_cpu.to(device=device, dtype=torch.float16).contiguous()
        except H3D2SourceError:
            raise
        except Exception as error:
            raise H3D2SourceError("H3 visual source could not be materialized") from error
        self._source = source
        self._torch = torch

    def descriptor(self) -> D2Source:
        return D2Source(
            cartridge_id=self._source.cartridge_id,
            archive_sha256=self._source.archive_sha256,
            shape=self._source.shape,
            read_slot=self.read_slot,
        )

    def read_slot(self, position: int) -> Any:
        if not 0 <= position < self._source.latent_slot_count:
            raise H3D2SourceError("H3 playhead is outside the source")
        return self._video[:, :, position : position + 1]

    def close(self) -> None:
        self._video = None


class H3D2StreamEngine:
    """Bind two exact H3 cartridges, synthesize F16 slots, then decode them."""

    def __init__(
        self,
        source_a: H3VideoSource,
        source_b: H3VideoSource,
        decoder: Any,
        *,
        torch: Any,
        device: Any,
        controls: Mapping[str, object] | None = None,
        transport: D2Transport | None = None,
        seed: int = 0,
        stream_generation: int = 1,
        operator_id: str = "org.latentdeck.builtin.ld_d2",
        operator_version: str = "0.1.0",
    ) -> None:
        registry = builtin_registry()
        operator = registry.load(operator_id, operator_version)
        _validate_source_metadata(source_a)
        _validate_source_metadata(source_b)
        if source_a.shape[3:] != source_b.shape[3:]:
            raise H3D2SourceError("A and B latent spatial geometry differs")
        self._source_a = H3TensorDeckSource(source_a, torch, device)
        try:
            self._source_b = H3TensorDeckSource(source_b, torch, device)
            self._engine = D2StreamEngine(
                operator,
                self._source_a.descriptor(),
                self._source_b.descriptor(),
                controls=controls,
                transport=transport,
                seed=seed,
                stream_generation=stream_generation,
            )
            self._pump = D2DecodePump(self._engine, decoder)
        except Exception:
            self._source_a.close()
            source_b_tensor = getattr(self, "_source_b", None)
            if source_b_tensor is not None:
                source_b_tensor.close()
            raise

    def step(
        self,
        before_decode: Callable[[D2ProcessedSlot], None] | None = None,
    ) -> D2DecodedSlot | D2ResetBarrier | D2Paused:
        """Produce/decode one slot or return a causal reset/paused event."""

        return self._pump.step(before_decode)

    def apply_reset_barrier(
        self,
        new_stream_generation: int,
        after_decoder_reset: Callable[[], None] | None = None,
    ) -> dict[str, object]:
        return self._pump.apply_reset_barrier(new_stream_generation, after_decoder_reset)

    def request_restart(self) -> D2ResetBarrier:
        return self._engine.request_restart()

    def update_controls(self, controls: Mapping[str, object]) -> dict[str, object]:
        return self._engine.update_controls(controls)

    def update_transport(self, transport: D2Transport) -> dict[str, object]:
        return self._engine.update_transport(transport)

    def update_seed(self, seed: int) -> dict[str, object]:
        return self._engine.update_seed(seed)

    def status(self) -> dict[str, object]:
        return self._engine.status()

    def close(self) -> None:
        self._source_a.close()
        self._source_b.close()


__all__ = [
    "H3D2SourceError",
    "H3D2StreamEngine",
    "H3TensorDeckSource",
]
