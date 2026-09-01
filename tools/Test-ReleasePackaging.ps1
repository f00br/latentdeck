[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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
            version = '0.1.0'
            kind = 'repository'
            source_url = 'https://latentdeck.org/'
            license_expression = 'Apache-2.0'
            license_files = @()
            content_sha256 = ('b' * 64)
        }
    )
    Write-Utf8Text -Path $InventoryPath -Content (([ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        platform = 'windows-x86_64'
        curator = [ordered]@{
            name = 'latentdeck-codec-pack-curator'
            schema_version = 1
        }
        components = $components
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
            }
        }
        components = $sbomComponents
    } | ConvertTo-Json -Depth 16) + "`n")
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null

    $generatedReleaseSbom = Join-Path $testRoot 'latentdeck-0.1.0-sbom.cdx.json'
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') -OutputPath $generatedReleaseSbom | Out-Null
    $generatedBom = Get-Content -Raw -LiteralPath $generatedReleaseSbom |
        ConvertFrom-Json -Depth 100
    Assert-Spout2CycloneDxComponent -Components @($generatedBom.components) | Out-Null
    $prebuiltSbomOutput = Join-Path $testRoot 'prebuilt-sbom-release-output'
    Assert-NativeFailureContains `
        -Context 'application release builder must reject every prebuilt SBOM input' `
        -ExpectedText 'Prebuilt SBOM input is not accepted; the release builder generates it from the current locked workspace.' `
        -Command {
            & pwsh -NoProfile -File (Join-Path $PSScriptRoot 'Build-ReleaseCandidate.ps1') `
                -OutputDirectory $prebuiltSbomOutput `
                -SbomPath $generatedReleaseSbom
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
            $config.version -cne '0.1.0' -or
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
    [System.IO.Directory]::CreateDirectory(
        (Join-Path $packageSource 'latentdeck_codec_h3')
    ) | Out-Null

    $python313 = Resolve-TestPython313
    $python313Root = Split-Path -Parent $python313
    $python313Dll = Join-Path $python313Root 'python313.dll'
    if (-not (Test-Path -LiteralPath $python313Dll -PathType Leaf)) {
        throw "The CPython 3.13 test installation has no python313.dll: $python313Dll"
    }
    [System.IO.File]::Copy($python313, (Join-Path $runtimeSource 'python.exe'), $false)
    [System.IO.File]::Copy($python313Dll, (Join-Path $runtimeSource 'python313.dll'), $false)
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
        -Path (Join-Path $packageSource 'latentdeck_codec_h3/__init__.py') `
        -Content "__version__ = '0.1.0'`n"
    Write-Utf8Text `
        -Path (Join-Path $packageSource 'latentdeck_codec_h3/worker.py') `
        -Content "raise SystemExit('synthetic packaging fixture')`n"
    Write-Utf8Text `
        -Path (Join-Path $packageSource 'latentdeck_codec_h3/d2_worker.py') `
        -Content "raise SystemExit('synthetic packaging fixture')`n"
    Write-Utf8Text `
        -Path (Join-Path $packageSource 'latentdeck_codec_h3/q4_worker.py') `
        -Content "raise SystemExit('synthetic packaging fixture')`n"
    Write-Utf8Text `
        -Path $noticeSource `
        -Content "Temporary local CPython identity fixture. Never published or retained.`n"
    Write-Utf8Text `
        -Path $assetContract `
        -Content (@{
            asset_id = 'org.latentdeck.taeh3'
            display_name = 'TAEH3 decoder weight'
            kind = 'decoder_weight'
            required = $true
            selection = 'explicit_file'
            format = 'safetensors'
            accepted_variants = @(
                @{
                    variant_id = 'synthetic-contract-test'
                    sha256 = ('a' * 64)
                    byte_length = 1
                    source_url = 'https://example.invalid/decoder'
                    license_label = 'test-only'
                    license_url = 'https://example.invalid/license'
                }
            )
        } | ConvertTo-Json -Depth 16)

    $outputRoot = Join-Path $testRoot 'codec-artifacts'
    Write-SyntheticDependencyMetadata `
        -InventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -PackVersion '0.1.0'
    $archive010 = & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion '0.1.0' `
        -OutputDirectory $outputRoot
    $hash010 = (Get-FileHash -LiteralPath $archive010 -Algorithm SHA256).Hash.ToLowerInvariant()

    $reproOutputRoot = Join-Path $testRoot 'codec-artifacts-repro'
    $archive010Repro = & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion '0.1.0' `
        -OutputDirectory $reproOutputRoot
    $hash010Repro = (
        Get-FileHash -LiteralPath $archive010Repro -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    if ($hash010Repro -cne $hash010) {
        throw 'Identical Codec Pack inputs did not produce an identical archive SHA-256.'
    }

    $installRoot = Join-Path $testRoot 'installed'
    & (Join-Path $PSScriptRoot 'Install-H3CodecPack.ps1') `
        -ArchivePath $archive010 `
        -TrustedArchiveSha256 $hash010 `
        -InstallRoot $installRoot | Out-Null
    $installedManifest010 = Get-Content -Raw -LiteralPath (
        Join-Path $installRoot 'org.latentdeck.h3/0.1.0/codec-pack.json'
    ) | ConvertFrom-Json -Depth 20
    $workerArgumentContracts = @(
        [pscustomobject]@{
            Actual = @($installedManifest010.worker.arguments)
            Expected = @('-I', '-s', '-B', '-m', 'latentdeck_codec_h3.worker')
        }
        [pscustomobject]@{
            Actual = @($installedManifest010.worker.d2_arguments)
            Expected = @('-I', '-s', '-B', '-m', 'latentdeck_codec_h3.d2_worker')
        }
        [pscustomobject]@{
            Actual = @($installedManifest010.worker.q4_arguments)
            Expected = @('-I', '-s', '-B', '-m', 'latentdeck_codec_h3.q4_worker')
        }
    )
    foreach ($contract in $workerArgumentContracts) {
        if (($contract.Actual -join "`0") -cne ($contract.Expected -join "`0")) {
            throw 'Physical H3 worker entrypoints must disable bytecode writes with -B.'
        }
    }
    Assert-Throws -Context 'install must refuse an existing version' -Action {
        & (Join-Path $PSScriptRoot 'Install-H3CodecPack.ps1') `
            -ArchivePath $archive010 `
            -TrustedArchiveSha256 $hash010 `
            -InstallRoot $installRoot | Out-Null
    }

    Write-SyntheticDependencyMetadata `
        -InventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -PackVersion '0.1.1'
    $archive011 = & (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion '0.1.1' `
        -OutputDirectory $outputRoot
    $hash011 = (Get-FileHash -LiteralPath $archive011 -Algorithm SHA256).Hash.ToLowerInvariant()
    & (Join-Path $PSScriptRoot 'Install-H3CodecPack.ps1') `
        -ArchivePath $archive011 `
        -TrustedArchiveSha256 $hash011 `
        -InstallRoot $installRoot | Out-Null

    $installedManifest011 = Get-Content -Raw -LiteralPath (
        Join-Path $installRoot 'org.latentdeck.h3/0.1.1/codec-pack.json'
    ) | ConvertFrom-Json -Depth 20
    if ($installedManifest011.pack_version -cne '0.1.1' -or
        $installedManifest011.adapter.adapter_id -cne 'org.latentdeck.h3' -or
        $installedManifest011.adapter.adapter_version -cne '0.1.0') {
        throw 'Codec Pack and H3 adapter versions must remain independently versioned.'
    }

    $packParent = Join-Path $installRoot 'org.latentdeck.h3'
    if (-not (Test-Path -LiteralPath (Join-Path $packParent '0.1.0') -PathType Container) -or
        -not (Test-Path -LiteralPath (Join-Path $packParent '0.1.1') -PathType Container)) {
        throw 'Side-by-side Codec Pack versions were not preserved.'
    }

    & (Join-Path $PSScriptRoot 'Uninstall-H3CodecPack.ps1') `
        -PackVersion '0.1.0' `
        -InstallRoot $installRoot | Out-Null
    if (Test-Path -LiteralPath (Join-Path $packParent '0.1.0')) {
        throw 'Version-scoped uninstall left the selected Codec Pack version behind.'
    }
    if (-not (Test-Path -LiteralPath (Join-Path $packParent '0.1.1') -PathType Container)) {
        throw 'Version-scoped uninstall removed another Codec Pack version.'
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
            -PackVersion '0.1.2'
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
                -PackVersion '0.1.2' `
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
                -PackVersion '0.1.2' `
                -OutputDirectory $outputRoot | Out-Null
        }
    } finally {
        [System.IO.File]::WriteAllBytes($stdlibZipPath, $originalStdlibZip)
    }

    $badManifestRoot = Join-Path $testRoot 'bad-manifest-types'
    Expand-SafeCodecPackArchive -ArchivePath $archive011 -DestinationPath $badManifestRoot
    $badManifestPath = Join-Path $badManifestRoot 'codec-pack.json'
    $badManifest = Get-Content -LiteralPath $badManifestPath -Raw | ConvertFrom-Json
    $badManifest.worker.probe_timeout_ms = '120000'
    Write-JsonFile -Value $badManifest -Path $badManifestPath
    Assert-Throws -Context 'string-for-number manifest field must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badManifestRoot | Out-Null
    }

    $badArrayRoot = Join-Path $testRoot 'bad-array-types'
    Expand-SafeCodecPackArchive -ArchivePath $archive011 -DestinationPath $badArrayRoot
    $badArrayManifestPath = Join-Path $badArrayRoot 'codec-pack.json'
    $badArrayManifest = Get-Content -LiteralPath $badArrayManifestPath -Raw | ConvertFrom-Json
    $badArrayManifest.worker.arguments = [pscustomobject]@{ invalid = 'object' }
    Write-JsonFile -Value $badArrayManifest -Path $badArrayManifestPath
    Assert-Throws -Context 'object-for-array manifest field must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badArrayRoot | Out-Null
    }

    $badCatalogRoot = Join-Path $testRoot 'bad-catalog-types'
    Expand-SafeCodecPackArchive -ArchivePath $archive011 -DestinationPath $badCatalogRoot
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
    Expand-SafeCodecPackArchive -ArchivePath $archive011 -DestinationPath $badUtf8ManifestRoot
    $badUtf8ManifestPath = Join-Path $badUtf8ManifestRoot 'codec-pack.json'
    Write-InvalidUtf8InsideAsciiMarker `
        -Path $badUtf8ManifestPath `
        -Marker 'LatentDeck H3 Codec Pack'
    Assert-Throws -Context 'malformed UTF-8 manifest must be rejected' -Action {
        Test-H3CodecPackDirectory -PackRoot $badUtf8ManifestRoot | Out-Null
    }

    $badUtf8CatalogRoot = Join-Path $testRoot 'bad-utf8-catalog'
    Expand-SafeCodecPackArchive -ArchivePath $archive011 -DestinationPath $badUtf8CatalogRoot
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

    $discoveryChildren = @(
        Get-ChildItem -LiteralPath $packParent -Force -Directory |
            Select-Object -ExpandProperty Name
    )
    if (@($discoveryChildren | Where-Object { $_ -notmatch '^[0-9]+\.[0-9]+\.[0-9]+' }).Count -gt 0) {
        throw 'Install/removal work directories leaked into Codec Pack discovery.'
    }

    & (Join-Path $PSScriptRoot 'Uninstall-H3CodecPack.ps1') `
        -PackVersion '0.1.1' `
        -InstallRoot $installRoot `
        -CleanupQuarantine | Out-Null
    if (-not (Test-Path -LiteralPath (Join-Path $packParent '0.1.1') -PathType Container)) {
        throw 'CleanupQuarantine uninstalled a healthy reinstalled Codec Pack version.'
    }

    & (Join-Path $PSScriptRoot 'Uninstall-H3CodecPack.ps1') `
        -PackVersion '0.1.1' `
        -InstallRoot $installRoot | Out-Null

    foreach ($auxiliaryKind in @('Staging', 'Trash')) {
        $auxiliaryRoot = Get-CodecPackAuxiliaryRoot `
            -InstallRoot $installRoot `
            -Kind $auxiliaryKind
        if ((Test-Path -LiteralPath $auxiliaryRoot) -and
            @(Get-ChildItem -LiteralPath $auxiliaryRoot -Force).Count -gt 0) {
            throw "Codec Pack $auxiliaryKind work root was not cleaned."
        }
    }

    Write-Host 'RELEASE PACKAGING CONTRACT: PASS' -ForegroundColor Green
    Write-Host 'Verified: independent NSIS config, fresh lock-bound application SBOM/no prebuilt reuse, offline embedded Tauri frontend/custom-protocol contract, pinned Spout2 SBOM/license/notice delivery, Spout release feature, strict JSON types, CPython x64 identity, out-of-discovery staging, integrity, side-by-side install, exact-version uninstall, payload rejection.'
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
