[CmdletBinding()]
param(
    [string]$OutputDirectory,

    [string]$SpoutArchivePath,

    [Parameter(Mandatory)]
    [string]$SbomPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'ReleaseSpoutMetadata.psm1') -Force

$releaseVersion = '0.1.0'
$targetTriple = 'x86_64-pc-windows-msvc'
$spoutMetadata = Get-Spout2ReleaseMetadata
$spoutTag = $spoutMetadata.Tag
$spoutCommit = $spoutMetadata.Commit
$spoutArchiveSha256 = $spoutMetadata.ArchiveSha256
$spoutArchiveBytes = [int64]$spoutMetadata.ArchiveBytes
$spoutArchiveUrl = $spoutMetadata.ArchiveUrl
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null

function Assert-ChildPath {
    param(
        [Parameter(Mandatory)]
        [string]$ParentPath,

        [Parameter(Mandatory)]
        [string]$CandidatePath,

        [switch]$AllowParent
    )

    $parent = [System.IO.Path]::GetFullPath($ParentPath).TrimEnd('\', '/')
    $candidate = [System.IO.Path]::GetFullPath($CandidatePath).TrimEnd('\', '/')
    if ($AllowParent -and $candidate.Equals($parent, [System.StringComparison]::OrdinalIgnoreCase)) {
        return $candidate
    }
    if (-not $candidate.StartsWith(
        $parent + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Release path is outside the ignored artifacts root: $candidate"
    }
    return $candidate
}

function Assert-TauriReleaseConfig {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$ProductName,

        [Parameter(Mandatory)]
        [string]$Identifier
    )

    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($config.productName -cne $ProductName -or
        $config.identifier -cne $Identifier -or
        $config.version -cne $releaseVersion) {
        throw "Tauri identity/version mismatch in $Path"
    }
    if ($config.bundle.active -ne $true -or
        (@($config.bundle.targets) -join ',') -cne 'nsis' -or
        $config.bundle.createUpdaterArtifacts -ne $false) {
        throw "Tauri bundle targets are not the local unsigned NSIS contract in $Path"
    }
    $resourceProperty = $config.bundle.PSObject.Properties['resources']
    $externalBinaryProperty = $config.bundle.PSObject.Properties['externalBin']
    if (($null -ne $resourceProperty -and $null -ne $resourceProperty.Value) -or
        ($null -ne $externalBinaryProperty -and $null -ne $externalBinaryProperty.Value)) {
        throw "Release installer must not bundle external resources or sidecar binaries: $Path"
    }
    if ($config.bundle.windows.allowDowngrades -ne $false -or
        $config.bundle.windows.webviewInstallMode.type -cne 'downloadBootstrapper' -or
        $config.bundle.windows.nsis.installMode -cne 'currentUser') {
        throw "Tauri Windows update/install policy mismatch in $Path"
    }
    return $config
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Command,

        [Parameter(Mandatory)]
        [string]$Description
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE."
    }
}

function Assert-PlausibleInstaller {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $item = Get-Item -LiteralPath $Path
    if ($item.Length -lt 128KB) {
        throw "NSIS installer is unexpectedly small: $($item.Name)"
    }
    $stream = [System.IO.File]::OpenRead($item.FullName)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) {
            throw "NSIS installer does not have a DOS/PE header: $($item.Name)"
        }
        $stream.Position = 0x3c
        $peOffset = [int64]$reader.ReadUInt32()
        if ($peOffset -lt 0x40 -or $peOffset -gt ($stream.Length - 26)) {
            throw "NSIS installer has an invalid PE offset: $($item.Name)"
        }
        $stream.Position = $peOffset
        $peSignature = $reader.ReadUInt32()
        $machine = $reader.ReadUInt16()
        $sectionCount = $reader.ReadUInt16()
        $stream.Position = $peOffset + 24
        $optionalHeaderMagic = $reader.ReadUInt16()
        if ($peSignature -ne 0x00004550 -or
            $machine -ne 0x014c -or
            $sectionCount -lt 1 -or
            $sectionCount -gt 96 -or
            $optionalHeaderMagic -ne 0x010b) {
            throw "NSIS installer is not the expected Windows PE32 bootstrapper: $($item.Name)"
        }
    } finally {
        $reader.Dispose()
    }
}

