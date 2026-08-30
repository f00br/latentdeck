[CmdletBinding()]
param(
    [string]$ComfyRoot,

    [string]$PythonExecutable,

    [string]$ModelsRoot,

    [string]$HqVaePath,

    [string]$EnvironmentRoot,

    [ValidateRange(1024, 65535)]
    [int]$Port = 8192
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$artifactsRoot = (Resolve-Path -LiteralPath $artifactsRoot).Path

function Assert-ChildPath {
    param(
        [Parameter(Mandatory)]
        [string]$Root,
        [Parameter(Mandatory)]
        [string]$Candidate,
        [Parameter(Mandatory)]
        [string]$Label,
        [switch]$AllowRoot
    )

    $rootFull = [System.IO.Path]::GetFullPath($Root)
    $candidateFull = [System.IO.Path]::GetFullPath($Candidate)
    $relative = [System.IO.Path]::GetRelativePath($rootFull, $candidateFull)
    $inside = -not $relative.StartsWith('..', [System.StringComparison]::Ordinal) -and
        -not [System.IO.Path]::IsPathFullyQualified($relative)
    if (-not $inside -or (-not $AllowRoot -and $relative -eq '.')) {
        throw "$Label must be a child of $rootFull."
    }
}

function Assert-NoReparseAncestor {
    param(
        [Parameter(Mandatory)]
        [string]$Root,
        [Parameter(Mandatory)]
        [string]$Candidate
    )

    $rootFull = [System.IO.Path]::GetFullPath($Root)
    $cursor = [System.IO.Path]::GetFullPath($Candidate)
    while (-not [string]::IsNullOrWhiteSpace($cursor)) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "EnvironmentRoot must not traverse a reparse point: $cursor"
            }
        }
        if ($cursor.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
            return
        }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent -eq $cursor) {
            break
        }
        $cursor = $parent
    }
    throw 'EnvironmentRoot ancestry could not be validated.'
}

function Resolve-ExistingDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "$Label directory does not exist: $Path"
    }
    (Resolve-Path -LiteralPath $Path).Path
}

function Resolve-ExistingFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label file does not exist: $Path"
    }
    (Resolve-Path -LiteralPath $Path).Path
}

function ConvertTo-PythonString {
    param([Parameter(Mandatory)][string]$Value)
    $Value | ConvertTo-Json -Compress
}

function ConvertTo-YamlSingleQuoted {
    param([Parameter(Mandatory)][string]$Value)
    "'$(($Value.Replace('\', '/')).Replace("'", "''"))'"
}

function Write-Utf8File {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Content
    )

    $parent = Split-Path -Parent $Path
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Remove-GeneratedDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Environment
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }
    Assert-ChildPath -Root $Environment -Candidate $Path -Label 'generated cleanup target'
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to remove a generated reparse point: $Path"
    }
    Remove-Item -LiteralPath $Path -Recurse -Force
}

if ([string]::IsNullOrWhiteSpace($EnvironmentRoot)) {
    $EnvironmentRoot = Join-Path $artifactsRoot 'comfy-test'
}
$environmentFull = [System.IO.Path]::GetFullPath($EnvironmentRoot)
Assert-ChildPath -Root $artifactsRoot -Candidate $environmentFull -Label 'EnvironmentRoot'
Assert-NoReparseAncestor -Root $artifactsRoot -Candidate $environmentFull
[System.IO.Directory]::CreateDirectory($environmentFull) | Out-Null

$listeners = @(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue)
if ($listeners.Count -gt 0) {
    throw "Port $Port is already in use. Stop that process or select another isolated port."
}

if ([string]::IsNullOrWhiteSpace($ComfyRoot)) {
    if (-not [string]::IsNullOrWhiteSpace($env:LATENTDECK_COMFY_ROOT)) {
        $ComfyRoot = $env:LATENTDECK_COMFY_ROOT
    }
    else {
        $ComfyRoot = Join-Path $repoRoot '..\h3-pipeline\ComfyUI_windows_portable\ComfyUI'
    }
}
$comfyFull = Resolve-ExistingDirectory -Path $ComfyRoot -Label 'ComfyRoot'
$mainPath = Resolve-ExistingFile -Path (Join-Path $comfyFull 'main.py') -Label 'ComfyUI main.py'
[void](Resolve-ExistingFile -Path (Join-Path $comfyFull 'folder_paths.py') -Label 'ComfyUI folder_paths.py')
$sourceExtraModelConfig = Join-Path $comfyFull 'extra_model_paths.yaml'
if (Test-Path -LiteralPath $sourceExtraModelConfig -PathType Leaf) {
    throw "ComfyRoot has an active extra_model_paths.yaml, so isolated custom-node discovery cannot be guaranteed: $sourceExtraModelConfig"
}

