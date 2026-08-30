[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".diagnostic-test-$([guid]::NewGuid().ToString('N'))"
$testFullPath = [System.IO.Path]::GetFullPath($testRoot)
$artifactsFullPath = [System.IO.Path]::GetFullPath($artifactsRoot).TrimEnd('\', '/')
if (-not $testFullPath.StartsWith(
    $artifactsFullPath + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
) -or -not ([System.IO.Path]::GetFileName($testFullPath)).StartsWith(
    '.diagnostic-test-',
    [System.StringComparison]::Ordinal
)) {
    throw 'Diagnostic test temporary directory failed its containment check.'
}

function Write-Utf8Text {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Content
    )

    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $Path)) | Out-Null
    [System.IO.File]::WriteAllText(
        $Path,
        $Content,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Assert-Throws {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Action,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $threw = $false
    try {
        & $Action
    } catch {
        $threw = $true
    }
    if (-not $threw) {
        throw "Expected failure did not occur: $Context"
    }
}

try {
    $deckRoot = Join-Path $testRoot 'deck'
    $playerRoot = Join-Path $testRoot 'player'
    $workerRoot = Join-Path $testRoot 'worker'
    foreach ($directory in @($deckRoot, $playerRoot, $workerRoot)) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }

    Write-Utf8Text -Path (Join-Path $deckRoot 'latentdeck-1.jsonl') -Content @'
{"schema_version":1,"timestamp_unix_ms":1777777000000,"level":"info","event":"app.started"}
{"schema_version":1,"timestamp_unix_ms":1777777000100,"level":"error","event":"app.command_failed","code":"library.import_failed","message":"C:\\Users\\private\\secret-token\\private.lc"}
{"schema_version":1,"timestamp_unix_ms":1777777000200,"level":"error","event":"C:\\Users\\private\\escape","code":"unsafe"}
not-json
'@
    Write-Utf8Text -Path (Join-Path $playerRoot 'latentplayer-1.jsonl') -Content @'
{"schema_version":2,"timestamp_unix_ms":1777777000300,"level":"warn","event":"future.schema"}
{"schema_version":1,"timestamp_unix_ms":1777777000400,"level":"warn","event":"player.command_failed","code":"player.runtime_unavailable","path":"W:\\private\\movie.mp4"}
'@
    Write-Utf8Text -Path (Join-Path $workerRoot 'worker-1.jsonl') -Content @'
{"schema_version":1,"timestamp_ns":1777777000500000000,"pid":1234,"event":"worker.control_failed","cause_code":"ring_map_failed","cause_detail":"MapViewOfFile failed at C:\\Users\\private\\payload.safetensors","detail":"secret-token"}
'@

    $oversizedPath = Join-Path $workerRoot 'worker-oversized.jsonl'
    [System.IO.File]::WriteAllBytes($oversizedPath, [byte[]]::new(2048))

    $outputPath = Join-Path $testRoot 'diagnostics.zip'
    $created = & (Join-Path $PSScriptRoot 'New-DiagnosticBundle.ps1') `
        -DeckLogRoot $deckRoot `
        -PlayerLogRoot $playerRoot `
        -WorkerLogRoot $workerRoot `
        -OutputPath $outputPath `
        -MaxFileBytes 1024 `
        -MaxInputBytes 8192
    if ([System.IO.Path]::GetFullPath([string]$created) -cne [System.IO.Path]::GetFullPath($outputPath)) {
        throw 'Diagnostic bundle returned an unexpected output path.'
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($outputPath)
    try {
        $entryNames = @($archive.Entries | ForEach-Object FullName | Sort-Object)
        if (($entryNames -join ',') -cne 'events.jsonl,manifest.json') {
            throw "Unexpected diagnostic archive layout: $($entryNames -join ', ')"
        }
        $manifestEntry = $archive.GetEntry('manifest.json')
        $eventsEntry = $archive.GetEntry('events.jsonl')
        $manifestReader = [System.IO.StreamReader]::new($manifestEntry.Open())
        $eventsReader = [System.IO.StreamReader]::new($eventsEntry.Open())
        try {
            $manifestText = $manifestReader.ReadToEnd()
            $eventsText = $eventsReader.ReadToEnd()
        } finally {
            $manifestReader.Dispose()
            $eventsReader.Dispose()
        }
    } finally {
        $archive.Dispose()
    }

    $manifest = $manifestText | ConvertFrom-Json
    if ($manifest.schema_version -ne 1 -or
        $manifest.accepted_event_count -ne 4 -or
        $manifest.dropped_record_count -ne 3 -or
        $manifest.skipped_file_count -ne 1) {
        throw 'Diagnostic manifest counters do not match the bounded fixture.'
    }

    $events = @(
        $eventsText -split "`r?`n" |
            Where-Object { $_.Length -gt 0 } |
            ForEach-Object { $_ | ConvertFrom-Json }
    )
    if ($events.Count -ne 4 -or
        @($events | Where-Object source -eq 'deck').Count -ne 2 -or
        @($events | Where-Object source -eq 'player').Count -ne 1 -or
        @($events | Where-Object source -eq 'worker').Count -ne 1) {
        throw 'Diagnostic bundle did not preserve the expected sanitized event set.'
    }

    $combined = $manifestText + "`n" + $eventsText
    foreach ($forbidden in @(
        'C:\\Users',
        'W:\\private',
        'secret-token',
        'private.lc',
        'movie.mp4',
        'payload.safetensors',
        'cause_detail',
        '"detail"',
        '"message"',
        '"path"',
        '"pid"'
    )) {
        if ($combined.Contains($forbidden, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Diagnostic bundle leaked forbidden content: $forbidden"
        }
    }

    Assert-Throws -Context 'existing output must not be overwritten' -Action {
        & (Join-Path $PSScriptRoot 'New-DiagnosticBundle.ps1') `
            -DeckLogRoot $deckRoot `
            -PlayerLogRoot $playerRoot `
            -WorkerLogRoot $workerRoot `
            -OutputPath $outputPath | Out-Null
    }

    if (@(Get-ChildItem -LiteralPath $testRoot -Force | Where-Object {
        $_.Name -like '.diagnostic-staging-*' -or $_.Name -like '*.partial-*'
    }).Count -ne 0) {
        throw 'Diagnostic bundle left a temporary artifact behind.'
    }

    Write-Host 'DIAGNOSTIC BUNDLE CONTRACT: PASS' -ForegroundColor Green
    Write-Host 'Verified: bounded inputs, strict field allowlist, path and secret removal, exact archive layout, atomic no-overwrite output.'
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
