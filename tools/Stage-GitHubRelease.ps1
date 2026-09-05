[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ApplicationArtifactDirectory,

    [Parameter(Mandatory)]
    [string]$CodecArtifactDirectory,

    [Parameter(Mandatory)]
    [string]$DeveloperKitArtifactDirectory,

    [Parameter(Mandatory)]
    [string]$ComfyRecorderArtifactDirectory,

    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts')).TrimEnd('\', '/')
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$maximumGitHubAssetBytes = [int64]2GB

function Assert-ArtifactPath {
    param(
        [Parameter(Mandatory)][string]$Path,
        [switch]$AllowArtifactsRoot,
        [switch]$RequireExistingDirectory
    )

    $candidate = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $isArtifactsRoot = $candidate.Equals(
        $artifactsRoot,
        [System.StringComparison]::OrdinalIgnoreCase
    )
    if ((-not $AllowArtifactsRoot -or -not $isArtifactsRoot) -and
        -not $candidate.StartsWith(
        $artifactsRoot + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Release artifact path is outside the ignored artifacts root: $candidate"
    }
    $cursor = $candidate
    while ($cursor.StartsWith(
        $artifactsRoot,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Release artifact path contains a reparse-point directory: $cursor"
            }
            if ($cursor -ceq $candidate -and $RequireExistingDirectory -and -not $item.PSIsContainer) {
                throw "Release artifact input is not a directory: $candidate"
            }
        } elseif ($cursor -ceq $candidate -and $RequireExistingDirectory) {
            throw "Release artifact input directory does not exist: $candidate"
        }
        if ($cursor.Equals($artifactsRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
            break
        }
        $parent = Split-Path -Parent $cursor
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent -ceq $cursor) {
            break
        }
        $cursor = $parent.TrimEnd('\', '/')
    }
    return $candidate
}

function Resolve-SafeRelativeFile {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$RelativePath
    )

    $canonical = $RelativePath.Replace('\', '/')
    $segments = @($canonical.Split('/'))
    if ([string]::IsNullOrWhiteSpace($canonical) -or
        [System.IO.Path]::IsPathRooted($canonical) -or
        $segments.Count -eq 0 -or
        @($segments | Where-Object { $_ -ceq '' -or $_ -ceq '.' -or $_ -ceq '..' }).Count -gt 0) {
        throw "Release receipt path is unsafe: $RelativePath"
    }
    $rootPath = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $candidate = [System.IO.Path]::GetFullPath(
        (Join-Path $rootPath $canonical.Replace('/', '\'))
    )
    $rootPrefix = $rootPath + [System.IO.Path]::DirectorySeparatorChar
    if (-not $candidate.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Release receipt path escapes its artifact root: $RelativePath"
    }
    $item = Get-Item -LiteralPath $candidate -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Release receipt path is not a regular file: $RelativePath"
    }
    return $item
}

function Read-JsonFile {
    param([Parameter(Mandatory)][string]$Path)

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or $item.Length -gt 16MB) {
        throw "Release receipt must be a bounded regular file: $Path"
    }
    $bytes = [System.IO.File]::ReadAllBytes($item.FullName)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "Release receipt is not strict UTF-8: $Path"
    }
    try {
        $value = $text | ConvertFrom-Json -Depth 100
    } catch {
        throw "Release receipt is not valid JSON: $Path"
    }
    return [pscustomobject]@{
        Value = $value
        Integrity = [pscustomobject]@{
            Path = $item.FullName
            ByteLength = [int64]$bytes.Length
            Sha256 = [System.Convert]::ToHexString(
                [System.Security.Cryptography.SHA256]::HashData($bytes)
            ).ToLowerInvariant()
        }
    }
}

function Assert-ExactJsonProperties {
    param(
        [Parameter(Mandatory)][object]$Value,
        [Parameter(Mandatory)][string[]]$Expected,
        [Parameter(Mandatory)][string]$Context
    )

    if ($null -eq $Value -or $null -eq $Value.PSObject) {
        throw "$Context must be a JSON object."
    }
    $actual = @(
        $Value.PSObject.Properties |
            ForEach-Object { [string]$_.Name } |
            Sort-Object
    )
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "`0") -cne ($expectedSorted -join "`0")) {
        throw "$Context does not have the exact supported property set."
    }
}

function Assert-JsonBoolean {
    param(
        [Parameter(Mandatory)][AllowNull()][object]$Value,
        [Parameter(Mandatory)][string]$Context
    )

    if ($Value -isnot [bool]) {
        throw "$Context must be a JSON Boolean."
    }
}

function Assert-JsonInteger {
    param(
        [Parameter(Mandatory)][AllowNull()][object]$Value,
        [Parameter(Mandatory)][string]$Context,
        [int64]$Minimum = [int64]::MinValue,
        [int64]$Maximum = [int64]::MaxValue
    )

    if ($Value -isnot [int64] -or [int64]$Value -lt $Minimum -or
        [int64]$Value -gt $Maximum) {
        throw "$Context must be a bounded JSON integer."
    }
}

function Assert-JsonArray {
    param(
        [Parameter(Mandatory)][AllowNull()][object]$Value,
        [Parameter(Mandatory)][string]$Context
    )

    if ($Value -isnot [System.Array]) {
        throw "$Context must be a JSON array."
    }
}

function Assert-JsonString {
    param(
        [Parameter(Mandatory)][AllowNull()][object]$Value,
        [Parameter(Mandatory)][string]$Context,
        [int]$MinimumLength = 1,
        [int]$MaximumLength = 4096
    )

    if ($Value -isnot [string] -or
        $Value.Length -lt $MinimumLength -or
        $Value.Length -gt $MaximumLength) {
        throw "$Context must be a bounded JSON string."
    }
}

function Assert-JsonStringProperties {
    param(
        [Parameter(Mandatory)][object]$Value,
        [Parameter(Mandatory)][string[]]$Properties,
        [Parameter(Mandatory)][string]$Context
    )

    foreach ($propertyName in $Properties) {
        $property = $Value.PSObject.Properties[$propertyName]
        if ($null -eq $property) {
            throw "$Context is missing string property '$propertyName'."
        }
        Assert-JsonString `
            -Value $property.Value `
            -Context "$Context.$propertyName"
    }
}

function Test-OrdinalStringInSet {
    param(
        [Parameter(Mandatory)][string]$Value,
        [Parameter(Mandatory)][string[]]$Expected
    )

    foreach ($candidate in $Expected) {
        if ([string]::Equals($Value, $candidate, [System.StringComparison]::Ordinal)) {
            return $true
        }
    }
    return $false
}

function Test-ComponentHasUsableLicense {
    param([Parameter(Mandatory)][object]$Component)

    $licenseProperty = $Component.PSObject.Properties['licenses']
    if ($null -eq $licenseProperty -or @($licenseProperty.Value).Count -eq 0) {
        return $false
    }
    return @(
        $licenseProperty.Value |
            Where-Object {
                ($null -ne $_.PSObject.Properties['expression'] -and
                 -not [string]::IsNullOrWhiteSpace([string]$_.expression)) -or
                ($null -ne $_.PSObject.Properties['license'] -and
                 (($null -ne $_.license.PSObject.Properties['id'] -and
                   -not [string]::IsNullOrWhiteSpace([string]$_.license.id)) -or
                  ($null -ne $_.license.PSObject.Properties['name'] -and
                   -not [string]::IsNullOrWhiteSpace([string]$_.license.name))))
            }
    ).Count -gt 0
}

function Read-BoundJsonSidecar {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][object]$SidecarRecords,
        [Parameter(Mandatory)][string]$Name
    )

    $property = $SidecarRecords.PSObject.Properties[$Name]
    if ($null -eq $property) {
        throw "H3 distributable proof does not bind sidecar: $Name"
    }
    $record = $property.Value
    Assert-ExactJsonProperties `
        -Value $record `
        -Expected @('name', 'byte_length', 'sha256') `
        -Context "H3 sidecar record $Name"
    Assert-JsonInteger `
        -Value $record.byte_length `
        -Context "H3 sidecar record $Name.byte_length" `
        -Minimum 1 `
        -Maximum ($maximumGitHubAssetBytes - 1)
    Assert-JsonStringProperties `
        -Value $record `
        -Properties @('name', 'sha256') `
        -Context "H3 sidecar record $Name"
    if ([string]$record.name -cne $Name -or
        [int64]$record.byte_length -le 0 -or
        [string]$record.sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw "H3 sidecar record is invalid: $Name"
    }
    $path = (Resolve-SafeRelativeFile -Root $Root -RelativePath $Name).FullName
    $read = Read-JsonFile -Path $path
    if ([int64]$read.Integrity.ByteLength -ne [int64]$record.byte_length -or
        [string]$read.Integrity.Sha256 -cne [string]$record.sha256) {
        throw "H3 sidecar hash binding failed: $Name"
    }
    return $read
}

function Assert-H3RuntimeSmoke {
    param(
        [Parameter(Mandatory)][object]$Smoke,
        [Parameter(Mandatory)][string]$PackVersion,
        [Parameter(Mandatory)][string]$Context
    )

    Assert-ExactJsonProperties `
        -Value $Smoke `
        -Expected @(
            'schema_version', 'pack_id', 'pack_version', 'platform', 'runtime',
            'contains_model_weights', 'contains_generator', 'contains_comfy',
            'external_decoder_selection_required', 'result'
        ) `
        -Context $Context
    foreach ($property in @(
        'contains_model_weights', 'contains_generator', 'contains_comfy',
        'external_decoder_selection_required'
    )) {
        Assert-JsonBoolean -Value $Smoke.$property -Context "$Context.$property"
    }
    Assert-JsonBoolean `
        -Value $Smoke.runtime.preload_guards.torch_imported `
        -Context "$Context.runtime.preload_guards.torch_imported"
    Assert-JsonArray `
        -Value $Smoke.runtime.protocol.commands `
        -Context "$Context.runtime.protocol.commands"
    Assert-JsonStringProperties `
        -Value $Smoke `
        -Properties @('pack_id', 'pack_version', 'platform', 'result') `
        -Context $Context
    Assert-JsonString `
        -Value $Smoke.runtime.rgb_ring_abi.protocol2 `
        -Context "$Context.runtime.rgb_ring_abi.protocol2"
    foreach ($commandName in @($Smoke.runtime.protocol.commands)) {
        Assert-JsonString -Value $commandName -Context "$Context runtime protocol command"
    }
    foreach ($integerRecord in @(
        [pscustomobject]@{ Value = $Smoke.schema_version; Name = 'schema_version'; Minimum = 1 },
        [pscustomobject]@{ Value = $Smoke.runtime.protocol.selected_version; Name = 'selected_version'; Minimum = 0 },
        [pscustomobject]@{ Value = $Smoke.runtime.protocol.worker_protocol; Name = 'worker_protocol'; Minimum = 0 },
        [pscustomobject]@{ Value = $Smoke.runtime.preload_guards.external_decoder_accesses; Name = 'external_decoder_accesses'; Minimum = 0 }
    )) {
        Assert-JsonInteger `
            -Value $integerRecord.Value `
            -Context "$Context.$($integerRecord.Name)" `
            -Minimum ([int64]$integerRecord.Minimum)
    }
    if ([int]$Smoke.schema_version -ne 1 -or
        [string]$Smoke.pack_id -cne 'org.latentdeck.h3' -or
        [string]$Smoke.pack_version -cne $PackVersion -or
        [string]$Smoke.platform -cne 'windows-x86_64' -or
        [string]$Smoke.result -cne 'passed' -or
        [bool]$Smoke.contains_model_weights -or
        [bool]$Smoke.contains_generator -or
        [bool]$Smoke.contains_comfy -or
        -not [bool]$Smoke.external_decoder_selection_required -or
        [int]$Smoke.runtime.protocol.selected_version -ne 2 -or
        [int]$Smoke.runtime.protocol.worker_protocol -ne 2 -or
        (@($Smoke.runtime.protocol.commands) -join "`0") -cne
            (@('session.configure', 'codec.descriptor') -join "`0") -or
        [string]$Smoke.runtime.rgb_ring_abi.protocol2 -cne '2' -or
        [bool]$Smoke.runtime.preload_guards.torch_imported -or
        [int]$Smoke.runtime.preload_guards.external_decoder_accesses -ne 0) {
        throw "$Context does not prove the supported H3 Protocol 2 runtime boundary."
    }
}

function Assert-ExactFileSet {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string[]]$Expected
    )

    $entries = @(Get-ChildItem -LiteralPath $Root -Force -Recurse)
    foreach ($entry in $entries) {
        if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Artifact set contains a reparse point: $($entry.FullName)"
        }
    }
    $actual = @(
        $entries |
            Where-Object { -not $_.PSIsContainer } |
            ForEach-Object {
                [System.IO.Path]::GetRelativePath($Root, $_.FullName).Replace('\', '/')
            } |
            Sort-Object
    )
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "`0") -cne ($expectedSorted -join "`0")) {
        throw (
            "Artifact set has an unexpected file inventory: $Root`n" +
            "Expected: $($expectedSorted -join ', ')`nActual: $($actual -join ', ')"
        )
    }
}

function Assert-ChecksumManifest {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$ManifestPath,
        [Parameter(Mandatory)][string[]]$ExpectedPaths
    )

    $manifestItem = Get-Item -LiteralPath $ManifestPath -Force
    if ($manifestItem.PSIsContainer -or
        ($manifestItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $manifestItem.Length -eq 0 -or $manifestItem.Length -gt 1MB) {
        throw "Checksum manifest must be a bounded regular file: $ManifestPath"
    }
    $manifestBytes = [System.IO.File]::ReadAllBytes($manifestItem.FullName)
    try {
        $manifestText = [System.Text.UTF8Encoding]::new($false, $true).GetString($manifestBytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "Checksum manifest is not strict UTF-8: $ManifestPath"
    }
    if (-not $manifestText.EndsWith("`n", [System.StringComparison]::Ordinal) -or
        $manifestText.Contains("`r") -or $manifestText.Contains("`0")) {
        throw "Checksum manifest is not canonical UTF-8/LF text: $ManifestPath"
    }
    $lines = @($manifestText.Split("`n") | Where-Object { $_.Length -gt 0 })
    if ($lines.Count -eq 0 -or $lines.Count -gt 1024) {
        throw "Checksum manifest has an invalid line count: $ManifestPath"
    }
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $records = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($line in $lines) {
        if ($line.Length -gt 4096) {
            throw "Checksum manifest line exceeds the 4096-character bound: $ManifestPath"
        }
        if ($line -cnotmatch '^(?<hash>[0-9a-f]{64})  (?<path>[^\r\n]+)$') {
            throw "Checksum manifest line is not canonical: $line"
        }
        $relative = $Matches.path.Replace('\', '/')
        if ([System.IO.Path]::IsPathRooted($relative) -or
            @($relative.Split('/') | Where-Object { $_ -ceq '' -or $_ -ceq '.' -or $_ -ceq '..' }).Count -gt 0 -or
            -not $seen.Add($relative)) {
            throw "Checksum manifest path is unsafe or duplicated: $relative"
        }
        $item = Resolve-SafeRelativeFile -Root $Root -RelativePath $relative
        $actual = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -cne $Matches.hash) {
            throw "Checksum mismatch in artifact set: $relative"
        }
        $records.Add($relative, [pscustomobject]@{
            Path = $item.FullName
            ByteLength = [int64]$item.Length
            Sha256 = $actual
        })
    }
    $expected = @($ExpectedPaths | ForEach-Object { $_.Replace('\', '/') } | Sort-Object -Unique)
    $actualPaths = @($seen | Sort-Object)
    if (($expected -join "`0") -cne ($actualPaths -join "`0")) {
        throw (
            "Checksum manifest coverage is not the exact artifact contract: $ManifestPath`n" +
            "Expected: $($expected -join ', ')`nActual: $($actualPaths -join ', ')"
        )
    }
    return ,$records
}

function Assert-ReceiptFile {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][object]$Receipt,
        [Parameter(Mandatory)][string]$Path
    )

    Assert-JsonInteger `
        -Value $Receipt.byte_length `
        -Context "receipt byte_length for $Path" `
        -Minimum 1 `
        -Maximum ($maximumGitHubAssetBytes - 1)
    Assert-JsonString `
        -Value $Receipt.sha256 `
        -Context "receipt sha256 for $Path" `
        -MinimumLength 64 `
        -MaximumLength 64
    $item = Resolve-SafeRelativeFile -Root $Root -RelativePath $Path
    if ([int64]$item.Length -ne [int64]$Receipt.byte_length) {
        throw "Receipt file size/type mismatch: $Path"
    }
    $hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -cne [string]$Receipt.sha256) {
        throw "Receipt file hash mismatch: $Path"
    }
    return [pscustomobject]@{
        Path = $item.FullName
        ByteLength = [int64]$item.Length
        Sha256 = $hash
    }
}

function Get-FileIntegrityRecord {
    param([Parameter(Mandatory)][string]$Path)

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Release metadata is not a regular file: $Path"
    }
    return [pscustomobject]@{
        Path = $item.FullName
        ByteLength = [int64]$item.Length
        Sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Get-ZipEntryIntegrityRecord {
    param(
        [Parameter(Mandatory)][string]$ArchivePath,
        [Parameter(Mandatory)][string]$EntryName,
        [int64]$MaximumBytes = 512MB
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ceq $EntryName })
        if ($entries.Count -ne 1 -or $entries[0].Length -le 0 -or
            [int64]$entries[0].Length -gt $MaximumBytes) {
            throw "Release archive does not contain one bounded required entry: $EntryName"
        }
        $input = $entries[0].Open()
        $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
            [System.Security.Cryptography.HashAlgorithmName]::SHA256
        )
        try {
            $buffer = [byte[]]::new(1MB)
            [int64]$length = 0
            while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $length += $read
                if ($length -gt $MaximumBytes -or $length -gt [int64]$entries[0].Length) {
                    throw "Release archive entry exceeded its declared bound: $EntryName"
                }
                $hasher.AppendData($buffer, 0, $read)
            }
            if ($length -ne [int64]$entries[0].Length) {
                throw "Release archive entry ended before its declared length: $EntryName"
            }
            $hash = [System.Convert]::ToHexString($hasher.GetHashAndReset()).ToLowerInvariant()
        } finally {
            $hasher.Dispose()
            $input.Dispose()
        }
    } finally {
        $archive.Dispose()
    }
    return [pscustomobject]@{
        Path = "$ArchivePath::$EntryName"
        ByteLength = $length
        Sha256 = $hash
    }
}

