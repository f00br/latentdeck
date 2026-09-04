[CmdletBinding()]
param(
    [string]$LifecycleHelperPath,

    [string]$NsisRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force
Add-Type -AssemblyName System.IO.Compression

if (-not $IsWindows) {
    throw 'H3 Codec Pack setup tests require Windows.'
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".h3-setup-test-$([guid]::NewGuid().ToString('N'))"
Assert-SafeTemporaryDirectory `
    -ParentPath $artifactsRoot `
    -CandidatePath $testRoot `
    -RequiredLeafPrefix '.h3-setup-test-' | Out-Null

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

function Assert-Condition {
    param(
        [Parameter(Mandatory)]
        [bool]$Condition,

        [Parameter(Mandatory)]
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-LowercaseSha256 {
    param(
        [Parameter(Mandatory)]
        [string]$Value,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($Value -cnotmatch '^[0-9a-f]{64}$') {
        throw "$Context is not a lowercase SHA-256 value."
    }
}

function Assert-WindowsPe {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [int]$ExpectedMachine,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or $item.Length -lt 1024) {
        throw "$Context is not a plausible Windows executable."
    }
    $stream = [System.IO.File]::Open(
        $item.FullName,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::Read
    )
    try {
        $reader = [System.IO.BinaryReader]::new($stream)
        try {
            if ($reader.ReadUInt16() -ne 0x5A4D) {
                throw "$Context is missing the DOS header."
            }
            $stream.Position = 0x3C
            $peOffset = [int64]$reader.ReadUInt32()
            if ($peOffset -lt 0x40 -or $peOffset -gt ($stream.Length - 6)) {
                throw "$Context has an invalid PE offset."
            }
            $stream.Position = $peOffset
            if ($reader.ReadUInt32() -ne 0x00004550 -or
                $reader.ReadUInt16() -ne $ExpectedMachine) {
                throw "$Context has the wrong PE identity."
            }
        } finally {
            $reader.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Invoke-NativeResult {
    param(
        [Parameter(Mandatory)]
        [string]$Executable,

        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    $output = (& $Executable @Arguments 2>&1 | Out-String).Trim()
    return [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output = $output
    }
}

function Assert-NativeExit {
    param(
        [Parameter(Mandatory)]
        [pscustomobject]$Result,

        [Parameter(Mandatory)]
        [int[]]$ExpectedExitCodes,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($ExpectedExitCodes -notcontains [int]$Result.ExitCode) {
        throw (
            "$Context returned exit code $($Result.ExitCode); expected " +
            "$($ExpectedExitCodes -join ', '). Output: $($Result.Output)"
        )
    }
}

function Get-DirectoryFingerprint {
    param(
        [Parameter(Mandatory)]
        [string]$Root
    )

    $resolvedRoot = (Resolve-Path -LiteralPath $Root).Path
    $records = @(
        Get-ChildItem -LiteralPath $resolvedRoot -File -Force -Recurse |
            ForEach-Object {
                $relative = [System.IO.Path]::GetRelativePath($resolvedRoot, $_.FullName).Replace('\', '/')
                $hash = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
                "$relative`0$($_.Length)`0$hash"
            } |
            Sort-Object
    )
    $bytes = [System.Text.Encoding]::UTF8.GetBytes(($records -join "`n") + "`n")
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return [System.Convert]::ToHexString($sha.ComputeHash($bytes)).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Get-PublicSourceSnapshot {
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot
    )

    $relativePaths = @(
        & git -C $RepositoryRoot -c core.quotepath=false `
            ls-files --cached --others --exclude-standard
    )
    if ($LASTEXITCODE -ne 0 -or $relativePaths.Count -eq 0) {
        throw 'Could not enumerate the Git/public source snapshot for setup receipt validation.'
    }
    $records = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in $relativePaths) {
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            continue
        }
        $fullPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot $relativePath))
        $rootWithSeparator = $RepositoryRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
        if (-not $fullPath.StartsWith(
            $rootWithSeparator,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Public source snapshot path escaped the repository: $relativePath"
        }
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            $records.Add("missing`0$relativePath")
            continue
        }
        $item = Get-Item -LiteralPath $fullPath -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Public source snapshot contains a reparse-point file: $relativePath"
        }
        $portablePath = $relativePath.Replace('\', '/')
        $hash = (Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $records.Add("file`0$portablePath`0$($item.Length)`0$hash")
    }
    $payload = [System.Text.UTF8Encoding]::new($false).GetBytes(
        (@($records) | Sort-Object -CaseSensitive) -join "`n"
    )
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $snapshotSha256 = [System.Convert]::ToHexString(
            $sha.ComputeHash($payload)
        ).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
    return [pscustomobject]@{
        Sha256 = $snapshotSha256
        FileCount = $records.Count
    }
}

function Resolve-TestPython313 {
    $candidates = [System.Collections.Generic.List[string]]::new()
    if (-not [string]::IsNullOrWhiteSpace($env:LATENTDECK_TEST_PYTHON313)) {
        $candidates.Add($env:LATENTDECK_TEST_PYTHON313)
    }

    $pyLauncher = Get-Command py.exe -ErrorAction SilentlyContinue
    if ($null -ne $pyLauncher) {
        $candidate = (& $pyLauncher.Source -3.13 -c 'import sys; print(sys.executable)' 2>$null)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($candidate)) {
            $candidates.Add($candidate.Trim())
        }
    }

    $python = Get-Command python.exe -ErrorAction SilentlyContinue
    if ($null -ne $python) {
        $candidate = (& $python.Source -c 'import sys; print(sys.executable if sys.version_info[:2] == (3, 13) else "")' 2>$null)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($candidate)) {
            $candidates.Add($candidate.Trim())
        }
    }

    foreach ($candidate in $candidates) {
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            continue
        }
        $resolvedPython = (Resolve-Path -LiteralPath $candidate).Path
        $dllCandidates = @(
            (Join-Path (Split-Path -Parent $resolvedPython) 'python313.dll')
        )
        $basePrefix = (& $resolvedPython -c 'import sys; print(sys.base_prefix)' 2>$null)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($basePrefix)) {
            $dllCandidates += Join-Path $basePrefix.Trim() 'python313.dll'
        }
        foreach ($dllCandidate in $dllCandidates | Select-Object -Unique) {
            if (Test-Path -LiteralPath $dllCandidate -PathType Leaf) {
                return [pscustomobject]@{
                    Executable = $resolvedPython
                    Dll = (Resolve-Path -LiteralPath $dllCandidate).Path
                }
            }
        }
    }

    throw (
        'Synthetic Codec Pack setup tests require local CPython 3.13 x64 identity files. ' +
        'Set LATENTDECK_TEST_PYTHON313 to the matching python.exe.'
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
                    [ordered]@{
                        name = 'latentdeck:excluded-dependency-scopes'
                        value = 'development'
                    }
                    [ordered]@{ name = 'latentdeck:target-platform'; value = 'windows-x86_64' }
                )
            }
        }
        components = @($sbomComponents) + @($nativeComponents)
    } | ConvertTo-Json -Depth 16) + "`n")
}

