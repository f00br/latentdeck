[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PythonEmbedArchive,

    [string]$DecoderAssetContractPath,

    [Parameter(Mandatory)]
    [string]$PackVersion,

    [ValidateSet('unsigned_preview', 'stable')]
    [string]$ReleaseChannel = 'unsigned_preview',

    [string]$ReleaseLabel,

    [string]$OutputDirectory,

    [switch]$AllowNetwork,

    [switch]$RequireCuda,

    [string]$SigningCommand,

    [switch]$DevelopmentBuild
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$ProgressPreference = 'SilentlyContinue'

if ($ReleaseChannel -cnotin @('unsigned_preview', 'stable')) {
    throw 'ReleaseChannel must be exactly unsigned_preview or stable.'
}

Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'PublicNativeBuild.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'PublicWheelAudit.psm1') -Force

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$sourceBefore = Get-PackagingSourceState -RepositoryRoot $repoRoot
$curationLock = Join-Path `
    $repoRoot `
    'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
$lock = Read-StrictJsonFile -Path $curationLock
$curationLockSha256 = (
    Get-FileHash -LiteralPath $curationLock -Algorithm SHA256
).Hash.ToLowerInvariant()
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
Assert-SemVer -Value $PackVersion -Name 'PackVersion'
if ($PackVersion -cne [string]$lock.pack_version -or $PackVersion -cne '0.2.1') {
    throw (
        "The official H3 builder is bound by its curation lock to immutable Codec Pack " +
        "version 0.2.1; found '$PackVersion'."
    )
}
if ([string]::IsNullOrWhiteSpace($ReleaseLabel)) {
    $ReleaseLabel = if ($ReleaseChannel -ceq 'unsigned_preview') {
        '0.1.0-preview.1'
    } else {
        '0.1.0'
    }
}
Assert-SemVer -Value $ReleaseLabel -Name 'ReleaseLabel'
if ($ReleaseChannel -ceq 'unsigned_preview') {
    if ($ReleaseLabel -cne '0.1.0-preview.1') {
        throw 'The unsigned preview channel requires release label 0.1.0-preview.1.'
    }
    if (-not [string]::IsNullOrWhiteSpace($SigningCommand)) {
        throw 'The unsigned preview channel refuses a signing command.'
    }
} else {
    if ($ReleaseLabel -cne '0.1.0') {
        throw 'The stable channel requires release label 0.1.0.'
    }
    if ([string]::IsNullOrWhiteSpace($SigningCommand)) {
        throw 'The stable channel requires an Authenticode signing command.'
    }
}

$distributable = (-not $DevelopmentBuild.IsPresent -and
    $sourceBefore.Branch -ceq 'main' -and -not [bool]$sourceBefore.Dirty)
if (-not $DevelopmentBuild.IsPresent -and -not $distributable) {
    throw 'H3 Codec Packs must be built from a clean main checkout; use -DevelopmentBuild only for non-distributable local contract work.'
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $artifactsRoot 'codec-pack'
}
$outputRoot = [System.IO.Path]::GetFullPath($OutputDirectory)
Assert-ChildPath -ParentPath $artifactsRoot -CandidatePath $outputRoot -AllowParent | Out-Null
$finalDirectory = Join-Path $outputRoot $PackVersion
if (Test-Path -LiteralPath $finalDirectory) {
    throw "Refusing to overwrite an existing Codec Pack artifact directory: $finalDirectory"
}

if ([string]::IsNullOrWhiteSpace($DecoderAssetContractPath)) {
    $DecoderAssetContractPath = Join-Path `
        $repoRoot `
        'codec-host/codecs/h3/packaging/taeh3.asset.json'
}
$runtimeArchive = (Resolve-Path -LiteralPath $PythonEmbedArchive).Path
$assetContract = (Resolve-Path -LiteralPath $DecoderAssetContractPath).Path
$curator = Join-Path $PSScriptRoot 'codec_pack_curator.py'
$baseNoticeSource = Join-Path $repoRoot 'codec-host/codecs/h3/THIRD_PARTY_NOTICES.md'
$apacheLicenseSource = Join-Path $repoRoot 'LICENSE'

$uv = Get-Command uv.exe -ErrorAction Stop
$uvVersion = (& $uv.Source --version).Trim()
if ($LASTEXITCODE -ne 0 -or $uvVersion -cne 'uv 0.11.8 (0e961dd9a 2026-04-27 x86_64-pc-windows-msvc)') {
    throw "Codec Pack builder requires the pinned uv 0.11.8 Windows binary; found '$uvVersion'."
}

$buildPython = Join-Path $repoRoot '.venv/Scripts/python.exe'
if (-not (Test-Path -LiteralPath $buildPython -PathType Leaf)) {
    throw 'Codec Pack curation requires the locked repository .venv Python.'
}
$buildPythonVersion = (& $buildPython -I -s -B -c 'import platform; print(platform.python_version())').Trim()
if ($LASTEXITCODE -ne 0 -or $buildPythonVersion -cnotmatch '^3\.13\.') {
    throw "Codec Pack curation requires Python 3.13; found '$buildPythonVersion'."
}

if ($lock.uv_version -cne '0.11.8' -or $lock.platform -cne 'windows-x86_64') {
    throw 'Codec Pack curation lock does not match the pinned builder platform.'
}
$expectedDependencyNames = @(
    'colorama', 'filelock', 'fsspec', 'Jinja2', 'MarkupSafe', 'mpmath', 'msgpack',
    'networkx', 'numpy', 'safetensors', 'setuptools', 'sympy', 'torch', 'tqdm',
    'typing_extensions'
) | Sort-Object -CaseSensitive
$actualDependencyNames = @($lock.dependencies | ForEach-Object { [string]$_.name }) |
    Sort-Object -CaseSensitive
if (($actualDependencyNames -join "`0") -cne ($expectedDependencyNames -join "`0") -or
    $actualDependencyNames.Count -ne
        @($actualDependencyNames | Sort-Object -CaseSensitive -Unique).Count) {
    throw 'Codec Pack curation lock does not contain the exact Windows runtime dependency set.'
}
$uvLockText = [System.IO.File]::ReadAllText((Join-Path $repoRoot 'uv.lock'))
foreach ($dependency in @($lock.dependencies)) {
    Assert-ExactProperties `
        -Object $dependency `
        -Required @(
            'name', 'version', 'source_url', 'license_expression', 'wheel',
            'content_sha256'
        ) `
        -Context "H3 runtime dependency $($dependency.name)"
    Assert-ExactProperties `
        -Object $dependency.wheel `
        -Required @('file_name', 'url', 'byte_length', 'sha256') `
        -Context "H3 runtime wheel $($dependency.name)"
    $wheelName = [string]$dependency.wheel.file_name
    $wheelUrl = [string]$dependency.wheel.url
    $wheelHash = [string]$dependency.wheel.sha256
    if ($wheelName -cnotmatch '^[A-Za-z0-9_.+\-]+\.whl$' -or
        [uri]::UnescapeDataString(([uri]$wheelUrl).Segments[-1]) -cne $wheelName -or
        [int64]$dependency.wheel.byte_length -le 0 -or
        [int64]$dependency.wheel.byte_length -ge 2GB -or
        $wheelHash -cnotmatch '^[0-9a-f]{64}$') {
        throw "H3 runtime wheel identity is invalid: $($dependency.name)"
    }
    if ([string]$dependency.name -ceq 'torch') {
        if ([string]$dependency.version -cne '2.13.0+cu130' -or
            $wheelUrl -cne
                'https://download-r2.pytorch.org/whl/cu130/torch-2.13.0%2Bcu130-cp313-cp313-win_amd64.whl' -or
            $wheelName -cne 'torch-2.13.0+cu130-cp313-cp313-win_amd64.whl' -or
            [int64]$dependency.wheel.byte_length -ne 1915517499 -or
            $wheelHash -cne
                'cf23236e9deed7d3510d14d9b9592d75d272ef7b35bbfee31a02bea339c73971' -or
            $uvLockText.IndexOf(
                "{ url = `"$wheelUrl`", upload-time = ",
                [System.StringComparison]::Ordinal
            ) -lt 0) {
            throw 'The reviewed Torch Windows wheel identity drifted from the H3/uv locks.'
        }
    } else {
        $uvWheelRecord = (
            "{ url = `"$wheelUrl`", hash = `"sha256:$wheelHash`", " +
            "size = $([int64]$dependency.wheel.byte_length),"
        )
        if ($uvLockText.IndexOf($uvWheelRecord, [System.StringComparison]::Ordinal) -lt 0) {
            throw "H3 runtime wheel identity drifted from uv.lock: $($dependency.name)"
        }
    }
}
$safetensorsClosureLock = Read-StrictJsonFile -Path (
    Join-Path $repoRoot 'comfy/latent-cartridge/packaging/safetensors-native-0.8.0.lock.json'
)
$safetensorsRuntimeLock = @($lock.dependencies | Where-Object {
    [string]$_.name -ceq 'safetensors'
})
if ($safetensorsRuntimeLock.Count -ne 1 -or
    [string]$safetensorsRuntimeLock[0].wheel.file_name -cne
        [string]$safetensorsClosureLock.wheel.file_name -or
    [int64]$safetensorsRuntimeLock[0].wheel.byte_length -ne
        [int64]$safetensorsClosureLock.wheel.byte_length -or
    [string]$safetensorsRuntimeLock[0].wheel.sha256 -cne
        [string]$safetensorsClosureLock.wheel.sha256 -or
    [string]$safetensorsRuntimeLock[0].wheel.url -cne
        [string]$safetensorsClosureLock.wheel.url) {
    throw 'H3 and Comfy Recorder Safetensors wheel identities are not identical.'
}
$expectedArchiveName = [string]$lock.python_runtime.archive_filename
if ((Split-Path -Leaf $runtimeArchive) -cne $expectedArchiveName) {
    throw "CPython runtime archive must retain its pinned filename '$expectedArchiveName'."
}
$runtimeArchiveHash = (
    Get-FileHash -LiteralPath $runtimeArchive -Algorithm SHA256
).Hash.ToLowerInvariant()
if ($runtimeArchiveHash -cne [string]$lock.python_runtime.sha256) {
    throw 'CPython runtime archive SHA-256 does not match the curation lock.'
}

$workRoot = Join-Path $artifactsRoot ".h3-codec-pack-$([guid]::NewGuid().ToString('N'))"
Assert-SafeTemporaryDirectory `
    -ParentPath $artifactsRoot `
    -CandidatePath $workRoot `
    -RequiredLeafPrefix '.h3-codec-pack-' | Out-Null

function Invoke-Checked {
    param(
        [Parameter(Mandatory)]
        [string]$Executable,

        [Parameter(Mandatory)]
        [string[]]$Arguments,

        [Parameter(Mandatory)]
        [string]$Context,

        [switch]$SuppressOutput
    )

    if ($SuppressOutput) {
        & $Executable @Arguments | Out-Null
    } else {
        & $Executable @Arguments
    }
    if ($LASTEXITCODE -ne 0) {
        throw "$Context failed with exit code $LASTEXITCODE."
    }
}

try {
    [System.IO.Directory]::CreateDirectory($workRoot) | Out-Null
    $exportPath = Join-Path $workRoot 'locked-requirements-export.txt'
    Invoke-Checked `
        -Executable $uv.Source `
        -Context 'locked dependency export' `
        -SuppressOutput `
        -Arguments @(
        'export', '--locked', '--package', 'latentdeck-codec-h3', '--extra', 'cu130',
        '--no-dev', '--no-emit-local', '--format', 'requirements.txt',
        '--output-file', $exportPath
    )
    $exported = [System.IO.File]::ReadAllText($exportPath)
    foreach ($dependency in @($lock.dependencies)) {
        $version = [regex]::Escape([string]$dependency.version)
        $candidateNames = @(
            [string]$dependency.name,
            ([string]$dependency.name).Replace('_', '-')
        ) | Select-Object -Unique
        $found = @($candidateNames | Where-Object {
            $name = [regex]::Escape($_)
            $exported -cmatch "(?im)^$name==$version(?:\s|$)"
        }).Count -gt 0
        if (-not $found) {
            throw "uv.lock export does not contain pinned dependency $($dependency.name)==$($dependency.version)."
        }
    }

    $requirementsPath = Join-Path $workRoot 'windows-requirements.txt'
    $requirementLines = @(
        foreach ($dependency in @($lock.dependencies)) {
            (
                "$($dependency.name) @ $($dependency.wheel.url) " +
                "--hash=sha256:$($dependency.wheel.sha256)"
            )
        }
    )
    [System.IO.File]::WriteAllText(
        $requirementsPath,
        (($requirementLines | Sort-Object -CaseSensitive) -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    $sitePackages = Join-Path $workRoot 'site-packages'
    [System.IO.Directory]::CreateDirectory($sitePackages) | Out-Null
    $installArguments = @(
        'pip', 'install', '--target', $sitePackages, '--python-version', '3.13',
        '--python-platform', 'windows', '--link-mode', 'copy', '--no-deps',
        '--only-binary', ':all:', '--require-hashes', '--requirement', $requirementsPath
    )
    if (-not $AllowNetwork) {
        $installArguments += '--offline'
    }
    Invoke-Checked `
        -Executable $uv.Source `
        -Arguments $installArguments `
        -Context 'pinned Windows runtime dependency installation'

    $wheels = Join-Path $workRoot 'wheels'
    [System.IO.Directory]::CreateDirectory($wheels) | Out-Null
    $buildConstraints = Join-Path `
        $repoRoot `
        'tools/packaging/windows-x64-build-constraints.txt'
    $localProjects = @($lock.local_projects | ForEach-Object { [string]$_.name })
    if ($localProjects.Count -eq 0 -or
        $localProjects.Count -ne (@($localProjects | Select-Object -Unique)).Count) {
        throw 'Codec Pack curation lock local_projects must be a non-empty unique set.'
    }

    $savedRustFlags = $env:RUSTFLAGS
    $savedEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
    $savedSourceDateEpoch = $env:SOURCE_DATE_EPOCH
    $nativeBuildPolicy = New-PublicRustBuildPolicy `
        -RepositoryRoot $repoRoot `
        -AdditionalForbiddenPathRoot @($workRoot) `
        -AdditionalRemapPathRoot @($workRoot)
    try {
        $env:SOURCE_DATE_EPOCH = '315532800'
        Set-PublicRustBuildPolicy -Policy $nativeBuildPolicy

        foreach ($project in $localProjects) {
            $buildArguments = @(
                'build', '--wheel', '--package', $project, '--out-dir', $wheels,
                '--build-constraints', $buildConstraints, '--require-hashes'
            )
            if (-not $AllowNetwork) {
                $buildArguments += '--offline'
            }
            Invoke-Checked `
                -Executable $uv.Source `
                -Arguments $buildArguments `
                -Context "local wheel build for $project"
        }
    } finally {
        if ($null -eq $savedRustFlags) {
            Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
        } else {
            $env:RUSTFLAGS = $savedRustFlags
        }
        if ($null -eq $savedEncodedRustFlags) {
            Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
        } else {
            $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustFlags
        }
        if ($null -eq $savedSourceDateEpoch) {
            Remove-Item Env:SOURCE_DATE_EPOCH -ErrorAction SilentlyContinue
        } else {
            $env:SOURCE_DATE_EPOCH = $savedSourceDateEpoch
        }
    }
    $wheelPaths = @(Get-ChildItem -LiteralPath $wheels -Filter '*.whl' -File | Sort-Object Name)
    if ($wheelPaths.Count -ne $localProjects.Count) {
        throw 'Local Codec Pack wheel build did not produce the exact expected wheel count.'
    }
    foreach ($wheel in $wheelPaths) {
        $wheelAuditParameters = @{
            Path = $wheel.FullName
            ForbiddenPathRoot = $nativeBuildPolicy.ForbiddenPathRoots
            Context = "H3 Codec Pack local wheel $($wheel.Name)"
            RequireDeterministicTimestamps = $true
            ForbidEmbeddedSbom = $true
        }
        Assert-PublicProjectWheel @wheelAuditParameters | Out-Null
    }
    $localInstallArguments = @(
        'pip', 'install', '--target', $sitePackages, '--python-version', '3.13',
        '--python-platform', 'windows', '--link-mode', 'copy', '--no-deps', '--reinstall'
    ) + @($wheelPaths | ForEach-Object { $_.FullName })
    if (-not $AllowNetwork) {
        $localInstallArguments += '--offline'
    }
    Invoke-Checked `
        -Executable $uv.Source `
        -Arguments $localInstallArguments `
        -Context 'local Codec Pack wheel installation'

    $baseNotice = Join-Path $workRoot 'BASE_NOTICES.md'
    $baseNoticeText = (
        [System.IO.File]::ReadAllText($baseNoticeSource) +
        "`n## LatentDeck original code`n`n" +
        "LatentDeck original Codec Pack code is distributed under Apache-2.0.`n`n" +
        [System.IO.File]::ReadAllText($apacheLicenseSource) +
        "`n"
    )
    [System.IO.File]::WriteAllText(
        $baseNotice,
        $baseNoticeText,
        [System.Text.UTF8Encoding]::new($false)
    )

    $nativeRustSbom = Join-Path $workRoot 'NATIVE_RUST_SBOM.cdx.json'
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath $nativeRustSbom `
        -ArtifactName 'LatentDeck H3 Native Extensions' `
        -ArtifactVersion $PackVersion `
        -ArtifactScope h3-native `
        -CargoPackage @('latentdeck-cartridge-python', 'latentdeck-gpu-python') `
        -Deterministic | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'H3 native extension Rust SBOM generation failed.'
    }
    $safetensorsNativeEvidence = Merge-SafetensorsNativeClosureIntoSbom `
        -SbomPath $nativeRustSbom `
        -DistributionRoot $sitePackages
    $nativeRustLicenses = Join-Path $workRoot 'NATIVE_RUST_LICENSES.json'
    $nativeLicenseBundle = New-ReleaseLicenseBundle `
        -SbomPath $nativeRustSbom `
        -ArtifactName 'LatentDeck H3 Native Extensions' `
        -ArtifactVersion $PackVersion `
        -OutputPath $nativeRustLicenses `
        -RepositoryNoticePath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md') `
        -SafetensorsDistributionRoot $sitePackages
    if ($nativeLicenseBundle.ComponentCount -lt 3 -or
        $nativeLicenseBundle.TextCount -lt 1) {
        throw 'H3 native extension Rust license bundle is incomplete.'
    }

    $metadata = Join-Path $workRoot 'metadata'
    Invoke-Checked -Executable $buildPython -Context 'Codec Pack dependency curation' -Arguments @(
        '-I', '-s', '-B', '-X', 'utf8', $curator, 'curate',
        '--lock', $curationLock,
        '--site-packages', $sitePackages,
        '--base-notice', $baseNotice,
        '--native-rust-sbom', $nativeRustSbom,
        '--native-rust-licenses', $nativeRustLicenses,
        '--metadata-output', $metadata,
        '--pack-version', $PackVersion,
        '--source-commit', ([string]$sourceBefore.Commit)
    )

    $runtime = Join-Path $workRoot 'runtime'
    Invoke-Checked -Executable $buildPython -Context 'CPython embed runtime preparation' -Arguments @(
        '-I', '-s', '-B', '-X', 'utf8', $curator, 'prepare-runtime',
        '--lock', $curationLock,
        '--archive', $runtimeArchive,
        '--destination', $runtime
    )

    $stagedOutput = Join-Path $workRoot 'output'
    $archiveOutput = @(& (Join-Path $PSScriptRoot 'New-H3CodecPack.ps1') `
        -RuntimeSource $runtime `
        -PackageSource $sitePackages `
        -NoticeSource (Join-Path $metadata 'THIRD_PARTY_NOTICES.md') `
        -DependencyInventoryPath (Join-Path $metadata 'DEPENDENCY_INVENTORY.json') `
        -SbomPath (Join-Path $metadata 'SBOM.cdx.json') `
        -DecoderAssetContractPath $assetContract `
        -PackVersion $PackVersion `
        -OutputDirectory $stagedOutput)
    if ($archiveOutput.Count -ne 1 -or
        -not (Test-Path -LiteralPath ([string]$archiveOutput[0]) -PathType Leaf)) {
        throw 'Codec Pack archive builder did not return exactly one archive path.'
    }
    $archivePath = (Resolve-Path -LiteralPath ([string]$archiveOutput[0])).Path
    $stagedVersion = Split-Path -Parent $archivePath
    $packageReceipt = Get-Content -Raw -LiteralPath (
        Join-Path $stagedVersion 'package-receipt.json'
    ) | ConvertFrom-Json -Depth 100
    if ([int]$packageReceipt.schema_version -ne 1 -or
        [string]$packageReceipt.pack_id -cne 'org.latentdeck.h3' -or
        [string]$packageReceipt.pack_version -cne $PackVersion -or
        [string]$packageReceipt.adapter_version -cne '0.2.0' -or
        [string]$packageReceipt.native_rust.sbom_path -cne 'NATIVE_RUST_SBOM.cdx.json' -or
        [string]$packageReceipt.native_rust.license_bundle_path -cne 'NATIVE_RUST_LICENSES.json' -or
        [string]$packageReceipt.native_rust.sbom_sha256 -cne
            (Get-FileHash -LiteralPath $nativeRustSbom -Algorithm SHA256).Hash.ToLowerInvariant() -or
        [string]$packageReceipt.native_rust.license_bundle_sha256 -cne
            (Get-FileHash -LiteralPath $nativeRustLicenses -Algorithm SHA256).Hash.ToLowerInvariant()) {
        throw 'H3 package receipt does not expose the exact pack and adapter identities.'
    }

    $expanded = Join-Path $workRoot 'expanded'
    Expand-SafeCodecPackArchive -ArchivePath $archivePath -DestinationPath $expanded
    $archiveSmokePath = Join-Path $stagedVersion 'archive-runtime-smoke.json'
    & (Join-Path $PSScriptRoot 'Test-H3CodecPackRuntime.ps1') `
        -PackRoot $expanded `
        -ReceiptPath $archiveSmokePath `
        -RequireCuda:$RequireCuda | Out-Null
    Remove-SafeTemporaryDirectory `
        -ParentPath $workRoot `
        -CandidatePath $expanded `
        -RequiredLeafPrefix 'expanded'

    $archiveHash = (
        Get-FileHash -LiteralPath $archivePath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $setupBuildParameters = @{
        ArchivePath = $archivePath
        PackVersion = $PackVersion
        OutputDirectory = $stagedVersion
        AllowNetwork = $AllowNetwork.IsPresent
    }
    if (-not [string]::IsNullOrWhiteSpace($SigningCommand)) {
        $setupBuildParameters.SigningCommand = $SigningCommand
    }
    $setupOutput = @(
        & (Join-Path $PSScriptRoot 'Build-H3CodecPackInstaller.ps1') `
            @setupBuildParameters
    )
    if ($setupOutput.Count -ne 1 -or
        -not (Test-Path -LiteralPath ([string]$setupOutput[0]) -PathType Leaf)) {
        throw 'Codec Pack setup builder did not return exactly one setup path.'
    }
    $setupPath = (Resolve-Path -LiteralPath ([string]$setupOutput[0])).Path
    $helperPath = Join-Path `
        $artifactsRoot `
        'codec-pack-installer-target/x86_64-pc-windows-msvc/release/latentdeck-codec-pack-installer.exe'
    if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
        throw 'Codec Pack setup builder did not retain its native lifecycle helper.'
    }

    $isolatedLocalAppData = Join-Path $workRoot 'installer-local-app-data'
    $isolatedProgramData = Join-Path $workRoot 'installer-program-data'
    $savedLocalAppData = $env:LOCALAPPDATA
    $savedProgramData = $env:PROGRAMDATA
    try {
        $env:LOCALAPPDATA = $isolatedLocalAppData
        $env:PROGRAMDATA = $isolatedProgramData
        Invoke-Checked `
            -Executable $helperPath `
            -Context 'native setup-helper install' `
            -SuppressOutput `
            -Arguments @(
            '--local-app-data', $isolatedLocalAppData,
            '--program-data', $isolatedProgramData,
            'install',
            '--archive', $archivePath
        )
        $installedVersion = Join-Path `
            $isolatedLocalAppData `
            "LatentDeck/CodecPacks/org.latentdeck.h3/$PackVersion"
        $installedSmokePath = Join-Path $stagedVersion 'installed-runtime-smoke.json'
        & (Join-Path $PSScriptRoot 'Test-H3CodecPackRuntime.ps1') `
            -PackRoot $installedVersion `
            -ReceiptPath $installedSmokePath `
            -RequireCuda:$RequireCuda | Out-Null
        Invoke-Checked `
            -Executable $helperPath `
            -Context 'native setup-helper uninstall' `
            -SuppressOutput `
            -Arguments @(
            '--local-app-data', $isolatedLocalAppData,
            '--program-data', $isolatedProgramData,
            'uninstall', '--version', $PackVersion
        )
        if (Test-Path -LiteralPath $installedVersion) {
            throw 'Native setup-helper uninstall left the installed version behind.'
        }
    } finally {
        $env:LOCALAPPDATA = $savedLocalAppData
        $env:PROGRAMDATA = $savedProgramData
    }

    $setupReceiptPath = Join-Path $stagedVersion 'setup-receipt.json'
    $setupReceipt = Get-Content -Raw -LiteralPath $setupReceiptPath |
        ConvertFrom-Json -Depth 100
    if ($setupReceipt.native_helper_lifecycle_smoke -cne 'pending' -or
        $setupReceipt.windows_setup_lifecycle -cne 'not_run_clean_machine_gate' -or
        $setupReceipt.setup.name -cne (Split-Path -Leaf $setupPath) -or
        $setupReceipt.payload.sha256 -cne $archiveHash -or
        [string]$setupReceipt.source.commit -cne [string]$sourceBefore.Commit -or
        [string]$setupReceipt.source.branch -cne [string]$sourceBefore.Branch -or
        [bool]$setupReceipt.source.git_dirty -ne [bool]$sourceBefore.Dirty -or
        [string]$setupReceipt.source.git_tree -cne [string]$sourceBefore.Tree -or
        [string]$setupReceipt.source.public_snapshot_sha256 -cne
            [string]$sourceBefore.PublicSnapshotSha256 -or
        [int64]$setupReceipt.source.public_snapshot_file_count -ne
            [int64]$sourceBefore.PublicSnapshotFileCount) {
        throw 'Codec Pack setup receipt does not match the completed native-helper smoke metadata.'
    }
    if ($ReleaseChannel -ceq 'unsigned_preview' -and
        $setupReceipt.signing.mode -cne 'unsigned_local_rc') {
        throw 'Unsigned preview H3 setup unexpectedly crossed the signing boundary.'
    }
    if ($ReleaseChannel -ceq 'stable' -and
        ($setupReceipt.signing.mode -cne 'authenticode_finalize_command' -or
         $setupReceipt.signing.outer_setup_authenticode -cne 'valid')) {
        throw 'Stable H3 setup did not produce valid Authenticode evidence.'
    }
    $setupReceipt.native_helper_lifecycle_smoke = 'passed'
    Write-JsonFile -Value $setupReceipt -Path $setupReceiptPath
    $setupReceiptHash = (
        Get-FileHash -LiteralPath $setupReceiptPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $setupSumsPath = Join-Path $stagedVersion 'SHA256SUMS.txt'
    $setupSumsItem = Get-Item -LiteralPath $setupSumsPath -Force
    if ($setupSumsItem.PSIsContainer -or
        ($setupSumsItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $setupSumsItem.Length -eq 0 -or $setupSumsItem.Length -gt 1MB) {
        throw 'Codec Pack checksum manifest is not a bounded regular file.'
    }
    $setupSumLines = @(Get-Content -LiteralPath $setupSumsPath)
    if (@($setupSumLines | Where-Object { $_.Length -gt 4096 }).Count -gt 0) {
        throw 'Codec Pack checksum manifest contains an overlong line.'
    }
    $receiptSumIndexes = @(
        for ($index = 0; $index -lt $setupSumLines.Count; $index += 1) {
            if ($setupSumLines[$index] -match '  setup-receipt\.json$') {
                $index
            }
        }
    )
    if ($receiptSumIndexes.Count -ne 1) {
        throw 'Codec Pack checksum manifest does not contain one setup receipt entry.'
    }
    $setupSumLines[$receiptSumIndexes[0]] = "$setupReceiptHash  setup-receipt.json"
    [System.IO.File]::WriteAllText(
        $setupSumsPath,
        ($setupSumLines -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    $sidecars = [ordered]@{}
    foreach ($sidecarName in @(
        'archive-runtime-smoke.json',
        'installed-runtime-smoke.json',
        'package-receipt.json',
        'setup-receipt.json'
    )) {
        $sidecarPath = Join-Path $stagedVersion $sidecarName
        $sidecarItem = Get-Item -LiteralPath $sidecarPath -Force
        if ($sidecarItem.PSIsContainer -or
            ($sidecarItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            $sidecarItem.Length -eq 0 -or $sidecarItem.Length -gt 16MB) {
            throw "Codec Pack sidecar must be a bounded regular file: $sidecarName"
        }
        $sidecars[$sidecarName] = [ordered]@{
            name = $sidecarName
            byte_length = [int64]$sidecarItem.Length
            sha256 = (Get-FileHash -LiteralPath $sidecarItem.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }

    $sourceAfter = Get-PackagingSourceState -RepositoryRoot $repoRoot
    Assert-PackagingSourceStateUnchanged `
        -Before $sourceBefore `
        -After $sourceAfter `
        -Context 'H3 Codec Pack source'

    Write-JsonFile -Value ([ordered]@{
        schema_version = 2
        release_label = $ReleaseLabel
        release_channel = $ReleaseChannel
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        adapter_version = [string]$packageReceipt.adapter_version
        distributable = $distributable
        signed = ($ReleaseChannel -ceq 'stable')
        unsigned = ($ReleaseChannel -ceq 'unsigned_preview')
        platform = 'windows-x86_64'
        archive = [ordered]@{
            name = Split-Path -Leaf $archivePath
            byte_length = [int64](Get-Item -LiteralPath $archivePath).Length
            sha256 = $archiveHash
        }
        setup = [ordered]@{
            name = Split-Path -Leaf $setupPath
            byte_length = [int64](Get-Item -LiteralPath $setupPath).Length
            sha256 = [string]$setupReceipt.setup.sha256
            payload_delivery = 'adjacent_hash_bound_ldcodec'
            native_helper_lifecycle_smoke = 'passed'
            windows_setup_lifecycle = [string]$setupReceipt.windows_setup_lifecycle
        }
        cpython = [ordered]@{
            version = [string]$lock.python_runtime.version
            archive_sha256 = $runtimeArchiveHash
        }
        runtime_wheel_lock = [ordered]@{
            name = 'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
            sha256 = $curationLockSha256
            install_policy = 'direct_https_wheels_only_sha256_required'
            wheel_count = @($lock.dependencies).Count
            wheels = @(
                $lock.dependencies |
                    Sort-Object { [string]$_.name } -CaseSensitive |
                    ForEach-Object {
                        [ordered]@{
                            name = [string]$_.name
                            version = [string]$_.version
                            file_name = [string]$_.wheel.file_name
                            url = [string]$_.wheel.url
                            byte_length = [int64]$_.wheel.byte_length
                            sha256 = [string]$_.wheel.sha256
                        }
                    }
            )
        }
        dependency_inventory = 'DEPENDENCY_INVENTORY.json'
        sbom = 'SBOM.cdx.json'
        safetensors_native_closure = $safetensorsNativeEvidence
        installer_license_review = $setupReceipt.sbom.license_review
        archive_runtime_smoke = 'passed'
        isolated_native_install_smoke = 'passed'
        isolated_native_uninstall = 'passed'
        cuda_required = $RequireCuda.IsPresent
        contains_model_weights = $false
        contains_generator = $false
        contains_comfy = $false
        source = [ordered]@{
            commit = [string]$sourceBefore.Commit
            branch = [string]$sourceBefore.Branch
            git_dirty = [bool]$sourceBefore.Dirty
            git_dirty_entry_count = [int64]$sourceBefore.DirtyEntryCount
            git_tree = [string]$sourceBefore.Tree
            public_snapshot_sha256 = [string]$sourceBefore.PublicSnapshotSha256
            public_snapshot_file_count = [int64]$sourceBefore.PublicSnapshotFileCount
        }
        sidecars = $sidecars
        signing = $setupReceipt.signing
        publisher_signature = [string]$setupReceipt.publisher_signature
    }) -Path (Join-Path $stagedVersion 'distributable-proof.json')

    [System.IO.Directory]::CreateDirectory($outputRoot) | Out-Null
    if (Test-Path -LiteralPath $finalDirectory) {
        throw "Codec Pack artifact destination appeared during build: $finalDirectory"
    }
    [System.IO.Directory]::Move($stagedVersion, $finalDirectory)
    Write-Output (Join-Path $finalDirectory (Split-Path -Leaf $archivePath))
} finally {
    Remove-SafeTemporaryDirectory `
        -ParentPath $artifactsRoot `
        -CandidatePath $workRoot `
        -RequiredLeafPrefix '.h3-codec-pack-'
}
