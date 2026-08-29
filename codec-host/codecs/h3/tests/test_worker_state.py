from __future__ import annotations

from dataclasses import dataclass
from typing import Any

import pytest

from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.decoder import DecodedCycle, RuntimeDevice, RuntimeInspection
from latentdeck_codec_h3.worker_state import H3WorkerState, WorkerCommandError

CARTRIDGE_ID = "4da30a3c-38dd-43a1-98af-49d55c01eff6"


class FakeDecoder:
    def __init__(self) -> None:
        self.source: H3VideoSource | None = None
        self.next_cycle = 0
        self.reset_calls = 0
        self.closed = False

    def bind_source(self, source: H3VideoSource) -> None:
        self.source = source
        self.reset()

    def reset(self) -> None:
        self.next_cycle = 0
        self.reset_calls += 1

    def decode_cycle(self, cycle_index: int) -> DecodedCycle:
        assert self.source is not None
        assert cycle_index == self.next_cycle
        timing = self.source.cycle(cycle_index)
        self.next_cycle += 1
        return DecodedCycle(
            timing, tuple(bytes([index]) for index in range(timing.decoded_frame_count))
        )

    def close(self) -> None:
        self.closed = True


@dataclass
class FakeRing:
    capacity: int = 24
    write_sequence: int = 0
    read_sequence: int = 0
    generation: int = 0
    presentation_skipped_total: int = 0
    closed: bool = False
    fail_publish: bool = False

    @property
    def occupancy(self) -> int:
        return self.write_sequence - self.read_sequence

    def can_publish(self, frame_count: int) -> bool:
        return self.capacity - self.occupancy >= frame_count

    def publish_cycle(
        self,
        frames: Any,
        *,
        stream_generation: int,
        cycle_index: int,
        decoded_start_frame: int,
    ) -> tuple[int, int]:
        del cycle_index, decoded_start_frame
        assert stream_generation == self.generation
        if self.fail_publish:
            raise RuntimeError("synthetic publish failure")
        first = self.write_sequence + 1
        self.write_sequence += len(frames)
        return first, self.write_sequence + 1

    def set_generation(self, stream_generation: int) -> None:
        self.generation = stream_generation
        self.write_sequence = 0
        self.read_sequence = 0

    def close(self) -> None:
        self.closed = True


def source() -> H3VideoSource:
    return H3VideoSource(
        cartridge_id=CARTRIDGE_ID,
        archive_sha256="1" * 64,
        storage_dtype="F16",
        shape=(1, 24, 32, 1, 1),
        video_bytes=bytes(1 * 24 * 32 * 2),
        width=16,
        height=16,
        frame_count=107,
        frame_rate_numerator=24,
        frame_rate_denominator=1,
    )


def configured_state(ring: FakeRing | None = None) -> tuple[H3WorkerState, FakeDecoder, FakeRing]:
    decoder = FakeDecoder()
    bound_ring = ring or FakeRing()

    def bind_ring(_: Any, __: Any, generation: int) -> FakeRing:
        bound_ring.generation = generation
        return bound_ring

    state = H3WorkerState(
        decoder_factory=lambda *_: decoder,
        source_loader=lambda *_: source(),
        ring_factory=bind_ring,
        runtime_inspector=lambda: RuntimeInspection(
            "2.13.0+cu130", True, "13.0", (RuntimeDevice(0, "Synthetic CUDA", 12),)
        ),
    )
    state.handle(
        "session.configure",
        {
            "selected_protocol_version": 1,
            "app_version": "0.1.0",
            "heartbeat_interval_ms": 1000,
            "heartbeat_hard_timeout_ms": 10000,
            "max_frame_bytes": 262144,
            "max_inflight_decode_batches": 1,
        },
    )
    state.handle(
        "codec.load",
        {
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
                {"asset_id": "taeh3", "path": "weight", "sha256": "0" * 64, "byte_length": 1}
            ],
        },
    )
    loaded = state.handle(
        "slot.load",
        {
            "slot_id": "player",
            "cartridge_path": "cartridge.lc",
            "cartridge_id": CARTRIDGE_ID,
            "expected_archive_sha256": "1" * 64,
            "stream_generation": 1,
        },
    )
    assert loaded["timing"]["decoded_frame_count"] == 107  # type: ignore[index]
    state.handle(
        "ring.bind",
        {
            "layout_version": 1,
            "mapping_handle": 1,
            "mapping_bytes": 4096,
            "frames_ready_event_handle": 2,
            "ring_id": "bbfb89cc-0739-423f-9474-d03e01bc34aa",
        },
    )
    return state, decoder, bound_ring


def decode_payload(cycle: int, generation: int = 1) -> dict[str, object]:
    return {
        "slot_id": "player",
        "slot_revision": 1,
        "stream_generation": generation,
        "cycle_index": cycle,
    }


