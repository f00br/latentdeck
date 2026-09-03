[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PythonEmbedArchive,

    [string]$DecoderAssetContractPath,

    [Parameter(Mandatory)]
    [string]$PackVersion,

    [string]$OutputDirectory,

    [switch]$AllowNetwork,

    [switch]$RequireCuda,

    [string]$SigningCommand
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
    $localProjects = @($lock.local_projects | ForEach-Object { [string]$_.name })
    if ($localProjects.Count -eq 0 -or
        $localProjects.Count -ne (@($localProjects | Select-Object -Unique)).Count) {
        throw 'Codec Pack curation lock local_projects must be a non-empty unique set.'
    }

    $savedRustFlags = $env:RUSTFLAGS
    $savedEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
    try {
        $env:RUSTFLAGS = '-C link-arg=/Brepro'
        if (Test-Path -LiteralPath 'Env:CARGO_ENCODED_RUSTFLAGS') {
            Remove-Item -LiteralPath 'Env:CARGO_ENCODED_RUSTFLAGS' -ErrorAction Stop
        }

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
    } finally {
        $env:RUSTFLAGS = $savedRustFlags
        $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustFlags
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
        $setupReceipt.payload.sha256 -cne $archiveHash) {
        throw 'Codec Pack setup receipt does not match the completed native-helper smoke metadata.'
    }
    $setupReceipt.native_helper_lifecycle_smoke = 'passed'
    Write-JsonFile -Value $setupReceipt -Path $setupReceiptPath
    $setupReceiptHash = (
        Get-FileHash -LiteralPath $setupReceiptPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
    $setupSumsPath = Join-Path $stagedVersion 'SHA256SUMS.txt'
    $setupSumLines = @(Get-Content -LiteralPath $setupSumsPath)
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
        setup = [ordered]@{
            name = Split-Path -Leaf $setupPath
            byte_length = [int64](Get-Item -LiteralPath $setupPath).Length
            sha256 = [string]$setupReceipt.setup.sha256
            payload_delivery = 'adjacent_hash_bound_zip'
            native_helper_lifecycle_smoke = 'passed'
            windows_setup_lifecycle = [string]$setupReceipt.windows_setup_lifecycle
        }
        cpython = [ordered]@{
            version = [string]$lock.python_runtime.version
            archive_sha256 = $runtimeArchiveHash
        }
        dependency_inventory = 'DEPENDENCY_INVENTORY.json'
        sbom = 'SBOM.cdx.json'
        archive_runtime_smoke = 'passed'
        isolated_native_install_smoke = 'passed'
        isolated_native_uninstall = 'passed'
        cuda_required = $RequireCuda.IsPresent
        contains_model_weights = $false
        contains_generator = $false
        contains_comfy = $false
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
