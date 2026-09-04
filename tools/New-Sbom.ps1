[CmdletBinding()]
param(
    [string]$OutputPath,

    [string]$ArtifactName = 'LatentDeck',

    [string]$ArtifactVersion = '0.1.0',

    [string]$ArtifactLicense = 'Apache-2.0',

    [ValidateSet('workspace', 'application', 'developer-kit', 'h3-native', 'comfy-recorder')]
    [string]$ArtifactScope = 'workspace',

    [string[]]$CargoPackage,

    [string]$NodePackage,

    [string[]]$NodeBuildPackage,

    [string[]]$NodeRuntimeBuildPackage,

    [string[]]$PythonPackage,

    [string[]]$PythonBuildPackage,

    [switch]$IncludePythonWorkspace,

    [switch]$IncludeSpout2,

    [switch]$IncludeTauriWindowsInstaller,

    [string]$TauriNsisRoot,

    [switch]$Deterministic
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'ReleaseSpoutMetadata.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$defaultWorkspaceInventory = (
    -not $PSBoundParameters.ContainsKey('CargoPackage') -and
    -not $PSBoundParameters.ContainsKey('NodePackage') -and
    -not $PSBoundParameters.ContainsKey('PythonPackage') -and
    -not $PSBoundParameters.ContainsKey('PythonBuildPackage') -and
    -not $PSBoundParameters.ContainsKey('IncludePythonWorkspace') -and
    -not $PSBoundParameters.ContainsKey('IncludeSpout2') -and
    -not $PSBoundParameters.ContainsKey('IncludeTauriWindowsInstaller')
)
if ($IncludeTauriWindowsInstaller -and
    [string]::IsNullOrWhiteSpace($TauriNsisRoot)) {
    throw 'IncludeTauriWindowsInstaller requires an explicit verified TauriNsisRoot.'
}
if (-not $IncludeTauriWindowsInstaller -and
    -not [string]::IsNullOrWhiteSpace($TauriNsisRoot)) {
    throw 'TauriNsisRoot cannot be supplied without IncludeTauriWindowsInstaller.'
}
$cargoSelectors = @($CargoPackage | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$pythonSelectors = @($PythonPackage | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$pythonBuildSelectors = @(
    $PythonBuildPackage | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)
$nodeBuildSelectors = @($NodeBuildPackage | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$nodeRuntimeBuildSelectors = @(
    $NodeRuntimeBuildPackage | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
)
if (-not [string]::IsNullOrWhiteSpace($NodePackage) -and $nodeBuildSelectors.Count -eq 0) {
    throw 'An application SBOM with Node runtime dependencies must declare its build-tool roots.'
}
if ([string]::IsNullOrWhiteSpace($NodePackage) -and $nodeBuildSelectors.Count -gt 0) {
    throw 'NodeBuildPackage cannot be supplied without NodePackage.'
}
foreach ($selector in $nodeRuntimeBuildSelectors) {
    if ($selector -cnotin $nodeBuildSelectors) {
        throw "NodeRuntimeBuildPackage must also be listed in NodeBuildPackage: $selector"
    }
}
if ([string]::IsNullOrWhiteSpace($ArtifactName) -or $ArtifactName.Length -gt 128 -or
    [string]::IsNullOrWhiteSpace($ArtifactVersion) -or $ArtifactVersion.Length -gt 64 -or
    [string]::IsNullOrWhiteSpace($ArtifactLicense) -or $ArtifactLicense.Length -gt 128) {
    throw 'SBOM artifact name, version, and reviewed license must be bounded non-empty text.'
}
if ($IncludeTauriWindowsInstaller.IsPresent -and $ArtifactScope -cne 'application') {
    throw 'The Tauri Windows installer closure is valid only for an application artifact SBOM.'
}
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $repositoryRoot "artifacts/release/latentdeck-$ArtifactVersion-sbom.cdx.json"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath = Join-Path $repositoryRoot $OutputPath
}
$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputDirectory = [System.IO.Path]::GetDirectoryName($outputFullPath)
if ([string]::IsNullOrWhiteSpace($outputDirectory)) {
    throw 'SBOM output must have a parent directory.'
}
if (Test-Path -LiteralPath $outputFullPath) {
    throw "Refusing to overwrite an existing SBOM: $outputFullPath"
}
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
$partialPath = "$outputFullPath.partial-$([guid]::NewGuid().ToString('N'))"
$pythonPath = Join-Path $outputDirectory ".python-sbom-$([guid]::NewGuid().ToString('N')).json"

function Invoke-JsonCommand {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Command,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $text = & $Command | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
    try {
        return $text | ConvertFrom-Json -Depth 100
    } catch {
        throw "$Label did not return valid JSON: $($_.Exception.Message)"
    }
}

function New-LicenseList {
    param([object]$License)

    if ($null -eq $License -or [string]::IsNullOrWhiteSpace([string]$License)) {
        return @()
    }
    return @([ordered]@{ license = [ordered]@{ name = [string]$License } })
}

function ConvertTo-SafePurlName {
    param([Parameter(Mandatory)][string]$Name)

    return [System.Uri]::EscapeDataString($Name).Replace('%2F', '/').Replace('%2f', '/')
}

function Get-SelectedPythonProjectLicenses {
    param([Parameter(Mandatory)][string[]]$Names)

    $wanted = [System.Collections.Generic.HashSet[string]]::new(
        $Names,
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $result = [System.Collections.Generic.Dictionary[string, string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $manifestPaths = @(
        & git -C $repositoryRoot -c core.quotepath=false ls-files -- 'pyproject.toml' '*/pyproject.toml'
    )
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not enumerate Python project manifests for SBOM license metadata.'
    }
    foreach ($manifestRelative in $manifestPaths) {
        $text = Get-Content -LiteralPath (Join-Path $repositoryRoot $manifestRelative) -Raw
        $nameMatches = [regex]::Matches($text, '(?m)^name\s*=\s*"(?<value>[^"]+)"\s*$')
        if ($nameMatches.Count -ne 1) {
            continue
        }
        $name = $nameMatches[0].Groups['value'].Value
        if (-not $wanted.Contains($name)) {
            continue
        }
        $licenseMatches = [regex]::Matches($text, '(?m)^license\s*=\s*"(?<value>[^"]+)"\s*$')
        if ($licenseMatches.Count -ne 1 -or
            [string]::IsNullOrWhiteSpace($licenseMatches[0].Groups['value'].Value) -or
            $result.ContainsKey($name)) {
            throw "Selected Python project must declare one unambiguous license string: $name"
        }
        $result.Add($name, $licenseMatches[0].Groups['value'].Value)
    }
    if ($wanted.Contains('safetensors') -and -not $result.ContainsKey('safetensors')) {
        $lockPath = Join-Path `
            $repositoryRoot `
            'comfy/latent-cartridge/packaging/windows-x64.lock.json'
        $lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json -Depth 32
        if ([int]$lock.schema_version -ne 1 -or
            [string]$lock.safetensors.name -cne 'safetensors' -or
            [string]$lock.safetensors.version -cne '0.8.0' -or
            [string]$lock.safetensors.license -cne 'Apache-2.0' -or
            [string]$lock.safetensors.sha256 -cnotmatch '^[0-9a-f]{64}$') {
            throw 'Reviewed external Python package lock identity is invalid: safetensors@0.8.0'
        }
        $result.Add('safetensors', [string]$lock.safetensors.license)
    }
    foreach ($name in $Names) {
        if (-not $result.ContainsKey($name)) {
            throw "Could not resolve selected Python project license metadata: $name"
        }
    }
    return ,$result
}

function New-DeterministicUuid {
    param([Parameter(Mandatory)][string]$Seed)

    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $digest = $hasher.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($Seed))
    } finally {
        $hasher.Dispose()
    }
    $bytes = [byte[]]$digest[0..15]
    $bytes[7] = ($bytes[7] -band 0x0F) -bor 0x50
    $bytes[8] = ($bytes[8] -band 0x3F) -bor 0x80
    return [guid]::new($bytes)
}

function Get-TauriWindowsInstallerComponents {
    $expectedNsisVersion = '3.11'
    $expectedNsisTreeSha256 = 'e9ddbf15e780350628b8e9e334b770bfbb59004f2d6b5c2c43ce764d9530e063'
    $expectedNsisCopyingSha256 = 'e7dd514003ab96cb3ddccbc028fe5c795fccf57dc41f21cfb9d4dd16ead23bf5'
    $expectedTauriUtilsVersion = '0.5.3'
    $expectedTauriUtilsSha1 = '75197fee3c6a814fe035788d1c34ead39349b860'
    $expectedTauriUtilsSha256 = '5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709'

    $root = (Resolve-Path -LiteralPath $TauriNsisRoot).Path
    $rootItem = Get-Item -LiteralPath $root -Force
    if (-not $rootItem.PSIsContainer -or
        ($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'The Tauri NSIS root must be a regular directory, not a reparse point.'
    }
    $treeItems = @(Get-ChildItem -LiteralPath $root -Force -Recurse)
    if (@($treeItems | Where-Object {
        ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
    }).Count -gt 0) {
        throw 'The Tauri NSIS tree cannot contain reparse points.'
    }
    $files = @($treeItems | Where-Object { -not $_.PSIsContainer })
    if ($files.Count -ne 442) {
        throw "The pinned Tauri NSIS tree file count drifted: $($files.Count)."
    }
    $treeRecords = @(
        foreach ($file in @($files | Sort-Object {
            $_.FullName.Substring($root.Length + 1).Replace('\', '/')
        } -CaseSensitive)) {
            $relative = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
            $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            "file`0$relative`0$($file.Length)`0$hash"
        }
    )
    $treeBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($treeRecords -join "`n")
    $treeSha256 = [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($treeBytes)
    ).ToLowerInvariant()
    if ($treeSha256 -cne $expectedNsisTreeSha256) {
        throw 'The pinned Tauri NSIS tree SHA-256 drifted.'
    }

    $makeNsis = Join-Path $root 'makensis.exe'
    $nsisVersion = (& $makeNsis /VERSION).Trim().TrimStart('v')
    if ($LASTEXITCODE -ne 0 -or $nsisVersion -cne $expectedNsisVersion) {
        throw "Expected Tauri NSIS $expectedNsisVersion, found '$nsisVersion'."
    }
    $copyingPath = Join-Path $root 'COPYING'
    if ((Get-FileHash -LiteralPath $copyingPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
        $expectedNsisCopyingSha256) {
        throw 'The pinned NSIS COPYING text SHA-256 drifted.'
    }
    $tauriUtilsPath = Join-Path $root 'Plugins/x86-unicode/additional/nsis_tauri_utils.dll'
    $tauriUtilsItem = Get-Item -LiteralPath $tauriUtilsPath -Force
    if ($tauriUtilsItem.PSIsContainer -or $tauriUtilsItem.Length -ne 34304 -or
        (Get-FileHash -LiteralPath $tauriUtilsPath -Algorithm SHA1).Hash.ToLowerInvariant() -cne
            $expectedTauriUtilsSha1 -or
        (Get-FileHash -LiteralPath $tauriUtilsPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            $expectedTauriUtilsSha256) {
        throw 'The pinned nsis_tauri_utils payload identity drifted.'
    }

    return @(
        [pscustomobject][ordered]@{
            type = 'framework'
            'bom-ref' = "tool:nsis@$expectedNsisVersion"
            name = 'NSIS'
            version = $expectedNsisVersion
            purl = "pkg:generic/nsis@$expectedNsisVersion"
            licenses = @(New-LicenseList 'NSIS bundled licenses; see bundled COPYING')
            externalReferences = @([ordered]@{
                type = 'distribution'
                url = 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip'
            })
            properties = @(
                [ordered]@{ name = 'latentdeck:ecosystem'; value = 'windows-installer' }
                [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'runtime+build' }
                [ordered]@{ name = 'latentdeck:selection-root'; value = 'true' }
                [ordered]@{ name = 'latentdeck:source-commit'; value = '7359413009afd4f0fff472d841fc2f2cc0e0a5f8' }
                [ordered]@{ name = 'latentdeck:source-archive-sha1'; value = 'ef7ff767e5cbd9edd22add3a32c9b8f4500bb10d' }
                [ordered]@{ name = 'latentdeck:source-tree-sha256'; value = $treeSha256 }
                [ordered]@{ name = 'latentdeck:source-tree-file-count'; value = [string]$files.Count }
                [ordered]@{ name = 'latentdeck:license-text-sha256'; value = $expectedNsisCopyingSha256 }
            )
        }
        [pscustomobject][ordered]@{
            type = 'library'
            'bom-ref' = "native:nsis-tauri-utils@$expectedTauriUtilsVersion"
            name = 'nsis-tauri-utils'
            version = $expectedTauriUtilsVersion
            purl = "pkg:generic/nsis-tauri-utils@$expectedTauriUtilsVersion"
            hashes = @(
                [ordered]@{ alg = 'SHA-1'; content = $expectedTauriUtilsSha1 }
                [ordered]@{ alg = 'SHA-256'; content = $expectedTauriUtilsSha256 }
            )
            licenses = @(New-LicenseList 'Apache-2.0 OR MIT')
            externalReferences = @([ordered]@{
                type = 'distribution'
                url = 'https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll'
            })
            properties = @(
                [ordered]@{ name = 'latentdeck:ecosystem'; value = 'windows-installer' }
                [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'runtime+build' }
                [ordered]@{ name = 'latentdeck:selection-root'; value = 'true' }
                [ordered]@{ name = 'latentdeck:source-commit'; value = '13d9edd27b69310e108d6fbd49f90992f8a05390' }
            )
        }
    )
}

function Get-CargoDependencyClassifications {
    param(
        [Parameter(Mandatory)]
        [object]$Metadata,

        [Parameter(Mandatory)]
        [string[]]$RootNames
    )

    $packageIdsByName = @{}
    foreach ($package in @($Metadata.packages)) {
        $name = [string]$package.name
        if (-not $packageIdsByName.ContainsKey($name)) {
            $packageIdsByName[$name] = [System.Collections.Generic.List[string]]::new()
        }
        $packageIdsByName[$name].Add([string]$package.id)
    }
    $nodesById = @{}
    foreach ($node in @($Metadata.resolve.nodes)) {
        $nodesById[[string]$node.id] = $node
    }
    $rootIds = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $classifications = @{}
    $pending = [System.Collections.Generic.Queue[object]]::new()
    foreach ($rootName in $RootNames) {
        if (-not $packageIdsByName.ContainsKey($rootName) -or
            $packageIdsByName[$rootName].Count -ne 1) {
            throw "SBOM Cargo selector must resolve exactly one package: $rootName"
        }
        $rootId = $packageIdsByName[$rootName][0]
        [void]$rootIds.Add($rootId)
        if (-not $classifications.ContainsKey($rootId)) {
            $classifications[$rootId] = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::Ordinal
            )
        }
        [void]$classifications[$rootId].Add('artifact')
        $pending.Enqueue([pscustomobject]@{ Id = $rootId; TraversalScope = 'runtime' })
    }
    $visitedStates = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    while ($pending.Count -gt 0) {
        $state = $pending.Dequeue()
        $packageId = [string]$state.Id
        $traversalScope = [string]$state.TraversalScope
        if ($traversalScope -notin @('runtime', 'build')) {
            throw "SBOM Cargo dependency traversal produced an unknown scope: $traversalScope"
        }
        if (-not $visitedStates.Add("$packageId`0$traversalScope")) {
            continue
        }
        if (-not $nodesById.ContainsKey($packageId)) {
            throw "SBOM Cargo metadata has no resolve node for package: $packageId"
        }
        foreach ($dependency in @($nodesById[$packageId].deps)) {
            $dependencyId = [string]$dependency.pkg
            $dependencyKinds = @($dependency.dep_kinds)
            if ($dependencyKinds.Count -eq 0) {
                throw "SBOM Cargo dependency has no dependency kind: $packageId -> $dependencyId"
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
                    throw "SBOM Cargo dependency has an unsupported dependency kind: $kind"
                }
                if (-not $classifications.ContainsKey($dependencyId)) {
                    $classifications[$dependencyId] =
                        [System.Collections.Generic.HashSet[string]]::new(
                            [System.StringComparer]::Ordinal
                        )
                }
                if (-not $rootIds.Contains($dependencyId)) {
                    [void]$classifications[$dependencyId].Add($dependencyScope)
                }
                $pending.Enqueue([pscustomobject]@{
                    Id = $dependencyId
                    TraversalScope = $dependencyScope
                })
            }
        }
    }
    return ,([pscustomobject]@{
        Classifications = $classifications
        RootIds = $rootIds
    })
}

function Get-DependencyScopeValue {
    param(
        [Parameter(Mandatory)][System.Collections.Generic.HashSet[string]]$Scopes,
        [Parameter(Mandatory)][string]$Context
    )

    $values = @($Scopes | Sort-Object)
    if (($values -join ',') -ceq 'artifact') {
        return 'artifact'
    }
    if (($values -join ',') -ceq 'runtime') {
        return 'runtime'
    }
    if (($values -join ',') -ceq 'build') {
        return 'build'
    }
    if (($values -join ',') -ceq 'build,runtime') {
        return 'runtime+build'
    }
    throw "SBOM dependency scope is empty or unsupported for ${Context}: $($values -join ',')"
}

function Convert-PnpmLicenseInventoryToRecords {
    param([Parameter(Mandatory)][object]$Inventory)

    $records = [System.Collections.Generic.List[object]]::new()
    foreach ($licenseGroup in $Inventory.PSObject.Properties) {
        foreach ($package in @($licenseGroup.Value)) {
            foreach ($versionValue in @($package.versions)) {
                $name = [string]$package.name
                $version = [string]$versionValue
                if ([string]::IsNullOrWhiteSpace($name) -or
                    [string]::IsNullOrWhiteSpace($version)) {
                    throw 'pnpm license inventory contains an empty package identity.'
                }
                $records.Add([pscustomobject]@{
                    Identity = "$name@$version"
                    Name = $name
                    Version = $version
                    License = [string]$package.license
                })
            }
        }
    }
    return $records.ToArray()
}

function Get-NodeBuildDependencyIdentities {
    param(
        [Parameter(Mandatory)][string]$NodeExecutable,
        [Parameter(Mandatory)][string]$PnpmExecutable,
        [Parameter(Mandatory)][string[]]$RootNames
    )

    $rootList = Invoke-JsonCommand -Label 'pnpm direct development dependency inventory' -Command {
        & $PnpmExecutable list --json --depth 0 --dev
    }
    $workspaceRoots = @($rootList | Where-Object { [string]$_.name -ceq 'latentdeck-workspace' })
    if ($workspaceRoots.Count -ne 1) {
        throw 'pnpm did not expose one latentdeck-workspace development root.'
    }
    $rootPaths = [System.Collections.Generic.List[string]]::new()
    foreach ($rootName in $RootNames) {
        $property = $workspaceRoots[0].devDependencies.PSObject.Properties[$rootName]
        if ($null -eq $property -or
            [string]::IsNullOrWhiteSpace([string]$property.Value.path)) {
            throw "Node build-tool root is not a direct locked workspace dependency: $rootName"
        }
        $rootPaths.Add([System.IO.Path]::GetFullPath([string]$property.Value.path))
    }
    $walker = @'
const fs = require('fs');
const path = require('path');
const { createRequire } = require('module');

function packageRoot(start, expectedName) {
  let current = fs.statSync(start).isDirectory() ? start : path.dirname(start);
  while (true) {
    const manifestPath = path.join(current, 'package.json');
    if (fs.existsSync(manifestPath)) {
      const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
      if (manifest.name === expectedName) return current;
    }
    const parent = path.dirname(current);
    if (parent === current) throw new Error(`cannot resolve package root for ${expectedName}`);
    current = parent;
  }
}

function dependencyRoot(parentRoot, name, optional) {
  let dependencyBase = path.dirname(parentRoot);
  while (path.basename(dependencyBase).toLowerCase() !== 'node_modules') {
    const next = path.dirname(dependencyBase);
    if (next === dependencyBase) throw new Error(`cannot locate dependency base for ${parentRoot}`);
    dependencyBase = next;
  }
  const sibling = path.join(dependencyBase, ...name.split('/'));
  const siblingManifest = path.join(sibling, 'package.json');
  if (fs.existsSync(siblingManifest)) {
    const manifest = JSON.parse(fs.readFileSync(siblingManifest, 'utf8'));
    if (manifest.name !== name) throw new Error(`dependency identity mismatch for ${name}`);
    return sibling;
  }
  const resolver = createRequire(path.join(parentRoot, 'package.json'));
  try {
    return packageRoot(resolver.resolve(name), name);
  } catch (error) {
    if (optional) return null;
    throw error;
  }
}

const queue = process.argv.slice(1);
const visitedRoots = new Set();
const records = new Map();
while (queue.length > 0) {
  const root = fs.realpathSync(path.resolve(queue.shift()));
  const key = root.toLowerCase();
  if (visitedRoots.has(key)) continue;
  visitedRoots.add(key);
  const manifestPath = path.join(root, 'package.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  if (!manifest.name || !manifest.version) throw new Error(`package identity missing at ${root}`);
  if (!manifest.license || typeof manifest.license !== 'string') {
    throw new Error(`package license missing for ${manifest.name}@${manifest.version}`);
  }
  const identity = `${manifest.name}@${manifest.version}`;
  const prior = records.get(identity);
  if (prior && prior.license !== manifest.license) {
    throw new Error(`package license disagreement for ${identity}`);
  }
  records.set(identity, {
    identity,
    name: manifest.name,
    version: manifest.version,
    license: manifest.license
  });
  const required = manifest.dependencies || {};
  const optional = manifest.optionalDependencies || {};
  for (const name of [...new Set([...Object.keys(required), ...Object.keys(optional)])].sort()) {
    const resolved = dependencyRoot(
      root,
      name,
      Object.prototype.hasOwnProperty.call(optional, name)
    );
    if (resolved !== null) queue.push(resolved);
  }
}
process.stdout.write(JSON.stringify([...records.values()].sort((a, b) => a.identity.localeCompare(b.identity))));
'@
    $identityText = & $NodeExecutable -e $walker @($rootPaths.ToArray()) | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "Node build dependency traversal failed with exit code $LASTEXITCODE"
    }
    $records = @($identityText | ConvertFrom-Json -Depth 8)
    if ($records.Count -lt $RootNames.Count -or
        @($records | Where-Object {
            [string]$_.identity -cnotmatch '^.+@[^@]+$' -or
            [string]::IsNullOrWhiteSpace([string]$_.name) -or
            [string]::IsNullOrWhiteSpace([string]$_.version) -or
            [string]::IsNullOrWhiteSpace([string]$_.license)
        }).Count -gt 0) {
        throw 'Node build dependency traversal returned an invalid or incomplete identity set.'
    }
    return @($records | Sort-Object identity -CaseSensitive)
}

$components = [System.Collections.Generic.List[object]]::new()
$excludedNodeDevelopmentCount = 0

try {
    Push-Location $repositoryRoot
    try {
        $cargoMetadataArguments = @('metadata', '--locked', '--format-version', '1')
        if ($ArtifactScope -in @('application', 'developer-kit', 'h3-native', 'comfy-recorder')) {
            $cargoMetadataArguments += @('--filter-platform', 'x86_64-pc-windows-msvc')
        }
        $cargo = Invoke-JsonCommand -Label 'cargo metadata' -Command {
            cargo @cargoMetadataArguments
        }
        $cargoDependencyClassifications = $null
        if ($cargoSelectors.Count -gt 0) {
            $cargoDependencyClassifications = Get-CargoDependencyClassifications `
                -Metadata $cargo `
                -RootNames $cargoSelectors
        }
        foreach ($package in $cargo.packages) {
            $packageId = [string]$package.id
            if ($null -ne $cargoDependencyClassifications -and
                -not $cargoDependencyClassifications.Classifications.ContainsKey($packageId)) {
                continue
            }
            $name = [string]$package.name
            $version = [string]$package.version
            $dependencyScope = if ($null -eq $cargoDependencyClassifications) {
                'development'
            } else {
                Get-DependencyScopeValue `
                    -Scopes $cargoDependencyClassifications.Classifications[$packageId] `
                    -Context "Cargo package $name@$version"
            }
            $component = [ordered]@{
                type = 'library'
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
            if ($cargoSelectors -contains $name) {
                $component.properties += @(
                    [ordered]@{ name = 'latentdeck:selection-root'; value = 'true' }
                )
            }
            $licenses = @(New-LicenseList $package.license)
            if ($licenses.Count -gt 0) {
                $component.licenses = $licenses
            }
            $components.Add([pscustomobject]$component)
        }

        $nodeRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNode.ps1')
        $node = Join-Path $nodeRoot 'node.exe'
        $pnpm = Join-Path $nodeRoot 'pnpm.cmd'
        if ($defaultWorkspaceInventory -or -not [string]::IsNullOrWhiteSpace($NodePackage)) {
            $pnpmRuntimeLicenses = Invoke-JsonCommand -Label 'pnpm runtime license inventory' -Command {
                if ([string]::IsNullOrWhiteSpace($NodePackage)) {
                    & $pnpm licenses list --json --long
                } else {
                    & $pnpm --filter $NodePackage licenses list --prod --json --long
                }
            }
            $nodeRecords = @{}
            foreach ($record in @(Convert-PnpmLicenseInventoryToRecords $pnpmRuntimeLicenses)) {
                $scopes = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
                $initialNodeScope = if ($defaultWorkspaceInventory) {
                    'development'
                } else {
                    'runtime'
                }
                [void]$scopes.Add($initialNodeScope)
                $nodeRecords[$record.Identity] = [pscustomobject]@{ Record = $record; Scopes = $scopes }
            }
            if (-not [string]::IsNullOrWhiteSpace($NodePackage)) {
                $nodeBuildRecords = @(Get-NodeBuildDependencyIdentities `
                    -NodeExecutable $node `
                    -PnpmExecutable $pnpm `
                    -RootNames $nodeBuildSelectors)
                $buildIdentityValues = @($nodeBuildRecords | ForEach-Object { [string]$_.identity })
                $runtimeBuildIdentityValues = if ($nodeRuntimeBuildSelectors.Count -gt 0) {
                    @(
                        Get-NodeBuildDependencyIdentities `
                            -NodeExecutable $node `
                            -PnpmExecutable $pnpm `
                            -RootNames $nodeRuntimeBuildSelectors |
                            ForEach-Object { [string]$_.identity }
                    )
                } else {
                    @()
                }
                $runtimeBuildIdentities = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
                foreach ($identity in $runtimeBuildIdentityValues) {
                    [void]$runtimeBuildIdentities.Add([string]$identity)
                }
                $buildIdentities = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
                foreach ($identity in $buildIdentityValues) {
                    [void]$buildIdentities.Add([string]$identity)
                }
                $pnpmDevelopmentLicenses = Invoke-JsonCommand `
                    -Label 'pnpm build/development license inventory' `
                    -Command { & $pnpm licenses list --dev --json --long }
                $developmentRecords = @(Convert-PnpmLicenseInventoryToRecords $pnpmDevelopmentLicenses)
                $developmentIdentities = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
                foreach ($record in $developmentRecords) {
                    [void]$developmentIdentities.Add([string]$record.Identity)
                    if (-not $buildIdentities.Contains([string]$record.Identity)) {
                        continue
                    }
                    if ($nodeRecords.ContainsKey([string]$record.Identity)) {
                        if ([string]$nodeRecords[$record.Identity].Record.License -cne [string]$record.License) {
                            throw "pnpm runtime/build license metadata disagrees for $($record.Identity)"
                        }
                        [void]$nodeRecords[$record.Identity].Scopes.Add('build')
                        if ($runtimeBuildIdentities.Contains([string]$record.Identity)) {
                            [void]$nodeRecords[$record.Identity].Scopes.Add('runtime')
                        }
                    } else {
                        $scopes = [System.Collections.Generic.HashSet[string]]::new(
                            [System.StringComparer]::Ordinal
                        )
                        [void]$scopes.Add('build')
                        if ($runtimeBuildIdentities.Contains([string]$record.Identity)) {
                            [void]$scopes.Add('runtime')
                        }
                        $nodeRecords[$record.Identity] = [pscustomobject]@{
                            Record = $record
                            Scopes = $scopes
                        }
                    }
                }
                foreach ($buildRecord in $nodeBuildRecords) {
                    $identity = [string]$buildRecord.identity
                    if ($developmentIdentities.Contains($identity)) {
                        continue
                    }
                    if ($nodeRecords.ContainsKey($identity)) {
                        if ([string]$nodeRecords[$identity].Record.License -cne
                            [string]$buildRecord.license) {
                            throw "Node runtime/build manifest license metadata disagrees for $identity"
                        }
                        [void]$nodeRecords[$identity].Scopes.Add('build')
                        if ($runtimeBuildIdentities.Contains($identity)) {
                            [void]$nodeRecords[$identity].Scopes.Add('runtime')
                        }
                        continue
                    }
                    $scopes = [System.Collections.Generic.HashSet[string]]::new(
                        [System.StringComparer]::Ordinal
                    )
                    [void]$scopes.Add('build')
                    if ($runtimeBuildIdentities.Contains($identity)) {
                        [void]$scopes.Add('runtime')
                    }
                    $nodeRecords[$identity] = [pscustomobject]@{
                        Record = [pscustomobject]@{
                            Identity = $identity
                            Name = [string]$buildRecord.name
                            Version = [string]$buildRecord.version
                            License = [string]$buildRecord.license
                        }
                        Scopes = $scopes
                    }
                }
                $runtimeIdentities = [System.Collections.Generic.HashSet[string]]::new(
                    [System.StringComparer]::Ordinal
                )
                foreach ($entry in $nodeRecords.GetEnumerator()) {
                    if ($entry.Value.Scopes.Contains('runtime')) {
                        [void]$runtimeIdentities.Add([string]$entry.Key)
                    }
                }
                $excludedNodeDevelopmentCount = @(
                    $developmentIdentities | Where-Object {
                        -not $buildIdentities.Contains([string]$_) -and
                        -not $runtimeIdentities.Contains([string]$_)
                    }
                ).Count
            }
            foreach ($entry in @($nodeRecords.GetEnumerator() | Sort-Object Key)) {
                $record = $entry.Value.Record
                $scopeValues = @($entry.Value.Scopes | Sort-Object)
                $dependencyScope = if (($scopeValues -join ',') -ceq 'build,runtime') {
                    'runtime+build'
                } elseif ($scopeValues.Count -eq 1 -and
                    $scopeValues[0] -in @('runtime', 'build', 'development')) {
                    [string]$scopeValues[0]
                } else {
                    throw "Node dependency has an unsupported scope combination: $($record.Identity)"
                }
                $components.Add([pscustomobject][ordered]@{
                    type = 'library'
                    'bom-ref' = "node:$($record.Identity)"
                    name = [string]$record.Name
                    version = [string]$record.Version
                    purl = "pkg:npm/$(ConvertTo-SafePurlName $record.Name)@$($record.Version)"
                    licenses = @(New-LicenseList $record.License)
                    properties = @(
                        [ordered]@{ name = 'latentdeck:ecosystem'; value = 'node' }
                        [ordered]@{ name = 'latentdeck:dependency-scope'; value = $dependencyScope }
                    )
                })
            }
        }

        if ($defaultWorkspaceInventory -or $IncludePythonWorkspace.IsPresent -or
            $pythonSelectors.Count -gt 0) {
            $selectedPythonLicenses = if ($pythonSelectors.Count -gt 0) {
                Get-SelectedPythonProjectLicenses -Names $pythonSelectors
            } else {
                $null
            }
            uv export --format cyclonedx1.5 --all-packages --all-extras --locked `
                --preview-features sbom-export --output-file $pythonPath | Out-Null
            if ($LASTEXITCODE -ne 0) {
                throw "uv CycloneDX export failed with exit code $LASTEXITCODE"
            }
            $pythonBom = Get-Content -Raw -LiteralPath $pythonPath | ConvertFrom-Json -Depth 100
            $selectedPythonNames = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::OrdinalIgnoreCase
            )
            foreach ($name in $pythonSelectors) {
                [void]$selectedPythonNames.Add($name)
            }
            foreach ($package in @($pythonBom.components)) {
                if (-not $defaultWorkspaceInventory -and
                    -not $IncludePythonWorkspace.IsPresent -and
                    -not $selectedPythonNames.Contains([string]$package.name)) {
                    continue
                }
                if ($selectedPythonNames.Contains([string]$package.name) -and
                    [string]$package.name -ceq 'safetensors' -and
                    [string]$package.version -cne '0.8.0') {
                    throw 'Selected external Python package version drifted from its reviewed lock: safetensors'
                }
                $originalReference = [string]$package.'bom-ref'
                $package.'bom-ref' = "python:$originalReference"
                $licenseField = $package.PSObject.Properties['licenses']
                if (($null -eq $licenseField -or @($licenseField.Value).Count -eq 0) -and
                    $null -ne $selectedPythonLicenses -and
                    $selectedPythonLicenses.ContainsKey([string]$package.name)) {
                    $package | Add-Member `
                        -NotePropertyName licenses `
                        -NotePropertyValue @(New-LicenseList $selectedPythonLicenses[[string]$package.name]) `
                        -Force
                }
                $propertyField = $package.PSObject.Properties['properties']
                $properties = [System.Collections.Generic.List[object]]::new()
                if ($null -ne $propertyField) {
                    foreach ($property in @($propertyField.Value)) {
                        $properties.Add($property)
                    }
                }
                $properties.Add([pscustomobject]@{
                    name = 'latentdeck:ecosystem'
                    value = 'python'
                })
                $properties.Add([pscustomobject]@{
                    name = 'latentdeck:dependency-scope'
                    value = if ($selectedPythonNames.Contains([string]$package.name)) {
                        'artifact'
                    } else {
                        'development'
                    }
                })
                if ($selectedPythonNames.Contains([string]$package.name)) {
                    $properties.Add([pscustomobject]@{
                        name = 'latentdeck:selection-root'
                        value = 'true'
                    })
                }
                $propertyArray = $properties.ToArray()
                if ($null -eq $propertyField) {
                    $package | Add-Member -NotePropertyName properties -NotePropertyValue $propertyArray
                } else {
                    $propertyField.Value = $propertyArray
                }
                $components.Add($package)
            }
        }
        if ($pythonBuildSelectors.Count -gt 0) {
            $reviewedPythonBuildTools = @{
                'uv-build==0.12.7' = [ordered]@{
                    Name = 'uv-build'
                    Version = '0.12.7'
                    License = 'MIT OR Apache-2.0'
                    Url = 'https://pypi.org/project/uv-build/0.12.7/'
                }
                'maturin==1.15.0' = [ordered]@{
                    Name = 'maturin'
                    Version = '1.15.0'
                    License = 'MIT OR Apache-2.0'
                    Url = 'https://pypi.org/project/maturin/1.15.0/'
                }
            }
            foreach ($selector in @($pythonBuildSelectors | Sort-Object -CaseSensitive -Unique)) {
                if (-not $reviewedPythonBuildTools.ContainsKey($selector)) {
                    throw "Python build package is not an exact reviewed identity: $selector"
                }
                $tool = $reviewedPythonBuildTools[$selector]
                $components.Add([pscustomobject][ordered]@{
                    type = 'application'
                    'bom-ref' = "python-build:$($tool.Name)@$($tool.Version)"
                    name = [string]$tool.Name
                    version = [string]$tool.Version
                    purl = "pkg:pypi/$($tool.Name)@$($tool.Version)"
                    licenses = @(New-LicenseList ([string]$tool.License))
                    externalReferences = @([ordered]@{
                        type = 'distribution'
                        url = [string]$tool.Url
                    })
                    properties = @(
                        [ordered]@{ name = 'latentdeck:ecosystem'; value = 'python' }
                        [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'build' }
                        [ordered]@{ name = 'latentdeck:selection-root'; value = 'true' }
                    )
                })
            }
        }
    }
    finally {
        Pop-Location
    }

    if ($defaultWorkspaceInventory -or $IncludeSpout2.IsPresent) {
        $spoutComponent = New-Spout2CycloneDxComponent
        $spoutProperties = [System.Collections.Generic.List[object]]::new()
        foreach ($property in @($spoutComponent.properties)) {
            $spoutProperties.Add($property)
        }
        $spoutProperties.Add([pscustomobject]@{
            name = 'latentdeck:dependency-scope'
            value = if ($defaultWorkspaceInventory) { 'development' } else { 'runtime' }
        })
        $spoutComponent.properties = $spoutProperties.ToArray()
        $components.Add($spoutComponent)
    }
    if ($IncludeTauriWindowsInstaller.IsPresent) {
        foreach ($installerComponent in @(Get-TauriWindowsInstallerComponents)) {
            $components.Add($installerComponent)
        }
    }
    $sortedComponents = @($components | Sort-Object -Property @{ Expression = { $_.'bom-ref' } })
    $duplicateReferences = @(
        $sortedComponents |
            Group-Object -Property 'bom-ref' |
            Where-Object Count -gt 1
    )
    if ($duplicateReferences.Count -gt 0) {
        throw 'Generated SBOM contains duplicate component references.'
    }
    $allowedDependencyScopes = @('artifact', 'runtime', 'build', 'runtime+build', 'development')
    foreach ($component in $sortedComponents) {
        $scopeProperties = @(
            $component.properties |
                Where-Object { [string]$_.name -ceq 'latentdeck:dependency-scope' }
        )
        if ($scopeProperties.Count -ne 1 -or
            [string]$scopeProperties[0].value -cnotin $allowedDependencyScopes) {
            throw "Generated SBOM component has no exact supported dependency scope: $($component.'bom-ref')"
        }
        if ($ArtifactScope -in @('application', 'developer-kit', 'h3-native', 'comfy-recorder') -and
            [string]$scopeProperties[0].value -ceq 'development') {
            throw "Distributable SBOM unexpectedly contains a development dependency: $($component.'bom-ref')"
        }
        $licenseField = $component.PSObject.Properties['licenses']
        if ($ArtifactScope -in @('application', 'developer-kit', 'h3-native', 'comfy-recorder') -and
            ($null -eq $licenseField -or @($licenseField.Value).Count -eq 0)) {
            throw "Distributable SBOM component has no reviewed license metadata: $($component.'bom-ref')"
        }
    }
    $componentIdentity = @($sortedComponents | ForEach-Object { [string]$_.'bom-ref' }) -join "`n"
    $serial = if ($Deterministic) {
        New-DeterministicUuid -Seed "$ArtifactName`0$ArtifactVersion`0$ArtifactScope`0$componentIdentity"
    } else {
        [guid]::NewGuid()
    }
    $timestamp = if ($Deterministic) {
        '1980-01-01T00:00:00.0000000+00:00'
    } else {
        [DateTimeOffset]::UtcNow.ToString('o')
    }
    $bom = [ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        serialNumber = "urn:uuid:$serial"
        version = 1
        metadata = [ordered]@{
            timestamp = $timestamp
            tools = @(
                [ordered]@{
                    vendor = 'LatentDeck'
                    name = 'tools/New-Sbom.ps1'
                    version = '0.1.0'
                }
            )
            component = [ordered]@{
                type = 'application'
                'bom-ref' = "pkg:generic/$(ConvertTo-SafePurlName $ArtifactName)@$ArtifactVersion"
                name = $ArtifactName
                version = $ArtifactVersion
                licenses = @(
                    [ordered]@{ license = [ordered]@{ name = $ArtifactLicense } }
                )
                properties = @(
                    [ordered]@{ name = 'latentdeck:artifact-scope'; value = $ArtifactScope }
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                    [ordered]@{
                        name = 'latentdeck:included-dependency-scopes'
                        value = if ($ArtifactScope -in @('application', 'developer-kit', 'h3-native', 'comfy-recorder')) {
                            'artifact,runtime,build,runtime+build'
                        } else {
                            'artifact,development'
                        }
                    }
                    [ordered]@{
                        name = 'latentdeck:excluded-dependency-scopes'
                        value = if ($ArtifactScope -in @('application', 'developer-kit', 'h3-native', 'comfy-recorder')) {
                            'development'
                        } else {
                            'none'
                        }
                    }
                    [ordered]@{
                        name = 'latentdeck:target-platform'
                        value = if ($ArtifactScope -in @('application', 'developer-kit', 'h3-native', 'comfy-recorder')) {
                            'x86_64-pc-windows-msvc'
                        } else {
                            'all-host-resolved'
                        }
                    }
                    [ordered]@{
                        name = 'latentdeck:excluded-node-development-component-count'
                        value = [string]$excludedNodeDevelopmentCount
                    }
                    if ($IncludeTauriWindowsInstaller.IsPresent) {
                        [ordered]@{
                            name = 'latentdeck:webview2-bootstrapper-disposition'
                            value = 'not_redistributed_install_time_download'
                        }
                        [ordered]@{
                            name = 'latentdeck:webview2-install-mode'
                            value = 'downloadBootstrapper'
                        }
                    }
                )
            }
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
            throw 'Generated SBOM contains a machine-local absolute path.'
        }
    }
    if ($json -match '(?i)(?<![A-Za-z])[A-Z]:[\\/]' -or
        $json -match '(?m)(?<![\\])\\\\[^\\\s]+\\' -or
        $json -match '(?i)file:/{2,3}' -or
        $json -match '(?i)/(?:Users|home)/[^/\s]+/' -or
        $json -match '(?i)node_modules') {
        throw 'Generated SBOM contains a machine-local package or filesystem reference.'
    }
    [System.IO.File]::WriteAllText(
        $partialPath,
        $json + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $roundTrip = Get-Content -Raw -LiteralPath $partialPath | ConvertFrom-Json -Depth 100
    if ($roundTrip.bomFormat -cne 'CycloneDX' -or
        $roundTrip.specVersion -cne '1.5' -or
        @($roundTrip.metadata.component.licenses).Count -ne 1 -or
        @($roundTrip.components).Count -lt 1) {
        throw 'Generated SBOM failed its structural self-check.'
    }
    if ($defaultWorkspaceInventory -or $IncludeSpout2.IsPresent) {
        Assert-Spout2CycloneDxComponent -Components @($roundTrip.components) | Out-Null
    }
    [System.IO.File]::Move($partialPath, $outputFullPath, $false)
    $hash = (Get-FileHash -LiteralPath $outputFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "SBOM: $outputFullPath"
    Write-Host "Components: $(@($roundTrip.components).Count)"
    Write-Host "SHA-256: $hash"
    [pscustomobject]@{
        Path = $outputFullPath
        Components = @($roundTrip.components).Count
        Sha256 = $hash
    }
}
finally {
    foreach ($temporaryPath in @($partialPath, $pythonPath)) {
        if (Test-Path -LiteralPath $temporaryPath) {
            Remove-Item -LiteralPath $temporaryPath -Force
        }
    }
}
