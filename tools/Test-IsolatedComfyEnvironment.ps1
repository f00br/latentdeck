[CmdletBinding()]
param(
    [string]$EnvironmentRoot,

    [switch]$ContractOnly,

    [switch]$ServerSmoke,

    [ValidateRange(0, 65535)]
    [int]$Port = 0,

    [ValidateRange(10, 120)]
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'artifacts'))
$initializer = Join-Path $PSScriptRoot 'Initialize-IsolatedComfyEnvironment.ps1'
$launcher = Join-Path $PSScriptRoot 'Start-IsolatedComfyEnvironment.ps1'
$masterWorkflowGenerator = Join-Path $PSScriptRoot 'New-PrivateComfyMasterWorkflows.ps1'

function Assert-True {
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

function Assert-ChildPath {
    param(
        [Parameter(Mandatory)]
        [string]$Root,
        [Parameter(Mandatory)]
        [string]$Candidate,
        [Parameter(Mandatory)]
        [string]$Label
    )

    $rootFull = [System.IO.Path]::GetFullPath($Root)
    $candidateFull = [System.IO.Path]::GetFullPath($Candidate)
    $relative = [System.IO.Path]::GetRelativePath($rootFull, $candidateFull)
    Assert-True `
        -Condition ($relative -ne '.' -and
            -not $relative.StartsWith('..', [System.StringComparison]::Ordinal) -and
            -not [System.IO.Path]::IsPathFullyQualified($relative)) `
        -Message "$Label must be a child of $rootFull."
}

function Restore-GeneratedTempDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$Environment,
        [Parameter(Mandatory)]
        [string]$BaseDirectory,
        [Parameter(Mandatory)]
        [string]$TempDirectory
    )

    Assert-ChildPath -Root $Environment -Candidate $TempDirectory -Label 'paths.temp'
    $expected = [System.IO.Path]::GetFullPath((Join-Path $BaseDirectory 'temp'))
    $actual = [System.IO.Path]::GetFullPath($TempDirectory)
    Assert-True `
        -Condition $actual.Equals($expected, [System.StringComparison]::OrdinalIgnoreCase) `
        -Message 'Isolated temp path must be the generated base-directory temp child.'
    if (Test-Path -LiteralPath $actual) {
        Assert-True -Condition (Test-Path -LiteralPath $actual -PathType Container) `
            -Message 'Generated temp path exists but is not a directory.'
        $item = Get-Item -LiteralPath $actual -Force
        Assert-True `
            -Condition (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) `
            -Message 'Generated temp directory must not be a reparse point.'
        return
    }
    [System.IO.Directory]::CreateDirectory($actual) | Out-Null
}

foreach ($script in @($initializer, $launcher, $masterWorkflowGenerator)) {
    Assert-True -Condition (Test-Path -LiteralPath $script -PathType Leaf) `
        -Message "Missing isolated Comfy environment script: $script"
    $tokens = $null
    $parseErrors = $null
    [void][System.Management.Automation.Language.Parser]::ParseFile(
        $script,
        [ref]$tokens,
        [ref]$parseErrors
    )
    if ($parseErrors.Count -ne 0) {
        throw "PowerShell syntax errors in $script`: $($parseErrors.Message -join '; ')"
    }

    $source = Get-Content -LiteralPath $script -Raw
    Assert-True -Condition ($source -notmatch '(?im)(?<![A-Z])[A-Z]:\\') `
        -Message "$script contains a machine-specific absolute Windows path literal."
}

$launcherSource = Get-Content -LiteralPath $launcher -Raw
Assert-True -Condition ($launcherSource.Contains("`$arguments += '--auto-launch'")) `
    -Message 'OpenBrowser must delegate to Comfy auto-launch after listener readiness.'
Assert-True -Condition ($launcherSource.Contains("`$arguments += '--disable-auto-launch'")) `
    -Message 'Non-browser launches must keep Comfy auto-launch disabled.'
Assert-True -Condition ($launcherSource -notmatch '(?m)^\s*Start-Process\s+\$url\s*$') `
    -Message 'Launcher must not open the browser before Comfy reports server readiness.'

$generatorSource = Get-Content -LiteralPath $masterWorkflowGenerator -Raw
foreach ($requiredContract in @(
    '[string]$AlignSourceA',
    '[string]$AlignSourceB',
    '[string]$RawSource',
    '[string]$HqVaePath',
    '[string]$HqVaeExpectedSha256',
    '& $Cli inspect $source',
    'Get-FullClipSignature',
    'Get-ValidatedTaeh3Asset',
    'Get-ValidatedHqVaeAsset',
    'Get-ValidatedRawSource',
    'Toolkit D2/Q4 full-clip mismatch',
    'full_clip_signature = $mixerSignature',
    'mixer_sources = $mixerReceipts',
    'align_sources = $alignReceipts',
    'raw_source = $rawMaterialized.receipt',
    'external_assets = [ordered]@{'
)) {
    Assert-True -Condition $generatorSource.Contains($requiredContract) `
        -Message "Private master generator omitted contract: $requiredContract"
}

function Test-MasterWorkflowGeneratorContract {
    $contractRoot = Join-Path `
        $artifactsRoot `
        "comfy-master-generator-contract-$([Guid]::NewGuid().ToString('N'))"
    Assert-ChildPath -Root $artifactsRoot -Candidate $contractRoot -Label 'generator contract root'
    $environment = Join-Path $contractRoot 'environment'
    $input = Join-Path $environment 'input'
    $sources = Join-Path $contractRoot 'sources'
    $models = Join-Path $contractRoot 'models'
    $vae = Join-Path $models 'vae'
    $vaeApprox = Join-Path $models 'vae_approx'
    [System.IO.Directory]::CreateDirectory($input) | Out-Null
    [System.IO.Directory]::CreateDirectory($sources) | Out-Null
    [System.IO.Directory]::CreateDirectory($vae) | Out-Null
    [System.IO.Directory]::CreateDirectory($vaeApprox) | Out-Null

    try {
        $sourcePaths = [ordered]@{
            T32A = Join-Path $sources 't32-a.lc'
            T32B = Join-Path $sources 't32-b.lc'
            T72 = Join-Path $sources 't72.lc'
            T107 = Join-Path $sources 't107.lc'
        }
        $byte = 1
        foreach ($path in $sourcePaths.Values) {
            [System.IO.File]::WriteAllBytes($path, [byte[]]@($byte))
            $byte++
        }
        $rawPath = Join-Path $sources 'raw-h3.safetensors'
        [System.IO.File]::WriteAllBytes(
            $rawPath,
            [System.Text.Encoding]::UTF8.GetBytes('synthetic contract-only raw H3')
        )
        $rawHash = (Get-FileHash -LiteralPath $rawPath -Algorithm SHA256).Hash.ToLowerInvariant()
        $rawLength = [int64](Get-Item -LiteralPath $rawPath).Length
        $taeh3Path = Join-Path $vaeApprox 'taeh3.safetensors'
        [System.IO.File]::WriteAllBytes(
            $taeh3Path,
            [System.Text.Encoding]::UTF8.GetBytes('synthetic contract-only taeh3')
        )
        $taeh3Hash = (Get-FileHash -LiteralPath $taeh3Path -Algorithm SHA256).Hash.ToLowerInvariant()
        $taeh3Length = [int64](Get-Item -LiteralPath $taeh3Path).Length
        $taeh3Descriptor = Join-Path $contractRoot 'taeh3.asset.json'
        $taeh3DescriptorValue = [ordered]@{
            asset_id = 'taeh3'
            format = 'safetensors'
            selection = 'explicit_file'
            accepted_variants = @(
                [ordered]@{
                    variant_id = 'synthetic-contract-only'
                    sha256 = $taeh3Hash
                    byte_length = $taeh3Length
                    source_url = 'https://example.invalid/taeh3.safetensors'
                    license_label = 'TEST-ONLY'
                    license_url = 'https://example.invalid/LICENSE'
                }
            )
        }
        [System.IO.File]::WriteAllText(
            $taeh3Descriptor,
            ($taeh3DescriptorValue | ConvertTo-Json -Depth 8) + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )
        $hqVaePath = Join-Path $vae 'minimax-h3-native-vae.safetensors'
        [System.IO.File]::WriteAllBytes(
            $hqVaePath,
            [System.Text.Encoding]::UTF8.GetBytes('synthetic contract-only native H3 VAE')
        )
        $hqVaeHash = (Get-FileHash -LiteralPath $hqVaePath -Algorithm SHA256).Hash.ToLowerInvariant()
        $hqVaeLength = [int64](Get-Item -LiteralPath $hqVaePath).Length
        $environmentReceipt = [ordered]@{
            schema_version = 1
            private_artifact = $true
            repository = [ordered]@{ commit = 'synthetic-contract' }
            paths = [ordered]@{ input = $input }
            external_models = [ordered]@{
                models_root = $models
                taeh3 = [ordered]@{
                    path = $taeh3Path
                    sha256 = $taeh3Hash
                    byte_length = $taeh3Length
                }
                hq_h3_vae = [ordered]@{
                    path = $hqVaePath
                    byte_length = $hqVaeLength
                }
            }
        }
        [System.IO.File]::WriteAllText(
            (Join-Path $environment 'environment.json'),
            ($environmentReceipt | ConvertTo-Json -Depth 8) + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )

        $mockCli = Join-Path $contractRoot 'mock-cartridge-cli.ps1'
        $mockCliSource = @'
param([string]$Command, [string]$Source)
$ErrorActionPreference = 'Stop'
$global:LASTEXITCODE = 0
$name = [System.IO.Path]::GetFileNameWithoutExtension($Source)
$sha = (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash.ToLowerInvariant()
if ($Command -ceq 'validate') {
    [ordered]@{
        command = 'validate'
        status = 'ok'
        cartridge_id = "synthetic-$name"
        validation = [ordered]@{
            validation_level = 'full'
            archive_sha256 = $sha
        }
    } | ConvertTo-Json -Depth 8
    return
}
if ($Command -cne 'inspect') {
    $global:LASTEXITCODE = 2
    throw "unsupported mock command $Command"
}
$slots = if ($name.StartsWith('t32')) { 32 } elseif ($name -ceq 't72') { 72 } else { 107 }
$frames = if ($slots -eq 32) { 107 } elseif ($slots -eq 72) { 243 } else { 362 }
$audioSlots = if ($slots -eq 32) { 178 } elseif ($slots -eq 72) { 405 } else { 603 }
$duration = if ($slots -eq 32) {
    [ordered]@{ numerator = 107; denominator = 24 }
} elseif ($slots -eq 72) {
    [ordered]@{ numerator = 81; denominator = 8 }
} else {
    [ordered]@{ numerator = 181; denominator = 12 }
}
[ordered]@{
    command = 'inspect'
    status = 'ok'
    validation_level = 'structure'
    manifest = [ordered]@{
        spec_version = '0.1.0'
        codec = [ordered]@{
            family = 'minimax_h3'
            profile = 'h3_av_latent'
            profile_version = '0.1.0'
        }
        tensors = @(
            [ordered]@{
                name = 'video'; stream = 'visual'; runtime_dtype = 'F16'
                storage_dtype = 'F16'; shape = @(1, 24, $slots, 48, 28)
            },
            [ordered]@{
                name = 'audio'; stream = 'audio'; runtime_dtype = 'F32'
                storage_dtype = 'F32'; shape = @(1, 32, 2, $audioSlots)
            }
        )
        timing = [ordered]@{
            contract = 'minimax_h3_causal'
            contract_version = '0.1.0'
            decoded_video = [ordered]@{
                frame_count = $frames
                width = 448
                height = 768
                duration = $duration
                frame_rate = [ordered]@{ numerator = 24; denominator = 1 }
            }
        }
    }
    profile = [ordered]@{
        audio_latent_slots = $audioSlots
        visual = [ordered]@{
            latent_slots = $slots
            latent_height = 48
            latent_width = 28
            decoded_frames = $frames
            decoded_height = 768
            decoded_width = 448
        }
    }
} | ConvertTo-Json -Depth 20
'@
        [System.IO.File]::WriteAllText(
            $mockCli,
            $mockCliSource,
            [System.Text.UTF8Encoding]::new($false)
        )

        $mockRawInspector = Join-Path $contractRoot 'mock-raw-inspector.ps1'
        $mockRawInspectorSource = @'
param([string]$Source)
$ErrorActionPreference = 'Stop'
$global:LASTEXITCODE = 0
$item = Get-Item -LiteralPath $Source
[ordered]@{
    status = 'ok'
    command = 'inspect_raw_h3'
    byte_length = [int64]$item.Length
    sha256 = (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash.ToLowerInvariant()
    profile = [ordered]@{
        codec_family = 'minimax_h3'
        profile = 'h3_av_latent'
        profile_version = '0.1.0'
        audio_latent_slots = 178
        visual = [ordered]@{
            latent_slots = 32
            latent_height = 48
            latent_width = 28
            decoded_frames = 107
            decoded_height = 768
            decoded_width = 448
        }
    }
} | ConvertTo-Json -Depth 10
'@
        [System.IO.File]::WriteAllText(
            $mockRawInspector,
            $mockRawInspectorSource,
            [System.Text.UTF8Encoding]::new($false)
        )
        $generatorCommon = @{
            RawSource = $rawPath
            HqVaePath = $hqVaePath
            HqVaeExpectedSha256 = $hqVaeHash
            HqVaeSource = 'https://example.invalid/native-h3-vae.safetensors'
            HqVaeLicense = 'TEST-ONLY'
            EnvironmentRoot = $environment
            CartridgeCli = $mockCli
            RawInspector = $mockRawInspector
            Taeh3AssetDescriptor = $taeh3Descriptor
        }

        $staleHqOutput = Join-Path $contractRoot 'stale-hq-output'
        $staleHqCommon = $generatorCommon.Clone()
        $staleHqCommon.HqVaeExpectedSha256 = ('0' * 64) -join ''
        $staleHqMessage = $null
        try {
            & $masterWorkflowGenerator @staleHqCommon `
                -SourceA $sourcePaths.T32A `
                -SourceB $sourcePaths.T32B `
                -SourceC $sourcePaths.T32A `
                -SourceD $sourcePaths.T32B `
                -OutputRoot $staleHqOutput
            throw 'Synthetic stale native H3 VAE hash was unexpectedly accepted.'
        }
        catch {
            $staleHqMessage = $_.Exception.Message
        }
        Assert-True `
            -Condition ($staleHqMessage -ceq
                'Selected native H3 VAE hash does not match HqVaeExpectedSha256.') `
            -Message "Generator returned an unexpected native VAE error: $staleHqMessage"
        Assert-True -Condition (-not (Test-Path -LiteralPath $staleHqOutput)) `
            -Message 'Generator created workflow output for a stale native H3 VAE hash.'

        $staleAssetOutput = Join-Path $contractRoot 'stale-asset-output'
        $environmentReceipt.external_models.taeh3.sha256 = ('0' * 64) -join ''
        [System.IO.File]::WriteAllText(
            (Join-Path $environment 'environment.json'),
            ($environmentReceipt | ConvertTo-Json -Depth 12) + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )
        $staleAssetMessage = $null
        try {
            & $masterWorkflowGenerator @generatorCommon `
                -SourceA $sourcePaths.T32A `
                -SourceB $sourcePaths.T32B `
                -SourceC $sourcePaths.T32A `
                -SourceD $sourcePaths.T32B `
                -OutputRoot $staleAssetOutput
            throw 'Synthetic stale TAEH3 receipt was unexpectedly accepted.'
        }
        catch {
            $staleAssetMessage = $_.Exception.Message
        }
        Assert-True `
            -Condition ($staleAssetMessage -ceq
                'Selected TAEH3 hash does not match the isolated environment receipt.') `
            -Message "Generator returned an unexpected stale-asset error: $staleAssetMessage"
        Assert-True -Condition (-not (Test-Path -LiteralPath $staleAssetOutput)) `
            -Message 'Generator created workflow output for a stale TAEH3 receipt.'
        $environmentReceipt.external_models.taeh3.sha256 = $taeh3Hash
        [System.IO.File]::WriteAllText(
            (Join-Path $environment 'environment.json'),
            ($environmentReceipt | ConvertTo-Json -Depth 12) + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )

        $mismatchOutput = Join-Path $contractRoot 'mismatch-output'
        $mismatchMessage = $null
        try {
            & $masterWorkflowGenerator @generatorCommon `
                -SourceA $sourcePaths.T72 `
                -SourceB $sourcePaths.T32A `
                -SourceC $sourcePaths.T32B `
                -SourceD $sourcePaths.T107 `
                -OutputRoot $mismatchOutput
            throw 'Synthetic incompatible sources were unexpectedly accepted.'
        }
        catch {
            $mismatchMessage = $_.Exception.Message
        }
        Assert-True `
            -Condition $mismatchMessage.StartsWith(
                'Toolkit D2/Q4 full-clip mismatch:',
                [System.StringComparison]::Ordinal
            ) `
            -Message "Generator returned an unexpected mismatch: $mismatchMessage"
        Assert-True -Condition $mismatchMessage.Contains('profile.visual.latent_slots') `
            -Message 'Generator mismatch did not identify the incompatible temporal signature.'
        Assert-True -Condition (-not (Test-Path -LiteralPath $mismatchOutput)) `
            -Message 'Generator created workflow output before rejecting incompatible sources.'
        Assert-True `
            -Condition (-not (Test-Path -LiteralPath (Join-Path $input 'latentdeck\master'))) `
            -Message 'Generator materialized inputs before rejecting incompatible sources.'

        $compatibleOutput = Join-Path $contractRoot 'compatible-output'
        & $masterWorkflowGenerator @generatorCommon `
            -SourceA $sourcePaths.T32A `
            -SourceB $sourcePaths.T32B `
            -SourceC $sourcePaths.T32A `
            -SourceD $sourcePaths.T32B `
            -AlignSourceA $sourcePaths.T72 `
            -AlignSourceB $sourcePaths.T107 `
            -OutputRoot $compatibleOutput
        $receiptRaw = Get-Content -LiteralPath (Join-Path $compatibleOutput 'receipt.json') -Raw
        $receipt = $receiptRaw |
            ConvertFrom-Json -Depth 30
        Assert-True `
            -Condition ($receipt.full_clip_signature.'profile.visual.latent_slots' -ceq '32') `
            -Message 'Generator receipt omitted the exact same-T full-clip signature.'
        Assert-True `
            -Condition (@($receipt.mixer_sources.PSObject.Properties).Count -eq 4) `
            -Message 'Generator receipt must distinguish four mixer sources.'
        Assert-True `
            -Condition (@($receipt.align_sources.PSObject.Properties).Count -eq 2) `
            -Message 'Generator receipt must distinguish two align sources.'
        Assert-True `
            -Condition ($receipt.mixer_sources.A.archive_sha256 -ceq
                $receipt.mixer_sources.C.archive_sha256) `
            -Message 'Generator rejected or rewrote the intentional A/C duplicate.'
        Assert-True `
            -Condition ($receipt.mixer_sources.B.archive_sha256 -ceq
                $receipt.mixer_sources.D.archive_sha256) `
            -Message 'Generator rejected or rewrote the intentional B/D duplicate.'
        Assert-True `
            -Condition ($receipt.align_sources.A.archive_sha256 -cne
                $receipt.align_sources.B.archive_sha256) `
            -Message 'Generator did not keep the independent mixed-geometry align sources.'
        Assert-True `
            -Condition ($receipt.external_assets.fast_vae.sha256 -ceq $taeh3Hash) `
            -Message 'Generator receipt omitted the exact selected TAEH3 identity.'
        Assert-True `
            -Condition ($receipt.external_assets.hq_vae.sha256 -ceq $hqVaeHash) `
            -Message 'Generator receipt omitted the rehashed native H3 VAE identity.'
        Assert-True -Condition ($receipt.raw_source.sha256 -ceq $rawHash) `
            -Message 'Generator receipt omitted the exact raw H3 source identity.'
        Assert-True -Condition (-not $receiptRaw.Contains($contractRoot)) `
            -Message 'Generator receipt leaked a private absolute path.'
        Assert-True `
            -Condition ($receiptRaw -notmatch '(?i)"(?:[a-z]:[\\/]|\\\\\\\\|/)') `
            -Message 'Generator receipt contains an absolute local path.'

        $expectedWorkflowNames = @(
            '01_LC_INSPECT.private.json',
            '02_FAST_HQ_COMPARE.private.json',
            '03_DUAL_SYNTH_LAB.private.json',
            '04_QUAD_CARRIER_DONORS.private.json',
            '05_PROJECT_RESAMPLE.private.json',
            '06_RAW_RECORD_INSPECT.private.json',
            '07_EXPLICIT_ALIGN_CROP.private.json',
            '99_OPERATOR_DEVELOPER_TEMPLATE.private.json'
        )
        Assert-True `
            -Condition ((@($receipt.workflows) -join '|') -ceq
                ($expectedWorkflowNames -join '|')) `
            -Message 'Generator receipt omitted or reordered a master-user workflow.'
        $generatedWorkflows = @{}
        foreach ($workflowName in $expectedWorkflowNames) {
            $workflowRaw = Get-Content -LiteralPath (Join-Path $compatibleOutput $workflowName) -Raw
            Assert-True `
                -Condition ($workflowRaw -cnotmatch 'REPLACE_WITH_[A-Z0-9_]+') `
                -Message "$workflowName retained an unresolved queue placeholder."
            Assert-True -Condition (-not $workflowRaw.Contains($contractRoot)) `
                -Message "$workflowName leaked a private absolute path."
            Assert-True `
                -Condition ($workflowRaw -notmatch '(?i)"(?:[a-z]:[\\/]|\\\\\\\\|/)') `
                -Message "$workflowName contains a machine-specific absolute path."
            $generatedWorkflows[$workflowName] = $workflowRaw | ConvertFrom-Json -Depth 100
        }

        $expectedLcSelections = @{
            '01_LC_INSPECT.private.json' = @($receipt.mixer_sources.A.input_selection)
            '02_FAST_HQ_COMPARE.private.json' = @($receipt.mixer_sources.A.input_selection)
            '03_DUAL_SYNTH_LAB.private.json' = @(
                $receipt.mixer_sources.A.input_selection,
                $receipt.mixer_sources.B.input_selection
            )
            '04_QUAD_CARRIER_DONORS.private.json' = @(
                $receipt.mixer_sources.A.input_selection,
                $receipt.mixer_sources.B.input_selection,
                $receipt.mixer_sources.C.input_selection,
                $receipt.mixer_sources.D.input_selection
            )
            '05_PROJECT_RESAMPLE.private.json' = @($receipt.mixer_sources.A.input_selection)
            '07_EXPLICIT_ALIGN_CROP.private.json' = @(
                $receipt.align_sources.A.input_selection,
                $receipt.align_sources.B.input_selection
            )
            '99_OPERATOR_DEVELOPER_TEMPLATE.private.json' = @(
                $receipt.mixer_sources.A.input_selection,
                $receipt.mixer_sources.B.input_selection
            )
        }
        foreach ($entry in $expectedLcSelections.GetEnumerator()) {
            $actualSelections = @(
                $generatedWorkflows[$entry.Key].nodes |
                    Where-Object { $_.type -ceq 'LatentDeckToolkitLCLoadInspect' } |
                    Sort-Object id |
                    ForEach-Object { [string]$_.widgets_values[0] }
            )
            Assert-True `
                -Condition (($actualSelections -join '|') -ceq (@($entry.Value) -join '|')) `
                -Message "$($entry.Key) has incorrect private LC selections."
        }

        $rawWorkflow = $generatedWorkflows['06_RAW_RECORD_INSPECT.private.json']
        $rawImporters = @(
            $rawWorkflow.nodes | Where-Object { $_.type -ceq 'LatentDeckToolkitRawH3Import' }
        )
        Assert-True `
            -Condition ($rawImporters.Count -eq 1 -and
                [string]$rawImporters[0].widgets_values[0] -ceq
                [string]$receipt.raw_source.input_selection) `
            -Message 'Raw Recorder workflow did not select the validated raw H3 source.'

        foreach ($workflowName in @(
            '02_FAST_HQ_COMPARE.private.json',
            '03_DUAL_SYNTH_LAB.private.json',
            '04_QUAD_CARRIER_DONORS.private.json',
            '05_PROJECT_RESAMPLE.private.json'
        )) {
            $workflow = $generatedWorkflows[$workflowName]
            $declarations = @(
                $workflow.nodes |
                    Where-Object { $_.type -ceq 'LatentDeckToolkitDeclareH3Vae' }
            )
            $vaeLoaders = @($workflow.nodes | Where-Object { $_.type -ceq 'VAELoader' })
            $fastDeclarations = @(
                $declarations | Where-Object { [string]$_.widgets_values[0] -ceq 'FAST' }
            )
            Assert-True -Condition ($fastDeclarations.Count -eq 1) `
                -Message "$workflowName must contain one declared FAST VAE."
            Assert-True -Condition ($fastDeclarations[0].widgets_values[3] -ceq
                'https://example.invalid/taeh3.safetensors') `
                -Message "$workflowName omitted the selected TAEH3 source."
            Assert-True -Condition ($fastDeclarations[0].widgets_values[4] -ceq
                'TEST-ONLY (https://example.invalid/LICENSE)') `
                -Message "$workflowName omitted the selected TAEH3 license."
            Assert-True -Condition ($fastDeclarations[0].widgets_values[5] -ceq $taeh3Hash) `
                -Message "$workflowName omitted the selected TAEH3 SHA-256."
            Assert-True -Condition ('taeh3.safetensors' -cin
                @($vaeLoaders | ForEach-Object { [string]$_.widgets_values[0] })) `
                -Message "$workflowName did not select the environment TAEH3 model."
            if ($workflowName -in @(
                '02_FAST_HQ_COMPARE.private.json',
                '05_PROJECT_RESAMPLE.private.json'
            )) {
                $hqDeclarations = @(
                    $declarations | Where-Object { [string]$_.widgets_values[0] -ceq 'HQ' }
                )
                Assert-True -Condition ($hqDeclarations.Count -eq 1) `
                    -Message "$workflowName must contain one declared native H3 VAE."
                Assert-True -Condition ($hqDeclarations[0].widgets_values[3] -ceq
                    'https://example.invalid/native-h3-vae.safetensors') `
                    -Message "$workflowName omitted native H3 VAE source provenance."
                Assert-True -Condition ($hqDeclarations[0].widgets_values[4] -ceq 'TEST-ONLY') `
                    -Message "$workflowName omitted native H3 VAE license provenance."
                Assert-True -Condition ($hqDeclarations[0].widgets_values[5] -ceq $hqVaeHash) `
                    -Message "$workflowName omitted the rehashed native H3 VAE SHA-256."
                Assert-True -Condition ('minimax-h3-native-vae.safetensors' -cin
                    @($vaeLoaders | ForEach-Object { [string]$_.widgets_values[0] })) `
                    -Message "$workflowName did not select the isolated native H3 VAE."
            }
        }

        $developerWorkflow = $generatedWorkflows['99_OPERATOR_DEVELOPER_TEMPLATE.private.json']
        $developerTransfers = @(
            $developerWorkflow.nodes |
                Where-Object { $_.type -ceq 'LatentDeckToolkitExplicitDeviceTransfer' }
        )
        Assert-True -Condition ($developerTransfers.Count -eq 2) `
            -Message 'Developer workflow must retain explicit carrier and donor device paths.'
        foreach ($transfer in $developerTransfers) {
            Assert-True `
                -Condition ((@($transfer.widgets_values) -join '|') -ceq
                    'CUDA|0|FALLBACK_TO_CPU') `
                -Message 'Developer workflow device path is not explicit CUDA:0 with CPU fallback.'
        }
    }
    finally {
        if (Test-Path -LiteralPath $contractRoot) {
            Assert-ChildPath `
                -Root $artifactsRoot `
                -Candidate $contractRoot `
                -Label 'generator contract cleanup'
            Remove-Item -LiteralPath $contractRoot -Recurse -Force
        }
    }
}

