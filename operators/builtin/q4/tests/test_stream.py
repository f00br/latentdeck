from __future__ import annotations

import json
import unittest

import torch

from latentdeck_operator_q4.contract import DeckSlot
from latentdeck_operator_q4.stream import (
    MAX_STREAM_GENERATION,
    Q4DecodedSlot,
    Q4DecodePump,
    Q4Paused,
    Q4ProcessedSlot,
    Q4ResetBarrier,
    Q4RoleAssignment,
    Q4Source,
    Q4StreamEngine,
    Q4StreamError,
    Q4Transport,
)
from latentdeck_operator_q4.trusted import builtin_registry


def source(
    slot: DeckSlot,
    value: float,
    *,
    latent_slots: int = 7,
    height: int = 2,
    width: int = 3,
    dtype: torch.dtype = torch.float16,
    reads: list[int] | None = None,
) -> Q4Source:
    tensor = torch.full(
        (1, 24, latent_slots, height, width),
        value,
        dtype=dtype,
    )
    ordinal = "ABCD".index(slot.value) + 1

    def read_slot(position: int) -> torch.Tensor:
        if reads is not None:
            reads.append(position)
        return tensor[:, :, position : position + 1]

    return Q4Source(
        cartridge_id=f"00000000-0000-4000-8000-{ordinal:012d}",
        archive_sha256=f"{ordinal:x}" * 64,
        shape=tuple(tensor.shape),
        read_slot=read_slot,
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


class Q4StreamTests(unittest.TestCase):
    def setUp(self) -> None:
        self.reads = {slot: [] for slot in DeckSlot}
        self.sources = {
            DeckSlot.A: source(DeckSlot.A, 1.0, latent_slots=7, reads=self.reads[DeckSlot.A]),
            DeckSlot.B: source(DeckSlot.B, 2.0, latent_slots=12, reads=self.reads[DeckSlot.B]),
            DeckSlot.C: source(DeckSlot.C, 3.0, latent_slots=17, reads=self.reads[DeckSlot.C]),
            DeckSlot.D: source(DeckSlot.D, 4.0, latent_slots=22, reads=self.reads[DeckSlot.D]),
        }
        self.operator = builtin_registry().load("org.latentdeck.builtin.ld_q4", "0.1.0")

    def engine(self, **kwargs) -> Q4StreamEngine:
        return Q4StreamEngine(
            self.operator,
            self.sources[DeckSlot.A],
            self.sources[DeckSlot.B],
            self.sources[DeckSlot.C],
            self.sources[DeckSlot.D],
            **kwargs,
        )

    def test_explicit_role_permutation_routes_one_immutable_carrier_slot(self) -> None:
        roles = Q4RoleAssignment(
            carrier=DeckSlot.C,
            donor_b=DeckSlot.A,
            donor_c=DeckSlot.D,
            donor_d=DeckSlot.B,
        )
        engine = self.engine(
            roles=roles,
            controls={
                "algorithm": "LINEAR",
                "interaction": 1.0,
                "donor_weight_b": 1.0,
                "donor_weight_c": 0.0,
                "donor_weight_d": 0.0,
            },
            seed=42,
        )

        step = engine.step()

        self.assertIsInstance(step, Q4ProcessedSlot)
        assert isinstance(step, Q4ProcessedSlot)
        self.assertTrue(torch.equal(step.output, torch.ones_like(step.output)))
        self.assertEqual(step.roles, roles)
        self.assertEqual(
            (step.playhead_a, step.playhead_b, step.playhead_c, step.playhead_d),
            (0, 0, 0, 0),
        )
        self.assertEqual(
            step.provenance["roles"]["carrier"],
            {
                "slot": "C",
                "identity": "00000000-0000-4000-8000-000000000003",
                "playhead": 0,
            },
        )
        self.assertEqual(
            step.provenance["routing"]["accumulation_order"],
            ["B", "C", "D"],
        )

    def test_role_assignment_requires_a_closed_full_permutation(self) -> None:
        roles = Q4RoleAssignment.from_mapping(
            {
                "carrier": "D",
                "donor_b": "B",
                "donor_c": "A",
                "donor_d": "C",
            }
        )
        self.assertEqual(
            roles.as_dict(),
            {"carrier": "D", "donor_b": "B", "donor_c": "A", "donor_d": "C"},
        )

        invalid_cases = (
            {"carrier": "A", "donor_b": "A", "donor_c": "C", "donor_d": "D"},
            {"carrier": "A", "donor_b": "B", "donor_c": "C"},
            {
                "carrier": "A",
                "donor_b": "B",
                "donor_c": "C",
                "donor_d": "D",
                "source_path": r"W:\private\cartridge.lc",
            },
        )
        for raw in invalid_cases:
            with self.subTest(raw=raw):
                with self.assertRaises(Q4StreamError) as caught:
                    Q4RoleAssignment.from_mapping(raw)
                self.assertEqual(caught.exception.code, "deck.roles_invalid")
                self.assertNotIn("W:\\private", caught.exception.detail)

    def test_four_playheads_advance_independently(self) -> None:
        engine = self.engine(
            transport=Q4Transport(
                playing_a=True,
                playing_b=False,
                playing_c=False,
                playing_d=True,
            )
        )
        first = engine.step()
        second = engine.step()
        assert isinstance(first, Q4ProcessedSlot)
        assert isinstance(second, Q4ProcessedSlot)
        self.assertEqual(
            (first.playhead_a, first.playhead_b, first.playhead_c, first.playhead_d),
            (0, 0, 0, 0),
        )
        self.assertEqual(
            (second.playhead_a, second.playhead_b, second.playhead_c, second.playhead_d),
            (1, 0, 0, 1),
        )
        self.assertEqual(self.reads[DeckSlot.A], [0, 1])
        self.assertEqual(self.reads[DeckSlot.B], [0, 0])
        self.assertEqual(self.reads[DeckSlot.C], [0, 0])
        self.assertEqual(self.reads[DeckSlot.D], [0, 1])

    def test_loop_yields_stable_barrier_until_decoder_reset_succeeds(self) -> None:
        engine = self.engine()
        for sequence in range(7):
            step = engine.step()
            assert isinstance(step, Q4ProcessedSlot)
            self.assertEqual(step.stream_sequence, sequence)
        barrier = engine.step()
        self.assertIsInstance(barrier, Q4ResetBarrier)
        assert isinstance(barrier, Q4ResetBarrier)
        self.assertEqual(barrier.reasons, ("slot_a.loop",))
        self.assertEqual(engine.step(), barrier)
        reads_before_reset = {slot: len(reads) for slot, reads in self.reads.items()}

        reset_count = 0

        def reset() -> None:
            nonlocal reset_count
            reset_count += 1

        applied = engine.apply_reset_barrier(2, reset)
        self.assertEqual(reset_count, 1)
        self.assertTrue(applied["causal_state_cleared"])
        self.assertEqual(
            (
                applied["playhead_a"],
                applied["playhead_b"],
                applied["playhead_c"],
                applied["playhead_d"],
            ),
            (0, 7, 7, 7),
        )
        after = engine.step()
        assert isinstance(after, Q4ProcessedSlot)
        self.assertEqual(
            (after.playhead_a, after.playhead_b, after.playhead_c, after.playhead_d),
            (0, 7, 7, 7),
        )
        self.assertEqual(after.stream_generation, 2)
        self.assertEqual(after.stream_sequence, 0)
        self.assertEqual(reads_before_reset, {slot: 7 for slot in DeckSlot})

    def test_simultaneous_loops_keep_fixed_physical_reason_order(self) -> None:
        same_length = {
            slot: source(slot, float(index), latent_slots=7)
            for index, slot in enumerate(DeckSlot, start=1)
        }
        engine = Q4StreamEngine(
            self.operator,
            same_length[DeckSlot.A],
            same_length[DeckSlot.B],
            same_length[DeckSlot.C],
            same_length[DeckSlot.D],
        )
        for _ in range(7):
            self.assertIsInstance(engine.step(), Q4ProcessedSlot)
        barrier = engine.step()
        assert isinstance(barrier, Q4ResetBarrier)
        self.assertEqual(
            barrier.reasons,
            ("slot_a.loop", "slot_b.loop", "slot_c.loop", "slot_d.loop"),
        )

    def test_restart_and_failed_reset_preserve_atomic_barrier_semantics(self) -> None:
        engine = self.engine()
        engine.step()
        engine.step()
        barrier = engine.request_restart()
        before = engine.status()

        def fail_reset() -> None:
            raise RuntimeError(r"W:\private\decoder-state.bin")

        with self.assertRaises(Q4StreamError) as caught:
            engine.apply_reset_barrier(2, fail_reset)
        self.assertEqual(caught.exception.code, "deck.reset_failed")
        self.assertNotIn("W:\\private", caught.exception.detail)
        self.assertEqual(engine.step(), barrier)
        self.assertEqual(engine.status(), before)

        applied = engine.apply_reset_barrier(3, lambda: None)
        self.assertEqual(
            (
                applied["playhead_a"],
                applied["playhead_b"],
                applied["playhead_c"],
                applied["playhead_d"],
            ),
            (0, 0, 0, 0),
        )
        restarted = engine.step()
        assert isinstance(restarted, Q4ProcessedSlot)
        self.assertEqual(restarted.stream_generation, 3)
        self.assertEqual(restarted.stream_sequence, 0)

    def test_generation_is_bounded_and_stale_resets_do_not_call_decoder(self) -> None:
        with self.assertRaises(Q4StreamError) as caught:
            self.engine(stream_generation=True)
        self.assertEqual(caught.exception.code, "deck.generation_invalid")

        engine = self.engine(stream_generation=MAX_STREAM_GENERATION)
        with self.assertRaises(Q4StreamError) as caught:
            engine.request_restart()
        self.assertEqual(caught.exception.code, "deck.generation_exhausted")
        self.assertFalse(engine.status()["pending_reset"])

        engine = self.engine()
        engine.request_restart()
        reset_calls = 0

        def reset() -> None:
            nonlocal reset_calls
            reset_calls += 1

        with self.assertRaises(Q4StreamError) as caught:
            engine.apply_reset_barrier(1, reset)
        self.assertEqual(caught.exception.code, "deck.generation_stale")
        self.assertEqual(reset_calls, 0)
        self.assertIsInstance(engine.step(), Q4ResetBarrier)

    def test_non_looping_source_holds_while_other_playheads_continue(self) -> None:
        engine = self.engine(
            transport=Q4Transport(
                playing_a=True,
                playing_b=True,
                playing_c=False,
                playing_d=False,
                loop_a=False,
            )
        )
        for _ in range(7):
            self.assertIsInstance(engine.step(), Q4ProcessedSlot)
        held = engine.step()
        assert isinstance(held, Q4ProcessedSlot)
        self.assertEqual(
            (held.playhead_a, held.playhead_b, held.playhead_c, held.playhead_d),
            (6, 7, 0, 0),
        )
        self.assertFalse(engine.transport.playing_a)
        self.assertTrue(engine.transport.playing_b)

    def test_source_paused_after_final_slot_holds_last_valid_slot(self) -> None:
        engine = self.engine(
            transport=Q4Transport(
                playing_a=True,
                playing_b=True,
                playing_c=False,
                playing_d=False,
            )
        )
        for _ in range(7):
            self.assertIsInstance(engine.step(), Q4ProcessedSlot)

        engine.update_transport(
            Q4Transport(
                playing_a=False,
                playing_b=True,
                playing_c=False,
                playing_d=False,
            )
        )
        held = engine.step()

        assert isinstance(held, Q4ProcessedSlot)
        self.assertEqual(
            (held.playhead_a, held.playhead_b, held.playhead_c, held.playhead_d),
            (6, 7, 0, 0),
        )
        self.assertEqual(self.reads[DeckSlot.A][-2:], [6, 6])
        self.assertFalse(engine.transport.playing_a)
        self.assertTrue(engine.transport.playing_b)

    def test_all_paused_returns_typed_status_without_reading_sources(self) -> None:
        engine = self.engine(
            transport=Q4Transport(
                playing_a=False,
                playing_b=False,
                playing_c=False,
                playing_d=False,
            )
        )
        paused = engine.step()
        self.assertIsInstance(paused, Q4Paused)
        assert isinstance(paused, Q4Paused)
        self.assertEqual(paused.roles.as_dict(), Q4RoleAssignment().as_dict())
        self.assertEqual(
            {slot: len(reads) for slot, reads in self.reads.items()},
            {slot: 0 for slot in DeckSlot},
        )

    def test_operator_and_capture_sink_run_before_causal_decode(self) -> None:
        engine = self.engine(
            controls={
                "algorithm": "LINEAR",
                "interaction": 1.0,
                "donor_weight_b": 1.0,
                "donor_weight_c": 0.0,
                "donor_weight_d": 0.0,
            }
        )
        decoder = RecordingDecoder()
        sink_outputs: list[torch.Tensor] = []

        def before_decode(step: Q4ProcessedSlot) -> None:
            self.assertEqual(decoder.slots, [])
            sink_outputs.append(step.output.clone())

        pump = Q4DecodePump(engine, decoder)
        decoded = pump.step(before_decode)
        self.assertIsInstance(decoded, Q4DecodedSlot)
        assert isinstance(decoded, Q4DecodedSlot)
        self.assertTrue(torch.equal(sink_outputs[0], decoded.latent.output))
        self.assertTrue(torch.equal(decoder.slots[0], decoded.latent.output))
        self.assertEqual(decoded.decoded, {"frames": 4})

        engine.request_restart()
        pump.apply_reset_barrier(2)
        self.assertEqual(decoder.reset_count, 1)

    def test_updates_are_typed_boundary_events_and_replay_deterministically(self) -> None:
        event_roles = {
            "carrier": "B",
            "donor_b": "D",
            "donor_c": "A",
            "donor_d": "C",
        }
        outputs: list[tuple[torch.Tensor, dict[str, object]]] = []
        for _ in range(2):
            engine = self.engine()
            first = engine.step()
            roles_ack = engine.update_roles(event_roles)
            controls_ack = engine.update_controls(
                {
                    "algorithm": "LINEAR",
                    "interaction": 0.75,
                    "donor_weight_b": 0.25,
                    "donor_weight_c": 0.5,
                    "donor_weight_d": 0.25,
                }
            )
            seed_ack = engine.update_seed(8128)
            second = engine.step()
            assert isinstance(first, Q4ProcessedSlot)
            assert isinstance(second, Q4ProcessedSlot)
            self.assertFalse(roles_ack["requires_causal_reset"])
            self.assertFalse(controls_ack["requires_causal_reset"])
            self.assertFalse(seed_ack["requires_causal_reset"])
            self.assertEqual(first.roles, Q4RoleAssignment())
            self.assertEqual(second.roles.as_dict(), event_roles)
            self.assertEqual(second.provenance["operation"]["seed"], 8128)
            outputs.append((second.output, second.provenance))

        self.assertTrue(torch.equal(outputs[0][0], outputs[1][0]))
        self.assertEqual(outputs[0][1], outputs[1][1])

    def test_provenance_has_source_ids_and_hashes_but_never_paths(self) -> None:
        step = self.engine().step()
        assert isinstance(step, Q4ProcessedSlot)
        encoded = json.dumps(step.provenance, allow_nan=False)
        for cartridge_source in self.sources.values():
            self.assertIn(cartridge_source.cartridge_id, encoded)
            self.assertIn(cartridge_source.archive_sha256, encoded)
        self.assertNotIn("cartridge_path", encoded)
        self.assertNotIn("\\", encoded)

    def test_incompatible_sources_and_invalid_reads_fail_path_free(self) -> None:
        incompatible = source(DeckSlot.D, 4.0, width=4)
        with self.assertRaises(Q4StreamError) as caught:
            Q4StreamEngine(
                self.operator,
                self.sources[DeckSlot.A],
                self.sources[DeckSlot.B],
                self.sources[DeckSlot.C],
                incompatible,
            )
        self.assertEqual(caught.exception.code, "deck.source_incompatible")

        invalid_hash = Q4Source(
            cartridge_id=self.sources[DeckSlot.A].cartridge_id,
            archive_sha256=r"W:\private\not-a-hash",
            shape=self.sources[DeckSlot.A].shape,
            read_slot=self.sources[DeckSlot.A].read_slot,
        )
        with self.assertRaises(Q4StreamError) as caught:
            Q4StreamEngine(
                self.operator,
                invalid_hash,
                self.sources[DeckSlot.B],
                self.sources[DeckSlot.C],
                self.sources[DeckSlot.D],
            )
        self.assertEqual(caught.exception.code, "deck.source_invalid")
        self.assertNotIn("W:\\private", caught.exception.detail)

        wrong_dtype = source(DeckSlot.A, 1.0, dtype=torch.float32)
        engine = Q4StreamEngine(
            self.operator,
            wrong_dtype,
            self.sources[DeckSlot.B],
            self.sources[DeckSlot.C],
            self.sources[DeckSlot.D],
        )
        with self.assertRaises(Q4StreamError) as caught:
            engine.step()
        self.assertEqual(caught.exception.code, "deck.process_failed")
        self.assertEqual(caught.exception.detail, "tensor.dtype")

        def fail_read(_position: int) -> torch.Tensor:
            raise OSError(r"W:\private\raw.safetensors")

        unreadable = Q4Source(
            cartridge_id=self.sources[DeckSlot.A].cartridge_id,
            archive_sha256=self.sources[DeckSlot.A].archive_sha256,
            shape=self.sources[DeckSlot.A].shape,
            read_slot=fail_read,
        )
        engine = Q4StreamEngine(
            self.operator,
            unreadable,
            self.sources[DeckSlot.B],
            self.sources[DeckSlot.C],
            self.sources[DeckSlot.D],
        )
        with self.assertRaises(Q4StreamError) as caught:
            engine.step()
        self.assertEqual(caught.exception.code, "deck.source_read_failed")
        self.assertNotIn("W:\\private", caught.exception.detail)


if __name__ == "__main__":
    unittest.main()