function New-SyntheticPackFixture {
    param(
        [Parameter(Mandatory)]
        [string]$ParentRoot,

        [Parameter(Mandatory)]
        [string]$PackVersion,

        [Parameter(Mandatory)]
        [string]$Variant,

        [Parameter(Mandatory)]
        [pscustomobject]$Python313,

        [ValidateSet('None', 'Py', 'Pyi')]
        [string]$CredentialPayloadKind = 'None'
    )

    $fixtureRoot = Join-Path $ParentRoot "$PackVersion-$Variant"
    if (Test-Path -LiteralPath $fixtureRoot) {
        throw "Synthetic fixture destination already exists: $fixtureRoot"
    }
    $runtimeSource = Join-Path $fixtureRoot 'runtime-source'
    $packageSource = Join-Path $fixtureRoot 'package-source'
    $packageModule = Join-Path $packageSource 'latentdeck_codec_h3'
    $noticeSource = Join-Path $fixtureRoot 'NOTICE.md'
    $inventorySource = Join-Path $fixtureRoot 'DEPENDENCY_INVENTORY.json'
    $sbomSource = Join-Path $fixtureRoot 'SBOM.cdx.json'
    $assetContract = Join-Path $fixtureRoot 'decoder-asset.json'
    $outputRoot = Join-Path $fixtureRoot 'artifacts'
    [System.IO.Directory]::CreateDirectory($runtimeSource) | Out-Null
    [System.IO.Directory]::CreateDirectory($packageModule) | Out-Null

    [System.IO.File]::Copy(
        $Python313.Executable,
        (Join-Path $runtimeSource 'python.exe'),
        $false
    )
    [System.IO.File]::Copy(
        $Python313.Dll,
        (Join-Path $runtimeSource 'python313.dll'),
        $false
    )
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
        $stdlibZip = [System.IO.Compression.ZipArchive]::new(
            $zipStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            $entry = $stdlibZip.CreateEntry('encodings/__init__.pyc')
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
            $stdlibZip.Dispose()
        }
    } finally {
        $zipStream.Dispose()
    }

    Write-Utf8Text `
        -Path (Join-Path $packageModule '__init__.py') `
        -Content "__version__ = '0.2.0'`nFIXTURE_VARIANT = '$Variant'`n"
    Write-Utf8Text `
        -Path (Join-Path $packageModule 'adapter.py') `
        -Content "def make_adapter():`n    raise RuntimeError('synthetic setup fixture')`n"
    if ($CredentialPayloadKind -cne 'None') {
        $credentialExtension = if ($CredentialPayloadKind -ceq 'Py') { '.py' } else { '.pyi' }
        Write-Utf8Text `
            -Path (Join-Path $packageModule "credential_leak$credentialExtension") `
            -Content "api_key = 'ld_fixture_secret_value_1234567890'`n"
    }

    $protocol2Modules = @(
        'latentdeck_codec_host',
        'latentdeck_codec_sdk',
        'latentdeck_deck_sdk',
        'latentdeck_cartridge',
        'latentdeck_rgb_ring'
    )
    foreach ($moduleName in $protocol2Modules) {
        $moduleRoot = Join-Path $packageSource $moduleName
        [System.IO.Directory]::CreateDirectory($moduleRoot) | Out-Null
        Write-Utf8Text `
            -Path (Join-Path $moduleRoot '__init__.py') `
            -Content "__version__ = '0.2.0'`n"
    }
    foreach ($hostModule in @('__main__.py', 'runtime_v2.py', 'native_cartridge.py')) {
        Write-Utf8Text `
            -Path (Join-Path $packageSource "latentdeck_codec_host/$hostModule") `
            -Content "# synthetic Protocol 2 setup fixture $Variant`n"
    }
    foreach ($nativeModule in @('latentdeck_cartridge', 'latentdeck_rgb_ring')) {
        [System.IO.File]::Copy(
            $Python313.Dll,
            (Join-Path $packageSource "$nativeModule/_native.cp313-win_amd64.pyd"),
            $false
        )
    }
    Write-Utf8Text `
        -Path $noticeSource `
        -Content "Synthetic Codec Pack setup lifecycle fixture $Variant. Not a release payload.`n"
    Write-SyntheticDependencyMetadata `
        -InventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -PackVersion $PackVersion
    Write-Utf8Text -Path $assetContract -Content (([ordered]@{
        asset_id = 'taeh3'
        display_name = 'TAEH3 decoder weight'
        kind = 'decoder_weight'
        required = $true
        selection = 'explicit_file'
        format = 'safetensors'
        accepted_variants = @(
            [ordered]@{
                variant_id = 'madebyollin-taeh3-62f7591f'
                sha256 = '4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13'
                byte_length = 22709752
                source_url = 'https://raw.githubusercontent.com/madebyollin/taehv/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/safetensors/taeh3.safetensors'
                license_label = 'MIT'
                license_url = 'https://github.com/madebyollin/taehv/blob/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/LICENSE'
            }
        )
    } | ConvertTo-Json -Depth 16) + "`n")

    $archiveOutput = @(& (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtimeSource `
        -PackageSource $packageSource `
        -NoticeSource $noticeSource `
        -DependencyInventoryPath $inventorySource `
        -SbomPath $sbomSource `
        -DecoderAssetContractPath $assetContract `
        -PackVersion $PackVersion `
        -OutputDirectory $outputRoot)
    if ($archiveOutput.Count -ne 1 -or
        -not (Test-Path -LiteralPath ([string]$archiveOutput[0]) -PathType Leaf)) {
        throw 'Synthetic Codec Pack builder did not return exactly one archive.'
    }
    $archivePath = (Resolve-Path -LiteralPath ([string]$archiveOutput[0])).Path
    $packageReceipt = Get-Content -Raw -LiteralPath (
        Join-Path (Split-Path -Parent $archivePath) 'package-receipt.json'
    ) | ConvertFrom-Json -Depth 32
    if ([int]$packageReceipt.schema_version -ne 1 -or
        [string]$packageReceipt.pack_id -cne 'org.latentdeck.h3' -or
        [string]$packageReceipt.pack_version -cne $PackVersion -or
        [string]$packageReceipt.adapter_version -cne '0.2.0') {
        throw 'Synthetic H3 package receipt does not bind its exact adapter identity.'
    }
    $expandedRoot = Join-Path $fixtureRoot 'expanded'
    Expand-SafeCodecPackArchive -ArchivePath $archivePath -DestinationPath $expandedRoot
    Test-H3CodecPackDirectory `
        -PackRoot $expandedRoot `
        -ExpectedPackVersion $PackVersion | Out-Null
    $expandedManifest = Get-Content -Raw -LiteralPath (
        Join-Path $expandedRoot 'codec-pack.json'
    ) | ConvertFrom-Json -Depth 20
    $expectedCapabilities = @(
        'player', 'realtime', 'resample', 'snapshot_capture', 'live_capture',
        'raw_import'
    )
    $expectedWorkerPrefix = @('-I', '-s', '-B', '-m', 'latentdeck_codec_host')
    if ((@($expandedManifest.capabilities) -join "`0") -cne
        ($expectedCapabilities -join "`0") -or
        ((@($expandedManifest.worker.arguments)[0..4]) -join "`0") -cne
        ($expectedWorkerPrefix -join "`0") -or
        [int]$expandedManifest.compatibility.worker_protocol -ne 2 -or
        $expandedManifest.adapter.adapter_version -cne '0.2.0') {
        throw 'Synthetic Setup fixture is not the complete H3 Protocol 2 + raw_import pack.'
    }
    $archiveItem = Get-Item -LiteralPath $archivePath
    return [pscustomobject]@{
        Version = $PackVersion
        Variant = $Variant
        ArchivePath = $archivePath
        ArchiveName = $archiveItem.Name
        ArchiveLength = [int64]$archiveItem.Length
        ArchiveSha256 = (
            Get-FileHash -LiteralPath $archivePath -Algorithm SHA256
        ).Hash.ToLowerInvariant()
        ExpandedRoot = $expandedRoot
        OutputRoot = $archiveItem.DirectoryName
    }
}

function New-EquivalentRepackedFixture {
    param(
        [Parameter(Mandatory)]
        [pscustomobject]$Fixture,

        [Parameter(Mandatory)]
        [string]$ArchivePath
    )

    $archiveFullPath = [System.IO.Path]::GetFullPath($ArchivePath)
    if (Test-Path -LiteralPath $archiveFullPath) {
        throw "Equivalent repack destination already exists: $archiveFullPath"
    }
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $archiveFullPath)) | Out-Null
    $packRoot = (Resolve-Path -LiteralPath $Fixture.ExpandedRoot).Path
    $files = @(
        Get-ChildItem -LiteralPath $packRoot -File -Force -Recurse |
            Sort-Object -Property FullName -Descending
    )
    if ($files.Count -eq 0) {
        throw 'Equivalent repack source is empty.'
    }

    $archiveStream = [System.IO.FileStream]::new(
        $archiveFullPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $archiveStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            foreach ($file in $files) {
                $relativePath = [System.IO.Path]::GetRelativePath(
                    $packRoot,
                    $file.FullName
                ).Replace('\', '/')
                $entry = $archive.CreateEntry(
                    $relativePath,
                    [System.IO.Compression.CompressionLevel]::NoCompression
                )
                $entry.LastWriteTime = [System.DateTimeOffset]::new(
                    1980, 1, 1, 0, 0, 0, [System.TimeSpan]::Zero
                )
                $input = [System.IO.File]::Open(
                    $file.FullName,
                    [System.IO.FileMode]::Open,
                    [System.IO.FileAccess]::Read,
                    [System.IO.FileShare]::Read
                )
                try {
                    $output = $entry.Open()
                    try {
                        $input.CopyTo($output)
                    } finally {
                        $output.Dispose()
                    }
                } finally {
                    $input.Dispose()
                }
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $archiveStream.Dispose()
    }

    $archiveItem = Get-Item -LiteralPath $archiveFullPath
    return [pscustomobject]@{
        Version = $Fixture.Version
        Variant = 'equivalent-repacked-tree'
        ArchivePath = $archiveItem.FullName
        ArchiveName = $archiveItem.Name
        ArchiveLength = [int64]$archiveItem.Length
        ArchiveSha256 = (
            Get-FileHash -LiteralPath $archiveItem.FullName -Algorithm SHA256
        ).Hash.ToLowerInvariant()
        ExpandedRoot = $Fixture.ExpandedRoot
        OutputRoot = $archiveItem.DirectoryName
    }
}

function New-InstallArguments {
    param(
        [Parameter(Mandatory)]
        [pscustomobject]$Fixture,

        [string]$ArchivePath
    )

    if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
        $ArchivePath = $Fixture.ArchivePath
    }
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA) -or
        [string]::IsNullOrWhiteSpace($env:PROGRAMDATA)) {
        throw 'Helper lifecycle tests require explicit isolated known-folder roots.'
    }
    return @(
        '--local-app-data', $env:LOCALAPPDATA,
        '--program-data', $env:PROGRAMDATA,
        'install',
        '--archive', $ArchivePath
    )
}

function New-VerifyArguments {
    param(
        [Parameter(Mandatory)]
        [string]$Version
    )

    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA) -or
        [string]::IsNullOrWhiteSpace($env:PROGRAMDATA)) {
        throw 'Helper lifecycle tests require explicit isolated known-folder roots.'
    }
    return @(
        '--local-app-data', $env:LOCALAPPDATA,
        '--program-data', $env:PROGRAMDATA,
        'verify',
        '--version', $Version
    )
}

