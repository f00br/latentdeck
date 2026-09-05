Set-StrictMode -Version Latest

$script:ForbiddenPayloadExtensions = @(
    '.lc', '.h3latent', '.safetensors', '.ckpt', '.pt', '.pth', '.onnx',
    '.engine', '.plan', '.gguf', '.bin', '.mp4', '.mov', '.mkv', '.avi',
    '.webm', '.wav', '.flac', '.mp3', '.whl', '.npy', '.npz', '.pkl',
    '.pickle', '.png', '.jpg', '.jpeg', '.webp', '.gif', '.bmp', '.tif',
    '.tiff', '.exr', '.hdr', '.psd', '.ps1', '.cmd', '.bat', '.sh'
)
$script:ArchiveExtensions = @(
    '.ld', '.ldcodec', '.zip', '.tar', '.tgz', '.gz', '.bz2', '.xz', '.7z', '.rar'
)
$script:ForbiddenDirectoryNames = @(
    '.git', '.hg', '.svn', '__pycache__', '.pytest_cache', '.ruff_cache'
)
$script:PortableTextExtensions = @(
    '.cfg', '.ini', '.json', '.md', '.txt', '.toml', '.yaml', '.yml', '._pth',
    '.py', '.pyi', '.pth', '.pem', '.key', '.crt', '.xml', '.html', '.rst',
    '.cmake', '.pc', '.h', '.hpp', '.c', '.cc', '.cpp', '.rs', '.js', '.mjs',
    '.ts', '.css'
)
$script:SensitivePortableTextExtensions = @(
    '.cfg', '.ini', '.json', '.md', '.txt', '.toml', '.yaml', '.yml', '._pth',
    '.pem', '.key', '.crt', '.xml', '.cmake', '.pc'
)
$script:MaximumCatalogFiles = 32766
$script:MaximumArchiveEntries = 32768
$script:MaximumArchiveBytes = [int64](32GB)
$script:MaximumPackBytes = [int64](64GB)
$script:MaximumJsonBytes = [int64](1MB)

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Assert-ChildPath {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$ParentPath,

        [Parameter(Mandatory)]
        [string]$CandidatePath,

        [switch]$AllowParent
    )

    $parentFullPath = [System.IO.Path]::GetFullPath($ParentPath).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    $candidateFullPath = [System.IO.Path]::GetFullPath($CandidatePath).TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )

    if ($AllowParent -and $candidateFullPath.Equals(
        $parentFullPath,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        return $candidateFullPath
    }

    $requiredPrefix = $parentFullPath + [System.IO.Path]::DirectorySeparatorChar
    if (-not $candidateFullPath.StartsWith(
        $requiredPrefix,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Path is outside the required root: $candidateFullPath"
    }

    return $candidateFullPath
}

function Assert-SafeTemporaryDirectory {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$ParentPath,

        [Parameter(Mandatory)]
        [string]$CandidatePath,

        [Parameter(Mandatory)]
        [string]$RequiredLeafPrefix
    )

    $candidateFullPath = Assert-ChildPath -ParentPath $ParentPath -CandidatePath $CandidatePath
    $leaf = [System.IO.Path]::GetFileName($candidateFullPath)
    if (-not $leaf.StartsWith($RequiredLeafPrefix, [System.StringComparison]::Ordinal)) {
        throw "Temporary directory does not have the required prefix: $candidateFullPath"
    }

    return $candidateFullPath
}

function Assert-DirectoryNotReparsePoint {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Container)) {
        throw "Required directory is missing: $Path"
    }
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Directory cannot be a reparse point: $Path"
    }
}

function Assert-PathComponentsNotReparsePoints {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $volumeRoot = [System.IO.Path]::GetPathRoot($fullPath)
    if ([string]::IsNullOrWhiteSpace($volumeRoot)) {
        throw "Path has no filesystem root: $fullPath"
    }
    $current = $volumeRoot.TrimEnd('\', '/')
    if ([string]::IsNullOrWhiteSpace($current)) {
        $current = $volumeRoot
    }
    $relative = $fullPath.Substring($volumeRoot.Length)
    foreach ($component in $relative.Split(
        @([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar),
        [System.StringSplitOptions]::RemoveEmptyEntries
    )) {
        $current = Join-Path $current $component
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Path contains a reparse-point component: $current"
            }
        }
    }
}

function Get-PackagingSourceState {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot
    )

    $root = (Resolve-Path -LiteralPath $RepositoryRoot).Path
    Assert-PathComponentsNotReparsePoints -Path $root
    $commit = (& git -C $root rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $commit -cnotmatch '^[0-9a-f]{40}$') {
        throw 'Could not resolve the packaging source commit.'
    }
    $branchOutput = @(& git -C $root branch --show-current)
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not resolve the packaging source branch.'
    }
    $branch = ($branchOutput -join '').Trim()
    $tree = (& git -C $root rev-parse ("{0}^{{tree}}" -f $commit)).Trim()
    if ($LASTEXITCODE -ne 0 -or $tree -cnotmatch '^[0-9a-f]{40}$') {
        throw 'Could not resolve the packaging source tree.'
    }
    $status = @(& git -C $root -c core.quotepath=false status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not inspect packaging source status.'
    }
    $relativePaths = @(
        & git -C $root -c core.quotepath=false ls-files --cached --others --exclude-standard
    )
    if ($LASTEXITCODE -ne 0 -or $relativePaths.Count -eq 0) {
        throw 'Could not enumerate the packaging public source snapshot.'
    }
    $records = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in @($relativePaths | Sort-Object -CaseSensitive)) {
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            continue
        }
        $fullPath = [System.IO.Path]::GetFullPath((Join-Path $root $relativePath))
        Assert-ChildPath -ParentPath $root -CandidatePath $fullPath | Out-Null
        Assert-PathComponentsNotReparsePoints -Path $fullPath
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            $records.Add("missing`0$relativePath")
            continue
        }
        $item = Get-Item -LiteralPath $fullPath -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Packaging source snapshot contains a reparse-point file: $relativePath"
        }
        $hash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $portablePath = $relativePath.Replace('\', '/')
        $records.Add("file`0$portablePath`0$($item.Length)`0$hash")
    }
    $payload = [System.Text.UTF8Encoding]::new($false).GetBytes(($records -join "`n"))
    $snapshotHash = [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($payload)
    ).ToLowerInvariant()
    return [pscustomobject]@{
        Commit = $commit
        Branch = $branch
        Tree = $tree
        Status = @($status)
        Dirty = ($status.Count -gt 0)
        DirtyEntryCount = $status.Count
        PublicSnapshotSha256 = $snapshotHash
        PublicSnapshotFileCount = $records.Count
    }
}

function Assert-PackagingSourceStateUnchanged {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [object]$Before,

        [Parameter(Mandatory)]
        [object]$After,

        [string]$Context = 'Packaging source'
    )

    if ([string]$After.Commit -cne [string]$Before.Commit -or
        [string]$After.Branch -cne [string]$Before.Branch -or
        [string]$After.Tree -cne [string]$Before.Tree -or
        (@($After.Status) -join "`n") -cne (@($Before.Status) -join "`n") -or
        [string]$After.PublicSnapshotSha256 -cne [string]$Before.PublicSnapshotSha256 -or
        [int64]$After.PublicSnapshotFileCount -ne [int64]$Before.PublicSnapshotFileCount) {
        throw "$Context changed while the artifact was being built."
    }
}

function Get-CodecPackInstallRoot {
    [CmdletBinding()]
    param(
        [ValidateSet('CurrentUser', 'AllUsers')]
        [string]$Scope = 'CurrentUser',

        [string]$InstallRoot
    )

    if (-not [string]::IsNullOrWhiteSpace($InstallRoot)) {
        return [System.IO.Path]::GetFullPath($InstallRoot)
    }
    if ($Scope -eq 'CurrentUser') {
        if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
            throw 'LOCALAPPDATA is unavailable for a current-user Codec Pack install.'
        }
        return Join-Path $env:LOCALAPPDATA 'LatentDeck/CodecPacks'
    }
    if ([string]::IsNullOrWhiteSpace($env:ProgramData)) {
        throw 'ProgramData is unavailable for an all-users Codec Pack install.'
    }
    return Join-Path $env:ProgramData 'LatentDeck/CodecPacks'
}

