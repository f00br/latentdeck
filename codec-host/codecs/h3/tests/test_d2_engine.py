# ruff: noqa: E402

from __future__ import annotations

from dataclasses import replace
from typing import Any

import pytest

torch = pytest.importorskip("torch", reason="D2 conformance requires the pinned runtime extra")

from latentdeck_operator_d2 import D2DecodedSlot, D2ResetBarrier

from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.d2_engine import (
    H3D2SourceError,
    H3D2StreamEngine,
    H3TensorDeckSource,
)
from latentdeck_codec_h3.decoder import CodecRuntimeError, H3Decoder


def source(
    slot_count: int,
    *,
    cartridge_id: str,
    archive_byte: str,
    width: int = 4,
    storage_dtype: str = "F32",
    scale: float = 1.0,
) -> H3VideoSource:
    dtype = torch.float32 if storage_dtype == "F32" else torch.float16
    index = torch.arange(24 * slot_count * 3 * width, dtype=torch.float32).reshape(
        1, 24, slot_count, 3, width
    )
    video = (scale * torch.sin(index * 0.031)).to(dtype)
    return H3VideoSource(
        cartridge_id=cartridge_id,
        archive_sha256=archive_byte * 64,
        storage_dtype=storage_dtype,
        shape=tuple(video.shape),
        video_bytes=video.numpy().tobytes(order="C"),
        width=width * 16,
        height=48,
        frame_count=5 + 17 * ((slot_count - 2) // 5),
        frame_rate_numerator=24,
        frame_rate_denominator=1,
    )


class RecordingDecoder:
    def __init__(self) -> None:
        self.slots: list[Any] = []
        self.reset_calls = 0

    def decode_slot(self, slot: Any) -> tuple[bytes, ...]:
        self.slots.append(slot.clone())
        return (b"synthetic-rgba",)

    def reset(self) -> None:
        self.reset_calls += 1


class FakeTaeStream:
    def __init__(self) -> None:
        self.reset_calls = 0

    def decode(self, _slot: Any = None) -> Any:
        return torch.full((1, 1, 3, 2, 2), 0.5, dtype=torch.float32)

    def reset(self) -> None:
        self.reset_calls += 1


def sources() -> tuple[H3VideoSource, H3VideoSource]:
    return (
        source(
            7,
            cartridge_id="11111111-1111-4111-8111-111111111111",
            archive_byte="a",
        ),
        source(
            12,
            cartridge_id="22222222-2222-4222-8222-222222222222",
            archive_byte="b",
        ),
    )


def test_h3_sources_are_cast_once_then_processed_before_decode() -> None:
    source_a, source_b = sources()
    decoder = RecordingDecoder()
    engine = H3D2StreamEngine(
        source_a,
        source_b,
        decoder,
        torch=torch,
        device=torch.device("cpu"),
        controls={"algorithm": "XS2", "interaction": 1.0, "preserve": 0.2},
        seed=77,
    )
    decoded = engine.step()
    assert isinstance(decoded, D2DecodedSlot)
    assert decoder.slots[0].dtype == torch.float16
    assert torch.equal(decoder.slots[0], decoded.latent.output)
    assert decoded.decoded == (b"synthetic-rgba",)
    assert decoded.latent.provenance["operation"]["seed"] == 77
    assert (
        decoded.latent.provenance["stream"]["sources"]["a"]["archive_sha256"]
        == source_a.archive_sha256
    )
    engine.close()


def test_h3_d2_loop_cannot_cross_decoder_reset_barrier() -> None:
    source_a, source_b = sources()
    decoder = RecordingDecoder()
    engine = H3D2StreamEngine(
        source_a,
        source_b,
        decoder,
        torch=torch,
        device=torch.device("cpu"),
    )
    for _ in range(7):
        assert isinstance(engine.step(), D2DecodedSlot)
    barrier = engine.step()
    assert isinstance(barrier, D2ResetBarrier)
    assert decoder.reset_calls == 0
    applied = engine.apply_reset_barrier(2)
    assert decoder.reset_calls == 1
    assert applied["causal_state_cleared"] is True
    after = engine.step()
    assert isinstance(after, D2DecodedSlot)
    assert (after.latent.playhead_a, after.latent.playhead_b) == (0, 7)


def test_incompatible_geometry_and_f16_overflow_are_explicit_rejects() -> None:
    source_a, _ = sources()
    incompatible = source(
        7,
        cartridge_id="33333333-3333-4333-8333-333333333333",
        archive_byte="c",
        width=5,
    )
    with pytest.raises(H3D2SourceError, match="spatial geometry"):
        H3D2StreamEngine(
            source_a,
            incompatible,
            RecordingDecoder(),
            torch=torch,
            device=torch.device("cpu"),
        )

    overflowing = source(
        7,
        cartridge_id="44444444-4444-4444-8444-444444444444",
        archive_byte="d",
        scale=1e10,
    )
    with pytest.raises(H3D2SourceError, match="NaN or Inf"):
        H3TensorDeckSource(overflowing, torch, torch.device("cpu"))


def test_h3_source_metadata_is_rejected_before_runtime_materialization() -> None:
    source_a, _ = sources()
    with pytest.raises(H3D2SourceError, match="storage dtype"):
        H3TensorDeckSource(
            replace(source_a, storage_dtype="F64"),
            torch,
            torch.device("cpu"),
        )
    with pytest.raises(H3D2SourceError, match="full-grid"):
        H3TensorDeckSource(
            replace(
                source_a,
                shape=(1, 24, 7, 65, 64),
                video_bytes=b"",
                width=1024,
                height=1040,
            ),
            torch,
            torch.device("cpu"),
        )


def test_h3_decoder_slot_surface_preserves_causal_cadence_and_f16_gate() -> None:
    stream = FakeTaeStream()
    decoder = H3Decoder(torch, torch.device("cpu"), object(), stream)
    slot = torch.zeros((1, 24, 1, 1, 1), dtype=torch.float16)
    first = decoder.decode_slot(slot)
    second = decoder.decode_slot(slot)
    assert len(first) == 1
    assert len(second) == 4
    assert all(len(frame) == 2 * 2 * 4 for frame in (*first, *second))
    decoder.reset()
    assert len(decoder.decode_slot(slot)) == 1
    assert stream.reset_calls == 1
    with pytest.raises(CodecRuntimeError, match="runtime dtype"):
        decoder.decode_slot(slot.float())
