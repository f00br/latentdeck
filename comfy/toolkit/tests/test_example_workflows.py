from __future__ import annotations

import json
import re
from pathlib import Path

from latentdeck_comfy_cartridge.nodes import NODE_CLASS_MAPPINGS as RECORDER_NODE_TYPES
from latentdeck_example_channel_roll import (
    NODE_CLASS_MAPPINGS as CHANNEL_ROLL_NODE_TYPES,
)

from latentdeck_comfy_toolkit import NODE_CLASS_MAPPINGS as TOOLKIT_NODE_TYPES
from latentdeck_comfy_toolkit.device_nodes import DEVICE_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.io_nodes import IO_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.report_nodes import REPORT_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.research_nodes import RESEARCH_NODE_CLASS_MAPPINGS
from latentdeck_comfy_toolkit.vae_nodes import VAE_NODE_CLASS_MAPPINGS

WORKFLOW_DIRECTORY = Path(__file__).parents[1] / "workflows"
CUSTOM_NODE_TYPES = {
    **TOOLKIT_NODE_TYPES,
    **RECORDER_NODE_TYPES,
    **CHANNEL_ROLL_NODE_TYPES,
}
REPOSITORY_OWNED_NODE_TYPES = {
    *TOOLKIT_NODE_TYPES,
    *RECORDER_NODE_TYPES,
    *CHANNEL_ROLL_NODE_TYPES,
}
GALLERY_WORKFLOW = "00_ALL_NODES_GALLERY.json"
assert len(TOOLKIT_NODE_TYPES) == 33
assert len(RECORDER_NODE_TYPES) == 1
assert len(CHANNEL_ROLL_NODE_TYPES) == 2
assert len(REPOSITORY_OWNED_NODE_TYPES) == (
    len(TOOLKIT_NODE_TYPES) + len(RECORDER_NODE_TYPES) + len(CHANNEL_ROLL_NODE_TYPES)
)
EXPECTED_WORKFLOWS = {
    GALLERY_WORKFLOW: REPOSITORY_OWNED_NODE_TYPES,
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
        "LatentDeckToolkitExplicitDeviceTransfer",
        "LatentDeckToolkitFastDecode",
    },
    "04_QUAD_CARRIER_DONORS.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitCompatibility",
        "LatentDeckToolkitCarrierDonorRouter",
        "LatentDeckToolkitQuadMixerLab",
        "LatentDeckToolkitExplicitDeviceTransfer",
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
        "LatentDeckToolkitExplicitDeviceTransfer",
        "LatentDeckToolkitOperatorBenchmark",
        "LatentDeckToolkitDeterminismTest",
        "LatentDeckToolkitStreamingCompatibilityTest",
        "LatentDeckToolkitResearchReport",
    },
    "06_RAW_RECORD_INSPECT.json": {
        "LatentDeckToolkitRawH3Import",
        "LatentDeckSaveLatentCartridge",
        "LatentDeckToolkitLatentScopes",
    },
    "07_EXPLICIT_ALIGN_CROP.json": {
        "LatentDeckToolkitLCLoadInspect",
        "LatentDeckToolkitExplicitAlign",
        "LatentDeckToolkitCompatibility",
        "LatentDeckToolkitDualMixerLab",
        "LatentDeckToolkitLCSaveResample",
        "LatentDeckToolkitLatentScopes",
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
        *DEVICE_NODE_CLASS_MAPPINGS,
        *RESEARCH_NODE_CLASS_MAPPINGS,
        *VAE_NODE_CLASS_MAPPINGS,
        *REPORT_NODE_CLASS_MAPPINGS,
        *RECORDER_NODE_TYPES,
        *CHANNEL_ROLL_NODE_TYPES,
        *TOOLKIT_NODE_TYPES,
        "VAELoader",
        "PreviewAny",
        "PreviewImage",
    }

    assert {path.name for path in WORKFLOW_DIRECTORY.glob("*.json")} == set(EXPECTED_WORKFLOWS)
    for name, required_types in EXPECTED_WORKFLOWS.items():
        workflow = json.loads((WORKFLOW_DIRECTORY / name).read_text(encoding="utf-8"))
        assert workflow["version"] == 0.4
        assert isinstance(workflow["id"], str)
        assert workflow["revision"] == 0
        assert isinstance(workflow["nodes"], list) and workflow["nodes"]
        assert isinstance(workflow["links"], list)
        if name == GALLERY_WORKFLOW:
            assert len(workflow["groups"]) == 8
        else:
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

    for workflow_name in set(EXPECTED_WORKFLOWS) - {GALLERY_WORKFLOW}:
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