if ([string]::IsNullOrWhiteSpace($PythonExecutable)) {
    if (-not [string]::IsNullOrWhiteSpace($env:LATENTDECK_COMFY_PYTHON)) {
        $PythonExecutable = $env:LATENTDECK_COMFY_PYTHON
    }
    else {
        $PythonExecutable = Join-Path (Split-Path -Parent $comfyFull) 'python_embeded\python.exe'
    }
}
$pythonFull = Resolve-ExistingFile -Path $PythonExecutable -Label 'Embedded Python'

if ([string]::IsNullOrWhiteSpace($ModelsRoot)) {
    $ModelsRoot = Join-Path $comfyFull 'models'
}
$modelsFull = Resolve-ExistingDirectory -Path $ModelsRoot -Label 'ModelsRoot'
$vaeDirectory = Resolve-ExistingDirectory -Path (Join-Path $modelsFull 'vae') -Label 'H3 VAE model'
$approxDirectory = Resolve-ExistingDirectory -Path (Join-Path $modelsFull 'vae_approx') -Label 'TAEHV model'

$taeh3Path = Resolve-ExistingFile -Path (Join-Path $approxDirectory 'taeh3.safetensors') `
    -Label 'TAEH3 decoder weight'
$expectedTaeh3Hash = '4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13'
$taeh3Hash = (Get-FileHash -LiteralPath $taeh3Path -Algorithm SHA256).Hash.ToLowerInvariant()
if ($taeh3Hash -cne $expectedTaeh3Hash) {
    throw "TAEH3 hash is not an accepted 0.1 variant: $taeh3Hash"
}

if ([string]::IsNullOrWhiteSpace($HqVaePath)) {
    $hqCandidates = @(
        Get-ChildItem -LiteralPath $vaeDirectory -File -Filter '*.safetensors' |
            Where-Object { $_.Name -match '(?i)(minimax.*h3.*vae|h3.*vae)' } |
            Sort-Object Name
    )
    if ($hqCandidates.Count -eq 0) {
        throw 'No native MiniMax H3 VAE Safetensors file was found in the external VAE directory.'
    }
    if ($hqCandidates.Count -gt 1) {
        throw 'Multiple native H3 VAE candidates were found. Pass -HqVaePath explicitly.'
    }
    $HqVaePath = $hqCandidates[0].FullName
}
$hqVaeFull = Resolve-ExistingFile -Path $HqVaePath -Label 'Native H3 HQ VAE'
Assert-ChildPath -Root $vaeDirectory -Candidate $hqVaeFull -Label 'HqVaePath'
if ([System.IO.Path]::GetExtension($hqVaeFull) -cne '.safetensors') {
    throw 'HqVaePath must select a Safetensors file.'
}

$stageRoot = Join-Path $environmentFull ".prepare-$([guid]::NewGuid().ToString('N'))"
$wheelStage = Join-Path $stageRoot 'wheels'
$overlayStage = Join-Path $stageRoot 'python_packages'
[System.IO.Directory]::CreateDirectory($wheelStage) | Out-Null
[System.IO.Directory]::CreateDirectory($overlayStage) | Out-Null

$uvCommand = Get-Command uv -CommandType Application -ErrorAction Stop
$packageSpecs = @(
    [ordered]@{ project = 'latentdeck-cartridge'; path = 'sdk/python'; wheel = 'latentdeck_cartridge-*.whl' },
    [ordered]@{ project = 'latentdeck-codec-host'; path = 'codec-host/python'; wheel = 'latentdeck_codec_host-*.whl' },
    [ordered]@{ project = 'latentdeck-operator-d2'; path = 'operators/builtin/d2'; wheel = 'latentdeck_operator_d2-*.whl' },
    [ordered]@{ project = 'latentdeck-operator-q4'; path = 'operators/builtin/q4'; wheel = 'latentdeck_operator_q4-*.whl' },
    [ordered]@{ project = 'latentdeck-comfy-toolkit'; path = 'comfy/toolkit'; wheel = 'latentdeck_comfy_toolkit-*.whl' },
    [ordered]@{ project = 'latentdeck-comfy-cartridge'; path = 'comfy/latent-cartridge'; wheel = 'latentdeck_comfy_cartridge-*.whl' },
    [ordered]@{ project = 'latentdeck-example-channel-roll'; path = 'operators/examples/channel-roll'; wheel = 'latentdeck_example_channel_roll-*.whl' }
)

try {
    $wheelFiles = [System.Collections.Generic.List[System.IO.FileInfo]]::new()
    foreach ($spec in $packageSpecs) {
        $source = Join-Path $repoRoot $spec.path
        & $uvCommand.Source build --wheel $source --out-dir $wheelStage --no-create-gitignore
        if ($LASTEXITCODE -ne 0) {
            throw "Wheel build failed for $($spec.project) with exit code $LASTEXITCODE."
        }
        $matches = @(Get-ChildItem -LiteralPath $wheelStage -File -Filter $spec.wheel)
        if ($matches.Count -ne 1) {
            throw "Expected exactly one wheel for $($spec.project), found $($matches.Count)."
        }
        $wheelFiles.Add($matches[0])
    }

    $installArguments = @(
        'pip',
        'install',
        '--python', $pythonFull,
        '--target', $overlayStage,
        '--no-deps',
        '--no-index',
        '--link-mode', 'copy',
        '--reinstall'
    ) + @($wheelFiles | ForEach-Object { $_.FullName })
    & $uvCommand.Source @installArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Wheel target installation failed with exit code $LASTEXITCODE."
    }

    $finalOverlay = Join-Path $environmentFull 'python_packages'
    $finalWheels = Join-Path $environmentFull 'wheels'
    $backupRoot = Join-Path $environmentFull ".backup-$([guid]::NewGuid().ToString('N'))"
    [System.IO.Directory]::CreateDirectory($backupRoot) | Out-Null
    $overlayBackup = Join-Path $backupRoot 'python_packages'
    $wheelsBackup = Join-Path $backupRoot 'wheels'
    $overlayReplaced = $false
    $wheelsReplaced = $false
    try {
        if (Test-Path -LiteralPath $finalOverlay) {
            Move-Item -LiteralPath $finalOverlay -Destination $overlayBackup
            $overlayReplaced = $true
        }
        if (Test-Path -LiteralPath $finalWheels) {
            Move-Item -LiteralPath $finalWheels -Destination $wheelsBackup
            $wheelsReplaced = $true
        }
        Move-Item -LiteralPath $overlayStage -Destination $finalOverlay
        Move-Item -LiteralPath $wheelStage -Destination $finalWheels
    }
    catch {
        if (Test-Path -LiteralPath $finalOverlay) {
            Remove-GeneratedDirectory -Path $finalOverlay -Environment $environmentFull
        }
        if (Test-Path -LiteralPath $finalWheels) {
            Remove-GeneratedDirectory -Path $finalWheels -Environment $environmentFull
        }
        if ($overlayReplaced -and (Test-Path -LiteralPath $overlayBackup)) {
            Move-Item -LiteralPath $overlayBackup -Destination $finalOverlay
        }
        if ($wheelsReplaced -and (Test-Path -LiteralPath $wheelsBackup)) {
            Move-Item -LiteralPath $wheelsBackup -Destination $finalWheels
        }
        throw
    }
    finally {
        if (Test-Path -LiteralPath $backupRoot) {
            Remove-GeneratedDirectory -Path $backupRoot -Environment $environmentFull
        }
    }

    $baseDirectory = Join-Path $environmentFull 'comfy-base'
    $customNodes = Join-Path $baseDirectory 'custom_nodes'
    $inputDirectory = Join-Path $baseDirectory 'input'
    $outputDirectory = Join-Path $baseDirectory 'output'
    $userDirectory = Join-Path $baseDirectory 'user'
    $databasePath = Join-Path $userDirectory 'comfyui.db'
    $tempDirectory = Join-Path $baseDirectory 'temp'
    $modelsDirectory = Join-Path $baseDirectory 'models'
    foreach ($directory in @(
        $customNodes,
        $inputDirectory,
        $outputDirectory,
        $userDirectory,
        $tempDirectory,
        $modelsDirectory
    )) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }

    $toolkitShim = Join-Path $customNodes 'latentdeck_toolkit\__init__.py'
    $recorderShim = Join-Path $customNodes 'comfyui_latent_cartridge\__init__.py'
    $exampleShim = Join-Path $customNodes 'latentdeck_example_channel_roll\__init__.py'
    Write-Utf8File -Path $toolkitShim -Content @'
"""Generated ComfyUI discovery shim for the isolated LatentDeck test profile."""
from latentdeck_comfy_toolkit import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS, WEB_DIRECTORY

__all__ = ["NODE_CLASS_MAPPINGS", "NODE_DISPLAY_NAME_MAPPINGS", "WEB_DIRECTORY"]
'@
    Write-Utf8File -Path $recorderShim -Content @'
"""Generated ComfyUI discovery shim for the isolated LatentCartridge test profile."""
from latentdeck_comfy_cartridge import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS

__all__ = ["NODE_CLASS_MAPPINGS", "NODE_DISPLAY_NAME_MAPPINGS"]
'@
    Write-Utf8File -Path $exampleShim -Content @'
"""Generated ComfyUI discovery shim for the isolated external-operator example."""
from latentdeck_example_channel_roll import NODE_CLASS_MAPPINGS, NODE_DISPLAY_NAME_MAPPINGS

__all__ = ["NODE_CLASS_MAPPINGS", "NODE_DISPLAY_NAME_MAPPINGS"]
'@

    $modelConfigPath = Join-Path $environmentFull 'extra_model_paths.yaml'
    $modelConfig = @"
latentdeck_external_h3:
  base_path: $(ConvertTo-YamlSingleQuoted -Value $comfyFull)
  is_default: false
  vae: 'models/vae'
  vae_approx: 'models/vae_approx'
"@
    Write-Utf8File -Path $modelConfigPath -Content ($modelConfig.TrimStart() + "`n")

    $bootstrapPath = Join-Path $environmentFull 'bootstrap_comfy.py'
    $bootstrap = @"