Test-MasterWorkflowGeneratorContract

if ($ContractOnly) {
    Write-Host 'ISOLATED COMFY ENVIRONMENT SCRIPT CONTRACT: PASS' -ForegroundColor Green
    return
}

if ([string]::IsNullOrWhiteSpace($EnvironmentRoot)) {
    $EnvironmentRoot = Join-Path $artifactsRoot 'comfy-test'
}
$environmentFull = [System.IO.Path]::GetFullPath($EnvironmentRoot)
Assert-ChildPath -Root $artifactsRoot -Candidate $environmentFull -Label 'EnvironmentRoot'

$receiptPath = Join-Path $environmentFull 'environment.json'
Assert-True -Condition (Test-Path -LiteralPath $receiptPath -PathType Leaf) `
    -Message "Environment receipt is missing. Run $initializer first."
$receipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json -Depth 20

Assert-True -Condition ($receipt.schema_version -eq 1) `
    -Message 'Unsupported isolated Comfy environment receipt schema.'
Assert-True -Condition ($receipt.environment_root -ceq $environmentFull) `
    -Message 'Receipt environment_root does not match the requested environment.'
Assert-True -Condition ($receipt.source_policy -ceq 'read_only_external_models') `
    -Message 'Receipt does not declare the read-only external model policy.'
