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
        "topology": "dual_source",
        "input_count": 2,
        "capabilities": {
            "full_clip": True,
            "streaming": True,
            "chunk": True,
            "deterministic": True,
        },
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
        "bypass": {"controls": {"amount": 0.0}, "output_source": 0},
        "limits": {"max_spatial_tokens": 4096},
    }


def expanded_descriptor() -> dict[str, object]:
    return descriptor()


def synthetic_pair() -> tuple[torch.Tensor, torch.Tensor]:
    index = torch.arange(24 * 3 * 4, dtype=torch.float32).reshape(1, 24, 1, 3, 4)
    return torch.sin(index * 0.03).half(), torch.cos(index * 0.05).half()


def identity_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    _context: OperatorContext,
) -> ToolkitOperatorResult:
    return ToolkitOperatorResult(
        sources[0].clone().contiguous(),
        {
            "operation": {
                "operator_id": "org.example.explicit_blend",
                "operator_version": "0.1.0",
                "controls": controls,
            }
        },
    )


def test_descriptor_declares_topology_capabilities_and_explicit_bypass() -> None:
    registry = TrustedOperatorRegistry()

    registry.install(
        expanded_descriptor(),
        identity_sources,
        exported_entrypoint="example_explicit_blend:process_slot",
    )

    parsed = registry.descriptors()[0]
    assert (parsed.topology, parsed.input_count) == ("dual_source", 2)
    assert parsed.capabilities.deterministic is True
    assert parsed.capabilities.streaming is True
    assert parsed.bypass.output_source == 0


def test_descriptor_rejects_missing_topology_contract_fields() -> None:
    incomplete = descriptor()
    incomplete.pop("topology")
    with pytest.raises(ToolkitContractError) as caught:
        TrustedOperatorRegistry().install(
            incomplete,
            lambda *_args: None,
            exported_entrypoint="example_explicit_blend:process_slot",
        )

    assert caught.value.code == "operator.descriptor_invalid"


def test_unified_sources_tuple_runs_a_dual_source_operator() -> None:
    def process_sources(
        sources: tuple[torch.Tensor, ...],
        controls: dict[str, object],
        context: OperatorContext,
    ) -> ToolkitOperatorResult:
        amount = float(controls["amount"])
        output = torch.lerp(sources[0].float(), sources[1].float(), amount).half().contiguous()
        return ToolkitOperatorResult(
            output,
            {
                "operation": {
                    "operator_id": "org.example.explicit_blend",
                    "operator_version": "0.1.0",
                }
            },
        )

    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        process_sources,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    a, b = synthetic_pair()

    result = registry.load("org.example.explicit_blend", "0.1.0").process_sources(
        (a, b), {"amount": 0.25}, OperatorContext(seed=3)
    )

    assert torch.equal(result.output, torch.lerp(a.float(), b.float(), 0.25).half())


def test_single_source_wrapper_enforces_the_declared_topology() -> None:
    single = descriptor()
    single["operator_id"] = "org.example.single"
    single["entrypoint"] = "example_single:process_sources"
    single["topology"] = "single_source"
    single["input_count"] = 1

    def process_sources(
        sources: tuple[torch.Tensor, ...],
        controls: dict[str, object],
        _context: OperatorContext,
    ) -> ToolkitOperatorResult:
        return ToolkitOperatorResult(
            torch.neg(sources[0]).contiguous(),
            {
                "operation": {
                    "operator_id": "org.example.single",
                    "operator_version": "0.1.0",
                    "controls": controls,
                }
            },
        )

    registry = TrustedOperatorRegistry()
    registry.install(
        single,
        process_sources,
        exported_entrypoint="example_single:process_sources",
    )
    source, _ = synthetic_pair()

    result = registry.load("org.example.single", "0.1.0").process_single(source, {"amount": 0.5})

    assert torch.equal(result.output, torch.neg(source))


