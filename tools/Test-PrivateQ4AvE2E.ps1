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

    [ValidateSet('DuplicateReuse', 'FourIndependent')]
    [string]$Acceptance = 'FourIndependent',

    [string]$SourceD = '',

    [Parameter(Mandatory)]
    [string]$ReceiptPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Resolve-ExistingFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction Stop
    $item = Get-Item -LiteralPath $resolved.Path -Force
    if (-not $item.PSIsContainer -and $item.Length -gt 0) {
        return $item.FullName
    }
    throw "$Label must be an existing non-empty file."
}

function Resolve-ExistingDirectory {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $resolved = Resolve-Path -LiteralPath $Path -ErrorAction Stop
    $item = Get-Item -LiteralPath $resolved.Path -Force
    if ($item.PSIsContainer) {
        return $item.FullName
    }
    throw "$Label must be an existing directory."
}

function Assert-NoReparseAncestor {
    param(
        [Parameter(Mandatory)]
        [string]$Root,

        [Parameter(Mandatory)]
        [string]$Candidate
    )

    $cursor = [System.IO.Path]::GetDirectoryName($Candidate)
    while (-not [string]::IsNullOrWhiteSpace($cursor)) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'ReceiptPath must not traverse a reparse-point ancestor.'
            }
        }
        if ($cursor.Equals($Root, [System.StringComparison]::OrdinalIgnoreCase)) {
            return
        }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ($parent -eq $cursor) {
            break
        }
        $cursor = $parent
    }
    throw 'ReceiptPath ancestry could not be validated beneath artifacts.'
}

function Invoke-GitText {
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot,

        [Parameter(Mandatory)]
        [string[]]$GitArguments
    )

    $lines = @(& git -C $RepositoryRoot @GitArguments 2>&1)
    if ($LASTEXITCODE -ne 0) {
        throw 'Git could not establish the private Q4 evidence execution context.'
    }
    return ($lines -join "`n").Trim()
}

function Test-Sha256 {
    param([AllowNull()][object]$Value)
    return ($Value -is [string] -and $Value -cmatch '^[0-9a-f]{64}$')
}

function Test-GitObjectId {
    param([AllowNull()][object]$Value)
    return ($Value -is [string] -and $Value -cmatch '^([0-9a-f]{40}|[0-9a-f]{64})$')
}

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$gitStatus = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @(
    'status', '--porcelain=v1', '--untracked-files=all'
)
if (-not [string]::IsNullOrEmpty($gitStatus)) {
    throw 'Private Q4 evidence requires a clean index, worktree, and untracked state.'
}
$expectedCommit = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @(
    'rev-parse', '--verify', 'HEAD'
)
$expectedHeadTree = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @(
    'rev-parse', '--verify', 'HEAD^{tree}'
)
$expectedIndexTree = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @('write-tree')
if ($expectedHeadTree -cne $expectedIndexTree) {
    throw 'Private Q4 evidence requires the Git index tree to equal the committed HEAD tree.'
}
$cargoLockPath = Join-Path $repoRoot 'Cargo.lock'
$expectedCargoLockSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $cargoLockPath).Hash.ToLowerInvariant()
$artifactsRoot = Join-Path $repoRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$artifactsRoot = (Resolve-Path -LiteralPath $artifactsRoot).Path

