# ruff: noqa: E402

from __future__ import annotations

import hashlib
import io
import math
import uuid
from array import array
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any, BinaryIO

import pytest

torch = pytest.importorskip("torch", reason="D2 worker conformance requires the runtime extra")

from latentdeck_codec_host.protocol import Bootstrap, encode_bootstrap, read_frame, write_frame

from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.d2_worker import run_d2_worker
from latentdeck_codec_h3.d2_worker_state import H3D2WorkerState
from latentdeck_codec_h3.decoder import RuntimeDevice, RuntimeInspection
from latentdeck_codec_h3.resample_spool import ResampleAudioSource
from latentdeck_codec_h3.worker import StreamConnection
from latentdeck_codec_h3.worker_state import WorkerCommandError

SESSION_ID = "9ca8c228-04c7-4b59-909f-6fbef591a43e"
PIPE_NAME = rf"\\.\pipe\LatentDeck.Worker.{SESSION_ID}"


def controls(**changes: object) -> dict[str, object]:
    result: dict[str, object] = {
        "algorithm": "LINEAR",
        "mix": 0.5,
        "mode": "HYBRIDIZE",
        "routing": "A",
        "interaction": 0.0,
        "preserve": 0.55,
        "chaos": 0.0,
        "xs1_channel_a": 0,
        "xs1_channel_b": 1,
        "xs1_angle_degrees": 30.0,
        "xs2_radius": 1,
        "xs3_high_gain": 0.5,
        "xs4_epsilon": 0.000001,
        "xs5_routing": "TOPK",
        "temperature": 0.12,
        "top_k": 8,
        "sinkhorn_iterations": 5,
    }
    result.update(changes)
    return result


class SyntheticAudio:
    def __init__(self, decoded_frames: int) -> None:
        audio_slots = (decoded_frames * 5 + 1) // 3
        self.storage_dtype = "F32"
        self.shape = (1, 32, 2, audio_slots)
        self.encoded = bytes(index % 251 for index in range(1 * 32 * 2 * audio_slots * 4))

    def to_resample_source(self) -> ResampleAudioSource:
        def copy_to(destination: BinaryIO) -> int:
            return destination.write(self.encoded)

        return ResampleAudioSource(
            storage_dtype=self.storage_dtype,
            shape=self.shape,
            byte_length=len(self.encoded),
            copy_to=copy_to,
        )


