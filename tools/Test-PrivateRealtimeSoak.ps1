[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$CodecRoot,

    [Parameter(Mandatory)]
    [string]$Taeh3,

    [Parameter(Mandatory)]
    [string]$SourceA,

    [Parameter(Mandatory)]
    [string]$SourceB,

    [Parameter(Mandatory)]
    [string]$SourceC,

    [Parameter(Mandatory)]
    [string]$ReceiptRoot,

    [string]$ResumeRoot,

    [ValidatePattern('^[0-9A-Fa-f]{64}$')]
    [string]$ExpectedLegacyD2ReceiptSha256,

    [ValidateRange(1, 7200)]
    [int]$DurationSeconds = 1800,

    [Nullable[int]]$WarmupSeconds,

    [ValidateRange(1, 3600)]
    [int]$ControlIntervalSeconds = 5,

    [ValidateRange(1, 3600)]
    [int]$ResourceIntervalSeconds = 5,

    [ValidateSet('d2-linear', 'd2-xs5', 'q4-topk', 'q4-sinkhorn')]
    [string[]]$Modes = @('d2-linear', 'd2-xs5', 'q4-topk', 'q4-sinkhorn')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Resolve-ExistingFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Label,

        [switch]$AllowEmpty
    )

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if (-not $item.PSIsContainer -and ($AllowEmpty -or $item.Length -gt 0)) {
        return $item.FullName
    }
    $requirement = if ($AllowEmpty) { 'file' } else { 'non-empty file' }
    throw "$Label must be an existing $requirement."
}

function Resolve-ExistingDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $resolved = (Resolve-Path -LiteralPath $Path -ErrorAction Stop).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if ($item.PSIsContainer) {
        return $item.FullName
    }
    throw "$Label must be an existing directory."
}

function Assert-BelowArtifacts {
    param(
        [Parameter(Mandatory)]
        [string]$ArtifactsRoot,

        [Parameter(Mandatory)]
        [string]$Candidate,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $relative = [System.IO.Path]::GetRelativePath($ArtifactsRoot, $Candidate)
    if ($relative -eq '.' -or
        $relative.StartsWith('..', [System.StringComparison]::Ordinal) -or
        [System.IO.Path]::IsPathFullyQualified($relative)) {
        throw "$Label must be a directory below the repository artifacts directory."
    }

    $cursor = if (Test-Path -LiteralPath $Candidate) {
        $Candidate
    }
    else {
        [System.IO.Path]::GetDirectoryName($Candidate)
    }
    while (-not [string]::IsNullOrWhiteSpace($cursor)) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "$Label must not traverse a reparse-point ancestor."
            }
        }
        if ($cursor.Equals($ArtifactsRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
            return
        }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) {
            break
        }
        $cursor = $parent
    }
    throw "$Label ancestry could not be validated beneath artifacts."
}

function Test-PathContains {
    param(
        [Parameter(Mandatory)]
        [string]$Parent,

        [Parameter(Mandatory)]
        [string]$Child
    )

    $relative = [System.IO.Path]::GetRelativePath($Parent, $Child)
    return $relative -eq '.' -or
        (-not $relative.StartsWith('..', [System.StringComparison]::Ordinal) -and
        -not [System.IO.Path]::IsPathFullyQualified($relative))
}

function Write-AtomicBytes {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [byte[]]$Bytes
    )

    if (Test-Path -LiteralPath $Path) {
        throw "Refusing to replace evidence: $Path"
    }
    $partial = "$Path.partial"
    if (Test-Path -LiteralPath $partial) {
        throw "Refusing to replace partial evidence: $partial"
    }
    try {
        $stream = [System.IO.FileStream]::new(
            $partial,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )
        try {
            $stream.Write($Bytes, 0, $Bytes.Length)
            $stream.Flush($true)
        }
        finally {
            $stream.Dispose()
        }
        [System.IO.File]::Move($partial, $Path)
    }
    finally {
        if (Test-Path -LiteralPath $partial) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
}

function Write-AtomicJson {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [object]$Value
    )

    $json = $Value | ConvertTo-Json -Depth 60
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    Write-AtomicBytes -Path $Path -Bytes $bytes
}

function Get-FileIdentity {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or $item.Length -le 0) {
        throw "Evidence input must be a non-empty regular file: $Path"
    }
    $hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    return [ordered]@{
        sha256 = $hash
        byte_length = [long]$item.Length
    }
}

function Assert-FileIdentityMatches {
    param(
        [Parameter(Mandatory)]
        [object]$Actual,

        [Parameter(Mandatory)]
        [object]$Expected,

        [Parameter(Mandatory)]
        [string]$Label
    )

    if ([string]$Actual.sha256 -cne [string]$Expected.sha256 -or
        [long]$Actual.byte_length -ne [long]$Expected.byte_length) {
        throw "$Label identity does not match the bound SHA-256 and byte length."
    }
}

function Test-MachinePathText {
    param([string]$Text)

    if ($null -eq $Text) {
        return $false
    }
    return $Text.StartsWith('/') -or
        $Text.StartsWith('\\') -or
        $Text -cmatch '^[A-Za-z]:[\\/]'
}

function Assert-PathFreeJson {
    param(
        [Parameter(Mandatory)]
        [AllowNull()]
        [object]$Value
    )

    if ($null -eq $Value) {
        return
    }
    if ($Value -is [string]) {
        if (Test-MachinePathText -Text $Value) {
            throw 'Persisted soak evidence contains a machine-local path.'
        }
        return
    }
    if ($Value -is [System.Collections.IDictionary]) {
        foreach ($entry in $Value.GetEnumerator()) {
            Assert-PathFreeJson -Value $entry.Value
        }
        return
    }
    if ($Value -is [System.Management.Automation.PSCustomObject]) {
        foreach ($property in $Value.PSObject.Properties) {
            Assert-PathFreeJson -Value $property.Value
        }
        return
    }
    if ($Value -is [System.Collections.IEnumerable]) {
        foreach ($entry in $Value) {
            Assert-PathFreeJson -Value $entry
        }
    }
}

function Get-RepositoryState {
    param(
        [Parameter(Mandatory)]
        [string]$RepoRoot
    )

    $commitOutput = @(& git -C $RepoRoot rev-parse HEAD)
    if ($LASTEXITCODE -ne 0 -or $commitOutput.Count -ne 1) {
        throw 'Unable to resolve the exact Git commit for soak evidence.'
    }

    & git -C $RepoRoot diff --quiet -- *> $null
    $worktreeExit = $LASTEXITCODE
    if ($worktreeExit -gt 1) {
        throw 'Unable to inspect tracked worktree state.'
    }
    & git -C $RepoRoot diff --cached --quiet -- *> $null
    $indexExit = $LASTEXITCODE
    if ($indexExit -gt 1) {
        throw 'Unable to inspect staged worktree state.'
    }
    $untracked = @(& git -C $RepoRoot ls-files --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to inspect nonignored untracked files.'
    }

    return [ordered]@{
        git_commit = $commitOutput[0].Trim().ToLowerInvariant()
        tracked_tree_clean = ($worktreeExit -eq 0 -and $indexExit -eq 0)
        nonignored_untracked_clean = ($untracked.Count -eq 0)
        nonignored_untracked_count = $untracked.Count
    }
}

