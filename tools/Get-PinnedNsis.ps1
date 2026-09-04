[CmdletBinding()]
param(
    [string]$NsisRoot,

    [switch]$IncludeTauriUtils,

    [switch]$AllowNetwork
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$nsisVersion = '3.11'
$archiveName = "nsis-$nsisVersion.zip"
$archiveSha256 = 'c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1'
$launcherSha256 = 'f497e92deb9f179f7b8f7553fcb3bd04f511bb2949e5e4a2aee80a10f7b20431'
$compilerSha256 = '42850802704ecb11163f7e0329d35ee54bd288953200d4966e226d572848cfc5'
$copyingSha256 = 'e7dd514003ab96cb3ddccbc028fe5c795fccf57dc41f21cfb9d4dd16ead23bf5'
$tauriUtilsVersion = '0.5.3'
$tauriUtilsByteLength = 34304
$tauriUtilsSha1 = '75197fee3c6a814fe035788d1c34ead39349b860'
$tauriUtilsSha256 = '5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709'
$treeFileCount = if ($IncludeTauriUtils) { 442 } else { 441 }
$treeSha256 = if ($IncludeTauriUtils) {
    'e9ddbf15e780350628b8e9e334b770bfbb59004f2d6b5c2c43ce764d9530e063'
} else {
    '9c81d169c38167ff2688ee187098096ac3c2e9744f017e0eea5936f83fc74ef8'
}
$downloadUrl = "https://github.com/tauri-apps/binary-releases/releases/download/nsis-$nsisVersion/$archiveName"
$tauriUtilsUrl = (
    'https://github.com/tauri-apps/nsis-tauri-utils/releases/download/' +
    "nsis_tauri_utils-v$tauriUtilsVersion/nsis_tauri_utils.dll"
)

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$toolsRoot = Join-Path $repositoryRoot '.tools'
$explicitNsisRoot = -not [string]::IsNullOrWhiteSpace($NsisRoot)
$selectedNsisRoot = if ($explicitNsisRoot) {
    [System.IO.Path]::GetFullPath($NsisRoot)
} elseif ($IncludeTauriUtils) {
    Join-Path $toolsRoot "nsis-$nsisVersion-tauri-$tauriUtilsVersion"
} else {
    Join-Path $toolsRoot "nsis-$nsisVersion"
}
$launcherPath = Join-Path $selectedNsisRoot 'makensis.exe'
$compilerPath = Join-Path $selectedNsisRoot 'Bin/makensis.exe'
$tauriUtilsRelativePath = 'Plugins/x86-unicode/additional/nsis_tauri_utils.dll'
$tauriUtilsPath = Join-Path $selectedNsisRoot $tauriUtilsRelativePath

function Assert-FileHash {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$ExpectedSha256,

        [Parameter(Mandatory)]
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is missing: $Path"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    if ($actual -cne $ExpectedSha256) {
        throw "$Label SHA-256 mismatch: $actual"
    }
}

function Assert-NsisTree {
    param(
        [Parameter(Mandatory)]
        [string]$RootPath
    )

    $resolvedRoot = (Resolve-Path -LiteralPath $RootPath).Path
    $entries = @(Get-ChildItem -LiteralPath $resolvedRoot -Force -Recurse)
    if (@(
        $entries |
            Where-Object {
                ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
            }
    ).Count -gt 0) {
        throw 'Pinned NSIS tree contains a reparse point.'
    }
    $files = @($entries | Where-Object { -not $_.PSIsContainer })
    if ($files.Count -ne $treeFileCount) {
        throw "Pinned NSIS tree file-count mismatch: $($files.Count)"
    }
    $records = @(
        $files |
            ForEach-Object {
                $relative = [System.IO.Path]::GetRelativePath(
                    $resolvedRoot,
                    $_.FullName
                ).Replace('\', '/')
                $hash = (
                    Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
                ).Hash.ToLowerInvariant()
                "file`0$relative`0$($_.Length)`0$hash"
            } |
            Sort-Object -CaseSensitive
    )
    $payload = [System.Text.UTF8Encoding]::new($false).GetBytes(
        ($records -join "`n")
    )
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $actualTreeSha256 = [System.Convert]::ToHexString(
            $hasher.ComputeHash($payload)
        ).ToLowerInvariant()
    } finally {
        $hasher.Dispose()
    }
    if ($actualTreeSha256 -cne $treeSha256) {
        throw "Pinned NSIS tree SHA-256 mismatch: $actualTreeSha256"
    }
}

function Assert-TauriUtilsFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Pinned nsis-tauri-utils payload is missing: $Path"
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -ne $tauriUtilsByteLength) {
        throw 'Pinned nsis-tauri-utils payload identity drifted.'
    }
    $actualSha1 = (Get-FileHash -Algorithm SHA1 -LiteralPath $Path).Hash.ToLowerInvariant()
    $actualSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
    if ($actualSha1 -cne $tauriUtilsSha1 -or $actualSha256 -cne $tauriUtilsSha256) {
        throw 'Pinned nsis-tauri-utils payload identity drifted.'
    }
}

