from __future__ import annotations

import json
import re
from pathlib import Path

from latentdeck_comfy_toolkit.io_nodes import IO_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.report_nodes import REPORT_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.research_nodes import RESEARCH_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.vae_nodes import VAE_NODE_CLASS_MAPPINGS

WORKFLOW_DIRECTORY = Path(__file__).parents[1] / "workflows"
CUSTOM_NODE_TYPES = {
    **IO_NODE_CLASS_MAPPINGS,
    **RESEARCH_NODE_CLASS_MAPPINGS,
    **VAE_NODE_CLASS_MAPPINGS,
    **REPORT_NODE_CLASS_MAPPINGS,
}
EXPECTED_WORKFLOWS = {
    "01_LC_INSPECT.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitLatentScopes",
    },
    "02_FAST_HQ_COMPARE.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitDeclareH3Vae",
        "LatentDeckToolkitFastHQComparator",
    },
    "03_DUAL_SYNTH_LAB.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitCompatibility",
        "LatentDeckToolkitDualMixerLab",
        "LatentDeckToolkitFastDecode",
    },
    "04_QUAD_CARRIER_DONORS.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitCompatibility",
        "LatentDeckToolkitCarrierDonorRouter",
        "LatentDeckToolkitQuadMixerLab",
        "LatentDeckToolkitFastDecode",
    },
    "05_PROJECT_RESAMPLE.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitDeclareH3Vae",
        "LatentDeckToolkitManifoldProjector",
        "LatentDeckToolkitProjectorComparison",
        "LatentDeckToolkitLCSaveResample",
    },
    "99_OPERATOR_DEVELOPER_TEMPLATE.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitDualOperatorHook",
        "LatentDeckToolkitOperatorBenchmark",
        "LatentDeckToolkitDeterminismTest",
        "LatentDeckToolkitStreamingCompatibilityTest",
        "LatentDeckToolkitResearchReport",
    },
}


def _all_strings(value: object):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from _all_strings(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            yield key
            yield from _all_strings(item)


def test_example_workflows_are_loadable_sanitized_comfy_graphs() -> None:
    installed_types = {
        *IO_NODE_CLASS_MAPPINGS,
        *RESEARCH_NODE_CLASS_MAPPINGS,
        *VAE_NODE_CLASS_MAPPINGS,
        *REPORT_NODE_CLASS_MAPPINGS,
        "VAELoader",
        "PreviewAny",
        "PreviewImage",
    }

    assert {path.name for path in WORKFLOW_DIRECTORY.glob("*.json")} == set(
        EXPECTED_WORKFLOWS
    )
    for name, required_types in EXPECTED_WORKFLOWS.items():
        workflow = json.loads((WORKFLOW_DIRECTORY / name).read_text(encoding="utf-8"))
        assert workflow["version"] == 0.4
        assert isinstance(workflow["id"], str)
        assert workflow["revision"] == 0
        assert isinstance(workflow["nodes"], list) and workflow["nodes"]
        assert isinstance(workflow["links"], list)
        assert workflow["groups"] == []
        assert workflow["config"] == {}
        assert isinstance(workflow["extra"], dict)

        nodes = {node["id"]: node for node in workflow["nodes"]}
        assert len(nodes) == len(workflow["nodes"])
        assert workflow["last_node_id"] == max(nodes)
        node_types = {node["type"] for node in nodes.values()}
        assert required_types <= node_types
        assert node_types <= installed_types

        links = {link[0]: link for link in workflow["links"]}
        assert len(links) == len(workflow["links"])
        assert workflow["last_link_id"] == (max(links) if links else 0)
        for link_id, origin_id, origin_slot, target_id, target_slot, link_type in links.values():
            origin = nodes[origin_id]
            target = nodes[target_id]
            assert link_id in origin["outputs"][origin_slot]["links"]
            assert target["inputs"][target_slot]["link"] == link_id
            assert origin["outputs"][origin_slot]["type"] == link_type
            assert target["inputs"][target_slot]["type"] in {link_type, "*"}

        text = "\n".join(_all_strings(workflow))
        lowered = text.lower()
        assert re.search(r"(?:^|[\"'])[a-z]:[\\/]", text, re.IGNORECASE) is None
        assert "\\\\" not in text
        assert "w:/" not in lowered
        assert "h3-pipeline" not in lowered
        assert "runpod" not in lowered
        assert "api_key" not in lowered
        assert "password" not in lowered


def test_workflow_and_operator_template_guides_cover_every_public_example() -> None:
    toolkit_root = Path(__file__).parents[1]
    workflow_guide = (toolkit_root / "workflows" / "README.md").read_text(encoding="utf-8")
    operator_guide = (toolkit_root / "docs" / "OPERATOR_DEVELOPER_TEMPLATE.md").read_text(
        encoding="utf-8"
    )
    toolkit_readme = (toolkit_root / "README.md").read_text(encoding="utf-8")

    for workflow_name in EXPECTED_WORKFLOWS:
        assert workflow_name in workflow_guide
    for topology in ("single_source", "dual_source", "carrier_donors"):
        assert f"`{topology}`" in operator_guide
    for contract_term in (
        "supported_profiles",
        "streaming",
        "deterministic",
        "zero/bypass",
    ):
        assert contract_term in operator_guide
    for replacement_term in (
        "LatentDeckToolkitDualOperatorHook",
        "LatentDeckExampleChannelRollHook",
        "LATENTDECK_OPERATOR_HOOK",
    ):
        assert replacement_term in workflow_guide
    assert "dynamic loader" in workflow_guide
    assert "workflows/README.md" in toolkit_readme
    assert "docs/OPERATOR_DEVELOPER_TEMPLATE.md" in toolkit_readme


def test_example_workflow_ports_match_the_registered_comfy_contracts() -> None:
    for path in WORKFLOW_DIRECTORY.glob("*.json"):
        workflow = json.loads(path.read_text(encoding="utf-8"))
        for node in workflow["nodes"]:
            node_type = CUSTOM_NODE_TYPES.get(node["type"])
            if node_type is None:
                continue

            return_types = tuple(node_type.RETURN_TYPES)
            return_names = tuple(
                getattr(
                    node_type,
                    "RETURN_NAMES",
                    tuple(f"output_{i}" for i in range(len(return_types))),
                )
            )
            assert len(node["outputs"]) == len(return_types), (path.name, node["type"])
            for index, output in enumerate(node["outputs"]):
                assert output["type"] == return_types[index], (path.name, node["type"])
                assert output["name"] == return_names[index], (path.name, node["type"])

            declared_inputs = node_type.INPUT_TYPES()
            inputs = {
                **declared_inputs.get("required", {}),
                **declared_inputs.get("optional", {}),
            }
            for serialized in node["inputs"]:
                assert serialized["name"] in inputs, (path.name, node["type"])
                declaration = inputs[serialized["name"]]
                assert serialized["type"] == declaration[0], (path.name, node["type"])

            serialized_names = {item["name"] for item in node["inputs"]}
            required_widgets = []
            for input_name, declaration in declared_inputs.get("required", {}).items():
                if input_name in serialized_names:
                    continue
                input_type = declaration[0]
                is_widget = isinstance(input_type, list) or input_type in {
                    "BOOLEAN",
                    "FLOAT",
                    "INT",
                    "STRING",
                }
                assert is_widget, (path.name, node["type"], input_name)
                required_widgets.append(input_name)
            assert len(node["widgets_values"]) == len(required_widgets), (
                path.name,
                node["type"],
                required_widgets,
            )
