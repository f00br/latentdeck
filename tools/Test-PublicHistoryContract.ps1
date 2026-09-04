[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$auditScript = Join-Path $PSScriptRoot 'Test-PublicHistory.ps1'
$temporaryBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\', '/')
$testRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $temporaryBase "latentdeck-public-history-test-$([guid]::NewGuid().ToString('N'))")
)
if (-not $testRoot.StartsWith(
    $temporaryBase + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw 'Temporary test root escaped the system temporary directory.'
}

function Invoke-Git {
    param(
        [Parameter(Mandatory)]
        [string]$Repository,

        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    & git -C $Repository @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
}

function New-TestRepository {
    param([Parameter(Mandatory)][string]$Name)

    $repository = Join-Path $testRoot $Name
    [System.IO.Directory]::CreateDirectory($repository) | Out-Null
    Invoke-Git -Repository $repository -Arguments @('init', '-b', 'main')
    Invoke-Git -Repository $repository -Arguments @('config', 'user.name', 'LatentDeck Test')
    Invoke-Git -Repository $repository -Arguments @('config', 'user.email', 'test@example.invalid')
    Invoke-Git -Repository $repository -Arguments @('config', 'core.autocrlf', 'false')
    return $repository
}

function Add-Commit {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string]$RelativePath,
        [Parameter(Mandatory)][string]$Content
    )

    $path = Join-Path $Repository $RelativePath
    $parent = [System.IO.Path]::GetDirectoryName($path)
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    [System.IO.File]::WriteAllText($path, $Content, [System.Text.UTF8Encoding]::new($false))
    Invoke-Git -Repository $Repository -Arguments @('add', '--', $RelativePath)
    Invoke-Git -Repository $Repository -Arguments @('commit', '-m', "add $RelativePath")
}

function Invoke-Audit {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [string]$Ref = 'refs/heads/main'
    )

    $output = @(
        & pwsh -NoProfile -File $auditScript -RepositoryRoot $Repository -Ref $Ref 2>&1 |
            ForEach-Object { $_.ToString() }
    )
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = $output -join "`n"
    }
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null

    $safe = New-TestRepository -Name 'safe'
    Add-Commit -Repository $safe -RelativePath 'README.md' -Content "# Safe repository`n"
    $safeResult = Invoke-Audit -Repository $safe
    if ($safeResult.ExitCode -ne 0 -or $safeResult.Output -notmatch 'PUBLIC HISTORY AUDIT: PASS') {
        throw "Safe history was rejected.`n$($safeResult.Output)"
    }

    $payload = New-TestRepository -Name 'payload'
    Add-Commit -Repository $payload -RelativePath 'README.md' -Content "# Payload repository`n"
    Add-Commit -Repository $payload -RelativePath 'private/sample.lc' -Content 'not a real cartridge'
    $payloadResult = Invoke-Audit -Repository $payload
    if ($payloadResult.ExitCode -eq 0 -or $payloadResult.Output -notmatch 'Forbidden payload type') {
        throw "Forbidden payload history was not rejected.`n$($payloadResult.Output)"
    }

    $secret = New-TestRepository -Name 'secret'
    Add-Commit -Repository $secret -RelativePath 'README.md' -Content "# Secret repository`n"
    $syntheticToken = 'ghp_' + ('A' * 40)
    Add-Commit -Repository $secret -RelativePath 'removed.txt' -Content $syntheticToken
    [System.IO.File]::Delete((Join-Path $secret 'removed.txt'))
    Invoke-Git -Repository $secret -Arguments @('add', '--all')
    Invoke-Git -Repository $secret -Arguments @('commit', '-m', 'remove synthetic token fixture')
    $secretResult = Invoke-Audit -Repository $secret
    if ($secretResult.ExitCode -eq 0 -or $secretResult.Output -notmatch 'High-confidence GitHub token') {
        throw "Removed secret history was not rejected.`n$($secretResult.Output)"
    }
    if ($secretResult.Output.Contains($syntheticToken, [System.StringComparison]::Ordinal)) {
        throw 'Secret audit printed the matched token value.'
    }

    $candidate = New-TestRepository -Name 'candidate'
    Add-Commit -Repository $candidate -RelativePath 'README.md' -Content "# Candidate repository`n"
    Invoke-Git -Repository $candidate -Arguments @('switch', '--quiet', '-c', 'feature')
    Add-Commit `
        -Repository $candidate `
        -RelativePath 'private/candidate.lc' `
        -Content 'not a real cartridge'
    $candidateResult = Invoke-Audit -Repository $candidate -Ref 'HEAD'
    if ($candidateResult.ExitCode -eq 0 -or $candidateResult.Output -notmatch 'Forbidden payload type') {
        throw "The explicit pull-request candidate ref was not audited.`n$($candidateResult.Output)"
    }
}
finally {
    if (Test-Path -LiteralPath $testRoot) {
        Get-ChildItem -LiteralPath $testRoot -Force -Recurse -ErrorAction SilentlyContinue |
            ForEach-Object { $_.Attributes = [System.IO.FileAttributes]::Normal }
        Remove-Item -LiteralPath $testRoot -Force -Recurse
    }
}

Write-Host 'PUBLIC HISTORY CONTRACT: PASS' -ForegroundColor Green