Assert-True -Condition ($receipt.private_artifact -eq $true) `
    -Message 'Receipt must identify the generated environment as a private artifact.'

foreach ($property in @(
    'base_directory',
    'python_packages',
    'custom_nodes',
    'input',
    'output',
    'user'
)) {
    $value = [string]$receipt.paths.$property
    Assert-ChildPath -Root $environmentFull -Candidate $value -Label "paths.$property"
    Assert-True -Condition (Test-Path -LiteralPath $value -PathType Container) `
        -Message "Prepared directory is missing: paths.$property"
}
Restore-GeneratedTempDirectory `
    -Environment $environmentFull `
    -BaseDirectory ([string]$receipt.paths.base_directory) `
    -TempDirectory ([string]$receipt.paths.temp)
$databasePath = [string]$receipt.paths.database
Assert-ChildPath -Root ([string]$receipt.paths.user) -Candidate $databasePath `
    -Label 'paths.database'
Assert-True `
    -Condition ([System.IO.Path]::GetFileName($databasePath) -ceq 'comfyui.db') `
    -Message 'Isolated database path must be user/comfyui.db.'

$customNodeDirectories = @(
    Get-ChildItem -LiteralPath ([string]$receipt.paths.custom_nodes) -Directory -Force
)
$expectedCustomNodeDirectories = @(
    'comfyui_latent_cartridge',
    'latentdeck_example_channel_roll',
    'latentdeck_toolkit'
)
Assert-True -Condition ($customNodeDirectories.Count -eq $expectedCustomNodeDirectories.Count) `
    -Message 'Isolated custom_nodes contains an unexpected directory.'
foreach ($name in $expectedCustomNodeDirectories) {
    $shimPath = Join-Path ([string]$receipt.paths.custom_nodes) "$name\__init__.py"
    Assert-True -Condition (Test-Path -LiteralPath $shimPath -PathType Leaf) `
        -Message "Required isolated custom-node shim is missing: $name"
}

