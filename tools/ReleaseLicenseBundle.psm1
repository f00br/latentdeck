Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force

function Get-LowerSha256Bytes {
    param([Parameter(Mandatory)][byte[]]$Bytes)

    return [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($Bytes)
    ).ToLowerInvariant()
}

function Get-LowerSha256File {
    param([Parameter(Mandatory)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-OrdinalSortedUniqueStrings {
    param([Parameter(Mandatory)][object[]]$Value)

    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($entry in $Value) {
        if (-not $seen.Add([string]$entry)) {
            continue
        }
    }
    [string[]]$result = @($seen)
    [System.Array]::Sort($result, [System.StringComparer]::Ordinal)
    return $result
}

function Merge-ReleaseDependencyScope {
    param(
        [Parameter(Mandatory)][string]$Current,
        [Parameter(Mandatory)][string]$Next
    )

    if ($Current -ceq $Next) {
        return $Current
    }
    if ($Current -ceq 'runtime+build' -or $Next -ceq 'runtime+build' -or
        (($Current -ceq 'build') -xor ($Next -ceq 'build'))) {
        return 'runtime+build'
    }
    if ($Current -ceq 'build' -and $Next -ceq 'build') {
        return 'build'
    }
    # A component selected as an artifact root in one product and a runtime
    # dependency in another is redistributed by both products.
    return 'runtime'
}

function Get-ComponentLicenseExpression {
    param([Parameter(Mandatory)][object]$Component)

    $labels = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in @($Component.licenses)) {
        if ($null -ne $entry.PSObject.Properties['expression'] -and
            -not [string]::IsNullOrWhiteSpace([string]$entry.expression)) {
            $labels.Add([string]$entry.expression)
            continue
        }
        if ($null -ne $entry.PSObject.Properties['license']) {
            foreach ($field in @('id', 'name')) {
                $property = $entry.license.PSObject.Properties[$field]
                if ($null -ne $property -and
                    -not [string]::IsNullOrWhiteSpace([string]$property.Value)) {
                    $labels.Add([string]$property.Value)
                    break
                }
            }
        }
    }
    $values = @($labels | Sort-Object -CaseSensitive -Unique)
    if ($values.Count -eq 0) {
        throw "License bundle component has no license expression: $($Component.name)@$($Component.version)"
    }
    return $values -join ' OR '
}

function Get-ExactPropertyValue {
    param(
        [Parameter(Mandatory)][object]$Component,
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Context
    )

    $matches = @($Component.properties | Where-Object { [string]$_.name -ceq $Name })
    if ($matches.Count -ne 1 -or [string]::IsNullOrWhiteSpace([string]$matches[0].value)) {
        throw "$Context must declare exactly one $Name property."
    }
    return [string]$matches[0].value
}

function Assert-ReleaseComponentIdentity {
    param(
        [Parameter(Mandatory)][object]$Component,
        [Parameter(Mandatory)][string]$Ecosystem,
        [Parameter(Mandatory)][string]$Scope,
        [Parameter(Mandatory)][bool]$IsRoot,
        [Parameter(Mandatory)][string]$Context
    )

    if ($Ecosystem -cnotin @(
            'artifact', 'rust', 'python', 'node', 'native-cpp', 'windows-installer'
        ) -or $Scope -cnotin @('artifact', 'runtime', 'build', 'runtime+build')) {
        throw "$Context refuses a non-canonical ecosystem or scope."
    }
    if (($IsRoot -and ($Ecosystem -cne 'artifact' -or $Scope -cne 'artifact')) -or
        (-not $IsRoot -and $Ecosystem -ceq 'artifact')) {
        throw "$Context has an invalid root/component ecosystem boundary."
    }
    $bomRef = [string]$Component.'bom-ref'
    $name = [string]$Component.name
    $version = [string]$Component.version
    if ([string]::IsNullOrWhiteSpace($bomRef) -or
        [string]::IsNullOrWhiteSpace($name) -or
        [string]::IsNullOrWhiteSpace($version) -or
        $bomRef.IndexOf([char]0) -ge 0 -or
        $name.IndexOf([char]0) -ge 0 -or
        $version.IndexOf([char]0) -ge 0) {
        throw "$Context has an empty or invalid component identity."
    }

    $validReference = switch ($Ecosystem) {
        'artifact' {
            $Scope -ceq 'artifact' -and
            $bomRef.StartsWith('pkg:generic/', [System.StringComparison]::Ordinal) -and
            $bomRef.EndsWith("@$version", [System.StringComparison]::Ordinal)
        }
        'rust' {
            $bomRef -ceq "rust:$name@$version" -or
            $bomRef -ceq "rust:safetensors-native:$name@$version"
        }
        'python' {
            if ($Scope -ceq 'build') {
                $bomRef -ceq "python-build:$name@$version"
            } else {
                $bomRef.StartsWith('python:', [System.StringComparison]::Ordinal) -and
                $bomRef.EndsWith("@$version", [System.StringComparison]::Ordinal)
            }
        }
        'node' { $bomRef -ceq "node:$name@$version" }
        'native-cpp' {
            $name -ceq 'Spout2' -and
            $bomRef -cmatch "^native:spout2@$([regex]::Escape($version))\+[0-9a-f]{40}$"
        }
        'windows-installer' {
            ($name -ceq 'NSIS' -and $bomRef -ceq "tool:nsis@$version") -or
            ($name -ceq 'nsis-tauri-utils' -and
                $bomRef -ceq "native:nsis-tauri-utils@$version")
        }
        default { $false }
    }
    if (-not $validReference) {
        throw "$Context has a non-canonical bom-ref for $Ecosystem/$name@$version."
    }
}

function ConvertTo-CanonicalLicenseText {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Context
    )

    Assert-PathComponentsNotReparsePoints -Path $Path
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or $item.Length -gt 4MB) {
        throw "$Context must be a bounded regular non-reparse text file."
    }
    $bytes = [System.IO.File]::ReadAllBytes($item.FullName)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "$Context is not strict UTF-8."
    }
    if ($text.IndexOf([char]0) -ge 0) {
        throw "$Context contains a NUL byte."
    }
    $text = $text.TrimStart([char]0xFEFF).Replace("`r`n", "`n").Replace("`r", "`n").TrimEnd("`n") + "`n"
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw "$Context has no license or notice text."
    }
    $canonicalBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($text)
    return [pscustomobject]@{
        Text = $text
        ByteLength = [int64]$canonicalBytes.Length
        Sha256 = Get-LowerSha256Bytes -Bytes $canonicalBytes
        RawSha256 = Get-LowerSha256Bytes -Bytes $bytes
    }
}

function Get-LicenseCandidateFiles {
    param([Parameter(Mandatory)][string]$PackageRoot)

    Assert-PathComponentsNotReparsePoints -Path $PackageRoot
    return @(
        Get-ChildItem -LiteralPath $PackageRoot -File -Force |
            Where-Object {
                $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT|UNLICENSE)(?:$|[._-])' -and
                $_.Extension -cne '.spdx'
            } |
            Sort-Object Name
    )
}

