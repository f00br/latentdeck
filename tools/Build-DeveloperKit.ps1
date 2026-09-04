[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ComfyRecorderArtifactDirectory,

    [string]$OutputDirectory,

    [ValidateSet('unsigned_preview', 'stable')]
    [string]$ReleaseChannel = 'unsigned_preview',

    [string]$ReleaseLabel,

    [switch]$DevelopmentBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

if ($ReleaseChannel -cnotin @('unsigned_preview', 'stable')) {
    throw 'ReleaseChannel must be exactly unsigned_preview or stable.'
}

Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'PublicWheelAudit.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repositoryRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null

if ([string]::IsNullOrWhiteSpace($ReleaseLabel)) {
    $ReleaseLabel = if ($ReleaseChannel -ceq 'unsigned_preview') {
        '0.1.0-preview.1'
    } else {
        '0.1.0'
    }
}
if ($ReleaseLabel -cnotmatch '^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$') {
    throw "ReleaseLabel is not canonical SemVer: $ReleaseLabel"
}
if ($ReleaseChannel -ceq 'unsigned_preview') {
    if ($ReleaseLabel -cne '0.1.0-preview.1') {
        throw 'The unsigned preview channel requires release label 0.1.0-preview.1.'
    }
} elseif ($ReleaseLabel -cne '0.1.0') {
    throw 'The stable channel requires release label 0.1.0.'
}

function Assert-ChildPath {
    param(
        [Parameter(Mandatory)][string]$ParentPath,
        [Parameter(Mandatory)][string]$CandidatePath,
        [switch]$AllowParent
    )

    $parent = [System.IO.Path]::GetFullPath($ParentPath).TrimEnd('\', '/')
    $candidate = [System.IO.Path]::GetFullPath($CandidatePath).TrimEnd('\', '/')
    if ($AllowParent -and $candidate.Equals(
        $parent,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        return $candidate
    }
    if (-not $candidate.StartsWith(
        $parent + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Developer Kit path is outside the ignored artifacts root: $candidate"
    }
    return $candidate
}

function Assert-PathComponentsNotReparsePoints {
    param([Parameter(Mandatory)][string]$Path)

    $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $volumeRoot = [System.IO.Path]::GetPathRoot($fullPath)
    if ([string]::IsNullOrWhiteSpace($volumeRoot)) {
        throw "Developer Kit path has no filesystem root: $fullPath"
    }
    $current = $volumeRoot.TrimEnd('\', '/')
    if ([string]::IsNullOrWhiteSpace($current)) {
        $current = $volumeRoot
    }
    foreach ($component in $fullPath.Substring($volumeRoot.Length).Split(
        @([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
        [System.StringSplitOptions]::RemoveEmptyEntries
    )) {
        $current = Join-Path $current $component
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Developer Kit path contains a reparse-point component: $current"
            }
        }
    }
}

function Read-BoundedJsonFile {
    param([Parameter(Mandatory)][string]$Path)

    Assert-PathComponentsNotReparsePoints -Path $Path
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 16MB) {
        throw "Developer Kit input JSON is not a bounded regular file: $Path"
    }
    $bytes = [System.IO.File]::ReadAllBytes($item.FullName)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
        $value = $text | ConvertFrom-Json -Depth 100
    } catch {
        throw "Developer Kit input JSON is not strict UTF-8 JSON: $Path"
    }
    return [pscustomobject]@{
        Value = $value
        Path = $item.FullName
        ByteLength = [int64]$bytes.Length
        Sha256 = [System.Convert]::ToHexString(
            [System.Security.Cryptography.SHA256]::HashData($bytes)
        ).ToLowerInvariant()
    }
}

function Assert-ExactJsonProperties {
    param(
        [Parameter(Mandatory)][object]$Value,
        [Parameter(Mandatory)][string[]]$Expected,
        [Parameter(Mandatory)][string]$Context
    )

    $actual = @($Value.PSObject.Properties.Name | Sort-Object)
    $expectedSorted = @($Expected | Sort-Object)
    if (($actual -join "`0") -cne ($expectedSorted -join "`0")) {
        throw "$Context does not have the exact supported property set."
    }
}

function Assert-ExactFlatArtifactSet {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string[]]$ExpectedNames
    )

    Assert-PathComponentsNotReparsePoints -Path $Root
    $rootItem = Get-Item -LiteralPath $Root -Force
    if (-not $rootItem.PSIsContainer -or
        ($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Developer Kit input artifact root is not a regular directory: $Root"
    }
    $entries = @(Get-ChildItem -LiteralPath $Root -Force -Recurse)
    foreach ($entry in $entries) {
        if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Developer Kit input artifact set contains a reparse point: $($entry.FullName)"
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
    $expected = @($ExpectedNames | Sort-Object)
    if (($actual -join "`0") -cne ($expected -join "`0")) {
        throw 'Comfy Recorder input does not match its exact seven-file artifact contract.'
    }
}

function Assert-BoundFlatFile {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][object]$Binding,
        [Parameter(Mandatory)][string]$ExpectedName,
        [int64]$MaximumBytes = 512MB
    )

    if ([string]$Binding.file_name -cne $ExpectedName -or
        [int64]$Binding.byte_length -le 0 -or
        [int64]$Binding.byte_length -gt $MaximumBytes -or
        [string]$Binding.sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [System.IO.Path]::GetFileName($ExpectedName) -cne $ExpectedName) {
        throw "Comfy Recorder file binding is invalid: $ExpectedName"
    }
    $path = Join-Path $Root $ExpectedName
    Assert-PathComponentsNotReparsePoints -Path $path
    $item = Get-Item -LiteralPath $path -Force
    $sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        [int64]$item.Length -ne [int64]$Binding.byte_length -or
        $sha256 -cne [string]$Binding.sha256) {
        throw "Comfy Recorder file differs from its receipt binding: $ExpectedName"
    }
    return [pscustomobject]@{
        Path = $item.FullName
        FileName = $item.Name
        ByteLength = [int64]$item.Length
        Sha256 = $sha256
    }
}

function Assert-ExactRecorderChecksums {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][object[]]$Records
    )

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt 1MB) {
        throw 'Comfy Recorder checksum manifest is not a bounded regular file.'
    }
    $bytes = [System.IO.File]::ReadAllBytes($item.FullName)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw 'Comfy Recorder checksum manifest is not strict UTF-8.'
    }
    if (-not $text.EndsWith("`n", [System.StringComparison]::Ordinal) -or
        $text.Contains("`r") -or $text.Contains("`0")) {
        throw 'Comfy Recorder checksum manifest is not canonical LF text.'
    }
    $expectedByName = @{}
    foreach ($record in $Records) {
        $expectedByName[[string]$record.FileName] = $record
    }
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($line in @($text.Split("`n") | Where-Object Length -gt 0)) {
        if ($line.Length -gt 4096 -or
            $line -cnotmatch '^(?<hash>[0-9a-f]{64})  (?<name>[^/\\\r\n]+)$' -or
            -not $seen.Add($Matches.name) -or
            -not $expectedByName.ContainsKey($Matches.name) -or
            [string]$expectedByName[$Matches.name].Sha256 -cne $Matches.hash) {
            throw 'Comfy Recorder checksum manifest has an unsafe, unexpected, or mismatched record.'
        }
    }
    if ($seen.Count -ne $expectedByName.Count) {
        throw 'Comfy Recorder checksum manifest does not have exact five-file coverage.'
    }
}

function Write-Utf8Text {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Text
    )

    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Write-JsonFile {
    param(
        [Parameter(Mandatory)][object]$Value,
        [Parameter(Mandatory)][string]$Path
    )

    Write-Utf8Text -Path $Path -Text (($Value | ConvertTo-Json -Depth 100) + "`n")
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory)][scriptblock]$Command,
        [Parameter(Mandatory)][string]$Context
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Context failed with exit code $LASTEXITCODE."
    }
}

function Get-ProjectIdentity {
    param([Parameter(Mandatory)][string]$ProjectPath)

    $manifestPath = Join-Path $repositoryRoot "$ProjectPath/pyproject.toml"
    $text = Get-Content -LiteralPath $manifestPath -Raw
    $nameMatches = [regex]::Matches($text, '(?m)^name\s*=\s*"(?<value>[^"]+)"\s*$')
    $versionMatches = [regex]::Matches($text, '(?m)^version\s*=\s*"(?<value>[^"]+)"\s*$')
    if ($nameMatches.Count -ne 1 -or $versionMatches.Count -ne 1) {
        throw "Project manifest must declare exactly one name and version: $manifestPath"
    }
    return [pscustomobject]@{
        Name = $nameMatches[0].Groups['value'].Value
        Version = $versionMatches[0].Groups['value'].Value
    }
}