"""Generated private bootstrap; absolute paths stay below ignored artifacts."""
from __future__ import annotations

import runpy
import sys

OVERLAY = $(ConvertTo-PythonString -Value $finalOverlay)
COMFY_ROOT = $(ConvertTo-PythonString -Value $comfyFull)
COMFY_MAIN = $(ConvertTo-PythonString -Value $mainPath)

sys.path.insert(0, COMFY_ROOT)
sys.path.insert(0, OVERLAY)
sys.argv[0] = COMFY_MAIN
runpy.run_path(COMFY_MAIN, run_name="__main__")
"@
    Write-Utf8File -Path $bootstrapPath -Content ($bootstrap.TrimStart() + "`n")

    $smokePath = Join-Path $environmentFull 'smoke.py'
    $smoke = @"
"""Generated no-server smoke for the private isolated LatentDeck Comfy profile."""
from __future__ import annotations

import asyncio
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import sys

OVERLAY = Path($(ConvertTo-PythonString -Value $finalOverlay))
COMFY_ROOT = Path($(ConvertTo-PythonString -Value $comfyFull))
COMFY_MAIN = Path($(ConvertTo-PythonString -Value $mainPath))
BASE_DIRECTORY = Path($(ConvertTo-PythonString -Value $baseDirectory))
DATABASE = Path($(ConvertTo-PythonString -Value $databasePath))
MODEL_CONFIG = Path($(ConvertTo-PythonString -Value $modelConfigPath))
MODELS_ROOT = Path($(ConvertTo-PythonString -Value $modelsFull))
TAEH3 = Path($(ConvertTo-PythonString -Value $taeh3Path))
HQ_H3_VAE = Path($(ConvertTo-PythonString -Value $hqVaeFull))
EXPECTED_TAEH3_SHA256 = $(ConvertTo-PythonString -Value $expectedTaeh3Hash)