function Normalize-RepositoryUrl {
    param([string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ''
    }
    $normalized = $Value.Trim()
    $normalized = $normalized -replace '^git\+', ''
    $normalized = $normalized -replace '^git://github\.com/', 'https://github.com/'
    $normalized = $normalized -replace '\.git/?$', ''
    return $normalized.TrimEnd('/')
}

function Get-NodeManifestRepositoryUrl {
    param([Parameter(Mandatory)][object]$Manifest)

    $repository = $Manifest.PSObject.Properties['repository']
    if ($null -eq $repository) {
        return ''
    }
    if ($repository.Value -is [string]) {
        return Normalize-RepositoryUrl -Value ([string]$repository.Value)
    }
    if ($null -ne $repository.Value.PSObject.Properties['url']) {
        return Normalize-RepositoryUrl -Value ([string]$repository.Value.url)
    }
    return ''
}

function Get-ReviewedNodeFallbacks {
    return @{
        '@rolldown/binding-win32-x64-msvc@1.2.6' = [ordered]@{
            Repository = 'https://github.com/rolldown/rolldown'
            SourceCommit = '5375362b36eeeaf514c67052ba65f3e97523dde5'
            LicenseExpression = 'MIT'
            PackageJsonSha256 = 'e6a992378cbbbb4a76743f9837f344875452f3d9ec75213f3af0801b909a9ae1'
            RegistryIntegrity = 'sha512-np8iZSLfXlAD4kWhiyq/u0Yt8oZDtRQ8lGhQaCXo2rl37KNjeU0GjJuwr4P3oeZ++ROfofsKNBqR5LTO8aXyWQ=='
            File = [ordered]@{
                Path = 'tools/installer/licenses/rolldown-1.2.6-LICENSE.txt'
                Sha256 = '9d69b72f845ebdbe1bba70814960127ceeb08bc4d1645b509cca104396c9da8c'
                Url = 'https://raw.githubusercontent.com/rolldown/rolldown/5375362b36eeeaf514c67052ba65f3e97523dde5/LICENSE'
                UpstreamSha256 = '23ecfff35a5a2e80d92142f75228912c3b1abc4b5a8337a821ff4397e2f9f734'
            }
        }
        'is-reference@3.0.3' = [ordered]@{
            Repository = 'https://github.com/Rich-Harris/is-reference'
            SourceCommit = '8bb053129bfabe2f6a7d7ed050159d67ebe82829'
            LicenseExpression = 'MIT'
            PackageJsonSha256 = '5ab4c14eb3e5726b0704c624ee6649e8026dc8a14471e7cdc4c7d55699162ec7'
            RegistryIntegrity = 'sha512-ixkJoqQvAP88E6wLydLGGqCJsrFUnqoH6HnaczB8XmDH1oaWU+xxdptvikTgaEhtZ53Ky6YXiBuUI2WXLMCwjw=='
            File = [ordered]@{
                Path = 'tools/installer/licenses/rich-harris-node-MIT.txt'
                Sha256 = '82838dbc68b885c3c92029dbabb59b87ceb3bfc3532ff8182961d4998aa7ec02'
                Url = 'reviewed-from-exact-package-license-and-author-metadata'
                UpstreamSha256 = ''
            }
        }
        'locate-character@3.0.0' = [ordered]@{
            Repository = 'https://gitlab.com/Rich-Harris/locate-character'
            SourceCommit = ''
            LicenseExpression = 'MIT'
            PackageJsonSha256 = 'b6a378f56e34ba7323a3c7a9e7e265acb49dbd616b1219477ca90fc0fc21b16d'
            RegistryIntegrity = 'sha512-SW13ws7BjaeJ6p7Q6CO2nchbYEc3X3J6WrmTTDto7yMPqVSZTUyY5Tjbid+Ab8gLnATtygYtiDIJGQRRn2ZOiA=='
            File = [ordered]@{
                Path = 'tools/installer/licenses/rich-harris-node-MIT.txt'
                Sha256 = '82838dbc68b885c3c92029dbabb59b87ceb3bfc3532ff8182961d4998aa7ec02'
                Url = 'reviewed-from-exact-package-license-and-author-metadata'
                UpstreamSha256 = ''
            }
        }
    }
}

function Get-ReviewedCargoFallbacks {
    $apacheDefmt = [ordered]@{
        Path = 'tools/installer/licenses/defmt-4a8cdb44-LICENSE-APACHE.txt'
        Sha256 = '8173d5c29b4f956d532781d2b86e4e30f83e6b7878dce18c919451d6ba707c90'
        Url = 'https://raw.githubusercontent.com/knurling-rs/defmt/4a8cdb44891ed57b8ff5a023b6bec7137c48708f/LICENSE-APACHE'
    }
    $mitDefmt = [ordered]@{
        Path = 'tools/installer/licenses/defmt-4a8cdb44-LICENSE-MIT.txt'
        Sha256 = '0d17b75c1867fd568bcbb735f329d0d4253846c4b756a65e4d440c1e4bd59187'
        Url = 'https://raw.githubusercontent.com/knurling-rs/defmt/4a8cdb44891ed57b8ff5a023b6bec7137c48708f/LICENSE-MIT'
    }
    $apacheProfiling = [ordered]@{
        Path = 'tools/installer/licenses/profiling-82715511-LICENSE-APACHE.txt'
        Sha256 = '6ee32466aaa724095c99d108c8e549fb9e2bd2c5d81708afb1cd373b3177f7d2'
        Url = 'https://raw.githubusercontent.com/aclysma/profiling/8271551172eb6fa4cba47369aedd93790c623df9/LICENSE-APACHE'
    }
    $mitProfiling = [ordered]@{
        Path = 'tools/installer/licenses/profiling-82715511-LICENSE-MIT.txt'
        Sha256 = 'e64891b51361725890d69bafa508ad013bc92aeef99054caecb6f5ce5f7dfd51'
        Url = 'https://raw.githubusercontent.com/aclysma/profiling/8271551172eb6fa4cba47369aedd93790c623df9/LICENSE-MIT'
    }
    $apacheUnic = [ordered]@{
        Path = 'tools/installer/licenses/rust-unic-58786053-LICENSE-APACHE.txt'
        Sha256 = 'a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2'
        Url = 'https://raw.githubusercontent.com/open-i18n/rust-unic/5878605364af97a3358368a6eaef02104af2e016/LICENSE-APACHE'
    }
    $mitUnic = [ordered]@{
        Path = 'tools/installer/licenses/rust-unic-58786053-LICENSE-MIT.txt'
        Sha256 = '23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3'
        Url = 'https://raw.githubusercontent.com/open-i18n/rust-unic/5878605364af97a3358368a6eaef02104af2e016/LICENSE-MIT'
    }
    $webview = [ordered]@{
        Path = 'tools/installer/licenses/webview2-rs-b74dc5e2-LICENSE.txt'
        Sha256 = '0dcf41516e608bbcb6cdc5229feb7b86fe4a643b85e7df251133c93408fdac73'
        Url = 'https://raw.githubusercontent.com/wravery/webview2-rs/b74dc5e2b394044bea5191052868ce7a106c202c/LICENSE'
    }
    $fallbacks = @{
        'alloc-stdlib@0.2.4' = [ordered]@{
            Repository = 'https://github.com/dropbox/rust-alloc-no-stdlib'
            SourceCommit = 'ae42d22078b98549e987d2f03d12df7b984fde47'
            LicenseExpression = 'BSD-3-Clause'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/alloc-no-stdlib-ae42d220-LICENSE.txt'
                Sha256 = 'c0c56f26d9c051cac4d200c34c84e7ae9aaa853e01a982a1df08b09931e518ae'
                Url = 'https://raw.githubusercontent.com/dropbox/rust-alloc-no-stdlib/ae42d22078b98549e987d2f03d12df7b984fde47/LICENSE'
            })
        }
        'defmt-parser@1.0.0' = [ordered]@{
            Repository = 'https://github.com/knurling-rs/defmt'
            SourceCommit = '4a8cdb44891ed57b8ff5a023b6bec7137c48708f'
            LicenseExpression = 'MIT OR Apache-2.0'
            Files = @($apacheDefmt, $mitDefmt)
        }
        'jsonschema-regex@0.53.0' = [ordered]@{
            Repository = 'https://github.com/Stranger6667/jsonschema'
            SourceCommit = '6af37d89619fdcb06d8ab82d02dbe6b3d1a4d1a7'
            LicenseExpression = 'MIT'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/jsonschema-0.53.0-LICENSE.txt'
                Sha256 = '117829c3ca21efb132d81a44b55363d395ab8eea18526873bc828da4c0e5f038'
                Url = 'https://raw.githubusercontent.com/Stranger6667/jsonschema/6af37d89619fdcb06d8ab82d02dbe6b3d1a4d1a7/LICENSE'
            })
        }
        'jsonschema-value@0.53.0' = [ordered]@{
            Repository = 'https://github.com/Stranger6667/jsonschema'
            SourceCommit = '6af37d89619fdcb06d8ab82d02dbe6b3d1a4d1a7'
            LicenseExpression = 'MIT'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/jsonschema-0.53.0-LICENSE.txt'
                Sha256 = '117829c3ca21efb132d81a44b55363d395ab8eea18526873bc828da4c0e5f038'
                Url = 'https://raw.githubusercontent.com/Stranger6667/jsonschema/6af37d89619fdcb06d8ab82d02dbe6b3d1a4d1a7/LICENSE'
            })
        }
        'profiling@1.0.18' = [ordered]@{
            Repository = 'https://github.com/aclysma/profiling'
            SourceCommit = '8271551172eb6fa4cba47369aedd93790c623df9'
            LicenseExpression = 'MIT OR Apache-2.0'
            Files = @($apacheProfiling, $mitProfiling)
        }
        'selectors@0.36.1' = [ordered]@{
            Repository = 'https://github.com/servo/stylo'
            SourceCommit = '635e1a19d02960588a00e189bd4bd5bdb150ec3d'
            LicenseExpression = 'MPL-2.0'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/MPL-2.0-mozilla.txt'
                Sha256 = '1f256ecad192880510e84ad60474eab7589218784b9a50bc7ceee34c2b91f1d5'
                Url = 'https://www.mozilla.org/media/MPL/2.0/index.815ca599c9df.txt'
            })
        }
        'spirv@0.4.0+sdk-1.4.341.0' = [ordered]@{
            Repository = 'https://github.com/gfx-rs/rspirv'
            SourceCommit = '8afc3d0ac8e158128cd1410bb2e4b4c26ab11bb4'
            LicenseExpression = 'Apache-2.0'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/rspirv-8afc3d0a-LICENSE.txt'
                Sha256 = 'cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30'
                Url = 'https://raw.githubusercontent.com/gfx-rs/rspirv/8afc3d0ac8e158128cd1410bb2e4b4c26ab11bb4/LICENSE'
            })
        }
        'uuid-simd@0.8.0' = [ordered]@{
            Repository = 'https://github.com/Nugine/simd'
            SourceCommit = 'd74c030d9dc4f3cae02146d1f497ff62726ef09a'
            LicenseExpression = 'MIT'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/simd-0.8.0-LICENSE.txt'
                Sha256 = '14e66de892a0e218a4d60b2cc41a17a28080c46621d812fa2471983d8c524748'
                Url = 'https://raw.githubusercontent.com/Nugine/simd/d74c030d9dc4f3cae02146d1f497ff62726ef09a/LICENSE'
            })
        }
        'vsimd@0.8.0' = [ordered]@{
            Repository = 'https://github.com/Nugine/simd'
            SourceCommit = 'd74c030d9dc4f3cae02146d1f497ff62726ef09a'
            LicenseExpression = 'MIT'
            Files = @([ordered]@{
                Path = 'tools/installer/licenses/simd-0.8.0-LICENSE.txt'
                Sha256 = '14e66de892a0e218a4d60b2cc41a17a28080c46621d812fa2471983d8c524748'
                Url = 'https://raw.githubusercontent.com/Nugine/simd/d74c030d9dc4f3cae02146d1f497ff62726ef09a/LICENSE'
            })
        }
    }
    foreach ($identity in @(
        'unic-char-property@0.9.0', 'unic-char-range@0.9.0', 'unic-common@0.9.0',
        'unic-ucd-version@0.9.0'
    )) {
        $fallbacks[$identity] = [ordered]@{
            Repository = 'https://github.com/open-i18n/rust-unic'
            SourceCommit = '5878605364af97a3358368a6eaef02104af2e016'
            LicenseExpression = 'MIT/Apache-2.0'
            Files = @($apacheUnic, $mitUnic)
        }
    }
    $fallbacks['unic-ucd-ident@0.9.0'] = [ordered]@{
        Repository = 'https://github.com/open-i18n/rust-unic'
        SourceCommit = '8a6ce83063d90b91ae2ce59eddb803edd393fca9'
        LicenseExpression = 'MIT/Apache-2.0'
        Files = @($apacheUnic, $mitUnic)
    }
    foreach ($entry in @(
        @('webview2-com@0.38.2', 'b74dc5e2b394044bea5191052868ce7a106c202c'),
        @('webview2-com-macros@0.8.1', 'dffa41a8a46d3f5565eefbff2de57d38d399f158'),
        @('webview2-com-sys@0.38.2', 'b74dc5e2b394044bea5191052868ce7a106c202c')
    )) {
        $fallbacks[$entry[0]] = [ordered]@{
            Repository = 'https://github.com/wravery/webview2-rs'
            SourceCommit = $entry[1]
            LicenseExpression = 'MIT'
            Files = @($webview)
        }
    }
    return ,$fallbacks
}

