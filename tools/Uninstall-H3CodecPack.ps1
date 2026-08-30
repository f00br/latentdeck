[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackVersion,

    [ValidateSet('CurrentUser', 'AllUsers')]
    [string]$Scope = 'CurrentUser',

    [string]$InstallRoot,

    [switch]$RemoveCorrupt,

    [switch]$CleanupQuarantine
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

Assert-SemVer -Value $PackVersion -Name 'PackVersion'
if ($Scope -eq 'AllUsers' -and [string]::IsNullOrWhiteSpace($InstallRoot)) {
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'An all-users Codec Pack uninstall requires an elevated PowerShell process.'
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
if (-not (Test-Path -LiteralPath $root -PathType Container)) {
    throw 'The requested Codec Pack install root does not exist.'
}
Assert-DirectoryNotReparsePoint -Path $root
Assert-PathComponentsNotReparsePoints -Path $root
$packParent = Join-Path $root 'org.latentdeck.h3'
$destination = Join-Path $packParent $PackVersion
Assert-ChildPath -ParentPath $root -CandidatePath $packParent | Out-Null
Assert-ChildPath -ParentPath $packParent -CandidatePath $destination | Out-Null
$trashRoot = Get-CodecPackAuxiliaryRoot -InstallRoot $root -Kind Trash
$removalPrefix = ".remove-org.latentdeck.h3-$PackVersion-"

if ($CleanupQuarantine -and (Test-Path -LiteralPath $trashRoot -PathType Container)) {
    Assert-DirectoryNotReparsePoint -Path $trashRoot
    Assert-PathComponentsNotReparsePoints -Path $trashRoot
    $matchingTrash = @(
        Get-ChildItem -LiteralPath $trashRoot -Force -Directory |
            Where-Object {
                $_.Name.StartsWith($removalPrefix, [System.StringComparison]::Ordinal)
            }
    )
    foreach ($trash in $matchingTrash) {
        Remove-SafeTemporaryDirectory `
            -ParentPath $trashRoot `
            -CandidatePath $trash.FullName `
            -RequiredLeafPrefix $removalPrefix
    }
    if (@(Get-ChildItem -LiteralPath $trashRoot -Force).Count -eq 0) {
        [System.IO.Directory]::Delete($trashRoot, $false)
    }
}

if ($CleanupQuarantine) {
    Write-Output "Matching quarantine cleanup completed for org.latentdeck.h3 $PackVersion; installed versions were not changed."
    return
}

if (-not (Test-Path -LiteralPath $destination -PathType Container)) {
    throw "H3 Codec Pack $PackVersion is not installed in the selected scope."
}
Assert-DirectoryNotReparsePoint -Path $packParent
Assert-DirectoryNotReparsePoint -Path $destination
Assert-PathComponentsNotReparsePoints -Path $destination

if (-not $RemoveCorrupt) {
    Test-H3CodecPackDirectory `
        -PackRoot $destination `
        -ExpectedPackVersion $PackVersion | Out-Null
}

[System.IO.Directory]::CreateDirectory($trashRoot) | Out-Null
Assert-DirectoryNotReparsePoint -Path $trashRoot
Assert-PathComponentsNotReparsePoints -Path $trashRoot
$removalPath = Join-Path $trashRoot "$removalPrefix$([guid]::NewGuid().ToString('N'))"
Assert-SafeTemporaryDirectory `
    -ParentPath $trashRoot `
    -CandidatePath $removalPath `
    -RequiredLeafPrefix $removalPrefix | Out-Null
$moved = $false
try {
    [System.IO.Directory]::Move($destination, $removalPath)
    $moved = $true
    Remove-SafeTemporaryDirectory `
        -ParentPath $trashRoot `
        -CandidatePath $removalPath `
        -RequiredLeafPrefix $removalPrefix
    $moved = $false
} catch {
    if ($moved) {
        throw (
            "Codec Pack $PackVersion was removed from discovery, but quarantined files " +
            "could not be deleted at '$removalPath'. Stop Codec Pack workers, then rerun " +
            "this command with -CleanupQuarantine. Original error: $($_.Exception.Message)"
        )
    }
    throw
}

if (@(Get-ChildItem -LiteralPath $packParent -Force).Count -eq 0) {
    [System.IO.Directory]::Delete($packParent, $false)
}
if ((Test-Path -LiteralPath $trashRoot -PathType Container) -and
    @(Get-ChildItem -LiteralPath $trashRoot -Force).Count -eq 0) {
    [System.IO.Directory]::Delete($trashRoot, $false)
}
Write-Output "Removed org.latentdeck.h3 $PackVersion from the selected scope."
} finally {
    if ($mutexHeld) {
        $lifecycleMutex.ReleaseMutex()
    }
    $lifecycleMutex.Dispose()
}
