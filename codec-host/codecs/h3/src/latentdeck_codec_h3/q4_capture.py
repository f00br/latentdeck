"""Bounded Snapshot and Live Capture state for the H3 LD-Q4 worker."""

from __future__ import annotations

import copy
import json
import uuid
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, Protocol

from .cartridge import H3VideoSource
from .resample_spool import H3ResampleSpool, ResampleSpoolError

CAPTURE_MODES = frozenset({"snapshot", "live_capture"})
MAX_CAPTURE_RECEIPT_BYTES = 32_768
MAX_CAPTURE_STATE_EVENTS = 32
SLOTS = ("A", "B", "C", "D")
ROLE_KEYS = ("carrier", "donor_b", "donor_c", "donor_d")


class Q4CaptureStep(Protocol):
    stream_generation: int
    stream_sequence: int
    playhead_a: int
    playhead_b: int
    playhead_c: int
    playhead_d: int
    output: Any


class Q4CaptureError(RuntimeError):
    """A capture command or pre-decode sink transition failed safely."""

    def __init__(self, code: str, detail: str) -> None:
        super().__init__(f"{code}: {detail}")
        self.code = code
        self.detail = detail


class Q4CaptureSession:
    """Own one Q4 disk spool from restart barrier through bounded receipt."""

    def __init__(
        self,
        *,
        capture_id: str,
        mode: str,
        temporary_root: str | Path,
        max_latent_slots: int,
        max_visual_bytes: int,
        source_a: H3VideoSource,
        source_b: H3VideoSource,
        source_c: H3VideoSource,
        source_d: H3VideoSource,
        roles: Mapping[str, object],
        controls: Mapping[str, object],
        seed: int,
        current_generation: int,
        minimum_new_generation: int,
    ) -> None:
        self.capture_id = _capture_id(capture_id)
        if mode not in CAPTURE_MODES:
            raise Q4CaptureError("capture.mode_invalid", "capture mode is not supported")
        self.mode = mode
        self._sources = {
            "A": source_a,
            "B": source_b,
            "C": source_c,
            "D": source_d,
        }
        self._frozen_roles = _roles(roles)
        self.structural_carrier = self._frozen_roles["carrier"]
        self._frozen_controls = copy.deepcopy(dict(controls))
        self._frozen_seed = _seed(seed)
        slot_bytes = 24 * source_a.shape[3] * source_a.shape[4] * 2
        bounded_slots = min(max_latent_slots, max_visual_bytes // slot_bytes)
        self._live_max_valid_slots = 2 + 5 * ((bounded_slots - 2) // 5) if bounded_slots >= 2 else 0
        if mode == "live_capture" and self._live_max_valid_slots < 2:
            raise Q4CaptureError(
                "capture.limit_exceeded",
                "Live Capture limits cannot contain one codec-valid temporal boundary",
            )
        self._state_events = [
            {
                "slot_offset": 0,
                "roles": copy.deepcopy(self._frozen_roles),
                "controls": copy.deepcopy(self._frozen_controls),
                "seed": self._frozen_seed,
            }
        ]
        self._carrier_mapping_changed = False
        self._current_generation = current_generation
        self._minimum_new_generation = minimum_new_generation
        self._stream_generation: int | None = None
        self._expected_carrier_playhead = 0
        self._loop_reset_pending = False
        self._state = "awaiting_reset"
        self._abort_reason: str | None = None
        self._receipt: dict[str, object] | None = None
        self._finish_after: tuple[int, int] | None = None
        self._finalize_after_latent_slots: int | None = None
        carrier = self._sources[self.structural_carrier]
        self._target_latent_slots = carrier.latent_slot_count if mode == "snapshot" else 0
        if mode == "snapshot" and max_latent_slots < self._target_latent_slots:
            raise Q4CaptureError(
                "capture.limit_exceeded", "snapshot exceeds the host capture slot limit"
            )
        snapshot_visual_bytes = (
            self._target_latent_slots * 24 * source_a.shape[3] * source_a.shape[4] * 2
        )
        if mode == "snapshot" and max_visual_bytes < snapshot_visual_bytes:
            raise Q4CaptureError(
                "capture.limit_exceeded", "snapshot exceeds the host visual-byte limit"
            )
        root = Path(temporary_root)
        if not root.is_absolute():
            raise Q4CaptureError(
                "capture.temporary_root_invalid", "temporary root must be absolute"
            )
        try:
            resolved_root = root.resolve(strict=True)
        except OSError as error:
            raise Q4CaptureError(
                "capture.temporary_root_invalid", "temporary root does not exist"
            ) from error
        if not resolved_root.is_dir():
            raise Q4CaptureError(
                "capture.temporary_root_invalid", "temporary root is not a directory"
            )
        try:
            self._spool = H3ResampleSpool(
                resolved_root,
                self.capture_id,
                latent_height=source_a.shape[3],
                latent_width=source_a.shape[4],
                max_latent_slots=max_latent_slots,
                max_visual_bytes=max_visual_bytes,
            )
        except ResampleSpoolError as error:
            raise Q4CaptureError("capture.start_failed", "capture spool could not start") from error

    @property
    def is_awaiting_reset(self) -> bool:
        return self._state == "awaiting_reset"

    @property
    def is_active(self) -> bool:
        return self._state in {"capturing", "stop_armed"}

    @property
    def is_awaiting_loop_reset(self) -> bool:
        return self.is_active and self._loop_reset_pending

    @property
    def is_snapshot_locked(self) -> bool:
        return self.mode == "snapshot" and self._state in {
            "awaiting_reset",
            "capturing",
            "stop_armed",
        }

    @property
    def should_cleanup_on_error(self) -> bool:
        return self._state in {"awaiting_reset", "capturing", "stop_armed", "finished"}

    @property
    def accepts_live_events(self) -> bool:
        return self.mode == "live_capture" and self._state in {
            "awaiting_reset",
            "capturing",
            "stop_armed",
        }

    @property
    def locks_transport(self) -> bool:
        return self._state in {"awaiting_reset", "capturing", "stop_armed"}

    def activate(self, reset_result: Mapping[str, object]) -> None:
        if self._state != "awaiting_reset":
            raise Q4CaptureError("capture.invalid_state", "capture is not awaiting its reset")
        generation = reset_result.get("stream_generation")
        at_origin = all(reset_result.get(f"playhead_{slot.lower()}") == 0 for slot in SLOTS)
        if (
            isinstance(generation, bool)
            or not isinstance(generation, int)
            or generation < self._minimum_new_generation
            or not at_origin
        ):
            self.abort("start_reset_invalid")
            raise Q4CaptureError(
                "capture.boundary_invalid", "capture reset did not reach the stream origin"
            )
        self._stream_generation = generation
        self._expected_carrier_playhead = 0
        self._loop_reset_pending = False
        self._state = "capturing"

    def before_decode(self, step: Q4CaptureStep) -> None:
        """Persist one exact post-operator slot before causal decode begins."""

        if self._state not in {"capturing", "stop_armed"}:
            return
        if self._loop_reset_pending:
            self.abort("reset_not_applied")
            raise Q4CaptureError(
                "capture.boundary_invalid", "capture received a slot before its loop reset"
            )
        playhead = getattr(step, f"playhead_{self.structural_carrier.lower()}")
        if (
            step.stream_generation != self._stream_generation
            or playhead != self._expected_carrier_playhead
        ):
            self.abort("stream_mapping_changed")
            raise Q4CaptureError(
                "capture.mapping_changed", "capture no longer follows the structural carrier"
            )
        try:
            self._spool.append_slot(step.output)
        except ResampleSpoolError as error:
            self.abort("spool_write_failed")
            raise Q4CaptureError(
                "capture.write_failed", "post-operator slot could not be persisted"
            ) from error
        self._expected_carrier_playhead += 1
        should_finish = (
            self.mode == "snapshot" and self._spool.latent_slots == self._target_latent_slots
        ) or (
            self.mode == "live_capture"
            and (
                (
                    self._state == "stop_armed"
                    and self._spool.latent_slots == self._finalize_after_latent_slots
                )
                or self._spool.latent_slots == self._live_max_valid_slots
            )
        )
        if should_finish:
            self._finish_after = (step.stream_generation, step.stream_sequence)

    def after_decode(self, step: Q4CaptureStep) -> None:
        if self._finish_after != (step.stream_generation, step.stream_sequence):
            return
        self._finish_after = None
        self._finish()

    def request_stop(self) -> None:
        if self.mode != "live_capture":
            raise Q4CaptureError("capture.mode_invalid", "Snapshot capture stops automatically")
        if self._state in {"finished", "stop_armed"}:
            return
        if self._state == "awaiting_reset":
            self.abort("stopped_before_start")
            return
        if self._state != "capturing":
            raise Q4CaptureError("capture.invalid_state", "Live Capture is not running")
        if self._spool.latent_slots >= 2 and (self._spool.latent_slots - 2) % 5 == 0:
            self._finish()
            return
        target = _next_valid_slot_count(self._spool.latent_slots + 1)
        if target > self._live_max_valid_slots:
            self.abort("stop_boundary_exceeds_limit")
            raise Q4CaptureError(
                "capture.limit_exceeded", "next codec-valid stop exceeds the capture limit"
            )
        self._finalize_after_latent_slots = target
        self._state = "stop_armed"

    def prepare_loop_reset(self, reasons: Sequence[str]) -> None:
        """Retain Live Capture ownership across one codec loop reset barrier."""

        if self.mode != "live_capture" or not self.is_active:
            raise Q4CaptureError(
                "capture.boundary_invalid", "only active Live Capture may cross a loop reset"
            )
        if self._loop_reset_pending:
            raise Q4CaptureError(
                "capture.boundary_invalid", "a Live Capture loop reset is already pending"
            )
        if not reasons or any(
            not isinstance(reason, str)
            or not reason.startswith("slot_")
            or not reason.endswith(".loop")
            for reason in reasons
        ):
            raise Q4CaptureError(
                "capture.boundary_invalid", "Live Capture may cross only automatic slot loops"
            )
        self._loop_reset_pending = True

    def resume_after_loop_reset(self, reset_result: Mapping[str, object]) -> None:
        if not self.is_awaiting_loop_reset or self._stream_generation is None:
            raise Q4CaptureError(
                "capture.boundary_invalid", "Live Capture has no pending loop reset"
            )
        generation = reset_result.get("stream_generation")
        playhead = reset_result.get(f"playhead_{self.structural_carrier.lower()}")
        carrier = self._sources[self.structural_carrier]
        if (
            isinstance(generation, bool)
            or not isinstance(generation, int)
            or generation <= self._stream_generation
            or isinstance(playhead, bool)
            or not isinstance(playhead, int)
            or not 0 <= playhead < carrier.latent_slot_count
        ):
            self.abort("loop_reset_invalid")
            raise Q4CaptureError(
                "capture.boundary_invalid", "Live Capture loop reset mapping is invalid"
            )
        self._stream_generation = generation
        self._expected_carrier_playhead = playhead
        self._loop_reset_pending = False

    def ensure_event_capacity(self) -> None:
        if self.accepts_live_events and len(self._state_events) >= MAX_CAPTURE_STATE_EVENTS:
            raise Q4CaptureError("capture.event_limit", "Live Capture state-event history is full")

    def record_state(
        self,
        roles: Mapping[str, object],
        controls: Mapping[str, object],
        seed: int,
    ) -> None:
        if not self.accepts_live_events:
            return
        self.ensure_event_capacity()
        parsed_roles = _roles(roles)
        event = {
            "slot_offset": self._spool.latent_slots,
            "roles": parsed_roles,
            "controls": copy.deepcopy(dict(controls)),
            "seed": _seed(seed),
        }
        if parsed_roles["carrier"] != self.structural_carrier:
            self._carrier_mapping_changed = True
        _bounded_json(event, "capture state event")
        self._state_events.append(event)

    def status(self) -> dict[str, object]:
        result: dict[str, object] = {
            "capture_id": self.capture_id,
            "mode": self.mode,
            "state": self._state,
            "structural_carrier": self.structural_carrier,
            "latent_slots": self._spool.latent_slots,
        }
        if self._state == "awaiting_reset":
            result.update(
                {
                    "current_generation": self._current_generation,
                    "minimum_new_generation": self._minimum_new_generation,
                    "target_latent_slots": self._target_latent_slots,
                }
            )
        elif self._stream_generation is not None:
            result["stream_generation"] = self._stream_generation
        if self._finalize_after_latent_slots is not None:
            result["finalize_after_latent_slots"] = self._finalize_after_latent_slots
        if self._abort_reason is not None:
            result["reason"] = self._abort_reason
        if self._receipt is not None:
            result["receipt"] = copy.deepcopy(self._receipt)
        _bounded_json(result, "capture status")
        return result

    def abort(self, reason: str) -> None:
        self._spool.abort()
        self._state = "aborted"
        self._abort_reason = reason
        self._receipt = None
        self._finish_after = None
        self._finalize_after_latent_slots = None
        self._loop_reset_pending = False

    def _finish(self) -> None:
        carrier = self._sources[self.structural_carrier]
        captured_frames = 5 + 17 * ((self._spool.latent_slots - 2) // 5)
        exact_carrier_duration = (
            self._spool.latent_slots == carrier.latent_slot_count
            and captured_frames == carrier.frame_count
        )
        audio = None
        audio_policy = "source_absent"
        audio_policy_reason: str | None = None
        if carrier.audio is not None:
            carrier_audio_slots = (carrier.frame_count * 5 + 1) // 3
            exact_temporal_mapping = (
                carrier.audio.shape[3] == carrier_audio_slots and not self._carrier_mapping_changed
            )
            if exact_carrier_duration and exact_temporal_mapping:
                audio = carrier.audio.to_resample_source()
                audio_policy = "copied_from_carrier_exact"
            else:
                audio_policy = "omitted_timing_mismatch"
                if not exact_carrier_duration and not exact_temporal_mapping:
                    audio_policy_reason = "duration_and_mapping_mismatch"
                elif not exact_carrier_duration:
                    audio_policy_reason = "duration_mismatch"
                else:
                    audio_policy_reason = "temporal_mapping_mismatch"
        try:
            spool_receipt = self._spool.finish(audio=audio)
        except Exception as error:
            self.abort("spool_finalize_failed")
            raise Q4CaptureError(
                "capture.write_failed", "capture payload could not be finalized"
            ) from error
        receipt: dict[str, object] = {
            "capture_id": self.capture_id,
            "mode": self.mode,
            "payload_path": str(spool_receipt.payload_path),
            "payload_sha256": spool_receipt.sha256,
            "payload_bytes": spool_receipt.byte_length,
            "storage_dtype": spool_receipt.storage_dtype,
            "visual_shape": list(spool_receipt.shape),
            "decoded_frame_count": spool_receipt.decoded_frame_count,
            "audio_policy": audio_policy,
            "structural_carrier": self.structural_carrier,
            "parents": [
                {
                    "slot": slot,
                    "cartridge_id": source.cartridge_id,
                    "archive_sha256": source.archive_sha256,
                }
                for slot, source in self._sources.items()
            ],
        }
        if audio_policy_reason is not None:
            receipt["audio_policy_reason"] = audio_policy_reason
        if spool_receipt.audio is not None:
            receipt["audio_descriptor"] = {
                "storage_dtype": spool_receipt.audio.storage_dtype,
                "shape": list(spool_receipt.audio.shape),
                "byte_length": spool_receipt.audio.byte_length,
            }
        if self.mode == "snapshot":
            receipt["frozen_seed"] = self._frozen_seed
            receipt["frozen_roles"] = copy.deepcopy(self._frozen_roles)
            receipt["frozen_controls"] = copy.deepcopy(self._frozen_controls)
        else:
            receipt["control_events"] = copy.deepcopy(
                [
                    event
                    for event in self._state_events
                    if int(event["slot_offset"]) < self._spool.latent_slots
                ]
            )
        _bounded_json(receipt, "capture receipt")
        self._receipt = receipt
        self._finalize_after_latent_slots = None
        self._loop_reset_pending = False
        self._state = "finished"


def _capture_id(value: str) -> str:
    try:
        parsed = uuid.UUID(value)
    except (AttributeError, ValueError) as error:
        raise Q4CaptureError("capture.id_invalid", "capture_id is not a UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise Q4CaptureError("capture.id_invalid", "capture_id is not canonical")
    return value


def _roles(value: Mapping[str, object]) -> dict[str, str]:
    if not isinstance(value, Mapping) or set(value) != set(ROLE_KEYS):
        raise Q4CaptureError("capture.roles_invalid", "Q4 roles are invalid")
    parsed = {key: value[key] for key in ROLE_KEYS}
    if any(not isinstance(slot, str) for slot in parsed.values()) or set(parsed.values()) != set(
        SLOTS
    ):
        raise Q4CaptureError("capture.roles_invalid", "Q4 roles must permute A, B, C, D")
    return {key: str(slot) for key, slot in parsed.items()}


def _seed(value: int) -> int:
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or not 0 <= value <= 9_007_199_254_740_991
    ):
        raise Q4CaptureError("capture.seed_invalid", "capture seed is outside u53")
    return value


def _next_valid_slot_count(minimum: int) -> int:
    if minimum <= 2:
        return 2
    return 2 + 5 * ((minimum - 2 + 4) // 5)


def _bounded_json(value: Mapping[str, object], label: str) -> str:
    try:
        encoded = json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=True,
            sort_keys=True,
            separators=(",", ":"),
        )
    except (TypeError, ValueError) as error:
        raise Q4CaptureError("capture.provenance_invalid", f"{label} is not JSON-safe") from error
    if len(encoded.encode("utf-8")) > MAX_CAPTURE_RECEIPT_BYTES:
        raise Q4CaptureError("capture.receipt_too_large", f"{label} exceeds its byte limit")
    return encoded


__all__ = [
    "CAPTURE_MODES",
    "MAX_CAPTURE_RECEIPT_BYTES",
    "MAX_CAPTURE_STATE_EVENTS",
    "Q4CaptureError",
    "Q4CaptureSession",
]
