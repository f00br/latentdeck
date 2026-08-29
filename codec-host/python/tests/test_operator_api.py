from __future__ import annotations

import copy

import pytest
from latentdeck_codec_host.operator_api import (
    BuiltinOperatorRegistry,
    OperatorLoadError,
    validate_descriptor,
)


def descriptor() -> dict[str, object]:
    return {
        "schema_version": "0.1.0",
        "operator_id": "org.latentdeck.builtin.synthetic",
        "operator_version": "0.1.0",
        "trust": "builtin",
        "entrypoint": "package_that_does_not_exist:process",
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
        "algorithms": ["LINEAR"],
        "controls": {
            "mix": {"type": "float", "default": 0.5, "minimum": 0.0, "maximum": 1.0},
            "routing": {"type": "enum", "default": "A", "values": ["A", "B"]},
            "top_k": {"type": "integer", "default": 8, "minimum": 1, "maximum": 64},
        },
        "limits": {"max_spatial_tokens": 4096},
    }


def test_descriptor_is_closed_and_typed() -> None:
    parsed = validate_descriptor(descriptor())
    assert parsed.operator_id == "org.latentdeck.builtin.synthetic"
    assert parsed.supported_profiles[0].runtime_dtype == "F16"
    assert parsed.limit("max_spatial_tokens") == 4096

    invalid = descriptor()
    invalid["installer"] = "payload/operator.py"
    with pytest.raises(OperatorLoadError, match="operator.descriptor_invalid"):
        validate_descriptor(invalid)


def test_registry_uses_explicit_callable_and_never_imports_entrypoint_text() -> None:
    registry = BuiltinOperatorRegistry()
    registry.register(
        descriptor(),
        lambda value: {"trusted": value},
        exported_entrypoint="package_that_does_not_exist:process",
    )
    loaded = registry.load("org.latentdeck.builtin.synthetic", "0.1.0")
    assert loaded.invoke(7) == {"trusted": 7}


def test_registry_rejects_unregistered_version_and_entrypoint() -> None:
    registry = BuiltinOperatorRegistry()
    with pytest.raises(OperatorLoadError, match="operator.entrypoint_mismatch"):
        registry.register(
            descriptor(),
            lambda: None,
            exported_entrypoint="different:callable",
        )
    assert registry.descriptors() == ()

    registry.register(
        descriptor(),
        lambda: None,
        exported_entrypoint="package_that_does_not_exist:process",
    )
    with pytest.raises(OperatorLoadError, match="operator.version_mismatch"):
        registry.load("org.latentdeck.builtin.synthetic", "0.2.0")
    with pytest.raises(OperatorLoadError, match="operator.not_installed"):
        registry.load("org.latentdeck.builtin.from_cartridge", "0.1.0")


def test_untrusted_and_nonfinite_descriptor_values_are_rejected() -> None:
    untrusted = descriptor()
    untrusted["trust"] = "cartridge"
    with pytest.raises(OperatorLoadError, match="operator.not_trusted"):
        validate_descriptor(untrusted)

    nonfinite = copy.deepcopy(descriptor())
    controls = nonfinite["controls"]
    assert isinstance(controls, dict)
    mix = controls["mix"]
    assert isinstance(mix, dict)
    mix["maximum"] = float("nan")
    with pytest.raises(OperatorLoadError, match="operator.descriptor_invalid"):
        validate_descriptor(nonfinite)