function Assert-ReleaseRepositoryState {
    param(
        [Parameter(Mandatory)]
        [object]$State,

        [string]$ExpectedCommit
    )

    if (-not $State.tracked_tree_clean) {
        throw 'A 1800-second release soak refuses dirty tracked source or index state.'
    }
    if (-not $State.nonignored_untracked_clean) {
        throw "A 1800-second release soak refuses nonignored untracked files (count: $($State.nonignored_untracked_count))."
    }
    if (-not [string]::IsNullOrWhiteSpace($ExpectedCommit) -and
        [string]$State.git_commit -cne $ExpectedCommit) {
        throw 'The Git commit changed during the release soak suite.'
    }
}

function Build-PrivateSoakTestBinary {
    param(
        [Parameter(Mandatory)]
        [string]$RepoRoot
    )

    Push-Location $RepoRoot
    try {
        $messages = @(& cargo test --release -p latentdeck-app --test private_realtime_soak `
            --no-run --message-format=json-render-diagnostics)
        $cargoExit = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }

    $executables = [System.Collections.Generic.List[string]]::new()
    foreach ($line in $messages) {
        try {
            $message = $line | ConvertFrom-Json -Depth 30 -ErrorAction Stop
        }
        catch {
            continue
        }
        if ($message.reason -ceq 'compiler-message' -and
            $null -ne $message.message -and
            $message.message.level -in @('error', 'warning') -and
            -not [string]::IsNullOrWhiteSpace([string]$message.message.rendered)) {
            Write-Host ([string]$message.message.rendered)
        }
        if ($message.reason -ceq 'compiler-artifact' -and
            $message.target.name -ceq 'private_realtime_soak' -and
            $null -ne $message.executable) {
            $executables.Add([string]$message.executable)
        }
    }
    if ($cargoExit -ne 0) {
        throw "Private realtime soak test build failed with exit code $cargoExit."
    }
    $distinct = @($executables | Select-Object -Unique)
    if ($distinct.Count -ne 1) {
        throw "Cargo did not report exactly one private_realtime_soak test executable (found $($distinct.Count))."
    }
    return Resolve-ExistingFile -Path $distinct[0] -Label 'Private realtime soak test executable'
}

function Open-ResumeRootReadOnly {
    param(
        [Parameter(Mandatory)]
        [string]$Root
    )

    $items = @(Get-ChildItem -LiteralPath $Root -Force | Sort-Object Name)
    if ($items.Count -gt 32) {
        throw 'ResumeRoot contains too many entries for the bounded receipt contract.'
    }
    $locks = [System.Collections.Generic.List[System.IO.FileStream]]::new()
    $fingerprint = [System.Collections.Generic.List[object]]::new()
    $files = @{}
    try {
        foreach ($item in $items) {
            if ($item.PSIsContainer -or
                ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
                $item.Length -gt 16MB) {
                throw 'ResumeRoot may contain only bounded, direct, non-reparse receipt files.'
            }
            $stream = [System.IO.FileStream]::new(
                $item.FullName,
                [System.IO.FileMode]::Open,
                [System.IO.FileAccess]::Read,
                [System.IO.FileShare]::Read
            )
            $locks.Add($stream)
            $bytes = [System.IO.File]::ReadAllBytes($item.FullName)
            $identity = Get-FileIdentity -Path $item.FullName
            $fingerprint.Add([ordered]@{
                name = $item.Name
                sha256 = $identity.sha256
                byte_length = $identity.byte_length
            })
            $files[$item.Name] = [pscustomobject]@{
                Bytes = $bytes
                Identity = $identity
            }
        }
        return [pscustomobject]@{
            Root = $Root
            Locks = $locks
            Files = $files
            FingerprintJson = (@($fingerprint) | ConvertTo-Json -Depth 10 -Compress)
        }
    }
    catch {
        foreach ($stream in $locks) {
            $stream.Dispose()
        }
        throw
    }
}

function Assert-ResumeRootUnchanged {
    param(
        [Parameter(Mandatory)]
        [object]$Lease
    )

    $items = @(Get-ChildItem -LiteralPath $Lease.Root -Force | Sort-Object Name)
    $fingerprint = [System.Collections.Generic.List[object]]::new()
    foreach ($item in $items) {
        if ($item.PSIsContainer -or
            ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            $item.Length -gt 16MB) {
            throw 'ResumeRoot changed during the soak suite.'
        }
        $identity = Get-FileIdentity -Path $item.FullName
        $fingerprint.Add([ordered]@{
            name = $item.Name
            sha256 = $identity.sha256
            byte_length = $identity.byte_length
        })
    }
    $current = @($fingerprint) | ConvertTo-Json -Depth 10 -Compress
    if ($current -cne $Lease.FingerprintJson) {
        throw 'ResumeRoot changed despite its read-only evidence contract.'
    }
}

function Resolve-BoundPackFile {
    param(
        [Parameter(Mandatory)]
        [string]$PackRoot,

        [Parameter(Mandatory)]
        [string]$RelativePath,

        [Parameter(Mandatory)]
        [string]$Label,

        [switch]$AllowEmpty
    )

    if ([string]::IsNullOrWhiteSpace($RelativePath) -or
        [System.IO.Path]::IsPathFullyQualified($RelativePath)) {
        throw "$Label path is not a safe relative codec-pack path."
    }
    $packRootFull = [System.IO.Path]::GetFullPath($PackRoot)
    $packRootItem = Get-Item -LiteralPath $packRootFull -Force
    if (-not $packRootItem.PSIsContainer -or
        ($packRootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "$Label pack root must be a direct non-reparse directory."
    }
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $packRootFull $RelativePath))
    $relative = [System.IO.Path]::GetRelativePath($packRootFull, $candidate)
    if ($relative.StartsWith('..', [System.StringComparison]::Ordinal) -or
        [System.IO.Path]::IsPathFullyQualified($relative)) {
        throw "$Label path escapes the selected codec pack."
    }
    $cursor = $candidate
    while (-not $cursor.Equals($packRootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        $item = Get-Item -LiteralPath $cursor -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Label path traverses a reparse point."
        }
        $cursor = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($cursor)) {
            throw "$Label ancestry is invalid."
        }
    }
    return Resolve-ExistingFile -Path $candidate -Label $Label -AllowEmpty:$AllowEmpty
}

function Get-PhysicalPackInventory {
    param(
        [Parameter(Mandatory)]
        [string]$PackRoot,

        [Parameter(Mandatory)]
        [string]$ManifestPath,

        [Parameter(Mandatory)]
        [string]$CatalogPath
    )

    $catalogItem = Get-Item -LiteralPath $CatalogPath -Force
    if ($catalogItem.Length -le 0 -or $catalogItem.Length -gt 1MB) {
        throw 'Codec-pack integrity catalog is outside the bounded physical-pack contract.'
    }
    $catalog = Get-Content -Raw -LiteralPath $CatalogPath | ConvertFrom-Json -Depth 20
    $entries = @($catalog.files)
    if ($catalog.manifest_version -cne '1.0.0' -or
        $entries.Count -eq 0 -or $entries.Count -gt 32768) {
        throw 'Codec-pack integrity catalog version or file count is invalid.'
    }

    $expected = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($required in @($ManifestPath, $CatalogPath)) {
        $relative = [System.IO.Path]::GetRelativePath($PackRoot, $required).Replace('\', '/')
        $expected[$relative] = $null
    }
    foreach ($entry in $entries) {
        $relative = [string]$entry.path
        if ([string]::IsNullOrWhiteSpace($relative) -or
            [System.IO.Path]::IsPathFullyQualified($relative) -or
            [regex]::Split($relative, '[\\/]') -contains '..' -or
            $expected.ContainsKey($relative)) {
            throw 'Codec-pack integrity catalog contains an unsafe or duplicate path.'
        }
        $filePath = Resolve-BoundPackFile -PackRoot $PackRoot -RelativePath $relative `
            -Label 'cataloged codec-pack file' -AllowEmpty
        $identity = Get-FileIdentity -Path $filePath
        if ($identity.sha256 -cne [string]$entry.sha256 -or
            $identity.byte_length -ne [long]$entry.byte_length) {
            throw 'Codec-pack file differs from its integrity catalog.'
        }
        $expected[$relative.Replace('\', '/')] = $null
    }

    $actual = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $pending.Push($PackRoot)
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force)) {
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'Physical codec pack contains a reparse point.'
            }
            if ($item.PSIsContainer) {
                $pending.Push($item.FullName)
                continue
            }
            $relative = [System.IO.Path]::GetRelativePath($PackRoot, $item.FullName).Replace('\', '/')
            if ($actual.ContainsKey($relative) -or $actual.Count -ge 32770) {
                throw 'Physical codec-pack inventory is duplicated or oversized.'
            }
            $actual[$relative] = $null

            $lower = $relative.ToLowerInvariant()
            if ($lower.EndsWith('.pth') -or $lower.EndsWith('._pth')) {
                if ($item.Length -gt 64KB) {
                    throw 'Codec-pack Python path file is oversized.'
                }
                foreach ($lineValue in @(Get-Content -LiteralPath $item.FullName)) {
                    $line = ([string]$lineValue).Trim()
                    if ([string]::IsNullOrWhiteSpace($line) -or $line.StartsWith('#')) {
                        continue
                    }
                    if ($line.StartsWith('import ') -or $line.StartsWith("import`t")) {
                        throw 'Physical codec pack may not execute Python path-file directives.'
                    }
                    if ([System.IO.Path]::IsPathFullyQualified($line) -or
                        [regex]::Split($line, '[\\/]') -contains '..') {
                        throw 'Physical codec-pack Python path file references an external path.'
                    }
                    $resolvedEntry = [System.IO.Path]::GetFullPath(
                        (Join-Path $item.DirectoryName $line)
                    )
                    if (-not (Test-PathContains -Parent $PackRoot -Child $resolvedEntry) -or
                        -not (Test-Path -LiteralPath $resolvedEntry)) {
                        throw 'Physical codec-pack Python path entry escapes or is missing.'
                    }
                }
            }
        }
    }
    if ($actual.Count -ne $expected.Count) {
        throw 'Physical codec-pack inventory differs from its integrity catalog.'
    }
    foreach ($relative in $actual.Keys) {
        if (-not $expected.ContainsKey($relative)) {
            throw 'Physical codec pack contains an untracked runtime file.'
        }
    }
    return [long]$entries.Count
}