function Get-NodePackageIndex {
    param([Parameter(Mandatory)][string]$RepositoryRoot)

    $storeRoot = Join-Path $RepositoryRoot 'node_modules/.pnpm'
    Assert-PathComponentsNotReparsePoints -Path $storeRoot
    $index = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($storeEntry in @(Get-ChildItem -LiteralPath $storeRoot -Directory -Force)) {
        $nodeModules = Join-Path $storeEntry.FullName 'node_modules'
        if (-not (Test-Path -LiteralPath $nodeModules -PathType Container)) {
            continue
        }
        foreach ($first in @(Get-ChildItem -LiteralPath $nodeModules -Directory -Force)) {
            if (($first.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                continue
            }
            $roots = if ($first.Name.StartsWith('@', [System.StringComparison]::Ordinal)) {
                @(Get-ChildItem -LiteralPath $first.FullName -Directory -Force | Where-Object {
                    ($_.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0
                })
            } else {
                @($first)
            }
            foreach ($root in $roots) {
                $manifestPath = Join-Path $root.FullName 'package.json'
                if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
                    continue
                }
                $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -Depth 32
                $identity = "$($manifest.name)@$($manifest.version)"
                if ($identity -cnotmatch '^.+@[^@]+$') {
                    throw 'Installed Node package has an invalid identity.'
                }
                if (-not $index.ContainsKey($identity)) {
                    $index[$identity] = [System.Collections.Generic.List[object]]::new()
                }
                $index[$identity].Add([pscustomobject]@{
                    Root = $root.FullName
                    Manifest = $manifest
                })
            }
        }
    }
    return ,$index
}

function Get-ReleaseBuildOnlyScopePolicy {
    param([Parameter(Mandatory)][string]$RepositoryRoot)

    $path = Join-Path `
        $RepositoryRoot `
        'tools/packaging/windows-x64-release-build-only.lock.json'
    Assert-PathComponentsNotReparsePoints -Path $path
    $item = Get-Item -LiteralPath $path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or $item.Length -gt 1MB) {
        throw 'Release build-only scope lock must be a bounded regular file.'
    }
    $lock = Read-StrictJsonFile -Path $item.FullName
    Assert-ExactProperties `
        -Object $lock `
        -Required @('schema_version', 'policy', 'artifacts') `
        -Context 'release build-only scope lock'
    if ([int]$lock.schema_version -ne 1 -or
        [string]$lock.policy -cne 'exact-build-only-component-allowlist') {
        throw 'Release build-only scope lock identity is invalid.'
    }
    $byArtifact = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($artifact in @($lock.artifacts)) {
        Assert-ExactProperties `
            -Object $artifact `
            -Required @('name', 'build_only_bom_refs') `
            -Context 'release build-only artifact policy'
        $name = [string]$artifact.name
        $references = @($artifact.build_only_bom_refs | ForEach-Object { [string]$_ })
        $canonical = @(Get-OrdinalSortedUniqueStrings -Value $references)
        if ([string]::IsNullOrWhiteSpace($name) -or
            $byArtifact.ContainsKey($name) -or
            $references.Count -eq 0 -or
            ($references -join "`0") -cne ($canonical -join "`0") -or
            @($references | Where-Object {
                [string]::IsNullOrWhiteSpace($_) -or $_ -cnotmatch '^[a-z][^\x00]+@[^\x00]+$'
            }).Count -ne 0) {
            throw 'Release build-only scope lock is not canonical and unique.'
        }
        $byArtifact[$name] = $references
    }
    $expectedArtifacts = @(
        'LatentDeck App', 'LatentDeck Comfy LC Recorder', 'LatentDeck Developer Kit',
        'LatentDeck H3 Native Extensions', 'LatentPlayer'
    )
    $expectedArtifacts = @(Get-OrdinalSortedUniqueStrings -Value $expectedArtifacts)
    $actualArtifacts = @(Get-OrdinalSortedUniqueStrings -Value @($byArtifact.Keys))
    if (($actualArtifacts -join "`0") -cne ($expectedArtifacts -join "`0")) {
        throw 'Release build-only scope lock does not cover the exact release artifact set.'
    }
    return [pscustomobject]@{
        Path = $item.FullName
        Sha256 = Get-LowerSha256File -Path $item.FullName
        ByArtifact = $byArtifact
    }
}

function Assert-ReleaseBuildOnlyScope {
    param(
        [Parameter(Mandatory)][object]$Policy,
        [Parameter(Mandatory)][string]$ArtifactName,
        [Parameter(Mandatory)][object[]]$Components
    )

    if (-not $Policy.ByArtifact.ContainsKey($ArtifactName)) {
        throw "Release build-only scope lock has no reviewed artifact identity: $ArtifactName"
    }
    $actual = @(
        $Components | Where-Object {
            (Get-ExactPropertyValue `
                -Component $_ `
                -Name 'latentdeck:dependency-scope' `
                -Context "SBOM component $($_.'bom-ref')") -ceq 'build'
        } | ForEach-Object { [string]$_.'bom-ref' }
    )
    $actual = @(Get-OrdinalSortedUniqueStrings -Value $actual)
    $expected = @($Policy.ByArtifact[$ArtifactName])
    if (($actual -join "`0") -cne ($expected -join "`0") -or
        $actual.Count -ne @($Components | Where-Object {
            (Get-ExactPropertyValue `
                -Component $_ `
                -Name 'latentdeck:dependency-scope' `
                -Context "SBOM component $($_.'bom-ref')") -ceq 'build'
        }).Count) {
        throw "Release SBOM build-only component set drifted from reviewed policy: $ArtifactName"
    }
}

