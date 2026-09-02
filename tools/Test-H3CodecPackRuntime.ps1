[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackRoot,

    [string]$ReceiptPath,

    [switch]$RequireCuda
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$nativeArgumentMode = Get-Variable `
    -Name PSNativeCommandArgumentPassing `
    -ValueOnly `
    -ErrorAction SilentlyContinue
if ([string]$nativeArgumentMode -ceq 'Legacy') {
    throw 'H3 Codec Pack runtime smoke requires PowerShell native argument mode Windows or Standard.'
}

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$resolvedRoot = (Resolve-Path -LiteralPath $PackRoot).Path
$manifest = Test-H3CodecPackDirectory -PackRoot $resolvedRoot
$python = Join-Path $resolvedRoot 'runtime/python.exe'
$probePath = Join-Path $PSScriptRoot 'h3_codec_pack_runtime_probe.py'
if (-not (Test-Path -LiteralPath $probePath -PathType Leaf)) {
    throw "H3 Codec Pack Protocol 2 probe is missing: $probePath"
}

$inventoryPath = Join-Path $resolvedRoot ([string]$manifest.runtime_lock.path)
$inventory = Read-StrictJsonFile -Path $inventoryPath
$pythonComponents = @(
    $inventory.components |
        Where-Object {
            [string]$_.name -ceq 'CPython' -and [string]$_.kind -ceq 'runtime'
        }
)
if ($pythonComponents.Count -ne 1 -or
    [string]$pythonComponents[0].version -cnotmatch '^3\.13\.\d+$') {
    throw 'Codec Pack dependency inventory does not declare one exact CPython 3.13 runtime.'
}

$manifestJson = ConvertTo-Json -InputObject $manifest -Depth 32 -Compress
$manifestBase64 = [System.Convert]::ToBase64String(
    [System.Text.Encoding]::UTF8.GetBytes($manifestJson)
)
$rawRuntime = & $python -I -s -B -X utf8 $probePath `
    $RequireCuda.IsPresent.ToString().ToLowerInvariant() `
    ([string]$pythonComponents[0].version) `
    $manifestBase64
if ($LASTEXITCODE -ne 0) {
    throw "H3 Codec Pack isolated Protocol 2 probe failed with exit code $LASTEXITCODE."
}
if (@($rawRuntime).Count -ne 1) {
    throw 'H3 Codec Pack isolated Protocol 2 probe emitted unexpected output.'
}
try {
    $runtime = [string]$rawRuntime | ConvertFrom-Json
} catch {
    throw 'H3 Codec Pack isolated Protocol 2 probe did not emit valid JSON.'
}

if ([int]$runtime.protocol.selected_version -ne 2 -or
    [int]$runtime.protocol.worker_protocol -ne 2 -or
    (@($runtime.protocol.commands) -join "`0") -cne
        (@('session.configure', 'codec.descriptor') -join "`0") -or
    [string]$runtime.rgb_ring_abi.protocol2 -cne '2' -or
    [bool]$runtime.preload_guards.torch_imported -or
    [int]$runtime.preload_guards.external_decoder_accesses -ne 0) {
    throw 'H3 Codec Pack runtime probe did not prove the strict Protocol 2 pre-load boundary.'
}

$receipt = [ordered]@{
    schema_version = 1
    pack_id = [string]$manifest.pack_id
    pack_version = [string]$manifest.pack_version
    platform = 'windows-x86_64'
    runtime = $runtime
    contains_model_weights = $false
    contains_generator = $false
    contains_comfy = $false
    external_decoder_selection_required = $true
    result = 'passed'
}

if (-not [string]::IsNullOrWhiteSpace($ReceiptPath)) {
    $receiptFullPath = [System.IO.Path]::GetFullPath($ReceiptPath)
    if (Test-Path -LiteralPath $receiptFullPath) {
        throw "Refusing to overwrite an existing runtime receipt: $receiptFullPath"
    }
    $receiptParent = Split-Path -Parent $receiptFullPath
    [System.IO.Directory]::CreateDirectory($receiptParent) | Out-Null
    $partial = Join-Path $receiptParent (
        ".$(Split-Path -Leaf $receiptFullPath).partial-$([guid]::NewGuid().ToString('N'))"
    )
    try {
        Write-JsonFile -Value $receipt -Path $partial
        [System.IO.File]::Move($partial, $receiptFullPath)
    } finally {
        if (Test-Path -LiteralPath $partial -PathType Leaf) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
}

$receipt | ConvertTo-Json -Depth 16 -Compress | Write-Output
