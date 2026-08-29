from __future__ import annotations

import json
import os
import unittest

import torch
from latentdeck_codec_host.operator_api import validate_descriptor

from latentdeck_operator_q4 import (
    MAX_SPATIAL_TOKENS,
    get_descriptor,
    get_descriptor_schema,
    process_slot,
    triangular_influence_weights,
)


def synthetic_quad(
    *, height: int = 2, width: int = 3, device: str = "cpu"
) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
    count = 24 * height * width
    index = torch.arange(count, dtype=torch.float32, device=device).reshape(1, 24, 1, height, width)
    carrier = (torch.sin(index * 0.071) + 0.1 * torch.cos(index * 0.017)).to(torch.float16)
    donor_b = (torch.cos(index * 0.043) - 0.15 * torch.sin(index * 0.113)).to(torch.float16)
    donor_c = (torch.sin(index * 0.031 + 0.4) + 0.2 * torch.cos(index * 0.089)).to(torch.float16)
    donor_d = (torch.cos(index * 0.097 - 0.2) - 0.1 * torch.sin(index * 0.053)).to(torch.float16)
    return carrier, donor_b, donor_c, donor_d


class Q4OperatorTests(unittest.TestCase):
    def test_descriptor_is_closed_machine_readable_and_defensive(self) -> None:
        descriptor = get_descriptor()
        schema = get_descriptor_schema()
        self.assertEqual(
            set(descriptor),
            {
                "schema_version",
                "operator_id",
                "operator_version",
                "trust",
                "entrypoint",
                "supported_profiles",
                "algorithms",
                "controls",
                "limits",
            },
        )
        self.assertEqual(descriptor["schema_version"], "0.1.0")
        self.assertEqual(descriptor["operator_id"], "org.latentdeck.builtin.ld_q4")
        self.assertEqual(descriptor["entrypoint"], "latentdeck_operator_q4:process_slot")
        self.assertEqual(descriptor["algorithms"], ["LINEAR", "XS5"])
        self.assertEqual(descriptor["limits"]["max_spatial_tokens"], MAX_SPATIAL_TOKENS)
        self.assertFalse(schema["additionalProperties"])
        self.assertEqual(set(schema["required"]), set(descriptor))
        self.assertEqual(set(schema["properties"]), set(descriptor))
        self.assertEqual(schema["properties"]["operator_id"]["const"], descriptor["operator_id"])
        self.assertEqual(validate_descriptor(descriptor).operator_id, descriptor["operator_id"])
        with self.assertRaisesRegex(ValueError, "operator.descriptor_invalid"):
            validate_descriptor({**descriptor, "topology": {"carrier_count": 1}})
        descriptor["algorithms"].clear()
        self.assertEqual(get_descriptor()["algorithms"], ["LINEAR", "XS5"])

    def test_triangular_influence_vertices_and_center_map_to_weights(self) -> None:
        self.assertEqual(triangular_influence_weights(0.0, 0.0), (1.0, 0.0, 0.0))
        self.assertEqual(triangular_influence_weights(1.0, 0.0), (0.0, 1.0, 0.0))
        self.assertEqual(triangular_influence_weights(0.5, 1.0), (0.0, 0.0, 1.0))
        center = triangular_influence_weights(0.5, 1.0 / 3.0)
        for weight in center:
            self.assertAlmostEqual(weight, 1.0 / 3.0)

    def test_triangular_influence_is_a_macro_over_donor_weights(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()

        result = process_slot(
            carrier,
            donor_b,
            donor_c,
            donor_d,
            {
                "algorithm": "LINEAR",
                "interaction": 1.0,
                "influence_mode": "TRIANGLE",
                "triangle_x": 0.5,
                "triangle_y": 1.0,
                "donor_weight_b": 0.8,
                "donor_weight_c": 0.1,
                "donor_weight_d": 0.1,
            },
        )

        self.assertTrue(torch.equal(result.output, donor_d))
        self.assertEqual(
            result.provenance["resolved_donor_weights"], {"B": 0.0, "C": 0.0, "D": 1.0}
        )
        self.assertEqual(result.provenance["influence_mode"], "TRIANGLE")

    def test_linear_uses_interaction_as_total_donor_strength(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        controls = {
            "algorithm": "LINEAR",
            "interaction": 0.25,
            "donor_weight_b": 0.25,
            "donor_weight_c": 0.5,
            "donor_weight_d": 0.25,
        }

        result = process_slot(carrier, donor_b, donor_c, donor_d, controls)

        donor_mix = 0.25 * donor_b.float() + 0.5 * donor_c.float() + 0.25 * donor_d.float()
        expected = torch.lerp(carrier.float(), donor_mix, 0.25).to(torch.float16)
        self.assertTrue(torch.equal(result.output, expected))
        self.assertEqual(
            result.provenance["resolved_donor_weights"], {"B": 0.25, "C": 0.5, "D": 0.25}
        )

    def test_zero_interaction_is_exact_and_inputs_remain_immutable(self) -> None:
        inputs = synthetic_quad()
        originals = tuple(tensor.clone() for tensor in inputs)

        for algorithm in ("LINEAR", "XS5"):
            with self.subTest(algorithm=algorithm):
                controls = {"algorithm": algorithm, "interaction": 0.0, "top_k": 3, "chaos": 0.0}
                result = process_slot(*inputs, controls)
                self.assertTrue(torch.equal(result.output, inputs[0]))
                self.assertNotEqual(result.output.data_ptr(), inputs[0].data_ptr())

        for current, original in zip(inputs, originals, strict=True):
            self.assertTrue(torch.equal(current, original))

    def test_proportional_manual_weights_have_identical_distribution(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        common = {"algorithm": "LINEAR", "interaction": 0.7}
        small = process_slot(
            carrier,
            donor_b,
            donor_c,
            donor_d,
            {
                **common,
                "donor_weight_b": 0.1,
                "donor_weight_c": 0.2,
                "donor_weight_d": 0.1,
            },
        )
        large = process_slot(
            carrier,
            donor_b,
            donor_c,
            donor_d,
            {
                **common,
                "donor_weight_b": 0.25,
                "donor_weight_c": 0.5,
                "donor_weight_d": 0.25,
            },
        )
        self.assertTrue(torch.equal(small.output, large.output))

    def test_rejects_non_f16_runtime_inputs(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()

        with self.assertRaises(ValueError) as caught:
            process_slot(carrier.float(), donor_b, donor_c, donor_d)

        self.assertEqual(caught.exception.code, "tensor.dtype")

    def test_rejects_incompatible_h3_slot_layouts(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()

        with self.assertRaises(ValueError) as caught:
            process_slot(carrier[:, :23], donor_b[:, :23], donor_c[:, :23], donor_d[:, :23])
        self.assertEqual(caught.exception.code, "tensor.shape")

        with self.assertRaises(ValueError) as caught:
            process_slot(carrier, donor_b[..., :2], donor_c, donor_d)
        self.assertEqual(caught.exception.code, "tensor.incompatible_shape")

    def test_rejects_nan_and_inf_before_processing(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()

        for value in (float("nan"), float("inf")):
            with self.subTest(value=value):
                damaged = donor_c.clone()
                damaged[0, 0, 0, 0, 0] = value
                with self.assertRaises(ValueError) as caught:
                    process_slot(carrier, donor_b, damaged, donor_d)
                self.assertEqual(caught.exception.code, "tensor.non_finite")

    def test_controls_reject_unknown_out_of_range_and_zero_distribution(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        cases = [
            ({"interaction": 1.01}, "control.out_of_range"),
            ({"donor_weight_b": float("nan")}, "control.non_finite"),
            ({"unknown_knob": 0.0}, "control.unknown"),
            (
                {"donor_weight_b": 0.0, "donor_weight_c": 0.0, "donor_weight_d": 0.0},
                "control.zero_distribution",
            ),
        ]

        for controls, code in cases:
            with self.subTest(controls=controls):
                with self.assertRaises(ValueError) as caught:
                    process_slot(carrier, donor_b, donor_c, donor_d, controls)
                self.assertEqual(caught.exception.code, code)

    def test_triangle_and_xs5_bounds_reject_without_clamping_or_fallback(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad(height=2, width=3)
        cases = [
            (
                {"influence_mode": "TRIANGLE", "triangle_x": 0.1, "triangle_y": 0.9},
                "control.outside_triangle",
            ),
            ({"algorithm": "XS5", "top_k": 7}, "control.out_of_range"),
            ({"sinkhorn_iterations": 13}, "control.out_of_range"),
        ]
        for controls, code in cases:
            with self.subTest(controls=controls):
                with self.assertRaises(ValueError) as caught:
                    process_slot(carrier, donor_b, donor_c, donor_d, controls)
                self.assertEqual(caught.exception.code, code)

        too_large = torch.zeros(
            (1, 24, 1, 1, MAX_SPATIAL_TOKENS + 1),
            dtype=torch.float16,
        )
        with self.assertRaises(ValueError) as caught:
            process_slot(too_large, too_large, too_large, too_large)
        self.assertEqual(caught.exception.code, "tensor.too_large")

    def test_xs5_topk_is_a_distinct_finite_full_grid_path(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad(height=3, width=4)
        controls = {
            "algorithm": "XS5",
            "xs5_routing": "TOPK",
            "top_k": 4,
            "temperature": 0.11,
            "interaction": 0.8,
            "preserve": 0.25,
            "donor_weight_b": 0.2,
            "donor_weight_c": 0.3,
            "donor_weight_d": 0.5,
        }

        routed = process_slot(carrier, donor_b, donor_c, donor_d, controls)
        linear = process_slot(
            carrier, donor_b, donor_c, donor_d, {**controls, "algorithm": "LINEAR"}
        )

        self.assertFalse(torch.equal(routed.output, linear.output))
        self.assertEqual(routed.output.shape, carrier.shape)
        self.assertEqual(routed.output.dtype, torch.float16)
        self.assertTrue(bool(torch.isfinite(routed.output).all().item()))
        self.assertEqual(
            routed.provenance["routing"],
            {
                "method": "TOPK",
                "reference": "UNCHANGED_CARRIER",
                "carrier_affinity_reused": True,
                "donor_batch_size": 3,
                "accumulation_order": ["B", "C", "D"],
            },
        )
        self.assertEqual(routed.provenance["grid"]["tokens"], 12)
        self.assertTrue(routed.provenance["grid"]["full"])

    def test_xs5_sinkhorn_is_bounded_deterministic_and_distinct_from_topk(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad(height=3, width=3)
        common = {
            "algorithm": "XS5",
            "interaction": 0.9,
            "preserve": 0.3,
            "temperature": 0.09,
            "top_k": 3,
            "sinkhorn_iterations": 4,
        }

        first = process_slot(
            carrier, donor_b, donor_c, donor_d, {**common, "xs5_routing": "SINKHORN"}
        )
        repeated = process_slot(
            carrier, donor_b, donor_c, donor_d, {**common, "xs5_routing": "SINKHORN"}
        )
        topk = process_slot(carrier, donor_b, donor_c, donor_d, {**common, "xs5_routing": "TOPK"})

        self.assertTrue(torch.equal(first.output, repeated.output))
        self.assertFalse(torch.equal(first.output, topk.output))
        self.assertTrue(bool(torch.isfinite(first.output).all().item()))
        self.assertEqual(first.provenance["routing"]["method"], "SINKHORN")

    def test_xs5_routes_the_complete_release_grid_without_downscaling(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad(height=50, width=28)
        common = {
            "algorithm": "XS5",
            "interaction": 0.7,
            "preserve": 0.4,
            "temperature": 0.12,
            "top_k": 8,
            "sinkhorn_iterations": 2,
        }

        for routing in ("TOPK", "SINKHORN"):
            with self.subTest(routing=routing):
                result = process_slot(
                    carrier,
                    donor_b,
                    donor_c,
                    donor_d,
                    {**common, "xs5_routing": routing},
                )
                self.assertEqual(result.output.shape, (1, 24, 1, 50, 28))
                self.assertEqual(result.provenance["grid"]["tokens"], 1_400)
                self.assertTrue(result.provenance["grid"]["full"])

    def test_each_donor_routes_from_the_same_unchanged_carrier(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad(height=3, width=3)
        common = {
            "algorithm": "XS5",
            "xs5_routing": "TOPK",
            "top_k": 3,
            "interaction": 1.0,
            "preserve": 0.2,
        }

        def render(weights: tuple[float, float, float]):
            return process_slot(
                carrier,
                donor_b,
                donor_c,
                donor_d,
                {
                    **common,
                    "donor_weight_b": weights[0],
                    "donor_weight_c": weights[1],
                    "donor_weight_d": weights[2],
                },
            )

        combined = render((0.2, 0.3, 0.5))
        b_only = render((1.0, 0.0, 0.0)).output
        c_only = render((0.0, 1.0, 0.0)).output
        d_only = render((0.0, 0.0, 1.0)).output
        expected = carrier.float().clone()
        for weight, donor_only in ((0.2, b_only), (0.3, c_only), (0.5, d_only)):
            expected.add_(donor_only.float() - carrier.float(), alpha=weight)

        torch.testing.assert_close(combined.output.float(), expected, rtol=2e-3, atol=2e-3)
        self.assertEqual(combined.provenance["routing"]["reference"], "UNCHANGED_CARRIER")

    def test_provenance_records_explicit_carrier_donor_identities_and_playheads(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        context = {
            "carrier_slot": "C",
            "donor_b_slot": "A",
            "donor_c_slot": "B",
            "donor_d_slot": "D",
            "carrier_identity": "cartridge-c",
            "donor_b_identity": "cartridge-a",
            "donor_c_identity": "cartridge-b",
            "donor_d_identity": "cartridge-d",
            "carrier_playhead": 31,
            "donor_b_playhead": 7,
            "donor_c_playhead": 13,
            "donor_d_playhead": 2,
            "seed": 8128,
        }

        result = process_slot(carrier, donor_b, donor_c, donor_d, context=context)

        self.assertEqual(
            result.provenance["roles"],
            {
                "carrier": {"slot": "C", "identity": "cartridge-c", "playhead": 31},
                "donors": [
                    {"role": "B", "slot": "A", "identity": "cartridge-a", "playhead": 7},
                    {"role": "C", "slot": "B", "identity": "cartridge-b", "playhead": 13},
                    {"role": "D", "slot": "D", "identity": "cartridge-d", "playhead": 2},
                ],
            },
        )
        self.assertEqual(result.provenance["operation"]["seed"], 8128)
        json.dumps(result.provenance, allow_nan=False)

    def test_profile_timing_and_distinct_slot_roles_are_strict(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        cases = [
            ({"profile_version": "0.2.0"}, "profile.incompatible"),
            ({"frame_rate_numerator": 30}, "timing.incompatible"),
            ({"carrier_slot": "A", "donor_b_slot": "A"}, "context.slot"),
            ({"carrier_identity": "not/a/path"}, "context.identity"),
        ]
        for context, code in cases:
            with self.subTest(context=context):
                with self.assertRaises(ValueError) as caught:
                    process_slot(carrier, donor_b, donor_c, donor_d, context=context)
                self.assertEqual(caught.exception.code, code)

    def test_seeded_chaos_is_stateless_and_zero_is_an_exact_bypass(self) -> None:
        carrier, donor_b, donor_c, donor_d = synthetic_quad(height=3, width=4)
        controls = {
            "algorithm": "XS5",
            "xs5_routing": "TOPK",
            "top_k": 4,
            "interaction": 0.8,
            "chaos": 0.7,
        }

        first = process_slot(carrier, donor_b, donor_c, donor_d, controls, {"seed": 101})
        repeated = process_slot(carrier, donor_b, donor_c, donor_d, controls, {"seed": 101})
        other_seed = process_slot(carrier, donor_b, donor_c, donor_d, controls, {"seed": 202})
        self.assertTrue(torch.equal(first.output, repeated.output))
        self.assertFalse(torch.equal(first.output, other_seed.output))

        zero_a = process_slot(
            carrier, donor_b, donor_c, donor_d, {**controls, "chaos": 0.0}, {"seed": 101}
        )
        zero_b = process_slot(
            carrier, donor_b, donor_c, donor_d, {**controls, "chaos": 0.0}, {"seed": 202}
        )
        self.assertTrue(torch.equal(zero_a.output, zero_b.output))


@unittest.skipUnless(
    torch.cuda.is_available() and os.environ.get("LATENTDECK_RUN_CUDA_TESTS") == "1",
    "optional CUDA parity tests; set LATENTDECK_RUN_CUDA_TESTS=1 to enable",
)
class OptionalCudaParityTests(unittest.TestCase):
    def test_xs5_topk_and_sinkhorn_cpu_cuda_parity(self) -> None:
        cpu_inputs = synthetic_quad(height=3, width=4)
        common = {
            "algorithm": "XS5",
            "top_k": 4,
            "sinkhorn_iterations": 4,
            "interaction": 0.8,
            "preserve": 0.3,
            "chaos": 0.2,
        }
        for routing in ("TOPK", "SINKHORN"):
            with self.subTest(routing=routing):
                controls = {**common, "xs5_routing": routing}
                cpu = process_slot(*cpu_inputs, controls, {"seed": 44}).output
                cuda_inputs = tuple(tensor.cuda() for tensor in cpu_inputs)
                cuda = process_slot(*cuda_inputs, controls, {"seed": 44}).output.cpu()
                torch.testing.assert_close(cpu.float(), cuda.float(), rtol=2e-3, atol=2e-3)


if __name__ == "__main__":
    unittest.main()
