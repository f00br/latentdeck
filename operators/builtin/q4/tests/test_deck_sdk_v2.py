from __future__ import annotations

import hashlib
import json
from dataclasses import replace

import pytest
import torch
from latentdeck_deck_sdk import DeckContractError, DeckOperatorContext, RoleBinding

from latentdeck_operator_q4 import (
    DECK_ID,
    DECK_VERSION,
    Q4ContractError,
    process_sources,
)

P1_GOLDEN_SOURCE_COMMIT = "b342a48e88753fe195e01986df7ac99fee607c8a"
P1_GOLDEN_OUTPUT_SHA256 = "b775a5e3887f4e0188469f7315b25855422b5a01ca24a99319caa6da58e34dfc"
P1_GOLDEN_PROVENANCE_SHA256 = (
    "b8ebdb63d0c2642836a00eda094edcdf131c76fa39364b3eb992f1e60dc57489"
)


def quad(
    *, channels: int = 24, dtype: torch.dtype = torch.float16
) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor, torch.Tensor]:
    index = torch.arange(channels * 12, dtype=torch.float32).reshape(1, channels, 1, 3, 4)
    return tuple(
        (torch.sin(index * scale) + 0.1 * torch.cos(index * (scale + 0.013))).to(dtype)
        for scale in (0.017, 0.031, 0.047, 0.071)
    )  # type: ignore[return-value]


def context(
    *,
    codec_family: str = "minimax_h3",
    profile: str = "h3_av_latent",
    profile_version: str = "0.1.0",
    timing_contract: str = "minimax_h3_causal",
    timing_contract_version: str = "0.1.0",
    physical_slots: tuple[int, int, int, int] = (1, 2, 3, 4),
    roles: tuple[RoleBinding, ...] = (
        RoleBinding("carrier", 1),
        RoleBinding("donor_b", 2),
        RoleBinding("donor_c", 3),
        RoleBinding("donor_d", 4),
    ),
    playheads: tuple[int, int, int, int] = (31, 7, 13, 2),
    previous_sources: tuple[object | None, ...] = (None, None, None, None),
    seed: int = 8128,
) -> DeckOperatorContext:
    return DeckOperatorContext(
        codec_family=codec_family,
        profile=profile,
        profile_version=profile_version,
        timing_contract=timing_contract,
        timing_contract_version=timing_contract_version,
        frame_rate_numerator=24,
        frame_rate_denominator=1,
        generation=3,
        sequence=11,
        seed=seed,
        playheads=playheads,
        physical_slots=physical_slots,
        roles=roles,
        previous_sources=previous_sources,
    )


def test_bundled_deck_identity_is_exact_and_versioned_side_by_side() -> None:
    assert (DECK_ID, DECK_VERSION) == ("org.latentdeck.deck.q4", "0.2.0")


