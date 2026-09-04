[CmdletBinding()]
param(
    [string]$OutputDirectory,

    [string]$SpoutArchivePath,

    [string]$SbomPath,

    [ValidateSet('unsigned_preview', 'stable')]
    [string]$ReleaseChannel = 'unsigned_preview',

    [string]$ReleaseLabel,

    [string]$SigningCommand,

    [switch]$DevelopmentBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

if ($ReleaseChannel -cnotin @('unsigned_preview', 'stable')) {
    throw 'ReleaseChannel must be exactly unsigned_preview or stable.'
}

if ($PSBoundParameters.ContainsKey('SbomPath')) {
    throw (
        'Prebuilt SBOM input is not accepted; the release builder generates it ' +
        'from the current locked workspace.'
    )
}

Import-Module (Join-Path $PSScriptRoot 'ReleaseSpoutMetadata.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'TauriReleaseContract.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force

$applicationApiVersion = '0.1.0'
$windowsInstallerVersion = '0.1.0+1'
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
    if (-not [string]::IsNullOrWhiteSpace($SigningCommand)) {
        throw 'The unsigned preview channel refuses a signing command.'
    }
} else {
    if ($ReleaseLabel -cne '0.1.0') {
        throw 'The stable channel requires release label 0.1.0.'
    }
    if ([string]::IsNullOrWhiteSpace($SigningCommand) -or
        $SigningCommand -notmatch '%1' -or
        $SigningCommand.Contains("'") -or
        $SigningCommand.Contains("`r") -or
        $SigningCommand.Contains("`n")) {
        throw 'The stable channel requires a single-line Tauri signCommand containing %1 and no single quote.'
    }
    throw (
        'Stable application builds remain disabled until the release workflow can verify ' +
        'Authenticode on both each installer and the corresponding installed executable. ' +
        'Build the unsigned_preview channel for 0.1.0-preview.1.'
    )
}
$artifactTrustSuffix = 'unsigned'
$targetTriple = 'x86_64-pc-windows-msvc'
$nodeBuildPackageNames = @(
    '@sveltejs/vite-plugin-svelte',
    '@tailwindcss/vite',
    '@tauri-apps/cli',
    'svelte',
    'tailwindcss',
    'typescript',
    'vite',
    'vitest'
)
$spoutMetadata = Get-Spout2ReleaseMetadata
$spoutTag = $spoutMetadata.Tag
$spoutCommit = $spoutMetadata.Commit
$spoutArchiveSha256 = $spoutMetadata.ArchiveSha256
$spoutArchiveBytes = [int64]$spoutMetadata.ArchiveBytes
$spoutArchiveUrl = $spoutMetadata.ArchiveUrl
$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null

function Get-PythonProjectIdentity {
    param([Parameter(Mandatory)][string]$ProjectPath)

    $manifestPath = Join-Path $repoRoot "$ProjectPath/pyproject.toml"
    $text = Get-Content -LiteralPath $manifestPath -Raw
    $nameMatches = [regex]::Matches($text, '(?m)^name\s*=\s*"(?<value>[^"]+)"\s*$')
    $versionMatches = [regex]::Matches($text, '(?m)^version\s*=\s*"(?<value>[^"]+)"\s*$')
    if ($nameMatches.Count -ne 1 -or $versionMatches.Count -ne 1) {
        throw "Release component must declare one Python project name and version: $manifestPath"
    }
    return [pscustomobject]@{
        Name = $nameMatches[0].Groups['value'].Value
        Version = $versionMatches[0].Groups['value'].Value
    }
}

$d2DeckIdentity = Get-Content -LiteralPath (
    Join-Path $repoRoot 'operators/builtin/d2/package/deck-pack.json'
) -Raw | ConvertFrom-Json -Depth 32
$q4DeckIdentity = Get-Content -LiteralPath (
    Join-Path $repoRoot 'operators/builtin/q4/package/deck-pack.json'
) -Raw | ConvertFrom-Json -Depth 32
$cartridgeSdkIdentity = Get-PythonProjectIdentity -ProjectPath 'sdk/python'
$deckSdkIdentity = Get-PythonProjectIdentity -ProjectPath 'sdk/deck-python'
$codecSdkIdentity = Get-PythonProjectIdentity -ProjectPath 'sdk/codec-python'
$invalidSdkVersions = @(
    @(
        $cartridgeSdkIdentity.Version,
        $deckSdkIdentity.Version,
        $codecSdkIdentity.Version
    ) | Where-Object { [string]$_ -cnotmatch '^\d+\.\d+\.\d+$' }
)
if ([string]$d2DeckIdentity.deck_id -cne 'org.latentdeck.deck.d2' -or
    [string]$q4DeckIdentity.deck_id -cne 'org.latentdeck.deck.q4' -or
    [string]$d2DeckIdentity.deck_version -cnotmatch '^\d+\.\d+\.\d+$' -or
    [string]$q4DeckIdentity.deck_version -cnotmatch '^\d+\.\d+\.\d+$' -or
    [string]$cartridgeSdkIdentity.Name -cne 'latentdeck-cartridge' -or
    [string]$deckSdkIdentity.Name -cne 'latentdeck-deck-sdk' -or
    [string]$codecSdkIdentity.Name -cne 'latentdeck-codec-sdk' -or
    $invalidSdkVersions.Count -gt 0) {
    throw 'Release component identities drifted from the supported Deck/SDK contracts.'
}
$releaseComponentVersions = [ordered]@{
    decks = [ordered]@{
        d2 = [ordered]@{
            deck_id = [string]$d2DeckIdentity.deck_id
            deck_version = [string]$d2DeckIdentity.deck_version
        }
        q4 = [ordered]@{
            deck_id = [string]$q4DeckIdentity.deck_id
            deck_version = [string]$q4DeckIdentity.deck_version
        }
    }
    sdks = [ordered]@{
        cartridge = [string]$cartridgeSdkIdentity.Version
        deck = [string]$deckSdkIdentity.Version
        codec = [string]$codecSdkIdentity.Version
    }
}

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

function Assert-PathComponentsNotReparsePoints {
    param([Parameter(Mandatory)][string]$Path)

    $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $volumeRoot = [System.IO.Path]::GetPathRoot($fullPath)
    if ([string]::IsNullOrWhiteSpace($volumeRoot)) {
        throw "Release path has no filesystem root: $fullPath"
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
                throw "Release path contains a reparse-point component: $current"
            }
        }
    }
}

