[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'ReleaseLicenseBundle.psm1') -Force
Import-Module (Join-Path $PSScriptRoot 'SafetensorsNativeClosure.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".github-stage-contract-$([guid]::NewGuid().ToString('N'))"
$releaseLabel = '0.1.0-preview.1'
$releaseChannel = 'unsigned_preview'
$commit = 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
$tree = 'e' * 40
$publicSnapshot = 'f' * 64
$publicSnapshotFileCount = 1
$packVersion = '0.2.1'

function Write-Text {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Text
    )

    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $Path)) | Out-Null
    [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

function Write-Json {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][object]$Value
    )

    Write-Text -Path $Path -Text (($Value | ConvertTo-Json -Depth 100) + "`n")
}

function Get-Receipt {
    param([Parameter(Mandatory)][string]$Path)

    $item = Get-Item -LiteralPath $Path
    return [ordered]@{
        name = $item.Name
        file_name = $item.Name
        byte_length = [int64]$item.Length
        sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Get-FileBinding {
    param([Parameter(Mandatory)][string]$Path)

    $record = Get-Receipt -Path $Path
    return [ordered]@{
        file_name = $record.file_name
        byte_length = $record.byte_length
        sha256 = $record.sha256
    }
}

function Write-Sums {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string[]]$Paths
    )

    $lines = @(
        foreach ($relative in $Paths) {
            $path = Join-Path $Root $relative.Replace('/', '\')
            $hash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
            "$hash  $relative"
        }
    )
    Write-Text -Path (Join-Path $Root 'SHA256SUMS.txt') -Text (($lines -join "`n") + "`n")
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
            throw "Unexpected failure: $($_.Exception.Message)`n$($_.ScriptStackTrace)"
        }
        return
    }
    throw "Expected failure containing '$ExpectedText'."
}

function Write-FixtureSbom {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$ArtifactName,
        [Parameter(Mandatory)][string]$ArtifactVersion,
        [Parameter(Mandatory)][string]$ArtifactScope
    )

    Write-Json -Path $Path -Value ([ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        serialNumber = 'urn:uuid:00000000-0000-0000-0000-000000000001'
        version = 1
        metadata = [ordered]@{
            component = [ordered]@{
                type = 'application'
                'bom-ref' = "pkg:generic/$($ArtifactName.Replace(' ', '%20'))@$ArtifactVersion"
                name = $ArtifactName
                version = $ArtifactVersion
                licenses = @([ordered]@{ license = [ordered]@{ name = 'Apache-2.0' } })
                properties = @(
                    [ordered]@{ name = 'latentdeck:artifact-scope'; value = $ArtifactScope }
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                )
            }
        }
        components = @()
    })
}

