[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'PublicWheelAudit.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repositoryRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".public-wheel-audit-$([guid]::NewGuid().ToString('N'))"

function Assert-Throws {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$ExpectedText
    )

    try {
        & $Action
    }
    catch {
        if ($_.Exception.Message -notlike "*$ExpectedText*") {
            throw "Unexpected failure: $($_.Exception.Message)"
        }
        return
    }
    throw "Expected failure containing '$ExpectedText'."
}

function New-TestWheel {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][hashtable]$Entries,
        [datetime]$Timestamp = [datetime]::new(1980, 1, 1, 0, 0, 0)
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [System.IO.FileStream]::new(
        $Path,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false
        )
        try {
            foreach ($name in @($Entries.Keys | Sort-Object -CaseSensitive)) {
                $entry = $archive.CreateEntry($name)
                $entry.LastWriteTime = [DateTimeOffset]::new(
                    $Timestamp,
                    [TimeSpan]::Zero
                )
                $entryStream = $entry.Open()
                try {
                    $value = $Entries[$name]
                    if ($value -is [byte[]]) {
                        $entryStream.Write($value, 0, $value.Length)
                    } else {
                        $writer = [System.IO.StreamWriter]::new(
                            $entryStream,
                            [System.Text.UTF8Encoding]::new($false),
                            4096,
                            $true
                        )
                        try {
                            $writer.Write([string]$value)
                        }
                        finally {
                            $writer.Dispose()
                        }
                    }
                }
                finally {
                    $entryStream.Dispose()
                }
            }
        }
        finally {
            $archive.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null
    $valid = Join-Path $testRoot 'valid-1.0.0-py3-none-any.whl'
    New-TestWheel -Path $valid -Entries @{
        'valid/__init__.py' = "VALUE = 1`n"
        'valid-1.0.0.dist-info/METADATA' = "Name: valid`nVersion: 1.0.0`n"
        'valid-1.0.0.dist-info/RECORD' = "valid/__init__.py,,`n"
    }
    $result = Assert-PublicProjectWheel `
        -Path $valid `
        -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
        -RequireDeterministicTimestamps
    if ($result.EntryCount -ne 3 -or $result.Sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'Valid project-wheel audit result is incomplete.'
    }

    $localPath = Join-Path $testRoot 'local-path-1.0.0-py3-none-any.whl'
    New-TestWheel -Path $localPath -Entries @{
        'local_path-1.0.0.dist-info/METADATA' = "Source: $($repositoryRoot.Replace('\', '/'))`n"
    }
    Assert-Throws -ExpectedText 'machine-local build path' -Action {
        Assert-PublicProjectWheel `
            -Path $localPath `
            -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
            -RequireDeterministicTimestamps | Out-Null
    }

    $binaryPath = Join-Path $testRoot 'binary-path-1.0.0-py3-none-any.whl'
    New-TestWheel -Path $binaryPath -Entries @{
        'binary_path/_native.pyd' = [System.Text.Encoding]::UTF8.GetBytes(
            "binary-prefix $repositoryRoot\crates\cartridge binary-suffix"
        )
    }
    Assert-Throws -ExpectedText 'machine-local build path' -Action {
        Assert-PublicProjectWheel `
            -Path $binaryPath `
            -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
            -RequireDeterministicTimestamps | Out-Null
    }

    $binaryUtf16Path = Join-Path $testRoot 'binary-utf16-path-1.0.0-py3-none-any.whl'
    New-TestWheel -Path $binaryUtf16Path -Entries @{
        'binary_utf16_path/_native.pyd' = [System.Text.Encoding]::Unicode.GetBytes(
            "binary-prefix $repositoryRoot\crates\cartridge binary-suffix"
        )
    }
    Assert-Throws -ExpectedText 'machine-local build path' -Action {
        Assert-PublicProjectWheel `
            -Path $binaryUtf16Path `
            -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
            -RequireDeterministicTimestamps | Out-Null
    }

    $fileUri = Join-Path $testRoot 'file-uri-1.0.0-py3-none-any.whl'
    New-TestWheel -Path $fileUri -Entries @{
        'file_uri-1.0.0.dist-info/METADATA' = "Source: path+file:///X:/private/source`n"
    }
    Assert-Throws -ExpectedText 'machine-local file URI' -Action {
        Assert-PublicProjectWheel `
            -Path $fileUri `
            -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
            -RequireDeterministicTimestamps | Out-Null
    }

    $timestamp = Join-Path $testRoot 'timestamp-1.0.0-py3-none-any.whl'
    New-TestWheel `
        -Path $timestamp `
        -Entries @{'timestamp-1.0.0.dist-info/METADATA' = "Name: timestamp`n"} `
        -Timestamp ([datetime]::new(2026, 1, 1, 0, 0, 0))
    Assert-Throws -ExpectedText 'non-deterministic ZIP timestamp' -Action {
        Assert-PublicProjectWheel `
            -Path $timestamp `
            -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
            -RequireDeterministicTimestamps | Out-Null
    }

    $embeddedSbom = Join-Path $testRoot 'sbom-1.0.0-py3-none-any.whl'
    New-TestWheel -Path $embeddedSbom -Entries @{
        'sbom-1.0.0.dist-info/METADATA' = "Name: sbom`n"
        'sbom-1.0.0.dist-info/sboms/build.cyclonedx.json' = '{}'
    }
    Assert-Throws -ExpectedText 'unexpected embedded build SBOM' -Action {
        Assert-PublicProjectWheel `
            -Path $embeddedSbom `
            -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
            -RequireDeterministicTimestamps `
            -ForbidEmbeddedSbom | Out-Null
    }

    Write-Host 'PUBLIC PROJECT-WHEEL AUDIT CONTRACT: PASS' -ForegroundColor Green
}
finally {
    $expectedPrefix = [System.IO.Path]::GetFullPath($artifactsRoot).TrimEnd('\') + '\.public-wheel-audit-'
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        if (-not $resolvedTestRoot.StartsWith(
            $expectedPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Refusing to remove an unexpected wheel-audit path: $resolvedTestRoot"
        }
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}
