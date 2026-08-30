[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$builder = Join-Path $PSScriptRoot 'New-LinkedDevCodecPack.ps1'
$temporaryParent = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testRoot = Join-Path $temporaryParent "latentdeck-linked-codec-test-$([guid]::NewGuid().ToString('N'))"

function Write-TestFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [string]$Content = 'fixture'
    )

    $parent = Split-Path -Parent $Path
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

try {
    $runtime = Join-Path $testRoot 'runtime-source'
    $pythonPackages = Join-Path $testRoot 'python-packages'
    $workerPackages = Join-Path $testRoot 'worker-packages'
    $workerModule = Join-Path $workerPackages 'latentdeck_codec_h3'
    [System.IO.Directory]::CreateDirectory($runtime) | Out-Null
    [System.IO.Directory]::CreateDirectory($pythonPackages) | Out-Null
    [System.IO.Directory]::CreateDirectory($workerModule) | Out-Null

    Write-TestFile -Path (Join-Path $runtime 'python.exe')
    Write-TestFile -Path (Join-Path $runtime 'python313.zip')
    Write-TestFile -Path (Join-Path $runtime 'python313._pth') -Content "python313.zip`n.`nimport site`n"
    Write-TestFile -Path (Join-Path $workerModule '__init__.py') -Content ''
    Write-TestFile -Path (Join-Path $workerModule 'worker.py')
    Write-TestFile -Path (Join-Path $workerModule 'd2_worker.py')

    $rejected = $false
    try {
        & $builder `
            -PythonRuntimeRoot $runtime `
            -PythonSitePackages $pythonPackages `
            -WorkerSitePackages $workerPackages `
            -OutputRoot (Join-Path $testRoot 'missing-q4-output') | Out-Null
    }
    catch {
        if ($_.Exception.Message -notmatch 'q4_worker\.py') {
            throw
        }
        $rejected = $true
    }
    if (-not $rejected) {
        throw 'Linked Codec Pack builder accepted worker packages without q4_worker.py.'
    }

    Write-TestFile -Path (Join-Path $workerModule 'q4_worker.py')
    $validOutput = Join-Path $testRoot 'valid-output'
    & $builder `
        -PythonRuntimeRoot $runtime `
        -PythonSitePackages $pythonPackages `
        -WorkerSitePackages $workerPackages `
        -OutputRoot $validOutput | Out-Null
    $packRoot = Join-Path $validOutput 'org.latentdeck.h3\0.1.0'
    $manifest = Get-Content -Raw -LiteralPath (Join-Path $packRoot 'codec-pack.json') |
        ConvertFrom-Json
    $expectedEntrypoints = @(
        @('-B', '-s', '-m', 'latentdeck_codec_h3.worker')
        @('-B', '-s', '-m', 'latentdeck_codec_h3.d2_worker')
        @('-B', '-s', '-m', 'latentdeck_codec_h3.q4_worker')
    )
    $actualEntrypoints = @(
        @($manifest.worker.arguments)
        @($manifest.worker.d2_arguments)
        @($manifest.worker.q4_arguments)
    )
    for ($index = 0; $index -lt $expectedEntrypoints.Count; $index++) {
        if ((@($actualEntrypoints[$index]) -join "`0") -cne
            (@($expectedEntrypoints[$index]) -join "`0")) {
            throw "Linked Codec Pack entrypoint $index does not match the Player/D2/Q4 contract."
        }
    }

    $pth = Get-Content -LiteralPath (Join-Path $packRoot 'runtime\python313._pth')
    $workerIndex = [Array]::IndexOf($pth, $workerPackages)
    $pythonIndex = [Array]::IndexOf($pth, $pythonPackages)
    if ($workerIndex -lt 0 -or $pythonIndex -lt 0 -or $workerIndex -ge $pythonIndex) {
        throw 'Linked worker packages must precede the laboratory Python packages on sys.path.'
    }
}
finally {
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    $leaf = Split-Path -Leaf $resolvedTestRoot
    if (-not $resolvedTestRoot.StartsWith($temporaryParent, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not $leaf.StartsWith('latentdeck-linked-codec-test-', [System.StringComparison]::Ordinal)) {
        throw 'Refusing to clean an unexpected linked Codec Pack test directory.'
    }
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}

Write-Host 'LINKED DEVELOPMENT CODEC PACK TEST: PASS' -ForegroundColor Green
