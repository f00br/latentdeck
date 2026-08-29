# ruff: noqa: E402

from __future__ import annotations

from typing import Any

import pytest

torch = pytest.importorskip("torch", reason="Q4 conformance requires the pinned runtime extra")

from latentdeck_codec_host.operator_api import OperatorLoadError
from latentdeck_operator_q4.stream import Q4DecodedSlot, Q4ResetBarrier, Q4StreamError

from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.q4_engine import (
    H3Q4SourceError,
    H3Q4StreamEngine,
    H3Q4TensorSource,
)


def source(
    slot_count: int,
    *,
    identity: int,
    archive_byte: str,
    width: int = 4,
    scale: float = 1.0,
) -> H3VideoSource:
    index = torch.arange(24 * slot_count * 3 * width, dtype=torch.float32).reshape(
        1, 24, slot_count, 3, width
    )
    video = scale * (torch.sin(index * (0.021 + identity * 0.001)) + identity * 0.01)
    return H3VideoSource(
        cartridge_id=f"00000000-0000-4000-8000-{identity:012d}",
        archive_sha256=archive_byte * 64,
        storage_dtype="F32",
        shape=tuple(video.shape),
        video_bytes=video.numpy().tobytes(order="C"),
        width=width * 16,
        height=48,
        frame_count=5 + 17 * ((slot_count - 2) // 5),
        frame_rate_numerator=24,
        frame_rate_denominator=1,
    )


def expected_slot(value: H3VideoSource, position: int) -> Any:
    tensor = torch.frombuffer(bytearray(value.video_bytes), dtype=torch.float32).reshape(
        value.shape
    )
    return tensor[:, :, position : position + 1].to(torch.float16).contiguous()


class RecordingDecoder:
    def __init__(self) -> None:
        self.slots: list[Any] = []
        self.reset_calls = 0

    def decode_slot(self, slot: Any) -> tuple[bytes, ...]:
        self.slots.append(slot.clone())
        return (b"synthetic-rgba",)

    def reset(self) -> None:
        self.reset_calls += 1


def quad(slot_count: int = 7) -> tuple[H3VideoSource, ...]:
    return tuple(
        source(slot_count, identity=index, archive_byte=archive)
        for index, archive in enumerate("abcd", start=1)
    )


def test_explicit_roles_bind_unchanged_carrier_and_ordered_donors_before_decode() -> None:
    source_a, source_b, source_c, source_d = quad()
    decoder = RecordingDecoder()
    engine = H3Q4StreamEngine(
        source_a,
        source_b,
        source_c,
        source_d,
        decoder,
        torch=torch,
        device=torch.device("cpu"),
        roles={"carrier": "C", "donor_b": "A", "donor_c": "D", "donor_d": "B"},
        controls={"algorithm": "XS5", "interaction": 0.0, "chaos": 0.0, "top_k": 4},
        seed=77,
    )

    decoded = engine.step()

    assert isinstance(decoded, Q4DecodedSlot)
    assert torch.equal(decoded.latent.output, expected_slot(source_c, 0))
    assert torch.equal(decoder.slots[0], decoded.latent.output)
    assert decoded.latent.roles.as_dict() == {
        "carrier": "C",
        "donor_b": "A",
        "donor_c": "D",
        "donor_d": "B",
    }
    assert decoded.latent.provenance["roles"] == {
        "carrier": {
            "slot": "C",
            "identity": source_c.cartridge_id,
            "playhead": 0,
        },
        "donors": [
            {"role": "B", "slot": "A", "identity": source_a.cartridge_id, "playhead": 0},
            {"role": "C", "slot": "D", "identity": source_d.cartridge_id, "playhead": 0},
            {"role": "D", "slot": "B", "identity": source_b.cartridge_id, "playhead": 0},
        ],
    }
    assert decoded.latent.provenance["routing"]["reference"] == "UNCHANGED_CARRIER"
    assert decoded.latent.provenance["routing"]["accumulation_order"] == ["B", "C", "D"]
    engine.close()


def test_role_and_seed_updates_are_deterministic_without_hidden_reset() -> None:
    inputs = quad()
    first_decoder = RecordingDecoder()
    first = H3Q4StreamEngine(
        *inputs,
        first_decoder,
        torch=torch,
        device=torch.device("cpu"),
        controls={"algorithm": "LINEAR", "interaction": 0.5, "chaos": 0.0},
        seed=1,
    )
    roles_result = first.update_roles(
        {"carrier": "D", "donor_b": "C", "donor_c": "B", "donor_d": "A"}
    )
    seed_result = first.update_seed(9001)
    output_a = first.step()
    assert isinstance(output_a, Q4DecodedSlot)
    assert roles_result["requires_causal_reset"] is False
    assert seed_result["requires_causal_reset"] is False

    second = H3Q4StreamEngine(
        *inputs,
        RecordingDecoder(),
        torch=torch,
        device=torch.device("cpu"),
        roles={"carrier": "D", "donor_b": "C", "donor_c": "B", "donor_d": "A"},
        controls={"algorithm": "LINEAR", "interaction": 0.5, "chaos": 0.0},
        seed=7,
    )
    output_b = second.step()
    assert isinstance(output_b, Q4DecodedSlot)
    assert torch.equal(output_a.latent.output, output_b.latent.output)
    first.close()
    second.close()


def test_four_source_loop_waits_for_explicit_causal_decoder_reset() -> None:
    decoder = RecordingDecoder()
    engine = H3Q4StreamEngine(
        *quad(slot_count=2),
        decoder,
        torch=torch,
        device=torch.device("cpu"),
    )
    assert isinstance(engine.step(), Q4DecodedSlot)
    assert isinstance(engine.step(), Q4DecodedSlot)
    barrier = engine.step()
    assert isinstance(barrier, Q4ResetBarrier)
    assert barrier.reasons == ("slot_a.loop", "slot_b.loop", "slot_c.loop", "slot_d.loop")
    assert decoder.reset_calls == 0

    applied = engine.apply_reset_barrier(2)

    assert decoder.reset_calls == 1
    assert applied["causal_state_cleared"] is True
    assert [applied[f"playhead_{slot}"] for slot in "abcd"] == [0, 0, 0, 0]
    after = engine.step()
    assert isinstance(after, Q4DecodedSlot)
    assert [getattr(after.latent, f"playhead_{slot}") for slot in "abcd"] == [0, 0, 0, 0]
    engine.close()


def test_geometry_overflow_role_and_operator_failures_are_explicit_and_path_free() -> None:
    source_a, source_b, source_c, source_d = quad()
    incompatible = source(7, identity=5, archive_byte="e", width=5)
    with pytest.raises(H3Q4SourceError, match="geometry differs"):
        H3Q4StreamEngine(
            source_a,
            source_b,
            source_c,
            incompatible,
            RecordingDecoder(),
            torch=object(),
            device=object(),
        )

    overflowing = source(7, identity=6, archive_byte="f", scale=1e10)
    with pytest.raises(H3Q4SourceError, match="could not be materialized"):
        H3Q4TensorSource(overflowing, torch, torch.device("cpu"))

    with pytest.raises(Q4StreamError, match="deck.roles_invalid"):
        H3Q4StreamEngine(
            source_a,
            source_b,
            source_c,
            source_d,
            RecordingDecoder(),
            torch=torch,
            device=torch.device("cpu"),
            roles={"carrier": "A", "donor_b": "A", "donor_c": "C", "donor_d": "D"},
        )

    with pytest.raises(OperatorLoadError, match="operator.version_mismatch"):
        H3Q4StreamEngine(
            source_a,
            source_b,
            source_c,
            source_d,
            RecordingDecoder(),
            torch=torch,
            device=torch.device("cpu"),
            operator_version="0.2.0",
        )