$codecRootFull = Resolve-ExistingDirectory -Path $CodecRoot -Label 'CodecRoot'
$taeh3Full = Resolve-ExistingFile -Path $Taeh3 -Label 'Taeh3'
$sourceAFull = Resolve-ExistingFile -Path $SourceA -Label 'SourceA'
$sourceBFull = Resolve-ExistingFile -Path $SourceB -Label 'SourceB'
$sourceCFull = Resolve-ExistingFile -Path $SourceC -Label 'SourceC'
$sourceDFull = $null
if ($Acceptance -ceq 'FourIndependent') {
    if ([string]::IsNullOrWhiteSpace($SourceD)) {
        throw 'SourceD is required for FourIndependent acceptance.'
    }
    $sourceDFull = Resolve-ExistingFile -Path $SourceD -Label 'SourceD'
}
elseif (-not [string]::IsNullOrWhiteSpace($SourceD)) {
    throw 'SourceD is accepted only with -Acceptance FourIndependent; duplicate reuse remains an explicit separate path.'
}
$physicalSourcePaths = if ($Acceptance -ceq 'FourIndependent') {
    @($sourceAFull, $sourceBFull, $sourceCFull, $sourceDFull)
}
else {
    @($sourceBFull, $sourceCFull, $sourceAFull, $sourceBFull)
}
$expectedSourceArchiveHashes = @(
    $physicalSourcePaths | ForEach-Object {
        (Get-FileHash -Algorithm SHA256 -LiteralPath $_).Hash.ToLowerInvariant()
    }
)
$expectedDistinctArchiveCount = if ($Acceptance -ceq 'FourIndependent') { 4 } else { 3 }
if (@($expectedSourceArchiveHashes | Sort-Object -Unique).Count -ne $expectedDistinctArchiveCount) {
    throw "The selected Q4 proof requires exactly $expectedDistinctArchiveCount distinct source archives."
}

$receiptFull = [System.IO.Path]::GetFullPath($ReceiptPath)
if ([System.IO.Path]::GetExtension($receiptFull) -cne '.json') {
    throw 'ReceiptPath must end in .json.'
}
$relativeReceipt = [System.IO.Path]::GetRelativePath($artifactsRoot, $receiptFull)
if ($relativeReceipt -eq '.' -or
    $relativeReceipt.StartsWith('..', [System.StringComparison]::Ordinal) -or
    [System.IO.Path]::IsPathFullyQualified($relativeReceipt)) {
    throw 'ReceiptPath must be a new file below the repository artifacts directory.'
}
Assert-NoReparseAncestor -Root $artifactsRoot -Candidate $receiptFull
if (Test-Path -LiteralPath $receiptFull) {
    throw "Refusing to replace an existing private Q4 evidence receipt: $receiptFull"
}
$receiptParent = [System.IO.Path]::GetDirectoryName($receiptFull)
[System.IO.Directory]::CreateDirectory($receiptParent) | Out-Null

$environmentNames = @(
    'LATENTDECK_PRIVATE_Q4_EXTERNAL_AV_E2E',
    'LATENTDECK_PRIVATE_CODEC_ROOT',
    'LATENTDECK_PRIVATE_TAEH3',
    'LATENTDECK_PRIVATE_Q4_SOURCE_A',
    'LATENTDECK_PRIVATE_Q4_SOURCE_B',
    'LATENTDECK_PRIVATE_Q4_SOURCE_C',
    'LATENTDECK_PRIVATE_Q4_SOURCE_D',
    'LATENTDECK_PRIVATE_Q4_RECEIPT'
)
$previous = @{}
foreach ($name in $environmentNames) {
    $previous[$name] = [System.Environment]::GetEnvironmentVariable($name, 'Process')
}

