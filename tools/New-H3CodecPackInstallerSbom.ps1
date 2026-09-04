[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackVersion,

    [Parameter(Mandatory)]
    [string]$OutputPath,

    [string]$NsisRoot,

    [switch]$AllowNetwork
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Assert-SemVer -Value $PackVersion -Name 'PackVersion'

$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
if (Test-Path -LiteralPath $outputFullPath) {
    throw "Refusing to overwrite an existing Codec Pack setup SBOM: $outputFullPath"
}
$outputDirectory = [System.IO.Path]::GetDirectoryName($outputFullPath)
if ([string]::IsNullOrWhiteSpace($outputDirectory)) {
    throw 'Codec Pack setup SBOM output must have a parent directory.'
}
[System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null

$nsisParameters = @{}
if (-not [string]::IsNullOrWhiteSpace($NsisRoot)) {
    $nsisParameters.NsisRoot = $NsisRoot
}
if ($AllowNetwork) {
    $nsisParameters.AllowNetwork = $true
}
$resolvedNsisRoot = [string](& (Join-Path $PSScriptRoot 'Get-PinnedNsis.ps1') @nsisParameters)
$makeNsis = Join-Path $resolvedNsisRoot 'makensis.exe'
$makeNsisCore = Join-Path $resolvedNsisRoot 'Bin/makensis.exe'
$nsisVersion = (& $makeNsis /VERSION 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $nsisVersion -cne 'v3.11') {
    throw "Codec Pack setup SBOM requires pinned NSIS v3.11; found '$nsisVersion'."
}
$makeNsisSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $makeNsisCore).Hash.ToLowerInvariant()
if ($makeNsisSha256 -cne '42850802704ecb11163f7e0329d35ee54bd288953200d4966e226d572848cfc5') {
    throw "Pinned NSIS compiler SHA-256 mismatch: $makeNsisSha256"
}
$nsisTreeSha256 = '9c81d169c38167ff2688ee187098096ac3c2e9744f017e0eea5936f83fc74ef8'
$cargoLockPath = Join-Path $repositoryRoot 'Cargo.lock'
$cargoLockSha256 = (
    Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256
).Hash.ToLowerInvariant()
$sourceDateEpoch = [int64]1741475120

function New-DeterministicUuid {
    param(
        [Parameter(Mandatory)]
        [string]$Seed
    )

    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $digest = $hasher.ComputeHash(
            [System.Text.UTF8Encoding]::new($false).GetBytes($Seed)
        )
    } finally {
        $hasher.Dispose()
    }
    $uuidBytes = [byte[]]::new(16)
    [System.Array]::Copy($digest, $uuidBytes, $uuidBytes.Length)
    # RFC 9562 UUIDv8 reserves this version for deterministic custom layouts.
    $uuidBytes[6] = [byte](($uuidBytes[6] -band 0x0F) -bor 0x80)
    $uuidBytes[8] = [byte](($uuidBytes[8] -band 0x3F) -bor 0x80)
    $hex = [System.Convert]::ToHexString($uuidBytes).ToLowerInvariant()
    return (
        $hex.Substring(0, 8) + '-' +
        $hex.Substring(8, 4) + '-' +
        $hex.Substring(12, 4) + '-' +
        $hex.Substring(16, 4) + '-' +
        $hex.Substring(20, 12)
    )
}

