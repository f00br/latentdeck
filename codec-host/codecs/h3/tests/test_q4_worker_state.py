# ruff: noqa: E402

from __future__ import annotations

import io
import json
import math
import uuid
from array import array
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

import pytest

torch = pytest.importorskip("torch", reason="Q4 worker conformance requires the runtime extra")

from latentdeck_codec_host.protocol import Bootstrap, encode_bootstrap, read_frame, write_frame

from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.decoder import RuntimeDevice, RuntimeInspection
from latentdeck_codec_h3.q4_worker import run_q4_worker
from latentdeck_codec_h3.q4_worker_state import H3Q4WorkerState
from latentdeck_codec_h3.worker import StreamConnection
from latentdeck_codec_h3.worker_state import WorkerCommandError

SESSION_ID = "9ca8c228-04c7-4b59-909f-6fbef591a43e"
PIPE_NAME = rf"\\.\pipe\LatentDeck.Worker.{SESSION_ID}"


def controls(**changes: object) -> dict[str, object]:
    result: dict[str, object] = {
        "algorithm": "LINEAR",
        "interaction": 0.5,
        "mode": "HYBRIDIZE",
        "preserve": 0.55,
        "influence_mode": "MANUAL",
        "donor_weight_b": 1.0,
        "donor_weight_c": 1.0,
        "donor_weight_d": 1.0,
        "triangle_x": 0.5,
        "triangle_y": 1.0 / 3.0,
        "xs5_routing": "TOPK",
        "temperature": 0.12,
        "top_k": 4,
        "sinkhorn_iterations": 5,
        "chaos": 0.0,
    }
    result.update(changes)
    return result


def roles(**changes: str) -> dict[str, str]:
    result = {"carrier": "A", "donor_b": "B", "donor_c": "C", "donor_d": "D"}
    result.update(changes)
    return result


def transport(**changes: bool) -> dict[str, bool]:
    result = {
        "playing_a": True,
        "playing_b": True,
        "playing_c": True,
        "playing_d": True,
        "loop_a": True,
        "loop_b": True,
        "loop_c": True,
        "loop_d": True,
    }
    result.update(changes)
    return result


def source(slot_count: int, identity: int, archive_byte: str) -> H3VideoSource:
    values = array(
        "f",
        (
            math.sin(index * (0.021 + identity * 0.001)) + identity * 0.01
            for index in range(24 * slot_count * 2 * 3)
        ),
    )
    return H3VideoSource(
        cartridge_id=f"00000000-0000-4000-8000-{identity:012d}",
        archive_sha256=archive_byte * 64,
        storage_dtype="F32",
        shape=(1, 24, slot_count, 2, 3),
        video_bytes=values.tobytes(),
        width=48,
        height=32,
        frame_count=5 + 17 * ((slot_count - 2) // 5),
        frame_rate_numerator=24,
        frame_rate_denominator=1,
    )


class FakeDecoder:
    def __init__(self) -> None:
        self.slots_seen = 0
        self.reset_calls = 0
        self.closed = False
        self.fail_decode = False

    def decode_slot(self, slot: Any) -> tuple[bytes, ...]:
        assert slot.dtype == torch.float16
        if self.fail_decode:
            raise RuntimeError("private machine path must not escape")
        count = 1 if self.slots_seen == 0 or self.slots_seen % 5 == 0 else 4
        self.slots_seen += 1
        return tuple(bytes(48 * 32 * 4) for _ in range(count))

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
        self.fail_publish = False

    def can_publish(self, frame_count: int) -> bool:
        return self.occupancy + frame_count <= self.capacity

    def publish_frames(self, frames: Sequence[bytes], *, stream_generation: int) -> tuple[int, int]:
        assert stream_generation == self.generation
        if self.fail_publish:
            raise RuntimeError(r"W:\private\q4-ring.mapping")
        first = self.write_sequence + 1
        self.write_sequence += len(frames)
        self.occupancy += len(frames)
        return first, self.write_sequence + 1

    def set_generation(self, stream_generation: int) -> None:
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
        "assets": [
            {
                "asset_id": "taeh3",
                "path": "weight",
                "sha256": "c" * 64,
                "byte_length": 1,
            }
        ],
    }


def test_codec_upgrade_keeps_pack_and_adapter_versions_independent() -> None:
    state, _decoder, _rings = configured_state(initialize=False)
    state.handle("session.configure", session_payload())
    payload = codec_payload()
    payload["pack_version"] = "0.1.1"

    loaded = state.handle("codec.load", payload)

    assert loaded["pack_version"] == "0.1.1"
    assert loaded["adapter_version"] == "0.1.0"


