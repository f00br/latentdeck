"""Worker Protocol state for the isolated H3 LD-D2 pre-decode engine."""

from __future__ import annotations

import json
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from latentdeck_operator_d2 import (
    D2DecodedSlot,
    D2Paused,
    D2ProcessedSlot,
    D2ResetBarrier,
    D2StreamError,
    D2Transport,
    OperatorLoadError,
)

from .cartridge import H3VideoSource, load_video_source
from .d2_capture import D2CaptureError, D2CaptureSession
from .d2_engine import H3D2SourceError, H3D2StreamEngine
from .decoder import H3Decoder, RuntimeInspection, inspect_runtime
from .worker_state import (
    ADAPTER_ID,
    ADAPTER_VERSION,
    PACK_ID,
    PROFILE,
    WORKER_VERSION,
    WorkerCommandError,
)

D2_OPERATOR_ID = "org.latentdeck.builtin.ld_d2"
D2_OPERATOR_VERSION = "0.1.0"
MAX_SAFE_SEED = 9_007_199_254_740_991


class D2Decoder(Protocol):
    def decode_slot(self, slot: Any) -> Sequence[bytes]: ...

    def reset(self) -> None: ...

    def close(self) -> None: ...


class D2RingProducer(Protocol):
    @property
    def write_sequence(self) -> int: ...

    @property
    def read_sequence(self) -> int: ...

    @property
    def occupancy(self) -> int: ...

    @property
    def presentation_skipped_total(self) -> int: ...

    def can_publish(self, frame_count: int) -> bool: ...

    def publish_frames(
        self,
        frames: Sequence[bytes],
        *,
        stream_generation: int,
    ) -> tuple[int, int]: ...

    def set_generation(self, stream_generation: int) -> None: ...

    def close(self) -> None: ...


@dataclass(slots=True)
class D2WorkerMetrics:
    started_ns: int
    decode_batches_total: int = 0
    decoded_frames_total: int = 0
    ring_backpressure_total: int = 0
    last_decode_duration_ns: int = 0


