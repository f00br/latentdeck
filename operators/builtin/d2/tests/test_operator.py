from __future__ import annotations

import json
import os
import unittest

import torch

from latentdeck_operator_d2 import (
    MAX_SPATIAL_TOKENS,
    D2Context,
    D2ContractError,
    D2Controls,
    get_descriptor,
    get_descriptor_schema,
    process_slot,
)


def synthetic_pair(
    *, height: int = 3, width: int = 4, device: str = "cpu"
) -> tuple[torch.Tensor, torch.Tensor]:
    count = 24 * height * width
    index = torch.arange(count, dtype=torch.float32, device=device).reshape(1, 24, 1, height, width)
    a = (torch.sin(index * 0.071) + 0.1 * torch.cos(index * 0.017)).to(torch.float16)
    b = (torch.cos(index * 0.043) - 0.15 * torch.sin(index * 0.113)).to(torch.float16)
    return a, b


def previous_pair(a: torch.Tensor, b: torch.Tensor) -> tuple[torch.Tensor, torch.Tensor]:
    previous_a = torch.roll(a, shifts=1, dims=1) * 0.75
    previous_b = torch.roll(b, shifts=-1, dims=1) * 0.65
    return previous_a, previous_b


class DescriptorTests(unittest.TestCase):
    def test_descriptor_is_machine_readable_and_defensive(self) -> None:
        descriptor = get_descriptor()
        schema = get_descriptor_schema()
        self.assertEqual(descriptor["schema_version"], "0.1.0")
        self.assertEqual(descriptor["operator_id"], "org.latentdeck.builtin.ld_d2")
        self.assertEqual(descriptor["entrypoint"], "latentdeck_operator_d2:process_slot")
        self.assertEqual(descriptor["algorithms"], ["LINEAR", "XS1", "XS2", "XS3", "XS4", "XS5"])
        self.assertEqual(descriptor["limits"]["max_spatial_tokens"], MAX_SPATIAL_TOKENS)
        self.assertEqual(schema["properties"]["operator_id"]["const"], descriptor["operator_id"])
        descriptor["algorithms"].clear()
        self.assertEqual(len(get_descriptor()["algorithms"]), 6)


class OperatorContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.a, self.b = synthetic_pair()
        self.previous_a, self.previous_b = previous_pair(self.a, self.b)

    def context(self, **changes: object) -> D2Context:
        values = {
            "playhead_a": 17,
            "playhead_b": 4,
            "seed": 8128,
            "previous_a": self.previous_a,
            "previous_b": self.previous_b,
        }
        values.update(changes)
        return D2Context(**values)

    def test_linear_identity_endpoints_are_exact(self) -> None:
        at_a = process_slot(self.a, self.b, {"algorithm": "LINEAR", "mix": 0.0})
        at_b = process_slot(self.a, self.b, {"algorithm": "LINEAR", "mix": 1.0})
        self.assertTrue(torch.equal(at_a.output, self.a))
        self.assertTrue(torch.equal(at_b.output, self.b))
        self.assertEqual(at_a.output.dtype, torch.float16)
        self.assertEqual(at_a.output.device.type, "cpu")

    def test_zero_interaction_is_exact_linear_bypass_for_every_xs_family(self) -> None:
        baseline = process_slot(self.a, self.b, {"algorithm": "LINEAR", "mix": 0.37})
        for algorithm in ("XS1", "XS2", "XS3", "XS4", "XS5"):
            with self.subTest(algorithm=algorithm):
                result = process_slot(
                    self.a,
                    self.b,
                    {
                        "algorithm": algorithm,
                        "mix": 0.37,
                        "interaction": 0.0,
                        "chaos": 0.0,
                        "top_k": 4,
                    },
                    self.context(),
                )
                self.assertTrue(torch.equal(result.output, baseline.output))

    def test_xs_families_are_numerically_distinct(self) -> None:
        outputs: dict[str, torch.Tensor] = {}
        for algorithm in ("XS1", "XS2", "XS3", "XS4", "XS5"):
            outputs[algorithm] = process_slot(
                self.a,
                self.b,
                {
                    "algorithm": algorithm,
                    "mix": 0.41,
                    "interaction": 0.9,
                    "preserve": 0.32,
                    "top_k": 4,
                },
                self.context(),
            ).output
        names = list(outputs)
        for index, left in enumerate(names):
            for right in names[index + 1 :]:
                with self.subTest(left=left, right=right):
                    self.assertFalse(torch.equal(outputs[left], outputs[right]))

    def test_topk_and_sinkhorn_paths_are_distinct_and_bounded(self) -> None:
        common = {
            "algorithm": "XS5",
            "mix": 0.5,
            "interaction": 1.0,
            "preserve": 0.2,
            "temperature": 0.11,
            "top_k": 3,
            "sinkhorn_iterations": 4,
        }
        topk = process_slot(self.a, self.b, {**common, "xs5_routing": "TOPK"})
        sinkhorn = process_slot(self.a, self.b, {**common, "xs5_routing": "SINKHORN"})
        self.assertFalse(torch.equal(topk.output, sinkhorn.output))
        self.assertTrue(bool(torch.isfinite(topk.output).all().item()))
        self.assertTrue(bool(torch.isfinite(sinkhorn.output).all().item()))
        self.assertEqual(topk.output.shape, self.a.shape)
        self.assertEqual(sinkhorn.output.shape, self.a.shape)

    def test_hybridize_and_interact_are_distinct(self) -> None:
        controls = {
            "algorithm": "XS2",
            "mix": 0.5,
            "interaction": 0.75,
            "preserve": 0.4,
        }
        hybrid = process_slot(self.a, self.b, {**controls, "mode": "HYBRIDIZE"})
        interact = process_slot(self.a, self.b, {**controls, "mode": "INTERACT"})
        self.assertFalse(torch.equal(hybrid.output, interact.output))

    def test_routing_selects_structural_carrier(self) -> None:
        controls = {
            "algorithm": "XS5",
            "xs5_routing": "TOPK",
            "top_k": 4,
            "interaction": 1.0,
            "preserve": 0.75,
        }
        routed_a = process_slot(self.a, self.b, {**controls, "routing": "A"})
        routed_b = process_slot(self.a, self.b, {**controls, "routing": "B"})
        self.assertFalse(torch.equal(routed_a.output, routed_b.output))
        self.assertEqual(routed_a.provenance["structural_carrier"], "A")
        self.assertEqual(routed_b.provenance["structural_carrier"], "B")

    def test_seeded_chaos_is_exactly_repeatable_and_zero_is_unchanged(self) -> None:
        controls = {
            "algorithm": "XS1",
            "interaction": 0.8,
            "chaos": 0.7,
        }
        first = process_slot(self.a, self.b, controls, self.context(seed=101))
        repeated = process_slot(self.a, self.b, controls, self.context(seed=101))
        other_seed = process_slot(self.a, self.b, controls, self.context(seed=202))
        self.assertTrue(torch.equal(first.output, repeated.output))
        self.assertFalse(torch.equal(first.output, other_seed.output))

        without_chaos_a = process_slot(
            self.a, self.b, {**controls, "chaos": 0.0}, self.context(seed=101)
        )
        without_chaos_b = process_slot(
            self.a, self.b, {**controls, "chaos": 0.0}, self.context(seed=202)
        )
        self.assertTrue(torch.equal(without_chaos_a.output, without_chaos_b.output))

    def test_independent_playheads_and_history_are_recorded(self) -> None:
        result = process_slot(
            self.a,
            self.b,
            {"algorithm": "XS3", "interaction": 1.0},
            self.context(playhead_a=29, playhead_b=3),
        )
        self.assertEqual(result.provenance["playheads"], {"a": 29, "b": 3})
        self.assertEqual(
            result.provenance["history"],
            {"previous_a_supplied": True, "previous_b_supplied": True},
        )
        json.dumps(result.provenance)

    def test_control_and_tensor_bounds_reject_instead_of_clamping(self) -> None:
        cases = [
            ({"mix": 1.01}, "control.out_of_range"),
            ({"sinkhorn_iterations": 13}, "control.out_of_range"),
            ({"unknown_knob": 0}, "control.unknown"),
            (
                {"algorithm": "XS5", "xs5_routing": "TOPK", "top_k": 13},
                "control.out_of_range",
            ),
        ]
        for controls, code in cases:
            with self.subTest(controls=controls):
                with self.assertRaises(D2ContractError) as caught:
                    process_slot(self.a, self.b, controls)
                self.assertEqual(caught.exception.code, code)

        with self.assertRaises(D2ContractError) as caught:
            process_slot(self.a, self.b, D2Controls(mix=2.0))
        self.assertEqual(caught.exception.code, "control.out_of_range")

        too_large, _ = synthetic_pair(height=65, width=64)
        with self.assertRaises(D2ContractError) as caught:
            process_slot(too_large, too_large)
        self.assertEqual(caught.exception.code, "tensor.too_large")

    def test_shape_dtype_profile_and_history_gates(self) -> None:
        with self.assertRaises(D2ContractError) as caught:
            process_slot(self.a.float(), self.b.float())
        self.assertEqual(caught.exception.code, "tensor.dtype")

        with self.assertRaises(D2ContractError) as caught:
            process_slot(self.a, self.b[..., :2])
        self.assertEqual(caught.exception.code, "tensor.incompatible_shape")

        with self.assertRaises(D2ContractError) as caught:
            process_slot(self.a, self.b, context={"profile_version": "0.2.0"})
        self.assertEqual(caught.exception.code, "profile.incompatible")

        with self.assertRaises(D2ContractError) as caught:
            process_slot(
                self.a,
                self.b,
                {"algorithm": "XS3", "interaction": 1.0},
                self.context(previous_a=self.previous_a[..., :2]),
            )
        self.assertEqual(caught.exception.code, "tensor.incompatible_history")

    def test_nan_and_inf_are_rejected_before_processing(self) -> None:
        for value in (float("nan"), float("inf")):
            damaged = self.a.clone()
            damaged[0, 0, 0, 0, 0] = value
            with self.subTest(value=value):
                with self.assertRaises(D2ContractError) as caught:
                    process_slot(damaged, self.b)
                self.assertEqual(caught.exception.code, "tensor.non_finite")


@unittest.skipUnless(
    torch.cuda.is_available() and os.environ.get("LATENTDECK_RUN_CUDA_TESTS") == "1",
    "optional CUDA parity test; set LATENTDECK_RUN_CUDA_TESTS=1 to enable",
)
class OptionalCudaParityTests(unittest.TestCase):
    def test_xs5_topk_cpu_cuda_parity(self) -> None:
        cpu_a, cpu_b = synthetic_pair()
        controls = {
            "algorithm": "XS5",
            "xs5_routing": "TOPK",
            "top_k": 4,
            "interaction": 0.8,
            "chaos": 0.2,
        }
        cpu = process_slot(cpu_a, cpu_b, controls, D2Context(seed=44)).output
        cuda = process_slot(cpu_a.cuda(), cpu_b.cuda(), controls, D2Context(seed=44)).output.cpu()
        torch.testing.assert_close(cpu.float(), cuda.float(), rtol=2e-3, atol=2e-3)


if __name__ == "__main__":
    unittest.main()
