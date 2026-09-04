[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'ReleaseSpoutMetadata.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'TauriReleaseContract.psm1') -Force

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".packaging-test-$([guid]::NewGuid().ToString('N'))"
$testFullPath = [System.IO.Path]::GetFullPath($testRoot)
$artifactsFullPath = [System.IO.Path]::GetFullPath($artifactsRoot).TrimEnd('\', '/')
if (-not $testFullPath.StartsWith(
    $artifactsFullPath + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
) -or -not ([System.IO.Path]::GetFileName($testFullPath)).StartsWith(
    '.packaging-test-',
    [System.StringComparison]::Ordinal
)) {
    throw 'Packaging test temporary directory failed its containment check.'
}

function Write-Utf8Text {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Content
    )

    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $Path)) | Out-Null
    [System.IO.File]::WriteAllText(
        $Path,
        $Content,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Add-CataloguedFixtureFile {
    param(
        [Parameter(Mandatory)]
        [string]$PackRoot,

        [Parameter(Mandatory)]
        [string]$RelativePath,

        [Parameter(Mandatory)]
        [byte[]]$Bytes
    )

    $filePath = Join-Path $PackRoot $RelativePath
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $filePath)) | Out-Null
    [System.IO.File]::WriteAllBytes($filePath, $Bytes)

    $catalogPath = Join-Path $PackRoot 'integrity.json'
    $catalog = Get-Content -Raw -LiteralPath $catalogPath | ConvertFrom-Json -Depth 32
    $catalogFiles = @($catalog.files) + @(
        Get-IntegrityEntry -RootPath $PackRoot -File (Get-Item -LiteralPath $filePath)
    )
    Write-JsonFile -Value ([ordered]@{
        manifest_version = '1.0.0'
        files = @($catalogFiles | Sort-Object -Property path -CaseSensitive)
    }) -Path $catalogPath

    $manifestPath = Join-Path $PackRoot 'codec-pack.json'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json -Depth 32
    $manifest.integrity.catalog_sha256 = (
        Get-FileHash -LiteralPath $catalogPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    Write-JsonFile -Value $manifest -Path $manifestPath
}

function Write-InvalidUtf8InsideAsciiMarker {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Marker
    )

    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $markerBytes = [System.Text.Encoding]::ASCII.GetBytes($Marker)
    $matchIndex = -1
    for ($index = 0; $index -le $bytes.Length - $markerBytes.Length; $index += 1) {
        $matches = $true
        for ($markerIndex = 0; $markerIndex -lt $markerBytes.Length; $markerIndex += 1) {
            if ($bytes[$index + $markerIndex] -ne $markerBytes[$markerIndex]) {
                $matches = $false
                break
            }
        }
        if ($matches) {
            $matchIndex = $index
            break
        }
    }
    if ($matchIndex -lt 0) {
        throw "Could not find ASCII marker '$Marker' in JSON fixture."
    }
    $bytes[$matchIndex] = 0xff
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

function Assert-Throws {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Action,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $threw = $false
    try {
        & $Action
    } catch {
        $threw = $true
    }
    if (-not $threw) {
        throw "Expected failure did not occur: $Context"
    }
}

function Assert-NativeFailureContains {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Command,

        [Parameter(Mandatory)]
        [string]$ExpectedText,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $output = & $Command 2>&1 | Out-String
    $exitCode = $LASTEXITCODE
    if ($exitCode -eq 0 -or
        $output.IndexOf($ExpectedText, [System.StringComparison]::Ordinal) -lt 0) {
        throw (
            "$Context did not fail with the required diagnostic. " +
            "Exit=$exitCode Output=$output"
        )
    }
}

function Resolve-TestPython313 {
    if (-not [string]::IsNullOrWhiteSpace($env:LATENTDECK_TEST_PYTHON313)) {
        return (Resolve-Path -LiteralPath $env:LATENTDECK_TEST_PYTHON313).Path
    }

    $pyLauncher = Get-Command py.exe -ErrorAction SilentlyContinue
    if ($null -ne $pyLauncher) {
        $candidate = (& $pyLauncher.Source -3.13 -c 'import sys; print(sys.executable)' 2>$null)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($candidate)) {
            return (Resolve-Path -LiteralPath $candidate.Trim()).Path
        }
    }

    $python = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($null -ne $python) {
        $candidate = (& $python.Source -c 'import sys; print(sys.executable if sys.version_info[:2] == (3, 13) else "")' 2>$null)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($candidate)) {
            return (Resolve-Path -LiteralPath $candidate.Trim()).Path
        }
    }

    throw (
        'Packaging contract tests require a local CPython 3.13 x64 installation. ' +
        'Set LATENTDECK_TEST_PYTHON313 to its python.exe path.'
    )
}