function Get-InstalledPackBinding {
    param(
        [Parameter(Mandatory)]
        [string]$CodecRoot,

        [Parameter(Mandatory)]
        [string]$PackId,

        [Parameter(Mandatory)]
        [string]$PackVersion
    )

    if ($PackId -cnotmatch '^[A-Za-z0-9._-]+$' -or
        $PackVersion -cnotmatch '^[A-Za-z0-9._-]+$') {
        throw 'Receipt codec-pack identity is not a safe installation token.'
    }
    $packRoot = Join-Path (Join-Path $CodecRoot $PackId) $PackVersion
    $packRoot = Resolve-ExistingDirectory -Path $packRoot -Label 'Installed codec-pack root'
    $manifestPath = Resolve-BoundPackFile -PackRoot $packRoot `
        -RelativePath 'codec-pack.json' -Label 'codec-pack.json'
    $manifest = Get-Content -Raw -LiteralPath $manifestPath | ConvertFrom-Json -Depth 40
    if ($manifest.pack_id -cne $PackId -or $manifest.pack_version -cne $PackVersion) {
        throw 'Installed codec-pack manifest identity differs from the receipt.'
    }
    $catalogPath = Resolve-BoundPackFile -PackRoot $packRoot `
        -RelativePath ([string]$manifest.integrity.catalog_path) -Label 'codec-pack integrity catalog'
    $workerPath = Resolve-BoundPackFile -PackRoot $packRoot `
        -RelativePath ([string]$manifest.worker.executable) -Label 'codec-pack worker executable'
    $binding = [ordered]@{
        pack_id = [string]$manifest.pack_id
        pack_version = [string]$manifest.pack_version
        adapter_id = [string]$manifest.adapter.adapter_id
        adapter_version = [string]$manifest.adapter.adapter_version
        codec_pack_manifest = Get-FileIdentity -Path $manifestPath
        integrity_catalog = Get-FileIdentity -Path $catalogPath
        worker_executable = Get-FileIdentity -Path $workerPath
    }
    if ($binding.integrity_catalog.sha256 -cne [string]$manifest.integrity.catalog_sha256) {
        throw 'Installed codec-pack integrity catalog differs from codec-pack.json.'
    }
    $binding['integrity_catalog_file_count'] = Get-PhysicalPackInventory `
        -PackRoot $packRoot -ManifestPath $manifestPath -CatalogPath $catalogPath
    $binding['self_contained'] = $true
    return $binding
}

