from __future__ import annotations

import torch

from latentdeck_comfy_toolkit.cartridge_io import H3AVSamples
from latentdeck_comfy_toolkit.workflow_metadata import (
    annotate_evaluation,
    annotate_operation,
    derive_resample_inputs,
    derive_research_report_inputs,
    initialize_lc_metadata,
    initialize_raw_metadata,
    record_saved_output,
)


def av_latent(marker: float = 0.0) -> dict[str, object]:
    video = torch.full((1, 24, 2, 2, 2), marker, dtype=torch.float16)
    audio = torch.full((1, 32, 2, 9), marker, dtype=torch.float32)
    return {"samples": H3AVSamples((video, audio))}


def lc_source(cartridge_id: str, digest: str, marker: float) -> dict[str, object]:
    latent = av_latent(marker)
    return initialize_lc_metadata(
        latent,
        manifest={
            "cartridge_id": cartridge_id,
            "codec": {
                "family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_version": "0.1.0",
            },
            "timing": {"contract": "minimax_h3_causal", "contract_version": "0.1.0"},
            "audio": {"policy": "preserved_source"},
        },
        validation={"archive_sha256": digest, "validation_level": "full"},
    )


def test_dual_operation_automatically_accumulates_genealogy_history_and_audio_carrier() -> None:
    carrier = lc_source("550e8400-e29b-41d4-a716-4466554400a0", "a" * 64, 0.0)
    donor = lc_source("550e8400-e29b-41d4-a716-4466554400b0", "b" * 64, 1.0)
    mixed = {**carrier, "samples": carrier["samples"]}

    annotated = annotate_operation(
        mixed,
        sources=(("carrier", carrier), ("donor", donor)),
        structural_role="carrier",
        provenance={
            "operation": "XS3_FREQUENCY_CROSS_SYNTHESIS",
            "parameters": {"cutoff": 0.25, "donor_band": "LOW", "strength": 0.8},
        },
    )
    derived = derive_resample_inputs(annotated)

    assert derived.parent_cartridges == (
        {
            "cartridge_id": "550e8400-e29b-41d4-a716-4466554400a0",
            "archive_sha256": "a" * 64,
            "role": "carrier",
        },
        {
            "cartridge_id": "550e8400-e29b-41d4-a716-4466554400b0",
            "archive_sha256": "b" * 64,
            "role": "donor",
        },
    )
    assert derived.operation_history == (
        {
            "operator_id": "org.latentdeck.toolkit.xs3_frequency_cross_synthesis",
            "operator_version": "0.1.0",
            "seed": 0,
            "controls": {"cutoff": 0.25, "donor_band": "LOW", "strength": 0.8},
        },
    )
    assert derived.audio_disposition == {
        "policy": "copied_from_carrier_exact",
        "source_cartridge": {
            "cartridge_id": "550e8400-e29b-41d4-a716-4466554400a0",
            "archive_sha256": "a" * 64,
        },
    }


def test_raw_operation_uses_source_provenance_without_inventing_a_parent() -> None:
    raw = initialize_raw_metadata(
        av_latent(),
        profile={"codec_family": "minimax_h3", "profile": "h3_av_latent"},
        source={"sha256": "c" * 64, "byte_length": 4096},
    )
    annotated = annotate_operation(
        raw,
        sources=(("source", raw),),
        structural_role="source",
        provenance={"operation": "CHANNEL_LAB", "parameters": {"strength": 0.5}},
    )

    derived = derive_resample_inputs(annotated)
    assert derived.parent_cartridges == ()
    assert derived.audio_disposition == {"policy": "preserved_source"}
    assert derived.provenance_sources == (
        {
            "kind": "raw_h3_safetensors",
            "sha256": "c" * 64,
            "metadata": {"byte_length": 4096},
        },
    )


def test_report_collects_versions_sources_operations_measurements_and_outputs() -> None:
    source = lc_source("550e8400-e29b-41d4-a716-4466554400d0", "d" * 64, 0.0)
    operated = annotate_operation(
        source,
        sources=(("carrier", source),),
        structural_role="carrier",
        provenance={
            "operation": {
                "operator_id": "org.example.operator",
                "operator_version": "1.2.3",
                "seed": 7,
                "controls": {"amount": 0.75},
            }
        },
    )
    measured = annotate_evaluation(
        operated,
        kind="determinism",
        report={"runs": 3, "deterministic": True},
    )
    saved = record_saved_output(
        measured,
        cartridge_id="550e8400-e29b-41d4-a716-4466554400e0",
        archive_sha256="e" * 64,
        file_name="study.lc",
    )

    report = derive_research_report_inputs(saved)
    assert report["versions"]["toolkit"] == "0.1.0"
    assert report["cartridges"][0]["archive_sha256"] == "d" * 64
    assert report["operators"][0]["operator_id"] == "org.example.operator"
    assert report["measurements"][0] == {
        "kind": "determinism",
        "report": {"deterministic": True, "runs": 3},
    }
    assert report["outputs"] == [
        {
            "archive_sha256": "e" * 64,
            "cartridge_id": "550e8400-e29b-41d4-a716-4466554400e0",
            "file_name": "study.lc",
        }
    ]
