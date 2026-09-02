[CmdletBinding()]
param(
    [string]$ReceiptPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $repoRoot 'apps/latentdeck/src-tauri/Cargo.toml'
$testName = 'private_protocol2_gpu_gate'

Push-Location $repoRoot
try {
    & cargo test --locked --manifest-path $manifest --test $testName
    if ($LASTEXITCODE -ne 0) {
        throw 'Private Protocol 2 GPU gate static contract failed.'
    }

    if ([string]::IsNullOrWhiteSpace($ReceiptPath)) {
        [ordered]@{
            schema_version = 1
            evidence_kind = 'latentdeck_private_protocol2_gpu_gate_contract'
            worker_protocol = 2
            worker_module = 'latentdeck_codec_host'
            codec_manifest_version = '2.0.0'
            codec = 'org.latentdeck.h3@0.2.0'
            adapter = 'org.latentdeck.h3@0.2.0'
            adapter_entrypoint = 'latentdeck_codec_h3.adapter:make_adapter'
            profile = 'minimax_h3/h3_av_latent/0.1.0'
            capabilities = @(
                'player',
                'realtime',
                'resample',
                'snapshot_capture',
                'live_capture',
                'raw_import'
            )
            decks = @(
                'org.latentdeck.deck.d2@0.2.0',
                'org.latentdeck.deck.q4@0.2.0'
            )
            external_deck = 'dev.latentdeck.private.h3_probe@0.2.0'
            required_surfaces = @(
                'player',
                'd2',
                'q4',
                'external_deck_after_runtime_start',
                'snapshot',
                'captured_lc_player_replay',
                'live_capture',
                'mp4',
                'spout',
                'stability_360_seconds_each'
            )
            gpu_executed = $false
            result = 'contract_passed'
        } | ConvertTo-Json -Depth 8 -Compress | Write-Output
        return
    }

    $receiptFullPath = [System.IO.Path]::GetFullPath($ReceiptPath)
    if (-not [System.IO.Path]::IsPathFullyQualified($receiptFullPath) -or
        -not (Test-Path -LiteralPath $receiptFullPath -PathType Leaf)) {
        throw 'Private Protocol 2 GPU evidence must be one existing absolute receipt file.'
    }
    $receiptInfo = Get-Item -LiteralPath $receiptFullPath -Force
    if (($receiptInfo.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $receiptInfo.Length -le 0 -or
        $receiptInfo.Length -gt 1MB) {
        throw 'Private Protocol 2 GPU evidence must be a non-reparse JSON file of at most 1 MiB.'
    }

    $previousOptIn = [Environment]::GetEnvironmentVariable(
        'LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE',
        'Process'
    )
    $previousReceipt = [Environment]::GetEnvironmentVariable(
        'LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT',
        'Process'
    )
    try {
        $env:LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE = '1'
        $env:LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT = $receiptFullPath
        & cargo test --locked --manifest-path $manifest --test $testName `
            -- validate_private_protocol2_gpu_gate_receipt --ignored --exact --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw 'Private Protocol 2 GPU evidence receipt failed its closed validator.'
        }
    } finally {
        [Environment]::SetEnvironmentVariable(
            'LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE',
            $previousOptIn,
            'Process'
        )
        [Environment]::SetEnvironmentVariable(
            'LATENTDECK_PRIVATE_PROTOCOL2_GPU_GATE_RECEIPT',
            $previousReceipt,
            'Process'
        )
    }

    [ordered]@{
        schema_version = 1
        evidence_kind = 'latentdeck_private_protocol2_gpu_gate_validation'
        worker_protocol = 2
        gpu_executed_by_this_command = $false
        result = 'passed'
    } | ConvertTo-Json -Depth 4 -Compress | Write-Output
} finally {
    Pop-Location
}