function Assert-ExecutionContextMatches {
    param(
        [Parameter(Mandatory)]
        [object]$Actual,

        [Parameter(Mandatory)]
        [object]$Expected
    )

    if ([int]$Actual.schema_version -ne [int]$Expected.schema_version -or
        [string]$Actual.evidence_kind -cne [string]$Expected.evidence_kind -or
        [string]$Actual.repository.git_commit -cne [string]$Expected.repository.git_commit -or
        [bool]$Actual.repository.tracked_tree_clean -ne [bool]$Expected.repository.tracked_tree_clean -or
        [bool]$Actual.repository.nonignored_untracked_clean -ne [bool]$Expected.repository.nonignored_untracked_clean -or
        [long]$Actual.measurement.duration_seconds -ne [long]$Expected.measurement.duration_seconds -or
        [long]$Actual.measurement.warmup_seconds -ne [long]$Expected.measurement.warmup_seconds -or
        [long]$Actual.measurement.control_interval_seconds -ne [long]$Expected.measurement.control_interval_seconds -or
        [long]$Actual.measurement.resource_interval_seconds -ne [long]$Expected.measurement.resource_interval_seconds -or
        [long]$Actual.measurement.frame_rate_numerator -ne [long]$Expected.measurement.frame_rate_numerator -or
        [long]$Actual.measurement.frame_rate_denominator -ne [long]$Expected.measurement.frame_rate_denominator) {
        throw 'Resumed v2 receipt repository context differs from this suite.'
    }
    Assert-FileIdentityMatches -Actual $Actual.cargo_lock -Expected $Expected.cargo_lock -Label 'Cargo.lock'
    Assert-FileIdentityMatches -Actual $Actual.test_binary -Expected $Expected.test_binary -Label 'test binary'
    Assert-FileIdentityMatches -Actual $Actual.decoder -Expected $Expected.decoder -Label 'decoder'
    Assert-FileIdentityMatches -Actual $Actual.sources.a -Expected $Expected.sources.a -Label 'source A'
    Assert-FileIdentityMatches -Actual $Actual.sources.b -Expected $Expected.sources.b -Label 'source B'
    Assert-FileIdentityMatches -Actual $Actual.sources.c -Expected $Expected.sources.c -Label 'source C'
}

function Assert-ReceiptRuntimeBindings {
    param(
        [Parameter(Mandatory)]
        [object]$Receipt,

        [Parameter(Mandatory)]
        [string]$CodecRoot
    )

    $installed = Get-InstalledPackBinding -CodecRoot $CodecRoot `
        -PackId ([string]$Receipt.runtime.codec_pack.pack_id) `
        -PackVersion ([string]$Receipt.runtime.codec_pack.pack_version)
    if ($installed.adapter_id -cne [string]$Receipt.runtime.codec_pack.adapter_id -or
        $installed.adapter_version -cne [string]$Receipt.runtime.codec_pack.adapter_version) {
        throw 'Receipt codec adapter identity differs from the installed codec pack.'
    }
    Assert-FileIdentityMatches -Actual $Receipt.codec_runtime_inputs.codec_pack_manifest `
        -Expected $installed.codec_pack_manifest -Label 'codec-pack manifest'
    Assert-FileIdentityMatches -Actual $Receipt.codec_runtime_inputs.integrity_catalog `
        -Expected $installed.integrity_catalog -Label 'codec-pack integrity catalog'
    Assert-FileIdentityMatches -Actual $Receipt.codec_runtime_inputs.worker_executable `
        -Expected $installed.worker_executable -Label 'codec-pack worker executable'
    if ([long]$Receipt.codec_runtime_inputs.integrity_catalog_file_count -ne
        [long]$installed.integrity_catalog_file_count -or
        -not [bool]$Receipt.codec_runtime_inputs.self_contained) {
        throw 'Receipt codec runtime is not the verified self-contained physical pack.'
    }
}

function Assert-ReceiptContract {
    param(
        [Parameter(Mandatory)]
        [object]$Receipt,

        [Parameter(Mandatory)]
        [string]$Mode,

        [Parameter(Mandatory)]
        [int]$DurationSeconds,

        [Parameter(Mandatory)]
        [int]$WarmupSeconds,

        [Parameter(Mandatory)]
        [int]$ControlIntervalSeconds,

        [Parameter(Mandatory)]
        [int]$ResourceIntervalSeconds,

        [Parameter(Mandatory)]
        [object]$RunContext,

        [Parameter(Mandatory)]
        [string]$CodecRoot,

        [Parameter(Mandatory)]
        [object]$ReceiptIdentity,

        [string]$ExpectedLegacySha256,

        [switch]$AllowLegacyV1
    )

    Assert-PathFreeJson -Value $Receipt
    $controlReceipt = if ($null -ne $Receipt.PSObject.Properties['control_to_processed_frame']) {
        $Receipt.control_to_processed_frame
    }
    else {
        $Receipt.control_to_effect
    }
    if ($Receipt.evidence_kind -cne 'latentdeck_private_realtime_soak' -or
        $Receipt.mode -cne $Mode -or
        [double]$Receipt.configuration.duration_seconds -ne [double]$DurationSeconds -or
        [double]$Receipt.configuration.warmup_seconds -ne [double]$WarmupSeconds -or
        [double]$Receipt.configuration.control_interval_seconds -ne [double]$ControlIntervalSeconds -or
        [double]$Receipt.configuration.resource_interval_seconds -ne [double]$ResourceIntervalSeconds -or
        $Receipt.privacy.receipt_is_path_free -isnot [bool] -or
        $Receipt.privacy.receipt_is_path_free -ne $true -or
        $Receipt.privacy.private_payload_embedded -isnot [bool] -or
        $Receipt.privacy.private_payload_embedded -ne $false -or
        $Receipt.runtime.host_build_profile -cne 'release' -or
        $Receipt.renderer.final_device_poll_completed -isnot [bool] -or
        $Receipt.renderer.final_device_poll_completed -ne $true -or
        $Receipt.renderer.submitted_frames -le 0 -or
        $controlReceipt.samples -le 0 -or
        $Receipt.partial_cleanup.scoped_partial_files_after -ne 0) {
        throw "Mode $Mode receipt does not satisfy the bounded measurement contract."
    }
    if ($DurationSeconds -ge 1800 -and
        ($Receipt.configuration.release_duration_exercised -isnot [bool] -or
        $Receipt.configuration.release_duration_exercised -ne $true -or
        $Receipt.release_gates.all_required_gates_passed -isnot [bool] -or
        $Receipt.release_gates.all_required_gates_passed -ne $true)) {
        throw "Mode $Mode failed one or more full-duration measurement gates."
    }
    if ($Receipt.runtime.decoder.asset_id -cne 'taeh3') {
        throw "Mode $Mode receipt has the wrong external decoder asset id."
    }
    Assert-FileIdentityMatches -Actual $Receipt.runtime.decoder `
        -Expected $RunContext.decoder -Label "$Mode decoder"

    $expectedSources = if ($Mode -in @('d2-linear', 'd2-xs5')) {
        @(
            [ordered]@{ logical = 'B'; identity = $RunContext.sources.b },
            [ordered]@{ logical = 'C'; identity = $RunContext.sources.c }
        )
    }
    else {
        @(
            [ordered]@{ logical = 'B'; identity = $RunContext.sources.b },
            [ordered]@{ logical = 'C'; identity = $RunContext.sources.c },
            [ordered]@{ logical = 'A'; identity = $RunContext.sources.a },
            [ordered]@{ logical = 'B'; identity = $RunContext.sources.b }
        )
    }
    $entries = @($Receipt.sources.entries)
    if ($entries.Count -ne $expectedSources.Count) {
        throw "Mode $Mode receipt has the wrong source entry count."
    }
    for ($index = 0; $index -lt $entries.Count; $index += 1) {
        if ([string]$entries[$index].logical_source -cne [string]$expectedSources[$index].logical -or
            [string]$entries[$index].archive_sha256 -cne [string]$expectedSources[$index].identity.sha256) {
            throw "Mode $Mode receipt source identity/order differs from this suite."
        }
    }

    $schema = [int]$Receipt.schema_version
    if ($schema -eq 1) {
        if (-not $AllowLegacyV1 -or $Mode -cne 'd2-linear' -or $DurationSeconds -lt 1800 -or
            [string]::IsNullOrWhiteSpace($ExpectedLegacySha256) -or
            [string]$ReceiptIdentity.sha256 -cne $ExpectedLegacySha256) {
            throw 'Only the known full-duration d2-linear v1 receipt may use the legacy resume path.'
        }
        $legacyPack = Get-InstalledPackBinding -CodecRoot $CodecRoot `
            -PackId ([string]$Receipt.runtime.codec_pack.pack_id) `
            -PackVersion ([string]$Receipt.runtime.codec_pack.pack_version)
        if ($legacyPack.adapter_id -cne [string]$Receipt.runtime.codec_pack.adapter_id -or
            $legacyPack.adapter_version -cne [string]$Receipt.runtime.codec_pack.adapter_version) {
            throw 'Legacy receipt codec adapter identity differs from the installed codec pack.'
        }
        return
    }
    if ($schema -ne 2) {
        throw "Unsupported realtime soak receipt schema: $schema"
    }
    Assert-ExecutionContextMatches -Actual $Receipt.execution_context -Expected $RunContext
    for ($index = 0; $index -lt $entries.Count; $index += 1) {
        if ([long]$entries[$index].archive_byte_length -ne
            [long]$expectedSources[$index].identity.byte_length) {
            throw "Mode $Mode v2 receipt source byte length differs from this suite."
        }
    }
    Assert-ReceiptRuntimeBindings -Receipt $Receipt -CodecRoot $CodecRoot
}

