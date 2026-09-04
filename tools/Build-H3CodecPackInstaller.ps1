[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ArchivePath,

    [Parameter(Mandatory)]
    [string]$PackVersion,

    [string]$OutputDirectory,

    [string]$NsisRoot,

    [switch]$AllowNetwork,

    [string]$SigningCommand
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$sourceBefore = Get-PackagingSourceState -RepositoryRoot $repoRoot
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
Assert-SemVer -Value $PackVersion -Name 'PackVersion'
if (-not [string]::IsNullOrWhiteSpace($SigningCommand) -and
    ($SigningCommand -notmatch '%1' -or
     $SigningCommand.Contains("'") -or
     $SigningCommand.Contains("`r") -or
     $SigningCommand.Contains("`n"))) {
    throw 'SigningCommand must be a single-line NSIS finalize command containing %1 and no single quote.'
}

$resolvedArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
Assert-PathComponentsNotReparsePoints -Path $resolvedArchive
$archive = Get-Item -LiteralPath $resolvedArchive -Force
if ($archive.PSIsContainer -or
    ($archive.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
    $archive.Length -eq 0 -or
    $archive.Length -gt 20GB) {
    throw 'Codec Pack setup payload must be a regular non-reparse ZIP below 20 GiB.'
}
$expectedArchiveName = "LatentDeck-H3-CodecPack-$PackVersion-windows-x64.ldcodec"
if ($archive.Name -cne $expectedArchiveName) {
    throw "Codec Pack setup payload must retain canonical name '$expectedArchiveName'."
}
$archiveSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolvedArchive).Hash.ToLowerInvariant()

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = $archive.DirectoryName
}
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $outputRoot -AllowParent | Out-Null
Assert-PathComponentsNotReparsePoints -Path $outputRoot
[string]$archiveDirectory = [System.IO.Path]::GetFullPath($archive.DirectoryName).TrimEnd('\')
if (-not [string]::Equals(
    $outputRoot.TrimEnd('\'),
    $archiveDirectory,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw 'Codec Pack setup must be built beside its exact adjacent .ldcodec payload.'
}
[System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
$setupName = "LatentDeck-H3-CodecPack-$PackVersion-setup.exe"
$setupPath = Join-Path $outputRoot $setupName
$setupReceiptPath = Join-Path $outputRoot 'setup-receipt.json'
$installerSbomPath = Join-Path $outputRoot 'installer-SBOM.cdx.json'
$installerNoticesPath = Join-Path $outputRoot 'INSTALLER_THIRD_PARTY_NOTICES.md'
$installerNsisCopyingPath = Join-Path $outputRoot 'INSTALLER_NSIS_COPYING.txt'
$installerRustLicensesPath = Join-Path $outputRoot 'INSTALLER_RUST_LICENSES.txt'
$sumsPath = Join-Path $outputRoot 'SHA256SUMS.txt'
$sumLines = @()
$initialSumsExists = Test-Path -LiteralPath $sumsPath
$initialSumsSha256 = $null
if (Test-Path -LiteralPath $sumsPath) {
    $sumsItem = Get-Item -LiteralPath $sumsPath -Force
    if ($sumsItem.PSIsContainer -or
        ($sumsItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $sumsItem.Length -eq 0 -or $sumsItem.Length -gt 1MB) {
        throw 'Existing SHA256SUMS.txt must be a bounded regular non-reparse file.'
    }
    $initialSumsSha256 = (
        Get-FileHash -LiteralPath $sumsPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $sumLines = @(
        Get-Content -LiteralPath $sumsPath |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
}
$expectedArchiveLine = "$archiveSha256  $($archive.Name)"
if ($sumLines.Count -eq 0) {
    $sumLines = @($expectedArchiveLine)
} elseif ($sumLines.Count -ne 1 -or
    $sumLines[0] -cne $expectedArchiveLine) {
    throw 'Existing SHA256SUMS.txt must contain only the exact selected Codec Pack archive.'
}
foreach ($reservedOutput in @(
    $setupPath,
    $setupReceiptPath,
    $installerSbomPath,
    $installerNoticesPath,
    $installerNsisCopyingPath,
    $installerRustLicensesPath
)) {
    if (Test-Path -LiteralPath $reservedOutput) {
        throw "Refusing to overwrite an existing Codec Pack setup artifact: $reservedOutput"
    }
}

$workRoot = Join-Path $artifactsRoot ".h3-codec-pack-installer-$([guid]::NewGuid().ToString('N'))"
Assert-SafeTemporaryDirectory `
    -ParentPath $artifactsRoot `
    -CandidatePath $workRoot `
    -RequiredLeafPrefix '.h3-codec-pack-installer-' | Out-Null
$publicationRoot = Join-Path $workRoot 'publication'
$setupStagePath = Join-Path $publicationRoot $setupName
$setupReceiptStagePath = Join-Path $publicationRoot 'setup-receipt.json'
$installerSbomStagePath = Join-Path $publicationRoot 'installer-SBOM.cdx.json'
$installerNoticesStagePath = Join-Path $publicationRoot 'INSTALLER_THIRD_PARTY_NOTICES.md'
$installerNsisCopyingStagePath = Join-Path $publicationRoot 'INSTALLER_NSIS_COPYING.txt'
$installerRustLicensesStagePath = Join-Path $publicationRoot 'INSTALLER_RUST_LICENSES.txt'
$sumsStagePath = Join-Path $publicationRoot 'SHA256SUMS.txt'

function Assert-NsisDefineValue {
    param(
        [Parameter(Mandatory)]
        [string]$Value,

        [Parameter(Mandatory)]
        [string]$Name
    )

    if ([string]::IsNullOrWhiteSpace($Value) -or
        $Value.Contains('"') -or
        $Value.Contains("`r") -or
        $Value.Contains("`n")) {
        throw "$Name cannot be represented safely as an NSIS build define."
    }
}

function Assert-WindowsPe {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [int]$ExpectedMachine,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or $item.Length -lt 1024) {
        throw "$Label is not a plausible Windows executable."
    }
    $stream = [System.IO.File]::Open($item.FullName, 'Open', 'Read', 'Read')
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        try {
            if ($reader.ReadUInt16() -ne 0x5A4D) {
                throw "$Label is missing the DOS executable header."
            }
            $stream.Position = 0x3C
            $peOffset = $reader.ReadInt32()
            if ($peOffset -lt 0x40 -or $peOffset -gt ($stream.Length - 6)) {
                throw "$Label has an invalid PE offset."
            }
            $stream.Position = $peOffset
            if ($reader.ReadUInt32() -ne 0x00004550) {
                throw "$Label is missing the PE signature."
            }
            if ($reader.ReadUInt16() -ne $ExpectedMachine) {
                throw "$Label has the wrong Windows machine type."
            }
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Write-Utf8Json {
    param(
        [Parameter(Mandatory)]
        [object]$Value,

        [Parameter(Mandatory)]
        [string]$Path
    )

    $json = $Value | ConvertTo-Json -Depth 20
    [System.IO.File]::WriteAllText(
        $Path,
        $json + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Get-PublicSourceSnapshot {
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot
    )

    $relativePaths = @(
        & git -C $RepositoryRoot -c core.quotepath=false `
            ls-files --cached --others --exclude-standard
    )
    if ($LASTEXITCODE -ne 0 -or $relativePaths.Count -eq 0) {
        throw 'Codec Pack setup could not enumerate its public source snapshot.'
    }
    $records = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in $relativePaths) {
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            continue
        }
        $fullPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot $relativePath))
        Assert-ChildPath -ParentPath $RepositoryRoot -CandidatePath $fullPath | Out-Null
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            $records.Add("missing`0$relativePath")
            continue
        }
        $item = Get-Item -LiteralPath $fullPath -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Codec Pack setup source snapshot contains a reparse-point file: $relativePath"
        }
        $hash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $portablePath = $relativePath.Replace('\', '/')
        $records.Add("file`0$portablePath`0$($item.Length)`0$hash")
    }
    $payload = [System.Text.UTF8Encoding]::new($false).GetBytes(
        ($records | Sort-Object -CaseSensitive) -join "`n"
    )
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $snapshotHash = [System.Convert]::ToHexString(
            $hasher.ComputeHash($payload)
        ).ToLowerInvariant()
    } finally {
        $hasher.Dispose()
    }
    return [pscustomobject]@{
        Sha256 = $snapshotHash
        FileCount = $records.Count
    }
}

try {
    [System.IO.Directory]::CreateDirectory($workRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($publicationRoot) | Out-Null

    $packRoot = Join-Path $workRoot 'expanded-pack'
    Expand-SafeCodecPackArchive -ArchivePath $resolvedArchive -DestinationPath $packRoot
    $manifest = Test-H3CodecPackDirectory `
        -PackRoot $packRoot `
        -ExpectedPackVersion $PackVersion
    if ($manifest.pack_id -cne 'org.latentdeck.h3') {
        throw 'Codec Pack setup accepts only org.latentdeck.h3.'
    }

    Add-Type -AssemblyName System.IO.Compression
    $zip = [System.IO.Compression.ZipFile]::OpenRead($resolvedArchive)
    try {
        if ($zip.Entries.Count -eq 0 -or $zip.Entries.Count -gt 32770) {
            throw 'Codec Pack setup payload is empty or exceeds the entry limit.'
        }
        $uncompressedBytes = [int64]0
        foreach ($entry in $zip.Entries) {
            if ([int64]$entry.Length -gt [int64]::MaxValue - $uncompressedBytes) {
                throw 'Codec Pack setup payload size overflowed its counter.'
            }
            $uncompressedBytes += [int64]$entry.Length
        }
        if ($uncompressedBytes -le 0 -or $uncompressedBytes -gt 20GB) {
            throw 'Codec Pack setup payload exceeds its uncompressed-size contract.'
        }
    } finally {
        $zip.Dispose()
    }
    $estimatedSizeKiB = [int64][Math]::Ceiling($uncompressedBytes / 1KB)
    if ($estimatedSizeKiB -gt [uint32]::MaxValue) {
        throw 'Codec Pack EstimatedSize does not fit the Windows Installed Apps field.'
    }

    $helperAuthorizationPath = Join-Path $workRoot 'h3-helper-authorization.json'
    Write-Utf8Json -Value ([ordered]@{
        schema_version = 1
        packages = @(
            [ordered]@{
                pack_id = 'org.latentdeck.h3'
                pack_version = $PackVersion
                archive_sha256 = $archiveSha256
                archive_byte_length = [int64]$archive.Length
            }
        )
    }) -Path $helperAuthorizationPath

    $helperTargetRoot = Join-Path $artifactsRoot 'codec-pack-installer-target'
    $savedTargetDirectory = $env:CARGO_TARGET_DIR
    $savedRustFlags = $env:RUSTFLAGS
    $savedAuthorizationFile = $env:LATENTDECK_H3_AUTHORIZATION_FILE
    try {
        $env:CARGO_TARGET_DIR = $helperTargetRoot
        $env:LATENTDECK_H3_AUTHORIZATION_FILE = $helperAuthorizationPath
        if ([string]::IsNullOrWhiteSpace($savedRustFlags)) {
            $env:RUSTFLAGS = '-C target-feature=+crt-static'
        } else {
            $env:RUSTFLAGS = "$savedRustFlags -C target-feature=+crt-static"
        }
        $cargoBuildArguments = @(
            'build', '--locked', '--release',
            '--target', 'x86_64-pc-windows-msvc',
            '--package', 'latentdeck-codec-pack-installer'
        )
        if (-not $AllowNetwork) {
            $cargoBuildArguments += '--offline'
        }
        & cargo @cargoBuildArguments
        if ($LASTEXITCODE -ne 0) {
            throw "Native Codec Pack installer helper build failed with exit code $LASTEXITCODE."
        }
    } finally {
        $env:CARGO_TARGET_DIR = $savedTargetDirectory
        $env:RUSTFLAGS = $savedRustFlags
        $env:LATENTDECK_H3_AUTHORIZATION_FILE = $savedAuthorizationFile
    }
    $helperPath = Join-Path `
        $helperTargetRoot `
        'x86_64-pc-windows-msvc/release/latentdeck-codec-pack-installer.exe'
    $helperPath = (Resolve-Path -LiteralPath $helperPath).Path
    Assert-WindowsPe -Path $helperPath -ExpectedMachine 0x8664 -Label 'Codec Pack lifecycle helper'
    $helperSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $helperPath).Hash.ToLowerInvariant()

    $nsisParameters = @{}
    if (-not [string]::IsNullOrWhiteSpace($NsisRoot)) {
        $nsisParameters.NsisRoot = $NsisRoot
    }
    if ($AllowNetwork) {
        $nsisParameters.AllowNetwork = $true
    }
    $resolvedNsisRoot = [string](
        & (Join-Path $PSScriptRoot 'Get-PinnedNsis.ps1') @nsisParameters
    )
    $makeNsis = Join-Path $resolvedNsisRoot 'makensis.exe'
    $makeNsisCore = Join-Path $resolvedNsisRoot 'Bin/makensis.exe'
    $nsisCopyingSource = Join-Path $resolvedNsisRoot 'COPYING'
    $nsisVersion = (& $makeNsis /VERSION 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $nsisVersion -cne 'v3.11') {
        throw "Codec Pack setup requires pinned NSIS v3.11; found '$nsisVersion'."
    }
    $makeNsisSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $makeNsisCore).Hash.ToLowerInvariant()
    if ($makeNsisSha256 -cne '42850802704ecb11163f7e0329d35ee54bd288953200d4966e226d572848cfc5') {
        throw "Pinned NSIS compiler SHA-256 mismatch: $makeNsisSha256"
    }
    $nsisCopyingSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $nsisCopyingSource).Hash.ToLowerInvariant()
    if ($nsisCopyingSha256 -cne 'e7dd514003ab96cb3ddccbc028fe5c795fccf57dc41f21cfb9d4dd16ead23bf5') {
        throw "Pinned NSIS license notice SHA-256 mismatch: $nsisCopyingSha256"
    }

    $installerSbomWorkPath = Join-Path $workRoot 'installer-SBOM.cdx.json'
    $installerSbom = & (Join-Path $PSScriptRoot 'New-H3CodecPackInstallerSbom.ps1') `
        -PackVersion $PackVersion `
        -OutputPath $installerSbomWorkPath `
        -NsisRoot $resolvedNsisRoot `
        -AllowNetwork:$AllowNetwork
    if ($null -eq $installerSbom -or
        -not (Test-Path -LiteralPath $installerSbomWorkPath -PathType Leaf) -or
        [string]$installerSbom.LicenseReview -cne 'complete' -or
        @($installerSbom.MissingLicenseComponents).Count -ne 0 -or
        [int]$installerSbom.Components -le 0) {
        throw 'Codec Pack setup SBOM generation did not produce its expected file.'
    }
    $installerSbomSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $installerSbomWorkPath).Hash.ToLowerInvariant()

    $installerRustLicensesWorkPath = Join-Path $workRoot 'INSTALLER_RUST_LICENSES.txt'
    $installerRustLicenses = & (
        Join-Path $PSScriptRoot 'New-H3CodecPackInstallerRustLicenses.ps1'
    ) `
        -PackVersion $PackVersion `
        -OutputPath $installerRustLicensesWorkPath `
        -AllowNetwork:$AllowNetwork
    if ($null -eq $installerRustLicenses -or
        -not (Test-Path -LiteralPath $installerRustLicensesWorkPath -PathType Leaf)) {
        throw 'Installer Rust license generation did not produce its expected file.'
    }
    $installerRustLicensesSha256 = (
        Get-FileHash -Algorithm SHA256 -LiteralPath $installerRustLicensesWorkPath
    ).Hash.ToLowerInvariant()

    $sourceCommit = [string]$sourceBefore.Commit
    $sourceBranch = [string]$sourceBefore.Branch
    $gitStatus = @($sourceBefore.Status)
    if (-not [string]::IsNullOrWhiteSpace($SigningCommand)) {
        if ($sourceBranch -cne 'main' -or $gitStatus.Count -ne 0) {
            throw 'A signed Codec Pack setup requires a clean main checkout.'
        }
        $publicTreeOutput = @(
            & (Join-Path $PSScriptRoot 'Test-PublicTree.ps1') 2>&1
        )
        if ($LASTEXITCODE -ne 0) {
            throw (
                'A signed Codec Pack setup requires a passing public-tree audit. ' +
                ($publicTreeOutput -join ' ')
            )
        }
    }
    $sourceTree = [string]$sourceBefore.Tree
    $sourceSnapshot = [pscustomobject]@{
        Sha256 = [string]$sourceBefore.PublicSnapshotSha256
        FileCount = [int64]$sourceBefore.PublicSnapshotFileCount
    }

    $metadataPath = Join-Path $workRoot 'install-metadata.json'
    Write-Utf8Json -Value ([ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        platform = 'windows-x86_64'
        source = [ordered]@{
            commit = $sourceCommit
            branch = $sourceBranch
            git_dirty = ($gitStatus.Count -gt 0)
            git_tree = $sourceTree
            public_snapshot_sha256 = $sourceSnapshot.Sha256
            public_snapshot_file_count = $sourceSnapshot.FileCount
        }
        payload = [ordered]@{
            delivery = 'adjacent_hash_bound_ldcodec'
            name = $archive.Name
            byte_length = [int64]$archive.Length
            sha256 = $archiveSha256
            uncompressed_bytes = $uncompressedBytes
        }
        lifecycle = [ordered]@{
            scope = 'current_user'
            offline = $true
            network_required = $false
            powershell_required = $false
            system_python_required = $false
            elevation_required = $false
            immutable_versions = $true
            maintenance_root = "%LOCALAPPDATA%/LatentDeck/CodecPackMaintenance/org.latentdeck.h3/$PackVersion"
        }
        helper = [ordered]@{
            name = 'latentdeck-codec-pack-installer.exe'
            sha256 = $helperSha256
            crt = 'static'
            delivery = 'embedded_in_setup_and_uninstaller'
            installed_as_loose_file = $false
            authorization = [ordered]@{
                source = 'build_generated_exact_allowlist'
                pack_version = $PackVersion
                archive_sha256 = $archiveSha256
                archive_byte_length = [int64]$archive.Length
            }
        }
        publisher_signature = if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
            'not_present_local_rc'
        } else {
            'authenticode_finalize_command_supplied'
        }
    }) -Path $metadataPath

    $template = Join-Path $PSScriptRoot 'installer/H3CodecPackInstaller.nsi'
    $licensePath = Join-Path $repoRoot 'LICENSE'
    $noticesPath = Join-Path $PSScriptRoot 'installer/H3CodecPackInstallerNotices.md'
    $noticesSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $noticesPath).Hash.ToLowerInvariant()
    $iconPath = Join-Path $repoRoot 'apps/latentdeck/src-tauri/icons/icon.ico'
    foreach ($define in @(
        $setupStagePath, $PackVersion, $archive.Name, $archiveSha256,
        [string]$archive.Length, [string]$estimatedSizeKiB, $helperPath,
        $metadataPath, $licensePath, $noticesPath, $nsisCopyingSource,
        $installerSbomWorkPath, $installerRustLicensesWorkPath, $iconPath
    )) {
        Assert-NsisDefineValue -Value $define -Name 'NSIS define'
    }
    $versionParts = $PackVersion.Split('-')[0].Split('.')
    if ($versionParts.Count -ne 3 -or @($versionParts | Where-Object { $_ -cnotmatch '^\d+$' }).Count -gt 0) {
        throw 'Public Codec Pack setup requires a three-component numeric SemVer.'
    }
    $productVersion4 = "$($versionParts[0]).$($versionParts[1]).$($versionParts[2]).0"

    $arguments = @(
        '/NOCONFIG', '/WX', '/V4', '/INPUTCHARSET', 'UTF8',
        "/DOUTPUT_PATH=$setupStagePath",
        "/DPACK_VERSION=$PackVersion",
        "/DPRODUCT_VERSION4=$productVersion4",
        "/DARCHIVE_NAME=$($archive.Name)",
        "/DARCHIVE_SHA256=$archiveSha256",
        "/DARCHIVE_LENGTH=$($archive.Length)",
        "/DESTIMATED_SIZE_KIB=$estimatedSizeKiB",
        "/DHELPER_PATH=$helperPath",
        "/DINSTALL_METADATA_PATH=$metadataPath",
        "/DLICENSE_PATH=$licensePath",
        "/DNOTICES_PATH=$noticesPath",
        "/DNSIS_COPYING_PATH=$nsisCopyingSource",
        "/DINSTALLER_SBOM_PATH=$installerSbomWorkPath",
        "/DRUST_LICENSES_PATH=$installerRustLicensesWorkPath",
        "/DICON_PATH=$iconPath",
        $template
    )
    if (-not [string]::IsNullOrWhiteSpace($SigningCommand)) {
        $arguments = @(
            $arguments[0..($arguments.Count - 2)]
            "/DSIGNING_COMMAND=$SigningCommand"
            $template
        )
    }
    $savedSourceDateEpoch = $env:NSIS_SOURCE_DATE_EPOCH
    try {
        $env:NSIS_SOURCE_DATE_EPOCH = '1741475120'
        $nsisOutput = (& $makeNsis @arguments 2>&1 | Out-String).Trim()
        $nsisExitCode = $LASTEXITCODE
        if ($nsisExitCode -ne 0) {
            throw (
                "NSIS Codec Pack setup build failed with exit code $nsisExitCode. " +
                $nsisOutput
            )
        }
        Write-Verbose $nsisOutput
    } finally {
        $env:NSIS_SOURCE_DATE_EPOCH = $savedSourceDateEpoch
    }

    Assert-WindowsPe -Path $setupStagePath -ExpectedMachine 0x014C -Label 'Codec Pack setup'
    $setup = Get-Item -LiteralPath $setupStagePath
    if ($setup.Length -gt 64MB) {
        throw 'Codec Pack setup unexpectedly embedded the large payload or exceeded 64 MiB.'
    }
    $authenticode = Get-AuthenticodeSignature -LiteralPath $setupStagePath
    if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
        if ($authenticode.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
            throw 'Unsigned local Codec Pack setup unexpectedly crossed the signing boundary.'
        }
    } elseif ($authenticode.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Signed Codec Pack setup failed Authenticode verification: $($authenticode.Status)"
    }
    $publisherSignature = if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
        'not_present_local_rc'
    } else {
        'outer_setup_authenticode_valid'
    }
    $signingEvidence = [ordered]@{
        mode = if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
            'unsigned_local_rc'
        } else {
            'authenticode_finalize_command'
        }
        outer_setup_authenticode = if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
            'not_present'
        } else {
            'valid'
        }
        embedded_uninstaller_finalize = if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
            'not_requested'
        } else {
            'exit_0'
        }
        installed_uninstaller_authenticode = 'not_run_clean_machine_gate'
    }

    $sourceAfter = Get-PackagingSourceState -RepositoryRoot $repoRoot
    Assert-PackagingSourceStateUnchanged `
        -Before $sourceBefore `
        -After $sourceAfter `
        -Context 'Codec Pack setup source'

    $setupSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $setupStagePath).Hash.ToLowerInvariant()

    [System.IO.File]::Copy($installerSbomWorkPath, $installerSbomStagePath, $false)
    [System.IO.File]::Copy($noticesPath, $installerNoticesStagePath, $false)
    [System.IO.File]::Copy($nsisCopyingSource, $installerNsisCopyingStagePath, $false)
    [System.IO.File]::Copy(
        $installerRustLicensesWorkPath,
        $installerRustLicensesStagePath,
        $false
    )
    $installerNoticesLength = [int64](Get-Item -LiteralPath $installerNoticesStagePath).Length
    $installerNsisCopyingLength = [int64](Get-Item -LiteralPath $installerNsisCopyingStagePath).Length
    $installerRustLicensesLength = [int64](
        Get-Item -LiteralPath $installerRustLicensesStagePath
    ).Length
    $installerSbomLength = [int64](Get-Item -LiteralPath $installerSbomStagePath).Length
    Write-Utf8Json -Value ([ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        platform = 'windows-x86_64'
        setup = [ordered]@{
            name = $setup.Name
            byte_length = [int64]$setup.Length
            sha256 = $setupSha256
            format = 'nsis'
            scope = 'current_user'
            payload_delivery = 'adjacent_hash_bound_ldcodec'
        }
        payload = [ordered]@{
            name = $archive.Name
            byte_length = [int64]$archive.Length
            sha256 = $archiveSha256
            uncompressed_bytes = $uncompressedBytes
        }
        helper = [ordered]@{
            sha256 = $helperSha256
            static_crt = $true
            delivery = 'embedded_in_setup_and_uninstaller'
            installed_as_loose_file = $false
            authorization = [ordered]@{
                source = 'build_generated_exact_allowlist'
                pack_version = $PackVersion
                archive_sha256 = $archiveSha256
                archive_byte_length = [int64]$archive.Length
            }
        }
        sbom = [ordered]@{
            name = 'installer-SBOM.cdx.json'
            byte_length = $installerSbomLength
            sha256 = $installerSbomSha256
            format = 'CycloneDX-1.5'
            component_count = [int]$installerSbom.Components
            license_review = 'complete'
            missing_license_component_count = 0
        }
        notices = [ordered]@{
            name = 'INSTALLER_THIRD_PARTY_NOTICES.md'
            byte_length = $installerNoticesLength
            sha256 = $noticesSha256
            nsis_copying_name = 'INSTALLER_NSIS_COPYING.txt'
            nsis_copying_byte_length = $installerNsisCopyingLength
            nsis_copying_sha256 = $nsisCopyingSha256
            rust_licenses_name = 'INSTALLER_RUST_LICENSES.txt'
            rust_licenses_byte_length = $installerRustLicensesLength
            rust_licenses_sha256 = $installerRustLicensesSha256
        }
        toolchain = [ordered]@{
            nsis_version = $nsisVersion.TrimStart('v')
            distribution_archive_sha256 = 'c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1'
            makensis_sha256 = $makeNsisSha256
            tree_file_count = 441
            tree_sha256 = '9c81d169c38167ff2688ee187098096ac3c2e9744f017e0eea5936f83fc74ef8'
            source_date_epoch = 1741475120
        }
        source = [ordered]@{
            commit = $sourceCommit
            branch = $sourceBranch
            git_dirty = ($gitStatus.Count -gt 0)
            git_tree = $sourceTree
            public_snapshot_sha256 = $sourceSnapshot.Sha256
            public_snapshot_file_count = $sourceSnapshot.FileCount
        }
        lifecycle = [ordered]@{
            scope = 'current_user'
            offline = $true
            network_required = $false
            powershell_required = $false
            system_python_required = $false
            elevation_required = $false
            immutable_versions = $true
        }
        native_helper_lifecycle_smoke = 'pending'
        windows_setup_lifecycle = 'not_run_clean_machine_gate'
        signing = $signingEvidence
        publisher_signature = $publisherSignature
    }) -Path $setupReceiptStagePath

    $sidecarHashes = [ordered]@{}
    $sidecarHashes[[string]$setup.Name] = $setupSha256
    $sidecarHashes['installer-SBOM.cdx.json'] = $installerSbomSha256
    $sidecarHashes['INSTALLER_THIRD_PARTY_NOTICES.md'] = $noticesSha256
    $sidecarHashes['INSTALLER_NSIS_COPYING.txt'] = $nsisCopyingSha256
    $sidecarHashes['INSTALLER_RUST_LICENSES.txt'] = $installerRustLicensesSha256
    $sidecarHashes['setup-receipt.json'] = (
        Get-FileHash -Algorithm SHA256 -LiteralPath $setupReceiptStagePath
    ).Hash.ToLowerInvariant()
    foreach ($name in $sidecarHashes.Keys) {
        if (@($sumLines | Where-Object { $_ -match ('  ' + [regex]::Escape($name) + '$') }).Count -gt 0) {
            throw "Existing SHA256SUMS.txt already contains a Codec Pack setup artifact entry: $name"
        }
        $sumLines += "$($sidecarHashes[$name])  $name"
    }
    [System.IO.File]::WriteAllText(
        $sumsStagePath,
        ($sumLines -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    Assert-PathComponentsNotReparsePoints -Path $outputRoot
    foreach ($reservedOutput in @(
        $setupPath,
        $setupReceiptPath,
        $installerSbomPath,
        $installerNoticesPath,
        $installerNsisCopyingPath,
        $installerRustLicensesPath
    )) {
        if (Test-Path -LiteralPath $reservedOutput) {
            throw "Codec Pack setup artifact destination appeared during build: $reservedOutput"
        }
    }
    if ($initialSumsExists) {
        if (-not (Test-Path -LiteralPath $sumsPath -PathType Leaf) -or
            (Get-FileHash -LiteralPath $sumsPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
                $initialSumsSha256) {
            throw 'Adjacent Codec Pack SHA256SUMS.txt changed during setup build.'
        }
    } elseif (Test-Path -LiteralPath $sumsPath) {
        throw 'Adjacent Codec Pack SHA256SUMS.txt appeared during setup build.'
    }
    foreach ($publication in @(
        [pscustomobject]@{ Source = $installerSbomStagePath; Destination = $installerSbomPath },
        [pscustomobject]@{ Source = $installerNoticesStagePath; Destination = $installerNoticesPath },
        [pscustomobject]@{ Source = $installerNsisCopyingStagePath; Destination = $installerNsisCopyingPath },
        [pscustomobject]@{ Source = $installerRustLicensesStagePath; Destination = $installerRustLicensesPath },
        [pscustomobject]@{ Source = $setupReceiptStagePath; Destination = $setupReceiptPath },
        [pscustomobject]@{ Source = $setupStagePath; Destination = $setupPath }
    )) {
        [System.IO.File]::Move($publication.Source, $publication.Destination, $false)
    }
    [System.IO.File]::Move($sumsStagePath, $sumsPath, $initialSumsExists)

    Write-Output $setupPath
} finally {
    Remove-SafeTemporaryDirectory `
        -ParentPath $artifactsRoot `
        -CandidatePath $workRoot `
        -RequiredLeafPrefix '.h3-codec-pack-installer-'
}
