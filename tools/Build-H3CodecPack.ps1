[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PythonEmbedArchive,

    [string]$DecoderAssetContractPath,

    [string]$PackVersion = '0.1.0',

    [string]$OutputDirectory,

    [switch]$AllowNetwork,

    [switch]$RequireCuda
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
$curationLock = Join-Path `
    $repoRoot `
    'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
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

$lock = Read-StrictJsonFile -Path $curationLock
if ($lock.uv_version -cne '0.11.8' -or $lock.platform -cne 'windows-x86_64') {
    throw 'Codec Pack curation lock does not match the pinned builder platform.'
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
    $torchWheel = (
        'https://download-r2.pytorch.org/whl/cu130/' +
        'torch-2.13.0%2Bcu130-cp313-cp313-win_amd64.whl'
    )
    $requirementLines = @(
        foreach ($dependency in @($lock.dependencies)) {
            if ([string]$dependency.name -ceq 'torch') {
                "torch @ $torchWheel"
            } else {
                "$($dependency.name)==$($dependency.version)"
            }
        }
    )
    [System.IO.File]::WriteAllText(
        $requirementsPath,
        (($requirementLines | Sort-Object) -join "`n") + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )

    $sitePackages = Join-Path $workRoot 'site-packages'
    [System.IO.Directory]::CreateDirectory($sitePackages) | Out-Null
    $installArguments = @(
        'pip', 'install', '--target', $sitePackages, '--python-version', '3.13',
        '--python-platform', 'windows', '--link-mode', 'copy', '--no-deps',
        '--requirement', $requirementsPath
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
    $localProjects = @(
        'latentdeck-codec-host',
        'latentdeck-operator-d2',
        'latentdeck-operator-q4',
        'latentdeck-rgb-ring',
        'latentdeck-codec-h3'
    )
    foreach ($project in $localProjects) {
        $buildArguments = @(
            'build', '--wheel', '--package', $project, '--out-dir', $wheels
        )
        if (-not $AllowNetwork) {
            $buildArguments += '--offline'
        }
        Invoke-Checked `
            -Executable $uv.Source `
            -Arguments $buildArguments `
            -Context "local wheel build for $project"
    }
    $wheelPaths = @(Get-ChildItem -LiteralPath $wheels -Filter '*.whl' -File | Sort-Object Name)
    if ($wheelPaths.Count -ne $localProjects.Count) {
        throw 'Local Codec Pack wheel build did not produce the exact expected wheel count.'
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

    $metadata = Join-Path $workRoot 'metadata'
    Invoke-Checked -Executable $buildPython -Context 'Codec Pack dependency curation' -Arguments @(
        '-I', '-s', '-B', '-X', 'utf8', $curator, 'curate',
        '--lock', $curationLock,
        '--site-packages', $sitePackages,
        '--base-notice', $baseNotice,
        '--metadata-output', $metadata,
        '--pack-version', $PackVersion
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

    $expanded = Join-Path $workRoot 'expanded'
    Expand-SafeCodecPackArchive -ArchivePath $archivePath -DestinationPath $expanded
    $archiveSmokePath = Join-Path $stagedVersion 'archive-runtime-smoke.json'
    & (Join-Path $PSScriptRoot 'Test-H3CodecPackRuntime.ps1') `
        -PackRoot $expanded `
        -ReceiptPath $archiveSmokePath `
        -RequireCuda:$RequireCuda | Out-Null

    $installRoot = Join-Path $workRoot 'installed'
    $archiveHash = (
        Get-FileHash -LiteralPath $archivePath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    & (Join-Path $PSScriptRoot 'Install-H3CodecPack.ps1') `
        -ArchivePath $archivePath `
        -TrustedArchiveSha256 $archiveHash `
        -InstallRoot $installRoot | Out-Null
    $installedVersion = Join-Path $installRoot "org.latentdeck.h3/$PackVersion"
    $installedSmokePath = Join-Path $stagedVersion 'installed-runtime-smoke.json'
    & (Join-Path $PSScriptRoot 'Test-H3CodecPackRuntime.ps1') `
        -PackRoot $installedVersion `
        -ReceiptPath $installedSmokePath `
        -RequireCuda:$RequireCuda | Out-Null
    & (Join-Path $PSScriptRoot 'Uninstall-H3CodecPack.ps1') `
        -PackVersion $PackVersion `
        -InstallRoot $installRoot | Out-Null
    if (Test-Path -LiteralPath $installedVersion) {
        throw 'Isolated Codec Pack uninstall left the installed version behind.'
    }

    Write-JsonFile -Value ([ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $PackVersion
        platform = 'windows-x86_64'
        archive = [ordered]@{
            name = Split-Path -Leaf $archivePath
            byte_length = [int64](Get-Item -LiteralPath $archivePath).Length
            sha256 = $archiveHash
        }
        cpython = [ordered]@{
            version = [string]$lock.python_runtime.version
            archive_sha256 = $runtimeArchiveHash
        }
        dependency_inventory = 'DEPENDENCY_INVENTORY.json'
        sbom = 'SBOM.cdx.json'
        archive_runtime_smoke = 'passed'
        isolated_install_smoke = 'passed'
        isolated_uninstall = 'passed'
        cuda_required = $RequireCuda.IsPresent
        contains_model_weights = $false
        contains_generator = $false
        contains_comfy = $false
        publisher_signature = 'not_present_local_rc'
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