sys.path.insert(0, str(COMFY_ROOT))
sys.path.insert(0, str(OVERLAY))
sys.argv = [
    str(COMFY_MAIN),
    "--base-directory", str(BASE_DIRECTORY),
    "--database-url", "sqlite:///" + DATABASE.as_posix(),
    "--extra-model-paths-config", str(MODEL_CONFIG),
    "--cpu",
    "--disable-api-nodes",
    "--disable-all-custom-nodes",
    "--whitelist-custom-nodes", "latentdeck_toolkit", "comfyui_latent_cartridge", "latentdeck_example_channel_roll",
]

import comfy.options

comfy.options.enable_args_parsing()
import folder_paths
from utils.extra_config import load_extra_path_config

load_extra_path_config(str(MODEL_CONFIG))

import safetensors
from safetensors import safe_open
import torch

import latentdeck_cartridge
import latentdeck_codec_host
import latentdeck_operator_d2
import latentdeck_operator_q4
import latentdeck_comfy_toolkit
import latentdeck_comfy_cartridge
import latentdeck_example_channel_roll

required_distributions = {
    "latentdeck-cartridge",
    "latentdeck-codec-host",
    "latentdeck-operator-d2",
    "latentdeck-operator-q4",
    "latentdeck-comfy-toolkit",
    "latentdeck-comfy-cartridge",
    "latentdeck-example-channel-roll",
}
package_versions = {
    name: importlib.metadata.version(name) for name in sorted(required_distributions)
}
if set(package_versions.values()) != {"0.1.0"}:
    raise RuntimeError(f"unexpected LatentDeck package versions: {package_versions}")