function Test-ReleaseLicenseBundle {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string]$BundlePath,
        [Parameter(Mandatory)][string[]]$SbomPath,
        [Parameter(Mandatory)][string]$ExpectedArtifactName,
        [Parameter(Mandatory)][string]$ExpectedArtifactVersion
    )

    $item = Get-Item -LiteralPath $BundlePath -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or $item.Length -gt 32MB) {
        throw 'Release license bundle must be a bounded regular file.'
    }
    $bundle = Get-Content -LiteralPath $item.FullName -Raw | ConvertFrom-Json -Depth 100
    Assert-ExactProperties `
        -Object $bundle `
        -Required @(
            'schema_version', 'artifact', 'policy', 'sboms', 'component_count',
            'text_count', 'components', 'texts'
        ) `
        -Context 'release license bundle'
    Assert-ExactProperties `
        -Object $bundle.policy `
        -Required @(
            'component_coverage', 'redistributed_components_require_text',
            'build_only_disposition', 'build_only_scope_lock',
            'build_only_scope_lock_sha256', 'text_canonicalization'
        ) `
        -Context 'release license bundle policy'
    if ([int]$bundle.schema_version -ne 1 -or
        [string]$bundle.artifact.name -cne $ExpectedArtifactName -or
        [string]$bundle.artifact.version -cne $ExpectedArtifactVersion) {
        throw 'Release license bundle identity is invalid.'
    }
    $repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
    $scopePolicy = Get-ReleaseBuildOnlyScopePolicy -RepositoryRoot $repositoryRoot
    if ([string]$bundle.policy.component_coverage -cne 'exact-sbom-closure' -or
        $bundle.policy.redistributed_components_require_text -isnot [bool] -or
        -not [bool]$bundle.policy.redistributed_components_require_text -or
        [string]$bundle.policy.build_only_disposition -cne
            'not_redistributed_no_text_required' -or
        [string]$bundle.policy.text_canonicalization -cne
            'strict-utf8-lf-final-newline' -or
        [string]$bundle.policy.build_only_scope_lock -cne
            'tools/packaging/windows-x64-release-build-only.lock.json' -or
        [string]$bundle.policy.build_only_scope_lock_sha256 -cne [string]$scopePolicy.Sha256) {
        throw 'Release license bundle does not bind the reviewed build-only scope lock.'
    }
    $expectedComponents = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    $expectedSboms = @(
        foreach ($path in $SbomPath) {
            $sbomItem = Get-Item -LiteralPath $path -Force
            $sbom = Get-Content -LiteralPath $sbomItem.FullName -Raw |
                ConvertFrom-Json -Depth 100
            $artifactName = [string]$sbom.metadata.component.name
            Assert-ReleaseBuildOnlyScope `
                -Policy $scopePolicy `
                -ArtifactName $artifactName `
                -Components @($sbom.components)
            foreach ($entry in @(
                [pscustomobject]@{
                    Component = $sbom.metadata.component
                    Ecosystem = 'artifact'
                    IsRoot = $true
                }
                @($sbom.components | ForEach-Object {
                    [pscustomobject]@{
                        Component = $_
                        IsRoot = $false
                        Ecosystem = Get-ExactPropertyValue `
                            -Component $_ `
                            -Name 'latentdeck:ecosystem' `
                            -Context "SBOM component $($_.'bom-ref')"
                    }
                })
            )) {
                $component = $entry.Component
                $bomRef = [string]$component.'bom-ref'
                $scope = Get-ExactPropertyValue `
                    -Component $component `
                    -Name 'latentdeck:dependency-scope' `
                    -Context "SBOM component $bomRef"
                if ([string]$entry.Ecosystem -cnotin @(
                    'artifact', 'rust', 'python', 'node', 'native-cpp', 'windows-installer'
                ) -or $scope -cnotin @('artifact', 'runtime', 'build', 'runtime+build')) {
                    throw "Release license bundle refuses a non-canonical ecosystem or scope: $bomRef"
                }
                Assert-ReleaseComponentIdentity `
                    -Component $component `
                    -Ecosystem ([string]$entry.Ecosystem) `
                    -Scope $scope `
                    -IsRoot ([bool]$entry.IsRoot) `
                    -Context "SBOM component $bomRef"
                $licenseExpression = Get-ComponentLicenseExpression -Component $component
                $identityShape = "$($component.name)`0$($component.version)`0$($entry.Ecosystem)`0$licenseExpression"
                if ($expectedComponents.ContainsKey($bomRef)) {
                    if ([string]$expectedComponents[$bomRef].IdentityShape -cne $identityShape) {
                        throw "Bound SBOMs disagree about release license component identity: $bomRef"
                    }
                    $expectedComponents[$bomRef].Scope = Merge-ReleaseDependencyScope `
                        -Current ([string]$expectedComponents[$bomRef].Scope) `
                        -Next $scope
                    [void]$expectedComponents[$bomRef].Artifacts.Add($artifactName)
                } else {
                    $artifacts = [System.Collections.Generic.HashSet[string]]::new(
                        [System.StringComparer]::Ordinal
                    )
                    [void]$artifacts.Add($artifactName)
                    $expectedComponents[$bomRef] = [pscustomobject]@{
                        Name = [string]$component.name
                        Version = [string]$component.version
                        Ecosystem = [string]$entry.Ecosystem
                        Scope = $scope
                        LicenseExpression = $licenseExpression
                        IdentityShape = $identityShape
                        Artifacts = $artifacts
                    }
                }
            }
            [pscustomobject]@{
                Name = $sbomItem.Name
                ByteLength = [int64]$sbomItem.Length
                Sha256 = Get-LowerSha256File -Path $sbomItem.FullName
            }
        }
    )
    if (@($bundle.sboms).Count -ne $expectedSboms.Count) {
        throw 'Release license bundle SBOM coverage is incomplete.'
    }
    foreach ($expected in $expectedSboms) {
        $matches = @($bundle.sboms | Where-Object {
            [string]$_.name -ceq $expected.Name -and
            [int64]$_.byte_length -eq $expected.ByteLength -and
            [string]$_.sha256 -ceq $expected.Sha256
        })
        if ($matches.Count -ne 1) {
            throw "Release license bundle is not bound to SBOM $($expected.Name)."
        }
    }
    $textsByHash = @{}
    foreach ($textRecord in @($bundle.texts)) {
        $textBytes = [System.Text.UTF8Encoding]::new($false).GetBytes([string]$textRecord.text)
        $hash = Get-LowerSha256Bytes -Bytes $textBytes
        if ($hash -cne [string]$textRecord.sha256 -or
            [int64]$textBytes.Length -ne [int64]$textRecord.byte_length -or
            $textsByHash.ContainsKey($hash)) {
            throw 'Release license bundle contains invalid or duplicate text content.'
        }
        $textsByHash[$hash] = $true
    }
    $componentKeys = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $usedTexts = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($component in @($bundle.components)) {
        $bomRef = [string]$component.'bom-ref'
        if (-not $componentKeys.Add($bomRef)) {
            throw 'Release license bundle contains a duplicate component mapping.'
        }
        if (-not $expectedComponents.ContainsKey($bomRef)) {
            throw "Release license bundle contains an unexpected component mapping: $bomRef"
        }
        $expectedComponent = $expectedComponents[$bomRef]
        $expectedArtifacts = @($expectedComponent.Artifacts | Sort-Object -CaseSensitive)
        $actualArtifacts = @($component.artifacts | Sort-Object -CaseSensitive -Unique)
        if ([string]$component.name -cne [string]$expectedComponent.Name -or
            [string]$component.version -cne [string]$expectedComponent.Version -or
            [string]$component.ecosystem -cne [string]$expectedComponent.Ecosystem -or
            [string]$component.dependency_scope -cne [string]$expectedComponent.Scope -or
            [string]$component.license_expression -cne [string]$expectedComponent.LicenseExpression -or
            ($actualArtifacts -join "`0") -cne ($expectedArtifacts -join "`0") -or
            @($component.artifacts).Count -ne $actualArtifacts.Count) {
            throw "Release license component mapping disagrees with its bound SBOM: $bomRef"
        }
        $hashes = @($component.text_sha256s)
        if ([string]$component.disposition -ceq 'license_text_in_bundle') {
            if ($hashes.Count -eq 0) {
                throw "Release license component mapping has no text: $($component.'bom-ref')"
            }
            foreach ($hash in $hashes) {
                if (-not $textsByHash.ContainsKey([string]$hash)) {
                    throw "Release license component mapping references unknown text: $($component.'bom-ref')"
                }
                [void]$usedTexts.Add([string]$hash)
            }
        } elseif ([string]$component.disposition -ceq 'not_redistributed_no_text_required') {
            if ([string]$component.dependency_scope -cne 'build' -or
                $hashes.Count -ne 0 -or
                [string]::IsNullOrWhiteSpace([string]$component.rationale)) {
                throw "Release license no-text disposition is invalid: $($component.'bom-ref')"
            }
        } else {
            throw "Release license component has an unknown disposition: $($component.'bom-ref')"
        }
    }
    if ($componentKeys.Count -ne $expectedComponents.Count -or
        $componentKeys.Count -ne [int]$bundle.component_count -or
        $textsByHash.Count -ne [int]$bundle.text_count -or
        $usedTexts.Count -ne $textsByHash.Count) {
        throw 'Release license bundle mapping/text closure is incomplete or contains extras.'
    }
    return [pscustomobject]@{
        Path = $item.FullName
        ByteLength = [int64]$item.Length
        Sha256 = Get-LowerSha256File -Path $item.FullName
        ComponentCount = $componentKeys.Count
        TextCount = $textsByHash.Count
        NoTextDispositionCount = @($bundle.components | Where-Object {
            [string]$_.disposition -ceq 'not_redistributed_no_text_required'
        }).Count
    }
}

