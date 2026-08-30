[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$RawLatentPath,

    [string]$EnvironmentRoot,

    [string]$CartridgeCli,

    [string]$ReceiptRoot,

    [ValidateRange(0, 65535)]
    [int]$Port = 0,

    [ValidateRange(10, 180)]
    [int]$StartupTimeoutSeconds = 90,

    [ValidateRange(30, 900)]
    [int]$ExecutionTimeoutSeconds = 300
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'artifacts'))

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

function Convert-CommandJson {
    param(
        [Parameter(Mandatory)]
        [string]$Label,
        [Parameter(Mandatory)]
        [scriptblock]$Command
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

if ([string]::IsNullOrWhiteSpace($EnvironmentRoot)) {
    $EnvironmentRoot = Join-Path $artifactsRoot 'comfy-test'
}
$environmentFull = [System.IO.Path]::GetFullPath($EnvironmentRoot)
Assert-ChildPath -Root $artifactsRoot -Candidate $environmentFull -Label 'EnvironmentRoot'

$environmentReceiptPath = Join-Path $environmentFull 'environment.json'
Assert-True `
    -Condition (Test-Path -LiteralPath $environmentReceiptPath -PathType Leaf) `
    -Message 'Isolated Comfy environment receipt is missing.'
$environmentReceipt = Get-Content -LiteralPath $environmentReceiptPath -Raw |
    ConvertFrom-Json -Depth 30
Assert-True -Condition ($environmentReceipt.schema_version -eq 1) `
    -Message 'Unsupported isolated Comfy environment receipt schema.'
Assert-True -Condition ($environmentReceipt.private_artifact -eq $true) `
    -Message 'Recorder E2E must run in the private isolated Comfy environment.'
Assert-True -Condition ($environmentReceipt.environment_root -ceq $environmentFull) `
    -Message 'Environment receipt root does not match EnvironmentRoot.'

$sourcePath = (Resolve-Path -LiteralPath $RawLatentPath).Path
Assert-True -Condition (Test-Path -LiteralPath $sourcePath -PathType Leaf) `
    -Message 'RawLatentPath must be an existing file.'
Assert-True `
    -Condition ([System.IO.Path]::GetExtension($sourcePath) -ceq '.safetensors') `
    -Message 'RawLatentPath must be a .safetensors file.'
$sourceItem = Get-Item -LiteralPath $sourcePath
$sourceHashBefore = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()

if ([string]::IsNullOrWhiteSpace($CartridgeCli)) {
    $releaseCli = Join-Path $repoRoot 'target\release\latentdeck-cartridge.exe'
    $debugCli = Join-Path $repoRoot 'target\debug\latentdeck-cartridge.exe'
    if (Test-Path -LiteralPath $releaseCli -PathType Leaf) {
        $CartridgeCli = $releaseCli
    }
    elseif (Test-Path -LiteralPath $debugCli -PathType Leaf) {
        $CartridgeCli = $debugCli
    }
    else {
        throw 'Build latentdeck-cartridge before the Recorder E2E or pass -CartridgeCli.'
    }
}
$cartridgeCliFull = (Resolve-Path -LiteralPath $CartridgeCli).Path
Assert-True -Condition (Test-Path -LiteralPath $cartridgeCliFull -PathType Leaf) `
    -Message 'CartridgeCli must be an existing file.'
$cartridgeCliHash = (
    Get-FileHash -LiteralPath $cartridgeCliFull -Algorithm SHA256
).Hash.ToLowerInvariant()

if ([string]::IsNullOrWhiteSpace($ReceiptRoot)) {
    $stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
    $ReceiptRoot = Join-Path $artifactsRoot "private-comfy-recorder-$stamp"
}
$receiptFull = [System.IO.Path]::GetFullPath($ReceiptRoot)
Assert-ChildPath -Root $artifactsRoot -Candidate $receiptFull -Label 'ReceiptRoot'
Assert-True -Condition (-not (Test-Path -LiteralPath $receiptFull)) `
    -Message 'ReceiptRoot already exists; Recorder E2E evidence is no-clobber.'
[System.IO.Directory]::CreateDirectory($receiptFull) | Out-Null

$python = [string]$environmentReceipt.python.executable
$bootstrap = [string]$environmentReceipt.paths.bootstrap
$baseDirectory = [string]$environmentReceipt.paths.base_directory
$database = [string]$environmentReceipt.paths.database
$modelConfig = [string]$environmentReceipt.paths.extra_model_paths
$pythonPackages = [string]$environmentReceipt.paths.python_packages
$inputRoot = [string]$environmentReceipt.paths.input
$outputRoot = [string]$environmentReceipt.paths.output
foreach ($requiredPath in @($python, $bootstrap, $modelConfig)) {
    Assert-True -Condition (Test-Path -LiteralPath $requiredPath -PathType Leaf) `
        -Message "Required isolated runtime file is missing: $requiredPath"
}
foreach ($requiredDirectory in @($baseDirectory, $pythonPackages, $inputRoot, $outputRoot)) {
    Assert-True -Condition (Test-Path -LiteralPath $requiredDirectory -PathType Container) `
        -Message "Required isolated runtime directory is missing: $requiredDirectory"
}
$pythonVersionOutput = @(& $python -B --version 2>&1)
Assert-True -Condition ($LASTEXITCODE -eq 0 -and $pythonVersionOutput.Count -eq 1) `
    -Message 'Could not resolve the isolated Python version.'
$pythonVersion = "$($pythonVersionOutput[0])".Trim()

$inspectRawCode = @'
import json
import sys
sys.path.insert(0, sys.argv[1])
import latentdeck_cartridge
print(json.dumps(latentdeck_cartridge.inspect_raw_h3(sys.argv[2]), allow_nan=False, sort_keys=True))
'@
$rawInspection = Convert-CommandJson -Label 'Python SDK raw H3 inspection' -Command {
    & $python -B -c $inspectRawCode $pythonPackages $sourcePath
}
Assert-True -Condition ($rawInspection.status -ceq 'ok') `
    -Message 'Python SDK did not accept the raw H3 source.'
Assert-True -Condition ($rawInspection.sha256 -ceq $sourceHashBefore) `
    -Message 'Python SDK raw H3 hash disagrees with the source file hash.'

$serverPort = if ($Port -eq 0) { [int]$environmentReceipt.port } else { $Port }
Assert-True -Condition ($serverPort -ge 1024 -and $serverPort -le 65535) `
    -Message 'Recorder E2E requires a user port from 1024 through 65535.'
$listeners = @(Get-NetTCPConnection -State Listen -LocalPort $serverPort -ErrorAction SilentlyContinue)
Assert-True -Condition ($listeners.Count -eq 0) `
    -Message "Recorder E2E port $serverPort is already in use."

$cartridgeOutputDirectory = Join-Path $outputRoot 'latentdeck\cartridges'
$beforeOutputs = @{}
if (Test-Path -LiteralPath $cartridgeOutputDirectory -PathType Container) {
    foreach ($file in Get-ChildItem -LiteralPath $cartridgeOutputDirectory -File -Force) {
        $beforeOutputs[$file.FullName] = $true
    }
}
$prefix = 'recorder-e2e-' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' +
    [Guid]::NewGuid().ToString('N').Substring(0, 8)

$serverArguments = @(
    '-B',
    $bootstrap,
    '--base-directory', $baseDirectory,
    '--database-url', ('sqlite:///' + $database.Replace('\', '/')),
    '--extra-model-paths-config', $modelConfig,
    '--listen', '127.0.0.1',
    '--port', "$serverPort",
    '--disable-auto-launch',
    '--disable-api-nodes',
    '--disable-all-custom-nodes',
    '--whitelist-custom-nodes', 'latentdeck_toolkit', 'comfyui_latent_cartridge',
    '--cpu',
    '--log-stdout'
)
$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = $python
$startInfo.WorkingDirectory = $baseDirectory
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
$stdoutTask = $null
$stderrTask = $null
$promptId = $null
$history = $null
$newOutput = $null
$inputRelative = 'latentdeck/e2e/' + [Guid]::NewGuid().ToString('N') + '.safetensors'
$inputCopy = Join-Path $inputRoot $inputRelative
Assert-ChildPath -Root $inputRoot -Candidate $inputCopy -Label 'temporary raw input copy'
[System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($inputCopy)) | Out-Null
Copy-Item -LiteralPath $sourcePath -Destination $inputCopy
Assert-True `
    -Condition ((Get-FileHash -LiteralPath $inputCopy -Algorithm SHA256).Hash.ToLowerInvariant() -ceq
        $sourceHashBefore) `
    -Message 'Temporary Comfy input copy disagrees with the raw source hash.'
try {
    Assert-True -Condition $process.Start() -Message 'Failed to start isolated Comfy server.'
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()

    $startupDeadline = [DateTime]::UtcNow.AddSeconds($StartupTimeoutSeconds)
    $objectInfo = $null
    $lastStartupError = $null
    while ([DateTime]::UtcNow -lt $startupDeadline) {
        if ($process.HasExited) {
            break
        }
        try {
            $objectInfo = Invoke-RestMethod `
                -Uri "http://127.0.0.1:$serverPort/object_info" `
                -TimeoutSec 10
            break
        }
        catch {
            $lastStartupError = $_.Exception.Message
            Start-Sleep -Milliseconds 500
        }
    }
    if ($null -eq $objectInfo) {
        throw "Isolated Comfy API did not start. Last API error: $lastStartupError"
    }
    foreach ($nodeName in @(
        'LatentDeckToolkitRawH3Import',
        'LatentDeckSaveLatentCartridge',
        'LatentDeckToolkitLatentScopes'
    )) {
        Assert-True `
            -Condition ($objectInfo.PSObject.Properties.Name -ccontains $nodeName) `
            -Message "Running Comfy omitted required Recorder E2E node $nodeName."
    }

    $graph = [ordered]@{
        '1' = [ordered]@{
            class_type = 'LatentDeckToolkitRawH3Import'
            inputs = [ordered]@{ safetensors_file = $inputRelative.Replace('\', '/') }
        }
        '2' = [ordered]@{
            class_type = 'LatentDeckSaveLatentCartridge'
            inputs = [ordered]@{
                latent = @('1', 0)
                filename_prefix = $prefix
            }
        }
        '3' = [ordered]@{
            class_type = 'LatentDeckToolkitLatentScopes'
            inputs = [ordered]@{ latent = @('2', 0) }
        }
    }
    $request = [ordered]@{
        client_id = [Guid]::NewGuid().ToString('D')
        prompt = $graph
    }
    $queueResponse = Invoke-RestMethod `
        -Method Post `
        -Uri "http://127.0.0.1:$serverPort/prompt" `
        -ContentType 'application/json' `
        -Body ($request | ConvertTo-Json -Depth 20 -Compress) `
        -TimeoutSec 30
    $promptId = [string]$queueResponse.prompt_id
    Assert-True -Condition (-not [string]::IsNullOrWhiteSpace($promptId)) `
        -Message 'Comfy did not return a prompt ID.'
    $nodeErrors = @($queueResponse.node_errors.PSObject.Properties)
    Assert-True -Condition ($nodeErrors.Count -eq 0) `
        -Message 'Comfy rejected the Recorder E2E graph during validation.'

    $executionDeadline = [DateTime]::UtcNow.AddSeconds($ExecutionTimeoutSeconds)
    while ([DateTime]::UtcNow -lt $executionDeadline) {
        if ($process.HasExited) {
            throw "Isolated Comfy exited during Recorder E2E with code $($process.ExitCode)."
        }
        $historyEnvelope = Invoke-RestMethod `
            -Uri "http://127.0.0.1:$serverPort/history/$promptId" `
            -TimeoutSec 10
        $entryProperty = $historyEnvelope.PSObject.Properties[$promptId]
        if ($null -ne $entryProperty) {
            $history = $entryProperty.Value
            break
        }
        Start-Sleep -Milliseconds 500
    }
    Assert-True -Condition ($null -ne $history) `
        -Message "Recorder E2E did not finish within $ExecutionTimeoutSeconds seconds."
    Assert-True -Condition ($history.status.status_str -ceq 'success') `
        -Message "Recorder E2E execution status was $($history.status.status_str)."
    Assert-True -Condition ($history.status.completed -eq $true) `
        -Message 'Recorder E2E history did not mark the graph complete.'

    $afterFiles = if (Test-Path -LiteralPath $cartridgeOutputDirectory -PathType Container) {
        @(Get-ChildItem -LiteralPath $cartridgeOutputDirectory -File -Force)
    }
    else {
        @()
    }
    $newFiles = @($afterFiles | Where-Object { -not $beforeOutputs.ContainsKey($_.FullName) })
    $newCartridges = @($newFiles | Where-Object { $_.Extension -ceq '.lc' })
    Assert-True -Condition ($newCartridges.Count -eq 1) `
        -Message 'Recorder E2E must create exactly one new .lc file.'
    Assert-True -Condition ($newFiles.Count -eq 1) `
        -Message 'Recorder E2E left an unexpected non-cartridge output file.'
    $newOutput = $newCartridges[0]
    Assert-True `
        -Condition ($newOutput.BaseName -match ('^' + [regex]::Escape($prefix) +
            '_[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$')) `
        -Message 'Recorder output name does not contain the requested prefix and canonical UUID.'

    $residue = @(
        Get-ChildItem -LiteralPath $cartridgeOutputDirectory -File -Force |
            Where-Object {
                $_.Name.StartsWith(".$prefix", [System.StringComparison]::Ordinal) -or
                $_.Name.Contains('.partial', [System.StringComparison]::Ordinal)
            }
    )
    Assert-True -Condition ($residue.Count -eq 0) `
        -Message 'Recorder E2E left a temporary Safetensors or .partial file.'
}
finally {
    if ($null -ne $process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
    }
    if ($null -ne $process) {
        [void]$process.WaitForExit(10000)
    }
    $stdout = if ($null -eq $stdoutTask) { '' } else { $stdoutTask.GetAwaiter().GetResult() }
    $stderr = if ($null -eq $stderrTask) { '' } else { $stderrTask.GetAwaiter().GetResult() }
    [System.IO.File]::WriteAllText(
        (Join-Path $receiptFull 'server.stdout.log'),
        $stdout,
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $receiptFull 'server.stderr.log'),
        $stderr,
        [System.Text.UTF8Encoding]::new($false)
    )
    if ($null -ne $process) {
        $process.Dispose()
    }
    if (Test-Path -LiteralPath $inputCopy -PathType Leaf) {
        [System.IO.File]::Delete($inputCopy)
    }
}

