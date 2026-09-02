[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PythonRuntimeRoot,

    [Parameter(Mandatory)]
    [string]$PythonSitePackages,

    [Parameter(Mandatory)]
    [string]$WorkerSitePackages,

    [Parameter(Mandatory)]
    [string]$OutputRoot,

    [string]$PackVersion = '0.1.0'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$runtimeSource = (Resolve-Path -LiteralPath $PythonRuntimeRoot).Path
$pythonPackages = (Resolve-Path -LiteralPath $PythonSitePackages).Path
$workerPackages = (Resolve-Path -LiteralPath $WorkerSitePackages).Path

if (-not (Test-Path -LiteralPath $runtimeSource -PathType Container)) {
    throw 'PythonRuntimeRoot must be an existing directory.'
}
if (-not (Test-Path -LiteralPath $pythonPackages -PathType Container)) {
    throw 'PythonSitePackages must be an existing directory.'
}
if (-not (Test-Path -LiteralPath $workerPackages -PathType Container)) {
    throw 'WorkerSitePackages must be an existing directory.'
}
$workerModuleRoot = Join-Path $workerPackages 'latentdeck_codec_h3'
foreach ($entrypoint in @('__init__.py', 'worker.py')) {
    $entrypointPath = Join-Path $workerModuleRoot $entrypoint
    if (-not (Test-Path -LiteralPath $entrypointPath -PathType Leaf)) {
        throw "WorkerSitePackages does not contain required H3 entrypoint latentdeck_codec_h3/$entrypoint."
    }
    $entrypointItem = Get-Item -LiteralPath $entrypointPath -Force
    if (($entrypointItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Linked Codec Pack entrypoint must be a regular file: latentdeck_codec_h3/$entrypoint."
    }
}
if ($PackVersion -notmatch '^0\.1\.0$') {
    throw 'The linked development pack currently supports only version 0.1.0.'
}

$outputBase = [System.IO.Path]::GetFullPath($OutputRoot)
$packRoot = Join-Path $outputBase (Join-Path 'org.latentdeck.h3' $PackVersion)
if (Test-Path -LiteralPath $packRoot) {
    throw "Refusing to replace an existing development pack: $packRoot"
}

$runtimeTarget = Join-Path $packRoot 'runtime'
New-Item -ItemType Directory -Path $runtimeTarget -Force | Out-Null

$runtimeFiles = @(Get-ChildItem -LiteralPath $runtimeSource -File)
if (-not ($runtimeFiles.Name -contains 'python.exe')) {
    throw 'PythonRuntimeRoot does not contain python.exe.'
}
$pthFiles = @($runtimeFiles | Where-Object { $_.Name -match '^python\d+\._pth$' })
if ($pthFiles.Count -ne 1) {
    throw 'PythonRuntimeRoot must contain exactly one pythonNNN._pth file.'
}

foreach ($file in $runtimeFiles) {
    if ($file.FullName -ne $pthFiles[0].FullName) {
        Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $runtimeTarget $file.Name)
    }
}

$stdlibArchive = $runtimeFiles |
    Where-Object { $_.Name -match '^python\d+\.zip$' } |
    Select-Object -First 1
if ($null -eq $stdlibArchive) {
    throw 'PythonRuntimeRoot does not contain its standard-library archive.'
}

# This file is deliberately machine-local and is written only below the
# caller-selected output root. It links a known working Python/CUDA laboratory
# to the public worker packages for private end-to-end development. It is not a
# distributable or self-contained Codec Pack.
$linkedPackagePaths = [System.Collections.Generic.List[string]]::new()
$linkedPackagePaths.Add($workerPackages)
foreach ($pthFile in Get-ChildItem -LiteralPath $workerPackages -Filter '*.pth' -File) {
    foreach ($line in Get-Content -LiteralPath $pthFile.FullName) {
        $candidate = $line.Trim()
        if ([string]::IsNullOrWhiteSpace($candidate) -or
            $candidate.StartsWith('#') -or
            $candidate.StartsWith('import ')) {
            continue
        }
        if (-not [System.IO.Path]::IsPathFullyQualified($candidate)) {
            $candidate = Join-Path $workerPackages $candidate
        }
        if (Test-Path -LiteralPath $candidate -PathType Container) {
            $resolved = (Resolve-Path -LiteralPath $candidate).Path
            if (-not $linkedPackagePaths.Contains($resolved)) {
                $linkedPackagePaths.Add($resolved)
            }
        }
    }
}
$linkedPackagePaths.Add($pythonPackages)
$pthLines = @(
    $stdlibArchive.Name
    '.'
    $linkedPackagePaths
    'import site'
)
$pthTarget = Join-Path $runtimeTarget $pthFiles[0].Name
[System.IO.File]::WriteAllLines($pthTarget, $pthLines, [System.Text.UTF8Encoding]::new($false))

$noticeSource = Join-Path $repoRoot 'codec-host/codecs/h3/THIRD_PARTY_NOTICES.md'
Copy-Item -LiteralPath $noticeSource -Destination (Join-Path $packRoot 'THIRD_PARTY_NOTICES.md')

function Get-PortableRelativePath {
    param(
        [Parameter(Mandatory)]
        [string]$BasePath,
        [Parameter(Mandatory)]
        [string]$TargetPath
    )

    [System.IO.Path]::GetRelativePath($BasePath, $TargetPath).Replace('\', '/')
}

function Get-MeasuredFile {
    param(
        [Parameter(Mandatory)]
        [string]$BasePath,
        [Parameter(Mandatory)]
        [System.IO.FileInfo]$File
    )

    [ordered]@{
        path        = Get-PortableRelativePath -BasePath $BasePath -TargetPath $File.FullName
        byte_length = [int64]$File.Length
        sha256      = (Get-FileHash -LiteralPath $File.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

$catalogEntries = @(
    Get-ChildItem -LiteralPath $packRoot -Recurse -File |
        Sort-Object FullName |
        ForEach-Object { Get-MeasuredFile -BasePath $packRoot -File $_ }
)
$catalog = [ordered]@{
    manifest_version = '1.0.0'
    files            = $catalogEntries
}
$catalogPath = Join-Path $packRoot 'integrity.json'
$catalogJson = $catalog | ConvertTo-Json -Depth 8
[System.IO.File]::WriteAllText(
    $catalogPath,
    $catalogJson + "`n",
    [System.Text.UTF8Encoding]::new($false)
)
$catalogHash = (Get-FileHash -LiteralPath $catalogPath -Algorithm SHA256).Hash.ToLowerInvariant()

$manifest = [ordered]@{
    manifest_version = '1.0.0'
    pack_id          = 'org.latentdeck.h3'
    pack_version     = $PackVersion
    display_name     = 'LatentDeck H3 Codec Pack (linked development runtime)'
    publisher        = [ordered]@{
        name = 'LatentDeck contributors'
        url  = $null
    }
    license          = [ordered]@{
        spdx_or_label = 'Apache-2.0 AND MIT'
        notice_path   = 'THIRD_PARTY_NOTICES.md'
    }
    platform         = [ordered]@{
        os   = 'windows'
        arch = 'x86_64'
    }
    compatibility    = [ordered]@{
        app_min_inclusive   = '0.1.0'
        app_max_exclusive   = '0.2.0'
        worker_protocol_min = 1
        worker_protocol_max = 1
        lc_spec_versions    = @('0.1.0')
        profiles            = @(
            [ordered]@{
                codec_family    = 'minimax_h3'
                profile         = 'h3_av_latent'
                profile_versions = @('0.1.0')
            }
        )
    }
    worker           = [ordered]@{
        executable       = 'runtime/python.exe'
        arguments        = @('-B', '-s', '-m', 'latentdeck_codec_h3.worker')
        working_directory = 'runtime'
        probe_timeout_ms = 120000
    }
    adapter          = [ordered]@{
        adapter_id      = 'org.latentdeck.h3'
        adapter_version = '0.1.0'
    }
    integrity        = [ordered]@{
        catalog_path   = 'integrity.json'
        catalog_sha256 = $catalogHash
    }
    external_assets  = @(
        [ordered]@{
            asset_id          = 'taeh3'
            display_name      = 'TAEH3 decoder weight'
            kind              = 'decoder_weight'
            required          = $true
            selection         = 'explicit_file'
            format            = 'safetensors'
            accepted_variants = @(
                [ordered]@{
                    variant_id   = 'madebyollin-taeh3-e743234f'
                    sha256       = '4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13'
                    byte_length  = 22709752
                    source_url   = 'https://huggingface.co/madebyollin/taehv/resolve/main/taeh3.safetensors'
                    license_label = 'MIT'
                    license_url  = 'https://github.com/madebyollin/taehv/blob/e743234f/LICENSE'
                }
            )
        }
    )
}

$manifestPath = Join-Path $packRoot 'codec-pack.json'
$manifestJson = $manifest | ConvertTo-Json -Depth 12
[System.IO.File]::WriteAllText(
    $manifestPath,
    $manifestJson + "`n",
    [System.Text.UTF8Encoding]::new($false)
)

Write-Host "Created linked development Codec Pack: $packRoot"
Write-Warning 'This pack links external machine-local Python packages and must never be distributed.'
