# ruff: noqa: E402

from __future__ import annotations

import math
import uuid
from array import array
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any, BinaryIO

import pytest

torch = pytest.importorskip("torch", reason="Q4 capture conformance requires the runtime extra")

from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.q4_capture import Q4CaptureError, Q4CaptureSession
from latentdeck_codec_h3.resample_spool import ResampleAudioSource


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
    slot_count: int, identity: int, archive_byte: str, *, audio: Any = None
) -> H3VideoSource:
    values = array(
        "f",
        (
            math.sin(index * 0.031) + 0.1 * math.cos(index * 0.079)
            for index in range(24 * slot_count * 2 * 3)
        ),
    )
    return H3VideoSource(
        cartridge_id=str(uuid.UUID(int=identity)),
        archive_sha256=archive_byte * 64,
        storage_dtype="F32",
        shape=(1, 24, slot_count, 2, 3),
        video_bytes=values.tobytes(),
        width=48,
        height=32,
        frame_count=5 + 17 * ((slot_count - 2) // 5),
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        audio=audio,
    )


def sources(slot_count: int = 2, *, carrier_audio: Any = None) -> dict[str, H3VideoSource]:
    return {
        "A": source(slot_count, 1, "a", audio=carrier_audio),
        "B": source(slot_count, 2, "b"),
        "C": source(slot_count, 3, "c"),
        "D": source(slot_count, 4, "d"),
    }


def roles(**changes: str) -> dict[str, str]:
    result = {"carrier": "A", "donor_b": "B", "donor_c": "C", "donor_d": "D"}
    result.update(changes)
    return result


@dataclass(frozen=True)
class Step:
    stream_generation: int
    stream_sequence: int
    playhead_a: int
    playhead_b: int
    playhead_c: int
    playhead_d: int
    output: Any


def feed(session: Q4CaptureSession, count: int, *, after_each: Callable[[int], None] | None = None):
    for index in range(count):
        step = Step(
            stream_generation=2,
            stream_sequence=index,
            playhead_a=index,
            playhead_b=index,
            playhead_c=index,
            playhead_d=index,
            output=torch.full((1, 24, 1, 2, 3), index + 0.25, dtype=torch.float16),
        )
        session.before_decode(step)
        session.after_decode(step)
        if after_each is not None:
            after_each(index)


def capture(
    temporary_root: Path,
    *,
    mode: str,
    source_map: dict[str, H3VideoSource],
) -> Q4CaptureSession:
    return Q4CaptureSession(
        capture_id=str(uuid.UUID(int=99)),
        mode=mode,
        temporary_root=temporary_root,
        max_latent_slots=12,
        max_visual_bytes=12 * 24 * 2 * 3 * 2,
        source_a=source_map["A"],
        source_b=source_map["B"],
        source_c=source_map["C"],
        source_d=source_map["D"],
        roles=roles(),
        controls={"algorithm": "XS5", "interaction": 0.7, "chaos": 0.0},
        seed=8128,
        current_generation=1,
        minimum_new_generation=2,
    )


def activate(session: Q4CaptureSession) -> None:
    session.activate(
        {
            "stream_generation": 2,
            "playhead_a": 0,
            "playhead_b": 0,
            "playhead_c": 0,
            "playhead_d": 0,
        }
    )


def test_snapshot_freezes_roles_controls_seed_and_records_four_parents(tmp_path: Path) -> None:
    session = capture(tmp_path, mode="snapshot", source_map=sources())
    activate(session)
    feed(session, 2)

    status = session.status()
    assert status["state"] == "finished"
    receipt = status["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["frozen_roles"] == roles()
    assert receipt["frozen_controls"] == {
        "algorithm": "XS5",
        "interaction": 0.7,
        "chaos": 0.0,
    }
    assert receipt["frozen_seed"] == 8128
    assert [parent["slot"] for parent in receipt["parents"]] == ["A", "B", "C", "D"]
    assert receipt["visual_shape"] == [1, 24, 2, 2, 3]
    assert Path(str(receipt["payload_path"])).is_file()


def test_live_role_event_is_bounded_and_carrier_change_omits_audio(tmp_path: Path) -> None:
    source_map = sources(carrier_audio=SyntheticAudio(5))
    session = capture(tmp_path, mode="live_capture", source_map=source_map)
    activate(session)

    def change_carrier(index: int) -> None:
        if index == 0:
            session.record_state(
                roles(carrier="B", donor_b="A"),
                {"algorithm": "XS5", "interaction": 0.9, "chaos": 0.0},
                99,
            )

    feed(session, 2, after_each=change_carrier)
    session.record_state(
        roles(carrier="B", donor_b="A"),
        {"algorithm": "XS5", "interaction": 0.1, "chaos": 0.0},
        100,
    )
    session.request_stop()

    status = session.status()
    assert status["state"] == "finished"
    receipt = status["receipt"]
    assert isinstance(receipt, dict)
    assert receipt["audio_policy"] == "omitted_timing_mismatch"
    assert receipt["audio_policy_reason"] == "temporal_mapping_mismatch"
    events = receipt["control_events"]
    assert isinstance(events, list)
    assert events[1] == {
        "slot_offset": 1,
        "roles": roles(carrier="B", donor_b="A"),
        "controls": {"algorithm": "XS5", "interaction": 0.9, "chaos": 0.0},
        "seed": 99,
    }
    assert len(events) == 2


def test_roles_and_reset_boundary_are_strict_and_cleanup_is_recoverable(tmp_path: Path) -> None:
    source_map = sources()
    with pytest.raises(Q4CaptureError, match="capture.roles_invalid"):
        Q4CaptureSession(
            capture_id=str(uuid.UUID(int=100)),
            mode="snapshot",
            temporary_root=tmp_path,
            max_latent_slots=2,
            max_visual_bytes=2 * 24 * 2 * 3 * 2,
            source_a=source_map["A"],
            source_b=source_map["B"],
            source_c=source_map["C"],
            source_d=source_map["D"],
            roles={"carrier": "A", "donor_b": "A", "donor_c": "C", "donor_d": "D"},
            controls={"algorithm": "LINEAR"},
            seed=0,
            current_generation=1,
            minimum_new_generation=2,
        )

    session = capture(tmp_path, mode="snapshot", source_map=source_map)
    with pytest.raises(Q4CaptureError, match="capture.boundary_invalid"):
        session.activate(
            {
                "stream_generation": 2,
                "playhead_a": 0,
                "playhead_b": 1,
                "playhead_c": 0,
                "playhead_d": 0,
            }
        )
    assert session.status()["state"] == "aborted"
    assert not list(tmp_path.glob("*.partial"))