def test_prime_and_steady_cycles_are_ordered_and_bounded() -> None:
    state, decoder, ring = configured_state()

    prime = state.handle("slot.decode_cycle", decode_payload(0))
    steady = state.handle("slot.decode_cycle", decode_payload(1))

    assert prime["decoded_frame_count"] == 5
    assert prime["ring_first_sequence"] == 1
    assert steady["decoded_frame_count"] == 17
    assert steady["ring_last_sequence_exclusive"] == 23
    assert decoder.next_cycle == 2
    assert ring.occupancy == 22


def test_backpressure_is_reported_before_decoder_state_changes() -> None:
    ring = FakeRing(capacity=4)
    state, decoder, _ = configured_state(ring)

    with pytest.raises(WorkerCommandError) as captured:
        state.handle("slot.decode_cycle", decode_payload(0))

    assert captured.value.code == "ring.backpressure"
    assert captured.value.retryable is True
    assert decoder.next_cycle == 0
    assert state.metrics()["ring_backpressure_total"] == 1


def test_publish_failure_is_fatal_after_causal_state_advanced() -> None:
    ring = FakeRing(fail_publish=True)
    state, decoder, _ = configured_state(ring)

    with pytest.raises(WorkerCommandError) as captured:
        state.handle("slot.decode_cycle", decode_payload(0))

    assert captured.value.code == "decode.failed"
    assert captured.value.fatal is True
    assert decoder.next_cycle == 1
    assert state.status()["slot_state"] == "faulted"


def test_ring_bind_preserves_native_diagnostics_without_changing_wire_error() -> None:
    class NativeRingError(RuntimeError):
        code = "ring_map_view_failed"
        detail = "MapViewOfFile failed with Windows error 5"

    decoder = FakeDecoder()
    state = H3WorkerState(
        decoder_factory=lambda *_: decoder,
        source_loader=lambda *_: source(),
        ring_factory=lambda *_: (_ for _ in ()).throw(NativeRingError()),
        runtime_inspector=lambda: RuntimeInspection(
            "2.13.0+cu130", True, "13.0", (RuntimeDevice(0, "Synthetic CUDA", 12),)
        ),
    )
    state.handle(
        "session.configure",
        {
            "selected_protocol_version": 1,
            "app_version": "0.1.0",
            "heartbeat_interval_ms": 1000,
            "heartbeat_hard_timeout_ms": 10000,
            "max_frame_bytes": 262144,
            "max_inflight_decode_batches": 1,
        },
    )
    state.handle(
        "codec.load",
        {
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
                {"asset_id": "taeh3", "path": "weight", "sha256": "0" * 64, "byte_length": 1}
            ],
        },
    )
    state.handle(
        "slot.load",
        {
            "slot_id": "player",
            "cartridge_path": "cartridge.lc",
            "cartridge_id": CARTRIDGE_ID,
            "expected_archive_sha256": "1" * 64,
            "stream_generation": 1,
        },
    )

    with pytest.raises(WorkerCommandError) as captured:
        state.handle(
            "ring.bind",
            {
                "layout_version": 1,
                "mapping_handle": 1,
                "mapping_bytes": 4096,
                "frames_ready_event_handle": 2,
                "ring_id": "bbfb89cc-0739-423f-9474-d03e01bc34aa",
            },
        )

    assert captured.value.code == "ring.layout_incompatible"
    assert captured.value.message == "RGB ring binding failed"
    assert captured.value.diagnostic_code == "ring_map_view_failed"
    assert captured.value.diagnostic_detail == "MapViewOfFile failed with Windows error 5"


def test_pause_resume_is_core_scheduling_and_restart_requires_new_generation() -> None:
    state, decoder, ring = configured_state()
    state.handle("slot.decode_cycle", decode_payload(0))

    reset = state.handle(
        "slot.reset",
        {
            "slot_id": "player",
            "slot_revision": 1,
            "new_stream_generation": 2,
            "reason": "restart",
        },
    )

    assert reset["next_cycle_index"] == 0
    assert decoder.reset_calls == 2
    assert ring.occupancy == 0
    restarted = state.handle("slot.decode_cycle", decode_payload(0, generation=2))
    assert restarted["decoded_start_frame"] == 0


def test_stale_cycle_revision_and_generation_never_decode() -> None:
    state, decoder, _ = configured_state()
    bad_revision = decode_payload(0)
    bad_revision["slot_revision"] = 9
    with pytest.raises(WorkerCommandError, match="revision"):
        state.handle("slot.decode_cycle", bad_revision)
    with pytest.raises(WorkerCommandError, match="generation"):
        state.handle("slot.decode_cycle", decode_payload(0, generation=9))
    with pytest.raises(WorkerCommandError, match="out of order"):
        state.handle("slot.decode_cycle", decode_payload(1))
    assert decoder.next_cycle == 0


def test_close_releases_ring_and_decoder_without_autoresume() -> None:
    state, decoder, ring = configured_state()
    state.close()
    assert decoder.closed is True
    assert ring.closed is True
    assert state.status()["worker_state"] == "stopped"
