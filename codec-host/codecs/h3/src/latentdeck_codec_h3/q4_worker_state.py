"""Worker Protocol state for the isolated H3 LD-Q4 pre-decode engine."""

from __future__ import annotations

import json
import time
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Protocol

from latentdeck_codec_host.operator_api import OperatorLoadError
from latentdeck_operator_q4.stream import (
    Q4DecodedSlot,
    Q4Paused,
    Q4ProcessedSlot,
    Q4ResetBarrier,
    Q4RoleAssignment,
    Q4StreamError,
    Q4Transport,
)

from .cartridge import H3VideoSource, load_video_source
from .decoder import H3Decoder, RuntimeInspection, inspect_runtime
from .q4_capture import Q4CaptureError, Q4CaptureSession
from .q4_engine import H3Q4SourceError, H3Q4StreamEngine
from .worker_state import (
    ADAPTER_ID,
    ADAPTER_VERSION,
    PACK_ID,
    PROFILE,
    WORKER_VERSION,
    WorkerCommandError,
)

Q4_OPERATOR_ID = "org.latentdeck.builtin.ld_q4"
Q4_OPERATOR_VERSION = "0.1.0"
MAX_SAFE_SEED = 9_007_199_254_740_991
MAX_PROVENANCE_JSON_BYTES = 32_768
SLOTS = ("A", "B", "C", "D")


class Q4Decoder(Protocol):
    def decode_slot(self, slot: Any) -> Sequence[bytes]: ...

    def reset(self) -> None: ...

    def close(self) -> None: ...


class Q4RingProducer(Protocol):
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
class Q4WorkerMetrics:
    started_ns: int
    decode_batches_total: int = 0
    decoded_frames_total: int = 0
    ring_backpressure_total: int = 0
    last_decode_duration_ns: int = 0


