from __future__ import annotations

import copy

import pytest
import torch

from latentdeck_comfy_toolkit import (
    OperatorContext,
    ToolkitContractError,
    ToolkitOperatorResult,
    TrustedOperatorRegistry,
    get_operator_descriptor_schema,
)


def descriptor() -> dict[str, object]:
    return {
        "schema_version": "0.1.0",
        "operator_id": "org.example.explicit_blend",
        "operator_version": "0.1.0",
        "trust": "explicit_install",
        "entrypoint": "example_explicit_blend:process_slot",
        "supported_profiles": [
            {
                "codec_family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_version": "0.1.0",
                "timing_contract": "minimax_h3_causal",
                "timing_contract_version": "0.1.0",
                "layout": "[1,24,1,H,W]",
                "runtime_dtype": "F16",
            }
        ],
        "controls": {"amount": {"type": "float", "default": 0.5, "minimum": 0.0, "maximum": 1.0}},
        "limits": {"max_spatial_tokens": 4096},
    }


def synthetic_pair() -> tuple[torch.Tensor, torch.Tensor]:
    index = torch.arange(24 * 3 * 4, dtype=torch.float32).reshape(1, 24, 1, 3, 4)
    return torch.sin(index * 0.03).half(), torch.cos(index * 0.05).half()


def test_external_operator_runs_only_after_an_explicit_callable_install() -> None:
    registry = TrustedOperatorRegistry()
    with pytest.raises(ToolkitContractError) as missing:
        registry.load("org.example.explicit_blend", "0.1.0")
    assert missing.value.code == "operator.not_installed"

    def process_slot(
        carrier: torch.Tensor,
        donor: torch.Tensor,
        controls: dict[str, object],
        context: OperatorContext,
    ) -> ToolkitOperatorResult:
        amount = float(controls["amount"])
        output = torch.lerp(carrier.float(), donor.float(), amount).half().contiguous()
        return ToolkitOperatorResult(
            output,
            {
                "operation": {
                    "operator_id": "org.example.explicit_blend",
                    "operator_version": "0.1.0",
                    "seed": context.seed,
                    "controls": controls,
                }
            },
        )

    registry.install(
        descriptor(),
        process_slot,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    loaded = registry.load("org.example.explicit_blend", "0.1.0")
    a, b = synthetic_pair()
    result = loaded.process_slot(a, b, {"amount": 0.25}, OperatorContext(seed=77))

    assert torch.equal(result.output, torch.lerp(a.float(), b.float(), 0.25).half())
    assert result.provenance["operation"]["seed"] == 77
    assert registry.descriptors()[0].trust == "explicit_install"


def test_install_manifest_is_closed_and_cannot_supply_or_fetch_code() -> None:
    registry = TrustedOperatorRegistry()
    malicious = copy.deepcopy(descriptor())
    malicious["download_url"] = "https://example.invalid/operator.py"

    with pytest.raises(ToolkitContractError) as caught:
        registry.install(
            malicious,
            lambda *_args: None,
            exported_entrypoint="example_explicit_blend:process_slot",
        )

    assert caught.value.code == "operator.descriptor_invalid"
    assert registry.descriptors() == ()


def test_operator_descriptor_schema_is_closed_and_explicit_install_only() -> None:
    schema = get_operator_descriptor_schema()

    assert schema["additionalProperties"] is False
    assert schema["properties"]["trust"] == {"const": "explicit_install"}
    assert schema["properties"]["entrypoint"]["pattern"].startswith("^")
    controls_schema = schema["properties"]["controls"]
    assert controls_schema["propertyNames"]["pattern"].startswith("^")
    numeric_variants = controls_schema["additionalProperties"]["oneOf"]
    integer_variant = next(
        item for item in numeric_variants if item["properties"]["type"].get("const") == "integer"
    )
    assert integer_variant["properties"]["default"] == {"type": "integer"}


def test_runtime_controls_report_control_errors_not_descriptor_errors() -> None:
    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        lambda *_args: None,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    loaded = registry.load("org.example.explicit_blend", "0.1.0")
    a, b = synthetic_pair()

    with pytest.raises(ToolkitContractError) as caught:
        loaded.process_slot(a, b, {"amount": float("nan")})

    assert caught.value.code == "operator.control_invalid"

    with pytest.raises(ToolkitContractError) as huge_runtime:
        loaded.process_slot(a, b, {"amount": 10**1000})
    assert huge_runtime.value.code == "operator.control_invalid"


def test_descriptor_rejects_numeric_overflow_and_invalid_unicode_stably() -> None:
    huge = copy.deepcopy(descriptor())
    huge["controls"]["amount"]["default"] = 10**1000
    surrogate = copy.deepcopy(descriptor())
    surrogate["operator_id"] = "\ud800"

    for invalid in (huge, surrogate):
        with pytest.raises(ToolkitContractError) as caught:
            TrustedOperatorRegistry().install(
                invalid,
                lambda *_args: None,
                exported_entrypoint="example_explicit_blend:process_slot",
            )
        assert caught.value.code == "operator.descriptor_invalid"


def test_descriptor_rejects_entrypoint_strings_that_are_not_python_modules() -> None:
    invalid = copy.deepcopy(descriptor())
    invalid["entrypoint"] = "example..operator:run"

    with pytest.raises(ToolkitContractError) as caught:
        TrustedOperatorRegistry().install(
            invalid,
            lambda *_args: None,
            exported_entrypoint="example..operator:run",
        )

    assert caught.value.code == "operator.descriptor_invalid"