function Assert-NoDuplicateJsonProperties {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Element,

        [string]$Context = '$'
    )

    if ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Object) {
        $names = [System.Collections.Generic.HashSet[string]]::new(
            [System.StringComparer]::Ordinal
        )
        foreach ($property in $Element.EnumerateObject()) {
            if (-not $names.Add($property.Name)) {
                throw "Duplicate JSON property '$($property.Name)' at $Context."
            }
            Assert-NoDuplicateJsonProperties `
                -Element $property.Value `
                -Context "$Context.$($property.Name)"
        }
    } elseif ($Element.ValueKind -eq [System.Text.Json.JsonValueKind]::Array) {
        $index = 0
        foreach ($child in $Element.EnumerateArray()) {
            Assert-NoDuplicateJsonProperties -Element $child -Context "$Context[$index]"
            $index += 1
        }
    }
}

function Get-RequiredJsonPropertyElement {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($Object.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
        throw "$Context must be a JSON object."
    }
    foreach ($property in $Object.EnumerateObject()) {
        if ($property.Name -ceq $Name) {
            return $property.Value.Clone()
        }
    }
    throw "$Context is missing required field '$Name'."
}

function Assert-CycloneDxSbom {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or
        $item.Length -gt 20MB) {
        throw 'Release SBOM must be a bounded regular non-reparse file.'
    }
    $bytes = [System.IO.File]::ReadAllBytes($resolved)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw 'Release SBOM is not strict UTF-8.'
    }
    if ($text -match '(?i)\bfile:(?:/{1,3}|[A-Za-z]:[\\/])' -or
        $text -match '(?im)(?<![A-Za-z])[A-Za-z]:(?:\\\\|/)' -or
        $text -match '(?im)/(?:Users|home)/[^/\s]+/' -or
        $text -match '(?im)\\\\\\\\[^\\\s]+\\[^\\\s]+' -or
        $text -match '(?im)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----' -or
        $text -match '(?i)\bAKIA[0-9A-Z]{16}\b' -or
        $text -match '(?i)\bgh[pousr]_[A-Za-z0-9]{20,}\b' -or
        $text -match '(?i)\bsk-[A-Za-z0-9_-]{20,}\b' -or
        $text -match '(?im)\b(?:api[_-]?key|access[_-]?token|auth[_-]?token|secret|password)\b\s*[:=]\s*(?:"[^"\r\n]{8,}"|''[^''\r\n]{8,}'')') {
        throw 'Release SBOM contains a machine-local path, file URI, or credential-like material.'
    }

    $document = [System.Text.Json.JsonDocument]::Parse($text)
    try {
        Assert-NoDuplicateJsonProperties -Element $document.RootElement
        $root = $document.RootElement
        $bomFormat = Get-RequiredJsonPropertyElement -Object $root -Name 'bomFormat' -Context 'SBOM'
        $specVersion = Get-RequiredJsonPropertyElement -Object $root -Name 'specVersion' -Context 'SBOM'
        $documentVersion = Get-RequiredJsonPropertyElement -Object $root -Name 'version' -Context 'SBOM'
        $metadata = Get-RequiredJsonPropertyElement -Object $root -Name 'metadata' -Context 'SBOM'
        $component = Get-RequiredJsonPropertyElement -Object $metadata -Name 'component' -Context 'SBOM.metadata'
        $componentName = Get-RequiredJsonPropertyElement -Object $component -Name 'name' -Context 'SBOM.metadata.component'
        $componentVersion = Get-RequiredJsonPropertyElement -Object $component -Name 'version' -Context 'SBOM.metadata.component'
        $components = Get-RequiredJsonPropertyElement -Object $root -Name 'components' -Context 'SBOM'
        if ($bomFormat.ValueKind -ne [System.Text.Json.JsonValueKind]::String -or
            $bomFormat.GetString() -cne 'CycloneDX' -or
            $specVersion.ValueKind -ne [System.Text.Json.JsonValueKind]::String -or
            $specVersion.GetString() -cne '1.5' -or
            $documentVersion.ValueKind -ne [System.Text.Json.JsonValueKind]::Number -or
            $documentVersion.GetRawText() -cne '1' -or
            $componentName.ValueKind -ne [System.Text.Json.JsonValueKind]::String -or
            $componentName.GetString() -cne 'LatentDeck' -or
            $componentVersion.ValueKind -ne [System.Text.Json.JsonValueKind]::String -or
            $componentVersion.GetString() -cne $releaseVersion -or
            $components.ValueKind -ne [System.Text.Json.JsonValueKind]::Array) {
            throw 'Release SBOM is not the LatentDeck 0.1.0 CycloneDX 1.5 document.'
        }
        $componentCount = $components.GetArrayLength()
        if ($componentCount -eq 0 -or $componentCount -gt 100000) {
            throw 'Release SBOM component count is empty or unbounded.'
        }
        $index = 0
        foreach ($entry in $components.EnumerateArray()) {
            if ($entry.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
                throw "Release SBOM component at index $index is not a JSON object."
            }
            $index += 1
        }
        $decoded = $text | ConvertFrom-Json -Depth 100
        Assert-Spout2CycloneDxComponent -Components @($decoded.components) | Out-Null
    } finally {
        $document.Dispose()
    }
    return [pscustomobject]@{
        Path = $resolved
        ByteLength = [int64]$item.Length
        Sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
        ComponentCount = $componentCount
    }
}

