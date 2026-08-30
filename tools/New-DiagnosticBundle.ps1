[CmdletBinding()]
param(
    [string]$DeckLogRoot = (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'studio.latentdeck.deck\logs'),

    [string]$PlayerLogRoot = (Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'studio.latentdeck.player\logs'),

    [string]$WorkerLogRoot = (Join-Path ([System.IO.Path]::GetTempPath()) 'LatentDeck\worker-diagnostics'),

    [string]$OutputPath = (Join-Path $PSScriptRoot "..\artifacts\diagnostics\latentdeck-diagnostics-$((Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss')).zip"),

    [ValidateRange(512, 16777216)]
    [long]$MaxFileBytes = 8388608,

    [ValidateRange(1024, 134217728)]
    [long]$MaxInputBytes = 25165824,

    [ValidateRange(1, 128)]
    [int]$MaxInputFiles = 48,

    [ValidateRange(1, 131072)]
    [int]$MaxEvents = 65536
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$bundleSchemaVersion = 1
$maxRecordBytes = 4096
$utf8Strict = [System.Text.UTF8Encoding]::new($false, $true)
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

function Test-BoundedToken {
    param(
        [AllowNull()]
        [object]$Value,

        [Parameter(Mandatory)]
        [int]$MaxBytes
    )

    if ($Value -isnot [string] -or
        $Value.Length -eq 0 -or
        $Value.Length -gt $MaxBytes) {
        return $false
    }
    return [System.Text.RegularExpressions.Regex]::IsMatch(
        $Value,
        '\A[A-Za-z0-9._-]+\z',
        [System.Text.RegularExpressions.RegexOptions]::CultureInvariant
    )
}

function Get-JsonProperty {
    param(
        [Parameter(Mandatory)]
        [psobject]$Object,

        [Parameter(Mandatory)]
        [string]$Name
    )

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Convert-IntegralJsonNumber {
    param(
        [AllowNull()]
        [object]$Value
    )

    if ($Value -is [byte] -or
        $Value -is [sbyte] -or
        $Value -is [int16] -or
        $Value -is [uint16] -or
        $Value -is [int32] -or
        $Value -is [uint32] -or
        $Value -is [int64]) {
        return [decimal]$Value
    }
    if ($Value -is [uint64] -and $Value -le [uint64][long]::MaxValue) {
        return [decimal]$Value
    }
    return $null
}

function Convert-EventRecord {
    param(
        [Parameter(Mandatory)]
        [psobject]$Record,

        [Parameter(Mandatory)]
        [ValidateSet('deck', 'player', 'worker')]
        [string]$Source
    )

    $schema = Convert-IntegralJsonNumber (Get-JsonProperty $Record 'schema_version')
    if ($null -eq $schema -or $schema -ne 1) {
        return $null
    }

    $event = Get-JsonProperty $Record 'event'
    if (-not (Test-BoundedToken $event 64)) {
        return $null
    }

    if ($Source -eq 'worker') {
        $timestampNs = Convert-IntegralJsonNumber (Get-JsonProperty $Record 'timestamp_ns')
        if ($null -eq $timestampNs -or $timestampNs -lt 0) {
            return $null
        }
        $timestampMs = [decimal]::Truncate($timestampNs / 1000000)
        $level = if ($event.EndsWith('_failed', [System.StringComparison]::Ordinal) -or
            $event.EndsWith('.error', [System.StringComparison]::Ordinal)) {
            'error'
        } else {
            'info'
        }
    } else {
        $timestampMs = Convert-IntegralJsonNumber (Get-JsonProperty $Record 'timestamp_unix_ms')
        if ($null -eq $timestampMs -or $timestampMs -lt 0) {
            return $null
        }
        $level = Get-JsonProperty $Record 'level'
        if ($level -isnot [string] -or $level -cnotin @('info', 'warn', 'error')) {
            return $null
        }
    }

    if ($timestampMs -gt 253402300799999) {
        return $null
    }

    $safe = [ordered]@{
        schema_version = 1
        timestamp_unix_ms = [long]$timestampMs
        source = $Source
        level = $level
        event = $event
    }

    foreach ($candidateName in @('code', 'cause_code', 'error_type')) {
        $candidate = Get-JsonProperty $Record $candidateName
        if (Test-BoundedToken $candidate 128) {
            $safe.code = $candidate
            break
        }
    }
    return [pscustomobject]$safe
}

function Get-BoundedInputCandidates {
    param(
        [Parameter(Mandatory)]
        [array]$Sources,

        [Parameter(Mandatory)]
        [int]$Limit
    )

    $candidates = [System.Collections.Generic.List[object]]::new()
    $enumerationLimit = [Math]::Min(512, [Math]::Max($Limit * 4, $Limit))
    foreach ($source in $Sources) {
        if ([string]::IsNullOrWhiteSpace($source.Root) -or
            -not (Test-Path -LiteralPath $source.Root -PathType Container)) {
            continue
        }
        $rootAttributes = [System.IO.File]::GetAttributes($source.Root)
        if (($rootAttributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            continue
        }

        $seen = 0
        foreach ($path in [System.IO.Directory]::EnumerateFiles(
            [System.IO.Path]::GetFullPath($source.Root),
            $source.Pattern,
            [System.IO.SearchOption]::TopDirectoryOnly
        )) {
            if ($seen -ge $enumerationLimit) {
                break
            }
            $seen += 1
            $info = [System.IO.FileInfo]::new($path)
            $info.Refresh()
            $candidates.Add([pscustomobject]@{
                Source = $source.Name
                Path = $info.FullName
                LastWriteTimeUtc = $info.LastWriteTimeUtc
                Name = $info.Name
            })
        }
    }

    return @(
        $candidates |
            Sort-Object -Property @{ Expression = 'LastWriteTimeUtc'; Descending = $true }, @{ Expression = 'Name'; Descending = $true } |
            Select-Object -First $Limit
    )
}

function Read-BoundedFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [long]$ByteLimit
    )

    if ($ByteLimit -le 0) {
        return $null
    }
    $attributes = [System.IO.File]::GetAttributes($Path)
    if (($attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        ($attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
        return $null
    }

    $effectiveLimit = [Math]::Min($ByteLimit, $MaxFileBytes)
    $capacity = [int]($effectiveLimit + 1)
    $buffer = [byte[]]::new($capacity)
    $stream = [System.IO.FileStream]::new(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
    )
    try {
        $read = 0
        while ($read -lt $capacity) {
            $count = $stream.Read($buffer, $read, $capacity - $read)
            if ($count -eq 0) {
                break
            }
            $read += $count
        }
        if ($read -gt $effectiveLimit -or $stream.ReadByte() -ne -1) {
            return $null
        }
        if ($read -eq 0) {
            return [pscustomobject]@{ Bytes = [byte[]]::new(0); Length = 0 }
        }
        $exact = [byte[]]::new($read)
        [System.Buffer]::BlockCopy($buffer, 0, $exact, 0, $read)
        return [pscustomobject]@{ Bytes = $exact; Length = $read }
    } finally {
        $stream.Dispose()
    }
}

$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputParent = [System.IO.Path]::GetDirectoryName($outputFullPath)
if ([string]::IsNullOrWhiteSpace($outputParent)) {
    throw 'Diagnostic output must have a parent directory.'
}
[System.IO.Directory]::CreateDirectory($outputParent) | Out-Null
if (Test-Path -LiteralPath $outputFullPath) {
    throw 'Diagnostic output already exists; choose a new path.'
}

$operationId = [guid]::NewGuid().ToString('N')
$stagingPath = Join-Path $outputParent ".diagnostic-staging-$operationId"
$partialPath = "$outputFullPath.partial-$operationId"
foreach ($ownedPath in @($stagingPath, $partialPath)) {
    $ownedFullPath = [System.IO.Path]::GetFullPath($ownedPath)
    if ([System.IO.Path]::GetDirectoryName($ownedFullPath) -cne $outputParent) {
        throw 'Diagnostic temporary path failed its parent containment check.'
    }
}
if (-not [System.IO.Path]::GetFileName($stagingPath).StartsWith(
    '.diagnostic-staging-',
    [System.StringComparison]::Ordinal
) -or -not [System.IO.Path]::GetFileName($partialPath).Contains(
    '.partial-',
    [System.StringComparison]::Ordinal
)) {
    throw 'Diagnostic temporary path failed its ownership check.'
}

$acceptedEventCount = 0
$droppedRecordCount = 0
$processedFileCount = 0
$skippedFileCount = 0
$inputBytes = 0L
$includedSources = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)

try {
    [System.IO.Directory]::CreateDirectory($stagingPath) | Out-Null
    $eventsPath = Join-Path $stagingPath 'events.jsonl'
    $writer = [System.IO.StreamWriter]::new($eventsPath, $false, $utf8NoBom)
    try {
        $sources = @(
            [pscustomobject]@{ Name = 'deck'; Root = $DeckLogRoot; Pattern = 'latentdeck-*.jsonl' },
            [pscustomobject]@{ Name = 'player'; Root = $PlayerLogRoot; Pattern = 'latentplayer-*.jsonl' },
            [pscustomobject]@{ Name = 'worker'; Root = $WorkerLogRoot; Pattern = 'worker-*.jsonl' }
        )
        $candidates = Get-BoundedInputCandidates -Sources $sources -Limit $MaxInputFiles
        foreach ($candidate in $candidates) {
            $remainingBytes = $MaxInputBytes - $inputBytes
            $bounded = $null
            try {
                $bounded = Read-BoundedFile -Path $candidate.Path -ByteLimit $remainingBytes
            } catch {
                $bounded = $null
            }
            if ($null -eq $bounded) {
                $skippedFileCount += 1
                continue
            }

            $processedFileCount += 1
            $inputBytes += [long]$bounded.Length
            $text = $null
            try {
                $text = $utf8Strict.GetString($bounded.Bytes)
            } catch {
                $droppedRecordCount += 1
                continue
            }

            $reader = [System.IO.StringReader]::new($text)
            try {
                while ($true) {
                    $line = $reader.ReadLine()
                    if ($null -eq $line) {
                        break
                    }
                    if ([string]::IsNullOrWhiteSpace($line)) {
                        continue
                    }
                    if ($utf8NoBom.GetByteCount($line) -gt $maxRecordBytes -or
                        $acceptedEventCount -ge $MaxEvents) {
                        $droppedRecordCount += 1
                        continue
                    }

                    $parsed = $null
                    try {
                        $parsed = $line | ConvertFrom-Json -Depth 8
                    } catch {
                        $droppedRecordCount += 1
                        continue
                    }
                    if ($parsed -isnot [pscustomobject]) {
                        $droppedRecordCount += 1
                        continue
                    }
                    $safe = Convert-EventRecord -Record $parsed -Source $candidate.Source
                    if ($null -eq $safe) {
                        $droppedRecordCount += 1
                        continue
                    }

                    $writer.WriteLine(($safe | ConvertTo-Json -Compress -Depth 4))
                    $acceptedEventCount += 1
                    $null = $includedSources.Add($candidate.Source)
                }
            } finally {
                $reader.Dispose()
            }
        }
    } finally {
        $writer.Dispose()
    }

    $manifest = [ordered]@{
        schema_version = $bundleSchemaVersion
        format = 'latentdeck-diagnostic-bundle'
        format_version = '0.1'
        generated_utc = (Get-Date).ToUniversalTime().ToString('o')
        accepted_event_count = $acceptedEventCount
        dropped_record_count = $droppedRecordCount
        processed_file_count = $processedFileCount
        skipped_file_count = $skippedFileCount
        input_byte_count = $inputBytes
        included_sources = @($includedSources | Sort-Object)
        privacy = [ordered]@{
            raw_logs_included = $false
            arbitrary_text_included = $false
            absolute_paths_included = $false
            database_included = $false
            cartridge_payloads_included = $false
            model_assets_included = $false
        }
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $stagingPath 'manifest.json'),
        ($manifest | ConvertTo-Json -Depth 8),
        $utf8NoBom
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::Open(
        $partialPath,
        [System.IO.Compression.ZipArchiveMode]::Create
    )
    try {
        foreach ($name in @('manifest.json', 'events.jsonl')) {
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive,
                (Join-Path $stagingPath $name),
                $name,
                [System.IO.Compression.CompressionLevel]::Optimal
            ) | Out-Null
        }
    } finally {
        $archive.Dispose()
    }

    $verification = [System.IO.Compression.ZipFile]::OpenRead($partialPath)
    try {
        $names = @($verification.Entries | ForEach-Object FullName | Sort-Object)
        if (($names -join ',') -cne 'events.jsonl,manifest.json' -or
            @($verification.Entries | Where-Object {
                $_.FullName.Contains('/') -or $_.FullName.Contains('\')
            }).Count -ne 0) {
            throw 'Diagnostic archive layout verification failed.'
        }
    } finally {
        $verification.Dispose()
    }

    [System.IO.File]::Move($partialPath, $outputFullPath, $false)
    Write-Output $outputFullPath
} finally {
    if (Test-Path -LiteralPath $partialPath) {
        Remove-Item -LiteralPath $partialPath -Force
    }
    if (Test-Path -LiteralPath $stagingPath) {
        Remove-Item -LiteralPath $stagingPath -Recurse -Force
    }
}