def test_carrier_donors_wrapper_preserves_the_declared_source_order() -> None:
    quad = descriptor()
    quad["operator_id"] = "org.example.quad"
    quad["entrypoint"] = "example_quad:process_sources"
    quad["topology"] = "carrier_donors"
    quad["input_count"] = 4

    def process_sources(
        sources: tuple[torch.Tensor, ...],
        controls: dict[str, object],
        _context: OperatorContext,
    ) -> ToolkitOperatorResult:
        output = (
            sum(
                source.float() * weight
                for source, weight in zip(sources, (1.0, 2.0, 3.0, 4.0), strict=True)
            )
            .half()
            .contiguous()
        )
        return ToolkitOperatorResult(
            output,
            {
                "operation": {
                    "operator_id": "org.example.quad",
                    "operator_version": "0.1.0",
                    "controls": controls,
                }
            },
        )

    registry = TrustedOperatorRegistry()
    registry.install(
        quad,
        process_sources,
        exported_entrypoint="example_quad:process_sources",
    )
    shape = (1, 24, 1, 2, 3)
    carrier = torch.full(shape, 1.0, dtype=torch.float16)
    donors = tuple(torch.full(shape, value, dtype=torch.float16) for value in (2.0, 3.0, 4.0))

    result = registry.load("org.example.quad", "0.1.0").process_carrier_donors(
        carrier, donors, {"amount": 0.5}
    )

    assert torch.equal(result.output, torch.full(shape, 30.0, dtype=torch.float16))


def test_dual_source_wrapper_keeps_process_slot_compatibility() -> None:
    def process_sources(
        sources: tuple[torch.Tensor, ...],
        controls: dict[str, object],
        _context: OperatorContext,
    ) -> ToolkitOperatorResult:
        output = (
            torch.lerp(sources[0].float(), sources[1].float(), float(controls["amount"]))
            .half()
            .contiguous()
        )
        return ToolkitOperatorResult(
            output,
            {
                "operation": {
                    "operator_id": "org.example.explicit_blend",
                    "operator_version": "0.1.0",
                }
            },
        )

    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        process_sources,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    loaded = registry.load("org.example.explicit_blend", "0.1.0")
    carrier, donor = synthetic_pair()

    dual = loaded.process_dual(carrier, donor, {"amount": 0.25})
    compatibility = loaded.process_slot(carrier, donor, {"amount": 0.25})

    assert torch.equal(dual.output, compatibility.output)
    with pytest.raises(ToolkitContractError) as caught:
        loaded.process_single(carrier, {"amount": 0.25})
    assert caught.value.code == "operator.topology_mismatch"


def test_runtime_owns_the_explicit_exact_bypass_state() -> None:
    def must_not_run(
        _sources: tuple[torch.Tensor, ...],
        _controls: dict[str, object],
        _context: OperatorContext,
    ) -> ToolkitOperatorResult:
        raise AssertionError("descriptor bypass must not invoke operator code")

    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        must_not_run,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    loaded = registry.load("org.example.explicit_blend", "0.1.0")
    carrier, donor = synthetic_pair()

    result = loaded.process_dual(
        carrier, donor, {"amount": 0.0}, OperatorContext(seed=41, slot_index=7)
    )

    assert torch.equal(result.output, carrier)
    assert result.output.data_ptr() != carrier.data_ptr()
    assert result.provenance["operation"]["bypass"] is True
    assert result.provenance["operation"]["controls"] == {"amount": 0.0}