if (-not (Test-Path -LiteralPath $launcherPath -PathType Leaf)) {
    if ($explicitNsisRoot) {
        throw "Explicit pinned NSIS root is unavailable: $selectedNsisRoot"
    }
    [System.IO.Directory]::CreateDirectory($toolsRoot) | Out-Null
    $archivePath = Join-Path $toolsRoot $archiveName
    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        if (-not $AllowNetwork) {
            throw (
                'Pinned NSIS is unavailable in the offline cache. ' +
                'Re-run with -AllowNetwork to download the exact pinned archive.'
            )
        }
        $partialArchive = "$archivePath.partial-$([guid]::NewGuid().ToString('N'))"
        try {
            Invoke-WebRequest -UseBasicParsing -Uri $downloadUrl -OutFile $partialArchive
            Assert-FileHash `
                -Path $partialArchive `
                -ExpectedSha256 $archiveSha256 `
                -Label 'Pinned NSIS archive'
            [System.IO.File]::Move($partialArchive, $archivePath, $false)
        } finally {
            if (Test-Path -LiteralPath $partialArchive -PathType Leaf) {
                [System.IO.File]::Delete($partialArchive)
            }
        }
    }
    Assert-FileHash `
        -Path $archivePath `
        -ExpectedSha256 $archiveSha256 `
        -Label 'Cached NSIS archive'

    $cachedTauriUtilsPath = Join-Path $toolsRoot "nsis-tauri-utils-$tauriUtilsVersion.dll"
    if ($IncludeTauriUtils -and
        -not (Test-Path -LiteralPath $cachedTauriUtilsPath -PathType Leaf)) {
        if (-not $AllowNetwork) {
            throw (
                'Pinned nsis-tauri-utils is unavailable in the offline cache. ' +
                'Re-run with -AllowNetwork to download the exact pinned payload.'
            )
        }
        $partialTauriUtils = "$cachedTauriUtilsPath.partial-$([guid]::NewGuid().ToString('N'))"
        try {
            Invoke-WebRequest `
                -UseBasicParsing `
                -Uri $tauriUtilsUrl `
                -OutFile $partialTauriUtils
            Assert-TauriUtilsFile -Path $partialTauriUtils
            [System.IO.File]::Move($partialTauriUtils, $cachedTauriUtilsPath, $false)
        } finally {
            if (Test-Path -LiteralPath $partialTauriUtils -PathType Leaf) {
                [System.IO.File]::Delete($partialTauriUtils)
            }
        }
    }
    if ($IncludeTauriUtils) {
        Assert-TauriUtilsFile -Path $cachedTauriUtilsPath
    }

    $stageRoot = Join-Path $toolsRoot ".nsis-$nsisVersion-$([guid]::NewGuid().ToString('N'))"
    try {
        Expand-Archive -LiteralPath $archivePath -DestinationPath $stageRoot
        $expandedRoot = Join-Path $stageRoot "nsis-$nsisVersion"
        if ($IncludeTauriUtils) {
            $expandedTauriUtilsPath = Join-Path $expandedRoot $tauriUtilsRelativePath
            [System.IO.Directory]::CreateDirectory(
                [System.IO.Path]::GetDirectoryName($expandedTauriUtilsPath)
            ) | Out-Null
            [System.IO.File]::Copy(
                $cachedTauriUtilsPath,
                $expandedTauriUtilsPath,
                $false
            )
            Assert-TauriUtilsFile -Path $expandedTauriUtilsPath
        }
        Assert-FileHash `
            -Path (Join-Path $expandedRoot 'makensis.exe') `
            -ExpectedSha256 $launcherSha256 `
            -Label 'Pinned NSIS launcher'
        Assert-FileHash `
            -Path (Join-Path $expandedRoot 'Bin/makensis.exe') `
            -ExpectedSha256 $compilerSha256 `
            -Label 'Pinned NSIS compiler'
        Assert-FileHash `
            -Path (Join-Path $expandedRoot 'COPYING') `
            -ExpectedSha256 $copyingSha256 `
            -Label 'Pinned NSIS license notice'
        Assert-NsisTree -RootPath $expandedRoot
        if (Test-Path -LiteralPath $selectedNsisRoot) {
            throw "Pinned NSIS destination appeared during extraction: $selectedNsisRoot"
        }
        [System.IO.Directory]::Move($expandedRoot, $selectedNsisRoot)
    } finally {
        if (Test-Path -LiteralPath $stageRoot -PathType Container) {
            $stageFullPath = [System.IO.Path]::GetFullPath($stageRoot)
            $toolsFullPath = [System.IO.Path]::GetFullPath($toolsRoot).TrimEnd('\') + '\'
            if (-not $stageFullPath.StartsWith($toolsFullPath, [System.StringComparison]::OrdinalIgnoreCase) -or
                -not ([System.IO.Path]::GetFileName($stageFullPath)).StartsWith(".nsis-$nsisVersion-", [System.StringComparison]::Ordinal)) {
                throw "Refusing to remove an unsafe NSIS staging directory: $stageFullPath"
            }
            [System.IO.Directory]::Delete($stageFullPath, $true)
        }
    }
}

Assert-FileHash `
    -Path $launcherPath `
    -ExpectedSha256 $launcherSha256 `
    -Label 'Pinned NSIS launcher'
Assert-FileHash `
    -Path $compilerPath `
    -ExpectedSha256 $compilerSha256 `
    -Label 'Pinned NSIS compiler'
Assert-FileHash `
    -Path (Join-Path $selectedNsisRoot 'COPYING') `
    -ExpectedSha256 $copyingSha256 `
    -Label 'Pinned NSIS license notice'
if ($IncludeTauriUtils) {
    Assert-TauriUtilsFile -Path $tauriUtilsPath
}
Assert-NsisTree -RootPath $selectedNsisRoot

$actualVersion = (& $launcherPath /VERSION 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $actualVersion -cne "v$nsisVersion") {
    throw "Expected NSIS v$nsisVersion, found '$actualVersion'."
}

Write-Output (Resolve-Path -LiteralPath $selectedNsisRoot).Path