function Assert-TauriReleaseConfig {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$ProductName,

        [Parameter(Mandatory)]
        [string]$Identifier,

        [Parameter(Mandatory)]
        [string]$CargoManifestPath,

        [Parameter(Mandatory)]
        [string]$PackageJsonPath
    )

    $config = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
    if ($config.productName -cne $ProductName -or
        $config.identifier -cne $Identifier -or
        $config.version -cne $windowsInstallerVersion) {
        throw "Tauri identity/version mismatch in $Path"
    }
    $package = Get-Content -LiteralPath $PackageJsonPath -Raw | ConvertFrom-Json
    if ($package.version -cne $applicationApiVersion) {
        throw "Application API/package version mismatch in $PackageJsonPath"
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
    Assert-TauriOfflineFrontendContract `
        -ConfigPath $Path `
        -CargoManifestPath $CargoManifestPath `
        -PackageJsonPath $PackageJsonPath
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

function Assert-ReleaseLocksUnchanged {
    param(
        [Parameter(Mandatory)]
        [string]$CargoLockPath,

        [Parameter(Mandatory)]
        [string]$CargoLockSha256,

        [Parameter(Mandatory)]
        [string]$PnpmLockPath,

        [Parameter(Mandatory)]
        [string]$PnpmLockSha256,

        [Parameter(Mandatory)]
        [string]$UvLockPath,

        [Parameter(Mandatory)]
        [string]$UvLockSha256,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ((Get-FileHash -LiteralPath $CargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            $CargoLockSha256 -or
        (Get-FileHash -LiteralPath $PnpmLockPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            $PnpmLockSha256 -or
        (Get-FileHash -LiteralPath $UvLockPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            $UvLockSha256) {
        throw "A release lock file changed $Context."
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
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$ExpectedName,

        [Parameter(Mandatory)]
        [string]$ExpectedVersion
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
            $componentName.GetString() -cne $ExpectedName -or
            $componentVersion.ValueKind -ne [System.Text.Json.JsonValueKind]::String -or
            $componentVersion.GetString() -cne $ExpectedVersion -or
            $components.ValueKind -ne [System.Text.Json.JsonValueKind]::Array) {
            throw "Release SBOM is not the expected $ExpectedName $ExpectedVersion CycloneDX 1.5 document."
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
        $rootLicenseMatches = @($decoded.metadata.component.licenses | Where-Object {
            $null -ne $_.PSObject.Properties['license'] -and
            [string]$_.license.name -ceq 'Apache-2.0'
        })
        if (@($decoded.metadata.component.licenses).Count -ne 1 -or
            $rootLicenseMatches.Count -ne 1) {
            throw 'Release SBOM root component does not declare the reviewed Apache-2.0 artifact license.'
        }
        $rootProperties = @($decoded.metadata.component.properties)
        $includedScopePolicy = @($rootProperties | Where-Object {
            [string]$_.name -ceq 'latentdeck:included-dependency-scopes' -and
            [string]$_.value -ceq 'artifact,runtime,build,runtime+build'
        })
        $excludedScopePolicy = @($rootProperties | Where-Object {
            [string]$_.name -ceq 'latentdeck:excluded-dependency-scopes' -and
            [string]$_.value -ceq 'development'
        })
        $targetPlatformPolicy = @($rootProperties | Where-Object {
            [string]$_.name -ceq 'latentdeck:target-platform' -and
            [string]$_.value -ceq 'x86_64-pc-windows-msvc'
        })
        $excludedNodeDevelopmentProperties = @($rootProperties | Where-Object {
            [string]$_.name -ceq 'latentdeck:excluded-node-development-component-count'
        })
        $excludedNodeDevelopmentCount = 0
        if ($excludedNodeDevelopmentProperties.Count -eq 1) {
            $excludedNodeDevelopmentValue = [string]$excludedNodeDevelopmentProperties[0].value
            if ($excludedNodeDevelopmentValue -notmatch '^(?:0|[1-9][0-9]*)$') {
                throw 'Release SBOM has an invalid excluded Node development component count.'
            }
            $excludedNodeDevelopmentCount = [int64]$excludedNodeDevelopmentValue
        }
        if ($includedScopePolicy.Count -ne 1 -or $excludedScopePolicy.Count -ne 1 -or
            $targetPlatformPolicy.Count -ne 1 -or
            $excludedNodeDevelopmentProperties.Count -ne 1 -or
            $excludedNodeDevelopmentCount -le 0) {
            throw 'Release SBOM does not declare the exact Windows dependency-scope policy.'
        }
        $allowedDependencyScopes = @('artifact', 'runtime', 'build', 'runtime+build')
        $dependencyScopeCounts = [ordered]@{}
        foreach ($decodedComponent in @($decoded.metadata.component) + @($decoded.components)) {
            $scopeProperties = @($decodedComponent.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope'
            })
            if ($scopeProperties.Count -ne 1 -or
                [string]$scopeProperties[0].value -cnotin $allowedDependencyScopes) {
                throw (
                    'Release SBOM component has no exact distributable dependency scope: ' +
                    "$($decodedComponent.name)@$($decodedComponent.version)"
                )
            }
            $scope = [string]$scopeProperties[0].value
            if (-not $dependencyScopeCounts.Contains($scope)) {
                $dependencyScopeCounts[$scope] = 0
            }
            $dependencyScopeCounts[$scope] = [int]$dependencyScopeCounts[$scope] + 1
        }
        $missingLicenseComponents = @(
            foreach ($decodedComponent in @($decoded.metadata.component) + @($decoded.components)) {
                $licenseProperty = $decodedComponent.PSObject.Properties['licenses']
                $validLicenseCount = 0
                if ($null -ne $licenseProperty) {
                    foreach ($licenseEntry in @($licenseProperty.Value)) {
                        if (($null -ne $licenseEntry.PSObject.Properties['expression'] -and
                            -not [string]::IsNullOrWhiteSpace([string]$licenseEntry.expression)) -or
                            ($null -ne $licenseEntry.PSObject.Properties['license'] -and
                            (($null -ne $licenseEntry.license.PSObject.Properties['id'] -and
                            -not [string]::IsNullOrWhiteSpace([string]$licenseEntry.license.id)) -or
                            ($null -ne $licenseEntry.license.PSObject.Properties['name'] -and
                            -not [string]::IsNullOrWhiteSpace([string]$licenseEntry.license.name))))) {
                            $validLicenseCount += 1
                        }
                    }
                }
                if ($validLicenseCount -eq 0) {
                    "$($decodedComponent.name)@$($decodedComponent.version)"
                }
            }
        )
        if ($missingLicenseComponents.Count -gt 0) {
            throw (
                'Release SBOM has missing license metadata and cannot be distributed: ' +
                ($missingLicenseComponents -join ', ')
            )
        }
        $workspaceNodeManifest = Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw |
            ConvertFrom-Json -Depth 16
        foreach ($nodeBuildPackageName in $nodeBuildPackageNames) {
            $expectedVersionProperty = $workspaceNodeManifest.devDependencies.PSObject.Properties[
                $nodeBuildPackageName
            ]
            if ($null -eq $expectedVersionProperty -or
                [string]::IsNullOrWhiteSpace([string]$expectedVersionProperty.Value)) {
                throw "Release SBOM build root is not pinned in package.json: $nodeBuildPackageName"
            }
            $expectedBuildComponents = @($decoded.components | Where-Object {
                if ([string]$_.name -cne $nodeBuildPackageName -or
                    [string]$_.version -cne [string]$expectedVersionProperty.Value) {
                    return $false
                }
                $ecosystem = @($_.properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:ecosystem' -and
                    [string]$_.value -ceq 'node'
                })
                $scope = @($_.properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                    [string]$_.value -cin @('build', 'runtime+build')
                })
                return $ecosystem.Count -eq 1 -and $scope.Count -eq 1
            })
            if ($expectedBuildComponents.Count -ne 1) {
                throw (
                    'Release SBOM does not contain the exact locked Node build root: ' +
                    "$nodeBuildPackageName@$($expectedVersionProperty.Value)"
                )
            }
        }
        $nsisComponents = @($decoded.components | Where-Object {
            [string]$_.'bom-ref' -ceq 'tool:nsis@3.11' -and
            [string]$_.name -ceq 'NSIS' -and
            [string]$_.version -ceq '3.11'
        })
        $tauriUtilsComponents = @($decoded.components | Where-Object {
            [string]$_.'bom-ref' -ceq 'native:nsis-tauri-utils@0.5.3' -and
            [string]$_.name -ceq 'nsis-tauri-utils' -and
            [string]$_.version -ceq '0.5.3'
        })
        $webViewDisposition = @($rootProperties | Where-Object {
            [string]$_.name -ceq 'latentdeck:webview2-bootstrapper-disposition' -and
            [string]$_.value -ceq 'not_redistributed_install_time_download'
        })
        $webViewMode = @($rootProperties | Where-Object {
            [string]$_.name -ceq 'latentdeck:webview2-install-mode' -and
            [string]$_.value -ceq 'downloadBootstrapper'
        })
        if ($nsisComponents.Count -ne 1 -or $tauriUtilsComponents.Count -ne 1 -or
            $webViewDisposition.Count -ne 1 -or $webViewMode.Count -ne 1) {
            throw 'Release SBOM does not bind the exact Tauri Windows installer runtime closure.'
        }
        $nsisProperties = @($nsisComponents[0].properties)
        $tauriUtilsProperties = @($tauriUtilsComponents[0].properties)
        if (@($nsisProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -ceq 'runtime+build'
            }).Count -ne 1 -or
            @($nsisProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:source-archive-sha1' -and
                [string]$_.value -ceq 'ef7ff767e5cbd9edd22add3a32c9b8f4500bb10d'
            }).Count -ne 1 -or
            @($nsisProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:source-tree-sha256' -and
                [string]$_.value -ceq 'e9ddbf15e780350628b8e9e334b770bfbb59004f2d6b5c2c43ce764d9530e063'
            }).Count -ne 1 -or
            @($nsisProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:license-text-sha256' -and
                [string]$_.value -ceq 'e7dd514003ab96cb3ddccbc028fe5c795fccf57dc41f21cfb9d4dd16ead23bf5'
            }).Count -ne 1 -or
            @($tauriUtilsProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -ceq 'runtime+build'
            }).Count -ne 1 -or
            @($tauriUtilsProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:source-commit' -and
                [string]$_.value -ceq '13d9edd27b69310e108d6fbd49f90992f8a05390'
            }).Count -ne 1 -or
            @($tauriUtilsComponents[0].hashes | Where-Object {
                [string]$_.alg -ceq 'SHA-256' -and
                [string]$_.content -ceq '5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709'
            }).Count -ne 1) {
            throw 'Release SBOM Tauri Windows installer identity or redistribution scope drifted.'
        }
    } finally {
        $document.Dispose()
    }
    return [pscustomobject]@{
        Path = $resolved
        ByteLength = [int64]$item.Length
        Sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
        ComponentCount = $componentCount
        MissingLicenseComponentCount = 0
        DependencyScopeCounts = $dependencyScopeCounts
    }
}

function New-ApplicationThirdPartyNotice {
    param(
        [Parameter(Mandatory)][string]$LatentDeckSbomPath,
        [Parameter(Mandatory)][string]$LatentPlayerSbomPath,
        [Parameter(Mandatory)][string]$RepositoryNoticePath,
        [Parameter(Mandatory)][string]$DestinationPath,
        [Parameter(Mandatory)][string]$ReleaseLabel
    )

    [void](Test-Spout2ThirdPartyNotice -Path $RepositoryNoticePath)
    $repositoryNotice = [System.IO.File]::ReadAllText($RepositoryNoticePath)
    $spoutStart = $repositoryNotice.IndexOf(
        '## Spout2',
        [System.StringComparison]::Ordinal
    )
    if ($spoutStart -lt 0) {
        throw 'Could not extract the reviewed Spout2 notice section.'
    }
    $spoutSection = $repositoryNotice.Substring($spoutStart).Trim()
    if ($spoutSection -match '(?i)\b(?:taehv|taeh3|H3)\b') {
        throw 'The extracted application Spout2 notice contains codec-only material.'
    }

    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in @(
        '# LatentDeck applications third-party notices',
        '',
        "Artifact set: LatentDeck App and LatentPlayer $ReleaseLabel",
        '',
        'This notice is scoped to the two Windows application installers. It does not',
        'cover separately distributed codec packs, decoder assets, or the Developer Kit.',
        'Dependency labels below come from the artifact-scoped, lock-generated SBOMs;',
        'no absent license value is inferred.',
        ''
    )) {
        $lines.Add($line)
    }
    foreach ($application in @(
        [pscustomobject]@{ Name = 'LatentDeck App'; Path = $LatentDeckSbomPath },
        [pscustomobject]@{ Name = 'LatentPlayer'; Path = $LatentPlayerSbomPath }
    )) {
        $bom = Get-Content -LiteralPath $application.Path -Raw | ConvertFrom-Json -Depth 100
        $lines.Add("## $($application.Name) locked component license inventory")
        $lines.Add('')
        foreach ($component in @($bom.components | Sort-Object 'bom-ref')) {
            if ([string]$component.name -ceq 'Spout2') {
                continue
            }
            $labels = @(
                foreach ($entry in @($component.licenses)) {
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
            if ($labels.Count -eq 0) {
                throw "Application notice source has missing license metadata: $($component.name)"
            }
            $lines.Add(
                "- $($component.name) $($component.version) - $(@($labels | Sort-Object -Unique) -join ' OR ')"
            )
        }
        $lines.Add('')
    }
    $lines.Add($spoutSection)
    $text = ($lines -join "`n") + "`n"
    if ($text -match '(?i)\b(?:taehv|taeh3|H3)\b' -or
        -not $text.Contains(
            '# LatentDeck applications third-party notices',
            [System.StringComparison]::Ordinal
        )) {
        throw 'Generated application notices are not artifact-scoped.'
    }
    [System.IO.File]::WriteAllText(
        $DestinationPath,
        $text,
        [System.Text.UTF8Encoding]::new($false)
    )
    $item = Get-Item -LiteralPath $DestinationPath -Force
    if ($item.Length -eq 0 -or $item.Length -gt 1MB) {
        throw 'Generated application notices are empty or unbounded.'
    }
    return [pscustomobject]@{
        Path = $item.FullName
        ByteLength = [int64]$item.Length
        Sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
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
Assert-PathComponentsNotReparsePoints -Path $outputRoot
$finalDirectory = Join-Path $outputRoot "$ReleaseLabel-windows-x64"
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $finalDirectory | Out-Null
if (Test-Path -LiteralPath $finalDirectory) {
    throw "Refusing to overwrite an existing release-candidate directory: $finalDirectory"
}

Invoke-Checked `
    -Description 'Pre-build public-tree audit' `
    -Command { & pwsh -NoProfile -File (Join-Path $repoRoot 'tools/Test-PublicTree.ps1') }
$sourceSnapshotBefore = Get-ReleaseSourceSnapshot -RepositoryRoot $repoRoot
$gitCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $gitCommit -cnotmatch '^[0-9a-f]{40}$') {
    throw 'Could not resolve the release source Git commit.'
}
$gitTree = (& git -C $repoRoot rev-parse 'HEAD^{tree}').Trim()
if ($LASTEXITCODE -ne 0 -or $gitTree -cnotmatch '^[0-9a-f]{40}$') {
    throw 'Could not resolve the release source Git tree.'
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
$distributable = (-not $DevelopmentBuild.IsPresent -and
    $gitBranch -ceq 'main' -and $gitStatusBefore.Count -eq 0)
if (-not $DevelopmentBuild.IsPresent -and -not $distributable) {
    throw 'Release candidates must be built from a clean main checkout; use -DevelopmentBuild only for non-distributable local contract work.'
}

$deckRoot = Join-Path $repoRoot 'apps/latentdeck'
$playerRoot = Join-Path $repoRoot 'apps/latentplayer'
$cargoLockPath = Join-Path $repoRoot 'Cargo.lock'
$pnpmLockPath = Join-Path $repoRoot 'pnpm-lock.yaml'
$uvLockPath = Join-Path $repoRoot 'uv.lock'
foreach ($lockPath in @($cargoLockPath, $pnpmLockPath, $uvLockPath)) {
    if (-not (Test-Path -LiteralPath $lockPath -PathType Leaf)) {
        throw "Required release lock file is missing: $lockPath"
    }
}
$cargoLockHash = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$pnpmLockHash = (Get-FileHash -LiteralPath $pnpmLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$uvLockHash = (Get-FileHash -LiteralPath $uvLockPath -Algorithm SHA256).Hash.ToLowerInvariant()
$deckConfig = Assert-TauriReleaseConfig `
    -Path (Join-Path $deckRoot 'src-tauri/tauri.conf.json') `
    -ProductName 'LatentDeck App' `
    -Identifier 'studio.latentdeck.deck' `
    -CargoManifestPath (Join-Path $deckRoot 'src-tauri/Cargo.toml') `
    -PackageJsonPath (Join-Path $deckRoot 'package.json')
$playerConfig = Assert-TauriReleaseConfig `
    -Path (Join-Path $playerRoot 'src-tauri/tauri.conf.json') `
    -ProductName 'LatentPlayer' `
    -Identifier 'studio.latentdeck.player' `
    -CargoManifestPath (Join-Path $playerRoot 'src-tauri/Cargo.toml') `
    -PackageJsonPath (Join-Path $playerRoot 'package.json')
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

    $generatedDeckSbomPath = Join-Path $buildRoot "LatentDeck-App-$ReleaseLabel-sbom.cdx.json"
    $generatedPlayerSbomPath = Join-Path $buildRoot "LatentPlayer-$ReleaseLabel-sbom.cdx.json"
    Invoke-Checked `
        -Description 'Fresh LatentDeck App artifact SBOM generation' `
        -Command {
            & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
                -OutputPath $generatedDeckSbomPath `
                -ArtifactName 'LatentDeck App' `
                -ArtifactVersion $windowsInstallerVersion `
                -ArtifactScope application `
                -CargoPackage latentdeck-app `
                -NodePackage '@latentdeck/app' `
                -NodeBuildPackage $nodeBuildPackageNames `
                -NodeRuntimeBuildPackage @('svelte', 'tailwindcss', 'vite') `
                -IncludeSpout2 `
                -IncludeTauriWindowsInstaller
        }
    Invoke-Checked `
        -Description 'Fresh LatentPlayer artifact SBOM generation' `
        -Command {
            & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
                -OutputPath $generatedPlayerSbomPath `
                -ArtifactName 'LatentPlayer' `
                -ArtifactVersion $windowsInstallerVersion `
                -ArtifactScope application `
                -CargoPackage latentplayer-app `
                -NodePackage '@latentdeck/player' `
                -NodeBuildPackage $nodeBuildPackageNames `
                -NodeRuntimeBuildPackage @('svelte', 'tailwindcss', 'vite') `
                -IncludeSpout2 `
                -IncludeTauriWindowsInstaller
        }
    $deckSbomInput = Assert-CycloneDxSbom `
        -Path $generatedDeckSbomPath `
        -ExpectedName 'LatentDeck App' `
        -ExpectedVersion $windowsInstallerVersion
    $playerSbomInput = Assert-CycloneDxSbom `
        -Path $generatedPlayerSbomPath `
        -ExpectedName 'LatentPlayer' `
        -ExpectedVersion $windowsInstallerVersion
    $applicationNoticeInput = New-ApplicationThirdPartyNotice `
        -LatentDeckSbomPath $generatedDeckSbomPath `
        -LatentPlayerSbomPath $generatedPlayerSbomPath `
        -RepositoryNoticePath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md') `
        -DestinationPath (Join-Path $buildRoot 'APPLICATION_THIRD_PARTY_NOTICES.md') `
        -ReleaseLabel $ReleaseLabel
    $applicationLicenseBundleInput = New-ReleaseLicenseBundle `
        -SbomPath @($generatedDeckSbomPath, $generatedPlayerSbomPath) `
        -ArtifactName 'LatentDeck Windows Applications' `
        -ArtifactVersion $ReleaseLabel `
        -OutputPath (Join-Path $buildRoot 'APPLICATION_THIRD_PARTY_LICENSES.json') `
        -RepositoryNoticePath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md')
    Assert-ReleaseLocksUnchanged `
        -CargoLockPath $cargoLockPath `
        -CargoLockSha256 $cargoLockHash `
        -PnpmLockPath $pnpmLockPath `
        -PnpmLockSha256 $pnpmLockHash `
        -UvLockPath $uvLockPath `
        -UvLockSha256 $uvLockHash `
        -Context 'during fresh artifact-scoped SBOM generation'

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
        '--features', 'spout-sdk'
    )
    if ($ReleaseChannel -ceq 'unsigned_preview') {
        $tauriArguments += '--no-sign'
    } else {
        $signingConfigPath = Join-Path $buildRoot 'tauri-signing.json'
        [System.IO.File]::WriteAllText(
            $signingConfigPath,
            (([ordered]@{
                bundle = [ordered]@{
                    windows = [ordered]@{ signCommand = $SigningCommand }
                }
            } | ConvertTo-Json -Depth 8) + "`n"),
            [System.Text.UTF8Encoding]::new($false)
        )
        $tauriArguments += @('--config', $signingConfigPath)
    }
    $tauriArguments += @('--', '--locked')
    Invoke-Checked `
        -Description "LatentDeck $ReleaseChannel NSIS build" `
        -Command { & $pnpm --dir $deckRoot @tauriArguments }
    Invoke-Checked `
        -Description "LatentPlayer $ReleaseChannel NSIS build" `
        -Command { & $pnpm --dir $playerRoot @tauriArguments }

    $releaseBinaryRoot = Join-Path $env:CARGO_TARGET_DIR "$targetTriple/release"
    Assert-TauriEmbeddedFrontendBinary `
        -BinaryPath (Join-Path $releaseBinaryRoot 'latentdeck-app.exe') `
        -FrontendDistPath (Join-Path $deckRoot 'dist')
    Assert-TauriEmbeddedFrontendBinary `
        -BinaryPath (Join-Path $releaseBinaryRoot 'latentplayer-app.exe') `
        -FrontendDistPath (Join-Path $playerRoot 'dist')

    Assert-ReleaseLocksUnchanged `
        -CargoLockPath $cargoLockPath `
        -CargoLockSha256 $cargoLockHash `
        -PnpmLockPath $pnpmLockPath `
        -PnpmLockSha256 $pnpmLockHash `
        -UvLockPath $uvLockPath `
        -UvLockSha256 $uvLockHash `
        -Context 'during the frozen/locked build'
    Invoke-Checked `
        -Description 'Post-build public-tree audit' `
        -Command { & pwsh -NoProfile -File (Join-Path $repoRoot 'tools/Test-PublicTree.ps1') }
    $sourceSnapshotAfter = Get-ReleaseSourceSnapshot -RepositoryRoot $repoRoot
    if ($sourceSnapshotAfter.Sha256 -cne $sourceSnapshotBefore.Sha256 -or
        $sourceSnapshotAfter.FileCount -ne $sourceSnapshotBefore.FileCount) {
        throw 'The public source snapshot changed while the release candidate was building.'
    }
    $gitCommitAfter = (& git -C $repoRoot rev-parse HEAD).Trim()
    $gitTreeAfter = (& git -C $repoRoot rev-parse 'HEAD^{tree}').Trim()
    $gitBranchAfter = (& git -C $repoRoot branch --show-current).Trim()
    $gitStatusAfter = @(& git -C $repoRoot status --porcelain=v1 --untracked-files=all)
    if ($LASTEXITCODE -ne 0 -or
        $gitCommitAfter -cne $gitCommit -or
        $gitTreeAfter -cne $gitTree -or
        $gitBranchAfter -cne $gitBranch -or
        ($gitStatusAfter -join "`n") -cne ($gitStatusBefore -join "`n")) {
        throw 'The Git source identity changed while the release candidate was building.'
    }

    $nsisRoot = Join-Path $env:CARGO_TARGET_DIR "$targetTriple/release/bundle/nsis"
    $expectedSources = @(
        [ordered]@{
            product = 'LatentDeck App'
            identifier = 'studio.latentdeck.deck'
            source_name = "LatentDeck App_${windowsInstallerVersion}_x64-setup.exe"
            artifact_name = "LatentDeck-$ReleaseLabel-windows-x64-$artifactTrustSuffix-setup.exe"
            sbom_input = $deckSbomInput
            sbom_slug = 'LatentDeck-App'
        },
        [ordered]@{
            product = 'LatentPlayer'
            identifier = 'studio.latentdeck.player'
            source_name = "LatentPlayer_${windowsInstallerVersion}_x64-setup.exe"
            artifact_name = "LatentPlayer-$ReleaseLabel-windows-x64-$artifactTrustSuffix-setup.exe"
            sbom_input = $playerSbomInput
            sbom_slug = 'LatentPlayer'
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
        $authenticode = Get-AuthenticodeSignature -LiteralPath $source
        if ($ReleaseChannel -ceq 'unsigned_preview' -and
            $authenticode.Status -ne [System.Management.Automation.SignatureStatus]::NotSigned) {
            throw "Unsigned preview installer unexpectedly crossed the signing boundary: $($expected.source_name)"
        }
        if ($ReleaseChannel -ceq 'stable' -and
            $authenticode.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            throw "Stable installer failed Authenticode verification: $($expected.source_name)"
        }
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
            application_api_version = $applicationApiVersion
            windows_installer_version = $windowsInstallerVersion
            unsigned = ($ReleaseChannel -ceq 'unsigned_preview')
            authenticode = if ($ReleaseChannel -ceq 'unsigned_preview') { 'not_present' } else { 'valid' }
            spout_sdk = $true
            sbom_file_name = "metadata/$($expected.sbom_slug)-$ReleaseLabel-sbom.cdx.json"
        }
    }

    $metadataDirectory = Join-Path $outputStage 'metadata'
    [System.IO.Directory]::CreateDirectory($metadataDirectory) | Out-Null
    $sbomReceipts = @()
    foreach ($expected in $expectedSources) {
        $sbomFileName = "$($expected.sbom_slug)-$ReleaseLabel-sbom.cdx.json"
        $sbomDestination = Join-Path $metadataDirectory $sbomFileName
        [System.IO.File]::Copy($expected.sbom_input.Path, $sbomDestination, $false)
        $stagedSbom = Assert-CycloneDxSbom `
            -Path $sbomDestination `
            -ExpectedName $expected.product `
            -ExpectedVersion $windowsInstallerVersion
        if ($stagedSbom.Sha256 -cne $expected.sbom_input.Sha256 -or
            $stagedSbom.ByteLength -ne $expected.sbom_input.ByteLength -or
            $stagedSbom.ComponentCount -ne $expected.sbom_input.ComponentCount) {
            throw "Staged $($expected.product) SBOM does not match its fresh artifact inventory."
        }
        $sbomReceipts += [ordered]@{
            product = $expected.product
            file_name = "metadata/$sbomFileName"
            byte_length = $stagedSbom.ByteLength
            sha256 = $stagedSbom.Sha256
            format = 'CycloneDX'
            spec_version = '1.5'
            component_count = $stagedSbom.ComponentCount
            artifact_scope = 'application'
            dependency_scope_counts = $stagedSbom.DependencyScopeCounts
            license_review = [ordered]@{
                status = 'complete'
                missing_license_component_count = $stagedSbom.MissingLicenseComponentCount
            }
            generated_from_locks = [ordered]@{
                cargo_lock_sha256 = $cargoLockHash
                pnpm_lock_sha256 = $pnpmLockHash
            }
        }
    }

    $noticeFileName = 'THIRD_PARTY_NOTICES.md'
    $noticeDestination = Join-Path $metadataDirectory $noticeFileName
    [System.IO.File]::Copy($applicationNoticeInput.Path, $noticeDestination, $false)
    $stagedNotice = Get-Item -LiteralPath $noticeDestination -Force
    $stagedNoticeHash = (Get-FileHash -LiteralPath $stagedNotice.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($stagedNoticeHash -cne $applicationNoticeInput.Sha256 -or
        [int64]$stagedNotice.Length -ne $applicationNoticeInput.ByteLength) {
        throw 'Staged application notices do not match the generated artifact-scoped notice.'
    }
    $noticeReceipt = [ordered]@{
        file_name = "metadata/$noticeFileName"
        byte_length = [int64]$stagedNotice.Length
        sha256 = $stagedNoticeHash
        components = @(
            [ordered]@{
                name = 'Spout2'
                version = $spoutTag
                commit = $spoutCommit
                license = $spoutMetadata.LicenseId
            }
        )
    }
    $licenseBundleFileName = 'THIRD_PARTY_LICENSES.json'
    $licenseBundleDestination = Join-Path $metadataDirectory $licenseBundleFileName
    [System.IO.File]::Copy(
        $applicationLicenseBundleInput.Path,
        $licenseBundleDestination,
        $false
    )
    $stagedLicenseBundle = Test-ReleaseLicenseBundle `
        -BundlePath $licenseBundleDestination `
        -SbomPath @($generatedDeckSbomPath, $generatedPlayerSbomPath) `
        -ExpectedArtifactName 'LatentDeck Windows Applications' `
        -ExpectedArtifactVersion $ReleaseLabel
    if ($stagedLicenseBundle.Sha256 -cne $applicationLicenseBundleInput.Sha256 -or
        $stagedLicenseBundle.ByteLength -ne $applicationLicenseBundleInput.ByteLength -or
        $stagedLicenseBundle.ComponentCount -ne $applicationLicenseBundleInput.ComponentCount) {
        throw 'Staged application license bundle does not match its exact SBOM closure.'
    }
    $licenseBundleReceipt = [ordered]@{
        file_name = "metadata/$licenseBundleFileName"
        byte_length = $stagedLicenseBundle.ByteLength
        sha256 = $stagedLicenseBundle.Sha256
        schema_version = 1
        component_count = $stagedLicenseBundle.ComponentCount
        text_count = $stagedLicenseBundle.TextCount
        build_only_no_text_disposition_count = $stagedLicenseBundle.NoTextDispositionCount
    }

    $manifestPath = Join-Path $outputStage 'release-candidate.json'
    $manifest = [ordered]@{
        schema_version = 6
        release_label = $ReleaseLabel
        release_channel = $ReleaseChannel
        application_api_version = $applicationApiVersion
        windows_installer_version = $windowsInstallerVersion
        component_versions = $releaseComponentVersions
        target = $targetTriple
        local_release_candidate = $true
        signed = ($ReleaseChannel -ceq 'stable')
        unsigned = ($ReleaseChannel -ceq 'unsigned_preview')
        distributable = $distributable
        updater_artifacts = $false
        contains_codec_pack = $false
        contains_model_weights = $false
        contains_cartridges = $false
        source = [ordered]@{
            git_commit = $gitCommit
            git_branch = $gitBranch
            git_tree = $gitTree
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
            uv_lock_sha256 = $uvLockHash
        }
        spout2 = [ordered]@{
            tag = $spoutTag
            commit = $spoutCommit
            archive_sha256 = $spoutArchiveSha256
            feature = 'spout-sdk'
        }
        sboms = $sbomReceipts
        license_review = [ordered]@{
            status = 'complete'
            missing_license_component_count = 0
            redistributed_component_text_coverage = 'complete'
        }
        license_bundle = $licenseBundleReceipt
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
        foreach ($sbomReceipt in $sbomReceipts) {
            "$($sbomReceipt.sha256)  $($sbomReceipt.file_name)"
        }
        "$($noticeReceipt.sha256)  $($noticeReceipt.file_name)"
        "$($licenseBundleReceipt.sha256)  $($licenseBundleReceipt.file_name)"
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $outputStage 'SHA256SUMS.txt'),
        ($hashLines -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $outputStage 'BUILD-COMMANDS.txt'),
        (@(
            "pwsh -NoProfile -File tools/Build-ReleaseCandidate.ps1 -ReleaseChannel $ReleaseChannel -ReleaseLabel $ReleaseLabel [-SpoutArchivePath <optional exact pinned archive>]"
            'pnpm --dir . install --frozen-lockfile'
            'pwsh -NoProfile -File tools/New-Sbom.ps1 -ArtifactScope application -OutputPath <unique private release staging path> ...'
            'pwsh -NoProfile -File tools/Prepare-Spout2.ps1'
            'Build helper: exclusive verify/extract pinned Spout2 archive to private ignored staging and set LATENTDECK_SPOUT2_SOURCE_ROOT'
            "pnpm --dir apps/latentdeck exec tauri build --ci --target x86_64-pc-windows-msvc --bundles nsis --features spout-sdk $(if ($ReleaseChannel -ceq 'unsigned_preview') { '--no-sign' } else { '--config <private signing config>' }) -- --locked"
            "pnpm --dir apps/latentplayer exec tauri build --ci --target x86_64-pc-windows-msvc --bundles nsis --features spout-sdk $(if ($ReleaseChannel -ceq 'unsigned_preview') { '--no-sign' } else { '--config <private signing config>' }) -- --locked"
        ) -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    $stageFiles = @(Get-ChildItem -LiteralPath $outputStage -File -Force -Recurse)
    $expectedRelativeNames = @(
        'BUILD-COMMANDS.txt',
        'release-candidate.json',
        'SHA256SUMS.txt',
        "metadata/LatentDeck-App-$ReleaseLabel-sbom.cdx.json",
        "metadata/LatentPlayer-$ReleaseLabel-sbom.cdx.json",
        'metadata/THIRD_PARTY_NOTICES.md',
        'metadata/THIRD_PARTY_LICENSES.json',
        "installers/LatentDeck-$ReleaseLabel-windows-x64-$artifactTrustSuffix-setup.exe",
        "installers/LatentPlayer-$ReleaseLabel-windows-x64-$artifactTrustSuffix-setup.exe"
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
    foreach ($sbomReceipt in $sbomReceipts) {
        $finalStagedSbomHash = (
            Get-FileHash -LiteralPath (Join-Path $outputStage $sbomReceipt.file_name) -Algorithm SHA256
        ).Hash.ToLowerInvariant()
        if ($finalStagedSbomHash -cne $sbomReceipt.sha256) {
            throw "Staged SBOM hash changed before finalization: $($sbomReceipt.file_name)"
        }
    }
    $finalStagedNoticeHash = (
        Get-FileHash -LiteralPath $noticeDestination -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    if ($finalStagedNoticeHash -cne $noticeReceipt.sha256) {
        throw 'Staged third-party notices changed before finalization.'
    }
    Assert-ReleaseLocksUnchanged `
        -CargoLockPath $cargoLockPath `
        -CargoLockSha256 $cargoLockHash `
        -PnpmLockPath $pnpmLockPath `
        -PnpmLockSha256 $pnpmLockHash `
        -UvLockPath $uvLockPath `
        -UvLockSha256 $uvLockHash `
        -Context 'before release-candidate finalization'

    [System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
    Assert-PathComponentsNotReparsePoints -Path $outputRoot
    Assert-PathComponentsNotReparsePoints -Path $outputStage
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
        Assert-PathComponentsNotReparsePoints -Path $buildRoot
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
        Assert-PathComponentsNotReparsePoints -Path $outputStage
        if (-not ([System.IO.Path]::GetFileName($outputStage)).StartsWith(
            '.rc-o-',
            [System.StringComparison]::Ordinal
        )) {
            throw "Refusing to remove unsafe output staging path: $outputStage"
        }
        Remove-Item -LiteralPath $outputStage -Recurse -Force
    }
}