def test_runtime_rejects_a_processing_mode_the_descriptor_does_not_support() -> None:
    full_clip_only = descriptor()
    full_clip_only["capabilities"] = {
        "full_clip": True,
        "streaming": False,
        "chunk": False,
        "deterministic": True,
    }

    def process_sources(
        sources: tuple[torch.Tensor, ...],
        _controls: dict[str, object],
        _context: OperatorContext,
    ) -> ToolkitOperatorResult:
        return ToolkitOperatorResult(
            sources[0].clone().contiguous(),
            {
                "operation": {
                    "operator_id": "org.example.explicit_blend",
                    "operator_version": "0.1.0",
                }
            },
        )

    registry = TrustedOperatorRegistry()
    registry.install(
        full_clip_only,
        process_sources,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    carrier, donor = synthetic_pair()

    with pytest.raises(ToolkitContractError) as caught:
        registry.load("org.example.explicit_blend", "0.1.0").process_dual(
            carrier,
            donor,
            {"amount": 0.5},
            OperatorContext(processing_mode="streaming"),
        )

    assert caught.value.code == "operator.processing_mode_unsupported"


def test_runtime_rejects_a_non_text_processing_mode_stably() -> None:
    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        identity_sources,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    carrier, donor = synthetic_pair()

    with pytest.raises(ToolkitContractError) as caught:
        registry.load("org.example.explicit_blend", "0.1.0").process_dual(
            carrier,
            donor,
            {"amount": 0.5},
            OperatorContext(processing_mode=[]),  # type: ignore[arg-type]
        )

    assert caught.value.code == "operator.context_invalid"


def test_sources_tuple_is_bounded_finite_f16_and_shape_compatible() -> None:
    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        identity_sources,
        exported_entrypoint="example_explicit_blend:process_slot",
    )
    loaded = registry.load("org.example.explicit_blend", "0.1.0")
    carrier, donor = synthetic_pair()
    non_finite = donor.clone()
    non_finite[0, 0, 0, 0, 0] = torch.inf

    cases = (
        ([carrier, donor], "operator.source_count_invalid"),
        ((carrier,), "operator.source_count_invalid"),
        ((carrier, donor.float()), "operator.tensor_invalid"),
        ((carrier, non_finite), "operator.tensor_non_finite"),
        ((carrier, donor[..., :2, :]), "operator.tensor_incompatible"),
    )
    for sources, code in cases:
        with pytest.raises(ToolkitContractError) as caught:
            loaded.process_sources(sources, {"amount": 0.5})  # type: ignore[arg-type]
        assert caught.value.code == code


def test_descriptor_cross_validates_topology_capabilities_and_bypass() -> None:
    invalid_descriptors: list[dict[str, object]] = []

    wrong_count = copy.deepcopy(descriptor())
    wrong_count["input_count"] = 3
    invalid_descriptors.append(wrong_count)

    no_mode = copy.deepcopy(descriptor())
    no_mode["capabilities"] = {
        "full_clip": False,
        "streaming": False,
        "chunk": False,
        "deterministic": True,
    }
    invalid_descriptors.append(no_mode)

    unknown_bypass = copy.deepcopy(descriptor())
    unknown_bypass["bypass"]["controls"] = {"missing": 0.0}
    invalid_descriptors.append(unknown_bypass)

    out_of_range_source = copy.deepcopy(descriptor())
    out_of_range_source["bypass"]["output_source"] = 2
    invalid_descriptors.append(out_of_range_source)

    for invalid in invalid_descriptors:
        with pytest.raises(ToolkitContractError) as caught:
            TrustedOperatorRegistry().install(
                invalid,
                identity_sources,
                exported_entrypoint="example_explicit_blend:process_slot",
            )
        assert caught.value.code == "operator.descriptor_invalid"


def test_install_rejects_an_ambiguous_variadic_callable_contract() -> None:
    with pytest.raises(ToolkitContractError) as caught:
        TrustedOperatorRegistry().install(
            descriptor(),
            lambda *_args: None,
            exported_entrypoint="example_explicit_blend:process_slot",
        )

    assert caught.value.code == "operator.implementation_invalid"


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


def test_machine_schema_requires_the_complete_operator_contract() -> None:
    schema = get_operator_descriptor_schema()

    required = set(schema["required"])
    assert {"topology", "input_count", "capabilities", "bypass"} <= required
    assert schema["properties"]["topology"]["enum"] == [
        "single_source",
        "dual_source",
        "carrier_donors",
    ]
    capabilities = schema["properties"]["capabilities"]
    assert capabilities["additionalProperties"] is False
    assert set(capabilities["required"]) == {
        "full_clip",
        "streaming",
        "chunk",
        "deterministic",
    }
    bypass = schema["properties"]["bypass"]
    assert bypass["additionalProperties"] is False
    assert set(bypass["required"]) == {"controls", "output_source"}


def test_runtime_controls_report_control_errors_not_descriptor_errors() -> None:
    registry = TrustedOperatorRegistry()
    registry.install(
        descriptor(),
        identity_sources,
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