function Get-CargoDependencyScopeMap {
    param(
        [Parameter(Mandatory)][object]$Metadata,
        [Parameter(Mandatory)][string]$RootPackageId
    )

    $nodesById = @{}
    foreach ($node in @($Metadata.resolve.nodes)) {
        $nodeId = [string]$node.id
        if ([string]::IsNullOrWhiteSpace($nodeId) -or $nodesById.ContainsKey($nodeId)) {
            throw 'Cargo metadata contains a missing or duplicate dependency node id.'
        }
        $nodesById[$nodeId] = $node
    }
    if (-not $nodesById.ContainsKey($RootPackageId)) {
        throw 'Cargo metadata does not contain the Codec Pack installer dependency root.'
    }
    $scopes = @{}
    $scopes[$RootPackageId] = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    [void]$scopes[$RootPackageId].Add('artifact')
    $pending = [System.Collections.Generic.Queue[object]]::new()
    $pending.Enqueue([pscustomobject]@{ Id = $RootPackageId; Scope = 'runtime' })
    $visited = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    while ($pending.Count -gt 0) {
        $state = $pending.Dequeue()
        $packageId = [string]$state.Id
        $traversalScope = [string]$state.Scope
        if ($traversalScope -notin @('runtime', 'build')) {
            throw "Cargo dependency traversal produced an unknown scope: $traversalScope"
        }
        if (-not $visited.Add("$packageId`0$traversalScope")) {
            continue
        }
        foreach ($dependency in @($nodesById[$packageId].deps)) {
            $dependencyKinds = @($dependency.dep_kinds)
            if ($dependencyKinds.Count -eq 0) {
                throw "Cargo dependency has no kind: $packageId -> $($dependency.pkg)"
            }
            foreach ($dependencyKind in $dependencyKinds) {
                $kind = if ($null -eq $dependencyKind.kind) {
                    'normal'
                } else {
                    [string]$dependencyKind.kind
                }
                if ($kind -ceq 'dev') {
                    continue
                }
                $dependencyScope = if ($kind -ceq 'build') {
                    'build'
                } elseif ($kind -ceq 'normal') {
                    $traversalScope
                } else {
                    throw "Cargo dependency has an unsupported kind: $kind"
                }
                $dependencyId = [string]$dependency.pkg
                if ([string]::IsNullOrWhiteSpace($dependencyId) -or
                    -not $nodesById.ContainsKey($dependencyId)) {
                    throw 'Cargo dependency graph contains an unresolved package id.'
                }
                if (-not $scopes.ContainsKey($dependencyId)) {
                    $scopes[$dependencyId] = [System.Collections.Generic.HashSet[string]]::new(
                        [System.StringComparer]::Ordinal
                    )
                }
                if ($dependencyId -cne $RootPackageId) {
                    [void]$scopes[$dependencyId].Add($dependencyScope)
                }
                $pending.Enqueue([pscustomobject]@{ Id = $dependencyId; Scope = $dependencyScope })
            }
        }
    }
    return ,$scopes
}

function Get-DependencyScopeValue {
    param(
        [Parameter(Mandatory)][System.Collections.Generic.HashSet[string]]$Scopes,
        [Parameter(Mandatory)][string]$Context
    )

    $values = @($Scopes | Sort-Object)
    $joined = $values -join ','
    if ($joined -ceq 'artifact' -or $joined -ceq 'runtime' -or $joined -ceq 'build') {
        return $joined
    }
    if ($joined -ceq 'build,runtime') {
        return 'runtime+build'
    }
    throw "Codec Pack setup SBOM dependency scope is invalid for ${Context}: $joined"
}