def source(
    slot_count: int,
    cartridge_id: str,
    archive_byte: str,
    *,
    audio: Any = None,
) -> H3VideoSource:
    values = array(
        "f",
        (
            math.sin(index * 0.031) + 0.1 * math.cos(index * 0.079)
            for index in range(24 * slot_count * 3 * 4)
        ),
    )
    return H3VideoSource(
        cartridge_id=cartridge_id,
        archive_sha256=archive_byte * 64,
        storage_dtype="F32",
        shape=(1, 24, slot_count, 3, 4),
        video_bytes=values.tobytes(),
        width=64,
        height=48,
        frame_count=5 + 17 * ((slot_count - 2) // 5),
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        audio=audio,
    )


class FakeDecoder:
    def __init__(self) -> None:
        self.slots_seen = 0
        self.reset_calls = 0
        self.closed = False
        self.decode_probe: Callable[[], None] | None = None
        self.fail_decode = False

    def decode_slot(self, slot: Any) -> tuple[bytes, ...]:
        assert slot.dtype == torch.float16
        if self.decode_probe is not None:
            self.decode_probe()
        if self.fail_decode:
            raise RuntimeError("synthetic decode failure")
        count = 1 if self.slots_seen == 0 or self.slots_seen % 5 == 0 else 4
        self.slots_seen += 1
        return tuple(bytes(64 * 48 * 4) for _ in range(count))

    def reset(self) -> None:
        self.reset_calls += 1
        self.slots_seen = 0

    def close(self) -> None:
        self.closed = True


class FakeRing:
    def __init__(self, generation: int) -> None:
        self.generation = generation
        self.write_sequence = 0
        self.read_sequence = 0
        self.occupancy = 0
        self.presentation_skipped_total = 0
        self.capacity = 128
        self.closed = False
        self.fail_reset = False
        self.publish_probe: Callable[[], None] | None = None

    def can_publish(self, frame_count: int) -> bool:
        return self.occupancy + frame_count <= self.capacity

    def publish_frames(self, frames: Sequence[bytes], *, stream_generation: int) -> tuple[int, int]:
        assert stream_generation == self.generation
        if self.publish_probe is not None:
            self.publish_probe()
        first = self.write_sequence + 1
        self.write_sequence += len(frames)
        self.occupancy += len(frames)
        return first, self.write_sequence + 1

    def set_generation(self, stream_generation: int) -> None:
        if self.fail_reset:
            raise RuntimeError("synthetic ring reset failure")
        assert stream_generation > self.generation
        self.generation = stream_generation
        self.write_sequence = 0
        self.read_sequence = 0
        self.occupancy = 0

    def close(self) -> None:
        self.closed = True


def session_payload() -> dict[str, object]:
    return {
        "selected_protocol_version": 1,
        "app_version": "0.1.0",
        "heartbeat_interval_ms": 100,
        "heartbeat_hard_timeout_ms": 300,
        "max_frame_bytes": 262_144,
        "max_inflight_decode_batches": 1,
    }


def codec_payload() -> dict[str, object]:
    return {
        "pack_id": "org.latentdeck.h3",
        "pack_version": "0.1.0",
        "adapter_id": "org.latentdeck.h3",
        "profile": {
            "codec_family": "minimax_h3",
            "profile": "h3_av_latent",
            "profile_version": "0.1.0",
        },
        "device_ordinal": 0,
        "assets": [{"asset_id": "taeh3", "path": "weight", "sha256": "c" * 64, "byte_length": 1}],
    }


def configured_state(
    *, initialize: bool = True, source_a_audio: Any = None, source_b_audio: Any = None
) -> tuple[H3D2WorkerState, FakeDecoder, dict[str, FakeRing]]:
    decoder = FakeDecoder()
    source_a = source(
        7,
        "11111111-1111-4111-8111-111111111111",
        "a",
        audio=source_a_audio,
    )
    source_b = source(
        12,
        "22222222-2222-4222-8222-222222222222",
        "b",
        audio=source_b_audio,
    )
    sources = {"A.lc": source_a, "B.lc": source_b}
    rings: dict[str, FakeRing] = {}

    def load_source(path: str | Path, expected_hash: str) -> H3VideoSource:
        loaded = sources[str(path)]
        assert loaded.archive_sha256 == expected_hash
        return loaded

    def bind_ring(
        _payload: Mapping[str, object], _source: H3VideoSource, generation: int
    ) -> FakeRing:
        ring = FakeRing(generation)
        rings["active"] = ring
        return ring

    inspection = RuntimeInspection(
        "2.13.0+cu130",
        True,
        "13.0",
        (RuntimeDevice(0, "Synthetic CUDA", 12_000_000_000),),
    )
    state = H3D2WorkerState(
        decoder_factory=lambda *_: decoder,
        source_loader=load_source,
        ring_factory=bind_ring,
        runtime_inspector=lambda: inspection,
        torch_loader=lambda: torch,
        device_factory=lambda _torch, _ordinal: torch.device("cpu"),
    )
    if initialize:
        state.handle("session.configure", session_payload())
        state.handle("codec.load", codec_payload())
    return state, decoder, rings


def load_payload() -> dict[str, object]:
    return {
        "deck_id": "main-d2",
        "operator_id": "org.latentdeck.builtin.ld_d2",
        "operator_version": "0.1.0",
        "source_a": {
            "cartridge_path": "A.lc",
            "cartridge_id": "11111111-1111-4111-8111-111111111111",
            "expected_archive_sha256": "a" * 64,
        },
        "source_b": {
            "cartridge_path": "B.lc",
            "cartridge_id": "22222222-2222-4222-8222-222222222222",
            "expected_archive_sha256": "b" * 64,
        },
        "controls": controls(),
        "transport": {"playing_a": True, "playing_b": True, "loop_a": True, "loop_b": True},
        "seed": 42,
        "stream_generation": 1,
    }


def bind_payload() -> dict[str, object]:
    return {
        "layout_version": 1,
        "mapping_handle": 1,
        "mapping_bytes": 1_052_672,
        "frames_ready_event_handle": 2,
        "ring_id": "bbfb89cc-0739-423f-9474-d03e01bc34aa",
    }


def identity(generation: int = 1) -> dict[str, object]:
    return {"deck_id": "main-d2", "deck_revision": 1, "stream_generation": generation}


def capture_start_payload(
    temporary_root: Path,
    *,
    capture_id: str = "33333333-3333-4333-8333-333333333333",
    mode: str = "snapshot",
) -> dict[str, object]:
    return {
        "deck_id": "main-d2",
        "deck_revision": 1,
        "capture_id": capture_id,
        "mode": mode,
        "temporary_root": str(temporary_root),
        "max_latent_slots": 128,
        "max_visual_bytes": 16 * 1024 * 1024,
    }


def test_worker_loads_explicit_builtin_and_processes_post_operator_before_ring() -> None:
    state, _decoder, _rings = configured_state()
    loaded = state.handle("deck.d2.load", load_payload())
    assert loaded["operator_id"] == "org.latentdeck.builtin.ld_d2"
    assert loaded["source_a"]["archive_sha256"] == "a" * 64  # type: ignore[index]
    assert "cartridge_path" not in str(loaded)
    state.handle("ring.bind", bind_payload())

    processed = state.handle("deck.d2.process_slot", identity())
    assert processed["kind"] == "decoded_slot"
    assert processed["stream_sequence"] == 0
    assert processed["transport"] == {
        "playing_a": True,
        "playing_b": True,
        "loop_a": True,
        "loop_b": True,
    }
    assert processed["decoded_frame_count"] == 1
    assert processed["ring_first_sequence"] == 1
    provenance = processed["provenance_json"]
    assert isinstance(provenance, str)
    decoded_provenance = __import__("json").loads(provenance)
    assert decoded_provenance["operation"]["operator_id"] == loaded["operator_id"]
    assert decoded_provenance["stream"]["sources"]["a"]["playhead"] == 0
    assert not any("post_operator" in key for key in processed)
    assert not any(isinstance(value, bytes) for value in processed.values())


def test_snapshot_capture_waits_for_a_restart_reset_before_writing(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    state.handle("deck.d2.process_slot", identity())

    armed = state.handle("deck.d2.capture.start", capture_start_payload(tmp_path))

    assert armed == {
        "capture_id": "33333333-3333-4333-8333-333333333333",
        "mode": "snapshot",
        "state": "awaiting_reset",
        "structural_carrier": "A",
        "latent_slots": 0,
        "current_generation": 1,
        "minimum_new_generation": 2,
        "target_latent_slots": 7,
    }
    assert state.handle("deck.d2.process_slot", identity())["kind"] == "reset_barrier"
    assert list(tmp_path.glob("*.visual.f16.partial"))[0].stat().st_size == 0

    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    status = state.handle(
        "deck.d2.capture.status",
        {
            "deck_id": "main-d2",
            "deck_revision": 1,
            "capture_id": "33333333-3333-4333-8333-333333333333",
        },
    )
    assert status["state"] == "capturing"
    assert status["stream_generation"] == 2
    assert status["latent_slots"] == 0


def test_snapshot_rejects_a_shorter_looping_noncarrier_before_restart(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    payload = load_payload()
    payload["controls"] = controls(routing="B")
    state.handle("deck.d2.load", payload)
    state.handle("ring.bind", bind_payload())

    with pytest.raises(WorkerCommandError) as caught:
        state.handle("deck.d2.capture.start", capture_start_payload(tmp_path))

    assert caught.value.code == "capture.source_cycle_incompatible"
    assert state.handle("deck.d2.status", {})["pending_reset"] is False
    assert not list(tmp_path.glob("*.partial"))


def test_snapshot_rejects_known_visual_limit_before_restart(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    start = capture_start_payload(tmp_path)
    full_snapshot_bytes = 1 * 24 * 7 * 3 * 4 * 2
    start["max_visual_bytes"] = full_snapshot_bytes - 1

    with pytest.raises(WorkerCommandError) as caught:
        state.handle("deck.d2.capture.start", start)

    assert caught.value.code == "capture.limit_exceeded"
    assert state.handle("deck.d2.status", {})["pending_reset"] is False
    assert not list(tmp_path.glob("*.partial"))


def test_snapshot_freezes_controls_seed_and_transport_until_finished(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    state.handle("deck.d2.capture.start", capture_start_payload(tmp_path))
    attempts = [
        (
            "deck.d2.controls.set",
            {"deck_id": "main-d2", "deck_revision": 1, "controls": controls(algorithm="XS1")},
            "capture.snapshot_frozen",
        ),
        (
            "deck.d2.seed.set",
            {"deck_id": "main-d2", "deck_revision": 1, "seed": 77},
            "capture.snapshot_frozen",
        ),
        (
            "deck.d2.transport.set",
            {
                "deck_id": "main-d2",
                "deck_revision": 1,
                "transport": {
                    "playing_a": True,
                    "playing_b": False,
                    "loop_a": True,
                    "loop_b": True,
                },
            },
            "capture.transport_locked",
        ),
    ]
    for command, payload, code in attempts:
        with pytest.raises(WorkerCommandError) as caught:
            state.handle(command, payload)
        assert caught.value.code == code
    status = state.handle("deck.d2.status", {})
    assert status["controls"] == controls()
    assert status["seed"] == 42


def test_snapshot_captures_before_decode_and_auto_finishes_one_carrier_cycle(
    tmp_path: Path,
) -> None:
    state, decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "33333333-3333-4333-8333-333333333333"
    state.handle("deck.d2.capture.start", capture_start_payload(tmp_path))
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )

    def observe_pre_decode_spool() -> None:
        status = state.handle(
            "deck.d2.capture.status",
            {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
        )
        assert status["latent_slots"] == decoder.slots_seen + 1

    decoder.decode_probe = observe_pre_decode_spool
    for _ in range(7):
        assert state.handle("deck.d2.process_slot", identity(2))["kind"] == "decoded_slot"

    finished = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert finished["state"] == "finished"
    receipt = finished["receipt"]
    assert isinstance(receipt, dict)
    payload = Path(str(receipt["payload_path"]))
    assert payload.exists()
    assert receipt["payload_sha256"] == hashlib.sha256(payload.read_bytes()).hexdigest()
    assert receipt["visual_shape"] == [1, 24, 7, 3, 4]
    assert receipt["storage_dtype"] == "F16"
    assert receipt["decoded_frame_count"] == 22
    assert receipt["audio_policy"] == "source_absent"
    assert receipt["frozen_seed"] == 42
    assert receipt["frozen_controls"] == controls()
    assert receipt["parents"] == [
        {
            "slot": "A",
            "cartridge_id": "11111111-1111-4111-8111-111111111111",
            "archive_sha256": "a" * 64,
        },
        {
            "slot": "B",
            "cartridge_id": "22222222-2222-4222-8222-222222222222",
            "archive_sha256": "b" * 64,
        },
    ]
    assert not any(isinstance(value, bytes) for value in receipt.values())


def test_live_stop_waits_for_next_valid_length_and_records_control_history(
    tmp_path: Path,
) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "44444444-4444-4444-8444-444444444444"
    awaiting = state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    assert awaiting == {
        "capture_id": capture_id,
        "mode": "live_capture",
        "state": "awaiting_reset",
        "structural_carrier": "A",
        "latent_slots": 0,
        "current_generation": 1,
        "minimum_new_generation": 2,
        "target_latent_slots": 0,
    }
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(2))
    state.handle(
        "deck.d2.controls.set",
        {"deck_id": "main-d2", "deck_revision": 1, "controls": controls(algorithm="XS1")},
    )
    state.handle(
        "deck.d2.seed.set",
        {"deck_id": "main-d2", "deck_revision": 1, "seed": 77},
    )
    state.handle("deck.d2.process_slot", identity(2))

    stop = state.handle(
        "deck.d2.capture.stop",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )

    assert stop["state"] == "stop_armed"
    assert stop["latent_slots"] == 3
    assert stop["finalize_after_latent_slots"] == 7
    for _ in range(3):
        state.handle("deck.d2.process_slot", identity(2))
        status = state.handle(
            "deck.d2.capture.status",
            {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
        )
        assert status["state"] == "stop_armed"
    state.handle("deck.d2.process_slot", identity(2))
    finished = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert finished["state"] == "finished"
    receipt = finished["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["visual_shape"] == [1, 24, 7, 3, 4]
    assert receipt["control_events"] == [
        {"slot_offset": 0, "controls": controls(), "seed": 42},
        {"slot_offset": 2, "controls": controls(algorithm="XS1"), "seed": 42},
        {"slot_offset": 2, "controls": controls(algorithm="XS1"), "seed": 77},
    ]


@pytest.mark.parametrize(
    ("valid_slots", "capture_id"),
    [
        (2, "dddddddd-dddd-4ddd-8ddd-dddddddddddd"),
        (7, "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"),
    ],
)
def test_live_stop_finishes_immediately_at_an_already_valid_boundary(
    tmp_path: Path,
    valid_slots: int,
    capture_id: str,
) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(valid_slots):
        state.handle("deck.d2.process_slot", identity(2))

    stopped = state.handle(
        "deck.d2.capture.stop",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )

    assert stopped["state"] == "finished"
    assert stopped["latent_slots"] == valid_slots
    receipt = stopped["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["visual_shape"] == [1, 24, valid_slots, 3, 4]


def test_snapshot_copies_exact_structural_carrier_audio(tmp_path: Path) -> None:
    audio = SyntheticAudio(decoded_frames=22)
    state, _decoder, _rings = configured_state(source_a_audio=audio)
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "55555555-5555-4555-8555-555555555555"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(7):
        state.handle("deck.d2.process_slot", identity(2))

    finished = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    receipt = finished["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["audio_policy"] == "copied_from_carrier_exact"
    assert receipt["audio_descriptor"] == {
        "storage_dtype": "F32",
        "shape": [1, 32, 2, 37],
        "byte_length": len(audio.encoded),
    }
    assert Path(str(receipt["payload_path"])).read_bytes().endswith(audio.encoded)


def test_live_capture_omits_audio_when_duration_does_not_match(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state(source_a_audio=SyntheticAudio(decoded_frames=22))
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "66666666-6666-4666-8666-666666666666"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    state.handle(
        "deck.d2.capture.stop",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(2))

    finished = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    receipt = finished["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["visual_shape"] == [1, 24, 2, 3, 4]
    assert receipt["audio_policy"] == "omitted_timing_mismatch"
    assert receipt["audio_policy_reason"] == "duration_mismatch"
    assert "audio_descriptor" not in receipt


def test_live_capture_copies_audio_only_for_exact_duration_and_mapping(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state(source_a_audio=SyntheticAudio(decoded_frames=22))
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "12121212-1212-4212-8212-121212121212"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(2))
    state.handle(
        "deck.d2.controls.set",
        {"deck_id": "main-d2", "deck_revision": 1, "controls": controls(algorithm="XS1")},
    )
    for _ in range(5):
        state.handle("deck.d2.process_slot", identity(2))

    stopped = state.handle(
        "deck.d2.capture.stop",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    receipt = stopped["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["audio_policy"] == "copied_from_carrier_exact"
    assert "audio_policy_reason" not in receipt
    assert receipt["audio_descriptor"]["shape"] == [1, 32, 2, 37]  # type: ignore[index]


def test_live_routing_change_marks_audio_mapping_mismatch(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state(source_a_audio=SyntheticAudio(decoded_frames=22))
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(2))
    state.handle(
        "deck.d2.controls.set",
        {"deck_id": "main-d2", "deck_revision": 1, "controls": controls(routing="B")},
    )
    state.handle("deck.d2.process_slot", identity(2))
    state.handle(
        "deck.d2.capture.stop",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    for _ in range(4):
        state.handle("deck.d2.process_slot", identity(2))

    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    receipt = status["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["audio_policy"] == "omitted_timing_mismatch"
    assert receipt["audio_policy_reason"] == "temporal_mapping_mismatch"
    assert "audio_descriptor" not in receipt


def test_decoder_failure_aborts_and_removes_capture_owned_partials(tmp_path: Path) -> None:
    state, decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "77777777-7777-4777-8777-777777777777"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    decoder.fail_decode = True

    with pytest.raises(WorkerCommandError) as caught:
        state.handle("deck.d2.process_slot", identity(2))

    assert caught.value.code == "decode.failed"
    assert caught.value.fatal is True
    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert status["state"] == "aborted"
    assert status["reason"] == "process_or_decode_error"
    assert not list(tmp_path.glob(f"{capture_id}*"))


def test_live_spool_auto_finishes_at_last_bounded_codec_boundary(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "88888888-8888-4888-8888-888888888888"
    start = capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture")
    start["max_latent_slots"] = 2
    state.handle("deck.d2.capture.start", start)
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    state.handle("deck.d2.process_slot", identity(2))
    state.handle("deck.d2.process_slot", identity(2))
    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert status["state"] == "finished"
    assert status["latent_slots"] == 2
    assert status["receipt"]["visual_shape"] == [1, 24, 2, 3, 4]  # type: ignore[index]
    assert list(tmp_path.glob(f"{capture_id}.safetensors.partial"))

    assert state.handle("deck.d2.process_slot", identity(2))["kind"] == "decoded_slot"


def test_live_control_history_is_bounded_before_mutating_runtime_state(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "ffffffff-ffff-4fff-8fff-ffffffffffff"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for seed in range(1, 32):
        state.handle(
            "deck.d2.seed.set",
            {"deck_id": "main-d2", "deck_revision": 1, "seed": seed},
        )

    with pytest.raises(WorkerCommandError) as caught:
        state.handle(
            "deck.d2.seed.set",
            {"deck_id": "main-d2", "deck_revision": 1, "seed": 99},
        )

    assert caught.value.code == "capture.event_limit"
    assert state.handle("deck.d2.status", {})["seed"] == 31
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(2))
    stopped = state.handle(
        "deck.d2.capture.stop",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    receipt = stopped["receipt"]
    assert isinstance(receipt, dict)
    assert len(receipt["control_events"]) == 32
    assert len(__import__("json").dumps(receipt).encode()) <= 32_768


def test_causal_reset_aborts_an_active_capture_and_close_removes_finished_output(
    tmp_path: Path,
) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "99999999-9999-4999-8999-999999999999"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    state.handle("deck.d2.process_slot", identity(2))
    state.handle("deck.d2.restart", {"deck_id": "main-d2", "deck_revision": 1})
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 3},
    )
    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert status["state"] == "aborted"
    assert status["reason"] == "causal_reset"
    assert not list(tmp_path.glob(f"{capture_id}*"))

    second_id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=second_id, mode="snapshot"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 4},
    )
    for _ in range(7):
        state.handle("deck.d2.process_slot", identity(4))
    finished = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": second_id},
    )
    assert finished["state"] == "finished"
    assert list(tmp_path.glob(f"{second_id}.safetensors.partial"))

    state.close()
    assert not list(tmp_path.glob(f"{second_id}*"))


def test_worker_loop_requires_causal_reset_before_more_decode() -> None:
    state, decoder, rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    for _ in range(7):
        assert state.handle("deck.d2.process_slot", identity())["kind"] == "decoded_slot"
    barrier = state.handle("deck.d2.process_slot", identity())
    assert barrier["kind"] == "reset_barrier"
    assert barrier["reasons"] == ["slot_a.loop"]
    assert decoder.reset_calls == 0

    reset = state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    assert reset["causal_state_cleared"] is True
    assert decoder.reset_calls == 1
    assert rings["active"].generation == 2
    after = state.handle("deck.d2.process_slot", identity(2))
    assert (after["playhead_a"], after["playhead_b"]) == (0, 7)


def test_live_capture_remains_active_at_automatic_loop_barrier(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "13131313-1313-4313-8313-131313131313"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(7):
        assert state.handle("deck.d2.process_slot", identity(2))["kind"] == "decoded_slot"

    barrier = state.handle("deck.d2.process_slot", identity(2))
    assert barrier["kind"] == "reset_barrier"
    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert status["state"] == "capturing"
    assert status["latent_slots"] == 7


def test_live_capture_resumes_after_automatic_loop_reset(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "14141414-1414-4414-8414-141414141414"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(7):
        state.handle("deck.d2.process_slot", identity(2))
    assert state.handle("deck.d2.process_slot", identity(2))["kind"] == "reset_barrier"

    reset = state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 3},
    )
    assert reset["stream_generation"] == 3
    assert state.handle("deck.d2.process_slot", identity(3))["kind"] == "decoded_slot"
    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert status["state"] == "capturing"
    assert status["stream_generation"] == 3
    assert status["latent_slots"] == 8


def test_live_capture_crosses_repeated_loop_resets_until_user_stop(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "15151515-1515-4515-8515-151515151515"
    capture_identity = {
        "deck_id": "main-d2",
        "deck_revision": 1,
        "capture_id": capture_id,
    }
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )

    for _ in range(7):
        state.handle("deck.d2.process_slot", identity(2))
    assert state.handle("deck.d2.process_slot", identity(2))["reasons"] == ["slot_a.loop"]
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 3},
    )
    for _ in range(5):
        state.handle("deck.d2.process_slot", identity(3))
    assert state.handle("deck.d2.process_slot", identity(3))["reasons"] == ["slot_b.loop"]
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 4},
    )
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(4))
    assert state.handle("deck.d2.process_slot", identity(4))["reasons"] == ["slot_a.loop"]
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 5},
    )
    for _ in range(3):
        state.handle("deck.d2.process_slot", identity(5))

    stopped = state.handle("deck.d2.capture.stop", capture_identity)
    assert stopped["state"] == "finished"
    assert stopped["latent_slots"] == 17
    assert stopped["receipt"]["visual_shape"] == [1, 24, 17, 3, 4]  # type: ignore[index]
    assert Path(str(stopped["receipt"]["payload_path"])).is_file()  # type: ignore[index]


def test_live_capture_receipt_excludes_event_at_final_slot_boundary(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "16161616-1616-4616-8616-161616161616"
    capture_identity = {
        "deck_id": "main-d2",
        "deck_revision": 1,
        "capture_id": capture_id,
    }
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    for _ in range(2):
        state.handle("deck.d2.process_slot", identity(2))
    state.handle(
        "deck.d2.seed.set",
        {"deck_id": "main-d2", "deck_revision": 1, "seed": 99},
    )

    stopped = state.handle("deck.d2.capture.stop", capture_identity)
    receipt = stopped["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["visual_shape"] == [1, 24, 2, 3, 4]
    assert [event["slot_offset"] for event in receipt["control_events"]] == [0]  # type: ignore[index]


def test_capture_finishes_only_after_its_rgb_slot_is_published(tmp_path: Path) -> None:
    state, _decoder, rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "17171717-1717-4717-8717-171717171717"
    capture_identity = {
        "deck_id": "main-d2",
        "deck_revision": 1,
        "capture_id": capture_id,
    }
    start = capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture")
    start["max_latent_slots"] = 2
    state.handle("deck.d2.capture.start", start)
    state.handle(
        "deck.d2.reset",
        {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
    )
    states_during_publish: list[str] = []
    rings["active"].publish_probe = lambda: states_during_publish.append(
        str(state.handle("deck.d2.capture.status", capture_identity)["state"])
    )

    state.handle("deck.d2.process_slot", identity(2))
    state.handle("deck.d2.process_slot", identity(2))

    assert states_during_publish == ["capturing", "capturing"]
    assert state.handle("deck.d2.capture.status", capture_identity)["state"] == "finished"


def test_non_looping_eos_reports_authoritative_transport_until_clean_pause() -> None:
    state, _decoder, _rings = configured_state()
    payload = load_payload()
    payload["transport"] = {
        "playing_a": True,
        "playing_b": True,
        "loop_a": False,
        "loop_b": False,
    }
    state.handle("deck.d2.load", payload)
    state.handle("ring.bind", bind_payload())

    for _ in range(7):
        assert state.handle("deck.d2.process_slot", identity())["kind"] == "decoded_slot"
    after_a_eos = state.handle("deck.d2.process_slot", identity())
    assert after_a_eos["kind"] == "decoded_slot"
    assert after_a_eos["transport"] == {
        "playing_a": False,
        "playing_b": True,
        "loop_a": False,
        "loop_b": False,
    }

    for _ in range(4):
        assert state.handle("deck.d2.process_slot", identity())["kind"] == "decoded_slot"
    paused = state.handle("deck.d2.process_slot", identity())
    assert paused == {
        "kind": "paused",
        "deck_id": "main-d2",
        "deck_revision": 1,
        "stream_generation": 1,
        "playhead_a": 6,
        "playhead_b": 11,
        "transport": {
            "playing_a": False,
            "playing_b": False,
            "loop_a": False,
            "loop_b": False,
        },
    }


def test_worker_controls_transport_seed_and_restart_are_typed() -> None:
    state, _decoder, _rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    control_ack = state.handle(
        "deck.d2.controls.set",
        {"deck_id": "main-d2", "deck_revision": 1, "controls": controls(algorithm="XS1")},
    )
    assert control_ack["controls"]["algorithm"] == "XS1"  # type: ignore[index]
    seed_ack = state.handle(
        "deck.d2.seed.set",
        {"deck_id": "main-d2", "deck_revision": 1, "seed": 77},
    )
    assert seed_ack == {
        "deck_id": "main-d2",
        "deck_revision": 1,
        "seed": 77,
        "requires_causal_reset": False,
    }
    state.handle(
        "deck.d2.transport.set",
        {
            "deck_id": "main-d2",
            "deck_revision": 1,
            "transport": {
                "playing_a": False,
                "playing_b": False,
                "loop_a": True,
                "loop_b": True,
            },
        },
    )
    paused = state.handle("deck.d2.process_slot", identity())
    assert paused["kind"] == "paused"
    assert paused["transport"] == {
        "playing_a": False,
        "playing_b": False,
        "loop_a": True,
        "loop_b": True,
    }
    assert state.handle("deck.d2.restart", {"deck_id": "main-d2", "deck_revision": 1})[
        "reasons"
    ] == ["transport.restart"]


def test_failed_ring_generation_change_preserves_engine_reset_barrier() -> None:
    state, decoder, rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    state.handle("deck.d2.restart", {"deck_id": "main-d2", "deck_revision": 1})
    rings["active"].fail_reset = True
    with pytest.raises(WorkerCommandError) as caught:
        state.handle(
            "deck.d2.reset",
            {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
        )
    assert caught.value.code == "deck.reset_failed"
    assert caught.value.fatal is True
    assert state.handle("deck.d2.process_slot", identity())["kind"] == "reset_barrier"
    assert decoder.reset_calls == 1


def test_capture_start_reset_failure_cleans_the_zero_length_spool(tmp_path: Path) -> None:
    state, _decoder, rings = configured_state()
    state.handle("deck.d2.load", load_payload())
    state.handle("ring.bind", bind_payload())
    capture_id = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
    state.handle(
        "deck.d2.capture.start",
        capture_start_payload(tmp_path, capture_id=capture_id, mode="live_capture"),
    )
    rings["active"].fail_reset = True

    with pytest.raises(WorkerCommandError) as caught:
        state.handle(
            "deck.d2.reset",
            {"deck_id": "main-d2", "deck_revision": 1, "new_stream_generation": 2},
        )

    assert caught.value.code == "deck.reset_failed"
    status = state.handle(
        "deck.d2.capture.status",
        {"deck_id": "main-d2", "deck_revision": 1, "capture_id": capture_id},
    )
    assert status["state"] == "aborted"
    assert status["reason"] == "start_reset_failed"
    assert not list(tmp_path.glob(f"{capture_id}*"))


def _command(sequence: int, name: str, payload: dict[str, object]) -> dict[str, object]:
    return {
        "protocol": "latentdeck.worker",
        "protocol_version": 1,
        "session_id": SESSION_ID,
        "sequence": sequence,
        "message_id": str(uuid.uuid4()),
        "sender_uptime_ns": sequence,
        "message": {"kind": "command", "body": {"name": name, "payload": payload}},
    }


def test_d2_process_entrypoint_uses_the_authenticated_worker_transport() -> None:
    inbound = io.BytesIO()
    write_frame(inbound, _command(1, "session.configure", session_payload()))
    write_frame(inbound, _command(2, "codec.load", codec_payload()))
    write_frame(inbound, _command(3, "deck.d2.load", load_payload()))
    write_frame(inbound, _command(4, "ring.bind", bind_payload()))
    write_frame(inbound, _command(5, "deck.d2.process_slot", identity()))
    write_frame(inbound, _command(6, "worker.shutdown", {"reason": "user_request"}))
    inbound.seek(0)
    outbound = io.BytesIO()

    class Connector:
        def connect(self, pipe_name: str) -> StreamConnection:
            assert pipe_name == PIPE_NAME
            return StreamConnection(inbound, outbound)

    fresh, _decoder, _rings = configured_state(initialize=False)
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, b"a" * 32)))
    assert run_d2_worker(bootstrap, connector=Connector(), state_factory=lambda: fresh) == 0
    outbound.seek(0)
    frames = []
    while outbound.tell() < len(outbound.getvalue()):
        frames.append(read_frame(outbound))
    replies = [
        frame["message"]
        for frame in frames
        if frame["message"]["kind"] in {"ack", "error"}  # type: ignore[index]
    ]
    assert all(reply["kind"] == "ack" for reply in replies)  # type: ignore[index]
    names = [reply["body"]["ack"]["name"] for reply in replies]  # type: ignore[index]
    assert names == [
        "session.configure",
        "codec.load",
        "deck.d2.load",
        "ring.bind",
        "deck.d2.process_slot",
        "worker.shutdown",
    ]
    process_ack = replies[4]["body"]["ack"]["payload"]  # type: ignore[index]
    assert process_ack["kind"] == "decoded_slot"
    assert process_ack["decoded_frame_count"] == 1
