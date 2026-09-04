[CmdletBinding()]
param(
    [string]$RepositoryRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..'
}
$repoRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$utf8 = [System.Text.UTF8Encoding]::new($false, $true)

$assets = [ordered]@{
    'docs/assets/screenshots/latentdeck-d2-missing-codec.png' = @{
        Hash = 'c3487f5d0086d70cdb4e0c02177387eb3e95fa33071d8f3827c26d83f0c55862'
        Record = 'docs/assets/screenshots/README.md'
    }
    'docs/assets/screenshots/latentdeck-library-empty.png' = @{
        Hash = '220d44d21768b29abd671aa73b97bd6922d5a25d5bc7cfec2081babab59e69d6'
        Record = 'docs/assets/screenshots/README.md'
    }
    'docs/assets/screenshots/latentdeck-q4-missing-codec.png' = @{
        Hash = '2b94081e45e1a67e1d667395656c221c82ec87b570ca7b549ed64f41b05e86c9'
        Record = 'docs/assets/screenshots/README.md'
    }
    'docs/assets/screenshots/latentplayer-empty.png' = @{
        Hash = 'fb8f1b0d58ad030fb94da954f0ece142223c14c65a4faff5925f23624b2f0d1f'
        Record = 'docs/assets/screenshots/README.md'
    }
    'apps/latentdeck/src-tauri/icons/source.svg' = @{
        Hash = '6a215c05222f77866729ea686974d6e0425754576cba55419136747c0ffd6e3e'
        Record = 'docs/assets/branding/README.md'
    }
    'apps/latentdeck/src-tauri/icons/icon.ico' = @{
        Hash = '4ce64accba31b689246794deb095c4df8b7b79bfb868f4083d96ae4debfc227b'
        Record = 'docs/assets/branding/README.md'
    }
    'apps/latentplayer/src-tauri/icons/source.svg' = @{
        Hash = '6c251651f9f17aad76d197693216fed667729345b54e5e61e9ae2dcfb390252b'
        Record = 'docs/assets/branding/README.md'
    }
    'apps/latentplayer/src-tauri/icons/icon.ico' = @{
        Hash = '38e84ce999e27b205cb24e01a84e9e087e80637c6f2dffcc7d4f0307dd9b0287'
        Record = 'docs/assets/branding/README.md'
    }
}

$trackedAssets = @(
    & git -C $repoRoot -c core.quotepath=false ls-files -- `
        'docs/assets/screenshots/*.png' `
        'apps/latentdeck/src-tauri/icons/source.svg' `
        'apps/latentdeck/src-tauri/icons/icon.ico' `
        'apps/latentplayer/src-tauri/icons/source.svg' `
        'apps/latentplayer/src-tauri/icons/icon.ico'
)
if ($LASTEXITCODE -ne 0) {
    throw 'Could not enumerate tracked public visual assets.'
}

$failures = [System.Collections.Generic.List[string]]::new()
$expectedPaths = @($assets.Keys | Sort-Object)
$actualPaths = @($trackedAssets | ForEach-Object { $_.Replace('\', '/') } | Sort-Object -Unique)
$pathDifference = @(Compare-Object -ReferenceObject $expectedPaths -DifferenceObject $actualPaths)
foreach ($difference in $pathDifference) {
    if ($difference.SideIndicator -ceq '<=') {
        $failures.Add("Expected visual asset is not tracked: $($difference.InputObject)")
    } else {
        $failures.Add("Tracked visual asset lacks a provenance record: $($difference.InputObject)")
    }
}

$recordCache = @{}
foreach ($relativePath in $assets.Keys) {
    $entry = $assets[$relativePath]
    $fullPath = Join-Path $repoRoot $relativePath
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        $failures.Add("Public visual asset is missing: $relativePath")
        continue
    }
    $item = Get-Item -LiteralPath $fullPath -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        $failures.Add("Public visual asset is a reparse point: $relativePath")
        continue
    }

    $actualHash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $expectedHash = [string]$entry.Hash
    if ($actualHash -cne $expectedHash) {
        $failures.Add("Public visual asset hash changed: $relativePath")
    }

    $recordPath = [string]$entry.Record
    if (-not $recordCache.ContainsKey($recordPath)) {
        $fullRecordPath = Join-Path $repoRoot $recordPath
        if (-not (Test-Path -LiteralPath $fullRecordPath -PathType Leaf)) {
            $failures.Add("Asset provenance record is missing: $recordPath")
            continue
        }
        try {
            $recordCache[$recordPath] = $utf8.GetString(
                [System.IO.File]::ReadAllBytes($fullRecordPath)
            )
        } catch [System.Text.DecoderFallbackException] {
            $failures.Add("Asset provenance record is not strict UTF-8: $recordPath")
            continue
        }
    }
    if (
        -not $recordCache.ContainsKey($recordPath) -or
        -not $recordCache[$recordPath].Contains($expectedHash, [System.StringComparison]::Ordinal)
    ) {
        $failures.Add("Asset hash is absent from its provenance record: $relativePath")
    }
}

if ($failures.Count -gt 0) {
    $details = ($failures | Sort-Object -Unique | ForEach-Object { " - $_" }) -join "`n"
    throw "Public asset provenance audit failed:`n$details"
}

Write-Host "PUBLIC ASSET PROVENANCE: PASS ($($assets.Count) files)" -ForegroundColor Green