function Read-ZipJsonEntry {
    param(
        [Parameter(Mandatory)][string]$ArchivePath,
        [Parameter(Mandatory)][string]$EntryName,
        [int64]$MaximumBytes = 32MB
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entries = @($archive.Entries | Where-Object { $_.FullName -ceq $EntryName })
        if ($entries.Count -ne 1 -or $entries[0].Length -le 0 -or
            [int64]$entries[0].Length -gt $MaximumBytes) {
            throw "Release archive does not contain one bounded JSON entry: $EntryName"
        }
        $input = $entries[0].Open()
        $memory = [System.IO.MemoryStream]::new([int]$entries[0].Length)
        $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
            [System.Security.Cryptography.HashAlgorithmName]::SHA256
        )
        try {
            $buffer = [byte[]]::new(1MB)
            [int64]$length = 0
            while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
                $length += $read
                if ($length -gt $MaximumBytes -or $length -gt [int64]$entries[0].Length) {
                    throw "Release archive JSON entry exceeded its declared bound: $EntryName"
                }
                $memory.Write($buffer, 0, $read)
                $hasher.AppendData($buffer, 0, $read)
            }
            if ($length -ne [int64]$entries[0].Length) {
                throw "Release archive JSON entry ended before its declared length: $EntryName"
            }
            $bytes = $memory.ToArray()
            $hash = [System.Convert]::ToHexString($hasher.GetHashAndReset()).ToLowerInvariant()
        } finally {
            $hasher.Dispose()
            $memory.Dispose()
            $input.Dispose()
        }
    } finally {
        $archive.Dispose()
    }
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "Release archive JSON entry is not strict UTF-8: $EntryName"
    }
    if ($text.IndexOf([char]0) -ge 0) {
        throw "Release archive JSON entry contains a NUL byte: $EntryName"
    }
    return [pscustomobject]@{
        Value = $text | ConvertFrom-Json -Depth 100
        Integrity = [pscustomobject]@{
            Path = "$ArchivePath::$EntryName"
            ByteLength = $length
            Sha256 = $hash
        }
    }
}

function Copy-ReleaseAsset {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$DestinationName,
        [Parameter(Mandatory)][string]$StageRoot,
        [Parameter(Mandatory)][int64]$ExpectedByteLength,
        [Parameter(Mandatory)][string]$ExpectedSha256
    )

    if ($DestinationName -cnotmatch '^[A-Za-z0-9][A-Za-z0-9._+ -]{0,199}$') {
        throw "Release asset name is unsafe: $DestinationName"
    }
    Assert-ArtifactPath `
        -Path (Split-Path -Parent ([System.IO.Path]::GetFullPath($Source))) `
        -RequireExistingDirectory | Out-Null
    Assert-ArtifactPath -Path $StageRoot -RequireExistingDirectory | Out-Null
    $sourceItem = Get-Item -LiteralPath $Source -Force
    if ($sourceItem.PSIsContainer -or
        ($sourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $sourceItem.Length -eq 0 -or [int64]$sourceItem.Length -ge $maximumGitHubAssetBytes -or
        [int64]$sourceItem.Length -ne $ExpectedByteLength -or
        $ExpectedSha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw "Release asset must be a regular non-empty file smaller than 2 GiB: $Source"
    }
    $destination = Join-Path $StageRoot $DestinationName
    if (Test-Path -LiteralPath $destination) {
        throw "Release asset name collision: $DestinationName"
    }
    $input = $null
    $output = $null
    $hasher = $null
    try {
        $input = [System.IO.File]::Open(
            $sourceItem.FullName,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::Read
        )
        $output = [System.IO.File]::Open(
            $destination,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
            [System.Security.Cryptography.HashAlgorithmName]::SHA256
        )
        $buffer = [byte[]]::new(1MB)
        while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $output.Write($buffer, 0, $read)
            $hasher.AppendData($buffer, 0, $read)
        }
        $copyHash = [System.Convert]::ToHexString($hasher.GetHashAndReset()).ToLowerInvariant()
    } finally {
        if ($null -ne $hasher) { $hasher.Dispose() }
        if ($null -ne $output) { $output.Dispose() }
        if ($null -ne $input) { $input.Dispose() }
    }
    $item = Get-Item -LiteralPath $destination -Force
    if ([int64]$item.Length -ne $ExpectedByteLength -or $copyHash -cne $ExpectedSha256) {
        throw "Release asset changed after validation and was not staged: $Source"
    }
    return [pscustomobject]@{
        name = $item.Name
        byte_length = [int64]$item.Length
        sha256 = $copyHash
    }
}

