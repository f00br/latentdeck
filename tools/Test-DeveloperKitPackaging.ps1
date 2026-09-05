[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".developer-kit-contract-$([guid]::NewGuid().ToString('N'))"
$outputRoot = Join-Path $testRoot 'output'
$expandedRoot = Join-Path $testRoot 'expanded'
$releaseLabel = '0.1.0-preview.1'
$sourceBranchOutput = @(& git -C $repositoryRoot branch --show-current)
if ($LASTEXITCODE -ne 0) {
    throw 'Developer Kit test could not resolve the source branch.'
}
$sourceBranch = ($sourceBranchOutput -join '').Trim()
$sourceStatus = @(& git -C $repositoryRoot status --short --untracked-files=all)
if ($LASTEXITCODE -ne 0) {
    throw 'Developer Kit test could not resolve the source status.'
}
$expectedDistributable = (
    $sourceBranch -ceq 'main' -and $sourceStatus.Count -eq 0
)
$developerKitMode = @{}
if (-not $expectedDistributable) {
    $developerKitMode.DevelopmentBuild = $true
}

function Assert-Throws {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$ExpectedText
    )

    try {
        & $Action
    } catch {
        if ($_.Exception.Message -notlike "*$ExpectedText*") {
            throw "Unexpected failure: $($_.Exception.Message)"
        }
        return
    }
    throw "Expected failure containing '$ExpectedText'."
}

