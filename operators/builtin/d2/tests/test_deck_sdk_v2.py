from __future__ import annotations

import hashlib
import json
from dataclasses import replace

import pytest
import torch
from latentdeck_deck_sdk import DeckContractError, DeckOperatorContext, RoleBinding

from latentdeck_operator_d2 import (
    DECK_ID,
    DECK_VERSION,
    D2ContractError,
    process_sources,
)

P1_GOLDEN_SOURCE_COMMIT = "b342a48e88753fe195e01986df7ac99fee607c8a"
P1_GOLDEN_OUTPUT_SHA256 = "ccfffdeab3b917a73327ca18ed5e09e73c0774b9177cc42ed3e52f7902342566"
P1_GOLDEN_PROVENANCE_SHA256 = (
    "33a510742a3868072a9dcdb95b470ce5badb02a6b8fc151f16800c775dae5b3b"
)


def pair(
    *, channels: int = 24, dtype: torch.dtype = torch.float16
) -> tuple[torch.Tensor, torch.Tensor]:
    index = torch.arange(channels * 12, dtype=torch.float32).reshape(1, channels, 1, 3, 4)
    return (
        (torch.sin(index * 0.071) + 0.1 * torch.cos(index * 0.017)).to(dtype),
        (torch.cos(index * 0.043) - 0.15 * torch.sin(index * 0.113)).to(dtype),
    )


def context(
    *,
    codec_family: str = "minimax_h3",
    profile: str = "h3_av_latent",
    profile_version: str = "0.1.0",
    timing_contract: str = "minimax_h3_causal",
    timing_contract_version: str = "0.1.0",
    physical_slots: tuple[int, int] = (1, 2),
    roles: tuple[RoleBinding, ...] = (
        RoleBinding("carrier", 1),
        RoleBinding("donor", 2),
    ),
    playheads: tuple[int, int] = (17, 4),
    previous_sources: tuple[object | None, object | None] = (None, None),
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
    assert (DECK_ID, DECK_VERSION) == ("org.latentdeck.deck.d2", "0.2.1")


def test_seeded_h3_profile_uses_the_authoritative_entrypoint_deterministically() -> None:
    a, b = pair()
    previous_a = torch.roll(a, shifts=1, dims=1) * 0.75
    previous_b = torch.roll(b, shifts=-1, dims=1) * 0.65
    controls = {
        "algorithm": "XS3",
        "mix": 0.41,
        "interaction": 0.9,
        "preserve": 0.32,
        "chaos": 0.2,
    }
    current = process_sources(
        (a, b),
        controls,
        context(previous_sources=(previous_a, previous_b)),
    )
    repeated = process_sources(
        (a, b),
        controls,
        context(previous_sources=(previous_a, previous_b)),
    )

    assert torch.equal(current.output, repeated.output)
    assert current.provenance == repeated.provenance
    assert hashlib.sha256(current.output.numpy().tobytes()).hexdigest() == (
        "f315e26583aaafc777e38985da1d5f7be34a4c8799f83d0ee2b80531b704a01d"
    )
    assert hashlib.sha256(
        json.dumps(current.provenance, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest() == "96309bed8d53efe3c247ebea6b673cfe63b4b5e608d389f00cd40327e4b8845a"


def test_seeded_protocol2_matches_the_immutable_protocol1_golden_trace() -> None:
    """Keep P1 tensor, status, and semantic provenance parity after deleting P1 runtime code."""

    assert len(P1_GOLDEN_SOURCE_COMMIT) == 40
    a, b = pair()
    previous_a = (torch.roll(a, shifts=1, dims=1) * 0.75).contiguous()
    previous_b = (torch.roll(b, shifts=-1, dims=1) * 0.65).contiguous()
    result = process_sources(
        (a, b),
        {
            "algorithm": "XS3",
            "mix": 0.41,
            "interaction": 0.9,
            "preserve": 0.32,
            "mode": "INTERACT",
            "routing": "B",
            "xs3_high_gain": 0.63,
            "chaos": 0.2,
        },
        context(
            roles=(RoleBinding("carrier", 2), RoleBinding("donor", 1)),
            previous_sources=(previous_a, previous_b),
        ),
    )

    assert hashlib.sha256(result.output.numpy().tobytes()).hexdigest() == P1_GOLDEN_OUTPUT_SHA256
    assert result.output.shape == (1, 24, 1, 3, 4)
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


def test_physical_slot_permutation_also_permutates_history_before_role_routing() -> None:
    a, b = pair()
    previous_a = torch.roll(a, shifts=1, dims=-1)
    previous_b = torch.roll(b, shifts=1, dims=-2)
    controls = {"algorithm": "XS3", "interaction": 1.0, "chaos": 0.15}
    reference = process_sources(
        (a, b),
        controls,
        context(
            roles=(RoleBinding("carrier", 2), RoleBinding("donor", 1)),
            playheads=(9, 5),
            previous_sources=(previous_a, previous_b),
            seed=77,
        ),
    )
    current = process_sources(
        (b, a),
        controls,
        context(
            physical_slots=(2, 1),
            roles=(RoleBinding("carrier", 2), RoleBinding("donor", 1)),
            playheads=(5, 9),
            previous_sources=(previous_b, previous_a),
            seed=77,
        ),
    )

    assert torch.equal(current.output, reference.output)
    assert current.provenance == reference.provenance


def test_non_h3_profile_uses_the_same_entrypoint_without_profile_fallback() -> None:
    sources = pair(channels=8, dtype=torch.float32)
    originals = tuple(source.clone() for source in sources)
    synthetic = context(
        codec_family="synthetic",
        profile="test_latent",
        timing_contract="synthetic_causal",
        roles=(RoleBinding("carrier", 2), RoleBinding("donor", 1)),
        seed=9,
    )
    result = process_sources(
        sources,
        {
            "algorithm": "xs5",
            "xs5_routing": "topk",
            "top_k": 4,
            "interaction": 0.8,
            "chaos": 0.25,
        },
        synthetic,
    )
    repeated = process_sources(
        sources,
        {
            "algorithm": "xs5",
            "xs5_routing": "topk",
            "top_k": 4,
            "interaction": 0.8,
            "chaos": 0.25,
        },
        synthetic,
    )

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
    assert result.provenance["structural_carrier"] == "B"


def test_roles_are_closed_and_legacy_routing_cannot_override_them() -> None:
    sources = pair()
    with pytest.raises(D2ContractError, match="context.roles"):
        process_sources(
            sources,
            {},
            context(roles=(RoleBinding("carrier", 1), RoleBinding("other", 2))),
        )
    with pytest.raises(D2ContractError, match="context.role_conflict"):
        process_sources(sources, {"routing": "B"}, context())


def test_invalid_history_is_rejected_without_reindex_or_repair() -> None:
    sources = pair()
    invalid_previous = torch.zeros((1, 24, 1, 2, 4), dtype=torch.float16)
    with pytest.raises(DeckContractError, match="tensor.previous_incompatible"):
        process_sources(
            sources,
            {"algorithm": "XS3", "interaction": 1.0},
            replace(context(), previous_sources=(invalid_previous, None)),
        )


def test_non_contiguous_sources_are_rejected_without_copy_or_repair() -> None:
    sources = tuple(source.transpose(-1, -2) for source in pair())
    with pytest.raises(DeckContractError, match="tensor.non_contiguous"):
        process_sources(sources, {}, context())
