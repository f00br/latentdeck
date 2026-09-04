[CmdletBinding()]
param(
    [string]$RepositoryRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..'
}
$repositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path
$utf8 = [System.Text.UTF8Encoding]::new($false, $true)

function Add-Failure {
    param(
        [System.Collections.Generic.List[string]]$Failures,

        [Parameter(Mandatory)]
        [string]$Message
    )

    $Failures.Add($Message)
}

function Convert-ToRepositoryPath {
    param([Parameter(Mandatory)][string]$Path)

    return [System.IO.Path]::GetRelativePath($repositoryRoot, $Path).Replace('\', '/')
}

function Get-MarkdownLinesOutsideFences {
    param([Parameter(Mandatory)][string]$Text)

    $result = [System.Collections.Generic.List[string]]::new()
    $insideFence = $false
    $fenceCharacter = [char]0
    $fenceLength = 0

    foreach ($line in ($Text -split '\r?\n')) {
        if (-not $insideFence -and $line -match '^\s{0,3}(?<marker>`{3,}|~{3,})') {
            $insideFence = $true
            $fenceCharacter = $Matches['marker'][0]
            $fenceLength = $Matches['marker'].Length
            $result.Add('')
            continue
        }

        if ($insideFence) {
            $isClosingFence = (
                ($fenceCharacter -eq '`' -and $line -match '^\s{0,3}`{3,}\s*$') -or
                ($fenceCharacter -eq '~' -and $line -match '^\s{0,3}~{3,}\s*$')
            )
            if ($isClosingFence) {
                $marker = $line.Trim()
                if ($marker.Length -ge $fenceLength) {
                    $insideFence = $false
                    $fenceCharacter = [char]0
                    $fenceLength = 0
                }
            }
            $result.Add('')
            continue
        }

        $result.Add($line)
    }

    return $result.ToArray()
}

function ConvertTo-GitHubHeadingSlug {
    param([Parameter(Mandatory)][string]$Heading)

    $visible = [regex]::Replace($Heading, '!\[(?<label>[^\]]*)\]\([^)]*\)', '${label}')
    $visible = [regex]::Replace($visible, '\[(?<label>[^\]]+)\]\([^)]*\)', '${label}')
    $visible = [regex]::Replace($visible, '<[^>]+>', '')
    $visible = [System.Net.WebUtility]::HtmlDecode($visible)
    $visible = [regex]::Replace($visible, '[`*_~]', '')
    $visible = $visible.Trim().ToLowerInvariant()
    $visible = [regex]::Replace($visible, '[^\p{L}\p{M}\p{N}\s_-]', '')
    return [regex]::Replace($visible, '\s', '-')
}

function Get-MarkdownAnchors {
    param([Parameter(Mandatory)][string]$Text)

    $anchors = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $generated = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    $lines = @(Get-MarkdownLinesOutsideFences -Text $Text)

    for ($index = 0; $index -lt $lines.Count; $index++) {
        $line = $lines[$index]

        foreach ($explicitMatch in [regex]::Matches(
            $line,
            '(?i)<(?:a|span)\b[^>]*(?:id|name)\s*=\s*["''](?<anchor>[^"'']+)["''][^>]*>'
        )) {
            [void]$anchors.Add($explicitMatch.Groups['anchor'].Value)
        }

        $heading = $null
        if ($line -match '^\s{0,3}#{1,6}[ \t]+(?<heading>.*?)[ \t]*#*[ \t]*$') {
            $heading = $Matches['heading']
        } elseif (
            $index -gt 0 -and
            -not [string]::IsNullOrWhiteSpace($lines[$index - 1]) -and
            $line -match '^\s{0,3}(?:=+|-+)\s*$'
        ) {
            $heading = $lines[$index - 1].Trim()
        }

        if ($null -eq $heading) {
            continue
        }

        $baseSlug = ConvertTo-GitHubHeadingSlug -Heading $heading
        if ([string]::IsNullOrWhiteSpace($baseSlug)) {
            continue
        }
        $slug = $baseSlug
        $suffix = 0
        while ($generated.Contains($slug)) {
            $suffix++
            $slug = "$baseSlug-$suffix"
        }
        [void]$generated.Add($slug)
        [void]$anchors.Add($slug)
    }

    return ,$anchors
}

$candidatePaths = @(
    & git -C $repositoryRoot -c core.quotepath=false ls-files `
        --cached --others --exclude-standard
)
if ($LASTEXITCODE -ne 0 -or $candidatePaths.Count -eq 0) {
    throw 'Could not enumerate public files from the index and working tree.'
}
$candidatePaths = @(
    $candidatePaths |
        ForEach-Object { $_.Replace('\', '/') } |
        Sort-Object -Unique
)
$candidatePathSet = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
$candidateDirectorySet = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
[void]$candidateDirectorySet.Add('')
foreach ($candidatePath in $candidatePaths) {
    [void]$candidatePathSet.Add($candidatePath)
    $components = @($candidatePath -split '/')
    for ($componentCount = 1; $componentCount -lt $components.Count; $componentCount++) {
        [void]$candidateDirectorySet.Add(
            ($components[0..($componentCount - 1)] -join '/')
        )
    }
}

$candidateMarkdown = @(
    & git -C $repositoryRoot -c core.quotepath=false ls-files `
        --cached --others --exclude-standard -- '*.md'
)
if ($LASTEXITCODE -ne 0 -or $candidateMarkdown.Count -eq 0) {
    throw 'Could not enumerate public Markdown files from the index and working tree.'
}
$publicMarkdown = @(
    $candidateMarkdown |
        ForEach-Object { $_.Replace('\', '/') } |
        Sort-Object -Unique |
        Where-Object { Test-Path -LiteralPath (Join-Path $repositoryRoot $_) -PathType Leaf }
)
$indexedPaths = @(
    & git -C $repositoryRoot -c core.quotepath=false ls-files --cached
)
if ($LASTEXITCODE -ne 0) {
    throw 'Could not enumerate the repository index.'
}

$retiredPaths = @(
    'docs/CONCEPT.md',
    'docs/latent_concept.md',
    'docs/assets/concepts/README.md',
    'docs/release/continue.md'
)
$indexedPathSet = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
foreach ($relativePath in $indexedPaths) {
    [void]$indexedPathSet.Add($relativePath.Replace('\', '/'))
}

$failures = [System.Collections.Generic.List[string]]::new()
$anchorCache = [System.Collections.Generic.Dictionary[
    string,
    System.Collections.Generic.HashSet[string]
]]::new([System.StringComparer]::OrdinalIgnoreCase)
$rootPrefix = $repositoryRoot.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
foreach ($retiredPath in $retiredPaths) {
    if ($indexedPathSet.Contains($retiredPath)) {
        Add-Failure -Failures $failures -Message "retired path remains in the index: $retiredPath"
    }
    if (Test-Path -LiteralPath (Join-Path $repositoryRoot $retiredPath)) {
        Add-Failure -Failures $failures -Message "retired path remains on disk: $retiredPath"
    }
}

$internalMarkers = [ordered]@{
    'internal handoff language' = '(?i)\b(?:operational handoff|current handoff|next agent)\b'
    'owner UAT language' = '(?i)\b(?:owner[- ]accepted|owner UAT|owner first-install|pending owner)\b'
    'obsolete implementation checkpoint' = '(?i)\b(?:3648e7c|0fd1303|dbe310a|bf1e189|e95ff74)\b'
    'retired concept path' = '(?i)docs/(?:CONCEPT\.md|latent_concept\.md|assets/concepts/)'
    'retired release handoff path' = '(?i)docs/release/continue\.md'
    'retired latentdeck.org domain' = '(?i)https?://(?:www\.)?latentdeck\.org(?:/|\b)'
}

foreach ($relativePath in $publicMarkdown) {
    $fullPath = Join-Path $repositoryRoot $relativePath
    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        continue
    }
    $item = Get-Item -LiteralPath $fullPath -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        Add-Failure -Failures $failures -Message "tracked Markdown is a reparse point: $relativePath"
        continue
    }
    try {
        $text = $utf8.GetString([System.IO.File]::ReadAllBytes($fullPath))
    } catch [System.Text.DecoderFallbackException] {
        Add-Failure -Failures $failures -Message "Markdown is not strict UTF-8: $relativePath"
        continue
    }

    if ($text -match '[\u0400-\u04FF]') {
        Add-Failure -Failures $failures -Message "Cyrillic prose remains in canonical English documentation: $relativePath"
    }
    if ($text -match '(?im)(?:^|[\s`"''(=])(?:file:///)?[A-Za-z]:[\\/]') {
        Add-Failure -Failures $failures -Message "machine-local Windows path remains in documentation: $relativePath"
    }
    if ($text -match '(?im)/(?:Users|home)/[^/\s]+/') {
        Add-Failure -Failures $failures -Message "machine-local user path remains in documentation: $relativePath"
    }
    foreach ($marker in $internalMarkers.GetEnumerator()) {
        if ($text -match $marker.Value) {
            Add-Failure -Failures $failures -Message "$($marker.Key) remains in: $relativePath"
        }
    }
    if ($relativePath -in @(
        'docs/guides/WINDOWS_INSTALL.md',
        'comfy/latent-cartridge/README.md',
        'comfy/latent-cartridge/packaging/BUNDLE_README.md'
    ) -and $text -match '(?i)(?:pip|uv\s+pip)\s+install[^\r\n]*(?:latentdeck[-_]cartridge|safetensors)') {
        Add-Failure `
            -Failures $failures `
            -Message "obsolete manual Recorder dependency installation remains in: $relativePath"
    }

    $linkText = (Get-MarkdownLinesOutsideFences -Text $text) -join "`n"
    $linkMatches = [regex]::Matches(
        $linkText,
        '(?m)!?\[[^\]\r\n]*\]\((?<target><[^>]+>|[^\s)]+)(?:\s+["''][^"'']*["''])?\)'
    )
    foreach ($match in $linkMatches) {
        $target = $match.Groups['target'].Value.Trim('<', '>')
        if ([string]::IsNullOrWhiteSpace($target) -or
            $target -match '^(?i)(?:https?|mailto):') {
            continue
        }

        $targetParts = @($target -split '#', 2)
        $pathPart = $targetParts[0]
        $fragmentPart = if ($targetParts.Count -eq 2) { $targetParts[1] } else { $null }
        $pathPart = ($pathPart -split '\?', 2)[0]
        try {
            $decodedPath = [System.Uri]::UnescapeDataString($pathPart).Replace('/', '\')
            $decodedFragment = if ($null -ne $fragmentPart) {
                [System.Uri]::UnescapeDataString($fragmentPart)
            } else {
                $null
            }
        } catch {
            Add-Failure -Failures $failures -Message "invalid encoded local link '$target' in: $relativePath"
            continue
        }

        $linkPath = if ([string]::IsNullOrWhiteSpace($decodedPath)) {
            [System.IO.Path]::GetFullPath($fullPath)
        } else {
            [System.IO.Path]::GetFullPath(
                (Join-Path (Split-Path -Parent $fullPath) $decodedPath)
            )
        }
        if (-not $linkPath.StartsWith($rootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            Add-Failure -Failures $failures -Message (
                "missing or out-of-tree local link '$target' in: $relativePath"
            )
            continue
        }

        $spelledRepositoryPath = [System.IO.Path]::GetRelativePath(
            $repositoryRoot,
            $linkPath
        ).Replace('\', '/')
        $targetIsCandidate = (
            $candidatePathSet.Contains($spelledRepositoryPath) -or
            $candidateDirectorySet.Contains($spelledRepositoryPath)
        )
        if (-not $targetIsCandidate) {
            Add-Failure -Failures $failures -Message (
                "local link path is not a public candidate or has incorrect case '$target' in: " +
                $relativePath
            )
            continue
        }
        if (-not (Test-Path -LiteralPath $linkPath)) {
            Add-Failure -Failures $failures -Message (
                "missing local link '$target' in: $relativePath"
            )
            continue
        }

        if ([string]::IsNullOrWhiteSpace($decodedFragment)) {
            continue
        }

        $anchorPath = $linkPath
        if (Test-Path -LiteralPath $anchorPath -PathType Container) {
            $anchorPath = Join-Path $anchorPath 'README.md'
        }
        if (
            -not $anchorPath.EndsWith('.md', [System.StringComparison]::OrdinalIgnoreCase) -or
            -not (Test-Path -LiteralPath $anchorPath -PathType Leaf)
        ) {
            continue
        }

        if (-not $anchorCache.ContainsKey($anchorPath)) {
            try {
                $targetText = $utf8.GetString([System.IO.File]::ReadAllBytes($anchorPath))
                $anchorCache[$anchorPath] = Get-MarkdownAnchors -Text $targetText
            } catch [System.Text.DecoderFallbackException] {
                Add-Failure -Failures $failures -Message (
                    "Markdown link target is not strict UTF-8: " +
                    (Convert-ToRepositoryPath -Path $anchorPath)
                )
                continue
            }
        }
        if (-not $anchorCache[$anchorPath].Contains($decodedFragment)) {
            Add-Failure -Failures $failures -Message (
                "missing local Markdown fragment '#$decodedFragment' in: " +
                "$relativePath -> $target"
            )
        }
    }
}

if ($failures.Count -gt 0) {
    $details = ($failures | Sort-Object -Unique | ForEach-Object { " - $_" }) -join "`n"
    throw "Public documentation audit failed:`n$details"
}

Write-Host (
    "PUBLIC DOCUMENTATION: PASS ($($publicMarkdown.Count) indexed/untracked English Markdown files)"
) -ForegroundColor Green