function Get-ProjectBuildRequirement {
    param([Parameter(Mandatory)][string]$ProjectPath)

    $manifestPath = Join-Path $repositoryRoot "$ProjectPath/pyproject.toml"
    $text = Get-Content -LiteralPath $manifestPath -Raw
    $section = [regex]::Match(
        $text,
        '(?ms)^\[build-system\]\s*\r?\n(?<body>.*?)(?=^\[|\z)'
    )
    if (-not $section.Success) {
        throw "Project manifest has no build-system section: $manifestPath"
    }
    $requires = [regex]::Matches(
        $section.Groups['body'].Value,
        '(?m)^requires\s*=\s*\[\s*"(?<value>[^"]+)"\s*\]\s*$'
    )
    if ($requires.Count -ne 1) {
        throw "Project manifest must pin exactly one build-system requirement: $manifestPath"
    }
    return $requires[0].Groups['value'].Value
}

function Copy-ReviewedTree {
    param(
        [Parameter(Mandatory)][string]$Source,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][string[]]$AllowedFiles
    )

    $sourceRoot = [System.IO.Path]::GetFullPath((Resolve-Path -LiteralPath $Source).Path)
    Assert-PathComponentsNotReparsePoints -Path $sourceRoot
    $sourceItem = Get-Item -LiteralPath $sourceRoot -Force
    if (-not $sourceItem.PSIsContainer -or
        ($sourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Developer Kit reviewed source is not a regular directory: $Source"
    }
    $repositoryPrefix = $repositoryRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    if (-not $sourceRoot.StartsWith($repositoryPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Developer Kit reviewed source is outside the repository: $Source"
    }
    $sourceRepositoryPath = [System.IO.Path]::GetRelativePath(
        $repositoryRoot,
        $sourceRoot
    ).Replace('\', '/')
    [System.IO.Directory]::CreateDirectory($Destination) | Out-Null
    $entries = @(Get-ChildItem -LiteralPath $sourceRoot -Force -Recurse)
    if ($entries.Count -eq 0 -or $entries.Count -gt 4096) {
        throw "Developer Kit source tree has an invalid file count: $Source"
    }
    foreach ($entry in $entries) {
        if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Developer Kit source tree contains a reparse point: $($entry.FullName)"
        }
    }
    $allowed = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($relativePath in $AllowedFiles) {
        $canonical = $relativePath.Replace('\', '/')
        if ([System.IO.Path]::IsPathRooted($canonical) -or
            @($canonical.Split('/') | Where-Object { $_ -ceq '' -or $_ -ceq '.' -or $_ -ceq '..' }).Count -gt 0 -or
            -not $allowed.Add($canonical)) {
            throw "Developer Kit reviewed allowlist contains an unsafe or duplicate path: $relativePath"
        }
        if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot $canonical.Replace('/', '\')) -PathType Leaf)) {
            throw "Developer Kit reviewed allowlist path is missing: $sourceRepositoryPath/$canonical"
        }
    }
    foreach ($file in @($entries | Where-Object { -not $_.PSIsContainer })) {
        $relative = [System.IO.Path]::GetRelativePath($sourceRoot, $file.FullName).Replace('\', '/')
        if ($allowed.Contains($relative)) {
            continue
        }
        $repositoryRelative = "$sourceRepositoryPath/$relative"
        & git -C $repositoryRoot check-ignore --quiet -- $repositoryRelative
        if ($LASTEXITCODE -eq 0) {
            continue
        }
        if ($LASTEXITCODE -ne 1) {
            throw "Could not classify Developer Kit example path with git check-ignore: $repositoryRelative"
        }
        throw "Developer Kit reviewed source contains an unapproved file: $repositoryRelative"
    }
    foreach ($relative in @($allowed | Sort-Object)) {
        $file = Get-Item -LiteralPath (Join-Path $sourceRoot $relative.Replace('/', '\')) -Force
        if ($relative -match '(?i)(?:^|[\\/])(?:__pycache__|target|node_modules|\.venv)(?:[\\/]|$)' -or
            $file.Extension.ToLowerInvariant() -in @(
                '.lc', '.h3latent', '.safetensors', '.ckpt', '.pt', '.pth', '.onnx',
                '.engine', '.plan', '.gguf', '.bin', '.mp4', '.mov', '.mkv', '.webm'
            )) {
            throw "Developer Kit source tree contains a forbidden payload: $($file.FullName)"
        }
        $destinationPath = Join-Path $Destination $relative
        [System.IO.Directory]::CreateDirectory((Split-Path -Parent $destinationPath)) | Out-Null
        [System.IO.File]::Copy($file.FullName, $destinationPath, $false)
    }
}

function New-DeterministicZip {
    param(
        [Parameter(Mandatory)][string]$SourceDirectory,
        [Parameter(Mandatory)][string]$DestinationPath
    )

    if (Test-Path -LiteralPath $DestinationPath) {
        throw "Refusing to overwrite Developer Kit archive: $DestinationPath"
    }
    Add-Type -AssemblyName System.IO.Compression
    $files = @(
        Get-ChildItem -LiteralPath $SourceDirectory -File -Force -Recurse |
            Sort-Object -Property @{ Expression = {
                [System.IO.Path]::GetRelativePath($SourceDirectory, $_.FullName).Replace('\', '/')
            } }
    )
    if ($files.Count -eq 0 -or $files.Count -gt 16384) {
        throw 'Developer Kit archive has an invalid file count.'
    }
    $stream = [System.IO.File]::Open(
        $DestinationPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $true,
            [System.Text.Encoding]::UTF8
        )
        try {
            foreach ($file in $files) {
                $relative = [System.IO.Path]::GetRelativePath(
                    $SourceDirectory,
                    $file.FullName
                ).Replace('\', '/')
                $entry = $archive.CreateEntry(
                    $relative,
                    [System.IO.Compression.CompressionLevel]::Optimal
                )
                $entry.LastWriteTime = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
                $input = [System.IO.File]::OpenRead($file.FullName)
                $output = $entry.Open()
                try {
                    $input.CopyTo($output)
                } finally {
                    $output.Dispose()
                    $input.Dispose()
                }
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Get-GitSource {
    $commit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
    $branch = (& git -C $repositoryRoot branch --show-current).Trim()
    $tree = (& git -C $repositoryRoot rev-parse 'HEAD^{tree}').Trim()
    $status = @(& git -C $repositoryRoot status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0 -or $commit -cnotmatch '^[0-9a-f]{40}$' -or
        $tree -cnotmatch '^[0-9a-f]{40}$') {
        throw 'Could not resolve Developer Kit Git source identity.'
    }
    $relativePaths = @(
        & git -C $repositoryRoot -c core.quotepath=false ls-files --cached --others --exclude-standard
    )
    if ($LASTEXITCODE -ne 0 -or $relativePaths.Count -eq 0) {
        throw 'Could not enumerate the Developer Kit public source snapshot.'
    }
    $snapshotRecords = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in @($relativePaths | Sort-Object -CaseSensitive)) {
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            continue
        }
        $fullPath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $relativePath))
        Assert-ChildPath -ParentPath $repositoryRoot -CandidatePath $fullPath | Out-Null
        Assert-PathComponentsNotReparsePoints -Path $fullPath
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            $snapshotRecords.Add("missing`0$relativePath")
            continue
        }
        $item = Get-Item -LiteralPath $fullPath -Force
        $hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $snapshotRecords.Add(
            "file`0$($relativePath.Replace('\', '/'))`0$($item.Length)`0$hash"
        )
    }
    $snapshotPayload = [System.Text.UTF8Encoding]::new($false).GetBytes(
        ($snapshotRecords -join "`n")
    )
    $snapshotSha256 = [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($snapshotPayload)
    ).ToLowerInvariant()
    return [pscustomobject]@{
        commit = $commit
        branch = $branch
        tree = $tree
        dirty = ($status.Count -gt 0)
        dirty_entry_count = $status.Count
        status = @($status)
        public_snapshot_sha256 = $snapshotSha256
        public_snapshot_file_count = $snapshotRecords.Count
    }
}

$workspaceManifestText = Get-Content -LiteralPath (Join-Path $repositoryRoot 'Cargo.toml') -Raw
$workspaceVersionMatches = [regex]::Matches(
    $workspaceManifestText,
    '(?ms)^\[workspace\.package\]\s*.*?^version\s*=\s*"(?<value>[^"]+)"\s*$'
)
if ($workspaceVersionMatches.Count -ne 1) {
    throw 'Could not derive the application API version from Cargo workspace.package.'
}
$applicationApiVersion = $workspaceVersionMatches[0].Groups['value'].Value
$applicationConfigs = @(
    'apps/latentdeck/src-tauri/tauri.conf.json',
    'apps/latentplayer/src-tauri/tauri.conf.json'
) | ForEach-Object {
    Get-Content -LiteralPath (Join-Path $repositoryRoot $_) -Raw | ConvertFrom-Json -Depth 32
}
$installerVersions = @($applicationConfigs | ForEach-Object { [string]$_.version } | Select-Object -Unique)
if ($installerVersions.Count -ne 1) {
    throw 'LatentDeck and LatentPlayer do not share one Windows installer identity.'
}
$windowsInstallerVersion = $installerVersions[0]
$h3LockPath = Join-Path `
    $repositoryRoot `
    'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
$h3Lock = Get-Content -LiteralPath $h3LockPath -Raw | ConvertFrom-Json -Depth 100
$h3PackVersion = [string]$h3Lock.pack_version
$torchComponents = @($h3Lock.dependencies | Where-Object { [string]$_.name -ceq 'torch' })
if ($h3PackVersion -cne '0.2.1' -or
    [string]$h3Lock.platform -cne 'windows-x86_64' -or
    [string]$h3Lock.python_runtime.version -cnotmatch '^3\.13\.\d+$' -or
    $torchComponents.Count -ne 1) {
    throw 'H3 curation lock does not expose one supported pack/Python/Torch compatibility identity.'
}
$torchVersion = [string]$torchComponents[0].version
$h3AdapterIdentity = Get-ProjectIdentity -ProjectPath 'codec-host/codecs/h3'
$d2DeckContract = Get-Content -LiteralPath (
    Join-Path $repositoryRoot 'operators/builtin/d2/package/deck-pack.json'
) -Raw | ConvertFrom-Json -Depth 32
$q4DeckContract = Get-Content -LiteralPath (
    Join-Path $repositoryRoot 'operators/builtin/q4/package/deck-pack.json'
) -Raw | ConvertFrom-Json -Depth 32
$deckSchema = Get-Content -LiteralPath (
    Join-Path $repositoryRoot 'spec/deck-package/deck-pack.schema.json'
) -Raw | ConvertFrom-Json -Depth 100
$codecSchema = Get-Content -LiteralPath (
    Join-Path $repositoryRoot 'spec/codec-pack/codec-pack.schema.json'
) -Raw | ConvertFrom-Json -Depth 100
$lcSchema = Get-Content -LiteralPath (
    Join-Path $repositoryRoot 'spec/latent-cartridge/manifest.schema.json'
) -Raw | ConvertFrom-Json -Depth 100
$operatorDescriptorSchema = Get-Content -LiteralPath (
    Join-Path `
        $repositoryRoot `
        'comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json'
) -Raw | ConvertFrom-Json -Depth 100
$buildConstraintsPath = Join-Path `
    $repositoryRoot `
    'tools/packaging/windows-x64-build-constraints.txt'
$buildConstraintsText = Get-Content -LiteralPath $buildConstraintsPath -Raw
foreach ($expectedConstraint in @(
    'maturin==1.15.0 --hash=sha256:552c2be4afd43fe8d5c9f3ec8d4c4756d973b8dcbe94c14084390301f50243e1',
    'uv-build==0.12.7 --hash=sha256:2c0baba9f1f1dfbfcb3dede01d04e7e9b94062b00aaf9880dcde73bd5e5c127b'
)) {
    if (@($buildConstraintsText -split "`r?`n" | Where-Object { $_ -ceq $expectedConstraint }).Count -ne 1) {
        throw "Developer Kit build-backend hash constraint is missing or duplicated: $expectedConstraint"
    }
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $artifactsRoot 'developer-kit'
}
$outputRoot = Assert-ChildPath `
    -ParentPath $artifactsRoot `
    -CandidatePath $OutputDirectory `
    -AllowParent
Assert-PathComponentsNotReparsePoints -Path $outputRoot
$finalDirectory = Join-Path $outputRoot "$ReleaseLabel-windows-x64"
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $finalDirectory | Out-Null
if (Test-Path -LiteralPath $finalDirectory) {
    throw "Refusing to overwrite an existing Developer Kit: $finalDirectory"
}

$sourceBefore = Get-GitSource
$distributable = (-not $DevelopmentBuild.IsPresent -and
    $sourceBefore.branch -ceq 'main' -and -not $sourceBefore.dirty)
if (-not $DevelopmentBuild.IsPresent -and -not $distributable) {
    throw 'Developer Kits must be built from a clean main checkout; use -DevelopmentBuild only for non-distributable local contract work.'
}

$recorderRoot = [System.IO.Path]::GetFullPath(
    (Resolve-Path -LiteralPath $ComfyRecorderArtifactDirectory).Path
)
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $recorderRoot | Out-Null
Assert-PathComponentsNotReparsePoints -Path $recorderRoot
$recorderBaseName = "LatentDeck-$ReleaseLabel-comfy-recorder-windows-x64"
$recorderNames = [ordered]@{
    archive = "$recorderBaseName.zip"
    receipt = "$recorderBaseName.receipt.json"
    checksums = "$recorderBaseName.SHA256SUMS.txt"
    sbom = "$recorderBaseName-sbom.cdx.json"
    notices = "$recorderBaseName-THIRD-PARTY-NOTICES.md"
    license_bundle = "$recorderBaseName-THIRD-PARTY-LICENSES.json"
    license_review = "$recorderBaseName-license-review.json"
}
Assert-ExactFlatArtifactSet -Root $recorderRoot -ExpectedNames @($recorderNames.Values)
$recorderReceiptRead = Read-BoundedJsonFile -Path (
    Join-Path $recorderRoot $recorderNames.receipt
)
$recorderReceipt = $recorderReceiptRead.Value
Assert-ExactJsonProperties `
    -Value $recorderReceipt `
    -Expected @(
        'schema_version', 'artifact_kind', 'release_label', 'release_channel', 'target',
        'python_abi', 'supported_python', 'signed', 'unsigned', 'distributable',
        'contains_model_weights', 'contains_cartridges', 'source', 'packages', 'archive',
        'sbom', 'third_party_notices', 'license_bundle', 'license_review'
    ) `
    -Context 'Comfy Recorder receipt'
Assert-ExactJsonProperties `
    -Value $recorderReceipt.source `
    -Expected @(
        'git_commit', 'git_branch', 'git_tree', 'git_dirty', 'git_dirty_entry_count',
        'public_snapshot_sha256', 'public_snapshot_file_count'
    ) `
    -Context 'Comfy Recorder source'
if ([int]$recorderReceipt.schema_version -ne 1 -or
    [string]$recorderReceipt.artifact_kind -cne 'comfy_recorder_bundle' -or
    [string]$recorderReceipt.release_label -cne $ReleaseLabel -or
    [string]$recorderReceipt.release_channel -cne $ReleaseChannel -or
    [string]$recorderReceipt.target -cne 'windows-x64' -or
    [string]$recorderReceipt.python_abi -cne 'cp312-abi3' -or
    (@($recorderReceipt.supported_python) -join "`0") -cne (@('cp312', 'cp313') -join "`0") -or
    [bool]$recorderReceipt.signed -or -not [bool]$recorderReceipt.unsigned -or
    [bool]$recorderReceipt.distributable -ne $distributable -or
    [bool]$recorderReceipt.contains_model_weights -or
    [bool]$recorderReceipt.contains_cartridges -or
    [string]$recorderReceipt.source.git_commit -cne $sourceBefore.commit -or
    [string]$recorderReceipt.source.git_branch -cne $sourceBefore.branch -or
    [string]$recorderReceipt.source.git_tree -cne $sourceBefore.tree -or
    [bool]$recorderReceipt.source.git_dirty -ne [bool]$sourceBefore.dirty -or
    [int64]$recorderReceipt.source.git_dirty_entry_count -ne [int64]$sourceBefore.dirty_entry_count -or
    [string]$recorderReceipt.source.public_snapshot_sha256 -cne
        $sourceBefore.public_snapshot_sha256 -or
    [int64]$recorderReceipt.source.public_snapshot_file_count -ne
        [int64]$sourceBefore.public_snapshot_file_count) {
    throw 'Comfy Recorder input does not match the Developer Kit release/source identity.'
}

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
    $expected = $expectedRecorderPackages[[string]$package.name]
    if ([string]$package.version -cne [string]$expected.Version -or
        [string]$package.file_name -cne [string]$expected.FileName -or
        [int64]$package.byte_length -le 0 -or
        [string]$package.sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw "Comfy Recorder package identity is invalid: $($package.name)"
    }
}

Assert-ExactJsonProperties `
    -Value $recorderReceipt.archive `
    -Expected @('file_name', 'byte_length', 'sha256') `
    -Context 'Comfy Recorder archive binding'
$recorderArchiveRecord = Assert-BoundFlatFile `
    -Root $recorderRoot `
    -Binding $recorderReceipt.archive `
    -ExpectedName $recorderNames.archive

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
$recorderSbomRecord = Assert-BoundFlatFile `
    -Root $recorderRoot -Binding $recorderReceipt.sbom -ExpectedName $recorderNames.sbom -MaximumBytes 64MB
$recorderNoticesRecord = Assert-BoundFlatFile `
    -Root $recorderRoot -Binding $recorderReceipt.third_party_notices `
    -ExpectedName $recorderNames.notices -MaximumBytes 16MB
$recorderLicenseBundleRecord = Assert-BoundFlatFile `
    -Root $recorderRoot -Binding $recorderReceipt.license_bundle `
    -ExpectedName $recorderNames.license_bundle -MaximumBytes 64MB
$recorderLicenseReviewRecord = Assert-BoundFlatFile `
    -Root $recorderRoot -Binding $recorderReceipt.license_review `
    -ExpectedName $recorderNames.license_review -MaximumBytes 16MB
Assert-ExactRecorderChecksums `
    -Root $recorderRoot `
    -Path (Join-Path $recorderRoot $recorderNames.checksums) `
    -Records @(
        $recorderArchiveRecord, $recorderSbomRecord, $recorderNoticesRecord,
        $recorderLicenseBundleRecord, $recorderLicenseReviewRecord
    )
$recorderSbom = (Read-BoundedJsonFile -Path $recorderSbomRecord.Path).Value
$recorderNativeClosure = Test-SafetensorsNativeClosureEvidence `
    -Evidence $recorderReceipt.sbom.safetensors_native_closure `
    -SbomPath $recorderSbomRecord.Path
$recorderSelectionRoots = @(
    @($recorderSbom.components) | Where-Object {
        @($_.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:selection-root' -and
            [string]$_.value -ceq 'true'
        }).Count -eq 1
    }
)
$recorderRootIdentities = @(
    foreach ($component in $recorderSelectionRoots) {
        $ecosystem = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:ecosystem'
        })
        if ($ecosystem.Count -ne 1) {
            throw "Comfy Recorder SBOM root ecosystem is ambiguous: $($component.'bom-ref')"
        }
        "$([string]$ecosystem[0].value):$($component.name)@$($component.version)"
    }
) | Sort-Object
$expectedRecorderRoots = @(
    'python:latentdeck-cartridge@0.1.0',
    'python:latentdeck-comfy-cartridge@0.1.0',
    'python:maturin@1.15.0',
    'python:safetensors@0.8.0',
    'python:uv-build@0.12.7',
    'rust:latentdeck-cartridge-python@0.1.0',
    'rust:latentdeck-cartridge@0.1.0'
) | Sort-Object
$actualRecorderScopeCounts = [ordered]@{
    artifact = 0
    runtime = 0
    build = 0
    'runtime+build' = 0
}
foreach ($component in @($recorderSbom.metadata.component) + @($recorderSbom.components)) {
    $scope = @($component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:dependency-scope'
    })
    if ($scope.Count -ne 1 -or
        [string]$scope[0].value -cnotin @('artifact', 'runtime', 'build', 'runtime+build') -or
        $null -eq $component.PSObject.Properties['licenses'] -or
        @($component.licenses).Count -eq 0) {
        throw "Comfy Recorder SBOM component scope/license is invalid: $($component.'bom-ref')"
    }
    $scopeName = [string]$scope[0].value
    $actualRecorderScopeCounts[$scopeName] = [int]$actualRecorderScopeCounts[$scopeName] + 1
}
if ([string]$recorderReceipt.sbom.format -cne 'CycloneDX-1.5' -or
    [string]$recorderSbom.bomFormat -cne 'CycloneDX' -or
    [string]$recorderSbom.specVersion -cne '1.5' -or
    [string]$recorderSbom.metadata.component.name -cne 'LatentDeck Comfy LC Recorder' -or
    [string]$recorderSbom.metadata.component.version -cne $ReleaseLabel -or
    [int]$recorderReceipt.sbom.component_count -ne @($recorderSbom.components).Count -or
    [int]$recorderReceipt.sbom.selection_root_count -ne 7 -or
    [int]$recorderNativeClosure.ComponentCount -ne 32 -or
    ($recorderRootIdentities -join "`0") -cne ($expectedRecorderRoots -join "`0") -or
    (@($recorderReceipt.sbom.selection_roots | Sort-Object) -join "`0") -cne
        ($expectedRecorderRoots -join "`0") -or
    @($actualRecorderScopeCounts.Keys | Where-Object {
        [int]$actualRecorderScopeCounts[$_] -ne
            [int]$recorderReceipt.sbom.dependency_scope_counts.$_
    }).Count -ne 0 -or
    [string]$recorderReceipt.license_review.status -cne 'complete' -or
    [int]$recorderReceipt.license_review.missing_license_component_count -ne 0) {
    throw 'Comfy Recorder SBOM or license review evidence is incomplete.'
}
$recorderLicenseResult = Test-ReleaseLicenseBundle `
    -BundlePath $recorderLicenseBundleRecord.Path `
    -SbomPath $recorderSbomRecord.Path `
    -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
    -ExpectedArtifactVersion $ReleaseLabel
if ([int64]$recorderLicenseResult.ByteLength -ne $recorderLicenseBundleRecord.ByteLength -or
    [string]$recorderLicenseResult.Sha256 -cne $recorderLicenseBundleRecord.Sha256 -or
    [int]$recorderLicenseResult.ComponentCount -ne [int]$recorderReceipt.license_bundle.component_count -or
    [int]$recorderLicenseResult.TextCount -ne [int]$recorderReceipt.license_bundle.text_count -or
    [int]$recorderLicenseResult.NoTextDispositionCount -ne
        [int]$recorderReceipt.license_bundle.build_only_no_text_disposition_count) {
    throw 'Comfy Recorder full-text license mapping does not match its receipt.'
}
$recorderLicenseReview = (Read-BoundedJsonFile -Path $recorderLicenseReviewRecord.Path).Value
if ([string]$recorderLicenseReview.status -cne 'complete' -or
    [int]$recorderLicenseReview.missing_license_component_count -ne 0 -or
    [int]$recorderLicenseReview.license_bundle.component_count -ne
        [int]$recorderReceipt.license_bundle.component_count -or
    [int]$recorderLicenseReview.license_bundle.text_count -ne
        [int]$recorderReceipt.license_bundle.text_count) {
    throw 'Comfy Recorder license review sidecar is incomplete or inconsistent.'
}
$recorderBundleRecord = [ordered]@{
    artifact_kind = 'comfy_recorder_bundle'
    archive = [ordered]@{
        path = "bundles/$($recorderArchiveRecord.FileName)"
        file_name = $recorderArchiveRecord.FileName
        byte_length = $recorderArchiveRecord.ByteLength
        sha256 = $recorderArchiveRecord.Sha256
    }
    standalone_receipt = [ordered]@{
        file_name = $recorderNames.receipt
        byte_length = $recorderReceiptRead.ByteLength
        sha256 = $recorderReceiptRead.Sha256
    }
    python_abi = 'cp312-abi3'
    supported_python = @('cp312', 'cp313')
    packages = @($recorderPackages | Sort-Object name)
}
$buildRoot = Join-Path $artifactsRoot ".developer-kit-build-$([guid]::NewGuid().ToString('N'))"
$outputStage = Join-Path $artifactsRoot ".developer-kit-output-$([guid]::NewGuid().ToString('N'))"
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $buildRoot | Out-Null
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $outputStage | Out-Null

$projectSpecs = @(
    [ordered]@{ Path = 'sdk/python'; Pattern = 'latentdeck_cartridge-*.whl' },
    [ordered]@{ Path = 'sdk/codec-python'; Pattern = 'latentdeck_codec_sdk-*.whl' },
    [ordered]@{ Path = 'sdk/deck-python'; Pattern = 'latentdeck_deck_sdk-*.whl' },
    [ordered]@{ Path = 'codec-host/python'; Pattern = 'latentdeck_codec_host-*.whl' },
    [ordered]@{ Path = 'operators/builtin/d2'; Pattern = 'latentdeck_operator_d2-*.whl' },
    [ordered]@{ Path = 'operators/builtin/q4'; Pattern = 'latentdeck_operator_q4-*.whl' },
    [ordered]@{ Path = 'comfy/toolkit'; Pattern = 'latentdeck_comfy_toolkit-*.whl' },
    [ordered]@{ Path = 'comfy/latent-cartridge'; Pattern = 'latentdeck_comfy_cartridge-*.whl' },
    [ordered]@{ Path = 'operators/examples/channel-roll'; Pattern = 'latentdeck_example_channel_roll-*.whl' }
)
$projectBuildRequirements = @(
    $projectSpecs |
        ForEach-Object { Get-ProjectBuildRequirement -ProjectPath $_.Path }
)
$expectedProjectBuildRequirements = @('maturin==1.15.0', 'uv-build==0.12.7')
if ((@($projectBuildRequirements | Sort-Object -CaseSensitive -Unique) -join "`0") -cne
    (@($expectedProjectBuildRequirements | Sort-Object -CaseSensitive) -join "`0") -or
    @($projectBuildRequirements | Where-Object { $_ -ceq 'maturin==1.15.0' }).Count -ne 1 -or
    @($projectBuildRequirements | Where-Object { $_ -ceq 'uv-build==0.12.7' }).Count -ne 8) {
    throw 'Developer Kit project build-system requirements drifted from the reviewed exact identities.'
}
$schemaSpecs = @(
        'spec/latent-cartridge/manifest.schema.json',
        'comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json',
        'spec/deck-package/deck-pack.schema.json',
    'spec/deck-package/operator.schema.json',
    'spec/deck-package/faceplate.schema.json',
    'spec/codec-pack/codec-pack.schema.json',
    'spec/extension-package/integrity.schema.json'
)
$exampleSpecs = @(
    [ordered]@{
        Path = 'examples/extensions/starter-deck'
        Files = @(
            'deck-pack.json', 'faceplate.json', 'NOTICE.txt', 'operator.json', 'README.md',
            'python/latentdeck_example_identity_deck/__init__.py',
            'python/latentdeck_example_identity_deck/operator.py'
        )
    },
    [ordered]@{
        Path = 'examples/extensions/synthetic-codec'
        Files = @('codec-pack.json', 'NOTICE.txt', 'README.md', 'runtime/adapter.py', 'runtime/runtime.lock')
    },
    [ordered]@{
        Path = 'examples/cartridge-genealogy'
        Files = @('README.md', 'transform.py')
    },
    [ordered]@{
        Path = 'operators/examples/channel-roll'
        Files = @(
            '__init__.py', 'LICENSE', 'pyproject.toml', 'README.md',
            'src/latentdeck_example_channel_roll/__init__.py',
            'src/latentdeck_example_channel_roll/comfy_node.py',
            'src/latentdeck_example_channel_roll/descriptor.json',
            'src/latentdeck_example_channel_roll/descriptor.py',
            'src/latentdeck_example_channel_roll/operator.py',
            'src/latentdeck_example_channel_roll/py.typed',
            'tests/test_example_operator.py'
        )
    }
)

$savedCargoTarget = $env:CARGO_TARGET_DIR
$savedSourceDateEpoch = $env:SOURCE_DATE_EPOCH
try {
    [System.IO.Directory]::CreateDirectory($buildRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($outputStage) | Out-Null
    $kitRoot = Join-Path $buildRoot 'kit'
    $wheelRoot = Join-Path $kitRoot 'wheels'
    $binaryRoot = Join-Path $kitRoot 'bin'
    $schemaRoot = Join-Path $kitRoot 'schemas'
    $exampleRoot = Join-Path $kitRoot 'examples'
    $bundlesRoot = Join-Path $kitRoot 'bundles'
    foreach ($directory in @(
        $kitRoot, $wheelRoot, $binaryRoot, $schemaRoot, $exampleRoot, $bundlesRoot
    )) {
        [System.IO.Directory]::CreateDirectory($directory) | Out-Null
    }

    $nestedRecorderPath = Join-Path $bundlesRoot $recorderArchiveRecord.FileName
    [System.IO.File]::Copy($recorderArchiveRecord.Path, $nestedRecorderPath, $false)
    $nestedRecorderItem = Get-Item -LiteralPath $nestedRecorderPath -Force
    if ([int64]$nestedRecorderItem.Length -ne $recorderArchiveRecord.ByteLength -or
        (Get-FileHash -LiteralPath $nestedRecorderItem.FullName -Algorithm SHA256).
            Hash.ToLowerInvariant() -cne $recorderArchiveRecord.Sha256) {
        throw 'Nested Comfy Recorder bundle changed while entering the Developer Kit.'
    }

    $uv = Get-Command uv.exe -ErrorAction Stop
    $uvVersion = (& $uv.Source --version).Trim()
    if ($LASTEXITCODE -ne 0 -or $uvVersion -cne 'uv 0.11.8 (0e961dd9a 2026-04-27 x86_64-pc-windows-msvc)') {
        throw "Developer Kit requires pinned uv 0.11.8; found '$uvVersion'."
    }
    $env:SOURCE_DATE_EPOCH = '315532800'
    $projectInventory = [System.Collections.Generic.List[object]]::new()
    foreach ($spec in $projectSpecs) {
        $identity = Get-ProjectIdentity -ProjectPath $spec.Path
        Invoke-Checked -Context "wheel build for $($identity.Name)" -Command {
            & $uv.Source build --wheel (Join-Path $repositoryRoot $spec.Path) `
                --out-dir $wheelRoot --no-create-gitignore `
                --build-constraints $buildConstraintsPath --require-hashes
        }
        $matches = @(Get-ChildItem -LiteralPath $wheelRoot -File -Filter $spec.Pattern)
        if ($matches.Count -ne 1) {
            throw "Expected exactly one Developer Kit wheel for $($identity.Name)."
        }
        $wheel = $matches[0]
        if ($wheel.Length -eq 0 -or $wheel.Length -gt 128MB -or
            ($wheel.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Developer Kit wheel is not a bounded regular file: $($wheel.Name)"
        }
        $wheelAuditParameters = @{
            Path = $wheel.FullName
            ForbiddenPathRoot = @($repositoryRoot, $buildRoot)
            Context = "Developer Kit wheel $($identity.Name)"
            RequireDeterministicTimestamps = $true
        }
        if ($identity.Name -ceq 'latentdeck-cartridge') {
            $wheelAuditParameters.ForbidEmbeddedSbom = $true
        }
        Assert-PublicProjectWheel @wheelAuditParameters | Out-Null
        $projectInventory.Add([pscustomobject]@{
            name = $identity.Name
            version = $identity.Version
            file_name = $wheel.Name
            byte_length = [int64]$wheel.Length
            sha256 = (Get-FileHash -LiteralPath $wheel.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        })
    }
    if ($projectInventory.Count -ne 9) {
        throw 'Developer Kit must contain exactly nine project wheels.'
    }
    foreach ($sharedName in @('latentdeck-cartridge', 'latentdeck-comfy-cartridge')) {
        $built = @($projectInventory | Where-Object { [string]$_.name -ceq $sharedName })
        $recorder = @($recorderReceipt.packages | Where-Object { [string]$_.name -ceq $sharedName })
        if ($built.Count -ne 1 -or $recorder.Count -ne 1 -or
            [string]$built[0].version -cne [string]$recorder[0].version -or
            [string]$built[0].file_name -cne [string]$recorder[0].file_name -or
            [int64]$built[0].byte_length -ne [int64]$recorder[0].byte_length -or
            [string]$built[0].sha256 -cne [string]$recorder[0].sha256) {
            throw "Developer Kit wheel differs from the exact Comfy Recorder wheel: $sharedName"
        }
    }

    $env:CARGO_TARGET_DIR = Join-Path $buildRoot 'cargo-target'
    Invoke-Checked -Context 'Developer Kit CLI build' -Command {
        cargo build --release --locked --target x86_64-pc-windows-msvc `
            -p latentdeck-cartridge -p latentdeck-extension-manager
    }
    $cliInventory = [System.Collections.Generic.List[object]]::new()
    foreach ($name in @('latentdeck-cartridge.exe', 'latentdeck-extension-manager.exe')) {
        $source = Join-Path $env:CARGO_TARGET_DIR "x86_64-pc-windows-msvc/release/$name"
        $item = Get-Item -LiteralPath $source -Force
        $bytes = [System.IO.File]::ReadAllBytes($item.FullName)
        if ($item.Length -lt 1MB -or $item.Length -gt 128MB -or
            $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
            throw "Developer Kit CLI is not a plausible bounded Windows PE: $name"
        }
        $destination = Join-Path $binaryRoot $name
        [System.IO.File]::Copy($item.FullName, $destination, $false)
        $cliInventory.Add([pscustomobject]@{
            name = $name
            version = $applicationApiVersion
            byte_length = [int64]$item.Length
            sha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToLowerInvariant()
        })
    }

    foreach ($schema in $schemaSpecs) {
        $source = Get-Item -LiteralPath (Join-Path $repositoryRoot $schema) -Force
        if (($source.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            $source.Length -eq 0 -or $source.Length -gt 2MB) {
            throw "Developer Kit schema is not a bounded regular file: $schema"
        }
        $destination = Join-Path $schemaRoot ($schema.Replace('/', '__'))
        [System.IO.File]::Copy($source.FullName, $destination, $false)
    }
    foreach ($example in $exampleSpecs) {
        Copy-ReviewedTree `
            -Source (Join-Path $repositoryRoot $example.Path) `
            -Destination (Join-Path $exampleRoot ($example.Path.Replace('/', '__'))) `
            -AllowedFiles @($example.Files)
    }

    [System.IO.File]::Copy((Join-Path $repositoryRoot 'LICENSE'), (Join-Path $kitRoot 'LICENSE'), $false)
    $bootstrapDirectory = Join-Path $kitRoot 'bootstrap'
    [System.IO.Directory]::CreateDirectory($bootstrapDirectory) | Out-Null
    $bootstrap = @'
[CmdletBinding()]
param([string]$EnvironmentDirectory = '.latentdeck-dev')

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$kitRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $kitRoot 'DEVELOPER-KIT-MANIFEST.json'
$sumsPath = Join-Path $kitRoot 'SHA256SUMS.txt'
$manifestItem = Get-Item -LiteralPath $manifestPath -Force
$sumsItem = Get-Item -LiteralPath $sumsPath -Force
foreach ($metadataItem in @($manifestItem, $sumsItem)) {
    if ($metadataItem.PSIsContainer -or
        ($metadataItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $metadataItem.Length -eq 0 -or $metadataItem.Length -gt 1MB) {
        throw "Developer Kit trust metadata must be a bounded regular file: $($metadataItem.Name)"
    }
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -Depth 100
$expectedWheels = @($manifest.wheels)
if ($expectedWheels.Count -ne 9) { throw 'Developer Kit manifest wheel inventory is incomplete.' }
$wheelNames = @($expectedWheels | ForEach-Object { [string]$_.file_name })
if (@($wheelNames | Sort-Object -Unique).Count -ne 9 -or
    @($wheelNames | Where-Object {
        [System.IO.Path]::GetFileName($_) -cne $_ -or $_ -cnotmatch '^[A-Za-z0-9_.+-]+\.whl$'
    }).Count -gt 0) {
    throw 'Developer Kit manifest contains an unsafe or duplicate wheel name.'
}
$wheels = @(Get-ChildItem -LiteralPath (Join-Path $kitRoot 'wheels') -File -Filter '*.whl' | Sort-Object Name)
if ((@($wheelNames | Sort-Object) -join "`0") -cne
    (@($wheels | Select-Object -ExpandProperty Name | Sort-Object) -join "`0")) {
    throw 'Developer Kit wheel directory does not exactly match its manifest.'
}
$sumByPath = @{}
foreach ($line in @(Get-Content -LiteralPath $sumsPath)) {
    if ($line.Length -gt 4096 -or
        $line -cnotmatch '^(?<hash>[0-9a-f]{64})  (?<path>[^\r\n]+)$' -or
        $sumByPath.ContainsKey($Matches.path)) {
        throw 'Developer Kit checksum manifest is malformed or duplicated.'
    }
    $sumByPath[$Matches.path] = $Matches.hash
}
foreach ($wheel in $expectedWheels) {
    $relative = "wheels/$($wheel.file_name)"
    $path = Join-Path $kitRoot $relative.Replace('/', '\')
    $item = Get-Item -LiteralPath $path -Force
    $actualHash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([int64]$item.Length -ne [int64]$wheel.byte_length -or
        $actualHash -cne [string]$wheel.sha256 -or
        -not $sumByPath.ContainsKey($relative) -or
        [string]$sumByPath[$relative] -cne $actualHash) {
        throw "Developer Kit wheel failed SHA-256/length verification: $($wheel.file_name)"
    }
}
$environmentPath = [System.IO.Path]::GetFullPath($EnvironmentDirectory)
if (Test-Path -LiteralPath $environmentPath) {
    throw "Refusing to overwrite an existing Python environment: $environmentPath"
}
$python = Get-Command py.exe -ErrorAction SilentlyContinue
if ($null -ne $python) {
    & $python.Source -3.13 -m venv $environmentPath
} else {
    $python = Get-Command python.exe -ErrorAction Stop
    & $python.Source -m venv $environmentPath
}
if ($LASTEXITCODE -ne 0) { throw 'Could not create the Python 3.13 environment.' }
$environmentPython = Join-Path $environmentPath 'Scripts/python.exe'
$version = (& $environmentPython -I -s -B -c 'import platform; print(platform.python_version())').Trim()
if ($LASTEXITCODE -ne 0 -or $version -cnotmatch '^3\.13\.') {
    throw "Developer Kit requires Python 3.13; found '$version'."
}
& $environmentPython -m pip install --no-deps @($wheels.FullName)
if ($LASTEXITCODE -ne 0) { throw 'Developer Kit project wheel installation failed.' }
$expectedDistributions = @($expectedWheels | ForEach-Object { "$($_.name)==$($_.version)" })
$verificationCode = 'import importlib.metadata as m,sys; pairs=[v.rsplit("==",1) for v in sys.argv[1:]]; bad=[(n,v,m.version(n)) for n,v in pairs if m.version(n)!=v]; raise SystemExit(str(bad) if bad else 0)'
& $environmentPython -I -s -B -c $verificationCode @expectedDistributions
if ($LASTEXITCODE -ne 0) { throw 'Developer Kit distribution verification failed.' }
Write-Host "Developer Kit project wheels installed in $environmentPath" -ForegroundColor Green
'@
    Write-Utf8Text -Path (Join-Path $bootstrapDirectory 'Install-ProjectWheels.ps1') -Text ($bootstrap + "`n")

    $kitReadme = @'
# LatentDeck Developer Kit

This archive contains the source-built LatentDeck project wheels, Windows
authoring CLIs, machine-readable schemas, data-free extension examples, and the
exact hash-bound Comfy LC Recorder bundle for release `{0}`. It does not contain model weights, decoder assets,
cartridges, generated media, or third-party Python runtime dependencies.

Run `bootstrap/Install-ProjectWheels.ps1` from PowerShell to install the nine
project wheels into a Python 3.13 virtual environment. The bootstrap deliberately
uses `--no-deps`; install the third-party dependencies required by the specific
workflow from the repository's locked environment or documented runtime profile.
'@
    Write-Utf8Text -Path (Join-Path $kitRoot 'README.md') -Text (
        ([string]::Format($kitReadme, $ReleaseLabel)) + "`n"
    )

    $projectVersions = [ordered]@{}
    foreach ($project in @($projectInventory | Sort-Object name)) {
        if ($projectVersions.Contains([string]$project.name)) {
            throw "Developer Kit project version inventory is duplicated: $($project.name)"
        }
        $projectVersions[[string]$project.name] = [string]$project.version
    }
    if ($projectVersions.Count -ne 9 -or
        [string]$h3AdapterIdentity.Name -cne 'latentdeck-codec-h3' -or
        [string]$h3AdapterIdentity.Version -cnotmatch '^\d+\.\d+\.\d+$' -or
        [string]$d2DeckContract.deck_id -cne 'org.latentdeck.deck.d2' -or
        [string]$q4DeckContract.deck_id -cne 'org.latentdeck.deck.q4' -or
        [string]$d2DeckContract.deck_version -cnotmatch '^\d+\.\d+\.\d+$' -or
        [string]$q4DeckContract.deck_version -cnotmatch '^\d+\.\d+\.\d+$' -or
        [int]$d2DeckContract.compatibility.worker_protocol -ne [int]$h3Lock.worker_protocol -or
        [int]$q4DeckContract.compatibility.worker_protocol -ne [int]$h3Lock.worker_protocol -or
        [int]$d2DeckContract.compatibility.deck_operator_api -ne
            [int]$q4DeckContract.compatibility.deck_operator_api) {
        throw 'Developer Kit project/protocol identities drifted from their authoritative manifests.'
    }
    $compatibility = [ordered]@{
        schema_version = 1
        release_label = $ReleaseLabel
        release_channel = $ReleaseChannel
        platform = 'windows-x86_64'
        application_api_version = $applicationApiVersion
        windows_installer_version = $windowsInstallerVersion
        distributable = $distributable
        python = [ordered]@{
            implementation = 'cpython'
            supported_series = '3.13'
            h3_runtime_version = [string]$h3Lock.python_runtime.version
            platform_tag = 'win_amd64'
            comfy_recorder = [ordered]@{
                python_abi = [string]$recorderReceipt.python_abi
                supported_abis = @($recorderReceipt.supported_python)
            }
        }
        torch = [ordered]@{
            h3_runtime_exact_build = $torchVersion
            bundled_in_developer_kit = $false
        }
        lc_spec_versions = @(
            [string]$lcSchema.properties.spec_version.PSObject.Properties['const'].Value
        )
        worker_protocol_versions = @([int]$h3Lock.worker_protocol)
        deck_manifest_version = [string](
            $deckSchema.properties.manifest_version.PSObject.Properties['const'].Value
        )
        codec_manifest_version = [string](
            $codecSchema.properties.manifest_version.PSObject.Properties['const'].Value
        )
        deck_package_operator_host_api_version = [int]$d2DeckContract.compatibility.deck_operator_api
        operator_descriptor_schema_version = [string](
            $operatorDescriptorSchema.properties.schema_version.PSObject.Properties['const'].Value
        )
        codec_adapter_api_version = [int]$h3Lock.codec_adapter_api
        h3_codec = [ordered]@{
            pack_version = $h3PackVersion
            adapter_version = [string]$h3AdapterIdentity.Version
        }
        sdks = [ordered]@{
            cartridge = [string]$projectVersions['latentdeck-cartridge']
            deck = [string]$projectVersions['latentdeck-deck-sdk']
            codec = [string]$projectVersions['latentdeck-codec-sdk']
        }
        decks = [ordered]@{
            d2 = [ordered]@{
                deck_id = [string]$d2DeckContract.deck_id
                deck_version = [string]$d2DeckContract.deck_version
            }
            q4 = [ordered]@{
                deck_id = [string]$q4DeckContract.deck_id
                deck_version = [string]$q4DeckContract.deck_version
            }
        }
        python_operator_packages = [ordered]@{
            d2 = [ordered]@{
                distribution = 'latentdeck-operator-d2'
                version = [string]$projectVersions['latentdeck-operator-d2']
            }
            q4 = [ordered]@{
                distribution = 'latentdeck-operator-q4'
                version = [string]$projectVersions['latentdeck-operator-q4']
            }
            channel_roll = [ordered]@{
                distribution = 'latentdeck-example-channel-roll'
                version = [string]$projectVersions['latentdeck-example-channel-roll']
            }
        }
        project_wheels = @(
            $projectInventory |
                Sort-Object name |
                ForEach-Object { [ordered]@{ name = $_.name; version = $_.version } }
        )
    }
    Write-JsonFile -Value $compatibility -Path (Join-Path $kitRoot 'COMPATIBILITY.json')

    $sbomPath = Join-Path $kitRoot 'SBOM.cdx.json'
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath $sbomPath `
        -ArtifactName 'LatentDeck Developer Kit' `
        -ArtifactVersion $ReleaseLabel `
        -ArtifactScope developer-kit `
        -CargoPackage @(
            'latentdeck-cartridge',
            'latentdeck-cartridge-python',
            'latentdeck-extension-manager'
        ) `
        -PythonPackage @($projectInventory | ForEach-Object { $_.name }) `
        -PythonBuildPackage $expectedProjectBuildRequirements `
        -Deterministic | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Developer Kit SBOM generation failed.'
    }
    $sbom = Get-Content -LiteralPath $sbomPath -Raw | ConvertFrom-Json -Depth 100
    $rootLicenseMatches = @($sbom.metadata.component.licenses | Where-Object {
        $null -ne $_.PSObject.Properties['license'] -and
        [string]$_.license.name -ceq 'Apache-2.0'
    })
    if (@($sbom.metadata.component.licenses).Count -ne 1 -or
        $rootLicenseMatches.Count -ne 1) {
        throw 'Developer Kit SBOM root does not declare the reviewed Apache-2.0 artifact license.'
    }
    $selectionRoots = @(
        @($sbom.metadata.component) + @($sbom.components) |
            Where-Object {
                @($_.properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:selection-root' -and
                    [string]$_.value -ceq 'true'
                }).Count -eq 1
            }
    )
    $rootIdentities = @(
        foreach ($component in $selectionRoots) {
            $ecosystems = @(
                $component.properties |
                    Where-Object { [string]$_.name -ceq 'latentdeck:ecosystem' } |
                    ForEach-Object { [string]$_.value }
            )
            if ($ecosystems.Count -ne 1) {
                throw "Developer Kit SBOM root has no exact ecosystem identity: $($component.name)"
            }
            "$($ecosystems[0]):$($component.name)@$($component.version)"
        }
    )
    $expectedRootIdentities = @(
        $projectInventory | ForEach-Object { "python:$($_.name)@$($_.version)" }
        "rust:latentdeck-cartridge@$applicationApiVersion"
        "rust:latentdeck-cartridge-python@$applicationApiVersion"
        "rust:latentdeck-extension-manager@$applicationApiVersion"
        'python:uv-build@0.12.7'
        'python:maturin@1.15.0'
    ) | Sort-Object
    if ((@($rootIdentities | Sort-Object) -join "`0") -cne
        ($expectedRootIdentities -join "`0")) {
        throw (
            'Developer Kit SBOM does not cover the exact wheel, native-wheel, CLI, and build-backend roots. ' +
            "Expected: $($expectedRootIdentities -join ', '); " +
            "actual: $(@($rootIdentities | Sort-Object) -join ', ')."
        )
    }
    $rootScopeProperties = @($sbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:dependency-scope' -and
        [string]$_.value -ceq 'artifact'
    })
    $includedScopePolicy = @($sbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:included-dependency-scopes' -and
        [string]$_.value -ceq 'artifact,runtime,build,runtime+build'
    })
    $excludedScopePolicy = @($sbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:excluded-dependency-scopes' -and
        [string]$_.value -ceq 'development'
    })
    $targetPlatformPolicy = @($sbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:target-platform' -and
        [string]$_.value -ceq 'x86_64-pc-windows-msvc'
    })
    if ($rootScopeProperties.Count -ne 1 -or
        $includedScopePolicy.Count -ne 1 -or
        $excludedScopePolicy.Count -ne 1 -or
        $targetPlatformPolicy.Count -ne 1) {
        throw 'Developer Kit SBOM does not declare the exact Windows dependency-scope policy.'
    }
    $allowedDependencyScopes = @('artifact', 'runtime', 'build', 'runtime+build')
    $dependencyScopeCounts = [ordered]@{}
    foreach ($component in @($sbom.metadata.component) + @($sbom.components)) {
        $scopeProperties = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope'
        })
        if ($scopeProperties.Count -ne 1 -or
            [string]$scopeProperties[0].value -cnotin $allowedDependencyScopes) {
            throw "Developer Kit SBOM component has no exact distributable dependency scope: $($component.name)@$($component.version)"
        }
        $scope = [string]$scopeProperties[0].value
        if (-not $dependencyScopeCounts.Contains($scope)) {
            $dependencyScopeCounts[$scope] = 0
        }
        $dependencyScopeCounts[$scope] = [int]$dependencyScopeCounts[$scope] + 1
    }
    $missingLicense = @(
        @($sbom.metadata.component) + @($sbom.components) |
            Where-Object {
                $licenseField = $_.PSObject.Properties['licenses']
                if ($null -eq $licenseField -or @($licenseField.Value).Count -eq 0) {
                    return $true
                }
                $usable = @(
                    $licenseField.Value |
                        Where-Object {
                            ($null -ne $_.PSObject.Properties['expression'] -and
                             -not [string]::IsNullOrWhiteSpace([string]$_.expression)) -or
                            ($null -ne $_.PSObject.Properties['license'] -and
                             (($null -ne $_.license.PSObject.Properties['id'] -and
                               -not [string]::IsNullOrWhiteSpace([string]$_.license.id)) -or
                              ($null -ne $_.license.PSObject.Properties['name'] -and
                               -not [string]::IsNullOrWhiteSpace([string]$_.license.name))))
                        }
                )
                return $usable.Count -eq 0
            } |
            ForEach-Object {
                [ordered]@{ name = [string]$_.name; version = [string]$_.version }
            }
    )
    $licenseReview = [ordered]@{
        schema_version = 1
        status = if ($missingLicense.Count -eq 0) { 'complete' } else { 'review_required' }
        policy = 'No license value is inferred when upstream metadata is absent.'
        component_count = @($sbom.components).Count
        root_component_reviewed = $true
        dependency_scope_counts = $dependencyScopeCounts
        selection_root_count = $selectionRoots.Count
        expected_selection_root_count = 14
        selection_roots = @($rootIdentities | Sort-Object)
        missing_license_component_count = $missingLicense.Count
        missing_license_components = $missingLicense
    }
    $noticeLines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in @(
        '# LatentDeck Developer Kit third-party notices',
        '',
        "Artifact: LatentDeck Developer Kit $ReleaseLabel",
        '',
        'This inventory is scoped to the nine project wheels, their compiled native Rust closure,',
        'the two Rust command-line tools, and the two exact non-redistributed build backends.',
        'Third-party Python runtime',
        'dependencies are not bundled; the bootstrap installs project wheels with `--no-deps`.',
        'License labels below come directly from locked package metadata. Missing metadata is',
        'reported as missing and blocks distributable staging; no license is inferred.',
        '',
        '## Components',
        ''
    )) {
        $noticeLines.Add($line)
    }
    foreach ($component in @($sbom.components | Sort-Object 'bom-ref')) {
        $componentLicenses = if ($null -eq $component.PSObject.Properties['licenses']) {
            @()
        } else {
            @($component.licenses)
        }
        $licenseLabels = @(
            foreach ($entry in $componentLicenses) {
                if ($null -ne $entry.PSObject.Properties['expression'] -and
                    -not [string]::IsNullOrWhiteSpace([string]$entry.expression)) {
                    [string]$entry.expression
                } elseif ($null -ne $entry.PSObject.Properties['license']) {
                    if ($null -ne $entry.license.PSObject.Properties['id'] -and
                        -not [string]::IsNullOrWhiteSpace([string]$entry.license.id)) {
                        [string]$entry.license.id
                    } elseif ($null -ne $entry.license.PSObject.Properties['name'] -and
                        -not [string]::IsNullOrWhiteSpace([string]$entry.license.name)) {
                        [string]$entry.license.name
                    }
                }
            }
        )
        $licenseText = if ($licenseLabels.Count -eq 0) {
            'MISSING LICENSE METADATA'
        } else {
            (@($licenseLabels | Sort-Object -Unique) -join ' OR ')
        }
        $ecosystem = @(
            $component.properties |
                Where-Object { $_.name -ceq 'latentdeck:ecosystem' } |
                Select-Object -ExpandProperty value
        ) -join ','
        $noticeLines.Add("- $($component.name) $($component.version) [$ecosystem] - $licenseText")
    }
    Write-Utf8Text `
        -Path (Join-Path $kitRoot 'THIRD_PARTY_NOTICES.md') `
        -Text (($noticeLines -join "`n") + "`n")
    $licenseBundle = New-ReleaseLicenseBundle `
        -SbomPath $sbomPath `
        -ArtifactName 'LatentDeck Developer Kit' `
        -ArtifactVersion $ReleaseLabel `
        -OutputPath (Join-Path $kitRoot 'THIRD_PARTY_LICENSES.json') `
        -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md')
    $licenseReview['license_bundle'] = [ordered]@{
        schema_version = 1
        component_count = $licenseBundle.ComponentCount
        text_count = $licenseBundle.TextCount
        build_only_no_text_disposition_count = $licenseBundle.NoTextDispositionCount
        redistributed_component_text_coverage = 'complete'
    }
    Write-JsonFile -Value $licenseReview -Path (Join-Path $kitRoot 'LICENSE-REVIEW.json')

    $payloadFiles = @(
        Get-ChildItem -LiteralPath $kitRoot -File -Force -Recurse |
            Sort-Object -Property @{ Expression = {
                [System.IO.Path]::GetRelativePath($kitRoot, $_.FullName).Replace('\', '/')
            } }
    )
    $contents = @(
        foreach ($file in $payloadFiles) {
            [ordered]@{
                path = [System.IO.Path]::GetRelativePath($kitRoot, $file.FullName).Replace('\', '/')
                byte_length = [int64]$file.Length
                sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            }
        }
    )
    $kitManifest = [ordered]@{
        schema_version = 1
        release_label = $ReleaseLabel
        release_channel = $ReleaseChannel
        platform = 'windows-x86_64'
        application_api_version = $applicationApiVersion
        windows_installer_version = $windowsInstallerVersion
        distributable = $distributable
        wheels = @($projectInventory)
        clis = @($cliInventory)
        schemas = @($schemaSpecs)
        examples = @($exampleSpecs | ForEach-Object { $_.Path })
        comfy_recorder_bundle = $recorderBundleRecord
        content = $contents
        contains_model_weights = $false
        contains_cartridges = $false
        contains_generated_media = $false
    }
    Write-JsonFile -Value $kitManifest -Path (Join-Path $kitRoot 'DEVELOPER-KIT-MANIFEST.json')
    Write-Utf8Text -Path (Join-Path $kitRoot 'SHA256SUMS.txt') -Text (
        (@($contents | ForEach-Object { "$($_.sha256)  $($_.path)" }) -join "`n") + "`n"
    )

    $archiveName = "LatentDeck-$ReleaseLabel-developer-kit-windows-x64.zip"
    $archiveStage = Join-Path $outputStage $archiveName
    New-DeterministicZip -SourceDirectory $kitRoot -DestinationPath $archiveStage
    $archiveItem = Get-Item -LiteralPath $archiveStage
    if ($archiveItem.Length -eq 0 -or $archiveItem.Length -ge 2GB) {
        throw 'Developer Kit archive is empty or reaches the GitHub per-asset limit.'
    }

    foreach ($metadataName in @(
        'SBOM.cdx.json', 'LICENSE-REVIEW.json', 'THIRD_PARTY_NOTICES.md',
        'THIRD_PARTY_LICENSES.json'
    )) {
        [System.IO.File]::Copy(
            (Join-Path $kitRoot $metadataName),
            (Join-Path $outputStage $metadataName),
            $false
        )
    }
    $archiveHash = (Get-FileHash -LiteralPath $archiveStage -Algorithm SHA256).Hash.ToLowerInvariant()
    $sbomOutput = Get-Item -LiteralPath (Join-Path $outputStage 'SBOM.cdx.json')
    $noticeOutput = Get-Item -LiteralPath (Join-Path $outputStage 'THIRD_PARTY_NOTICES.md')
    $licenseReviewOutput = Get-Item -LiteralPath (Join-Path $outputStage 'LICENSE-REVIEW.json')
    $licenseBundleOutput = Get-Item -LiteralPath (Join-Path $outputStage 'THIRD_PARTY_LICENSES.json')
    $sourceAfter = Get-GitSource
    if ($sourceAfter.commit -cne $sourceBefore.commit -or
        $sourceAfter.branch -cne $sourceBefore.branch -or
        $sourceAfter.tree -cne $sourceBefore.tree -or
        $sourceAfter.public_snapshot_sha256 -cne $sourceBefore.public_snapshot_sha256 -or
        [int64]$sourceAfter.public_snapshot_file_count -ne
            [int64]$sourceBefore.public_snapshot_file_count -or
        ($sourceAfter.status -join "`n") -cne ($sourceBefore.status -join "`n")) {
        throw 'Repository source changed while the Developer Kit was building.'
    }
    $receipt = [ordered]@{
        schema_version = 2
        release_label = $ReleaseLabel
        release_channel = $ReleaseChannel
        application_api_version = $applicationApiVersion
        windows_installer_version = $windowsInstallerVersion
        distributable = $distributable
        platform = 'windows-x86_64'
        source = [ordered]@{
            git_commit = $sourceBefore.commit
            git_branch = $sourceBefore.branch
            git_tree = $sourceBefore.tree
            git_dirty = $sourceBefore.dirty
            git_dirty_entry_count = $sourceBefore.dirty_entry_count
            public_snapshot_sha256 = $sourceBefore.public_snapshot_sha256
            public_snapshot_file_count = $sourceBefore.public_snapshot_file_count
        }
        archive = [ordered]@{
            name = $archiveItem.Name
            byte_length = [int64]$archiveItem.Length
            sha256 = $archiveHash
        }
        sbom = [ordered]@{
            name = $sbomOutput.Name
            byte_length = [int64]$sbomOutput.Length
            sha256 = (Get-FileHash -LiteralPath $sbomOutput.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            component_count = @($sbom.components).Count
            selection_root_count = $selectionRoots.Count
            selection_roots = @($rootIdentities | Sort-Object)
            dependency_scope_counts = $dependencyScopeCounts
        }
        notices = [ordered]@{
            name = $noticeOutput.Name
            byte_length = [int64]$noticeOutput.Length
            sha256 = (Get-FileHash -LiteralPath $noticeOutput.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        license_review = [ordered]@{
            name = $licenseReviewOutput.Name
            byte_length = [int64]$licenseReviewOutput.Length
            sha256 = (Get-FileHash -LiteralPath $licenseReviewOutput.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            status = $licenseReview.status
            missing_license_component_count = $licenseReview.missing_license_component_count
        }
        license_bundle = [ordered]@{
            name = $licenseBundleOutput.Name
            byte_length = [int64]$licenseBundleOutput.Length
            sha256 = (Get-FileHash -LiteralPath $licenseBundleOutput.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            schema_version = 1
            component_count = $licenseBundle.ComponentCount
            text_count = $licenseBundle.TextCount
            build_only_no_text_disposition_count = $licenseBundle.NoTextDispositionCount
        }
        comfy_recorder_bundle = $recorderBundleRecord
        wheel_count = 9
        cli_count = 2
        signed = $false
        unsigned = $true
        contains_model_weights = $false
        contains_cartridges = $false
    }
    Write-JsonFile -Value $receipt -Path (Join-Path $outputStage 'developer-kit.json')
    $sums = @(
        "$archiveHash  $archiveName"
        "$($receipt.sbom.sha256)  SBOM.cdx.json"
        "$($receipt.notices.sha256)  THIRD_PARTY_NOTICES.md"
        "$($receipt.license_review.sha256)  LICENSE-REVIEW.json"
        "$($receipt.license_bundle.sha256)  THIRD_PARTY_LICENSES.json"
    )
    Write-Utf8Text -Path (Join-Path $outputStage 'SHA256SUMS.txt') -Text (
        ($sums -join "`n") + "`n"
    )

    $expectedFiles = @(
        $archiveName,
        'developer-kit.json',
        'LICENSE-REVIEW.json',
        'SBOM.cdx.json',
        'SHA256SUMS.txt',
        'THIRD_PARTY_NOTICES.md'
        'THIRD_PARTY_LICENSES.json'
    ) | Sort-Object
    $actualFiles = @(
        Get-ChildItem -LiteralPath $outputStage -File -Force |
            Select-Object -ExpandProperty Name |
            Sort-Object
    )
    if (($expectedFiles -join "`0") -cne ($actualFiles -join "`0")) {
        throw 'Developer Kit output contains an unexpected file set.'
    }

    [System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
    Assert-PathComponentsNotReparsePoints -Path $outputRoot
    Assert-PathComponentsNotReparsePoints -Path $outputStage
    if (Test-Path -LiteralPath $finalDirectory) {
        throw "Developer Kit destination appeared during build: $finalDirectory"
    }
    [System.IO.Directory]::Move($outputStage, $finalDirectory)
    $outputStage = $null
    Write-Output $finalDirectory
} finally {
    $env:CARGO_TARGET_DIR = $savedCargoTarget
    if ($null -eq $savedSourceDateEpoch) {
        Remove-Item Env:SOURCE_DATE_EPOCH -ErrorAction SilentlyContinue
    } else {
        $env:SOURCE_DATE_EPOCH = $savedSourceDateEpoch
    }
    foreach ($temporary in @(
        [pscustomobject]@{ Path = $buildRoot; Prefix = '.developer-kit-build-' },
        [pscustomobject]@{ Path = $outputStage; Prefix = '.developer-kit-output-' }
    )) {
        if ($null -eq $temporary.Path -or -not (Test-Path -LiteralPath $temporary.Path)) {
            continue
        }
        Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $temporary.Path | Out-Null
        Assert-PathComponentsNotReparsePoints -Path $temporary.Path
        if (-not ([System.IO.Path]::GetFileName($temporary.Path)).StartsWith(
            $temporary.Prefix,
            [System.StringComparison]::Ordinal
        )) {
            throw "Refusing to remove unsafe Developer Kit staging path: $($temporary.Path)"
        }
        Remove-Item -LiteralPath $temporary.Path -Recurse -Force
    }
}