if not sys.version.startswith("3.13."):
    raise RuntimeError(f"expected Python 3.13, got {sys.version}")
if torch.__version__ != "2.13.0+cu130" or torch.version.cuda != "13.0":
    raise RuntimeError(
        f"expected torch 2.13.0+cu130 / CUDA 13.0, got {torch.__version__} / {torch.version.cuda}"
    )
if safetensors.__version__ != "0.8.0":
    raise RuntimeError(f"expected safetensors 0.8.0, got {safetensors.__version__}")

expected_custom_nodes = os.path.normcase(os.path.normpath(str(BASE_DIRECTORY / "custom_nodes")))
actual_custom_nodes = {
    os.path.normcase(os.path.normpath(path))
    for path in folder_paths.get_folder_paths("custom_nodes")
}
if expected_custom_nodes not in actual_custom_nodes:
    raise RuntimeError(f"isolated custom_nodes path is not active: {actual_custom_nodes}")

expected_vae = os.path.normcase(os.path.normpath(str(MODELS_ROOT / "vae")))
expected_approx = os.path.normcase(os.path.normpath(str(MODELS_ROOT / "vae_approx")))
actual_vae = {
    os.path.normcase(os.path.normpath(path)) for path in folder_paths.get_folder_paths("vae")
}
actual_approx = {
    os.path.normcase(os.path.normpath(path))
    for path in folder_paths.get_folder_paths("vae_approx")
}
if expected_vae not in actual_vae or expected_approx not in actual_approx:
    raise RuntimeError("external H3 VAE paths were not registered")
if TAEH3.name not in folder_paths.get_filename_list("vae_approx"):
    raise RuntimeError("taeh3 is not visible through ComfyUI model discovery")
if HQ_H3_VAE.name not in folder_paths.get_filename_list("vae"):
    raise RuntimeError("native H3 VAE is not visible through ComfyUI model discovery")

import nodes

asyncio.run(nodes.init_external_custom_nodes())
toolkit_nodes = set(latentdeck_comfy_toolkit.NODE_CLASS_MAPPINGS)
recorder_nodes = set(latentdeck_comfy_cartridge.NODE_CLASS_MAPPINGS)
example_nodes = set(latentdeck_example_channel_roll.NODE_CLASS_MAPPINGS)
expected_example_nodes = {
    "LatentDeckExampleChannelRoll",
    "LatentDeckExampleChannelRollHook",
}
missing_nodes = (toolkit_nodes | recorder_nodes | example_nodes) - set(nodes.NODE_CLASS_MAPPINGS)
if missing_nodes:
    raise RuntimeError(f"ComfyUI discovery omitted LatentDeck nodes: {sorted(missing_nodes)}")
if "LatentDeckSaveLatentCartridge" not in recorder_nodes:
    raise RuntimeError("recorder package omitted LatentDeckSaveLatentCartridge")
if not toolkit_nodes:
    raise RuntimeError("Toolkit package exported no ComfyUI nodes")
if example_nodes != expected_example_nodes:
    raise RuntimeError(f"example package exported unexpected nodes: {sorted(example_nodes)}")

taeh3_digest = hashlib.sha256()
with TAEH3.open("rb") as stream:
    while block := stream.read(1024 * 1024):
        taeh3_digest.update(block)
if taeh3_digest.hexdigest() != EXPECTED_TAEH3_SHA256:
    raise RuntimeError("taeh3 changed after environment preparation")

with safe_open(HQ_H3_VAE, framework="pt", device="cpu") as handle:
    hq_tensor_count = len(handle.keys())
if hq_tensor_count == 0:
    raise RuntimeError("native H3 VAE Safetensors contains no tensors")

