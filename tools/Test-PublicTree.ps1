[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$gitDirectory = Join-Path $repoRoot '.git'

if (-not (Test-Path -LiteralPath $gitDirectory -PathType Container)) {
    Write-Error 'PUBLIC TREE AUDIT: Git repository is not initialized.'
    exit 1
}

$candidatePaths = @(
    & git -c core.quotepath=false -C $repoRoot ls-files --cached --others --exclude-standard
)

if ($LASTEXITCODE -ne 0) {
    Write-Error 'PUBLIC TREE AUDIT: Could not enumerate candidate files.'
    exit 1
}

$candidatePaths = @($candidatePaths | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$failures = [System.Collections.Generic.List[string]]::new()
$warnings = [System.Collections.Generic.List[string]]::new()
$maximumBytes = 25MB

$forbiddenExtensions = @(
    '.lc', '.h3latent', '.safetensors', '.ckpt', '.pt', '.pth', '.onnx',
    '.engine', '.plan', '.gguf', '.bin', '.p12', '.pfx', '.key', '.pem',
    '.exe', '.dll', '.msi', '.msix', '.appx', '.zip', '.7z', '.rar',
    '.tar', '.tgz', '.gz', '.mp4', '.mov', '.mkv', '.avi', '.webm',
    '.wav', '.flac', '.mp3'
)

$forbiddenLeafNames = @(
    '.env', 'credentials.json', 'credentials.toml', 'secrets.json',
    'secrets.toml'
)

$textExtensions = @(
    '.md', '.txt', '.json', '.toml', '.yaml', '.yml', '.rs', '.py',
    '.js', '.ts', '.tsx', '.svelte', '.css', '.html', '.xml'
)

$selfRelativePath = 'tools/Test-PublicTree.ps1'
$totalBytes = [int64]0

foreach ($relativePath in $candidatePaths) {
    $normalizedPath = $relativePath.Replace('\', '/')
    $fullPath = Join-Path $repoRoot $relativePath

    if (-not (Test-Path -LiteralPath $fullPath -PathType Leaf)) {
        $failures.Add("Missing candidate file: $normalizedPath")
        continue
    }

    $file = Get-Item -LiteralPath $fullPath
    $totalBytes += $file.Length
    $extension = $file.Extension.ToLowerInvariant()
    $leafName = $file.Name.ToLowerInvariant()

    if ($forbiddenExtensions -contains $extension) {
        $failures.Add("Forbidden public payload type: $normalizedPath")
    }

    if (($forbiddenLeafNames -contains $leafName) -or $leafName.StartsWith('.env.')) {
        if ($leafName -ne '.env.example') {
            $failures.Add("Credential or local-config filename: $normalizedPath")
        }
    }

    if ($file.Length -gt $maximumBytes) {
        $failures.Add("File exceeds 25 MiB review threshold: $normalizedPath ($($file.Length) bytes)")
    }

    if (($textExtensions -contains $extension) -and
        ($file.Length -le 4MB) -and
        ($normalizedPath -ne $selfRelativePath)) {
        $content = Get-Content -LiteralPath $fullPath -Raw

        if ($content -match '(?im)(?:^|[\s"''])[A-Za-z]:\\(?:Users|ComfyUI|h3-pipeline|runpod-minimax-setup|latentdeck)\\') {
            $failures.Add("Machine-local Windows path in public text: $normalizedPath")
        }

        if ($content -match '(?im)/(?:Users|home)/[^/\s]+/') {
            $failures.Add("Machine-local Unix path in public text: $normalizedPath")
        }

        if ($content -match '(?im)\b(?:api[_-]?key|access[_-]?token|secret|password)\b\s*[:=]\s*["''][^"''\r\n\s]{8,}') {
            $failures.Add("Credential-like assignment in public text: $normalizedPath")
        }
    }
}

$trackedIgnored = @(
    & git -c core.quotepath=false -C $repoRoot ls-files --cached --ignored --exclude-standard
)

if ($LASTEXITCODE -ne 0) {
    $warnings.Add('Could not check for tracked files that now match .gitignore.')
} else {
    foreach ($relativePath in $trackedIgnored) {
        if (-not [string]::IsNullOrWhiteSpace($relativePath)) {
            $failures.Add("Tracked file is now ignored: $($relativePath.Replace('\', '/'))")
        }
    }
}

& git -C $repoRoot diff --check
if ($LASTEXITCODE -ne 0) {
    $failures.Add('git diff --check reported whitespace errors.')
}

if ($warnings.Count -gt 0) {
    Write-Host 'WARNINGS:' -ForegroundColor Yellow
    foreach ($warning in $warnings) {
        Write-Host "  - $warning" -ForegroundColor Yellow
    }
}

if ($failures.Count -gt 0) {
    Write-Host 'PUBLIC TREE AUDIT: FAIL' -ForegroundColor Red
    foreach ($failure in $failures) {
        Write-Host "  - $failure" -ForegroundColor Red
    }
    exit 1
}

Write-Host 'PUBLIC TREE AUDIT: PASS' -ForegroundColor Green
Write-Host "Candidate files: $($candidatePaths.Count)"
Write-Host "Candidate bytes: $totalBytes"
Write-Host 'Note: ignored local files are not publication candidates; review git status --ignored before release.'