Push-Location $repositoryRoot
try {
    $metadataArguments = @(
        'metadata', '--locked', '--format-version', '1',
        '--filter-platform', 'x86_64-pc-windows-msvc'
    )
    if (-not $AllowNetwork) {
        $metadataArguments += '--offline'
    }
    $metadataText = & cargo @metadataArguments | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
$metadata = $metadataText | ConvertFrom-Json -Depth 100
$rootPackage = @(
    $metadata.packages |
        Where-Object { [string]$_.name -ceq 'latentdeck-codec-pack-installer' }
)
if ($rootPackage.Count -ne 1) {
    throw 'Cargo metadata did not contain exactly one Codec Pack installer package.'
}
$artifactLicense = [string]$rootPackage[0].license
if ([string]::IsNullOrWhiteSpace($artifactLicense)) {
    throw 'Codec Pack installer package metadata has no reviewed license value.'
}

$cargoDependencyScopes = Get-CargoDependencyScopeMap `
    -Metadata $metadata `
    -RootPackageId ([string]$rootPackage[0].id)

function ConvertTo-SafePurlName {
    param([Parameter(Mandatory)][string]$Name)

    return [System.Uri]::EscapeDataString($Name).Replace('%2F', '/').Replace('%2f', '/')
}

$components = [System.Collections.Generic.List[object]]::new()
foreach ($package in @($metadata.packages | Sort-Object name, version)) {
    if (-not $cargoDependencyScopes.ContainsKey([string]$package.id)) {
        continue
    }
    $name = [string]$package.name
    $version = [string]$package.version
    $dependencyScope = Get-DependencyScopeValue `
        -Scopes $cargoDependencyScopes[[string]$package.id] `
        -Context "$name@$version"
    $component = [ordered]@{
        type = if ($name -ceq 'latentdeck-codec-pack-installer') { 'application' } else { 'library' }
        'bom-ref' = "rust:$name@$version"
        name = $name
        version = $version
        purl = "pkg:cargo/$(ConvertTo-SafePurlName $name)@$version"
        properties = @(
            [ordered]@{ name = 'latentdeck:ecosystem'; value = 'rust' }
            [ordered]@{
                name = 'latentdeck:source-kind'
                value = if ($null -eq $package.source) { 'workspace' } else { 'registry' }
            }
            [ordered]@{ name = 'latentdeck:dependency-scope'; value = $dependencyScope }
        )
    }
    if (-not [string]::IsNullOrWhiteSpace([string]$package.license)) {
        $component.licenses = @(
            [pscustomobject][ordered]@{
                license = [pscustomobject][ordered]@{ name = [string]$package.license }
            }
        )
    }
    $components.Add([pscustomobject]$component)
}
$components.Add([pscustomobject][ordered]@{
    type = 'application'
    'bom-ref' = 'tool:nsis@3.11'
    name = 'Nullsoft Scriptable Install System'
    version = '3.11'
    hashes = @(
        [ordered]@{ alg = 'SHA-256'; content = $makeNsisSha256 }
    )
    licenses = @(
        [pscustomobject][ordered]@{
            license = [pscustomobject][ordered]@{
                name = 'NSIS bundled licenses; see INSTALLER_NSIS_COPYING.txt'
            }
        }
    )
    externalReferences = @(
        [ordered]@{ type = 'website'; url = 'https://nsis.sourceforge.io/' }
    )
    properties = @(
        [ordered]@{ name = 'latentdeck:ecosystem'; value = 'installer-toolchain' }
        [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'build' }
    )
})

$sortedComponents = @($components | Sort-Object -Property @{ Expression = { $_.'bom-ref' } })
$duplicateReferences = @(
    $sortedComponents |
        Group-Object -Property 'bom-ref' |
        Where-Object Count -gt 1
)
if ($duplicateReferences.Count -gt 0) {
    throw 'Generated Codec Pack setup SBOM contains duplicate component references.'
}
$missingLicenseComponents = @(
    $sortedComponents |
        Where-Object {
            $licenseProperty = $_.PSObject.Properties['licenses']
            if ($null -eq $licenseProperty -or @($licenseProperty.Value).Count -eq 0) {
                return $true
            }
            $usable = @(
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
            )
            return $usable.Count -eq 0
        } |
        ForEach-Object { [string]$_.'bom-ref' }
)
if ($missingLicenseComponents.Count -gt 0) {
    throw (
        'Codec Pack setup SBOM cannot be distributed because locked metadata lacks a ' +
        'reviewed license value for: ' + ($missingLicenseComponents -join ', ')
    )
}
$allowedDependencyScopes = @('artifact', 'runtime', 'build', 'runtime+build')
foreach ($component in $sortedComponents) {
    $componentScopes = @($component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:dependency-scope'
    })
    if ($componentScopes.Count -ne 1 -or
        [string]$componentScopes[0].value -cnotin $allowedDependencyScopes) {
        throw "Codec Pack setup SBOM component has no exact dependency scope: $($component.'bom-ref')"
    }
}
$serialUuid = New-DeterministicUuid -Seed (
    "latentdeck-h3-installer-sbom`0$PackVersion`0$cargoLockSha256`0$nsisTreeSha256"
)
$deterministicTimestamp = [System.DateTimeOffset]::FromUnixTimeSeconds(
    $sourceDateEpoch
).UtcDateTime.ToString(
    'yyyy-MM-ddTHH:mm:ssZ',
    [System.Globalization.CultureInfo]::InvariantCulture
)

