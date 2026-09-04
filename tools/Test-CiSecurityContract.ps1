[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$workflowPath = Join-Path $repoRoot '.github/workflows/ci.yml'
$configPath = Join-Path $repoRoot '.gitleaks.toml'
$utf8 = [System.Text.UTF8Encoding]::new($false, $true)

$workflow = $utf8.GetString([System.IO.File]::ReadAllBytes($workflowPath))
$config = $utf8.GetString([System.IO.File]::ReadAllBytes($configPath))
$failures = [System.Collections.Generic.List[string]]::new()

$usesMatches = [regex]::Matches($workflow, '(?m)^\s*uses:\s*[^@\s]+@(?<ref>\S+)')
if ($usesMatches.Count -eq 0) {
    $failures.Add('CI contains no external action references to validate.')
}
foreach ($match in $usesMatches) {
    if ($match.Groups['ref'].Value -cnotmatch '^[0-9a-f]{40}$') {
        $failures.Add("CI action is not pinned to a full commit SHA: $($match.Value.Trim())")
    }
}

if ($workflow -notmatch '(?m)^permissions:\s*\r?\n\s+contents:\s*read\s*$') {
    $failures.Add('CI does not declare repository contents read-only at workflow scope.')
}
$secretHistoryJob = [regex]::Match(
    $workflow,
    '(?ms)^  secret-history:[ \t]*\r?\n(?<body>.*?)(?=^  [A-Za-z0-9_-]+:[ \t]*\r?\n|\z)'
)
$secretHistoryPermissions = if ($secretHistoryJob.Success) {
    [regex]::Match(
        $secretHistoryJob.Groups['body'].Value,
        '(?m)^    permissions:[ \t]*\r?\n(?<body>(?:^      [a-z-]+:[ \t]*(?:read|write|none)[ \t]*(?:\r?\n|$))+)'
    )
} else {
    [System.Text.RegularExpressions.Match]::Empty
}
$secretHistoryPermissionLines = if ($secretHistoryPermissions.Success) {
    @(
        [regex]::Matches(
            $secretHistoryPermissions.Groups['body'].Value,
            '(?m)^      (?<scope>[a-z-]+):[ \t]*(?<access>read|write|none)[ \t]*$'
        ) | ForEach-Object {
            "$($_.Groups['scope'].Value): $($_.Groups['access'].Value)"
        }
    )
} else {
    @()
}
$expectedSecretHistoryPermissions = "contents: read`npull-requests: read"
if (($secretHistoryPermissionLines -join "`n") -cne $expectedSecretHistoryPermissions) {
    $failures.Add('Secret history must have only the contents: read and pull-requests: read scopes needed to inspect PR commits.')
}
$pullRequestReadCount = [regex]::Matches(
    $workflow,
    '(?m)^\s+pull-requests:\s*read\s*$'
).Count
if ($pullRequestReadCount -ne 1) {
    $failures.Add('CI must grant pull-requests: read exactly once, only to the Secret history job.')
}
if ($workflow -match '(?im)^\s*[a-z-]+:\s*write\s*$') {
    $failures.Add('CI declares a write-capable token permission.')
}
if ($workflow -match '(?m)^\s*pull_request_target\s*:') {
    $failures.Add('CI must not execute pull-request code through pull_request_target.')
}

$checkoutCount = [regex]::Matches(
    $workflow,
    '(?m)^\s*uses:\s*actions/checkout@[0-9a-f]{40}'
).Count
$fullFetchCount = [regex]::Matches($workflow, '(?m)^\s*fetch-depth:\s*0\s*$').Count
$noCredentialCount = [regex]::Matches(
    $workflow,
    '(?m)^\s*persist-credentials:\s*false\s*$'
).Count
if ($checkoutCount -eq 0 -or $fullFetchCount -ne $checkoutCount) {
    $failures.Add('Every CI checkout must fetch complete history with fetch-depth: 0.')
}
if ($noCredentialCount -ne $checkoutCount) {
    $failures.Add('Every CI checkout must disable persisted Git credentials.')
}

$requiredWorkflowText = @(
    'gitleaks/gitleaks-action@e0c47f4f8be36e29cdc102c57e68cb5cbf0e8d1e',
    'GITLEAKS_CONFIG: .gitleaks.toml',
    'GITLEAKS_ENABLE_COMMENTS: "false"',
    'GITLEAKS_ENABLE_SUMMARY: "false"',
    'GITLEAKS_ENABLE_UPLOAD_ARTIFACT: "false"',
    'GITLEAKS_VERSION: 8.30.1',
    "PUBLIC_HISTORY_REF: `${{ github.event_name == 'push' && 'refs/heads/main' || 'HEAD' }}"
)
foreach ($required in $requiredWorkflowText) {
    if (-not $workflow.Contains($required, [System.StringComparison]::Ordinal)) {
        $failures.Add("CI security contract is missing: $required")
    }
}

if ($config -notmatch '(?m)^minVersion\s*=\s*"8\.30\.1"\s*$') {
    $failures.Add('Gitleaks minimum version is not pinned to 8.30.1.')
}
if ($config -notmatch '(?ms)^\[extend\]\s*\r?\nuseDefault\s*=\s*true\s*$') {
    $failures.Add('Gitleaks configuration does not extend the complete default rule set.')
}

if ($failures.Count -gt 0) {
    $details = ($failures | Sort-Object -Unique | ForEach-Object { " - $_" }) -join "`n"
    throw "CI security contract failed:`n$details"
}

Write-Host 'CI SECURITY CONTRACT: PASS' -ForegroundColor Green
