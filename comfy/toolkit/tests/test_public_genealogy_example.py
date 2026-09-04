from __future__ import annotations

import importlib.util
import sys
import uuid
from pathlib import Path

import latentdeck_cartridge
import pytest
import torch
from safetensors.torch import save_file

from latentdeck_comfy_toolkit import ToolkitIOError

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
EXAMPLE_PATH = REPOSITORY_ROOT / "examples/cartridge-genealogy/transform.py"


def _load_example():
    module_name = "latentdeck_public_cartridge_genealogy"
    specification = importlib.util.spec_from_file_location(module_name, EXAMPLE_PATH)
    assert specification is not None and specification.loader is not None
    module = importlib.util.module_from_spec(specification)
    sys.modules[module_name] = module
    previous = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        specification.loader.exec_module(module)
    finally:
        sys.dont_write_bytecode = previous
    return module


def test_public_example_generates_distinct_validated_b_with_exact_a_parent(
    tmp_path: Path,
) -> None:
    raw = tmp_path / "raw.safetensors"
    source = tmp_path / "A.lc"
    output = tmp_path / "B.lc"
    save_file(
        {"video": torch.arange(48, dtype=torch.float32).reshape(1, 24, 2, 1, 1)},
        str(raw),
    )
    latentdeck_cartridge.pack_raw_h3(raw, source)
    source_inspection = latentdeck_cartridge.inspect(source)

    example = _load_example()

    def invert(samples, controls, seed):
        assert controls == {"amount": 1.0}
        assert seed == 17
        return (-samples).contiguous()

    receipt = example.transform_cartridge(
        source,
        output,
        operator=invert,
        operator_id="org.example.invert",
        operator_version="0.1.0",
        controls={"amount": 1.0},
        seed=17,
    )

    final = latentdeck_cartridge.inspect(output)
    assert uuid.UUID(receipt["cartridge_id"])
    assert final["manifest"]["cartridge_id"] != source_inspection["manifest"]["cartridge_id"]
    assert final["manifest"]["parent_cartridges"] == [
        {
            "cartridge_id": source_inspection["manifest"]["cartridge_id"],
            "archive_sha256": latentdeck_cartridge.hash(source)["sha256"],
            "role": "source",
        }
    ]
    assert final["manifest"]["operation_history"] == [
        {
            "operator_id": "org.example.invert",
            "operator_version": "0.1.0",
            "seed": 17,
            "controls": {"amount": 1.0},
        }
    ]
    assert final["manifest"]["audio"] == {"policy": "source_absent"}
    assert receipt["validation_level"] == "full"

    preserved_bytes = output.read_bytes()
    preserved_hash = latentdeck_cartridge.hash(output)["sha256"]
    with pytest.raises(ToolkitIOError) as failure:
        example.transform_cartridge(
            source,
            output,
            operator=invert,
            operator_id="org.example.invert",
            operator_version="0.1.0",
            controls={"amount": 1.0},
            seed=17,
        )
    assert failure.value.code == "resample.target_exists"
    assert output.read_bytes() == preserved_bytes
    assert latentdeck_cartridge.hash(output)["sha256"] == preserved_hash