$releaseDeadline = [DateTime]::UtcNow.AddSeconds(10)
do {
    $remainingListeners = @(
        Get-NetTCPConnection -State Listen -LocalPort $serverPort -ErrorAction SilentlyContinue
    )
    if ($remainingListeners.Count -eq 0) {
        break
    }
    Start-Sleep -Milliseconds 250
} while ([DateTime]::UtcNow -lt $releaseDeadline)
Assert-True -Condition ($remainingListeners.Count -eq 0) `
    -Message "Isolated Comfy did not release port $serverPort."

Assert-True -Condition ($null -ne $newOutput) `
    -Message 'Recorder E2E completed without retaining the output identity.'
$sourceHashAfter = (Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant()
Assert-True -Condition ($sourceHashAfter -ceq $sourceHashBefore) `
    -Message 'Recorder E2E modified the raw source file.'
Assert-True -Condition ((Get-Item -LiteralPath $sourcePath).Length -eq $sourceItem.Length) `
    -Message 'Recorder E2E changed the raw source byte length.'

$validation = Convert-CommandJson -Label 'Rust cartridge validation' -Command {
    & $cartridgeCliFull validate $newOutput.FullName
}
$inspection = Convert-CommandJson -Label 'Rust cartridge inspection' -Command {
    & $cartridgeCliFull inspect $newOutput.FullName
}
Assert-True -Condition ($validation.status -ceq 'ok') `
    -Message 'Rust SDK rejected the Recorder output.'
Assert-True -Condition ($validation.validation.validation_level -ceq 'full') `
    -Message 'Recorder output did not receive full Rust validation.'
Assert-True -Condition ($inspection.status -ceq 'ok') `
    -Message 'Rust SDK could not inspect the Recorder output.'
Assert-True -Condition ($inspection.manifest.codec.family -ceq 'minimax_h3') `
    -Message 'Recorder output has the wrong codec family.'
Assert-True -Condition ($inspection.manifest.codec.profile -ceq 'h3_av_latent') `
    -Message 'Recorder output has the wrong profile.'

$rawVisual = $rawInspection.profile.visual
$recordedVisual = $inspection.profile.visual
foreach ($field in @(
    'latent_slots',
    'latent_height',
    'latent_width',
    'decoded_frames',
    'decoded_height',
    'decoded_width'
)) {
    Assert-True -Condition ($recordedVisual.$field -eq $rawVisual.$field) `
        -Message "Recorder changed H3 visual field $field."
}
Assert-True `
    -Condition ($inspection.profile.audio_latent_slots -eq
        $rawInspection.profile.audio_latent_slots) `
    -Message 'Recorder changed H3 audio cadence.'
