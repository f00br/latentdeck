[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force
$scratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
    'latentdeck-comfy-recorder-contract-' + [guid]::NewGuid().ToString('N')
)

function Write-TestJson {
    param(
        [Parameter(Mandatory)][object]$Value,
        [Parameter(Mandatory)][string]$Path
    )

    [System.IO.File]::WriteAllText(
        $Path,
        (($Value | ConvertTo-Json -Depth 100) + "`n"),
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Assert-ThrowsMatching {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$Pattern,
        [Parameter(Mandatory)][string]$Context
    )

    $rejected = $false
    try {
        & $Action
    } catch {
        if ($_.Exception.Message -notmatch $Pattern) {
            throw
        }
        $rejected = $true
    }
    if (-not $rejected) {
        throw "$Context was not rejected."
    }
}

try {
    $wheelRoot = Join-Path $scratchRoot 'wheels'
    [System.IO.Directory]::CreateDirectory($wheelRoot) | Out-Null

    Push-Location $repositoryRoot
    try {
        & uv build --wheel sdk/python --out-dir $wheelRoot --no-create-gitignore `
            --build-constraints tools/packaging/windows-x64-build-constraints.txt `
            --require-hashes
        if ($LASTEXITCODE -ne 0) {
            throw "Cartridge SDK wheel build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    $wheels = @(
        Get-ChildItem -LiteralPath $wheelRoot -File |
            Where-Object Name -like 'latentdeck_cartridge-0.1.0-*.whl'
    )
    if ($wheels.Count -ne 1 -or
        $wheels[0].Name -cnotmatch '^latentdeck_cartridge-0\.1\.0-cp312-abi3-win_amd64\.whl$') {
        $found = @($wheels | ForEach-Object Name) -join ', '
        throw "Cartridge SDK must build one cp312-abi3 Windows x64 wheel; found: $found"
    }

    Push-Location $repositoryRoot
    try {
        & uv build --wheel comfy/latent-cartridge --out-dir $wheelRoot --no-create-gitignore `
            --build-constraints tools/packaging/windows-x64-build-constraints.txt `
            --require-hashes
        if ($LASTEXITCODE -ne 0) {
            throw "Recorder wheel build failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
    $recorderWheels = @(
        Get-ChildItem -LiteralPath $wheelRoot -File |
            Where-Object Name -like 'latentdeck_comfy_cartridge-0.1.0-*.whl'
    )
    if ($recorderWheels.Count -ne 1 -or
        $recorderWheels[0].Name -cnotmatch '^latentdeck_comfy_cartridge-0\.1\.0-py3-none-any\.whl$') {
        throw 'Recorder must build one platform-neutral wheel.'
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $recorderArchive = [System.IO.Compression.ZipFile]::OpenRead($recorderWheels[0].FullName)
    try {
        $metadataEntry = @(
            $recorderArchive.Entries |
                Where-Object FullName -like 'latentdeck_comfy_cartridge-0.1.0.dist-info/METADATA'
        )
        if ($metadataEntry.Count -ne 1) {
            throw 'Recorder wheel does not contain its exact METADATA entry.'
        }
        $reader = [System.IO.StreamReader]::new($metadataEntry[0].Open())
        try {
            $metadata = $reader.ReadToEnd()
        }
        finally {
            $reader.Dispose()
        }
        if ($metadata -cnotmatch '(?m)^Requires-Python: >=3\.12, ?<3\.14\r?$') {
            throw 'Recorder wheel must declare support for CPython 3.12 and 3.13 only.'
        }
    }
    finally {
        $recorderArchive.Dispose()
    }

    $lock = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'comfy/latent-cartridge/packaging/windows-x64.lock.json'
    ) -Raw | ConvertFrom-Json -Depth 32
    $safetensorsWheel = Join-Path $scratchRoot ([string]$lock.safetensors.file_name)
    Invoke-WebRequest -Uri ([string]$lock.safetensors.url) -OutFile $safetensorsWheel
    $expectedSourceCommit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
    $expectedSourceTree = (& git -C $repositoryRoot rev-parse 'HEAD^{tree}').Trim()
    $expectedSourceBranch = (& git -C $repositoryRoot branch --show-current).Trim()
    $expectedSourceStatus = @(& git -C $repositoryRoot status --short --untracked-files=all)
    if ($LASTEXITCODE -ne 0) {
        throw 'Comfy Recorder test could not resolve the source identity.'
    }
    $expectedSourceDirty = $expectedSourceStatus.Count -gt 0
    $expectedDistributable = (
        -not $expectedSourceDirty -and $expectedSourceBranch -ceq 'main'
    )
    $artifactRoot = Join-Path $scratchRoot 'artifact-set'
    & (Join-Path $PSScriptRoot 'Build-ComfyRecorderBundle.ps1') `
        -OutputDirectory $artifactRoot `
        -SafetensorsWheelPath $safetensorsWheel `
        -AllowDirtySource
    if ($LASTEXITCODE -ne 0) {
        throw "Comfy Recorder bundle build failed with exit code $LASTEXITCODE."
    }
    $baseName = 'LatentDeck-0.1.0-preview.1-comfy-recorder-windows-x64'
    $archivePath = Join-Path $artifactRoot "$baseName.zip"
    $receiptPath = Join-Path $artifactRoot "$baseName.receipt.json"
    $checksumPath = Join-Path $artifactRoot "$baseName.SHA256SUMS.txt"
    $sbomPath = Join-Path $artifactRoot "$baseName-sbom.cdx.json"
    $noticePath = Join-Path $artifactRoot "$baseName-THIRD-PARTY-NOTICES.md"
    $licenseBundlePath = Join-Path $artifactRoot "$baseName-THIRD-PARTY-LICENSES.json"
    $licenseReviewPath = Join-Path $artifactRoot "$baseName-license-review.json"
    foreach ($required in @(
        $archivePath,
        $receiptPath,
        $checksumPath,
        $sbomPath,
        $noticePath,
        $licenseBundlePath,
        $licenseReviewPath
    )) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "Comfy Recorder artifact set is missing: $required"
        }
    }
    $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -Depth 32
    if ([int]$receipt.schema_version -ne 1 -or
        [string]$receipt.artifact_kind -cne 'comfy_recorder_bundle' -or
        [string]$receipt.release_label -cne '0.1.0-preview.1' -or
        [string]$receipt.release_channel -cne 'unsigned_preview' -or
        [string]$receipt.target -cne 'windows-x64' -or
        [string]$receipt.python_abi -cne 'cp312-abi3' -or
        (@($receipt.supported_python) -join "`0") -cne (@('cp312', 'cp313') -join "`0") -or
        [string]$receipt.source.git_commit -cne $expectedSourceCommit -or
        [string]$receipt.source.git_tree -cne $expectedSourceTree -or
        [string]$receipt.source.git_branch -cne $expectedSourceBranch -or
        [bool]$receipt.source.git_dirty -ne $expectedSourceDirty -or
        [int64]$receipt.source.git_dirty_entry_count -ne $expectedSourceStatus.Count -or
        [string]$receipt.source.git_tree -cnotmatch '^[0-9a-f]{40}$' -or
        [string]$receipt.source.public_snapshot_sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [int64]$receipt.source.public_snapshot_file_count -le 0 -or
        [bool]$receipt.signed -or -not [bool]$receipt.unsigned -or
        [bool]$receipt.distributable -ne $expectedDistributable) {
        throw 'Comfy Recorder receipt identity or dirty-build contract is invalid.'
    }
    if ([string]$receipt.archive.file_name -cne "$baseName.zip" -or
        [string]$receipt.archive.sha256 -cne
            (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant() -or
        [int64]$receipt.archive.byte_length -ne (Get-Item -LiteralPath $archivePath).Length) {
        throw 'Comfy Recorder receipt does not bind the exact bundle archive.'
    }
    $bindingPaths = @{
        sbom = $sbomPath
        third_party_notices = $noticePath
        license_bundle = $licenseBundlePath
        license_review = $licenseReviewPath
    }
    foreach ($bindingName in @('sbom', 'third_party_notices', 'license_bundle', 'license_review')) {
        $binding = $receipt.PSObject.Properties[$bindingName]
        $expectedFileName = [System.IO.Path]::GetFileName([string]$bindingPaths[$bindingName])
        $bindingItem = Get-Item -LiteralPath $bindingPaths[$bindingName]
        $bindingHash = (
            Get-FileHash -LiteralPath $bindingItem.FullName -Algorithm SHA256
        ).Hash.ToLowerInvariant()
        if ($null -eq $binding -or
            [string]$binding.Value.file_name -cne $expectedFileName -or
            [string]$binding.Value.sha256 -cne $bindingHash -or
            [int64]$binding.Value.byte_length -ne [int64]$bindingItem.Length) {
            throw "Comfy Recorder receipt sidecar binding is invalid: $bindingName"
        }
    }
    $checksumLines = @(
        Get-Content -LiteralPath $checksumPath |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
    )
    if ($checksumLines.Count -ne 5) {
        throw 'Comfy Recorder SHA256SUMS must bind the archive and four release sidecars.'
    }
    $expectedChecksumLines = @(
        foreach ($path in @(
            $archivePath,
            $sbomPath,
            $noticePath,
            $licenseBundlePath,
            $licenseReviewPath
        ) | Sort-Object { [System.IO.Path]::GetFileName($_) } -CaseSensitive) {
            $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            "$hash  $([System.IO.Path]::GetFileName($path))"
        }
    )
    if (($checksumLines -join "`0") -cne ($expectedChecksumLines -join "`0")) {
        throw 'Comfy Recorder SHA256SUMS does not exactly bind the release payload.'
    }
    $externalLicenseBundleResult = Test-ReleaseLicenseBundle `
        -BundlePath $licenseBundlePath `
        -SbomPath $sbomPath `
        -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
        -ExpectedArtifactVersion '0.1.0-preview.1'
    if ([int]$receipt.license_bundle.component_count -ne
            [int]$externalLicenseBundleResult.ComponentCount -or
        [int]$receipt.license_bundle.text_count -ne
            [int]$externalLicenseBundleResult.TextCount -or
        [int]$receipt.license_bundle.build_only_no_text_disposition_count -ne
            [int]$externalLicenseBundleResult.NoTextDispositionCount) {
        throw 'Comfy Recorder receipt does not bind the external full-text license mapping.'
    }
    $nativeClosureResult = Test-SafetensorsNativeClosureEvidence `
        -Evidence $receipt.sbom.safetensors_native_closure `
        -SbomPath $sbomPath
    if ([int]$nativeClosureResult.ComponentCount -ne 32) {
        throw 'Comfy Recorder receipt does not bind the exact Safetensors native closure.'
    }

    $expandedRoot = Join-Path $scratchRoot 'expanded'
    [System.IO.Compression.ZipFile]::ExtractToDirectory($archivePath, $expandedRoot)
    $installerPath = Join-Path $expandedRoot 'Install-ComfyRecorder.ps1'
    foreach ($requiredRelative in @(
        'Install-ComfyRecorder.ps1',
        'Verify-ComfyRecorder.py',
        'custom_node/__init__.py',
        'SBOM.cdx.json',
        'THIRD_PARTY_NOTICES.md',
        'THIRD_PARTY_LICENSES.json',
        'LICENSE-REVIEW.json'
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $expandedRoot $requiredRelative) -PathType Leaf)) {
            throw "Comfy Recorder bundle is missing install payload: $requiredRelative"
        }
    }
    $internalSbomPath = Join-Path $expandedRoot 'SBOM.cdx.json'
    $internalLicenseBundlePath = Join-Path $expandedRoot 'THIRD_PARTY_LICENSES.json'
    $licenseBundleResult = Test-ReleaseLicenseBundle `
        -BundlePath $internalLicenseBundlePath `
        -SbomPath $internalSbomPath `
        -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
        -ExpectedArtifactVersion '0.1.0-preview.1'
    if ($licenseBundleResult.ComponentCount -le 0 -or $licenseBundleResult.TextCount -le 0) {
        throw 'Comfy Recorder full-text license bundle is empty.'
    }
    $sbom = Get-Content -LiteralPath $internalSbomPath -Raw | ConvertFrom-Json -Depth 100
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
                throw "Comfy Recorder SBOM selection root is ambiguous: $($component.name)"
            }
            "$([string]$ecosystem[0].value):$($component.name)@$($component.version)"
        }
    ) | Sort-Object
    $expectedSbomRoots = @(
        'python:latentdeck-cartridge@0.1.0',
        'python:latentdeck-comfy-cartridge@0.1.0',
        'python:maturin@1.15.0',
        'python:safetensors@0.8.0',
        'python:uv-build@0.12.7',
        'rust:latentdeck-cartridge-python@0.1.0',
        'rust:latentdeck-cartridge@0.1.0'
    ) | Sort-Object
    $invalidScopes = @(
        foreach ($component in @($sbom.metadata.component) + @($sbom.components)) {
            $scopes = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope'
            })
            if ($scopes.Count -ne 1 -or
                [string]$scopes[0].value -cnotin @(
                    'artifact', 'runtime', 'build', 'runtime+build'
                )) {
                "$($component.name)@$($component.version)"
            }
        }
    )
    if (($sbomRoots -join "`0") -cne ($expectedSbomRoots -join "`0") -or
        $invalidScopes.Count -ne 0 -or [int]$receipt.sbom.selection_root_count -ne 7) {
        throw 'Comfy Recorder SBOM runtime/build closure is invalid.'
    }
    $licenseReview = Get-Content -LiteralPath (
        Join-Path $expandedRoot 'LICENSE-REVIEW.json'
    ) -Raw | ConvertFrom-Json -Depth 100
    if ([string]$licenseReview.status -cne 'complete' -or
        [int]$licenseReview.missing_license_component_count -ne 0 -or
        [int]$licenseReview.selection_root_count -ne 7 -or
        [int]$receipt.license_bundle.component_count -ne $licenseBundleResult.ComponentCount -or
        [int]$receipt.license_bundle.text_count -ne $licenseBundleResult.TextCount) {
        throw 'Comfy Recorder license review or receipt closure is invalid.'
    }
    $notices = Get-Content -LiteralPath (Join-Path $expandedRoot 'THIRD_PARTY_NOTICES.md') -Raw
    if ($notices -cnotmatch 'relocated under\s+`latentdeck_recorder_vendor`' -or
        $notices -cnotmatch 'relative import solely for namespace isolation') {
        throw 'Comfy Recorder notices do not disclose the Safetensors namespace relocation.'
    }

    $nativeComponent = @($sbom.components | Where-Object {
        [string]$_.'bom-ref' -ceq 'rust:safetensors-native:allocator-api2@0.2.21'
    })
    if ($nativeComponent.Count -ne 1) {
        throw 'Comfy Recorder SBOM is missing the locked Safetensors allocator-api2 component.'
    }
    $mutations = @(
        [pscustomobject]@{
            Name = 'runtime-to-build'
            Pattern = 'build-only component set drifted'
            Apply = {
                param($value)
                ($value.components | Where-Object {
                    [string]$_.'bom-ref' -ceq 'rust:safetensors-native:allocator-api2@0.2.21'
                }).properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:dependency-scope'
                } | ForEach-Object { $_.value = 'build' }
            }
        }
        [pscustomobject]@{
            Name = 'uppercase-scope'
            Pattern = 'non-canonical ecosystem or scope'
            Apply = {
                param($value)
                ($value.components | Where-Object {
                    [string]$_.'bom-ref' -ceq 'rust:safetensors-native:allocator-api2@0.2.21'
                }).properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:dependency-scope'
                } | ForEach-Object { $_.value = 'Runtime' }
            }
        }
        [pscustomobject]@{
            Name = 'uppercase-ecosystem'
            Pattern = 'non-canonical ecosystem or scope'
            Apply = {
                param($value)
                ($value.components | Where-Object {
                    [string]$_.'bom-ref' -ceq 'rust:safetensors-native:allocator-api2@0.2.21'
                }).properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:ecosystem'
                } | ForEach-Object { $_.value = 'RUST' }
            }
        }
        [pscustomobject]@{
            Name = 'unknown-ecosystem'
            Pattern = 'non-canonical ecosystem or scope'
            Apply = {
                param($value)
                ($value.components | Where-Object {
                    [string]$_.'bom-ref' -ceq 'rust:safetensors-native:allocator-api2@0.2.21'
                }).properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:ecosystem'
                } | ForEach-Object { $_.value = 'unknown' }
            }
        }
        [pscustomobject]@{
            Name = 'child-artifact-ecosystem'
            Pattern = 'invalid root/component ecosystem boundary'
            Apply = {
                param($value)
                ($value.components | Where-Object {
                    [string]$_.'bom-ref' -ceq 'rust:safetensors-native:allocator-api2@0.2.21'
                }).properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:ecosystem'
                } | ForEach-Object { $_.value = 'artifact' }
            }
        }
    )
    foreach ($mutation in $mutations) {
        $mutatedSbom = ($sbom | ConvertTo-Json -Depth 100) | ConvertFrom-Json -Depth 100
        & $mutation.Apply $mutatedSbom
        $mutatedSbomPath = Join-Path $scratchRoot "mutated-$($mutation.Name)-SBOM.cdx.json"
        Write-TestJson -Value $mutatedSbom -Path $mutatedSbomPath
        Assert-ThrowsMatching `
            -Action {
                Test-ReleaseLicenseBundle `
                    -BundlePath $licenseBundlePath `
                    -SbomPath $mutatedSbomPath `
                    -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
                    -ExpectedArtifactVersion '0.1.0-preview.1' | Out-Null
            } `
            -Pattern ([string]$mutation.Pattern) `
            -Context "Test-ReleaseLicenseBundle mutation $($mutation.Name)"
        $mutatedBundlePath = Join-Path $scratchRoot "mutated-$($mutation.Name)-licenses.json"
        Assert-ThrowsMatching `
            -Action {
                New-ReleaseLicenseBundle `
                    -SbomPath $mutatedSbomPath `
                    -ArtifactName 'LatentDeck Comfy LC Recorder' `
                    -ArtifactVersion '0.1.0-preview.1' `
                    -OutputPath $mutatedBundlePath `
                    -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md') `
                    -SafetensorsWheelPath $safetensorsWheel | Out-Null
            } `
            -Pattern ([string]$mutation.Pattern) `
            -Context "New-ReleaseLicenseBundle mutation $($mutation.Name)"
    }

    $nativeMissingSbom = ($sbom | ConvertTo-Json -Depth 100) | ConvertFrom-Json -Depth 100
    $nativeMissingSbom.components = @($nativeMissingSbom.components | Where-Object {
        [string]$_.'bom-ref' -cne 'rust:safetensors-native:allocator-api2@0.2.21'
    })
    Assert-ThrowsMatching `
        -Action {
            Test-SafetensorsNativeClosureEvidence `
                -Evidence $receipt.sbom.safetensors_native_closure `
                -Sbom $nativeMissingSbom | Out-Null
        } `
        -Pattern 'exact native component count' `
        -Context 'Safetensors native missing-component mutation'

    $licenseBundle = Get-Content -LiteralPath $licenseBundlePath -Raw |
        ConvertFrom-Json -Depth 100
    $policyTampered = ($licenseBundle | ConvertTo-Json -Depth 100) | ConvertFrom-Json -Depth 100
    $policyTampered.policy.redistributed_components_require_text = 'true'
    $policyTamperedPath = Join-Path $scratchRoot 'mutated-license-policy.json'
    Write-TestJson -Value $policyTampered -Path $policyTamperedPath
    Assert-ThrowsMatching `
        -Action {
            Test-ReleaseLicenseBundle `
                -BundlePath $policyTamperedPath `
                -SbomPath $sbomPath `
                -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
                -ExpectedArtifactVersion '0.1.0-preview.1' | Out-Null
        } `
        -Pattern 'does not bind the reviewed build-only scope lock' `
        -Context 'tampered license policy'

    $mappingMissing = ($licenseBundle | ConvertTo-Json -Depth 100) | ConvertFrom-Json -Depth 100
    $mappingMissing.components = @($mappingMissing.components | Select-Object -Skip 1)
    $mappingMissing.component_count = [int]$mappingMissing.component_count - 1
    $mappingMissingPath = Join-Path $scratchRoot 'mutated-license-mapping.json'
    Write-TestJson -Value $mappingMissing -Path $mappingMissingPath
    Assert-ThrowsMatching `
        -Action {
            Test-ReleaseLicenseBundle `
                -BundlePath $mappingMissingPath `
                -SbomPath $sbomPath `
                -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
                -ExpectedArtifactVersion '0.1.0-preview.1' | Out-Null
        } `
        -Pattern 'mapping/text closure is incomplete' `
        -Context 'missing license component mapping'

    $textTampered = ($licenseBundle | ConvertTo-Json -Depth 100) | ConvertFrom-Json -Depth 100
    $textTampered.texts[0].text = [string]$textTampered.texts[0].text + 'tampered'
    $textTamperedPath = Join-Path $scratchRoot 'mutated-license-text.json'
    Write-TestJson -Value $textTampered -Path $textTamperedPath
    Assert-ThrowsMatching `
        -Action {
            Test-ReleaseLicenseBundle `
                -BundlePath $textTamperedPath `
                -SbomPath $sbomPath `
                -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
                -ExpectedArtifactVersion '0.1.0-preview.1' | Out-Null
        } `
        -Pattern 'invalid or duplicate text content' `
        -Context 'tampered license text'

    $mappingTampered = ($licenseBundle | ConvertTo-Json -Depth 100) | ConvertFrom-Json -Depth 100
    $mappedWithText = @($mappingTampered.components | Where-Object {
        @($_.text_sha256s).Count -gt 0
    })[0]
    $mappedWithText.text_sha256s[0] = '0' * 64
    $mappingTamperedPath = Join-Path $scratchRoot 'mutated-license-text-reference.json'
    Write-TestJson -Value $mappingTampered -Path $mappingTamperedPath
    Assert-ThrowsMatching `
        -Action {
            Test-ReleaseLicenseBundle `
                -BundlePath $mappingTamperedPath `
                -SbomPath $sbomPath `
                -ExpectedArtifactName 'LatentDeck Comfy LC Recorder' `
                -ExpectedArtifactVersion '0.1.0-preview.1' | Out-Null
        } `
        -Pattern 'references unknown text' `
        -Context 'tampered license mapping text reference'
    $installerText = Get-Content -LiteralPath $installerPath -Raw
    if ($installerText -match '(?i)\b(?:cargo|maturin)\b|pip\s+install|--no-binary') {
        throw 'Comfy Recorder installer must not compile or request a source installation.'
    }
    $builderText = Get-Content -LiteralPath (
        Join-Path $PSScriptRoot 'Build-ComfyRecorderBundle.ps1'
    ) -Raw
    if ($builderText -match '(?i)Invoke-WebRequest|Start-BitsTransfer|curl(?:\.exe)?' -or
        $builderText -cnotmatch '--build-constraints\s+tools/packaging/windows-x64-build-constraints\.txt\s+`?\s*--require-hashes') {
        throw 'Comfy Recorder release assembly must be offline and use the exact hashed build constraints.'
    }

    $python313 = (& py -3.13 -c 'import sys; print(sys.executable)').Trim()
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $python313 -PathType Leaf)) {
        throw 'Comfy Recorder contract test requires CPython 3.13.'
    }
    $comfy313 = Join-Path $scratchRoot 'ComfyUI-313'
    [System.IO.Directory]::CreateDirectory((Join-Path $comfy313 'custom_nodes')) | Out-Null
    & $installerPath -ComfyUIRoot $comfy313 -PythonPath $python313
    $installed313 = Join-Path $comfy313 'custom_nodes/ComfyUI-LatentCartridge'
    if (Test-Path -LiteralPath (Join-Path $installed313 'vendor/safetensors')) {
        throw 'Recorder installation exposes a top-level bundled Safetensors package.'
    }
    if (-not (Test-Path -LiteralPath (
        Join-Path $installed313 'vendor/latentdeck_recorder_vendor/safetensors'
    ) -PathType Container)) {
        throw 'Recorder installation does not contain its uniquely namespaced Safetensors fallback.'
    }
    $installationReceipt = Get-Content `
        -LiteralPath (Join-Path $installed313 'INSTALLATION.json') `
        -Raw | ConvertFrom-Json -Depth 16
    if ([string]$installationReceipt.python_abi -cne 'cp313' -or
        [string]$installationReceipt.bundle_python_abi -cne 'cp312-abi3' -or
        (@($installationReceipt.packages).Count -ne 3)) {
        throw 'CPython 3.13 installation receipt is invalid.'
    }
    & $python313 -I (Join-Path $expandedRoot 'Verify-ComfyRecorder.py') `
        --shim-host (Join-Path $installed313 '__init__.py') | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Recorder shim replaced an existing host Safetensors package.'
    }

    $python312 = (& uv python find --no-project 3.12).Trim()
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $python312 -PathType Leaf)) {
        throw 'Comfy Recorder contract test requires an uv-managed CPython 3.12.'
    }
    $comfy312 = Join-Path $scratchRoot 'ComfyUI-312'
    [System.IO.Directory]::CreateDirectory((Join-Path $comfy312 'custom_nodes')) | Out-Null
    & $installerPath -ComfyUIRoot $comfy312 -PythonPath $python312
    $installed312 = Join-Path $comfy312 'custom_nodes/ComfyUI-LatentCartridge'
    $installationReceipt312 = Get-Content `
        -LiteralPath (Join-Path $installed312 'INSTALLATION.json') `
        -Raw | ConvertFrom-Json -Depth 16
    if ([string]$installationReceipt312.python_abi -cne 'cp312' -or
        [string]$installationReceipt312.bundle_python_abi -cne 'cp312-abi3' -or
        (@($installationReceipt312.packages).Count -ne 3)) {
        throw 'CPython 3.12 installation receipt is invalid.'
    }
    & $python312 -I (Join-Path $expandedRoot 'Verify-ComfyRecorder.py') `
        --shim-bundled (Join-Path $installed312 '__init__.py') | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Recorder shim could not use bundled Safetensors when no host package exists.'
    }

    $secondArtifactRoot = Join-Path $scratchRoot 'artifact-set-second'
    & (Join-Path $PSScriptRoot 'Build-ComfyRecorderBundle.ps1') `
        -OutputDirectory $secondArtifactRoot `
        -SafetensorsWheelPath $safetensorsWheel `
        -AllowDirtySource | Out-Null
    $firstHashes = @(
        Get-ChildItem -LiteralPath $artifactRoot -File |
            Sort-Object Name |
            ForEach-Object {
                $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
                "$hash $($_.Name)"
            }
    )
    $secondHashes = @(
        Get-ChildItem -LiteralPath $secondArtifactRoot -File |
            Sort-Object Name |
            ForEach-Object {
                $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
                "$hash $($_.Name)"
            }
    )
    if (($firstHashes -join "`n") -cne ($secondHashes -join "`n")) {
        throw 'Repeated Comfy Recorder builds are not deterministic.'
    }

    $buildNoClobberRejected = $false
    try {
        & (Join-Path $PSScriptRoot 'Build-ComfyRecorderBundle.ps1') `
            -OutputDirectory $artifactRoot `
            -SafetensorsWheelPath $safetensorsWheel `
            -AllowDirtySource | Out-Null
    }
    catch {
        if ($_.Exception.Message -notmatch 'Refusing to overwrite') {
            throw
        }
        $buildNoClobberRejected = $true
    }
    if (-not $buildNoClobberRejected) {
        throw 'Comfy Recorder builder accepted an existing artifact-set destination.'
    }

    $overwriteRejected = $false
    try {
        & $installerPath -ComfyUIRoot $comfy313 -PythonPath $python313
    }
    catch {
        if ($_.Exception.Message -notmatch 'Refusing to overwrite an existing Recorder installation') {
            throw
        }
        $overwriteRejected = $true
    }
    if (-not $overwriteRejected) {
        throw 'Recorder installer accepted an existing destination.'
    }

    $python310 = (& py -3.10 -c 'import sys; print(sys.executable)').Trim()
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $python310 -PathType Leaf)) {
        throw 'Comfy Recorder contract test requires unsupported CPython 3.10.'
    }
    $comfy310 = Join-Path $scratchRoot 'ComfyUI-310'
    [System.IO.Directory]::CreateDirectory((Join-Path $comfy310 'custom_nodes')) | Out-Null
    $rejected = $false
    try {
        & $installerPath -ComfyUIRoot $comfy310 -PythonPath $python310
    }
    catch {
        if ($_.Exception.Message -notmatch 'supports CPython 3\.12 and 3\.13 x64 only') {
            throw
        }
        $rejected = $true
    }
    if (-not $rejected -or
        (Test-Path -LiteralPath (Join-Path $comfy310 'custom_nodes/ComfyUI-LatentCartridge'))) {
        throw 'Unsupported CPython 3.10 was not rejected before installation.'
    }

    $tamperedRoot = Join-Path $scratchRoot 'expanded-tampered'
    [System.IO.Compression.ZipFile]::ExtractToDirectory($archivePath, $tamperedRoot)
    $tamperedManifest = Get-Content `
        -LiteralPath (Join-Path $tamperedRoot 'BUNDLE-MANIFEST.json') `
        -Raw | ConvertFrom-Json -Depth 16
    $tamperedWheel = Join-Path $tamperedRoot "wheels/$($tamperedManifest.wheels[0].file_name)"
    $tamperedBytes = [System.IO.File]::ReadAllBytes($tamperedWheel)
    $tamperedBytes[0] = $tamperedBytes[0] -bxor 0xFF
    [System.IO.File]::WriteAllBytes($tamperedWheel, $tamperedBytes)
    $comfyTampered = Join-Path $scratchRoot 'ComfyUI-tampered'
    [System.IO.Directory]::CreateDirectory((Join-Path $comfyTampered 'custom_nodes')) | Out-Null
    $tamperRejected = $false
    try {
        & (Join-Path $tamperedRoot 'Install-ComfyRecorder.ps1') `
            -ComfyUIRoot $comfyTampered `
            -PythonPath $python313
    }
    catch {
        if ($_.Exception.Message -notmatch 'failed exact length/SHA-256 verification') {
            throw
        }
        $tamperRejected = $true
    }
    if (-not $tamperRejected -or
        (Test-Path -LiteralPath (Join-Path $comfyTampered 'custom_nodes/ComfyUI-LatentCartridge'))) {
        throw 'A tampered bundled wheel was not rejected before installation.'
    }

    Write-Host 'Comfy Recorder ABI contract passed.' -ForegroundColor Green
}
finally {
    if (Test-Path -LiteralPath $scratchRoot) {
        Remove-Item -LiteralPath $scratchRoot -Recurse -Force
    }
}