class H3D2WorkerState:
    """One H3 decoder, two trusted cartridge sources, and one D2 deck."""

    def __init__(
        self,
        *,
        decoder_factory: Callable[[str, str, int, int], D2Decoder] | None = None,
        source_loader: Callable[[str | Path, str], H3VideoSource] = load_video_source,
        ring_factory: Callable[[Mapping[str, object], H3VideoSource, int], D2RingProducer]
        | None = None,
        runtime_inspector: Callable[[], RuntimeInspection] = inspect_runtime,
        torch_loader: Callable[[], Any] | None = None,
        device_factory: Callable[[Any, int], Any] | None = None,
        engine_factory: Callable[..., H3D2StreamEngine] = H3D2StreamEngine,
    ) -> None:
        self._decoder_factory = decoder_factory or self._load_decoder
        self._source_loader = source_loader
        self._ring_factory = ring_factory or self._bind_ring
        self._runtime_inspector = runtime_inspector
        self._torch_loader = torch_loader or self._load_torch
        self._device_factory = device_factory or self._cuda_device
        self._engine_factory = engine_factory
        self._configured = False
        self._decoder: D2Decoder | None = None
        self._engine: H3D2StreamEngine | None = None
        self._source_a: H3VideoSource | None = None
        self._source_b: H3VideoSource | None = None
        self._ring: D2RingProducer | None = None
        self._capture: D2CaptureSession | None = None
        self._torch: Any = None
        self._device: Any = None
        self._device_ordinal: int | None = None
        self._deck_id: str | None = None
        self._deck_revision = 0
        self._decoded_start_frame = 0
        self._shutdown = False
        self._worker_state = "handshaking"
        self._codec_state = "unloaded"
        self._slot_state = "empty"
        self._ring_state = "unbound"
        self._metrics = D2WorkerMetrics(time.monotonic_ns())

    @property
    def shutdown_requested(self) -> bool:
        return self._shutdown

    def handle(self, name: str, payload: Mapping[str, object]) -> dict[str, object]:
        handlers: dict[str, Callable[[Mapping[str, object]], dict[str, object]]] = {
            "session.configure": self._configure,
            "codec.inspect": self._inspect,
            "codec.load": self._codec_load,
            "ring.bind": self._ring_bind,
            "deck.d2.load": self._deck_load,
            "deck.d2.process_slot": self._process_slot,
            "deck.d2.reset": self._reset,
            "deck.d2.restart": self._restart,
            "deck.d2.controls.set": self._controls_set,
            "deck.d2.transport.set": self._transport_set,
            "deck.d2.seed.set": self._seed_set,
            "deck.d2.status": lambda _: self.deck_status(),
            "deck.d2.capture.start": self._capture_start,
            "deck.d2.capture.stop": self._capture_stop,
            "deck.d2.capture.status": self._capture_status,
            "worker.status": lambda _: self.status(),
            "metrics.get": lambda _: self.metrics(),
            "worker.shutdown": self._shutdown_worker,
        }
        handler = handlers.get(name)
        if handler is None:
            raise WorkerCommandError(
                "protocol.unknown_command", "unknown D2 worker command", fatal=True
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
        if self._engine is not None:
            status["active_generation"] = self._engine.status()["stream_generation"]
        if self._deck_id is not None:
            status["active_slot_id"] = self._deck_id
        return status

    def deck_status(self) -> dict[str, object]:
        engine = self._require_engine()
        source_a = self._require_source(self._source_a)
        source_b = self._require_source(self._source_b)
        status = engine.status()
        return {
            "deck_id": self._deck_id,
            "deck_revision": self._deck_revision,
            "operator_id": D2_OPERATOR_ID,
            "operator_version": D2_OPERATOR_VERSION,
            "stream_generation": status["stream_generation"],
            "stream_sequence": status["stream_sequence"],
            "playhead_a": status["playhead_a"],
            "playhead_b": status["playhead_b"],
            "transport": status["transport"],
            "controls": status["controls"],
            "seed": status["seed"],
            "pending_reset": status["pending_reset"],
            "pending_reset_reasons": status["pending_reset_reasons"],
            "decoded_start_frame": self._decoded_start_frame,
            "source_a": self._source_status(source_a),
            "source_b": self._source_status(source_b),
        }

    def heartbeat(self, last_completed_core_sequence: int) -> dict[str, object]:
        generation = 0
        if self._engine is not None:
            generation = int(self._engine.status()["stream_generation"])
        return {
            "worker_state": self._worker_state,
            "codec_state": self._codec_state,
            "slot_state": self._slot_state,
            "ring_state": self._ring_state,
            "stream_generation": generation,
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
            if self._codec_state == "ready" and self._device_ordinal is not None:
                result["gpu_allocated_bytes"] = int(
                    self._torch.cuda.memory_allocated(self._device_ordinal)
                )
                result["gpu_reserved_bytes"] = int(
                    self._torch.cuda.memory_reserved(self._device_ordinal)
                )
        except (AttributeError, RuntimeError):
            pass
        return result

    def close(self) -> None:
        first_error: Exception | None = None
        if self._capture is not None:
            self._capture.abort("worker_closed")
            self._capture = None
        for resource in (self._ring, self._engine, self._decoder):
            if resource is None:
                continue
            try:
                resource.close()
            except Exception as error:
                if first_error is None:
                    first_error = error
        self._ring = None
        self._engine = None
        self._decoder = None
        self._source_a = None
        self._source_b = None
        self._ring_state = "unbound"
        self._slot_state = "empty"
        self._codec_state = "unloaded"
        self._worker_state = "stopped"
        if first_error is not None:
            raise first_error

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
        if self._decoder is not None or self._engine is not None or self._ring is not None:
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
        decoder: D2Decoder | None = None
        try:
            decoder = self._decoder_factory(
                str(asset["path"]),
                str(asset["sha256"]),
                int(asset["byte_length"]),
                device_ordinal,
            )
            inspection = self._runtime_inspector()
            device = next(
                (
                    candidate
                    for candidate in inspection.devices
                    if candidate.ordinal == device_ordinal
                ),
                None,
            )
            if device is None:
                raise WorkerCommandError(
                    "codec.cuda_unavailable", "selected CUDA device is unavailable"
                )
            torch = self._torch_loader()
            runtime_device = self._device_factory(torch, device_ordinal)
        except WorkerCommandError:
            if decoder is not None:
                decoder.close()
            self._codec_state = "faulted"
            self._worker_state = "ready"
            raise
        except Exception as error:
            if decoder is not None:
                decoder.close()
            self._codec_state = "faulted"
            self._worker_state = "ready"
            raise WorkerCommandError("codec.load_failed", "H3 decoder load failed") from error
        self._decoder = decoder
        self._torch = torch
        self._device = runtime_device
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

    def _deck_load(self, payload: Mapping[str, object]) -> dict[str, object]:
        decoder = self._require_decoder()
        if self._engine is not None or self._ring is not None:
            raise WorkerCommandError("state.invalid_transition", "a D2 deck is already active")
        self._slot_state = "loading"
        source_a_binding = self._source_binding(payload["source_a"], "source_a")
        source_b_binding = self._source_binding(payload["source_b"], "source_b")
        try:
            source_a = self._source_loader(
                source_a_binding["cartridge_path"],
                source_a_binding["expected_archive_sha256"],
            )
            source_b = self._source_loader(
                source_b_binding["cartridge_path"],
                source_b_binding["expected_archive_sha256"],
            )
        except Exception as error:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "slot.cartridge_invalid", "D2 cartridge validation changed"
            ) from error
        if (
            source_a.cartridge_id != source_a_binding["cartridge_id"]
            or source_b.cartridge_id != source_b_binding["cartridge_id"]
        ):
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "slot.cartridge_hash_mismatch", "D2 cartridge identity changed"
            )
        if source_a.width != source_b.width or source_a.height != source_b.height:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "deck.source_incompatible", "D2 decoded presentation geometry differs"
            )
        try:
            engine = self._engine_factory(
                source_a,
                source_b,
                decoder,
                torch=self._torch,
                device=self._device,
                controls=payload["controls"],
                transport=self._transport(payload["transport"]),
                seed=int(payload["seed"]),
                stream_generation=int(payload["stream_generation"]),
                operator_id=str(payload["operator_id"]),
                operator_version=str(payload["operator_version"]),
            )
        except OperatorLoadError as error:
            self._slot_state = "faulted"
            code = (
                error.code
                if error.code
                in {
                    "operator.not_installed",
                    "operator.version_mismatch",
                    "operator.not_trusted",
                }
                else "operator.profile_incompatible"
            )
            raise WorkerCommandError(code, "trusted D2 operator could not be loaded") from error
        except (D2StreamError, H3D2SourceError) as error:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "deck.source_incompatible", "D2 sources are incompatible"
            ) from error
        self._engine = engine
        self._source_a = source_a
        self._source_b = source_b
        self._deck_id = str(payload["deck_id"])
        self._deck_revision += 1
        self._decoded_start_frame = 0
        self._slot_state = "ready"
        return self.deck_status()

    def _ring_bind(self, payload: Mapping[str, object]) -> dict[str, object]:
        source = self._require_source(self._source_a)
        if self._ring is not None:
            raise WorkerCommandError("state.invalid_transition", "RGB ring is already bound")
        self._ring_state = "binding"
        try:
            ring = self._ring_factory(
                payload,
                source,
                int(self._require_engine().status()["stream_generation"]),
            )
        except Exception as error:
            self._ring_state = "faulted"
            raise WorkerCommandError(
                "ring.layout_incompatible", "D2 RGB ring binding failed"
            ) from error
        self._ring = ring
        self._ring_state = "ready"
        return {
            "layout_version": int(payload["layout_version"]),
            "ring_id": str(payload["ring_id"]),
            "mapping_bytes": int(payload["mapping_bytes"]),
        }

    def _process_slot(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        engine = self._require_engine()
        status = engine.status()
        if int(payload["stream_generation"]) != int(status["stream_generation"]):
            raise WorkerCommandError("state.stale_generation", "D2 generation is stale")
        ring = self._require_ring()
        transport = status["transport"]
        is_paused = isinstance(transport, Mapping) and not (
            transport.get("playing_a") or transport.get("playing_b")
        )
        if not status["pending_reset"] and not is_paused and not ring.can_publish(4):
            self._metrics.ring_backpressure_total += 1
            raise WorkerCommandError(
                "ring.backpressure",
                "RGB ring has insufficient capacity for one D2 decoder slot",
                retryable=True,
            )
        self._worker_state = "busy"
        self._slot_state = "decoding"
        started = time.monotonic_ns()
        try:
            step = engine.step(self._capture_before_decode)
            if isinstance(step, D2ResetBarrier):
                if (
                    self._capture is not None
                    and self._capture.is_active
                    and not self._capture.finish_at_reset_boundary()
                ):
                    self._capture.abort("causal_boundary_changed")
                self._slot_state = "ready"
                return self._barrier_payload(step)
            if isinstance(step, D2Paused):
                if self._capture is not None and self._capture.is_active:
                    self._capture.abort("transport_paused")
                self._slot_state = "ready"
                return {
                    "kind": "paused",
                    "deck_id": self._deck_id,
                    "deck_revision": self._deck_revision,
                    "stream_generation": step.stream_generation,
                    "playhead_a": step.playhead_a,
                    "playhead_b": step.playhead_b,
                    "transport": engine.status()["transport"],
                }
            if not isinstance(step, D2DecodedSlot):
                raise D2StreamError("deck.process_failed", "D2 step type is invalid")
            if self._capture is not None:
                self._capture.after_decode(step.latent)
            frames = tuple(step.decoded)
            source_a = self._require_source(self._source_a)
            expected_bytes = source_a.width * source_a.height * 4
            if not 1 <= len(frames) <= 4 or any(
                not isinstance(frame, bytes) or len(frame) != expected_bytes for frame in frames
            ):
                raise D2StreamError("deck.decode_failed", "decoder frame batch is invalid")
            provenance_json = json.dumps(
                step.latent.provenance,
                allow_nan=False,
                sort_keys=True,
                separators=(",", ":"),
            )
            first, last = ring.publish_frames(
                frames,
                stream_generation=step.latent.stream_generation,
            )
        except D2CaptureError as error:
            if self._capture is not None and self._capture.should_cleanup_on_error:
                self._capture.abort("capture_error")
            self._slot_state = "faulted"
            raise WorkerCommandError(
                error.code,
                "D2 capture failed after stream advance; restart the worker session",
                fatal=True,
            ) from error
        except D2StreamError as error:
            if self._capture is not None and self._capture.should_cleanup_on_error:
                self._capture.abort("process_or_decode_error")
            fatal = error.code == "deck.decode_failed"
            code = "decode.failed" if fatal else "deck.process_failed"
            if fatal:
                self._slot_state = "faulted"
            else:
                self._slot_state = "ready"
            raise WorkerCommandError(code, "D2 process/decode failed", fatal=fatal) from error
        except Exception as error:
            if self._capture is not None and self._capture.should_cleanup_on_error:
                self._capture.abort("process_or_publish_error")
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "decode.failed",
                "D2 decode or RGB publish failed; restart the worker session",
                fatal=True,
            ) from error
        finally:
            self._worker_state = "ready"
        decoded_start = self._decoded_start_frame
        if decoded_start > 0xFFFF_FFFF_FFFF_FFFF - len(frames):
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "decode.failed", "D2 decoded frame counter is exhausted", fatal=True
            )
        self._decoded_start_frame += len(frames)
        self._metrics.last_decode_duration_ns = time.monotonic_ns() - started
        self._metrics.decode_batches_total += 1
        self._metrics.decoded_frames_total += len(frames)
        self._slot_state = "ready"
        return {
            "kind": "decoded_slot",
            "deck_id": self._deck_id,
            "deck_revision": self._deck_revision,
            "stream_generation": step.latent.stream_generation,
            "stream_sequence": step.latent.stream_sequence,
            "playhead_a": step.latent.playhead_a,
            "playhead_b": step.latent.playhead_b,
            "transport": engine.status()["transport"],
            "decoded_start_frame": decoded_start,
            "decoded_frame_count": len(frames),
            "ring_first_sequence": first,
            "ring_last_sequence_exclusive": last,
            "provenance_json": provenance_json,
        }

    def _reset(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        engine = self._require_engine()
        ring = self._require_ring()
        new_generation = int(payload["new_stream_generation"])
        capture = self._capture
        if capture is not None and capture.is_active:
            capture.abort("causal_reset")
        try:
            result = engine.apply_reset_barrier(
                new_generation,
                lambda: ring.set_generation(new_generation),
            )
        except D2StreamError as error:
            if capture is not None and capture.is_awaiting_reset:
                capture.abort("start_reset_failed")
            if error.code == "deck.generation_stale":
                code = "state.stale_generation"
            elif error.code == "deck.generation_exhausted":
                code = "deck.generation_exhausted"
            elif error.code == "deck.reset_not_required":
                code = "deck.reset_not_required"
            else:
                code = "deck.reset_failed"
            raise WorkerCommandError(
                code,
                "D2 causal reset was rejected",
                fatal=code == "deck.reset_failed",
            ) from error
        if capture is not None and capture.is_awaiting_reset:
            try:
                capture.activate(result)
            except D2CaptureError as error:
                raise WorkerCommandError(
                    error.code,
                    "D2 capture could not enter its codec boundary",
                    fatal=True,
                ) from error
        self._decoded_start_frame = 0
        self._slot_state = "ready"
        return {
            "kind": "reset_applied",
            "deck_id": self._deck_id,
            "deck_revision": self._deck_revision,
            **result,
        }

    def _restart(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        self._require_ring()
        try:
            barrier = self._require_engine().request_restart()
        except D2StreamError as error:
            raise WorkerCommandError(
                "deck.generation_exhausted", "D2 stream generation is exhausted"
            ) from error
        return self._barrier_payload(barrier)

    def _controls_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        capture = self._capture
        if capture is not None and capture.is_snapshot_locked:
            raise WorkerCommandError(
                "capture.snapshot_frozen", "Snapshot controls are frozen until capture completes"
            )
        if capture is not None:
            try:
                capture.ensure_event_capacity()
            except D2CaptureError as error:
                raise WorkerCommandError(
                    error.code, "Live Capture event history is full"
                ) from error
        try:
            result = self._require_engine().update_controls(payload["controls"])
        except (D2StreamError, OperatorLoadError, ValueError) as error:
            raise WorkerCommandError("deck.process_failed", "D2 controls are invalid") from error
        if capture is not None and capture.accepts_live_events:
            capture.record_control_state(
                result["controls"],  # type: ignore[arg-type]
                int(self._require_engine().status()["seed"]),
            )
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _transport_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        if self._capture is not None and self._capture.locks_transport:
            raise WorkerCommandError(
                "capture.transport_locked", "capture transport is frozen until completion"
            )
        result = self._require_engine().update_transport(self._transport(payload["transport"]))
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _seed_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        capture = self._capture
        if capture is not None and capture.is_snapshot_locked:
            raise WorkerCommandError(
                "capture.snapshot_frozen", "Snapshot seed is frozen until capture completes"
            )
        if capture is not None:
            try:
                capture.ensure_event_capacity()
            except D2CaptureError as error:
                raise WorkerCommandError(
                    error.code, "Live Capture event history is full"
                ) from error
        seed = int(payload["seed"])
        if not 0 <= seed <= MAX_SAFE_SEED:
            raise WorkerCommandError("protocol.schema_invalid", "D2 seed is outside u53")
        result = self._require_engine().update_seed(seed)
        if capture is not None and capture.accepts_live_events:
            capture.record_control_state(
                self._require_engine().status()["controls"],  # type: ignore[arg-type]
                seed,
            )
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _capture_start(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        self._require_ring()
        engine = self._require_engine()
        status = engine.status()
        if status["pending_reset"]:
            raise WorkerCommandError(
                "capture.boundary_unavailable", "D2 already has a causal reset pending"
            )
        previous_capture = self._capture
        if previous_capture is not None and (
            previous_capture.is_active or previous_capture.is_awaiting_reset
        ):
            raise WorkerCommandError("capture.already_active", "a D2 capture is already active")
        source_a = self._require_source(self._source_a)
        source_b = self._require_source(self._source_b)
        controls = status["controls"]
        transport = status["transport"]
        if not isinstance(controls, Mapping) or not isinstance(transport, Mapping):
            raise WorkerCommandError("capture.start_failed", "D2 capture state is invalid")
        carrier = controls.get("routing")
        if carrier not in {"A", "B"} or not transport.get(f"playing_{str(carrier).lower()}"):
            raise WorkerCommandError(
                "capture.carrier_paused", "structural carrier must be playing at capture start"
            )
        if payload["mode"] == "snapshot":
            carrier_source = source_a if carrier == "A" else source_b
            donor_slot = "B" if carrier == "A" else "A"
            donor_source = source_b if donor_slot == "B" else source_a
            if (
                donor_source.latent_slot_count < carrier_source.latent_slot_count
                and transport.get(f"playing_{donor_slot.lower()}") is True
                and transport.get(f"loop_{donor_slot.lower()}") is True
            ):
                raise WorkerCommandError(
                    "capture.source_cycle_incompatible",
                    "looping non-carrier ends before the Snapshot carrier cycle",
                )
        capture: D2CaptureSession | None = None
        try:
            current_generation = int(status["stream_generation"])
            capture = D2CaptureSession(
                capture_id=str(payload["capture_id"]),
                mode=str(payload["mode"]),
                temporary_root=str(payload["temporary_root"]),
                max_latent_slots=int(payload["max_latent_slots"]),
                max_visual_bytes=int(payload["max_visual_bytes"]),
                source_a=source_a,
                source_b=source_b,
                controls=controls,
                seed=int(status["seed"]),
                current_generation=current_generation,
                minimum_new_generation=current_generation + 1,
            )
            barrier = engine.request_restart()
            if (
                barrier.current_generation != current_generation
                or barrier.minimum_new_generation != current_generation + 1
            ):
                raise D2CaptureError(
                    "capture.boundary_invalid", "restart barrier generation changed unexpectedly"
                )
        except (D2CaptureError, D2StreamError, OSError, ValueError) as error:
            if capture is not None:
                capture.abort("start_failed")
            code = error.code if isinstance(error, D2CaptureError) else "capture.start_failed"
            raise WorkerCommandError(code, "D2 capture could not start") from error
        if previous_capture is not None:
            previous_capture.abort("replaced")
        self._capture = capture
        return capture.status()

    def _capture_stop(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        capture = self._checked_capture(payload)
        try:
            capture.request_stop()
        except D2CaptureError as error:
            raise WorkerCommandError(error.code, "D2 capture could not stop") from error
        return capture.status()

    def _capture_status(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        return self._checked_capture(payload).status()

    def _checked_capture(self, payload: Mapping[str, object]) -> D2CaptureSession:
        capture = self._capture
        if capture is None:
            raise WorkerCommandError("capture.not_found", "D2 capture is absent")
        if payload["capture_id"] != capture.capture_id:
            raise WorkerCommandError("capture.id_mismatch", "D2 capture identity is stale")
        return capture

    def _capture_before_decode(self, step: D2ProcessedSlot) -> None:
        capture = self._capture
        if capture is not None:
            capture.before_decode(step)

    def _shutdown_worker(self, _: Mapping[str, object]) -> dict[str, object]:
        self._worker_state = "stopping"
        self._shutdown = True
        return {"accepted": True}

    def _check_deck(self, payload: Mapping[str, object]) -> None:
        if (
            payload["deck_id"] != self._deck_id
            or int(payload["deck_revision"]) != self._deck_revision
        ):
            raise WorkerCommandError("state.stale_slot_revision", "D2 deck revision is stale")

    def _barrier_payload(self, barrier: D2ResetBarrier) -> dict[str, object]:
        return {
            "kind": "reset_barrier",
            "deck_id": self._deck_id,
            "deck_revision": self._deck_revision,
            **barrier.as_dict(),
        }

    def _require_decoder(self) -> D2Decoder:
        if self._decoder is None or self._codec_state != "ready":
            raise WorkerCommandError("state.invalid_transition", "H3 codec is not ready")
        return self._decoder

    def _require_engine(self) -> H3D2StreamEngine:
        if self._engine is None or self._deck_id is None:
            raise WorkerCommandError("state.invalid_transition", "D2 deck is not loaded")
        return self._engine

    def _require_ring(self) -> D2RingProducer:
        if self._ring is None or self._ring_state != "ready":
            raise WorkerCommandError("ring.unbound", "D2 RGB ring is not ready")
        return self._ring

    @staticmethod
    def _require_source(source: H3VideoSource | None) -> H3VideoSource:
        if source is None:
            raise WorkerCommandError("state.invalid_transition", "D2 source is absent")
        return source

    @staticmethod
    def _source_binding(raw: object, label: str) -> dict[str, str]:
        if not isinstance(raw, dict):
            raise WorkerCommandError("protocol.schema_invalid", f"{label} is invalid")
        return {
            "cartridge_path": str(raw["cartridge_path"]),
            "cartridge_id": str(raw["cartridge_id"]),
            "expected_archive_sha256": str(raw["expected_archive_sha256"]),
        }

    @staticmethod
    def _source_status(source: H3VideoSource) -> dict[str, object]:
        return {
            "cartridge_id": source.cartridge_id,
            "archive_sha256": source.archive_sha256,
            "latent_slot_count": source.latent_slot_count,
        }

    @staticmethod
    def _transport(raw: object) -> D2Transport:
        if not isinstance(raw, Mapping):
            raise WorkerCommandError("protocol.schema_invalid", "D2 transport is invalid")
        return D2Transport(
            playing_a=raw["playing_a"],  # type: ignore[arg-type]
            playing_b=raw["playing_b"],  # type: ignore[arg-type]
            loop_a=raw["loop_a"],  # type: ignore[arg-type]
            loop_b=raw["loop_b"],  # type: ignore[arg-type]
        )

    @staticmethod
    def _load_decoder(path: str, sha256: str, byte_length: int, ordinal: int) -> D2Decoder:
        return H3Decoder.load(path, sha256, byte_length, ordinal)

    @staticmethod
    def _load_torch() -> Any:
        import torch

        return torch

    @staticmethod
    def _cuda_device(torch: Any, ordinal: int) -> Any:
        return torch.device(f"cuda:{ordinal}")

    @staticmethod
    def _bind_ring(
        payload: Mapping[str, object], source: H3VideoSource, stream_generation: int
    ) -> D2RingProducer:
        from .ring import WindowsRingProducer

        return WindowsRingProducer.bind(
            payload,
            source.width,
            source.height,
            stream_generation,
        )


__all__ = ["H3D2WorkerState"]