class H3Q4WorkerState:
    """One H3 decoder, four trusted sources, and one explicit-role Q4 deck."""

    def __init__(
        self,
        *,
        decoder_factory: Callable[[str, str, int, int], Q4Decoder] | None = None,
        source_loader: Callable[[str | Path, str], H3VideoSource] = load_video_source,
        ring_factory: Callable[[Mapping[str, object], H3VideoSource, int], Q4RingProducer]
        | None = None,
        runtime_inspector: Callable[[], RuntimeInspection] = inspect_runtime,
        torch_loader: Callable[[], Any] | None = None,
        device_factory: Callable[[Any, int], Any] | None = None,
        engine_factory: Callable[..., H3Q4StreamEngine] = H3Q4StreamEngine,
    ) -> None:
        self._decoder_factory = decoder_factory or self._load_decoder
        self._source_loader = source_loader
        self._ring_factory = ring_factory or self._bind_ring
        self._runtime_inspector = runtime_inspector
        self._torch_loader = torch_loader or self._load_torch
        self._device_factory = device_factory or self._cuda_device
        self._engine_factory = engine_factory
        self._configured = False
        self._decoder: Q4Decoder | None = None
        self._engine: H3Q4StreamEngine | None = None
        self._sources: dict[str, H3VideoSource | None] = {slot: None for slot in SLOTS}
        self._ring: Q4RingProducer | None = None
        self._capture: Q4CaptureSession | None = None
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
        self._metrics = Q4WorkerMetrics(time.monotonic_ns())

    @property
    def shutdown_requested(self) -> bool:
        return self._shutdown

    def handle(self, name: str, payload: Mapping[str, object]) -> dict[str, object]:
        handlers: dict[str, Callable[[Mapping[str, object]], dict[str, object]]] = {
            "session.configure": self._configure,
            "codec.inspect": self._inspect,
            "codec.load": self._codec_load,
            "ring.bind": self._ring_bind,
            "deck.q4.load": self._deck_load,
            "deck.q4.process_slot": self._process_slot,
            "deck.q4.reset": self._reset,
            "deck.q4.restart": self._restart,
            "deck.q4.controls.set": self._controls_set,
            "deck.q4.roles.set": self._roles_set,
            "deck.q4.transport.set": self._transport_set,
            "deck.q4.seed.set": self._seed_set,
            "deck.q4.status": lambda _: self.deck_status(),
            "deck.q4.capture.start": self._capture_start,
            "deck.q4.capture.stop": self._capture_stop,
            "deck.q4.capture.status": self._capture_status,
            "worker.status": lambda _: self.status(),
            "metrics.get": lambda _: self.metrics(),
            "worker.shutdown": self._shutdown_worker,
        }
        handler = handlers.get(name)
        if handler is None:
            raise WorkerCommandError(
                "protocol.unknown_command", "unknown Q4 worker command", fatal=True
            )
        if name != "session.configure" and not self._configured:
            raise WorkerCommandError("state.invalid_transition", "worker session is not configured")
        return handler(payload)

    def status(self) -> dict[str, object]:
        result: dict[str, object] = {
            "worker_state": self._worker_state,
            "codec_state": self._codec_state,
            "slot_state": self._slot_state,
            "ring_state": self._ring_state,
            "worker_version": WORKER_VERSION,
            "protocol_version": 1,
        }
        if self._engine is not None:
            result["active_generation"] = self._engine.status()["stream_generation"]
        if self._deck_id is not None:
            result["active_slot_id"] = self._deck_id
        return result

    def deck_status(self) -> dict[str, object]:
        engine = self._require_engine()
        status = engine.status()
        return {
            "deck_id": self._deck_id,
            "deck_revision": self._deck_revision,
            "operator_id": Q4_OPERATOR_ID,
            "operator_version": Q4_OPERATOR_VERSION,
            "stream_generation": status["stream_generation"],
            "stream_sequence": status["stream_sequence"],
            "playhead_a": status["playhead_a"],
            "playhead_b": status["playhead_b"],
            "playhead_c": status["playhead_c"],
            "playhead_d": status["playhead_d"],
            "roles": status["roles"],
            "transport": status["transport"],
            "controls": status["controls"],
            "seed": status["seed"],
            "pending_reset": status["pending_reset"],
            "pending_reset_reasons": status["pending_reset_reasons"],
            "decoded_start_frame": self._decoded_start_frame,
            **{
                f"source_{slot.lower()}": self._source_status(self._require_source(slot))
                for slot in SLOTS
            },
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
        self._sources = {slot: None for slot in SLOTS}
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
        decoder: Q4Decoder | None = None
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
            raise WorkerCommandError("state.invalid_transition", "a Q4 deck is already active")
        self._slot_state = "loading"
        bindings = {
            slot: self._source_binding(payload[f"source_{slot.lower()}"], f"source_{slot.lower()}")
            for slot in SLOTS
        }
        try:
            sources = {
                slot: self._source_loader(
                    binding["cartridge_path"],
                    binding["expected_archive_sha256"],
                )
                for slot, binding in bindings.items()
            }
        except Exception as error:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "slot.cartridge_invalid", "Q4 cartridge validation changed"
            ) from error
        if any(sources[slot].cartridge_id != bindings[slot]["cartridge_id"] for slot in SLOTS):
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "slot.cartridge_hash_mismatch", "Q4 cartridge identity changed"
            )
        reference = sources["A"]
        if any(
            (sources[slot].width, sources[slot].height) != (reference.width, reference.height)
            for slot in SLOTS[1:]
        ):
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "deck.source_incompatible", "Q4 decoded presentation geometry differs"
            )
        try:
            engine = self._engine_factory(
                *(sources[slot] for slot in SLOTS),
                decoder,
                torch=self._torch,
                device=self._device,
                roles=self._roles(payload["roles"]),
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
            raise WorkerCommandError(code, "trusted Q4 operator could not be loaded") from error
        except (Q4StreamError, H3Q4SourceError) as error:
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "deck.source_incompatible", "Q4 sources or roles are incompatible"
            ) from error
        self._engine = engine
        self._sources = {slot: sources[slot] for slot in SLOTS}
        self._deck_id = str(payload["deck_id"])
        self._deck_revision += 1
        self._decoded_start_frame = 0
        self._slot_state = "ready"
        return self.deck_status()

    def _ring_bind(self, payload: Mapping[str, object]) -> dict[str, object]:
        source = self._require_source("A")
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
                "ring.layout_incompatible", "Q4 RGB ring binding failed"
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
            raise WorkerCommandError("state.stale_generation", "Q4 generation is stale")
        ring = self._require_ring()
        transport = status["transport"]
        is_paused = isinstance(transport, Mapping) and not any(
            transport.get(f"playing_{slot.lower()}") for slot in SLOTS
        )
        if not status["pending_reset"] and not is_paused and not ring.can_publish(4):
            self._metrics.ring_backpressure_total += 1
            raise WorkerCommandError(
                "ring.backpressure",
                "RGB ring has insufficient capacity for one Q4 decoder slot",
                retryable=True,
            )
        self._worker_state = "busy"
        self._slot_state = "decoding"
        started = time.monotonic_ns()
        try:
            step = engine.step(self._capture_before_decode)
            if isinstance(step, Q4ResetBarrier):
                if self._capture is not None and self._capture.is_active:
                    self._capture.prepare_loop_reset(step.reasons)
                self._slot_state = "ready"
                return self._barrier_payload(step)
            if isinstance(step, Q4Paused):
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
                    "playhead_c": step.playhead_c,
                    "playhead_d": step.playhead_d,
                    "roles": step.roles.as_dict(),
                    "transport": engine.status()["transport"],
                }
            if not isinstance(step, Q4DecodedSlot):
                raise Q4StreamError("deck.process_failed", "Q4 step type is invalid")
            frames = tuple(step.decoded)
            source_a = self._require_source("A")
            expected_bytes = source_a.width * source_a.height * 4
            if not 1 <= len(frames) <= 4 or any(
                not isinstance(frame, bytes) or len(frame) != expected_bytes for frame in frames
            ):
                raise Q4StreamError("deck.decode_failed", "decoder frame batch is invalid")
            provenance_json = json.dumps(
                step.latent.provenance,
                allow_nan=False,
                sort_keys=True,
                separators=(",", ":"),
            )
            if len(provenance_json.encode("utf-8")) > MAX_PROVENANCE_JSON_BYTES:
                raise Q4StreamError("deck.process_failed", "Q4 provenance exceeds its wire bound")
            decoded_start = self._decoded_start_frame
            if decoded_start > 0xFFFF_FFFF_FFFF_FFFF - len(frames):
                raise Q4StreamError("deck.decode_failed", "Q4 decoded frame counter is exhausted")
            first, last = ring.publish_frames(
                frames,
                stream_generation=step.latent.stream_generation,
            )
            if self._capture is not None:
                self._capture.after_decode(step.latent)
        except Q4CaptureError as error:
            if self._capture is not None and self._capture.should_cleanup_on_error:
                self._capture.abort("capture_error")
            self._slot_state = "faulted"
            raise WorkerCommandError(
                error.code,
                "Q4 capture failed after stream advance; restart the worker session",
                fatal=True,
            ) from error
        except Q4StreamError as error:
            if self._capture is not None and self._capture.should_cleanup_on_error:
                self._capture.abort("process_or_decode_error")
            fatal = error.code == "deck.decode_failed"
            self._slot_state = "faulted" if fatal else "ready"
            raise WorkerCommandError(
                "decode.failed" if fatal else "deck.process_failed",
                "Q4 process/decode failed",
                fatal=fatal,
            ) from error
        except Exception as error:
            if self._capture is not None and self._capture.should_cleanup_on_error:
                self._capture.abort("process_or_publish_error")
            self._slot_state = "faulted"
            raise WorkerCommandError(
                "decode.failed",
                "Q4 decode or RGB publish failed; restart the worker session",
                fatal=True,
            ) from error
        finally:
            self._worker_state = "ready"
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
            "playhead_c": step.latent.playhead_c,
            "playhead_d": step.latent.playhead_d,
            "roles": step.latent.roles.as_dict(),
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
        crossing_live_loop = capture is not None and capture.is_awaiting_loop_reset
        if capture is not None and capture.is_active and not crossing_live_loop:
            capture.abort("causal_reset")
        try:
            result = engine.apply_reset_barrier(
                new_generation,
                lambda: ring.set_generation(new_generation),
            )
        except Q4StreamError as error:
            if capture is not None and (capture.is_awaiting_reset or crossing_live_loop):
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
                "Q4 causal reset was rejected",
                fatal=code == "deck.reset_failed",
            ) from error
        if capture is not None and capture.is_awaiting_reset:
            try:
                capture.activate(result)
            except Q4CaptureError as error:
                raise WorkerCommandError(
                    error.code,
                    "Q4 capture could not enter its codec boundary",
                    fatal=True,
                ) from error
        elif capture is not None and crossing_live_loop:
            try:
                capture.resume_after_loop_reset(result)
            except Q4CaptureError as error:
                raise WorkerCommandError(
                    error.code,
                    "Q4 Live Capture could not resume after its loop reset",
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
        if self._capture is not None and self._capture.locks_transport:
            raise WorkerCommandError(
                "capture.transport_locked", "capture transport is frozen until completion"
            )
        try:
            barrier = self._require_engine().request_restart()
        except Q4StreamError as error:
            raise WorkerCommandError(
                "deck.generation_exhausted", "Q4 stream generation is exhausted"
            ) from error
        return self._barrier_payload(barrier)

    def _controls_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        self._prepare_state_change("controls")
        try:
            result = self._require_engine().update_controls(payload["controls"])
        except (Q4StreamError, OperatorLoadError, ValueError) as error:
            raise WorkerCommandError("deck.process_failed", "Q4 controls are invalid") from error
        self._record_capture_state()
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _roles_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        self._prepare_state_change("roles")
        try:
            result = self._require_engine().update_roles(self._roles(payload["roles"]))
        except (Q4StreamError, ValueError) as error:
            raise WorkerCommandError("deck.roles_invalid", "Q4 roles are invalid") from error
        self._record_capture_state()
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _transport_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        if self._capture is not None and self._capture.locks_transport:
            raise WorkerCommandError(
                "capture.transport_locked", "capture transport is frozen until completion"
            )
        try:
            result = self._require_engine().update_transport(self._transport(payload["transport"]))
        except Q4StreamError as error:
            raise WorkerCommandError("deck.transport_invalid", "Q4 transport is invalid") from error
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _seed_set(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        self._prepare_state_change("seed")
        seed = int(payload["seed"])
        if not 0 <= seed <= MAX_SAFE_SEED:
            raise WorkerCommandError("protocol.schema_invalid", "Q4 seed is outside u53")
        try:
            result = self._require_engine().update_seed(seed)
        except Q4StreamError as error:
            raise WorkerCommandError("deck.seed_invalid", "Q4 seed is invalid") from error
        self._record_capture_state()
        return {"deck_id": self._deck_id, "deck_revision": self._deck_revision, **result}

    def _prepare_state_change(self, label: str) -> None:
        capture = self._capture
        if capture is not None and capture.is_snapshot_locked:
            raise WorkerCommandError(
                "capture.snapshot_frozen",
                f"Snapshot {label} are frozen until capture completes",
            )
        if capture is not None:
            try:
                capture.ensure_event_capacity()
            except Q4CaptureError as error:
                raise WorkerCommandError(
                    error.code, "Live Capture state history is full"
                ) from error

    def _record_capture_state(self) -> None:
        capture = self._capture
        if capture is None or not capture.accepts_live_events:
            return
        status = self._require_engine().status()
        roles = status["roles"]
        controls = status["controls"]
        if not isinstance(roles, Mapping) or not isinstance(controls, Mapping):
            raise WorkerCommandError(
                "capture.provenance_invalid", "Q4 runtime state is not serializable", fatal=True
            )
        try:
            capture.record_state(roles, controls, int(status["seed"]))
        except Q4CaptureError as error:
            raise WorkerCommandError(
                error.code, "Live Capture state event is invalid", fatal=True
            ) from error

    def _capture_start(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        self._require_ring()
        engine = self._require_engine()
        status = engine.status()
        if status["pending_reset"]:
            raise WorkerCommandError(
                "capture.boundary_unavailable", "Q4 already has a causal reset pending"
            )
        previous_capture = self._capture
        if previous_capture is not None and (
            previous_capture.is_active or previous_capture.is_awaiting_reset
        ):
            raise WorkerCommandError("capture.already_active", "a Q4 capture is already active")
        controls = status["controls"]
        roles = status["roles"]
        transport = status["transport"]
        if not all(isinstance(value, Mapping) for value in (controls, roles, transport)):
            raise WorkerCommandError("capture.start_failed", "Q4 capture state is invalid")
        carrier = roles.get("carrier")  # type: ignore[union-attr]
        if carrier not in SLOTS or not transport.get(f"playing_{str(carrier).lower()}"):  # type: ignore[union-attr]
            raise WorkerCommandError(
                "capture.carrier_paused", "structural carrier must be playing at capture start"
            )
        if payload["mode"] == "snapshot":
            carrier_source = self._require_source(str(carrier))
            for slot in SLOTS:
                if slot == carrier:
                    continue
                donor_source = self._require_source(slot)
                if (
                    donor_source.latent_slot_count < carrier_source.latent_slot_count
                    and transport.get(f"playing_{slot.lower()}") is True  # type: ignore[union-attr]
                    and transport.get(f"loop_{slot.lower()}") is True  # type: ignore[union-attr]
                ):
                    raise WorkerCommandError(
                        "capture.source_cycle_incompatible",
                        "looping donor ends before the Snapshot carrier cycle",
                    )
        capture: Q4CaptureSession | None = None
        try:
            current_generation = int(status["stream_generation"])
            capture = Q4CaptureSession(
                capture_id=str(payload["capture_id"]),
                mode=str(payload["mode"]),
                temporary_root=str(payload["temporary_root"]),
                max_latent_slots=int(payload["max_latent_slots"]),
                max_visual_bytes=int(payload["max_visual_bytes"]),
                source_a=self._require_source("A"),
                source_b=self._require_source("B"),
                source_c=self._require_source("C"),
                source_d=self._require_source("D"),
                roles=roles,  # type: ignore[arg-type]
                controls=controls,  # type: ignore[arg-type]
                seed=int(status["seed"]),
                current_generation=current_generation,
                minimum_new_generation=current_generation + 1,
            )
            barrier = engine.request_restart()
            if (
                barrier.current_generation != current_generation
                or barrier.minimum_new_generation != current_generation + 1
            ):
                raise Q4CaptureError(
                    "capture.boundary_invalid", "restart barrier generation changed unexpectedly"
                )
        except (Q4CaptureError, Q4StreamError, OSError, ValueError) as error:
            if capture is not None:
                capture.abort("start_failed")
            code = error.code if isinstance(error, Q4CaptureError) else "capture.start_failed"
            raise WorkerCommandError(code, "Q4 capture could not start") from error
        if previous_capture is not None:
            previous_capture.abort("replaced")
        self._capture = capture
        return capture.status()

    def _capture_stop(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        capture = self._checked_capture(payload)
        try:
            capture.request_stop()
        except Q4CaptureError as error:
            raise WorkerCommandError(error.code, "Q4 capture could not stop") from error
        return capture.status()

    def _capture_status(self, payload: Mapping[str, object]) -> dict[str, object]:
        self._check_deck(payload)
        return self._checked_capture(payload).status()

    def _checked_capture(self, payload: Mapping[str, object]) -> Q4CaptureSession:
        capture = self._capture
        if capture is None:
            raise WorkerCommandError("capture.not_found", "Q4 capture is absent")
        if payload["capture_id"] != capture.capture_id:
            raise WorkerCommandError("capture.id_mismatch", "Q4 capture identity is stale")
        return capture

    def _capture_before_decode(self, step: Q4ProcessedSlot) -> None:
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
            raise WorkerCommandError("state.stale_slot_revision", "Q4 deck revision is stale")

    def _barrier_payload(self, barrier: Q4ResetBarrier) -> dict[str, object]:
        return {
            "kind": "reset_barrier",
            "deck_id": self._deck_id,
            "deck_revision": self._deck_revision,
            **barrier.as_dict(),
        }

    def _require_decoder(self) -> Q4Decoder:
        if self._decoder is None or self._codec_state != "ready":
            raise WorkerCommandError("state.invalid_transition", "H3 codec is not ready")
        return self._decoder

    def _require_engine(self) -> H3Q4StreamEngine:
        if self._engine is None or self._deck_id is None:
            raise WorkerCommandError("state.invalid_transition", "Q4 deck is not loaded")
        return self._engine

    def _require_ring(self) -> Q4RingProducer:
        if self._ring is None or self._ring_state != "ready":
            raise WorkerCommandError("ring.unbound", "Q4 RGB ring is not ready")
        return self._ring

    def _require_source(self, slot: str) -> H3VideoSource:
        source = self._sources.get(slot)
        if source is None:
            raise WorkerCommandError("state.invalid_transition", "Q4 source is absent")
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
    def _roles(raw: object) -> Q4RoleAssignment:
        if not isinstance(raw, Mapping):
            raise WorkerCommandError("protocol.schema_invalid", "Q4 roles are invalid")
        try:
            return Q4RoleAssignment.from_mapping(raw)
        except Q4StreamError as error:
            raise WorkerCommandError("deck.roles_invalid", "Q4 roles are invalid") from error

    @staticmethod
    def _transport(raw: object) -> Q4Transport:
        if not isinstance(raw, Mapping):
            raise WorkerCommandError("protocol.schema_invalid", "Q4 transport is invalid")
        try:
            return Q4Transport.from_mapping(raw)
        except Q4StreamError as error:
            raise WorkerCommandError("deck.transport_invalid", "Q4 transport is invalid") from error

    @staticmethod
    def _load_decoder(path: str, sha256: str, byte_length: int, ordinal: int) -> Q4Decoder:
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
    ) -> Q4RingProducer:
        from .ring import WindowsRingProducer

        return WindowsRingProducer.bind(
            payload,
            source.width,
            source.height,
            stream_generation,
        )


__all__ = ["H3Q4WorkerState"]
