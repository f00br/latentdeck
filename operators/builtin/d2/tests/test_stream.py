from __future__ import annotations

import json
import unittest

import torch

from latentdeck_operator_d2 import (
    MAX_STREAM_GENERATION,
    D2DecodedSlot,
    D2DecodePump,
    D2ProcessedSlot,
    D2ResetBarrier,
    D2Source,
    D2StreamEngine,
    D2StreamError,
    D2Transport,
    builtin_registry,
)


def source(
    slot_count: int,
    *,
    cartridge_id: str,
    archive_byte: str,
    height: int = 3,
    width: int = 4,
    dtype: torch.dtype = torch.float16,
) -> tuple[D2Source, torch.Tensor, list[int]]:
    count = 24 * slot_count * height * width
    index = torch.arange(count, dtype=torch.float32).reshape(1, 24, slot_count, height, width)
    video = (torch.sin(index * 0.031) + 0.2 * torch.cos(index * 0.079)).to(dtype)
    reads: list[int] = []

    def read_slot(position: int) -> torch.Tensor:
        reads.append(position)
        return video[:, :, position : position + 1]

    return (
        D2Source(
            cartridge_id=cartridge_id,
            archive_sha256=archive_byte * 64,
            shape=tuple(video.shape),
            read_slot=read_slot,
        ),
        video,
        reads,
    )


class RecordingDecoder:
    def __init__(self) -> None:
        self.slots: list[torch.Tensor] = []
        self.reset_count = 0

    def decode_slot(self, slot: torch.Tensor) -> dict[str, int]:
        self.slots.append(slot.clone())
        return {"frames": 4}

    def reset(self) -> None:
        self.reset_count += 1