function Get-CodecPackAuxiliaryRoot {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$InstallRoot,

        [Parameter(Mandatory)]
        [ValidateSet('Staging', 'Trash')]
        [string]$Kind
    )

    $installFullPath = [System.IO.Path]::GetFullPath($InstallRoot).TrimEnd('\', '/')
    $parent = [System.IO.Path]::GetDirectoryName($installFullPath)
    $leaf = [System.IO.Path]::GetFileName($installFullPath)
    if ([string]::IsNullOrWhiteSpace($parent) -or [string]::IsNullOrWhiteSpace($leaf)) {
        throw "Codec Pack install root cannot have a safe sibling work root: $installFullPath"
    }
    $auxiliaryLeaf = if ($leaf -ieq 'CodecPacks') {
        "CodecPack$Kind"
    } else {
        "$leaf.CodecPack$Kind"
    }
    $auxiliaryRoot = Join-Path $parent $auxiliaryLeaf
    Assert-ChildPath -ParentPath $parent -CandidatePath $auxiliaryRoot | Out-Null
    if ($auxiliaryRoot.Equals($installFullPath, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Codec Pack work root overlaps the discovery root.'
    }
    return $auxiliaryRoot
}

function Remove-SafeTemporaryDirectory {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$ParentPath,

        [Parameter(Mandatory)]
        [string]$CandidatePath,

        [Parameter(Mandatory)]
        [string]$RequiredLeafPrefix
    )

    if (-not (Test-Path -LiteralPath $CandidatePath)) {
        return
    }

    $candidateFullPath = Assert-SafeTemporaryDirectory `
        -ParentPath $ParentPath `
        -CandidatePath $CandidatePath `
        -RequiredLeafPrefix $RequiredLeafPrefix
    $candidateItem = Get-Item -LiteralPath $candidateFullPath -Force
    if (-not $candidateItem.PSIsContainer -or
        ($candidateItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to remove a non-directory or reparse-point work path: $candidateFullPath"
    }
    Assert-PathComponentsNotReparsePoints -Path $candidateFullPath
    Remove-Item -LiteralPath $candidateFullPath -Recurse -Force
}

function Assert-Token {
    param(
        [Parameter(Mandatory)]
        [string]$Value,

        [Parameter(Mandatory)]
        [string]$Name
    )

    if ($Value.Length -gt 128 -or
        $Value -cnotmatch '^[a-z][a-z0-9-]*(?:\.[a-z][a-z0-9-]*)*$') {
        throw "$Name is not a bounded lowercase identifier."
    }
}

function Assert-SemVer {
    param(
        [Parameter(Mandatory)]
        [string]$Value,

        [Parameter(Mandatory)]
        [string]$Name
    )

    $numeric = '(?:0|[1-9][0-9]*)'
    $alphaNumeric = '(?:[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)'
    $preRelease = "(?:$numeric|$alphaNumeric)"
    $build = '[0-9A-Za-z-]+'
    $pattern = "^(?<major>$numeric)\.(?<minor>$numeric)\.(?<patch>$numeric)" +
        "(?:-(?<pre>$preRelease(?:\.$preRelease)*))?" +
        "(?:\+$build(?:\.$build)*)?$"
    $match = [regex]::Match($Value, $pattern, [System.Text.RegularExpressions.RegexOptions]::CultureInvariant)
    if (-not $match.Success) {
        throw "$Name is not canonical SemVer."
    }
    foreach ($component in @('major', 'minor', 'patch')) {
        $parsed = [uint64]0
        if (-not [uint64]::TryParse(
            $match.Groups[$component].Value,
            [System.Globalization.NumberStyles]::None,
            [System.Globalization.CultureInfo]::InvariantCulture,
            [ref]$parsed
        )) {
            throw "$Name contains a SemVer numeric component outside the u64 range."
        }
    }
    if ($match.Groups['pre'].Success) {
        foreach ($identifier in $match.Groups['pre'].Value.Split('.')) {
            if ($identifier -cmatch '^[0-9]+$') {
                $parsed = [uint64]0
                if (-not [uint64]::TryParse(
                    $identifier,
                    [System.Globalization.NumberStyles]::None,
                    [System.Globalization.CultureInfo]::InvariantCulture,
                    [ref]$parsed
                )) {
                    throw "$Name contains a SemVer prerelease number outside the u64 range."
                }
            }
        }
    }
}

function Assert-Sha256 {
    param(
        [Parameter(Mandatory)]
        [string]$Value,

        [Parameter(Mandatory)]
        [string]$Name
    )

    if ($Value -cnotmatch '^[0-9a-f]{64}$') {
        throw "$Name is not a lowercase SHA-256 value."
    }
}

function Assert-ExactProperties {
    param(
        [Parameter(Mandatory)]
        [object]$Object,

        [Parameter(Mandatory)]
        [string[]]$Required,

        [string[]]$Optional = @(),

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($null -eq $Object -or $Object -isnot [pscustomobject]) {
        throw "$Context must be a JSON object."
    }

    $actual = @($Object.PSObject.Properties.Name)
    foreach ($name in $Required) {
        if ($actual -cnotcontains $name) {
            throw "$Context is missing required field '$name'."
        }
    }

    $allowed = @($Required) + @($Optional)
    foreach ($name in $actual) {
        if ($allowed -cnotcontains $name) {
            throw "$Context contains unknown field '$name'."
        }
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

function Read-StrictJsonElement {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required JSON file is missing: $Path"
    }

    $file = Get-Item -LiteralPath $Path
    if ($file.Length -eq 0 -or $file.Length -gt $script:MaximumJsonBytes) {
        throw "JSON file is empty or exceeds the one MiB limit: $Path"
    }

    $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "JSON file is not strict UTF-8: $Path"
    }
    $document = [System.Text.Json.JsonDocument]::Parse([string]$text)
    try {
        Assert-NoDuplicateJsonProperties -Element $document.RootElement
        return $document.RootElement.Clone()
    } finally {
        $document.Dispose()
    }
}

function Read-StrictJsonFile {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $root = Read-StrictJsonElement -Path $Path
    return $root.GetRawText() | ConvertFrom-Json
}

function Get-JsonPropertyElement {
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

function Assert-JsonStringProperty {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $value = Get-JsonPropertyElement -Object $Object -Name $Name -Context $Context
    if ($value.ValueKind -ne [System.Text.Json.JsonValueKind]::String) {
        throw "$Context.$Name must be a JSON string."
    }
}

function Assert-JsonBooleanProperty {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $value = Get-JsonPropertyElement -Object $Object -Name $Name -Context $Context
    if ($value.ValueKind -ne [System.Text.Json.JsonValueKind]::True -and
        $value.ValueKind -ne [System.Text.Json.JsonValueKind]::False) {
        throw "$Context.$Name must be a JSON boolean."
    }
}

function Assert-JsonNullableStringProperty {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $value = Get-JsonPropertyElement -Object $Object -Name $Name -Context $Context
    if ($value.ValueKind -ne [System.Text.Json.JsonValueKind]::String -and
        $value.ValueKind -ne [System.Text.Json.JsonValueKind]::Null) {
        throw "$Context.$Name must be a JSON string or null."
    }
}

function Assert-JsonUnsignedIntegerProperty {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [uint64]$Maximum,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $value = Get-JsonPropertyElement -Object $Object -Name $Name -Context $Context
    $raw = $value.GetRawText()
    $parsed = [uint64]0
    if ($value.ValueKind -ne [System.Text.Json.JsonValueKind]::Number -or
        $raw -cnotmatch '^(?:0|[1-9][0-9]*)$' -or
        -not [uint64]::TryParse(
            $raw,
            [System.Globalization.NumberStyles]::None,
            [System.Globalization.CultureInfo]::InvariantCulture,
            [ref]$parsed
        ) -or
        $parsed -gt $Maximum) {
        throw "$Context.$Name must be an unsigned JSON integer no greater than $Maximum."
    }
}

function Get-JsonArrayElements {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $value = Get-JsonPropertyElement -Object $Object -Name $Name -Context $Context
    if ($value.ValueKind -ne [System.Text.Json.JsonValueKind]::Array) {
        throw "$Context.$Name must be a JSON array."
    }
    return @($value.EnumerateArray() | ForEach-Object { $_.Clone() })
}

function Assert-JsonStringArrayProperty {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $index = 0
    foreach ($value in @(Get-JsonArrayElements -Object $Object -Name $Name -Context $Context)) {
        if ($value.ValueKind -ne [System.Text.Json.JsonValueKind]::String) {
            throw "$Context.$Name[$index] must be a JSON string."
        }
        $index += 1
    }
}

function Assert-CodecPackManifestJsonTypes {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Root
    )

    $context = 'codec-pack.json'
    foreach ($name in @('manifest_version', 'kind', 'pack_id', 'pack_version', 'display_name', 'summary')) {
        Assert-JsonStringProperty -Object $Root -Name $name -Context $context
    }

    $publisher = Get-JsonPropertyElement -Object $Root -Name 'publisher' -Context $context
    Assert-JsonStringProperty -Object $publisher -Name 'name' -Context 'publisher'
    Assert-JsonNullableStringProperty -Object $publisher -Name 'url' -Context 'publisher'
    Assert-JsonStringProperty -Object $publisher -Name 'identity_claim' -Context 'publisher'

    $license = Get-JsonPropertyElement -Object $Root -Name 'license' -Context $context
    foreach ($name in @('spdx_or_label', 'notice_path')) {
        Assert-JsonStringProperty -Object $license -Name $name -Context 'license'
    }

    $platform = Get-JsonPropertyElement -Object $Root -Name 'platform' -Context $context
    foreach ($name in @('os', 'arch')) {
        Assert-JsonStringProperty -Object $platform -Name $name -Context 'platform'
    }

    $compatibility = Get-JsonPropertyElement -Object $Root -Name 'compatibility' -Context $context
    foreach ($name in @('app_min_inclusive', 'app_max_exclusive')) {
        Assert-JsonStringProperty -Object $compatibility -Name $name -Context 'compatibility'
    }
    foreach ($name in @('worker_protocol', 'codec_adapter_api')) {
        Assert-JsonUnsignedIntegerProperty -Object $compatibility -Name $name -Maximum ([uint16]::MaxValue) -Context 'compatibility'
    }
    foreach ($name in @('tensor_abi', 'torch_exact_build')) {
        Assert-JsonStringProperty -Object $compatibility -Name $name -Context 'compatibility'
    }
    $python = Get-JsonPropertyElement -Object $compatibility -Name 'python' -Context 'compatibility'
    foreach ($name in @('implementation', 'version', 'platform_tag')) {
        Assert-JsonStringProperty -Object $python -Name $name -Context 'compatibility.python'
    }
    Assert-JsonStringArrayProperty -Object $compatibility -Name 'lc_spec_versions' -Context 'compatibility'
    $profileIndex = 0
    foreach ($profile in @(Get-JsonArrayElements -Object $compatibility -Name 'profiles' -Context 'compatibility')) {
        if ($profile.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
            throw "compatibility.profiles[$profileIndex] must be a JSON object."
        }
        foreach ($name in @('codec_family', 'profile', 'profile_version')) {
            Assert-JsonStringProperty -Object $profile -Name $name -Context "compatibility.profiles[$profileIndex]"
        }
        $profileIndex += 1
    }

    $worker = Get-JsonPropertyElement -Object $Root -Name 'worker' -Context $context
    foreach ($name in @('executable', 'working_directory')) {
        Assert-JsonStringProperty -Object $worker -Name $name -Context 'worker'
    }
    Assert-JsonStringArrayProperty -Object $worker -Name 'arguments' -Context 'worker'
    foreach ($name in @('start_timeout_ms', 'heartbeat_timeout_ms')) {
        Assert-JsonUnsignedIntegerProperty -Object $worker -Name $name -Maximum ([uint32]::MaxValue) -Context 'worker'
    }

    $adapter = Get-JsonPropertyElement -Object $Root -Name 'adapter' -Context $context
    foreach ($name in @('adapter_id', 'adapter_version', 'entrypoint')) {
        Assert-JsonStringProperty -Object $adapter -Name $name -Context 'adapter'
    }
    Assert-JsonStringArrayProperty -Object $Root -Name 'capabilities' -Context $context
    $runtimeLock = Get-JsonPropertyElement -Object $Root -Name 'runtime_lock' -Context $context
    foreach ($name in @('path', 'sha256')) {
        Assert-JsonStringProperty -Object $runtimeLock -Name $name -Context 'runtime_lock'
    }
    $integrity = Get-JsonPropertyElement -Object $Root -Name 'integrity' -Context $context
    foreach ($name in @('catalog_path', 'catalog_sha256')) {
        Assert-JsonStringProperty -Object $integrity -Name $name -Context 'integrity'
    }

    $assetIndex = 0
    foreach ($asset in @(Get-JsonArrayElements -Object $Root -Name 'external_assets' -Context $context)) {
        if ($asset.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
            throw "external_assets[$assetIndex] must be a JSON object."
        }
        foreach ($name in @(
            'asset_id', 'display_name', 'sha256', 'source_url', 'license_label', 'license_url'
        )) {
            Assert-JsonStringProperty -Object $asset -Name $name -Context "external_assets[$assetIndex]"
        }
        Assert-JsonBooleanProperty -Object $asset -Name 'required' -Context "external_assets[$assetIndex]"
        Assert-JsonUnsignedIntegerProperty `
            -Object $asset `
            -Name 'byte_length' `
            -Maximum ([uint64]::MaxValue) `
            -Context "external_assets[$assetIndex]"
        $assetIndex += 1
    }
}

function Assert-IntegrityCatalogJsonTypes {
    param(
        [Parameter(Mandatory)]
        [System.Text.Json.JsonElement]$Root
    )

    Assert-JsonStringProperty -Object $Root -Name 'manifest_version' -Context 'integrity.json'
    $index = 0
    foreach ($entry in @(Get-JsonArrayElements -Object $Root -Name 'files' -Context 'integrity.json')) {
        if ($entry.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
            throw "integrity.json.files[$index] must be a JSON object."
        }
        foreach ($name in @('path', 'sha256')) {
            Assert-JsonStringProperty -Object $entry -Name $name -Context "integrity.json.files[$index]"
        }
        Assert-JsonUnsignedIntegerProperty `
            -Object $entry `
            -Name 'byte_length' `
            -Maximum ([uint64]::MaxValue) `
            -Context "integrity.json.files[$index]"
        $index += 1
    }
}

function Write-JsonFile {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [object]$Value,

        [Parameter(Mandatory)]
        [string]$Path
    )

    $json = $Value | ConvertTo-Json -Depth 32
    [System.IO.File]::WriteAllText(
        $Path,
        $json + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Convert-ToPortableRelativePath {
    param(
        [Parameter(Mandatory)]
        [string]$RootPath,

        [Parameter(Mandatory)]
        [string]$Path
    )

    $relative = [System.IO.Path]::GetRelativePath(
        [System.IO.Path]::GetFullPath($RootPath),
        [System.IO.Path]::GetFullPath($Path)
    )
    return $relative.Replace('\', '/')
}

function Assert-PortableRelativePath {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ([string]::IsNullOrWhiteSpace($Path) -or
        $Path.Length -gt 4096 -or
        $Path.StartsWith('/') -or
        $Path.Contains('\') -or
        $Path.Contains(':') -or
        $Path.Contains([char]0)) {
        throw "$Context is not a safe portable relative path."
    }

    $components = $Path.Split('/')
    if ($components.Count -eq 0 -or @($components | Where-Object {
        [string]::IsNullOrWhiteSpace($_) -or $_ -eq '.' -or $_ -eq '..'
    }).Count -gt 0) {
        throw "$Context contains an unsafe path component."
    }
}

function Assert-PortableTextPolicy {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$Text,

        [Parameter(Mandatory)]
        [string]$Context,

        [switch]$Sensitive
    )

    if ($Text.Contains([char]0)) {
        throw "$Context contains a NUL byte and is not portable text."
    }
    if ($Sensitive) {
        if ($Text -match '(?im)(?:^|[\s"''(=])(?:file:///)?[A-Za-z]:[\\/]' -or
            $Text -match '(?im)/(?:Users|home)/[^/\s]+/' -or
            $Text -match '(?im)\\\\[A-Za-z0-9][A-Za-z0-9._-]{0,63}\\[A-Za-z0-9$][A-Za-z0-9$._-]{0,63}(?:\\|[\s"''])') {
            throw "$Context contains a machine-local absolute path."
        }
    }
    if ($Text -match '(?im)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----' -or
        $Text -match '(?i)\bAKIA[0-9A-Z]{16}\b' -or
        $Text -match '(?i)\bgh[pousr]_[A-Za-z0-9]{20,}\b' -or
        $Text -match '(?i)\bsk-[A-Za-z0-9_-]{20,}\b') {
        throw "$Context contains credential-like material."
    }

    $credentialPattern = '(?im)\b(?<key>api[_-]?key|access[_-]?token|auth[_-]?token|secret|password)\b(?:["'']\s*)?\s*[:=]\s*(?:"(?<double>[^"\r\n]{8,})"|''(?<single>[^''\r\n]{8,})'')'
    $credentialMatches = @([regex]::Matches($Text, $credentialPattern))
    if ($credentialMatches.Count -gt 0) {
        $nonPlaceholder = @($credentialMatches | Where-Object {
            $value = if ($_.Groups['double'].Success) {
                $_.Groups['double'].Value
            } else {
                $_.Groups['single'].Value
            }
            $key = $_.Groups['key'].Value
            $cursor = $_.Index + $_.Length
            while ($cursor -lt $Text.Length -and
                ($Text[$cursor] -eq [char]' ' -or $Text[$cursor] -eq [char]9)) {
                $cursor += 1
            }
            $hasSafeExpressionTerminator = (
                $cursor -eq $Text.Length -or
                $Text[$cursor] -eq [char]10 -or
                $Text[$cursor] -eq [char]13 -or
                '#,)]};'.Contains($Text[$cursor])
            )
            -not (
                $key -ceq 'password' -and
                $value -ceq 'password' -and
                $hasSafeExpressionTerminator
            )
        })
        # Executable source commonly documents APIs with the literal
        # `password='password'`. It is exempt only when the literal ends the
        # expression (EOL/comment or a closing/separating delimiter), so string
        # concatenation and conditional expressions remain credential-like.
        # Sensitive metadata/configuration remains strict.
        if ($Sensitive -or $nonPlaceholder.Count -gt 0) {
            throw "$Context contains credential-like material."
        }
    }
}

function Test-PortableTextCandidate {
    param(
        [Parameter(Mandatory)]
        [string]$Name
    )

    $leaf = [System.IO.Path]::GetFileName($Name)
    $extension = [System.IO.Path]::GetExtension($leaf).ToLowerInvariant()
    return (
        $script:PortableTextExtensions -contains $extension -or
        [string]::IsNullOrEmpty($extension)
    )
}

function Assert-NotForbiddenLeafName {
    param(
        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $leaf = [System.IO.Path]::GetFileName($Name)
    if ($leaf -ieq '.env' -or $leaf.StartsWith('.env.', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Context contains a forbidden environment file '$leaf'."
    }
}

function Assert-PortableTextFile {
    param(
        [Parameter(Mandatory)]
        [System.IO.FileInfo]$File,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if (-not (Test-PortableTextCandidate -Name $File.Name)) {
        return
    }
    if ($File.Length -gt 4MB) {
        throw "$Context exceeds the four MiB portable-text inspection limit."
    }
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString(
            [System.IO.File]::ReadAllBytes($File.FullName)
        )
    } catch [System.Text.DecoderFallbackException] {
        throw "$Context is expected to be UTF-8 text but contains invalid bytes."
    }
    $extension = $File.Extension.ToLowerInvariant()
    $sensitive = $script:SensitivePortableTextExtensions -contains $extension
    Assert-PortableTextPolicy -Text $text -Context $Context -Sensitive:$sensitive
}

function Read-BoundedStreamBytes {
    param(
        [Parameter(Mandatory)]
        [System.IO.Stream]$Stream,

        [Parameter(Mandatory)]
        [int64]$MaximumBytes,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $memory = [System.IO.MemoryStream]::new()
    $buffer = [byte[]]::new(81920)
    try {
        while ($true) {
            $read = $Stream.Read($buffer, 0, $buffer.Length)
            if ($read -eq 0) {
                break
            }
            if ([int64]$read -gt $MaximumBytes - $memory.Length) {
                throw "$Context exceeds the four MiB portable linker-script inspection limit."
            }
            $memory.Write($buffer, 0, $read)
        }
        return ,$memory.ToArray()
    } finally {
        $memory.Dispose()
    }
}

function Test-ZipArchiveBytes {
    param(
        [Parameter(Mandatory)]
        [byte[]]$Bytes
    )

    $stream = [System.IO.MemoryStream]::new($Bytes, $false)
    $archive = $null
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Read,
            $true,
            [System.Text.Encoding]::UTF8
        )
        return $true
    } catch [System.IO.InvalidDataException] {
        return $false
    } finally {
        if ($null -ne $archive) {
            $archive.Dispose()
        }
        $stream.Dispose()
    }
}

function Assert-PortableLinkerScriptBytes {
    param(
        [Parameter(Mandatory)]
        [byte[]]$Bytes,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($Bytes.Length -eq 0 -or $Bytes.Length -gt 4MB) {
        throw "$Context must be a non-empty linker script within the four MiB inspection limit."
    }
    if (Test-ZipArchiveBytes -Bytes $Bytes) {
        throw "$Context is a nested Deck archive, not a portable linker script."
    }
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($Bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "$Context uses the ambiguous .ld extension but is not valid UTF-8 linker-script text."
    }
    Assert-PortableTextPolicy -Text $text -Context $Context -Sensitive:$false
}

function Assert-PortableLinkerScriptFile {
    param(
        [Parameter(Mandatory)]
        [System.IO.FileInfo]$File,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($File.Length -eq 0 -or $File.Length -gt 4MB) {
        throw "$Context must be a non-empty linker script within the four MiB inspection limit."
    }
    Assert-PortableLinkerScriptBytes `
        -Bytes ([System.IO.File]::ReadAllBytes($File.FullName)) `
        -Context $Context
}

function Assert-PortableLinkerScriptArchiveEntry {
    param(
        [Parameter(Mandatory)]
        [System.IO.Compression.ZipArchiveEntry]$Entry,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ([int64]$Entry.Length -eq 0 -or [int64]$Entry.Length -gt 4MB) {
        throw "$Context must be a non-empty linker script within the four MiB inspection limit."
    }
    $stream = $Entry.Open()
    try {
        [byte[]]$bytes = Read-BoundedStreamBytes `
            -Stream $stream `
            -MaximumBytes 4MB `
            -Context $Context
    } finally {
        $stream.Dispose()
    }
    Assert-PortableLinkerScriptBytes -Bytes $bytes -Context $Context
}