$bom = [ordered]@{
    bomFormat = 'CycloneDX'
    specVersion = '1.5'
    serialNumber = "urn:uuid:$serialUuid"
    version = 1
    metadata = [ordered]@{
        timestamp = $deterministicTimestamp
        tools = @(
            [ordered]@{
                vendor = 'LatentDeck'
                name = 'tools/New-H3CodecPackInstallerSbom.ps1'
                version = '0.1.0'
            }
        )
        component = [ordered]@{
            type = 'application'
            'bom-ref' = "pkg:generic/latentdeck-h3-codec-pack-setup@$PackVersion"
            name = 'LatentDeck H3 Codec Pack Setup'
            version = $PackVersion
            licenses = @(
                [pscustomobject][ordered]@{
                    license = [pscustomobject][ordered]@{ name = $artifactLicense }
                }
            )
            properties = @(
                [ordered]@{ name = 'latentdeck:install-scope'; value = 'current-user' }
                [ordered]@{ name = 'latentdeck:payload-delivery'; value = 'adjacent-hash-bound-ldcodec' }
                [ordered]@{ name = 'latentdeck:artifact-scope'; value = 'h3-codec-pack-setup' }
                [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                [ordered]@{
                    name = 'latentdeck:included-dependency-scopes'
                    value = 'artifact,runtime,build,runtime+build'
                }
                [ordered]@{ name = 'latentdeck:excluded-dependency-scopes'; value = 'development' }
                [ordered]@{ name = 'latentdeck:target-platform'; value = 'x86_64-pc-windows-msvc' }
            )
        }
        properties = @(
            [ordered]@{ name = 'latentdeck:cargo-lock-sha256'; value = $cargoLockSha256 }
            [ordered]@{ name = 'latentdeck:nsis-tree-sha256'; value = $nsisTreeSha256 }
            [ordered]@{ name = 'latentdeck:source-date-epoch'; value = [string]$sourceDateEpoch }
        )
    }
    components = $sortedComponents
}

$json = $bom | ConvertTo-Json -Depth 100
foreach ($forbidden in @(
    $repositoryRoot,
    $repositoryRoot.Replace('\', '\\'),
    $env:USERPROFILE,
    $env:USERPROFILE.Replace('\', '\\')
)) {
    if (-not [string]::IsNullOrWhiteSpace($forbidden) -and
        $json.IndexOf($forbidden, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        throw 'Generated Codec Pack setup SBOM contains a machine-local absolute path.'
    }
}
if ($json -match '(?i)(?<![A-Za-z])[A-Z]:[\\/]' -or
    $json -match '(?m)(?<![\\])\\\\[^\\\s]+\\' -or
    $json -match '(?i)file:/{2,3}' -or
    $json -match '(?i)/(?:Users|home)/[^/\s]+/') {
    throw 'Generated Codec Pack setup SBOM contains a machine-local filesystem reference.'
}

$partialPath = "$outputFullPath.partial-$([guid]::NewGuid().ToString('N'))"
try {
    [System.IO.File]::WriteAllText(
        $partialPath,
        $json + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $roundTrip = Get-Content -Raw -LiteralPath $partialPath | ConvertFrom-Json -Depth 100
    if ($roundTrip.bomFormat -cne 'CycloneDX' -or
        $roundTrip.specVersion -cne '1.5' -or
        @($roundTrip.components).Count -lt 2 -or
        @($roundTrip.components | Where-Object {
            $null -eq $_.PSObject.Properties['licenses'] -or @($_.licenses).Count -eq 0
        }).Count -ne 0 -or
        @($roundTrip.metadata.component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope' -and
            [string]$_.value -ceq 'artifact'
        }).Count -ne 1 -or
        @($roundTrip.components | Where-Object {
            @($_.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -cin @('artifact', 'runtime', 'build', 'runtime+build')
            }).Count -ne 1
        }).Count -ne 0 -or
        @($roundTrip.metadata.component.licenses).Count -ne 1 -or
        @($roundTrip.components | Where-Object { $_.'bom-ref' -ceq 'tool:nsis@3.11' }).Count -ne 1 -or
        @($roundTrip.components | Where-Object { $_.name -ceq 'latentdeck-codec-pack-installer' }).Count -ne 1 -or
        @($roundTrip.components | Where-Object { $_.name -ceq 'libc' }).Count -ne 0) {
        throw 'Generated Codec Pack setup SBOM failed its structural self-check.'
    }
    [System.IO.File]::Move($partialPath, $outputFullPath, $false)
} finally {
    if (Test-Path -LiteralPath $partialPath -PathType Leaf) {
        [System.IO.File]::Delete($partialPath)
    }
}

$hash = (Get-FileHash -LiteralPath $outputFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
[pscustomobject]@{
    Path = $outputFullPath
    Components = @($sortedComponents).Count
    LicenseReview = 'complete'
    MissingLicenseComponents = @()
    Sha256 = $hash
}
