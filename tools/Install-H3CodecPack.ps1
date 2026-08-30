[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ArchivePath,

    [Parameter(Mandatory)]
    [Alias('ExpectedArchiveSha256')]
    [string]$TrustedArchiveSha256,

    [ValidateSet('CurrentUser', 'AllUsers')]
    [string]$Scope = 'CurrentUser',

    [string]$InstallRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

Assert-Sha256 -Value $TrustedArchiveSha256 -Name 'TrustedArchiveSha256'
$resolvedArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
$archiveItem = Get-Item -LiteralPath $resolvedArchive -Force
if ($archiveItem.PSIsContainer -or
    ($archiveItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
    $archiveItem.Length -eq 0 -or
    $archiveItem.Length -gt 20GB) {
    throw 'Codec Pack archive must be a regular non-reparse file.'
}
if ($Scope -eq 'AllUsers' -and [string]::IsNullOrWhiteSpace($InstallRoot)) {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'An all-users Codec Pack install requires an elevated PowerShell process.'
    }
}

$lifecycleMutex = [System.Threading.Mutex]::new(
    $false,
    'Local\LatentDeck.CodecPackLifecycle.org.latentdeck.h3'
)
$mutexHeld = $false
try {
    try {
        $mutexHeld = $lifecycleMutex.WaitOne([System.TimeSpan]::FromSeconds(30))
    } catch [System.Threading.AbandonedMutexException] {
        $mutexHeld = $true
    }
    if (-not $mutexHeld) {
        throw 'Timed out waiting for another Codec Pack lifecycle operation to finish.'
    }

$root = Get-CodecPackInstallRoot -Scope $Scope -InstallRoot $InstallRoot
$existing = $root
while (-not [string]::IsNullOrWhiteSpace($existing) -and
    -not (Test-Path -LiteralPath $existing)) {
    $parent = Split-Path -Parent $existing
    if ($parent -eq $existing) {
        break
    }
    $existing = $parent
}
if (-not [string]::IsNullOrWhiteSpace($existing) -and
    (Test-Path -LiteralPath $existing -PathType Container)) {
    Assert-DirectoryNotReparsePoint -Path $existing
}

[System.IO.Directory]::CreateDirectory($root) | Out-Null
Assert-DirectoryNotReparsePoint -Path $root
Assert-PathComponentsNotReparsePoints -Path $root
$packParent = Join-Path $root 'org.latentdeck.h3'
[System.IO.Directory]::CreateDirectory($packParent) | Out-Null
Assert-DirectoryNotReparsePoint -Path $packParent
Assert-PathComponentsNotReparsePoints -Path $packParent
Assert-ChildPath -ParentPath $root -CandidatePath $packParent | Out-Null

$stagingRoot = Get-CodecPackAuxiliaryRoot -InstallRoot $root -Kind Staging
[System.IO.Directory]::CreateDirectory($stagingRoot) | Out-Null
Assert-DirectoryNotReparsePoint -Path $stagingRoot
Assert-PathComponentsNotReparsePoints -Path $stagingRoot
$stagingPath = Join-Path $stagingRoot ".install-$([guid]::NewGuid().ToString('N'))"
Assert-SafeTemporaryDirectory `
    -ParentPath $stagingRoot `
    -CandidatePath $stagingPath `
    -RequiredLeafPrefix '.install-' | Out-Null

try {
    $archiveStream = [System.IO.FileStream]::new(
        $resolvedArchive,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::None
    )
    try {
        if ($archiveStream.Length -eq 0 -or $archiveStream.Length -gt 20GB) {
            throw 'Codec Pack archive is empty or exceeds its compressed-size limit.'
        }
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            $archiveHash = [System.Convert]::ToHexString(
                $sha256.ComputeHash($archiveStream)
            ).ToLowerInvariant()
        } finally {
            $sha256.Dispose()
        }
        if ($archiveHash -cne $TrustedArchiveSha256) {
            throw "Codec Pack archive SHA-256 mismatch: found $archiveHash"
        }
        $archiveStream.Position = 0
        Expand-SafeCodecPackArchive `
            -ArchiveStream $archiveStream `
            -DestinationPath $stagingPath
    } finally {
        $archiveStream.Dispose()
    }
    $manifest = Test-H3CodecPackDirectory -PackRoot $stagingPath
    $destination = Join-Path $packParent ([string]$manifest.pack_version)
    Assert-ChildPath -ParentPath $packParent -CandidatePath $destination | Out-Null
    if (Test-Path -LiteralPath $destination) {
        throw "Codec Pack version is already installed; refusing overwrite: $($manifest.pack_version)"
    }
    if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
        $otherScope = if ($Scope -eq 'CurrentUser') { 'AllUsers' } else { 'CurrentUser' }
        $otherRoot = Get-CodecPackInstallRoot -Scope $otherScope
        $otherVersion = Join-Path $otherRoot "org.latentdeck.h3/$($manifest.pack_version)"
        if (Test-Path -LiteralPath $otherVersion) {
            throw (
                "The same Codec Pack version exists in the $otherScope scope. " +
                'Remove that copy before installing to avoid a discovery conflict.'
            )
        }
    }

    [System.IO.Directory]::Move($stagingPath, $destination)
    try {
        Assert-PathComponentsNotReparsePoints -Path $destination
        Test-H3CodecPackDirectory `
            -PackRoot $destination `
            -ExpectedPackVersion ([string]$manifest.pack_version) | Out-Null
    } catch {
        if ((Test-Path -LiteralPath $destination -PathType Container) -and
            -not (Test-Path -LiteralPath $stagingPath)) {
            [System.IO.Directory]::Move($destination, $stagingPath)
        }
        throw
    }
    $stagingPath = $null
    Write-Output $destination
} finally {
    if ($null -ne $stagingPath) {
        Remove-SafeTemporaryDirectory `
            -ParentPath $stagingRoot `
            -CandidatePath $stagingPath `
            -RequiredLeafPrefix '.install-'
    }
    if ((Test-Path -LiteralPath $stagingRoot -PathType Container) -and
        @(Get-ChildItem -LiteralPath $stagingRoot -Force).Count -eq 0) {
        [System.IO.Directory]::Delete($stagingRoot, $false)
    }
}
} finally {
    if ($mutexHeld) {
        $lifecycleMutex.ReleaseMutex()
    }
    $lifecycleMutex.Dispose()
}
