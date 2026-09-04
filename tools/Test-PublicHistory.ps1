[CmdletBinding()]
param(
    [string]$RepositoryRoot,

    [string]$Ref = 'refs/heads/main'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..'
}
$repoRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path

function Invoke-GitLines {
    param(
        [Parameter(Mandatory)]
        [string[]]$Arguments,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $lines = @(& git -c core.quotepath=false -C $repoRoot @Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE."
    }
    return $lines
}

$null = Invoke-GitLines -Arguments @('rev-parse', '--verify', "$Ref^{commit}") -Label 'Resolve public ref'
$failures = [System.Collections.Generic.List[string]]::new()
$warnings = [System.Collections.Generic.List[string]]::new()
$shallowState = @(
    Invoke-GitLines `
        -Arguments @('rev-parse', '--is-shallow-repository') `
        -Label 'Determine history completeness'
)
if ($shallowState.Count -ne 1 -or $shallowState[0] -cnotin @('true', 'false')) {
    throw 'Git returned an unexpected shallow-repository state.'
}
if ($shallowState[0] -ceq 'true') {
    $failures.Add(
        'Public history audit requires a complete clone; configure checkout with fetch-depth: 0.'
    )
}

$forbiddenExtensions = @(
    '.lc', '.h3latent', '.safetensors', '.npy', '.npz', '.ckpt', '.pt', '.pth', '.onnx',
    '.engine', '.plan', '.gguf', '.bin', '.p12', '.pfx', '.key', '.pem',
    '.exe', '.dll', '.msi', '.msix', '.appx', '.zip', '.7z', '.rar',
    '.tar', '.tgz', '.gz', '.mp4', '.mov', '.mkv', '.avi', '.webm',
    '.wav', '.flac', '.mp3'
)
$forbiddenLeafNames = @(
    '.env', 'credentials.json', 'credentials.toml', 'secrets.json',
    'secrets.toml'
)

$historicalPaths = @(
    Invoke-GitLines `
        -Arguments @('log', '--format=', '--name-only', $Ref, '--') `
        -Label 'Enumerate public history paths' |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Sort-Object -Unique
)
foreach ($path in $historicalPaths) {
    $portable = $path.Replace('\', '/')
    $leaf = [System.IO.Path]::GetFileName($portable).ToLowerInvariant()
    $extension = [System.IO.Path]::GetExtension($portable).ToLowerInvariant()
    if ($forbiddenExtensions -contains $extension) {
        $failures.Add("Forbidden payload type exists in public history: $portable")
    }
    if (($forbiddenLeafNames -contains $leaf) -or $leaf.StartsWith('.env.')) {
        if ($leaf -ne '.env.example') {
            $failures.Add("Credential or local-config filename exists in public history: $portable")
        }
    }
}

$objectLines = Invoke-GitLines -Arguments @('rev-list', '--objects', $Ref) -Label 'Enumerate public objects'
$objectIds = @(
    $objectLines |
        ForEach-Object { ($_ -split ' ', 2)[0] } |
        Where-Object { $_ -match '^[0-9a-f]{40,64}$' } |
        Sort-Object -Unique
)
$objectInfo = @($objectIds | & git -C $repoRoot cat-file --batch-check='%(objectname) %(objecttype) %(objectsize)')
if ($LASTEXITCODE -ne 0) {
    throw "Measure public objects failed with exit code $LASTEXITCODE."
}

$blobCount = 0
$maximumBlobBytes = 25MB
foreach ($line in $objectInfo) {
    if ($line -notmatch '^([0-9a-f]{40,64}) (\S+) (\d+)$') {
        throw "Unexpected git cat-file output: $line"
    }
    if ($Matches[2] -cne 'blob') {
        continue
    }
    $blobCount++
    $size = [int64]$Matches[3]
    if ($size -gt $maximumBlobBytes) {
        $failures.Add("Historical blob exceeds 25 MiB review threshold: $($Matches[1]) ($size bytes)")
    }
}

# Scan every patch reachable from the exact public branch. Patterns are kept
# high-confidence so negative test fixtures and ordinary credential vocabulary
# do not create noisy false positives. Matching values are never printed.
$historyText = (
    Invoke-GitLines `
        -Arguments @('log', '--format=fuller', '--no-ext-diff', '--no-textconv', '-p', $Ref, '--') `
        -Label 'Read public history patches'
) -join "`n"
$secretPatterns = [ordered]@{
    'private key material' = '-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----'
    'GitHub token' = 'gh[pousr]_[A-Za-z0-9]{36,}'
    'OpenAI token' = 'sk-(?:proj-)?[A-Za-z0-9_-]{32,}'
    'AWS access key' = 'AKIA[0-9A-Z]{16}'
    'Hugging Face token' = 'hf_[A-Za-z0-9]{30,}'
    'Slack token' = 'xox[baprs]-[A-Za-z0-9-]{20,}'
}
foreach ($entry in $secretPatterns.GetEnumerator()) {
    if ($historyText -match $entry.Value) {
        $failures.Add("High-confidence $($entry.Key) pattern exists in public history.")
    }
}

$auxiliaryRefs = @(
    Invoke-GitLines `
        -Arguments @('for-each-ref', '--format=%(refname)', 'refs/codex/') `
        -Label 'Enumerate local Codex refs' |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)
if ($auxiliaryRefs.Count -gt 0) {
    $warnings.Add(
        "Found $($auxiliaryRefs.Count) local refs/codex ref(s). Publication still permits only refs/heads/main; never use --all or --mirror."
    )
}

if ($warnings.Count -gt 0) {
    Write-Host 'WARNINGS:' -ForegroundColor Yellow
    foreach ($warning in $warnings) {
        Write-Host "  - $warning" -ForegroundColor Yellow
    }
}

if ($failures.Count -gt 0) {
    Write-Host 'PUBLIC HISTORY AUDIT: FAIL' -ForegroundColor Red
    foreach ($failure in $failures) {
        Write-Host "  - $failure" -ForegroundColor Red
    }
    exit 1
}

Write-Host 'PUBLIC HISTORY AUDIT: PASS' -ForegroundColor Green
Write-Host "Ref: $Ref"
Write-Host "Historical paths: $($historicalPaths.Count)"
Write-Host "Reachable blobs: $blobCount"