function New-ReleaseLicenseBundle {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)][string[]]$SbomPath,
        [Parameter(Mandatory)][string]$ArtifactName,
        [Parameter(Mandatory)][string]$ArtifactVersion,
        [Parameter(Mandatory)][string]$OutputPath,
        [Parameter(Mandatory)][string]$RepositoryNoticePath,
        [string]$SafetensorsWheelPath,
        [string]$SafetensorsDistributionRoot,
        [string]$TauriNsisRoot
    )

    $repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
    $scopePolicy = Get-ReleaseBuildOnlyScopePolicy -RepositoryRoot $repositoryRoot
    $outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
    if (Test-Path -LiteralPath $outputFullPath) {
        throw "Refusing to overwrite an existing release license bundle: $outputFullPath"
    }
    $outputDirectory = [System.IO.Path]::GetDirectoryName($outputFullPath)
    [System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
    Assert-PathComponentsNotReparsePoints -Path $outputDirectory

    $repoLicensePath = Join-Path $repositoryRoot 'LICENSE'
    $repoLicense = ConvertTo-CanonicalLicenseText `
        -Path $repoLicensePath `
        -Context 'repository Apache-2.0 license'
    $repositoryNotice = Get-Content -LiteralPath $RepositoryNoticePath -Raw
    $spoutStart = $repositoryNotice.IndexOf('## Spout2', [System.StringComparison]::Ordinal)
    if ($spoutStart -lt 0) {
        throw 'Release license bundle could not resolve the reviewed Spout2 notice text.'
    }
    $spoutTextValue = $repositoryNotice.Substring($spoutStart).Replace("`r`n", "`n").Replace("`r", "`n").TrimEnd("`n") + "`n"
    $spoutBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($spoutTextValue)
    $spoutText = [pscustomobject]@{
        Text = $spoutTextValue
        ByteLength = [int64]$spoutBytes.Length
        Sha256 = Get-LowerSha256Bytes -Bytes $spoutBytes
        RawSha256 = Get-LowerSha256File -Path $RepositoryNoticePath
    }

    $sbomRecords = [System.Collections.Generic.List[object]]::new()
    $componentIndex = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($path in $SbomPath) {
        $resolved = (Resolve-Path -LiteralPath $path).Path
        Assert-PathComponentsNotReparsePoints -Path $resolved
        $sbomItem = Get-Item -LiteralPath $resolved -Force
        $sbom = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json -Depth 100
        $artifact = [string]$sbom.metadata.component.name
        Assert-ReleaseBuildOnlyScope `
            -Policy $scopePolicy `
            -ArtifactName $artifact `
            -Components @($sbom.components)
        $sbomRecords.Add([pscustomobject][ordered]@{
            name = $sbomItem.Name
            artifact = $artifact
            byte_length = [int64]$sbomItem.Length
            sha256 = Get-LowerSha256File -Path $resolved
        })
        foreach ($componentAndEcosystem in @(
            [pscustomobject]@{
                Component = $sbom.metadata.component
                Ecosystem = 'artifact'
                IsRoot = $true
            }
            @($sbom.components | ForEach-Object {
                [pscustomobject]@{
                    Component = $_
                    IsRoot = $false
                    Ecosystem = Get-ExactPropertyValue `
                        -Component $_ `
                        -Name 'latentdeck:ecosystem' `
                        -Context "SBOM component $($_.'bom-ref')"
                }
            })
        )) {
            $component = $componentAndEcosystem.Component
            $bomRef = [string]$component.'bom-ref'
            $scope = Get-ExactPropertyValue `
                -Component $component `
                -Name 'latentdeck:dependency-scope' `
                -Context "SBOM component $bomRef"
            if ([string]$componentAndEcosystem.Ecosystem -cnotin @(
                    'artifact', 'rust', 'python', 'node', 'native-cpp', 'windows-installer'
                ) -or $scope -cnotin @('artifact', 'runtime', 'build', 'runtime+build')) {
                throw "Release license bundle refuses a non-canonical ecosystem or scope: $bomRef"
            }
            Assert-ReleaseComponentIdentity `
                -Component $component `
                -Ecosystem ([string]$componentAndEcosystem.Ecosystem) `
                -Scope $scope `
                -IsRoot ([bool]$componentAndEcosystem.IsRoot) `
                -Context "SBOM component $bomRef"
            $licenseExpression = Get-ComponentLicenseExpression -Component $component
            $identityShape = "$($component.name)`0$($component.version)`0$($componentAndEcosystem.Ecosystem)`0$licenseExpression"
            if ($componentIndex.ContainsKey($bomRef)) {
                if ([string]$componentIndex[$bomRef].IdentityShape -cne $identityShape) {
                    throw "Release SBOMs disagree about component identity: $bomRef"
                }
                $componentIndex[$bomRef].Scope = Merge-ReleaseDependencyScope `
                    -Current ([string]$componentIndex[$bomRef].Scope) `
                    -Next $scope
                [void]$componentIndex[$bomRef].Artifacts.Add($artifact)
                continue
            }
            $artifacts = [System.Collections.Generic.HashSet[string]]::new(
                [System.StringComparer]::Ordinal
            )
            [void]$artifacts.Add($artifact)
            $componentIndex[$bomRef] = [pscustomobject]@{
                Component = $component
                Ecosystem = [string]$componentAndEcosystem.Ecosystem
                Scope = $scope
                LicenseExpression = $licenseExpression
                IdentityShape = $identityShape
                Artifacts = $artifacts
            }
        }
    }
    if ($componentIndex.Count -eq 0 -or $componentIndex.Count -gt 100000) {
        throw 'Release license bundle component closure is empty or unbounded.'
    }

    Push-Location $repositoryRoot
    try {
        $cargoMetadata = cargo metadata --locked --format-version 1 `
            --filter-platform x86_64-pc-windows-msvc | Out-String | ConvertFrom-Json -Depth 100
        if ($LASTEXITCODE -ne 0) {
            throw 'Cargo metadata failed while resolving release license texts.'
        }
    } finally {
        Pop-Location
    }
    $cargoByIdentity = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($package in @($cargoMetadata.packages)) {
        $identity = "$($package.name)@$($package.version)"
        if (-not $cargoByIdentity.ContainsKey($identity)) {
            $cargoByIdentity[$identity] = [System.Collections.Generic.List[object]]::new()
        }
        $cargoByIdentity[$identity].Add($package)
    }
    $nodeIndex = Get-NodePackageIndex -RepositoryRoot $repositoryRoot
    $pythonByIdentity = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($manifestRelative in @(& git -C $repositoryRoot -c core.quotepath=false ls-files -- 'pyproject.toml' '*/pyproject.toml')) {
        $manifestPath = Join-Path $repositoryRoot $manifestRelative
        $manifestText = Get-Content -LiteralPath $manifestPath -Raw
        $nameMatch = [regex]::Match($manifestText, '(?m)^name\s*=\s*"(?<v>[^"]+)"\s*$')
        $versionMatch = [regex]::Match($manifestText, '(?m)^version\s*=\s*"(?<v>[^"]+)"\s*$')
        $licenseMatch = [regex]::Match($manifestText, '(?m)^license\s*=\s*"(?<v>[^"]+)"\s*$')
        if ($nameMatch.Success -and $versionMatch.Success -and $licenseMatch.Success) {
            $pythonByIdentity["$($nameMatch.Groups['v'].Value)@$($versionMatch.Groups['v'].Value)"] = [pscustomobject]@{
                License = $licenseMatch.Groups['v'].Value
            }
        }
    }
    $cargoFallbacks = Get-ReviewedCargoFallbacks
    $nodeFallbacks = Get-ReviewedNodeFallbacks
    $pnpmLockText = Get-Content -LiteralPath (Join-Path $repositoryRoot 'pnpm-lock.yaml') -Raw
    if (-not [string]::IsNullOrWhiteSpace($SafetensorsWheelPath) -and
        -not [string]::IsNullOrWhiteSpace($SafetensorsDistributionRoot)) {
        throw 'Release license bundle accepts either a Safetensors wheel or installed tree, not both.'
    }
    $safetensorsClosure = $null
    if (-not [string]::IsNullOrWhiteSpace($SafetensorsWheelPath)) {
        $safetensorsClosure = Read-SafetensorsNativeClosure -WheelPath $SafetensorsWheelPath
    } elseif (-not [string]::IsNullOrWhiteSpace($SafetensorsDistributionRoot)) {
        $safetensorsClosure = Read-SafetensorsNativeClosure `
            -DistributionRoot $SafetensorsDistributionRoot
    }
    $texts = @{}
    $mappings = [System.Collections.Generic.List[object]]::new()

    function Add-TextRecord {
        param(
            [Parameter(Mandatory)][object]$TextRecord,
            [Parameter(Mandatory)][object]$Source
        )
        $hash = [string]$TextRecord.Sha256
        if (-not $texts.ContainsKey($hash)) {
            $texts[$hash] = [pscustomobject]@{
                Text = [string]$TextRecord.Text
                ByteLength = [int64]$TextRecord.ByteLength
                Sources = [System.Collections.Generic.List[object]]::new()
            }
        } elseif ([string]$texts[$hash].Text -cne [string]$TextRecord.Text -or
            [int64]$texts[$hash].ByteLength -ne [int64]$TextRecord.ByteLength) {
            throw "Release license text hash collision: $hash"
        }
        $sourceJson = $Source | ConvertTo-Json -Compress -Depth 16
        if (@($texts[$hash].Sources | Where-Object {
            ($_ | ConvertTo-Json -Compress -Depth 16) -ceq $sourceJson
        }).Count -eq 0) {
            $texts[$hash].Sources.Add($Source)
        }
        return $hash
    }

    foreach ($entry in @($componentIndex.GetEnumerator() | Sort-Object Key -CaseSensitive)) {
        $bomRef = [string]$entry.Key
        $record = $entry.Value
        $component = $record.Component
        $scope = [string]$record.Scope
        $ecosystem = [string]$record.Ecosystem
        $licenseExpression = [string]$record.LicenseExpression
        $textHashes = [System.Collections.Generic.List[string]]::new()
        $disposition = 'license_text_in_bundle'
        $rationale = ''

        if ($scope -ceq 'build') {
            $disposition = 'not_redistributed_no_text_required'
            $rationale = 'Build-only tool or library; no package bytes are included in the distributed artifact.'
        } elseif ($ecosystem -ceq 'artifact') {
            $source = [ordered]@{
                source_kind = 'repository-license'
                source_identity = 'f00br/latentdeck'
                file_name = 'LICENSE'
                raw_sha256 = [string]$repoLicense.RawSha256
            }
            $textHashes.Add((Add-TextRecord -TextRecord $repoLicense -Source $source))
        } elseif ($ecosystem -ceq 'python' -and
            [string]$component.name -ceq 'safetensors' -and
            [string]$component.version -ceq '0.8.0') {
            if ($licenseExpression -cne 'Apache-2.0' -or $null -eq $safetensorsClosure) {
                throw 'Safetensors release license identity or exact wheel input is missing.'
            }
            $source = [ordered]@{
                source_kind = 'pinned-python-wheel'
                source_identity = 'safetensors@0.8.0'
                source_url = [string]$safetensorsClosure.Lock.wheel.url
                wheel_file_name = [string]$safetensorsClosure.Lock.wheel.file_name
                wheel_sha256 = [string]$safetensorsClosure.Lock.wheel.sha256
                file_name = [string]$safetensorsClosure.Lock.license.entry
                raw_sha256 = [string]$safetensorsClosure.LicenseText.RawSha256
            }
            $textHashes.Add((Add-TextRecord `
                -TextRecord $safetensorsClosure.LicenseText `
                -Source $source))
        } elseif ($ecosystem -ceq 'python') {
            $identity = "$($component.name)@$($component.version)"
            if (-not $pythonByIdentity.ContainsKey($identity) -or
                [string]$pythonByIdentity[$identity].License -cne $licenseExpression) {
                throw "Release license bundle cannot bind Python project identity: $identity"
            }
            $source = [ordered]@{
                source_kind = 'repository-license'
                source_identity = $identity
                file_name = 'LICENSE'
                raw_sha256 = [string]$repoLicense.RawSha256
            }
            $textHashes.Add((Add-TextRecord -TextRecord $repoLicense -Source $source))
        } elseif ($ecosystem -ceq 'native-cpp' -and [string]$component.name -ceq 'Spout2') {
            if ($licenseExpression -cne 'BSD-2-Clause') {
                throw 'Spout2 release license identity drifted.'
            }
            $source = [ordered]@{
                source_kind = 'reviewed-project-notice'
                source_identity = "Spout2@$($component.version)"
                file_name = [System.IO.Path]::GetFileName($RepositoryNoticePath)
                raw_sha256 = [string]$spoutText.RawSha256
            }
            $textHashes.Add((Add-TextRecord -TextRecord $spoutText -Source $source))
        } elseif ($ecosystem -ceq 'windows-installer' -and
            [string]$component.name -ceq 'NSIS' -and
            [string]$component.version -ceq '3.11') {
            if ($licenseExpression -cne 'NSIS bundled licenses; see bundled COPYING' -or
                (Get-ExactPropertyValue -Component $component `
                    -Name 'latentdeck:license-text-sha256' `
                    -Context 'NSIS SBOM component') -cne
                    'e7dd514003ab96cb3ddccbc028fe5c795fccf57dc41f21cfb9d4dd16ead23bf5') {
                throw 'NSIS release license identity drifted.'
            }
            if ([string]::IsNullOrWhiteSpace($TauriNsisRoot)) {
                throw 'An application license bundle requires an explicit verified TauriNsisRoot.'
            }
            $nsisCopyingPath = Join-Path $TauriNsisRoot 'COPYING'
            $nsisText = ConvertTo-CanonicalLicenseText `
                -Path $nsisCopyingPath `
                -Context 'pinned NSIS 3.11 COPYING text'
            if ([string]$nsisText.RawSha256 -cne
                'e7dd514003ab96cb3ddccbc028fe5c795fccf57dc41f21cfb9d4dd16ead23bf5') {
                throw 'Pinned NSIS 3.11 COPYING bytes drifted.'
            }
            $source = [ordered]@{
                source_kind = 'pinned-tauri-installer-runtime'
                source_identity = 'NSIS@3.11'
                source_commit = '7359413009afd4f0fff472d841fc2f2cc0e0a5f8'
                source_url = 'https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip'
                file_name = 'COPYING'
                raw_sha256 = [string]$nsisText.RawSha256
            }
            $textHashes.Add((Add-TextRecord -TextRecord $nsisText -Source $source))
        } elseif ($ecosystem -ceq 'windows-installer' -and
            [string]$component.name -ceq 'nsis-tauri-utils' -and
            [string]$component.version -ceq '0.5.3') {
            if ($licenseExpression -cne 'Apache-2.0 OR MIT' -or
                (Get-ExactPropertyValue -Component $component `
                    -Name 'latentdeck:source-commit' `
                    -Context 'nsis-tauri-utils SBOM component') -cne
                    '13d9edd27b69310e108d6fbd49f90992f8a05390') {
                throw 'nsis-tauri-utils release license identity drifted.'
            }
            $mitFallbackPath = Join-Path `
                $repositoryRoot `
                'tools/installer/licenses/nsis-tauri-utils-0.5.3-LICENSE-MIT.txt'
            $mitText = ConvertTo-CanonicalLicenseText `
                -Path $mitFallbackPath `
                -Context 'reviewed nsis-tauri-utils 0.5.3 MIT text'
            if ([string]$mitText.RawSha256 -cne
                '1c1020fa10a6bf318717e82c911bcc54ebdfb9bb280460ae332bcb2f82f57fbe') {
                throw 'Reviewed nsis-tauri-utils MIT fallback bytes drifted.'
            }
            $apacheSource = [ordered]@{
                source_kind = 'reviewed-standard-license-text'
                source_identity = 'nsis-tauri-utils@0.5.3/Apache-2.0'
                file_name = 'LICENSE'
                raw_sha256 = [string]$repoLicense.RawSha256
            }
            $mitSource = [ordered]@{
                source_kind = 'reviewed-upstream-fallback'
                source_identity = 'nsis-tauri-utils@0.5.3/MIT'
                source_commit = '13d9edd27b69310e108d6fbd49f90992f8a05390'
                source_url = 'https://raw.githubusercontent.com/tauri-apps/nsis-tauri-utils/13d9edd27b69310e108d6fbd49f90992f8a05390/LICENSE_MIT'
                reviewed_file_sha256 = '1c1020fa10a6bf318717e82c911bcc54ebdfb9bb280460ae332bcb2f82f57fbe'
            }
            $textHashes.Add((Add-TextRecord -TextRecord $repoLicense -Source $apacheSource))
            $textHashes.Add((Add-TextRecord -TextRecord $mitText -Source $mitSource))
        } elseif ($ecosystem -ceq 'rust' -and
            $bomRef.StartsWith(
                'rust:safetensors-native:',
                [System.StringComparison]::Ordinal
            )) {
            if ($null -eq $safetensorsClosure) {
                throw 'Safetensors native runtime closure requires its exact wheel or installed tree.'
            }
            $identity = "$($component.name)@$($component.version)"
            $expectedComponents = @($safetensorsClosure.Components | Where-Object {
                [string]$_.'bom-ref' -ceq $bomRef
            })
            $lockedComponents = @($safetensorsClosure.Lock.components | Where-Object {
                "$($_.name)@$($_.version)" -ceq $identity
            })
            if ($expectedComponents.Count -ne 1 -or $lockedComponents.Count -ne 1) {
                throw "Safetensors native license component is not locked exactly once: $identity"
            }
            $expectedComponent = $expectedComponents[0]
            $lockedComponent = $lockedComponents[0]
            $actualNames = @($component.PSObject.Properties.Name | Sort-Object -CaseSensitive)
            $expectedNames = @(
                'type', 'bom-ref', 'name', 'version', 'hashes', 'licenses', 'purl',
                'externalReferences', 'properties'
            ) | Sort-Object -CaseSensitive
            $actualHashes = @($component.hashes)
            $expectedHashes = @($expectedComponent.hashes)
            $actualReferences = @($component.externalReferences)
            $expectedReferences = @($expectedComponent.externalReferences)
            $propertyNames = @($component.properties | ForEach-Object { [string]$_.name })
            $expectedPropertyNames = @($expectedComponent.properties | ForEach-Object {
                [string]$_.name
            })
            if (($actualNames -join "`0") -cne ($expectedNames -join "`0") -or
                [string]$component.type -cne 'library' -or
                [string]$component.name -cne [string]$expectedComponent.name -or
                [string]$component.version -cne [string]$expectedComponent.version -or
                [string]$component.purl -cne [string]$expectedComponent.purl -or
                $licenseExpression -cne [string]$lockedComponent.license_expression -or
                $actualHashes.Count -ne 1 -or $expectedHashes.Count -ne 1 -or
                [string]$actualHashes[0].alg -cne [string]$expectedHashes[0].alg -or
                [string]$actualHashes[0].content -cne [string]$expectedHashes[0].content -or
                $actualReferences.Count -ne 1 -or $expectedReferences.Count -ne 1 -or
                [string]$actualReferences[0].type -cne [string]$expectedReferences[0].type -or
                [string]$actualReferences[0].url -cne [string]$expectedReferences[0].url -or
                ($propertyNames -join "`0") -cne ($expectedPropertyNames -join "`0")) {
                throw "Safetensors native SBOM component drifted from its reviewed lock: $identity"
            }
            foreach ($expectedProperty in @($expectedComponent.properties)) {
                if ((Get-ExactPropertyValue `
                    -Component $component `
                    -Name ([string]$expectedProperty.name) `
                    -Context "Safetensors native SBOM component $bomRef") -cne
                        [string]$expectedProperty.value) {
                    throw "Safetensors native SBOM property drifted: $identity/$($expectedProperty.name)"
                }
            }
            foreach ($textReference in @($lockedComponent.texts)) {
                $reviewedPath = [string]$textReference.reviewed_path
                if ($reviewedPath.StartsWith('wheel:', [System.StringComparison]::Ordinal)) {
                    if ($identity -cne 'safetensors@0.8.0' -or
                        $reviewedPath.Substring(6) -cne [string]$safetensorsClosure.Lock.license.entry -or
                        [string]$textReference.reviewed_sha256 -cne
                            [string]$safetensorsClosure.LicenseText.Sha256 -or
                        [string]$textReference.source_raw_sha256 -cne
                            [string]$safetensorsClosure.LicenseText.RawSha256) {
                        throw 'Safetensors native project license evidence drifted.'
                    }
                    $source = [ordered]@{
                        source_kind = 'pinned-python-wheel-native-project'
                        source_identity = $identity
                        source_url = [string]$safetensorsClosure.Lock.wheel.url
                        wheel_sha256 = [string]$safetensorsClosure.Lock.wheel.sha256
                        source_entry = [string]$textReference.source_entry
                        source_raw_sha256 = [string]$textReference.source_raw_sha256
                    }
                    $textHashes.Add((Add-TextRecord `
                        -TextRecord $safetensorsClosure.LicenseText `
                        -Source $source))
                    continue
                }
                if ([string]$lockedComponent.source_kind -cne 'crates.io-archive' -or
                    [string]::IsNullOrWhiteSpace([string]$lockedComponent.archive_sha256) -or
                    [string]::IsNullOrWhiteSpace([string]$lockedComponent.archive_url)) {
                    throw "Safetensors native crate license provenance is incomplete: $identity"
                }
                $fullReviewedPath = Join-Path $repositoryRoot $reviewedPath
                $reviewedText = ConvertTo-CanonicalLicenseText `
                    -Path $fullReviewedPath `
                    -Context "reviewed Safetensors native license text $identity"
                if ([string]$reviewedText.Sha256 -cne [string]$textReference.reviewed_sha256) {
                    throw "Reviewed Safetensors native license text drifted: $identity/$reviewedPath"
                }
                $source = [ordered]@{
                    source_kind = 'reviewed-exact-crates.io-archive'
                    source_identity = $identity
                    source_url = [string]$lockedComponent.archive_url
                    archive_sha256 = [string]$lockedComponent.archive_sha256
                    source_entry = [string]$textReference.source_entry
                    source_raw_sha256 = [string]$textReference.source_raw_sha256
                    reviewed_file_sha256 = [string]$textReference.reviewed_sha256
                }
                $textHashes.Add((Add-TextRecord -TextRecord $reviewedText -Source $source))
            }
            if ($textHashes.Count -eq 0) {
                throw "Safetensors native runtime component has no reviewed full text: $identity"
            }
        } elseif ($ecosystem -ceq 'rust') {
            $identity = "$($component.name)@$($component.version)"
            if (-not $cargoByIdentity.ContainsKey($identity) -or
                $cargoByIdentity[$identity].Count -ne 1) {
                throw "Release license bundle cannot resolve one Cargo package: $identity"
            }
            $package = $cargoByIdentity[$identity][0]
            if ([string]$package.license -cne $licenseExpression) {
                throw "Release license bundle Cargo expression drifted: $identity"
            }
            $packageRoot = [System.IO.Path]::GetDirectoryName(
                [System.IO.Path]::GetFullPath([string]$package.manifest_path)
            )
            $licenseFiles = @(Get-LicenseCandidateFiles -PackageRoot $packageRoot)
            if (-not [string]::IsNullOrWhiteSpace([string]$package.license_file)) {
                $declared = Get-Item -LiteralPath ([string]$package.license_file) -Force
                if (@($licenseFiles | Where-Object FullName -CEQ $declared.FullName).Count -eq 0) {
                    $licenseFiles += $declared
                }
            }
            if ($licenseFiles.Count -eq 0 -and $null -eq $package.source) {
                $licenseFiles = @(Get-Item -LiteralPath $repoLicensePath -Force)
            }
            if ($licenseFiles.Count -gt 0) {
                foreach ($licenseFile in @($licenseFiles | Sort-Object Name)) {
                    $text = ConvertTo-CanonicalLicenseText `
                        -Path $licenseFile.FullName `
                        -Context "Cargo license text $identity/$($licenseFile.Name)"
                    $source = [ordered]@{
                        source_kind = if ($null -eq $package.source) {
                            'repository-license'
                        } else {
                            'locked-cargo-package'
                        }
                        source_identity = $identity
                        file_name = $licenseFile.Name
                        raw_sha256 = [string]$text.RawSha256
                    }
                    $textHashes.Add((Add-TextRecord -TextRecord $text -Source $source))
                }
            } else {
                if (-not $cargoFallbacks.ContainsKey($identity)) {
                    throw "Runtime Cargo package has no reviewed license text or fallback: $identity"
                }
                $fallback = $cargoFallbacks[$identity]
                $vcsInfoPath = Join-Path $packageRoot '.cargo_vcs_info.json'
                if ((Normalize-RepositoryUrl -Value ([string]$package.repository)) -cne
                        (Normalize-RepositoryUrl -Value ([string]$fallback.Repository)) -or
                    $licenseExpression -cne [string]$fallback.LicenseExpression -or
                    -not (Test-Path -LiteralPath $vcsInfoPath -PathType Leaf)) {
                    throw "Reviewed Cargo license fallback identity drifted: $identity"
                }
                $vcsInfo = Get-Content -LiteralPath $vcsInfoPath -Raw | ConvertFrom-Json -Depth 16
                if ([string]$vcsInfo.git.sha1 -cne [string]$fallback.SourceCommit) {
                    throw "Reviewed Cargo license fallback commit drifted: $identity"
                }
                foreach ($fallbackFile in @($fallback.Files)) {
                    $fallbackPath = Join-Path $repositoryRoot ([string]$fallbackFile.Path)
                    $fallbackHash = Get-LowerSha256File -Path $fallbackPath
                    if ($fallbackHash -cne [string]$fallbackFile.Sha256) {
                        throw "Reviewed Cargo license fallback bytes drifted: $identity"
                    }
                    $text = ConvertTo-CanonicalLicenseText `
                        -Path $fallbackPath `
                        -Context "reviewed Cargo license fallback $identity"
                    $source = [ordered]@{
                        source_kind = 'reviewed-upstream-fallback'
                        source_identity = $identity
                        source_commit = [string]$fallback.SourceCommit
                        source_url = [string]$fallbackFile.Url
                        reviewed_file_sha256 = [string]$fallbackFile.Sha256
                    }
                    $textHashes.Add((Add-TextRecord -TextRecord $text -Source $source))
                }
            }
        } elseif ($ecosystem -ceq 'node' -and
            [string]$component.name -ceq '@tauri-apps/plugin-dialog' -and
            [string]$component.version -ceq '2.7.2') {
            $identity = '@tauri-apps/plugin-dialog@2.7.2'
            if ($licenseExpression -cne 'MIT OR Apache-2.0' -or
                -not $nodeIndex.ContainsKey($identity)) {
                throw 'Tauri dialog plugin release license identity drifted.'
            }
            $nodePackages = @($nodeIndex[$identity])
            if ($nodePackages.Count -eq 0 -or @($nodePackages | Where-Object {
                [string]$_.Manifest.repository -cne 'https://github.com/tauri-apps/plugins-workspace' -or
                [string]$_.Manifest.license -cne 'MIT OR Apache-2.0'
            }).Count -gt 0) {
                throw 'Tauri dialog plugin package metadata drifted.'
            }
            foreach ($nodePackage in $nodePackages) {
                $spdxPath = Join-Path $nodePackage.Root 'LICENSE.spdx'
                if ((Get-LowerSha256File -Path $spdxPath) -cne
                    'eb8a6c84630461b352badcab1dbe5d0168c56d377358b2b8c86b51003272d5ef') {
                    throw 'Tauri dialog plugin SPDX metadata drifted.'
                }
            }
            $pluginMitPath = Join-Path `
                $repositoryRoot `
                'tools/installer/licenses/tauri-plugins-workspace-03afae6d-LICENSE-MIT.txt'
            $pluginMitText = ConvertTo-CanonicalLicenseText `
                -Path $pluginMitPath `
                -Context 'reviewed Tauri plugin MIT text'
            if ([string]$pluginMitText.RawSha256 -cne
                '9dd42ea92cff2ede5cd477cbfcce051b2d0115c0ac7f368ee88cb545055dff1d') {
                throw 'Reviewed Tauri plugin MIT fallback bytes drifted.'
            }
            $pluginApacheSource = [ordered]@{
                source_kind = 'reviewed-standard-license-text'
                source_identity = "$identity/Apache-2.0"
                source_commit = '03afae6d7275030a708eaedd39f0e604ccb901f3'
                source_url = 'https://raw.githubusercontent.com/tauri-apps/plugins-workspace/03afae6d7275030a708eaedd39f0e604ccb901f3/LICENSE_APACHE-2.0'
                upstream_file_sha256 = '0cec06e0e55fbc3dc5cee4fca9b607f66cb8f4e4dbcf3b3c013594dd156732e9'
                normalized_text_sha256 = [string]$repoLicense.Sha256
            }
            $pluginMitSource = [ordered]@{
                source_kind = 'reviewed-upstream-fallback'
                source_identity = "$identity/MIT"
                source_commit = '03afae6d7275030a708eaedd39f0e604ccb901f3'
                source_url = 'https://raw.githubusercontent.com/tauri-apps/plugins-workspace/03afae6d7275030a708eaedd39f0e604ccb901f3/LICENSE_MIT'
                upstream_file_sha256 = '89ff9689dcf9dd53968785d05a26f7898bb169dbfcada8d032b3e68cf0d55607'
                normalized_text_sha256 = [string]$pluginMitText.Sha256
            }
            $textHashes.Add((Add-TextRecord -TextRecord $repoLicense -Source $pluginApacheSource))
            $textHashes.Add((Add-TextRecord -TextRecord $pluginMitText -Source $pluginMitSource))
        } elseif ($ecosystem -ceq 'node') {
            $identity = "$($component.name)@$($component.version)"
            if (-not $nodeIndex.ContainsKey($identity)) {
                throw "Release license bundle cannot resolve installed Node package: $identity"
            }
            $candidateSets = @()
            $usedReviewedFallback = $false
            foreach ($nodePackage in @($nodeIndex[$identity])) {
                if ([string]$nodePackage.Manifest.license -cne $licenseExpression) {
                    throw "Release license bundle Node expression drifted: $identity"
                }
                $files = @(Get-LicenseCandidateFiles -PackageRoot $nodePackage.Root)
                if ($files.Count -gt 0) {
                    $candidateSets += ,@($files)
                }
            }
            if ($candidateSets.Count -eq 0) {
                if (-not $nodeFallbacks.ContainsKey($identity)) {
                    throw "Runtime Node package has no distributable license or notice text: $identity"
                }
                $fallback = $nodeFallbacks[$identity]
                $nodePackages = @($nodeIndex[$identity])
                if ($licenseExpression -cne [string]$fallback.LicenseExpression -or
                    $pnpmLockText.IndexOf(
                        [string]$fallback.RegistryIntegrity,
                        [System.StringComparison]::Ordinal
                    ) -lt 0 -or
                    $nodePackages.Count -eq 0 -or @($nodePackages | Where-Object {
                        (Get-NodeManifestRepositoryUrl -Manifest $_.Manifest) -cne
                            [string]$fallback.Repository -or
                        (Get-LowerSha256File -Path (Join-Path $_.Root 'package.json')) -cne
                            [string]$fallback.PackageJsonSha256
                    }).Count -gt 0) {
                    throw "Reviewed Node license fallback identity drifted: $identity"
                }
                $fallbackPath = Join-Path $repositoryRoot ([string]$fallback.File.Path)
                if ((Get-LowerSha256File -Path $fallbackPath) -cne [string]$fallback.File.Sha256) {
                    throw "Reviewed Node license fallback bytes drifted: $identity"
                }
                $text = ConvertTo-CanonicalLicenseText `
                    -Path $fallbackPath `
                    -Context "reviewed Node license fallback $identity"
                $source = [ordered]@{
                    source_kind = 'reviewed-locked-node-fallback'
                    source_identity = $identity
                    source_commit = [string]$fallback.SourceCommit
                    source_url = [string]$fallback.File.Url
                    registry_integrity = [string]$fallback.RegistryIntegrity
                    reviewed_file_sha256 = [string]$fallback.File.Sha256
                    upstream_file_sha256 = [string]$fallback.File.UpstreamSha256
                }
                $textHashes.Add((Add-TextRecord -TextRecord $text -Source $source))
                $usedReviewedFallback = $true
            }
            $selectedFiles = if ($usedReviewedFallback) {
                @()
            } else {
                @($candidateSets[0])
            }
            foreach ($licenseFile in $selectedFiles) {
                $text = ConvertTo-CanonicalLicenseText `
                    -Path $licenseFile.FullName `
                    -Context "Node license text $identity/$($licenseFile.Name)"
                $source = [ordered]@{
                    source_kind = 'locked-node-package'
                    source_identity = $identity
                    file_name = $licenseFile.Name
                    raw_sha256 = [string]$text.RawSha256
                }
                $textHashes.Add((Add-TextRecord -TextRecord $text -Source $source))
            }
        } else {
            throw "Release license bundle does not support ecosystem '$ecosystem' for $bomRef."
        }

        $mappings.Add([pscustomobject][ordered]@{
            'bom-ref' = $bomRef
            name = [string]$component.name
            version = [string]$component.version
            ecosystem = $ecosystem
            dependency_scope = $scope
            license_expression = $licenseExpression
            artifacts = @($record.Artifacts | Sort-Object -CaseSensitive)
            disposition = $disposition
            rationale = $rationale
            text_sha256s = @($textHashes | Sort-Object -CaseSensitive -Unique)
        })
    }

    $textRecords = @(
        foreach ($entry in @($texts.GetEnumerator() | Sort-Object Key -CaseSensitive)) {
            [ordered]@{
                sha256 = [string]$entry.Key
                byte_length = [int64]$entry.Value.ByteLength
                sources = @($entry.Value.Sources | Sort-Object {
                    $_ | ConvertTo-Json -Compress -Depth 16
                })
                text = [string]$entry.Value.Text
            }
        }
    )
    $bundle = [ordered]@{
        schema_version = 1
        artifact = [ordered]@{ name = $ArtifactName; version = $ArtifactVersion }
        policy = [ordered]@{
            component_coverage = 'exact-sbom-closure'
            redistributed_components_require_text = $true
            build_only_disposition = 'not_redistributed_no_text_required'
            build_only_scope_lock = 'tools/packaging/windows-x64-release-build-only.lock.json'
            build_only_scope_lock_sha256 = [string]$scopePolicy.Sha256
            text_canonicalization = 'strict-utf8-lf-final-newline'
        }
        sboms = @($sbomRecords | Sort-Object name -CaseSensitive)
        component_count = $mappings.Count
        text_count = $textRecords.Count
        components = @($mappings)
        texts = $textRecords
    }
    $json = ($bundle | ConvertTo-Json -Depth 100) + "`n"
    foreach ($forbidden in @($repositoryRoot, $env:USERPROFILE)) {
        if (-not [string]::IsNullOrWhiteSpace($forbidden) -and
            $json.IndexOf($forbidden, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw 'Generated release license bundle contains a machine-local path.'
        }
    }
    # Third-party license texts can contain illustrative filesystem paths. The
    # generated metadata itself carries only basenames and pinned HTTPS URLs;
    # exact local repository/profile roots are rejected above.
    $partialPath = "$outputFullPath.partial-$([guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllText(
            $partialPath,
            $json,
            [System.Text.UTF8Encoding]::new($false)
        )
        [System.IO.File]::Move($partialPath, $outputFullPath, $false)
    } finally {
        if (Test-Path -LiteralPath $partialPath -PathType Leaf) {
            [System.IO.File]::Delete($partialPath)
        }
    }
    return Test-ReleaseLicenseBundle `
        -BundlePath $outputFullPath `
        -SbomPath $SbomPath `
        -ExpectedArtifactName $ArtifactName `
        -ExpectedArtifactVersion $ArtifactVersion
}

Export-ModuleMember -Function @('New-ReleaseLicenseBundle', 'Test-ReleaseLicenseBundle')