function Assert-NoForbiddenArchiveEntries {
    param(
        [Parameter(Mandatory)]
        [string]$ArchivePath
    )

    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        if ($archive.Entries.Count -eq 0 -or $archive.Entries.Count -gt 10000) {
            throw "Approved Python standard-library archive is empty or has too many entries: $ArchivePath"
        }
        $seen = [System.Collections.Generic.HashSet[string]]::new(
            [System.StringComparer]::OrdinalIgnoreCase
        )
        $total = [int64]0
        foreach ($entry in $archive.Entries) {
            $name = $entry.FullName.TrimEnd('/')
            if ([string]::IsNullOrWhiteSpace($name)) {
                continue
            }
            Assert-PortableRelativePath -Path $name -Context 'nested archive entry'
            if (-not $seen.Add($name)) {
                throw "Nested archive contains a duplicate entry: $name"
            }
            Assert-NotForbiddenLeafName -Name $name -Context 'nested archive'
            foreach ($component in $name.Split('/')) {
                if ($script:ForbiddenDirectoryNames -contains $component) {
                    throw "Nested archive contains a forbidden directory '$component'."
                }
            }
            if ([int64]$entry.Length -gt [int64]::MaxValue - $total) {
                throw "Nested archive size overflowed its counter: $ArchivePath"
            }
            $total = [int64]($total + [int64]$entry.Length)
            if ($total -gt 512MB) {
                throw "Nested archive exceeds the 512 MiB inspection limit: $ArchivePath"
            }
            $extension = [System.IO.Path]::GetExtension($name).ToLowerInvariant()
            if ($script:ForbiddenPayloadExtensions -contains $extension) {
                throw "Nested archive contains forbidden payload type '$extension'."
            }
            if ($extension -ceq '.ld') {
                Assert-PortableLinkerScriptArchiveEntry `
                    -Entry $entry `
                    -Context "nested archive entry '$name'"
            } elseif ($script:ArchiveExtensions -contains $extension) {
                throw "Approved Python standard-library archive contains a nested archive '$name'."
            }
            if (Test-PortableTextCandidate -Name $name) {
                if ([int64]$entry.Length -gt 4MB) {
                    throw "Nested text entry '$name' exceeds the four MiB inspection limit."
                }
                $entryStream = $entry.Open()
                try {
                    $reader = [System.IO.StreamReader]::new(
                        $entryStream,
                        [System.Text.UTF8Encoding]::new($false, $true),
                        $true,
                        4096,
                        $true
                    )
                    try {
                        $text = $reader.ReadToEnd()
                    } finally {
                        $reader.Dispose()
                    }
                } catch [System.Text.DecoderFallbackException] {
                    throw "Nested text entry '$name' is not valid UTF-8."
                } finally {
                    $entryStream.Dispose()
                }
                $extension = [System.IO.Path]::GetExtension($name).ToLowerInvariant()
                $sensitive = $script:SensitivePortableTextExtensions -contains $extension
                Assert-PortableTextPolicy `
                    -Text $text `
                    -Context "nested archive entry '$name'" `
                    -Sensitive:$sensitive
            }
        }
    } finally {
        $archive.Dispose()
    }
}

function Assert-PackagingSourceTree {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$RootPath,

        [int]$MaximumFiles = $script:MaximumCatalogFiles,

        [int64]$MaximumBytes = $script:MaximumPackBytes,

        [string[]]$AllowedArchiveRelativePaths = @()
    )

    if (-not (Test-Path -LiteralPath $RootPath -PathType Container)) {
        throw "Packaging source directory is missing: $RootPath"
    }

    $resolvedRoot = (Resolve-Path -LiteralPath $RootPath).Path
    $rootItem = Get-Item -LiteralPath $resolvedRoot -Force
    if (($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Packaging source root cannot be a reparse point: $resolvedRoot"
    }

    $items = @(Get-ChildItem -LiteralPath $resolvedRoot -Force -Recurse)
    $files = @($items | Where-Object { -not $_.PSIsContainer })
    if ($files.Count -gt $MaximumFiles) {
        throw "Packaging source exceeds the bounded file count: $($files.Count)"
    }

    $totalBytes = [int64]0
    $allowedArchives = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($allowedArchive in $AllowedArchiveRelativePaths) {
        Assert-PortableRelativePath -Path $allowedArchive -Context 'allowed archive path'
        if (-not $allowedArchives.Add($allowedArchive)) {
            throw "Allowed archive path is duplicated: $allowedArchive"
        }
    }
    foreach ($item in $items) {
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Packaging source contains a reparse point: $($item.FullName)"
        }
        if ($item.PSIsContainer -and $script:ForbiddenDirectoryNames -contains $item.Name) {
            throw "Packaging source contains a cache or repository directory: $($item.Name)"
        }
    }

    foreach ($file in $files) {
        if ([int64]$file.Length -gt [int64]::MaxValue - $totalBytes) {
            throw 'Packaging source size overflowed its counter.'
        }
        $totalBytes = [int64]($totalBytes + [int64]$file.Length)
        if ($totalBytes -gt $MaximumBytes) {
            throw "Packaging source exceeds the bounded byte count."
        }

        $relativePath = Convert-ToPortableRelativePath -RootPath $resolvedRoot -Path $file.FullName
        Assert-NotForbiddenLeafName -Name $relativePath -Context 'packaging source'
        $extension = $file.Extension.ToLowerInvariant()
        if ($script:ForbiddenPayloadExtensions -contains $extension) {
            throw "Packaging source contains forbidden payload type '$extension': $($file.Name)"
        }
        if ($extension -ceq '.ld') {
            Assert-PortableLinkerScriptFile `
                -File $file `
                -Context "packaging source '$relativePath'"
        } elseif ($script:ArchiveExtensions -contains $extension) {
            if (-not $allowedArchives.Contains($relativePath)) {
                throw "Packaging source contains an unapproved archive: $relativePath"
            }
            Assert-NoForbiddenArchiveEntries -ArchivePath $file.FullName
        }
        Assert-PortableTextFile -File $file -Context "packaging source '$relativePath'"
    }

    return $files
}

function Copy-PackagingTree {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$SourcePath,

        [Parameter(Mandatory)]
        [string]$DestinationPath,

        [int]$MaximumFiles = $script:MaximumCatalogFiles,

        [string[]]$AllowedArchiveRelativePaths = @()
    )

    $sourceRoot = (Resolve-Path -LiteralPath $SourcePath).Path
    $files = @(Assert-PackagingSourceTree `
        -RootPath $sourceRoot `
        -MaximumFiles $MaximumFiles `
        -AllowedArchiveRelativePaths $AllowedArchiveRelativePaths)
    [System.IO.Directory]::CreateDirectory($DestinationPath) | Out-Null

    foreach ($file in $files | Sort-Object FullName) {
        $relative = [System.IO.Path]::GetRelativePath($sourceRoot, $file.FullName)
        $destination = Join-Path $DestinationPath $relative
        Assert-ChildPath -ParentPath $DestinationPath -CandidatePath $destination | Out-Null
        if (Test-Path -LiteralPath $destination) {
            throw "Packaging source collision at: $relative"
        }
        $parent = Split-Path -Parent $destination
        [System.IO.Directory]::CreateDirectory($parent) | Out-Null
        [System.IO.File]::Copy($file.FullName, $destination, $false)
    }
}

function Get-IntegrityEntry {
    param(
        [Parameter(Mandatory)]
        [string]$RootPath,

        [Parameter(Mandatory)]
        [System.IO.FileInfo]$File
    )

    return [ordered]@{
        path = Convert-ToPortableRelativePath -RootPath $RootPath -Path $File.FullName
        byte_length = [int64]$File.Length
        sha256 = (Get-FileHash -LiteralPath $File.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Assert-WindowsX64PeFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Context,

        [switch]$RequireDll,

        [switch]$RequirePython313Version
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Context is missing: $Path"
    }
    $stream = [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read
    )
    try {
        if ($stream.Length -lt 256) {
            throw "$Context is too small to be a bounded PE32+ executable."
        }
        $reader = [System.IO.BinaryReader]::new($stream, [System.Text.Encoding]::UTF8, $true)
        try {
            if ($reader.ReadUInt16() -ne 0x5a4d) {
                throw "$Context has no DOS MZ signature."
            }
            $stream.Position = 0x3c
            $peOffset = $reader.ReadInt32()
            if ($peOffset -lt 64 -or $peOffset -gt ($stream.Length - 26)) {
                throw "$Context has an out-of-bounds PE header offset."
            }
            $stream.Position = $peOffset
            if ($reader.ReadUInt32() -ne 0x00004550) {
                throw "$Context has no PE signature."
            }
            $machine = $reader.ReadUInt16()
            $sectionCount = $reader.ReadUInt16()
            $stream.Position += 12
            $optionalHeaderBytes = $reader.ReadUInt16()
            $characteristics = $reader.ReadUInt16()
            if ($machine -ne 0x8664 -or $sectionCount -eq 0 -or
                ($characteristics -band 0x0002) -eq 0) {
                throw "$Context is not an executable Windows x86-64 PE file."
            }
            if ($RequireDll -and ($characteristics -band 0x2000) -eq 0) {
                throw "$Context is not marked as a Windows DLL."
            }
            if (-not $RequireDll -and ($characteristics -band 0x2000) -ne 0) {
                throw "$Context is a DLL, not the required executable."
            }
            if ($optionalHeaderBytes -lt 2 -or
                $peOffset + 24 + $optionalHeaderBytes -gt $stream.Length -or
                $reader.ReadUInt16() -ne 0x020b) {
                throw "$Context is not a bounded PE32+ image."
            }
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }

    if ($RequirePython313Version) {
        $version = [System.Diagnostics.FileVersionInfo]::GetVersionInfo(
            (Resolve-Path -LiteralPath $Path).Path
        )
        $expectedOriginal = if ($RequireDll) { 'python313.dll' } else { 'python.exe' }
        if ($version.FileMajorPart -ne 3 -or
            $version.FileMinorPart -ne 13 -or
            $version.OriginalFilename -ine $expectedOriginal -or
            $version.ProductName -inotmatch '^Python(?:$|\s)') {
            throw "$Context does not carry the expected CPython 3.13 version resource."
        }
    }
}

function Assert-Python313RuntimeLayout {
    param(
        [Parameter(Mandatory)]
        [string]$PackRoot,

        [Parameter(Mandatory)]
        [System.Collections.Generic.HashSet[string]]$CatalogPaths
    )

    $expectedNativePaths = [ordered]@{
        latentdeck_cartridge = (
            'runtime/Lib/site-packages/latentdeck_cartridge/_native.pyd'
        )
        latentdeck_rgb_ring = (
            'runtime/Lib/site-packages/latentdeck_rgb_ring/_native.cp313-win_amd64.pyd'
        )
    }
    $requiredPaths = @(
        'runtime/python.exe',
        'runtime/python313.dll',
        'runtime/python313._pth',
        'runtime/python313.zip',
        'runtime/Lib/site-packages/latentdeck_codec_h3/__init__.py',
        'runtime/Lib/site-packages/latentdeck_codec_h3/adapter.py',
        'runtime/Lib/site-packages/latentdeck_codec_host/__init__.py',
        'runtime/Lib/site-packages/latentdeck_codec_host/__main__.py',
        'runtime/Lib/site-packages/latentdeck_codec_host/runtime_v2.py',
        'runtime/Lib/site-packages/latentdeck_codec_host/native_cartridge.py',
        'runtime/Lib/site-packages/latentdeck_codec_sdk/__init__.py',
        'runtime/Lib/site-packages/latentdeck_deck_sdk/__init__.py',
        'runtime/Lib/site-packages/latentdeck_cartridge/__init__.py',
        $expectedNativePaths['latentdeck_cartridge'],
        'runtime/Lib/site-packages/latentdeck_rgb_ring/__init__.py',
        $expectedNativePaths['latentdeck_rgb_ring'],
        'THIRD_PARTY_NOTICES.md',
        'DEPENDENCY_INVENTORY.json',
        'NATIVE_RUST_LICENSES.json',
        'NATIVE_RUST_SBOM.cdx.json',
        'SBOM.cdx.json'
    )
    foreach ($requiredPath in $requiredPaths) {
        if (-not $CatalogPaths.Contains($requiredPath)) {
            throw "Codec Pack catalog omits required file '$requiredPath'."
        }
    }

    foreach ($nativeModule in $expectedNativePaths.Keys) {
        $nativeModuleRoot = "runtime/Lib/site-packages/$nativeModule"
        $expectedNativePath = [string]$expectedNativePaths[$nativeModule]
        $allowedTypingStubPath = "$nativeModuleRoot/_native.pyi"
        $nativeStemPath = "$nativeModuleRoot/_native"
        $nativeRelatedPaths = @($CatalogPaths | Where-Object {
            $_.Equals($nativeStemPath, [System.StringComparison]::OrdinalIgnoreCase) -or
            $_.StartsWith("$nativeStemPath.", [System.StringComparison]::OrdinalIgnoreCase) -or
            $_.StartsWith("$nativeStemPath/", [System.StringComparison]::OrdinalIgnoreCase)
        })
        $ambiguousNativePaths = @($nativeRelatedPaths | Where-Object {
            $_ -cne $expectedNativePath -and $_ -cne $allowedTypingStubPath
        })
        if (-not ($nativeRelatedPaths -ccontains $expectedNativePath) -or
            $ambiguousNativePaths.Count -gt 0) {
            throw (
                "Codec Pack must contain exactly the expected Windows x64 native binding " +
                "'$expectedNativePath', may contain only the non-executable typing stub " +
                "'$allowedTypingStubPath', and must contain no importable _native aliases."
            )
        }
    }

    $pythonPath = Join-Path $PackRoot 'runtime/python.exe'
    $pythonDllPath = Join-Path $PackRoot 'runtime/python313.dll'
    Assert-WindowsX64PeFile `
        -Path $pythonPath `
        -Context 'runtime/python.exe' `
        -RequirePython313Version
    Assert-WindowsX64PeFile `
        -Path $pythonDllPath `
        -Context 'runtime/python313.dll' `
        -RequireDll `
        -RequirePython313Version

    foreach ($binary in Get-ChildItem -LiteralPath (Join-Path $PackRoot 'runtime') -File -Recurse) {
        if (@('.dll', '.pyd') -contains $binary.Extension.ToLowerInvariant()) {
            Assert-WindowsX64PeFile `
                -Path $binary.FullName `
                -Context (Convert-ToPortableRelativePath -RootPath $PackRoot -Path $binary.FullName) `
                -RequireDll
        } elseif ($binary.Extension -ieq '.exe') {
            Assert-WindowsX64PeFile `
                -Path $binary.FullName `
                -Context (Convert-ToPortableRelativePath -RootPath $PackRoot -Path $binary.FullName)
        }
    }

    $pthPath = Join-Path $PackRoot 'runtime/python313._pth'
    $pthText = [System.Text.UTF8Encoding]::new($false, $true).GetString(
        [System.IO.File]::ReadAllBytes($pthPath)
    )
    Assert-PortableTextPolicy -Text $pthText -Context 'runtime/python313._pth' -Sensitive
    $activeLines = @(
        $pthText -split '\r?\n' |
            ForEach-Object { $_.Trim() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and -not $_.StartsWith('#') }
    )
    $expectedLines = @('python313.zip', '.', 'Lib/site-packages')
    if (($activeLines -join "`0") -cne ($expectedLines -join "`0")) {
        throw (
            'runtime/python313._pth must contain exactly python313.zip, dot, and ' +
            'Lib/site-packages, in that order, with no site import or machine path.'
        )
    }
}

function Assert-CodecPackDependencyMetadata {
    param(
        [Parameter(Mandatory)]
        [string]$PackRoot,

        [Parameter(Mandatory)]
        [string]$PackVersion
    )

    $inventory = Read-StrictJsonFile -Path (Join-Path $PackRoot 'DEPENDENCY_INVENTORY.json')
    Assert-ExactProperties -Object $inventory -Required @(
        'schema_version', 'pack_id', 'pack_version', 'source_commit', 'platform',
        'curator', 'components', 'native_rust'
    ) -Context 'DEPENDENCY_INVENTORY.json'
    Assert-ExactProperties -Object $inventory.curator -Required @(
        'name', 'schema_version'
    ) -Context 'DEPENDENCY_INVENTORY.json.curator'
    if ([int64]$inventory.schema_version -ne 1 -or
        $inventory.pack_id -cne 'org.latentdeck.h3' -or
        $inventory.pack_version -cne $PackVersion -or
        ([string]$inventory.source_commit) -cnotmatch '^[0-9a-f]{40}$' -or
        $inventory.platform -cne 'windows-x86_64' -or
        $inventory.curator.name -cne 'latentdeck-codec-pack-curator' -or
        [int64]$inventory.curator.schema_version -ne 1) {
        throw 'Codec Pack dependency inventory identity is invalid.'
    }

    $inventoryComponents = @($inventory.components)
    if ($inventoryComponents.Count -eq 0 -or $inventoryComponents.Count -gt 128) {
        throw 'Codec Pack dependency inventory has an invalid component count.'
    }
    $inventoryIds = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $inventoryById = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($component in $inventoryComponents) {
        Assert-ExactProperties -Object $component -Required @(
            'name', 'version', 'kind', 'source_url', 'license_expression',
            'license_files', 'content_sha256'
        ) -Context 'DEPENDENCY_INVENTORY.json component'
        $identity = "$($component.name)@$($component.version)"
        if ([string]::IsNullOrWhiteSpace([string]$component.name) -or
            [string]::IsNullOrWhiteSpace([string]$component.version) -or
            -not $inventoryIds.Add($identity)) {
            throw 'Codec Pack dependency inventory contains an invalid or duplicate identity.'
        }
        $inventoryById.Add($identity, $component)
        if (@('runtime', 'dependency', 'repository') -cnotcontains [string]$component.kind -or
            ([string]$component.source_url) -cnotmatch '^https://' -or
            [string]::IsNullOrWhiteSpace([string]$component.license_expression)) {
            throw "Codec Pack dependency inventory component '$identity' is incomplete."
        }
        Assert-Sha256 -Value ([string]$component.content_sha256) -Name 'component.content_sha256'
        $licenseFiles = @($component.license_files)
        if ($licenseFiles.Count -gt 512) {
            throw "Codec Pack dependency inventory component '$identity' has too many license files."
        }
        foreach ($licenseFile in $licenseFiles) {
            Assert-PortableRelativePath `
                -Path ([string]$licenseFile) `
                -Context "dependency inventory license file for '$identity'"
        }
    }

    Assert-ExactProperties -Object $inventory.native_rust -Required @(
        'sbom_path', 'sbom_sha256', 'license_bundle_path', 'license_bundle_sha256',
        'component_count', 'selection_roots'
    ) -Context 'DEPENDENCY_INVENTORY.json.native_rust'
    if ([string]$inventory.native_rust.sbom_path -cne 'NATIVE_RUST_SBOM.cdx.json' -or
        [string]$inventory.native_rust.license_bundle_path -cne 'NATIVE_RUST_LICENSES.json' -or
        [int64]$inventory.native_rust.component_count -lt 2 -or
        [int64]$inventory.native_rust.component_count -gt 256 -or
        (@($inventory.native_rust.selection_roots | Sort-Object) -join "`0") -cne
            ((@('latentdeck-cartridge-python', 'latentdeck-gpu-python') | Sort-Object) -join "`0")) {
        throw 'Codec Pack native Rust inventory identity is invalid.'
    }
    Assert-Sha256 -Value ([string]$inventory.native_rust.sbom_sha256) -Name 'native Rust SBOM SHA-256'
    Assert-Sha256 -Value ([string]$inventory.native_rust.license_bundle_sha256) -Name 'native Rust license bundle SHA-256'
    $nativeRustSbomPath = Join-Path $PackRoot 'NATIVE_RUST_SBOM.cdx.json'
    $nativeRustLicensesPath = Join-Path $PackRoot 'NATIVE_RUST_LICENSES.json'
    if ((Get-FileHash -LiteralPath $nativeRustSbomPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            [string]$inventory.native_rust.sbom_sha256 -or
        (Get-FileHash -LiteralPath $nativeRustLicensesPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            [string]$inventory.native_rust.license_bundle_sha256) {
        throw 'Codec Pack native Rust metadata differs from its dependency inventory binding.'
    }
    $nativeRustSbom = Read-StrictJsonFile -Path $nativeRustSbomPath
    Assert-ExactProperties -Object $nativeRustSbom -Required @(
        'bomFormat', 'specVersion', 'serialNumber', 'version', 'metadata', 'components'
    ) -Context 'NATIVE_RUST_SBOM.cdx.json'
    if ([string]$nativeRustSbom.bomFormat -cne 'CycloneDX' -or
        [string]$nativeRustSbom.specVersion -cne '1.5' -or
        [int64]$nativeRustSbom.version -ne 1 -or
        [string]$nativeRustSbom.metadata.component.'bom-ref' -cne
            "pkg:generic/LatentDeck%20H3%20Native%20Extensions@$PackVersion" -or
        [string]$nativeRustSbom.metadata.component.name -cne 'LatentDeck H3 Native Extensions' -or
        [string]$nativeRustSbom.metadata.component.version -cne $PackVersion) {
        throw 'Codec Pack native Rust SBOM artifact identity is invalid.'
    }
    $nativeRootScope = @($nativeRustSbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:dependency-scope' -and
        [string]$_.value -ceq 'artifact'
    })
    $nativeArtifactScope = @($nativeRustSbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:artifact-scope' -and
        [string]$_.value -ceq 'h3-native'
    })
    $nativeTargetPlatform = @($nativeRustSbom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:target-platform' -and
        [string]$_.value -ceq 'x86_64-pc-windows-msvc'
    })
    if ($nativeRootScope.Count -ne 1 -or $nativeArtifactScope.Count -ne 1 -or
        $nativeTargetPlatform.Count -ne 1) {
        throw 'Codec Pack native Rust SBOM scope is invalid.'
    }
    $nativeRustComponents = @($nativeRustSbom.components)
    if ($nativeRustComponents.Count -ne [int64]$inventory.native_rust.component_count) {
        throw 'Codec Pack native Rust SBOM component count differs from its inventory.'
    }
    $nativeRustByRef = @{}
    $nativeRootNames = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($component in $nativeRustComponents) {
        $reference = [string]$component.'bom-ref'
        $ecosystem = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:ecosystem' -and [string]$_.value -ceq 'rust'
        })
        $scope = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope' -and
            [string]$_.value -cin @('artifact', 'runtime', 'build', 'runtime+build')
        })
        $selectionRoot = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:selection-root' -and [string]$_.value -ceq 'true'
        })
        if ($reference -cnotmatch '^rust:.+@[^@]+$' -or
            $nativeRustByRef.ContainsKey($reference) -or
            $ecosystem.Count -ne 1 -or $scope.Count -ne 1 -or
            @($component.licenses).Count -eq 0) {
            throw "Codec Pack native Rust SBOM component is incomplete: $reference"
        }
        if ($selectionRoot.Count -eq 1) {
            [void]$nativeRootNames.Add([string]$component.name)
        } elseif ($selectionRoot.Count -gt 1) {
            throw "Codec Pack native Rust SBOM selection root is duplicated: $reference"
        }
        $nativeRustByRef[$reference] = $component
    }
    if ((@($nativeRootNames | Sort-Object) -join "`0") -cne
        ((@('latentdeck-cartridge-python', 'latentdeck-gpu-python') | Sort-Object) -join "`0")) {
        throw 'Codec Pack native Rust SBOM selection-root set is not exact.'
    }

    $nativeLicenseBundleItem = Get-Item -LiteralPath $nativeRustLicensesPath -Force
    if ($nativeLicenseBundleItem.PSIsContainer -or
        ($nativeLicenseBundleItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $nativeLicenseBundleItem.Length -eq 0 -or $nativeLicenseBundleItem.Length -gt 32MB) {
        throw 'Codec Pack native Rust license bundle is not a bounded regular file.'
    }
    $nativeLicenseBundle = Read-StrictJsonFile -Path $nativeRustLicensesPath
    Assert-ExactProperties -Object $nativeLicenseBundle -Required @(
        'schema_version', 'artifact', 'policy', 'sboms', 'component_count',
        'text_count', 'components', 'texts'
    ) -Context 'NATIVE_RUST_LICENSES.json'
    $nativeRootReference = [string]$nativeRustSbom.metadata.component.'bom-ref'
    $expectedNativeLicenseReferences = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    [void]$expectedNativeLicenseReferences.Add($nativeRootReference)
    foreach ($reference in $nativeRustByRef.Keys) {
        [void]$expectedNativeLicenseReferences.Add([string]$reference)
    }
    $nativeTextsByHash = @{}
    foreach ($textRecord in @($nativeLicenseBundle.texts)) {
        $textBytes = [System.Text.UTF8Encoding]::new($false).GetBytes([string]$textRecord.text)
        $textHash = [System.Convert]::ToHexString(
            [System.Security.Cryptography.SHA256]::HashData($textBytes)
        ).ToLowerInvariant()
        if ($textHash -cne [string]$textRecord.sha256 -or
            [int64]$textBytes.Length -ne [int64]$textRecord.byte_length -or
            $nativeTextsByHash.ContainsKey($textHash)) {
            throw 'Codec Pack native Rust license bundle contains invalid or duplicate text.'
        }
        $nativeTextsByHash[$textHash] = $true
    }
    $nativeMappings = @($nativeLicenseBundle.components)
    $actualNativeLicenseReferences = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $usedNativeTexts = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $getNativeLicenseExpression = {
        param([Parameter(Mandatory)][object]$Component)

        $labels = @(@(
            foreach ($entry in @($Component.licenses)) {
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
        ) | Sort-Object -CaseSensitive -Unique)
        if ($labels.Count -eq 0) {
            throw "Codec Pack native Rust component has no license expression: $($Component.'bom-ref')"
        }
        return $labels -join ' OR '
    }
    foreach ($mapping in $nativeMappings) {
        $reference = [string]$mapping.'bom-ref'
        if (-not $actualNativeLicenseReferences.Add($reference) -or
            -not $expectedNativeLicenseReferences.Contains($reference)) {
            throw "Codec Pack native Rust license mapping is unexpected or duplicated: $reference"
        }
        $expectedComponent = if ($reference -ceq $nativeRootReference) {
            $nativeRustSbom.metadata.component
        } else {
            $nativeRustByRef[$reference]
        }
        $expectedEcosystem = if ($reference -ceq $nativeRootReference) { 'artifact' } else { 'rust' }
        $expectedScopeValues = @($expectedComponent.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope'
        })
        $actualArtifacts = @($mapping.artifacts | Sort-Object -CaseSensitive -Unique)
        if ($expectedScopeValues.Count -ne 1 -or
            [string]$mapping.name -cne [string]$expectedComponent.name -or
            [string]$mapping.version -cne [string]$expectedComponent.version -or
            [string]$mapping.ecosystem -cne $expectedEcosystem -or
            [string]$mapping.dependency_scope -cne [string]$expectedScopeValues[0].value -or
            [string]$mapping.license_expression -cne
                [string](& $getNativeLicenseExpression -Component $expectedComponent) -or
            @($mapping.artifacts).Count -ne 1 -or $actualArtifacts.Count -ne 1 -or
            [string]$actualArtifacts[0] -cne 'LatentDeck H3 Native Extensions') {
            throw "Codec Pack native Rust license mapping drifted from its SBOM: $reference"
        }
        $hashes = @($mapping.text_sha256s)
        if ([string]$mapping.disposition -ceq 'license_text_in_bundle') {
            if ($hashes.Count -eq 0) {
                throw "Codec Pack redistributed native Rust component lacks license text: $reference"
            }
            foreach ($hash in $hashes) {
                if (-not $nativeTextsByHash.ContainsKey([string]$hash)) {
                    throw "Codec Pack native Rust license mapping references unknown text: $reference"
                }
                [void]$usedNativeTexts.Add([string]$hash)
            }
        } elseif ([string]$mapping.disposition -ceq 'not_redistributed_no_text_required') {
            if ([string]$mapping.dependency_scope -cne 'build' -or
                $hashes.Count -ne 0 -or
                [string]::IsNullOrWhiteSpace([string]$mapping.rationale)) {
                throw "Codec Pack native Rust no-text disposition is invalid: $reference"
            }
        } else {
            throw "Codec Pack native Rust license disposition is invalid: $reference"
        }
    }
    $nativeSbomItem = Get-Item -LiteralPath $nativeRustSbomPath -Force
    $nativeSbomBindings = @($nativeLicenseBundle.sboms | Where-Object {
        [string]$_.name -ceq 'NATIVE_RUST_SBOM.cdx.json' -and
        [int64]$_.byte_length -eq [int64]$nativeSbomItem.Length -and
        [string]$_.sha256 -ceq [string]$inventory.native_rust.sbom_sha256
    })
    if ([int]$nativeLicenseBundle.schema_version -ne 1 -or
        [string]$nativeLicenseBundle.artifact.name -cne 'LatentDeck H3 Native Extensions' -or
        [string]$nativeLicenseBundle.artifact.version -cne $PackVersion -or
        $nativeSbomBindings.Count -ne 1 -or @($nativeLicenseBundle.sboms).Count -ne 1 -or
        $actualNativeLicenseReferences.Count -ne $expectedNativeLicenseReferences.Count -or
        [int]$nativeLicenseBundle.component_count -ne $actualNativeLicenseReferences.Count -or
        [int]$nativeLicenseBundle.text_count -ne $nativeTextsByHash.Count -or
        $usedNativeTexts.Count -ne $nativeTextsByHash.Count) {
        throw 'Codec Pack native Rust license bundle closure or SBOM binding is incomplete.'
    }

    $sbom = Read-StrictJsonFile -Path (Join-Path $PackRoot 'SBOM.cdx.json')
    Assert-ExactProperties -Object $sbom -Required @(
        'bomFormat', 'specVersion', 'version', 'metadata', 'components'
    ) -Context 'SBOM.cdx.json'
    Assert-ExactProperties -Object $sbom.metadata -Required @('component') -Context 'SBOM metadata'
    Assert-ExactProperties -Object $sbom.metadata.component -Required @(
        'bom-ref', 'type', 'name', 'version', 'licenses', 'properties'
    ) -Context 'SBOM metadata.component'
    if ($sbom.bomFormat -cne 'CycloneDX' -or
        $sbom.specVersion -cne '1.5' -or
        [int64]$sbom.version -ne 1 -or
        $sbom.metadata.component.'bom-ref' -cne "pkg:generic/latentdeck-h3-codec-pack@$PackVersion" -or
        $sbom.metadata.component.type -cne 'application' -or
        $sbom.metadata.component.name -cne 'LatentDeck H3 Codec Pack' -or
        $sbom.metadata.component.version -cne $PackVersion -or
        @($sbom.metadata.component.licenses).Count -ne 1 -or
        [string]$sbom.metadata.component.licenses[0].expression -cne 'Apache-2.0' -or
        @($sbom.metadata.component.properties).Count -ne 6) {
        throw 'Codec Pack CycloneDX SBOM identity is invalid.'
    }
    $expectedRootProperties = [ordered]@{
        'latentdeck:source-commit' = [string]$inventory.source_commit
        'latentdeck:artifact-scope' = 'h3-codec-pack'
        'latentdeck:dependency-scope' = 'artifact'
        'latentdeck:included-dependency-scopes' = 'artifact,runtime,build,runtime+build'
        'latentdeck:excluded-dependency-scopes' = 'development'
        'latentdeck:target-platform' = 'windows-x86_64'
    }
    foreach ($expectedProperty in $expectedRootProperties.GetEnumerator()) {
        $matches = @($sbom.metadata.component.properties | Where-Object {
            [string]$_.name -ceq [string]$expectedProperty.Key -and
            [string]$_.value -ceq [string]$expectedProperty.Value
        })
        if ($matches.Count -ne 1) {
            throw "Codec Pack CycloneDX SBOM root property is invalid: $($expectedProperty.Key)"
        }
    }
    $sbomComponents = @($sbom.components)
    if ($sbomComponents.Count -ne ($inventoryComponents.Count + $nativeRustComponents.Count)) {
        throw 'Codec Pack SBOM and dependency inventory component counts differ.'
    }
    $sbomReferences = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($component in $sbomComponents) {
        $reference = [string]$component.'bom-ref'
        if (-not $sbomReferences.Add($reference)) {
            throw "Codec Pack SBOM contains a duplicate component reference '$reference'."
        }
        if ($nativeRustByRef.ContainsKey($reference)) {
            $nativeComponent = $nativeRustByRef[$reference]
            $nativeScope = @($nativeComponent.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope'
            })
            $mergedScope = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope'
            })
            $mergedEcosystem = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:ecosystem' -and [string]$_.value -ceq 'rust'
            })
            if ([string]$component.name -cne [string]$nativeComponent.name -or
                [string]$component.version -cne [string]$nativeComponent.version -or
                (@($component.licenses | ConvertTo-Json -Compress) -join '') -cne
                    (@($nativeComponent.licenses | ConvertTo-Json -Compress) -join '') -or
                $nativeScope.Count -ne 1 -or $mergedScope.Count -ne 1 -or
                [string]$nativeScope[0].value -cne [string]$mergedScope[0].value -or
                $mergedEcosystem.Count -ne 1) {
                throw "Codec Pack merged native Rust SBOM component drifted: $reference"
            }
            continue
        }
        Assert-ExactProperties -Object $component -Required @(
            'bom-ref', 'type', 'name', 'version', 'hashes', 'licenses',
            'externalReferences', 'properties'
        ) -Optional @('purl') -Context 'SBOM component'
        $identity = "$($component.name)@$($component.version)"
        if (-not $inventoryIds.Contains($identity)) {
            throw "Codec Pack SBOM contains an unknown or duplicate component '$identity'."
        }
        $hashes = @($component.hashes)
        if ($hashes.Count -ne 1 -or $hashes[0].alg -cne 'SHA-256') {
            throw "Codec Pack SBOM component '$identity' has an invalid hash contract."
        }
        Assert-Sha256 -Value ([string]$hashes[0].content) -Name 'SBOM component hash'
        $inventoryComponent = $inventoryById[$identity]
        $scopeProperties = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope' -and
            [string]$_.value -ceq 'runtime'
        })
        $ecosystemProperties = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:ecosystem' -and
            [string]$_.value -ceq 'python'
        })
        if (@($component.licenses).Count -ne 1 -or
            [string]::IsNullOrWhiteSpace([string]$component.licenses[0].expression) -or
            @($component.externalReferences).Count -ne 1 -or
            $component.externalReferences[0].type -cne 'distribution' -or
            ([string]$component.externalReferences[0].url) -cnotmatch '^https://' -or
            [string]$hashes[0].content -cne [string]$inventoryComponent.content_sha256 -or
            [string]$component.licenses[0].expression -cne
                [string]$inventoryComponent.license_expression -or
            [string]$component.externalReferences[0].url -cne
                [string]$inventoryComponent.source_url -or
            @($component.properties).Count -ne 2 -or
            $scopeProperties.Count -ne 1 -or $ecosystemProperties.Count -ne 1) {
            throw "Codec Pack SBOM component '$identity' has incomplete provenance."
        }
    }
    if ($sbomReferences.Count -ne ($inventoryComponents.Count + $nativeRustComponents.Count)) {
        throw 'Codec Pack merged SBOM reference closure is incomplete.'
    }
}