def test_seeded_h3_profile_uses_the_authoritative_entrypoint_deterministically() -> None:
    sources = quad()
    controls = {
        "algorithm": "XS5",
        "xs5_routing": "TOPK",
        "top_k": 4,
        "interaction": 0.8,
        "preserve": 0.3,
        "chaos": 0.2,
    }
    current = process_sources(sources, controls, context())
    repeated = process_sources(sources, controls, context())

    assert torch.equal(current.output, repeated.output)
    assert current.provenance == repeated.provenance
    assert hashlib.sha256(current.output.numpy().tobytes()).hexdigest() == (
        "5db87efabc8829ccd672d42a71871070dd7de64c7ce87d44281f850ec7d4689d"
    )
    assert hashlib.sha256(
        json.dumps(current.provenance, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest() == "7f38a3be30a908f89a5a95b966be9dd15b74b550941ad7c8fe8bb6dd350101e0"


def test_seeded_protocol2_matches_the_immutable_protocol1_golden_trace() -> None:
    """Keep P1 tensor, status, and semantic provenance parity after deleting P1 runtime code."""

    assert len(P1_GOLDEN_SOURCE_COMMIT) == 40
    index = torch.arange(24 * 2 * 3, dtype=torch.float32).reshape(1, 24, 1, 2, 3)
    sources = (
        (torch.sin(index * 0.071) + 0.1 * torch.cos(index * 0.017)).half().contiguous(),
        (torch.cos(index * 0.043) - 0.15 * torch.sin(index * 0.113)).half().contiguous(),
        (torch.sin(index * 0.031 + 0.4) + 0.2 * torch.cos(index * 0.089))
        .half()
        .contiguous(),
        (torch.cos(index * 0.097 - 0.2) - 0.1 * torch.sin(index * 0.053))
        .half()
        .contiguous(),
    )
    result = process_sources(
        sources,
        {
            "algorithm": "XS5",
            "interaction": 0.82,
            "preserve": 0.27,
            "mode": "HYBRIDIZE",
            "influence_mode": "MANUAL",
            "donor_weight_b": 0.2,
            "donor_weight_c": 0.3,
            "donor_weight_d": 0.5,
            "xs5_routing": "SINKHORN",
            "temperature": 0.17,
            "top_k": 3,
            "sinkhorn_iterations": 4,
            "chaos": 0.15,
        },
        context(playheads=(5, 7, 11, 13), seed=4242),
    )

    assert hashlib.sha256(result.output.numpy().tobytes()).hexdigest() == P1_GOLDEN_OUTPUT_SHA256
    assert result.output.shape == (1, 24, 1, 2, 3)
    assert result.output.dtype == torch.float16
    assert result.output.device.type == "cpu"
    assert result.output.is_contiguous()
    assert bool(torch.isfinite(result.output).all().item())
    assert result.provenance["operation"]["operator_version"] == "0.2.0"
    semantic_provenance = {
        **result.provenance,
        "operation": {
            key: value
            for key, value in result.provenance["operation"].items()
            if key != "operator_version"
        },
    }
    assert hashlib.sha256(
        json.dumps(semantic_provenance, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest() == P1_GOLDEN_PROVENANCE_SHA256


def test_role_binding_and_physical_source_order_are_independent() -> None:
    a, b, c, d = quad()
    controls = {
        "algorithm": "LINEAR",
        "interaction": 0.75,
        "donor_weight_b": 0.2,
        "donor_weight_c": 0.3,
        "donor_weight_d": 0.5,
        "chaos": 0.1,
    }
    reference = process_sources(
        (a, b, c, d),
        controls,
        context(
            roles=(
                RoleBinding("carrier", 3),
                RoleBinding("donor_b", 1),
                RoleBinding("donor_c", 4),
                RoleBinding("donor_d", 2),
            ),
            seed=99,
        ),
    )
    current = process_sources(
        (c, a, d, b),
        controls,
        context(
            physical_slots=(3, 1, 4, 2),
            roles=(
                RoleBinding("carrier", 3),
                RoleBinding("donor_b", 1),
                RoleBinding("donor_c", 4),
                RoleBinding("donor_d", 2),
            ),
            playheads=(13, 31, 2, 7),
            seed=99,
        ),
    )

    assert torch.equal(current.output, reference.output)
    assert current.provenance == reference.provenance


def test_non_h3_profile_uses_generic_channels_and_dtype_without_fallback() -> None:
    sources = quad(channels=8, dtype=torch.float32)
    originals = tuple(source.clone() for source in sources)
    controls = {
        "algorithm": "xs5",
        "xs5_routing": "topk",
        "top_k": 4,
        "interaction": 0.8,
        "chaos": 0.25,
    }
    synthetic = context(
        codec_family="synthetic",
        profile="test_latent",
        timing_contract="synthetic_causal",
        seed=17,
    )
    result = process_sources(
        sources,
        controls,
        synthetic,
    )
    repeated = process_sources(sources, controls, synthetic)

    assert result.output.shape == sources[0].shape
    assert result.output.dtype == torch.float32
    assert result.output.device == sources[0].device
    assert result.output.is_contiguous()
    assert torch.equal(result.output, repeated.output)
    assert result.provenance == repeated.provenance
    assert all(
        torch.equal(source, original) for source, original in zip(sources, originals, strict=True)
    )
    assert result.provenance["profile"]["codec_family"] == "synthetic"


def test_roles_are_a_closed_four_role_permutation() -> None:
    with pytest.raises(Q4ContractError, match="context.roles"):
        process_sources(
            quad(),
            {},
            context(
                roles=(
                    RoleBinding("carrier", 1),
                    RoleBinding("donor_b", 2),
                    RoleBinding("donor_c", 3),
                    RoleBinding("other", 4),
                )
            ),
        )


def test_invalid_previous_source_is_rejected_even_for_stateless_q4() -> None:
    invalid_previous = torch.zeros((1, 24, 1, 2, 4), dtype=torch.float16)
    with pytest.raises(DeckContractError, match="tensor.previous_incompatible"):
        process_sources(
            quad(),
            {},
            replace(context(), previous_sources=(invalid_previous, None, None, None)),
        )


def test_non_contiguous_sources_are_rejected_without_copy_or_repair() -> None:
    sources = tuple(source.transpose(-1, -2) for source in quad())
    with pytest.raises(DeckContractError, match="tensor.non_contiguous"):
        process_sources(sources, {}, context())