Assert-True -Condition ($inspection.safetensors.video.dtype -ceq 'F16') `
    -Message 'Recorder output must contain the profile-approved F16 runtime visual.'
Assert-True `
    -Condition ($inspection.safetensors.audio.dtype -ceq
        $rawInspection.safetensors.audio.dtype) `
    -Message 'Recorder changed the preserved audio dtype.'
Assert-True -Condition ($inspection.manifest.audio.policy -ceq 'preserved_source') `
    -Message 'Recorder output did not preserve the AV audio policy.'

$repoCommit = (& git -C $repoRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0) {
    throw 'Could not resolve the repository commit for the Recorder receipt.'
}
$trackedStatus = @(& git -C $repoRoot status --porcelain --untracked-files=no)
if ($LASTEXITCODE -ne 0) {
    throw 'Could not inspect tracked repository status for the Recorder receipt.'
}
$receipt = [ordered]@{
    schema_version = 1
    status = 'passed'
    mode = 'isolated_comfy_cpu_api_graph'
    repository = [ordered]@{
        commit = $repoCommit
        tracked_clean = ($trackedStatus.Count -eq 0)
        environment_commit = [string]$environmentReceipt.repository.commit
        environment_dirty_at_build = [bool]$environmentReceipt.repository.dirty_at_build
    }
    runtime = [ordered]@{
        environment_name = [System.IO.Path]::GetFileName($environmentFull)
        python_version = $pythonVersion
        port = $serverPort
        cartridge_cli_sha256 = $cartridgeCliHash
    }
    graph = [ordered]@{
        prompt_id = $promptId
        nodes = @(
            'LatentDeckToolkitRawH3Import',
            'LatentDeckSaveLatentCartridge',
            'LatentDeckToolkitLatentScopes'
        )
        execution_status = [string]$history.status.status_str
        recorder_passthrough_consumed = $true
    }
    source = [ordered]@{
        file_name = [System.IO.Path]::GetFileName($sourcePath)
        byte_length = $sourceItem.Length
        sha256 = $sourceHashBefore
        video_shape = @($rawInspection.safetensors.video.shape)
        video_dtype = [string]$rawInspection.safetensors.video.dtype
        audio_shape = @($rawInspection.safetensors.audio.shape)
        audio_dtype = [string]$rawInspection.safetensors.audio.dtype
    }
    output = [ordered]@{
        file_name = $newOutput.Name
        byte_length = $newOutput.Length
        archive_sha256 = [string]$validation.validation.archive_sha256
        cartridge_id = [string]$validation.cartridge_id
        validation_level = [string]$validation.validation.validation_level
        video_shape = @($inspection.safetensors.video.shape)
        video_dtype = [string]$inspection.safetensors.video.dtype
        audio_shape = @($inspection.safetensors.audio.shape)
        audio_dtype = [string]$inspection.safetensors.audio.dtype
        audio_policy = [string]$inspection.manifest.audio.policy
        temp_residue_count = 0
    }
    invariants = [ordered]@{
        source_hash_unchanged = $true
        source_length_unchanged = $true
        exact_geometry_preserved = $true
        audio_cadence_preserved = $true
        runtime_visual_cast_explicit = ($rawInspection.safetensors.video.dtype -ceq 'F32')
        full_rust_validation = $true
        output_count = 1
    }
}
$receiptPath = Join-Path $receiptFull 'receipt.json'
[System.IO.File]::WriteAllText(
    $receiptPath,
    ($receipt | ConvertTo-Json -Depth 30) + "`n",
    [System.Text.UTF8Encoding]::new($false)
)

Write-Host 'ISOLATED COMFY RECORDER E2E: PASS' -ForegroundColor Green
Write-Host "Source: $($receipt.source.file_name) / $($receipt.source.sha256)"
Write-Host "Output: $($receipt.output.file_name) / $($receipt.output.archive_sha256)"
Write-Host "Receipt: $receiptPath"