$privatePayloadExtensions = @(
    '.safetensors',
    '.lc',
    '.h3latent',
    '.ckpt',
    '.pt',
    '.onnx',
    '.engine'
)
$distributionSurfaces = @(
    [string]$receipt.paths.python_packages,
    [string]$receipt.paths.wheels,
    [string]$receipt.paths.custom_nodes
)
$copiedPayloads = @(
    foreach ($surface in $distributionSurfaces) {
        Get-ChildItem -LiteralPath $surface -Recurse -File |
            Where-Object { $privatePayloadExtensions -ccontains $_.Extension.ToLowerInvariant() }
    }
)
if ($copiedPayloads.Count -ne 0) {
    throw "Private/model payloads entered an install surface: $($copiedPayloads.FullName -join ', ')"
}

Assert-True -Condition ([int]$receipt.port -ge 1024 -and [int]$receipt.port -le 65535) `
    -Message 'Receipt port is outside the allowed user-port range.'
Assert-True -Condition ($receipt.packages.install_mode -ceq 'wheel_target_no_deps') `
    -Message 'Repository packages were not installed with the required target/no-deps policy.'

$requiredPackages = @(
    'latentdeck-cartridge',
    'latentdeck-codec-sdk',
    'latentdeck-deck-sdk',
    'latentdeck-codec-host',
    'latentdeck-operator-d2',
    'latentdeck-operator-q4',
    'latentdeck-comfy-toolkit',
    'latentdeck-comfy-cartridge',
    'latentdeck-example-channel-roll'
)
$actualPackages = @($receipt.packages.wheels | ForEach-Object { [string]$_.project })
foreach ($package in $requiredPackages) {
    Assert-True -Condition ($actualPackages -ccontains $package) `
        -Message "Receipt is missing the required wheel: $package"
}

$bootstrapPath = [string]$receipt.paths.bootstrap
$smokePath = [string]$receipt.paths.smoke
$modelConfigPath = [string]$receipt.paths.extra_model_paths
foreach ($path in @($bootstrapPath, $smokePath, $modelConfigPath)) {
    Assert-ChildPath -Root $environmentFull -Candidate $path -Label 'generated runtime file'
    Assert-True -Condition (Test-Path -LiteralPath $path -PathType Leaf) `
        -Message "Generated runtime file is missing: $path"
}
$modelConfigSource = Get-Content -LiteralPath $modelConfigPath -Raw
Assert-True -Condition ($modelConfigSource -notmatch '(?im)^\s*custom_nodes\s*:') `
    -Message 'Generated model config must never expose source custom_nodes.'

$python = [string]$receipt.python.executable
Assert-True -Condition (Test-Path -LiteralPath $python -PathType Leaf) `
    -Message "Embedded Python executable is missing: $python"

