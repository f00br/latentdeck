from __future__ import annotations

import importlib.util
from pathlib import Path


def _load_discovery_shim() -> object:
    shim_path = Path(__file__).resolve().parents[1] / "__init__.py"
    spec = importlib.util.spec_from_file_location("latentdeck_toolkit_discovery_test", shim_path)
    if spec is None or spec.loader is None:
        raise AssertionError("Toolkit discovery shim could not be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_comfy_discovery_shim_uses_the_complete_canonical_package_registry() -> None:
    discovery = _load_discovery_shim()
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