function New-UninstallArguments {
    param(
        [Parameter(Mandatory)]
        [string]$Version,

        [switch]$RemoveCorrupt
    )

    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA) -or
        [string]::IsNullOrWhiteSpace($env:PROGRAMDATA)) {
        throw 'Helper lifecycle tests require explicit isolated known-folder roots.'
    }
    $arguments = @(
        '--local-app-data', $env:LOCALAPPDATA,
        '--program-data', $env:PROGRAMDATA,
        'uninstall', '--version', $Version
    )
    if ($RemoveCorrupt) {
        $arguments += '--remove-corrupt'
    }
    return $arguments
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null

    $mutationRepository = Join-Path $testRoot 'source-mutation-repository'
    [System.IO.Directory]::CreateDirectory($mutationRepository) | Out-Null
    & git -C $mutationRepository init --initial-branch=main | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not initialize the packaging source-mutation fixture.'
    }
    & git -C $mutationRepository config user.name 'LatentDeck contract test'
    & git -C $mutationRepository config user.email 'contract-test@invalid.example'
    Write-Utf8Text -Path (Join-Path $mutationRepository 'source.txt') -Content "before`n"
    & git -C $mutationRepository add -- source.txt
    & git -C $mutationRepository commit -m 'source fixture' | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not commit the packaging source-mutation fixture.'
    }
    $sourceBeforeMutation = Get-PackagingSourceState -RepositoryRoot $mutationRepository
    Write-Utf8Text -Path (Join-Path $mutationRepository 'source.txt') -Content "after`n"
    $sourceAfterMutation = Get-PackagingSourceState -RepositoryRoot $mutationRepository
    $mutationRejected = $false
    try {
        Assert-PackagingSourceStateUnchanged `
            -Before $sourceBeforeMutation `
            -After $sourceAfterMutation `
            -Context 'Synthetic source'
    } catch {
        if ($_.Exception.Message -like '*changed while the artifact was being built*') {
            $mutationRejected = $true
        } else {
            throw
        }
    }
    Assert-Condition `
        -Condition $mutationRejected `
        -Message 'Packaging source mutation was not rejected by the shared H3 source-state gate.'

    $fullPackBuilderCommand = Get-Command (
        Join-Path $PSScriptRoot 'Build-H3CodecPack.ps1'
    )
    $lowLevelPackBuilderCommand = Get-Command (
        Join-Path $PSScriptRoot 'New-H3CodecPack.ps1'
    )
    $setupBuilderCommand = Get-Command (
        Join-Path $PSScriptRoot 'Build-H3CodecPackInstaller.ps1'
    )
    $fullPackVersionIsMandatory = @(
        $fullPackBuilderCommand.Parameters['PackVersion'].Attributes |
            Where-Object { $_ -is [System.Management.Automation.ParameterAttribute] }
    ).Mandatory -contains $true
    $lowLevelPackVersionIsMandatory = @(
        $lowLevelPackBuilderCommand.Parameters['PackVersion'].Attributes |
            Where-Object { $_ -is [System.Management.Automation.ParameterAttribute] }
    ).Mandatory -contains $true
    Assert-Condition `
        -Condition (
            $fullPackBuilderCommand.Parameters.ContainsKey('AllowNetwork') -and
            $fullPackBuilderCommand.Parameters.ContainsKey('SigningCommand') -and
            $fullPackVersionIsMandatory -and
            $lowLevelPackVersionIsMandatory -and
            -not $setupBuilderCommand.Parameters.ContainsKey('LifecycleHelperPath') -and
            -not $setupBuilderCommand.Parameters.ContainsKey('ValidatedPackRoot')
        ) `
        -Message (
            'H3 builders do not preserve the installer network/signing boundaries ' +
            'or expose an untrusted prebuilt-helper bypass.'
        )

    $fullPackBuilderSource = Get-Content -LiteralPath $fullPackBuilderCommand.Source -Raw
    $setupBuilderSource = Get-Content -LiteralPath $setupBuilderCommand.Source -Raw
    $sourceCaptureIndex = $fullPackBuilderSource.IndexOf(
        '$sourceBefore = Get-PackagingSourceState',
        [System.StringComparison]::Ordinal
    )
    $artifactCreationIndex = $fullPackBuilderSource.IndexOf(
        '[System.IO.Directory]::CreateDirectory($artifactsRoot)',
        [System.StringComparison]::Ordinal
    )
    Assert-Condition `
        -Condition (
            $sourceCaptureIndex -ge 0 -and
            $artifactCreationIndex -gt $sourceCaptureIndex -and
            $fullPackBuilderSource -cmatch 'Assert-PackagingSourceStateUnchanged' -and
            $fullPackBuilderSource -cmatch 'public_snapshot_sha256' -and
            $fullPackBuilderSource -cmatch 'sidecars = \$sidecars'
        ) `
        -Message 'Full H3 build no longer captures, rechecks, and receipt-binds its public source snapshot.'
    Assert-Condition `
        -Condition (
            $setupBuilderSource.IndexOf(
                '$sourceBefore = Get-PackagingSourceState',
                [System.StringComparison]::Ordinal
            ) -ge 0 -and
            $setupBuilderSource.IndexOf(
                '[System.IO.Directory]::CreateDirectory($artifactsRoot)',
                [System.StringComparison]::Ordinal
            ) -gt $setupBuilderSource.IndexOf(
                '$sourceBefore = Get-PackagingSourceState',
                [System.StringComparison]::Ordinal
            ) -and
            $setupBuilderSource -cmatch 'Assert-PackagingSourceStateUnchanged' -and
            $setupBuilderSource -cmatch '\$publicationRoot\s*=\s*Join-Path \$workRoot' -and
            $setupBuilderSource -cmatch '"/DOUTPUT_PATH=\$setupStagePath"' -and
            $setupBuilderSource -cmatch '\[System\.IO\.File\]::Move\(\$publication\.Source, \$publication\.Destination, \$false\)' -and
            $setupBuilderSource -cmatch 'Length -gt 1MB'
        ) `
        -Message 'H3 setup builder no longer uses bounded private staging and no-overwrite publication.'

    $builderEntrypointProbeArchive = Join-Path $testRoot 'python-3.13.14-embed-amd64.zip'
    [System.IO.File]::WriteAllBytes($builderEntrypointProbeArchive, [byte[]]@(0))
    $builderEntrypointProbe = Invoke-NativeResult `
        -Executable (Get-Command pwsh).Source `
        -Arguments @(
            '-NoProfile',
            '-File',
            $fullPackBuilderCommand.Source,
            '-PythonEmbedArchive',
            $builderEntrypointProbeArchive,
            '-PackVersion',
            '0.2.1',
            '-OutputDirectory',
            (Join-Path $testRoot 'builder-entrypoint-probe-output'),
            '-DevelopmentBuild'
        )
    Assert-Condition `
        -Condition (
            $builderEntrypointProbe.ExitCode -ne 0 -and
            $builderEntrypointProbe.Output -like (
                '*CPython runtime archive SHA-256 does not match the curation lock.*'
            )
        ) `
        -Message (
            'The production H3 builder entrypoint did not load its packaging modules before ' +
            "validating inputs. Output: $($builderEntrypointProbe.Output)"
        )

    $offlineProbeRoot = Join-Path $testRoot 'offline-nsis-probe'
    $offlineProbeTools = Join-Path $offlineProbeRoot 'tools'
    [System.IO.Directory]::CreateDirectory($offlineProbeTools) | Out-Null
    $offlineProbeScript = Join-Path $offlineProbeTools 'Get-PinnedNsis.ps1'
    [System.IO.File]::Copy(
        (Join-Path $PSScriptRoot 'Get-PinnedNsis.ps1'),
        $offlineProbeScript,
        $false
    )
    $offlineProbeResult = & {
        function Invoke-WebRequest {
            throw '__NETWORK_ATTEMPTED_WITHOUT_ALLOW_NETWORK__'
        }
        try {
            & $offlineProbeScript | Out-Null
            return [pscustomobject]@{ Succeeded = $true; Message = '' }
        } catch {
            return [pscustomobject]@{
                Succeeded = $false
                Message = $_.Exception.Message
            }
        }
    }
    Assert-Condition `
        -Condition (
            -not $offlineProbeResult.Succeeded -and
            $offlineProbeResult.Message -match '(?i)offline' -and
            $offlineProbeResult.Message -notmatch '__NETWORK_ATTEMPTED_WITHOUT_ALLOW_NETWORK__'
        ) `
        -Message (
            'Get-PinnedNsis did not fail offline before a network attempt. Result: ' +
            $offlineProbeResult.Message
        )

    $pinnedNsisParameters = @{}
    if (-not [string]::IsNullOrWhiteSpace($NsisRoot)) {
        $pinnedNsisParameters.NsisRoot = $NsisRoot
    }
    $pinnedNsisRoot = (
        & (Join-Path $PSScriptRoot 'Get-PinnedNsis.ps1') @pinnedNsisParameters
    ).Trim()
    $tamperedNsisRoot = Join-Path $testRoot 'tampered-nsis-3.11'
    Copy-Item -LiteralPath $pinnedNsisRoot -Destination $tamperedNsisRoot -Recurse
    $tamperedNsisFile = @(
        Get-ChildItem -LiteralPath $tamperedNsisRoot -File -Force -Recurse |
            Where-Object {
                $relative = [System.IO.Path]::GetRelativePath(
                    $tamperedNsisRoot,
                    $_.FullName
                ).Replace('\', '/')
                $relative -cnotin @('makensis.exe', 'Bin/makensis.exe', 'COPYING')
            } |
            Select-Object -First 1
    )
    if ($tamperedNsisFile.Count -ne 1) {
        throw 'Could not select a non-core NSIS tree file for the tamper regression.'
    }
    $tamperStream = [System.IO.File]::Open(
        $tamperedNsisFile[0].FullName,
        [System.IO.FileMode]::Append,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $tamperStream.WriteByte(0)
    } finally {
        $tamperStream.Dispose()
    }
    $explicitNsisFailure = $null
    try {
        & (Join-Path $PSScriptRoot 'Get-PinnedNsis.ps1') `
            -NsisRoot $tamperedNsisRoot | Out-Null
    } catch {
        $explicitNsisFailure = $_.Exception.Message
    }
    Assert-Condition `
        -Condition (
            -not [string]::IsNullOrWhiteSpace($explicitNsisFailure) -and
            $explicitNsisFailure -match '(?i)tree SHA-256 mismatch'
        ) `
        -Message (
            'Explicit NSIS root was not rejected by the full pinned-tree identity. Result: ' +
            $explicitNsisFailure
        )

    $installerTemplateText = Get-Content -Raw -LiteralPath (
        Join-Path $PSScriptRoot 'installer/H3CodecPackInstaller.nsi'
    )
    $pluginHelperEmbeds = [regex]::Matches(
        $installerTemplateText,
        '(?ms)InitPluginsDir\s+SetOutPath "\$PLUGINSDIR".*?' +
        'File "/oname=\$\{HELPER_FILE\}" "\$\{HELPER_PATH\}"'
    ).Count
    $maintenancePublishWindow = [regex]::Match(
        $installerTemplateText,
        '(?ms)WriteUninstaller "\$\{MAINTENANCE_STAGE\}\\Uninstall\.exe"\s+' +
        'IfErrors maintenance_failed(?<window>.*?)' +
        'maintenance_publish_stage:\s+' +
        'ClearErrors\s+' +
        'Rename "\$\{MAINTENANCE_STAGE\}" "\$INSTDIR"'
    )
    $maintenancePublishSetOutPaths = @(
        if ($maintenancePublishWindow.Success) {
            [regex]::Matches(
            $maintenancePublishWindow.Groups['window'].Value,
            '(?m)^\s*SetOutPath\b[^\r\n]*\r?$'
            )
        }
    )
    $maintenanceStageLeavesCurrentDirectory =
        $maintenancePublishWindow.Success -and
        $maintenancePublishSetOutPaths.Count -eq 1 -and
        $maintenancePublishSetOutPaths[0].Value.Trim() -ceq 'SetOutPath "$PLUGINSDIR"'
    $maintenanceRollbackWindow = [regex]::Match(
        $installerTemplateText,
        '(?ms)maintenance_rollback_complete:(?<window>.*?)' +
        'maintenance_rollback_cleanup_failed:'
    )
    $maintenanceRollbackDeleteKeys = @(
        if ($maintenanceRollbackWindow.Success) {
            [regex]::Matches(
            $maintenanceRollbackWindow.Groups['window'].Value,
            '(?m)^\s*DeleteRegKey\b[^\r\n]*\r?$'
            )
        }
    )
    $installSectionPrologue = [regex]::Match(
        $installerTemplateText,
        '(?ms)Section "Install H3 Codec Pack" SEC_MAIN(?<window>.*?)payload_present:'
    )
    $maintenanceRollbackIsTransactionScoped =
        $installerTemplateText -match '(?m)^Var MaintenanceRegistryTouched\s*$' -and
        $installSectionPrologue.Success -and
        $installSectionPrologue.Groups['window'].Value -match (
            '(?m)^\s*StrCpy \$MaintenanceRegistryTouched "0"\s*$'
        ) -and
        $installerTemplateText -match (
            '(?ms)WriteRegStr HKCU "\$\{UNINSTALL_KEY\}" "DisplayName".*?' +
            'IfErrors maintenance_registry_failed\s+' +
            'StrCpy \$MaintenanceRegistryTouched "1"'
        ) -and
        $installerTemplateText -match (
            '(?ms)maintenance_failed:\s+' +
            '(?:;[^\r\n]*\r?\n\s*)*' +
            'SetOutPath "\$PLUGINSDIR"\s+' +
            'DetailPrint "Failed to create Installed Apps maintenance data\."'
        ) -and
        $maintenanceRollbackWindow.Success -and
        $maintenanceRollbackDeleteKeys.Count -eq 1 -and
        $maintenanceRollbackWindow.Groups['window'].Value -match (
            '(?ms)\$\{If\} \$MaintenanceRegistryTouched == "1"\s+' +
            'ClearErrors\s+' +
            'DeleteRegKey HKCU "\$\{UNINSTALL_KEY\}"\s+' +
            'IfErrors maintenance_rollback_cleanup_failed\s+' +
            '\$\{EndIf\}'
        )
    Assert-Condition `
        -Condition (
            $pluginHelperEmbeds -eq 2 -and
            $maintenanceStageLeavesCurrentDirectory -and
            $maintenanceRollbackIsTransactionScoped -and
            $installerTemplateText -notmatch '(?m)nsExec::ExecToStack.*\$INSTDIR\\\$\{HELPER_FILE\}' -and
            $installerTemplateText -match '(?m)^\s*StrCmp \$9 "INSTALLER_RUST_LICENSES\.txt" uninstall_inventory_known\s*$' -and
            $installerTemplateText -match '(?ms)uninstall_inventory_unsafe:.*?Goto uninstall_maintenance_root_unsafe' -and
            $installerTemplateText -match '(?ms)uninstall_maintenance_root_unsafe:.*?SetErrorLevel 40.*?Abort'
        ) `
        -Message (
            'NSIS template does not embed the helper only through $PLUGINSDIR or ' +
            'leave the maintenance stage before publication/rollback, scope rollback ' +
            'registry deletion to this transaction, or fail closed on an unknown ' +
            'maintenance entry before uninstall mutation.'
        )

    $syntheticCargoMetadata = [pscustomobject]@{
        resolve = [pscustomobject]@{
            nodes = @(
                [pscustomobject]@{
                    id = 'root'
                    deps = @(
                        [pscustomobject]@{
                            pkg = 'normal'
                            dep_kinds = @([pscustomobject]@{ kind = $null })
                        },
                        [pscustomobject]@{
                            pkg = 'build'
                            dep_kinds = @([pscustomobject]@{ kind = 'build' })
                        },
                        [pscustomobject]@{
                            pkg = 'dev-only'
                            dep_kinds = @([pscustomobject]@{ kind = 'dev' })
                        }
                    )
                },
                [pscustomobject]@{
                    id = 'normal'
                    deps = @([pscustomobject]@{
                        pkg = 'nested-dev-only'
                        dep_kinds = @([pscustomobject]@{ kind = 'dev' })
                    })
                },
                [pscustomobject]@{
                    id = 'build'
                    deps = @([pscustomobject]@{
                        pkg = 'build-normal-child'
                        dep_kinds = @([pscustomobject]@{ kind = $null })
                    })
                },
                [pscustomobject]@{ id = 'dev-only'; deps = @() },
                [pscustomobject]@{ id = 'nested-dev-only'; deps = @() },
                [pscustomobject]@{ id = 'build-normal-child'; deps = @() }
            )
        }
    }
    $syntheticClosure = @(
        Get-CargoNormalBuildDependencyIds `
            -Metadata $syntheticCargoMetadata `
            -RootPackageId 'root'
    )
    Assert-Condition `
        -Condition (
            ($syntheticClosure -join ',') -ceq 'build,build-normal-child,normal,root'
        ) `
        -Message (
            'Cargo dependency closure included dev-only edges or lost normal/build edges: ' +
            ($syntheticClosure -join ',')
        )

    $reproSbomOne = Join-Path $testRoot 'repro-installer-sbom-one.json'
    $reproSbomTwo = Join-Path $testRoot 'repro-installer-sbom-two.json'
    & (Join-Path $PSScriptRoot 'New-H3CodecPackInstallerSbom.ps1') `
        -PackVersion '0.2.1' `
        -OutputPath $reproSbomOne `
        -NsisRoot $pinnedNsisRoot | Out-Null
    & (Join-Path $PSScriptRoot 'New-H3CodecPackInstallerSbom.ps1') `
        -PackVersion '0.2.1' `
        -OutputPath $reproSbomTwo `
        -NsisRoot $pinnedNsisRoot | Out-Null
    $reproSbomOneBytes = [System.IO.File]::ReadAllBytes($reproSbomOne)
    $reproSbomTwoBytes = [System.IO.File]::ReadAllBytes($reproSbomTwo)
    Assert-Condition `
        -Condition (
            $reproSbomOneBytes.Length -eq $reproSbomTwoBytes.Length -and
            [System.Convert]::ToBase64String($reproSbomOneBytes) -ceq
                [System.Convert]::ToBase64String($reproSbomTwoBytes) -and
            (Get-FileHash -LiteralPath $reproSbomOne -Algorithm SHA256).Hash -ceq
                (Get-FileHash -LiteralPath $reproSbomTwo -Algorithm SHA256).Hash
        ) `
        -Message 'Installer SBOM is not byte-for-byte reproducible from the same locked inputs.'

    $rustLicensesOne = Join-Path $testRoot 'installer-rust-licenses-one.txt'
    $rustLicensesTwo = Join-Path $testRoot 'installer-rust-licenses-two.txt'
    & (Join-Path $PSScriptRoot 'New-H3CodecPackInstallerRustLicenses.ps1') `
        -PackVersion '0.2.1' `
        -OutputPath $rustLicensesOne | Out-Null
    & (Join-Path $PSScriptRoot 'New-H3CodecPackInstallerRustLicenses.ps1') `
        -PackVersion '0.2.1' `
        -OutputPath $rustLicensesTwo | Out-Null
    $rustLicensesOneBytes = [System.IO.File]::ReadAllBytes($rustLicensesOne)
    $rustLicensesTwoBytes = [System.IO.File]::ReadAllBytes($rustLicensesTwo)
    $rustLicensesText = [System.Text.UTF8Encoding]::new(
        $false,
        $true
    ).GetString($rustLicensesOneBytes)
    $reproInstallerSbom = Get-Content -LiteralPath $reproSbomOne -Raw |
        ConvertFrom-Json -Depth 100
    $installerSbomComponentsWithoutLicense = @(
        $reproInstallerSbom.components |
            Where-Object {
                $null -eq $_.PSObject.Properties['licenses'] -or @($_.licenses).Count -eq 0
            }
    )
    $installerRootScopeProperties = @($reproInstallerSbom.metadata.component.properties)
    $installerInvalidComponentScopes = @($reproInstallerSbom.components | Where-Object {
        @($_.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope' -and
            [string]$_.value -cin @('artifact', 'runtime', 'build', 'runtime+build')
        }).Count -ne 1
    })
    Assert-Condition `
        -Condition (
            @($reproInstallerSbom.metadata.component.licenses).Count -eq 1 -and
            $installerSbomComponentsWithoutLicense.Count -eq 0 -and
            @($installerRootScopeProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                [string]$_.value -ceq 'artifact'
            }).Count -eq 1 -and
            @($installerRootScopeProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:included-dependency-scopes' -and
                [string]$_.value -ceq 'artifact,runtime,build,runtime+build'
            }).Count -eq 1 -and
            @($installerRootScopeProperties | Where-Object {
                [string]$_.name -ceq 'latentdeck:excluded-dependency-scopes' -and
                [string]$_.value -ceq 'development'
            }).Count -eq 1 -and
            $installerInvalidComponentScopes.Count -eq 0
        ) `
        -Message 'Installer SBOM license review is incomplete.'
    $rustSbomComponents = @(
        $reproInstallerSbom.components |
            Where-Object {
                @($_.properties | Where-Object {
                    $_.name -ceq 'latentdeck:ecosystem' -and $_.value -ceq 'rust'
                }).Count -eq 1
            }
    )
    $missingRustLicenseSections = @(
        $rustSbomComponents |
            Where-Object {
                $rustLicensesText -cnotmatch (
                    '(?m)^Package: ' +
                    [regex]::Escape([string]$_.name) + ' ' +
                    [regex]::Escape([string]$_.version) + '$'
                )
            }
    )
    Assert-Condition `
        -Condition (
            $rustLicensesOneBytes.Length -gt 0 -and
            $rustLicensesOneBytes.Length -eq $rustLicensesTwoBytes.Length -and
            [System.Convert]::ToBase64String($rustLicensesOneBytes) -ceq
                [System.Convert]::ToBase64String($rustLicensesTwoBytes) -and
            $rustSbomComponents.Count -gt 0 -and
            $missingRustLicenseSections.Count -eq 0 -and
            $rustLicensesText.Contains(
                'Reviewed upstream source commit: 6af37d89619fdcb06d8ab82d02dbe6b3d1a4d1a7',
                [System.StringComparison]::Ordinal
            ) -and
            $rustLicensesText.Contains(
                'Reviewed upstream source commit: d74c030d9dc4f3cae02146d1f497ff62726ef09a',
                [System.StringComparison]::Ordinal
            ) -and
            $rustLicensesText.IndexOf($repoRoot, [System.StringComparison]::OrdinalIgnoreCase) -lt 0 -and
            $rustLicensesText.IndexOf($env:USERPROFILE, [System.StringComparison]::OrdinalIgnoreCase) -lt 0 -and
            $rustLicensesText -cnotmatch '(?m)(?<![A-Za-z])[A-Z]:[\\/]'
        ) `
        -Message 'Installer Rust license bundle is incomplete, non-deterministic, or path-bearing.'

    $python313 = Resolve-TestPython313
    $credentialFixtureResults = @{}
    foreach ($credentialPayload in @(
        [pscustomobject]@{ Kind = 'Py'; Extension = '.py' },
        [pscustomobject]@{ Kind = 'Pyi'; Extension = '.pyi' }
    )) {
        $credentialFailure = $null
        try {
            New-SyntheticPackFixture `
                -ParentRoot $testRoot `
                -PackVersion '0.2.0' `
                -Variant "credential-leak-$($credentialPayload.Kind.ToLowerInvariant())" `
                -Python313 $python313 `
                -CredentialPayloadKind $credentialPayload.Kind | Out-Null
        } catch {
            $credentialFailure = $_.Exception.Message
        }
        $credentialFixtureResults[$credentialPayload.Extension] = $credentialFailure
    }
    foreach ($credentialExtension in @('.py', '.pyi')) {
        $expectedCredentialFailure = (
            "packaging source 'latentdeck_codec_h3/credential_leak$credentialExtension' " +
            'contains credential-like material.'
        )
        Assert-Condition `
            -Condition (
                [string]$credentialFixtureResults[$credentialExtension] -ceq
                    $expectedCredentialFailure
            ) `
            -Message (
                "Codec Pack builder accepted credential-like material in $credentialExtension. " +
                'Result: ' + [string]$credentialFixtureResults[$credentialExtension]
            )
    }
    $fixture010 = New-SyntheticPackFixture `
        -ParentRoot $testRoot `
        -PackVersion '0.2.0' `
        -Variant 'base' `
        -Python313 $python313
    $fixture011 = New-SyntheticPackFixture `
        -ParentRoot $testRoot `
        -PackVersion '0.2.1' `
        -Variant 'base' `
        -Python313 $python313
    $fixture011Different = New-SyntheticPackFixture `
        -ParentRoot $testRoot `
        -PackVersion '0.2.1' `
        -Variant 'different-bytes' `
        -Python313 $python313
    $fixture011Equivalent = New-EquivalentRepackedFixture `
        -Fixture $fixture011 `
        -ArchivePath (Join-Path $testRoot 'equivalent-repacked-0.2.1.ldcodec')
    Assert-Condition `
        -Condition ($fixture011.ArchiveSha256 -cne $fixture011Different.ArchiveSha256) `
        -Message 'Same-version immutability fixtures unexpectedly have identical hashes.'
    Assert-Condition `
        -Condition ($fixture011.ArchiveSha256 -cne $fixture011Equivalent.ArchiveSha256) `
        -Message 'Equivalent-tree repack unexpectedly retained the original archive hash.'

    $fixtureSbomPath = Join-Path $fixture011.ExpandedRoot 'SBOM.cdx.json'
    $fixtureSbomOriginal = [System.IO.File]::ReadAllText($fixtureSbomPath)
    $fixtureSbom = $fixtureSbomOriginal | ConvertFrom-Json -Depth 32
    $fixtureSbom.components[0].hashes[0].content = ('f' * 64)
    Write-Utf8Text `
        -Path $fixtureSbomPath `
        -Content (($fixtureSbom | ConvertTo-Json -Depth 32) + "`n")
    $packagingModule = Get-Module -Name CodecPackPackaging | Select-Object -First 1
    $provenanceFailure = & $packagingModule {
        param($PackRoot, $PackVersion)
        try {
            Assert-CodecPackDependencyMetadata -PackRoot $PackRoot -PackVersion $PackVersion
            return $null
        } catch {
            return $_.Exception.Message
        }
    } $fixture011.ExpandedRoot $fixture011.Version
    Assert-Condition `
        -Condition ([string]$provenanceFailure -match '(?i)incomplete provenance') `
        -Message 'Codec Pack dependency validation accepted a false SBOM content digest.'
    Write-Utf8Text -Path $fixtureSbomPath -Content $fixtureSbomOriginal

    $duplicateSumsRoot = Join-Path $testRoot 'duplicate-sums-artifacts'
    [System.IO.Directory]::CreateDirectory($duplicateSumsRoot) | Out-Null
    $duplicateSumsArchive = Join-Path $duplicateSumsRoot $fixture011.ArchiveName
    [System.IO.File]::Copy($fixture011.ArchivePath, $duplicateSumsArchive, $false)
    $duplicateArchiveLine = "$($fixture011.ArchiveSha256)  $($fixture011.ArchiveName)"
    Write-Utf8Text `
        -Path (Join-Path $duplicateSumsRoot 'SHA256SUMS.txt') `
        -Content "$duplicateArchiveLine`n$duplicateArchiveLine`n"
    $duplicateSumsFailure = $null
    try {
        & (Join-Path $PSScriptRoot 'Build-H3CodecPackInstaller.ps1') `
            -ArchivePath $duplicateSumsArchive `
            -PackVersion '0.2.1' `
            -OutputDirectory $duplicateSumsRoot | Out-Null
    } catch {
        $duplicateSumsFailure = $_.Exception.Message
    }
    Assert-Condition `
        -Condition (
            -not [string]::IsNullOrWhiteSpace($duplicateSumsFailure) -and
            $duplicateSumsFailure -match '(?i)(?:contain )?only the exact selected' -and
            -not (Test-Path -LiteralPath (
                Join-Path $duplicateSumsRoot 'LatentDeck-H3-CodecPack-0.2.1-setup.exe'
            ))
        ) `
        -Message (
            'Setup builder accepted duplicate payload entries in SHA256SUMS.txt. Result: ' +
            [string]$duplicateSumsFailure
        )

    $tamperedNsisSbomFailure = $null
    try {
        & (Join-Path $PSScriptRoot 'New-H3CodecPackInstallerSbom.ps1') `
            -PackVersion '0.2.1' `
            -OutputPath (Join-Path $testRoot 'tampered-nsis-installer-sbom.json') `
            -NsisRoot $tamperedNsisRoot | Out-Null
    } catch {
        $tamperedNsisSbomFailure = $_.Exception.Message
    }
    Assert-Condition `
        -Condition (
            -not [string]::IsNullOrWhiteSpace($tamperedNsisSbomFailure) -and
            $tamperedNsisSbomFailure -match '(?i)tree SHA-256 mismatch'
        ) `
        -Message (
            'Installer SBOM accepted an explicit non-canonical NSIS tree. Result: ' +
            $tamperedNsisSbomFailure
        )

    $tamperedNsisBuilderFailure = $null
    $tamperedBuilderParameters = @{
        ArchivePath = $fixture011.ArchivePath
        PackVersion = '0.2.1'
        OutputDirectory = $fixture011.OutputRoot
        NsisRoot = $tamperedNsisRoot
    }
    try {
        & (Join-Path $PSScriptRoot 'Build-H3CodecPackInstaller.ps1') `
            @tamperedBuilderParameters | Out-Null
    } catch {
        $tamperedNsisBuilderFailure = $_.Exception.Message
    }
    Assert-Condition `
        -Condition (
            -not [string]::IsNullOrWhiteSpace($tamperedNsisBuilderFailure) -and
            $tamperedNsisBuilderFailure -match '(?i)tree SHA-256 mismatch'
        ) `
        -Message (
            'Setup builder accepted an explicit non-canonical NSIS tree. Result: ' +
            $tamperedNsisBuilderFailure
        )

    $buildParameters = @{
        ArchivePath = $fixture011.ArchivePath
        PackVersion = '0.2.1'
        OutputDirectory = $fixture011.OutputRoot
    }
    if (-not [string]::IsNullOrWhiteSpace($NsisRoot)) {
        $buildParameters.NsisRoot = $NsisRoot
    }
    $expectedSetupSource = Get-PackagingSourceState -RepositoryRoot $repoRoot
    $setupOutput = @(& (Join-Path $PSScriptRoot 'Build-H3CodecPackInstaller.ps1') @buildParameters)
    if ($setupOutput.Count -eq 0) {
        throw 'Codec Pack setup builder did not return its setup executable path.'
    }
    $reportedSetupPath = [string]$setupOutput[-1]
    if (-not (Test-Path -LiteralPath $reportedSetupPath -PathType Leaf)) {
        throw 'Codec Pack setup builder did not end with a valid setup executable path.'
    }
    $setupPath = (Resolve-Path -LiteralPath $reportedSetupPath).Path
    $expectedSetupName = 'LatentDeck-H3-CodecPack-0.2.1-setup.exe'
    Assert-Condition `
        -Condition ((Split-Path -Leaf $setupPath) -ceq $expectedSetupName) `
        -Message 'Codec Pack setup name is not canonical.'
    Assert-WindowsPe `
        -Path $setupPath `
        -ExpectedMachine 0x014C `
        -Context 'Codec Pack NSIS setup'
    $setupItem = Get-Item -LiteralPath $setupPath
    Assert-Condition `
        -Condition ($setupItem.Length -lt 64MB) `
        -Message 'Codec Pack setup embedded the payload or exceeded the small-bootstrapper limit.'

    $productionHelperPath = Join-Path `
        $artifactsRoot `
        'codec-pack-installer-target/x86_64-pc-windows-msvc/release/latentdeck-codec-pack-installer.exe'
    $productionHelperPath = (Resolve-Path -LiteralPath $productionHelperPath).Path
    Assert-WindowsPe `
        -Path $productionHelperPath `
        -ExpectedMachine 0x8664 `
        -Context 'Production Codec Pack lifecycle helper'
    $productionHelperSha256 = (
        Get-FileHash -LiteralPath $productionHelperPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()

    if ([string]::IsNullOrWhiteSpace($LifecycleHelperPath)) {
        $testHelperAuthorizationPath = Join-Path $testRoot 'test-helper-authorization.json'
        $testHelperAuthorization = [ordered]@{
            schema_version = 1
            packages = @(
                [ordered]@{
                    pack_id = 'org.latentdeck.h3'
                    pack_version = $fixture010.Version
                    archive_sha256 = $fixture010.ArchiveSha256
                    archive_byte_length = [int64]$fixture010.ArchiveLength
                },
                [ordered]@{
                    pack_id = 'org.latentdeck.h3'
                    pack_version = $fixture011.Version
                    archive_sha256 = $fixture011.ArchiveSha256
                    archive_byte_length = [int64]$fixture011.ArchiveLength
                }
            )
        }
        Write-Utf8Text `
            -Path $testHelperAuthorizationPath `
            -Content (($testHelperAuthorization | ConvertTo-Json -Depth 10) + "`n")
        $testHelperTargetRoot = Join-Path $testRoot 'codec-pack-installer-test-target'
        $savedTargetDirectory = $env:CARGO_TARGET_DIR
        $savedRustFlags = $env:RUSTFLAGS
        $savedAuthorizationFile = $env:LATENTDECK_H3_AUTHORIZATION_FILE
        try {
            $env:CARGO_TARGET_DIR = $testHelperTargetRoot
            $env:LATENTDECK_H3_AUTHORIZATION_FILE = $testHelperAuthorizationPath
            if ([string]::IsNullOrWhiteSpace($savedRustFlags)) {
                $env:RUSTFLAGS = '-C target-feature=+crt-static'
            } else {
                $env:RUSTFLAGS = "$savedRustFlags -C target-feature=+crt-static"
            }
            & cargo build --locked --release --offline `
                --target x86_64-pc-windows-msvc `
                --package latentdeck-codec-pack-installer
            if ($LASTEXITCODE -ne 0) {
                throw "Test lifecycle helper build failed with exit code $LASTEXITCODE."
            }
        } finally {
            $env:CARGO_TARGET_DIR = $savedTargetDirectory
            $env:RUSTFLAGS = $savedRustFlags
            $env:LATENTDECK_H3_AUTHORIZATION_FILE = $savedAuthorizationFile
        }
        $LifecycleHelperPath = Join-Path `
            $testHelperTargetRoot `
            'x86_64-pc-windows-msvc/release/latentdeck-codec-pack-installer.exe'
    }
    $LifecycleHelperPath = (Resolve-Path -LiteralPath $LifecycleHelperPath).Path
    Assert-WindowsPe `
        -Path $LifecycleHelperPath `
        -ExpectedMachine 0x8664 `
        -Context 'Codec Pack lifecycle helper'

    $receiptPath = Join-Path $fixture011.OutputRoot 'setup-receipt.json'
    $receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -Depth 30
    $setupSha256 = (
        Get-FileHash -LiteralPath $setupPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $installerSbomPath = Join-Path $fixture011.OutputRoot 'installer-SBOM.cdx.json'
    $installerNoticesPath = Join-Path $fixture011.OutputRoot 'INSTALLER_THIRD_PARTY_NOTICES.md'
    $installerNsisCopyingPath = Join-Path $fixture011.OutputRoot 'INSTALLER_NSIS_COPYING.txt'
    $installerRustLicensesPath = Join-Path $fixture011.OutputRoot 'INSTALLER_RUST_LICENSES.txt'
    $installerSbomItem = Get-Item -LiteralPath $installerSbomPath
    $installerNoticesItem = Get-Item -LiteralPath $installerNoticesPath
    $installerNsisCopyingItem = Get-Item -LiteralPath $installerNsisCopyingPath
    $installerRustLicensesItem = Get-Item -LiteralPath $installerRustLicensesPath
    $installerSbomSha256 = (
        Get-FileHash -LiteralPath $installerSbomPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $installerNoticesSha256 = (
        Get-FileHash -LiteralPath $installerNoticesPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $expectedInstallerNoticesSha256 = (
        Get-FileHash `
            -LiteralPath (Join-Path $PSScriptRoot 'installer/H3CodecPackInstallerNotices.md') `
            -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $installerNsisCopyingSha256 = (
        Get-FileHash -LiteralPath $installerNsisCopyingPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $installerRustLicensesSha256 = (
        Get-FileHash -LiteralPath $installerRustLicensesPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $expectedInstallerRustLicensesSha256 = (
        Get-FileHash -LiteralPath $rustLicensesOne -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    Assert-LowercaseSha256 -Value $setupSha256 -Context 'setup SHA-256'
    Assert-Condition `
        -Condition (
            $receipt.schema_version -eq 1 -and
            $receipt.pack_id -ceq 'org.latentdeck.h3' -and
            $receipt.pack_version -ceq '0.2.1' -and
            $receipt.setup.name -ceq $expectedSetupName -and
            [int64]$receipt.setup.byte_length -eq [int64]$setupItem.Length -and
            $receipt.setup.sha256 -ceq $setupSha256 -and
            $receipt.setup.format -ceq 'nsis' -and
            $receipt.setup.scope -ceq 'current_user' -and
            $receipt.setup.payload_delivery -ceq 'adjacent_hash_bound_ldcodec' -and
            $receipt.payload.name -ceq $fixture011.ArchiveName -and
            [int64]$receipt.payload.byte_length -eq $fixture011.ArchiveLength -and
            $receipt.payload.sha256 -ceq $fixture011.ArchiveSha256 -and
            $receipt.helper.sha256 -ceq $productionHelperSha256 -and
            $receipt.helper.static_crt -eq $true -and
            $receipt.helper.delivery -ceq 'embedded_in_setup_and_uninstaller' -and
            $receipt.helper.installed_as_loose_file -eq $false -and
            $receipt.helper.authorization.source -ceq 'build_generated_exact_allowlist' -and
            $receipt.helper.authorization.pack_version -ceq $fixture011.Version -and
            $receipt.helper.authorization.archive_sha256 -ceq $fixture011.ArchiveSha256 -and
            [int64]$receipt.helper.authorization.archive_byte_length -eq $fixture011.ArchiveLength -and
            $receipt.sbom.name -ceq 'installer-SBOM.cdx.json' -and
            [int64]$receipt.sbom.byte_length -eq [int64]$installerSbomItem.Length -and
            $receipt.sbom.sha256 -ceq $installerSbomSha256 -and
            [int]$receipt.sbom.component_count -gt 1 -and
            $receipt.sbom.license_review -ceq 'complete' -and
            [int]$receipt.sbom.missing_license_component_count -eq 0 -and
            $receipt.notices.name -ceq 'INSTALLER_THIRD_PARTY_NOTICES.md' -and
            [int64]$receipt.notices.byte_length -eq [int64]$installerNoticesItem.Length -and
            $receipt.notices.sha256 -ceq $installerNoticesSha256 -and
            $installerNoticesSha256 -ceq $expectedInstallerNoticesSha256 -and
            $receipt.notices.nsis_copying_name -ceq 'INSTALLER_NSIS_COPYING.txt' -and
            [int64]$receipt.notices.nsis_copying_byte_length -eq [int64]$installerNsisCopyingItem.Length -and
            $receipt.notices.nsis_copying_sha256 -ceq $installerNsisCopyingSha256 -and
            $receipt.notices.rust_licenses_name -ceq 'INSTALLER_RUST_LICENSES.txt' -and
            [int64]$receipt.notices.rust_licenses_byte_length -eq [int64]$installerRustLicensesItem.Length -and
            $receipt.notices.rust_licenses_sha256 -ceq $installerRustLicensesSha256 -and
            $installerRustLicensesSha256 -ceq $expectedInstallerRustLicensesSha256 -and
            $receipt.toolchain.nsis_version -ceq '3.11' -and
            $receipt.lifecycle.scope -ceq 'current_user' -and
            $receipt.lifecycle.offline -eq $true -and
            $receipt.lifecycle.network_required -eq $false -and
            $receipt.lifecycle.powershell_required -eq $false -and
            $receipt.lifecycle.system_python_required -eq $false -and
            $receipt.lifecycle.elevation_required -eq $false -and
            $receipt.lifecycle.immutable_versions -eq $true -and
            $receipt.native_helper_lifecycle_smoke -ceq 'pending' -and
            $receipt.windows_setup_lifecycle -ceq 'not_run_clean_machine_gate' -and
            $receipt.signing.mode -ceq 'unsigned_local_rc' -and
            $receipt.signing.outer_setup_authenticode -ceq 'not_present' -and
            $receipt.signing.embedded_uninstaller_finalize -ceq 'not_requested' -and
            $receipt.signing.installed_uninstaller_authenticode -ceq 'not_run_clean_machine_gate' -and
            $receipt.publisher_signature -ceq 'not_present_local_rc'
        ) `
        -Message 'Codec Pack setup receipt does not bind the exact setup, helper, and adjacent payload.'

    Assert-Condition `
        -Condition (
            $receipt.source.commit -ceq [string]$expectedSetupSource.Commit -and
            $receipt.source.branch -ceq [string]$expectedSetupSource.Branch -and
            [bool]$receipt.source.git_dirty -eq [bool]$expectedSetupSource.Dirty -and
            $receipt.source.git_tree -ceq [string]$expectedSetupSource.Tree -and
            $receipt.source.git_tree -cmatch '^[0-9a-f]{40}$' -and
            $receipt.source.public_snapshot_sha256 -ceq
                [string]$expectedSetupSource.PublicSnapshotSha256 -and
            [int64]$receipt.source.public_snapshot_file_count -eq
                [int64]$expectedSetupSource.PublicSnapshotFileCount
        ) `
        -Message (
            'Codec Pack setup receipt source identity is stale. ' +
            "receipt(commit=$($receipt.source.commit), branch=$($receipt.source.branch), " +
            "dirty=$($receipt.source.git_dirty), tree=$($receipt.source.git_tree), " +
            "snapshot=$($receipt.source.public_snapshot_sha256)/$($receipt.source.public_snapshot_file_count)); " +
            "expected(commit=$($expectedSetupSource.Commit), branch=$($expectedSetupSource.Branch), " +
            "dirty=$($expectedSetupSource.Dirty), tree=$($expectedSetupSource.Tree), " +
            "snapshot=$($expectedSetupSource.PublicSnapshotSha256)/$($expectedSetupSource.PublicSnapshotFileCount))."
        )

    $installerSbom = Get-Content -LiteralPath $installerSbomPath -Raw |
        ConvertFrom-Json -Depth 100
    Assert-Condition `
        -Condition (
            $installerSbom.bomFormat -ceq 'CycloneDX' -and
            $installerSbom.specVersion -ceq '1.5' -and
            @($installerSbom.metadata.component.licenses).Count -eq 1 -and
            @($installerSbom.components | Where-Object {
                $null -eq $_.PSObject.Properties['licenses'] -or @($_.licenses).Count -eq 0
            }).Count -eq 0 -and
            [int]$receipt.sbom.component_count -eq @($installerSbom.components).Count -and
            @($installerSbom.components | Where-Object {
                $_.name -ceq 'latentdeck-codec-pack-installer'
            }).Count -eq 1 -and
            @($installerSbom.components | Where-Object {
                $_.'bom-ref' -ceq 'tool:nsis@3.11'
            }).Count -eq 1 -and
            @($installerSbom.components | Where-Object {
                $_.name -ceq 'libc'
            }).Count -eq 0 -and
            @($installerSbom.components | Where-Object {
                $_.name -ceq 'zip'
            }).Count -eq 1 -and
            @($installerSbom.components | Where-Object {
                $_.name -ceq 'winapi'
            }).Count -eq 1 -and
            @($installerSbom.metadata.component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:target-platform' -and
                [string]$_.value -ceq 'x86_64-pc-windows-msvc'
            }).Count -eq 1 -and
            @($installerSbom.components | Where-Object {
                @($_.properties | Where-Object {
                    [string]$_.name -ceq 'latentdeck:dependency-scope' -and
                    [string]$_.value -cin @('artifact', 'runtime', 'build', 'runtime+build')
                }).Count -ne 1
            }).Count -eq 0
        ) `
        -Message (
            'Installer SBOM does not match the Windows normal/build dependency closure ' +
            'or cover the native helper and pinned NSIS toolchain.'
        )

    $sumsPath = Join-Path $fixture011.OutputRoot 'SHA256SUMS.txt'
    $sumLines = @(Get-Content -LiteralPath $sumsPath)
    $setupReceiptSha256 = (
        Get-FileHash -LiteralPath $receiptPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    Assert-Condition `
        -Condition (
            $sumLines.Count -eq 7 -and
            $sumLines -ccontains "$($fixture011.ArchiveSha256)  $($fixture011.ArchiveName)" -and
            $sumLines -ccontains "$setupSha256  $expectedSetupName" -and
            $sumLines -ccontains "$installerSbomSha256  installer-SBOM.cdx.json" -and
            $sumLines -ccontains "$installerNoticesSha256  INSTALLER_THIRD_PARTY_NOTICES.md" -and
            $sumLines -ccontains "$installerNsisCopyingSha256  INSTALLER_NSIS_COPYING.txt" -and
            $sumLines -ccontains "$installerRustLicensesSha256  INSTALLER_RUST_LICENSES.txt" -and
            $sumLines -ccontains "$setupReceiptSha256  setup-receipt.json"
        ) `
        -Message 'SHA256SUMS.txt does not bind the complete setup sidecar set.'

    $signature = Get-AuthenticodeSignature -LiteralPath $setupPath
    Assert-Condition `
        -Condition ($signature.Status -eq [System.Management.Automation.SignatureStatus]::NotSigned) `
        -Message 'Synthetic local setup unexpectedly crossed the signing boundary.'

    $missingAdjacentRoot = Join-Path $testRoot 'missing-adjacent'
    [System.IO.Directory]::CreateDirectory($missingAdjacentRoot) | Out-Null
    $missingSetupPath = Join-Path $missingAdjacentRoot $expectedSetupName
    [System.IO.File]::Copy($setupPath, $missingSetupPath, $false)
    $missingProcess = Start-Process `
        -FilePath $missingSetupPath `
        -ArgumentList '/S' `
        -WorkingDirectory $missingAdjacentRoot `
        -WindowStyle Hidden `
        -Wait `
        -PassThru
    Assert-Condition `
        -Condition ($missingProcess.ExitCode -eq 20) `
        -Message "Setup without its adjacent payload returned $($missingProcess.ExitCode), expected 20."

    $isolatedLocalAppData = Join-Path $testRoot 'isolated/LocalAppData'
    $isolatedProgramData = Join-Path $testRoot 'isolated/ProgramData'
    [System.IO.Directory]::CreateDirectory($isolatedLocalAppData) | Out-Null
    [System.IO.Directory]::CreateDirectory($isolatedProgramData) | Out-Null
    $savedLocalAppData = $env:LOCALAPPDATA
    $savedProgramData = $env:PROGRAMDATA
    try {
        $env:LOCALAPPDATA = $isolatedLocalAppData
        $env:PROGRAMDATA = $isolatedProgramData
        $packParent = Join-Path `
            $isolatedLocalAppData `
            'LatentDeck/CodecPacks/org.latentdeck.h3'
        $lifecycleParent = Join-Path $isolatedLocalAppData 'LatentDeck'
        $stagingRoot = Join-Path $lifecycleParent 'ExtensionStaging'
        $staleStaging = Join-Path `
            $stagingRoot `
            '.install-00000000000000000000000000000001'
        $similarStaging = Join-Path $stagingRoot '.install-owner-notes'
        $outsideStagingSentinel = Join-Path $lifecycleParent 'keep-outside-staging.txt'
        Write-Utf8Text -Path (Join-Path $staleStaging 'partial.bin') -Content 'stale'
        Write-Utf8Text -Path (Join-Path $similarStaging 'keep.txt') -Content 'keep'
        Write-Utf8Text -Path $outsideStagingSentinel -Content 'keep'

        $tamperedSetupRoot = Join-Path $testRoot 'tampered-adjacent'
        [System.IO.Directory]::CreateDirectory($tamperedSetupRoot) | Out-Null
        $tamperedSetupPath = Join-Path $tamperedSetupRoot $expectedSetupName
        $tamperedSetupArchivePath = Join-Path $tamperedSetupRoot $fixture011.ArchiveName
        [System.IO.File]::Copy($setupPath, $tamperedSetupPath, $false)
        [System.IO.File]::Copy($fixture011.ArchivePath, $tamperedSetupArchivePath, $false)
        $tamperedSetupBytes = [System.IO.File]::ReadAllBytes($tamperedSetupArchivePath)
        $tamperedSetupIndex = [Math]::Min(128, $tamperedSetupBytes.Length - 1)
        $tamperedSetupBytes[$tamperedSetupIndex] =
            $tamperedSetupBytes[$tamperedSetupIndex] -bxor 0x01
        [System.IO.File]::WriteAllBytes($tamperedSetupArchivePath, $tamperedSetupBytes)
        $tamperedSetupProcess = Start-Process `
            -FilePath $tamperedSetupPath `
            -ArgumentList '/S' `
            -WorkingDirectory $tamperedSetupRoot `
            -WindowStyle Hidden `
            -Wait `
            -PassThru
        Assert-Condition `
            -Condition ($tamperedSetupProcess.ExitCode -eq 20) `
            -Message (
                'Setup with a same-length tampered adjacent payload returned ' +
                "$($tamperedSetupProcess.ExitCode), expected 20."
            )

        $missingArchivePath = Join-Path $testRoot 'does-not-exist.ldcodec'
        $missingResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments `
                -Fixture $fixture011 `
                -ArchivePath $missingArchivePath)
        Assert-NativeExit `
            -Result $missingResult `
            -ExpectedExitCodes @(20) `
            -Context 'missing archive rejection'

        $tamperedArchivePath = Join-Path $testRoot 'tampered.ldcodec'
        [System.IO.File]::Copy($fixture011.ArchivePath, $tamperedArchivePath, $false)
        $tamperedBytes = [System.IO.File]::ReadAllBytes($tamperedArchivePath)
        $tamperIndex = [Math]::Min(128, $tamperedBytes.Length - 1)
        $tamperedBytes[$tamperIndex] = $tamperedBytes[$tamperIndex] -bxor 0x01
        [System.IO.File]::WriteAllBytes($tamperedArchivePath, $tamperedBytes)
        $tamperedResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments `
                -Fixture $fixture011 `
                -ArchivePath $tamperedArchivePath)
        Assert-NativeExit `
            -Result $tamperedResult `
            -ExpectedExitCodes @(20) `
            -Context 'tampered archive rejection'

        $runtimeAuthorizationInjectionResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (@(
                New-InstallArguments -Fixture $fixture011
            ) + @(
                '--expected-sha256', ('0' * 64),
                '--expected-length', ([string]$fixture011.ArchiveLength),
                '--expected-version', $fixture011.Version
            ))
        Assert-NativeExit `
            -Result $runtimeAuthorizationInjectionResult `
            -ExpectedExitCodes @(10) `
            -Context 'runtime authorization argument rejection'

        $unknownHashResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011Equivalent)
        Assert-NativeExit `
            -Result $unknownHashResult `
            -ExpectedExitCodes @(40) `
            -Context 'unknown valid archive hash rejection'
        Assert-Condition `
            -Condition ($unknownHashResult.Output -like '*extension.package_untrusted*') `
            -Message 'Unknown valid archive hash did not report extension.package_untrusted.'
        Assert-Condition `
            -Condition (
                -not (Test-Path -LiteralPath $packParent) -or
                @(Get-ChildItem -LiteralPath $packParent -Force).Count -eq 0
            ) `
            -Message 'A rejected archive leaked a version entry into Codec Pack discovery.'

        foreach ($fixture in @($fixture010, $fixture011)) {
            $installResult = Invoke-NativeResult `
                -Executable $LifecycleHelperPath `
                -Arguments (New-InstallArguments -Fixture $fixture)
            Assert-NativeExit `
                -Result $installResult `
                -ExpectedExitCodes @(0) `
                -Context "installing synthetic H3 pack $($fixture.Version)"
        }
        Assert-Condition `
            -Condition (
                -not (Test-Path -LiteralPath $staleStaging) -and
                (Test-Path -LiteralPath (Join-Path $similarStaging 'keep.txt') -PathType Leaf) -and
                (Test-Path -LiteralPath $outsideStagingSentinel -PathType Leaf)
            ) `
            -Message 'Lifecycle cleanup did not remove only the exact stale staging directory.'

        $installed010 = Join-Path $packParent '0.2.0'
        $installed011 = Join-Path $packParent '0.2.1'
        Assert-Condition `
            -Condition (
                (Test-Path -LiteralPath $installed010 -PathType Container) -and
                (Test-Path -LiteralPath $installed011 -PathType Container)
            ) `
            -Message 'Helper did not preserve side-by-side immutable versions.'
        foreach ($installedRoot in @($installed010, $installed011)) {
            Test-H3CodecPackDirectory -PackRoot $installedRoot | Out-Null
            foreach ($forbiddenName in @(
                'install-metadata.json',
                'setup-receipt.json',
                'installer-SBOM.cdx.json',
                'INSTALLER_THIRD_PARTY_NOTICES.md',
                'INSTALLER_NSIS_COPYING.txt',
                'INSTALLER_RUST_LICENSES.txt',
                'latentdeck-codec-pack-installer.exe',
                'Uninstall.exe',
                $expectedSetupName
            )) {
                $leaks = @(
                    Get-ChildItem -LiteralPath $installedRoot -File -Force -Recurse |
                        Where-Object { $_.Name -ceq $forbiddenName }
                )
                if ($leaks.Count -gt 0) {
                    throw "Installer metadata leaked inside the pack integrity tree: $forbiddenName"
                }
            }
        }

        $original011Fingerprint = Get-DirectoryFingerprint -Root $installed011
        $alreadyInstalledResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011)
        Assert-NativeExit `
            -Result $alreadyInstalledResult `
            -ExpectedExitCodes @(30) `
            -Context 'same-byte immutable reinstall'
        Assert-Condition `
            -Condition ((Get-DirectoryFingerprint -Root $installed011) -ceq $original011Fingerprint) `
            -Message 'Same-byte reinstall changed the installed pack.'

        $installedAdapterPath = Join-Path `
            $installed011 `
            'runtime/Lib/site-packages/latentdeck_codec_h3/__init__.py'
        Add-Content `
            -LiteralPath $installedAdapterPath `
            -Value '# deliberate existing-version corruption' `
            -Encoding utf8
        $corruptExistingInstall = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011)
        Assert-NativeExit `
            -Result $corruptExistingInstall `
            -ExpectedExitCodes @(20) `
            -Context 'corrupt exact version is not reported as already installed'
        Assert-Condition `
            -Condition ($corruptExistingInstall.Output -like '*extension.integrity_failed*') `
            -Message 'Corrupt exact install did not surface integrity_failed.'
        $corruptExistingVerify = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-VerifyArguments -Version $fixture011.Version)
        Assert-NativeExit `
            -Result $corruptExistingVerify `
            -ExpectedExitCodes @(20) `
            -Context 'explicit corrupt exact-version verification'
        $explicitRepairOutput = @(& (Join-Path $PSScriptRoot 'Install-H3CodecPack.ps1') `
            -ArchivePath $fixture011.ArchivePath `
            -TrustedArchiveSha256 $fixture011.ArchiveSha256 `
            -InstallRoot (Join-Path $env:LOCALAPPDATA 'LatentDeck/CodecPacks') `
            -LifecycleHelperPath $LifecycleHelperPath `
            -Repair)
        Assert-Condition `
            -Condition (($explicitRepairOutput | Out-String) -like '*repaired org.latentdeck.h3*') `
            -Message 'Explicit build-authorized H3 repair wrapper did not report success.'
        $repairedVerify = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-VerifyArguments -Version $fixture011.Version)
        Assert-NativeExit `
            -Result $repairedVerify `
            -ExpectedExitCodes @(0) `
            -Context 'verification after explicit H3 repair'
        Assert-Condition `
            -Condition ((Get-DirectoryFingerprint -Root $installed011) -ceq $original011Fingerprint) `
            -Message 'Explicit H3 repair did not restore the exact build-authorized tree.'

        $installedTamperedArchiveResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments `
                -Fixture $fixture011 `
                -ArchivePath $tamperedArchivePath)
        Assert-NativeExit `
            -Result $installedTamperedArchiveResult `
            -ExpectedExitCodes @(20) `
            -Context 'installed version does not bypass adjacent archive binding'
        Assert-Condition `
            -Condition ((Get-DirectoryFingerprint -Root $installed011) -ceq $original011Fingerprint) `
            -Message 'Rejected archive binding changed the installed pack.'

        $equivalentRepackResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011Equivalent)
        Assert-NativeExit `
            -Result $equivalentRepackResult `
            -ExpectedExitCodes @(40) `
            -Context 'equivalent repacked tree is not build-authorized'
        Assert-Condition `
            -Condition ($equivalentRepackResult.Output -like '*extension.package_untrusted*') `
            -Message 'Equivalent repack did not fail with package_untrusted.'
        Assert-Condition `
            -Condition ((Get-DirectoryFingerprint -Root $installed011) -ceq $original011Fingerprint) `
            -Message 'Equivalent-tree repack changed the installed pack.'

        $differentBytesResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011Different)
        Assert-NativeExit `
            -Result $differentBytesResult `
            -ExpectedExitCodes @(40) `
            -Context 'different valid tree is not build-authorized'
        Assert-Condition `
            -Condition ((Get-DirectoryFingerprint -Root $installed011) -ceq $original011Fingerprint) `
            -Message 'Rejected same-version bytes changed the installed pack.'

        $trashRoot = Join-Path $lifecycleParent 'CodecPackTrash'
        $prereleaseVersion = '0.2.1-alpha'
        $prereleaseLength = [System.Text.Encoding]::UTF8.GetByteCount($prereleaseVersion)
        $prereleaseQuarantine = Join-Path $trashRoot (
            ".remove-org.latentdeck.h3-v$prereleaseLength-$prereleaseVersion-" +
            '00000000000000000000000000000002'
        )
        $prereleaseSentinel = Join-Path $prereleaseQuarantine 'keep.txt'
        Write-Utf8Text -Path $prereleaseSentinel -Content 'keep'

        $stableUninstallWithPrereleaseQuarantine = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-UninstallArguments -Version '0.2.1')
        Assert-NativeExit `
            -Result $stableUninstallWithPrereleaseQuarantine `
            -ExpectedExitCodes @(0) `
            -Context 'stable uninstall beside prerelease quarantine'
        Assert-Condition `
            -Condition (
                -not (Test-Path -LiteralPath $installed011) -and
                (Test-Path -LiteralPath $installed010 -PathType Container) -and
                (Test-Path -LiteralPath $prereleaseSentinel -PathType Leaf)
            ) `
            -Message 'Stable uninstall aliased or removed the prerelease quarantine.'

        $stableReinstallWithPrereleaseQuarantine = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011)
        Assert-NativeExit `
            -Result $stableReinstallWithPrereleaseQuarantine `
            -ExpectedExitCodes @(0) `
            -Context 'stable reinstall beside prerelease quarantine'
        Assert-Condition `
            -Condition (
                (Test-Path -LiteralPath $installed011 -PathType Container) -and
                (Test-Path -LiteralPath $prereleaseSentinel -PathType Leaf) -and
                (Get-DirectoryFingerprint -Root $installed011) -ceq $original011Fingerprint
            ) `
            -Message 'Prerelease quarantine blocked or changed the stable reinstall.'

        $programPackParent = Join-Path `
            $isolatedProgramData `
            'LatentDeck/CodecPacks/org.latentdeck.h3'
        [System.IO.Directory]::CreateDirectory($programPackParent) | Out-Null
        Copy-Item `
            -LiteralPath $installed011 `
            -Destination $programPackParent `
            -Recurse
        $programInstalled011 = Join-Path $programPackParent '0.2.1'
        Test-H3CodecPackDirectory -PackRoot $programInstalled011 | Out-Null
        $program011Fingerprint = Get-DirectoryFingerprint -Root $programInstalled011
        $local011BeforeCrossScope = Get-DirectoryFingerprint -Root $installed011
        $crossScopeResult = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-InstallArguments -Fixture $fixture011)
        Assert-NativeExit `
            -Result $crossScopeResult `
            -ExpectedExitCodes @(30) `
            -Context 'current-user immutable version with ignored all-users tree'
        Assert-Condition `
            -Condition (
                (Get-DirectoryFingerprint -Root $installed011) -ceq $local011BeforeCrossScope -and
                (Get-DirectoryFingerprint -Root $programInstalled011) -ceq $program011Fingerprint
            ) `
            -Message 'Cross-scope rejection changed a local or all-users pack tree.'

        $uninstall010 = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-UninstallArguments -Version '0.2.0')
        Assert-NativeExit `
            -Result $uninstall010 `
            -ExpectedExitCodes @(0) `
            -Context 'exact-version 0.2.0 uninstall'
        Assert-Condition `
            -Condition (
                -not (Test-Path -LiteralPath $installed010) -and
                (Test-Path -LiteralPath $installed011 -PathType Container)
            ) `
            -Message 'Exact uninstall removed the wrong side-by-side version.'

        Add-Content `
            -LiteralPath (Join-Path $installed011 'runtime/Lib/site-packages/latentdeck_codec_h3/__init__.py') `
            -Value '# deliberate lifecycle corruption' `
            -Encoding utf8
        $corruptUninstall = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-UninstallArguments -Version '0.2.1')
        Assert-NativeExit `
            -Result $corruptUninstall `
            -ExpectedExitCodes @(20) `
            -Context 'corrupt pack normal-uninstall rejection'
        Assert-Condition `
            -Condition (Test-Path -LiteralPath $installed011 -PathType Container) `
            -Message 'Normal uninstall removed a corrupt pack without explicit force.'

        $forceUninstall = Invoke-NativeResult `
            -Executable $LifecycleHelperPath `
            -Arguments (New-UninstallArguments -Version '0.2.1' -RemoveCorrupt)
        Assert-NativeExit `
            -Result $forceUninstall `
            -ExpectedExitCodes @(0) `
            -Context 'explicit corrupt-version removal'
        Assert-Condition `
            -Condition (-not (Test-Path -LiteralPath $installed011)) `
            -Message 'Forced exact-version removal left the corrupt pack in discovery.'

        Assert-Condition `
            -Condition (
                (Test-Path -LiteralPath $programInstalled011 -PathType Container) -and
                (Get-DirectoryFingerprint -Root $programInstalled011) -ceq $program011Fingerprint
            ) `
            -Message 'Current-user lifecycle mutated the isolated all-users conflict fixture.'
        $ownedStagingResidue = @(
            Get-ChildItem -LiteralPath $stagingRoot -Directory -Force |
                Where-Object { $_.Name -cmatch '^\.install-[0-9a-f]{32}$' }
        )
        Assert-Condition `
            -Condition (
                $ownedStagingResidue.Count -eq 0 -and
                (Test-Path -LiteralPath (Join-Path $similarStaging 'keep.txt') -PathType Leaf) -and
                (Test-Path -LiteralPath $outsideStagingSentinel -PathType Leaf)
            ) `
            -Message 'Lifecycle left exact staging residue or removed an unowned sentinel.'
        $stableQuarantineResidue = @(
            Get-ChildItem -LiteralPath $trashRoot -Directory -Force |
                Where-Object {
                    $_.Name -cmatch '^\.remove-org\.latentdeck\.h3-v5-0\.1\.[01]-[0-9a-f]{32}$'
                }
        )
        Assert-Condition `
            -Condition (
                $stableQuarantineResidue.Count -eq 0 -and
                (Test-Path -LiteralPath $prereleaseSentinel -PathType Leaf)
            ) `
            -Message 'Lifecycle left stable quarantine residue or removed prerelease state.'
    } finally {
        $env:LOCALAPPDATA = $savedLocalAppData
        $env:PROGRAMDATA = $savedProgramData
    }

    Write-Host 'H3 CODEC PACK INSTALLER TOOLING CONTRACT: PASS' -ForegroundColor Green
    Write-Host (
        'Verified: small unsigned NSIS PE, canonical name, exact setup/receipt/checksum binding, ' +
        'required adjacent payload, isolated native install failures, equivalent-tree proof, ' +
        'same-version immutability, current-user scope isolation, exact temporary ownership, ' +
        'side-by-side versions, ' +
        'exact uninstall, explicit corrupt removal, and no installer metadata inside the pack. ' +
        'A successful Windows setup/Installed Apps lifecycle remains the clean-machine gate.'
    )
} finally {
    if (Test-Path -LiteralPath $testRoot -PathType Container) {
        Remove-SafeTemporaryDirectory `
            -ParentPath $artifactsRoot `
            -CandidatePath $testRoot `
            -RequiredLeafPrefix '.h3-setup-test-'
    }
}
