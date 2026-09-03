"""UI-independent state machine for the trusted H3 codec worker."""

from __future__ import annotations

import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from .cartridge import H3VideoSource, load_video_source
from .decoder import (
    DecodedCycle,
    H3Decoder,
    RuntimeInspection,
    configure_torch_cpu_threads,
    inspect_runtime,
)

WORKER_VERSION = "0.1.0"
ADAPTER_ID = "org.latentdeck.h3"
ADAPTER_VERSION = "0.1.0"
PACK_ID = "org.latentdeck.h3"
PROFILE = {
    "codec_family": "minimax_h3",
    "profile": "h3_av_latent",
    "profile_version": "0.1.0",
}


class Decoder(Protocol):
    def bind_source(self, source: H3VideoSource) -> None: ...
    def reset(self) -> None: ...
    def decode_cycle(self, cycle_index: int) -> DecodedCycle: ...
    def close(self) -> None: ...


class RingProducer(Protocol):
    @property
    def write_sequence(self) -> int: ...
    @property
    def read_sequence(self) -> int: ...
    @property
    def occupancy(self) -> int: ...
    @property
    def presentation_skipped_total(self) -> int: ...
    def can_publish(self, frame_count: int) -> bool: ...
    def publish_cycle(
        self,
        frames: Sequence[bytes],
        *,
        stream_generation: int,
        cycle_index: int,
        decoded_start_frame: int,
    ) -> tuple[int, int]: ...
    def set_generation(self, stream_generation: int) -> None: ...
    def close(self) -> None: ...


class WorkerCommandError(RuntimeError):
    """Stable command failure returned without local paths or stack traces."""

    def __init__(
        self,
        code: str,
        message: str,
        *,
        retryable: bool = False,
        fatal: bool = False,
        diagnostic_code: str | None = None,
        diagnostic_detail: str | None = None,
    ) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.retryable = retryable
        self.fatal = fatal
        self.diagnostic_code = diagnostic_code
        self.diagnostic_detail = diagnostic_detail


@dataclass
class WorkerMetrics:
    started_ns: int
    decode_batches_total: int = 0
    decoded_frames_total: int = 0
    ring_backpressure_total: int = 0
    last_decode_duration_ns: int = 0


