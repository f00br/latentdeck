[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$nodeRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNode.ps1')
$env:PATH = "$nodeRoot;$env:PATH"
$pnpm = Join-Path $nodeRoot 'pnpm.cmd'

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
    Invoke-Checked { cargo clippy --workspace --all-targets -- -D warnings } 'Rust Clippy'
    Invoke-Checked { cargo test --workspace --all-targets } 'Rust tests'
    Invoke-Checked { & $pnpm install --frozen-lockfile } 'pnpm lock/install'
    Invoke-Checked { & $pnpm lint } 'Svelte/TypeScript checks'
    Invoke-Checked { & $pnpm test } 'Frontend tests'
    Invoke-Checked { & $pnpm build } 'Frontend builds'
    Invoke-Checked { uv lock --check } 'uv lock'
    Invoke-Checked { uv run ruff check pyproject.toml codec-host sdk/python } 'Python lint'
    Invoke-Checked { uv run --all-packages pytest } 'Python tests'
    Invoke-Checked { pwsh -NoProfile -File tools/Test-PublicTree.ps1 } 'Public-tree audit'
    Invoke-Checked { git diff --check } 'Working-tree whitespace'
    Invoke-Checked { git diff --cached --check } 'Staged whitespace'
}
finally {
    Pop-Location
}

Write-Host 'WORKSPACE CHECK: PASS' -ForegroundColor Green
