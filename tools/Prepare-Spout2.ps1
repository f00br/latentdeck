[CmdletBinding()]
param(
    [string]$ArchivePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'ReleaseSpoutMetadata.psm1') -Force

$spoutMetadata = Get-Spout2ReleaseMetadata
$spoutTag = $spoutMetadata.Tag
$spoutCommit = $spoutMetadata.Commit
$archiveSha256 = $spoutMetadata.ArchiveSha256
$archiveBytes = [int64]$spoutMetadata.ArchiveBytes
$archiveUrl = $spoutMetadata.ArchiveUrl
$archiveLeafName = "spout2-$spoutCommit.zip"
$installLeafName = "$spoutTag-$spoutCommit"
$upstreamLeafName = "Spout2-$spoutCommit"
$stampLeafName = 'LATENTDECK_SPOUT2_SOURCE.txt'

$criticalFiles = @(
    [pscustomobject]@{
        Path = 'LICENSE'
        Sha256 = '7b602b5c652a76ced1c6ff5f3f4c15c37a733230eeb5b8d075f1282b446b10be'
    }
    [pscustomobject]@{
        Path = 'CMakeLists.txt'
        Sha256 = '4b78c6930b52e5a013ef3cc40717a4534349d1693fbc3d4bffbdb17b61201dea'
    }
    [pscustomobject]@{
        Path = 'SPOUTSDK/SpoutGL/CMakeLists.txt'
        Sha256 = '69dc548b163c01690b7cd23f9b2ad8fea0603ed5b935e3e3718393889d5a408e'
    }
    [pscustomobject]@{
        Path = 'SPOUTSDK/SpoutDirectX/SpoutDX/CMakeLists.txt'
        Sha256 = '95e3f52a1ee518773c6d9735edc4e5bf68b4d88005808798a2bdf2384b830a69'
    }
    [pscustomobject]@{
        Path = 'SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/CMakeLists.txt'
        Sha256 = 'd3c3d823d0e53421be98ceee262d178c7deb8afe5ba7b6a5f90f5273ce26d552'
    }
    [pscustomobject]@{
        Path = 'SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.cpp'
        Sha256 = '4c8ded4a561d74dcc95fdc2ab7f76b5a90c940990ef19280843d0e023ee002e3'
    }
    [pscustomobject]@{
        Path = 'SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.h'
        Sha256 = '5e48a55a0b70a274b303d20ea4c688ba8e100fce2c8eb9df6ad361d341271cb8'
    }
)

$expectedStamp = @(
    'schema=1'
    "tag=$spoutTag"
    "commit=$spoutCommit"
    "archive_sha256=$archiveSha256"
    "archive_bytes=$archiveBytes"
    'source_directory=source'
) -join "`n"
$expectedStamp += "`n"

function Assert-ChildPath {
    param(
        [Parameter(Mandatory)]
        [string]$ParentPath,

        [Parameter(Mandatory)]
        [string]$CandidatePath
    )

    $parentFullPath = [System.IO.Path]::GetFullPath($ParentPath).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    $candidateFullPath = [System.IO.Path]::GetFullPath($CandidatePath)
    $requiredPrefix = $parentFullPath + [System.IO.Path]::DirectorySeparatorChar

    if (-not $candidateFullPath.StartsWith(
        $requiredPrefix,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Refusing an operation outside the expected local root: $candidateFullPath"
    }
}

function Remove-PrivateStagingDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$SpoutRoot,

        [Parameter(Mandatory)]
        [string]$StagingPath
    )

    if (-not (Test-Path -LiteralPath $StagingPath)) {
        return
    }

    Assert-ChildPath -ParentPath $SpoutRoot -CandidatePath $StagingPath
    if (-not ([System.IO.Path]::GetFileName($StagingPath)).StartsWith(
        '.prepare-',
        [System.StringComparison]::Ordinal
    )) {
        throw "Refusing to remove a non-staging directory: $StagingPath"
    }

    Remove-Item -LiteralPath $StagingPath -Recurse -Force
}

function Assert-PinnedArchive {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Spout2 archive does not exist: $Path"
    }

    $archive = Get-Item -LiteralPath $Path
    if ($archive.Length -ne $archiveBytes) {
        throw (
            "Spout2 archive byte length mismatch: expected $archiveBytes, " +
            "found $($archive.Length)."
        )
    }

    $actualHash = (Get-FileHash -LiteralPath $archive.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualHash -ne $archiveSha256) {
        throw "Spout2 archive SHA-256 mismatch: expected $archiveSha256, found $actualHash."
    }

    return $archive.FullName
}

function Assert-PinnedSource {
    param(
        [Parameter(Mandatory)]
        [string]$SourcePath
    )

    if (-not (Test-Path -LiteralPath $SourcePath -PathType Container)) {
        throw "Prepared Spout2 source directory is missing: $SourcePath"
    }

    foreach ($criticalFile in $criticalFiles) {
        $filePath = Join-Path $SourcePath $criticalFile.Path
        if (-not (Test-Path -LiteralPath $filePath -PathType Leaf)) {
            throw "Prepared Spout2 source is missing a critical file: $($criticalFile.Path)"
        }

        $actualHash = (Get-FileHash -LiteralPath $filePath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $criticalFile.Sha256) {
            throw (
                "Prepared Spout2 critical-file hash mismatch for $($criticalFile.Path): " +
                "expected $($criticalFile.Sha256), found $actualHash."
            )
        }
    }
}