function Write-Utf8Text {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Text
    )

    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Assert-SafeZipEntryName {
    param([Parameter(Mandatory)][string]$Name)

    $canonical = $Name.Replace('\', '/')
    $segments = @($canonical.Split('/'))
    if ([string]::IsNullOrWhiteSpace($canonical) -or
        $canonical.Length -gt 240 -or
        $canonical -cne $Name -or
        [System.IO.Path]::IsPathRooted($canonical) -or
        $canonical.Contains(':') -or
        @($segments | Where-Object { $_ -ceq '' -or $_ -ceq '.' -or $_ -ceq '..' }).Count -gt 0) {
        throw "Release bundle entry name is unsafe: $Name"
    }
}

function New-ChecksumText {
    param([Parameter(Mandatory)][object[]]$Mappings)

    return ((@(
        $Mappings |
            Sort-Object EntryName -CaseSensitive |
            ForEach-Object {
                "$([string]$_.Record.Sha256)  $([string]$_.EntryName)"
            }
    ) -join "`n") + "`n")
}

function New-DeterministicBoundZip {
    param(
        [Parameter(Mandatory)][object[]]$Mappings,
        [Parameter(Mandatory)][string]$DestinationPath,
        [System.IO.Compression.CompressionLevel]$CompressionLevel =
            [System.IO.Compression.CompressionLevel]::NoCompression
    )

    if (Test-Path -LiteralPath $DestinationPath) {
        throw "Refusing to overwrite a release bundle: $DestinationPath"
    }
    if ($Mappings.Count -eq 0 -or $Mappings.Count -gt 256) {
        throw 'Release bundle has an invalid entry count.'
    }
    $orderedMappings = @($Mappings | Sort-Object EntryName -CaseSensitive)
    $entryNames = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($mapping in $orderedMappings) {
        Assert-SafeZipEntryName -Name ([string]$mapping.EntryName)
        if (-not $entryNames.Add([string]$mapping.EntryName)) {
            throw "Release bundle contains a duplicate entry: $($mapping.EntryName)"
        }
        $record = $mapping.Record
        $sourceItem = Get-Item -LiteralPath ([string]$record.Path) -Force
        if ($sourceItem.PSIsContainer -or
            ($sourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            [int64]$sourceItem.Length -le 0 -or
            [int64]$sourceItem.Length -ne [int64]$record.ByteLength -or
            [string]$record.Sha256 -cnotmatch '^[0-9a-f]{64}$') {
            throw "Release bundle source is not the exact validated regular file: $($record.Path)"
        }
    }

    Add-Type -AssemblyName System.IO.Compression
    $archiveStream = [System.IO.File]::Open(
        $DestinationPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $archiveStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $true,
            [System.Text.Encoding]::UTF8
        )
        try {
            foreach ($mapping in $orderedMappings) {
                $record = $mapping.Record
                $entry = $archive.CreateEntry([string]$mapping.EntryName, $CompressionLevel)
                $entry.LastWriteTime = [System.DateTimeOffset]::new(
                    1980, 1, 1, 0, 0, 0, [System.TimeSpan]::Zero
                )
                $input = $null
                $output = $null
                $hasher = $null
                $copiedBytes = [int64]0
                try {
                    $input = [System.IO.File]::Open(
                        [string]$record.Path,
                        [System.IO.FileMode]::Open,
                        [System.IO.FileAccess]::Read,
                        [System.IO.FileShare]::Read
                    )
                    $output = $entry.Open()
                    $hasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
                        [System.Security.Cryptography.HashAlgorithmName]::SHA256
                    )
                    $buffer = [byte[]]::new(1MB)
                    while (($read = $input.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        $output.Write($buffer, 0, $read)
                        $hasher.AppendData($buffer, 0, $read)
                        $copiedBytes += $read
                    }
                    $copyHash = [System.Convert]::ToHexString(
                        $hasher.GetHashAndReset()
                    ).ToLowerInvariant()
                } finally {
                    if ($null -ne $hasher) { $hasher.Dispose() }
                    if ($null -ne $output) { $output.Dispose() }
                    if ($null -ne $input) { $input.Dispose() }
                }
                if ($copiedBytes -ne [int64]$record.ByteLength -or
                    $copyHash -cne [string]$record.Sha256) {
                    throw (
                        "Release bundle source changed after validation and was not staged: " +
                        [string]$record.Path
                    )
                }
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $archiveStream.Dispose()
    }

    $archiveItem = Get-Item -LiteralPath $DestinationPath -Force
    if ($archiveItem.PSIsContainer -or
        ($archiveItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        [int64]$archiveItem.Length -le 0 -or
        [int64]$archiveItem.Length -ge $maximumGitHubAssetBytes) {
        throw "Release bundle must be a regular non-empty file smaller than 2 GiB: $DestinationPath"
    }

    $readArchive = [System.IO.Compression.ZipFile]::OpenRead($archiveItem.FullName)
    try {
        $actualEntries = @($readArchive.Entries)
        if ($actualEntries.Count -ne $orderedMappings.Count) {
            throw 'Release bundle entry inventory changed after creation.'
        }
        foreach ($mapping in $orderedMappings) {
            $matches = @($actualEntries | Where-Object {
                [string]$_.FullName -ceq [string]$mapping.EntryName
            })
            if ($matches.Count -ne 1 -or
                [int64]$matches[0].Length -ne [int64]$mapping.Record.ByteLength -or
                $matches[0].LastWriteTime.DateTime -ne
                    [datetime]::new(1980, 1, 1, 0, 0, 0)) {
                throw "Release bundle entry metadata is invalid: $($mapping.EntryName)"
            }
            $entryStream = $matches[0].Open()
            $entryHasher = [System.Security.Cryptography.IncrementalHash]::CreateHash(
                [System.Security.Cryptography.HashAlgorithmName]::SHA256
            )
            try {
                $buffer = [byte[]]::new(1MB)
                while (($read = $entryStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                    $entryHasher.AppendData($buffer, 0, $read)
                }
                $entryHash = [System.Convert]::ToHexString(
                    $entryHasher.GetHashAndReset()
                ).ToLowerInvariant()
            } finally {
                $entryHasher.Dispose()
                $entryStream.Dispose()
            }
            if ($entryHash -cne [string]$mapping.Record.Sha256) {
                throw "Release bundle entry hash differs after creation: $($mapping.EntryName)"
            }
        }
    } finally {
        $readArchive.Dispose()
    }
    return Get-FileIntegrityRecord -Path $archiveItem.FullName
}

$appRoot = Assert-ArtifactPath -Path $ApplicationArtifactDirectory -RequireExistingDirectory
$codecRoot = Assert-ArtifactPath -Path $CodecArtifactDirectory -RequireExistingDirectory
$developerRoot = Assert-ArtifactPath -Path $DeveloperKitArtifactDirectory -RequireExistingDirectory
$recorderRoot = Assert-ArtifactPath -Path $ComfyRecorderArtifactDirectory -RequireExistingDirectory
$appReceiptPath = Join-Path $appRoot 'release-candidate.json'
$codecReceiptPath = Join-Path $codecRoot 'distributable-proof.json'
$developerReceiptPath = Join-Path $developerRoot 'developer-kit.json'
$recorderReceiptPath = Join-Path `
    $recorderRoot `
    'LatentDeck-0.1.0-preview.1-comfy-recorder-windows-x64.receipt.json'
$appRead = Read-JsonFile -Path $appReceiptPath
$codecRead = Read-JsonFile -Path $codecReceiptPath
$developerRead = Read-JsonFile -Path $developerReceiptPath
$recorderRead = Read-JsonFile -Path $recorderReceiptPath
$appReceipt = $appRead.Value
$codecReceipt = $codecRead.Value
$developerReceipt = $developerRead.Value
$recorderReceipt = $recorderRead.Value
$appReceiptRecord = $appRead.Integrity
$codecReceiptRecord = $codecRead.Integrity
$developerReceiptRecord = $developerRead.Integrity
$recorderReceiptRecord = $recorderRead.Integrity

foreach ($receiptRecord in @(
    [pscustomobject]@{ Name = 'application'; Value = $appReceipt },
    [pscustomobject]@{ Name = 'H3'; Value = $codecReceipt },
    [pscustomobject]@{ Name = 'Developer Kit'; Value = $developerReceipt },
    [pscustomobject]@{ Name = 'Comfy Recorder'; Value = $recorderReceipt }
)) {
    Assert-JsonStringProperties `
        -Value $receiptRecord.Value `
        -Properties @('release_label', 'release_channel') `
        -Context "$($receiptRecord.Name) receipt"
    Assert-JsonBoolean `
        -Value $receiptRecord.Value.distributable `
        -Context "$($receiptRecord.Name) receipt.distributable"
    Assert-JsonBoolean `
        -Value $receiptRecord.Value.signed `
        -Context "$($receiptRecord.Name) receipt.signed"
    Assert-JsonBoolean `
        -Value $receiptRecord.Value.unsigned `
        -Context "$($receiptRecord.Name) receipt.unsigned"
}
foreach ($sourceRecord in @(
    [pscustomobject]@{
        Name = 'application'; Value = $appReceipt.source
        Strings = @('git_commit', 'git_branch', 'git_tree', 'public_snapshot_sha256')
    },
    [pscustomobject]@{
        Name = 'H3'; Value = $codecReceipt.source
        Strings = @('commit', 'branch', 'git_tree', 'public_snapshot_sha256')
    },
    [pscustomobject]@{
        Name = 'Developer Kit'; Value = $developerReceipt.source
        Strings = @('git_commit', 'git_branch', 'git_tree', 'public_snapshot_sha256')
    },
    [pscustomobject]@{
        Name = 'Comfy Recorder'; Value = $recorderReceipt.source
        Strings = @('git_commit', 'git_branch', 'git_tree', 'public_snapshot_sha256')
    }
)) {
    Assert-JsonStringProperties `
        -Value $sourceRecord.Value `
        -Properties $sourceRecord.Strings `
        -Context "$($sourceRecord.Name) receipt.source"
    Assert-JsonBoolean `
        -Value $sourceRecord.Value.git_dirty `
        -Context "$($sourceRecord.Name) receipt.source.git_dirty"
    Assert-JsonInteger `
        -Value $sourceRecord.Value.git_dirty_entry_count `
        -Context "$($sourceRecord.Name) receipt.source.git_dirty_entry_count" `
        -Minimum 0
    Assert-JsonInteger `
        -Value $sourceRecord.Value.public_snapshot_file_count `
        -Context "$($sourceRecord.Name) receipt.source.public_snapshot_file_count" `
        -Minimum 1
}
foreach ($application in @($appReceipt.applications)) {
    Assert-JsonStringProperties `
        -Value $application `
        -Properties @('product', 'file_name', 'sha256', 'authenticode') `
        -Context 'application artifact'
    Assert-JsonBoolean `
        -Value $application.unsigned `
        -Context "application artifact $($application.product).unsigned"
}
foreach ($schemaRecord in @(
    [pscustomobject]@{ Name = 'application'; Value = $appReceipt.schema_version },
    [pscustomobject]@{ Name = 'H3'; Value = $codecReceipt.schema_version },
    [pscustomobject]@{ Name = 'Developer Kit'; Value = $developerReceipt.schema_version },
    [pscustomobject]@{ Name = 'Comfy Recorder'; Value = $recorderReceipt.schema_version }
)) {
    Assert-JsonInteger `
        -Value $schemaRecord.Value `
        -Context "$($schemaRecord.Name) receipt.schema_version" `
        -Minimum 1
}
foreach ($arrayRecord in @(
    [pscustomobject]@{ Name = 'application receipt.applications'; Value = $appReceipt.applications },
    [pscustomobject]@{ Name = 'application receipt.sboms'; Value = $appReceipt.sboms },
    [pscustomobject]@{ Name = 'H3 runtime wheel lock.wheels'; Value = $codecReceipt.runtime_wheel_lock.wheels },
    [pscustomobject]@{ Name = 'Comfy Recorder receipt.supported_python'; Value = $recorderReceipt.supported_python },
    [pscustomobject]@{ Name = 'Comfy Recorder receipt.packages'; Value = $recorderReceipt.packages },
    [pscustomobject]@{ Name = 'Comfy Recorder receipt.sbom.selection_roots'; Value = $recorderReceipt.sbom.selection_roots }
)) {
    Assert-JsonArray -Value $arrayRecord.Value -Context $arrayRecord.Name
}

if ([int]$appReceipt.schema_version -ne 6 -or
    [int]$codecReceipt.schema_version -ne 2 -or
    [int]$developerReceipt.schema_version -ne 2 -or
    [int]$recorderReceipt.schema_version -ne 1) {
    throw 'Release input receipt schema versions are unsupported.'
}
Assert-ExactJsonProperties `
    -Value $codecReceipt `
    -Expected @(
        'schema_version', 'release_label', 'release_channel', 'pack_id', 'pack_version',
        'adapter_version', 'distributable', 'signed', 'unsigned', 'platform', 'archive',
        'setup', 'cpython', 'runtime_wheel_lock', 'dependency_inventory', 'sbom',
        'safetensors_native_closure', 'installer_license_review', 'archive_runtime_smoke',
        'isolated_native_install_smoke', 'isolated_native_uninstall', 'cuda_required',
        'contains_model_weights', 'contains_generator', 'contains_comfy', 'source',
        'sidecars', 'signing', 'publisher_signature'
    ) `
    -Context 'H3 distributable proof'
Assert-JsonStringProperties `
    -Value $codecReceipt `
    -Properties @(
        'release_label', 'release_channel', 'pack_id', 'pack_version',
        'adapter_version', 'platform', 'dependency_inventory', 'sbom',
        'installer_license_review', 'archive_runtime_smoke',
        'isolated_native_install_smoke', 'isolated_native_uninstall',
        'publisher_signature'
    ) `
    -Context 'H3 distributable proof'
Assert-ExactJsonProperties `
    -Value $codecReceipt.runtime_wheel_lock `
    -Expected @('name', 'sha256', 'install_policy', 'wheel_count', 'wheels') `
    -Context 'H3 runtime wheel lock evidence'
Assert-JsonStringProperties `
    -Value $codecReceipt.runtime_wheel_lock `
    -Properties @('name', 'sha256', 'install_policy') `
    -Context 'H3 runtime wheel lock evidence'
Assert-JsonInteger `
    -Value $codecReceipt.runtime_wheel_lock.wheel_count `
    -Context 'H3 runtime wheel lock.wheel_count' `
    -Minimum 1 `
    -Maximum 128
foreach ($h3Boolean in @(
    'contains_model_weights', 'contains_generator', 'contains_comfy', 'cuda_required'
)) {
    Assert-JsonBoolean `
        -Value $codecReceipt.$h3Boolean `
        -Context "H3 distributable proof.$h3Boolean"
}
$h3RuntimeLockPath = Join-Path `
    $repositoryRoot `
    'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
$h3RuntimeLockRead = Read-JsonFile -Path $h3RuntimeLockPath
$h3RuntimeLock = $h3RuntimeLockRead.Value
$expectedH3Wheels = @(
    $h3RuntimeLock.dependencies |
        Sort-Object { [string]$_.name } -CaseSensitive |
        ForEach-Object {
            [pscustomobject][ordered]@{
                name = [string]$_.name
                version = [string]$_.version
                file_name = [string]$_.wheel.file_name
                url = [string]$_.wheel.url
                byte_length = [int64]$_.wheel.byte_length
                sha256 = [string]$_.wheel.sha256
            }
        }
)
$actualH3Wheels = @($codecReceipt.runtime_wheel_lock.wheels)
if ([string]$codecReceipt.runtime_wheel_lock.name -cne
        'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json' -or
    [string]$codecReceipt.runtime_wheel_lock.sha256 -cne
        [string]$h3RuntimeLockRead.Integrity.Sha256 -or
    [string]$codecReceipt.runtime_wheel_lock.install_policy -cne
        'direct_https_wheels_only_sha256_required' -or
    [int]$codecReceipt.runtime_wheel_lock.wheel_count -ne $expectedH3Wheels.Count -or
    $actualH3Wheels.Count -ne $expectedH3Wheels.Count) {
    throw 'H3 runtime wheel lock evidence is incomplete or does not match the source lock.'
}
for ($wheelIndex = 0; $wheelIndex -lt $expectedH3Wheels.Count; $wheelIndex++) {
    $expectedWheel = $expectedH3Wheels[$wheelIndex]
    $actualWheel = $actualH3Wheels[$wheelIndex]
    Assert-ExactJsonProperties `
        -Value $actualWheel `
        -Expected @('name', 'version', 'file_name', 'url', 'byte_length', 'sha256') `
        -Context "H3 runtime wheel evidence $wheelIndex"
    Assert-JsonStringProperties `
        -Value $actualWheel `
        -Properties @('name', 'version', 'file_name', 'url', 'sha256') `
        -Context "H3 runtime wheel evidence $wheelIndex"
    Assert-JsonInteger `
        -Value $actualWheel.byte_length `
        -Context "H3 runtime wheel evidence $wheelIndex.byte_length" `
        -Minimum 1 `
        -Maximum ($maximumGitHubAssetBytes - 1)
    if ([string]$actualWheel.name -cne [string]$expectedWheel.name -or
        [string]$actualWheel.version -cne [string]$expectedWheel.version -or
        [string]$actualWheel.file_name -cne [string]$expectedWheel.file_name -or
        [string]$actualWheel.url -cne [string]$expectedWheel.url -or
        [int64]$actualWheel.byte_length -ne [int64]$expectedWheel.byte_length -or
        [string]$actualWheel.sha256 -cne [string]$expectedWheel.sha256) {
        throw "H3 runtime wheel evidence drifted from the source lock at index $wheelIndex."
    }
}
$releaseLabel = [string]$appReceipt.release_label
$releaseChannel = [string]$appReceipt.release_channel
if ($releaseLabel -cnotin @('0.1.0-preview.1', '0.1.0') -or
    $releaseChannel -cnotin @('unsigned_preview', 'stable') -or
    [string]$codecReceipt.release_label -cne $releaseLabel -or
    [string]$developerReceipt.release_label -cne $releaseLabel -or
    [string]$recorderReceipt.release_label -cne $releaseLabel -or
    [string]$codecReceipt.release_channel -cne $releaseChannel -or
    [string]$developerReceipt.release_channel -cne $releaseChannel -or
    [string]$recorderReceipt.release_channel -cne $releaseChannel) {
    throw 'Release inputs do not share one supported release label and channel.'
}
if (($releaseChannel -ceq 'unsigned_preview' -and $releaseLabel -cne '0.1.0-preview.1') -or
    ($releaseChannel -ceq 'stable' -and $releaseLabel -cne '0.1.0')) {
    throw 'Release label and channel are not the exact supported pair.'
}
if ([string]$appReceipt.application_api_version -cne '0.1.0' -or
    [string]$appReceipt.windows_installer_version -cne '0.1.0+1' -or
    [string]$developerReceipt.application_api_version -cne '0.1.0' -or
    [string]$developerReceipt.windows_installer_version -cne '0.1.0+1' -or
    [string]$codecReceipt.pack_version -cne '0.2.1' -or
    [string]$codecReceipt.adapter_version -cne '0.2.0') {
    throw 'Release input application, installer, Developer Kit, or H3 identity is incorrect.'
}
Assert-JsonStringProperties `
    -Value $appReceipt `
    -Properties @('application_api_version', 'windows_installer_version') `
    -Context 'Application release receipt'
Assert-JsonStringProperties `
    -Value $developerReceipt `
    -Properties @('application_api_version', 'windows_installer_version') `
    -Context 'Developer Kit receipt'
Assert-ExactJsonProperties `
    -Value $recorderReceipt `
    -Expected @(
        'schema_version', 'artifact_kind', 'release_label', 'release_channel', 'target',
        'python_abi', 'supported_python', 'signed', 'unsigned', 'distributable',
        'contains_model_weights', 'contains_cartridges', 'source', 'packages', 'archive',
        'sbom', 'third_party_notices', 'license_bundle', 'license_review'
    ) `
    -Context 'Comfy Recorder receipt'
Assert-JsonStringProperties `
    -Value $recorderReceipt `
    -Properties @('artifact_kind', 'target', 'python_abi') `
    -Context 'Comfy Recorder receipt'
foreach ($pythonAbi in @($recorderReceipt.supported_python)) {
    Assert-JsonString -Value $pythonAbi -Context 'Comfy Recorder supported Python ABI'
}
Assert-JsonBoolean `
    -Value $recorderReceipt.contains_model_weights `
    -Context 'Comfy Recorder receipt.contains_model_weights'
Assert-JsonBoolean `
    -Value $recorderReceipt.contains_cartridges `
    -Context 'Comfy Recorder receipt.contains_cartridges'
if ([string]$recorderReceipt.artifact_kind -cne 'comfy_recorder_bundle' -or
    [string]$recorderReceipt.target -cne 'windows-x64' -or
    [string]$recorderReceipt.python_abi -cne 'cp312-abi3' -or
    (@($recorderReceipt.supported_python) -join "`0") -cne (@('cp312', 'cp313') -join "`0") -or
    [bool]$recorderReceipt.contains_model_weights -or
    [bool]$recorderReceipt.contains_cartridges) {
    throw 'Comfy Recorder identity or public payload boundary is incorrect.'
}
Assert-ExactJsonProperties `
    -Value $appReceipt.component_versions `
    -Expected @('decks', 'sdks') `
    -Context 'Application release component versions'
Assert-ExactJsonProperties `
    -Value $appReceipt.component_versions.decks `
    -Expected @('d2', 'q4') `
    -Context 'Application release Deck versions'
foreach ($deckName in @('d2', 'q4')) {
    Assert-ExactJsonProperties `
        -Value $appReceipt.component_versions.decks.$deckName `
        -Expected @('deck_id', 'deck_version') `
        -Context "Application release $deckName Deck identity"
    Assert-JsonStringProperties `
        -Value $appReceipt.component_versions.decks.$deckName `
        -Properties @('deck_id', 'deck_version') `
        -Context "Application release $deckName Deck identity"
}
Assert-ExactJsonProperties `
    -Value $appReceipt.component_versions.sdks `
    -Expected @('cartridge', 'deck', 'codec') `
    -Context 'Application release SDK versions'
Assert-JsonStringProperties `
    -Value $appReceipt.component_versions.sdks `
    -Properties @('cartridge', 'deck', 'codec') `
    -Context 'Application release SDK versions'
if ([string]$appReceipt.component_versions.decks.d2.deck_id -cne 'org.latentdeck.deck.d2' -or
    [string]$appReceipt.component_versions.decks.d2.deck_version -cne '0.2.1' -or
    [string]$appReceipt.component_versions.decks.q4.deck_id -cne 'org.latentdeck.deck.q4' -or
    [string]$appReceipt.component_versions.decks.q4.deck_version -cne '0.2.1' -or
    [string]$appReceipt.component_versions.sdks.cartridge -cne '0.1.0' -or
    [string]$appReceipt.component_versions.sdks.deck -cne '0.2.0' -or
    [string]$appReceipt.component_versions.sdks.codec -cne '0.2.0') {
    throw 'Release input Deck or SDK component version is not the supported preview identity.'
}
Assert-ExactJsonProperties `
    -Value $appReceipt.source `
    -Expected @(
        'git_commit', 'git_branch', 'git_tree', 'git_dirty', 'git_dirty_entry_count',
        'public_snapshot_sha256', 'public_snapshot_file_count'
    ) `
    -Context 'Application release source'
Assert-ExactJsonProperties `
    -Value $developerReceipt.source `
    -Expected @(
        'git_commit', 'git_branch', 'git_tree', 'git_dirty', 'git_dirty_entry_count',
        'public_snapshot_sha256', 'public_snapshot_file_count'
    ) `
    -Context 'Developer Kit source'
Assert-ExactJsonProperties `
    -Value $codecReceipt.source `
    -Expected @(
        'commit', 'branch', 'git_dirty', 'git_dirty_entry_count', 'git_tree',
        'public_snapshot_sha256', 'public_snapshot_file_count'
    ) `
    -Context 'H3 distributable proof source'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.source `
    -Expected @(
        'git_commit', 'git_branch', 'git_tree', 'git_dirty', 'git_dirty_entry_count',
        'public_snapshot_sha256', 'public_snapshot_file_count'
    ) `
    -Context 'Comfy Recorder source'
if ([string]$appReceipt.source.git_tree -cnotmatch '^[0-9a-f]{40}$' -or
    [string]$developerReceipt.source.git_tree -cnotmatch '^[0-9a-f]{40}$' -or
    [int64]$appReceipt.source.git_dirty_entry_count -ne 0 -or
    [int64]$developerReceipt.source.git_dirty_entry_count -ne 0 -or
    [string]$appReceipt.source.public_snapshot_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    [string]$developerReceipt.source.public_snapshot_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    [int64]$appReceipt.source.public_snapshot_file_count -le 0 -or
    [int64]$developerReceipt.source.public_snapshot_file_count -le 0 -or
    [string]$codecReceipt.source.git_tree -cnotmatch '^[0-9a-f]{40}$' -or
    [int64]$codecReceipt.source.git_dirty_entry_count -ne 0 -or
    [string]$codecReceipt.source.public_snapshot_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    [int64]$codecReceipt.source.public_snapshot_file_count -le 0) {
    throw 'A release input source identity is malformed.'
}
if ([string]$recorderReceipt.source.git_tree -cnotmatch '^[0-9a-f]{40}$' -or
    [int64]$recorderReceipt.source.git_dirty_entry_count -ne 0 -or
    [string]$recorderReceipt.source.public_snapshot_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    [int64]$recorderReceipt.source.public_snapshot_file_count -le 0) {
    throw 'The Comfy Recorder source identity is malformed.'
}
if ($appReceipt.PSObject.Properties['distributable'] -eq $null -or
    $codecReceipt.PSObject.Properties['distributable'] -eq $null -or
    $developerReceipt.PSObject.Properties['distributable'] -eq $null -or
    $recorderReceipt.PSObject.Properties['distributable'] -eq $null -or
    -not [bool]$appReceipt.distributable -or
    -not [bool]$codecReceipt.distributable -or
    -not [bool]$developerReceipt.distributable -or
    -not [bool]$recorderReceipt.distributable) {
    throw 'Release staging accepts only distributable artifacts built from clean main.'
}

$appCommit = [string]$appReceipt.source.git_commit
$codecCommit = [string]$codecReceipt.source.commit
$developerCommit = [string]$developerReceipt.source.git_commit
$recorderCommit = [string]$recorderReceipt.source.git_commit
if ($appCommit -cnotmatch '^[0-9a-f]{40}$' -or
    $appCommit -cne $codecCommit -or $appCommit -cne $developerCommit -or
    $appCommit -cne $recorderCommit -or
    [string]$appReceipt.source.git_branch -cne 'main' -or
    [string]$codecReceipt.source.branch -cne 'main' -or
    [string]$developerReceipt.source.git_branch -cne 'main' -or
    [string]$recorderReceipt.source.git_branch -cne 'main' -or
    [bool]$appReceipt.source.git_dirty -or
    [bool]$codecReceipt.source.git_dirty -or
    [bool]$developerReceipt.source.git_dirty -or
    [bool]$recorderReceipt.source.git_dirty -or
    [string]$appReceipt.source.git_tree -cne [string]$codecReceipt.source.git_tree -or
    [string]$appReceipt.source.git_tree -cne [string]$developerReceipt.source.git_tree -or
    [string]$appReceipt.source.git_tree -cne [string]$recorderReceipt.source.git_tree -or
    [string]$appReceipt.source.public_snapshot_sha256 -cne
        [string]$codecReceipt.source.public_snapshot_sha256 -or
    [string]$appReceipt.source.public_snapshot_sha256 -cne
        [string]$developerReceipt.source.public_snapshot_sha256 -or
    [string]$appReceipt.source.public_snapshot_sha256 -cne
        [string]$recorderReceipt.source.public_snapshot_sha256 -or
    [int64]$appReceipt.source.public_snapshot_file_count -ne
        [int64]$codecReceipt.source.public_snapshot_file_count -or
    [int64]$appReceipt.source.public_snapshot_file_count -ne
        [int64]$developerReceipt.source.public_snapshot_file_count -or
    [int64]$appReceipt.source.public_snapshot_file_count -ne
        [int64]$recorderReceipt.source.public_snapshot_file_count) {
    throw 'Release inputs must come from the same clean main source snapshot.'
}
Assert-JsonStringProperties `
    -Value $codecReceipt.signing `
    -Properties @(
        'mode', 'outer_setup_authenticode', 'embedded_uninstaller_finalize',
        'installed_uninstaller_authenticode'
    ) `
    -Context 'H3 signing evidence'
if ($releaseChannel -ceq 'unsigned_preview') {
    if ([bool]$appReceipt.signed -or -not [bool]$appReceipt.unsigned -or
        [bool]$codecReceipt.signed -or -not [bool]$codecReceipt.unsigned -or
        [bool]$developerReceipt.signed -or
        -not [bool]$developerReceipt.unsigned -or
        [bool]$recorderReceipt.signed -or
        -not [bool]$recorderReceipt.unsigned -or
        @($appReceipt.applications | Where-Object {
            -not [bool]$_.unsigned -or [string]$_.authenticode -cne 'not_present'
        }).Count -gt 0 -or
        [string]$codecReceipt.signing.mode -cne 'unsigned_local_rc' -or
        [string]$codecReceipt.signing.outer_setup_authenticode -cne 'not_present' -or
        [string]$codecReceipt.signing.embedded_uninstaller_finalize -cne 'not_requested' -or
        [string]$codecReceipt.publisher_signature -cne 'not_present_local_rc' -or
        [string]$codecReceipt.signing.installed_uninstaller_authenticode -cne 'not_run_clean_machine_gate') {
        throw 'Unsigned preview inputs contain unexpected signing evidence.'
    }
} else {
    $installedAppEvidence = $appReceipt.PSObject.Properties['installed_binaries_authenticode']
    $installedAppRecords = if ($null -eq $installedAppEvidence) {
        @()
    } else {
        Assert-JsonArray `
            -Value $installedAppEvidence.Value `
            -Context 'application installed-binary Authenticode evidence'
        @($installedAppEvidence.Value)
    }
    foreach ($installedAppRecord in $installedAppRecords) {
        Assert-JsonStringProperties `
            -Value $installedAppRecord `
            -Properties @('status', 'product') `
            -Context 'application installed-binary Authenticode evidence'
    }
    $validInstalledApps = @(
        $installedAppRecords |
            Where-Object {
                [string]$_.status -ceq 'valid' -and
                [string]$_.product -cin @('LatentDeck App', 'LatentPlayer')
            }
    )
    if (-not [bool]$appReceipt.signed -or
        @($appReceipt.applications | Where-Object {
            [bool]$_.unsigned -or [string]$_.authenticode -cne 'valid'
        }).Count -gt 0 -or
        [string]$codecReceipt.signing.mode -cne 'authenticode_finalize_command' -or
        [string]$codecReceipt.signing.outer_setup_authenticode -cne 'valid' -or
        [string]$codecReceipt.signing.installed_uninstaller_authenticode -cne 'valid' -or
        $installedAppRecords.Count -ne 2 -or $validInstalledApps.Count -ne 2 -or
        @($validInstalledApps | Select-Object -ExpandProperty product -Unique).Count -ne 2) {
        throw (
            'Stable release inputs lack clean-machine Authenticode evidence for both ' +
            'application installers/installed executables and the installed Codec Pack uninstaller.'
        )
    }
}
foreach ($reviewRecord in @(
    [pscustomobject]@{ Name = 'application'; Value = $appReceipt.license_review },
    [pscustomobject]@{ Name = 'Developer Kit'; Value = $developerReceipt.license_review }
)) {
    Assert-JsonString `
        -Value $reviewRecord.Value.status `
        -Context "$($reviewRecord.Name) license review.status"
    Assert-JsonInteger `
        -Value $reviewRecord.Value.missing_license_component_count `
        -Context "$($reviewRecord.Name) license review.missing_license_component_count" `
        -Minimum 0 `
        -Maximum 100000
}
foreach ($sbomRecord in @($appReceipt.sboms)) {
    Assert-JsonStringProperties `
        -Value $sbomRecord `
        -Properties @('product', 'file_name', 'sha256') `
        -Context 'application SBOM receipt'
    Assert-JsonString `
        -Value $sbomRecord.license_review.status `
        -Context "application SBOM $($sbomRecord.product).license_review.status"
    Assert-JsonInteger `
        -Value $sbomRecord.license_review.missing_license_component_count `
        -Context "application SBOM $($sbomRecord.product).missing_license_component_count" `
        -Minimum 0 `
        -Maximum 100000
}
if ([string]$developerReceipt.license_review.status -cne 'complete' -or
    [int]$developerReceipt.license_review.missing_license_component_count -ne 0) {
    throw 'Developer Kit license review is incomplete.'
}
if ([string]$codecReceipt.installer_license_review -cne 'complete') {
    throw 'H3 installer artifact license review is incomplete.'
}
if ([string]$appReceipt.license_review.status -cne 'complete' -or
    [int]$appReceipt.license_review.missing_license_component_count -ne 0 -or
    @($appReceipt.sboms | Where-Object {
        [string]$_.license_review.status -cne 'complete' -or
        [int]$_.license_review.missing_license_component_count -ne 0
    }).Count -gt 0) {
    throw 'Application artifact SBOM license review is incomplete.'
}

$applicationTrustSuffix = if ($releaseChannel -ceq 'unsigned_preview') { 'unsigned' } else { 'signed' }
$deckInstallerName = "LatentDeck-$releaseLabel-windows-x64-$applicationTrustSuffix-setup.exe"
$playerInstallerName = "LatentPlayer-$releaseLabel-windows-x64-$applicationTrustSuffix-setup.exe"
$deckSbomPath = "metadata/LatentDeck-App-$releaseLabel-sbom.cdx.json"
$playerSbomPath = "metadata/LatentPlayer-$releaseLabel-sbom.cdx.json"
$appExpected = @(
    'BUILD-COMMANDS.txt',
    'release-candidate.json',
    'SHA256SUMS.txt',
    $deckSbomPath,
    $playerSbomPath,
    'metadata/THIRD_PARTY_NOTICES.md',
    'metadata/THIRD_PARTY_LICENSES.json',
    "installers/$deckInstallerName",
    "installers/$playerInstallerName"
)
$packVersion = [string]$codecReceipt.pack_version
$codecExpected = @(
    'archive-runtime-smoke.json',
    'distributable-proof.json',
    'installed-runtime-smoke.json',
    'INSTALLER_NSIS_COPYING.txt',
    'INSTALLER_RUST_LICENSES.txt',
    'INSTALLER_THIRD_PARTY_NOTICES.md',
    'installer-SBOM.cdx.json',
    "LatentDeck-H3-CodecPack-$packVersion-setup.exe",
    "LatentDeck-H3-CodecPack-$packVersion-windows-x64.ldcodec",
    'package-receipt.json',
    'setup-receipt.json',
    'SHA256SUMS.txt'
)
$developerArchiveName = "LatentDeck-$releaseLabel-developer-kit-windows-x64.zip"
$developerExpected = @(
    $developerArchiveName,
    'developer-kit.json',
    'LICENSE-REVIEW.json',
    'SBOM.cdx.json',
    'SHA256SUMS.txt',
    'THIRD_PARTY_NOTICES.md',
    'THIRD_PARTY_LICENSES.json'
)
$recorderBaseName = "LatentDeck-$releaseLabel-comfy-recorder-windows-x64"
$recorderExpected = @(
    "$recorderBaseName.zip",
    "$recorderBaseName.receipt.json",
    "$recorderBaseName.SHA256SUMS.txt",
    "$recorderBaseName-sbom.cdx.json",
    "$recorderBaseName-THIRD-PARTY-NOTICES.md",
    "$recorderBaseName-THIRD-PARTY-LICENSES.json",
    "$recorderBaseName-license-review.json"
)
$expectedRecorderPackages = [ordered]@{
    'latentdeck-cartridge' = [ordered]@{
        Version = '0.1.0'
        FileName = 'latentdeck_cartridge-0.1.0-cp312-abi3-win_amd64.whl'
    }
    'latentdeck-comfy-cartridge' = [ordered]@{
        Version = '0.1.0'
        FileName = 'latentdeck_comfy_cartridge-0.1.0-py3-none-any.whl'
    }
    'safetensors' = [ordered]@{
        Version = '0.8.0'
        FileName = 'safetensors-0.8.0-cp310-abi3-win_amd64.whl'
    }
}
$recorderPackages = @($recorderReceipt.packages)
if ($recorderPackages.Count -ne 3 -or
    (@($recorderPackages.name | Sort-Object -CaseSensitive) -join "`0") -cne
        (@($expectedRecorderPackages.Keys | Sort-Object -CaseSensitive) -join "`0")) {
    throw 'Comfy Recorder receipt does not contain the exact three package identities.'
}
foreach ($package in $recorderPackages) {
    Assert-ExactJsonProperties `
        -Value $package `
        -Expected @('name', 'version', 'file_name', 'byte_length', 'sha256') `
        -Context "Comfy Recorder package $($package.name)"
    Assert-JsonStringProperties `
        -Value $package `
        -Properties @('name', 'version', 'file_name', 'sha256') `
        -Context 'Comfy Recorder package'
    Assert-JsonInteger `
        -Value $package.byte_length `
        -Context "Comfy Recorder package $($package.name).byte_length" `
        -Minimum 1 `
        -Maximum ($maximumGitHubAssetBytes - 1)
    $expectedPackage = $expectedRecorderPackages[[string]$package.name]
    if ([string]$package.version -cne [string]$expectedPackage.Version -or
        [string]$package.file_name -cne [string]$expectedPackage.FileName -or
        [int64]$package.byte_length -le 0 -or
        [string]$package.sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw "Comfy Recorder package identity is invalid: $($package.name)"
    }
}
Assert-ExactJsonProperties `
    -Value $recorderReceipt.archive `
    -Expected @('file_name', 'byte_length', 'sha256') `
    -Context 'Comfy Recorder archive binding'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.sbom `
    -Expected @(
        'file_name', 'byte_length', 'sha256', 'format', 'component_count',
        'selection_root_count', 'selection_roots', 'dependency_scope_counts',
        'safetensors_native_closure'
    ) `
    -Context 'Comfy Recorder SBOM binding'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.sbom.dependency_scope_counts `
    -Expected @('artifact', 'runtime', 'build', 'runtime+build') `
    -Context 'Comfy Recorder SBOM scope counts'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.third_party_notices `
    -Expected @('file_name', 'byte_length', 'sha256') `
    -Context 'Comfy Recorder notices binding'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.license_bundle `
    -Expected @(
        'file_name', 'byte_length', 'sha256', 'schema_version', 'component_count',
        'text_count', 'build_only_no_text_disposition_count'
    ) `
    -Context 'Comfy Recorder license bundle binding'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.license_review `
    -Expected @(
        'file_name', 'byte_length', 'sha256', 'status', 'missing_license_component_count'
    ) `
    -Context 'Comfy Recorder license review binding'
foreach ($stringBinding in @(
    [pscustomobject]@{
        Name = 'archive'; Value = $recorderReceipt.archive
        Properties = @('file_name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'SBOM'; Value = $recorderReceipt.sbom
        Properties = @('file_name', 'sha256', 'format')
    },
    [pscustomobject]@{
        Name = 'notices'; Value = $recorderReceipt.third_party_notices
        Properties = @('file_name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'license bundle'; Value = $recorderReceipt.license_bundle
        Properties = @('file_name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'license review'; Value = $recorderReceipt.license_review
        Properties = @('file_name', 'sha256', 'status')
    }
)) {
    Assert-JsonStringProperties `
        -Value $stringBinding.Value `
        -Properties $stringBinding.Properties `
        -Context "Comfy Recorder $($stringBinding.Name) binding"
}
foreach ($countRecord in @(
    [pscustomobject]@{ Name = 'sbom.component_count'; Value = $recorderReceipt.sbom.component_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'sbom.selection_root_count'; Value = $recorderReceipt.sbom.selection_root_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'license_bundle.schema_version'; Value = $recorderReceipt.license_bundle.schema_version; Minimum = 1 },
    [pscustomobject]@{ Name = 'license_bundle.component_count'; Value = $recorderReceipt.license_bundle.component_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'license_bundle.text_count'; Value = $recorderReceipt.license_bundle.text_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'license_bundle.build_only_no_text_disposition_count'; Value = $recorderReceipt.license_bundle.build_only_no_text_disposition_count; Minimum = 0 },
    [pscustomobject]@{ Name = 'license_review.missing_license_component_count'; Value = $recorderReceipt.license_review.missing_license_component_count; Minimum = 0 }
)) {
    Assert-JsonInteger `
        -Value $countRecord.Value `
        -Context "Comfy Recorder receipt.$($countRecord.Name)" `
        -Minimum ([int64]$countRecord.Minimum) `
        -Maximum 100000
}
if ([string]$recorderReceipt.archive.file_name -cne "$recorderBaseName.zip" -or
    [string]$recorderReceipt.sbom.file_name -cne "$recorderBaseName-sbom.cdx.json" -or
    [string]$recorderReceipt.sbom.format -cne 'CycloneDX-1.5' -or
    [int]$recorderReceipt.sbom.selection_root_count -ne 7 -or
    [string]$recorderReceipt.third_party_notices.file_name -cne
        "$recorderBaseName-THIRD-PARTY-NOTICES.md" -or
    [string]$recorderReceipt.license_bundle.file_name -cne
        "$recorderBaseName-THIRD-PARTY-LICENSES.json" -or
    [int]$recorderReceipt.license_bundle.schema_version -ne 1 -or
    [string]$recorderReceipt.license_review.file_name -cne
        "$recorderBaseName-license-review.json" -or
    [string]$recorderReceipt.license_review.status -cne 'complete' -or
    [int]$recorderReceipt.license_review.missing_license_component_count -ne 0) {
    throw 'Comfy Recorder release sidecar identity or license review is invalid.'
}

$applications = @($appReceipt.applications)
$sboms = @($appReceipt.sboms)
$deckApplications = @($applications | Where-Object {
    [string]$_.file_name -ceq $deckInstallerName -and [string]$_.product -ceq 'LatentDeck App'
})
$playerApplications = @($applications | Where-Object {
    [string]$_.file_name -ceq $playerInstallerName -and [string]$_.product -ceq 'LatentPlayer'
})
$deckSboms = @($sboms | Where-Object {
    [string]$_.file_name -ceq $deckSbomPath -and [string]$_.product -ceq 'LatentDeck App'
})
$playerSboms = @($sboms | Where-Object {
    [string]$_.file_name -ceq $playerSbomPath -and [string]$_.product -ceq 'LatentPlayer'
})
foreach ($fileBinding in @(
    [pscustomobject]@{
        Name = 'H3 archive'; Value = $codecReceipt.archive
        Properties = @('name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'H3 setup'; Value = $codecReceipt.setup
        Properties = @(
            'name', 'sha256', 'payload_delivery', 'native_helper_lifecycle_smoke',
            'windows_setup_lifecycle'
        )
    },
    [pscustomobject]@{
        Name = 'Developer Kit archive'; Value = $developerReceipt.archive
        Properties = @('name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'Developer Kit SBOM'; Value = $developerReceipt.sbom
        Properties = @('name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'Developer Kit notices'; Value = $developerReceipt.notices
        Properties = @('name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'Developer Kit license review'; Value = $developerReceipt.license_review
        Properties = @('name', 'sha256', 'status')
    },
    [pscustomobject]@{
        Name = 'Developer Kit license bundle'; Value = $developerReceipt.license_bundle
        Properties = @('name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'application license bundle'; Value = $appReceipt.license_bundle
        Properties = @('file_name', 'sha256')
    }
)) {
    Assert-JsonStringProperties `
        -Value $fileBinding.Value `
        -Properties $fileBinding.Properties `
        -Context $fileBinding.Name
}
if ($applications.Count -ne 2 -or $sboms.Count -ne 2 -or
    $deckApplications.Count -ne 1 -or $playerApplications.Count -ne 1 -or
    $deckSboms.Count -ne 1 -or $playerSboms.Count -ne 1 -or
    [string]$codecReceipt.archive.name -cne "LatentDeck-H3-CodecPack-$packVersion-windows-x64.ldcodec" -or
    [string]$codecReceipt.setup.name -cne "LatentDeck-H3-CodecPack-$packVersion-setup.exe" -or
    [string]$developerReceipt.archive.name -cne $developerArchiveName -or
    [string]$developerReceipt.sbom.name -cne 'SBOM.cdx.json' -or
    [string]$developerReceipt.notices.name -cne 'THIRD_PARTY_NOTICES.md' -or
    [string]$developerReceipt.license_review.name -cne 'LICENSE-REVIEW.json' -or
    [string]$appReceipt.license_bundle.file_name -cne 'metadata/THIRD_PARTY_LICENSES.json' -or
    [string]$developerReceipt.license_bundle.name -cne 'THIRD_PARTY_LICENSES.json') {
    throw 'Release receipt file identities do not match the exact artifact allowlist.'
}

Assert-ExactFileSet -Root $appRoot -Expected $appExpected
Assert-ExactFileSet -Root $codecRoot -Expected $codecExpected
Assert-ExactFileSet -Root $developerRoot -Expected $developerExpected
Assert-ExactFileSet -Root $recorderRoot -Expected $recorderExpected
$appChecksums = Assert-ChecksumManifest `
    -Root $appRoot `
    -ManifestPath (Join-Path $appRoot 'SHA256SUMS.txt') `
    -ExpectedPaths @(
        "installers/$deckInstallerName", "installers/$playerInstallerName",
        $deckSbomPath, $playerSbomPath, 'metadata/THIRD_PARTY_NOTICES.md',
        'metadata/THIRD_PARTY_LICENSES.json'
    )
$codecChecksums = Assert-ChecksumManifest `
    -Root $codecRoot `
    -ManifestPath (Join-Path $codecRoot 'SHA256SUMS.txt') `
    -ExpectedPaths @(
        "LatentDeck-H3-CodecPack-$packVersion-windows-x64.ldcodec",
        "LatentDeck-H3-CodecPack-$packVersion-setup.exe",
        'installer-SBOM.cdx.json', 'INSTALLER_THIRD_PARTY_NOTICES.md',
        'INSTALLER_NSIS_COPYING.txt', 'INSTALLER_RUST_LICENSES.txt', 'setup-receipt.json'
    )
$developerChecksums = Assert-ChecksumManifest `
    -Root $developerRoot `
    -ManifestPath (Join-Path $developerRoot 'SHA256SUMS.txt') `
    -ExpectedPaths @(
        $developerArchiveName, 'SBOM.cdx.json', 'THIRD_PARTY_NOTICES.md',
        'LICENSE-REVIEW.json', 'THIRD_PARTY_LICENSES.json'
    )
$recorderChecksums = Assert-ChecksumManifest `
    -Root $recorderRoot `
    -ManifestPath (Join-Path $recorderRoot "$recorderBaseName.SHA256SUMS.txt") `
    -ExpectedPaths @(
        "$recorderBaseName.zip",
        "$recorderBaseName-sbom.cdx.json",
        "$recorderBaseName-THIRD-PARTY-NOTICES.md",
        "$recorderBaseName-THIRD-PARTY-LICENSES.json",
        "$recorderBaseName-license-review.json"
    )

$applicationLicenseBundle = Test-ReleaseLicenseBundle `
    -BundlePath (Join-Path $appRoot 'metadata/THIRD_PARTY_LICENSES.json') `
    -SbomPath @((Join-Path $appRoot $deckSbomPath), (Join-Path $appRoot $playerSbomPath)) `
    -ExpectedArtifactName 'LatentDeck Windows Applications' `
    -ExpectedArtifactVersion $releaseLabel
$developerLicenseBundle = Test-ReleaseLicenseBundle `
    -BundlePath (Join-Path $developerRoot 'THIRD_PARTY_LICENSES.json') `
    -SbomPath (Join-Path $developerRoot 'SBOM.cdx.json') `
    -ExpectedArtifactName 'LatentDeck Developer Kit' `
    -ExpectedArtifactVersion $releaseLabel
$recorderLicenseBundle = Test-ReleaseLicenseBundle `
    -BundlePath (Join-Path $recorderRoot "$recorderBaseName-THIRD-PARTY-LICENSES.json") `
    -SbomPath (Join-Path $recorderRoot "$recorderBaseName-sbom.cdx.json") `
    -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
    -ExpectedArtifactVersion $releaseLabel
foreach ($bundleCountRecord in @(
    [pscustomobject]@{ Name = 'application.component_count'; Value = $appReceipt.license_bundle.component_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'application.text_count'; Value = $appReceipt.license_bundle.text_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'application.build_only_no_text_disposition_count'; Value = $appReceipt.license_bundle.build_only_no_text_disposition_count; Minimum = 0 },
    [pscustomobject]@{ Name = 'Developer Kit.component_count'; Value = $developerReceipt.license_bundle.component_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'Developer Kit.text_count'; Value = $developerReceipt.license_bundle.text_count; Minimum = 1 },
    [pscustomobject]@{ Name = 'Developer Kit.build_only_no_text_disposition_count'; Value = $developerReceipt.license_bundle.build_only_no_text_disposition_count; Minimum = 0 }
)) {
    Assert-JsonInteger `
        -Value $bundleCountRecord.Value `
        -Context "license bundle $($bundleCountRecord.Name)" `
        -Minimum ([int64]$bundleCountRecord.Minimum) `
        -Maximum 100000
}
if ([int64]$applicationLicenseBundle.ByteLength -ne
        [int64]$appChecksums['metadata/THIRD_PARTY_LICENSES.json'].ByteLength -or
    [string]$applicationLicenseBundle.Sha256 -cne
        [string]$appChecksums['metadata/THIRD_PARTY_LICENSES.json'].Sha256 -or
    [int64]$applicationLicenseBundle.ByteLength -ne
        [int64]$appReceipt.license_bundle.byte_length -or
    [string]$applicationLicenseBundle.Sha256 -cne
        [string]$appReceipt.license_bundle.sha256 -or
    [int]$applicationLicenseBundle.ComponentCount -ne
        [int]$appReceipt.license_bundle.component_count -or
    [int64]$developerLicenseBundle.ByteLength -ne
        [int64]$developerChecksums['THIRD_PARTY_LICENSES.json'].ByteLength -or
    [string]$developerLicenseBundle.Sha256 -cne
        [string]$developerChecksums['THIRD_PARTY_LICENSES.json'].Sha256 -or
    [int64]$developerLicenseBundle.ByteLength -ne
        [int64]$developerReceipt.license_bundle.byte_length -or
    [string]$developerLicenseBundle.Sha256 -cne
        [string]$developerReceipt.license_bundle.sha256 -or
    [int]$developerLicenseBundle.ComponentCount -ne
        [int]$developerReceipt.license_bundle.component_count -or
    [int64]$recorderLicenseBundle.ByteLength -ne
        [int64]$recorderChecksums["$recorderBaseName-THIRD-PARTY-LICENSES.json"].ByteLength -or
    [string]$recorderLicenseBundle.Sha256 -cne
        [string]$recorderChecksums["$recorderBaseName-THIRD-PARTY-LICENSES.json"].Sha256 -or
    [int64]$recorderLicenseBundle.ByteLength -ne [int64]$recorderReceipt.license_bundle.byte_length -or
    [string]$recorderLicenseBundle.Sha256 -cne [string]$recorderReceipt.license_bundle.sha256 -or
    [int]$recorderLicenseBundle.ComponentCount -ne [int]$recorderReceipt.license_bundle.component_count -or
    [int]$recorderLicenseBundle.TextCount -ne [int]$recorderReceipt.license_bundle.text_count -or
    [int]$recorderLicenseBundle.NoTextDispositionCount -ne
        [int]$recorderReceipt.license_bundle.build_only_no_text_disposition_count) {
    throw 'Application, Developer Kit, or Recorder license bundle is not hash-bound to its receipt and SBOM closure.'
}

$recorderSbomRead = Read-JsonFile -Path (
    Join-Path $recorderRoot "$recorderBaseName-sbom.cdx.json"
)
$recorderLicenseReviewRead = Read-JsonFile -Path (
    Join-Path $recorderRoot "$recorderBaseName-license-review.json"
)
$recorderSbom = $recorderSbomRead.Value
$recorderNativeClosure = Test-SafetensorsNativeClosureEvidence `
    -Evidence $recorderReceipt.sbom.safetensors_native_closure `
    -SbomPath $recorderSbomRead.Integrity.Path
$recorderRootIdentities = @(
    foreach ($component in @($recorderSbom.components | Where-Object {
        @($_.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:selection-root' -and
            [string]$_.value -ceq 'true'
        }).Count -eq 1
    })) {
        $ecosystems = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:ecosystem'
        })
        if ($ecosystems.Count -ne 1) {
            throw "Comfy Recorder SBOM root ecosystem is ambiguous: $($component.'bom-ref')"
        }
        "$([string]$ecosystems[0].value):$($component.name)@$($component.version)"
    }
) | Sort-Object
$expectedRecorderRootIdentities = @(
    'python:latentdeck-cartridge@0.1.0',
    'python:latentdeck-comfy-cartridge@0.1.0',
    'python:maturin@1.15.0',
    'python:safetensors@0.8.0',
    'python:uv-build@0.12.7',
    'rust:latentdeck-cartridge-python@0.1.0',
    'rust:latentdeck-cartridge@0.1.0'
) | Sort-Object
$recorderInvalidComponents = @(
    @($recorderSbom.metadata.component) + @($recorderSbom.components) | Where-Object {
        -not (Test-ComponentHasUsableLicense -Component $_) -or
        @($_.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope' -and
            (Test-OrdinalStringInSet `
                -Value ([string]$_.value) `
                -Expected @('artifact', 'runtime', 'build', 'runtime+build'))
        }).Count -ne 1
    }
)
if ([int64]$recorderSbomRead.Integrity.ByteLength -ne
        [int64]$recorderChecksums["$recorderBaseName-sbom.cdx.json"].ByteLength -or
    [string]$recorderSbomRead.Integrity.Sha256 -cne
        [string]$recorderChecksums["$recorderBaseName-sbom.cdx.json"].Sha256 -or
    [int64]$recorderLicenseReviewRead.Integrity.ByteLength -ne
        [int64]$recorderChecksums["$recorderBaseName-license-review.json"].ByteLength -or
    [string]$recorderLicenseReviewRead.Integrity.Sha256 -cne
        [string]$recorderChecksums["$recorderBaseName-license-review.json"].Sha256 -or
    [string]$recorderSbom.bomFormat -cne 'CycloneDX' -or
    [string]$recorderSbom.specVersion -cne '1.5' -or
    [string]$recorderSbom.metadata.component.name -cne 'LatentDeck Comfy LC Recorder' -or
    [string]$recorderSbom.metadata.component.version -cne $releaseLabel -or
    [int]$recorderReceipt.sbom.component_count -ne @($recorderSbom.components).Count -or
    [int]$recorderNativeClosure.ComponentCount -ne 32 -or
    ($recorderRootIdentities -join "`0") -cne ($expectedRecorderRootIdentities -join "`0") -or
    (@($recorderReceipt.sbom.selection_roots | Sort-Object) -join "`0") -cne
        ($expectedRecorderRootIdentities -join "`0") -or
    $recorderInvalidComponents.Count -ne 0 -or
    [string]$recorderLicenseReviewRead.Value.status -cne 'complete' -or
    [int]$recorderLicenseReviewRead.Value.missing_license_component_count -ne 0 -or
    [int]$recorderLicenseReviewRead.Value.license_bundle.component_count -ne
        [int]$recorderReceipt.license_bundle.component_count -or
    [int]$recorderLicenseReviewRead.Value.license_bundle.text_count -ne
        [int]$recorderReceipt.license_bundle.text_count) {
    throw 'Comfy Recorder SBOM, license review, or exact selection-root closure is invalid.'
}

$installerSbomRead = Read-JsonFile -Path (Join-Path $codecRoot 'installer-SBOM.cdx.json')
if ([int64]$installerSbomRead.Integrity.ByteLength -ne
        [int64]$codecChecksums['installer-SBOM.cdx.json'].ByteLength -or
    [string]$installerSbomRead.Integrity.Sha256 -cne
        [string]$codecChecksums['installer-SBOM.cdx.json'].Sha256) {
    throw 'H3 installer SBOM differs from its checksum binding.'
}
$installerSbom = $installerSbomRead.Value
$installerSbomComponents = @($installerSbom.components)
$installerRootProperties = @($installerSbom.metadata.component.properties)
$installerInvalidScopes = @($installerSbomComponents | Where-Object {
    @($_.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:dependency-scope' -and
        (Test-OrdinalStringInSet `
            -Value ([string]$_.value) `
            -Expected @('artifact', 'runtime', 'build', 'runtime+build'))
    }).Count -ne 1
})
if ([string]$installerSbom.bomFormat -cne 'CycloneDX' -or
    [string]$installerSbom.specVersion -cne '1.5' -or
    [string]$installerSbom.metadata.component.name -cne 'LatentDeck H3 Codec Pack Setup' -or
    [string]$installerSbom.metadata.component.version -cne $packVersion -or
    -not (Test-ComponentHasUsableLicense -Component $installerSbom.metadata.component) -or
    $installerSbomComponents.Count -lt 2 -or
    @($installerSbomComponents | Where-Object {
        -not (Test-ComponentHasUsableLicense -Component $_)
    }).Count -ne 0 -or
    @($installerSbomComponents | Where-Object {
        [string]$_.'bom-ref' -ceq 'tool:nsis@3.11'
    }).Count -ne 1 -or
    @($installerSbomComponents | Where-Object {
        [string]$_.name -ceq 'latentdeck-codec-pack-installer'
    }).Count -ne 1 -or
    @($installerRootProperties | Where-Object {
        [string]$_.name -ceq 'latentdeck:dependency-scope' -and
        [string]$_.value -ceq 'artifact'
    }).Count -ne 1 -or
    @($installerRootProperties | Where-Object {
        [string]$_.name -ceq 'latentdeck:included-dependency-scopes' -and
        [string]$_.value -ceq 'artifact,runtime,build,runtime+build'
    }).Count -ne 1 -or
    @($installerRootProperties | Where-Object {
        [string]$_.name -ceq 'latentdeck:excluded-dependency-scopes' -and
        [string]$_.value -ceq 'development'
    }).Count -ne 1 -or
    @($installerRootProperties | Where-Object {
        [string]$_.name -ceq 'latentdeck:target-platform' -and
        [string]$_.value -ceq 'x86_64-pc-windows-msvc'
    }).Count -ne 1 -or
    $installerInvalidScopes.Count -ne 0) {
    throw 'H3 installer SBOM is out of scope or has incomplete license metadata.'
}

Assert-ExactJsonProperties `
    -Value $codecReceipt.sidecars `
    -Expected @(
        'archive-runtime-smoke.json',
        'installed-runtime-smoke.json',
        'package-receipt.json',
        'setup-receipt.json'
    ) `
    -Context 'H3 distributable proof sidecars'
$archiveSmokeRead = Read-BoundJsonSidecar `
    -Root $codecRoot `
    -SidecarRecords $codecReceipt.sidecars `
    -Name 'archive-runtime-smoke.json'
$installedSmokeRead = Read-BoundJsonSidecar `
    -Root $codecRoot `
    -SidecarRecords $codecReceipt.sidecars `
    -Name 'installed-runtime-smoke.json'
$packageReceiptRead = Read-BoundJsonSidecar `
    -Root $codecRoot `
    -SidecarRecords $codecReceipt.sidecars `
    -Name 'package-receipt.json'
$setupReceiptRead = Read-BoundJsonSidecar `
    -Root $codecRoot `
    -SidecarRecords $codecReceipt.sidecars `
    -Name 'setup-receipt.json'

Assert-H3RuntimeSmoke `
    -Smoke $archiveSmokeRead.Value `
    -PackVersion $packVersion `
    -Context 'H3 archive runtime smoke'
Assert-H3RuntimeSmoke `
    -Smoke $installedSmokeRead.Value `
    -PackVersion $packVersion `
    -Context 'H3 installed runtime smoke'

$packageReceipt = $packageReceiptRead.Value
Assert-ExactJsonProperties `
    -Value $packageReceipt `
    -Expected @(
        'schema_version', 'pack_id', 'pack_version', 'adapter_version', 'platform', 'archive',
        'contains_runtime', 'contains_adapter', 'dependency_inventory', 'sbom', 'native_rust',
        'external_decoder_selection_required', 'archive_digest_purpose',
        'publisher_signature', 'content_policy'
    ) `
    -Context 'H3 package receipt'
Assert-ExactJsonProperties `
    -Value $packageReceipt.content_policy `
    -Expected @(
        'model_weights_allowed', 'cartridges_allowed', 'forbidden_payload_scan',
        'semantic_source_review'
    ) `
    -Context 'H3 package receipt content policy'
Assert-ExactJsonProperties `
    -Value $packageReceipt.native_rust `
    -Expected @('sbom_path', 'sbom_sha256', 'license_bundle_path', 'license_bundle_sha256') `
    -Context 'H3 package receipt native Rust evidence'
Assert-JsonStringProperties `
    -Value $packageReceipt `
    -Properties @(
        'pack_id', 'pack_version', 'adapter_version', 'platform',
        'archive_digest_purpose', 'publisher_signature'
    ) `
    -Context 'H3 package receipt'
Assert-JsonStringProperties `
    -Value $packageReceipt.archive `
    -Properties @('name', 'sha256') `
    -Context 'H3 package receipt archive'
Assert-JsonStringProperties `
    -Value $packageReceipt.native_rust `
    -Properties @(
        'sbom_path', 'sbom_sha256', 'license_bundle_path', 'license_bundle_sha256'
    ) `
    -Context 'H3 package receipt native Rust evidence'
Assert-JsonStringProperties `
    -Value $packageReceipt.content_policy `
    -Properties @('forbidden_payload_scan', 'semantic_source_review') `
    -Context 'H3 package receipt content policy'
foreach ($booleanRecord in @(
    [pscustomobject]@{ Name = 'contains_runtime'; Value = $packageReceipt.contains_runtime },
    [pscustomobject]@{ Name = 'contains_adapter'; Value = $packageReceipt.contains_adapter },
    [pscustomobject]@{ Name = 'external_decoder_selection_required'; Value = $packageReceipt.external_decoder_selection_required },
    [pscustomobject]@{ Name = 'content_policy.model_weights_allowed'; Value = $packageReceipt.content_policy.model_weights_allowed },
    [pscustomobject]@{ Name = 'content_policy.cartridges_allowed'; Value = $packageReceipt.content_policy.cartridges_allowed }
)) {
    Assert-JsonBoolean `
        -Value $booleanRecord.Value `
        -Context "H3 package receipt.$($booleanRecord.Name)"
}
Assert-JsonInteger `
    -Value $packageReceipt.schema_version `
    -Context 'H3 package receipt.schema_version' `
    -Minimum 1
if ([int]$packageReceipt.schema_version -ne 1 -or
    [string]$packageReceipt.pack_id -cne 'org.latentdeck.h3' -or
    [string]$packageReceipt.pack_version -cne $packVersion -or
    [string]$packageReceipt.adapter_version -cne [string]$codecReceipt.adapter_version -or
    [string]$packageReceipt.platform -cne 'windows-x86_64' -or
    [string]$packageReceipt.archive.name -cne [string]$codecReceipt.archive.name -or
    [int64]$packageReceipt.archive.byte_length -ne [int64]$codecReceipt.archive.byte_length -or
    [string]$packageReceipt.archive.sha256 -cne [string]$codecReceipt.archive.sha256 -or
    [string]$packageReceipt.native_rust.sbom_path -cne 'NATIVE_RUST_SBOM.cdx.json' -or
    [string]$packageReceipt.native_rust.sbom_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    [string]$packageReceipt.native_rust.license_bundle_path -cne 'NATIVE_RUST_LICENSES.json' -or
    [string]$packageReceipt.native_rust.license_bundle_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    -not [bool]$packageReceipt.contains_runtime -or
    -not [bool]$packageReceipt.contains_adapter -or
    -not [bool]$packageReceipt.external_decoder_selection_required -or
    [string]$packageReceipt.archive_digest_purpose -cne 'transport_integrity_only' -or
    [string]$packageReceipt.publisher_signature -cne 'not_present_local_rc' -or
    [bool]$packageReceipt.content_policy.model_weights_allowed -or
    [bool]$packageReceipt.content_policy.cartridges_allowed -or
    [string]$packageReceipt.content_policy.forbidden_payload_scan -cne 'passed' -or
    [string]$packageReceipt.content_policy.semantic_source_review -cne 'passed') {
    throw 'H3 package receipt does not match the distributable proof and content policy.'
}

$setupReceipt = $setupReceiptRead.Value
Assert-ExactJsonProperties `
    -Value $setupReceipt `
    -Expected @(
        'schema_version', 'pack_id', 'pack_version', 'platform', 'setup', 'payload',
        'helper', 'sbom', 'notices', 'toolchain', 'source', 'lifecycle',
        'native_helper_lifecycle_smoke', 'windows_setup_lifecycle', 'signing',
        'publisher_signature'
    ) `
    -Context 'H3 setup receipt'
Assert-ExactJsonProperties `
    -Value $setupReceipt.source `
    -Expected @(
        'commit', 'branch', 'git_dirty', 'git_tree', 'public_snapshot_sha256',
        'public_snapshot_file_count'
    ) `
    -Context 'H3 setup receipt source'
Assert-ExactJsonProperties `
    -Value $setupReceipt.sbom `
    -Expected @(
        'name', 'byte_length', 'sha256', 'format', 'component_count',
        'license_review', 'missing_license_component_count'
    ) `
    -Context 'H3 setup receipt installer SBOM'
Assert-ExactJsonProperties `
    -Value $setupReceipt.signing `
    -Expected @(
        'mode', 'outer_setup_authenticode', 'embedded_uninstaller_finalize',
        'installed_uninstaller_authenticode'
    ) `
    -Context 'H3 setup receipt signing evidence'
Assert-JsonStringProperties `
    -Value $setupReceipt `
    -Properties @(
        'pack_id', 'pack_version', 'platform', 'native_helper_lifecycle_smoke',
        'windows_setup_lifecycle', 'publisher_signature'
    ) `
    -Context 'H3 setup receipt'
foreach ($setupStringBinding in @(
    [pscustomobject]@{
        Name = 'setup'; Value = $setupReceipt.setup
        Properties = @('name', 'sha256', 'payload_delivery')
    },
    [pscustomobject]@{
        Name = 'payload'; Value = $setupReceipt.payload
        Properties = @('name', 'sha256')
    },
    [pscustomobject]@{
        Name = 'SBOM'; Value = $setupReceipt.sbom
        Properties = @('name', 'sha256', 'format', 'license_review')
    },
    [pscustomobject]@{
        Name = 'notices'; Value = $setupReceipt.notices
        Properties = @(
            'name', 'sha256', 'nsis_copying_name', 'nsis_copying_sha256',
            'rust_licenses_name', 'rust_licenses_sha256'
        )
    },
    [pscustomobject]@{
        Name = 'helper'; Value = $setupReceipt.helper
        Properties = @('sha256', 'delivery')
    },
    [pscustomobject]@{
        Name = 'source'; Value = $setupReceipt.source
        Properties = @('commit', 'branch', 'git_tree', 'public_snapshot_sha256')
    },
    [pscustomobject]@{
        Name = 'signing'; Value = $setupReceipt.signing
        Properties = @(
            'mode', 'outer_setup_authenticode', 'embedded_uninstaller_finalize',
            'installed_uninstaller_authenticode'
        )
    }
)) {
    Assert-JsonStringProperties `
        -Value $setupStringBinding.Value `
        -Properties $setupStringBinding.Properties `
        -Context "H3 setup receipt $($setupStringBinding.Name)"
}
Assert-JsonBoolean `
    -Value $setupReceipt.source.git_dirty `
    -Context 'H3 setup receipt.source.git_dirty'
Assert-JsonInteger `
    -Value $setupReceipt.schema_version `
    -Context 'H3 setup receipt.schema_version' `
    -Minimum 1
Assert-JsonInteger `
    -Value $setupReceipt.source.public_snapshot_file_count `
    -Context 'H3 setup receipt.source.public_snapshot_file_count' `
    -Minimum 1
if ([int]$setupReceipt.schema_version -ne 1 -or
    [string]$setupReceipt.pack_id -cne 'org.latentdeck.h3' -or
    [string]$setupReceipt.pack_version -cne $packVersion -or
    [string]$setupReceipt.platform -cne 'windows-x86_64' -or
    [string]$setupReceipt.setup.name -cne [string]$codecReceipt.setup.name -or
    [int64]$setupReceipt.setup.byte_length -ne [int64]$codecReceipt.setup.byte_length -or
    [string]$setupReceipt.setup.sha256 -cne [string]$codecReceipt.setup.sha256 -or
    [string]$setupReceipt.setup.payload_delivery -cne 'adjacent_hash_bound_ldcodec' -or
    [string]$codecReceipt.setup.payload_delivery -cne 'adjacent_hash_bound_ldcodec' -or
    [string]$setupReceipt.payload.name -cne [string]$codecReceipt.archive.name -or
    [int64]$setupReceipt.payload.byte_length -ne [int64]$codecReceipt.archive.byte_length -or
    [string]$setupReceipt.payload.sha256 -cne [string]$codecReceipt.archive.sha256 -or
    [string]$setupReceipt.native_helper_lifecycle_smoke -cne 'passed' -or
    [string]$setupReceipt.windows_setup_lifecycle -cne
        [string]$codecReceipt.setup.windows_setup_lifecycle -or
    [string]$setupReceipt.source.commit -cne [string]$codecReceipt.source.commit -or
    [string]$setupReceipt.source.branch -cne [string]$codecReceipt.source.branch -or
    [bool]$setupReceipt.source.git_dirty -ne [bool]$codecReceipt.source.git_dirty -or
    [string]$setupReceipt.source.git_tree -cne [string]$codecReceipt.source.git_tree -or
    [string]$setupReceipt.source.git_tree -cnotmatch '^[0-9a-f]{40}$' -or
    [string]$setupReceipt.source.public_snapshot_sha256 -cne
        [string]$codecReceipt.source.public_snapshot_sha256 -or
    [string]$setupReceipt.source.public_snapshot_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
    [int64]$setupReceipt.source.public_snapshot_file_count -ne
        [int64]$codecReceipt.source.public_snapshot_file_count -or
    [int64]$setupReceipt.source.public_snapshot_file_count -le 0 -or
    [string]$setupReceipt.signing.mode -cne [string]$codecReceipt.signing.mode -or
    [string]$setupReceipt.signing.outer_setup_authenticode -cne
        [string]$codecReceipt.signing.outer_setup_authenticode -or
    [string]$setupReceipt.signing.installed_uninstaller_authenticode -cne
        [string]$codecReceipt.signing.installed_uninstaller_authenticode -or
    [string]$setupReceipt.publisher_signature -cne [string]$codecReceipt.publisher_signature) {
    throw 'H3 setup receipt does not match the distributable proof, payload, source, and signing evidence.'
}
if ([string]$setupReceipt.sbom.name -cne 'installer-SBOM.cdx.json' -or
    [int64]$setupReceipt.sbom.byte_length -ne [int64]$codecChecksums['installer-SBOM.cdx.json'].ByteLength -or
    [string]$setupReceipt.sbom.sha256 -cne [string]$codecChecksums['installer-SBOM.cdx.json'].Sha256 -or
    [int]$setupReceipt.sbom.component_count -ne $installerSbomComponents.Count -or
    [string]$setupReceipt.sbom.license_review -cne 'complete' -or
    [int]$setupReceipt.sbom.missing_license_component_count -ne 0 -or
    [string]$setupReceipt.notices.name -cne 'INSTALLER_THIRD_PARTY_NOTICES.md' -or
    [int64]$setupReceipt.notices.byte_length -ne [int64]$codecChecksums['INSTALLER_THIRD_PARTY_NOTICES.md'].ByteLength -or
    [string]$setupReceipt.notices.sha256 -cne [string]$codecChecksums['INSTALLER_THIRD_PARTY_NOTICES.md'].Sha256 -or
    [string]$setupReceipt.notices.nsis_copying_name -cne 'INSTALLER_NSIS_COPYING.txt' -or
    [int64]$setupReceipt.notices.nsis_copying_byte_length -ne [int64]$codecChecksums['INSTALLER_NSIS_COPYING.txt'].ByteLength -or
    [string]$setupReceipt.notices.nsis_copying_sha256 -cne [string]$codecChecksums['INSTALLER_NSIS_COPYING.txt'].Sha256 -or
    [string]$setupReceipt.notices.rust_licenses_name -cne 'INSTALLER_RUST_LICENSES.txt' -or
    [int64]$setupReceipt.notices.rust_licenses_byte_length -ne [int64]$codecChecksums['INSTALLER_RUST_LICENSES.txt'].ByteLength -or
    [string]$setupReceipt.notices.rust_licenses_sha256 -cne [string]$codecChecksums['INSTALLER_RUST_LICENSES.txt'].Sha256) {
    throw 'H3 setup receipt does not hash-bind its installer SBOM and notices.'
}
if ([int64]$setupReceiptRead.Integrity.ByteLength -ne
        [int64]$codecChecksums['setup-receipt.json'].ByteLength -or
    [string]$setupReceiptRead.Integrity.Sha256 -cne
        [string]$codecChecksums['setup-receipt.json'].Sha256) {
    throw 'H3 setup receipt differs between its sidecar and checksum bindings.'
}

$deckApplicationRecord = Assert-ReceiptFile -Root $appRoot -Receipt $deckApplications[0] -Path "installers/$deckInstallerName"
$playerApplicationRecord = Assert-ReceiptFile -Root $appRoot -Receipt $playerApplications[0] -Path "installers/$playerInstallerName"
$deckSbomRecord = Assert-ReceiptFile -Root $appRoot -Receipt $deckSboms[0] -Path $deckSbomPath
$playerSbomRecord = Assert-ReceiptFile -Root $appRoot -Receipt $playerSboms[0] -Path $playerSbomPath
$applicationLicenseBundleRecord = Assert-ReceiptFile `
    -Root $appRoot `
    -Receipt $appReceipt.license_bundle `
    -Path 'metadata/THIRD_PARTY_LICENSES.json'
$codecArchiveRecord = Assert-ReceiptFile -Root $codecRoot -Receipt $codecReceipt.archive -Path $codecReceipt.archive.name
$codecNativeSbomRead = Read-ZipJsonEntry `
    -ArchivePath $codecArchiveRecord.Path `
    -EntryName 'NATIVE_RUST_SBOM.cdx.json'
if ([int64]$codecNativeSbomRead.Integrity.ByteLength -le 0 -or
    [string]$codecNativeSbomRead.Integrity.Sha256 -cne
        [string]$packageReceipt.native_rust.sbom_sha256) {
    throw 'H3 native Rust SBOM differs between the .ldcodec payload and package receipt.'
}
$codecNativeClosure = Test-SafetensorsNativeClosureEvidence `
    -Evidence $codecReceipt.safetensors_native_closure `
    -Sbom $codecNativeSbomRead.Value
if (($codecReceipt.safetensors_native_closure | ConvertTo-Json -Compress -Depth 16) -cne
    ($recorderReceipt.sbom.safetensors_native_closure | ConvertTo-Json -Compress -Depth 16)) {
    throw 'H3 and Comfy Recorder do not bind the same reviewed Safetensors native closure.'
}
$codecSetupRecord = Assert-ReceiptFile -Root $codecRoot -Receipt $codecReceipt.setup -Path $codecReceipt.setup.name
$developerArchiveRecord = Assert-ReceiptFile -Root $developerRoot -Receipt $developerReceipt.archive -Path $developerReceipt.archive.name
$developerSbomRecord = Assert-ReceiptFile -Root $developerRoot -Receipt $developerReceipt.sbom -Path $developerReceipt.sbom.name
$developerNoticesRecord = Assert-ReceiptFile -Root $developerRoot -Receipt $developerReceipt.notices -Path $developerReceipt.notices.name
$developerLicenseRecord = Assert-ReceiptFile -Root $developerRoot -Receipt $developerReceipt.license_review -Path $developerReceipt.license_review.name
$developerLicenseBundleRecord = Assert-ReceiptFile `
    -Root $developerRoot `
    -Receipt $developerReceipt.license_bundle `
    -Path 'THIRD_PARTY_LICENSES.json'
$recorderArchiveRecord = Assert-ReceiptFile `
    -Root $recorderRoot `
    -Receipt $recorderReceipt.archive `
    -Path "$recorderBaseName.zip"
$recorderSbomRecord = Assert-ReceiptFile `
    -Root $recorderRoot `
    -Receipt $recorderReceipt.sbom `
    -Path "$recorderBaseName-sbom.cdx.json"
$recorderNoticesRecord = Assert-ReceiptFile `
    -Root $recorderRoot `
    -Receipt $recorderReceipt.third_party_notices `
    -Path "$recorderBaseName-THIRD-PARTY-NOTICES.md"
$recorderLicenseBundleRecord = Assert-ReceiptFile `
    -Root $recorderRoot `
    -Receipt $recorderReceipt.license_bundle `
    -Path "$recorderBaseName-THIRD-PARTY-LICENSES.json"
$recorderLicenseReviewRecord = Assert-ReceiptFile `
    -Root $recorderRoot `
    -Receipt $recorderReceipt.license_review `
    -Path "$recorderBaseName-license-review.json"
$recorderChecksumRecord = Get-FileIntegrityRecord -Path (
    Join-Path $recorderRoot "$recorderBaseName.SHA256SUMS.txt"
)
$codecPackageReceiptRecord = $packageReceiptRead.Integrity
$codecSetupReceiptRecord = $setupReceiptRead.Integrity

Assert-ExactJsonProperties `
    -Value $developerReceipt.comfy_recorder_bundle `
    -Expected @(
        'artifact_kind', 'archive', 'standalone_receipt', 'python_abi',
        'supported_python', 'packages'
    ) `
    -Context 'Developer Kit nested Comfy Recorder binding'
Assert-ExactJsonProperties `
    -Value $developerReceipt.comfy_recorder_bundle.archive `
    -Expected @('path', 'file_name', 'byte_length', 'sha256') `
    -Context 'Developer Kit nested Comfy Recorder archive'
Assert-ExactJsonProperties `
    -Value $developerReceipt.comfy_recorder_bundle.standalone_receipt `
    -Expected @('file_name', 'byte_length', 'sha256') `
    -Context 'Developer Kit nested Comfy Recorder receipt'
Assert-JsonArray `
    -Value $developerReceipt.comfy_recorder_bundle.supported_python `
    -Context 'Developer Kit nested Comfy Recorder supported_python'
Assert-JsonArray `
    -Value $developerReceipt.comfy_recorder_bundle.packages `
    -Context 'Developer Kit nested Comfy Recorder packages'
Assert-JsonStringProperties `
    -Value $developerReceipt.comfy_recorder_bundle `
    -Properties @('artifact_kind', 'python_abi') `
    -Context 'Developer Kit nested Comfy Recorder binding'
Assert-JsonStringProperties `
    -Value $developerReceipt.comfy_recorder_bundle.archive `
    -Properties @('path', 'file_name', 'sha256') `
    -Context 'Developer Kit nested Comfy Recorder archive'
Assert-JsonStringProperties `
    -Value $developerReceipt.comfy_recorder_bundle.standalone_receipt `
    -Properties @('file_name', 'sha256') `
    -Context 'Developer Kit nested Comfy Recorder receipt'
foreach ($nestedPythonAbi in @($developerReceipt.comfy_recorder_bundle.supported_python)) {
    Assert-JsonString `
        -Value $nestedPythonAbi `
        -Context 'Developer Kit nested Comfy Recorder supported Python ABI'
}
$nestedRecorderPackages = @($developerReceipt.comfy_recorder_bundle.packages)
foreach ($candidate in $nestedRecorderPackages) {
    Assert-JsonStringProperties `
        -Value $candidate `
        -Properties @('name', 'version', 'file_name', 'sha256') `
        -Context 'Developer Kit nested Comfy Recorder package'
    Assert-JsonInteger `
        -Value $candidate.byte_length `
        -Context "Developer Kit nested package $($candidate.name).byte_length" `
        -Minimum 1 `
        -Maximum ($maximumGitHubAssetBytes - 1)
}
if ([string]$developerReceipt.comfy_recorder_bundle.artifact_kind -cne
        'comfy_recorder_bundle' -or
    [string]$developerReceipt.comfy_recorder_bundle.archive.path -cne
        "bundles/$recorderBaseName.zip" -or
    [string]$developerReceipt.comfy_recorder_bundle.archive.file_name -cne
        "$recorderBaseName.zip" -or
    [int64]$developerReceipt.comfy_recorder_bundle.archive.byte_length -ne
        $recorderArchiveRecord.ByteLength -or
    [string]$developerReceipt.comfy_recorder_bundle.archive.sha256 -cne
        $recorderArchiveRecord.Sha256 -or
    [string]$developerReceipt.comfy_recorder_bundle.standalone_receipt.file_name -cne
        "$recorderBaseName.receipt.json" -or
    [int64]$developerReceipt.comfy_recorder_bundle.standalone_receipt.byte_length -ne
        $recorderReceiptRecord.ByteLength -or
    [string]$developerReceipt.comfy_recorder_bundle.standalone_receipt.sha256 -cne
        $recorderReceiptRecord.Sha256 -or
    [string]$developerReceipt.comfy_recorder_bundle.python_abi -cne 'cp312-abi3' -or
    (@($developerReceipt.comfy_recorder_bundle.supported_python) -join "`0") -cne
        (@('cp312', 'cp313') -join "`0") -or
    $nestedRecorderPackages.Count -ne 3) {
    throw 'Developer Kit does not bind the exact standalone Comfy Recorder artifact.'
}
foreach ($package in $recorderPackages) {
    $nestedMatches = @($nestedRecorderPackages | Where-Object {
        [string]$_.name -ceq [string]$package.name -and
        [string]$_.version -ceq [string]$package.version -and
        [string]$_.file_name -ceq [string]$package.file_name -and
        [int64]$_.byte_length -eq [int64]$package.byte_length -and
        [string]$_.sha256 -ceq [string]$package.sha256
    })
    if ($nestedMatches.Count -ne 1) {
        throw "Developer Kit nested Recorder package binding drifted: $($package.name)"
    }
}
$nestedRecorderRecord = Get-ZipEntryIntegrityRecord `
    -ArchivePath $developerArchiveRecord.Path `
    -EntryName "bundles/$recorderBaseName.zip"
if ($nestedRecorderRecord.ByteLength -ne $recorderArchiveRecord.ByteLength -or
    $nestedRecorderRecord.Sha256 -cne $recorderArchiveRecord.Sha256) {
    throw 'Developer Kit archive does not contain the exact standalone Comfy Recorder ZIP.'
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $artifactsRoot "github-release/$releaseLabel"
}
$finalDirectory = Assert-ArtifactPath -Path $OutputDirectory
if (Test-Path -LiteralPath $finalDirectory) {
    throw "Refusing to overwrite an existing GitHub Release staging directory: $finalDirectory"
}
$stageRoot = Join-Path $artifactsRoot ".github-release-stage-$([guid]::NewGuid().ToString('N'))"
Assert-ArtifactPath -Path $stageRoot | Out-Null

try {
    [System.IO.Directory]::CreateDirectory($stageRoot) | Out-Null
    foreach ($inputRoot in @($appRoot, $codecRoot, $developerRoot, $recorderRoot)) {
        Assert-ArtifactPath -Path $inputRoot -RequireExistingDirectory | Out-Null
    }
    $workRoot = Join-Path $stageRoot '.bundle-work'
    [System.IO.Directory]::CreateDirectory($workRoot) | Out-Null
    $projectLicenseRecord = Get-FileIntegrityRecord -Path (Join-Path $repositoryRoot 'LICENSE')
    $artistStarterName =
        "LatentDeck-$releaseLabel-Artist-Starter-Windows-x64-$applicationTrustSuffix.zip"
    $evidenceName = "LatentDeck-$releaseLabel-Release-Evidence.zip"
    $outerChecksumName = "LatentDeck-$releaseLabel-SHA256SUMS.txt"

    $artistReadmePath = Join-Path $workRoot 'artist-README-FIRST.txt'
    $artistReadme = @"
LatentDeck $releaseLabel - Artist Starter for Windows x64
=========================================================

UNSIGNED PREVIEW
These installers are not Authenticode-signed. Verify the outer release ZIP
against $outerChecksumName before extracting it, and verify this directory
against SHA256SUMS.txt before running an installer.

START HERE
1. Extract this entire ZIP to a new directory. Do not run setup from inside
   the ZIP viewer.
2. Install either or both applications:
   - Installers/$playerInstallerName plays one .lc cartridge.
   - Installers/$deckInstallerName organizes cartridges and runs D2/Q4 latent
     synthesis.
3. Keep both files in H3-Codec in that same directory, then run:
   H3-Codec/$([string]$codecReceipt.setup.name)
4. In each application, open Extensions, refresh, enable H3 Codec Pack
   $packVersion, and select it explicitly.
5. The bundle does not contain decoder weights. Select the exact TAEH3 decoder
   declared by H3: 22,709,752 bytes, SHA-256
   4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13
   Source: https://raw.githubusercontent.com/madebyollin/taehv/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/safetensors/taeh3.safetensors
6. Download demo cartridges from the pinned CC BY 4.0 pack:
   https://huggingface.co/datasets/f00br/latentdeck-demo-lc-pack/tree/0e7b98f7152607c2d1709a896f9173859886ad79

The Artist Starter contains no decoder weight, cartridge, generator, ComfyUI,
or source dependency. Use the separate ComfyUI Recorder ZIP to create .lc
cartridges and the separate Developer Kit ZIP to build extensions.
"@
    Write-Utf8Text -Path $artistReadmePath -Text ($artistReadme.Replace("`r`n", "`n"))
    $artistReadmeRecord = Get-FileIntegrityRecord -Path $artistReadmePath
    $artistMappings = @(
        [pscustomobject]@{ EntryName = 'README-FIRST.txt'; Record = $artistReadmeRecord },
        [pscustomobject]@{ EntryName = 'LICENSE.txt'; Record = $projectLicenseRecord },
        [pscustomobject]@{ EntryName = "Installers/$deckInstallerName"; Record = $deckApplicationRecord },
        [pscustomobject]@{ EntryName = "Installers/$playerInstallerName"; Record = $playerApplicationRecord },
        [pscustomobject]@{ EntryName = "H3-Codec/$([string]$codecReceipt.setup.name)"; Record = $codecSetupRecord },
        [pscustomobject]@{ EntryName = "H3-Codec/$([string]$codecReceipt.archive.name)"; Record = $codecArchiveRecord },
        [pscustomobject]@{ EntryName = 'Licenses/Applications-THIRD-PARTY-NOTICES.md'; Record = $appChecksums['metadata/THIRD_PARTY_NOTICES.md'] },
        [pscustomobject]@{ EntryName = 'Licenses/Applications-THIRD-PARTY-LICENSES.json'; Record = $applicationLicenseBundleRecord },
        [pscustomobject]@{ EntryName = 'Licenses/H3-CodecPack-THIRD-PARTY-NOTICES.md'; Record = $codecChecksums['INSTALLER_THIRD_PARTY_NOTICES.md'] },
        [pscustomobject]@{ EntryName = 'Licenses/H3-CodecPack-NSIS-COPYING.txt'; Record = $codecChecksums['INSTALLER_NSIS_COPYING.txt'] },
        [pscustomobject]@{ EntryName = 'Licenses/H3-CodecPack-RUST-LICENSES.txt'; Record = $codecChecksums['INSTALLER_RUST_LICENSES.txt'] }
    )
    $artistChecksumPath = Join-Path $workRoot 'artist-SHA256SUMS.txt'
    Write-Utf8Text -Path $artistChecksumPath -Text (New-ChecksumText -Mappings $artistMappings)
    $artistMappings += [pscustomobject]@{
        EntryName = 'SHA256SUMS.txt'
        Record = Get-FileIntegrityRecord -Path $artistChecksumPath
    }
    $artistStarterRecord = New-DeterministicBoundZip `
        -Mappings $artistMappings `
        -DestinationPath (Join-Path $stageRoot $artistStarterName) `
        -CompressionLevel ([System.IO.Compression.CompressionLevel]::NoCompression)

    $recorderAsset = Copy-ReleaseAsset `
        -Source $recorderArchiveRecord.Path `
        -DestinationName "$recorderBaseName.zip" `
        -StageRoot $stageRoot `
        -ExpectedByteLength $recorderArchiveRecord.ByteLength `
        -ExpectedSha256 $recorderArchiveRecord.Sha256
    $developerAsset = Copy-ReleaseAsset `
        -Source $developerArchiveRecord.Path `
        -DestinationName $developerArchiveName `
        -StageRoot $stageRoot `
        -ExpectedByteLength $developerArchiveRecord.ByteLength `
        -ExpectedSha256 $developerArchiveRecord.Sha256

    $manifest = [ordered]@{
        schema_version = 2
        release_label = $releaseLabel
        release_channel = $releaseChannel
        prerelease = ($releaseChannel -ceq 'unsigned_preview')
        release_layout = 'five_file_user_first'
        outer_asset_count = 5
        source = [ordered]@{
            git_commit = $appCommit
            git_branch = 'main'
            git_tree = [string]$appReceipt.source.git_tree
            git_dirty = $false
            public_snapshot_sha256 = [string]$appReceipt.source.public_snapshot_sha256
            public_snapshot_file_count = [int64]$appReceipt.source.public_snapshot_file_count
        }
        identities = [ordered]@{
            application_api_version = '0.1.0'
            windows_installer_version = '0.1.0+1'
            h3_codec_pack_version = $packVersion
            h3_adapter_version = [string]$codecReceipt.adapter_version
            decks = $appReceipt.component_versions.decks
            sdks = $appReceipt.component_versions.sdks
            comfy_recorder = [ordered]@{
                python_abi = [string]$recorderReceipt.python_abi
                supported_python = @($recorderReceipt.supported_python)
                packages = @(
                    $recorderPackages |
                        Sort-Object name |
                        ForEach-Object {
                            [ordered]@{ name = [string]$_.name; version = [string]$_.version }
                        }
                )
                archive_sha256 = $recorderArchiveRecord.Sha256
                developer_kit_nested_archive_sha256 = $nestedRecorderRecord.Sha256
            }
        }
        asset_limit_bytes_exclusive = $maximumGitHubAssetBytes
        evidence_bundle = [ordered]@{
            name = $evidenceName
            internal_checksum = 'EVIDENCE-SHA256SUMS.txt'
            manifest_integrity_scope = 'direct hashes for primary payloads; evidence bundle hash is in the outer checksum'
        }
        assets = @(
            [ordered]@{
                name = $artistStarterName
                display_label = 'For artists - Player, LatentDeck, and H3 Codec Pack'
                role = 'artist_starter'
                verification = [ordered]@{
                    method = 'manifest_sha256'
                    byte_length = [int64]$artistStarterRecord.ByteLength
                    sha256 = [string]$artistStarterRecord.Sha256
                }
            },
            [ordered]@{
                name = "$recorderBaseName.zip"
                display_label = 'For ComfyUI - LC Recorder for Windows x64'
                role = 'comfy_recorder'
                verification = [ordered]@{
                    method = 'manifest_sha256'
                    byte_length = [int64]$recorderAsset.byte_length
                    sha256 = [string]$recorderAsset.sha256
                }
            },
            [ordered]@{
                name = $developerArchiveName
                display_label = 'For developers - SDKs, examples, and tools'
                role = 'developer_kit'
                verification = [ordered]@{
                    method = 'manifest_sha256'
                    byte_length = [int64]$developerAsset.byte_length
                    sha256 = [string]$developerAsset.sha256
                }
            },
            [ordered]@{
                name = $evidenceName
                display_label = 'Verification - receipts, SBOMs, licenses, and manifests'
                role = 'release_evidence'
                verification = [ordered]@{ method = 'outer_sha256sums' }
            },
            [ordered]@{
                name = $outerChecksumName
                display_label = 'Verification - SHA-256 checksums'
                role = 'outer_checksums'
                verification = [ordered]@{ method = 'self_excluded_checksum_manifest' }
            }
        )
    }
    $manifestPath = Join-Path $workRoot 'RELEASE-MANIFEST.json'
    $manifestText = (($manifest | ConvertTo-Json -Depth 32) + "`n").Replace("`r`n", "`n")
    Write-Utf8Text -Path $manifestPath -Text $manifestText

    $evidenceReadmePath = Join-Path $workRoot 'evidence-README.txt'
    $evidenceReadme = @"
LatentDeck $releaseLabel - Release Evidence
============================================

This archive contains the complete receipts, input checksum manifests,
artifact-scoped SBOMs, license evidence, notices, and runtime smoke records
validated before the five-file GitHub release set was staged.

RELEASE-MANIFEST.json records the exact source identity, component versions,
five uploaded filenames, recommended GitHub display labels, and direct hashes
for the three primary payload archives. EVIDENCE-SHA256SUMS.txt verifies every
other file in this archive. The outer $outerChecksumName verifies this evidence
archive together with the three primary payload archives; like conventional
checksum manifests, it does not list its own hash.
"@
    Write-Utf8Text -Path $evidenceReadmePath -Text ($evidenceReadme.Replace("`r`n", "`n"))
    $appInputChecksumRecord = Get-FileIntegrityRecord -Path (Join-Path $appRoot 'SHA256SUMS.txt')
    $appBuildCommandsRecord = Get-FileIntegrityRecord -Path (Join-Path $appRoot 'BUILD-COMMANDS.txt')
    $codecInputChecksumRecord = Get-FileIntegrityRecord -Path (Join-Path $codecRoot 'SHA256SUMS.txt')
    $developerInputChecksumRecord = Get-FileIntegrityRecord -Path (Join-Path $developerRoot 'SHA256SUMS.txt')
    $evidenceMappings = @(
        [pscustomobject]@{ EntryName = 'README.txt'; Record = Get-FileIntegrityRecord -Path $evidenceReadmePath },
        [pscustomobject]@{ EntryName = 'LICENSE.txt'; Record = $projectLicenseRecord },
        [pscustomobject]@{ EntryName = 'RELEASE-MANIFEST.json'; Record = Get-FileIntegrityRecord -Path $manifestPath },
        [pscustomobject]@{ EntryName = 'Applications/BUILD-COMMANDS.txt'; Record = $appBuildCommandsRecord },
        [pscustomobject]@{ EntryName = 'Applications/INPUT-SHA256SUMS.txt'; Record = $appInputChecksumRecord },
        [pscustomobject]@{ EntryName = 'Applications/RELEASE-RECEIPT.json'; Record = $appReceiptRecord },
        [pscustomobject]@{ EntryName = 'Applications/LatentDeck-App-SBOM.cdx.json'; Record = $deckSbomRecord },
        [pscustomobject]@{ EntryName = 'Applications/LatentPlayer-SBOM.cdx.json'; Record = $playerSbomRecord },
        [pscustomobject]@{ EntryName = 'Applications/THIRD-PARTY-NOTICES.md'; Record = $appChecksums['metadata/THIRD_PARTY_NOTICES.md'] },
        [pscustomobject]@{ EntryName = 'Applications/THIRD-PARTY-LICENSES.json'; Record = $applicationLicenseBundleRecord },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/ARCHIVE-RUNTIME-SMOKE.json'; Record = $archiveSmokeRead.Integrity },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/DISTRIBUTABLE-PROOF.json'; Record = $codecReceiptRecord },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/INSTALLED-RUNTIME-SMOKE.json'; Record = $installedSmokeRead.Integrity },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/INPUT-SHA256SUMS.txt'; Record = $codecInputChecksumRecord },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/INSTALLER-SBOM.cdx.json'; Record = $codecChecksums['installer-SBOM.cdx.json'] },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/INSTALLER-THIRD-PARTY-NOTICES.md'; Record = $codecChecksums['INSTALLER_THIRD_PARTY_NOTICES.md'] },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/INSTALLER-NSIS-COPYING.txt'; Record = $codecChecksums['INSTALLER_NSIS_COPYING.txt'] },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/INSTALLER-RUST-LICENSES.txt'; Record = $codecChecksums['INSTALLER_RUST_LICENSES.txt'] },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/PACKAGE-RECEIPT.json'; Record = $codecPackageReceiptRecord },
        [pscustomobject]@{ EntryName = 'H3-CodecPack/SETUP-RECEIPT.json'; Record = $codecSetupReceiptRecord },
        [pscustomobject]@{ EntryName = 'Developer-Kit/INPUT-SHA256SUMS.txt'; Record = $developerInputChecksumRecord },
        [pscustomobject]@{ EntryName = 'Developer-Kit/LICENSE-REVIEW.json'; Record = $developerLicenseRecord },
        [pscustomobject]@{ EntryName = 'Developer-Kit/RECEIPT.json'; Record = $developerReceiptRecord },
        [pscustomobject]@{ EntryName = 'Developer-Kit/SBOM.cdx.json'; Record = $developerSbomRecord },
        [pscustomobject]@{ EntryName = 'Developer-Kit/THIRD-PARTY-NOTICES.md'; Record = $developerNoticesRecord },
        [pscustomobject]@{ EntryName = 'Developer-Kit/THIRD-PARTY-LICENSES.json'; Record = $developerLicenseBundleRecord },
        [pscustomobject]@{ EntryName = 'Comfy-Recorder/INPUT-SHA256SUMS.txt'; Record = $recorderChecksumRecord },
        [pscustomobject]@{ EntryName = 'Comfy-Recorder/LICENSE-REVIEW.json'; Record = $recorderLicenseReviewRecord },
        [pscustomobject]@{ EntryName = 'Comfy-Recorder/RECEIPT.json'; Record = $recorderReceiptRecord },
        [pscustomobject]@{ EntryName = 'Comfy-Recorder/SBOM.cdx.json'; Record = $recorderSbomRecord },
        [pscustomobject]@{ EntryName = 'Comfy-Recorder/THIRD-PARTY-NOTICES.md'; Record = $recorderNoticesRecord },
        [pscustomobject]@{ EntryName = 'Comfy-Recorder/THIRD-PARTY-LICENSES.json'; Record = $recorderLicenseBundleRecord }
    )
    $evidenceChecksumPath = Join-Path $workRoot 'EVIDENCE-SHA256SUMS.txt'
    Write-Utf8Text -Path $evidenceChecksumPath -Text (New-ChecksumText -Mappings $evidenceMappings)
    $evidenceMappings += [pscustomobject]@{
        EntryName = 'EVIDENCE-SHA256SUMS.txt'
        Record = Get-FileIntegrityRecord -Path $evidenceChecksumPath
    }
    $evidenceRecord = New-DeterministicBoundZip `
        -Mappings $evidenceMappings `
        -DestinationPath (Join-Path $stageRoot $evidenceName) `
        -CompressionLevel ([System.IO.Compression.CompressionLevel]::Optimal)

    $outerMappings = @(
        [pscustomobject]@{ EntryName = $artistStarterName; Record = $artistStarterRecord },
        [pscustomobject]@{ EntryName = "$recorderBaseName.zip"; Record = [pscustomobject]@{ Path = Join-Path $stageRoot "$recorderBaseName.zip"; ByteLength = $recorderAsset.byte_length; Sha256 = $recorderAsset.sha256 } },
        [pscustomobject]@{ EntryName = $developerArchiveName; Record = [pscustomobject]@{ Path = Join-Path $stageRoot $developerArchiveName; ByteLength = $developerAsset.byte_length; Sha256 = $developerAsset.sha256 } },
        [pscustomobject]@{ EntryName = $evidenceName; Record = $evidenceRecord }
    )
    Write-Utf8Text `
        -Path (Join-Path $stageRoot $outerChecksumName) `
        -Text (New-ChecksumText -Mappings $outerMappings)
    Assert-ChecksumManifest `
        -Root $stageRoot `
        -ManifestPath (Join-Path $stageRoot $outerChecksumName) `
        -ExpectedPaths @($outerMappings.EntryName) | Out-Null

    $workRootResolved = [System.IO.Path]::GetFullPath($workRoot)
    if ((Split-Path -Parent $workRootResolved) -cne [System.IO.Path]::GetFullPath($stageRoot) -or
        (Split-Path -Leaf $workRootResolved) -cne '.bundle-work') {
        throw "Refusing to remove unsafe release bundle work directory: $workRootResolved"
    }
    Remove-Item -LiteralPath $workRootResolved -Recurse -Force

    $finalFiles = @(Get-ChildItem -LiteralPath $stageRoot -File -Force)
    $expectedFinalNames = @(
        $artistStarterName,
        "$recorderBaseName.zip",
        $developerArchiveName,
        $evidenceName,
        $outerChecksumName
    ) | Sort-Object -CaseSensitive
    if ($finalFiles.Count -ne 5 -or
        (@($finalFiles.Name | Sort-Object -CaseSensitive) -join "`0") -cne
            ($expectedFinalNames -join "`0") -or
        @($finalFiles | Where-Object { [int64]$_.Length -ge $maximumGitHubAssetBytes }).Count -gt 0) {
        throw 'GitHub Release final allowlist or per-file size limit failed.'
    }
    $finalParent = Split-Path -Parent $finalDirectory
    Assert-ArtifactPath -Path $finalParent -AllowArtifactsRoot | Out-Null
    [System.IO.Directory]::CreateDirectory($finalParent) | Out-Null
    Assert-ArtifactPath -Path $finalParent -AllowArtifactsRoot -RequireExistingDirectory | Out-Null
    if (Test-Path -LiteralPath $finalDirectory) {
        throw "GitHub Release destination appeared during staging: $finalDirectory"
    }
    [System.IO.Directory]::Move($stageRoot, $finalDirectory)
    $stageRoot = $null
    Write-Output $finalDirectory
} finally {
    if ($null -ne $stageRoot -and (Test-Path -LiteralPath $stageRoot)) {
        Assert-ArtifactPath -Path $stageRoot | Out-Null
        if (-not ([System.IO.Path]::GetFileName($stageRoot)).StartsWith(
            '.github-release-stage-',
            [System.StringComparison]::Ordinal
        )) {
            throw "Refusing to remove unsafe GitHub Release staging path: $stageRoot"
        }
        Remove-Item -LiteralPath $stageRoot -Recurse -Force
    }
}
