[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$auditScript = Join-Path $PSScriptRoot 'Test-PublicTree.ps1'
$temporaryBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\', '/')
$testRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $temporaryBase "latentdeck-public-tree-test-$([guid]::NewGuid().ToString('N'))")
)
if (-not $testRoot.StartsWith(
    $temporaryBase + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw 'Temporary test root escaped the system temporary directory.'
}

$utf8 = [System.Text.UTF8Encoding]::new($false)
$junctionPath = $null

function Invoke-Git {
    param(
        [Parameter(Mandatory)][string]$Repository,
        [Parameter(Mandatory)][string[]]$Arguments
    )

    $output = @(& git -C $Repository @Arguments)
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE."
    }
    return $output
}

function New-TestRepository {
    param([Parameter(Mandatory)][string]$Name)

    $repository = Join-Path $testRoot $Name
    [System.IO.Directory]::CreateDirectory($repository) | Out-Null
    Invoke-Git -Repository $repository -Arguments @('init', '-b', 'main') | Out-Null
    Invoke-Git -Repository $repository -Arguments @('config', 'user.name', 'LatentDeck Test') | Out-Null
    Invoke-Git -Repository $repository -Arguments @('config', 'user.email', 'test@example.invalid') | Out-Null
    Invoke-Git -Repository $repository -Arguments @('config', 'core.autocrlf', 'false') | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $repository 'README.md'), "# Safe`n", $utf8)
    Invoke-Git -Repository $repository -Arguments @('add', '--', 'README.md') | Out-Null
    Invoke-Git -Repository $repository -Arguments @('commit', '-m', 'add safe file') | Out-Null
    return $repository
}

function Invoke-Audit {
    param([Parameter(Mandatory)][string]$Repository)

    $output = @(
        & pwsh -NoProfile -File $auditScript -RepositoryRoot $Repository 2>&1 |
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
    $safeResult = Invoke-Audit -Repository $safe
    if ($safeResult.ExitCode -ne 0 -or $safeResult.Output -notmatch 'PUBLIC TREE AUDIT: PASS') {
        throw "Safe public tree was rejected.`n$($safeResult.Output)"
    }

    $gitSymlink = New-TestRepository -Name 'git-symlink'
    $linkPath = Join-Path $gitSymlink 'linked.md'
    [System.IO.File]::WriteAllText($linkPath, "README.md`n", $utf8)
    $linkBlob = @(Invoke-Git -Repository $gitSymlink -Arguments @('hash-object', '-w', '--', 'linked.md'))[0]
    Invoke-Git `
        -Repository $gitSymlink `
        -Arguments @('update-index', '--add', '--cacheinfo', "120000,$linkBlob,linked.md") |
        Out-Null
    $gitSymlinkResult = Invoke-Audit -Repository $gitSymlink
    if (
        $gitSymlinkResult.ExitCode -eq 0 -or
        $gitSymlinkResult.Output -notmatch 'Tracked Git symlink mode is forbidden: linked.md'
    ) {
        throw "Tracked Git symlink mode was not rejected.`n$($gitSymlinkResult.Output)"
    }

    if ($IsWindows) {
        $reparseRepository = New-TestRepository -Name 'reparse'
        $reparseTarget = Join-Path $testRoot 'reparse-target'
        [System.IO.Directory]::CreateDirectory($reparseTarget) | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $reparseTarget 'outside.md'), "# Outside`n", $utf8)
        $junctionPath = Join-Path $reparseRepository 'linked-content'
        New-Item -ItemType Junction -Path $junctionPath -Target $reparseTarget | Out-Null

        $reparseResult = Invoke-Audit -Repository $reparseRepository
        if (
            $reparseResult.ExitCode -eq 0 -or
            $reparseResult.Output -notmatch 'Reparse point is forbidden in candidate path:'
        ) {
            throw "A candidate path through a directory junction was not rejected.`n$($reparseResult.Output)"
        }
    }
}
finally {
    if ($null -ne $junctionPath -and (Test-Path -LiteralPath $junctionPath)) {
        Remove-Item -LiteralPath $junctionPath -Force
    }
    if (Test-Path -LiteralPath $testRoot) {
        Get-ChildItem -LiteralPath $testRoot -Force -Recurse -ErrorAction SilentlyContinue |
            ForEach-Object { $_.Attributes = [System.IO.FileAttributes]::Normal }
        Remove-Item -LiteralPath $testRoot -Force -Recurse
    }
}

Write-Host 'PUBLIC TREE CONTRACT: PASS' -ForegroundColor Green
