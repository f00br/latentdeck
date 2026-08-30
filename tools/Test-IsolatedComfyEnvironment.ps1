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

foreach ($script in @($initializer, $launcher)) {
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