$oldBytecode = $env:PYTHONDONTWRITEBYTECODE
try {
    $env:PYTHONDONTWRITEBYTECODE = '1'
    $helpOutput = & $python -B $bootstrapPath --help 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Generated Comfy bootstrap --help failed with exit code $LASTEXITCODE.`n$($helpOutput -join "`n")"
    }
    $helpText = $helpOutput -join "`n"
    foreach ($requiredArgument in @(
        '--base-directory',
        '--extra-model-paths-config',
        '--whitelist-custom-nodes'
    )) {
        Assert-True -Condition $helpText.Contains($requiredArgument) `
            -Message "ComfyUI parser help omitted required argument $requiredArgument."
    }

    $output = & $python -B $smokePath 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Isolated Comfy smoke failed with exit code $LASTEXITCODE.`n$($output -join "`n")"
    }
}
finally {
    $env:PYTHONDONTWRITEBYTECODE = $oldBytecode
}

$sentinel = @($output | Where-Object { "$_".StartsWith('LATENTDECK_SMOKE_JSON=') })
Assert-True -Condition ($sentinel.Count -eq 1) `
    -Message "Smoke output did not contain exactly one JSON result.`n$($output -join "`n")"
$result = $sentinel[0].Substring('LATENTDECK_SMOKE_JSON='.Length) |
    ConvertFrom-Json -Depth 20

