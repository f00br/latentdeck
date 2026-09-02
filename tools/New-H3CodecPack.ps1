[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$RuntimeSource,

    [Parameter(Mandatory)]
    [string]$PackageSource,

    [Parameter(Mandatory)]
    [string]$NoticeSource,

    [Parameter(Mandatory)]
    [string]$DependencyInventoryPath,

    [Parameter(Mandatory)]
    [string]$SbomPath,

    [Parameter(Mandatory)]
    [string]$DecoderAssetContractPath,

    [Parameter(Mandatory)]
    [string]$PackVersion,

    [string]$OutputDirectory,

    [string]$PublisherName = 'LatentDeck Project',

    [string]$PublisherUrl,

    [string]$LicenseLabel = 'SEE-NOTICES'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null

Assert-SemVer -Value $PackVersion -Name 'PackVersion'
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $artifactsRoot 'codec-pack'
}
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $outputRoot -AllowParent | Out-Null

$runtimeRoot = (Resolve-Path -LiteralPath $RuntimeSource).Path
$packageRoot = (Resolve-Path -LiteralPath $PackageSource).Path
$noticePath = (Resolve-Path -LiteralPath $NoticeSource).Path
$inventoryPath = (Resolve-Path -LiteralPath $DependencyInventoryPath).Path
$sbomPath = (Resolve-Path -LiteralPath $SbomPath).Path
$assetContractPath = (Resolve-Path -LiteralPath $DecoderAssetContractPath).Path

foreach ($inputPath in @($noticePath, $inventoryPath, $sbomPath, $assetContractPath)) {
    $item = Get-Item -LiteralPath $inputPath -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Codec Pack metadata input must be a regular file: $inputPath"
    }
}
$notice = Get-Item -LiteralPath $noticePath
if ($notice.Length -eq 0 -or $notice.Length -gt 1MB -or
    @('.md', '.txt') -cnotcontains $notice.Extension.ToLowerInvariant()) {
    throw 'Codec Pack notice must be a non-empty Markdown or text file no larger than one MiB.'
}
$noticeText = [System.IO.File]::ReadAllText($notice.FullName)
if ($noticeText -match '(?im)(?:^|[\s"''(=])(?:file:///)?[A-Za-z]:[\\/]' -or
    $noticeText -match '(?im)/(?:Users|home)/[^/\s]+/' -or
    $noticeText -match '(?im)\\\\[^\\\s]+\\[^\\\s]+') {
    throw 'Codec Pack notice contains a machine-local absolute path.'
}
if ($noticeText -match '(?im)\b(?:api[_-]?key|access[_-]?token|secret|password)\b\s*[:=]\s*["''][^"''\r\n\s]{8,}') {
    throw 'Codec Pack notice contains a credential-like assignment.'
}

$forbiddenTopLevelPackages = @('comfy', 'comfyui', 'diffusers', 'transformers')
foreach ($item in Get-ChildItem -LiteralPath $packageRoot -Force) {
    $normalizedName = $item.Name.ToLowerInvariant()
    $matchesForbiddenPackage = @($forbiddenTopLevelPackages | Where-Object {
        $normalizedName -eq $_ -or
        $normalizedName.StartsWith("$_-", [System.StringComparison]::Ordinal) -or
        $normalizedName.StartsWith("$_.", [System.StringComparison]::Ordinal)
    }).Count -gt 0
    if ($matchesForbiddenPackage -or
        $normalizedName.StartsWith('minimax', [System.StringComparison]::Ordinal)) {
        throw "Package source contains generator-side component '$($item.Name)'."
    }
}
if (-not [string]::IsNullOrWhiteSpace($PublisherUrl) -and $PublisherUrl -cnotmatch '^https://') {
    throw 'PublisherUrl must be an HTTPS URL when supplied.'
}
foreach ($boundedText in @($PublisherName, $LicenseLabel)) {
    if ([string]::IsNullOrWhiteSpace($boundedText) -or
        $boundedText.Length -gt 256 -or
        $boundedText.Contains([char]0)) {
        throw 'Codec Pack publisher and license labels must be bounded non-empty text.'
    }
}

$assetContract = Read-StrictJsonFile -Path $assetContractPath
Assert-ExactProperties -Object $assetContract -Required @(
    'asset_id', 'display_name', 'kind', 'required', 'selection', 'format',
    'accepted_variants'
) -Context 'decoder asset contract'
$assetVariants = @($assetContract.accepted_variants)
if ($assetVariants.Count -ne 1) {
    throw 'Protocol 2 H3 packs require one exact external decoder asset declaration.'
}
$assetVariant = $assetVariants[0]
Assert-ExactProperties -Object $assetVariant -Required @(
    'variant_id', 'sha256', 'byte_length', 'source_url', 'license_label', 'license_url'
) -Context 'decoder asset variant'

