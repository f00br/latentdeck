from __future__ import annotations

import comfy.toolkit as discovery


def test_comfy_discovery_shim_uses_the_complete_canonical_package_registry() -> None:
    required = {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitRawH3Import",
        "LatentDeckToolkitLCSaveResample",
        "LatentDeckToolkitXS5",
        "LatentDeckToolkitQuadMixerLab",
        "LatentDeckToolkitFastHQComparator",
        "LatentDeckToolkitManifoldProjector",
        "LatentDeckToolkitOperatorBenchmark",
        "LatentDeckToolkitResearchReport",
    }

    assert required <= set(discovery.NODE_CLASS_MAPPINGS)
    assert set(discovery.NODE_CLASS_MAPPINGS) == set(discovery.NODE_DISPLAY_NAME_MAPPINGS)