try {
    $env:LATENTDECK_PRIVATE_Q4_EXTERNAL_AV_E2E = '1'
    $env:LATENTDECK_PRIVATE_CODEC_ROOT = $codecRootFull
    $env:LATENTDECK_PRIVATE_TAEH3 = $taeh3Full
    $env:LATENTDECK_PRIVATE_Q4_SOURCE_A = $sourceAFull
    $env:LATENTDECK_PRIVATE_Q4_SOURCE_B = $sourceBFull
    $env:LATENTDECK_PRIVATE_Q4_SOURCE_C = $sourceCFull
    if ($Acceptance -ceq 'FourIndependent') {
        $env:LATENTDECK_PRIVATE_Q4_SOURCE_D = $sourceDFull
    }
    else {
        Remove-Item Env:LATENTDECK_PRIVATE_Q4_SOURCE_D -ErrorAction SilentlyContinue
    }
    $env:LATENTDECK_PRIVATE_Q4_RECEIPT = $receiptFull

    $testName = if ($Acceptance -ceq 'FourIndependent') {
        'private_external_a_b_c_d_av_q4_release_proof'
    }
    else {
        'private_external_b_c_a_b_av_q4_functional_proof'
    }

    Push-Location $repoRoot
    try {
        & cargo test --locked -p latentdeck-app --test private_q4_worker_e2e `
            $testName -- `
            --ignored --exact --nocapture
        if ($LASTEXITCODE -ne 0) {
            throw "Private external AV Q4 E2E failed with exit code $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    foreach ($name in $environmentNames) {
        [System.Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
    }
}

if (-not (Test-Path -LiteralPath $receiptFull -PathType Leaf)) {
    throw 'Private external AV Q4 E2E passed without its required evidence receipt.'
}
$receipt = Get-Content -Raw -LiteralPath $receiptFull | ConvertFrom-Json -Depth 40
$postGitStatus = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @(
    'status', '--porcelain=v1', '--untracked-files=all'
)
$postCommit = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @(
    'rev-parse', '--verify', 'HEAD'
)
$postHeadTree = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @(
    'rev-parse', '--verify', 'HEAD^{tree}'
)
$postIndexTree = Invoke-GitText -RepositoryRoot $repoRoot -GitArguments @('write-tree')
$postCargoLockSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $cargoLockPath).Hash.ToLowerInvariant()
$postSourceArchiveHashes = @(
    $physicalSourcePaths | ForEach-Object {
        (Get-FileHash -Algorithm SHA256 -LiteralPath $_).Hash.ToLowerInvariant()
    }
)
if (-not [string]::IsNullOrEmpty($postGitStatus) -or
    $postCommit -cne $expectedCommit -or
    $postHeadTree -cne $expectedHeadTree -or
    $postIndexTree -cne $expectedIndexTree -or
    $postCargoLockSha256 -cne $expectedCargoLockSha256 -or
    ($postSourceArchiveHashes -join ',') -cne ($expectedSourceArchiveHashes -join ',')) {
    throw 'Private Q4 execution changed, or was detached from, its clean committed source/input context.'
}

$sourceReceipts = @($receipt.sources)
$sourceIds = @($sourceReceipts | ForEach-Object { $_.cartridge_id })
$sourceArchiveHashes = @($sourceReceipts | ForEach-Object { $_.archive_sha256 })
$sourceVideoHashes = @($sourceReceipts | ForEach-Object { $_.video_payload_sha256 })
$distinctSourceIds = @($sourceIds | Sort-Object -Unique)
$distinctArchiveHashes = @($sourceArchiveHashes | Sort-Object -Unique)
$distinctVideoHashes = @($sourceVideoHashes | Sort-Object -Unique)
$invalidSourceReceipt = ($sourceReceipts.Count -ne 4)
$lineageAnchorCount = 0
$allLineageAnchors = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
$lineagePairwiseDisjoint = $true
foreach ($source in $sourceReceipts) {
    $parsedCartridgeId = [guid]::Empty
    if (-not [guid]::TryParse([string]$source.cartridge_id, [ref]$parsedCartridgeId) -or
        -not (Test-Sha256 -Value $source.archive_sha256) -or
        -not (Test-Sha256 -Value $source.video_payload_sha256)) {
        $invalidSourceReceipt = $true
    }
    $lineageBasis = [string]$source.lineage_basis
    $lineageAnchors = @($source.lineage_anchors)
    if (@('original_self', 'declared_immediate_parents') -cnotcontains $lineageBasis -or
        $lineageAnchors.Count -eq 0) {
        $invalidSourceReceipt = $true
    }
    $sourceLineageAnchors = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($anchor in $lineageAnchors) {
        $parsedAnchorId = [guid]::Empty
        if (-not [guid]::TryParse([string]$anchor.cartridge_id, [ref]$parsedAnchorId) -or
            -not (Test-Sha256 -Value $anchor.archive_sha256)) {
            $invalidSourceReceipt = $true
            continue
        }
        $anchorKey = '{0}:{1}' -f ([string]$anchor.cartridge_id).ToLowerInvariant(), `
            ([string]$anchor.archive_sha256).ToLowerInvariant()
        if (-not $sourceLineageAnchors.Add($anchorKey)) {
            $invalidSourceReceipt = $true
        }
        if (-not $allLineageAnchors.Add($anchorKey)) {
            $lineagePairwiseDisjoint = $false
        }
        $lineageAnchorCount++
    }
    if ($lineageBasis -ceq 'original_self' -and
        ($lineageAnchors.Count -ne 1 -or
        [string]$lineageAnchors[0].cartridge_id -cne [string]$source.cartridge_id -or
        [string]$lineageAnchors[0].archive_sha256 -cne [string]$source.archive_sha256)) {
        $invalidSourceReceipt = $true
    }
}
$distinctLineageAnchorCount = $allLineageAnchors.Count

$context = $receipt.execution_context
$contextFailed = ($null -eq $context -or
    $context.schema_version -ne 1 -or
    -not (Test-GitObjectId -Value $context.git.commit) -or
    -not (Test-GitObjectId -Value $context.git.head_tree) -or
    -not (Test-GitObjectId -Value $context.git.index_tree) -or
    $context.git.commit -cne $expectedCommit -or
    $context.git.head_tree -cne $expectedHeadTree -or
    $context.git.index_tree -cne $expectedIndexTree -or
    -not $context.git.index_clean -or
    -not $context.git.worktree_clean -or
    -not $context.git.untracked_clean -or
    -not (Test-Sha256 -Value $context.cargo_lock.sha256) -or
    $context.cargo_lock.sha256 -cne $expectedCargoLockSha256 -or
    $context.cargo_lock.byte_length -le 0 -or
    -not (Test-Sha256 -Value $context.test_executable.sha256) -or
    $context.test_executable.byte_length -le 0 -or
    -not (Test-Sha256 -Value $context.worker_executable.sha256) -or
    $context.worker_executable.byte_length -le 0 -or
    -not (Test-Sha256 -Value $context.codec_pack_manifest.sha256) -or
    $context.codec_pack_manifest.byte_length -le 0 -or
    -not (Test-Sha256 -Value $context.codec_pack_integrity_catalog.sha256) -or
    $context.codec_pack_integrity_catalog.byte_length -le 0 -or
    -not $context.codec_pack_integrity_catalog.validated -or
    $context.codec_pack_integrity_catalog.sha256 -cne $receipt.codec_pack.integrity_catalog_sha256)

$commonReceiptFailed = ($receipt.schema_version -ne 2 -or
    $invalidSourceReceipt -or
    $contextFailed -or
    $receipt.lineage_rule -cne 'declared_immediate_parents_or_original_self' -or
    $receipt.lineage_anchor_count -ne $lineageAnchorCount -or
    $receipt.distinct_lineage_anchor_count -ne $distinctLineageAnchorCount -or
    $receipt.lineage_pairwise_disjoint -isnot [bool] -or
    $receipt.lineage_pairwise_disjoint -ne $lineagePairwiseDisjoint -or
    -not (Test-Sha256 -Value $receipt.codec_pack.integrity_catalog_sha256) -or
    -not (Test-Sha256 -Value $receipt.codec_pack.decoder_sha256) -or
    $receipt.codec_pack.decoder_byte_length -le 0 -or
    -not $receipt.effects.topk_vs_sinkhorn_distinct -or
    -not $receipt.effects.deterministic_restart_replay -or
    -not $receipt.effects.carrier_reassignment.distinct_from_preceding_sinkhorn -or
    $receipt.snapshot.audio_policy -cne 'copied_from_carrier_exact' -or
    -not $receipt.snapshot.audio_bytes_identical -or
    -not $receipt.snapshot.reload_passed -or
    $receipt.live_capture.audio_policy -cne 'omitted_timing_mismatch' -or
    $receipt.live_capture.audio_policy_reason -cne 'duration_mismatch' -or
    -not $receipt.live_capture.reload_passed -or
    $receipt.cleanup.partial_files_remaining)
$topologyReceiptFailed = if ($Acceptance -ceq 'FourIndependent') {
    ($receipt.test_id -cne 'private_external_a_b_c_d_av_q4_release_proof' -or
        $receipt.acceptance_class -cne 'release_four_independent' -or
        $receipt.result -cne 'passed' -or
        ($receipt.source_order -join ',') -cne 'A,B,C,D' -or
        $distinctSourceIds.Count -ne 4 -or
        $distinctArchiveHashes.Count -ne 4 -or
        $distinctVideoHashes.Count -ne 4 -or
        ($sourceArchiveHashes -join ',') -cne ($expectedSourceArchiveHashes -join ',') -or
        $receipt.distinct_cartridge_id_count -ne 4 -or
        $receipt.distinct_archive_count -ne 4 -or
        $receipt.distinct_video_payload_count -ne 4 -or
        -not $lineagePairwiseDisjoint -or
        -not $receipt.lineage_pairwise_disjoint -or
        -not $receipt.four_independent_source_acceptance -or
        $null -ne $receipt.duplicate_binding)
}
else {
    ($receipt.test_id -cne 'private_external_b_c_a_b_av_q4_functional_proof' -or
        $receipt.acceptance_class -cne 'functional_duplicate_reuse_only' -or
        $receipt.result -cne 'functional_only_passed' -or
        ($receipt.source_order -join ',') -cne 'B,C,A,B' -or
        $distinctSourceIds.Count -ne 3 -or
        $distinctArchiveHashes.Count -ne 3 -or
        $distinctVideoHashes.Count -ne 3 -or
        ($sourceArchiveHashes -join ',') -cne ($expectedSourceArchiveHashes -join ',') -or
        $receipt.distinct_cartridge_id_count -ne 3 -or
        $receipt.distinct_archive_count -ne 3 -or
        $receipt.distinct_video_payload_count -ne 3 -or
        $lineagePairwiseDisjoint -or
        $receipt.lineage_pairwise_disjoint -or
        $receipt.four_independent_source_acceptance -or
        $null -eq $receipt.duplicate_binding -or
        $sourceIds[0] -cne $sourceIds[3] -or
        $sourceArchiveHashes[0] -cne $sourceArchiveHashes[3] -or
        $sourceVideoHashes[0] -cne $sourceVideoHashes[3])
}
if ($commonReceiptFailed -or $topologyReceiptFailed) {
    throw 'Private external AV Q4 evidence receipt does not satisfy the selected proof contract.'
}

if ($Acceptance -ceq 'FourIndependent') {
    Write-Host 'PRIVATE EXTERNAL AV Q4 RELEASE ACCEPTANCE: PASS' -ForegroundColor Green
    Write-Host 'Physical slot order: A,B,C,D (four distinct cartridge IDs, archives, video payloads, and disjoint declared lineage anchors).'
}
else {
    Write-Host 'PRIVATE EXTERNAL AV Q4 FUNCTIONAL ONLY: PASS' -ForegroundColor Yellow
    Write-Host 'Physical slot order: B,C,A,B (three distinct source identities; D reuses logical B).'
    Write-Host 'This result never satisfies LatentDeck v0.1 Q4 release acceptance.' -ForegroundColor Yellow
}
Write-Output $receiptFull