result = {
    "status": "ok",
    "python": {"version": sys.version.split()[0], "executable": sys.executable},
    "torch": {
        "version": torch.__version__,
        "cuda_build": torch.version.cuda,
        "cuda_available": torch.cuda.is_available(),
    },
    "safetensors": {"version": safetensors.__version__},
    "packages": package_versions,
    "discovery": {
        "recorder": "LatentDeckSaveLatentCartridge" in nodes.NODE_CLASS_MAPPINGS,
        "example": "LatentDeckExampleChannelRoll" in nodes.NODE_CLASS_MAPPINGS,
        "example_hook": "LatentDeckExampleChannelRollHook" in nodes.NODE_CLASS_MAPPINGS,
        "example_nodes": sorted(example_nodes),
        "toolkit_node_count": len(toolkit_nodes),
        "toolkit_nodes": sorted(toolkit_nodes),
    },
    "models": {
        "taeh3": {
            "status": "verified",
            "path": str(TAEH3),
            "sha256": taeh3_digest.hexdigest(),
            "byte_length": TAEH3.stat().st_size,
        },
        "hq_h3_vae": {
            "status": "available",
            "path": str(HQ_H3_VAE),
            "byte_length": HQ_H3_VAE.stat().st_size,
            "tensor_count": hq_tensor_count,
        },
    },
}
print("LATENTDECK_SMOKE_JSON=" + json.dumps(result, sort_keys=True))
"@
    Write-Utf8File -Path $smokePath -Content ($smoke.TrimStart() + "`n")

    $finalWheelInventory = @(
        foreach ($spec in $packageSpecs) {
            $wheel = @(Get-ChildItem -LiteralPath $finalWheels -File -Filter $spec.wheel)
            if ($wheel.Count -ne 1) {
                throw "Final wheel inventory is invalid for $($spec.project)."
            }
            [ordered]@{
                project = $spec.project
                filename = $wheel[0].Name
                byte_length = [int64]$wheel[0].Length
                sha256 = (Get-FileHash -LiteralPath $wheel[0].FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
    )
    $gitCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to identify the repository commit for the environment receipt.'
    }
    $gitDirty = @(& git -C $repoRoot status --short).Count -gt 0

    $receipt = [ordered]@{
        schema_version = 1
        private_artifact = $true
        distributable = $false
        generated_utc = [DateTime]::UtcNow.ToString('o')
        source_policy = 'read_only_external_models'
        environment_root = $environmentFull
        port = $Port
        repository = [ordered]@{
            root = $repoRoot
            commit = $gitCommit
            dirty_at_build = $gitDirty
        }
        comfy = [ordered]@{
            root = $comfyFull
            main = $mainPath
            main_sha256 = (Get-FileHash -LiteralPath $mainPath -Algorithm SHA256).Hash.ToLowerInvariant()
            source_policy = 'read_only'
        }
        python = [ordered]@{
            executable = $pythonFull
        }
        paths = [ordered]@{
            base_directory = $baseDirectory
            python_packages = $finalOverlay
            wheels = $finalWheels
            custom_nodes = $customNodes
            input = $inputDirectory
            output = $outputDirectory
            user = $userDirectory
            database = $databasePath
            temp = $tempDirectory
            bootstrap = $bootstrapPath
            smoke = $smokePath
            extra_model_paths = $modelConfigPath
        }
        packages = [ordered]@{
            install_mode = 'wheel_target_no_deps'
            wheels = $finalWheelInventory
        }
        external_models = [ordered]@{
            copied = $false
            models_root = $modelsFull
            taeh3 = [ordered]@{
                path = $taeh3Path
                sha256 = $taeh3Hash
                byte_length = [int64](Get-Item -LiteralPath $taeh3Path).Length
            }
            hq_h3_vae = [ordered]@{
                path = $hqVaeFull
                byte_length = [int64](Get-Item -LiteralPath $hqVaeFull).Length
            }
        }
    }
    $receiptPath = Join-Path $environmentFull 'environment.json'
    Write-Utf8File -Path $receiptPath -Content (($receipt | ConvertTo-Json -Depth 20) + "`n")

    & (Join-Path $PSScriptRoot 'Test-IsolatedComfyEnvironment.ps1') `
        -EnvironmentRoot $environmentFull
    if ($LASTEXITCODE -ne 0) {
        throw "Prepared environment smoke failed with exit code $LASTEXITCODE."
    }

    Write-Host "Prepared isolated Comfy test environment: $environmentFull" -ForegroundColor Green
    Write-Host "Start with: pwsh -NoProfile -File tools/Start-IsolatedComfyEnvironment.ps1"
    Write-Warning 'The generated environment links external model paths and is private/non-distributable.'
}
finally {
    if (Test-Path -LiteralPath $stageRoot) {
        Remove-GeneratedDirectory -Path $stageRoot -Environment $environmentFull
    }
}
