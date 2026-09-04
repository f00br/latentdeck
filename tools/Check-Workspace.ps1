[CmdletBinding()]
param(
    [ValidateNotNullOrEmpty()]
    [string]$PublicHistoryRef = 'refs/heads/main'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$nodeRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNode.ps1')
$nsisRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNsis.ps1') -AllowNetwork
$env:PATH = "$nodeRoot;$env:PATH"
$pnpm = Join-Path $nodeRoot 'pnpm.cmd'
$previousPythonNoBytecode = $env:PYTHONDONTWRITEBYTECODE
$env:PYTHONDONTWRITEBYTECODE = '1'

function Invoke-Checked {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Command,
        [Parameter(Mandatory)]
        [string]$Label
    )

    Write-Host "==> $Label" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
}

Push-Location $repositoryRoot
try {
    Invoke-Checked { & $pnpm --version } 'pnpm version'
    Invoke-Checked { cargo fmt --all -- --check } 'Rust format'
    Invoke-Checked { & $pnpm install --frozen-lockfile } 'pnpm lock/install'
    Invoke-Checked { & $pnpm format:check } 'Frontend format'
    Invoke-Checked { & $pnpm lint } 'Svelte/TypeScript checks'
    Invoke-Checked { & $pnpm test } 'Frontend tests'
    Invoke-Checked { & $pnpm build } 'Frontend builds'
    # Tauri's generate_context! macro validates frontendDist during Rust
    # compilation, so a genuinely clean clone must build both dist trees first.
    Invoke-Checked { cargo clippy --workspace --all-targets -- -D warnings } 'Rust Clippy'
    Invoke-Checked { cargo test --workspace --all-targets } 'Rust tests'
    Invoke-Checked { uv lock --check } 'uv lock'
    Invoke-Checked {
        uv sync --all-packages --all-extras --no-editable --locked `
            --reinstall-package latentdeck-cartridge `
            --reinstall-package latentdeck-codec-h3 `
            --reinstall-package latentdeck-codec-host `
            --reinstall-package latentdeck-codec-sdk `
            --reinstall-package latentdeck-comfy-cartridge `
            --reinstall-package latentdeck-comfy-toolkit `
            --reinstall-package latentdeck-deck-sdk `
            --reinstall-package latentdeck-example-channel-roll `
            --reinstall-package latentdeck-operator-d2 `
            --reinstall-package latentdeck-operator-q4 `
            --reinstall-package latentdeck-rgb-ring
    } 'Python lock/install'
    Invoke-Checked {
        uv run --no-sync ruff check `
            pyproject.toml codec-host comfy operators sdk `
            tools/codec_pack_curator.py tools/tests/test_codec_pack_curator.py
    } 'Python lint'
    Invoke-Checked { uv run --no-sync pytest } 'Python tests'
    Invoke-Checked {
        uv run --no-sync pytest -q tools/tests/test_codec_pack_curator.py
    } 'Codec Pack curator tests'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1 } 'Developer onboarding contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-ComfyRecorderBundle.ps1 } 'Comfy Recorder bundle contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-LinkedDevCodecPack.ps1 } 'Linked development Codec Pack contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-ReleasePackaging.ps1 } 'Release packaging contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-DeveloperKitPackaging.ps1 } 'Developer Kit packaging contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-GitHubReleaseStaging.ps1 } 'GitHub Release staging contract'
    Invoke-Checked {
        pwsh -NoProfile -File tools/Test-H3CodecPackSetup.ps1 -NsisRoot $nsisRoot
    } 'H3 Codec Pack setup contract'
    Invoke-Checked {
        pwsh -NoProfile -File tools/Test-PrivateProtocol2GpuGate.ps1
    } 'Private Protocol 2 GPU gate contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-DiagnosticBundle.ps1 } 'Diagnostic bundle contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicDocumentationContract.ps1 } 'Public-documentation audit contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicDocumentation.ps1 } 'Public documentation audit'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicAssetProvenance.ps1 } 'Public asset provenance audit'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-CiSecurityContract.ps1 } 'CI security contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicHistoryContract.ps1 } 'Public-history audit contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicHistory.ps1 -Ref $PublicHistoryRef } 'Public history/archive audit'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicTreeContract.ps1 } 'Public-tree audit contract'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicTree.ps1 } 'Public-tree audit'
    Invoke-Checked { git diff --check } 'Working-tree whitespace'
    Invoke-Checked { git diff --cached --check } 'Staged whitespace'
}
finally {
    Pop-Location
    if ($null -eq $previousPythonNoBytecode) {
        Remove-Item Env:PYTHONDONTWRITEBYTECODE -ErrorAction SilentlyContinue
    }
    else {
        $env:PYTHONDONTWRITEBYTECODE = $previousPythonNoBytecode
    }
}

Write-Host 'WORKSPACE CHECK: PASS' -ForegroundColor Green
