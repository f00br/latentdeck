from __future__ import annotations

import copy
import unittest

import torch

from latentdeck_operator_d2 import (
    BuiltinOperatorRegistry,
    OperatorLoadError,
    ProcessResult,
    builtin_registry,
    get_descriptor,
    validate_descriptor,
)


def synthetic_pair() -> tuple[torch.Tensor, torch.Tensor]:
    index = torch.arange(24 * 3 * 4, dtype=torch.float32).reshape(1, 24, 1, 3, 4)
    return torch.sin(index * 0.071).half(), torch.cos(index * 0.043).half()


class TrustedOperatorTests(unittest.TestCase):
    def test_builtin_is_loaded_only_by_explicit_id_and_version(self) -> None:
        registry = builtin_registry()
        descriptors = registry.descriptors()
        self.assertEqual(len(descriptors), 1)
        self.assertEqual(descriptors[0].operator_id, "org.latentdeck.builtin.ld_d2")
        loaded = registry.load("org.latentdeck.builtin.ld_d2", "0.1.0")
        a, b = synthetic_pair()
        result = loaded.process_slot(a, b, {"algorithm": "LINEAR"})
        self.assertEqual(result.output.shape, a.shape)

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
        malformed_control["controls"]["mix"]["maximum"] = float("inf")
        cases.append((malformed_control, "operator.descriptor_invalid"))

        for raw, code in cases:
            with self.subTest(code=code):
                with self.assertRaises(OperatorLoadError) as caught:
                    validate_descriptor(raw)
                self.assertEqual(caught.exception.code, code)

    def test_registry_never_imports_the_descriptor_entrypoint(self) -> None:
        registry = BuiltinOperatorRegistry()
        descriptor = get_descriptor()
        with self.assertRaises(OperatorLoadError) as caught:
            registry.register(
                descriptor,
                lambda a, b, controls, context: ProcessResult(a, {}),
                exported_entrypoint="some_other_module:run",
            )
        self.assertEqual(caught.exception.code, "operator.entrypoint_mismatch")
        self.assertEqual(registry.descriptors(), ())

    def test_loaded_operator_revalidates_output_and_provenance(self) -> None:
        descriptor = get_descriptor()
        descriptor["operator_id"] = "org.latentdeck.builtin.invalid_test"
        descriptor["entrypoint"] = "trusted_test:run"

        def wrong_output(a, _b, _controls, _context):
            return ProcessResult(a.float(), {"operation": {}})

        registry = BuiltinOperatorRegistry()
        registry.register(descriptor, wrong_output, exported_entrypoint="trusted_test:run")
        loaded = registry.load("org.latentdeck.builtin.invalid_test", "0.1.0")
        a, b = synthetic_pair()
        with self.assertRaises(OperatorLoadError) as caught:
            loaded.process_slot(a, b)
        self.assertEqual(caught.exception.code, "operator.result_invalid")

        descriptor["operator_id"] = "org.latentdeck.builtin.nonfinite_test"

        def nonfinite_output(a, _b, _controls, _context):
            output = a.clone()
            output[0, 0, 0, 0, 0] = float("nan")
            return ProcessResult(
                output,
                {
                    "operation": {
                        "operator_id": "org.latentdeck.builtin.nonfinite_test",
                        "operator_version": "0.1.0",
                    }
                },
            )

        registry = BuiltinOperatorRegistry()
        registry.register(descriptor, nonfinite_output, exported_entrypoint="trusted_test:run")
        loaded = registry.load("org.latentdeck.builtin.nonfinite_test", "0.1.0")
        with self.assertRaises(OperatorLoadError) as caught:
            loaded.process_slot(a, b)
        self.assertEqual(caught.exception.code, "operator.result_invalid")

    def test_loaded_operator_wraps_unexpected_execution_errors_path_free(self) -> None:
        descriptor = get_descriptor()
        descriptor["operator_id"] = "org.latentdeck.builtin.failure_test"
        descriptor["entrypoint"] = "trusted_test:fail"

        def fail(*_args):
            raise RuntimeError("machine-local implementation detail")

        registry = BuiltinOperatorRegistry()
        registry.register(descriptor, fail, exported_entrypoint="trusted_test:fail")
        loaded = registry.load("org.latentdeck.builtin.failure_test", "0.1.0")
        a, b = synthetic_pair()
        with self.assertRaises(OperatorLoadError) as caught:
            loaded.process_slot(a, b)
        self.assertEqual(caught.exception.code, "operator.process_failed")
        self.assertNotIn("machine-local", caught.exception.detail)


if __name__ == "__main__":
    unittest.main()
