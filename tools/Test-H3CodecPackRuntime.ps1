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

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$resolvedRoot = (Resolve-Path -LiteralPath $PackRoot).Path
$manifest = Test-H3CodecPackDirectory -PackRoot $resolvedRoot
$python = Join-Path $resolvedRoot 'runtime/python.exe'

$probe = @'
import importlib.metadata
import importlib.util
import json
import platform
import sys

import msgpack
import numpy
import safetensors
import torch
import tqdm
from latentdeck_codec_h3 import descriptor as codec_descriptor
from latentdeck_codec_host import runtime_descriptor
from latentdeck_operator_d2 import get_descriptor as d2_descriptor
from latentdeck_operator_q4 import get_descriptor as q4_descriptor
from latentdeck_rgb_ring import BINDING_ABI_VERSION

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
if d2.get("operator_id") != "org.latentdeck.builtin.ld_d2":
    raise RuntimeError("D2 operator descriptor drift")
if q4.get("operator_id") != "org.latentdeck.builtin.ld_q4":
    raise RuntimeError("Q4 operator descriptor drift")
if BINDING_ABI_VERSION != "1":
    raise RuntimeError("RGB ring binding ABI drift")

for forbidden in ("comfy", "comfyui", "diffusers", "transformers"):
    if importlib.util.find_spec(forbidden) is not None:
        raise RuntimeError(f"forbidden module is importable: {forbidden}")

require_cuda = sys.argv[1] == "true"
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
    "cuda": {
        "available": cuda_available,
        "compiled_runtime": torch.version.cuda,
        "device_name": device_name,
        "required": require_cuda,
    },
    "forbidden_modules": "absent",
    "operators": {
        "d2": d2["operator_id"],
        "q4": q4["operator_id"],
    },
    "packages": versions,
    "python": platform.python_version(),
    "rgb_ring_abi": BINDING_ABI_VERSION,
}, sort_keys=True, separators=(",", ":"), allow_nan=False))
'@

$rawReceipt = & $python -I -s -B -X utf8 -c $probe $RequireCuda.IsPresent.ToString().ToLowerInvariant()
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