function Write-ZipWithFile {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$EntryName,
        [Parameter(Mandatory)][string]$SourcePath
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $stream = [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::Write,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $stream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $true,
            [System.Text.Encoding]::UTF8
        )
        try {
            $entry = $archive.CreateEntry(
                $EntryName,
                [System.IO.Compression.CompressionLevel]::Optimal
            )
            $entry.LastWriteTime = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            $input = [System.IO.File]::OpenRead($SourcePath)
            $output = $entry.Open()
            try {
                $input.CopyTo($output)
            } finally {
                $output.Dispose()
                $input.Dispose()
            }
        } finally {
            $archive.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

try {
    $appRoot = Join-Path $testRoot 'app'
    $codecRoot = Join-Path $testRoot 'codec'
    $developerRoot = Join-Path $testRoot 'developer'
    $recorderRoot = Join-Path $testRoot 'recorder'
    foreach ($root in @($appRoot, $codecRoot, $developerRoot, $recorderRoot)) {
        [System.IO.Directory]::CreateDirectory($root) | Out-Null
    }

    $appNames = @(
        "LatentDeck-$releaseLabel-windows-x64-unsigned-setup.exe",
        "LatentPlayer-$releaseLabel-windows-x64-unsigned-setup.exe"
    )
    foreach ($name in $appNames) {
        Write-Text -Path (Join-Path $appRoot "installers/$name") -Text "fixture-$name"
    }
    $appSbomPaths = @(
        "metadata/LatentDeck-App-$releaseLabel-sbom.cdx.json",
        "metadata/LatentPlayer-$releaseLabel-sbom.cdx.json"
    )
    foreach ($path in $appSbomPaths) {
        $artifactName = if ($path -like '*LatentDeck-App*') { 'LatentDeck App' } else { 'LatentPlayer' }
        $cargoPackage = if ($artifactName -ceq 'LatentDeck App') {
            'latentdeck-app'
        } else {
            'latentplayer-app'
        }
        $nodePackage = if ($artifactName -ceq 'LatentDeck App') {
            '@latentdeck/app'
        } else {
            '@latentdeck/player'
        }
        & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
            -OutputPath (Join-Path $appRoot $path) `
            -ArtifactName $artifactName `
            -ArtifactVersion '0.1.0+1' `
            -ArtifactScope application `
            -CargoPackage $cargoPackage `
            -NodePackage $nodePackage `
            -NodeBuildPackage @(
                '@sveltejs/vite-plugin-svelte', '@tailwindcss/vite', '@tauri-apps/cli',
                'svelte', 'tailwindcss', 'typescript', 'vite', 'vitest'
            ) `
            -NodeRuntimeBuildPackage @('svelte', 'tailwindcss', 'vite') `
            -IncludeSpout2 `
            -IncludeTauriWindowsInstaller `
            -Deterministic | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "Could not generate the $artifactName staging fixture SBOM."
        }
    }
    Write-Text -Path (Join-Path $appRoot 'metadata/THIRD_PARTY_NOTICES.md') -Text "fixture notices`n"
    Write-Text -Path (Join-Path $appRoot 'BUILD-COMMANDS.txt') -Text "fixture build`n"
    $appLicenseBundle = New-ReleaseLicenseBundle `
        -SbomPath @($appSbomPaths | ForEach-Object { Join-Path $appRoot $_ }) `
        -ArtifactName 'LatentDeck Windows Applications' `
        -ArtifactVersion $releaseLabel `
        -OutputPath (Join-Path $appRoot 'metadata/THIRD_PARTY_LICENSES.json') `
        -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md')
    $applications = @(
        foreach ($name in $appNames) {
            $receipt = Get-Receipt -Path (Join-Path $appRoot "installers/$name")
            [ordered]@{
                product = if ($name.StartsWith('LatentDeck-')) { 'LatentDeck App' } else { 'LatentPlayer' }
                file_name = $receipt.file_name
                byte_length = $receipt.byte_length
                sha256 = $receipt.sha256
                license_review = [ordered]@{ status = 'complete'; missing_license_component_count = 0 }
                unsigned = $true
                authenticode = 'not_present'
            }
        }
    )
    $appSboms = @(
        foreach ($path in $appSbomPaths) {
            $receipt = Get-Receipt -Path (Join-Path $appRoot $path)
            [ordered]@{
                product = if ($path -like '*LatentDeck-App*') { 'LatentDeck App' } else { 'LatentPlayer' }
                file_name = $path
                byte_length = $receipt.byte_length
                sha256 = $receipt.sha256
                license_review = [ordered]@{ status = 'complete'; missing_license_component_count = 0 }
            }
        }
    )
    $appReceipt = [ordered]@{
        schema_version = 6
        release_label = $releaseLabel
        release_channel = $releaseChannel
        application_api_version = '0.1.0'
        windows_installer_version = '0.1.0+1'
        component_versions = [ordered]@{
            decks = [ordered]@{
                d2 = [ordered]@{ deck_id = 'org.latentdeck.deck.d2'; deck_version = '0.2.1' }
                q4 = [ordered]@{ deck_id = 'org.latentdeck.deck.q4'; deck_version = '0.2.1' }
            }
            sdks = [ordered]@{ cartridge = '0.1.0'; deck = '0.2.0'; codec = '0.2.0' }
        }
        signed = $false
        unsigned = $true
        distributable = $true
        license_review = [ordered]@{ status = 'complete'; missing_license_component_count = 0 }
        source = [ordered]@{
            git_commit = $commit
            git_branch = 'main'
            git_tree = $tree
            git_dirty = $false
            git_dirty_entry_count = 0
            public_snapshot_sha256 = $publicSnapshot
            public_snapshot_file_count = $publicSnapshotFileCount
        }
        applications = $applications
        sboms = $appSboms
        license_bundle = [ordered]@{
            file_name = 'metadata/THIRD_PARTY_LICENSES.json'
            byte_length = $appLicenseBundle.ByteLength
            sha256 = $appLicenseBundle.Sha256
            schema_version = 1
            component_count = $appLicenseBundle.ComponentCount
            text_count = $appLicenseBundle.TextCount
            build_only_no_text_disposition_count = $appLicenseBundle.NoTextDispositionCount
        }
    }
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Write-Sums -Root $appRoot -Paths @(
        "installers/$($appNames[0])",
        "installers/$($appNames[1])",
        $appSbomPaths[0],
        $appSbomPaths[1],
        'metadata/THIRD_PARTY_NOTICES.md',
        'metadata/THIRD_PARTY_LICENSES.json'
    )

    $archiveName = "LatentDeck-H3-CodecPack-$packVersion-windows-x64.ldcodec"
    $setupName = "LatentDeck-H3-CodecPack-$packVersion-setup.exe"
    $recorderLock = Get-Content -LiteralPath (
        Join-Path $repositoryRoot 'comfy/latent-cartridge/packaging/windows-x64.lock.json'
    ) -Raw | ConvertFrom-Json -Depth 32
    $safetensorsWheelPath = Join-Path $testRoot ([string]$recorderLock.safetensors.file_name)
    Invoke-WebRequest -Uri ([string]$recorderLock.safetensors.url) -OutFile $safetensorsWheelPath
    $nativeFixtureSbomPath = Join-Path $testRoot 'NATIVE_RUST_SBOM.cdx.json'
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath $nativeFixtureSbomPath `
        -ArtifactName 'LatentDeck H3 Native Extensions' `
        -ArtifactVersion $packVersion `
        -ArtifactScope h3-native `
        -CargoPackage @('latentdeck-cartridge-python', 'latentdeck-gpu-python') `
        -Deterministic | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not generate the H3 native staging fixture SBOM.'
    }
    $safetensorsNativeEvidence = Merge-SafetensorsNativeClosureIntoSbom `
        -SbomPath $nativeFixtureSbomPath `
        -WheelPath $safetensorsWheelPath
    Write-ZipWithFile `
        -Path (Join-Path $codecRoot $archiveName) `
        -EntryName 'NATIVE_RUST_SBOM.cdx.json' `
        -SourcePath $nativeFixtureSbomPath
    foreach ($entry in ([ordered]@{
        $setupName = 'fixture setup'
        'INSTALLER_NSIS_COPYING.txt' = 'fixture NSIS license'
        'INSTALLER_RUST_LICENSES.txt' = 'fixture Rust licenses'
        'INSTALLER_THIRD_PARTY_NOTICES.md' = 'fixture installer notices'
        'installer-SBOM.cdx.json' = ''
    }).GetEnumerator()) {
        Write-Text -Path (Join-Path $codecRoot $entry.Key) -Text ([string]$entry.Value + "`n")
    }
    $installerSbomFixture = [ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        metadata = [ordered]@{
            component = [ordered]@{
                type = 'application'
                name = 'LatentDeck H3 Codec Pack Setup'
                version = $packVersion
                licenses = @([ordered]@{ license = [ordered]@{ id = 'Apache-2.0' } })
                properties = @(
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                    [ordered]@{
                        name = 'latentdeck:included-dependency-scopes'
                        value = 'artifact,runtime,build,runtime+build'
                    }
                    [ordered]@{ name = 'latentdeck:excluded-dependency-scopes'; value = 'development' }
                    [ordered]@{ name = 'latentdeck:target-platform'; value = 'x86_64-pc-windows-msvc' }
                )
            }
        }
        components = @(
            [ordered]@{
                type = 'application'
                'bom-ref' = 'rust:latentdeck-codec-pack-installer@0.1.0'
                name = 'latentdeck-codec-pack-installer'
                version = '0.1.0'
                licenses = @([ordered]@{ license = [ordered]@{ id = 'Apache-2.0' } })
                properties = @(
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'artifact' }
                )
            },
            [ordered]@{
                type = 'application'
                'bom-ref' = 'tool:nsis@3.11'
                name = 'Nullsoft Scriptable Install System'
                version = '3.11'
                licenses = @([ordered]@{ license = [ordered]@{ name = 'NSIS bundled licenses' } })
                properties = @(
                    [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'build' }
                )
            }
        )
    }
    Write-Json -Path (Join-Path $codecRoot 'installer-SBOM.cdx.json') -Value $installerSbomFixture
    $archiveReceipt = Get-Receipt -Path (Join-Path $codecRoot $archiveName)
    $setupReceipt = Get-Receipt -Path (Join-Path $codecRoot $setupName)
    $installerSbomReceipt = Get-Receipt -Path (Join-Path $codecRoot 'installer-SBOM.cdx.json')
    $installerNoticeReceipt = Get-Receipt -Path (Join-Path $codecRoot 'INSTALLER_THIRD_PARTY_NOTICES.md')
    $nsisNoticeReceipt = Get-Receipt -Path (Join-Path $codecRoot 'INSTALLER_NSIS_COPYING.txt')
    $rustNoticeReceipt = Get-Receipt -Path (Join-Path $codecRoot 'INSTALLER_RUST_LICENSES.txt')
    $runtimeSmoke = [ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $packVersion
        platform = 'windows-x86_64'
        runtime = [ordered]@{
            protocol = [ordered]@{
                selected_version = 2
                worker_protocol = 2
                commands = @('session.configure', 'codec.descriptor')
            }
            rgb_ring_abi = [ordered]@{ protocol2 = '2' }
            preload_guards = [ordered]@{ torch_imported = $false; external_decoder_accesses = 0 }
        }
        contains_model_weights = $false
        contains_generator = $false
        contains_comfy = $false
        external_decoder_selection_required = $true
        result = 'passed'
    }
    Write-Json -Path (Join-Path $codecRoot 'archive-runtime-smoke.json') -Value $runtimeSmoke
    Write-Json -Path (Join-Path $codecRoot 'installed-runtime-smoke.json') -Value $runtimeSmoke
    $packageReceipt = [ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $packVersion
        adapter_version = '0.2.0'
        platform = 'windows-x86_64'
        archive = [ordered]@{
            name = $archiveName
            byte_length = $archiveReceipt.byte_length
            sha256 = $archiveReceipt.sha256
        }
        contains_runtime = $true
        contains_adapter = $true
        dependency_inventory = [ordered]@{ path = 'DEPENDENCY_INVENTORY.json'; sha256 = ('b' * 64) }
        sbom = [ordered]@{ format = 'CycloneDX-1.5'; path = 'SBOM.cdx.json'; sha256 = ('c' * 64) }
        native_rust = [ordered]@{
            sbom_path = 'NATIVE_RUST_SBOM.cdx.json'
            sbom_sha256 = (Get-FileHash -LiteralPath $nativeFixtureSbomPath -Algorithm SHA256).
                Hash.ToLowerInvariant()
            license_bundle_path = 'NATIVE_RUST_LICENSES.json'
            license_bundle_sha256 = ('2' * 64)
        }
        external_decoder_selection_required = $true
        archive_digest_purpose = 'transport_integrity_only'
        publisher_signature = 'not_present_local_rc'
        content_policy = [ordered]@{
            model_weights_allowed = $false
            cartridges_allowed = $false
            forbidden_payload_scan = 'passed'
            semantic_source_review = 'passed'
        }
    }
    Write-Json -Path (Join-Path $codecRoot 'package-receipt.json') -Value $packageReceipt
    $setupSidecar = [ordered]@{
        schema_version = 1
        pack_id = 'org.latentdeck.h3'
        pack_version = $packVersion
        platform = 'windows-x86_64'
        setup = [ordered]@{
            name = $setupName
            byte_length = $setupReceipt.byte_length
            sha256 = $setupReceipt.sha256
            format = 'nsis'
            scope = 'current_user'
            payload_delivery = 'adjacent_hash_bound_ldcodec'
        }
        payload = [ordered]@{
            name = $archiveName
            byte_length = $archiveReceipt.byte_length
            sha256 = $archiveReceipt.sha256
            uncompressed_bytes = 1
        }
        helper = [ordered]@{ sha256 = ('d' * 64); static_crt = $true; delivery = 'embedded_in_setup_and_uninstaller'; installed_as_loose_file = $false }
        sbom = [ordered]@{
            name = 'installer-SBOM.cdx.json'
            byte_length = $installerSbomReceipt.byte_length
            sha256 = $installerSbomReceipt.sha256
            format = 'CycloneDX-1.5'
            component_count = 2
            license_review = 'complete'
            missing_license_component_count = 0
        }
        notices = [ordered]@{
            name = 'INSTALLER_THIRD_PARTY_NOTICES.md'
            byte_length = $installerNoticeReceipt.byte_length
            sha256 = $installerNoticeReceipt.sha256
            nsis_copying_name = 'INSTALLER_NSIS_COPYING.txt'
            nsis_copying_byte_length = $nsisNoticeReceipt.byte_length
            nsis_copying_sha256 = $nsisNoticeReceipt.sha256
            rust_licenses_name = 'INSTALLER_RUST_LICENSES.txt'
            rust_licenses_byte_length = $rustNoticeReceipt.byte_length
            rust_licenses_sha256 = $rustNoticeReceipt.sha256
        }
        toolchain = [ordered]@{ nsis_version = '3.11' }
        source = [ordered]@{
            commit = $commit
            branch = 'main'
            git_dirty = $false
            git_tree = $tree
            public_snapshot_sha256 = $publicSnapshot
            public_snapshot_file_count = $publicSnapshotFileCount
        }
        lifecycle = [ordered]@{ scope = 'current_user'; offline = $true }
        native_helper_lifecycle_smoke = 'passed'
        windows_setup_lifecycle = 'not_run_clean_machine_gate'
        signing = [ordered]@{
            mode = 'unsigned_local_rc'
            outer_setup_authenticode = 'not_present'
            embedded_uninstaller_finalize = 'not_requested'
            installed_uninstaller_authenticode = 'not_run_clean_machine_gate'
        }
        publisher_signature = 'not_present_local_rc'
    }
    Write-Json -Path (Join-Path $codecRoot 'setup-receipt.json') -Value $setupSidecar
    $sidecars = [ordered]@{}
    foreach ($name in @(
        'archive-runtime-smoke.json',
        'installed-runtime-smoke.json',
        'package-receipt.json',
        'setup-receipt.json'
    )) {
        $record = Get-Receipt -Path (Join-Path $codecRoot $name)
        $sidecars[$name] = [ordered]@{
            name = $name
            byte_length = $record.byte_length
            sha256 = $record.sha256
        }
    }
    $h3RuntimeLockPath = Join-Path `
        $repositoryRoot `
        'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
    $h3RuntimeLock = Get-Content -LiteralPath $h3RuntimeLockPath -Raw |
        ConvertFrom-Json -Depth 100
    $h3RuntimeWheels = @(
        $h3RuntimeLock.dependencies |
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
    $codecReceipt = [ordered]@{
        schema_version = 2
        release_label = $releaseLabel
        release_channel = $releaseChannel
        pack_id = 'org.latentdeck.h3'
        pack_version = $packVersion
        adapter_version = '0.2.0'
        distributable = $true
        signed = $false
        unsigned = $true
        platform = 'windows-x86_64'
        source = [ordered]@{
            commit = $setupSidecar.source.commit
            branch = $setupSidecar.source.branch
            git_dirty = $setupSidecar.source.git_dirty
            git_dirty_entry_count = 0
            git_tree = $setupSidecar.source.git_tree
            public_snapshot_sha256 = $setupSidecar.source.public_snapshot_sha256
            public_snapshot_file_count = $setupSidecar.source.public_snapshot_file_count
        }
        archive = [ordered]@{
            name = $archiveName
            byte_length = $archiveReceipt.byte_length
            sha256 = $archiveReceipt.sha256
        }
        setup = [ordered]@{
            name = $setupName
            byte_length = $setupReceipt.byte_length
            sha256 = $setupReceipt.sha256
            payload_delivery = 'adjacent_hash_bound_ldcodec'
            native_helper_lifecycle_smoke = 'passed'
            windows_setup_lifecycle = 'not_run_clean_machine_gate'
        }
        cpython = [ordered]@{
            version = [string]$h3RuntimeLock.python_runtime.version
            archive_sha256 = [string]$h3RuntimeLock.python_runtime.sha256
        }
        runtime_wheel_lock = [ordered]@{
            name = 'codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json'
            sha256 = (Get-FileHash -LiteralPath $h3RuntimeLockPath -Algorithm SHA256).
                Hash.ToLowerInvariant()
            install_policy = 'direct_https_wheels_only_sha256_required'
            wheel_count = $h3RuntimeWheels.Count
            wheels = $h3RuntimeWheels
        }
        dependency_inventory = 'DEPENDENCY_INVENTORY.json'
        sbom = 'SBOM.cdx.json'
        safetensors_native_closure = $safetensorsNativeEvidence
        sidecars = $sidecars
        installer_license_review = 'complete'
        archive_runtime_smoke = 'passed'
        isolated_native_install_smoke = 'passed'
        isolated_native_uninstall = 'passed'
        cuda_required = $false
        contains_model_weights = $false
        contains_generator = $false
        contains_comfy = $false
        signing = [ordered]@{
            mode = 'unsigned_local_rc'
            outer_setup_authenticode = 'not_present'
            embedded_uninstaller_finalize = 'not_requested'
            installed_uninstaller_authenticode = 'not_run_clean_machine_gate'
        }
        publisher_signature = 'not_present_local_rc'
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    $codecChecksumPaths = @(
        $archiveName,
        $setupName,
        'installer-SBOM.cdx.json',
        'INSTALLER_THIRD_PARTY_NOTICES.md',
        'INSTALLER_NSIS_COPYING.txt',
        'INSTALLER_RUST_LICENSES.txt',
        'setup-receipt.json'
    )
    Write-Sums -Root $codecRoot -Paths $codecChecksumPaths

    $recorderBaseName = "LatentDeck-$releaseLabel-comfy-recorder-windows-x64"
    $recorderArchiveName = "$recorderBaseName.zip"
    Write-Text -Path (Join-Path $recorderRoot $recorderArchiveName) -Text "fixture recorder archive`n"
    $recorderSbomPath = Join-Path $recorderRoot "$recorderBaseName-sbom.cdx.json"
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath $recorderSbomPath `
        -ArtifactName 'LatentDeck Comfy LC Recorder' `
        -ArtifactVersion $releaseLabel `
        -ArtifactScope comfy-recorder `
        -CargoPackage @('latentdeck-cartridge', 'latentdeck-cartridge-python') `
        -PythonPackage @(
            'latentdeck-cartridge', 'latentdeck-comfy-cartridge', 'safetensors'
        ) `
        -PythonBuildPackage @('maturin==1.15.0', 'uv-build==0.12.7') `
        -Deterministic | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not generate the Recorder staging fixture SBOM.'
    }
    $recorderNativeEvidence = Merge-SafetensorsNativeClosureIntoSbom `
        -SbomPath $recorderSbomPath `
        -WheelPath $safetensorsWheelPath
    $recorderSbom = Get-Content -LiteralPath $recorderSbomPath -Raw |
        ConvertFrom-Json -Depth 100
    $recorderRoots = @(
        foreach ($component in @($recorderSbom.components | Where-Object {
            @($_.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:selection-root' -and
                [string]$_.value -ceq 'true'
            }).Count -eq 1
        })) {
            $ecosystem = @($component.properties | Where-Object {
                [string]$_.name -ceq 'latentdeck:ecosystem'
            })
            "$([string]$ecosystem[0].value):$($component.name)@$($component.version)"
        }
    ) | Sort-Object
    $recorderScopeCounts = [ordered]@{
        artifact = 0
        runtime = 0
        build = 0
        'runtime+build' = 0
    }
    foreach ($component in @($recorderSbom.metadata.component) + @($recorderSbom.components)) {
        $scope = @($component.properties | Where-Object {
            [string]$_.name -ceq 'latentdeck:dependency-scope'
        })
        $scopeName = [string]$scope[0].value
        $recorderScopeCounts[$scopeName] = [int]$recorderScopeCounts[$scopeName] + 1
    }
    $recorderNoticePath = Join-Path $recorderRoot "$recorderBaseName-THIRD-PARTY-NOTICES.md"
    Write-Text -Path $recorderNoticePath -Text "# Recorder fixture notices`n"
    $recorderLicenseBundlePath = Join-Path `
        $recorderRoot `
        "$recorderBaseName-THIRD-PARTY-LICENSES.json"
    $recorderLicenseBundle = New-ReleaseLicenseBundle `
        -SbomPath $recorderSbomPath `
        -ArtifactName 'LatentDeck Comfy LC Recorder' `
        -ArtifactVersion $releaseLabel `
        -OutputPath $recorderLicenseBundlePath `
        -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md') `
        -SafetensorsWheelPath $safetensorsWheelPath
    $recorderLicenseReviewPath = Join-Path $recorderRoot "$recorderBaseName-license-review.json"
    Write-Json -Path $recorderLicenseReviewPath -Value ([ordered]@{
        schema_version = 1
        status = 'complete'
        policy = 'fixture'
        component_count = @($recorderSbom.components).Count
        root_component_reviewed = $true
        dependency_scope_counts = $recorderScopeCounts
        selection_root_count = $recorderRoots.Count
        expected_selection_root_count = 7
        selection_roots = $recorderRoots
        missing_license_component_count = 0
        missing_license_components = @()
        license_bundle = [ordered]@{
            schema_version = 1
            component_count = $recorderLicenseBundle.ComponentCount
            text_count = $recorderLicenseBundle.TextCount
            build_only_no_text_disposition_count = $recorderLicenseBundle.NoTextDispositionCount
            redistributed_component_text_coverage = 'complete'
        }
    })
    $recorderArchiveBinding = Get-FileBinding -Path (
        Join-Path $recorderRoot $recorderArchiveName
    )
    $recorderSbomBinding = Get-FileBinding -Path $recorderSbomPath
    $recorderSbomBinding['safetensors_native_closure'] = $recorderNativeEvidence
    $recorderSbomBinding['format'] = 'CycloneDX-1.5'
    $recorderSbomBinding['component_count'] = @($recorderSbom.components).Count
    $recorderSbomBinding['selection_root_count'] = $recorderRoots.Count
    $recorderSbomBinding['selection_roots'] = $recorderRoots
    $recorderSbomBinding['dependency_scope_counts'] = $recorderScopeCounts
    $recorderNoticeBinding = Get-FileBinding -Path $recorderNoticePath
    $recorderLicenseBundleBinding = Get-FileBinding -Path $recorderLicenseBundlePath
    $recorderLicenseBundleBinding['schema_version'] = 1
    $recorderLicenseBundleBinding['component_count'] = $recorderLicenseBundle.ComponentCount
    $recorderLicenseBundleBinding['text_count'] = $recorderLicenseBundle.TextCount
    $recorderLicenseBundleBinding['build_only_no_text_disposition_count'] = `
        $recorderLicenseBundle.NoTextDispositionCount
    $recorderLicenseReviewBinding = Get-FileBinding -Path $recorderLicenseReviewPath
    $recorderLicenseReviewBinding['status'] = 'complete'
    $recorderLicenseReviewBinding['missing_license_component_count'] = 0
    $recorderPackages = @(
        [ordered]@{
            name = 'latentdeck-cartridge'
            version = '0.1.0'
            file_name = 'latentdeck_cartridge-0.1.0-cp312-abi3-win_amd64.whl'
            byte_length = 1
            sha256 = ('3' * 64)
        },
        [ordered]@{
            name = 'latentdeck-comfy-cartridge'
            version = '0.1.0'
            file_name = 'latentdeck_comfy_cartridge-0.1.0-py3-none-any.whl'
            byte_length = 1
            sha256 = ('4' * 64)
        },
        [ordered]@{
            name = 'safetensors'
            version = '0.8.0'
            file_name = 'safetensors-0.8.0-cp310-abi3-win_amd64.whl'
            byte_length = [int64]$recorderLock.safetensors.byte_length
            sha256 = [string]$recorderLock.safetensors.sha256
        }
    )
    $recorderReceipt = [ordered]@{
        schema_version = 1
        artifact_kind = 'comfy_recorder_bundle'
        release_label = $releaseLabel
        release_channel = $releaseChannel
        target = 'windows-x64'
        python_abi = 'cp312-abi3'
        supported_python = @('cp312', 'cp313')
        signed = $false
        unsigned = $true
        distributable = $true
        contains_model_weights = $false
        contains_cartridges = $false
        source = [ordered]@{
            git_commit = $commit
            git_branch = 'main'
            git_tree = $tree
            git_dirty = $false
            git_dirty_entry_count = 0
            public_snapshot_sha256 = $publicSnapshot
            public_snapshot_file_count = $publicSnapshotFileCount
        }
        packages = $recorderPackages
        archive = $recorderArchiveBinding
        sbom = $recorderSbomBinding
        third_party_notices = $recorderNoticeBinding
        license_bundle = $recorderLicenseBundleBinding
        license_review = $recorderLicenseReviewBinding
    }
    $recorderReceiptPath = Join-Path $recorderRoot "$recorderBaseName.receipt.json"
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
    Write-Sums -Root $recorderRoot -Paths @(
        $recorderArchiveName,
        "$recorderBaseName-sbom.cdx.json",
        "$recorderBaseName-THIRD-PARTY-NOTICES.md",
        "$recorderBaseName-THIRD-PARTY-LICENSES.json",
        "$recorderBaseName-license-review.json"
    )
    Move-Item `
        -LiteralPath (Join-Path $recorderRoot 'SHA256SUMS.txt') `
        -Destination (Join-Path $recorderRoot "$recorderBaseName.SHA256SUMS.txt")

    $developerArchive = "LatentDeck-$releaseLabel-developer-kit-windows-x64.zip"
    foreach ($entry in ([ordered]@{
        'THIRD_PARTY_NOTICES.md' = 'fixture developer notices'
        'LICENSE-REVIEW.json' = '{"status":"complete"}'
    }).GetEnumerator()) {
        Write-Text -Path (Join-Path $developerRoot $entry.Key) -Text ([string]$entry.Value + "`n")
    }
    Write-ZipWithFile `
        -Path (Join-Path $developerRoot $developerArchive) `
        -EntryName "bundles/$recorderArchiveName" `
        -SourcePath (Join-Path $recorderRoot $recorderArchiveName)
    & (Join-Path $PSScriptRoot 'New-Sbom.ps1') `
        -OutputPath (Join-Path $developerRoot 'SBOM.cdx.json') `
        -ArtifactName 'LatentDeck Developer Kit' `
        -ArtifactVersion $releaseLabel `
        -ArtifactScope developer-kit `
        -CargoPackage @(
            'latentdeck-cartridge', 'latentdeck-cartridge-python',
            'latentdeck-extension-manager'
        ) `
        -PythonPackage @(
            'latentdeck-cartridge', 'latentdeck-codec-sdk', 'latentdeck-deck-sdk',
            'latentdeck-codec-host', 'latentdeck-operator-d2', 'latentdeck-operator-q4',
            'latentdeck-comfy-toolkit', 'latentdeck-comfy-cartridge',
            'latentdeck-example-channel-roll'
        ) `
        -PythonBuildPackage @('maturin==1.15.0', 'uv-build==0.12.7') `
        -Deterministic | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not generate the Developer Kit staging fixture SBOM.'
    }
    $developerLicenseBundle = New-ReleaseLicenseBundle `
        -SbomPath (Join-Path $developerRoot 'SBOM.cdx.json') `
        -ArtifactName 'LatentDeck Developer Kit' `
        -ArtifactVersion $releaseLabel `
        -OutputPath (Join-Path $developerRoot 'THIRD_PARTY_LICENSES.json') `
        -RepositoryNoticePath (Join-Path $repositoryRoot 'THIRD_PARTY_NOTICES.md')
    $developerReceipt = [ordered]@{
        schema_version = 2
        release_label = $releaseLabel
        release_channel = $releaseChannel
        application_api_version = '0.1.0'
        windows_installer_version = '0.1.0+1'
        distributable = $true
        signed = $false
        unsigned = $true
        source = [ordered]@{
            git_commit = $commit
            git_branch = 'main'
            git_tree = $tree
            git_dirty = $false
            git_dirty_entry_count = 0
            public_snapshot_sha256 = $publicSnapshot
            public_snapshot_file_count = $publicSnapshotFileCount
        }
        archive = (Get-Receipt -Path (Join-Path $developerRoot $developerArchive))
        sbom = (Get-Receipt -Path (Join-Path $developerRoot 'SBOM.cdx.json'))
        notices = (Get-Receipt -Path (Join-Path $developerRoot 'THIRD_PARTY_NOTICES.md'))
        license_review = [ordered]@{
            name = 'LICENSE-REVIEW.json'
            byte_length = (Get-Item -LiteralPath (Join-Path $developerRoot 'LICENSE-REVIEW.json')).Length
            sha256 = (Get-FileHash -LiteralPath (Join-Path $developerRoot 'LICENSE-REVIEW.json') -Algorithm SHA256).Hash.ToLowerInvariant()
            status = 'complete'
            missing_license_component_count = 0
        }
        license_bundle = [ordered]@{
            name = 'THIRD_PARTY_LICENSES.json'
            byte_length = $developerLicenseBundle.ByteLength
            sha256 = $developerLicenseBundle.Sha256
            schema_version = 1
            component_count = $developerLicenseBundle.ComponentCount
            text_count = $developerLicenseBundle.TextCount
            build_only_no_text_disposition_count = $developerLicenseBundle.NoTextDispositionCount
        }
        comfy_recorder_bundle = [ordered]@{
            artifact_kind = 'comfy_recorder_bundle'
            archive = [ordered]@{
                path = "bundles/$recorderArchiveName"
                file_name = $recorderArchiveName
                byte_length = [int64]$recorderArchiveBinding.byte_length
                sha256 = [string]$recorderArchiveBinding.sha256
            }
            standalone_receipt = [ordered]@{
                file_name = "$recorderBaseName.receipt.json"
                byte_length = [int64](Get-Item -LiteralPath $recorderReceiptPath).Length
                sha256 = (Get-FileHash -LiteralPath $recorderReceiptPath -Algorithm SHA256).
                    Hash.ToLowerInvariant()
            }
            python_abi = 'cp312-abi3'
            supported_python = @('cp312', 'cp313')
            packages = $recorderPackages
        }
    }
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
    Write-Sums -Root $developerRoot -Paths @(
        $developerArchive,
        'SBOM.cdx.json',
        'THIRD_PARTY_NOTICES.md',
        'LICENSE-REVIEW.json',
        'THIRD_PARTY_LICENSES.json'
    )

    foreach ($booleanMutation in @(
        [pscustomobject]@{
            Name = 'application-distributable-string'
            Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.distributable }
            Set = { param($value) $appReceipt.distributable = $value }
            Value = 'false'
        },
        [pscustomobject]@{
            Name = 'h3-dirty-number'
            Receipt = $codecReceipt
            Path = (Join-Path $codecRoot 'distributable-proof.json')
            Get = { $codecReceipt.source.git_dirty }
            Set = { param($value) $codecReceipt.source.git_dirty = $value }
            Value = [int64]0
        },
        [pscustomobject]@{
            Name = 'developer-signed-number'
            Receipt = $developerReceipt
            Path = (Join-Path $developerRoot 'developer-kit.json')
            Get = { $developerReceipt.signed }
            Set = { param($value) $developerReceipt.signed = $value }
            Value = [int64]0
        },
        [pscustomobject]@{
            Name = 'recorder-unsigned-string'
            Receipt = $recorderReceipt
            Path = $recorderReceiptPath
            Get = { $recorderReceipt.unsigned }
            Set = { param($value) $recorderReceipt.unsigned = $value }
            Value = 'false'
        },
        [pscustomobject]@{
            Name = 'application-payload-unsigned-string'
            Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.applications[0].unsigned }
            Set = { param($value) $appReceipt.applications[0].unsigned = $value }
            Value = 'false'
        }
    )) {
        $originalBoolean = & $booleanMutation.Get
        & $booleanMutation.Set $booleanMutation.Value
        Write-Json -Path ([string]$booleanMutation.Path) -Value $booleanMutation.Receipt
        Assert-Throws -ExpectedText 'JSON Boolean' -Action {
            & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
                -ApplicationArtifactDirectory $appRoot `
                -CodecArtifactDirectory $codecRoot `
                -DeveloperKitArtifactDirectory $developerRoot `
                -ComfyRecorderArtifactDirectory $recorderRoot `
                -OutputDirectory (Join-Path $testRoot "boolean-$($booleanMutation.Name)-output")
        }
        & $booleanMutation.Set $originalBoolean
        Write-Json -Path ([string]$booleanMutation.Path) -Value $booleanMutation.Receipt
    }
    foreach ($typeMutation in @(
        [pscustomobject]@{
            Name = 'schema-version-string'; Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.schema_version }
            Set = { param($value) $appReceipt.schema_version = $value }
            Value = '6'; Expected = 'JSON integer'
        },
        [pscustomobject]@{
            Name = 'artifact-length-string'; Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.applications[0].byte_length }
            Set = { param($value) $appReceipt.applications[0].byte_length = $value }
            Value = [string]$appReceipt.applications[0].byte_length; Expected = 'JSON integer'
        },
        [pscustomobject]@{
            Name = 'license-missing-count-string'; Receipt = $developerReceipt
            Path = (Join-Path $developerRoot 'developer-kit.json')
            Get = { $developerReceipt.license_review.missing_license_component_count }
            Set = { param($value) $developerReceipt.license_review.missing_license_component_count = $value }
            Value = '0'; Expected = 'JSON integer'
        },
        [pscustomobject]@{
            Name = 'selection-root-count-string'; Receipt = $recorderReceipt
            Path = $recorderReceiptPath
            Get = { $recorderReceipt.sbom.selection_root_count }
            Set = { param($value) $recorderReceipt.sbom.selection_root_count = $value }
            Value = '7'; Expected = 'JSON integer'
        }
    )) {
        $originalValue = & $typeMutation.Get
        & $typeMutation.Set $typeMutation.Value
        Write-Json -Path ([string]$typeMutation.Path) -Value $typeMutation.Receipt
        Assert-Throws -ExpectedText ([string]$typeMutation.Expected) -Action {
            & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
                -ApplicationArtifactDirectory $appRoot `
                -CodecArtifactDirectory $codecRoot `
                -DeveloperKitArtifactDirectory $developerRoot `
                -ComfyRecorderArtifactDirectory $recorderRoot `
                -OutputDirectory (Join-Path $testRoot "type-$($typeMutation.Name)-output")
        }
        & $typeMutation.Set $originalValue
        Write-Json -Path ([string]$typeMutation.Path) -Value $typeMutation.Receipt
    }
    foreach ($stringMutation in @(
        [pscustomobject]@{
            Name = 'release-channel-array'; Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.release_channel }
            Set = { param($value) $appReceipt.release_channel = $value }
            Value = ,@($releaseChannel)
        },
        [pscustomobject]@{
            Name = 'source-commit-array'; Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.source.git_commit }
            Set = { param($value) $appReceipt.source.git_commit = $value }
            Value = ,@($commit)
        },
        [pscustomobject]@{
            Name = 'artifact-sha-array'; Receipt = $appReceipt
            Path = (Join-Path $appRoot 'release-candidate.json')
            Get = { $appReceipt.applications[0].sha256 }
            Set = { param($value) $appReceipt.applications[0].sha256 = $value }
            Value = ,@([string]$appReceipt.applications[0].sha256)
        }
    )) {
        $originalString = & $stringMutation.Get
        & $stringMutation.Set $stringMutation.Value
        Write-Json -Path ([string]$stringMutation.Path) -Value $stringMutation.Receipt
        Assert-Throws -ExpectedText 'JSON string' -Action {
            & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
                -ApplicationArtifactDirectory $appRoot `
                -CodecArtifactDirectory $codecRoot `
                -DeveloperKitArtifactDirectory $developerRoot `
                -ComfyRecorderArtifactDirectory $recorderRoot `
                -OutputDirectory (Join-Path $testRoot "string-$($stringMutation.Name)-output")
        }
        & $stringMutation.Set $originalString
        Write-Json -Path ([string]$stringMutation.Path) -Value $stringMutation.Receipt
    }
    $originalSupportedPython = @($recorderReceipt.supported_python)
    $recorderReceipt.supported_python = 'cp312'
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
    Assert-Throws -ExpectedText 'JSON array' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'supported-python-scalar-output')
    }
    $recorderReceipt.supported_python = $originalSupportedPython
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
    $originalSnapshotCount = $appReceipt.source.public_snapshot_file_count
    $appReceipt.source.public_snapshot_file_count = '1'
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Assert-Throws -ExpectedText 'JSON integer' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'string-source-count-output')
    }
    $appReceipt.source.public_snapshot_file_count = $originalSnapshotCount
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt

    $originalNativeEvidenceHash = [string]$codecReceipt.safetensors_native_closure.native_binary_sha256
    $codecReceipt.safetensors_native_closure.native_binary_sha256 = '0' * 64
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Assert-Throws -ExpectedText 'reviewed lock' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'h3-native-evidence-output')
    }
    $codecReceipt.safetensors_native_closure.native_binary_sha256 = $originalNativeEvidenceHash
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt

    $packageReceiptPath = Join-Path $codecRoot 'package-receipt.json'
    $originalNativeSbomHash = [string]$packageReceipt.native_rust.sbom_sha256
    $packageReceipt.native_rust.sbom_sha256 = '3' * 64
    Write-Json -Path $packageReceiptPath -Value $packageReceipt
    $tamperedPackageReceiptRecord = Get-Receipt -Path $packageReceiptPath
    $codecReceipt.sidecars.'package-receipt.json' = [ordered]@{
        name = 'package-receipt.json'
        byte_length = $tamperedPackageReceiptRecord.byte_length
        sha256 = $tamperedPackageReceiptRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Assert-Throws -ExpectedText 'differs between the .ldcodec payload and package receipt' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'h3-native-sbom-binding-output')
    }
    $packageReceipt.native_rust.sbom_sha256 = $originalNativeSbomHash
    Write-Json -Path $packageReceiptPath -Value $packageReceipt
    $restoredPackageReceiptRecord = Get-Receipt -Path $packageReceiptPath
    $codecReceipt.sidecars.'package-receipt.json' = [ordered]@{
        name = 'package-receipt.json'
        byte_length = $restoredPackageReceiptRecord.byte_length
        sha256 = $restoredPackageReceiptRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt

    $packageReceipt.content_policy.model_weights_allowed = 'false'
    Write-Json -Path $packageReceiptPath -Value $packageReceipt
    $typedPackageReceiptRecord = Get-Receipt -Path $packageReceiptPath
    $codecReceipt.sidecars.'package-receipt.json' = [ordered]@{
        name = 'package-receipt.json'
        byte_length = $typedPackageReceiptRecord.byte_length
        sha256 = $typedPackageReceiptRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Assert-Throws -ExpectedText 'JSON Boolean' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'h3-string-content-policy-output')
    }
    $packageReceipt.content_policy.model_weights_allowed = $false
    Write-Json -Path $packageReceiptPath -Value $packageReceipt
    $restoredPackageReceiptRecord = Get-Receipt -Path $packageReceiptPath
    $codecReceipt.sidecars.'package-receipt.json' = [ordered]@{
        name = 'package-receipt.json'
        byte_length = $restoredPackageReceiptRecord.byte_length
        sha256 = $restoredPackageReceiptRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt

    $appReceipt.component_versions.sdks.codec = '9.9.9'
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Assert-Throws -ExpectedText 'Deck or SDK component version' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'component-version-output')
    }
    $appReceipt.component_versions.sdks.codec = '0.2.0'
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Write-Sums -Root $developerRoot -Paths @(
        $developerArchive,
        'SBOM.cdx.json',
        'THIRD_PARTY_NOTICES.md',
        'LICENSE-REVIEW.json',
        'THIRD_PARTY_LICENSES.json'
    )

    $artifactAlias = Join-Path $testRoot 'artifact-alias'
    New-Item -ItemType Junction -Path $artifactAlias -Target $testRoot | Out-Null
    try {
        Assert-Throws -ExpectedText 'reparse-point directory' -Action {
            & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
                -ApplicationArtifactDirectory (Join-Path $artifactAlias 'app') `
                -CodecArtifactDirectory $codecRoot `
                -DeveloperKitArtifactDirectory $developerRoot `
                -ComfyRecorderArtifactDirectory $recorderRoot `
                -OutputDirectory (Join-Path $testRoot 'reparse-output')
        }
    } finally {
        if (Test-Path -LiteralPath $artifactAlias) {
            Remove-Item -LiteralPath $artifactAlias -Force
        }
    }

    foreach ($receipt in @($appReceipt, $codecReceipt, $developerReceipt, $recorderReceipt)) {
        $receipt.release_label = '0.1.0'
    }
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
    Assert-Throws -ExpectedText 'exact supported pair' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'label-channel-output')
    }
    foreach ($receipt in @($appReceipt, $codecReceipt, $developerReceipt, $recorderReceipt)) {
        $receipt.release_label = $releaseLabel
    }
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt

    foreach ($caseVariant in @('UNSIGNED_PREVIEW', 'Unsigned_Preview')) {
        foreach ($receipt in @($appReceipt, $codecReceipt, $developerReceipt, $recorderReceipt)) {
            $receipt.release_channel = $caseVariant
        }
        Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
        Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
        Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
        Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
        Assert-Throws -ExpectedText 'supported release label and channel' -Action {
            & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
                -ApplicationArtifactDirectory $appRoot `
                -CodecArtifactDirectory $codecRoot `
                -DeveloperKitArtifactDirectory $developerRoot `
                -ComfyRecorderArtifactDirectory $recorderRoot `
                -OutputDirectory (Join-Path $testRoot "channel-case-$caseVariant-output")
        }
    }
    foreach ($receipt in @($appReceipt, $codecReceipt, $developerReceipt, $recorderReceipt)) {
        $receipt.release_channel = $releaseChannel
    }
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt

    foreach ($sourceMutation in @('git_tree', 'public_snapshot_sha256', 'public_snapshot_file_count')) {
        $originalValue = $developerReceipt.source.$sourceMutation
        $developerReceipt.source.$sourceMutation = if ($sourceMutation -ceq 'git_tree') {
            'b' * 40
        } elseif ($sourceMutation -ceq 'public_snapshot_sha256') {
            'c' * 64
        } else {
            2
        }
        Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
        Assert-Throws -ExpectedText 'same clean main source snapshot' -Action {
            & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
                -ApplicationArtifactDirectory $appRoot `
                -CodecArtifactDirectory $codecRoot `
                -DeveloperKitArtifactDirectory $developerRoot `
                -ComfyRecorderArtifactDirectory $recorderRoot `
                -OutputDirectory (Join-Path $testRoot "source-$sourceMutation-output")
        }
        $developerReceipt.source.$sourceMutation = $originalValue
    }
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt

    $originalRecorderTree = $recorderReceipt.source.git_tree
    $recorderReceipt.source.git_tree = 'd' * 40
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
    Assert-Throws -ExpectedText 'same clean main source snapshot' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'recorder-source-output')
    }
    $recorderReceipt.source.git_tree = $originalRecorderTree
    Write-Json -Path $recorderReceiptPath -Value $recorderReceipt
    $restoredRecorderReceiptItem = Get-Item -LiteralPath $recorderReceiptPath
    $developerReceipt.comfy_recorder_bundle.standalone_receipt.byte_length = `
        [int64]$restoredRecorderReceiptItem.Length
    $developerReceipt.comfy_recorder_bundle.standalone_receipt.sha256 = `
        (Get-FileHash -LiteralPath $recorderReceiptPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt

    $originalNestedRecorderHash = $developerReceipt.comfy_recorder_bundle.archive.sha256
    $developerReceipt.comfy_recorder_bundle.archive.sha256 = 'd' * 64
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt
    Assert-Throws -ExpectedText 'exact standalone Comfy Recorder artifact' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'recorder-nested-hash-output')
    }
    $developerReceipt.comfy_recorder_bundle.archive.sha256 = $originalNestedRecorderHash
    Write-Json -Path (Join-Path $developerRoot 'developer-kit.json') -Value $developerReceipt

    $recorderSumsPath = Join-Path $recorderRoot "$recorderBaseName.SHA256SUMS.txt"
    $recorderSumsOriginal = [System.IO.File]::ReadAllText($recorderSumsPath)
    $recorderSumsOmitted = @(
        Get-Content -LiteralPath $recorderSumsPath |
            Where-Object { $_ -notlike "*  $recorderBaseName-license-review.json" }
    )
    Write-Text -Path $recorderSumsPath -Text (($recorderSumsOmitted -join "`n") + "`n")
    Assert-Throws -ExpectedText 'coverage is not the exact artifact contract' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'recorder-checksum-omission-output')
    }
    Write-Text -Path $recorderSumsPath -Text $recorderSumsOriginal

    $archiveSmokePath = Join-Path $codecRoot 'archive-runtime-smoke.json'
    $archiveSmokeOriginal = [System.IO.File]::ReadAllText($archiveSmokePath)
    Write-Text -Path $archiveSmokePath -Text ($archiveSmokeOriginal + " `n")
    Assert-Throws -ExpectedText 'sidecar hash binding failed' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'sidecar-tamper-output')
    }
    Write-Text -Path $archiveSmokePath -Text $archiveSmokeOriginal

    Write-Text -Path $archiveSmokePath -Text "{}`n"
    $emptySmokeRecord = Get-Receipt -Path $archiveSmokePath
    $codecReceipt.sidecars.'archive-runtime-smoke.json' = [ordered]@{
        name = 'archive-runtime-smoke.json'
        byte_length = $emptySmokeRecord.byte_length
        sha256 = $emptySmokeRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Assert-Throws -ExpectedText 'exact supported property set' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'empty-sidecar-output')
    }
    Write-Text -Path $archiveSmokePath -Text $archiveSmokeOriginal
    $restoredSmokeRecord = Get-Receipt -Path $archiveSmokePath
    $codecReceipt.sidecars.'archive-runtime-smoke.json' = [ordered]@{
        name = 'archive-runtime-smoke.json'
        byte_length = $restoredSmokeRecord.byte_length
        sha256 = $restoredSmokeRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt

    $installerSbomPath = Join-Path $codecRoot 'installer-SBOM.cdx.json'
    $installerSbomOriginal = [System.IO.File]::ReadAllText($installerSbomPath)
    [void]$installerSbomFixture.components[0].Remove('licenses')
    Write-Json -Path $installerSbomPath -Value $installerSbomFixture
    $incompleteSbomRecord = Get-Receipt -Path $installerSbomPath
    $setupSidecar.sbom.byte_length = $incompleteSbomRecord.byte_length
    $setupSidecar.sbom.sha256 = $incompleteSbomRecord.sha256
    Write-Json -Path (Join-Path $codecRoot 'setup-receipt.json') -Value $setupSidecar
    $incompleteSetupRecord = Get-Receipt -Path (Join-Path $codecRoot 'setup-receipt.json')
    $codecReceipt.sidecars.'setup-receipt.json' = [ordered]@{
        name = 'setup-receipt.json'
        byte_length = $incompleteSetupRecord.byte_length
        sha256 = $incompleteSetupRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Write-Sums -Root $codecRoot -Paths $codecChecksumPaths
    Assert-Throws -ExpectedText 'incomplete license metadata' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'incomplete-installer-sbom-output')
    }
    Write-Text -Path $installerSbomPath -Text $installerSbomOriginal
    $restoredInstallerSbomRecord = Get-Receipt -Path $installerSbomPath
    $setupSidecar.sbom.byte_length = $restoredInstallerSbomRecord.byte_length
    $setupSidecar.sbom.sha256 = $restoredInstallerSbomRecord.sha256
    Write-Json -Path (Join-Path $codecRoot 'setup-receipt.json') -Value $setupSidecar
    $restoredSetupRecord = Get-Receipt -Path (Join-Path $codecRoot 'setup-receipt.json')
    $codecReceipt.sidecars.'setup-receipt.json' = [ordered]@{
        name = 'setup-receipt.json'
        byte_length = $restoredSetupRecord.byte_length
        sha256 = $restoredSetupRecord.sha256
    }
    Write-Json -Path (Join-Path $codecRoot 'distributable-proof.json') -Value $codecReceipt
    Write-Sums -Root $codecRoot -Paths $codecChecksumPaths

    $appSumsPath = Join-Path $appRoot 'SHA256SUMS.txt'
    $appSumsOriginal = [System.IO.File]::ReadAllText($appSumsPath)
    Write-Text -Path $appSumsPath -Text (('a' * 5000) + "`n")
    Assert-Throws -ExpectedText '4096-character bound' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'long-checksum-line-output')
    }
    Write-Text -Path $appSumsPath -Text $appSumsOriginal

    $outsideSbomPath = Join-Path $testRoot 'outside-sbom.json'
    Write-Text -Path $outsideSbomPath -Text '{"bomFormat":"CycloneDX","outside":true}'
    $outsideSbom = Get-Receipt -Path $outsideSbomPath
    $appReceipt.sboms[0].file_name = '../outside-sbom.json'
    $appReceipt.sboms[0].byte_length = $outsideSbom.byte_length
    $appReceipt.sboms[0].sha256 = $outsideSbom.sha256
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt
    Assert-Throws -ExpectedText 'exact artifact allowlist' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'traversal-output')
    }
    $restoredSbom = Get-Receipt -Path (Join-Path $appRoot $appSbomPaths[0])
    $appReceipt.sboms[0].file_name = $appSbomPaths[0]
    $appReceipt.sboms[0].byte_length = $restoredSbom.byte_length
    $appReceipt.sboms[0].sha256 = $restoredSbom.sha256
    Write-Json -Path (Join-Path $appRoot 'release-candidate.json') -Value $appReceipt

    Write-Sums -Root $appRoot -Paths @(
        "installers/$($appNames[0])",
        "installers/$($appNames[1])",
        $appSbomPaths[0],
        $appSbomPaths[1],
        'metadata/THIRD_PARTY_NOTICES.md'
    )
    Assert-Throws -ExpectedText 'coverage is not the exact artifact contract' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'omitted-checksum-output')
    }
    Write-Sums -Root $appRoot -Paths @(
        "installers/$($appNames[0])",
        "installers/$($appNames[1])",
        $appSbomPaths[0],
        $appSbomPaths[1],
        'metadata/THIRD_PARTY_NOTICES.md',
        'metadata/THIRD_PARTY_LICENSES.json'
    )

    $output = Join-Path $testRoot 'release-output'
    $actualOutput = & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
        -ApplicationArtifactDirectory $appRoot `
        -CodecArtifactDirectory $codecRoot `
        -DeveloperKitArtifactDirectory $developerRoot `
        -ComfyRecorderArtifactDirectory $recorderRoot `
        -OutputDirectory $output
    if ([System.IO.Path]::GetFullPath([string]$actualOutput) -cne
        [System.IO.Path]::GetFullPath($output)) {
        throw 'GitHub Release staging returned an unexpected output directory.'
    }
    $manifest = Get-Content -LiteralPath (Join-Path $output 'RELEASE-MANIFEST.json') -Raw |
        ConvertFrom-Json -Depth 100
    if ($manifest.release_label -cne $releaseLabel -or
        $manifest.release_channel -cne $releaseChannel -or
        -not [bool]$manifest.prerelease -or
        [string]$manifest.identities.decks.d2.deck_version -cne '0.2.1' -or
        [string]$manifest.identities.decks.q4.deck_version -cne '0.2.1' -or
        [string]$manifest.identities.sdks.cartridge -cne '0.1.0' -or
        [string]$manifest.identities.sdks.deck -cne '0.2.0' -or
        [string]$manifest.identities.sdks.codec -cne '0.2.0' -or
        [string]$manifest.identities.comfy_recorder.python_abi -cne 'cp312-abi3' -or
        (@($manifest.identities.comfy_recorder.supported_python) -join "`0") -cne
            (@('cp312', 'cp313') -join "`0") -or
        @($manifest.identities.comfy_recorder.packages).Count -ne 3 -or
        [string]$manifest.identities.comfy_recorder.archive_sha256 -cne
            [string]$recorderReceipt.archive.sha256 -or
        [string]$manifest.identities.comfy_recorder.developer_kit_nested_archive_sha256 -cne
            [string]$recorderReceipt.archive.sha256 -or
        [string]$manifest.source.git_tree -cne $tree -or
        [string]$manifest.source.public_snapshot_sha256 -cne $publicSnapshot -or
        [int64]$manifest.source.public_snapshot_file_count -ne $publicSnapshotFileCount -or
        @($manifest.assets).Count -ne 31 -or
        @($manifest.assets | Group-Object name | Where-Object Count -ne 1).Count -gt 0) {
        throw 'GitHub Release manifest identity or unique asset inventory is incorrect.'
    }
    $sumLines = @(Get-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt'))
    if ($sumLines.Count -ne 32) {
        throw 'GitHub Release checksum manifest must cover thirty-one assets and RELEASE-MANIFEST.json.'
    }
    foreach ($line in $sumLines) {
        if ($line -cnotmatch '^(?<hash>[0-9a-f]{64})  (?<name>[^/\\]+)$') {
            throw "GitHub Release checksum line is invalid: $line"
        }
        $path = Join-Path $output $Matches.name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or
            (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $Matches.hash) {
            throw "GitHub Release checksum verification failed: $($Matches.name)"
        }
    }

    Assert-Throws -ExpectedText 'Refusing to overwrite' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory $output
    }

    Write-Text `
        -Path (Join-Path $appRoot "installers/$($appNames[0])") `
        -Text 'tampered fixture installer'
    Assert-Throws -ExpectedText 'Checksum mismatch' -Action {
        & (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') `
            -ApplicationArtifactDirectory $appRoot `
            -CodecArtifactDirectory $codecRoot `
            -DeveloperKitArtifactDirectory $developerRoot `
            -ComfyRecorderArtifactDirectory $recorderRoot `
            -OutputDirectory (Join-Path $testRoot 'tampered-output')
    }

    $stagerSource = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'Stage-GitHubRelease.ps1') -Raw
    if ($stagerSource -cnotmatch '\[int64\]2GB' -or
        $stagerSource -cnotmatch 'Length -ge \$maximumGitHubAssetBytes' -or
        $stagerSource -cnotmatch 'FileShare\]::Read' -or
        $stagerSource -cnotmatch 'changed after validation and was not staged') {
        throw 'GitHub Release stager no longer enforces size or validation-to-copy integrity.'
    }
    Write-Host 'GITHUB RELEASE STAGING CONTRACT: PASS' -ForegroundColor Green
} finally {
    if (Test-Path -LiteralPath $testRoot) {
        $resolved = [System.IO.Path]::GetFullPath($testRoot)
        $rootPrefix = $artifactsRoot.TrimEnd('\') + '\'
        if (-not $resolved.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not ([System.IO.Path]::GetFileName($resolved)).StartsWith(
                '.github-stage-contract-',
                [System.StringComparison]::Ordinal
            )) {
            throw "Refusing to remove unsafe GitHub staging test directory: $resolved"
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
