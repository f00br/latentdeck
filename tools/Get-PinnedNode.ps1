[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$nodeVersion = '24.20.0'
$archiveName = "node-v$nodeVersion-win-x64.zip"
$archiveSha256 = '6cac9ffbca8f6a47091e4b5c772e0606049c3871cb67d900c0cedde630e545ba'
$repositoryRoot = Split-Path -Parent $PSScriptRoot
$toolsRoot = Join-Path $repositoryRoot '.tools'
$nodeRoot = Join-Path $toolsRoot "node-v$nodeVersion-win-x64"
$nodeExecutable = Join-Path $nodeRoot 'node.exe'

if (-not (Test-Path -LiteralPath $nodeExecutable)) {
    New-Item -ItemType Directory -Force -Path $toolsRoot | Out-Null
    $archivePath = Join-Path $toolsRoot $archiveName

    if (-not (Test-Path -LiteralPath $archivePath)) {
        $partialPath = "$archivePath.partial"
        Invoke-WebRequest -UseBasicParsing `
            -Uri "https://nodejs.org/dist/v$nodeVersion/$archiveName" `
            -OutFile $partialPath

        $partialHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $partialPath).Hash.ToLowerInvariant()
        if ($partialHash -ne $archiveSha256) {
            throw "Pinned Node archive checksum mismatch: $partialHash"
        }

        Move-Item -LiteralPath $partialPath -Destination $archivePath
    }

    $archiveHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
    if ($archiveHash -ne $archiveSha256) {
        throw "Cached Node archive checksum mismatch: $archiveHash"
    }

    Expand-Archive -LiteralPath $archivePath -DestinationPath $toolsRoot
}

$actualVersion = (& $nodeExecutable --version).TrimStart('v')
if ($actualVersion -ne $nodeVersion) {
    throw "Expected Node $nodeVersion, found $actualVersion"
}

$corepack = Join-Path $nodeRoot 'corepack.cmd'
& $corepack enable pnpm --install-directory $nodeRoot
if ($LASTEXITCODE -ne 0) {
    throw "Corepack could not enable the pinned pnpm shim"
}

Write-Output $nodeRoot