function Get-ReleaseSourceSnapshot {
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot
    )

    $relativePaths = @(& git -C $RepositoryRoot -c core.quotepath=false ls-files --cached --others --exclude-standard)
    if ($LASTEXITCODE -ne 0 -or $relativePaths.Count -eq 0) {
        throw 'Could not enumerate the Git/public source snapshot.'
    }
    $records = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in $relativePaths) {
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            continue
        }
        $fullPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot $relativePath))
        Assert-ChildPath -ParentPath $RepositoryRoot -CandidatePath $fullPath | Out-Null
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            $records.Add("missing`0$relativePath")
            continue
        }
        $item = Get-Item -LiteralPath $fullPath -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Release source snapshot contains a reparse-point file: $relativePath"
        }
        $hash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $portablePath = $relativePath.Replace('\', '/')
        $records.Add("file`0$portablePath`0$($item.Length)`0$hash")
    }
    $payload = [System.Text.UTF8Encoding]::new($false).GetBytes(
        ($records | Sort-Object -CaseSensitive) -join "`n"
    )
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $snapshotHash = [System.Convert]::ToHexString(
            $hasher.ComputeHash($payload)
        ).ToLowerInvariant()
    } finally {
        $hasher.Dispose()
    }
    return [pscustomobject]@{
        Sha256 = $snapshotHash
        FileCount = $records.Count
    }
}

