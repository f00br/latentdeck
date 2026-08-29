from __future__ import annotations

import copy
import unittest

import torch

from latentdeck_operator_q4.contract import ProcessResult
from latentdeck_operator_q4.descriptor import get_descriptor
from latentdeck_operator_q4.trusted import (
    BuiltinOperatorRegistry,
    OperatorLoadError,
    builtin_registry,
    validate_descriptor,
)


def synthetic_quad() -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
    index = torch.arange(24 * 3 * 4, dtype=torch.float32).reshape(1, 24, 1, 3, 4)
    return tuple(torch.sin(index * scale).half() for scale in (0.017, 0.031, 0.047, 0.071))  # type: ignore[return-value]


class TrustedQ4OperatorTests(unittest.TestCase):
    def test_builtin_loads_only_by_its_explicit_id_and_version(self) -> None:
        registry = builtin_registry()
        descriptors = registry.descriptors()
        self.assertEqual(len(descriptors), 1)
        self.assertEqual(descriptors[0].operator_id, "org.latentdeck.builtin.ld_q4")

        loaded = registry.load("org.latentdeck.builtin.ld_q4", "0.1.0")
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        result = loaded.process_slot(
            carrier,
            donor_b,
            donor_c,
            donor_d,
            {"algorithm": "LINEAR"},
        )
        self.assertEqual(result.output.shape, carrier.shape)

        with self.assertRaises(OperatorLoadError) as caught:
            registry.load("org.example.from_cartridge", "0.1.0")
        self.assertEqual(caught.exception.code, "operator.not_installed")

    def test_descriptor_validation_is_closed_and_rejects_untrusted_code(self) -> None:
        descriptor = get_descriptor()
        self.assertEqual(validate_descriptor(descriptor).trust, "builtin")

        cases = []
        unknown = copy.deepcopy(descriptor)
        unknown["download_url"] = "https://example.invalid/operator.py"
        cases.append((unknown, "operator.descriptor_invalid"))
        external = copy.deepcopy(descriptor)
        external["trust"] = "cartridge"
        cases.append((external, "operator.not_trusted"))
        executable = copy.deepcopy(descriptor)
        executable["entrypoint"] = "../../payload:run"
        cases.append((executable, "operator.descriptor_invalid"))
        malformed_control = copy.deepcopy(descriptor)
        malformed_control["controls"]["interaction"]["maximum"] = float("inf")
        cases.append((malformed_control, "operator.descriptor_invalid"))

        for raw, code in cases:
            with self.subTest(code=code):
                with self.assertRaises(OperatorLoadError) as caught:
                    validate_descriptor(raw)
                self.assertEqual(caught.exception.code, code)

    def test_registry_never_imports_the_descriptor_entrypoint(self) -> None:
        registry = BuiltinOperatorRegistry()
        with self.assertRaises(OperatorLoadError) as caught:
            registry.register(
                get_descriptor(),
                lambda carrier, _b, _c, _d, _controls, _context: ProcessResult(carrier, {}),
                exported_entrypoint="some_other_module:run",
            )
        self.assertEqual(caught.exception.code, "operator.entrypoint_mismatch")
        self.assertEqual(registry.descriptors(), ())

    def test_loaded_operator_revalidates_output_and_provenance(self) -> None:
        descriptor = get_descriptor()

        def wrong_output(carrier, _b, _c, _d, _controls, _context):
            return ProcessResult(carrier.float(), {"operation": {}})

        registry = BuiltinOperatorRegistry()
        registry.register(
            descriptor,
            wrong_output,
            exported_entrypoint="latentdeck_operator_q4:process_slot",
        )
        loaded = registry.load("org.latentdeck.builtin.ld_q4", "0.1.0")
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        with self.assertRaises(OperatorLoadError) as caught:
            loaded.process_slot(carrier, donor_b, donor_c, donor_d)
        self.assertEqual(caught.exception.code, "operator.result_invalid")

        def nonfinite_output(carrier, _b, _c, _d, _controls, _context):
            output = carrier.clone()
            output[0, 0, 0, 0, 0] = float("nan")
            return ProcessResult(
                output,
                {
                    "operation": {
                        "operator_id": "org.latentdeck.builtin.ld_q4",
                        "operator_version": "0.1.0",
                    }
                },
            )

        registry = BuiltinOperatorRegistry()
        registry.register(
            descriptor,
            nonfinite_output,
            exported_entrypoint="latentdeck_operator_q4:process_slot",
        )
        loaded = registry.load("org.latentdeck.builtin.ld_q4", "0.1.0")
        with self.assertRaises(OperatorLoadError) as caught:
            loaded.process_slot(carrier, donor_b, donor_c, donor_d)
        self.assertEqual(caught.exception.code, "operator.result_invalid")

    def test_loaded_operator_wraps_unexpected_execution_errors_path_free(self) -> None:
        def fail(*_args):
            raise RuntimeError(r"W:\private\q4-implementation.py")

        registry = BuiltinOperatorRegistry()
        registry.register(
            get_descriptor(),
            fail,
            exported_entrypoint="latentdeck_operator_q4:process_slot",
        )
        loaded = registry.load("org.latentdeck.builtin.ld_q4", "0.1.0")
        carrier, donor_b, donor_c, donor_d = synthetic_quad()
        with self.assertRaises(OperatorLoadError) as caught:
            loaded.process_slot(carrier, donor_b, donor_c, donor_d)
        self.assertEqual(caught.exception.code, "operator.process_failed")
        self.assertNotIn("W:\\private", caught.exception.detail)


if __name__ == "__main__":
    unittest.main()
