[CmdletBinding()]
param(
    [string]$EnvironmentRoot,

    [ValidateRange(0, 65535)]
    [int]$Port = 0,

    [switch]$Refresh,

    [string]$ComfyRoot,

    [string]$PythonExecutable,

    [string]$ModelsRoot,

    [string]$HqVaePath,

    [switch]$Cpu,

    [switch]$OpenBrowser,

    [Parameter(ValueFromRemainingArguments)]
    [string[]]$ComfyArguments
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
if ([string]::IsNullOrWhiteSpace($EnvironmentRoot)) {
    $EnvironmentRoot = Join-Path $artifactsRoot 'comfy-test'
}
$environmentFull = [System.IO.Path]::GetFullPath($EnvironmentRoot)
$relativeEnvironment = [System.IO.Path]::GetRelativePath(
    [System.IO.Path]::GetFullPath($artifactsRoot),
    $environmentFull
)
if ($relativeEnvironment -eq '.' -or
    $relativeEnvironment.StartsWith('..', [System.StringComparison]::Ordinal) -or
    [System.IO.Path]::IsPathFullyQualified($relativeEnvironment)) {
    throw 'EnvironmentRoot must be a child of the repository artifacts directory.'
}

$receiptPath = Join-Path $environmentFull 'environment.json'
if ($Refresh -or -not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
    $initialize = Join-Path $PSScriptRoot 'Initialize-IsolatedComfyEnvironment.ps1'
    $initializeArguments = @{
        EnvironmentRoot = $environmentFull
        Port = if ($Port -eq 0) { 8192 } else { $Port }
    }
    foreach ($entry in @(
        @{ Name = 'ComfyRoot'; Value = $ComfyRoot },
        @{ Name = 'PythonExecutable'; Value = $PythonExecutable },
        @{ Name = 'ModelsRoot'; Value = $ModelsRoot },
        @{ Name = 'HqVaePath'; Value = $HqVaePath }
    )) {
        if (-not [string]::IsNullOrWhiteSpace($entry.Value)) {
            $initializeArguments[$entry.Name] = $entry.Value
        }
    }
    & $initialize @initializeArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Environment preparation failed with exit code $LASTEXITCODE."
    }
}
elseif (-not [string]::IsNullOrWhiteSpace($ComfyRoot) -or
    -not [string]::IsNullOrWhiteSpace($PythonExecutable) -or
    -not [string]::IsNullOrWhiteSpace($ModelsRoot) -or
    -not [string]::IsNullOrWhiteSpace($HqVaePath)) {
    throw 'Source path overrides require -Refresh so the environment receipt is rebuilt.'
}

& (Join-Path $PSScriptRoot 'Test-IsolatedComfyEnvironment.ps1') `
    -EnvironmentRoot $environmentFull
if ($LASTEXITCODE -ne 0) {
    throw "Environment smoke failed with exit code $LASTEXITCODE."
}

$receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -Depth 20
$launchPort = if ($Port -eq 0) { [int]$receipt.port } else { $Port }
if ($launchPort -lt 1024) {
    throw 'The isolated Comfy launcher requires a user port from 1024 through 65535.'
}
$listeners = @(Get-NetTCPConnection -State Listen -LocalPort $launchPort -ErrorAction SilentlyContinue)
if ($listeners.Count -gt 0) {
    $processes = @($listeners | ForEach-Object { $_.OwningProcess } | Sort-Object -Unique)
    throw "Port $launchPort is already in use by process id(s): $($processes -join ', ')."
}

$blockedArguments = @(
    '--base-directory',
    '--models-directory',
    '--output-directory',
    '--temp-directory',
    '--input-directory',
    '--user-directory',
    '--database-url',
    '--extra-model-paths-config',
    '--listen',
    '--port',
    '--enable-manager',
    '--disable-all-custom-nodes',
    '--whitelist-custom-nodes',
    '--front-end-version',
    '--auto-launch'
)
$forwardedArguments = @(
    $ComfyArguments | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)
foreach ($argument in $forwardedArguments) {
    $name = $argument.Split('=', 2)[0]
    if ($blockedArguments -ccontains $name) {
        throw "Comfy argument $name would violate the isolated launch contract."
    }
}

$python = [string]$receipt.python.executable
$bootstrap = [string]$receipt.paths.bootstrap
$databaseUrl = 'sqlite:///' + ([string]$receipt.paths.database).Replace('\', '/')
$arguments = @(
    '-B',
    $bootstrap,
    '--base-directory', [string]$receipt.paths.base_directory,
    '--database-url', $databaseUrl,
    '--extra-model-paths-config', [string]$receipt.paths.extra_model_paths,
    '--listen', '127.0.0.1',
    '--port', "$launchPort",
    '--disable-auto-launch',
    '--disable-api-nodes',
    '--disable-all-custom-nodes',
    '--whitelist-custom-nodes', 'latentdeck_toolkit', 'comfyui_latent_cartridge', 'latentdeck_example_channel_roll',
    '--log-stdout'
)
if ($Cpu) {
    $arguments += '--cpu'
}
$arguments += $forwardedArguments

$url = "http://127.0.0.1:$launchPort"
Write-Host "Starting isolated LatentDeck Comfy environment: $url" -ForegroundColor Green
Write-Host "Base directory: $($receipt.paths.base_directory)"
Write-Host "Python overlay: $($receipt.paths.python_packages)"
Write-Host 'Only Toolkit, ComfyUI-LatentCartridge, and the reviewed example operator are enabled.'
Write-Warning 'Press Ctrl+C in this terminal to stop the isolated ComfyUI process.'

if ($OpenBrowser) {
    Start-Process $url
}

$oldBytecode = $env:PYTHONDONTWRITEBYTECODE
try {
    $env:PYTHONDONTWRITEBYTECODE = '1'
    Push-Location ([string]$receipt.paths.base_directory)
    try {
        & $python @arguments
        if ($LASTEXITCODE -ne 0) {
            throw "Isolated ComfyUI exited with code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    $env:PYTHONDONTWRITEBYTECODE = $oldBytecode
    $tempDirectory = [string]$receipt.paths.temp
    if (Test-Path -LiteralPath $tempDirectory) {
        if (-not (Test-Path -LiteralPath $tempDirectory -PathType Container)) {
            throw 'Generated temp path exists but is not a directory after Comfy shutdown.'
        }
        $tempItem = Get-Item -LiteralPath $tempDirectory -Force
        if (($tempItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'Generated temp directory became a reparse point after Comfy shutdown.'
        }
    }
    else {
        [System.IO.Directory]::CreateDirectory($tempDirectory) | Out-Null
    }
}