function New-PrivatePinnedSpout2Source {
    param(
        [Parameter(Mandatory)]
        [string]$ArchivePath,

        [Parameter(Mandatory)]
        [string]$DestinationRoot
    )

    if (Test-Path -LiteralPath $DestinationRoot) {
        throw "Private Spout2 destination already exists: $DestinationRoot"
    }
    Assert-ChildPath -ParentPath $buildRoot -CandidatePath $DestinationRoot | Out-Null

    $resolvedArchive = (Resolve-Path -LiteralPath $ArchivePath).Path
    $archiveItem = Get-Item -LiteralPath $resolvedArchive -Force
    if ($archiveItem.PSIsContainer -or
        ($archiveItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Pinned Spout2 archive must be a regular non-reparse file.'
    }

    $unpackRoot = Join-Path $buildRoot 'spout-unpack'
    if (Test-Path -LiteralPath $unpackRoot) {
        throw "Private Spout2 unpack root already exists: $unpackRoot"
    }
    [System.IO.Directory]::CreateDirectory($unpackRoot) | Out-Null
    $expectedPrefix = "Spout2-$spoutCommit/"
    $expectedExpandedRoot = Join-Path $unpackRoot "Spout2-$spoutCommit"
    $seenEntries = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $maxEntryBytes = [int64](32MB)
    $maxTotalBytes = [int64](128MB)
    $declaredTotalBytes = [int64]0
    $extractedTotalBytes = [int64]0

    $stream = [System.IO.File]::Open(
        $resolvedArchive,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::None
    )
    try {
        if ($stream.Length -ne $spoutArchiveBytes) {
            throw (
                "Spout2 archive byte length mismatch: expected $spoutArchiveBytes, " +
                "found $($stream.Length)."
            )
        }
        $hasher = [System.Security.Cryptography.SHA256]::Create()
        try {
            $archiveHash = [System.Convert]::ToHexString(
                $hasher.ComputeHash($stream)
            ).ToLowerInvariant()
        } finally {
            $hasher.Dispose()
        }
        if ($archiveHash -cne $spoutArchiveSha256) {
            throw (
                "Spout2 archive SHA-256 mismatch: expected $spoutArchiveSha256, " +
                "found $archiveHash."
            )
        }
        $stream.Position = 0
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Read,
            $true
        )
        try {
            if ($archive.Entries.Count -eq 0 -or $archive.Entries.Count -gt 10000) {
                throw 'Pinned Spout2 archive has an empty or unbounded entry table.'
            }
            foreach ($entry in $archive.Entries) {
                $entryName = $entry.FullName
                if ([string]::IsNullOrWhiteSpace($entryName) -or
                    $entryName.Contains('\') -or
                    $entryName.Contains(':') -or
                    -not $entryName.StartsWith(
                        $expectedPrefix,
                        [System.StringComparison]::Ordinal
                    ) -or
                    -not $seenEntries.Add($entryName)) {
                    throw "Pinned Spout2 archive has an unsafe or duplicate entry: $entryName"
                }
                $relativeName = $entryName.Substring($expectedPrefix.Length)
                $segments = @($relativeName.Split('/', [System.StringSplitOptions]::None))
                $isDirectory = $entryName.EndsWith('/', [System.StringComparison]::Ordinal)
                if ($relativeName.Length -eq 0) {
                    if (-not $isDirectory) {
                        throw "Pinned Spout2 archive has an invalid root entry: $entryName"
                    }
                    continue
                }
                $segmentLimit = if ($isDirectory) { $segments.Count - 1 } else { $segments.Count }
                for ($segmentIndex = 0; $segmentIndex -lt $segmentLimit; $segmentIndex += 1) {
                    if ([string]::IsNullOrEmpty($segments[$segmentIndex]) -or
                        $segments[$segmentIndex] -ceq '.' -or
                        $segments[$segmentIndex] -ceq '..') {
                        throw "Pinned Spout2 archive has an unsafe path segment: $entryName"
                    }
                }
                $unixFileType = ([uint32]$entry.ExternalAttributes -shr 16) -band 0xf000
                if ($unixFileType -eq 0xa000) {
                    throw "Pinned Spout2 archive contains a symbolic link: $entryName"
                }
                if ($entry.Length -lt 0 -or $entry.Length -gt $maxEntryBytes) {
                    throw "Pinned Spout2 archive entry exceeds the release bound: $entryName"
                }
                $declaredTotalBytes += [int64]$entry.Length
                if ($declaredTotalBytes -gt $maxTotalBytes) {
                    throw 'Pinned Spout2 archive exceeds the release extraction bound.'
                }

                $destination = Join-Path $unpackRoot $entryName.Replace('/', '\')
                Assert-ChildPath -ParentPath $unpackRoot -CandidatePath $destination | Out-Null
                if ($isDirectory) {
                    [System.IO.Directory]::CreateDirectory($destination) | Out-Null
                    continue
                }
                if (Test-Path -LiteralPath $destination) {
                    throw "Pinned Spout2 archive entry would overwrite a path: $entryName"
                }
                $parentDirectory = [System.IO.Path]::GetDirectoryName($destination)
                [System.IO.Directory]::CreateDirectory($parentDirectory) | Out-Null
                $entryStream = $entry.Open()
                $destinationStream = [System.IO.File]::Open(
                    $destination,
                    [System.IO.FileMode]::CreateNew,
                    [System.IO.FileAccess]::Write,
                    [System.IO.FileShare]::None
                )
                try {
                    $buffer = [byte[]]::new(64KB)
                    $entryBytes = [int64]0
                    while (($read = $entryStream.Read($buffer, 0, $buffer.Length)) -gt 0) {
                        $entryBytes += $read
                        $extractedTotalBytes += $read
                        if ($entryBytes -gt $entry.Length -or
                            $extractedTotalBytes -gt $maxTotalBytes) {
                            throw "Pinned Spout2 archive expanded past its declared bounds: $entryName"
                        }
                        $destinationStream.Write($buffer, 0, $read)
                    }
                    if ($entryBytes -ne $entry.Length) {
                        throw "Pinned Spout2 archive entry length mismatch: $entryName"
                    }
                } finally {
                    $destinationStream.Dispose()
                    $entryStream.Dispose()
                }
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $stream.Dispose()
    }

    if (-not (Test-Path -LiteralPath $expectedExpandedRoot -PathType Container)) {
        throw 'Pinned Spout2 archive did not contain its exact expected source root.'
    }
    [System.IO.Directory]::CreateDirectory($DestinationRoot) | Out-Null
    [System.IO.Directory]::Move(
        $expectedExpandedRoot,
        (Join-Path $DestinationRoot 'source')
    )
    $stamp = @(
        'schema=1'
        "tag=$spoutTag"
        "commit=$spoutCommit"
        "archive_sha256=$spoutArchiveSha256"
        "archive_bytes=$spoutArchiveBytes"
        'source_directory=source'
    ) -join "`n"
    $stamp += "`n"
    [System.IO.File]::WriteAllText(
        (Join-Path $DestinationRoot 'LATENTDECK_SPOUT2_SOURCE.txt'),
        $stamp,
        [System.Text.UTF8Encoding]::new($false)
    )
    return $DestinationRoot
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $artifactsRoot 'release-candidate'
}
$outputRoot = Assert-ChildPath `
    -ParentPath $artifactsRoot `
    -CandidatePath $OutputDirectory `
    -AllowParent
$finalDirectory = Join-Path $outputRoot "$releaseVersion-windows-x64"
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $finalDirectory | Out-Null
if (Test-Path -LiteralPath $finalDirectory) {
    throw "Refusing to overwrite an existing release-candidate directory: $finalDirectory"
}

Invoke-Checked `
    -Description 'Pre-build public-tree audit' `
    -Command { & pwsh -NoProfile -File (Join-Path $repoRoot 'tools/Test-PublicTree.ps1') }
$sbomInput = Assert-CycloneDxSbom -Path $SbomPath
$noticeInput = Test-Spout2ThirdPartyNotice `
    -Path (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md')
$sourceSnapshotBefore = Get-ReleaseSourceSnapshot -RepositoryRoot $repoRoot
$gitCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $gitCommit -cnotmatch '^[0-9a-f]{40}$') {
    throw 'Could not resolve the release source Git commit.'
}
$gitBranch = (& git -C $repoRoot branch --show-current).Trim()
if ($LASTEXITCODE -ne 0) {
    throw 'Could not resolve the release source Git branch.'
}
if ([string]::IsNullOrWhiteSpace($gitBranch)) {
    $gitBranch = '(detached)'
}
$gitStatusBefore = @(& git -C $repoRoot status --porcelain=v1 --untracked-files=all)
if ($LASTEXITCODE -ne 0) {
    throw 'Could not inspect the release source working-tree state.'
}

$deckRoot = Join-Path $repoRoot 'apps/latentdeck'
$playerRoot = Join-Path $repoRoot 'apps/latentplayer'
$cargoLockPath = Join-Path $repoRoot 'Cargo.lock'
$pnpmLockPath = Join-Path $repoRoot 'pnpm-lock.yaml'
foreach ($lockPath in @($cargoLockPath, $pnpmLockPath)) {
    if (-not (Test-Path -LiteralPath $lockPath -PathType Leaf)) {
        throw "Required release lock file is missing: $lockPath"
    }
}
$cargoLockHash = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$pnpmLockHash = (Get-FileHash -LiteralPath $pnpmLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$deckConfig = Assert-TauriReleaseConfig `
    -Path (Join-Path $deckRoot 'src-tauri/tauri.conf.json') `
    -ProductName 'LatentDeck App' `
    -Identifier 'studio.latentdeck.deck'
$playerConfig = Assert-TauriReleaseConfig `
    -Path (Join-Path $playerRoot 'src-tauri/tauri.conf.json') `
    -ProductName 'LatentPlayer' `
    -Identifier 'studio.latentdeck.player'
if ($deckConfig.identifier -ceq $playerConfig.identifier) {
    throw 'LatentDeck and LatentPlayer must keep independent Windows identities.'
}
foreach ($cargoManifest in @(
    (Join-Path $deckRoot 'src-tauri/Cargo.toml'),
    (Join-Path $playerRoot 'src-tauri/Cargo.toml')
)) {
    $cargoText = Get-Content -LiteralPath $cargoManifest -Raw
    if ($cargoText -cnotmatch '(?m)^spout-sdk\s*=') {
        throw "Release application has no explicit spout-sdk feature: $cargoManifest"
    }
}

$buildId = [guid]::NewGuid().ToString('N').Substring(0, 8)
$outputId = [guid]::NewGuid().ToString('N').Substring(0, 8)
$buildRoot = Join-Path $artifactsRoot ".rc-b-$buildId"
$outputStage = Join-Path $artifactsRoot ".rc-o-$outputId"
foreach ($temporary in @(
    @{ Path = $buildRoot; Prefix = '.rc-b-' },
    @{ Path = $outputStage; Prefix = '.rc-o-' }
)) {
    Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $temporary.Path | Out-Null
    if (-not ([System.IO.Path]::GetFileName($temporary.Path)).StartsWith(
        $temporary.Prefix,
        [System.StringComparison]::Ordinal
    )) {
        throw "Unsafe release staging directory: $($temporary.Path)"
    }
}

$previousCargoTarget = $env:CARGO_TARGET_DIR
$previousPath = $env:PATH
$previousSpoutSourceRoot = $env:LATENTDECK_SPOUT2_SOURCE_ROOT
try {
    [System.IO.Directory]::CreateDirectory($buildRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($outputStage) | Out-Null

    $nodeRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNode.ps1')
    $env:PATH = "$nodeRoot;$previousPath"
    $node = Join-Path $nodeRoot 'node.exe'
    $nodeVersion = (& $node --version).Trim()
    if ($nodeVersion -cne 'v24.20.0') {
        throw "Expected Node v24.20.0, found $nodeVersion"
    }
    $pnpm = Join-Path $nodeRoot 'pnpm.cmd'
    $pnpmVersion = (& $pnpm --version).Trim()
    if ($pnpmVersion -cne '11.24.0') {
        throw "Expected pnpm 11.24.0, found $pnpmVersion"
    }
    $rustcVerbose = (& rustc -Vv | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $rustcVerbose -cnotmatch '(?m)^rustc 1\.93\.1 ') {
        throw 'The pinned rustc 1.93.1 toolchain is unavailable.'
    }
    $cargoVerbose = (& cargo -Vv | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $cargoVerbose -cnotmatch '(?m)^cargo 1\.93\.1 ') {
        throw 'The pinned Cargo 1.93.1 toolchain is unavailable.'
    }

    Invoke-Checked `
        -Description 'Frozen workspace dependency install' `
        -Command { & $pnpm --dir $repoRoot install --frozen-lockfile }

    $tauriVersion = (& $pnpm --dir $deckRoot exec tauri --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $tauriVersion -cne 'tauri-cli 2.11.4') {
        throw "Expected tauri-cli 2.11.4, found $tauriVersion"
    }

    if ([string]::IsNullOrWhiteSpace($SpoutArchivePath)) {
        & (Join-Path $PSScriptRoot 'Prepare-Spout2.ps1') | Out-Null
        $resolvedSpoutArchive = Join-Path $repoRoot "vendor-local/spout2-$spoutCommit.zip"
        if (-not (Test-Path -LiteralPath $resolvedSpoutArchive -PathType Leaf)) {
            $resolvedSpoutArchive = Join-Path $buildRoot "spout2-$spoutCommit.zip"
            Invoke-WebRequest `
                -UseBasicParsing `
                -Uri $spoutArchiveUrl `
                -OutFile $resolvedSpoutArchive
        }
    } else {
        & (Join-Path $PSScriptRoot 'Prepare-Spout2.ps1') `
            -ArchivePath $SpoutArchivePath | Out-Null
        $resolvedSpoutArchive = (Resolve-Path -LiteralPath $SpoutArchivePath).Path
    }
    $privateSpoutRoot = New-PrivatePinnedSpout2Source `
        -ArchivePath $resolvedSpoutArchive `
        -DestinationRoot (Join-Path $buildRoot 's')
    $env:LATENTDECK_SPOUT2_SOURCE_ROOT = $privateSpoutRoot

    $env:CARGO_TARGET_DIR = Join-Path $buildRoot 't'
    $tauriArguments = @(
        'exec', 'tauri', 'build',
        '--ci',
        '--target', $targetTriple,
        '--bundles', 'nsis',
        '--features', 'spout-sdk',
        '--no-sign',
        '--', '--locked'
    )
    Invoke-Checked `
        -Description 'LatentDeck unsigned NSIS build' `
        -Command { & $pnpm --dir $deckRoot @tauriArguments }
    Invoke-Checked `
        -Description 'LatentPlayer unsigned NSIS build' `
        -Command { & $pnpm --dir $playerRoot @tauriArguments }

    if ((Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            $cargoLockHash -or
        (Get-FileHash -LiteralPath $pnpmLockPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            $pnpmLockHash) {
        throw 'A release lock file changed during the frozen/locked build.'
    }
    Invoke-Checked `
        -Description 'Post-build public-tree audit' `
        -Command { & pwsh -NoProfile -File (Join-Path $repoRoot 'tools/Test-PublicTree.ps1') }
    $sourceSnapshotAfter = Get-ReleaseSourceSnapshot -RepositoryRoot $repoRoot
    if ($sourceSnapshotAfter.Sha256 -cne $sourceSnapshotBefore.Sha256 -or
        $sourceSnapshotAfter.FileCount -ne $sourceSnapshotBefore.FileCount) {
        throw 'The public source snapshot changed while the release candidate was building.'
    }

    $nsisRoot = Join-Path $env:CARGO_TARGET_DIR "$targetTriple/release/bundle/nsis"
    $expectedSources = @(
        [ordered]@{
            product = 'LatentDeck App'
            identifier = 'studio.latentdeck.deck'
            source_name = 'LatentDeck App_0.1.0_x64-setup.exe'
            artifact_name = 'LatentDeck-0.1.0-windows-x64-unsigned-setup.exe'
        },
        [ordered]@{
            product = 'LatentPlayer'
            identifier = 'studio.latentdeck.player'
            source_name = 'LatentPlayer_0.1.0_x64-setup.exe'
            artifact_name = 'LatentPlayer-0.1.0-windows-x64-unsigned-setup.exe'
        }
    )

    $installerDirectory = Join-Path $outputStage 'installers'
    [System.IO.Directory]::CreateDirectory($installerDirectory) | Out-Null
    $receipts = @()
    foreach ($expected in $expectedSources) {
        $source = Join-Path $nsisRoot $expected.source_name
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            $actualNames = @(
                Get-ChildItem -LiteralPath $nsisRoot -File -ErrorAction SilentlyContinue |
                    Select-Object -ExpandProperty Name
            )
            throw (
                "Expected Tauri installer '$($expected.source_name)' was not produced. " +
                "Found: $($actualNames -join ', ')"
            )
        }
        Assert-PlausibleInstaller -Path $source
        $destination = Join-Path $installerDirectory $expected.artifact_name
        [System.IO.File]::Copy($source, $destination, $false)
        $item = Get-Item -LiteralPath $destination
        $receipts += [ordered]@{
            product = $expected.product
            identifier = $expected.identifier
            file_name = $item.Name
            byte_length = [int64]$item.Length
            sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            installer = 'nsis'
            install_mode = 'currentUser'
            unsigned = $true
            spout_sdk = $true
        }
    }

    $metadataDirectory = Join-Path $outputStage 'metadata'
    [System.IO.Directory]::CreateDirectory($metadataDirectory) | Out-Null
    $sbomFileName = "latentdeck-$releaseVersion-sbom.cdx.json"
    $sbomDestination = Join-Path $metadataDirectory $sbomFileName
    [System.IO.File]::Copy($sbomInput.Path, $sbomDestination, $false)
    $stagedSbom = Assert-CycloneDxSbom -Path $sbomDestination
    if ($stagedSbom.Sha256 -cne $sbomInput.Sha256 -or
        $stagedSbom.ByteLength -ne $sbomInput.ByteLength -or
        $stagedSbom.ComponentCount -ne $sbomInput.ComponentCount) {
        throw 'Staged SBOM does not match the explicitly supplied SBOM.'
    }
    $sbomReceipt = [ordered]@{
        file_name = "metadata/$sbomFileName"
        byte_length = $stagedSbom.ByteLength
        sha256 = $stagedSbom.Sha256
        format = 'CycloneDX'
        spec_version = '1.5'
        component_count = $stagedSbom.ComponentCount
    }

    $noticeFileName = 'THIRD_PARTY_NOTICES.md'
    $stagedNotice = Copy-Spout2ThirdPartyNotice `
        -SourcePath $noticeInput.Path `
        -DestinationDirectory $metadataDirectory
    $noticeDestination = $stagedNotice.Path
    if ($stagedNotice.Sha256 -cne $noticeInput.Sha256 -or
        $stagedNotice.ByteLength -ne $noticeInput.ByteLength) {
        throw 'Staged third-party notices do not match the reviewed source notice.'
    }
    $noticeReceipt = [ordered]@{
        file_name = "metadata/$noticeFileName"
        byte_length = $stagedNotice.ByteLength
        sha256 = $stagedNotice.Sha256
        components = @(
            [ordered]@{
                name = 'Spout2'
                version = $spoutTag
                commit = $spoutCommit
                license = $spoutMetadata.LicenseId
            }
        )
    }

    $manifestPath = Join-Path $outputStage 'release-candidate.json'
    $manifest = [ordered]@{
        schema_version = 3
        release_version = $releaseVersion
        target = $targetTriple
        local_release_candidate = $true
        signed = $false
        updater_artifacts = $false
        contains_codec_pack = $false
        contains_model_weights = $false
        contains_cartridges = $false
        source = [ordered]@{
            git_commit = $gitCommit
            git_branch = $gitBranch
            git_dirty = ($gitStatusBefore.Count -gt 0)
            git_dirty_entry_count = $gitStatusBefore.Count
            public_snapshot_sha256 = $sourceSnapshotBefore.Sha256
            public_snapshot_file_count = $sourceSnapshotBefore.FileCount
        }
        toolchain = [ordered]@{
            node = $nodeVersion
            pnpm = $pnpmVersion
            tauri_cli = $tauriVersion
            rustc_verbose = $rustcVerbose
            cargo_verbose = $cargoVerbose
            cargo_locked = $true
        }
        locks = [ordered]@{
            cargo_lock_sha256 = $cargoLockHash
            pnpm_lock_sha256 = $pnpmLockHash
        }
        spout2 = [ordered]@{
            tag = $spoutTag
            commit = $spoutCommit
            archive_sha256 = $spoutArchiveSha256
            feature = 'spout-sdk'
        }
        sbom = $sbomReceipt
        third_party_notices = @($noticeReceipt)
        applications = $receipts
    }
    [System.IO.File]::WriteAllText(
        $manifestPath,
        ($manifest | ConvertTo-Json -Depth 16) + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    $hashLines = @(
        foreach ($receipt in $receipts) {
            "$($receipt.sha256)  installers/$($receipt.file_name)"
        }
        "$($sbomReceipt.sha256)  $($sbomReceipt.file_name)"
        "$($noticeReceipt.sha256)  $($noticeReceipt.file_name)"
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $outputStage 'SHA256SUMS.txt'),
        ($hashLines -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $outputStage 'BUILD-COMMANDS.txt'),
        (@(
            'pwsh -NoProfile -File tools/Build-ReleaseCandidate.ps1 -SbomPath <validated CycloneDX input> [-SpoutArchivePath <optional exact pinned archive>]'
            'pnpm --dir . install --frozen-lockfile'
            'pwsh -NoProfile -File tools/Prepare-Spout2.ps1'
            'Build helper: exclusive verify/extract pinned Spout2 archive to private ignored staging and set LATENTDECK_SPOUT2_SOURCE_ROOT'
            'pnpm --dir apps/latentdeck exec tauri build --ci --target x86_64-pc-windows-msvc --bundles nsis --features spout-sdk --no-sign -- --locked'
            'pnpm --dir apps/latentplayer exec tauri build --ci --target x86_64-pc-windows-msvc --bundles nsis --features spout-sdk --no-sign -- --locked'
        ) -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    $stageFiles = @(Get-ChildItem -LiteralPath $outputStage -File -Force -Recurse)
    $expectedRelativeNames = @(
        'BUILD-COMMANDS.txt',
        'release-candidate.json',
        'SHA256SUMS.txt',
        'metadata/latentdeck-0.1.0-sbom.cdx.json',
        'metadata/THIRD_PARTY_NOTICES.md',
        'installers/LatentDeck-0.1.0-windows-x64-unsigned-setup.exe',
        'installers/LatentPlayer-0.1.0-windows-x64-unsigned-setup.exe'
    )
    $actualRelativeNames = @(
        $stageFiles |
            ForEach-Object {
                [System.IO.Path]::GetRelativePath($outputStage, $_.FullName).Replace('\', '/')
            } |
            Sort-Object
    )
    if (($actualRelativeNames -join "`0") -cne (($expectedRelativeNames | Sort-Object) -join "`0")) {
        throw 'Release-candidate staging contains an unexpected file set.'
    }
    foreach ($receipt in $receipts) {
        $path = Join-Path $installerDirectory $receipt.file_name
        $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($hash -cne $receipt.sha256) {
            throw "Staged installer hash changed: $($receipt.file_name)"
        }
    }
    $finalStagedSbomHash = (
        Get-FileHash -LiteralPath $sbomDestination -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    if ($finalStagedSbomHash -cne $sbomReceipt.sha256) {
        throw 'Staged SBOM hash changed before finalization.'
    }
    $finalStagedNoticeHash = (
        Get-FileHash -LiteralPath $noticeDestination -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    if ($finalStagedNoticeHash -cne $noticeReceipt.sha256) {
        throw 'Staged third-party notices changed before finalization.'
    }

    [System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
    if (Test-Path -LiteralPath $finalDirectory) {
        throw "Release-candidate destination appeared during build: $finalDirectory"
    }
    [System.IO.Directory]::Move($outputStage, $finalDirectory)
    $outputStage = $null
    Write-Output $finalDirectory
} finally {
    $env:CARGO_TARGET_DIR = $previousCargoTarget
    $env:PATH = $previousPath
    $env:LATENTDECK_SPOUT2_SOURCE_ROOT = $previousSpoutSourceRoot
    if (Test-Path -LiteralPath $buildRoot) {
        Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $buildRoot | Out-Null
        if (-not ([System.IO.Path]::GetFileName($buildRoot)).StartsWith(
            '.rc-b-',
            [System.StringComparison]::Ordinal
        )) {
            throw "Refusing to remove unsafe build staging path: $buildRoot"
        }
        Remove-Item -LiteralPath $buildRoot -Recurse -Force
    }
    if ($null -ne $outputStage -and (Test-Path -LiteralPath $outputStage)) {
        Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $outputStage | Out-Null
        if (-not ([System.IO.Path]::GetFileName($outputStage)).StartsWith(
            '.rc-o-',
            [System.StringComparison]::Ordinal
        )) {
            throw "Refusing to remove unsafe output staging path: $outputStage"
        }
        Remove-Item -LiteralPath $outputStage -Recurse -Force
    }
}
