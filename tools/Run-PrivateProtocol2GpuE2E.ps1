[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$BaseRoot,

    [Parameter(Mandatory)]
    [string]$WorkRoot,

    [Parameter(Mandatory)]
    [string]$ReceiptPath,

    [Parameter(Mandatory)]
    [string]$Taeh3Path,

    [Parameter(Mandatory)]
    [string]$Source1,

    [Parameter(Mandatory)]
    [string]$Source2,

    [Parameter(Mandatory)]
    [string]$Source3,

    [Parameter(Mandatory)]
    [string]$Source4
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$repoRoot = Split-Path -Parent $PSScriptRoot
$repoRootFull = (Resolve-Path -LiteralPath $repoRoot).Path
$manifest = Join-Path $repoRoot 'apps/latentdeck/src-tauri/Cargo.toml'
$validator = Join-Path $PSScriptRoot 'Test-PrivateProtocol2GpuGate.ps1'

function Resolve-ExistingFile([string]$Value, [string]$Label) {
    if (-not [System.IO.Path]::IsPathFullyQualified($Value)) {
        throw "$Label must be one existing absolute regular file."
    }
    $fullPath = [System.IO.Path]::GetFullPath($Value)
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        throw "$Label must be one existing absolute regular file."
    }
    $item = Get-Item -LiteralPath $fullPath -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0) {
        throw "$Label must be one non-empty, non-reparse regular file."
    }
    return $item.FullName
}

function Resolve-ExistingDirectory([string]$Value, [string]$Label) {
    if (-not [System.IO.Path]::IsPathFullyQualified($Value)) {
        throw "$Label must be one existing absolute directory."
    }
    $fullPath = [System.IO.Path]::GetFullPath($Value)
    if (-not (Test-Path -LiteralPath $fullPath -PathType Container)) {
        throw "$Label must be one existing absolute directory."
    }
    $item = Get-Item -LiteralPath $fullPath -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Label cannot be a reparse point."
    }
    return $item.FullName
}

function Resolve-NewPath([string]$Value, [string]$Label) {
    if (-not [System.IO.Path]::IsPathFullyQualified($Value)) {
        throw "$Label must be one new absolute path."
    }
    $fullPath = [System.IO.Path]::GetFullPath($Value)
    if (Test-Path -LiteralPath $fullPath) {
        throw "$Label must be one new absolute path."
    }
    $parent = Split-Path -Parent $fullPath
    if ([string]::IsNullOrWhiteSpace($parent) -or
        -not (Test-Path -LiteralPath $parent -PathType Container)) {
        throw "$Label parent directory must already exist."
    }
    $parentItem = Get-Item -LiteralPath $parent -Force
    if (($parentItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Label parent cannot be a reparse point."
    }
    return $fullPath
}

function Assert-OutsideRepository([string]$Value, [string]$Label) {
    $repositoryPrefix = $repoRootFull.TrimEnd([char[]]@('\', '/')) +
        [System.IO.Path]::DirectorySeparatorChar
    if ($Value.Equals($repoRootFull, [System.StringComparison]::OrdinalIgnoreCase) -or
        $Value.StartsWith($repositoryPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label must remain outside the source repository."
    }
}

$baseRootFull = Resolve-ExistingDirectory $BaseRoot 'Extension base root'
$workRootFull = Resolve-NewPath $WorkRoot 'Private work root'
$receiptFull = Resolve-NewPath $ReceiptPath 'Private receipt'
if ($workRootFull -eq $receiptFull) {
    throw 'Private work root and receipt must be distinct paths.'
}
$taeh3Full = Resolve-ExistingFile $Taeh3Path 'TAEH3 decoder asset'
$sourceFull = @(
    Resolve-ExistingFile $Source1 'LC source 1'
    Resolve-ExistingFile $Source2 'LC source 2'
    Resolve-ExistingFile $Source3 'LC source 3'
    Resolve-ExistingFile $Source4 'LC source 4'
)
Assert-OutsideRepository $baseRootFull 'Extension base root'
Assert-OutsideRepository $workRootFull 'Private work root'
Assert-OutsideRepository $receiptFull 'Private receipt'
Assert-OutsideRepository $taeh3Full 'TAEH3 decoder asset'
for ($index = 0; $index -lt $sourceFull.Count; $index += 1) {
    Assert-OutsideRepository $sourceFull[$index] "LC source $($index + 1)"
}

$environmentNames = @(
    'LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE',
    'LATENTDECK_PRIVATE_PROTOCOL2_BASE_ROOT',
    'LATENTDECK_PRIVATE_PROTOCOL2_WORK_ROOT',
    'LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT',
    'LATENTDECK_PRIVATE_PROTOCOL2_TAEH3',
    'LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_1',
    'LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_2',
    'LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_3',
    'LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_4'
)
$previous = @{}
foreach ($name in $environmentNames) {
    $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

Push-Location $repoRoot
try {
    $env:LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE = '1'
    $env:LATENTDECK_PRIVATE_PROTOCOL2_BASE_ROOT = $baseRootFull
    $env:LATENTDECK_PRIVATE_PROTOCOL2_WORK_ROOT = $workRootFull
    $env:LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT = $receiptFull
    $env:LATENTDECK_PRIVATE_PROTOCOL2_TAEH3 = $taeh3Full
    $env:LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_1 = $sourceFull[0]
    $env:LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_2 = $sourceFull[1]
    $env:LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_3 = $sourceFull[2]
    $env:LATENTDECK_PRIVATE_PROTOCOL2_SOURCE_4 = $sourceFull[3]

    & cargo test --locked --manifest-path $manifest `
        --features private-protocol2-gpu-e2e `
        --test private_protocol2_gpu_e2e `
        -- h3_full_matrix_executes_and_emits_receipt --ignored --exact --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw 'Executable private Protocol 2 H3 GPU E2E failed.'
    }

    & pwsh -NoProfile -File $validator -ReceiptPath $receiptFull
    if ($LASTEXITCODE -ne 0) {
        throw 'Executable private Protocol 2 H3 GPU receipt validation failed.'
    }
} finally {
    foreach ($name in $environmentNames) {
        [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
    }
    Pop-Location
}
