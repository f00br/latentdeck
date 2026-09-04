[CmdletBinding()]
param(
    [string]$OutputDirectory = 'artifacts/release/comfy-recorder',

    [Parameter(Mandatory)]
    [string]$SafetensorsWheelPath,

    [switch]$AllowDirtySource
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force
if (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $repositoryRoot $OutputDirectory
}
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
$lockPath = Join-Path $repositoryRoot 'comfy/latent-cartridge/packaging/windows-x64.lock.json'
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json -Depth 32

function Invoke-Checked {
    param(
        [Parameter(Mandatory)][string]$Context,
        [Parameter(Mandatory)][scriptblock]$Command
    )

    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Context failed with exit code $LASTEXITCODE."
    }
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-NoReparseAncestor {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Context
    )

    $current = [System.IO.Path]::GetFullPath($Path)
    while (-not [string]::IsNullOrWhiteSpace($current)) {
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "$Context traverses a reparse point: $current"
            }
        }
        $parent = [System.IO.Path]::GetDirectoryName($current)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent -ceq $current) {
            break
        }
        $current = $parent
    }
}

function New-FileBinding {
    param([Parameter(Mandatory)][string]$Path)

    $item = Assert-RegularBoundedFile -Path $Path -Context 'Release sidecar' -MaximumBytes 64MB
    return [ordered]@{
        file_name = $item.Name
        byte_length = [int64]$item.Length
        sha256 = Get-Sha256 -Path $item.FullName
    }
}

function Write-Utf8Text {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Text
    )

    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Get-GitSource {
    $commit = (& git -C $repositoryRoot rev-parse HEAD).Trim()
    $branch = (& git -C $repositoryRoot branch --show-current).Trim()
    $tree = (& git -C $repositoryRoot rev-parse 'HEAD^{tree}').Trim()
    $status = @(
        & git -C $repositoryRoot -c core.quotepath=false `
            status --porcelain=v1 --untracked-files=all
    )
    if ($LASTEXITCODE -ne 0 -or $commit -cnotmatch '^[0-9a-f]{40}$' -or
        $tree -cnotmatch '^[0-9a-f]{40}$') {
        throw 'Could not resolve the Comfy Recorder Git source identity.'
    }
    $relativePaths = @(
        & git -C $repositoryRoot -c core.quotepath=false `
            ls-files --cached --others --exclude-standard
    )
    if ($LASTEXITCODE -ne 0 -or $relativePaths.Count -eq 0) {
        throw 'Could not enumerate the Comfy Recorder public source snapshot.'
    }
    $records = [System.Collections.Generic.List[string]]::new()
    foreach ($relativePath in @($relativePaths | Sort-Object -CaseSensitive)) {
        if ([string]::IsNullOrWhiteSpace($relativePath)) {
            continue
        }
        $fullPath = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $relativePath))
        $repositoryPrefix = $repositoryRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
        if (-not $fullPath.StartsWith(
            $repositoryPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Public source snapshot contains an unsafe path: $relativePath"
        }
        if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
            $records.Add("missing`0$relativePath")
            continue
        }
        $item = Get-Item -LiteralPath $fullPath -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Public source snapshot contains a reparse point: $relativePath"
        }
        $records.Add(
            "file`0$($relativePath.Replace('\', '/'))`0$($item.Length)`0$(Get-Sha256 -Path $fullPath)"
        )
    }
    $payload = [System.Text.UTF8Encoding]::new($false).GetBytes(($records -join "`n"))
    $snapshotHash = [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($payload)
    ).ToLowerInvariant()
    return [pscustomobject]@{
        git_commit = $commit
        git_branch = $branch
        git_tree = $tree
        git_dirty = ($status.Count -gt 0)
        git_dirty_entry_count = $status.Count
        public_snapshot_sha256 = $snapshotHash
        public_snapshot_file_count = $records.Count
    }
}

