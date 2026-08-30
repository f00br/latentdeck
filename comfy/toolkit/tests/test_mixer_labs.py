from __future__ import annotations

import torch

from latentdeck_comfy_toolkit.mixer_labs import (
    dual_mixer_lab,
    quad_mixer_lab,
    route_carrier_donors,
)


class NestedSamples:
    is_nested = True

    def __init__(self, streams: tuple[torch.Tensor, ...]) -> None:
        self.streams = streams

    def unbind(self) -> tuple[torch.Tensor, ...]:
        return self.streams


def latent(offset: float) -> dict[str, object]:
    base = torch.arange(24 * 2 * 2 * 2, dtype=torch.float32).reshape(1, 24, 2, 2, 2)
    video = torch.sin(base * 0.03 + offset).half().contiguous()
    audio = torch.full((1, 32, 2, 9), offset, dtype=torch.float16)
    return {"samples": NestedSamples((video, audio)), "source_id": f"source-{offset}"}


def test_carrier_donor_router_reorders_donors_and_weights_explicitly() -> None:
    a, b, c, d = (latent(value) for value in (0.0, 1.0, 2.0, 3.0))

    routed = route_carrier_donors(
        a,
        b,
        c,
        d,
        donor_weights=(0.2, 0.3, 0.5),
        order=("D", "B", "C"),
    )

    assert routed.carrier is a
    assert routed.donors == (d, b, c)
    assert routed.weights == (0.5, 0.2, 0.3)
    assert routed.provenance["order"] == ["D", "B", "C"]
    assert routed.provenance["normalized_weights"] == [0.5, 0.2, 0.3]


def test_dual_mixer_lab_exposes_reviewed_xs5_modes_and_is_repeatable() -> None:
    a, b = latent(0.1), latent(0.8)
    controls = {
        "mix": 0.5,
        "mode": "INTERACT",
        "routing": "A",
        "interaction": 0.8,
        "preserve": 0.4,
        "chaos": 0.0,
        "xs5_routing": "TOPK",
        "temperature": 0.12,
        "top_k": 2,
        "sinkhorn_iterations": 3,
    }

    first = dual_mixer_lab(a, b, operator="XS5", controls=controls, seed=41)
    repeated = dual_mixer_lab(a, b, operator="XS5", controls=controls, seed=41)

    first_video, first_audio = first.output["samples"].unbind()
    repeated_video, _ = repeated.output["samples"].unbind()
    assert torch.equal(first_video, repeated_video)
    assert first_audio.data_ptr() == a["samples"].unbind()[1].data_ptr()
    assert first.provenance["operator"] == "XS5"
    assert first.provenance["mode"] == "INTERACT"


def test_quad_mixer_accepts_duplicate_sources_for_functional_testing_and_marks_them() -> None:
    carrier, donor = latent(0.0), latent(1.0)
    controls = {
        "algorithm": "LINEAR",
        "interaction": 0.7,
        "mode": "HYBRIDIZE",
        "preserve": 0.5,
        "influence_mode": "MANUAL",
        "donor_weight_b": 1.0,
        "donor_weight_c": 0.5,
        "donor_weight_d": 0.25,
        "chaos": 0.0,
    }

    first = quad_mixer_lab(
        carrier,
        donor,
        donor,
        donor,
        controls=controls,
        seed=99,
        source_identities=("A", "DUP", "DUP", "DUP"),
    )
    repeated = quad_mixer_lab(
        carrier,
        donor,
        donor,
        donor,
        controls=controls,
        seed=99,
        source_identities=("A", "DUP", "DUP", "DUP"),
    )

    first_video, first_audio = first.output["samples"].unbind()
    repeated_video, _ = repeated.output["samples"].unbind()
    assert torch.equal(first_video, repeated_video)
    assert first_audio.data_ptr() == carrier["samples"].unbind()[1].data_ptr()
    assert first.provenance["duplicate_source_test"] is True
    assert first.provenance["duplicate_identities"] == ["DUP"]
    assert first.provenance["acceptance_scope"] == "functional_not_source_diversity"
    assert first.provenance["slot_order"] == ["B", "C", "D"]


def test_quad_xs5_processes_every_temporal_slot_without_downscale() -> None:
    a, b, c, d = (latent(value) for value in (0.0, 0.5, 1.0, 1.5))
    result = quad_mixer_lab(
        a,
        b,
        c,
        d,
        controls={
            "algorithm": "XS5",
            "interaction": 0.8,
            "mode": "HYBRIDIZE",
            "preserve": 0.4,
            "influence_mode": "MANUAL",
            "donor_weight_b": 1.0,
            "donor_weight_c": 1.0,
            "donor_weight_d": 1.0,
            "xs5_routing": "SINKHORN",
            "temperature": 0.12,
            "top_k": 2,
            "sinkhorn_iterations": 3,
            "chaos": 0.0,
        },
        seed=12,
        source_identities=("A", "B", "C", "D"),
    )

    output, _audio = result.output["samples"].unbind()
    assert output.shape == (1, 24, 2, 2, 2)
    assert result.provenance["processed_slots"] == 2
    assert result.provenance["full_grid"] is True
    assert result.provenance["hidden_downscale"] is False
