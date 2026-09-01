[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$PackVersion,

    [Parameter(Mandatory)]
    [string]$OutputPath,

    [switch]$AllowNetwork
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'CodecPackPackaging.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
Assert-SemVer -Value $PackVersion -Name 'PackVersion'

$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
if (Test-Path -LiteralPath $outputFullPath) {
    throw "Refusing to overwrite an existing installer Rust license bundle: $outputFullPath"
}
$outputDirectory = [System.IO.Path]::GetDirectoryName($outputFullPath)
if ([string]::IsNullOrWhiteSpace($outputDirectory)) {
    throw 'Installer Rust license bundle output must have a parent directory.'
}
[System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null

$cargoLockPath = Join-Path $repositoryRoot 'Cargo.lock'
$cargoLockSha256 = (
    Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256
).Hash.ToLowerInvariant()

Push-Location $repositoryRoot
try {
    $metadataArguments = @(
        'metadata', '--locked', '--format-version', '1',
        '--filter-platform', 'x86_64-pc-windows-msvc'
    )
    if (-not $AllowNetwork) {
        $metadataArguments += '--offline'
    }
    $metadataText = & cargo @metadataArguments | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$metadata = $metadataText | ConvertFrom-Json -Depth 100
$rootPackages = @(
    $metadata.packages |
        Where-Object { [string]$_.name -ceq 'latentdeck-codec-pack-installer' }
)
if ($rootPackages.Count -ne 1) {
    throw 'Cargo metadata did not contain exactly one Codec Pack installer package.'
}
$dependencyIds = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::Ordinal
)
foreach ($packageId in @(
    Get-CargoNormalBuildDependencyIds `
        -Metadata $metadata `
        -RootPackageId ([string]$rootPackages[0].id)
)) {
    [void]$dependencyIds.Add([string]$packageId)
}

$packages = @(
    $metadata.packages |
        Where-Object { $dependencyIds.Contains([string]$_.id) } |
        Sort-Object `
            @{ Expression = { [string]$_.name } }, `
            @{ Expression = { [string]$_.version } }, `
            @{ Expression = { [string]$_.id } }
)
if ($packages.Count -eq 0 -or
    @($packages | Where-Object { [string]$_.name -ceq 'latentdeck-codec-pack-installer' }).Count -ne 1) {
    throw 'Installer Rust dependency closure is empty or missing its root package.'
}

$strictUtf8 = [System.Text.UTF8Encoding]::new($false, $true)
$builder = [System.Text.StringBuilder]::new()
[void]$builder.AppendLine('LatentDeck H3 Codec Pack installer Rust license bundle')
[void]$builder.AppendLine('Schema-Version: 1')
[void]$builder.AppendLine("Pack-Version: $PackVersion")
[void]$builder.AppendLine('Target: x86_64-pc-windows-msvc')
[void]$builder.AppendLine("Cargo.lock-SHA256: $cargoLockSha256")
[void]$builder.AppendLine("Component-Count: $($packages.Count)")
[void]$builder.AppendLine()
[void]$builder.AppendLine(
    'This bundle contains the license and notice files shipped with every Rust ' +
    'package in the locked Windows normal/build dependency closure.'
)

foreach ($package in $packages) {
    $name = [string]$package.name
    $version = [string]$package.version
    $licenseExpression = [string]$package.license
    if ([string]::IsNullOrWhiteSpace($name) -or
        [string]::IsNullOrWhiteSpace($version) -or
        [string]::IsNullOrWhiteSpace($licenseExpression)) {
        throw 'Installer Rust dependency metadata is missing name, version, or license.'
    }

    $manifestPath = [System.IO.Path]::GetFullPath([string]$package.manifest_path)
    $packageRoot = [System.IO.Path]::GetDirectoryName($manifestPath)
    Assert-PathComponentsNotReparsePoints -Path $packageRoot
    $sourceKind = if ($null -eq $package.source) { 'workspace' } else { 'registry' }

    $licenseFilesByPath = @{}
    foreach ($candidate in @(
        Get-ChildItem -LiteralPath $packageRoot -File -Force |
            Where-Object {
                $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE|COPYRIGHT)(?:$|[._-])'
            }
    )) {
        if (($candidate.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Rust dependency license file cannot be a reparse point: $name $version"
        }
        $candidatePath = Assert-ChildPath `
            -ParentPath $packageRoot `
            -CandidatePath $candidate.FullName
        $licenseFilesByPath[$candidatePath.ToLowerInvariant()] = $candidatePath
    }

    if (-not [string]::IsNullOrWhiteSpace([string]$package.license_file)) {
        $declaredLicensePath = [System.IO.Path]::GetFullPath([string]$package.license_file)
        if ($sourceKind -ceq 'workspace') {
            Assert-ChildPath `
                -ParentPath $repositoryRoot `
                -CandidatePath $declaredLicensePath | Out-Null
        } else {
            Assert-ChildPath `
                -ParentPath $packageRoot `
                -CandidatePath $declaredLicensePath | Out-Null
        }
        if (-not (Test-Path -LiteralPath $declaredLicensePath -PathType Leaf)) {
            throw "Declared Rust dependency license file is missing: $name $version"
        }
        $declaredItem = Get-Item -LiteralPath $declaredLicensePath -Force
        if (($declaredItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Declared Rust dependency license file cannot be a reparse point: $name $version"
        }
        $licenseFilesByPath[$declaredLicensePath.ToLowerInvariant()] = $declaredLicensePath
    }

    if ($licenseFilesByPath.Count -eq 0 -and $sourceKind -ceq 'workspace') {
        $workspaceLicensePath = Join-Path $repositoryRoot 'LICENSE'
        if (-not (Test-Path -LiteralPath $workspaceLicensePath -PathType Leaf)) {
            throw "Workspace Rust dependency has no license file: $name $version"
        }
        $workspaceLicenseItem = Get-Item -LiteralPath $workspaceLicensePath -Force
        if (($workspaceLicenseItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'Repository LICENSE cannot be a reparse point.'
        }
        $licenseFilesByPath[$workspaceLicensePath.ToLowerInvariant()] = $workspaceLicensePath
    }
    if ($licenseFilesByPath.Count -eq 0) {
        throw "Rust dependency has no distributable license or notice file: $name $version"
    }

    [void]$builder.AppendLine()
    [void]$builder.AppendLine(('=' * 79))
    [void]$builder.AppendLine("Package: $name $version")
    [void]$builder.AppendLine("Source kind: $sourceKind")
    [void]$builder.AppendLine("License expression: $licenseExpression")

    $licenseFiles = @(
        $licenseFilesByPath.Values |
            Sort-Object `
                @{ Expression = { [System.IO.Path]::GetFileName([string]$_) } }, `
                @{ Expression = { [string]$_ } }
    )
    [void]$builder.AppendLine("License/notice files: $($licenseFiles.Count)")
    foreach ($licenseFilePath in $licenseFiles) {
        $licenseFileName = [System.IO.Path]::GetFileName([string]$licenseFilePath)
        $bytes = [System.IO.File]::ReadAllBytes([string]$licenseFilePath)
        if ($bytes.Length -eq 0 -or $bytes.Length -gt 4MB) {
            throw "Rust dependency license file is empty or unexpectedly large: $name $version $licenseFileName"
        }
        try {
            $licenseText = $strictUtf8.GetString($bytes)
        } catch {
            throw "Rust dependency license file is not strict UTF-8: $name $version $licenseFileName"
        }
        $licenseText = $licenseText.TrimStart([char]0xFEFF)
        $licenseText = $licenseText.Replace("`r`n", "`n").Replace("`r", "`n").TrimEnd("`n")
        if ([string]::IsNullOrWhiteSpace($licenseText)) {
            throw "Rust dependency license file has no text: $name $version $licenseFileName"
        }
        [void]$builder.AppendLine(('-' * 79))
        [void]$builder.AppendLine("File: $licenseFileName")
        [void]$builder.AppendLine(('-' * 79))
        [void]$builder.AppendLine($licenseText)
    }
}

$bundleText = $builder.ToString().Replace("`r`n", "`n").Replace("`r", "`n")
if (-not $bundleText.EndsWith("`n", [System.StringComparison]::Ordinal)) {
    $bundleText += "`n"
}
foreach ($forbidden in @($repositoryRoot, $env:USERPROFILE)) {
    if (-not [string]::IsNullOrWhiteSpace($forbidden) -and
        $bundleText.IndexOf($forbidden, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        throw 'Generated installer Rust license bundle contains a machine-local absolute path.'
    }
}
if ($bundleText -match '(?m)(?<![A-Za-z])[A-Z]:[\\/]' -or
    $bundleText -match '(?m)(?<![\\])\\\\[^\\\s]+\\' -or
    $bundleText -match '(?i)file:/{2,3}' -or
    $bundleText -match '(?i)/(?:Users|home)/[^/\s]+/') {
    throw 'Generated installer Rust license bundle contains a machine-local filesystem reference.'
}

$partialPath = "$outputFullPath.partial-$([guid]::NewGuid().ToString('N'))"
try {
    [System.IO.File]::WriteAllText(
        $partialPath,
        $bundleText,
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::Move($partialPath, $outputFullPath, $false)
} finally {
    if (Test-Path -LiteralPath $partialPath -PathType Leaf) {
        [System.IO.File]::Delete($partialPath)
    }
}

[pscustomobject]@{
    Path = $outputFullPath
    Components = $packages.Count
    Sha256 = (
        Get-FileHash -LiteralPath $outputFullPath -Algorithm SHA256
    ).Hash.ToLowerInvariant()
}
