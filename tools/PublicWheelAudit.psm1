Set-StrictMode -Version Latest

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

    $forbiddenRoots = @(
        foreach ($root in $ForbiddenPathRoot) {
            if ([string]::IsNullOrWhiteSpace($root)) {
                throw "$Context received an empty forbidden path root."
            }
            [System.IO.Path]::GetFullPath($root).Replace('\', '/').TrimEnd('/')
        }
    )
    $expectedTimestamp = [datetime]::new(1980, 1, 1, 0, 0, 0)
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $archive = [System.IO.Compression.ZipFile]::OpenRead($resolved)
    try {
        if ($archive.Entries.Count -eq 0 -or $archive.Entries.Count -gt 4096) {
            throw "$Context has an empty or unbounded ZIP entry set."
        }
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

            $baseName = [System.IO.Path]::GetFileName($name)
            $isTextMetadata = (
                $name -match '(?i)\.(?:cfg|csv|ini|json|md|pth|py|toml|txt)$' -or
                $baseName -cin @('METADATA', 'RECORD', 'WHEEL', 'entry_points.txt')
            )
            if (-not $isTextMetadata -or $name.EndsWith('/')) {
                continue
            }
            if ($entry.Length -gt 16MB) {
                throw "$Context contains oversized text metadata: $name"
            }

            $entryStream = $entry.Open()
            try {
                $reader = [System.IO.StreamReader]::new(
                    $entryStream,
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
                $entryStream.Dispose()
            }

            foreach ($root in $forbiddenRoots) {
                if ($text.IndexOf($root, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
                    throw "$Context contains a machine-local build path in ${name}: $root"
                }
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