function Write-SyntheticDependencyMetadata {
    param(
        [Parameter(Mandatory)]
        [string]$InventoryPath,

        [Parameter(Mandatory)]
        [string]$SbomPath,

        [Parameter(Mandatory)]
        [string]$PackVersion
    )

    $sourceCommit = 'c' * 40
    $metadataRoot = Split-Path -Parent $InventoryPath
    $nativeSbomPath = Join-Path $metadataRoot 'NATIVE_RUST_SBOM.cdx.json'
    $nativeLicensesPath = Join-Path $metadataRoot 'NATIVE_RUST_LICENSES.json'
    $nativeArtifactName = 'LatentDeck H3 Native Extensions'
    $nativeArtifactReference = "pkg:generic/LatentDeck%20H3%20Native%20Extensions@$PackVersion"
    $nativeComponents = @(
        foreach ($nativeName in @('latentdeck-cartridge-python', 'latentdeck-gpu-python')) {
            [ordered]@{
                'bom-ref' = "rust:$nativeName@0.1.0"
                type = 'library'
                name = $nativeName
                version = '0.1.0'
                licenses = @([ordered]@{ license = [ordered]@{ name = 'Apache-2.0' } })
                properties = @(
                    [ordered]@{ name = 'latentdeck:ecosystem'; value = 'rust' }
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                    [ordered]@{ name = 'latentdeck:selection-root'; value = 'true' }
                )
            }
        }
    )
    $nativeRoot = [ordered]@{
        'bom-ref' = $nativeArtifactReference
        type = 'application'
        name = $nativeArtifactName
        version = $PackVersion
        licenses = @([ordered]@{ license = [ordered]@{ name = 'Apache-2.0' } })
        properties = @(
            [ordered]@{ name = 'latentdeck:artifact-scope'; value = 'h3-native' }
            [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
            [ordered]@{ name = 'latentdeck:target-platform'; value = 'x86_64-pc-windows-msvc' }
        )
    }
    Write-Utf8Text -Path $nativeSbomPath -Content (([ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        serialNumber = 'urn:uuid:00000000-0000-0000-0000-000000000001'
        version = 1
        metadata = [ordered]@{ component = $nativeRoot }
        components = $nativeComponents
    } | ConvertTo-Json -Depth 32) + "`n")
    $nativeLicenseText = "Synthetic Apache-2.0 fixture text.`n"
    $nativeLicenseBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($nativeLicenseText)
    $nativeLicenseHash = [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($nativeLicenseBytes)
    ).ToLowerInvariant()
    $nativeMappings = @(
        foreach ($mappingEntry in @(
            [pscustomobject]@{ Component = $nativeRoot; Ecosystem = 'artifact' }
            @($nativeComponents | ForEach-Object {
                [pscustomobject]@{ Component = $_; Ecosystem = 'rust' }
            })
        )) {
            [ordered]@{
                'bom-ref' = [string]$mappingEntry.Component.'bom-ref'
                name = [string]$mappingEntry.Component.name
                version = [string]$mappingEntry.Component.version
                ecosystem = [string]$mappingEntry.Ecosystem
                dependency_scope = 'artifact'
                license_expression = 'Apache-2.0'
                artifacts = @($nativeArtifactName)
                disposition = 'license_text_in_bundle'
                rationale = ''
                text_sha256s = @($nativeLicenseHash)
            }
        }
    )
    $nativeSbomItem = Get-Item -LiteralPath $nativeSbomPath
    Write-Utf8Text -Path $nativeLicensesPath -Content (([ordered]@{
        schema_version = 1
        artifact = [ordered]@{ name = $nativeArtifactName; version = $PackVersion }
        policy = [ordered]@{
            component_coverage = 'exact-sbom-closure'
            redistributed_components_require_text = $true
            build_only_disposition = 'not_redistributed_no_text_required'
            text_canonicalization = 'strict-utf8-lf-final-newline'
        }
        sboms = @([ordered]@{
            name = 'NATIVE_RUST_SBOM.cdx.json'
            artifact = $nativeArtifactName
            byte_length = [int64]$nativeSbomItem.Length
            sha256 = (Get-FileHash -LiteralPath $nativeSbomPath -Algorithm SHA256).Hash.ToLowerInvariant()
        })
        component_count = $nativeMappings.Count
        text_count = 1
        components = $nativeMappings
        texts = @([ordered]@{
            sha256 = $nativeLicenseHash
            byte_length = [int64]$nativeLicenseBytes.Length
            sources = @([ordered]@{ source_kind = 'synthetic-test' })
            text = $nativeLicenseText
        })
    } | ConvertTo-Json -Depth 32) + "`n")
    $components = @(
        [ordered]@{
            name = 'CPython'
            version = '3.13.14'
            kind = 'runtime'
            source_url = 'https://www.python.org/'
            license_expression = 'PSF-2.0'
            license_files = @('runtime/LICENSE.txt')
            content_sha256 = ('a' * 64)
        },
        [ordered]@{
            name = 'latentdeck-codec-h3'
            version = '0.2.0'
            kind = 'repository'
            source_url = 'https://github.com/f00br/latentdeck'
            license_expression = 'Apache-2.0'
            license_files = @()
            content_sha256 = ('b' * 64)
        }
    )
    Write-Utf8Text -Path $InventoryPath -Content (([ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        source_commit = $sourceCommit
        platform = 'windows-x86_64'
        curator = [ordered]@{
            name = 'latentdeck-codec-pack-curator'
            schema_version = 1
        }
        components = $components
        native_rust = [ordered]@{
            sbom_path = 'NATIVE_RUST_SBOM.cdx.json'
            sbom_sha256 = (Get-FileHash -LiteralPath $nativeSbomPath -Algorithm SHA256).Hash.ToLowerInvariant()
            license_bundle_path = 'NATIVE_RUST_LICENSES.json'
            license_bundle_sha256 = (Get-FileHash -LiteralPath $nativeLicensesPath -Algorithm SHA256).Hash.ToLowerInvariant()
            component_count = $nativeComponents.Count
            selection_roots = @('latentdeck-cartridge-python', 'latentdeck-gpu-python')
        }
    } | ConvertTo-Json -Depth 16) + "`n")

    $sbomComponents = @(
        foreach ($component in $components) {
            [ordered]@{
                'bom-ref' = "pkg:generic/$($component.name.ToLowerInvariant())@$($component.version)"
                type = if ($component.kind -ceq 'runtime') { 'application' } else { 'library' }
                name = $component.name
                version = $component.version
                hashes = @([ordered]@{ alg = 'SHA-256'; content = $component.content_sha256 })
                licenses = @([ordered]@{ expression = $component.license_expression })
                externalReferences = @(
                    [ordered]@{ type = 'distribution'; url = $component.source_url }
                )
                properties = @(
                    [ordered]@{ name = 'latentdeck:ecosystem'; value = 'python' }
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'runtime' }
                )
            }
        }
    )
    Write-Utf8Text -Path $SbomPath -Content (([ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        version = 1
        metadata = [ordered]@{
            component = [ordered]@{
                'bom-ref' = "pkg:generic/latentdeck-h3-codec-pack@$PackVersion"
                type = 'application'
                name = 'LatentDeck H3 Codec Pack'
                version = $PackVersion
                licenses = @([ordered]@{ expression = 'Apache-2.0' })
                properties = @(
                    [ordered]@{ name = 'latentdeck:source-commit'; value = $sourceCommit }
                    [ordered]@{ name = 'latentdeck:artifact-scope'; value = 'h3-codec-pack' }
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                    [ordered]@{
                        name = 'latentdeck:included-dependency-scopes'
                        value = 'artifact,runtime,build,runtime+build'
                    }
                    [ordered]@{ name = 'latentdeck:excluded-dependency-scopes'; value = 'development' }
                    [ordered]@{ name = 'latentdeck:target-platform'; value = 'windows-x86_64' }
                )
            }
        }
        components = @($sbomComponents) + @($nativeComponents)
    } | ConvertTo-Json -Depth 16) + "`n")
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null

    foreach ($caseVariant in @('UNSIGNED_PREVIEW', 'Unsigned_Preview')) {
        Assert-NativeFailureContains `
            -Context "application release channel case variant $caseVariant" `
            -ExpectedText 'ReleaseChannel must be exactly unsigned_preview or stable.' `
            -Command {
                & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1') `
                    -ReleaseChannel $caseVariant `
                    -DevelopmentBuild
            }
        Assert-NativeFailureContains `
            -Context "H3 release channel case variant $caseVariant" `
            -ExpectedText 'ReleaseChannel must be exactly unsigned_preview or stable.' `
            -Command {
                & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-H3CodecPack.ps1') `
                    -PythonEmbedArchive (Join-Path $testRoot 'unused-python-embed.zip') `
                    -PackVersion 0.2.1 `
                    -ReleaseChannel $caseVariant `
                    -DevelopmentBuild
            }
    }

    $portableSourceProbe = Join-Path $testRoot 'portable-source-probe'
    Write-Utf8Text `
        -Path (Join-Path $portableSourceProbe 'documented-placeholder.py') `
        -Content (
            "client = Client(password='password', asynchronous=True)`n" +
            "password = 'password'  # documented fsspec placeholder`n"
        )
    Write-Utf8Text `
        -Path (Join-Path $portableSourceProbe 'runtime-script.ld') `
        -Content "/* GNU ld script */`nINPUT(libportable_runtime.so)`n"
    Assert-PackagingSourceTree -RootPath $portableSourceProbe | Out-Null

    Write-Utf8Text `
        -Path (Join-Path $portableSourceProbe 'wrong-key-placeholder.py') `
        -Content "client = Client(api_key='password')`n"
    Assert-Throws `
        -Context 'only the exact documented password placeholder may bypass source credential scanning' `
        -Action { Assert-PackagingSourceTree -RootPath $portableSourceProbe | Out-Null }
    Remove-Item -LiteralPath (Join-Path $portableSourceProbe 'wrong-key-placeholder.py') -Force

    Write-Utf8Text `
        -Path (Join-Path $portableSourceProbe 'embedded-secret.py') `
        -Content "client = Client(password='real-secret-value')`n"
    Assert-Throws `
        -Context 'portable source scanner must distinguish one exact documented placeholder from a credential' `
        -Action { Assert-PackagingSourceTree -RootPath $portableSourceProbe | Out-Null }
    Remove-Item -LiteralPath (Join-Path $portableSourceProbe 'embedded-secret.py') -Force

    foreach ($composedPassword in @(
        [pscustomobject]@{
            Name = 'concatenated-password.py'
            Content = "client = Client(password='password' + 'embedded-secret-value')`n"
        },
        [pscustomobject]@{
            Name = 'conditional-password.py'
            Content = "client = Client(password='password' if use_placeholder else 'embedded-secret-value')`n"
        },
        [pscustomobject]@{
            Name = 'adjacent-password.py'
            Content = "client = Client(password='password' 'embedded-secret-value')`n"
        }
    )) {
        $composedPasswordPath = Join-Path $portableSourceProbe $composedPassword.Name
        Write-Utf8Text -Path $composedPasswordPath -Content $composedPassword.Content
        Assert-Throws `
            -Context "the exact password placeholder must reject $($composedPassword.Name) expression continuation" `
            -Action { Assert-PackagingSourceTree -RootPath $portableSourceProbe | Out-Null }
        Remove-Item -LiteralPath $composedPasswordPath -Force
    }

    Write-Utf8Text `
        -Path (Join-Path $portableSourceProbe 'sensitive-config.json') `
        -Content '{"password":"password"}'
    Assert-Throws `
        -Context 'credential placeholders remain forbidden in sensitive metadata and configuration' `
        -Action { Assert-PackagingSourceTree -RootPath $portableSourceProbe | Out-Null }
    Remove-Item -LiteralPath (Join-Path $portableSourceProbe 'sensitive-config.json') -Force


    $nestedDeckPath = Join-Path $portableSourceProbe 'nested-deck.ld'
    $nestedDeckStream = [System.IO.FileStream]::new(
        $nestedDeckPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $nestedDeck = [System.IO.Compression.ZipArchive]::new(
            $nestedDeckStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            $entry = $nestedDeck.CreateEntry('deck-pack.json')
            $writer = [System.IO.StreamWriter]::new(
                $entry.Open(),
                [System.Text.UTF8Encoding]::new($false)
            )
            try {
                $writer.Write('{}')
            } finally {
                $writer.Dispose()
            }
        } finally {
            $nestedDeck.Dispose()
        }
    } finally {
        $nestedDeckStream.Dispose()
    }
    Assert-Throws `
        -Context 'a valid Deck ZIP renamed to .ld must remain forbidden inside a Codec Pack source' `
        -Action { Assert-PackagingSourceTree -RootPath $portableSourceProbe | Out-Null }
    [byte[]]$nestedDeckBytes = [System.IO.File]::ReadAllBytes($nestedDeckPath)
    Remove-Item -LiteralPath $nestedDeckPath -Force

    $outerCodecArchivePath = Join-Path $testRoot 'nested-deck-probe.ldcodec'
    $outerCodecStream = [System.IO.FileStream]::new(
        $outerCodecArchivePath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $outerCodecArchive = [System.IO.Compression.ZipArchive]::new(
            $outerCodecStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            $entry = $outerCodecArchive.CreateEntry('runtime/nested-deck.ld')
            $entryStream = $entry.Open()
            try {
                $entryStream.Write($nestedDeckBytes, 0, $nestedDeckBytes.Length)
            } finally {
                $entryStream.Dispose()
            }
        } finally {
            $outerCodecArchive.Dispose()
        }
    } finally {
        $outerCodecStream.Dispose()
    }
    Assert-Throws `
        -Context 'safe Codec Pack extraction must reject a nested Deck ZIP under the ambiguous .ld extension' `
        -Action {
            Expand-SafeCodecPackArchive `
                -ArchivePath $outerCodecArchivePath `
                -DestinationPath (Join-Path $testRoot 'nested-deck-expanded')
        }

    $generatedReleaseSbom = Join-Path $testRoot 'latentdeck-0.1.0-sbom.cdx.json'
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath $generatedReleaseSbom `
        -ArtifactName 'LatentDeck App' `
        -ArtifactVersion '0.1.0+1' `
        -ArtifactScope application `
        -CargoPackage latentdeck-app `
        -NodePackage '@latentdeck/app' `
        -NodeBuildPackage @(
            '@sveltejs/vite-plugin-svelte',
            '@tailwindcss/vite',
            '@tauri-apps/cli',
            'svelte',
            'tailwindcss',
            'typescript',
            'vite',
            'vitest'
        ) `
        -NodeRuntimeBuildPackage @('svelte', 'tailwindcss', 'vite') `
        -IncludeSpout2 `
        -IncludeTauriWindowsInstaller | Out-Null
    $generatedBom = Get-Content -Raw -LiteralPath $generatedReleaseSbom |
        ConvertFrom-Json -Depth 100
    Assert-Spout2CycloneDxComponent -Components @($generatedBom.components) | Out-Null
    $missingReleaseLicenses = @(
        @($generatedBom.metadata.component) + @($generatedBom.components) |
            Where-Object {
                $null -eq $_.PSObject.Properties['licenses'] -or @($_.licenses).Count -eq 0
            }
    )
    if ($missingReleaseLicenses.Count -gt 0) {
        throw 'Generated release SBOM has components without license metadata.'
    }
    if (@($generatedBom.metadata.component.licenses).Count -ne 1 -or
        [string]$generatedBom.metadata.component.licenses[0].license.name -cne 'Apache-2.0') {
        throw 'Generated release SBOM root does not bind the Apache-2.0 artifact license.'
    }
    $releaseScopePolicy = @($generatedBom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:included-dependency-scopes' -and
        [string]$_.value -ceq 'artifact,runtime,build,runtime+build'
    })
    $releaseExcludedScope = @($generatedBom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:excluded-dependency-scopes' -and
        [string]$_.value -ceq 'development'
    })
    $releaseTargetPlatform = @($generatedBom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:target-platform' -and
        [string]$_.value -ceq 'x86_64-pc-windows-msvc'
    })
    $releaseExcludedNodeDevelopment = @($generatedBom.metadata.component.properties | Where-Object {
        [string]$_.name -ceq 'latentdeck:excluded-node-development-component-count' -and
        [string]$_.value -cmatch '^[1-9][0-9]*$'
    })
    $allowedReleaseScopes = @('artifact', 'runtime', 'build', 'runtime+build')
    $invalidReleaseScopes = @(
        foreach ($component in @($generatedBom.metadata.component) + @($generatedBom.components)) {
            $scopes = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope'
            })
            if ($scopes.Count -ne 1 -or
                [string]$scopes[0].value -cnotin $allowedReleaseScopes) {
                "$($component.name)@$($component.version)"
            }
        }
    )
    if ($releaseScopePolicy.Count -ne 1 -or $releaseExcludedScope.Count -ne 1 -or
        $releaseTargetPlatform.Count -ne 1 -or
        $releaseExcludedNodeDevelopment.Count -ne 1 -or
        $invalidReleaseScopes.Count -ne 0) {
        throw 'Generated Windows release SBOM dependency scope classification is invalid.'
    }
    $workspaceNodeManifest = Get-Content -LiteralPath (Join-Path $repoRoot 'package.json') -Raw |
        ConvertFrom-Json -Depth 16
    $expectedNodeBuildRoots = @(
        '@sveltejs/vite-plugin-svelte',
        '@tailwindcss/vite',
        '@tauri-apps/cli',
        'svelte',
        'tailwindcss',
        'typescript',
        'vite',
        'vitest'
    )
    foreach ($expectedNodeBuildRoot in $expectedNodeBuildRoots) {
        $expectedVersion = [string](
            $workspaceNodeManifest.devDependencies.PSObject.Properties[$expectedNodeBuildRoot].Value
        )
        $matches = @($generatedBom.components | Where-Object {
            [string]$_.name -ceq $expectedNodeBuildRoot -and
            [string]$_.version -ceq $expectedVersion -and
            @($_.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:ecosystem' -and
                [string]$_.value -ceq 'node'
            }).Count -eq 1 -and
            @($_.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -cin @('build', 'runtime+build')
            }).Count -eq 1
        })
        if ($matches.Count -ne 1) {
            throw "Generated release SBOM omitted locked Node build root $expectedNodeBuildRoot@$expectedVersion."
        }
    }
    foreach ($emittedBuildRoot in @('svelte', 'tailwindcss', 'vite')) {
        $emittedMatches = @($generatedBom.components | Where-Object {
            [string]$_.name -ceq $emittedBuildRoot -and
            @($_.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -ceq 'runtime+build'
            }).Count -eq 1
        })
        if ($emittedMatches.Count -ne 1) {
            throw "Generated release SBOM does not classify emitted $emittedBuildRoot code as runtime+build."
        }
    }
    $linuxOnlyCargoNames = @(
        'gdk', 'gdk-sys', 'gdkwayland-sys', 'gdkx11', 'gdkx11-sys',
        'gtk', 'gtk-sys', 'gtk3-macros', 'javascriptcore-rs',
        'javascriptcore-rs-sys', 'soup3', 'soup3-sys', 'webkit2gtk', 'webkit2gtk-sys'
    )
    if (@($generatedBom.components | Where-Object {
        [string]$_.name -cin $linuxOnlyCargoNames
    }).Count -gt 0) {
        throw 'Generated Windows release SBOM includes a Linux-only Cargo target branch.'
    }
    $releaseBuilderSource = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1') -Raw
    if ($releaseBuilderSource -cnotmatch 'missing license metadata and cannot be distributed' -or
        $releaseBuilderSource -cnotmatch 'MissingLicenseComponentCount = 0' -or
        $releaseBuilderSource -cnotmatch 'component_versions = \$releaseComponentVersions' -or
        $releaseBuilderSource -cnotmatch 'windows-x64-\$artifactTrustSuffix-setup\.exe' -or
        $releaseBuilderSource -cnotmatch 'Assert-PathComponentsNotReparsePoints' -or
        $releaseBuilderSource -cnotmatch 'Stable application builds remain disabled') {
        throw 'Application release builder license, unsigned-name, or stable-signing gate is missing.'
    }
    $builderTokens = $null
    $builderErrors = $null
    $builderAst = [System.Management.Automation.Language.Parser]::ParseFile(
        (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1'),
        [ref]$builderTokens,
        [ref]$builderErrors
    )
    if ($builderErrors.Count -gt 0) {
        throw 'Application release builder does not parse for notice contract testing.'
    }
    $noticeFunctions = @($builderAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -ceq 'New-ApplicationThirdPartyNotice'
    }, $true))
    if ($noticeFunctions.Count -ne 1) {
        throw 'Application notice generator function is missing or duplicated.'
    }
    Invoke-Expression $noticeFunctions[0].Extent.Text
    $scopedApplicationNotice = Join-Path $testRoot 'APPLICATION_THIRD_PARTY_NOTICES.md'
    New-ApplicationThirdPartyNotice `
        -LatentDeckSbomPath $generatedReleaseSbom `
        -LatentPlayerSbomPath $generatedReleaseSbom `
        -RepositoryNoticePath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md') `
        -DestinationPath $scopedApplicationNotice `
        -ReleaseLabel '0.1.0-preview.1' | Out-Null
    $scopedApplicationNoticeText = Get-Content -LiteralPath $scopedApplicationNotice -Raw
    if ($scopedApplicationNoticeText -cnotmatch '^# LatentDeck applications third-party notices' -or
        $scopedApplicationNoticeText -cnotmatch '(?m)^## LatentDeck App locked component license inventory$' -or
        $scopedApplicationNoticeText -cnotmatch '(?m)^## LatentPlayer locked component license inventory$' -or
        $scopedApplicationNoticeText -cnotmatch '(?m)^## Spout2$' -or
        $scopedApplicationNoticeText -match '(?i)\b(?:taehv|taeh3|H3)\b') {
        throw 'Application third-party notice is not exact artifact scope or contains codec-only material.'
    }
    $applicationLicenseBundle = Join-Path $testRoot 'APPLICATION_THIRD_PARTY_LICENSES.json'
    $applicationLicenseResult = New-ReleaseLicenseBundle `
        -SbomPath $generatedReleaseSbom `
        -ArtifactName 'LatentDeck Windows Applications' `
        -ArtifactVersion '0.1.0-preview.1' `
        -OutputPath $applicationLicenseBundle `
        -RepositoryNoticePath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md')
    if ($applicationLicenseResult.ComponentCount -ne (@($generatedBom.components).Count + 1) -or
        $applicationLicenseResult.TextCount -lt 1 -or
        $applicationLicenseResult.NoTextDispositionCount -lt 1) {
        throw 'Application license-text bundle does not cover the exact SBOM closure.'
    }
    foreach ($installerComponentName in @('NSIS', 'nsis-tauri-utils')) {
        $matches = @($generatedBom.components | Where-Object {
            [string]$_.name -ceq $installerComponentName -and
            @($_.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -ceq 'runtime+build'
            }).Count -eq 1
        })
        if ($matches.Count -ne 1) {
            throw "Application SBOM omitted redistributed installer component $installerComponentName."
        }
    }
    foreach ($mutation in @('missing-mapping', 'scope', 'license', 'artifacts')) {
        $tamperedBundlePath = Join-Path $testRoot "APPLICATION_THIRD_PARTY_LICENSES-$mutation.json"
        $tamperedBundle = Get-Content -LiteralPath $applicationLicenseBundle -Raw |
            ConvertFrom-Json -Depth 100
        if ($mutation -ceq 'missing-mapping') {
            $tamperedBundle.components = @($tamperedBundle.components | Select-Object -Skip 1)
            $tamperedBundle.component_count = [int]$tamperedBundle.component_count - 1
        } elseif ($mutation -ceq 'scope') {
            $tamperedBundle.components[0].dependency_scope = 'runtime'
        } elseif ($mutation -ceq 'license') {
            $tamperedBundle.components[0].license_expression = 'Wrong-License'
        } else {
            $tamperedBundle.components[0].artifacts = @('Wrong Artifact')
        }
        Write-Utf8Text `
            -Path $tamperedBundlePath `
            -Content (($tamperedBundle | ConvertTo-Json -Depth 100) + "`n")
        Assert-Throws -Context "license bundle must reject $mutation drift" -Action {
            Test-ReleaseLicenseBundle `
                -BundlePath $tamperedBundlePath `
                -SbomPath $generatedReleaseSbom `
                -ExpectedArtifactName 'LatentDeck Windows Applications' `
                -ExpectedArtifactVersion '0.1.0-preview.1' | Out-Null
        }
    }
    $prebuiltSbomOutput = Join-Path $testRoot 'prebuilt-sbom-release-output'
    Assert-NativeFailureContains `
        -Context 'application release builder must reject every prebuilt SBOM input' `
        -ExpectedText 'Prebuilt SBOM input is not accepted; the release builder generates it from the current locked workspace.' `
        -Command {
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1') `
                -OutputDirectory $prebuiltSbomOutput `
                -SbomPath $generatedReleaseSbom
        }
    Assert-NativeFailureContains `
        -Context 'stable application release must remain closed until installed EXE verification exists' `
        -ExpectedText 'Stable application builds remain disabled until' `
        -Command {
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1') `
                -ReleaseChannel stable `
                -ReleaseLabel 0.1.0 `
                -SigningCommand 'signtool sign %1'
        }
    $releaseJunctionTarget = Join-Path $testRoot 'release-junction-target'
    $releaseJunctionOutput = Join-Path $testRoot 'release-junction-output'
    [System.IO.Directory]::CreateDirectory($releaseJunctionTarget) | Out-Null
    New-Item -ItemType Junction -Path $releaseJunctionOutput -Target $releaseJunctionTarget | Out-Null
    try {
        Assert-NativeFailureContains `
            -Context 'application release builder must reject output reparse ancestors' `
            -ExpectedText 'reparse-point component' `
            -Command {
                & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1') `
                    -OutputDirectory $releaseJunctionOutput `
                    -DevelopmentBuild
            }
    } finally {
        if (Test-Path -LiteralPath $releaseJunctionOutput) {
            Remove-Item -LiteralPath $releaseJunctionOutput -Force
        }
    }

    $spoutComponent = New-Spout2CycloneDxComponent
    $badSpoutComponent = $spoutComponent | ConvertTo-Json -Depth 16 | ConvertFrom-Json -Depth 16
    $badSpoutComponent.licenses[0].license.id = 'Apache-2.0'
    Assert-Throws -Context 'Spout2 SBOM component must retain BSD-2-Clause' -Action {
        Assert-Spout2CycloneDxComponent -Components @($badSpoutComponent) | Out-Null
    }
    $conflictingSpoutComponent = $spoutComponent |
        ConvertTo-Json -Depth 16 |
        ConvertFrom-Json -Depth 16
    $conflictingSpoutComponent.licenses = @(
        $conflictingSpoutComponent.licenses[0],
        [pscustomobject]@{ license = [pscustomobject]@{ id = 'Apache-2.0' } }
    )
    Assert-Throws -Context 'Spout2 SBOM component must reject conflicting licenses' -Action {
        Assert-Spout2CycloneDxComponent -Components @($conflictingSpoutComponent) | Out-Null
    }

    $noticeSource = Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md'
    $noticeSourceReceipt = Test-Spout2ThirdPartyNotice -Path $noticeSource
    $noticeStage = Join-Path $testRoot 'application-release-metadata'
    [System.IO.Directory]::CreateDirectory($noticeStage) | Out-Null
    $noticeStageReceipt = Copy-Spout2ThirdPartyNotice `
        -SourcePath $noticeSource `
        -DestinationDirectory $noticeStage
    if ($noticeStageReceipt.Sha256 -cne $noticeSourceReceipt.Sha256 -or
        $noticeStageReceipt.ByteLength -ne $noticeSourceReceipt.ByteLength -or
        [System.IO.Path]::GetFileName($noticeStageReceipt.Path) -cne 'THIRD_PARTY_NOTICES.md') {
        throw 'Application release third-party notice staging contract failed.'
    }
    Assert-Throws -Context 'staged third-party notices must not be overwritten' -Action {
        Copy-Spout2ThirdPartyNotice `
            -SourcePath $noticeSource `
            -DestinationDirectory $noticeStage | Out-Null
    }
    $badNoticePath = Join-Path $testRoot 'BAD_THIRD_PARTY_NOTICES.md'
    $badNoticeText = [System.IO.File]::ReadAllText($noticeSource).Replace(
        '- License: BSD-2-Clause',
        '- License: Apache-2.0'
    )
    Write-Utf8Text -Path $badNoticePath -Content $badNoticeText
    Assert-Throws -Context 'Spout2 notice must retain the BSD-2-Clause identity' -Action {
        Test-Spout2ThirdPartyNotice -Path $badNoticePath | Out-Null
    }
    $conflictingNoticePath = Join-Path $testRoot 'CONFLICTING_THIRD_PARTY_NOTICES.md'
    $conflictingNoticeText = [System.IO.File]::ReadAllText($noticeSource).Replace(
        '- License: BSD-2-Clause',
        "- License: BSD-2-Clause`n- License: Apache-2.0"
    )
    Write-Utf8Text -Path $conflictingNoticePath -Content $conflictingNoticeText
    Assert-Throws -Context 'Spout2 notice must reject conflicting license metadata' -Action {
        Test-Spout2ThirdPartyNotice -Path $conflictingNoticePath | Out-Null
    }
    $misplacedLegalPath = Join-Path $testRoot 'MISPLACED_SPOUT_LEGAL_TEXT.md'
    $requiredSpoutLegalLine =
        '2. Redistributions in binary form must reproduce the above copyright notice,'
    $misplacedLegalText = [System.IO.File]::ReadAllText($noticeSource).Replace(
        $requiredSpoutLegalLine,
        '2. [Spout legal text deliberately removed for contract test]'
    ) + "`n## Synthetic unrelated component`n`n$requiredSpoutLegalLine`n"
    Write-Utf8Text -Path $misplacedLegalPath -Content $misplacedLegalText
    Assert-Throws -Context 'Spout2 legal text must be contained by the Spout2 section' -Action {
        Test-Spout2ThirdPartyNotice -Path $misplacedLegalPath | Out-Null
    }

    foreach ($invalidToken in @('1variant', 'tae_h3')) {
        Assert-Throws -Context "Rust-incompatible token '$invalidToken' must be rejected" -Action {
            Assert-Token -Value $invalidToken -Name 'test token'
        }
    }

    foreach ($app in @(
        @{
            Config = 'apps/latentdeck/src-tauri/tauri.conf.json'
            Cargo = 'apps/latentdeck/src-tauri/Cargo.toml'
            Package = 'apps/latentdeck/package.json'
            Product = 'LatentDeck App'
            Identifier = 'studio.latentdeck.deck'
        },
        @{
            Config = 'apps/latentplayer/src-tauri/tauri.conf.json'
            Cargo = 'apps/latentplayer/src-tauri/Cargo.toml'
            Package = 'apps/latentplayer/package.json'
            Product = 'LatentPlayer'
            Identifier = 'studio.latentdeck.player'
        }
    )) {
        $configPath = Join-Path $repoRoot $app.Config
        $config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
        if ($config.productName -cne $app.Product -or
            $config.identifier -cne $app.Identifier -or
            $config.version -cne '0.1.0+1' -or
            $config.bundle.active -ne $true -or
            (@($config.bundle.targets) -join ',') -cne 'nsis' -or
            $config.bundle.createUpdaterArtifacts -ne $false -or
            $config.bundle.windows.allowDowngrades -ne $false -or
            $config.bundle.windows.nsis.installMode -cne 'currentUser') {
            throw "Release configuration contract failed for $($app.Product)."
        }
        $cargo = Get-Content -LiteralPath (Join-Path $repoRoot $app.Cargo) -Raw
        if ($cargo -cnotmatch '(?m)^spout-sdk\s*=') {
            throw "Spout release feature is absent for $($app.Product)."
        }
        Assert-TauriOfflineFrontendContract `
            -ConfigPath $configPath `
            -CargoManifestPath (Join-Path $repoRoot $app.Cargo) `
            -PackageJsonPath (Join-Path $repoRoot $app.Package)
    }

    $tauriFixtureRoot = Join-Path $testRoot 'tauri-frontend-contract'
    $tauriFixtureDist = Join-Path $tauriFixtureRoot 'dist'
    $tauriFixtureAssets = Join-Path $tauriFixtureDist 'assets'
    [System.IO.Directory]::CreateDirectory($tauriFixtureAssets) | Out-Null
    Write-Utf8Text `
        -Path (Join-Path $tauriFixtureDist 'index.html') `
        -Content @'
<script type="module" src="/assets/index-fixture.js"></script>
<link rel="stylesheet" href="/assets/index-fixture.css">
'@
    Write-Utf8Text `
        -Path (Join-Path $tauriFixtureAssets 'index-fixture.js') `
        -Content "document.body.dataset.releaseFixture = 'ready';`n"
    Write-Utf8Text `
        -Path (Join-Path $tauriFixtureAssets 'index-fixture.css') `
        -Content "body { color: white; }`n"
    $tauriFixtureBinary = Join-Path $tauriFixtureRoot 'fixture.exe'
    Write-Utf8Text `
        -Path $tauriFixtureBinary `
        -Content "MZ synthetic /index.html /assets/index-fixture.js /assets/index-fixture.css`n"
    Assert-TauriEmbeddedFrontendBinary `
        -BinaryPath $tauriFixtureBinary `
        -FrontendDistPath $tauriFixtureDist
    $tauriStaleBinary = Join-Path $tauriFixtureRoot 'stale-fixture.exe'
    Write-Utf8Text `
        -Path $tauriStaleBinary `
        -Content "MZ synthetic /index.html /assets/index-old.js /assets/index-fixture.css`n"
    Assert-Throws -Context 'release binary must embed the current Vite asset names' -Action {
        Assert-TauriEmbeddedFrontendBinary `
            -BinaryPath $tauriStaleBinary `
            -FrontendDistPath $tauriFixtureDist
    }

    $runtimeSource = Join-Path $testRoot 'runtime-source'
    $packageSource = Join-Path $testRoot 'package-source'
    $noticeSource = Join-Path $testRoot 'NOTICE.md'
    $inventorySource = Join-Path $testRoot 'DEPENDENCY_INVENTORY.json'
    $sbomSource = Join-Path $testRoot 'SBOM.cdx.json'
    $assetContract = Join-Path $testRoot 'decoder-asset.json'
    [System.IO.Directory]::CreateDirectory($runtimeSource) | Out-Null
    foreach ($moduleName in @(
        'latentdeck_codec_h3',
        'latentdeck_codec_host',
        'latentdeck_codec_sdk',
        'latentdeck_deck_sdk',
        'latentdeck_cartridge',
        'latentdeck_rgb_ring'
    )) {
        [System.IO.Directory]::CreateDirectory(
            (Join-Path $packageSource $moduleName)
        ) | Out-Null
        Write-Utf8Text `
            -Path (Join-Path $packageSource "$moduleName/__init__.py") `
            -Content "__version__ = '0.2.0'`n"
    }

    $python313 = Resolve-TestPython313
    $python313Root = Split-Path -Parent $python313
    $python313Dll = Join-Path $python313Root 'python313.dll'
    if (-not (Test-Path -LiteralPath $python313Dll -PathType Leaf)) {
        throw "The CPython 3.13 test installation has no python313.dll: $python313Dll"
    }
    [System.IO.File]::Copy($python313, (Join-Path $runtimeSource 'python.exe'), $false)
    [System.IO.File]::Copy($python313Dll, (Join-Path $runtimeSource 'python313.dll'), $false)
    foreach ($nativeBinding in @(
        'latentdeck_cartridge/_native.pyd',
        'latentdeck_rgb_ring/_native.cp313-win_amd64.pyd'
    )) {
        [System.IO.File]::Copy(
            $python313Dll,
            (Join-Path $packageSource $nativeBinding),
            $false
        )
    }
    Write-Utf8Text `
        -Path (Join-Path $runtimeSource 'python313._pth') `
        -Content "python313.zip`n.`nLib/site-packages`n"
    $stdlibZipPath = Join-Path $runtimeSource 'python313.zip'
    $zipStream = [System.IO.FileStream]::new(
        $stdlibZipPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $zip = [System.IO.Compression.ZipArchive]::new(
            $zipStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            $entry = $zip.CreateEntry('encodings/__init__.pyc')
            $entry.LastWriteTime = [System.DateTimeOffset]::new(
                1980, 1, 1, 0, 0, 0, [System.TimeSpan]::Zero
            )
            $entryStream = $entry.Open()
            try {
                $entryStream.Write([byte[]](0x00, 0x01, 0x02, 0x03))
            } finally {
                $entryStream.Dispose()
            }
        } finally {
            $zip.Dispose()
        }
    } finally {
        $zipStream.Dispose()
    }
    Write-Utf8Text `
        -Path (Join-Path $packageSource 'latentdeck_codec_h3/adapter.py') `
        -Content "def make_adapter():`n    raise RuntimeError('synthetic packaging fixture')`n"
    Write-Utf8Text `
        -Path (Join-Path $packageSource 'native/runtime-script.ld') `
        -Content "/* GNU ld script */`nINPUT(libportable_runtime.so)`n"
    foreach ($hostModule in @('__main__.py', 'runtime_v2.py', 'native_cartridge.py')) {
        Write-Utf8Text `
            -Path (Join-Path $packageSource "latentdeck_codec_host/$hostModule") `
            -Content "# synthetic Protocol 2 packaging fixture`n"
    }
    Write-Utf8Text `
        -Path $noticeSource `
        -Content "Temporary local CPython identity fixture. Never published or retained.`n"
    Write-Utf8Text `
        -Path $assetContract `
        -Content (@{
            asset_id = 'taeh3'
            display_name = 'TAEH3 decoder weight'
            kind = 'decoder_weight'
            required = $true
            selection = 'explicit_file'
            format = 'safetensors'
            accepted_variants = @(
                @{
                    variant_id = 'madebyollin-taeh3-62f7591f'
                    sha256 = '4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13'
                    byte_length = 22709752
                    source_url = 'https://raw.githubusercontent.com/madebyollin/taehv/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/safetensors/taeh3.safetensors'
                    license_label = 'MIT'
                    license_url = 'https://github.com/madebyollin/taehv/blob/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/LICENSE'
                }
            )
        } | ConvertTo-Json -Depth 16)

    $outputRoot = Join-Path $testRoot 'codec-artifacts'
    Write-SyntheticDependencyMetadata `
        -InventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -PackVersion '0.2.0'
    $archive020 = & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion '0.2.0' `
        -OutputDirectory $outputRoot
    $hash020 = (Get-FileHash -LiteralPath $archive020 -Algorithm SHA256).Hash.ToLowerInvariant()

    $reproOutputRoot = Join-Path $testRoot 'codec-artifacts-repro'
    $archive020Repro = & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion '0.2.0' `
        -OutputDirectory $reproOutputRoot
    $hash020Repro = (
        Get-FileHash -LiteralPath $archive020Repro -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    if ($hash020Repro -cne $hash020) {
        throw 'Identical Codec Pack inputs did not produce an identical archive SHA-256.'
    }

    $expanded020 = Join-Path $testRoot 'expanded-0.2.0'
    Expand-SafeCodecPackArchive -ArchivePath $archive020 -DestinationPath $expanded020
    $manifest020 = Get-Content -Raw -LiteralPath (
        Join-Path $expanded020 'codec-pack.json'
    ) | ConvertFrom-Json -Depth 20
    $expandedLinkerScript = Get-Content -Raw -LiteralPath (
        Join-Path $expanded020 'runtime/Lib/site-packages/native/runtime-script.ld'
    )
    if ($expandedLinkerScript -cne "/* GNU ld script */`nINPUT(libportable_runtime.so)`n") {
        throw 'Portable .ld linker-script content did not survive Codec Pack validation and expansion.'
    }

    $nativeTypingStubRoot = Join-Path $testRoot 'native-typing-stub'
    Expand-SafeCodecPackArchive -ArchivePath $archive020 -DestinationPath $nativeTypingStubRoot
    Add-CataloguedFixtureFile `
        -PackRoot $nativeTypingStubRoot `
        -RelativePath 'runtime/Lib/site-packages/latentdeck_cartridge/_native.pyi' `
        -Bytes ([System.Text.Encoding]::UTF8.GetBytes("def read_slot(index: int) -> bytes: ...`n"))
    Test-H3CodecPackDirectory -PackRoot $nativeTypingStubRoot | Out-Null

    $nativePackageShadowRoot = Join-Path $testRoot 'native-package-shadow'
    Expand-SafeCodecPackArchive -ArchivePath $archive020 -DestinationPath $nativePackageShadowRoot
    Add-CataloguedFixtureFile `
        -PackRoot $nativePackageShadowRoot `
        -RelativePath 'runtime/Lib/site-packages/latentdeck_cartridge/_native/__init__.py' `
        -Bytes ([System.Text.Encoding]::UTF8.GetBytes("raise RuntimeError('shadowed native binding')`n"))
    Assert-Throws `
        -Context 'an importable _native package must not shadow the exact native binding' `
        -Action { Test-H3CodecPackDirectory -PackRoot $nativePackageShadowRoot | Out-Null }

    $nativeSourceShadowRoot = Join-Path $testRoot 'native-source-shadow'
    Expand-SafeCodecPackArchive -ArchivePath $archive020 -DestinationPath $nativeSourceShadowRoot
    Add-CataloguedFixtureFile `
        -PackRoot $nativeSourceShadowRoot `
        -RelativePath 'runtime/Lib/site-packages/latentdeck_cartridge/_native.py' `
        -Bytes ([System.Text.Encoding]::UTF8.GetBytes("raise RuntimeError('ambiguous native binding')`n"))
    Assert-Throws `
        -Context 'an importable _native Python module must not accompany the exact native binding' `
        -Action { Test-H3CodecPackDirectory -PackRoot $nativeSourceShadowRoot | Out-Null }

    $nativeWrongTagRoot = Join-Path $testRoot 'native-wrong-tag'
    Expand-SafeCodecPackArchive -ArchivePath $archive020 -DestinationPath $nativeWrongTagRoot
    Add-CataloguedFixtureFile `
        -PackRoot $nativeWrongTagRoot `
        -RelativePath 'runtime/Lib/site-packages/latentdeck_cartridge/_native.cp312-win_amd64.pyd' `
        -Bytes ([System.IO.File]::ReadAllBytes($python313Dll))
    Assert-Throws `
        -Context 'an ABI-tagged _native alias must not accompany the exact cartridge ABI3 binding' `
        -Action { Test-H3CodecPackDirectory -PackRoot $nativeWrongTagRoot | Out-Null }

    $nativeUntaggedRoot = Join-Path $testRoot 'native-untagged'
    Expand-SafeCodecPackArchive -ArchivePath $archive020 -DestinationPath $nativeUntaggedRoot
    Add-CataloguedFixtureFile `
        -PackRoot $nativeUntaggedRoot `
        -RelativePath 'runtime/Lib/site-packages/latentdeck_rgb_ring/_native.pyd' `
        -Bytes ([System.IO.File]::ReadAllBytes($python313Dll))
    Assert-Throws `
        -Context 'an untagged _native extension must not accompany the exact RGB ring CPython 3.13 binding' `
        -Action { Test-H3CodecPackDirectory -PackRoot $nativeUntaggedRoot | Out-Null }

    $expectedWorkerArguments020 = @(
        '-I', '-s', '-B', '-m', 'latentdeck_codec_host',
        '--worker-protocol', '2',
        '--codec-pack-id', 'org.latentdeck.h3',
        '--codec-pack-version', '0.2.0',
        '--codec-adapter-id', 'org.latentdeck.h3',
        '--codec-adapter-version', '0.2.0',
        '--codec-entrypoint', 'latentdeck_codec_h3.adapter:make_adapter'
    )
    if ((@($manifest020.worker.arguments) -join "`0") -cne
        ($expectedWorkerArguments020 -join "`0")) {
        throw 'Synthetic H3 pack does not select the exact generic Protocol 2 host.'
    }
    foreach ($obsoleteWorkerField in @('d2_arguments', 'q4_arguments')) {
        if ($manifest020.worker.PSObject.Properties.Name -contains $obsoleteWorkerField) {
            throw "Codec Pack v2 retained obsolete worker.$obsoleteWorkerField."
        }
    }
    $expectedCapabilities = @(
        'player', 'realtime', 'resample', 'snapshot_capture', 'live_capture',
        'raw_import'
    )
    if ((@($manifest020.capabilities) -join "`0") -cne
        ($expectedCapabilities -join "`0") -or
        [int]$manifest020.compatibility.worker_protocol -ne 2 -or
        [int]$manifest020.compatibility.codec_adapter_api -ne 1 -or
        $manifest020.adapter.adapter_id -cne 'org.latentdeck.h3' -or
        $manifest020.adapter.adapter_version -cne '0.2.0' -or
        $manifest020.adapter.entrypoint -cne 'latentdeck_codec_h3.adapter:make_adapter') {
        throw 'Synthetic H3 Codec Pack v2 capability or adapter contract is incomplete.'
    }

    Write-SyntheticDependencyMetadata `
        -InventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -PackVersion '0.2.1'
    $archive021 = & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion '0.2.1' `
        -OutputDirectory $outputRoot
    $expanded021 = Join-Path $testRoot 'expanded-0.2.1'
    Expand-SafeCodecPackArchive -ArchivePath $archive021 -DestinationPath $expanded021
    $manifest021 = Get-Content -Raw -LiteralPath (
        Join-Path $expanded021 'codec-pack.json'
    ) | ConvertFrom-Json -Depth 20
    if ($manifest021.pack_version -cne '0.2.1' -or
        $manifest021.adapter.adapter_id -cne 'org.latentdeck.h3' -or
        $manifest021.adapter.adapter_version -cne '0.2.0') {
        throw 'Codec Pack and H3 adapter versions must remain independently versioned.'
    }

    foreach ($lifecycleScriptName in @(
        'Install-H3CodecPack.ps1',
        'Uninstall-H3CodecPack.ps1'
    )) {
        $lifecycleScript = Get-Content -Raw -LiteralPath (
            Join-Path $PSScriptRoot $lifecycleScriptName
        )
        if ($lifecycleScript -match
            '(?i)Expand-SafeCodecPackArchive|ZipArchive|Directory\]\:\:Move|Remove-SafeTemporaryDirectory' -or
            $lifecycleScript -cnotmatch 'LifecycleHelperPath' -or
            $lifecycleScript -cnotmatch 'latentdeck-codec-pack-installer') {
            throw "$lifecycleScriptName must remain a thin native lifecycle wrapper."
        }
    }
    $fullPackBuilder = Get-Content -Raw -LiteralPath (
        Join-Path $PSScriptRoot 'Build-H3CodecPack.ps1'
    )
    foreach ($requiredNativeBuildFragment in @(
        "Import-Module (Join-Path `$PSScriptRoot 'PublicNativeBuild.psm1') -Force",
        '`$savedRustFlags = `$env:RUSTFLAGS',
        '`$savedEncodedRustFlags = `$env:CARGO_ENCODED_RUSTFLAGS',
        'New-PublicRustBuildPolicy',
        'Set-PublicRustBuildPolicy -Policy `$nativeBuildPolicy',
        'ForbidEmbeddedSbom = `$true',
        'Assert-PublicProjectWheel @wheelAuditParameters',
        'Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue',
        'Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue'
    )) {
        if ($fullPackBuilder.IndexOf(
            $requiredNativeBuildFragment.Replace('`$', '$'),
            [System.StringComparison]::Ordinal
        ) -lt 0) {
            throw (
                'Build-H3CodecPack.ps1 lost its reproducible, path-hygienic native wheel ' +
                "contract fragment: $requiredNativeBuildFragment"
            )
        }
    }
    foreach ($nativeWheelProject in @(
        'sdk/python/pyproject.toml',
        'codec-host/rgb-ring/pyproject.toml'
    )) {
        $nativeWheelConfiguration = Get-Content -Raw -LiteralPath (
            Join-Path (Split-Path -Parent $PSScriptRoot) $nativeWheelProject
        )
        if ($nativeWheelConfiguration -cnotmatch
            '(?ms)^\[tool\.maturin\.sbom\]\s*^rust\s*=\s*false\s*$') {
            throw "$nativeWheelProject must disable Maturin's non-portable embedded Rust SBOM."
        }
    }
    foreach ($requiredSupplyChainFragment in @(
        "'--only-binary', ':all:', '--require-hashes', '--requirement', `$requirementsPath",
        "'--build-constraints', `$buildConstraints, '--require-hashes'",
        "'tools/packaging/windows-x64-build-constraints.txt'",
        "install_policy = 'direct_https_wheels_only_sha256_required'"
    )) {
        if ($fullPackBuilder.IndexOf(
            $requiredSupplyChainFragment,
            [System.StringComparison]::Ordinal
        ) -lt 0) {
            throw (
                'Build-H3CodecPack.ps1 lost a hash-bound wheels-only acquisition/build ' +
                "contract fragment: $requiredSupplyChainFragment"
            )
        }
    }
    $h3WindowsLockPath = Join-Path (
        Split-Path -Parent $PSScriptRoot
    ) 'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
    $h3WindowsLock = Get-Content -Raw -LiteralPath $h3WindowsLockPath |
        ConvertFrom-Json -Depth 32
    $h3Wheels = @($h3WindowsLock.dependencies)
    if ([string]$h3WindowsLock.pack_version -cne '0.2.1' -or
        $h3Wheels.Count -ne 15 -or
        @($h3Wheels | Where-Object { $null -eq $_.wheel }).Count -ne 0) {
        throw 'H3 Windows runtime lock must retain pack 0.2.1 and exactly 15 wheel identities.'
    }
    foreach ($lockedDependency in $h3Wheels) {
        $lockedWheel = $lockedDependency.wheel
        if ([string]$lockedWheel.file_name -cnotmatch '^[A-Za-z0-9_.+\-]+\.whl$' -or
            [string]$lockedWheel.url -cnotmatch '^https://' -or
            [int64]$lockedWheel.byte_length -le 0 -or
            [int64]$lockedWheel.byte_length -ge 2GB -or
            [string]$lockedWheel.sha256 -cnotmatch '^[0-9a-f]{64}$') {
            throw "H3 Windows runtime lock has an invalid wheel identity: $($lockedDependency.name)"
        }
    }
    $lockedTorch = @($h3Wheels | Where-Object { [string]$_.name -ceq 'torch' })
    if ($lockedTorch.Count -ne 1 -or
        [string]$lockedTorch[0].version -cne '2.13.0+cu130' -or
        [string]$lockedTorch[0].wheel.file_name -cne
            'torch-2.13.0+cu130-cp313-cp313-win_amd64.whl' -or
        [int64]$lockedTorch[0].wheel.byte_length -ne 1915517499 -or
        [string]$lockedTorch[0].wheel.sha256 -cne
            'cf23236e9deed7d3510d14d9b9592d75d272ef7b35bbfee31a02bea339c73971') {
        throw 'H3 Windows runtime lock lost the reviewed Torch 2.13.0+cu130 wheel identity.'
    }
    $lockedSafetensors = @($h3Wheels | Where-Object {
        [string]$_.name -ceq 'safetensors'
    })
    $safetensorsClosureLock = Get-Content -Raw -LiteralPath (Join-Path (
        Split-Path -Parent $PSScriptRoot
    ) 'comfy/latent-cartridge/packaging/safetensors-native-0.8.0.lock.json') |
        ConvertFrom-Json -Depth 32
    if ($lockedSafetensors.Count -ne 1 -or
        [string]$lockedSafetensors[0].wheel.file_name -cne
            [string]$safetensorsClosureLock.wheel.file_name -or
        [int64]$lockedSafetensors[0].wheel.byte_length -ne
            [int64]$safetensorsClosureLock.wheel.byte_length -or
        [string]$lockedSafetensors[0].wheel.sha256 -cne
            [string]$safetensorsClosureLock.wheel.sha256 -or
        [string]$lockedSafetensors[0].wheel.url -cne
            [string]$safetensorsClosureLock.wheel.url) {
        throw 'H3 and Recorder Safetensors wheel locks must remain byte-identical.'
    }
    foreach ($removedAuthorizationArgument in @(
        '--expected-sha256',
        '--expected-length',
        '--expected-version'
    )) {
        if ($fullPackBuilder.Contains($removedAuthorizationArgument)) {
            throw (
                'Build-H3CodecPack.ps1 must not pass removed runtime authorization argument ' +
                "'$removedAuthorizationArgument'; authorization is build-generated and embedded."
            )
        }
    }

    foreach ($forbiddenFixture in @(
        @{ Name = 'forbidden.safetensors'; Bytes = [byte[]](1, 2, 3) },
        @{ Name = 'latent.npy'; Bytes = [byte[]](1, 2, 3) },
        @{ Name = 'private.png'; Bytes = [byte[]](1, 2, 3) },
        @{ Name = '.env'; Bytes = [System.Text.Encoding]::UTF8.GetBytes('TOKEN=private') },
        @{ Name = 'nested.zip'; Bytes = [byte[]](1, 2, 3) },
        @{
            Name = 'embedded_secret.json'
            Bytes = [System.Text.Encoding]::UTF8.GetBytes('api_key = "1234567890abcdef"')
        },
        @{ Name = 'oversized.py'; Bytes = [byte[]]::new((4MB) + 1) }
    )) {
        Write-SyntheticDependencyMetadata `
            -InventoryPath $inventorySource `
            -SbomPath $sbomSource `
            -PackVersion '0.2.2'
        $forbiddenPath = Join-Path $packageSource $forbiddenFixture.Name
        [System.IO.File]::WriteAllBytes($forbiddenPath, $forbiddenFixture.Bytes)
        Assert-Throws -Context "Codec Pack builder must reject $($forbiddenFixture.Name)" -Action {
            & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
                -RuntimeSource $runtimeSource `
                -PackageSource $packageSource `
                -NoticeSource $noticeSource `
                -DependencyInventoryPath $inventorySource `
                -SbomPath $sbomSource `
                -DecoderAssetContractPath $assetContract `
                -PackVersion '0.2.2' `
                -OutputDirectory $outputRoot | Out-Null
        }
        Remove-Item -LiteralPath $forbiddenPath -Force
    }

    $originalStdlibZip = [System.IO.File]::ReadAllBytes($stdlibZipPath)
    try {
        Remove-Item -LiteralPath $stdlibZipPath -Force
        $oversizedZipStream = [System.IO.FileStream]::new(
            $stdlibZipPath,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        try {
            $oversizedZip = [System.IO.Compression.ZipArchive]::new(
                $oversizedZipStream,
                [System.IO.Compression.ZipArchiveMode]::Create,
                $false,
                [System.Text.Encoding]::UTF8
            )
            try {
                $oversizedEntry = $oversizedZip.CreateEntry('oversized.py')
                $oversizedEntryStream = $oversizedEntry.Open()
                try {
                    $oversizedEntryStream.Write([byte[]]::new((4MB) + 1))
                } finally {
                    $oversizedEntryStream.Dispose()
                }
            } finally {
                $oversizedZip.Dispose()
            }
        } finally {
            $oversizedZipStream.Dispose()
        }
        Assert-Throws -Context 'oversized nested Python text must be rejected' -Action {
            & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
                -RuntimeSource $runtimeSource `
                -PackageSource $packageSource `
                -NoticeSource $noticeSource `
                -DependencyInventoryPath $inventorySource `
                -SbomPath $sbomSource `
                -DecoderAssetContractPath $assetContract `
                -PackVersion '0.2.2' `
                -OutputDirectory $outputRoot | Out-Null
        }
    } finally {
        [System.IO.File]::WriteAllBytes($stdlibZipPath, $originalStdlibZip)
    }

    $badManifestRoot = Join-Path $testRoot 'bad-manifest-types'
    Expand-SafeCodecPackArchive -ArchivePath $archive021 -DestinationPath $badManifestRoot
    $badManifestPath = Join-Path $badManifestRoot 'codec-pack.json'
    $badManifest = Get-Content -LiteralPath $badManifestPath -Raw | ConvertFrom-Json
    $badManifest.worker.start_timeout_ms = '120000'
    Write-JsonFile -Value $badManifest -Path $badManifestPath
    Assert-Throws -Context 'string-for-number manifest field must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badManifestRoot | Out-Null
    }

    $badArrayRoot = Join-Path $testRoot 'bad-array-types'
    Expand-SafeCodecPackArchive -ArchivePath $archive021 -DestinationPath $badArrayRoot
    $badArrayManifestPath = Join-Path $badArrayRoot 'codec-pack.json'
    $badArrayManifest = Get-Content -LiteralPath $badArrayManifestPath -Raw | ConvertFrom-Json
    $badArrayManifest.worker.arguments = [pscustomobject]@{ invalid = 'object' }
    Write-JsonFile -Value $badArrayManifest -Path $badArrayManifestPath
    Assert-Throws -Context 'object-for-array manifest field must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badArrayRoot | Out-Null
    }

    $badCatalogRoot = Join-Path $testRoot 'bad-catalog-types'
    Expand-SafeCodecPackArchive -ArchivePath $archive021 -DestinationPath $badCatalogRoot
    $badCatalogPath = Join-Path $badCatalogRoot 'integrity.json'
    $badCatalogText = [System.IO.File]::ReadAllText($badCatalogPath)
    $badCatalogText = [regex]::new('"byte_length"\s*:\s*([0-9]+)').Replace(
        $badCatalogText,
        '"byte_length": "$1"',
        1
    )
    [System.IO.File]::WriteAllText(
        $badCatalogPath,
        $badCatalogText,
        [System.Text.UTF8Encoding]::new($false)
    )
    $badCatalogManifestPath = Join-Path $badCatalogRoot 'codec-pack.json'
    $badCatalogManifest = Get-Content -LiteralPath $badCatalogManifestPath -Raw | ConvertFrom-Json
    $badCatalogManifest.integrity.catalog_sha256 = (
        Get-FileHash -LiteralPath $badCatalogPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    Write-JsonFile -Value $badCatalogManifest -Path $badCatalogManifestPath
    Assert-Throws -Context 'string-for-number catalog field must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badCatalogRoot | Out-Null
    }

    $badUtf8ManifestRoot = Join-Path $testRoot 'bad-utf8-manifest'
    Expand-SafeCodecPackArchive -ArchivePath $archive021 -DestinationPath $badUtf8ManifestRoot
    $badUtf8ManifestPath = Join-Path $badUtf8ManifestRoot 'codec-pack.json'
    Write-InvalidUtf8InsideAsciiMarker `
        -Path $badUtf8ManifestPath `
        -Marker 'LatentDeck H3 Codec Pack'
    Assert-Throws -Context 'malformed UTF-8 manifest must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badUtf8ManifestRoot | Out-Null
    }

    $badUtf8CatalogRoot = Join-Path $testRoot 'bad-utf8-catalog'
    Expand-SafeCodecPackArchive -ArchivePath $archive021 -DestinationPath $badUtf8CatalogRoot
    $badUtf8CatalogPath = Join-Path $badUtf8CatalogRoot 'integrity.json'
    Write-InvalidUtf8InsideAsciiMarker `
        -Path $badUtf8CatalogPath `
        -Marker 'runtime/python.exe'
    $badUtf8CatalogManifestPath = Join-Path $badUtf8CatalogRoot 'codec-pack.json'
    $badUtf8CatalogManifest = Get-Content -LiteralPath $badUtf8CatalogManifestPath -Raw | ConvertFrom-Json
    $badUtf8CatalogManifest.integrity.catalog_sha256 = (
        Get-FileHash -LiteralPath $badUtf8CatalogPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    Write-JsonFile -Value $badUtf8CatalogManifest -Path $badUtf8CatalogManifestPath
    Assert-Throws -Context 'malformed UTF-8 catalog must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badUtf8CatalogRoot | Out-Null
    }

    Write-Host 'RELEASE PACKAGING CONTRACT: PASS' -ForegroundColor Green
    Write-Host 'Verified: independent NSIS config, fresh lock-bound application SBOM/no prebuilt reuse, offline embedded Tauri frontend/custom-protocol contract, pinned Spout2 SBOM/license/notice delivery, Spout release feature, strict Protocol 2 H3 manifest/worker/capability contracts, CPython x64 identity, deterministic .ldcodec archives, integrity, thin native lifecycle wrappers, and payload rejection.'
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
