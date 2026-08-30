Set-StrictMode -Version Latest

$script:ForbiddenPayloadExtensions = @(
    '.lc', '.h3latent', '.safetensors', '.ckpt', '.pt', '.pth', '.onnx',
    '.engine', '.plan', '.gguf', '.bin', '.mp4', '.mov', '.mkv', '.avi',
    '.webm', '.wav', '.flac', '.mp3', '.whl', '.npy', '.npz', '.pkl',
    '.pickle', '.png', '.jpg', '.jpeg', '.webp', '.gif', '.bmp', '.tif',
    '.tiff', '.exr', '.hdr', '.psd', '.ps1', '.cmd', '.bat', '.sh'
)
$script:ArchiveExtensions = @(
    '.zip', '.tar', '.tgz', '.gz', '.bz2', '.xz', '.7z', '.rar'
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
$script:MaximumCatalogFiles = 32768
$script:MaximumArchiveEntries = 32770
$script:MaximumPackBytes = [int64](20GB)
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
    foreach ($name in @('manifest_version', 'pack_id', 'pack_version', 'display_name')) {
        Assert-JsonStringProperty -Object $Root -Name $name -Context $context
    }

    $publisher = Get-JsonPropertyElement -Object $Root -Name 'publisher' -Context $context
    Assert-JsonStringProperty -Object $publisher -Name 'name' -Context 'publisher'
    Assert-JsonNullableStringProperty -Object $publisher -Name 'url' -Context 'publisher'

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
    Assert-JsonUnsignedIntegerProperty -Object $compatibility -Name 'worker_protocol_min' -Maximum ([uint16]::MaxValue) -Context 'compatibility'
    Assert-JsonUnsignedIntegerProperty -Object $compatibility -Name 'worker_protocol_max' -Maximum ([uint16]::MaxValue) -Context 'compatibility'
    Assert-JsonStringArrayProperty -Object $compatibility -Name 'lc_spec_versions' -Context 'compatibility'
    $profileIndex = 0
    foreach ($profile in @(Get-JsonArrayElements -Object $compatibility -Name 'profiles' -Context 'compatibility')) {
        if ($profile.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
            throw "compatibility.profiles[$profileIndex] must be a JSON object."
        }
        foreach ($name in @('codec_family', 'profile')) {
            Assert-JsonStringProperty -Object $profile -Name $name -Context "compatibility.profiles[$profileIndex]"
        }
        Assert-JsonStringArrayProperty -Object $profile -Name 'profile_versions' -Context "compatibility.profiles[$profileIndex]"
        $profileIndex += 1
    }

    $worker = Get-JsonPropertyElement -Object $Root -Name 'worker' -Context $context
    foreach ($name in @('executable', 'working_directory')) {
        Assert-JsonStringProperty -Object $worker -Name $name -Context 'worker'
    }
    foreach ($name in @('arguments', 'd2_arguments', 'q4_arguments')) {
        Assert-JsonStringArrayProperty -Object $worker -Name $name -Context 'worker'
    }
    Assert-JsonUnsignedIntegerProperty -Object $worker -Name 'probe_timeout_ms' -Maximum ([uint32]::MaxValue) -Context 'worker'

    $adapter = Get-JsonPropertyElement -Object $Root -Name 'adapter' -Context $context
    foreach ($name in @('adapter_id', 'adapter_version')) {
        Assert-JsonStringProperty -Object $adapter -Name $name -Context 'adapter'
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
        foreach ($name in @('asset_id', 'display_name', 'kind', 'selection', 'format')) {
            Assert-JsonStringProperty -Object $asset -Name $name -Context "external_assets[$assetIndex]"
        }
        Assert-JsonBooleanProperty -Object $asset -Name 'required' -Context "external_assets[$assetIndex]"
        $variantIndex = 0
        foreach ($variant in @(Get-JsonArrayElements -Object $asset -Name 'accepted_variants' -Context "external_assets[$assetIndex]")) {
            if ($variant.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) {
                throw "external_assets[$assetIndex].accepted_variants[$variantIndex] must be a JSON object."
            }
            foreach ($name in @(
                'variant_id', 'sha256', 'source_url', 'license_label', 'license_url'
            )) {
                Assert-JsonStringProperty `
                    -Object $variant `
                    -Name $name `
                    -Context "external_assets[$assetIndex].accepted_variants[$variantIndex]"
            }
            Assert-JsonUnsignedIntegerProperty `
                -Object $variant `
                -Name 'byte_length' `
                -Maximum ([uint64]::MaxValue) `
                -Context "external_assets[$assetIndex].accepted_variants[$variantIndex]"
            $variantIndex += 1
        }
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
        if ($Text -match '(?im)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----' -or
            $Text -match '(?i)\bAKIA[0-9A-Z]{16}\b' -or
            $Text -match '(?i)\bgh[pousr]_[A-Za-z0-9]{20,}\b' -or
            $Text -match '(?i)\bsk-[A-Za-z0-9_-]{20,}\b' -or
            $Text -match '(?im)\b(?:api[_-]?key|access[_-]?token|auth[_-]?token|secret|password)\b\s*[:=]\s*(?:"[^"\r\n]{8,}"|''[^''\r\n]{8,}'')') {
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
            if ($script:ArchiveExtensions -contains $extension) {
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
        if ($script:ArchiveExtensions -contains $extension) {
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

    $requiredPaths = @(
        'runtime/python.exe',
        'runtime/python313.dll',
        'runtime/python313._pth',
        'runtime/python313.zip',
        'runtime/Lib/site-packages/latentdeck_codec_h3/__init__.py',
        'runtime/Lib/site-packages/latentdeck_codec_h3/worker.py',
        'runtime/Lib/site-packages/latentdeck_codec_h3/d2_worker.py',
        'runtime/Lib/site-packages/latentdeck_codec_h3/q4_worker.py',
        'THIRD_PARTY_NOTICES.md',
        'DEPENDENCY_INVENTORY.json',
        'SBOM.cdx.json'
    )
    foreach ($requiredPath in $requiredPaths) {
        if (-not $CatalogPaths.Contains($requiredPath)) {
            throw "Codec Pack catalog omits required file '$requiredPath'."
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
        'schema_version', 'pack_id', 'pack_version', 'platform', 'curator', 'components'
    ) -Context 'DEPENDENCY_INVENTORY.json'
    Assert-ExactProperties -Object $inventory.curator -Required @(
        'name', 'schema_version'
    ) -Context 'DEPENDENCY_INVENTORY.json.curator'
    if ([int64]$inventory.schema_version -ne 1 -or
        $inventory.pack_id -cne 'org.latentdeck.h3' -or
        $inventory.pack_version -cne $PackVersion -or
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

    $sbom = Read-StrictJsonFile -Path (Join-Path $PackRoot 'SBOM.cdx.json')
    Assert-ExactProperties -Object $sbom -Required @(
        'bomFormat', 'specVersion', 'version', 'metadata', 'components'
    ) -Context 'SBOM.cdx.json'
    Assert-ExactProperties -Object $sbom.metadata -Required @('component') -Context 'SBOM metadata'
    Assert-ExactProperties -Object $sbom.metadata.component -Required @(
        'bom-ref', 'type', 'name', 'version'
    ) -Context 'SBOM metadata.component'
    if ($sbom.bomFormat -cne 'CycloneDX' -or
        $sbom.specVersion -cne '1.5' -or
        [int64]$sbom.version -ne 1 -or
        $sbom.metadata.component.'bom-ref' -cne "pkg:generic/latentdeck-h3-codec-pack@$PackVersion" -or
        $sbom.metadata.component.type -cne 'application' -or
        $sbom.metadata.component.name -cne 'LatentDeck H3 Codec Pack' -or
        $sbom.metadata.component.version -cne $PackVersion) {
        throw 'Codec Pack CycloneDX SBOM identity is invalid.'
    }
    $sbomComponents = @($sbom.components)
    if ($sbomComponents.Count -ne $inventoryComponents.Count) {
        throw 'Codec Pack SBOM and dependency inventory component counts differ.'
    }
    $sbomIds = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($component in $sbomComponents) {
        Assert-ExactProperties -Object $component -Required @(
            'bom-ref', 'type', 'name', 'version', 'hashes', 'licenses', 'externalReferences'
        ) -Optional @('purl') -Context 'SBOM component'
        $identity = "$($component.name)@$($component.version)"
        if (-not $sbomIds.Add($identity) -or -not $inventoryIds.Contains($identity)) {
            throw "Codec Pack SBOM contains an unknown or duplicate component '$identity'."
        }
        $hashes = @($component.hashes)
        if ($hashes.Count -ne 1 -or $hashes[0].alg -cne 'SHA-256') {
            throw "Codec Pack SBOM component '$identity' has an invalid hash contract."
        }
        Assert-Sha256 -Value ([string]$hashes[0].content) -Name 'SBOM component hash'
        if (@($component.licenses).Count -ne 1 -or
            [string]::IsNullOrWhiteSpace([string]$component.licenses[0].expression) -or
            @($component.externalReferences).Count -ne 1 -or
            $component.externalReferences[0].type -cne 'distribution' -or
            ([string]$component.externalReferences[0].url) -cnotmatch '^https://') {
            throw "Codec Pack SBOM component '$identity' has incomplete provenance."
        }
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
        'manifest_version', 'pack_id', 'pack_version', 'display_name', 'publisher',
        'license', 'platform', 'compatibility', 'worker', 'adapter', 'integrity',
        'external_assets'
    ) -Context 'codec-pack.json'

    if ($manifest.manifest_version -cne '1.0.0') {
        throw 'Unsupported codec-pack manifest version.'
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

    Assert-ExactProperties -Object $manifest.publisher -Required @('name', 'url') -Context 'publisher'
    Assert-ExactProperties -Object $manifest.license -Required @('spdx_or_label', 'notice_path') -Context 'license'
    Assert-ExactProperties -Object $manifest.platform -Required @('os', 'arch') -Context 'platform'
    Assert-ExactProperties -Object $manifest.compatibility -Required @(
        'app_min_inclusive', 'app_max_exclusive', 'worker_protocol_min',
        'worker_protocol_max', 'lc_spec_versions', 'profiles'
    ) -Context 'compatibility'
    Assert-ExactProperties -Object $manifest.worker -Required @(
        'executable', 'arguments', 'd2_arguments', 'q4_arguments',
        'working_directory', 'probe_timeout_ms'
    ) -Context 'worker'
    Assert-ExactProperties -Object $manifest.adapter -Required @(
        'adapter_id', 'adapter_version'
    ) -Context 'adapter'
    Assert-ExactProperties -Object $manifest.integrity -Required @(
        'catalog_path', 'catalog_sha256'
    ) -Context 'integrity'

    if ($manifest.platform.os -cne 'windows' -or $manifest.platform.arch -cne 'x86_64') {
        throw 'Codec Pack platform must be Windows x86-64.'
    }
    if ($manifest.adapter.adapter_id -cne 'org.latentdeck.h3' -or
        $manifest.adapter.adapter_version -cne $manifest.pack_version) {
        throw 'Codec Pack adapter identity is inconsistent.'
    }
    if ($manifest.worker.executable -cne 'runtime/python.exe' -or
        $manifest.worker.working_directory -cne 'runtime') {
        throw 'Codec Pack worker launch path is not the approved isolated runtime path.'
    }

    $expectedPlayerArguments = @('-I', '-s', '-B', '-m', 'latentdeck_codec_h3.worker')
    $expectedD2Arguments = @('-I', '-s', '-B', '-m', 'latentdeck_codec_h3.d2_worker')
    $expectedQ4Arguments = @('-I', '-s', '-B', '-m', 'latentdeck_codec_h3.q4_worker')
    if ((@($manifest.worker.arguments) -join "`0") -cne ($expectedPlayerArguments -join "`0") -or
        (@($manifest.worker.d2_arguments) -join "`0") -cne ($expectedD2Arguments -join "`0") -or
        (@($manifest.worker.q4_arguments) -join "`0") -cne ($expectedQ4Arguments -join "`0")) {
        throw 'Codec Pack worker arguments are not the approved H3 entrypoints.'
    }
    if ([int64]$manifest.worker.probe_timeout_ms -ne 120000) {
        throw 'Codec Pack probe timeout must match the bounded H3 startup contract.'
    }

    if ($manifest.compatibility.app_min_inclusive -cne '0.1.0' -or
        $manifest.compatibility.app_max_exclusive -cne '0.2.0' -or
        [int]$manifest.compatibility.worker_protocol_min -ne 1 -or
        [int]$manifest.compatibility.worker_protocol_max -ne 1 -or
        (@($manifest.compatibility.lc_spec_versions) -join ',') -cne '0.1.0') {
        throw 'Codec Pack application or protocol compatibility is invalid.'
    }
    $profiles = @($manifest.compatibility.profiles)
    if ($profiles.Count -ne 1) {
        throw 'Codec Pack must declare exactly one H3 profile.'
    }
    Assert-ExactProperties -Object $profiles[0] -Required @(
        'codec_family', 'profile', 'profile_versions'
    ) -Context 'compatibility.profiles[0]'
    if ($profiles[0].codec_family -cne 'minimax_h3' -or
        $profiles[0].profile -cne 'h3_av_latent' -or
        (@($profiles[0].profile_versions) -join ',') -cne '0.1.0') {
        throw 'Codec Pack H3 profile declaration is invalid.'
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

    $externalAssets = @($manifest.external_assets)
    if ($externalAssets.Count -eq 0 -or $externalAssets.Count -gt 16) {
        throw 'Codec Pack must declare at least one bounded external decoder asset.'
    }
    foreach ($asset in $externalAssets) {
        Assert-ExactProperties -Object $asset -Required @(
            'asset_id', 'display_name', 'kind', 'required', 'selection', 'format',
            'accepted_variants'
        ) -Context 'external asset'
        Assert-Token -Value ([string]$asset.asset_id) -Name 'asset_id'
        if ($asset.kind -cne 'decoder_weight' -or
            $asset.required -ne $true -or
            $asset.selection -cne 'explicit_file' -or
            $asset.format -cne 'safetensors') {
            throw 'External asset must remain an explicitly selected decoder weight.'
        }
        $variants = @($asset.accepted_variants)
        if ($variants.Count -eq 0 -or $variants.Count -gt 32) {
            throw 'External asset has no bounded accepted variants.'
        }
        $variantIds = [System.Collections.Generic.HashSet[string]]::new(
            [System.StringComparer]::Ordinal
        )
        foreach ($variant in $variants) {
            Assert-ExactProperties -Object $variant -Required @(
                'variant_id', 'sha256', 'byte_length', 'source_url',
                'license_label', 'license_url'
            ) -Context 'external asset variant'
            Assert-Token -Value ([string]$variant.variant_id) -Name 'variant_id'
            if (-not $variantIds.Add([string]$variant.variant_id)) {
                throw 'External asset contains duplicate variant identifiers.'
            }
            Assert-Sha256 -Value ([string]$variant.sha256) -Name 'variant.sha256'
            if ([int64]$variant.byte_length -le 0 -or
                ([string]$variant.source_url) -cnotmatch '^https://' -or
                ([string]$variant.license_url) -cnotmatch '^https://') {
                throw 'External asset variant metadata is incomplete or unsafe.'
            }
        }
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
                if ($script:ArchiveExtensions -contains $extension -and
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

Export-ModuleMember -Function @(
    'Assert-ChildPath',
    'Assert-DirectoryNotReparsePoint',
    'Assert-ExactProperties',
    'Assert-PackagingSourceTree',
    'Assert-PathComponentsNotReparsePoints',
    'Assert-PortableRelativePath',
    'Assert-SafeTemporaryDirectory',
    'Assert-SemVer',
    'Assert-Sha256',
    'Assert-Token',
    'Copy-PackagingTree',
    'Convert-ToPortableRelativePath',
    'Expand-SafeCodecPackArchive',
    'Get-CodecPackAuxiliaryRoot',
    'Get-CodecPackInstallRoot',
    'Get-IntegrityEntry',
    'New-DeterministicZip',
    'Read-StrictJsonFile',
    'Remove-SafeTemporaryDirectory',
    'Test-H3CodecPackDirectory',
    'Write-JsonFile'
)