class H3WorkerState:
    """One codec, one slot, one ring, and one in-flight decode cycle."""

    def __init__(
        self,
        *,
        decoder_factory: Callable[[str, str, int, int], Decoder] | None = None,
        source_loader: Callable[[str | Path, str], H3VideoSource] = load_video_source,
        ring_factory: Callable[[Mapping[str, object], H3VideoSource, int], RingProducer]
        | None = None,
        runtime_inspector: Callable[[], RuntimeInspection] = inspect_runtime,
    ) -> None:
        self._decoder_factory = decoder_factory or self._load_decoder
        self._source_loader = source_loader
        self._ring_factory = ring_factory or self._bind_ring
        self._runtime_inspector = runtime_inspector
        self._configured = False
        self._decoder: Decoder | None = None
        self._source: H3VideoSource | None = None
        self._ring: RingProducer | None = None
        self._slot_id: str | None = None
        self._slot_revision = 0
        self._stream_generation: int | None = None
        self._next_cycle_index = 0
        self._shutdown = False
        self._worker_state = "handshaking"
        self._codec_state = "unloaded"
        self._slot_state = "empty"
        self._ring_state = "unbound"
        self._device_ordinal: int | None = None
        self._metrics = WorkerMetrics(time.monotonic_ns())

    @property
    def shutdown_requested(self) -> bool:
        return self._shutdown

    def handle(self, name: str, payload: Mapping[str, object]) -> dict[str, object]:
        """Execute one already wire-validated command and return its ack payload."""

        handlers: dict[str, Callable[[Mapping[str, object]], dict[str, object]]] = {
            "session.configure": self._configure,
            "codec.inspect": self._inspect,
            "codec.load": self._codec_load,
            "slot.load": self._slot_load,
            "slot.reset": self._slot_reset,
            "slot.decode_cycle": self._decode_cycle,
            "ring.bind": self._ring_bind,
            "worker.status": lambda _: self.status(),
            "metrics.get": lambda _: self.metrics(),
            "worker.shutdown": self._shutdown_worker,
        }
        handler = handlers.get(name)
        if handler is None:
            raise WorkerCommandError(
                "protocol.unknown_command", "unknown worker command", fatal=True
            )
        if name != "session.configure" and not self._configured:
            raise WorkerCommandError("state.invalid_transition", "worker session is not configured")
        return handler(payload)

    def status(self) -> dict[str, object]:
        status: dict[str, object] = {
            "worker_state": self._worker_state,
            "codec_state": self._codec_state,
            "slot_state": self._slot_state,
            "ring_state": self._ring_state,
            "worker_version": WORKER_VERSION,
            "protocol_version": 1,
        }
        if self._stream_generation is not None:
            status["active_generation"] = self._stream_generation
        if self._slot_id is not None:
            status["active_slot_id"] = self._slot_id
        return status

    def heartbeat(self, last_completed_core_sequence: int) -> dict[str, object]:
        return {
            "worker_state": self._worker_state,
            "codec_state": self._codec_state,
            "slot_state": self._slot_state,
            "ring_state": self._ring_state,
            "stream_generation": self._stream_generation or 0,
            "last_completed_core_sequence": last_completed_core_sequence,
            "decode_in_flight": self._worker_state == "busy",
            "worker_uptime_ns": time.monotonic_ns() - self._metrics.started_ns,
        }

    def metrics(self) -> dict[str, object]:
        ring = self._ring
        result: dict[str, object] = {
            "worker_uptime_ns": time.monotonic_ns() - self._metrics.started_ns,
            "decode_batches_total": self._metrics.decode_batches_total,
            "decoded_frames_total": self._metrics.decoded_frames_total,
            "ring_backpressure_total": self._metrics.ring_backpressure_total,
            "presentation_skipped_total": 0 if ring is None else ring.presentation_skipped_total,
            "last_decode_duration_ns": self._metrics.last_decode_duration_ns,
            "ring_write_sequence": 0 if ring is None else ring.write_sequence,
            "ring_read_sequence": 0 if ring is None else ring.read_sequence,
            "ring_occupancy": 0 if ring is None else ring.occupancy,
        }
        try:
            import torch

            configure_torch_cpu_threads(torch)

            if self._codec_state == "ready" and self._device_ordinal is not None:
                result["gpu_allocated_bytes"] = int(
                    torch.cuda.memory_allocated(self._device_ordinal)
                )
                result["gpu_reserved_bytes"] = int(torch.cuda.memory_reserved(self._device_ordinal))
        except ImportError:
            pass
        return result

    def close(self) -> None:
        if self._ring is not None:
            self._ring.close()
            self._ring = None
        if self._decoder is not None:
            self._decoder.close()
            self._decoder = None
        self._ring_state = "unbound"
        self._slot_state = "empty"
        self._codec_state = "unloaded"
        self._worker_state = "stopped"

    def _configure(self, payload: Mapping[str, object]) -> dict[str, object]:
        if self._configured:
            raise WorkerCommandError("state.invalid_transition", "session is already configured")
        if payload["selected_protocol_version"] != 1:
            raise WorkerCommandError(
                "protocol.unsupported_version", "worker protocol 1 was not selected", fatal=True
            )
        if payload["max_frame_bytes"] != 262_144 or payload["max_inflight_decode_batches"] != 1:
            raise WorkerCommandError(
                "protocol.schema_invalid", "unsupported session bounds", fatal=True
            )
        self._configured = True
        self._worker_state = "ready"
        return {
            "selected_protocol_version": 1,
            "heartbeat_interval_ms": payload["heartbeat_interval_ms"],
            "heartbeat_hard_timeout_ms": payload["heartbeat_hard_timeout_ms"],
            "max_frame_bytes": 262_144,
            "max_inflight_decode_batches": 1,
        }

    def _inspect(self, _: Mapping[str, object]) -> dict[str, object]:
        inspection = self._runtime_inspector()
        result: dict[str, object] = {
            "cuda_available": inspection.cuda_available,
            "devices": [
                {
                    "ordinal": device.ordinal,
                    "name": device.name,
                    "total_memory_bytes": device.total_memory_bytes,
                }
                for device in inspection.devices
            ],
            "adapters": [
                {
                    "adapter_id": ADAPTER_ID,
                    "adapter_version": ADAPTER_VERSION,
                    "profiles": [PROFILE],
                }
            ],
        }
        if inspection.torch_version is not None:
            result["torch_version"] = inspection.torch_version
        if inspection.cuda_runtime is not None:
            result["cuda_runtime"] = inspection.cuda_runtime
        return result

    def _codec_load(self, payload: Mapping[str, object]) -> dict[str, object]:
        if self._decoder is not None or self._slot_id is not None or self._ring is not None:
            raise WorkerCommandError("state.invalid_transition", "codec is already active")
        if payload["pack_id"] != PACK_ID or payload["adapter_id"] != ADAPTER_ID:
            raise WorkerCommandError("codec.pack_incompatible", "codec identity is incompatible")
        if payload["profile"] != PROFILE:
            raise WorkerCommandError("codec.pack_incompatible", "codec profile is incompatible")
        assets = payload["assets"]
        if not isinstance(assets, list) or len(assets) != 1:
            raise WorkerCommandError("codec.asset_unbound", "one TAEH3 asset must be bound")
        asset = assets[0]
        if not isinstance(asset, dict) or asset.get("asset_id") != "taeh3":
            raise WorkerCommandError("codec.asset_incompatible", "TAEH3 asset binding is missing")
        device_ordinal = int(payload["device_ordinal"])
        self._codec_state = "loading"
        self._worker_state = "busy"
        try:
            decoder = self._decoder_factory(
                str(asset["path"]),
                str(asset["sha256"]),
                int(asset["byte_length"]),
                device_ordinal,
            )
        except Exception as error:
            self._codec_state = "faulted"
            self._worker_state = "ready"
            raise WorkerCommandError("codec.load_failed", "H3 decoder load failed") from error
        inspection = self._runtime_inspector()
        device = next(
            (candidate for candidate in inspection.devices if candidate.ordinal == device_ordinal),
            None,
        )
        if device is None:
            decoder.close()
            self._codec_state = "faulted"
            self._worker_state = "ready"
            raise WorkerCommandError(
                "codec.cuda_unavailable", "selected CUDA device is unavailable"
            )
        self._decoder = decoder
        self._device_ordinal = device_ordinal
        self._codec_state = "ready"
        self._worker_state = "ready"
        return {
            "pack_id": PACK_ID,
            "pack_version": str(payload["pack_version"]),
            "adapter_id": ADAPTER_ID,
            "adapter_version": ADAPTER_VERSION,
            "profile": PROFILE,
            "device": {
                "ordinal": device.ordinal,
                "name": device.name,
                "total_memory_bytes": device.total_memory_bytes,
            },
        }

    def _slot_load(self, payload: Mapping[str, object]) -> dict[str, object]:
        decoder = self._require_decoder()
        if self._slot_id is not None or self._ring is not None:
            raise WorkerCommandError(
                "state.invalid_transition", "a cartridge slot is already active"
            )
        self._slot_state = "loading"
        try:
            source = self._source_loader(
                str(payload["cartridge_path"]),
                str(payload["expected_archive_sha256"]),
            )
        except Exception as error:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "slot.cartridge_invalid", "cartridge validation changed"
            ) from error
        if source.cartridge_id != payload["cartridge_id"]:
            self._slot_state = "faulted"
            raise WorkerCommandError("slot.cartridge_hash_mismatch", "cartridge identity changed")
        decoder.bind_source(source)
        self._source = source
        self._slot_id = str(payload["slot_id"])
        self._slot_revision += 1
        self._stream_generation = int(payload["stream_generation"])
        self._next_cycle_index = 0
        self._slot_state = "ready"
        return {
            "slot_id": self._slot_id,
            "slot_revision": self._slot_revision,
            "width": source.width,
            "height": source.height,
            "profile": PROFILE,
            "timing": self._timing(source),
        }

    def _ring_bind(self, payload: Mapping[str, object]) -> dict[str, object]:
        source = self._require_source()
        if self._ring is not None:
            raise WorkerCommandError("state.invalid_transition", "RGB ring is already bound")
        self._ring_state = "binding"
        try:
            ring = self._ring_factory(payload, source, self._require_generation())
        except Exception as error:
            self._ring_state = "faulted"
            diagnostic_code = getattr(error, "code", None)
            diagnostic_detail = getattr(error, "detail", None)
            if not isinstance(diagnostic_code, str):
                diagnostic_code = type(error).__name__
            if not isinstance(diagnostic_detail, str):
                # RingBind contains only numeric handles, geometry and ABI
                # metadata, so its bounded local diagnostic cannot disclose a
                # cartridge or decoder path.
                diagnostic_detail = str(error)
            raise WorkerCommandError(
                "ring.layout_incompatible",
                "RGB ring binding failed",
                diagnostic_code=diagnostic_code,
                diagnostic_detail=diagnostic_detail,
            ) from error
        self._ring = ring
        self._ring_state = "ready"
        return {
            "layout_version": int(payload["layout_version"]),
            "ring_id": str(payload["ring_id"]),
            "mapping_bytes": int(payload["mapping_bytes"]),
        }

    def _slot_reset(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_slot(payload)
        generation = self._require_generation()
        new_generation = int(payload["new_stream_generation"])
        if new_generation <= generation:
            raise WorkerCommandError("state.stale_generation", "reset generation is not newer")
        decoder = self._require_decoder()
        ring = self._require_ring()
        ring.set_generation(new_generation)
        decoder.reset()
        self._stream_generation = new_generation
        self._next_cycle_index = 0
        self._slot_state = "ready"
        return {
            "slot_id": self._slot_id,
            "slot_revision": self._slot_revision,
            "stream_generation": new_generation,
            "next_cycle_index": 0,
            "ring_write_sequence": ring.write_sequence,
        }

    def _decode_cycle(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_slot(payload)
        if int(payload["stream_generation"]) != self._require_generation():
            raise WorkerCommandError("state.stale_generation", "decode generation is stale")
        cycle_index = int(payload["cycle_index"])
        if cycle_index != self._next_cycle_index:
            raise WorkerCommandError("decode.cycle_out_of_order", "decode cycle is out of order")
        source = self._require_source()
        timing = source.cycle(cycle_index)
        ring = self._require_ring()
        if not ring.can_publish(timing.decoded_frame_count):
            self._metrics.ring_backpressure_total += 1
            raise WorkerCommandError(
                "ring.backpressure",
                "RGB ring has insufficient capacity for the complete H3 cycle",
                retryable=True,
            )
        self._worker_state = "busy"
        self._slot_state = "decoding"
        started = time.monotonic_ns()
        try:
            decoded = self._require_decoder().decode_cycle(cycle_index)
            first, last = ring.publish_cycle(
                decoded.rgba_frames,
                stream_generation=self._require_generation(),
                cycle_index=cycle_index,
                decoded_start_frame=timing.decoded_start_frame,
            )
        except WorkerCommandError:
            raise
        except Exception as error:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "decode.failed",
                "TAEH3 decode or RGB publish failed; restart the worker session",
                fatal=True,
            ) from error
        finally:
            self._worker_state = "ready"
        self._metrics.last_decode_duration_ns = time.monotonic_ns() - started
        self._metrics.decode_batches_total += 1
        self._metrics.decoded_frames_total += timing.decoded_frame_count
        self._next_cycle_index += 1
        self._slot_state = "end_of_stream" if timing.end_of_stream else "ready"
        return {
            "slot_id": self._slot_id,
            "slot_revision": self._slot_revision,
            "stream_generation": self._require_generation(),
            "cycle_index": cycle_index,
            "latent_start": timing.latent_start,
            "latent_count": timing.latent_count,
            "decoded_start_frame": timing.decoded_start_frame,
            "decoded_frame_count": timing.decoded_frame_count,
            "ring_first_sequence": first,
            "ring_last_sequence_exclusive": last,
            "end_of_stream": timing.end_of_stream,
        }

    def _shutdown_worker(self, _: Mapping[str, object]) -> dict[str, object]:
        self._worker_state = "stopping"
        self._shutdown = True
        return {"accepted": True}

    def _check_slot(self, payload: Mapping[str, object]) -> None:
        if (
            payload["slot_id"] != self._slot_id
            or int(payload["slot_revision"]) != self._slot_revision
        ):
            raise WorkerCommandError("state.stale_slot_revision", "slot revision is stale")

    def _require_decoder(self) -> Decoder:
        if self._decoder is None or self._codec_state != "ready":
            raise WorkerCommandError("state.invalid_transition", "H3 codec is not ready")
        return self._decoder

    def _require_source(self) -> H3VideoSource:
        if self._source is None or self._slot_id is None:
            raise WorkerCommandError("state.invalid_transition", "cartridge slot is empty")
        return self._source

    def _require_ring(self) -> RingProducer:
        if self._ring is None or self._ring_state != "ready":
            raise WorkerCommandError("ring.unbound", "RGB ring is not ready")
        return self._ring

    def _require_generation(self) -> int:
        if self._stream_generation is None:
            raise WorkerCommandError("state.invalid_transition", "stream generation is absent")
        return self._stream_generation

    @staticmethod
    def _load_decoder(path: str, sha256: str, byte_length: int, ordinal: int) -> Decoder:
        return H3Decoder.load(path, sha256, byte_length, ordinal)

    @staticmethod
    def _bind_ring(
        payload: Mapping[str, object], source: H3VideoSource, stream_generation: int
    ) -> RingProducer:
        from .ring import WindowsRingProducer

        return WindowsRingProducer.bind(
            payload,
            source.width,
            source.height,
            stream_generation,
        )

    @staticmethod
    def _timing(source: H3VideoSource) -> dict[str, object]:
        steady_cycles = source.cycle_count - 1
        return {
            "frame_rate_numerator": source.frame_rate_numerator,
            "frame_rate_denominator": source.frame_rate_denominator,
            "latent_slot_count": source.latent_slot_count,
            "decoded_frame_count": source.frame_count,
            "cycle_count": source.cycle_count,
            "initial": {
                "first_cycle_index": 0,
                "cycle_count": 1,
                "latent_base": 0,
                "latent_stride": 0,
                "latent_count": 2,
                "decoded_base": 0,
                "decoded_stride": 0,
                "decoded_count": 5,
            },
            "steady": {
                "first_cycle_index": 1,
                "cycle_count": steady_cycles,
                "latent_base": 2,
                "latent_stride": 5,
                "latent_count": 5,
                "decoded_base": 5,
                "decoded_stride": 17,
                "decoded_count": 17,
            },
            "reset_required_on_wrap": True,
            "arbitrary_seek": False,
            "max_frames_per_cycle": 17,
        }


__all__ = ["H3WorkerState", "RingProducer", "WorkerCommandError"]