def load_payload() -> dict[str, object]:
    result: dict[str, object] = {
        "deck_id": "main-q4",
        "operator_id": "org.latentdeck.builtin.ld_q4",
        "operator_version": "0.1.0",
        "roles": roles(carrier="C", donor_b="A", donor_c="D", donor_d="B"),
        "controls": controls(),
        "transport": transport(),
        "seed": 42,
        "stream_generation": 1,
    }
    for identity, slot in enumerate("ABCD", start=1):
        result[f"source_{slot.lower()}"] = {
            "cartridge_path": f"{slot}.lc",
            "cartridge_id": f"00000000-0000-4000-8000-{identity:012d}",
            "expected_archive_sha256": slot.lower() * 64,
        }
    return result


def bind_payload() -> dict[str, object]:
    return {
        "layout_version": 1,
        "mapping_handle": 1,
        "mapping_bytes": 1_052_672,
        "frames_ready_event_handle": 2,
        "ring_id": "bbfb89cc-0739-423f-9474-d03e01bc34aa",
    }


def identity(generation: int = 1) -> dict[str, object]:
    return {"deck_id": "main-q4", "deck_revision": 1, "stream_generation": generation}


def configured_state(
    *, initialize: bool = True, slot_count: int = 2
) -> tuple[H3Q4WorkerState, FakeDecoder, dict[str, FakeRing]]:
    decoder = FakeDecoder()
    sources = {
        f"{slot}.lc": source(slot_count, identity, slot.lower())
        for identity, slot in enumerate("ABCD", start=1)
    }
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
    state = H3Q4WorkerState(
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


def initialize_deck(state: H3Q4WorkerState) -> dict[str, object]:
    loaded = state.handle("deck.q4.load", load_payload())
    state.handle("ring.bind", bind_payload())
    return loaded


def test_worker_loads_four_sources_and_processes_explicit_roles_before_ring() -> None:
    state, _decoder, _rings = configured_state()
    loaded = initialize_deck(state)
    assert loaded["operator_id"] == "org.latentdeck.builtin.ld_q4"
    assert loaded["roles"] == roles(carrier="C", donor_b="A", donor_c="D", donor_d="B")
    assert "cartridge_path" not in str(loaded)

    processed = state.handle("deck.q4.process_slot", identity())

    assert processed["kind"] == "decoded_slot"
    assert [processed[f"playhead_{slot}"] for slot in "abcd"] == [0, 0, 0, 0]
    assert processed["roles"] == loaded["roles"]
    assert processed["transport"] == transport()
    assert processed["decoded_frame_count"] == 1
    provenance = json.loads(str(processed["provenance_json"]))
    assert provenance["roles"]["carrier"]["slot"] == "C"
    assert [donor["slot"] for donor in provenance["roles"]["donors"]] == ["A", "D", "B"]
    assert provenance["routing"]["reference"] == "UNCHANGED_CARRIER"
    assert provenance["routing"]["accumulation_order"] == ["B", "C", "D"]
    assert not any(isinstance(value, bytes) for value in processed.values())


def test_roles_controls_seed_and_four_source_causal_reset_are_typed() -> None:
    state, decoder, rings = configured_state()
    initialize_deck(state)
    update_identity = {"deck_id": "main-q4", "deck_revision": 1}

    role_update = state.handle(
        "deck.q4.roles.set",
        {
            **update_identity,
            "roles": roles(carrier="D", donor_b="C", donor_c="B", donor_d="A"),
        },
    )
    control_update = state.handle(
        "deck.q4.controls.set",
        {**update_identity, "controls": controls(algorithm="XS5", top_k=3)},
    )
    seed_update = state.handle("deck.q4.seed.set", {**update_identity, "seed": 8128})
    assert role_update["requires_causal_reset"] is False
    assert control_update["requires_causal_reset"] is False
    assert seed_update == {**update_identity, "seed": 8128, "requires_causal_reset": False}

    assert state.handle("deck.q4.process_slot", identity())["kind"] == "decoded_slot"
    assert state.handle("deck.q4.process_slot", identity())["kind"] == "decoded_slot"
    barrier = state.handle("deck.q4.process_slot", identity())
    assert barrier["kind"] == "reset_barrier"
    assert barrier["reasons"] == [
        "slot_a.loop",
        "slot_b.loop",
        "slot_c.loop",
        "slot_d.loop",
    ]
    assert decoder.reset_calls == 0

    reset = state.handle(
        "deck.q4.reset",
        {**update_identity, "new_stream_generation": 2},
    )
    assert reset["causal_state_cleared"] is True
    assert decoder.reset_calls == 1
    assert rings["active"].generation == 2
    assert [reset[f"playhead_{slot}"] for slot in "abcd"] == [0, 0, 0, 0]


def test_snapshot_capture_freezes_q4_state_and_records_four_parents(tmp_path: Path) -> None:
    state, _decoder, _rings = configured_state()
    initialize_deck(state)
    capture_id = "33333333-3333-4333-8333-333333333333"
    capture_identity = {"deck_id": "main-q4", "deck_revision": 1, "capture_id": capture_id}

    started = state.handle(
        "deck.q4.capture.start",
        {
            **capture_identity,
            "mode": "snapshot",
            "temporary_root": str(tmp_path),
            "max_latent_slots": 12,
            "max_visual_bytes": 16 * 1024 * 1024,
        },
    )
    assert started["state"] == "awaiting_reset"
    with pytest.raises(WorkerCommandError, match="Snapshot roles") as frozen:
        state.handle(
            "deck.q4.roles.set",
            {
                "deck_id": "main-q4",
                "deck_revision": 1,
                "roles": roles(carrier="D", donor_b="A", donor_c="B", donor_d="C"),
            },
        )
    assert frozen.value.code == "capture.snapshot_frozen"

    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 2},
    )
    assert state.handle("deck.q4.process_slot", identity(2))["kind"] == "decoded_slot"
    assert state.handle("deck.q4.process_slot", identity(2))["kind"] == "decoded_slot"
    finished = state.handle("deck.q4.capture.status", capture_identity)
    assert finished["state"] == "finished"
    receipt = finished["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["frozen_roles"] == roles(carrier="C", donor_b="A", donor_c="D", donor_d="B")
    assert [parent["slot"] for parent in receipt["parents"]] == ["A", "B", "C", "D"]
    assert Path(str(receipt["payload_path"])).is_file()


def test_snapshot_finalizes_only_after_each_ring_publish_succeeds(tmp_path: Path) -> None:
    state, _decoder, rings = configured_state()
    initialize_deck(state)
    capture_id = "44444444-4444-4444-8444-444444444444"
    capture_identity = {"deck_id": "main-q4", "deck_revision": 1, "capture_id": capture_id}
    state.handle(
        "deck.q4.capture.start",
        {
            **capture_identity,
            "mode": "snapshot",
            "temporary_root": str(tmp_path),
            "max_latent_slots": 12,
            "max_visual_bytes": 16 * 1024 * 1024,
        },
    )
    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 2},
    )

    capture = state._capture
    assert capture is not None
    ring = rings["active"]
    events: list[str] = []
    publish_frames = ring.publish_frames
    after_decode = capture.after_decode

    def record_publish(frames: Sequence[bytes], *, stream_generation: int) -> tuple[int, int]:
        events.append("ring.publish")
        return publish_frames(frames, stream_generation=stream_generation)

    def record_after_decode(step: Any) -> None:
        events.append("capture.after_decode")
        after_decode(step)

    ring.publish_frames = record_publish  # type: ignore[method-assign]
    capture.after_decode = record_after_decode  # type: ignore[method-assign]

    state.handle("deck.q4.process_slot", identity(2))
    assert events == ["ring.publish", "capture.after_decode"]
    events.clear()
    state.handle("deck.q4.process_slot", identity(2))
    assert events == ["ring.publish", "capture.after_decode"]
    assert state.handle("deck.q4.capture.status", capture_identity)["state"] == "finished"


