[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackVersion,

    [ValidateSet('CurrentUser', 'AllUsers')]
    [string]$Scope = 'CurrentUser',

    [string]$InstallRoot,

    [switch]$RemoveCorrupt,

    [switch]$CleanupQuarantine,

    [string]$LifecycleHelperPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

if ($Scope -cne 'CurrentUser') {
    throw 'Protocol 2 Codec Packs are installed only for the current user.'
}
if ($CleanupQuarantine) {
    throw (
        'CleanupQuarantine is retired. The shared extension lifecycle owns and ' +
        'recovers its exact staging and quarantine entries.'
    )
}
Assert-SemVer -Value $PackVersion -Name 'PackVersion'

if ([string]::IsNullOrWhiteSpace($LifecycleHelperPath)) {
    $LifecycleHelperPath = $env:LATENTDECK_H3_LIFECYCLE_HELPER
}
if ([string]::IsNullOrWhiteSpace($LifecycleHelperPath)) {
    throw (
        'LifecycleHelperPath is required. Use a latentdeck-codec-pack-installer ' +
        'built from the current source tree.'
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

$arguments = @(
    '--local-app-data', $localAppData,
    '--program-data', $programData,
    'uninstall',
    '--version', $PackVersion
)
if ($RemoveCorrupt) {
    $arguments += '--remove-corrupt'
}
$nativeOutput = @(& $resolvedHelper @arguments 2>&1)
$nativeExitCode = $LASTEXITCODE
if ($nativeExitCode -ne 0) {
    $detail = ($nativeOutput | Out-String).Trim().Replace("`r", ' ').Replace("`n", ' ')
    throw "Native H3 lifecycle helper failed with exit code $nativeExitCode`: $detail"
}
$nativeOutput