function Test-H3CodecPackDirectory {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$PackRoot,

        [string]$ExpectedPackVersion
    )

    $resolvedRoot = (Resolve-Path -LiteralPath $PackRoot).Path
    $allFiles = @(Assert-PackagingSourceTree `
        -RootPath $resolvedRoot `
        -MaximumFiles $script:MaximumArchiveEntries `
        -AllowedArchiveRelativePaths @('runtime/python313.zip'))
    $manifestPath = Join-Path $resolvedRoot 'codec-pack.json'
    $manifestJson = Read-StrictJsonElement -Path $manifestPath
    Assert-CodecPackManifestJsonTypes -Root $manifestJson
    $manifest = $manifestJson.GetRawText() | ConvertFrom-Json
    Assert-ExactProperties -Object $manifest -Required @(
        'manifest_version', 'kind', 'pack_id', 'pack_version', 'display_name',
        'summary', 'publisher', 'license', 'platform', 'compatibility', 'adapter',
        'worker', 'capabilities', 'external_assets', 'runtime_lock', 'integrity'
    ) -Context 'codec-pack.json'

    if ($manifest.manifest_version -cne '2.0.0' -or $manifest.kind -cne 'codec_pack') {
        throw 'H3 Setup accepts only codec-pack.json v2.'
    }
    Assert-Token -Value ([string]$manifest.pack_id) -Name 'pack_id'
    if ($manifest.pack_id -cne 'org.latentdeck.h3') {
        throw 'This installer accepts only the official H3 pack identifier.'
    }
    Assert-SemVer -Value ([string]$manifest.pack_version) -Name 'pack_version'
    if (-not [string]::IsNullOrWhiteSpace($ExpectedPackVersion) -and
        $manifest.pack_version -cne $ExpectedPackVersion) {
        throw 'Codec Pack version does not match the requested version.'
    }

    Assert-ExactProperties -Object $manifest.publisher -Required @(
        'name', 'url', 'identity_claim'
    ) -Context 'publisher'
    Assert-ExactProperties -Object $manifest.license -Required @('spdx_or_label', 'notice_path') -Context 'license'
    Assert-ExactProperties -Object $manifest.platform -Required @('os', 'arch') -Context 'platform'
    Assert-ExactProperties -Object $manifest.compatibility -Required @(
        'app_min_inclusive', 'app_max_exclusive', 'worker_protocol',
        'codec_adapter_api', 'tensor_abi', 'python', 'torch_exact_build',
        'lc_spec_versions', 'profiles'
    ) -Context 'compatibility'
    Assert-ExactProperties -Object $manifest.compatibility.python -Required @(
        'implementation', 'version', 'platform_tag'
    ) -Context 'compatibility.python'
    Assert-ExactProperties -Object $manifest.worker -Required @(
        'executable', 'arguments', 'working_directory', 'start_timeout_ms',
        'heartbeat_timeout_ms'
    ) -Context 'worker'
    Assert-ExactProperties -Object $manifest.adapter -Required @(
        'adapter_id', 'adapter_version', 'entrypoint'
    ) -Context 'adapter'
    Assert-ExactProperties -Object $manifest.runtime_lock -Required @(
        'path', 'sha256'
    ) -Context 'runtime_lock'
    Assert-ExactProperties -Object $manifest.integrity -Required @(
        'catalog_path', 'catalog_sha256'
    ) -Context 'integrity'

    if ($manifest.platform.os -cne 'windows' -or $manifest.platform.arch -cne 'x86_64') {
        throw 'Codec Pack platform must be Windows x86-64.'
    }
    if ($manifest.publisher.identity_claim -cne 'self_declared') {
        throw 'Codec Pack publisher metadata must remain explicitly self-declared.'
    }
    Assert-SemVer -Value ([string]$manifest.adapter.adapter_version) -Name 'adapter_version'
    if ($manifest.adapter.adapter_id -cne 'org.latentdeck.h3' -or
        $manifest.adapter.adapter_version -cne '0.2.0' -or
        $manifest.adapter.entrypoint -cne 'latentdeck_codec_h3.adapter:make_adapter') {
        throw 'Codec Pack adapter identity is inconsistent.'
    }
    if ($manifest.worker.executable -cne 'runtime/python.exe' -or
        $manifest.worker.working_directory -cne 'runtime') {
        throw 'Codec Pack worker launch path is not the approved isolated runtime path.'
    }

    $expectedWorkerArguments = @(
        '-I', '-s', '-B', '-m', 'latentdeck_codec_host',
        '--worker-protocol', '2',
        '--codec-pack-id', 'org.latentdeck.h3',
        '--codec-pack-version', [string]$manifest.pack_version,
        '--codec-adapter-id', 'org.latentdeck.h3',
        '--codec-adapter-version', '0.2.0',
        '--codec-entrypoint', 'latentdeck_codec_h3.adapter:make_adapter'
    )
    if ((@($manifest.worker.arguments) -join "`0") -cne ($expectedWorkerArguments -join "`0")) {
        throw 'Codec Pack worker arguments do not select the generic Protocol 2 host.'
    }
    if ([int64]$manifest.worker.start_timeout_ms -ne 120000 -or
        [int64]$manifest.worker.heartbeat_timeout_ms -ne 5000) {
        throw 'Codec Pack worker timeouts do not match the bounded Protocol 2 contract.'
    }

    if ($manifest.compatibility.app_min_inclusive -cne '0.1.0' -or
        $manifest.compatibility.app_max_exclusive -cne '1.0.0' -or
        [int]$manifest.compatibility.worker_protocol -ne 2 -or
        [int]$manifest.compatibility.codec_adapter_api -ne 1 -or
        $manifest.compatibility.tensor_abi -cne 'latentdeck.tensor.v1' -or
        $manifest.compatibility.python.implementation -cne 'cpython' -or
        $manifest.compatibility.python.version -cne '3.13' -or
        $manifest.compatibility.python.platform_tag -cne 'win_amd64' -or
        $manifest.compatibility.torch_exact_build -cne '2.13.0+cu130' -or
        (@($manifest.compatibility.lc_spec_versions) -join ',') -cne '0.1.0') {
        throw 'Codec Pack application or protocol compatibility is invalid.'
    }
    $profiles = @($manifest.compatibility.profiles)
    if ($profiles.Count -ne 1) {
        throw 'Codec Pack must declare exactly one H3 profile.'
    }
    Assert-ExactProperties -Object $profiles[0] -Required @(
        'codec_family', 'profile', 'profile_version'
    ) -Context 'compatibility.profiles[0]'
    if ($profiles[0].codec_family -cne 'minimax_h3' -or
        $profiles[0].profile -cne 'h3_av_latent' -or
        $profiles[0].profile_version -cne '0.1.0') {
        throw 'Codec Pack H3 profile declaration is invalid.'
    }

    $requiredCapabilities = @(
        'player', 'realtime', 'resample', 'snapshot_capture', 'live_capture',
        'raw_import'
    )
    if ((@($manifest.capabilities) -join "`0") -cne ($requiredCapabilities -join "`0")) {
        throw 'H3 Codec Pack v2 must declare the mandatory capabilities and raw import.'
    }

    Assert-PortableRelativePath -Path ([string]$manifest.license.notice_path) -Context 'license.notice_path'
    if ($manifest.license.notice_path -cne 'THIRD_PARTY_NOTICES.md') {
        throw 'Codec Pack notice must use the canonical pack-relative path.'
    }
    Assert-PortableRelativePath -Path ([string]$manifest.integrity.catalog_path) -Context 'integrity.catalog_path'
    if ($manifest.integrity.catalog_path -cne 'integrity.json') {
        throw 'Codec Pack integrity catalog must use the canonical pack-relative path.'
    }
    Assert-Sha256 -Value ([string]$manifest.integrity.catalog_sha256) -Name 'catalog_sha256'
    Assert-PortableRelativePath -Path ([string]$manifest.runtime_lock.path) -Context 'runtime_lock.path'
    if ($manifest.runtime_lock.path -cne 'DEPENDENCY_INVENTORY.json') {
        throw 'Codec Pack runtime lock must bind the curated dependency inventory.'
    }
    Assert-Sha256 -Value ([string]$manifest.runtime_lock.sha256) -Name 'runtime_lock.sha256'
    $runtimeLockPath = Join-Path $resolvedRoot 'DEPENDENCY_INVENTORY.json'
    if ((Get-FileHash -LiteralPath $runtimeLockPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
        $manifest.runtime_lock.sha256) {
        throw 'Codec Pack runtime lock hash does not match the dependency inventory.'
    }

    $externalAssets = @($manifest.external_assets)
    if ($externalAssets.Count -ne 1) {
        throw 'H3 Codec Pack must declare one exact external decoder asset.'
    }
    foreach ($asset in $externalAssets) {
        Assert-ExactProperties -Object $asset -Required @(
            'asset_id', 'display_name', 'required', 'byte_length', 'sha256',
            'source_url', 'license_label', 'license_url'
        ) -Context 'external asset'
        Assert-Sha256 -Value ([string]$asset.sha256) -Name 'external asset sha256'
        if ($asset.asset_id -cne 'taeh3' -or
            $asset.display_name -cne 'TAEH3 decoder weight' -or
            $asset.required -ne $true -or
            [int64]$asset.byte_length -ne 22709752 -or
            $asset.sha256 -cne '4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13' -or
            $asset.source_url -cne 'https://raw.githubusercontent.com/madebyollin/taehv/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/safetensors/taeh3.safetensors' -or
            $asset.license_label -cne 'MIT' -or
            $asset.license_url -cne 'https://github.com/madebyollin/taehv/blob/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/LICENSE') {
            throw 'H3 Codec Pack external TAEH3 asset identity is not exact.'
        }
    }

    $adapterPath = Join-Path $resolvedRoot 'runtime/Lib/site-packages/latentdeck_codec_h3/adapter.py'
    if (-not (Test-Path -LiteralPath $adapterPath -PathType Leaf)) {
        throw 'H3 Codec Pack is missing the declared Codec SDK v2 adapter entrypoint.'
    }

    $catalogPath = Join-Path $resolvedRoot 'integrity.json'
    $catalogHash = (Get-FileHash -LiteralPath $catalogPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($catalogHash -cne $manifest.integrity.catalog_sha256) {
        throw 'Codec Pack integrity catalog hash does not match the manifest.'
    }
    $catalogJson = Read-StrictJsonElement -Path $catalogPath
    Assert-IntegrityCatalogJsonTypes -Root $catalogJson
    $catalog = $catalogJson.GetRawText() | ConvertFrom-Json
    Assert-ExactProperties -Object $catalog -Required @('manifest_version', 'files') -Context 'integrity.json'
    $catalogFiles = @($catalog.files)
    if ($catalog.manifest_version -cne '1.0.0' -or
        $catalogFiles.Count -eq 0 -or
        $catalogFiles.Count -gt $script:MaximumCatalogFiles) {
        throw 'Codec Pack integrity catalog is empty or exceeds its bound.'
    }

    $catalogPaths = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($entry in $catalogFiles) {
        Assert-ExactProperties -Object $entry -Required @(
            'path', 'byte_length', 'sha256'
        ) -Context 'integrity file'
        $portablePath = [string]$entry.path
        Assert-PortableRelativePath -Path $portablePath -Context 'integrity file path'
        if (-not $catalogPaths.Add($portablePath)) {
            throw "Codec Pack integrity catalog contains duplicate path '$portablePath'."
        }
        Assert-Sha256 -Value ([string]$entry.sha256) -Name 'integrity file sha256'
        $filePath = Join-Path $resolvedRoot $portablePath.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
        Assert-ChildPath -ParentPath $resolvedRoot -CandidatePath $filePath | Out-Null
        if (-not (Test-Path -LiteralPath $filePath -PathType Leaf)) {
            throw "Catalogued Codec Pack file is missing: $portablePath"
        }
        $file = Get-Item -LiteralPath $filePath -Force
        if (($file.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            [int64]$file.Length -ne [int64]$entry.byte_length -or
            (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant() -cne $entry.sha256) {
            throw "Catalogued Codec Pack file failed integrity validation: $portablePath"
        }
    }

    Assert-Python313RuntimeLayout -PackRoot $resolvedRoot -CatalogPaths $catalogPaths
    Assert-CodecPackDependencyMetadata `
        -PackRoot $resolvedRoot `
        -PackVersion ([string]$manifest.pack_version)

    $actualPayloadPaths = @(
        $allFiles |
            ForEach-Object { Convert-ToPortableRelativePath -RootPath $resolvedRoot -Path $_.FullName } |
            Where-Object { $_ -cne 'codec-pack.json' -and $_ -cne 'integrity.json' }
    )
    if ($actualPayloadPaths.Count -ne $catalogPaths.Count) {
        throw 'Codec Pack contains uncatalogued files.'
    }
    foreach ($actualPath in $actualPayloadPaths) {
        if (-not $catalogPaths.Contains($actualPath)) {
            throw "Codec Pack contains uncatalogued file '$actualPath'."
        }
    }

    return $manifest
}

function New-DeterministicZip {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$SourceDirectory,

        [Parameter(Mandatory)]
        [string]$DestinationPath
    )

    if (Test-Path -LiteralPath $DestinationPath) {
        throw "Archive destination already exists: $DestinationPath"
    }
    $sourceRoot = (Resolve-Path -LiteralPath $SourceDirectory).Path
    $files = @(Get-ChildItem -LiteralPath $sourceRoot -File -Force -Recurse | Sort-Object {
        Convert-ToPortableRelativePath -RootPath $sourceRoot -Path $_.FullName
    })

    $stream = [System.IO.FileStream]::new(
        $DestinationPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            foreach ($file in $files) {
                $entryName = Convert-ToPortableRelativePath -RootPath $sourceRoot -Path $file.FullName
                $entry = $archive.CreateEntry(
                    $entryName,
                    [System.IO.Compression.CompressionLevel]::Optimal
                )
                $entry.LastWriteTime = [System.DateTimeOffset]::new(
                    1980, 1, 1, 0, 0, 0, [System.TimeSpan]::Zero
                )
                $entryStream = $entry.Open()
                $fileStream = [System.IO.File]::OpenRead($file.FullName)
                try {
                    $fileStream.CopyTo($entryStream)
                } finally {
                    $fileStream.Dispose()
                    $entryStream.Dispose()
                }
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Expand-SafeCodecPackArchive {
    [CmdletBinding(DefaultParameterSetName = 'Path')]
    param(
        [Parameter(Mandatory, ParameterSetName = 'Path')]
        [string]$ArchivePath,

        [Parameter(Mandatory, ParameterSetName = 'Stream')]
        [System.IO.Stream]$ArchiveStream,

        [Parameter(Mandatory)]
        [string]$DestinationPath
    )

    if ($PSCmdlet.ParameterSetName -eq 'Path' -and
        -not (Test-Path -LiteralPath $ArchivePath -PathType Leaf)) {
        throw "Codec Pack archive is missing: $ArchivePath"
    }
    if (Test-Path -LiteralPath $DestinationPath) {
        throw "Codec Pack extraction destination already exists: $DestinationPath"
    }
    [System.IO.Directory]::CreateDirectory($DestinationPath) | Out-Null
    $destinationRoot = [System.IO.Path]::GetFullPath($DestinationPath)
    $ownedStream = $null
    try {
        if ($PSCmdlet.ParameterSetName -eq 'Path') {
            $ownedStream = [System.IO.FileStream]::new(
                (Resolve-Path -LiteralPath $ArchivePath).Path,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read,
                [System.IO.FileShare]::Read
            )
            $ArchiveStream = $ownedStream
        }
        if (-not $ArchiveStream.CanRead -or -not $ArchiveStream.CanSeek) {
            throw 'Codec Pack archive stream must be readable and seekable.'
        }
        if ([int64]$ArchiveStream.Length -gt $script:MaximumArchiveBytes) {
            throw 'Codec Pack archive exceeds the 32 GiB archive bound.'
        }
        $ArchiveStream.Position = 0
        $archive = [System.IO.Compression.ZipArchive]::new(
            $ArchiveStream,
            [System.IO.Compression.ZipArchiveMode]::Read,
            $true,
            [System.Text.Encoding]::UTF8
        )
        try {
            if ($archive.Entries.Count -eq 0 -or
                $archive.Entries.Count -gt $script:MaximumArchiveEntries) {
                throw 'Codec Pack archive is empty or exceeds its entry limit.'
            }
            $seen = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::OrdinalIgnoreCase
            )
            $totalBytes = [int64]0
            foreach ($entry in $archive.Entries) {
                if ($entry.FullName.EndsWith('/')) {
                    continue
                }
                $entryName = $entry.FullName
                Assert-PortableRelativePath -Path $entryName -Context 'Codec Pack archive entry'
                if (-not $seen.Add($entryName)) {
                    throw "Codec Pack archive contains duplicate entry '$entryName'."
                }
                $unixMode = ([int64]$entry.ExternalAttributes -shr 16) -band 0xF000
                if ($unixMode -eq 0xA000) {
                    throw "Codec Pack archive contains a symbolic link entry '$entryName'."
                }
                if ([int64]$entry.Length -gt [int64]::MaxValue - $totalBytes) {
                    throw 'Codec Pack archive size overflowed its counter.'
                }
                $totalBytes = [int64]($totalBytes + [int64]$entry.Length)
                if ($totalBytes -gt $script:MaximumPackBytes) {
                    throw 'Codec Pack archive exceeds the bounded uncompressed size.'
                }
                $extension = [System.IO.Path]::GetExtension($entryName).ToLowerInvariant()
                if ($script:ForbiddenPayloadExtensions -contains $extension) {
                    throw "Codec Pack archive contains forbidden payload type '$extension'."
                }
                Assert-NotForbiddenLeafName -Name $entryName -Context 'Codec Pack archive'
                if ($extension -ceq '.ld') {
                    Assert-PortableLinkerScriptArchiveEntry `
                        -Entry $entry `
                        -Context "Codec Pack archive entry '$entryName'"
                } elseif ($script:ArchiveExtensions -contains $extension -and
                    $entryName -ine 'runtime/python313.zip') {
                    throw "Codec Pack archive contains an unapproved nested archive '$entryName'."
                }

                $destination = Join-Path $destinationRoot $entryName.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
                Assert-ChildPath -ParentPath $destinationRoot -CandidatePath $destination | Out-Null
                if (Test-Path -LiteralPath $destination) {
                    throw "Codec Pack archive entry would overwrite '$entryName'."
                }
                [System.IO.Directory]::CreateDirectory((Split-Path -Parent $destination)) | Out-Null
                $input = $entry.Open()
                $output = [System.IO.FileStream]::new(
                    $destination,
                    [System.IO.FileMode]::CreateNew,
                    [System.IO.FileAccess]::Write,
                    [System.IO.FileShare]::None
                )
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
        if ($null -ne $ownedStream) {
            $ownedStream.Dispose()
        }
    }
}

function Get-CargoNormalBuildDependencyIds {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [object]$Metadata,

        [Parameter(Mandatory)]
        [string]$RootPackageId
    )

    $nodes = @($Metadata.resolve.nodes)
    if ($nodes.Count -eq 0) {
        throw 'Cargo metadata resolve graph is empty.'
    }
    $nodesById = @{}
    foreach ($node in $nodes) {
        $nodeId = [string]$node.id
        if ([string]::IsNullOrWhiteSpace($nodeId) -or $nodesById.ContainsKey($nodeId)) {
            throw 'Cargo metadata contains a missing or duplicate dependency node id.'
        }
        $nodesById[$nodeId] = $node
    }
    if (-not $nodesById.ContainsKey($RootPackageId)) {
        throw "Cargo metadata is missing the root dependency node: $RootPackageId"
    }

    $included = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($RootPackageId)
    while ($pending.Count -gt 0) {
        $packageId = $pending.Dequeue()
        if (-not $included.Add($packageId)) {
            continue
        }
        if (-not $nodesById.ContainsKey($packageId)) {
            throw "Cargo metadata is missing a dependency node: $packageId"
        }
        foreach ($dependency in @($nodesById[$packageId].deps)) {
            $includeDependency = @(
                $dependency.dep_kinds |
                    Where-Object {
                        $null -eq $_.kind -or
                        [string]$_.kind -ceq 'normal' -or
                        [string]$_.kind -ceq 'build'
                    }
            ).Count -gt 0
            if ($includeDependency) {
                $dependencyId = [string]$dependency.pkg
                if ([string]::IsNullOrWhiteSpace($dependencyId)) {
                    throw 'Cargo metadata contains a dependency edge without a package id.'
                }
                $pending.Enqueue($dependencyId)
            }
        }
    }
    return @($included | Sort-Object -CaseSensitive)
}

Export-ModuleMember -Function @(
    'Assert-ChildPath',
    'Assert-DirectoryNotReparsePoint',
    'Assert-ExactProperties',
    'Assert-PackagingSourceTree',
    'Assert-PathComponentsNotReparsePoints',
    'Assert-PackagingSourceStateUnchanged',
    'Assert-PortableRelativePath',
    'Assert-SafeTemporaryDirectory',
    'Assert-SemVer',
    'Assert-Sha256',
    'Assert-Token',
    'Copy-PackagingTree',
    'Convert-ToPortableRelativePath',
    'Expand-SafeCodecPackArchive',
    'Get-CargoNormalBuildDependencyIds',
    'Get-CodecPackAuxiliaryRoot',
    'Get-CodecPackInstallRoot',
    'Get-IntegrityEntry',
    'Get-PackagingSourceState',
    'New-DeterministicZip',
    'Read-StrictJsonFile',
    'Remove-SafeTemporaryDirectory',
    'Test-H3CodecPackDirectory',
    'Write-JsonFile'
)
