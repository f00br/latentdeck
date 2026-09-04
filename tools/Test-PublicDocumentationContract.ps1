[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$auditScript = Join-Path $PSScriptRoot 'Test-PublicDocumentation.ps1'
$temporaryBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\', '/')
$testRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $temporaryBase "latentdeck-public-docs-test-$([guid]::NewGuid().ToString('N'))")
)
if (-not $testRoot.StartsWith(
    $temporaryBase + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw 'Temporary test root escaped the system temporary directory.'
}

$utf8 = [System.Text.UTF8Encoding]::new($false)

function Write-TestFile {
    param(
        [Parameter(Mandatory)][string]$RelativePath,
        [Parameter(Mandatory)][string]$Content
    )

    $path = Join-Path $testRoot $RelativePath
    [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($path)) | Out-Null
    [System.IO.File]::WriteAllText($path, $Content, $utf8)
}

function Invoke-Audit {
    $output = @(
        & pwsh -NoProfile -File $auditScript -RepositoryRoot $testRoot 2>&1 |
            ForEach-Object { $_.ToString() }
    )
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = $output -join "`n"
    }
}

$safeReadme = @'
# Documentation contract

[Same-file anchor](#same-file-anchor)
[First duplicate](#repeated-heading)
[Second duplicate](#repeated-heading-1)
[Relative-file anchor](docs/GUIDE.md#target-heading)
[External fragment is not fetched](https://example.invalid/page#not-checked)

```markdown
[A link inside a code fence](missing.md#not-a-link)
```

## Same-file anchor

## Repeated heading

## Repeated heading
'@ + "`n"

$safeGuide = @'
# Guide

## Target heading
'@ + "`n"

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null
    & git -C $testRoot init -b main | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not initialize the documentation contract repository.'
    }

    Write-TestFile -RelativePath 'README.md' -Content $safeReadme
    Write-TestFile -RelativePath 'docs/GUIDE.md' -Content $safeGuide

    $safeResult = Invoke-Audit
    if ($safeResult.ExitCode -ne 0 -or $safeResult.Output -notmatch 'PUBLIC DOCUMENTATION: PASS') {
        throw "Valid same-file, relative-file, and duplicate anchors were rejected.`n$($safeResult.Output)"
    }

    Write-TestFile `
        -RelativePath 'README.md' `
        -Content ($safeReadme.Replace('#same-file-anchor)', '#missing-same-file-anchor)'))
    $sameFileResult = Invoke-Audit
    if (
        $sameFileResult.ExitCode -eq 0 -or
        $sameFileResult.Output -notmatch "missing local Markdown fragment '#missing-same-file-anchor'"
    ) {
        throw "A missing same-file fragment was not rejected.`n$($sameFileResult.Output)"
    }

    Write-TestFile `
        -RelativePath 'README.md' `
        -Content ($safeReadme.Replace('#target-heading)', '#missing-relative-heading)'))
    $relativeFileResult = Invoke-Audit
    if (
        $relativeFileResult.ExitCode -eq 0 -or
        $relativeFileResult.Output -notmatch "missing local Markdown fragment '#missing-relative-heading'"
    ) {
        throw "A missing relative-file fragment was not rejected.`n$($relativeFileResult.Output)"
    }

    Write-TestFile `
        -RelativePath 'README.md' `
        -Content ($safeReadme.Replace('docs/GUIDE.md#target-heading', 'docs/guide.md#target-heading'))
    $pathCaseResult = Invoke-Audit
    if (
        $pathCaseResult.ExitCode -eq 0 -or
        $pathCaseResult.Output -notmatch 'incorrect case'
    ) {
        throw "Incorrect local-link path casing was not rejected.`n$($pathCaseResult.Output)"
    }

    Write-TestFile -RelativePath 'README.md' -Content $safeReadme
    Write-TestFile `
        -RelativePath 'docs/guides/WINDOWS_INSTALL.md' `
        -Content "# Windows install`n`npip install latentdeck-cartridge safetensors`n"
    $manualRecorderResult = Invoke-Audit
    if (
        $manualRecorderResult.ExitCode -eq 0 -or
        $manualRecorderResult.Output -notmatch 'obsolete manual Recorder dependency installation'
    ) {
        throw (
            "An obsolete manual Recorder dependency installation was not rejected.`n" +
            $manualRecorderResult.Output
        )
    }
}
finally {
    if (Test-Path -LiteralPath $testRoot) {
        Get-ChildItem -LiteralPath $testRoot -Force -Recurse -ErrorAction SilentlyContinue |
            ForEach-Object { $_.Attributes = [System.IO.FileAttributes]::Normal }
        Remove-Item -LiteralPath $testRoot -Force -Recurse
    }
}

Write-Host 'PUBLIC DOCUMENTATION CONTRACT: PASS' -ForegroundColor Green
