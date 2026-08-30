[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$SourceA,

    [Parameter(Mandatory)]
    [string]$SourceB,

    [Parameter(Mandatory)]
    [string]$SourceC,

    [Parameter(Mandatory)]
    [string]$SourceD,

    [Parameter(Mandatory)]
    [string]$RawSource,

    [Parameter(Mandatory)]
    [string]$HqVaePath,

    [Parameter(Mandatory)]
    [string]$HqVaeExpectedSha256,

    [Parameter(Mandatory)]
    [string]$HqVaeSource,

    [Parameter(Mandatory)]
    [string]$HqVaeLicense,

    [string]$AlignSourceA,

    [string]$AlignSourceB,

    [string]$EnvironmentRoot,

    [string]$OutputRoot,

    [string]$CartridgeCli,

    [string]$RawInspector,

    [string]$Taeh3AssetDescriptor
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'artifacts'))

function Assert-ChildPath {
    param(
        [Parameter(Mandatory)]
        [string]$Root,
        [Parameter(Mandatory)]
        [string]$Candidate,
        [Parameter(Mandatory)]
        [string]$Label
    )

    $relative = [System.IO.Path]::GetRelativePath(
        [System.IO.Path]::GetFullPath($Root),
        [System.IO.Path]::GetFullPath($Candidate)
    )
    if ($relative -eq '.' -or
        $relative.StartsWith('..', [System.StringComparison]::Ordinal) -or
        [System.IO.Path]::IsPathFullyQualified($relative)) {
        throw "$Label must be a child of $Root."
    }
}