function Invoke-StrictReceiptValidator {
    param(
        [Parameter(Mandatory)]
        [string]$TestBinary,

        [Parameter(Mandatory)]
        [string]$ReceiptPath,

        [string]$ExpectedLegacySha256
    )

    $env:LATENTDECK_PRIVATE_SOAK_VALIDATE_RECEIPT = $ReceiptPath
    if ([string]::IsNullOrWhiteSpace($ExpectedLegacySha256)) {
        Remove-Item Env:LATENTDECK_PRIVATE_SOAK_EXPECTED_LEGACY_SHA256 `
            -ErrorAction SilentlyContinue
    }
    else {
        $env:LATENTDECK_PRIVATE_SOAK_EXPECTED_LEGACY_SHA256 = $ExpectedLegacySha256
    }
    & $TestBinary validate_private_realtime_soak_receipt --ignored --exact --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "Strict realtime-soak receipt validation failed with exit code $LASTEXITCODE."
    }
}

$canonicalModes = @('d2-linear', 'd2-xs5', 'q4-topk', 'q4-sinkhorn')
$selectedModes = @($canonicalModes | Where-Object { $Modes -contains $_ })
if ($Modes.Count -ne (@($Modes | Select-Object -Unique)).Count) {
    throw 'Modes must not contain duplicates.'
}
if ($selectedModes.Count -eq 0) {
    throw 'At least one soak mode is required.'
}
if ($DurationSeconds -ge 1800 -and ($selectedModes -join ',') -cne ($canonicalModes -join ',')) {
    throw 'A release-duration suite must account for all four modes in canonical order.'
}
if ($null -ne $WarmupSeconds -and ($WarmupSeconds -lt 0 -or $WarmupSeconds -ge $DurationSeconds)) {
    throw 'WarmupSeconds must be non-negative and shorter than DurationSeconds.'
}
$effectiveWarmupSeconds = if ($null -eq $WarmupSeconds) {
    if ($DurationSeconds -ge 1800) { 60 } else { [int][Math]::Floor($DurationSeconds / 5) }
}
else {
    [int]$WarmupSeconds
}
if ($DurationSeconds -ge 1800 -and
    ($ControlIntervalSeconds -gt 60 -or $ResourceIntervalSeconds -gt 60 -or
    $effectiveWarmupSeconds -gt [Math]::Floor($DurationSeconds / 3))) {
    throw 'Release-duration timing must use control/resource intervals <=60 seconds and warmup <= one third of duration.'
}
$expectedLegacySha256 = if ([string]::IsNullOrWhiteSpace($ExpectedLegacyD2ReceiptSha256)) {
    $null
}
else {
    $ExpectedLegacyD2ReceiptSha256.ToLowerInvariant()
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$artifactsRoot = (Resolve-Path -LiteralPath $artifactsRoot).Path
$receiptRootFull = [System.IO.Path]::GetFullPath($ReceiptRoot)
Assert-BelowArtifacts -ArtifactsRoot $artifactsRoot -Candidate $receiptRootFull -Label 'ReceiptRoot'
if (Test-Path -LiteralPath $receiptRootFull) {
    throw "ReceiptRoot must not already exist: $receiptRootFull"
}
$receiptParent = [System.IO.Path]::GetDirectoryName($receiptRootFull)
if ([string]::IsNullOrWhiteSpace($receiptParent)) {
    throw 'ReceiptRoot must have a parent directory beneath artifacts.'
}
[System.IO.Directory]::CreateDirectory($receiptParent) | Out-Null
$receiptParent = (Resolve-Path -LiteralPath $receiptParent).Path
Assert-BelowArtifacts -ArtifactsRoot $artifactsRoot -Candidate $receiptRootFull -Label 'ReceiptRoot'
$receiptLeaf = [System.IO.Path]::GetFileName($receiptRootFull)
$stagingRootFull = Join-Path $receiptParent `
    (".{0}.{1}.partial" -f $receiptLeaf, [guid]::NewGuid().ToString('N'))
Assert-BelowArtifacts -ArtifactsRoot $artifactsRoot -Candidate $stagingRootFull -Label 'Receipt staging root'
if (Test-Path -LiteralPath $stagingRootFull) {
    throw 'Randomized receipt staging root already exists.'
}

$resumeRootFull = $null
if ($PSBoundParameters.ContainsKey('ResumeRoot')) {
    $resumeRootFull = Resolve-ExistingDirectory -Path $ResumeRoot -Label 'ResumeRoot'
    Assert-BelowArtifacts -ArtifactsRoot $artifactsRoot -Candidate $resumeRootFull -Label 'ResumeRoot'
    if ((Test-PathContains -Parent $resumeRootFull -Child $receiptRootFull) -or
        (Test-PathContains -Parent $receiptRootFull -Child $resumeRootFull)) {
        throw 'ResumeRoot and ReceiptRoot must be separate, non-nested directories.'
    }
}

$codecRootFull = Resolve-ExistingDirectory -Path $CodecRoot -Label 'CodecRoot'
$taeh3Full = Resolve-ExistingFile -Path $Taeh3 -Label 'Taeh3'
$sourceAFull = Resolve-ExistingFile -Path $SourceA -Label 'SourceA'
$sourceBFull = Resolve-ExistingFile -Path $SourceB -Label 'SourceB'
$sourceCFull = Resolve-ExistingFile -Path $SourceC -Label 'SourceC'
$cargoLockFull = Resolve-ExistingFile -Path (Join-Path $repoRoot 'Cargo.lock') -Label 'Cargo.lock'

$resumeLease = $null
$environmentNames = @(
    'LATENTDECK_PRIVATE_REALTIME_SOAK',
    'LATENTDECK_PRIVATE_SOAK_MODE',
    'LATENTDECK_PRIVATE_CODEC_ROOT',
    'LATENTDECK_PRIVATE_TAEH3',
    'LATENTDECK_PRIVATE_SOAK_SOURCE_A',
    'LATENTDECK_PRIVATE_SOAK_SOURCE_B',
    'LATENTDECK_PRIVATE_SOAK_SOURCE_C',
    'LATENTDECK_PRIVATE_SOAK_RECEIPT',
    'LATENTDECK_PRIVATE_SOAK_DURATION_SECONDS',
    'LATENTDECK_PRIVATE_SOAK_WARMUP_SECONDS',
    'LATENTDECK_PRIVATE_SOAK_CONTROL_INTERVAL_SECONDS',
    'LATENTDECK_PRIVATE_SOAK_RESOURCE_INTERVAL_SECONDS',
    'LATENTDECK_PRIVATE_SOAK_EXECUTION_CONTEXT',
    'LATENTDECK_PRIVATE_SOAK_VALIDATE_RECEIPT',
    'LATENTDECK_PRIVATE_SOAK_EXPECTED_LEGACY_SHA256'
)
$previous = @{}
foreach ($name in $environmentNames) {
    $previous[$name] = [System.Environment]::GetEnvironmentVariable($name, 'Process')
}
$suitePath = $null
$stagingPublished = $false

try {
    if ($null -ne $resumeRootFull) {
        $resumeLease = Open-ResumeRootReadOnly -Root $resumeRootFull
        foreach ($mode in $selectedModes) {
            $resumeName = "$mode.json"
            if (-not $resumeLease.Files.ContainsKey($resumeName)) {
                continue
            }
            $candidate = [System.Text.UTF8Encoding]::new($false).GetString(
                [byte[]]$resumeLease.Files[$resumeName].Bytes
            ) | ConvertFrom-Json -Depth 60
            if ([int]$candidate.schema_version -eq 1 -and
                ($mode -cne 'd2-linear' -or
                [string]::IsNullOrWhiteSpace($expectedLegacySha256) -or
                [string]$resumeLease.Files[$resumeName].Identity.sha256 -cne
                    $expectedLegacySha256)) {
                throw 'Legacy v1 resume requires d2-linear and its explicitly supplied exact SHA-256.'
            }
        }
    }

    $preBuildState = Get-RepositoryState -RepoRoot $repoRoot
    if ($DurationSeconds -ge 1800) {
        Assert-ReleaseRepositoryState -State $preBuildState
    }
    Write-Host 'REALTIME SOAK: building exact release test executable' -ForegroundColor Cyan
    $testBinaryFull = Build-PrivateSoakTestBinary -RepoRoot $repoRoot
    $postBuildState = Get-RepositoryState -RepoRoot $repoRoot
    if ($DurationSeconds -ge 1800) {
        Assert-ReleaseRepositoryState -State $postBuildState -ExpectedCommit $preBuildState.git_commit
    }

    $runContext = [ordered]@{
        schema_version = 2
        evidence_kind = 'latentdeck_private_realtime_soak_execution_context'
        repository = [ordered]@{
            git_commit = $postBuildState.git_commit
            tracked_tree_clean = $postBuildState.tracked_tree_clean
            nonignored_untracked_clean = $postBuildState.nonignored_untracked_clean
        }
        measurement = [ordered]@{
            duration_seconds = $DurationSeconds
            warmup_seconds = $effectiveWarmupSeconds
            control_interval_seconds = $ControlIntervalSeconds
            resource_interval_seconds = $ResourceIntervalSeconds
            frame_rate_numerator = 24
            frame_rate_denominator = 1
        }
        cargo_lock = Get-FileIdentity -Path $cargoLockFull
        test_binary = Get-FileIdentity -Path $testBinaryFull
        decoder = Get-FileIdentity -Path $taeh3Full
        sources = [ordered]@{
            a = Get-FileIdentity -Path $sourceAFull
            b = Get-FileIdentity -Path $sourceBFull
            c = Get-FileIdentity -Path $sourceCFull
        }
    }
    Assert-PathFreeJson -Value $runContext

    if (Test-Path -LiteralPath $receiptRootFull) {
        throw 'ReceiptRoot appeared after preflight; refusing to share or reuse it.'
    }
    [System.IO.Directory]::CreateDirectory($stagingRootFull) | Out-Null
    $stagingItem = Get-Item -LiteralPath $stagingRootFull -Force
    if (($stagingItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        @(Get-ChildItem -LiteralPath $stagingRootFull -Force).Count -ne 0) {
        throw 'Receipt staging root was not a fresh direct directory.'
    }
    $executionContextPath = Join-Path $stagingRootFull 'execution-context.json'
    Write-AtomicJson -Path $executionContextPath -Value $runContext
    $executionContextIdentity = Get-FileIdentity -Path $executionContextPath

    $env:LATENTDECK_PRIVATE_REALTIME_SOAK = '1'
    $env:LATENTDECK_PRIVATE_CODEC_ROOT = $codecRootFull
    $env:LATENTDECK_PRIVATE_TAEH3 = $taeh3Full
    $env:LATENTDECK_PRIVATE_SOAK_SOURCE_A = $sourceAFull
    $env:LATENTDECK_PRIVATE_SOAK_SOURCE_B = $sourceBFull
    $env:LATENTDECK_PRIVATE_SOAK_SOURCE_C = $sourceCFull
    $env:LATENTDECK_PRIVATE_SOAK_DURATION_SECONDS = [string]$DurationSeconds
    $env:LATENTDECK_PRIVATE_SOAK_EXECUTION_CONTEXT = $executionContextPath
    $env:LATENTDECK_PRIVATE_SOAK_WARMUP_SECONDS = [string]$effectiveWarmupSeconds
    $env:LATENTDECK_PRIVATE_SOAK_CONTROL_INTERVAL_SECONDS = [string]$ControlIntervalSeconds
    $env:LATENTDECK_PRIVATE_SOAK_RESOURCE_INTERVAL_SECONDS = [string]$ResourceIntervalSeconds

    $records = [System.Collections.Generic.List[object]]::new()
    Push-Location $repoRoot
    try {
        foreach ($mode in $selectedModes) {
            if ($DurationSeconds -ge 1800) {
                $currentState = Get-RepositoryState -RepoRoot $repoRoot
                Assert-ReleaseRepositoryState -State $currentState `
                    -ExpectedCommit $runContext.repository.git_commit
                Assert-FileIdentityMatches -Actual (Get-FileIdentity -Path $cargoLockFull) `
                    -Expected $runContext.cargo_lock -Label 'Cargo.lock'
                Assert-FileIdentityMatches -Actual (Get-FileIdentity -Path $testBinaryFull) `
                    -Expected $runContext.test_binary -Label 'test binary'
            }

            $receiptPath = Join-Path $stagingRootFull "$mode.json"
            $resumeName = "$mode.json"
            if ($null -ne $resumeLease -and $resumeLease.Files.ContainsKey($resumeName)) {
                $resumeBytes = [byte[]]$resumeLease.Files[$resumeName].Bytes
                Write-AtomicBytes -Path $receiptPath -Bytes $resumeBytes
                $receiptIdentity = Get-FileIdentity -Path $receiptPath
                $receipt = [System.Text.UTF8Encoding]::new($false).GetString($resumeBytes) |
                    ConvertFrom-Json -Depth 60
                $env:LATENTDECK_PRIVATE_SOAK_MODE = $mode
                Invoke-StrictReceiptValidator -TestBinary $testBinaryFull `
                    -ReceiptPath $receiptPath -ExpectedLegacySha256 $expectedLegacySha256
                Assert-ReceiptContract -Receipt $receipt -Mode $mode `
                    -DurationSeconds $DurationSeconds -WarmupSeconds $effectiveWarmupSeconds `
                    -ControlIntervalSeconds $ControlIntervalSeconds `
                    -ResourceIntervalSeconds $ResourceIntervalSeconds `
                    -RunContext $runContext -CodecRoot $codecRootFull `
                    -ReceiptIdentity $receiptIdentity `
                    -ExpectedLegacySha256 $expectedLegacySha256 -AllowLegacyV1
                $origin = if ([int]$receipt.schema_version -eq 1) {
                    'legacy_v1_verified'
                }
                else {
                    'resumed_v2_verified'
                }
                Write-Host "REALTIME SOAK: reused $mode ($origin)" -ForegroundColor Yellow
            }
            else {
                $env:LATENTDECK_PRIVATE_SOAK_MODE = $mode
                $env:LATENTDECK_PRIVATE_SOAK_RECEIPT = $receiptPath
                Write-Host "REALTIME SOAK: $mode ($DurationSeconds seconds)" -ForegroundColor Cyan
                & $testBinaryFull private_realtime_soak_mode --ignored --exact --nocapture
                if ($LASTEXITCODE -ne 0) {
                    throw "Private realtime soak mode $mode failed with exit code $LASTEXITCODE."
                }
                if (-not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
                    throw "Mode $mode passed without its required receipt."
                }
                $receipt = Get-Content -Raw -LiteralPath $receiptPath | ConvertFrom-Json -Depth 60
                $receiptIdentity = Get-FileIdentity -Path $receiptPath
                Invoke-StrictReceiptValidator -TestBinary $testBinaryFull `
                    -ReceiptPath $receiptPath
                Assert-ReceiptContract -Receipt $receipt -Mode $mode `
                    -DurationSeconds $DurationSeconds -WarmupSeconds $effectiveWarmupSeconds `
                    -ControlIntervalSeconds $ControlIntervalSeconds `
                    -ResourceIntervalSeconds $ResourceIntervalSeconds `
                    -RunContext $runContext -CodecRoot $codecRootFull `
                    -ReceiptIdentity $receiptIdentity
                $origin = 'executed_v2'
            }

            $isV2 = [int]$receipt.schema_version -eq 2
            $records.Add([pscustomobject]@{
                mode = $mode
                receipt = $receipt
                receipt_sha256 = $receiptIdentity.sha256
                receipt_byte_length = $receiptIdentity.byte_length
                origin = $origin
                provenance_v2_bound = $isV2
                historical_execution_context_available = $isV2
                historical_context_status = if ($isV2) {
                    'exact_v2_execution_context'
                }
                else {
                    'unavailable_legacy_v1'
                }
            })
        }
    }
    finally {
        Pop-Location
    }

    if ($DurationSeconds -ge 1800) {
        $finalState = Get-RepositoryState -RepoRoot $repoRoot
        Assert-ReleaseRepositoryState -State $finalState `
            -ExpectedCommit $runContext.repository.git_commit
        Assert-FileIdentityMatches -Actual (Get-FileIdentity -Path $cargoLockFull) `
            -Expected $runContext.cargo_lock -Label 'Cargo.lock'
        Assert-FileIdentityMatches -Actual (Get-FileIdentity -Path $testBinaryFull) `
            -Expected $runContext.test_binary -Label 'test binary'
    }
    foreach ($record in $records) {
        if ($record.provenance_v2_bound) {
            Assert-ReceiptRuntimeBindings -Receipt $record.receipt -CodecRoot $codecRootFull
        }
        Assert-FileIdentityMatches `
            -Actual (Get-FileIdentity -Path (Join-Path $stagingRootFull "$($record.mode).json")) `
            -Expected ([pscustomobject]@{
                sha256 = $record.receipt_sha256
                byte_length = $record.receipt_byte_length
            }) -Label "$($record.mode) receipt"
    }
    Assert-FileIdentityMatches -Actual (Get-FileIdentity -Path $executionContextPath) `
        -Expected $executionContextIdentity -Label 'execution context'

    $summaries = @($records | ForEach-Object {
        $receipt = $_.receipt
        $controlReceipt = if ($null -ne $receipt.PSObject.Properties['control_to_processed_frame']) {
            $receipt.control_to_processed_frame
        }
        else {
            $receipt.control_to_effect
        }
        [ordered]@{
            mode = $_.mode
            origin = $_.origin
            receipt_schema_version = [int]$receipt.schema_version
            receipt_sha256 = $_.receipt_sha256
            receipt_byte_length = $_.receipt_byte_length
            measurement_gates_passed = [bool]$receipt.release_gates.all_required_gates_passed
            provenance_v2_bound = $_.provenance_v2_bound
            historical_execution_context_available = $_.historical_execution_context_available
            historical_context_status = $_.historical_context_status
            codec_pack = $receipt.runtime.codec_pack
            codec_runtime_inputs = if ($_.provenance_v2_bound) {
                $receipt.codec_runtime_inputs
            }
            else {
                $null
            }
            worker_environment = if ($null -ne $receipt.runtime.PSObject.Properties['worker_environment']) {
                $receipt.runtime.worker_environment
            }
            else {
                $null
            }
            native_renderer_vram_measured = $false
            measured_output_fps = $receipt.presentation.measured_output_fps
            intervals_over_two_frames_rate = $receipt.presentation.intervals_over_two_frames_rate
            control_to_processed_frame_p95_ms = $controlReceipt.p95_ms
            ring_backpressure_delta = $receipt.queue_and_backpressure.worker_ring_backpressure_delta
            worker_ram_end_minus_start_bytes = $receipt.memory.worker_process_private_usage.end_minus_start_bytes
            torch_allocated_end_minus_start_bytes = $receipt.memory.torch_cuda_allocated.end_minus_start_bytes
            torch_reserved_end_minus_start_bytes = $receipt.memory.torch_cuda_reserved.end_minus_start_bytes
        }
    })
    $measurementPassCount = @($records | Where-Object {
        $_.receipt.release_gates.all_required_gates_passed
    }).Count
    $v2Count = @($records | Where-Object { $_.provenance_v2_bound }).Count
    $legacyCount = @($records | Where-Object { $_.origin -ceq 'legacy_v1_verified' }).Count
    $allFourModes = ($selectedModes -join ',') -ceq ($canonicalModes -join ',')
    $allFourMeasurementGates = $DurationSeconds -ge 1800 -and
        $allFourModes -and $measurementPassCount -eq 4
    $allFourV2 = $allFourModes -and $v2Count -eq 4
    $resumableContractPassed = $allFourMeasurementGates -and
        ($v2Count + $legacyCount) -eq 4

    $suite = [ordered]@{
        schema_version = 2
        evidence_kind = 'latentdeck_private_realtime_soak_suite'
        mode_order = @($selectedModes)
        duration_seconds_per_mode = $DurationSeconds
        measurement_schedule = $runContext.measurement
        release_duration_exercised = ($DurationSeconds -ge 1800)
        host_build_profile = 'release'
        current_suite_execution_context = $runContext
        measurement_verdict = [ordered]@{
            required_release_modes = 4
            passed_release_modes = $measurementPassCount
            all_four_measurement_gates_passed = $allFourMeasurementGates
        }
        provenance_verdict = [ordered]@{
            provenance_v2_bound_modes = $v2Count
            legacy_v1_verified_modes = $legacyCount
            all_four_provenance_v2_bound = $allFourV2
            explicit_legacy_policy = 'Only d2-linear schema v1 may be accepted from the immutable initial ResumeRoot inventory when its SHA-256 was explicitly supplied. Its strict schema, raw measurements, independently recomputed gates, codec/source identifiers, and current physical-pack/input match are verified. Historical Git, Cargo.lock, test-binary, worker, and codec-file hashes remain unavailable and are not reconstructed.'
        }
        resumable_release_contract_passed = $resumableContractPassed
        duplicate_source_disclosure = 'Q4 uses B,C,A,B: 3 distinct real AV cartridges across 4 slots; slot D reuses logical B.'
        results = $summaries
        privacy = [ordered]@{
            receipt_is_path_free = $true
            private_payload_embedded = $false
        }
    }
    Assert-PathFreeJson -Value $suite
    $partials = @(Get-ChildItem -LiteralPath $stagingRootFull -File -Recurse -Filter '*.partial')
    if ($partials.Count -ne 0) {
        throw 'Realtime soak left an unfinished evidence partial.'
    }
    $expectedPreSuiteNames = @('execution-context.json') +
        @($selectedModes | ForEach-Object { "$_.json" })
    $actualPreSuiteNames = @(
        Get-ChildItem -LiteralPath $stagingRootFull -Force |
            Sort-Object Name |
            ForEach-Object { $_.Name }
    )
    if (($actualPreSuiteNames -join ',') -cne
        (@($expectedPreSuiteNames | Sort-Object) -join ',')) {
        throw 'Receipt staging root contains unexpected or missing evidence files.'
    }
    if ($null -ne $resumeLease) {
        Assert-ResumeRootUnchanged -Lease $resumeLease
    }

    $stagingSuitePath = Join-Path $stagingRootFull 'suite.json'
    Write-AtomicJson -Path $stagingSuitePath -Value $suite
    $expectedEvidenceNames = @($expectedPreSuiteNames) + 'suite.json'
    $actualEvidenceNames = @(
        Get-ChildItem -LiteralPath $stagingRootFull -Force |
            Sort-Object Name |
            ForEach-Object { $_.Name }
    )
    if (($actualEvidenceNames -join ',') -cne
        (@($expectedEvidenceNames | Sort-Object) -join ',')) {
        throw 'Final receipt staging root is not the exact bounded evidence set.'
    }
    if (Test-Path -LiteralPath $receiptRootFull) {
        throw 'ReceiptRoot appeared before atomic publication; refusing to overwrite it.'
    }
    [System.IO.Directory]::Move($stagingRootFull, $receiptRootFull)
    $stagingPublished = $true
    $suitePath = Join-Path $receiptRootFull 'suite.json'

    Write-Host 'PRIVATE REALTIME SOAK SUITE: COMPLETE' -ForegroundColor Green
    Write-Host "Mode order: $($selectedModes -join ' -> ')"
    Write-Host "Duration: $DurationSeconds seconds per mode"
    Write-Host "Measurement gates: $measurementPassCount/$($selectedModes.Count)"
    Write-Host "Provenance v2: $v2Count/$($selectedModes.Count); legacy v1 verified: $legacyCount"
    Write-Host 'Q4 disclosure: B,C,A,B; 3 distinct real AV sources; D reuses B.'
}
finally {
    foreach ($name in $environmentNames) {
        [System.Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
    }
    if ($null -ne $resumeLease) {
        foreach ($stream in $resumeLease.Locks) {
            $stream.Dispose()
        }
    }
    if (-not $stagingPublished -and (Test-Path -LiteralPath $stagingRootFull)) {
        try {
            $stagingItem = Get-Item -LiteralPath $stagingRootFull -Force
            $stagingRelative = [System.IO.Path]::GetRelativePath($receiptParent, $stagingItem.FullName)
            if ($stagingItem.PSIsContainer -and
                ($stagingItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0 -and
                -not [System.IO.Path]::IsPathFullyQualified($stagingRelative) -and
                -not $stagingRelative.StartsWith('..', [System.StringComparison]::Ordinal) -and
                $stagingItem.Name.StartsWith(".$receiptLeaf.", [System.StringComparison]::Ordinal) -and
                $stagingItem.Name.EndsWith('.partial', [System.StringComparison]::Ordinal)) {
                [System.IO.Directory]::Delete($stagingItem.FullName, $true)
            }
        }
        catch {
            Write-Warning 'Failed to remove the private soak staging directory after an unsuccessful run.'
        }
    }
}

Write-Output $suitePath
