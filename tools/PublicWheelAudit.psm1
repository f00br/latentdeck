Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'PublicNativeBuild.psm1')

function Assert-PublicProjectWheel {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string[]]$ForbiddenPathRoot,

        [string]$Context = 'Project wheel',

        [switch]$RequireDeterministicTimestamps,

        [switch]$ForbidEmbeddedSbom
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 128MB -or
        $item.Extension -cne '.whl') {
        throw "$Context is not a bounded regular wheel: $Path"
    }

    $expectedTimestamp = [datetime]::new(1980, 1, 1, 0, 0, 0)
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $archive = [System.IO.Compression.ZipFile]::OpenRead($resolved)
    try {
        if ($archive.Entries.Count -eq 0 -or $archive.Entries.Count -gt 4096) {
            throw "$Context has an empty or unbounded ZIP entry set."
        }
        [int64]$totalExpandedBytes = 0
        foreach ($entry in $archive.Entries) {
            $name = [string]$entry.FullName
            if ([string]::IsNullOrWhiteSpace($name) -or $name.Length -gt 4096 -or
                $name.Contains('\') -or $name.StartsWith('/') -or
                $name -match '(^|/)\.\.($|/)' -or -not $seen.Add($name)) {
                throw "$Context contains an unsafe or duplicate ZIP entry: $name"
            }
            if ($RequireDeterministicTimestamps.IsPresent -and
                $entry.LastWriteTime.DateTime -ne $expectedTimestamp) {
                throw "$Context contains a non-deterministic ZIP timestamp: $name"
            }
            if ($ForbidEmbeddedSbom.IsPresent -and $name -match '(?i)(^|/)sboms/') {
                throw "$Context contains an unexpected embedded build SBOM: $name"
            }

            if ($entry.Length -lt 0 -or $entry.Length -gt 128MB) {
                throw "$Context contains an oversized ZIP entry: $name"
            }
            $totalExpandedBytes += [int64]$entry.Length
            if ($totalExpandedBytes -gt 512MB) {
                throw "$Context has an unbounded expanded ZIP payload."
            }
            if ($name.EndsWith('/')) {
                if ($entry.Length -ne 0) {
                    throw "$Context contains a non-empty directory entry: $name"
                }
                continue
            }

            $entryStream = $entry.Open()
            try {
                $memory = [System.IO.MemoryStream]::new()
                try {
                    $entryStream.CopyTo($memory)
                    $entryBytes = $memory.ToArray()
                }
                finally {
                    $memory.Dispose()
                }
            }
            finally {
                $entryStream.Dispose()
            }
            Assert-PublicBytesPathHygiene `
                -Bytes $entryBytes `
                -ForbiddenPathRoot $ForbiddenPathRoot `
                -Context "$Context entry $name"

            $baseName = [System.IO.Path]::GetFileName($name)
            $isTextMetadata = (
                $name -match '(?i)\.(?:cfg|csv|ini|json|md|pth|py|toml|txt)$' -or
                $baseName -cin @('METADATA', 'RECORD', 'WHEEL', 'entry_points.txt')
            )
            if (-not $isTextMetadata) {
                continue
            }
            if ($entry.Length -gt 16MB) {
                throw "$Context contains oversized text metadata: $name"
            }

            $textStream = [System.IO.MemoryStream]::new($entryBytes, $false)
            try {
                $reader = [System.IO.StreamReader]::new(
                    $textStream,
                    [System.Text.UTF8Encoding]::new($false, $true),
                    $true,
                    4096,
                    $false
                )
                try {
                    $text = $reader.ReadToEnd()
                }
                catch [System.Text.DecoderFallbackException] {
                    throw "$Context contains non-UTF-8 text metadata: $name"
                }
                finally {
                    $reader.Dispose()
                }
            }
            finally {
                $textStream.Dispose()
            }

            if ($text -match '(?i)(?:path\+)?file:///(?:[a-z]:/|(?:home|users|tmp|private|workspace|workspaces)/)') {
                throw "$Context contains a machine-local file URI in ${name}."
            }
        }
    }
    finally {
        $archive.Dispose()
    }

    return [pscustomobject]@{
        Path = $resolved
        EntryCount = $seen.Count
        Sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

Export-ModuleMember -Function Assert-PublicProjectWheel