def test_all_nodes_gallery_is_an_exact_data_free_registry_view() -> None:
    workflow = json.loads((WORKFLOW_DIRECTORY / GALLERY_WORKFLOW).read_text(encoding="utf-8"))
    node_types = [node["type"] for node in workflow["nodes"]]

    assert len(REPOSITORY_OWNED_NODE_TYPES) == 36
    assert len(node_types) == 36
    assert set(node_types) == REPOSITORY_OWNED_NODE_TYPES
    assert all(node_types.count(node_type) == 1 for node_type in REPOSITORY_OWNED_NODE_TYPES)
    assert workflow["links"] == []
    assert {group["title"] for group in workflow["groups"]} == {
        "Cartridge / Conversion",
        "Decode / Offline",
        "XS Operators",
        "Labs",
        "Diagnostics / Evaluation",
        "Developer / Utilities",
        "Recorder",
        "External Example",
    }

    text = "\n".join(_all_strings(workflow))
    lowered = text.lower()
    assert "prompt" not in lowered
    assert "payloads/" not in lowered
    assert re.search(r"(?:^|[\"'])[a-z]:[\\/]", text, re.IGNORECASE) is None
    assert "w:/" not in lowered


def test_comparison_previews_are_unambiguous_for_visual_master_testing() -> None:
    fast_hq = json.loads(
        (WORKFLOW_DIRECTORY / "02_FAST_HQ_COMPARE.json").read_text(encoding="utf-8")
    )
    fast_hq_titles = {node.get("title") for node in fast_hq["nodes"]}
    assert "FAST preview (TAEHV / taeh3)" in fast_hq_titles
    assert "HQ reference preview (native H3 VAE)" in fast_hq_titles

    projector = json.loads(
        (WORKFLOW_DIRECTORY / "05_PROJECT_RESAMPLE.json").read_text(encoding="utf-8")
    )
    projector_titles = {node.get("title") for node in projector["nodes"]}
    assert {
        "RAW latent — FAST preview",
        "PROJECTED latent — FAST preview",
        "RAW latent — HQ reference",
        "PROJECTED latent — HQ reference",
    } <= projector_titles


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
            widget_declarations = dict(declared_inputs.get("required", {}))
            if path.name == GALLERY_WORKFLOW:
                widget_declarations.update(declared_inputs.get("optional", {}))
            for input_name, declaration in widget_declarations.items():
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


def test_cuda_research_examples_stage_every_operator_source_explicitly() -> None:
    expected_transfer_counts = {
        "03_DUAL_SYNTH_LAB.json": 2,
        "04_QUAD_CARRIER_DONORS.json": 4,
        "99_OPERATOR_DEVELOPER_TEMPLATE.json": 2,
    }
    downstream_inputs = {
        "03_DUAL_SYNTH_LAB.json": ("LatentDeckToolkitDualMixerLab", {"carrier", "donor"}),
        "04_QUAD_CARRIER_DONORS.json": (
            "LatentDeckToolkitCarrierDonorRouter",
            {"carrier", "donor_b", "donor_c", "donor_d"},
        ),
        "99_OPERATOR_DEVELOPER_TEMPLATE.json": (
            "LatentDeckToolkitDualOperatorHook",
            {"donor"},
        ),
    }

    for name, expected_count in expected_transfer_counts.items():
        workflow = json.loads((WORKFLOW_DIRECTORY / name).read_text(encoding="utf-8"))
        nodes = {node["id"]: node for node in workflow["nodes"]}
        links = {link[0]: link for link in workflow["links"]}
        transfers = [
            node
            for node in workflow["nodes"]
            if node["type"] == "LatentDeckToolkitExplicitDeviceTransfer"
        ]
        assert len(transfers) == expected_count
        for transfer in transfers:
            assert transfer["widgets_values"] == ["CUDA", 0, "FALLBACK_TO_CPU"]
            assert "explicit CPU fallback" in transfer["title"]
            assert transfer["inputs"][0]["link"] is not None
            assert transfer["outputs"][0]["links"]

        downstream_type, input_names = downstream_inputs[name]
        downstream = next(node for node in workflow["nodes"] if node["type"] == downstream_type)
        for input_socket in downstream["inputs"]:
            if input_socket["name"] not in input_names:
                continue
            origin_id = links[input_socket["link"]][1]
            assert nodes[origin_id]["type"] == "LatentDeckToolkitExplicitDeviceTransfer"

    template = json.loads(
        (WORKFLOW_DIRECTORY / "99_OPERATOR_DEVELOPER_TEMPLATE.json").read_text(encoding="utf-8")
    )
    nodes = {node["id"]: node for node in template["nodes"]}
    links = {link[0]: link for link in template["links"]}
    benchmark = next(
        node for node in template["nodes"] if node["type"] == "LatentDeckToolkitOperatorBenchmark"
    )
    latent_input = next(value for value in benchmark["inputs"] if value["name"] == "latent")
    assert nodes[links[latent_input["link"]][1]]["type"] == (
        "LatentDeckToolkitExplicitDeviceTransfer"
    )