Assert-True -Condition ($result.status -ceq 'ok') -Message 'Smoke result was not successful.'
Assert-True -Condition ($result.python.version -match '^3\.13\.') `
    -Message "Expected embedded Python 3.13, got $($result.python.version)."
Assert-True -Condition ($result.torch.version -match '^2\.13\.0\+cu130$') `
    -Message "Expected torch 2.13.0+cu130, got $($result.torch.version)."
Assert-True -Condition ($result.torch.cuda_build -ceq '13.0') `
    -Message "Expected CUDA 13.0 torch build, got $($result.torch.cuda_build)."
Assert-True -Condition ($result.safetensors.version -ceq '0.8.0') `
    -Message "Expected safetensors 0.8.0, got $($result.safetensors.version)."
Assert-True -Condition ($result.models.taeh3.status -ceq 'verified') `
    -Message 'TAEH3 was not found and hash-verified through the external model path.'
Assert-True -Condition ($result.models.hq_h3_vae.status -ceq 'available') `
    -Message 'Native H3 HQ VAE was not found through the external model path.'
Assert-True -Condition ($result.discovery.recorder -eq $true) `
    -Message 'ComfyUI did not discover the LatentCartridge recorder node.'
Assert-True -Condition ($result.discovery.example -eq $true) `
    -Message 'ComfyUI did not discover the external operator example node.'