function Assert-RegularBoundedFile {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Context,
        [int64]$MaximumBytes = 128MB
    )

    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.PSIsContainer -or $item.Length -le 0 -or $item.Length -gt $MaximumBytes) {
        throw "$Context is not a bounded regular file: $Path"
    }
    return $item
}

function New-DeterministicZip {
    param(
        [Parameter(Mandatory)][string]$SourceRoot,
        [Parameter(Mandatory)][string]$Destination
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [System.IO.FileStream]::new(
        $Destination,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $false,
            [System.Text.Encoding]::UTF8
        )
        try {
            $files = @(
                Get-ChildItem -LiteralPath $SourceRoot -File -Recurse -Force |
                    Sort-Object { [System.IO.Path]::GetRelativePath($SourceRoot, $_.FullName).Replace('\', '/') }
            )
            foreach ($file in $files) {
                if (($file.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "Bundle staging contains a reparse point: $($file.FullName)"
                }
                $relative = [System.IO.Path]::GetRelativePath($SourceRoot, $file.FullName).Replace('\', '/')
                if ($relative.StartsWith('/') -or $relative.Contains('../') -or
                    $relative -match '(^|/)\.\.($|/)') {
                    throw "Bundle staging contains an unsafe path: $relative"
                }
                $entry = $archive.CreateEntry(
                    $relative,
                    [System.IO.Compression.CompressionLevel]::Optimal
                )
                $entry.LastWriteTime = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
                $input = [System.IO.File]::OpenRead($file.FullName)
                try {
                    $output = $entry.Open()
                    try {
                        $input.CopyTo($output)
                    }
                    finally {
                        $output.Dispose()
                    }
                }
                finally {
                    $input.Dispose()
                }
            }
        }
        finally {
            $archive.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }
}

function Copy-ZipEntry {
    param(
        [Parameter(Mandatory)][string]$ArchivePath,
        [Parameter(Mandatory)][string]$EntryName,
        [Parameter(Mandatory)][string]$Destination
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entries = @($archive.Entries | Where-Object FullName -CEQ $EntryName)
        if ($entries.Count -ne 1 -or $entries[0].Length -le 0 -or $entries[0].Length -gt 1MB) {
            throw "Archive does not contain one bounded required entry: $EntryName"
        }
        $input = $entries[0].Open()
        try {
            $output = [System.IO.FileStream]::new(
                $Destination,
                [System.IO.FileMode]::CreateNew,
                [System.IO.FileAccess]::Write,
                [System.IO.FileShare]::None
            )
            try {
                $input.CopyTo($output)
            }
            finally {
                $output.Dispose()
            }
        }
        finally {
            $input.Dispose()
        }
    }
    finally {
        $archive.Dispose()
    }
}

if (-not $IsWindows -or [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne
    [System.Runtime.InteropServices.Architecture]::X64) {
    throw 'Comfy Recorder bundle builds require Windows x64.'
}
if ([int]$lock.schema_version -ne 1 -or
    [string]$lock.release_label -cne '0.1.0-preview.1' -or
    [string]$lock.release_channel -cne 'unsigned_preview' -or
    [string]$lock.target -cne 'windows-x64' -or
    [string]$lock.python_abi -cne 'cp312-abi3' -or
    (@($lock.supported_python) -join "`0") -cne (@('cp312', 'cp313') -join "`0")) {
    throw 'Comfy Recorder Windows lock identity is invalid.'
}
if (Test-Path -LiteralPath $outputRoot) {
    throw "Refusing to overwrite an existing output directory: $outputRoot"
}
$outputParent = [System.IO.Path]::GetDirectoryName($outputRoot)
if ([string]::IsNullOrWhiteSpace($outputParent)) {
    throw 'Comfy Recorder output directory must have a parent.'
}
[System.IO.Directory]::CreateDirectory($outputParent) | Out-Null
Assert-NoReparseAncestor -Path $outputParent -Context 'Comfy Recorder output path'
if (-not $AllowDirtySource.IsPresent) {
    $artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
    $artifactsPrefix = $artifactsRoot.TrimEnd('\', '/') +
        [System.IO.Path]::DirectorySeparatorChar
    if (-not $outputRoot.StartsWith(
        $artifactsPrefix,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw 'A distributable Comfy Recorder output must stay under the repository artifacts directory.'
    }
    $relativeOutput = [System.IO.Path]::GetRelativePath($repositoryRoot, $outputRoot).
        Replace('\', '/')
    & git -C $repositoryRoot check-ignore --quiet -- $relativeOutput
    if ($LASTEXITCODE -ne 0) {
        throw 'A distributable Comfy Recorder output must be ignored by Git.'
    }
}

$sourceBefore = Get-GitSource
if ($sourceBefore.git_dirty -and -not $AllowDirtySource.IsPresent) {
    throw 'A distributable Comfy Recorder bundle requires a clean source checkout.'
}
if (-not $sourceBefore.git_dirty -and $sourceBefore.git_branch -cne 'main' -and
    -not $AllowDirtySource.IsPresent) {
    throw 'A distributable Comfy Recorder bundle must be built from main.'
}

$scratchRoot = Join-Path $outputParent (
    'latentdeck-comfy-recorder-build-' + [guid]::NewGuid().ToString('N')
)
try {
    $wheelRoot = Join-Path $scratchRoot 'wheels'
    $bundleRoot = Join-Path $scratchRoot 'bundle'
    $artifactStage = Join-Path $scratchRoot 'artifact-set'
    [System.IO.Directory]::CreateDirectory($wheelRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($bundleRoot) | Out-Null
    [System.IO.Directory]::CreateDirectory($artifactStage) | Out-Null
    $bundleWheels = Join-Path $bundleRoot 'wheels'
    [System.IO.Directory]::CreateDirectory($bundleWheels) | Out-Null

    $priorSourceDateEpoch = $env:SOURCE_DATE_EPOCH
    $env:SOURCE_DATE_EPOCH = '315532800'
    Push-Location $repositoryRoot
    try {
        Invoke-Checked -Context 'Cartridge SDK wheel build' -Command {
            & uv build --wheel sdk/python --out-dir $wheelRoot --no-create-gitignore `
                --build-constraints tools/packaging/windows-x64-build-constraints.txt `
                --require-hashes
        }
        Invoke-Checked -Context 'Comfy Recorder wheel build' -Command {
            & uv build --wheel comfy/latent-cartridge --out-dir $wheelRoot --no-create-gitignore `
                --build-constraints tools/packaging/windows-x64-build-constraints.txt `
                --require-hashes
        }
    }
    finally {
        Pop-Location
        if ($null -eq $priorSourceDateEpoch) {
            Remove-Item Env:SOURCE_DATE_EPOCH -ErrorAction SilentlyContinue
        } else {
            $env:SOURCE_DATE_EPOCH = $priorSourceDateEpoch
        }
    }

    $sdkWheel = @(
        Get-ChildItem -LiteralPath $wheelRoot -File -Filter 'latentdeck_cartridge-0.1.0-*.whl'
    )
    if ($sdkWheel.Count -ne 1 -or
        $sdkWheel[0].Name -cne 'latentdeck_cartridge-0.1.0-cp312-abi3-win_amd64.whl') {
        throw 'Cartridge SDK did not build the exact cp312-abi3 Windows x64 wheel.'
    }
    $recorderWheel = @(
        Get-ChildItem -LiteralPath $wheelRoot -File -Filter 'latentdeck_comfy_cartridge-0.1.0-*.whl'
    )
    if ($recorderWheel.Count -ne 1 -or
        $recorderWheel[0].Name -cne 'latentdeck_comfy_cartridge-0.1.0-py3-none-any.whl') {
        throw 'Comfy Recorder did not build the exact platform-neutral wheel.'
    }

    $resolvedSafetensors = (Resolve-Path -LiteralPath $SafetensorsWheelPath).Path
    Assert-NoReparseAncestor -Path $resolvedSafetensors -Context 'Safetensors wheel input'
    if ([System.IO.Path]::GetFileName($resolvedSafetensors) -cne [string]$lock.safetensors.file_name) {
        throw 'Safetensors wheel name does not match the Windows lock.'
    }
    $safetensorsItem = Assert-RegularBoundedFile `
        -Path $resolvedSafetensors `
        -Context 'Safetensors wheel' `
        -MaximumBytes 8MB
    if ($safetensorsItem.Length -ne [int64]$lock.safetensors.byte_length -or
        (Get-Sha256 -Path $resolvedSafetensors) -cne [string]$lock.safetensors.sha256) {
        throw 'Safetensors wheel does not match its pinned length and SHA-256.'
    }

    $wheelSources = @($sdkWheel[0].FullName, $recorderWheel[0].FullName, $resolvedSafetensors)
    $wheelReceipts = [System.Collections.Generic.List[object]]::new()
    foreach ($wheel in $wheelSources) {
        $item = Assert-RegularBoundedFile -Path $wheel -Context 'Bundle wheel'
        $destination = Join-Path $bundleWheels $item.Name
        [System.IO.File]::Copy($item.FullName, $destination, $false)
        $name = if ($item.Name.StartsWith('latentdeck_cartridge-')) {
            'latentdeck-cartridge'
        } elseif ($item.Name.StartsWith('latentdeck_comfy_cartridge-')) {
            'latentdeck-comfy-cartridge'
        } else {
            'safetensors'
        }
        $version = if ($name -ceq 'safetensors') { '0.8.0' } else { '0.1.0' }
        $wheelReceipts.Add([pscustomobject][ordered]@{
            name = $name
            version = $version
            file_name = $item.Name
            byte_length = [int64]$item.Length
            sha256 = Get-Sha256 -Path $item.FullName
        })
    }

    [System.IO.File]::Copy((Join-Path $repositoryRoot 'LICENSE'), (Join-Path $bundleRoot 'LICENSE'), $false)
    [System.IO.File]::Copy(
        (Join-Path $repositoryRoot 'tools/installer/Install-ComfyRecorder.ps1'),
        (Join-Path $bundleRoot 'Install-ComfyRecorder.ps1'),
        $false
    )
    [System.IO.File]::Copy(
        (Join-Path $repositoryRoot 'tools/installer/Verify-ComfyRecorder.py'),
        (Join-Path $bundleRoot 'Verify-ComfyRecorder.py'),
        $false
    )
    [System.IO.File]::Copy(
        (Join-Path $repositoryRoot 'comfy/latent-cartridge/packaging/BUNDLE_README.md'),
        (Join-Path $bundleRoot 'README.md'),
        $false
    )
    $customNodeRoot = Join-Path $bundleRoot 'custom_node'
    [System.IO.Directory]::CreateDirectory($customNodeRoot) | Out-Null
    [System.IO.File]::Copy(
        (Join-Path $repositoryRoot 'comfy/latent-cartridge/packaging/custom_node/__init__.py'),
        (Join-Path $customNodeRoot '__init__.py'),
        $false
    )
    $licenseRoot = Join-Path $bundleRoot 'licenses'
    [System.IO.Directory]::CreateDirectory($licenseRoot) | Out-Null
    Copy-ZipEntry `
        -ArchivePath $resolvedSafetensors `
        -EntryName 'safetensors-0.8.0.dist-info/licenses/LICENSE' `
        -Destination (Join-Path $licenseRoot 'safetensors-LICENSE')

    $sbomPath = Join-Path $bundleRoot 'SBOM.cdx.json'
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath $sbomPath `
        -ArtifactName 'LatentDeck Comfy LC Recorder' `
        -ArtifactVersion ([string]$lock.release_label) `
        -ArtifactScope comfy-recorder `
        -CargoPackage @('latentdeck-cartridge', 'latentdeck-cartridge-python') `
        -PythonPackage @(
            'latentdeck-cartridge',
            'latentdeck-comfy-cartridge',
            'safetensors'
        ) `
        -PythonBuildPackage @('maturin==1.15.0', 'uv-build==0.12.7') `
        -Deterministic | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Comfy Recorder SBOM generation failed.'
    }
    $safetensorsNativeEvidence = Merge-SafetensorsNativeClosureIntoSbom `
        -SbomPath $sbomPath `
        -WheelPath $resolvedSafetensors
    $sbom = Get-Content -LiteralPath $sbomPath -Raw | ConvertFrom-Json -Depth 100
    $rootLicense = @($sbom.metadata.component.licenses | Where-Object {
        $null -ne $_.PSObject.Properties['license'] -and
        [string]$_.license.name -ceq 'Apache-2.0'
    })
    if ([string]$sbom.metadata.component.name -cne 'LatentDeck Comfy LC Recorder' -or
        [string]$sbom.metadata.component.version -cne [string]$lock.release_label -or
        @($sbom.metadata.component.licenses).Count -ne 1 -or $rootLicense.Count -ne 1) {
        throw 'Comfy Recorder SBOM root identity or reviewed license is invalid.'
    }
    $selectionRoots = @($sbom.components | Where-Object {
        @($_.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:selection-root' -and
            [string]$_.value -ceq 'true'
        }).Count -eq 1
    })
    $rootIdentities = @(
        foreach ($component in $selectionRoots) {
            $ecosystems = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:ecosystem'
            })
            if ($ecosystems.Count -ne 1) {
                throw "Comfy Recorder SBOM root has no exact ecosystem: $($component.name)"
            }
            "$([string]$ecosystems[0].value):$($component.name)@$($component.version)"
        }
    ) | Sort-Object
    $expectedRootIdentities = @(
        'python:latentdeck-cartridge@0.1.0',
        'python:latentdeck-comfy-cartridge@0.1.0',
        'python:maturin@1.15.0',
        'python:safetensors@0.8.0',
        'python:uv-build@0.12.7',
        'rust:latentdeck-cartridge-python@0.1.0',
        'rust:latentdeck-cartridge@0.1.0'
    ) | Sort-Object
    if (($rootIdentities -join "`0") -cne ($expectedRootIdentities -join "`0")) {
        throw 'Comfy Recorder SBOM does not cover its exact runtime and build roots.'
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
    if ($includedScopePolicy.Count -ne 1 -or $excludedScopePolicy.Count -ne 1 -or
        $targetPlatformPolicy.Count -ne 1) {
        throw 'Comfy Recorder SBOM dependency scope policy is invalid.'
    }
    $dependencyScopeCounts = [ordered]@{
        artifact = 0
        runtime = 0
        build = 0
        'runtime+build' = 0
    }
    $missingLicenses = [System.Collections.Generic.List[object]]::new()
    foreach ($component in @($sbom.metadata.component) + @($sbom.components)) {
        $scopeProperties = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope'
        })
        if ($scopeProperties.Count -ne 1 -or
            [string]$scopeProperties[0].value -cnotin @(
                'artifact', 'runtime', 'build', 'runtime+build'
            )) {
            throw "Comfy Recorder SBOM component has an invalid scope: $($component.name)"
        }
        $scope = [string]$scopeProperties[0].value
        $dependencyScopeCounts[$scope] = [int]$dependencyScopeCounts[$scope] + 1
        if ($null -eq $component.PSObject.Properties['licenses'] -or
            @($component.licenses).Count -eq 0) {
            $missingLicenses.Add([ordered]@{
                name = [string]$component.name
                version = [string]$component.version
            })
        }
    }
    if ($missingLicenses.Count -ne 0) {
        throw 'Comfy Recorder SBOM contains a dependency without reviewed license metadata.'
    }

    $noticeLines = [System.Collections.Generic.List[string]]::new()
    foreach ($line in @(
        '# LatentDeck Comfy LC Recorder third-party notices',
        '',
        "Artifact: LatentDeck Comfy LC Recorder $([string]$lock.release_label)",
        '',
        'This inventory covers the exact Windows runtime wheels, their native Rust closure,',
        'and the exact non-redistributed wheel build backends for this artifact.',
        'The bundled Safetensors Python package is relocated under',
        '`latentdeck_recorder_vendor`, and its internal absolute import is rewritten to a',
        'relative import solely for namespace isolation.',
        'License labels are taken from locked package metadata. Full reviewed license texts',
        'and exact component mappings are in `THIRD_PARTY_LICENSES.json`.',
        '',
        '## Components',
        ''
    )) {
        $noticeLines.Add($line)
    }
    foreach ($component in @($sbom.components | Sort-Object 'bom-ref')) {
        $licenseLabels = @(
            foreach ($entry in @($component.licenses)) {
                if ($null -ne $entry.PSObject.Properties['expression'] -and
                    -not [string]::IsNullOrWhiteSpace([string]$entry.expression)) {
                    [string]$entry.expression
                } elseif ($null -ne $entry.PSObject.Properties['license']) {
                    if ($null -ne $entry.license.PSObject.Properties['id'] -and
                        -not [string]::IsNullOrWhiteSpace([string]$entry.license.id)) {
                        [string]$entry.license.id
                    } elseif ($null -ne $entry.license.PSObject.Properties['name'] -and
                        -not [string]::IsNullOrWhiteSpace([string]$entry.license.name)) {
                        [string]$entry.license.name
                    }
                }
            }
        )
        $ecosystems = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:ecosystem'
        })
        $scopes = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope'
        })
        if ($ecosystems.Count -ne 1 -or $scopes.Count -ne 1 -or
            $licenseLabels.Count -eq 0) {
            throw "Comfy Recorder notice identity is incomplete: $($component.'bom-ref')"
        }
        $noticeLines.Add(
            "- $($component.name) $($component.version) " +
            "[$([string]$ecosystems[0].value); $([string]$scopes[0].value)] - " +
            (@($licenseLabels | Sort-Object -Unique) -join ' OR ')
        )
    }
    $noticePath = Join-Path $bundleRoot 'THIRD_PARTY_NOTICES.md'
    Write-Utf8Text -Path $noticePath -Text (($noticeLines -join "`n") + "`n")

    $licenseBundlePath = Join-Path $bundleRoot 'THIRD_PARTY_LICENSES.json'
    $licenseBundle = New-ReleaseLicenseBundle `
        -SbomPath $sbomPath `
        -ArtifactName 'LatentDeck Comfy LC Recorder' `
        -ArtifactVersion ([string]$lock.release_label) `
        -OutputPath $licenseBundlePath `
        -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md') `
        -SafetensorsWheelPath $resolvedSafetensors
    $licenseReview = [ordered]@{
        schema_version = 1
        status = 'complete'
        policy = 'No license value is inferred when upstream metadata is absent.'
        component_count = @($sbom.components).Count
        root_component_reviewed = $true
        dependency_scope_counts = $dependencyScopeCounts
        selection_root_count = $selectionRoots.Count
        expected_selection_root_count = 7
        selection_roots = $rootIdentities
        missing_license_component_count = 0
        missing_license_components = @()
        license_bundle = [ordered]@{
            schema_version = 1
            component_count = $licenseBundle.ComponentCount
            text_count = $licenseBundle.TextCount
            build_only_no_text_disposition_count = $licenseBundle.NoTextDispositionCount
            redistributed_component_text_coverage = 'complete'
        }
    }
    $licenseReviewPath = Join-Path $bundleRoot 'LICENSE-REVIEW.json'
    Write-Utf8Text `
        -Path $licenseReviewPath `
        -Text (($licenseReview | ConvertTo-Json -Depth 32) + "`n")
    $manifest = [ordered]@{
        schema_version = 1
        release_label = [string]$lock.release_label
        target = [string]$lock.target
        python_abi = [string]$lock.python_abi
        supported_python = @($lock.supported_python)
        install_mode = 'isolated_project_vendor_private_safetensors'
        source = [ordered]@{
            git_commit = $sourceBefore.git_commit
            git_tree = $sourceBefore.git_tree
            public_snapshot_sha256 = $sourceBefore.public_snapshot_sha256
            public_snapshot_file_count = $sourceBefore.public_snapshot_file_count
        }
        wheels = @($wheelReceipts | Sort-Object name)
        release_material = [ordered]@{
            sbom = New-FileBinding -Path $sbomPath
            safetensors_native_closure = $safetensorsNativeEvidence
            third_party_notices = New-FileBinding -Path $noticePath
            license_bundle = New-FileBinding -Path $licenseBundlePath
            license_review = New-FileBinding -Path $licenseReviewPath
        }
    }
    Write-Utf8Text `
        -Path (Join-Path $bundleRoot 'BUNDLE-MANIFEST.json') `
        -Text (($manifest | ConvertTo-Json -Depth 16) + "`n")

    $baseName = [string]$lock.artifact_name
    $archivePath = Join-Path $artifactStage "$baseName.zip"
    New-DeterministicZip -SourceRoot $bundleRoot -Destination $archivePath
    $archiveItem = Assert-RegularBoundedFile `
        -Path $archivePath `
        -Context 'Recorder bundle' `
        -MaximumBytes 256MB

    $externalSbomPath = Join-Path $artifactStage "$baseName-sbom.cdx.json"
    $externalNoticePath = Join-Path $artifactStage "$baseName-THIRD-PARTY-NOTICES.md"
    $externalLicenseBundlePath = Join-Path $artifactStage "$baseName-THIRD-PARTY-LICENSES.json"
    $externalLicenseReviewPath = Join-Path $artifactStage "$baseName-license-review.json"
    [System.IO.File]::Copy($sbomPath, $externalSbomPath, $false)
    [System.IO.File]::Copy($noticePath, $externalNoticePath, $false)
    [System.IO.File]::Copy($licenseReviewPath, $externalLicenseReviewPath, $false)
    $externalLicenseBundle = New-ReleaseLicenseBundle `
        -SbomPath $externalSbomPath `
        -ArtifactName 'LatentDeck Comfy LC Recorder' `
        -ArtifactVersion ([string]$lock.release_label) `
        -OutputPath $externalLicenseBundlePath `
        -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md') `
        -SafetensorsWheelPath $resolvedSafetensors
    if ($externalLicenseBundle.ComponentCount -ne $licenseBundle.ComponentCount -or
        $externalLicenseBundle.TextCount -ne $licenseBundle.TextCount -or
        $externalLicenseBundle.NoTextDispositionCount -ne $licenseBundle.NoTextDispositionCount) {
        throw 'External Recorder license mapping drifted from the in-bundle runtime closure.'
    }

    $sourceAfter = Get-GitSource
    if ($sourceAfter.git_commit -cne $sourceBefore.git_commit -or
        $sourceAfter.git_branch -cne $sourceBefore.git_branch -or
        $sourceAfter.git_tree -cne $sourceBefore.git_tree -or
        $sourceAfter.git_dirty -ne $sourceBefore.git_dirty -or
        $sourceAfter.git_dirty_entry_count -ne $sourceBefore.git_dirty_entry_count -or
        $sourceAfter.public_snapshot_sha256 -cne $sourceBefore.public_snapshot_sha256 -or
        $sourceAfter.public_snapshot_file_count -ne $sourceBefore.public_snapshot_file_count) {
        throw 'The public source snapshot changed while building the Comfy Recorder bundle.'
    }
    $receipt = [ordered]@{
        schema_version = 1
        artifact_kind = 'comfy_recorder_bundle'
        release_label = [string]$lock.release_label
        release_channel = [string]$lock.release_channel
        target = [string]$lock.target
        python_abi = [string]$lock.python_abi
        supported_python = @($lock.supported_python)
        signed = $false
        unsigned = $true
        distributable = (-not $sourceBefore.git_dirty -and $sourceBefore.git_branch -ceq 'main')
        contains_model_weights = $false
        contains_cartridges = $false
        source = [ordered]@{
            git_commit = $sourceBefore.git_commit
            git_branch = $sourceBefore.git_branch
            git_tree = $sourceBefore.git_tree
            git_dirty = $sourceBefore.git_dirty
            git_dirty_entry_count = $sourceBefore.git_dirty_entry_count
            public_snapshot_sha256 = $sourceBefore.public_snapshot_sha256
            public_snapshot_file_count = $sourceBefore.public_snapshot_file_count
        }
        packages = @($wheelReceipts | Sort-Object name)
        archive = [ordered]@{
            file_name = $archiveItem.Name
            byte_length = [int64]$archiveItem.Length
            sha256 = Get-Sha256 -Path $archiveItem.FullName
        }
        sbom = New-FileBinding -Path $externalSbomPath
        third_party_notices = New-FileBinding -Path $externalNoticePath
        license_bundle = New-FileBinding -Path $externalLicenseBundlePath
        license_review = New-FileBinding -Path $externalLicenseReviewPath
    }
    $receipt.sbom['format'] = 'CycloneDX-1.5'
    $receipt.sbom['component_count'] = @($sbom.components).Count
    $receipt.sbom['selection_root_count'] = $selectionRoots.Count
    $receipt.sbom['selection_roots'] = $rootIdentities
    $receipt.sbom['dependency_scope_counts'] = $dependencyScopeCounts
    $receipt.sbom['safetensors_native_closure'] = $safetensorsNativeEvidence
    $receipt.license_bundle['schema_version'] = 1
    $receipt.license_bundle['component_count'] = $externalLicenseBundle.ComponentCount
    $receipt.license_bundle['text_count'] = $externalLicenseBundle.TextCount
    $receipt.license_bundle['build_only_no_text_disposition_count'] = `
        $externalLicenseBundle.NoTextDispositionCount
    $receipt.license_review['status'] = [string]$licenseReview.status
    $receipt.license_review['missing_license_component_count'] = 0
    $receiptPath = Join-Path $artifactStage "$baseName.receipt.json"
    Write-Utf8Text -Path $receiptPath -Text (($receipt | ConvertTo-Json -Depth 16) + "`n")
    $checksumPath = Join-Path $artifactStage "$baseName.SHA256SUMS.txt"
    $checksumBindings = @(
        $receipt.archive,
        $receipt.sbom,
        $receipt.third_party_notices,
        $receipt.license_bundle,
        $receipt.license_review
    ) | Sort-Object { [string]$_['file_name'] } -CaseSensitive
    Write-Utf8Text -Path $checksumPath -Text (
        (@($checksumBindings | ForEach-Object { "$($_.sha256)  $($_.file_name)" }) -join "`n") +
        "`n"
    )

    $expectedOutputFiles = @(
        "$baseName.zip",
        "$baseName.receipt.json",
        "$baseName.SHA256SUMS.txt",
        "$baseName-sbom.cdx.json",
        "$baseName-THIRD-PARTY-NOTICES.md",
        "$baseName-THIRD-PARTY-LICENSES.json",
        "$baseName-license-review.json"
    ) | Sort-Object
    $actualOutputFiles = @(
        Get-ChildItem -LiteralPath $artifactStage -File -Force |
            Select-Object -ExpandProperty Name |
            Sort-Object
    )
    if (($actualOutputFiles -join "`0") -cne ($expectedOutputFiles -join "`0")) {
        throw 'Comfy Recorder artifact set contains an unexpected file.'
    }
    [System.IO.Directory]::Move($artifactStage, $outputRoot)

    $receipt | ConvertTo-Json -Depth 16
}
finally {
    if (Test-Path -LiteralPath $scratchRoot) {
        Remove-Item -LiteralPath $scratchRoot -Recurse -Force
    }
}
