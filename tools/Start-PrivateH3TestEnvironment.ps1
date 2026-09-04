[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PythonRuntimeRoot,

    [Parameter(Mandatory)]
    [string]$PythonSitePackages,

    [string]$WorkerSitePackages,

    [string]$EnvironmentRoot,

    [ValidateSet('PrepareOnly', 'LatentDeck', 'LatentPlayer')]
    [string]$Mode = 'PrepareOnly'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($Mode -ceq 'LatentDeck') {
    throw (
        'LatentDeck requires an installed Protocol 2 Codec Pack. The linked ' +
        'development helper is an explicit Protocol 1 Player-only bridge.'
    )
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$artifactsRoot = (Resolve-Path -LiteralPath $artifactsRoot).Path

if ([string]::IsNullOrWhiteSpace($WorkerSitePackages)) {
    $WorkerSitePackages = Join-Path $repoRoot '.venv\Lib\site-packages'
}

$runtimeSource = (Resolve-Path -LiteralPath $PythonRuntimeRoot).Path
$pythonPackages = (Resolve-Path -LiteralPath $PythonSitePackages).Path
$workerPackages = (Resolve-Path -LiteralPath $WorkerSitePackages).Path

if ([string]::IsNullOrWhiteSpace($EnvironmentRoot)) {
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
    $suffix = [guid]::NewGuid().ToString('N').Substring(0, 8)
    $EnvironmentRoot = Join-Path $artifactsRoot "private-h3-test-$stamp-$suffix"
}
$testRoot = [System.IO.Path]::GetFullPath($EnvironmentRoot)
$relativeTestRoot = [System.IO.Path]::GetRelativePath($artifactsRoot, $testRoot)
if ($relativeTestRoot -eq '.' -or
    $relativeTestRoot.StartsWith('..', [System.StringComparison]::Ordinal) -or
    [System.IO.Path]::IsPathFullyQualified($relativeTestRoot)) {
    throw 'EnvironmentRoot must be a new child directory of the repository artifacts directory.'
}
if (Test-Path -LiteralPath $testRoot) {
    throw "Refusing to replace an existing private test environment: $testRoot"
}

# A caller-selected path must not tunnel writes out of artifacts through an
# existing junction or symbolic-link ancestor. The default path is new, but an
# explicit EnvironmentRoot may include pre-created parents.
$ancestor = [System.IO.Path]::GetDirectoryName($testRoot)
while (-not [string]::IsNullOrWhiteSpace($ancestor)) {
    if (Test-Path -LiteralPath $ancestor) {
        $ancestorItem = Get-Item -LiteralPath $ancestor -Force
        if (($ancestorItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'EnvironmentRoot must not traverse a reparse-point ancestor.'
        }
    }
    if ($ancestor.Equals($artifactsRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        break
    }
    $parent = [System.IO.Path]::GetDirectoryName($ancestor)
    if ($parent -eq $ancestor) {
        throw 'EnvironmentRoot ancestry could not be validated.'
    }
    $ancestor = $parent
}

$localAppData = Join-Path $testRoot 'local-app-data'
$programData = Join-Path $testRoot 'program-data'
$codecRoot = Join-Path $localAppData 'LatentDeck\CodecPacks'
[System.IO.Directory]::CreateDirectory($localAppData) | Out-Null
[System.IO.Directory]::CreateDirectory($programData) | Out-Null

try {
    & (Join-Path $PSScriptRoot 'New-LinkedDevCodecPack.ps1') `
        -PythonRuntimeRoot $runtimeSource `
        -PythonSitePackages $pythonPackages `
        -WorkerSitePackages $workerPackages `
        -OutputRoot $codecRoot `
        -PackVersion '0.1.0'

    $packRoot = Join-Path $codecRoot 'org.latentdeck.h3\0.1.0'
    $manifestPath = Join-Path $packRoot 'codec-pack.json'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json
    if (@($manifest.worker.arguments).Count -eq 0) {
        throw 'Prepared linked Codec Pack omitted its P1 Player worker arguments.'
    }
    foreach ($forbiddenWorkerField in @('d2_arguments', 'q4_arguments')) {
        if ($manifest.worker.PSObject.Properties.Name -contains $forbiddenWorkerField) {
            throw "P1 linked helper must not expose Deck worker.$forbiddenWorkerField."
        }
    }

    $runtimePython = Join-Path $packRoot $manifest.worker.executable.Replace('/', '\')
    $probe = @'
import importlib
importlib.import_module("latentdeck_codec_h3.worker")
'@
    & $runtimePython -B -s -c $probe
    if ($LASTEXITCODE -ne 0) {
        throw 'Linked H3 runtime could not import its P1 Player entrypoint.'
    }

    $receipt = [ordered]@{
        schema_version = 1
        purpose = 'private_linked_h3_player_p1_test'
        worker_protocol = 1
        player_only_bridge = $true
        latentdeck_supported = $false
        distributable = $false
        installed = $false
        created_utc = [DateTime]::UtcNow.ToString('o')
        environment_root = $testRoot
        local_app_data = $localAppData
        program_data = $programData
        codec_discovery_root = $codecRoot
        pack_root = $packRoot
        source_policy = 'read_only_inputs'
        entrypoints = [ordered]@{
            player = @($manifest.worker.arguments)
        }
    }
    $receiptPath = Join-Path $testRoot 'test-environment.json'
    [System.IO.File]::WriteAllText(
        $receiptPath,
        ($receipt | ConvertTo-Json -Depth 8) + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    Write-Host "Prepared isolated linked H3 test environment: $testRoot" -ForegroundColor Green
    Write-Host "Codec discovery root: $codecRoot"
    Write-Warning (
        'This environment links machine-local packages. It is private, ' +
        'non-distributable, not installed, and supports only the P1 Player bridge.'
    )

    if ($Mode -eq 'PrepareOnly') {
        Write-Output $receiptPath
        return
    }

    $nodeRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNode.ps1')
    $pnpm = Join-Path $nodeRoot 'pnpm.cmd'
    $package = '@latentdeck/player'
    $previousLocalAppData = $env:LOCALAPPDATA
    $previousProgramData = $env:PROGRAMDATA
    $previousPrivateCodecRoot = $env:LATENTDECK_PRIVATE_CODEC_ROOT
    try {
        $env:LOCALAPPDATA = $localAppData
        $env:PROGRAMDATA = $programData
        $env:LATENTDECK_PRIVATE_CODEC_ROOT = $codecRoot
        Push-Location $repoRoot
        try {
            & $pnpm --filter $package tauri dev
            if ($LASTEXITCODE -ne 0) {
                throw "$Mode test application exited with code $LASTEXITCODE."
            }
        }
        finally {
            Pop-Location
        }
    }
    finally {
        $env:LOCALAPPDATA = $previousLocalAppData
        $env:PROGRAMDATA = $previousProgramData
        $env:LATENTDECK_PRIVATE_CODEC_ROOT = $previousPrivateCodecRoot
    }
}
catch {
    Write-Warning "Private H3 test environment preparation failed. Partial files remain isolated under: $testRoot"
    throw
}
