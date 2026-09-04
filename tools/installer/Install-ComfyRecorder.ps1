[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$ComfyUIRoot,

    [string]$PythonPath,

    [string]$CustomNodesPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$bundleRoot = $PSScriptRoot
$manifestPath = Join-Path $bundleRoot 'BUNDLE-MANIFEST.json'
$verifyScript = Join-Path $bundleRoot 'Verify-ComfyRecorder.py'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json -Depth 32

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Resolve-OnePath {
    param(
        [Parameter(Mandatory)][string[]]$Candidates,
        [Parameter(Mandatory)][string]$Context
    )

    $matches = @(
        $Candidates |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            ForEach-Object { (Resolve-Path -LiteralPath $_).Path } |
            Sort-Object -Unique
    )
    if ($matches.Count -ne 1) {
        throw "$Context could not be resolved uniquely; pass the explicit path."
    }
    return $matches[0]
}

function Resolve-OneDirectory {
    param(
        [Parameter(Mandatory)][string[]]$Candidates,
        [Parameter(Mandatory)][string]$Context
    )

    $matches = @(
        $Candidates |
            Where-Object { Test-Path -LiteralPath $_ -PathType Container } |
            ForEach-Object { (Resolve-Path -LiteralPath $_).Path } |
            Sort-Object -Unique
    )
    if ($matches.Count -ne 1) {
        throw "$Context could not be resolved uniquely; pass the explicit path."
    }
    return $matches[0]
}

function Expand-WheelSafely {
    param(
        [Parameter(Mandatory)][string]$WheelPath,
        [Parameter(Mandatory)][string]$PackageName,
        [Parameter(Mandatory)][string]$Destination,
        [Parameter(Mandatory)][AllowEmptyCollection()]
        [System.Collections.Generic.HashSet[string]]$SeenPaths
    )

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($WheelPath)
    try {
        if ($archive.Entries.Count -le 0 -or $archive.Entries.Count -gt 4096) {
            throw "Wheel has an invalid entry count: $WheelPath"
        }
        [int64]$expandedBytes = 0
        $destinationPrefix = [System.IO.Path]::GetFullPath($Destination) + [System.IO.Path]::DirectorySeparatorChar
        foreach ($entry in $archive.Entries) {
            $relative = $entry.FullName.Replace('\', '/')
            if ([string]::IsNullOrWhiteSpace($relative) -or $relative.StartsWith('/') -or
                $relative.Contains(':') -or $relative -match '(^|/)\.\.($|/)' -or
                (($entry.ExternalAttributes -shr 16) -band 0xF000) -eq 0xA000) {
                throw "Wheel contains an unsafe entry: $relative"
            }
            if ($relative.EndsWith('/')) {
                continue
            }
            $installedRelative = if ($PackageName -ceq 'safetensors') {
                if (-not ($relative.StartsWith('safetensors/') -or
                    $relative.StartsWith('safetensors-0.8.0.dist-info/'))) {
                    throw "Safetensors wheel contains an unexpected path: $relative"
                }
                "latentdeck_recorder_vendor/$relative"
            } else {
                $relative
            }
            if ([int64]$entry.Length -gt (128MB - $expandedBytes)) {
                throw "Wheel expansion exceeds the bundle limit: $WheelPath"
            }
            $expandedBytes += [int64]$entry.Length
            if (-not $SeenPaths.Add($installedRelative)) {
                throw "Bundled wheels contain a duplicate path: $installedRelative"
            }
            $target = [System.IO.Path]::GetFullPath((Join-Path $Destination $installedRelative))
            if (-not $target.StartsWith($destinationPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Wheel entry escapes the vendor directory: $installedRelative"
            }
            $parent = [System.IO.Path]::GetDirectoryName($target)
            [System.IO.Directory]::CreateDirectory($parent) | Out-Null
            if ($PackageName -ceq 'safetensors' -and $relative.EndsWith('.py')) {
                $input = $entry.Open()
                $memory = [System.IO.MemoryStream]::new()
                try {
                    $input.CopyTo($memory)
                    $source = [System.Text.UTF8Encoding]::new($false, $true).GetString(
                        $memory.ToArray()
                    )
                }
                finally {
                    $memory.Dispose()
                    $input.Dispose()
                }
                $source = $source -creplace '(?m)^from safetensors import ', 'from . import '
                if ($source -cmatch '(?m)^(?:from|import) safetensors(?:[ .]|$)') {
                    throw "Safetensors source retains a global package import: $relative"
                }
                [System.IO.File]::WriteAllText(
                    $target,
                    $source,
                    [System.Text.UTF8Encoding]::new($false)
                )
            } else {
                $input = $entry.Open()
                try {
                    $output = [System.IO.FileStream]::new(
                        $target,
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
        }
    }
    finally {
        $archive.Dispose()
    }
}

if ([int]$manifest.schema_version -ne 1 -or
    [string]$manifest.release_label -cne '0.1.0-preview.1' -or
    [string]$manifest.target -cne 'windows-x64' -or
    [string]$manifest.python_abi -cne 'cp312-abi3' -or
    (@($manifest.supported_python) -join "`0") -cne (@('cp312', 'cp313') -join "`0")) {
    throw 'The Comfy Recorder bundle manifest is incompatible with this installer.'
}

$comfyRoot = (Resolve-Path -LiteralPath $ComfyUIRoot).Path
$python = if ([string]::IsNullOrWhiteSpace($PythonPath)) {
    Resolve-OnePath -Context 'ComfyUI Python' -Candidates @(
        (Join-Path $comfyRoot 'python_embeded/python.exe'),
        (Join-Path $comfyRoot 'python_embedded/python.exe'),
        (Join-Path $comfyRoot '.venv/Scripts/python.exe'),
        (Join-Path $comfyRoot 'venv/Scripts/python.exe'),
        (Join-Path $comfyRoot 'python/python.exe')
    )
} else {
    (Resolve-Path -LiteralPath $PythonPath).Path
}
$customNodes = if ([string]::IsNullOrWhiteSpace($CustomNodesPath)) {
    Resolve-OneDirectory -Context 'ComfyUI custom_nodes directory' -Candidates @(
        (Join-Path $comfyRoot 'custom_nodes'),
        (Join-Path $comfyRoot 'ComfyUI/custom_nodes')
    )
} else {
    (Resolve-Path -LiteralPath $CustomNodesPath).Path
}

$probeText = & $python -I $verifyScript --probe
if ($LASTEXITCODE -ne 0) {
    throw 'Could not inspect the target ComfyUI Python interpreter.'
}
$probe = $probeText | ConvertFrom-Json -Depth 8
$pythonAbi = "cp$($probe.major)$($probe.minor)"
if ([string]$probe.implementation -cne 'CPython' -or
    [int]$probe.major -ne 3 -or [int]$probe.minor -notin @(12, 13) -or
    [int]$probe.pointer_bits -ne 64 -or [string]$probe.platform -cne 'win32' -or
    [string]$probe.machine -cnotmatch '^(?i:amd64|x86_64)$') {
    throw 'LatentDeck Comfy Recorder supports CPython 3.12 and 3.13 x64 only; no files were installed.'
}

$expectedPackages = [ordered]@{
    'latentdeck-cartridge' = [ordered]@{
        version = '0.1.0'
        file_name = 'latentdeck_cartridge-0.1.0-cp312-abi3-win_amd64.whl'
    }
    'latentdeck-comfy-cartridge' = [ordered]@{
        version = '0.1.0'
        file_name = 'latentdeck_comfy_cartridge-0.1.0-py3-none-any.whl'
    }
    'safetensors' = [ordered]@{
        version = '0.8.0'
        file_name = 'safetensors-0.8.0-cp310-abi3-win_amd64.whl'
    }
}
if (@($manifest.wheels).Count -ne $expectedPackages.Count) {
    throw 'The Comfy Recorder bundle does not contain the exact expected wheel inventory.'
}
foreach ($expected in $expectedPackages.GetEnumerator()) {
    $record = @($manifest.wheels | Where-Object name -CEQ $expected.Key)
    if ($record.Count -ne 1 -or
        [string]$record[0].version -cne [string]$expected.Value.version -or
        [string]$record[0].file_name -cne [string]$expected.Value.file_name -or
        [string]$record[0].sha256 -cnotmatch '^[0-9a-f]{64}$' -or
        [int64]$record[0].byte_length -le 0) {
        throw "The Comfy Recorder bundle has an invalid wheel record: $($expected.Key)"
    }
    $wheelPath = Join-Path $bundleRoot "wheels/$($record[0].file_name)"
    $item = Get-Item -LiteralPath $wheelPath -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -ne [int64]$record[0].byte_length -or
        (Get-Sha256 -Path $wheelPath) -cne [string]$record[0].sha256) {
        throw "Bundled wheel failed exact length/SHA-256 verification: $($record[0].file_name)"
    }
}
$actualWheelNames = @(
    Get-ChildItem -LiteralPath (Join-Path $bundleRoot 'wheels') -File -Filter '*.whl' |
        Sort-Object Name |
        ForEach-Object Name
)
$expectedWheelNames = @($manifest.wheels | ForEach-Object file_name | Sort-Object)
if (($actualWheelNames -join "`0") -cne ($expectedWheelNames -join "`0")) {
    throw 'The Comfy Recorder wheel directory differs from its manifest.'
}

$finalRoot = Join-Path $customNodes 'ComfyUI-LatentCartridge'
if (Test-Path -LiteralPath $finalRoot) {
    throw "Refusing to overwrite an existing Recorder installation: $finalRoot"
}
$stageRoot = Join-Path $customNodes ('.latentdeck-recorder-' + [guid]::NewGuid().ToString('N'))
try {
    [System.IO.Directory]::CreateDirectory($stageRoot) | Out-Null
    [System.IO.File]::Copy(
        (Join-Path $bundleRoot 'custom_node/__init__.py'),
        (Join-Path $stageRoot '__init__.py'),
        $false
    )
    $vendorRoot = Join-Path $stageRoot 'vendor'
    [System.IO.Directory]::CreateDirectory($vendorRoot) | Out-Null
    $seenPaths = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($wheel in @($manifest.wheels | Sort-Object name)) {
        Expand-WheelSafely `
            -WheelPath (Join-Path $bundleRoot "wheels/$($wheel.file_name)") `
            -PackageName ([string]$wheel.name) `
            -Destination $vendorRoot `
            -SeenPaths $seenPaths
    }
    $privateNamespace = Join-Path $vendorRoot 'latentdeck_recorder_vendor'
    [System.IO.File]::WriteAllText(
        (Join-Path $privateNamespace '__init__.py'),
        '"""Private dependencies for the LatentDeck Comfy Recorder bundle."""' + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $allowedVendorEntries = @(
        'latentdeck_cartridge',
        'latentdeck_cartridge-0.1.0.dist-info',
        'latentdeck_comfy_cartridge',
        'latentdeck_comfy_cartridge-0.1.0.dist-info',
        'latentdeck_recorder_vendor'
    ) | Sort-Object
    $actualVendorEntries = @(
        Get-ChildItem -LiteralPath $vendorRoot -Force |
            Select-Object -ExpandProperty Name |
            Sort-Object
    )
    if (($actualVendorEntries -join "`0") -cne ($allowedVendorEntries -join "`0")) {
        throw 'Recorder vendor root contains a non-private or unexpected top-level entry.'
    }

    $verificationText = & $python -I $verifyScript --vendor $vendorRoot
    if ($LASTEXITCODE -ne 0) {
        throw 'Target ComfyUI Python could not import the bundled Recorder dependencies.'
    }
    $verification = $verificationText | ConvertFrom-Json -Depth 16
    $installation = [ordered]@{
        schema_version = 1
        release_label = [string]$manifest.release_label
        install_mode = 'isolated_vendor_wheels'
        python_abi = $pythonAbi
        bundle_python_abi = [string]$manifest.python_abi
        packages = @($manifest.wheels)
        verification = $verification
    }
    [System.IO.File]::WriteAllText(
        (Join-Path $stageRoot 'INSTALLATION.json'),
        ($installation | ConvertTo-Json -Depth 16) + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.Directory]::Move($stageRoot, $finalRoot)
    $stageRoot = $null
    Write-Host "Installed LatentDeck Comfy Recorder at $finalRoot" -ForegroundColor Green
    Write-Host 'Restart ComfyUI, then find Save Latent Cartridge (.lc) under LatentDeck / Cartridge.'
}
finally {
    if ($null -ne $stageRoot -and (Test-Path -LiteralPath $stageRoot)) {
        Remove-Item -LiteralPath $stageRoot -Recurse -Force
    }
}