def test_live_capture_crosses_loop_resets_until_the_user_stops(tmp_path: Path) -> None:
    state, decoder, _rings = configured_state()
    initialize_deck(state)
    capture_id = "66666666-6666-4666-8666-666666666666"
    capture_identity = {"deck_id": "main-q4", "deck_revision": 1, "capture_id": capture_id}
    started = state.handle(
        "deck.q4.capture.start",
        {
            **capture_identity,
            "mode": "live_capture",
            "temporary_root": str(tmp_path),
            "max_latent_slots": 12,
            "max_visual_bytes": 16 * 1024 * 1024,
        },
    )
    assert started["state"] == "awaiting_reset"
    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 2},
    )

    assert state.handle("deck.q4.process_slot", identity(2))["kind"] == "decoded_slot"
    assert state.handle("deck.q4.process_slot", identity(2))["kind"] == "decoded_slot"
    first_loop = state.handle("deck.q4.process_slot", identity(2))
    assert first_loop["kind"] == "reset_barrier"
    assert state.handle("deck.q4.capture.status", capture_identity)["state"] == "capturing"

    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 3},
    )
    assert decoder.reset_calls == 2
    assert state.handle("deck.q4.process_slot", identity(3))["kind"] == "decoded_slot"
    assert state.handle("deck.q4.process_slot", identity(3))["kind"] == "decoded_slot"
    second_loop = state.handle("deck.q4.process_slot", identity(3))
    assert second_loop["kind"] == "reset_barrier"
    assert state.handle("deck.q4.capture.status", capture_identity)["state"] == "capturing"

    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 4},
    )
    assert state.handle("deck.q4.process_slot", identity(4))["kind"] == "decoded_slot"
    assert state.handle("deck.q4.process_slot", identity(4))["kind"] == "decoded_slot"
    third_loop = state.handle("deck.q4.process_slot", identity(4))
    assert third_loop["kind"] == "reset_barrier"
    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 5},
    )
    assert state.handle("deck.q4.process_slot", identity(5))["kind"] == "decoded_slot"
    assert state.handle("deck.q4.capture.status", capture_identity)["latent_slots"] == 7

    stopped = state.handle("deck.q4.capture.stop", capture_identity)
    assert stopped["state"] == "finished"
    receipt = stopped["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["visual_shape"] == [1, 24, 7, 2, 3]
    assert receipt["audio_policy"] == "source_absent"


def test_ring_failure_aborts_capture_without_payload_or_path_leak(tmp_path: Path) -> None:
    state, _decoder, rings = configured_state()
    initialize_deck(state)
    capture_id = "55555555-5555-4555-8555-555555555555"
    capture_identity = {"deck_id": "main-q4", "deck_revision": 1, "capture_id": capture_id}
    state.handle(
        "deck.q4.capture.start",
        {
            **capture_identity,
            "mode": "snapshot",
            "temporary_root": str(tmp_path),
            "max_latent_slots": 12,
            "max_visual_bytes": 16 * 1024 * 1024,
        },
    )
    state.handle(
        "deck.q4.reset",
        {"deck_id": "main-q4", "deck_revision": 1, "new_stream_generation": 2},
    )
    state.handle("deck.q4.process_slot", identity(2))
    rings["active"].fail_publish = True

    with pytest.raises(WorkerCommandError) as failed:
        state.handle("deck.q4.process_slot", identity(2))

    assert failed.value.code == "decode.failed"
    assert failed.value.fatal
    assert "W:\\private" not in failed.value.message
    aborted = state.handle("deck.q4.capture.status", capture_identity)
    assert aborted["state"] == "aborted"
    assert aborted["reason"] == "process_or_publish_error"
    assert list(tmp_path.iterdir()) == []


def test_backpressure_invalid_roles_and_decode_failures_are_path_free() -> None:
    state, decoder, rings = configured_state()
    initialize_deck(state)
    rings["active"].capacity = 0
    with pytest.raises(WorkerCommandError) as blocked:
        state.handle("deck.q4.process_slot", identity())
    assert blocked.value.code == "ring.backpressure"
    assert blocked.value.retryable

    with pytest.raises(WorkerCommandError) as invalid:
        state.handle(
            "deck.q4.roles.set",
            {
                "deck_id": "main-q4",
                "deck_revision": 1,
                "roles": {"carrier": "A", "donor_b": "A", "donor_c": "C", "donor_d": "D"},
            },
        )
    assert invalid.value.code == "deck.roles_invalid"

    rings["active"].capacity = 128
    decoder.fail_decode = True
    with pytest.raises(WorkerCommandError) as failed:
        state.handle("deck.q4.process_slot", identity())
    assert failed.value.code == "decode.failed"
    assert failed.value.fatal
    assert "private machine path" not in failed.value.message


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


def test_q4_entrypoint_uses_authenticated_worker_transport() -> None:
    inbound = io.BytesIO()
    write_frame(inbound, _command(1, "session.configure", session_payload()))
    write_frame(inbound, _command(2, "codec.load", codec_payload()))
    write_frame(inbound, _command(3, "deck.q4.load", load_payload()))
    write_frame(inbound, _command(4, "ring.bind", bind_payload()))
    write_frame(inbound, _command(5, "deck.q4.process_slot", identity()))
    write_frame(inbound, _command(6, "worker.shutdown", {"reason": "user_request"}))
    inbound.seek(0)
    outbound = io.BytesIO()

    class Connector:
        def connect(self, pipe_name: str) -> StreamConnection:
            assert pipe_name == PIPE_NAME
            return StreamConnection(inbound, outbound)

    fresh, _decoder, _rings = configured_state(initialize=False)
    bootstrap = io.BytesIO(encode_bootstrap(Bootstrap(SESSION_ID, PIPE_NAME, b"a" * 32)))
    assert run_q4_worker(bootstrap, connector=Connector(), state_factory=lambda: fresh) == 0
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
        "deck.q4.load",
        "ring.bind",
        "deck.q4.process_slot",
        "worker.shutdown",
    ]