function Assert-PinnedInstall {
    param(
        [Parameter(Mandatory)]
        [string]$InstallPath
    )

    $stampPath = Join-Path $InstallPath $stampLeafName
    if (-not (Test-Path -LiteralPath $stampPath -PathType Leaf)) {
        throw "Prepared Spout2 stamp is missing: $stampPath"
    }

    $actualStamp = [System.IO.File]::ReadAllText($stampPath)
    if ($actualStamp -cne $expectedStamp) {
        throw "Prepared Spout2 stamp does not match the exact approved pin: $stampPath"
    }

    Assert-PinnedSource -SourcePath (Join-Path $InstallPath 'source')
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$vendorRoot = Join-Path $repoRoot 'vendor-local'
$spoutRoot = Join-Path $vendorRoot 'spout2'
$installRoot = Join-Path $spoutRoot $installLeafName
$sourceRoot = Join-Path $installRoot 'source'

if (Test-Path -LiteralPath $installRoot) {
    try {
        Assert-PinnedInstall -InstallPath $installRoot
    } catch {
        throw (
            "Refusing to replace the existing Spout2 destination because it is not the exact " +
            "approved install: $installRoot`n$($_.Exception.Message)"
        )
    }

    Write-Host "Pinned Spout2 source is already prepared: $sourceRoot"
    Write-Output $sourceRoot
    return
}

New-Item -ItemType Directory -Path $vendorRoot -Force | Out-Null
New-Item -ItemType Directory -Path $spoutRoot -Force | Out-Null

$downloadPartialPath = $null
$stagingRoot = Join-Path $spoutRoot ".prepare-$([guid]::NewGuid().ToString('N'))"

try {
    if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
        $resolvedArchivePath = Join-Path $vendorRoot $archiveLeafName
        if (-not (Test-Path -LiteralPath $resolvedArchivePath)) {
            $downloadPartialPath = Join-Path $vendorRoot ".$archiveLeafName.partial-$([guid]::NewGuid().ToString('N'))"
            Assert-ChildPath -ParentPath $vendorRoot -CandidatePath $downloadPartialPath
            Invoke-WebRequest -UseBasicParsing -Uri $archiveUrl -OutFile $downloadPartialPath
            Assert-PinnedArchive -Path $downloadPartialPath | Out-Null
            [System.IO.File]::Move($downloadPartialPath, $resolvedArchivePath)
            $downloadPartialPath = $null
        }
    } else {
        $resolvedArchivePath = (Resolve-Path -LiteralPath $ArchivePath).Path
    }

    $resolvedArchivePath = Assert-PinnedArchive -Path $resolvedArchivePath

    Assert-ChildPath -ParentPath $spoutRoot -CandidatePath $stagingRoot
    $unpackRoot = Join-Path $stagingRoot 'unpacked'
    $stagedInstallRoot = Join-Path $stagingRoot 'install'
    New-Item -ItemType Directory -Path $unpackRoot | Out-Null
    New-Item -ItemType Directory -Path $stagedInstallRoot | Out-Null

    Expand-Archive -LiteralPath $resolvedArchivePath -DestinationPath $unpackRoot

    $expandedSourceRoot = Join-Path $unpackRoot $upstreamLeafName
    Assert-PinnedSource -SourcePath $expandedSourceRoot

    $stagedSourceRoot = Join-Path $stagedInstallRoot 'source'
    [System.IO.Directory]::Move($expandedSourceRoot, $stagedSourceRoot)
    $stagedStampPath = Join-Path $stagedInstallRoot $stampLeafName
    [System.IO.File]::WriteAllText(
        $stagedStampPath,
        $expectedStamp,
        [System.Text.UTF8Encoding]::new($false)
    )
    Assert-PinnedInstall -InstallPath $stagedInstallRoot

    if (Test-Path -LiteralPath $installRoot) {
        throw "Spout2 destination appeared during preparation; refusing to overwrite it: $installRoot"
    }
    [System.IO.Directory]::Move($stagedInstallRoot, $installRoot)
    Assert-PinnedInstall -InstallPath $installRoot
} finally {
    if ($null -ne $downloadPartialPath -and (Test-Path -LiteralPath $downloadPartialPath)) {
        Assert-ChildPath -ParentPath $vendorRoot -CandidatePath $downloadPartialPath
        Remove-Item -LiteralPath $downloadPartialPath -Force
    }
    Remove-PrivateStagingDirectory -SpoutRoot $spoutRoot -StagingPath $stagingRoot
}

Write-Host "Prepared pinned Spout2 source: $sourceRoot"
Write-Output $sourceRoot
