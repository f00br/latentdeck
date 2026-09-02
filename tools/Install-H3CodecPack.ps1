[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ArchivePath,

    [Parameter(Mandatory)]
    [Alias('ExpectedArchiveSha256')]
    [string]$TrustedArchiveSha256,

    [ValidateSet('CurrentUser', 'AllUsers')]
    [string]$Scope = 'CurrentUser',

    [string]$InstallRoot,

    [switch]$Repair,

    [string]$LifecycleHelperPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

if ($Scope -cne 'CurrentUser') {
    throw 'Protocol 2 Codec Packs are installed only for the current user.'
}
Assert-Sha256 -Value $TrustedArchiveSha256 -Name 'TrustedArchiveSha256'

$resolvedArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
$archiveItem = Get-Item -LiteralPath $resolvedArchive -Force
if ($archiveItem.PSIsContainer -or
    ($archiveItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw 'Codec Pack archive must be a regular non-reparse file.'
}
$measuredArchiveSha256 = (
    Get-FileHash -LiteralPath $resolvedArchive -Algorithm SHA256
).Hash.ToLowerInvariant()
if ($measuredArchiveSha256 -cne $TrustedArchiveSha256) {
    throw "Codec Pack archive SHA-256 mismatch: found $measuredArchiveSha256"
}

if ([string]::IsNullOrWhiteSpace($LifecycleHelperPath)) {
    $LifecycleHelperPath = $env:LATENTDECK_H3_LIFECYCLE_HELPER
}
if ([string]::IsNullOrWhiteSpace($LifecycleHelperPath)) {
    throw (
        'LifecycleHelperPath is required. Use the exact build-authorized ' +
        'latentdeck-codec-pack-installer produced for this archive.'
    )
}
$resolvedHelper = (Resolve-Path -LiteralPath $LifecycleHelperPath).Path
$helperItem = Get-Item -LiteralPath $resolvedHelper -Force
if ($helperItem.PSIsContainer -or
    ($helperItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw 'LifecycleHelperPath must be a regular non-reparse file.'
}

if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        throw 'LOCALAPPDATA is unavailable; provide a canonical InstallRoot.'
    }
    $localAppData = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
} else {
    $codecRoot = [System.IO.Path]::GetFullPath($InstallRoot)
    $latentDeckRoot = [System.IO.Path]::GetDirectoryName($codecRoot)
    if ([System.IO.Path]::GetFileName($codecRoot) -cne 'CodecPacks' -or
        [System.IO.Path]::GetFileName($latentDeckRoot) -cne 'LatentDeck') {
        throw 'InstallRoot must have the canonical <LocalAppData>\LatentDeck\CodecPacks shape.'
    }
    $localAppData = [System.IO.Path]::GetDirectoryName($latentDeckRoot)
}
$programData = if ([string]::IsNullOrWhiteSpace($env:PROGRAMDATA)) {
    $localAppData
} else {
    [System.IO.Path]::GetFullPath($env:PROGRAMDATA)
}

$operation = if ($Repair) { 'repair' } else { 'install' }
$nativeOutput = @(& $resolvedHelper `
    --local-app-data $localAppData `
    --program-data $programData `
    $operation `
    --archive $resolvedArchive 2>&1)
$nativeExitCode = $LASTEXITCODE
if ($nativeExitCode -ne 0) {
    $detail = ($nativeOutput | Out-String).Trim().Replace("`r", ' ').Replace("`n", ' ')
    throw "Native H3 lifecycle helper failed with exit code $nativeExitCode`: $detail"
}
$nativeOutput