function Test-Sums {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string[]]$ExpectedPaths
    )

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or $item.Length -gt 1MB) {
        throw "Developer Kit checksum manifest is not a bounded regular file: $Path"
    }
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($line in @(Get-Content -LiteralPath $Path)) {
        if ($line.Length -gt 4096) {
            throw "Developer Kit checksum line exceeds its bound: $Path"
        }
        if ($line -cnotmatch '^(?<hash>[0-9a-f]{64})  (?<path>[^\r\n]+)$') {
            throw "Developer Kit checksum line is invalid: $line"
        }
        $expectedHash = $Matches.hash
        $canonical = $Matches.path.Replace('\', '/')
        if (-not $seen.Add($canonical)) {
            throw "Developer Kit checksum path is duplicated: $canonical"
        }
        $relative = $canonical.Replace('/', '\')
        $candidate = [System.IO.Path]::GetFullPath((Join-Path $Root $relative))
        $rootPrefix = [System.IO.Path]::GetFullPath($Root).TrimEnd('\') + '\'
        if (-not $candidate.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            throw "Developer Kit checksum target is missing or out of tree: $relative"
        }
        $actualHash = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -cne $expectedHash) {
            throw "Developer Kit checksum mismatch: $relative"
        }
    }
    $expected = @($ExpectedPaths | ForEach-Object { $_.Replace('\', '/') } | Sort-Object -Unique)
    $actual = @($seen | Sort-Object)
    if (($expected -join "`0") -cne ($actual -join "`0")) {
        throw 'Developer Kit checksum manifest does not have exact expected coverage.'
    }
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null
    foreach ($caseVariant in @('UNSIGNED_PREVIEW', 'Unsigned_Preview')) {
        Assert-Throws -ExpectedText 'ReleaseChannel must be exactly unsigned_preview or stable.' -Action {
            & (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') `
                -ComfyRecorderArtifactDirectory (Join-Path $testRoot 'unused-recorder') `
                -OutputDirectory (Join-Path $testRoot "case-$caseVariant") `
                -ReleaseChannel $caseVariant `
                -DevelopmentBuild
        }
    }

    $recorderLock = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'comfy/latent-cartridge/packaging/windows-x64.lock.json'
    ) -Raw | ConvertFrom-Json -Depth 32
    $recorderWheelPath = Join-Path $testRoot ([string]$recorderLock.safetensors.file_name)
    Invoke-WebRequest -Uri ([string]$recorderLock.safetensors.url) -OutFile $recorderWheelPath
    if ((Get-Item -LiteralPath $recorderWheelPath).Length -ne
            [int64]$recorderLock.safetensors.byte_length -or
        (Get-FileHash -LiteralPath $recorderWheelPath -Algorithm SHA256).Hash.ToLowerInvariant() -cne
            [string]$recorderLock.safetensors.sha256) {
        throw 'Developer Kit test did not acquire the exact pinned Safetensors wheel.'
    }
    $recorderArtifactRoot = Join-Path $testRoot 'comfy-recorder'
    & (Join-Path $PSScriptRoot 'Build-ComfyRecorderBundle.ps1') `
        -OutputDirectory $recorderArtifactRoot `
        -SafetensorsWheelPath $recorderWheelPath `
        -AllowDirtySource | Out-Null
    if ($LASTEXITCODE -ne 0 -or
        -not (Test-Path -LiteralPath $recorderArtifactRoot -PathType Container)) {
        throw 'Developer Kit test could not build its exact Comfy Recorder input.'
    }
    $recorderBaseName = "LatentDeck-$releaseLabel-comfy-recorder-windows-x64"
    $recorderReceiptPath = Join-Path $recorderArtifactRoot "$recorderBaseName.receipt.json"
    $recorderReceiptOriginal = [System.IO.File]::ReadAllText($recorderReceiptPath)
    $recorderReceiptMutation = $recorderReceiptOriginal | ConvertFrom-Json -Depth 100
    $recorderReceiptMutation.source.git_tree = 'd' * 40
    [System.IO.File]::WriteAllText(
        $recorderReceiptPath,
        (($recorderReceiptMutation | ConvertTo-Json -Depth 100) + "`n"),
        [System.Text.UTF8Encoding]::new($false)
    )
    Assert-Throws -ExpectedText 'release/source identity' -Action {
        & (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') `
            -ComfyRecorderArtifactDirectory $recorderArtifactRoot `
            -OutputDirectory (Join-Path $testRoot 'recorder-source-output') `
            -ReleaseChannel unsigned_preview `
            -ReleaseLabel $releaseLabel `
            @developerKitMode
    }
    [System.IO.File]::WriteAllText(
        $recorderReceiptPath,
        $recorderReceiptOriginal,
        [System.Text.UTF8Encoding]::new($false)
    )
    $recorderReviewPath = Join-Path $recorderArtifactRoot "$recorderBaseName-license-review.json"
    $recorderReviewOriginal = [System.IO.File]::ReadAllText($recorderReviewPath)
    [System.IO.File]::WriteAllText(
        $recorderReviewPath,
        ($recorderReviewOriginal + ' '),
        [System.Text.UTF8Encoding]::new($false)
    )
    Assert-Throws -ExpectedText 'differs from its receipt binding' -Action {
        & (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') `
            -ComfyRecorderArtifactDirectory $recorderArtifactRoot `
            -OutputDirectory (Join-Path $testRoot 'recorder-sidecar-output') `
            -ReleaseChannel unsigned_preview `
            -ReleaseLabel $releaseLabel `
            @developerKitMode
    }
    [System.IO.File]::WriteAllText(
        $recorderReviewPath,
        $recorderReviewOriginal,
        [System.Text.UTF8Encoding]::new($false)
    )
    $junctionTarget = Join-Path $testRoot 'junction-target'
    $junctionOutput = Join-Path $testRoot 'junction-output'
    [System.IO.Directory]::CreateDirectory($junctionTarget) | Out-Null
    New-Item -ItemType Junction -Path $junctionOutput -Target $junctionTarget | Out-Null
    try {
        Assert-Throws -ExpectedText 'reparse-point component' -Action {
            & (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') `
                -ComfyRecorderArtifactDirectory $recorderArtifactRoot `
                -OutputDirectory $junctionOutput `
                -ReleaseChannel unsigned_preview `
                -ReleaseLabel $releaseLabel `
                @developerKitMode
        }
    } finally {
        if (Test-Path -LiteralPath $junctionOutput) {
            Remove-Item -LiteralPath $junctionOutput -Force
        }
    }
    $result = @(& (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') `
        -ComfyRecorderArtifactDirectory $recorderArtifactRoot `
        -OutputDirectory $outputRoot `
        -ReleaseChannel unsigned_preview `
        -ReleaseLabel $releaseLabel `
        @developerKitMode)
    if ($result.Count -ne 1 -or -not (Test-Path -LiteralPath $result[0] -PathType Container)) {
        throw 'Developer Kit builder did not return exactly one artifact-set directory.'
    }
    $artifactRoot = (Resolve-Path -LiteralPath $result[0]).Path
    $archiveName = "LatentDeck-$releaseLabel-developer-kit-windows-x64.zip"
    $recorderArchiveName = "$recorderBaseName.zip"
    $recorderReceiptName = "$recorderBaseName.receipt.json"
    $recorderSourceReceipt = Get-Content -LiteralPath (
        Join-Path $recorderArtifactRoot $recorderReceiptName
    ) -Raw | ConvertFrom-Json -Depth 100
    $recorderSourceReceiptItem = Get-Item -LiteralPath (
        Join-Path $recorderArtifactRoot $recorderReceiptName
    )
    $expectedOuter = @(
        $archiveName,
        'developer-kit.json',
        'LICENSE-REVIEW.json',
        'SBOM.cdx.json',
        'SHA256SUMS.txt',
        'THIRD_PARTY_LICENSES.json',
        'THIRD_PARTY_NOTICES.md'
    ) | Sort-Object
    $actualOuter = @(
        Get-ChildItem -LiteralPath $artifactRoot -File -Force |
            Select-Object -ExpandProperty Name |
            Sort-Object
    )
    if (($actualOuter -join "`0") -cne ($expectedOuter -join "`0")) {
        throw 'Developer Kit artifact set does not match its exact allowlist.'
    }
    Test-Sums `
        -Root $artifactRoot `
        -Path (Join-Path $artifactRoot 'SHA256SUMS.txt') `
        -ExpectedPaths @(
            $archiveName, 'LICENSE-REVIEW.json', 'SBOM.cdx.json',
            'THIRD_PARTY_LICENSES.json', 'THIRD_PARTY_NOTICES.md'
        )

    $receipt = Get-Content -LiteralPath (Join-Path $artifactRoot 'developer-kit.json') -Raw |
        ConvertFrom-Json -Depth 100
    if ([int]$receipt.schema_version -ne 2 -or
        $receipt.release_label -cne $releaseLabel -or
        $receipt.release_channel -cne 'unsigned_preview' -or
        $receipt.application_api_version -cne '0.1.0' -or
        $receipt.windows_installer_version -cne '0.1.0+1' -or
        [bool]$receipt.distributable -ne $expectedDistributable -or
        [bool]$receipt.signed -or -not [bool]$receipt.unsigned -or
        [int]$receipt.wheel_count -ne 9 -or [int]$receipt.cli_count -ne 2 -or
        [bool]$receipt.contains_model_weights -or [bool]$receipt.contains_cartridges -or
        $receipt.license_review.status -cne 'complete' -or
        [int]$receipt.license_review.missing_license_component_count -ne 0) {
        throw 'Developer Kit receipt identity, content boundary, or license review is invalid.'
    }

    Add-Type -AssemblyName System.IO.Compression
    $zip = [System.IO.Compression.ZipFile]::OpenRead((Join-Path $artifactRoot $archiveName))
    try {
        $entryNames = @($zip.Entries | ForEach-Object FullName)
        $sortedNames = @($entryNames | Sort-Object -CaseSensitive)
        if (($entryNames -join "`0") -cne ($sortedNames -join "`0") -or
            @($zip.Entries | Where-Object {
                $_.LastWriteTime.DateTime -ne [datetime]::new(1980, 1, 1, 0, 0, 0)
            }).Count -gt 0) {
            throw 'Developer Kit ZIP entry order or normalized timestamp is not deterministic.'
        }
        foreach ($required in @(
            'DEVELOPER-KIT-MANIFEST.json',
            'COMPATIBILITY.json',
            'LICENSE',
            'LICENSE-REVIEW.json',
            'README.md',
            'SBOM.cdx.json',
            'SHA256SUMS.txt',
            'THIRD_PARTY_LICENSES.json',
            'THIRD_PARTY_NOTICES.md',
            'bin/latentdeck-cartridge.exe',
            'bin/latentdeck-extension-manager.exe',
            'bootstrap/Install-ProjectWheels.ps1',
            "bundles/$recorderArchiveName",
            'schemas/comfy__toolkit__src__latentdeck_comfy_toolkit__operator-descriptor.schema.json'
        )) {
            if ($entryNames -cnotcontains $required) {
                throw "Developer Kit archive is missing required entry: $required"
            }
        }
        if (@($entryNames | Where-Object { $_ -like 'wheels/*.whl' }).Count -ne 9 -or
            @($entryNames | Where-Object { $_ -like 'schemas/*.schema.json' }).Count -ne 7 -or
            @($entryNames | Where-Object { $_ -match '(?i)\.(?:lc|h3latent|safetensors|ckpt|pt|pth|onnx|engine|gguf|mp4|mov|mkv|webm)$' }).Count -gt 0) {
            throw 'Developer Kit wheel/schema count or forbidden-payload boundary is invalid.'
        }
    } finally {
        $zip.Dispose()
    }

    Expand-Archive -LiteralPath (Join-Path $artifactRoot $archiveName) -DestinationPath $expandedRoot
    $kitManifest = Get-Content -LiteralPath (Join-Path $expandedRoot 'DEVELOPER-KIT-MANIFEST.json') -Raw |
        ConvertFrom-Json -Depth 100
    $compatibility = Get-Content -LiteralPath (Join-Path $expandedRoot 'COMPATIBILITY.json') -Raw |
        ConvertFrom-Json -Depth 100
    $sbom = Get-Content -LiteralPath (Join-Path $expandedRoot 'SBOM.cdx.json') -Raw |
        ConvertFrom-Json -Depth 100
    if (@($kitManifest.wheels).Count -ne 9 -or @($kitManifest.clis).Count -ne 2 -or
        $kitManifest.application_api_version -cne '0.1.0' -or
        $compatibility.application_api_version -cne '0.1.0' -or
        $compatibility.windows_installer_version -cne '0.1.0+1' -or
        $compatibility.python.supported_series -cne '3.13' -or
        $sbom.metadata.component.name -cne 'LatentDeck Developer Kit' -or
        $sbom.metadata.component.version -cne $releaseLabel -or
        @($sbom.metadata.component.licenses).Count -ne 1 -or
        [string]$sbom.metadata.component.licenses[0].license.name -cne 'Apache-2.0' -or
        @($sbom.components).Count -eq 0) {
        throw 'Developer Kit manifest, compatibility matrix, or artifact SBOM is invalid.'
    }
    foreach ($container in @($kitManifest, $receipt)) {
        $bundle = $container.comfy_recorder_bundle
        if ([string]$bundle.artifact_kind -cne 'comfy_recorder_bundle' -or
            [string]$bundle.archive.path -cne "bundles/$recorderArchiveName" -or
            [string]$bundle.archive.file_name -cne $recorderArchiveName -or
            [int64]$bundle.archive.byte_length -ne [int64]$recorderSourceReceipt.archive.byte_length -or
            [string]$bundle.archive.sha256 -cne [string]$recorderSourceReceipt.archive.sha256 -or
            [string]$bundle.standalone_receipt.file_name -cne $recorderReceiptName -or
            [int64]$bundle.standalone_receipt.byte_length -ne
                [int64]$recorderSourceReceiptItem.Length -or
            [string]$bundle.standalone_receipt.sha256 -cne
                (Get-FileHash -LiteralPath $recorderSourceReceiptItem.FullName -Algorithm SHA256).
                    Hash.ToLowerInvariant() -or
            [string]$bundle.python_abi -cne 'cp312-abi3' -or
            (@($bundle.supported_python) -join "`0") -cne (@('cp312', 'cp313') -join "`0") -or
            @($bundle.packages).Count -ne 3) {
            throw 'Developer Kit does not bind the exact standalone Comfy Recorder artifact.'
        }
    }
    $nestedRecorder = Get-Item -LiteralPath (
        Join-Path $expandedRoot "bundles/$recorderArchiveName"
    )
    if ([int64]$nestedRecorder.Length -ne [int64]$recorderSourceReceipt.archive.byte_length -or
        (Get-FileHash -LiteralPath $nestedRecorder.FullName -Algorithm SHA256).
            Hash.ToLowerInvariant() -cne [string]$recorderSourceReceipt.archive.sha256 -or
        [string]$compatibility.python.comfy_recorder.python_abi -cne 'cp312-abi3' -or
        (@($compatibility.python.comfy_recorder.supported_abis) -join "`0") -cne
            (@('cp312', 'cp313') -join "`0")) {
        throw 'Developer Kit nested Recorder bytes or compatibility record drifted.'
    }
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
    $allowedDependencyScopes = @('artifact', 'runtime', 'build', 'runtime+build')
    $invalidScopedComponents = @(
        foreach ($component in @($sbom.metadata.component) + @($sbom.components)) {
            $scopes = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope'
            })
            if ($scopes.Count -ne 1 -or
                [string]$scopes[0].value -cnotin $allowedDependencyScopes) {
                "$($component.name)@$($component.version)"
            }
        }
    )
    if ($includedScopePolicy.Count -ne 1 -or $excludedScopePolicy.Count -ne 1 -or
        $targetPlatformPolicy.Count -ne 1 -or $invalidScopedComponents.Count -ne 0) {
        throw 'Developer Kit SBOM dependency scope or Windows target classification is invalid.'
    }
    $linuxOnlyCargoNames = @(
        'gdk', 'gdk-sys', 'gdkwayland-sys', 'gdkx11', 'gdkx11-sys',
        'gtk', 'gtk-sys', 'gtk3-macros', 'javascriptcore-rs',
        'javascriptcore-rs-sys', 'soup3', 'soup3-sys', 'webkit2gtk', 'webkit2gtk-sys'
    )
    if (@($sbom.components | Where-Object {
        [string]$_.name -cin $linuxOnlyCargoNames
    }).Count -gt 0) {
        throw 'Developer Kit Windows SBOM includes a Linux-only Cargo target branch.'
    }
    $h3Lock = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
    ) -Raw | ConvertFrom-Json -Depth 100
    $d2Deck = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'operators/builtin/d2/package/deck-pack.json'
    ) -Raw | ConvertFrom-Json -Depth 32
    $q4Deck = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'operators/builtin/q4/package/deck-pack.json'
    ) -Raw | ConvertFrom-Json -Depth 32
    $torch = @($h3Lock.dependencies | Where-Object { [string]$_.name -ceq 'torch' })
    $h3Pyproject = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'codec-host/codecs/h3/pyproject.toml'
    ) -Raw
    $h3VersionMatches = [regex]::Matches(
        $h3Pyproject,
        '(?m)^version\s*=\s*"(?<value>[^"\r\n]+)"\s*$'
    )
    $compatibilityWheels = @(
        $compatibility.project_wheels |
            ForEach-Object { "$($_.name)@$($_.version)" } |
            Sort-Object
    )
    $manifestWheels = @(
        $kitManifest.wheels |
            ForEach-Object { "$($_.name)@$($_.version)" } |
            Sort-Object
    )
    if ($torch.Count -ne 1 -or
        [string]$compatibility.h3_codec.pack_version -cne [string]$h3Lock.pack_version -or
        $h3VersionMatches.Count -ne 1 -or
        [string]$compatibility.h3_codec.adapter_version -cne
            [string]$h3VersionMatches[0].Groups['value'].Value -or
        [string]$compatibility.torch.h3_runtime_exact_build -cne [string]$torch[0].version -or
        [string]$compatibility.python.h3_runtime_version -cne
            [string]$h3Lock.python_runtime.version -or
        [int]$compatibility.worker_protocol_versions[0] -ne [int]$h3Lock.worker_protocol -or
        [int]$compatibility.deck_package_operator_host_api_version -ne
            [int]$d2Deck.compatibility.deck_operator_api -or
        [string]$compatibility.operator_descriptor_schema_version -cne '0.1.0' -or
        [int]$compatibility.codec_adapter_api_version -ne [int]$h3Lock.codec_adapter_api -or
        [string]$compatibility.sdks.deck -cne
            [string]$(@($kitManifest.wheels | Where-Object name -ceq 'latentdeck-deck-sdk')[0].version) -or
        [string]$compatibility.decks.d2.deck_id -cne [string]$d2Deck.deck_id -or
        [string]$compatibility.decks.d2.deck_version -cne [string]$d2Deck.deck_version -or
        [string]$compatibility.decks.q4.deck_id -cne [string]$q4Deck.deck_id -or
        [string]$compatibility.decks.q4.deck_version -cne [string]$q4Deck.deck_version -or
        [string]$compatibility.python_operator_packages.d2.distribution -cne
            'latentdeck-operator-d2' -or
        [string]$compatibility.python_operator_packages.d2.version -cne
            [string]$(@($kitManifest.wheels | Where-Object name -ceq 'latentdeck-operator-d2')[0].version) -or
        [string]$compatibility.python_operator_packages.q4.distribution -cne
            'latentdeck-operator-q4' -or
        [string]$compatibility.python_operator_packages.q4.version -cne
            [string]$(@($kitManifest.wheels | Where-Object name -ceq 'latentdeck-operator-q4')[0].version) -or
        ($compatibilityWheels -join "`0") -cne ($manifestWheels -join "`0")) {
        throw 'Developer Kit compatibility manifest drifted from H3 lock or wheel identities.'
    }
    $operatorSchema = Get-Content -LiteralPath (
        Join-Path $expandedRoot 'schemas/comfy__toolkit__src__latentdeck_comfy_toolkit__operator-descriptor.schema.json'
    ) -Raw | ConvertFrom-Json -Depth 100
    if ([string]$operatorSchema.'$id' -cne
        'https://raw.githubusercontent.com/f00br/latentdeck/main/comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json') {
        throw 'Developer Kit Operator descriptor schema does not retain its canonical GitHub identity.'
    }
    $sbomRoots = @(
        foreach ($component in @($sbom.components)) {
            $rootMarker = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:selection-root' -and
                [string]$_.value -ceq 'true'
            })
            if ($rootMarker.Count -eq 0) {
                continue
            }
            $ecosystem = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:ecosystem'
            })
            if ($rootMarker.Count -ne 1 -or $ecosystem.Count -ne 1) {
                throw "Developer Kit SBOM selection root is ambiguous: $($component.name)"
            }
            "$($ecosystem[0].value):$($component.name)@$($component.version)"
        }
    ) | Sort-Object
    $expectedSbomRoots = @(
        $kitManifest.wheels | ForEach-Object { "python:$($_.name)@$($_.version)" }
        'rust:latentdeck-cartridge@0.1.0'
        'rust:latentdeck-cartridge-python@0.1.0'
        'rust:latentdeck-extension-manager@0.1.0'
        'python:maturin@1.15.0'
        'python:uv-build@0.12.7'
    ) | Sort-Object
    if (($sbomRoots -join "`0") -cne ($expectedSbomRoots -join "`0") -or
        $sbomRoots.Count -ne 14 -or
        [int]$receipt.sbom.selection_root_count -ne 14 -or
        (@($receipt.sbom.selection_roots | Sort-Object) -join "`0") -cne
            ($expectedSbomRoots -join "`0")) {
        throw 'Developer Kit SBOM does not prove exact wheel/native/CLI/build-backend root coverage.'
    }
    $buildBackendRoots = @($sbom.components | Where-Object {
        [string]$_.name -cin @('maturin', 'uv-build') -and
        @($_.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope' -and
            [string]$_.value -ceq 'build'
        }).Count -eq 1
    })
    if ($buildBackendRoots.Count -ne 2) {
        throw 'Developer Kit SBOM does not explicitly classify its exact build backends.'
    }
    $componentsWithoutLicense = @(
        @($sbom.metadata.component) + @($sbom.components) | Where-Object {
            $null -eq $_.PSObject.Properties['licenses'] -or @($_.licenses).Count -eq 0
        }
    )
    if ($componentsWithoutLicense.Count -gt 0) {
        throw 'Developer Kit SBOM contains components without reviewed license metadata.'
    }
    $developerNotices = Get-Content -LiteralPath (Join-Path $expandedRoot 'THIRD_PARTY_NOTICES.md') -Raw
    if ($developerNotices -cnotmatch '^# LatentDeck Developer Kit third-party notices' -or
        $developerNotices -match '(?i)\b(?:Spout2|taehv|taeh3)\b') {
        throw 'Developer Kit notices are not scoped to the Developer Kit payload.'
    }
    $licenseBundlePath = Join-Path $expandedRoot 'THIRD_PARTY_LICENSES.json'
    $licenseBundleResult = Test-ReleaseLicenseBundle `
        -BundlePath $licenseBundlePath `
        -SbomPath (Join-Path $expandedRoot 'SBOM.cdx.json') `
        -ExpectedArtifactName 'LatentDeck Developer Kit' `
        -ExpectedArtifactVersion $releaseLabel
    if ([int]$receipt.license_bundle.component_count -ne $licenseBundleResult.ComponentCount -or
        [int]$receipt.license_bundle.text_count -ne $licenseBundleResult.TextCount -or
        [string]$receipt.license_bundle.sha256 -cne $licenseBundleResult.Sha256) {
        throw 'Developer Kit receipt does not bind the exact license-text bundle.'
    }
    foreach ($mutation in @('missing-mapping', 'scope', 'artifacts')) {
        $tamperedLicensePath = Join-Path $testRoot "THIRD_PARTY_LICENSES-$mutation.json"
        $tamperedLicense = Get-Content -LiteralPath $licenseBundlePath -Raw | ConvertFrom-Json -Depth 100
        if ($mutation -ceq 'missing-mapping') {
            $tamperedLicense.components = @($tamperedLicense.components | Select-Object -Skip 1)
            $tamperedLicense.component_count = [int]$tamperedLicense.component_count - 1
        } elseif ($mutation -ceq 'scope') {
            $tamperedLicense.components[0].dependency_scope = 'runtime'
        } else {
            $tamperedLicense.components[0].artifacts = @('Wrong Artifact')
        }
        [System.IO.File]::WriteAllText(
            $tamperedLicensePath,
            (($tamperedLicense | ConvertTo-Json -Depth 100) + "`n"),
            [System.Text.UTF8Encoding]::new($false)
        )
        Assert-Throws -ExpectedText 'mapping' -Action {
            Test-ReleaseLicenseBundle `
                -BundlePath $tamperedLicensePath `
                -SbomPath (Join-Path $expandedRoot 'SBOM.cdx.json') `
                -ExpectedArtifactName 'LatentDeck Developer Kit' `
                -ExpectedArtifactVersion $releaseLabel | Out-Null
        }
    }
    $builderSource = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') -Raw
    if ($builderSource -cnotmatch 'AllowedFiles' -or
        $builderSource -cnotmatch 'unapproved file' -or
        $builderSource -cnotmatch 'ReparsePoint' -or
        $builderSource -cnotmatch '--build-constraints\s+\$buildConstraintsPath\s+--require-hashes') {
        throw 'Developer Kit reviewed-tree allowlist/reparse contract is missing.'
    }
    $buildConstraints = @(
        Get-Content -LiteralPath (
            Join-Path $repositoryRoot 'tools/packaging/windows-x64-build-constraints.txt'
        ) |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and -not $_.StartsWith('#') }
    )
    $expectedBuildConstraints = @(
        'maturin==1.15.0 --hash=sha256:552c2be4afd43fe8d5c9f3ec8d4c4756d973b8dcbe94c14084390301f50243e1',
        'uv-build==0.12.7 --hash=sha256:2c0baba9f1f1dfbfcb3dede01d04e7e9b94062b00aaf9880dcde73bd5e5c127b'
    )
    if ((@($buildConstraints | Sort-Object -CaseSensitive) -join "`0") -cne
        (@($expectedBuildConstraints | Sort-Object -CaseSensitive) -join "`0")) {
        throw 'Developer Kit build backends are not constrained by the exact reviewed hashes.'
    }
    Test-Sums `
        -Root $expandedRoot `
        -Path (Join-Path $expandedRoot 'SHA256SUMS.txt') `
        -ExpectedPaths @($kitManifest.content | ForEach-Object { [string]$_.path })

    $moduleByProject = [ordered]@{
        'latentdeck-cartridge' = 'latentdeck_cartridge/'
        'latentdeck-codec-sdk' = 'latentdeck_codec_sdk/'
        'latentdeck-deck-sdk' = 'latentdeck_deck_sdk/'
        'latentdeck-codec-host' = 'latentdeck_codec_host/'
        'latentdeck-operator-d2' = 'latentdeck_operator_d2/'
        'latentdeck-operator-q4' = 'latentdeck_operator_q4/'
        'latentdeck-comfy-toolkit' = 'latentdeck_comfy_toolkit/'
        'latentdeck-comfy-cartridge' = 'latentdeck_comfy_cartridge/'
        'latentdeck-example-channel-roll' = 'latentdeck_example_channel_roll/'
    }
    foreach ($wheelReceipt in @($kitManifest.wheels)) {
        $project = [string]$wheelReceipt.name
        if (-not $moduleByProject.Contains($project)) {
            throw "Developer Kit has an unexpected project wheel: $project"
        }
        $wheelPath = Join-Path $expandedRoot "wheels/$($wheelReceipt.file_name)"
        $wheel = [System.IO.Compression.ZipFile]::OpenRead($wheelPath)
        try {
            $names = @($wheel.Entries | ForEach-Object FullName)
            $modulePrefix = [string]$moduleByProject[$project]
            if (@($names | Where-Object { $_.StartsWith($modulePrefix, [System.StringComparison]::Ordinal) }).Count -eq 0 -or
                $names -cnotcontains "${modulePrefix}py.typed") {
                throw "Developer Kit wheel lacks its expected module or py.typed marker: $project"
            }
        } finally {
            $wheel.Dispose()
        }
    }

    $bootstrapPath = Join-Path $expandedRoot 'bootstrap/Install-ProjectWheels.ps1'
    $environmentPath = Join-Path $testRoot 'python-3.13-environment'
    & $bootstrapPath -EnvironmentDirectory $environmentPath
    if ($LASTEXITCODE -ne 0) {
        throw 'Developer Kit bootstrap failed in a fresh Python 3.13 environment.'
    }
    $environmentPython = Join-Path $environmentPath 'Scripts/python.exe'
    foreach ($wheelReceipt in @($kitManifest.wheels)) {
        $installedVersion = (& $environmentPython -I -s -B -c `
            'import importlib.metadata as m,sys; print(m.version(sys.argv[1]))' `
            ([string]$wheelReceipt.name)).Trim()
        if ($LASTEXITCODE -ne 0 -or $installedVersion -cne [string]$wheelReceipt.version) {
            throw "Fresh Developer Kit environment lacks exact distribution: $($wheelReceipt.name)"
        }
    }
    Assert-Throws -ExpectedText 'Refusing to overwrite an existing Python environment' -Action {
        & $bootstrapPath -EnvironmentDirectory $environmentPath
    }

    $tamperedWheel = Join-Path $expandedRoot "wheels/$($kitManifest.wheels[0].file_name)"
    $tamperStream = [System.IO.File]::Open(
        $tamperedWheel,
        [System.IO.FileMode]::Append,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $tamperStream.WriteByte(0)
    } finally {
        $tamperStream.Dispose()
    }
    Assert-Throws -ExpectedText 'failed SHA-256/length verification' -Action {
        & $bootstrapPath -EnvironmentDirectory (Join-Path $testRoot 'tampered-environment')
    }

    Assert-Throws -ExpectedText 'Refusing to overwrite' -Action {
        & (Join-Path $PSScriptRoot 'Build-DeveloperKit.ps1') `
            -ComfyRecorderArtifactDirectory $recorderArtifactRoot `
            -OutputDirectory $outputRoot `
            -ReleaseChannel unsigned_preview `
            -ReleaseLabel $releaseLabel `
            @developerKitMode
    }
    Write-Host 'DEVELOPER KIT PACKAGING CONTRACT: PASS' -ForegroundColor Green
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        $resolved = [System.IO.Path]::GetFullPath($testRoot)
        $rootPrefix = $artifactsRoot.TrimEnd('\') + '\'
        if (-not $resolved.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not ([System.IO.Path]::GetFileName($resolved)).StartsWith(
                '.developer-kit-contract-',
                [System.StringComparison]::Ordinal
            )) {
            throw "Refusing to remove unsafe Developer Kit test directory: $resolved"
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
