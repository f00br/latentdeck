[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackRoot,

    [string]$ReceiptPath,

    [switch]$RequireCuda
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$nativeArgumentMode = Get-Variable `
    -Name PSNativeCommandArgumentPassing `
    -ValueOnly `
    -ErrorAction SilentlyContinue
if ([string]$nativeArgumentMode -ceq 'Legacy') {
    throw 'H3 Codec Pack runtime smoke requires PowerShell native argument mode Windows or Standard.'
}

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$resolvedRoot = (Resolve-Path -LiteralPath $PackRoot).Path
$manifest = Test-H3CodecPackDirectory -PackRoot $resolvedRoot
$python = Join-Path $resolvedRoot 'runtime/python.exe'

$probe = @'
import base64
import importlib.metadata
import importlib.util
import json
import platform
import sys
from pathlib import Path
from unittest.mock import patch

import msgpack
import numpy
import safetensors
import torch
import tqdm
import latentdeck_codec_h3.d2_capture as d2_capture
import latentdeck_codec_h3.q4_capture as q4_capture
from latentdeck_codec_h3 import descriptor as codec_descriptor
from latentdeck_codec_h3.cartridge import H3VideoSource
from latentdeck_codec_h3.d2_worker_state import H3D2WorkerState
from latentdeck_codec_h3.q4_worker_state import H3Q4WorkerState
from latentdeck_codec_h3.worker_state import (
    ADAPTER_ID,
    ADAPTER_VERSION,
    PACK_ID,
    PROFILE,
    H3WorkerState,
)
from latentdeck_codec_host import runtime_descriptor
from latentdeck_operator_d2 import get_descriptor as d2_descriptor
from latentdeck_operator_q4 import get_descriptor as q4_descriptor
from latentdeck_rgb_ring import BINDING_ABI_VERSION


class NoWriteSpool:
    def __init__(self, *args, **kwargs):
        self.latent_slots = 0

    def abort(self):
        pass


def capture_source(index):
    return H3VideoSource(
        cartridge_id=f"00000000-0000-4000-8000-{index:012d}",
        archive_sha256=f"{index:x}" * 64,
        storage_dtype="F16",
        shape=(1, 24, 107, 48, 84),
        video_bytes=b"",
        width=1344,
        height=768,
        frame_count=362,
        frame_rate_numerator=24,
        frame_rate_denominator=1,
    )


def prove_loop_preservation(session, start_reset, loop_reset):
    session.activate(start_reset)
    session.prepare_loop_reset(("slot_a.loop",))
    if not session.is_awaiting_loop_reset or session.status()["state"] != "capturing":
        raise RuntimeError("Live Capture did not retain ownership across a loop barrier")
    session.resume_after_loop_reset(loop_reset)
    status = session.status()
    if (
        not session.is_active
        or session.is_awaiting_loop_reset
        or status["state"] != "capturing"
        or status["stream_generation"] != 3
    ):
        raise RuntimeError("Live Capture did not resume after a loop reset")

expected = {
    "latentdeck-codec-h3": "0.1.0",
    "latentdeck-codec-host": "0.1.0",
    "latentdeck-operator-d2": "0.1.0",
    "latentdeck-operator-q4": "0.1.0",
    "latentdeck-rgb-ring": "0.1.0",
    "msgpack": "1.2.2",
    "numpy": "2.5.2",
    "safetensors": "0.8.0",
    "torch": "2.13.0+cu130",
    "tqdm": "4.70.0",
}
require_cuda = sys.argv[1] == "true"
manifest_pack_id = sys.argv[2]
manifest_adapter_id = sys.argv[3]
manifest_adapter_version = sys.argv[4]
manifest_profiles = json.loads(base64.b64decode(sys.argv[5], validate=True).decode("utf-8"))
versions = {name: importlib.metadata.version(name) for name in sorted(expected)}
if versions != expected:
    raise RuntimeError(f"runtime version drift: {versions!r}")
if sys.version_info[:3] != (3, 13, 14) or platform.architecture()[0] != "64bit":
    raise RuntimeError("runtime is not CPython 3.13.14 x64")
if torch.version.cuda != "13.0":
    raise RuntimeError(f"unexpected PyTorch CUDA runtime: {torch.version.cuda!r}")
if codec_descriptor() != {
    "codec_family": "minimax_h3",
    "profile_version": "0.1.0",
    "runtime_extra": "cu130",
}:
    raise RuntimeError("H3 adapter descriptor drift")
if runtime_descriptor()["package_version"] != "0.1.0":
    raise RuntimeError("codec-host descriptor drift")
d2 = d2_descriptor()
q4 = q4_descriptor()
if (
    d2.get("operator_id") != "org.latentdeck.builtin.ld_d2"
    or d2.get("operator_version") != "0.1.0"
    or d2.get("schema_version") != "0.1.0"
):
    raise RuntimeError("D2 operator descriptor drift")
if (
    q4.get("operator_id") != "org.latentdeck.builtin.ld_q4"
    or q4.get("operator_version") != "0.1.0"
    or q4.get("schema_version") != "0.1.0"
):
    raise RuntimeError("Q4 operator descriptor drift")
if BINDING_ABI_VERSION != "1":
    raise RuntimeError("RGB ring binding ABI drift")

if PACK_ID != manifest_pack_id:
    raise RuntimeError("H3 worker pack identity does not match codec-pack.json")
if ADAPTER_ID != manifest_adapter_id or ADAPTER_VERSION != manifest_adapter_version:
    raise RuntimeError("H3 worker adapter identity does not match codec-pack.json")
if not any(
    candidate.get("codec_family") == PROFILE["codec_family"]
    and candidate.get("profile") == PROFILE["profile"]
    and PROFILE["profile_version"] in candidate.get("profile_versions", [])
    for candidate in manifest_profiles
):
    raise RuntimeError("H3 worker profile is not declared by codec-pack.json")

session_config = {
    "selected_protocol_version": 1,
    "app_version": "0.1.0",
    "heartbeat_interval_ms": 1000,
    "heartbeat_hard_timeout_ms": 5000,
    "max_frame_bytes": 262144,
    "max_inflight_decode_batches": 1,
}
worker_states = {
    "player": H3WorkerState(),
    "d2": H3D2WorkerState(),
    "q4": H3Q4WorkerState(),
}
for worker_name, worker_state in worker_states.items():
    configured = worker_state.handle("session.configure", session_config)
    if configured["selected_protocol_version"] != 1:
        raise RuntimeError(f"{worker_name} worker protocol version drift")
    status = worker_state.handle("worker.status", {})
    if (
        status.get("worker_version") != versions["latentdeck-codec-h3"]
        or status.get("protocol_version") != 1
    ):
        raise RuntimeError(f"{worker_name} worker status version drift")
    inspection = worker_state.handle("codec.inspect", {})
    adapters = [
        adapter
        for adapter in inspection.get("adapters", [])
        if adapter.get("adapter_id") == manifest_adapter_id
    ]
    if (
        len(adapters) != 1
        or adapters[0].get("adapter_version") != manifest_adapter_version
        or PROFILE not in adapters[0].get("profiles", [])
    ):
        raise RuntimeError(f"{worker_name} codec.inspect does not match codec-pack.json")
    if require_cuda and (
        not inspection.get("cuda_available")
        or not any(device.get("ordinal") == 0 for device in inspection.get("devices", []))
    ):
        raise RuntimeError(f"{worker_name} codec.inspect does not expose CUDA device 0")

capture_sources = [capture_source(index) for index in range(1, 5)]
with patch.object(d2_capture, "H3ResampleSpool", NoWriteSpool):
    prove_loop_preservation(
        d2_capture.D2CaptureSession(
            capture_id="11111111-1111-4111-8111-111111111111",
            mode="live_capture",
            temporary_root=Path.cwd(),
            max_latent_slots=1_048_576,
            max_visual_bytes=15 * 1024**3,
            source_a=capture_sources[0],
            source_b=capture_sources[1],
            controls={"routing": "A"},
            seed=0,
            current_generation=1,
            minimum_new_generation=2,
        ),
        {"stream_generation": 2, "playhead_a": 0, "playhead_b": 0},
        {"stream_generation": 3, "playhead_a": 0, "playhead_b": 0},
    )
with patch.object(q4_capture, "H3ResampleSpool", NoWriteSpool):
    prove_loop_preservation(
        q4_capture.Q4CaptureSession(
            capture_id="22222222-2222-4222-8222-222222222222",
            mode="live_capture",
            temporary_root=Path.cwd(),
            max_latent_slots=1_048_576,
            max_visual_bytes=15 * 1024**3,
            source_a=capture_sources[0],
            source_b=capture_sources[1],
            source_c=capture_sources[2],
            source_d=capture_sources[3],
            roles={"carrier": "A", "donor_b": "B", "donor_c": "C", "donor_d": "D"},
            controls={},
            seed=0,
            current_generation=1,
            minimum_new_generation=2,
        ),
        {
            "stream_generation": 2,
            "playhead_a": 0,
            "playhead_b": 0,
            "playhead_c": 0,
            "playhead_d": 0,
        },
        {
            "stream_generation": 3,
            "playhead_a": 0,
            "playhead_b": 0,
            "playhead_c": 0,
            "playhead_d": 0,
        },
    )

for forbidden in ("comfy", "comfyui", "diffusers", "transformers"):
    if importlib.util.find_spec(forbidden) is not None:
        raise RuntimeError(f"forbidden module is importable: {forbidden}")

cuda_available = bool(torch.cuda.is_available())
device_name = None
if require_cuda and not cuda_available:
    raise RuntimeError("CUDA was required but is unavailable")
if cuda_available:
    value = torch.tensor([2.0], device="cuda") * 3.0
    torch.cuda.synchronize()
    if value.cpu().item() != 6.0:
        raise RuntimeError("CUDA arithmetic smoke failed")
    device_name = torch.cuda.get_device_name(0)

print(json.dumps({
    "adapter": codec_descriptor(),
    "capture_loop_preservation": "passed",
    "cuda": {
        "available": cuda_available,
        "compiled_runtime": torch.version.cuda,
        "device_name": device_name,
        "required": require_cuda,
    },
    "forbidden_modules": "absent",
    "operators": {
        "d2": {"id": d2["operator_id"], "version": d2["operator_version"]},
        "q4": {"id": q4["operator_id"], "version": q4["operator_version"]},
    },
    "packages": versions,
    "python": platform.python_version(),
    "rgb_ring_abi": BINDING_ABI_VERSION,
    "worker_contract": {
        "adapter_id": ADAPTER_ID,
        "adapter_version": ADAPTER_VERSION,
        "codec_inspect": sorted(worker_states),
        "pack_id": PACK_ID,
        "profile": PROFILE,
        "worker_version": versions["latentdeck-codec-h3"],
    },
}, sort_keys=True, separators=(",", ":"), allow_nan=False))
'@

$manifestProfilesJson = ConvertTo-Json `
    -InputObject @($manifest.compatibility.profiles) `
    -Depth 16 `
    -Compress
$manifestProfilesBase64 = [System.Convert]::ToBase64String(
    [System.Text.Encoding]::UTF8.GetBytes($manifestProfilesJson)
)
$rawReceipt = & $python -I -s -B -X utf8 -c $probe `
    $RequireCuda.IsPresent.ToString().ToLowerInvariant() `
    ([string]$manifest.pack_id) `
    ([string]$manifest.adapter.adapter_id) `
    ([string]$manifest.adapter.adapter_version) `
    $manifestProfilesBase64
if ($LASTEXITCODE -ne 0) {
    throw "H3 Codec Pack isolated runtime probe failed with exit code $LASTEXITCODE."
}
if (@($rawReceipt).Count -ne 1) {
    throw 'H3 Codec Pack isolated runtime probe emitted unexpected output.'
}
try {
    $runtime = [string]$rawReceipt | ConvertFrom-Json
} catch {
    throw 'H3 Codec Pack isolated runtime probe did not emit valid JSON.'
}

$receipt = [ordered]@{
    schema_version = 1
    pack_id = [string]$manifest.pack_id
    pack_version = [string]$manifest.pack_version
    platform = 'windows-x86_64'
    runtime = $runtime
    contains_model_weights = $false
    contains_generator = $false
    contains_comfy = $false
    external_decoder_selection_required = $true
    result = 'passed'
}

if (-not [string]::IsNullOrWhiteSpace($ReceiptPath)) {
    $receiptFullPath = [System.IO.Path]::GetFullPath($ReceiptPath)
    if (Test-Path -LiteralPath $receiptFullPath) {
        throw "Refusing to overwrite an existing runtime receipt: $receiptFullPath"
    }
    $receiptParent = Split-Path -Parent $receiptFullPath
    [System.IO.Directory]::CreateDirectory($receiptParent) | Out-Null
    $partial = Join-Path $receiptParent (
        ".$(Split-Path -Leaf $receiptFullPath).partial-$([guid]::NewGuid().ToString('N'))"
    )
    try {
        Write-JsonFile -Value $receipt -Path $partial
        [System.IO.File]::Move($partial, $receiptFullPath)
    } finally {
        if (Test-Path -LiteralPath $partial -PathType Leaf) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
}

$receipt | ConvertTo-Json -Depth 16 -Compress | Write-Output