class D2StreamTests(unittest.TestCase):
    def setUp(self) -> None:
        self.a, self.a_video, self.a_reads = source(
            7,
            cartridge_id="11111111-1111-4111-8111-111111111111",
            archive_byte="a",
        )
        self.b, self.b_video, self.b_reads = source(
            12,
            cartridge_id="22222222-2222-4222-8222-222222222222",
            archive_byte="b",
        )
        self.operator = builtin_registry().load("org.latentdeck.builtin.ld_d2", "0.1.0")

    def engine(self, **kwargs) -> D2StreamEngine:
        return D2StreamEngine(self.operator, self.a, self.b, **kwargs)

    def test_playheads_advance_independently(self) -> None:
        engine = self.engine(
            transport=D2Transport(playing_a=True, playing_b=False, loop_a=True, loop_b=True)
        )
        first = engine.step()
        second = engine.step()
        self.assertIsInstance(first, D2ProcessedSlot)
        self.assertIsInstance(second, D2ProcessedSlot)
        assert isinstance(first, D2ProcessedSlot)
        assert isinstance(second, D2ProcessedSlot)
        self.assertEqual((first.playhead_a, first.playhead_b), (0, 0))
        self.assertEqual((second.playhead_a, second.playhead_b), (1, 0))
        self.assertEqual(self.a_reads, [0, 1])
        self.assertEqual(self.b_reads, [0, 0])
        self.assertEqual(engine.status()["playhead_a"], 2)
        self.assertEqual(engine.status()["playhead_b"], 0)

    def test_operator_runs_before_decoder_without_rgb_fallback(self) -> None:
        engine = self.engine(
            controls={
                "algorithm": "XS2",
                "interaction": 1.0,
                "preserve": 0.2,
                "mix": 0.45,
            }
        )
        decoder = RecordingDecoder()
        decoded = D2DecodePump(engine, decoder).step()
        self.assertIsInstance(decoded, D2DecodedSlot)
        assert isinstance(decoded, D2DecodedSlot)
        self.assertTrue(torch.equal(decoder.slots[0], decoded.latent.output))
        self.assertFalse(torch.equal(decoder.slots[0], self.a_video[:, :, 0:1]))
        self.assertFalse(torch.equal(decoder.slots[0], self.b_video[:, :, 0:1]))
        self.assertEqual(decoded.decoded, {"frames": 4})

    def test_loop_yields_stable_barrier_until_decoder_reset_succeeds(self) -> None:
        engine = self.engine()
        for sequence in range(7):
            step = engine.step()
            self.assertIsInstance(step, D2ProcessedSlot)
            assert isinstance(step, D2ProcessedSlot)
            self.assertEqual(step.stream_sequence, sequence)
        barrier = engine.step()
        self.assertIsInstance(barrier, D2ResetBarrier)
        assert isinstance(barrier, D2ResetBarrier)
        self.assertEqual(barrier.reasons, ("slot_a.loop",))
        self.assertEqual(engine.step(), barrier)
        reads_before_reset = (len(self.a_reads), len(self.b_reads))

        reset_count = 0

        def reset() -> None:
            nonlocal reset_count
            reset_count += 1

        applied = engine.apply_reset_barrier(2, reset)
        self.assertEqual(reset_count, 1)
        self.assertTrue(applied["causal_state_cleared"])
        self.assertEqual((applied["playhead_a"], applied["playhead_b"]), (0, 7))
        self.assertEqual(engine.status()["stream_sequence"], 0)
        after = engine.step()
        self.assertIsInstance(after, D2ProcessedSlot)
        assert isinstance(after, D2ProcessedSlot)
        self.assertEqual((after.playhead_a, after.playhead_b), (0, 7))
        self.assertEqual(after.stream_generation, 2)
        self.assertEqual(after.stream_sequence, 0)
        self.assertEqual(reads_before_reset, (7, 7))

    def test_stale_generation_cannot_cross_reset_barrier(self) -> None:
        engine = self.engine()
        engine.request_restart()
        calls = 0

        def reset() -> None:
            nonlocal calls
            calls += 1

        with self.assertRaises(D2StreamError) as caught:
            engine.apply_reset_barrier(1, reset)
        self.assertEqual(caught.exception.code, "deck.generation_stale")
        self.assertEqual(calls, 0)
        self.assertIsInstance(engine.step(), D2ResetBarrier)

    def test_failed_decoder_reset_keeps_the_barrier_and_stream_state(self) -> None:
        engine = self.engine()
        barrier = engine.request_restart()
        before = engine.status()

        def fail_reset() -> None:
            raise RuntimeError("private decoder detail")

        with self.assertRaises(D2StreamError) as caught:
            engine.apply_reset_barrier(2, fail_reset)
        self.assertEqual(caught.exception.code, "deck.reset_failed")
        self.assertNotIn("private decoder detail", caught.exception.detail)
        self.assertEqual(engine.step(), barrier)
        self.assertEqual(engine.status(), before)

    def test_generation_is_bounded_to_nonzero_u64(self) -> None:
        with self.assertRaises(D2StreamError) as caught:
            self.engine(stream_generation=True)
        self.assertEqual(caught.exception.code, "deck.generation_invalid")

        engine = self.engine(stream_generation=MAX_STREAM_GENERATION)
        with self.assertRaises(D2StreamError) as caught:
            engine.request_restart()
        self.assertEqual(caught.exception.code, "deck.generation_exhausted")
        self.assertFalse(engine.status()["pending_reset"])

        engine = self.engine()
        engine.request_restart()
        with self.assertRaises(D2StreamError) as caught:
            engine.apply_reset_barrier(MAX_STREAM_GENERATION + 1, lambda: None)
        self.assertEqual(caught.exception.code, "deck.generation_invalid")
        self.assertIsInstance(engine.step(), D2ResetBarrier)

    def test_restart_resets_both_playheads_and_xs3_history(self) -> None:
        engine = self.engine(controls={"algorithm": "XS3", "interaction": 1.0})
        first = engine.step()
        second = engine.step()
        assert isinstance(first, D2ProcessedSlot)
        assert isinstance(second, D2ProcessedSlot)
        self.assertEqual(second.provenance["history"]["previous_a_supplied"], True)
        engine.request_restart()
        reset_calls = 0

        def reset() -> None:
            nonlocal reset_calls
            reset_calls += 1

        engine.apply_reset_barrier(3, reset)
        restarted = engine.step()
        assert isinstance(restarted, D2ProcessedSlot)
        self.assertEqual((restarted.playhead_a, restarted.playhead_b), (0, 0))
        self.assertEqual(restarted.provenance["history"]["previous_a_supplied"], False)
        self.assertEqual(reset_calls, 1)

    def test_controls_and_seed_are_typed_and_do_not_force_decoder_reset(self) -> None:
        engine = self.engine()
        control_ack = engine.update_controls(
            {"algorithm": "XS5", "xs5_routing": "SINKHORN", "interaction": 0.8}
        )
        seed_ack = engine.update_seed(42)
        self.assertFalse(control_ack["requires_causal_reset"])
        self.assertFalse(seed_ack["requires_causal_reset"])
        step = engine.step()
        assert isinstance(step, D2ProcessedSlot)
        operation = step.provenance["operation"]
        self.assertEqual(operation["seed"], 42)
        self.assertEqual(operation["controls"]["algorithm"], "XS5")
        self.assertEqual(operation["controls"]["xs5_routing"], "SINKHORN")

    def test_provenance_contains_only_source_identity_not_paths(self) -> None:
        step = self.engine().step()
        assert isinstance(step, D2ProcessedSlot)
        encoded = json.dumps(step.provenance, allow_nan=False)
        self.assertIn(self.a.cartridge_id, encoded)
        self.assertIn(self.b.archive_sha256, encoded)
        self.assertNotIn("cartridge_path", encoded)
        self.assertNotIn("\\", encoded)

    def test_incompatible_sources_and_invalid_runtime_slots_are_rejected(self) -> None:
        incompatible, _, _ = source(
            7,
            cartridge_id="33333333-3333-4333-8333-333333333333",
            archive_byte="c",
            width=5,
        )
        with self.assertRaises(D2StreamError) as caught:
            D2StreamEngine(self.operator, self.a, incompatible)
        self.assertEqual(caught.exception.code, "deck.source_incompatible")

        wrong_dtype, _, _ = source(
            7,
            cartridge_id="44444444-4444-4444-8444-444444444444",
            archive_byte="d",
            dtype=torch.float32,
        )
        engine = D2StreamEngine(self.operator, wrong_dtype, wrong_dtype)
        with self.assertRaises(D2StreamError) as caught:
            engine.step()
        self.assertEqual(caught.exception.code, "deck.process_failed")
        self.assertEqual(caught.exception.detail, "tensor.dtype")

    def test_non_looping_source_holds_last_slot_while_other_source_continues(self) -> None:
        engine = self.engine(
            transport=D2Transport(playing_a=True, playing_b=True, loop_a=False, loop_b=True)
        )
        for _ in range(7):
            self.assertIsInstance(engine.step(), D2ProcessedSlot)
        held = engine.step()
        self.assertIsInstance(held, D2ProcessedSlot)
        assert isinstance(held, D2ProcessedSlot)
        self.assertEqual((held.playhead_a, held.playhead_b), (6, 7))
        self.assertFalse(engine.transport.playing_a)
        self.assertTrue(engine.transport.playing_b)


if __name__ == "__main__":
    unittest.main()