$finalDirectory = Join-Path $outputRoot $PackVersion
if (Test-Path -LiteralPath $finalDirectory) {
    throw "Refusing to overwrite an existing Codec Pack artifact directory: $finalDirectory"
}

$buildRoot = Join-Path $artifactsRoot ".codec-pack-build-$([guid]::NewGuid().ToString('N'))"
$outputStage = Join-Path $artifactsRoot ".codec-pack-output-$([guid]::NewGuid().ToString('N'))"
Assert-SafeTemporaryDirectory `
    -ParentPath $artifactsRoot `
    -CandidatePath $buildRoot `
    -RequiredLeafPrefix '.codec-pack-build-' | Out-Null
Assert-SafeTemporaryDirectory `
    -ParentPath $artifactsRoot `
    -CandidatePath $outputStage `
    -RequiredLeafPrefix '.codec-pack-output-' | Out-Null

try {
    [System.IO.Directory]::CreateDirectory($buildRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($outputStage) | Out-Null
    $packRoot = Join-Path $buildRoot 'pack'
    $runtimeDestination = Join-Path $packRoot 'runtime'
    $packageDestination = Join-Path $runtimeDestination 'Lib/site-packages'
    [System.IO.Directory]::CreateDirectory($packRoot) | Out-Null

    Copy-PackagingTree `
        -SourcePath $runtimeRoot `
        -DestinationPath $runtimeDestination `
        -AllowedArchiveRelativePaths @('python313.zip')
    Copy-PackagingTree -SourcePath $packageRoot -DestinationPath $packageDestination
    [System.IO.File]::Copy(
        $noticePath,
        (Join-Path $packRoot 'THIRD_PARTY_NOTICES.md'),
        $false
    )
    [System.IO.File]::Copy(
        $inventoryPath,
        (Join-Path $packRoot 'DEPENDENCY_INVENTORY.json'),
        $false
    )
    [System.IO.File]::Copy(
        $sbomPath,
        (Join-Path $packRoot 'SBOM.cdx.json'),
        $false
    )

    $payloadFilesByPath = [System.Collections.Generic.SortedDictionary[
        string, System.IO.FileInfo
    ]]::new([System.StringComparer]::Ordinal)
    foreach ($payloadFile in Get-ChildItem -LiteralPath $packRoot -File -Force -Recurse) {
        $portablePath = Convert-ToPortableRelativePath `
            -RootPath $packRoot `
            -Path $payloadFile.FullName
        $payloadFilesByPath.Add($portablePath, $payloadFile)
    }
    $payloadFiles = @($payloadFilesByPath.Values)
    if ($payloadFiles.Count -eq 0 -or $payloadFiles.Count -gt 32766) {
        throw "Codec Pack payload has an invalid file count: $($payloadFiles.Count)"
    }

    $catalogEntries = @(
        foreach ($file in $payloadFiles) {
            Get-IntegrityEntry -RootPath $packRoot -File $file
        }
    )
    $catalogPath = Join-Path $packRoot 'integrity.json'
    Write-JsonFile -Value ([ordered]@{
        manifest_version = '1.0.0'
        files = $catalogEntries
    }) -Path $catalogPath
    $catalogHash = (Get-FileHash -LiteralPath $catalogPath -Algorithm SHA256).Hash.ToLowerInvariant()

    $manifestPath = Join-Path $packRoot 'codec-pack.json'
    Write-JsonFile -Value ([ordered]@{
        manifest_version = '2.0.0'
        kind = 'codec_pack'
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        display_name = 'LatentDeck H3 Codec Pack'
        summary = 'MiniMax H3 adapter and isolated runtime for LatentDeck.'
        publisher = [ordered]@{
            name = $PublisherName
            url = if ([string]::IsNullOrWhiteSpace($PublisherUrl)) { $null } else { $PublisherUrl }
            identity_claim = 'self_declared'
        }
        license = [ordered]@{
            spdx_or_label = $LicenseLabel
            notice_path = 'THIRD_PARTY_NOTICES.md'
        }
        platform = [ordered]@{
            os = 'windows'
            arch = 'x86_64'
        }
        compatibility = [ordered]@{
            app_min_inclusive = '0.1.0'
            app_max_exclusive = '1.0.0'
            worker_protocol = 2
            codec_adapter_api = 1
            tensor_abi = 'latentdeck.tensor.v1'
            python = [ordered]@{
                implementation = 'cpython'
                version = '3.13'
                platform_tag = 'win_amd64'
            }
            torch_exact_build = '2.13.0+cu130'
            lc_spec_versions = @('0.1.0')
            profiles = @(
                [ordered]@{
                    codec_family = 'minimax_h3'
                    profile = 'h3_av_latent'
                    profile_version = '0.1.0'
                }
            )
        }
        adapter = [ordered]@{
            adapter_id = 'org.latentdeck.h3'
            adapter_version = '0.2.0'
            entrypoint = 'latentdeck_codec_h3.adapter:make_adapter'
        }
        worker = [ordered]@{
            executable = 'runtime/python.exe'
            arguments = @(
                '-I', '-s', '-B', '-m', 'latentdeck_codec_host',
                '--worker-protocol', '2',
                '--codec-pack-id', 'org.latentdeck.h3',
                '--codec-pack-version', $PackVersion,
                '--codec-adapter-id', 'org.latentdeck.h3',
                '--codec-adapter-version', '0.2.0',
                '--codec-entrypoint', 'latentdeck_codec_h3.adapter:make_adapter'
            )
            working_directory = 'runtime'
            start_timeout_ms = 120000
            heartbeat_timeout_ms = 5000
        }
        capabilities = @(
            'player', 'realtime', 'resample', 'snapshot_capture', 'live_capture',
            'raw_import'
        )
        external_assets = @(
            [ordered]@{
                asset_id = [string]$assetContract.asset_id
                display_name = [string]$assetContract.display_name
                required = [bool]$assetContract.required
                byte_length = [int64]$assetVariant.byte_length
                sha256 = [string]$assetVariant.sha256
                source_url = [string]$assetVariant.source_url
                license_label = [string]$assetVariant.license_label
                license_url = [string]$assetVariant.license_url
            }
        )
        runtime_lock = [ordered]@{
            path = 'DEPENDENCY_INVENTORY.json'
            sha256 = (Get-FileHash -LiteralPath $inventoryPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        integrity = [ordered]@{
            catalog_path = 'integrity.json'
            catalog_sha256 = $catalogHash
        }
    }) -Path $manifestPath

    Test-H3CodecPackDirectory -PackRoot $packRoot -ExpectedPackVersion $PackVersion | Out-Null

    $archiveName = "LatentDeck-H3-CodecPack-$PackVersion-windows-x64.ldcodec"
    $archiveStagePath = Join-Path $outputStage $archiveName
    New-DeterministicZip -SourceDirectory $packRoot -DestinationPath $archiveStagePath

    $verificationRoot = Join-Path $buildRoot 'verify'
    Expand-SafeCodecPackArchive -ArchivePath $archiveStagePath -DestinationPath $verificationRoot
    Test-H3CodecPackDirectory -PackRoot $verificationRoot -ExpectedPackVersion $PackVersion | Out-Null

    $archive = Get-Item -LiteralPath $archiveStagePath
    $archiveHash = (Get-FileHash -LiteralPath $archive.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    [System.IO.File]::WriteAllText(
        (Join-Path $outputStage 'SHA256SUMS.txt'),
        "$archiveHash  $archiveName`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    Write-JsonFile -Value ([ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        platform = 'windows-x86_64'
        archive = [ordered]@{
            name = $archiveName
            byte_length = [int64]$archive.Length
            sha256 = $archiveHash
        }
        contains_runtime = $true
        contains_adapter = $true
        dependency_inventory = [ordered]@{
            path = 'DEPENDENCY_INVENTORY.json'
            sha256 = (Get-FileHash -LiteralPath $inventoryPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        sbom = [ordered]@{
            format = 'CycloneDX-1.5'
            path = 'SBOM.cdx.json'
            sha256 = (Get-FileHash -LiteralPath $sbomPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
        external_decoder_selection_required = $true
        archive_digest_purpose = 'transport_integrity_only'
        publisher_signature = 'not_present_local_rc'
        content_policy = [ordered]@{
            model_weights_allowed = $false
            cartridges_allowed = $false
            forbidden_payload_scan = 'passed'
            semantic_source_review = 'required_before_distribution'
        }
    }) -Path (Join-Path $outputStage 'package-receipt.json')

    [System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
    if (Test-Path -LiteralPath $finalDirectory) {
        throw "Codec Pack artifact destination appeared during build: $finalDirectory"
    }
    [System.IO.Directory]::Move($outputStage, $finalDirectory)
    $outputStage = $null

    $finalArchive = Join-Path $finalDirectory $archiveName
    $measuredFinalHash = (Get-FileHash -LiteralPath $finalArchive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($measuredFinalHash -cne $archiveHash) {
        throw 'Final Codec Pack archive hash changed during staging.'
    }
    Write-Output $finalArchive
} finally {
    Remove-SafeTemporaryDirectory `
        -ParentPath $artifactsRoot `
        -CandidatePath $buildRoot `
        -RequiredLeafPrefix '.codec-pack-build-'
    if ($null -ne $outputStage) {
        Remove-SafeTemporaryDirectory `
            -ParentPath $artifactsRoot `
            -CandidatePath $outputStage `
            -RequiredLeafPrefix '.codec-pack-output-'
    }
}
