from __future__ import annotations

import json

import torch

from latentdeck_comfy_toolkit import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS
from latentdeck_comfy_toolkit.io_nodes import IO_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.report_nodes import REPORT_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.research_nodes import RESEARCH_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.vae_nodes import VAE_NODE_CLASS_MAPPINGS


def latent_pair() -> tuple[dict[str, object], dict[str, object]]:
    index = torch.arange(24 * 2 * 3 * 4, dtype=torch.float32).reshape(1, 24, 2, 3, 4)
    return (
        {"samples": torch.sin(index * 0.03).half(), "batch_index": [0]},
        {"samples": torch.cos(index * 0.05).half(), "batch_index": [1]},
    )


def test_canonical_registry_aggregates_io_research_and_vae_surfaces_without_collisions() -> None:
    expected = set(IO_NODE_CLASS_MAPPINGS) | set(RESEARCH_NODE_CLASS_MAPPINGS) | set(
        VAE_NODE_CLASS_MAPPINGS
    ) | set(REPORT_NODE_CLASS_MAPPINGS) | {
        "LatentDeckToolkitCompareDecoders",
        "LatentDeckToolkitOfflineProjector",
    }

    assert set(NODE_CLASS_MAPPINGS) == expected
    assert set(NODE_DISPLAY_NAME_MAPPINGS) == expected
    assert len(expected) == (
        len(IO_NODE_CLASS_MAPPINGS)
        + len(RESEARCH_NODE_CLASS_MAPPINGS)
        + len(VAE_NODE_CLASS_MAPPINGS)
        + len(REPORT_NODE_CLASS_MAPPINGS)
        + 2
    )


def test_registered_comfy_widget_metadata_uses_min_and_max_keys() -> None:
    def declarations(node_type: type):
        input_types = node_type.INPUT_TYPES()
        for section in ("required", "optional"):
            yield from input_types.get(section, {}).values()

    for node_type in NODE_CLASS_MAPPINGS.values():
        for declaration in declarations(node_type):
            if not isinstance(declaration, tuple) or len(declaration) < 2:
                continue
            metadata = declaration[1]
            if isinstance(metadata, dict):
                assert "minimum" not in metadata
                assert "maximum" not in metadata


def test_canonical_xs3_is_frequency_domain_not_the_old_temporal_prototype() -> None:
    carrier, donor = latent_pair()
    node = NODE_CLASS_MAPPINGS["LatentDeckToolkitXS3"]()

    output, provenance_json = node.process(carrier, donor, 0.25, "LOW", 1.0)
    provenance = json.loads(provenance_json)

    assert output["samples"].shape == carrier["samples"].shape
    assert provenance["operation"] == "XS3_FREQUENCY_CROSS_SYNTHESIS"
    assert provenance["parameters"]["domain"] == "FFT2_SPATIAL"
    assert NODE_DISPLAY_NAME_MAPPINGS["LatentDeckToolkitXS3"].endswith(
        "Frequency Cross-Synthesis"
    )


def test_canonical_registry_has_no_misleading_manifold_name_for_pca_diagnostic() -> None:
    assert NODE_DISPLAY_NAME_MAPPINGS["LatentDeckToolkitOfflineProjector"] == (
        "LatentDeck PCA Diagnostic (Offline CPU)"
    )
    assert "Native H3" in NODE_DISPLAY_NAME_MAPPINGS[
        "LatentDeckToolkitManifoldProjector"
    ]