function Read-CommandJson {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Command,
        [Parameter(Mandatory)]
        [string]$Label
    )

    $output = @(& $Command 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE.`n$($output -join "`n")"
    }
    try {
        return ($output -join "`n") | ConvertFrom-Json -Depth 40
    }
    catch {
        throw "$Label did not return one JSON object.`n$($output -join "`n")"
    }
}

function Get-FullClipSignature {
    param(
        [Parameter(Mandatory)]
        [object]$Inspection,
        [Parameter(Mandatory)]
        [string]$Label
    )

    if ($Inspection.status -cne 'ok' -or $Inspection.validation_level -cne 'structure') {
        throw "$Label inspect did not return a successful structural inspection."
    }

    try {
        $manifest = $Inspection.manifest
        $codec = $manifest.codec
        $profile = $Inspection.profile
        $visualProfile = $profile.visual
        $timing = $manifest.timing
        $decoded = $timing.decoded_video
        $duration = $decoded.duration
        $frameRate = $decoded.frame_rate
        $tensors = @(
            $manifest.tensors |
                Sort-Object `
                    @{ Expression = { [string]$_.stream } }, `
                    @{ Expression = { [string]$_.name } }
        )
    }
    catch {
        throw "$Label inspect omitted a required LC 0.1 manifest/profile field."
    }
    if ($tensors.Count -eq 0) {
        throw "$Label inspect returned no tensor descriptors."
    }

    $audioSlotsProperty = $profile.PSObject.Properties['audio_latent_slots']
    $audioSlots = if ($null -eq $audioSlotsProperty) {
        '<absent>'
    }
    else {
        [string]$audioSlotsProperty.Value
    }
    $signature = [ordered]@{
        'spec.version' = [string]$manifest.spec_version
        'codec.family' = [string]$codec.family
        'codec.profile' = [string]$codec.profile
        'codec.profile_version' = [string]$codec.profile_version
        'profile.visual.latent_slots' = [string]$visualProfile.latent_slots
        'profile.visual.latent_height' = [string]$visualProfile.latent_height
        'profile.visual.latent_width' = [string]$visualProfile.latent_width
        'profile.visual.decoded_frames' = [string]$visualProfile.decoded_frames
        'profile.visual.decoded_height' = [string]$visualProfile.decoded_height
        'profile.visual.decoded_width' = [string]$visualProfile.decoded_width
        'profile.audio_latent_slots' = $audioSlots
        'timing.contract' = [string]$timing.contract
        'timing.contract_version' = [string]$timing.contract_version
        'timing.decoded_video.frame_count' = [string]$decoded.frame_count
        'timing.decoded_video.width' = [string]$decoded.width
        'timing.decoded_video.height' = [string]$decoded.height
        'timing.decoded_video.duration.numerator' = [string]$duration.numerator
        'timing.decoded_video.duration.denominator' = [string]$duration.denominator
        'timing.decoded_video.frame_rate.numerator' = [string]$frameRate.numerator
        'timing.decoded_video.frame_rate.denominator' = [string]$frameRate.denominator
        'tensor.count' = [string]$tensors.Count
    }
    for ($index = 0; $index -lt $tensors.Count; $index++) {
        $tensor = $tensors[$index]
        $shape = @($tensor.shape | ForEach-Object { [int64]$_ })
        $stream = [string]$tensor.stream
        $name = [string]$tensor.name
        $layout = switch ("$stream/$name") {
            'visual/video' {
                if ($shape.Count -ne 5) {
                    throw "$Label video tensor does not have layout [B,C,T,H,W]."
                }
                '[B,C,T,H,W]'
            }
            'audio/audio' {
                if ($shape.Count -ne 4) {
                    throw "$Label audio tensor does not have layout [B,C,S,T_audio]."
                }
                '[B,C,S,T_audio]'
            }
            default {
                throw "$Label contains an unsupported tensor descriptor $stream/$name."
            }
        }
        $prefix = "tensor.$index"
        $signature["$prefix.stream"] = $stream
        $signature["$prefix.name"] = $name
        $signature["$prefix.layout"] = $layout
        $signature["$prefix.runtime_dtype"] = [string]$tensor.runtime_dtype
        $signature["$prefix.storage_dtype"] = [string]$tensor.storage_dtype
        $signature["$prefix.shape"] = $shape -join 'x'
    }
    return $signature
}

function Get-SignatureDifferences {
    param(
        [Parameter(Mandatory)]
        [System.Collections.IDictionary]$Reference,
        [Parameter(Mandatory)]
        [System.Collections.IDictionary]$Candidate
    )

    $keys = @(@($Reference.Keys) + @($Candidate.Keys) | Sort-Object -Unique)
    foreach ($key in $keys) {
        $referenceValue = if ($Reference.Contains($key)) {
            [string]$Reference[$key]
        }
        else {
            '<missing>'
        }
        $candidateValue = if ($Candidate.Contains($key)) {
            [string]$Candidate[$key]
        }
        else {
            '<missing>'
        }
        if ($referenceValue -cne $candidateValue) {
            "$key (A=$referenceValue, candidate=$candidateValue)"
        }
    }
}

function Get-ValidatedSource {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Label,
        [Parameter(Mandatory)]
        [string]$Cli
    )

    $source = (Resolve-Path -LiteralPath $Path).Path
    if ([System.IO.Path]::GetExtension($source) -ine '.lc') {
        throw "$Label must use the .lc extension."
    }
    $validation = Read-CommandJson -Label "$Label validation" -Command {
        & $Cli validate $source
    }
    if ($validation.status -cne 'ok' -or
        $validation.validation.validation_level -cne 'full') {
        throw "$Label did not pass full LC validation."
    }
    $inspection = Read-CommandJson -Label "$Label inspect" -Command {
        & $Cli inspect $source
    }
    $signature = Get-FullClipSignature -Inspection $inspection -Label $Label
    $sha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    if ([string]$validation.validation.archive_sha256 -cne $sha256) {
        throw "$Label validation hash does not match its source file."
    }
    return [pscustomobject]@{
        source = $source
        validation = $validation
        signature = $signature
        sha256 = $sha256
    }
}

function Materialize-Source {
    param(
        [Parameter(Mandatory)]
        [object]$Record,
        [Parameter(Mandatory)]
        [string]$Slug,
        [Parameter(Mandatory)]
        [string]$InputRoot,
        [Parameter(Mandatory)]
        [string]$MasterInput
    )

    $targetName = "$($Slug.ToLowerInvariant())-$($Record.sha256.Substring(0, 16)).lc"
    $target = Join-Path $MasterInput $targetName
    Assert-ChildPath -Root $InputRoot -Candidate $target -Label "$Slug input"
    $materialization = 'existing'
    if (Test-Path -LiteralPath $target -PathType Leaf) {
        $existingHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($existingHash -cne $Record.sha256) {
            throw "Existing isolated input target for $Slug has another hash."
        }
    }
    else {
        try {
            New-Item -ItemType HardLink -Path $target -Target $Record.source -ErrorAction Stop |
                Out-Null
            $materialization = 'hardlink'
        }
        catch {
            Copy-Item -LiteralPath $Record.source -Destination $target
            $materialization = 'copy'
        }
    }
    $relative = [System.IO.Path]::GetRelativePath($InputRoot, $target).Replace('\', '/')
    return [pscustomobject]@{
        selection = $relative
        receipt = [ordered]@{
            file_name = [System.IO.Path]::GetFileName($Record.source)
            archive_sha256 = $Record.sha256
            cartridge_id = [string]$Record.validation.cartridge_id
            input_selection = $relative
            materialization = $materialization
        }
    }
}

function Get-RequiredPropertyValue {
    param(
        [Parameter(Mandatory)]
        [object]$InputObject,
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [string]$Label
    )

    $property = $InputObject.PSObject.Properties[$Name]
    if ($null -eq $property -or $null -eq $property.Value) {
        throw "$Label omitted required property '$Name'."
    }
    return $property.Value
}

function Get-ValidatedTaeh3Asset {
    param(
        [Parameter(Mandatory)]
        [object]$EnvironmentReceipt,
        [Parameter(Mandatory)]
        [string]$DescriptorPath
    )

    $externalModels = Get-RequiredPropertyValue `
        -InputObject $EnvironmentReceipt `
        -Name 'external_models' `
        -Label 'Environment receipt'
    $modelsRootValue = [string](
        Get-RequiredPropertyValue `
            -InputObject $externalModels `
            -Name 'models_root' `
            -Label 'Environment receipt external_models'
    )
    if ([string]::IsNullOrWhiteSpace($modelsRootValue)) {
        throw 'Environment receipt external_models.models_root must not be empty.'
    }
    $modelsRoot = (Resolve-Path -LiteralPath $modelsRootValue).Path
    if (-not (Test-Path -LiteralPath $modelsRoot -PathType Container)) {
        throw 'Environment receipt external_models.models_root is not a directory.'
    }
    $approxRoot = (Resolve-Path -LiteralPath (Join-Path $modelsRoot 'vae_approx')).Path
    if (-not (Test-Path -LiteralPath $approxRoot -PathType Container)) {
        throw 'Environment receipt models_root has no vae_approx directory.'
    }

    $taeh3 = Get-RequiredPropertyValue `
        -InputObject $externalModels `
        -Name 'taeh3' `
        -Label 'Environment receipt external_models'
    $assetPathValue = [string](
        Get-RequiredPropertyValue -InputObject $taeh3 -Name 'path' -Label 'TAEH3 receipt'
    )
    if ([string]::IsNullOrWhiteSpace($assetPathValue)) {
        throw 'Environment receipt TAEH3 path must not be empty.'
    }
    $assetPath = (Resolve-Path -LiteralPath $assetPathValue).Path
    if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
        throw 'Environment receipt TAEH3 path is not a file.'
    }
    Assert-ChildPath -Root $approxRoot -Candidate $assetPath -Label 'TAEH3 path'
    if ([System.IO.Path]::GetExtension($assetPath) -ine '.safetensors') {
        throw 'TAEH3 path must select a Safetensors file.'
    }
    $assetItem = Get-Item -LiteralPath $assetPath -Force
    if (($assetItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'TAEH3 path must not be a reparse point.'
    }

    $assetHash = (Get-FileHash -LiteralPath $assetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    $assetLength = [int64]$assetItem.Length
    $receiptHash = [string](
        Get-RequiredPropertyValue -InputObject $taeh3 -Name 'sha256' -Label 'TAEH3 receipt'
    )
    $receiptLength = [int64](
        Get-RequiredPropertyValue -InputObject $taeh3 -Name 'byte_length' -Label 'TAEH3 receipt'
    )
    if ($receiptHash -cnotmatch '^[0-9a-f]{64}$' -or $receiptHash -cne $assetHash) {
        throw 'Selected TAEH3 hash does not match the isolated environment receipt.'
    }
    if ($receiptLength -ne $assetLength) {
        throw 'Selected TAEH3 byte length does not match the isolated environment receipt.'
    }

    $descriptorFull = (Resolve-Path -LiteralPath $DescriptorPath).Path
    if (-not (Test-Path -LiteralPath $descriptorFull -PathType Leaf)) {
        throw 'TAEH3 asset descriptor is not a file.'
    }
    $descriptor = Get-Content -LiteralPath $descriptorFull -Raw | ConvertFrom-Json -Depth 20
    if ($descriptor.asset_id -cne 'taeh3' -or
        $descriptor.format -cne 'safetensors' -or
        $descriptor.selection -cne 'explicit_file') {
        throw 'TAEH3 asset descriptor does not declare the expected external asset contract.'
    }
    $variants = @(
        $descriptor.accepted_variants |
            Where-Object {
                [string]$_.sha256 -ceq $assetHash -and
                [int64]$_.byte_length -eq $assetLength
            }
    )
    if ($variants.Count -ne 1) {
        throw 'Selected TAEH3 file is not one exact accepted descriptor variant.'
    }
    $variant = $variants[0]
    foreach ($field in @('variant_id', 'source_url', 'license_label', 'license_url')) {
        $value = [string](
            Get-RequiredPropertyValue `
                -InputObject $variant `
                -Name $field `
                -Label 'Accepted TAEH3 variant'
        )
        if ([string]::IsNullOrWhiteSpace($value) -or $value.Length -gt 4096) {
            throw "Accepted TAEH3 variant field '$field' must be bounded non-empty text."
        }
    }
    foreach ($urlField in @('source_url', 'license_url')) {
        $uri = $null
        if (-not [System.Uri]::TryCreate(
                [string]$variant.$urlField,
                [System.UriKind]::Absolute,
                [ref]$uri
            ) -or $uri.Scheme -cne 'https') {
            throw "Accepted TAEH3 variant field '$urlField' must be an absolute HTTPS URL."
        }
    }

    $selection = [System.IO.Path]::GetRelativePath($approxRoot, $assetPath)
    if ($selection -eq '.' -or
        $selection.StartsWith('..', [System.StringComparison]::Ordinal) -or
        [System.IO.Path]::IsPathFullyQualified($selection)) {
        throw 'Selected TAEH3 file cannot be represented as a safe Comfy model selection.'
    }
    return [pscustomobject]@{
        role = 'FAST'
        decoder_id = 'org.comfy.taeh3'
        decoder_version = '0.1.0'
        model_selection = $selection
        source = [string]$variant.source_url
        license = "$([string]$variant.license_label) ($([string]$variant.license_url))"
        asset_sha256 = $assetHash
        receipt = [ordered]@{
            role = 'FAST'
            decoder_id = 'org.comfy.taeh3'
            decoder_version = '0.1.0'
            model_selection = $selection
            variant_id = [string]$variant.variant_id
            sha256 = $assetHash
            byte_length = $assetLength
            source = [string]$variant.source_url
            license = [string]$variant.license_label
            license_url = [string]$variant.license_url
        }
    }
}

function Assert-BoundedText {
    param(
        [Parameter(Mandatory)]
        [string]$Value,
        [Parameter(Mandatory)]
        [string]$Label
    )

    if ([string]::IsNullOrWhiteSpace($Value) -or
        [System.Text.Encoding]::UTF8.GetByteCount($Value) -gt 4096) {
        throw "$Label must be bounded non-empty text."
    }
}

function Get-ValidatedHqVaeAsset {
    param(
        [Parameter(Mandatory)]
        [object]$EnvironmentReceipt,
        [Parameter(Mandatory)]
        [string]$SelectedPath,
        [Parameter(Mandatory)]
        [string]$ExpectedSha256,
        [Parameter(Mandatory)]
        [string]$Source,
        [Parameter(Mandatory)]
        [string]$License
    )

    if ($ExpectedSha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'HqVaeExpectedSha256 must be one lowercase SHA-256.'
    }
    Assert-BoundedText -Value $Source -Label 'HqVaeSource'
    Assert-BoundedText -Value $License -Label 'HqVaeLicense'
    $sourceUri = $null
    if (-not [System.Uri]::TryCreate(
            $Source,
            [System.UriKind]::Absolute,
            [ref]$sourceUri
        ) -or $sourceUri.Scheme -cne 'https') {
        throw 'HqVaeSource must be an absolute HTTPS URL.'
    }

    $externalModels = Get-RequiredPropertyValue `
        -InputObject $EnvironmentReceipt `
        -Name 'external_models' `
        -Label 'Environment receipt'
    $modelsRootValue = [string](
        Get-RequiredPropertyValue `
            -InputObject $externalModels `
            -Name 'models_root' `
            -Label 'Environment receipt external_models'
    )
    $modelsRoot = (Resolve-Path -LiteralPath $modelsRootValue).Path
    if (-not (Test-Path -LiteralPath $modelsRoot -PathType Container)) {
        throw 'Environment receipt external_models.models_root is not a directory.'
    }
    $vaeRoot = (Resolve-Path -LiteralPath (Join-Path $modelsRoot 'vae')).Path
    if (-not (Test-Path -LiteralPath $vaeRoot -PathType Container)) {
        throw 'Environment receipt models_root has no vae directory.'
    }
    $hqReceipt = Get-RequiredPropertyValue `
        -InputObject $externalModels `
        -Name 'hq_h3_vae' `
        -Label 'Environment receipt external_models'
    $receiptPathValue = [string](
        Get-RequiredPropertyValue -InputObject $hqReceipt -Name 'path' -Label 'HQ VAE receipt'
    )
    $receiptPath = (Resolve-Path -LiteralPath $receiptPathValue).Path
    $assetPath = (Resolve-Path -LiteralPath $SelectedPath).Path
    if (-not $assetPath.Equals($receiptPath, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'HqVaePath must select the exact native H3 VAE recorded by the isolated profile.'
    }
    if (-not (Test-Path -LiteralPath $assetPath -PathType Leaf)) {
        throw 'Selected native H3 VAE path is not a file.'
    }
    Assert-ChildPath -Root $vaeRoot -Candidate $assetPath -Label 'Native H3 VAE path'
    if ([System.IO.Path]::GetExtension($assetPath) -ine '.safetensors') {
        throw 'Native H3 VAE path must select a Safetensors file.'
    }
    $assetItem = Get-Item -LiteralPath $assetPath -Force
    if (($assetItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Native H3 VAE path must not be a reparse point.'
    }
    $assetLength = [int64]$assetItem.Length
    $receiptLength = [int64](
        Get-RequiredPropertyValue `
            -InputObject $hqReceipt `
            -Name 'byte_length' `
            -Label 'HQ VAE receipt'
    )
    if ($assetLength -ne $receiptLength) {
        throw 'Selected native H3 VAE byte length does not match the isolated profile.'
    }

    # Hash the selected 5+ GB asset now. Never trust a prior note or filename.
    $assetHash = (Get-FileHash -LiteralPath $assetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($assetHash -cne $ExpectedSha256) {
        throw 'Selected native H3 VAE hash does not match HqVaeExpectedSha256.'
    }
    $receiptHashProperty = $hqReceipt.PSObject.Properties['sha256']
    if ($null -ne $receiptHashProperty -and
        [string]$receiptHashProperty.Value -cne $assetHash) {
        throw 'Selected native H3 VAE hash does not match the isolated profile.'
    }
    $selection = [System.IO.Path]::GetRelativePath($vaeRoot, $assetPath)
    if ($selection -eq '.' -or
        $selection.StartsWith('..', [System.StringComparison]::Ordinal) -or
        [System.IO.Path]::IsPathFullyQualified($selection)) {
        throw 'Selected native H3 VAE cannot be represented as a safe Comfy model selection.'
    }

    return [pscustomobject]@{
        role = 'HQ'
        decoder_id = 'org.minimax.h3.native-vae'
        decoder_version = '0.1.0'
        model_selection = $selection
        source = $Source
        license = $License
        asset_sha256 = $assetHash
        receipt = [ordered]@{
            role = 'HQ'
            decoder_id = 'org.minimax.h3.native-vae'
            decoder_version = '0.1.0'
            model_selection = $selection
            sha256 = $assetHash
            byte_length = $assetLength
            source = $Source
            license = $License
        }
    }
}

function Get-ValidatedRawSource {
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [object]$EnvironmentReceipt,
        [string]$Inspector
    )

    $source = (Resolve-Path -LiteralPath $Path).Path
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw 'RawSource is not a file.'
    }
    if ([System.IO.Path]::GetExtension($source) -ine '.safetensors') {
        throw 'RawSource must use the .safetensors extension.'
    }
    $sourceItem = Get-Item -LiteralPath $source -Force
    if (($sourceItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'RawSource must not be a reparse point.'
    }
    $sourceHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
    $inspection = if (-not [string]::IsNullOrWhiteSpace($Inspector)) {
        $inspectorFull = (Resolve-Path -LiteralPath $Inspector).Path
        if (-not (Test-Path -LiteralPath $inspectorFull -PathType Leaf)) {
            throw 'RawInspector is not a file.'
        }
        Read-CommandJson -Label 'RawSource inspection' -Command {
            & $inspectorFull $source
        }
    }
    else {
        $pythonReceipt = Get-RequiredPropertyValue `
            -InputObject $EnvironmentReceipt `
            -Name 'python' `
            -Label 'Environment receipt'
        $pathsReceipt = Get-RequiredPropertyValue `
            -InputObject $EnvironmentReceipt `
            -Name 'paths' `
            -Label 'Environment receipt'
        $pythonValue = [string](
            Get-RequiredPropertyValue `
                -InputObject $pythonReceipt `
                -Name 'executable' `
                -Label 'Environment receipt python'
        )
        $packagesValue = [string](
            Get-RequiredPropertyValue `
                -InputObject $pathsReceipt `
                -Name 'python_packages' `
                -Label 'Environment receipt paths'
        )
        $python = (Resolve-Path -LiteralPath $pythonValue).Path
        $pythonPackages = (Resolve-Path -LiteralPath $packagesValue).Path
        if (-not (Test-Path -LiteralPath $python -PathType Leaf) -or
            -not (Test-Path -LiteralPath $pythonPackages -PathType Container)) {
            throw 'Isolated Python raw-inspection runtime is incomplete.'
        }
        $inspectRawCode = @'
import json
import sys
sys.path.insert(0, sys.argv[1])
import latentdeck_cartridge
print(json.dumps(latentdeck_cartridge.inspect_raw_h3(sys.argv[2]), allow_nan=False, sort_keys=True))
'@
        Read-CommandJson -Label 'RawSource inspection' -Command {
            & $python -B -c $inspectRawCode $pythonPackages $source
        }
    }
    if ($inspection.status -cne 'ok' -or
        $inspection.command -cne 'inspect_raw_h3' -or
        [string]$inspection.sha256 -cne $sourceHash -or
        [int64]$inspection.byte_length -ne [int64]$sourceItem.Length) {
        throw 'RawSource inspection did not match the exact selected file.'
    }
    if ($inspection.profile.codec_family -cne 'minimax_h3' -or
        $inspection.profile.profile -cne 'h3_av_latent' -or
        $inspection.profile.profile_version -cne '0.1.0') {
        throw 'RawSource is not a supported MiniMax H3 AV latent.'
    }
    return [pscustomobject]@{
        source = $source
        sha256 = $sourceHash
        byte_length = [int64]$sourceItem.Length
        inspection = $inspection
    }
}

function Materialize-RawSource {
    param(
        [Parameter(Mandatory)]
        [object]$Record,
        [Parameter(Mandatory)]
        [string]$InputRoot,
        [Parameter(Mandatory)]
        [string]$MasterInput
    )

    $targetName = "raw-$($Record.sha256.Substring(0, 16)).safetensors"
    $target = Join-Path $MasterInput $targetName
    Assert-ChildPath -Root $InputRoot -Candidate $target -Label 'RawSource input'
    $materialization = 'existing'
    if (Test-Path -LiteralPath $target -PathType Leaf) {
        $existingHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($existingHash -cne $Record.sha256) {
            throw 'Existing isolated RawSource target has another hash.'
        }
    }
    else {
        try {
            New-Item -ItemType HardLink -Path $target -Target $Record.source -ErrorAction Stop |
                Out-Null
            $materialization = 'hardlink'
        }
        catch {
            Copy-Item -LiteralPath $Record.source -Destination $target
            $materialization = 'copy'
        }
    }
    $relative = [System.IO.Path]::GetRelativePath($InputRoot, $target).Replace('\', '/')
    return [pscustomobject]@{
        selection = $relative
        receipt = [ordered]@{
            file_name = [System.IO.Path]::GetFileName($Record.source)
            sha256 = $Record.sha256
            byte_length = $Record.byte_length
            input_selection = $relative
            materialization = $materialization
            profile = $Record.inspection.profile
        }
    }
}

if ([string]::IsNullOrWhiteSpace($EnvironmentRoot)) {
    $EnvironmentRoot = Join-Path $artifactsRoot 'comfy-test'
}
$environmentFull = [System.IO.Path]::GetFullPath($EnvironmentRoot)
Assert-ChildPath -Root $artifactsRoot -Candidate $environmentFull -Label 'EnvironmentRoot'
$environmentReceiptPath = Join-Path $environmentFull 'environment.json'
if (-not (Test-Path -LiteralPath $environmentReceiptPath -PathType Leaf)) {
    throw 'Initialize the isolated Comfy environment before generating master workflows.'
}
$environmentReceipt = Get-Content -LiteralPath $environmentReceiptPath -Raw |
    ConvertFrom-Json -Depth 30
if ($environmentReceipt.schema_version -ne 1 -or $environmentReceipt.private_artifact -ne $true) {
    throw 'Environment receipt is not a supported private isolated Comfy profile.'
}
if ([string]::IsNullOrWhiteSpace($Taeh3AssetDescriptor)) {
    $Taeh3AssetDescriptor = Join-Path `
        $repoRoot `
        'codec-host\codecs\h3\packaging\taeh3.asset.json'
}
$taeh3Asset = Get-ValidatedTaeh3Asset `
    -EnvironmentReceipt $environmentReceipt `
    -DescriptorPath $Taeh3AssetDescriptor
$hqVaeAsset = Get-ValidatedHqVaeAsset `
    -EnvironmentReceipt $environmentReceipt `
    -SelectedPath $HqVaePath `
    -ExpectedSha256 $HqVaeExpectedSha256 `
    -Source $HqVaeSource `
    -License $HqVaeLicense
$rawRecord = Get-ValidatedRawSource `
    -Path $RawSource `
    -EnvironmentReceipt $environmentReceipt `
    -Inspector $RawInspector

if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
    $OutputRoot = Join-Path $artifactsRoot "private-comfy-master-$stamp"
}
$outputFull = [System.IO.Path]::GetFullPath($OutputRoot)
Assert-ChildPath -Root $artifactsRoot -Candidate $outputFull -Label 'OutputRoot'
if (Test-Path -LiteralPath $outputFull) {
    throw 'OutputRoot already exists; generated master workflows are no-clobber.'
}

if ([string]::IsNullOrWhiteSpace($CartridgeCli)) {
    foreach ($candidate in @(
        (Join-Path $repoRoot 'target\release\latentdeck-cartridge.exe'),
        (Join-Path $repoRoot 'target\debug\latentdeck-cartridge.exe')
    )) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            $CartridgeCli = $candidate
            break
        }
    }
}
if ([string]::IsNullOrWhiteSpace($CartridgeCli) -or
    -not (Test-Path -LiteralPath $CartridgeCli -PathType Leaf)) {
    throw 'Build latentdeck-cartridge or pass -CartridgeCli.'
}
$cartridgeCliFull = (Resolve-Path -LiteralPath $CartridgeCli).Path

$inputRoot = [System.IO.Path]::GetFullPath([string]$environmentReceipt.paths.input)
Assert-ChildPath -Root $environmentFull -Candidate $inputRoot -Label 'receipt paths.input'
$masterInput = Join-Path $inputRoot 'latentdeck\master'

$mixerSourcePaths = [ordered]@{
    A = $SourceA
    B = $SourceB
    C = $SourceC
    D = $SourceD
}
if ([string]::IsNullOrWhiteSpace($AlignSourceA)) {
    $AlignSourceA = $SourceA
}
if ([string]::IsNullOrWhiteSpace($AlignSourceB)) {
    $AlignSourceB = $SourceD
}
$alignSourcePaths = [ordered]@{
    A = $AlignSourceA
    B = $AlignSourceB
}

# Inspect every source and reject incompatible full-clip mixer inputs before
# creating or changing anything beneath Comfy's input directory.
$mixerRecords = [ordered]@{}
foreach ($entry in $mixerSourcePaths.GetEnumerator()) {
    $mixerRecords[$entry.Key] = Get-ValidatedSource `
        -Path ([string]$entry.Value) `
        -Label "Mixer source $($entry.Key)" `
        -Cli $cartridgeCliFull
}
$mixerSignature = $mixerRecords.A.signature
foreach ($key in @('B', 'C', 'D')) {
    $differences = @(
        Get-SignatureDifferences `
            -Reference $mixerSignature `
            -Candidate $mixerRecords[$key].signature
    )
    if ($differences.Count -ne 0) {
        $summary = @($differences | Select-Object -First 8) -join '; '
        throw "Toolkit D2/Q4 full-clip mismatch: mixer source $key differs from mixer source A: " +
            "$summary. SourceA..D must have the same codec/profile, tensor layout/dtypes/" +
            'shapes, decoded geometry, duration, and frame rate. Duplicate compatible inputs ' +
            'are allowed for a functional master-test. Use AlignSourceA/AlignSourceB only for ' +
            'the explicit mixed-geometry Align workflow.'
    }
}

$alignRecords = [ordered]@{}
foreach ($entry in $alignSourcePaths.GetEnumerator()) {
    $alignRecords[$entry.Key] = Get-ValidatedSource `
        -Path ([string]$entry.Value) `
        -Label "Align source $($entry.Key)" `
        -Cli $cartridgeCliFull
}

[System.IO.Directory]::CreateDirectory($masterInput) | Out-Null
$mixerSelections = [ordered]@{}
$mixerReceipts = [ordered]@{}
foreach ($entry in $mixerRecords.GetEnumerator()) {
    $materialized = Materialize-Source `
        -Record $entry.Value `
        -Slug "mixer-$($entry.Key)" `
        -InputRoot $inputRoot `
        -MasterInput $masterInput
    $mixerSelections[$entry.Key] = $materialized.selection
    $mixerReceipts[$entry.Key] = $materialized.receipt
}
$alignSelections = [ordered]@{}
$alignReceipts = [ordered]@{}
foreach ($entry in $alignRecords.GetEnumerator()) {
    $materialized = Materialize-Source `
        -Record $entry.Value `
        -Slug "align-$($entry.Key)" `
        -InputRoot $inputRoot `
        -MasterInput $masterInput
    $alignSelections[$entry.Key] = $materialized.selection
    $alignReceipts[$entry.Key] = $materialized.receipt
}
$rawMaterialized = Materialize-RawSource `
    -Record $rawRecord `
    -InputRoot $inputRoot `
    -MasterInput $masterInput

function Write-PrefilledWorkflow {
    param(
        [Parameter(Mandatory)]
        [string]$TemplateName,
        [Parameter(Mandatory)]
        [string]$OutputName,
        [string[]]$Selections = @(),
        [object[]]$VaeAssets = @(),
        [string]$RawSelection,
        [switch]$RequireExplicitDevicePath
    )

    $templatePath = Join-Path $repoRoot "comfy\toolkit\workflows\$TemplateName"
    $workflow = Get-Content -LiteralPath $templatePath -Raw | ConvertFrom-Json -Depth 100
    $loaders = @(
        $workflow.nodes |
            Where-Object { $_.type -ceq 'LatentDeckToolkitLCLoadInspect' } |
            Sort-Object id
    )
    if ($loaders.Count -lt $Selections.Count) {
        throw "$TemplateName has fewer LC Load nodes than the requested private inputs."
    }
    for ($index = 0; $index -lt $Selections.Count; $index++) {
        $loaders[$index].widgets_values[0] = $Selections[$index]
    }
    if (-not [string]::IsNullOrWhiteSpace($RawSelection)) {
        $rawLoaders = @(
            $workflow.nodes |
                Where-Object { $_.type -ceq 'LatentDeckToolkitRawH3Import' }
        )
        if ($rawLoaders.Count -ne 1 -or @($rawLoaders[0].widgets_values).Count -ne 1) {
            throw "$TemplateName must contain one standard Raw H3 Import node."
        }
        $rawLoaders[0].widgets_values[0] = $RawSelection
    }

    foreach ($asset in $VaeAssets) {
        $declarations = @(
            $workflow.nodes |
                Where-Object {
                    $_.type -ceq 'LatentDeckToolkitDeclareH3Vae' -and
                    [string]$_.widgets_values[0] -ceq [string]$asset.role
                }
        )
        if ($declarations.Count -ne 1) {
            throw "$TemplateName must have exactly one $($asset.role) H3 VAE declaration."
        }
        $declaration = $declarations[0]
        if (@($declaration.widgets_values).Count -ne 6) {
            throw "$TemplateName has an unexpected H3 VAE declaration widget contract."
        }
        $vaeInput = @($declaration.inputs | Where-Object { $_.name -ceq 'vae' })
        if ($vaeInput.Count -ne 1 -or $null -eq $vaeInput[0].link) {
            throw "$TemplateName H3 VAE declaration is not linked to a loader."
        }
        $vaeLinks = @(
            $workflow.links |
                Where-Object { [int64]$_[0] -eq [int64]$vaeInput[0].link }
        )
        if ($vaeLinks.Count -ne 1) {
            throw "$TemplateName H3 VAE declaration has an invalid loader link."
        }
        $loaderId = [int64]$vaeLinks[0][1]
        $vaeLoaders = @(
            $workflow.nodes |
                Where-Object { [int64]$_.id -eq $loaderId -and $_.type -ceq 'VAELoader' }
        )
        if ($vaeLoaders.Count -ne 1 -or @($vaeLoaders[0].widgets_values).Count -ne 1) {
            throw "$TemplateName H3 VAE declaration must use one standard VAELoader."
        }

        $vaeLoaders[0].widgets_values[0] = [string]$asset.model_selection
        $declaration.widgets_values[1] = [string]$asset.decoder_id
        $declaration.widgets_values[2] = [string]$asset.decoder_version
        $declaration.widgets_values[3] = [string]$asset.source
        $declaration.widgets_values[4] = [string]$asset.license
        $declaration.widgets_values[5] = [string]$asset.asset_sha256
    }
    if ($RequireExplicitDevicePath) {
        $deviceTransfers = @(
            $workflow.nodes |
                Where-Object { $_.type -ceq 'LatentDeckToolkitExplicitDeviceTransfer' }
        )
        if ($deviceTransfers.Count -lt $Selections.Count) {
            throw "$TemplateName must route every selected LC through an explicit device node."
        }
        $deviceIds = @{}
        foreach ($deviceTransfer in $deviceTransfers) {
            $widgets = @($deviceTransfer.widgets_values)
            if ($widgets.Count -ne 3 -or
                [string]$widgets[0] -cne 'CUDA' -or
                [int64]$widgets[1] -ne 0 -or
                [string]$widgets[2] -cne 'FALLBACK_TO_CPU') {
                throw "$TemplateName explicit device path must declare CUDA:0 with visible CPU fallback."
            }
            $deviceIds[[int64]$deviceTransfer.id] = $true
        }
        foreach ($loader in @($loaders | Select-Object -First $Selections.Count)) {
            $routes = @(
                $workflow.links |
                    Where-Object {
                        [int64]$_[1] -eq [int64]$loader.id -and
                        $deviceIds.ContainsKey([int64]$_[3])
                    }
            )
            if ($routes.Count -ne 1) {
                throw "$TemplateName must visibly route each selected LC into one device transfer."
            }
        }
    }
    $workflowJson = ($workflow | ConvertTo-Json -Depth 100) + "`n"
    if ($workflowJson -cmatch 'REPLACE_WITH_[A-Z0-9_]+') {
        throw "$TemplateName still contains an unresolved queue placeholder."
    }
    if ($workflowJson -match '(?i)"(?:[a-z]:[\\/]|\\\\\\\\|/)') {
        throw "$TemplateName contains a machine-specific absolute path."
    }
    $target = Join-Path $outputFull $OutputName
    [System.IO.File]::WriteAllText(
        $target,
        $workflowJson,
        [System.Text.UTF8Encoding]::new($false)
    )
}

[System.IO.Directory]::CreateDirectory($outputFull) | Out-Null
Write-PrefilledWorkflow `
    -TemplateName '01_LC_INSPECT.json' `
    -OutputName '01_LC_INSPECT.private.json' `
    -Selections @($mixerSelections.A)
Write-PrefilledWorkflow `
    -TemplateName '02_FAST_HQ_COMPARE.json' `
    -OutputName '02_FAST_HQ_COMPARE.private.json' `
    -Selections @($mixerSelections.A) `
    -VaeAssets @($taeh3Asset, $hqVaeAsset)
Write-PrefilledWorkflow `
    -TemplateName '03_DUAL_SYNTH_LAB.json' `
    -OutputName '03_DUAL_SYNTH_LAB.private.json' `
    -Selections @($mixerSelections.A, $mixerSelections.B) `
    -VaeAssets @($taeh3Asset) `
    -RequireExplicitDevicePath
Write-PrefilledWorkflow `
    -TemplateName '04_QUAD_CARRIER_DONORS.json' `
    -OutputName '04_QUAD_CARRIER_DONORS.private.json' `
    -Selections @(
        $mixerSelections.A,
        $mixerSelections.B,
        $mixerSelections.C,
        $mixerSelections.D
    ) `
    -VaeAssets @($taeh3Asset) `
    -RequireExplicitDevicePath
Write-PrefilledWorkflow `
    -TemplateName '05_PROJECT_RESAMPLE.json' `
    -OutputName '05_PROJECT_RESAMPLE.private.json' `
    -Selections @($mixerSelections.A) `
    -VaeAssets @($taeh3Asset, $hqVaeAsset)
Write-PrefilledWorkflow `
    -TemplateName '06_RAW_RECORD_INSPECT.json' `
    -OutputName '06_RAW_RECORD_INSPECT.private.json' `
    -RawSelection $rawMaterialized.selection
Write-PrefilledWorkflow `
    -TemplateName '07_EXPLICIT_ALIGN_CROP.json' `
    -OutputName '07_EXPLICIT_ALIGN_CROP.private.json' `
    -Selections @($alignSelections.A, $alignSelections.B)
Write-PrefilledWorkflow `
    -TemplateName '99_OPERATOR_DEVELOPER_TEMPLATE.json' `
    -OutputName '99_OPERATOR_DEVELOPER_TEMPLATE.private.json' `
    -Selections @($mixerSelections.A, $mixerSelections.B) `
    -RequireExplicitDevicePath

$receipt = [ordered]@{
    schema_version = 1
    private_artifact = $true
    generated_utc = [DateTime]::UtcNow.ToString('o')
    environment_commit = [string]$environmentReceipt.repository.commit
    full_clip_signature = $mixerSignature
    mixer_sources = $mixerReceipts
    align_sources = $alignReceipts
    raw_source = $rawMaterialized.receipt
    external_assets = [ordered]@{
        fast_vae = $taeh3Asset.receipt
        hq_vae = $hqVaeAsset.receipt
    }
    workflows = @(
        '01_LC_INSPECT.private.json',
        '02_FAST_HQ_COMPARE.private.json',
        '03_DUAL_SYNTH_LAB.private.json',
        '04_QUAD_CARRIER_DONORS.private.json',
        '05_PROJECT_RESAMPLE.private.json',
        '06_RAW_RECORD_INSPECT.private.json',
        '07_EXPLICIT_ALIGN_CROP.private.json',
        '99_OPERATOR_DEVELOPER_TEMPLATE.private.json'
    )
}
[System.IO.File]::WriteAllText(
    (Join-Path $outputFull 'receipt.json'),
    ($receipt | ConvertTo-Json -Depth 30) + "`n",
    [System.Text.UTF8Encoding]::new($false)
)

Write-Host 'PRIVATE COMFY MASTER WORKFLOWS: READY' -ForegroundColor Green
Write-Host "Open these workflows from: $outputFull"
Write-Host "Isolated Comfy input selections: $masterInput"
