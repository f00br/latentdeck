from __future__ import annotations

import base64
import json
import platform
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).parents[4]
PROBE = REPO_ROOT / "tools/h3_codec_pack_runtime_probe.py"
POWERSHELL_GATE = REPO_ROOT / "tools/Test-H3CodecPackRuntime.ps1"


def _manifest() -> dict[str, object]:
    return {
        "manifest_version": "2.0.0",
        "kind": "codec_pack",
        "pack_id": "org.latentdeck.h3",
        "pack_version": "0.2.0",
        "compatibility": {
            "worker_protocol": 2,
            "codec_adapter_api": 1,
            "tensor_abi": "latentdeck.tensor.v1",
            "python": {
                "implementation": "cpython",
                "version": "3.13",
                "platform_tag": "win_amd64",
            },
            "torch_exact_build": "2.13.0+cu130",
            "profiles": [
                {
                    "codec_family": "minimax_h3",
                    "profile": "h3_av_latent",
                    "profile_version": "0.1.0",
                }
            ],
        },
        "adapter": {
            "adapter_id": "org.latentdeck.h3",
            "adapter_version": "0.2.0",
            "entrypoint": "latentdeck_codec_h3.adapter:make_adapter",
        },
        "capabilities": [
            "player",
            "realtime",
            "resample",
            "snapshot_capture",
            "live_capture",
            "raw_import",
        ],
        "external_assets": [
            {
                "asset_id": "taeh3",
                "byte_length": 22_709_752,
                "sha256": "4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13",
            }
        ],
    }


def test_probe_exercises_exact_h3_protocol2_preload_boundary() -> None:
    manifest = base64.b64encode(
        json.dumps(_manifest(), separators=(",", ":")).encode("utf-8")
    ).decode("ascii")
    completed = subprocess.run(
        [
            sys.executable,
            "-I",
            "-s",
            "-B",
            "-X",
            "utf8",
            str(PROBE),
            "false",
            platform.python_version(),
            manifest,
        ],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )

    runtime = json.loads(completed.stdout)
    assert runtime["protocol"] == {
        "commands": ["session.configure", "codec.descriptor"],
        "selected_version": 2,
        "worker_protocol": 2,
    }
    assert runtime["adapter"]["pack_version"] == "0.2.0"
    assert runtime["adapter"]["adapter_version"] == "0.2.0"
    assert runtime["adapter"]["capabilities"] == _manifest()["capabilities"]
    assert runtime["preload_guards"] == {
        "external_decoder_accesses": 0,
        "torch_imported": False,
    }
    assert runtime["rgb_ring_abi"] == {"protocol1": "1", "protocol2": "2"}
    assert runtime["forbidden_modules"] == "absent"


def test_powershell_gate_uses_only_the_protocol2_probe() -> None:
    source = POWERSHELL_GATE.read_text(encoding="utf-8")
    assert "h3_codec_pack_runtime_probe.py" in source
    assert "Protocol2Worker" not in source
    for deleted_or_p1_surface in (
        "latentdeck_codec_h3.d2_capture",
        "latentdeck_codec_h3.q4_capture",
        "H3WorkerState",
        "H3D2WorkerState",
        "H3Q4WorkerState",
        "latentdeck_operator_d2",
        "latentdeck_operator_q4",
        '"selected_protocol_version": 1',
    ):
        assert deleted_or_p1_surface not in source