Assert-True -Condition ($result.discovery.example_hook -eq $true) `
    -Message 'ComfyUI did not discover the external operator hook example node.'
$expectedExampleNodes = @(
    'LatentDeckExampleChannelRoll',
    'LatentDeckExampleChannelRollHook'
)
$actualExampleNodes = @($result.discovery.example_nodes)
Assert-True -Condition ($actualExampleNodes.Count -eq $expectedExampleNodes.Count) `
    -Message 'External example discovery exported an unexpected node count.'
foreach ($nodeName in $expectedExampleNodes) {
    Assert-True -Condition ($actualExampleNodes -ccontains $nodeName) `
        -Message "External example discovery omitted $nodeName."
}
Assert-True -Condition ([int]$result.discovery.toolkit_node_count -gt 0) `
    -Message 'ComfyUI did not discover Toolkit nodes.'

if ($ServerSmoke) {
    $serverPort = if ($Port -eq 0) { [int]$receipt.port } else { $Port }
    Assert-True -Condition ($serverPort -ge 1024 -and $serverPort -le 65535) `
        -Message 'Server smoke requires a user port from 1024 through 65535.'
    $existingListeners = @(
        Get-NetTCPConnection -State Listen -LocalPort $serverPort -ErrorAction SilentlyContinue
    )
    Assert-True -Condition ($existingListeners.Count -eq 0) `
        -Message "Server smoke port $serverPort is already in use."

    $serverArguments = @(
        '-B',
        $bootstrapPath,
        '--base-directory', [string]$receipt.paths.base_directory,
        '--database-url', ('sqlite:///' + ([string]$receipt.paths.database).Replace('\', '/')),
        '--extra-model-paths-config', $modelConfigPath,
        '--listen', '127.0.0.1',
        '--port', "$serverPort",
        '--disable-auto-launch',
        '--disable-api-nodes',
        '--disable-all-custom-nodes',
        '--whitelist-custom-nodes', 'latentdeck_toolkit', 'comfyui_latent_cartridge', 'latentdeck_example_channel_roll',
        '--cpu',
        '--log-stdout'
    )
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $python
    $startInfo.WorkingDirectory = [string]$receipt.paths.base_directory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.WindowStyle = [System.Diagnostics.ProcessWindowStyle]::Hidden
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.Environment['PYTHONDONTWRITEBYTECODE'] = '1'
    $startInfo.Environment['CUDA_VISIBLE_DEVICES'] = '-1'
    foreach ($argument in $serverArguments) {
        $startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    $sourceLegacyDatabase = Join-Path ([string]$receipt.comfy.root) 'user\comfyui.db'
    $sourceLegacyBackup = "$sourceLegacyDatabase.bak"
    $sourceDatabaseExisted = Test-Path -LiteralPath $sourceLegacyDatabase -PathType Leaf
    $sourceDatabaseHash = if ($sourceDatabaseExisted) {
        (Get-FileHash -LiteralPath $sourceLegacyDatabase -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    else {
        $null
    }
    $sourceBackupExisted = Test-Path -LiteralPath $sourceLegacyBackup -PathType Leaf
    $stdoutTask = $null
    $stderrTask = $null
    $serverResult = $null
    try {
        Assert-True -Condition $process.Start() -Message 'Failed to start isolated Comfy server.'
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        $deadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
        $systemStats = $null
        $objectInfo = $null
        $lastApiError = $null
        while ([DateTime]::UtcNow -lt $deadline) {
            if ($process.HasExited) {
                break
            }
            try {
                $systemStats = Invoke-RestMethod `
                    -Uri "http://127.0.0.1:$serverPort/system_stats" `
                    -TimeoutSec 5
                $objectInfo = Invoke-RestMethod `
                    -Uri "http://127.0.0.1:$serverPort/object_info" `
                    -TimeoutSec 10
                break
            }
            catch {
                $lastApiError = $_.Exception.Message
                Start-Sleep -Milliseconds 500
            }
        }

        if ($null -eq $systemStats -or $null -eq $objectInfo) {
            $exitDescription = if ($process.HasExited) {
                "process exited with code $($process.ExitCode)"
            }
            else {
                "timeout after $StartupTimeoutSeconds seconds"
            }
            throw "Isolated Comfy API was not ready ($exitDescription). Last API error: $lastApiError"
        }

        $apiNodeNames = @($objectInfo.PSObject.Properties.Name)
        $expectedApiNodes = @($result.discovery.toolkit_nodes) + @(
            'LatentDeckSaveLatentCartridge'
        ) + @($result.discovery.example_nodes)
        $missingApiNodes = @(
            $expectedApiNodes | Where-Object { $apiNodeNames -cnotcontains $_ }
        )
        if ($missingApiNodes.Count -ne 0) {
            throw "Running Comfy /object_info omitted LatentDeck nodes: $($missingApiNodes -join ', ')"
        }

        $latentDeckObjectInfo = [ordered]@{}
        foreach ($nodeName in $expectedApiNodes) {
            $latentDeckObjectInfo[$nodeName] = $objectInfo.PSObject.Properties[$nodeName].Value
        }
        [System.IO.File]::WriteAllText(
            (Join-Path $environmentFull 'server-object-info.json'),
            ($latentDeckObjectInfo | ConvertTo-Json -Depth 30) + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )

        $serverResult = [ordered]@{
            schema_version = 1
            status = 'ok'
            checked_utc = [DateTime]::UtcNow.ToString('o')
            url = "http://127.0.0.1:$serverPort"
            port = $serverPort
            launch_mode = 'cpu_no_cuda_visible_devices'
            process_id = $process.Id
            endpoints = @('/system_stats', '/object_info')
            toolkit_node_count = [int]$result.discovery.toolkit_node_count
            recorder_node = 'LatentDeckSaveLatentCartridge'
            example_nodes = @($result.discovery.example_nodes)
            api_node_count = $apiNodeNames.Count
            base_directory = [string]$receipt.paths.base_directory
        }
        $serverResultPath = Join-Path $environmentFull 'server-smoke.json'
        [System.IO.File]::WriteAllText(
            $serverResultPath,
            ($serverResult | ConvertTo-Json -Depth 8) + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )
    }
    finally {
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id -Force
        }
        [void]$process.WaitForExit(10000)
        $stdout = if ($null -eq $stdoutTask) { '' } else { $stdoutTask.GetAwaiter().GetResult() }
        $stderr = if ($null -eq $stderrTask) { '' } else { $stderrTask.GetAwaiter().GetResult() }
        [System.IO.File]::WriteAllText(
            (Join-Path $environmentFull 'server-smoke.stdout.log'),
            $stdout,
            [System.Text.UTF8Encoding]::new($false)
        )
        [System.IO.File]::WriteAllText(
            (Join-Path $environmentFull 'server-smoke.stderr.log'),
            $stderr,
            [System.Text.UTF8Encoding]::new($false)
        )
        $process.Dispose()
    }

    $portReleaseDeadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $remainingListeners = @(
            Get-NetTCPConnection -State Listen -LocalPort $serverPort -ErrorAction SilentlyContinue
        )
        if ($remainingListeners.Count -eq 0) {
            break
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $portReleaseDeadline)
    Assert-True -Condition ($remainingListeners.Count -eq 0) `
        -Message "Isolated Comfy server did not release port $serverPort after shutdown."
    Assert-True `
        -Condition ((Test-Path -LiteralPath $sourceLegacyDatabase -PathType Leaf) -eq
            $sourceDatabaseExisted) `
        -Message 'Server smoke changed the existence of the source Comfy legacy database.'
    if ($sourceDatabaseExisted) {
        $sourceDatabaseHashAfter = (
            Get-FileHash -LiteralPath $sourceLegacyDatabase -Algorithm SHA256
        ).Hash.ToLowerInvariant()
        Assert-True -Condition ($sourceDatabaseHashAfter -ceq $sourceDatabaseHash) `
            -Message 'Server smoke modified the source Comfy legacy database.'
    }
    Assert-True `
        -Condition ((Test-Path -LiteralPath $sourceLegacyBackup -PathType Leaf) -eq
            $sourceBackupExisted) `
        -Message 'Server smoke changed the source Comfy legacy database backup state.'
    Assert-True -Condition ($null -ne $serverResult -and $serverResult.status -ceq 'ok') `
        -Message 'Isolated Comfy server smoke did not produce a successful result.'
    Restore-GeneratedTempDirectory `
        -Environment $environmentFull `
        -BaseDirectory ([string]$receipt.paths.base_directory) `
        -TempDirectory ([string]$receipt.paths.temp)

    Write-Host "ISOLATED COMFY SERVER/API SMOKE: PASS ($($serverResult.url))" `
        -ForegroundColor Green
}

Write-Host 'ISOLATED COMFY ENVIRONMENT SMOKE: PASS' -ForegroundColor Green
Write-Host "Environment: $environmentFull"
Write-Host "Toolkit nodes: $($result.discovery.toolkit_node_count)"
Write-Host "Torch: $($result.torch.version) / CUDA build $($result.torch.cuda_build)"
Write-Host "TAEH3: $($result.models.taeh3.path)"
Write-Host "HQ H3 VAE: $($result.models.hq_h3_vae.path)"
