Set-StrictMode -Version Latest

function Get-TomlStringArrayAssignment {
    param(
        [Parameter(Mandatory)]
        [string]$SectionBody,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $escapedName = [regex]::Escape($Name)
    $matches = [regex]::Matches(
        $SectionBody,
        "(?m)^[ \t]*$escapedName[ \t]*=[ \t]*(?<value>\[[^\r\n]*\])[ \t]*(?:#.*)?$"
    )
    if ($matches.Count -ne 1) {
        throw "$Context must declare '$Name' exactly once as a string array."
    }

    try {
        $values = @($matches[0].Groups['value'].Value | ConvertFrom-Json)
    } catch {
        throw "$Context has an invalid '$Name' string array."
    }
    if ($values.Count -eq 0 -or @($values | Where-Object { $_ -isnot [string] }).Count -gt 0) {
        throw "$Context must declare '$Name' as a non-empty string array."
    }
    return $values
}

function Assert-TauriOfflineFrontendContract {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$ConfigPath,

        [Parameter(Mandatory)]
        [string]$CargoManifestPath,

        [Parameter(Mandatory)]
        [string]$PackageJsonPath
    )

    $config = Get-Content -LiteralPath $ConfigPath -Raw | ConvertFrom-Json
    if ($config.build.beforeBuildCommand -cne 'pnpm build') {
        throw "Tauri release frontend must be rebuilt with 'pnpm build': $ConfigPath"
    }
    if ($config.build.frontendDist -cne '../dist') {
        throw "Tauri release frontendDist must be the local ../dist directory: $ConfigPath"
    }

    $tauriDirectory = [System.IO.Path]::GetFullPath((Split-Path -Parent $ConfigPath))
    $appDirectory = [System.IO.Path]::GetFullPath((Join-Path $tauriDirectory '..'))
    $resolvedFrontendDist = [System.IO.Path]::GetFullPath(
        (Join-Path $tauriDirectory $config.build.frontendDist)
    )
    $expectedFrontendDist = [System.IO.Path]::GetFullPath((Join-Path $appDirectory 'dist'))
    if (-not $resolvedFrontendDist.Equals(
        $expectedFrontendDist,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Tauri release frontendDist resolves outside the application dist directory: $ConfigPath"
    }

    $package = Get-Content -LiteralPath $PackageJsonPath -Raw | ConvertFrom-Json
    if ($package.scripts.build -cne 'vite build') {
        throw "The Tauri beforeBuildCommand must resolve to the local Vite production build: $PackageJsonPath"
    }

    $mainWindows = @($config.app.windows | Where-Object { $_.label -ceq 'main' })
    if ($mainWindows.Count -ne 1) {
        throw "Tauri release configuration must declare exactly one 'main' window: $ConfigPath"
    }
    if ($null -ne $mainWindows[0].PSObject.Properties['url']) {
        throw "The Tauri main window must load the embedded frontend instead of an explicit URL: $ConfigPath"
    }

    $cargoText = Get-Content -LiteralPath $CargoManifestPath -Raw
    $featureSections = [regex]::Matches(
        $cargoText,
        '(?ms)^[ \t]*\[features\][ \t]*(?:#.*)?\r?\n(?<body>.*?)(?=^[ \t]*\[|\z)'
    )
    if ($featureSections.Count -ne 1) {
        throw "Cargo manifest must declare exactly one [features] section: $CargoManifestPath"
    }
    $featureBody = $featureSections[0].Groups['body'].Value
    $defaultFeatures = @(Get-TomlStringArrayAssignment `
        -SectionBody $featureBody `
        -Name 'default' `
        -Context $CargoManifestPath)
    if (-not ($defaultFeatures -ccontains 'custom-protocol')) {
        throw "Cargo default features must enable custom-protocol for offline release startup: $CargoManifestPath"
    }
    $customProtocolMapping = @(Get-TomlStringArrayAssignment `
        -SectionBody $featureBody `
        -Name 'custom-protocol' `
        -Context $CargoManifestPath)
    if (-not ($customProtocolMapping -ccontains 'tauri/custom-protocol')) {
        throw "Cargo custom-protocol must map to tauri/custom-protocol: $CargoManifestPath"
    }
}

function Assert-TauriEmbeddedFrontendBinary {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$BinaryPath,

        [Parameter(Mandatory)]
        [string]$FrontendDistPath
    )

    $resolvedBinary = (Resolve-Path -LiteralPath $BinaryPath).Path
    $binaryItem = Get-Item -LiteralPath $resolvedBinary -Force
    if ($binaryItem.PSIsContainer -or
        ($binaryItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $binaryItem.Length -eq 0 -or
        $binaryItem.Length -gt 256MB) {
        throw "Tauri release binary must be a bounded regular non-reparse file: $BinaryPath"
    }

    $resolvedDist = (Resolve-Path -LiteralPath $FrontendDistPath).Path.TrimEnd('\', '/')
    $indexPath = Join-Path $resolvedDist 'index.html'
    $indexItem = Get-Item -LiteralPath $indexPath -Force
    if ($indexItem.PSIsContainer -or
        ($indexItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $indexItem.Length -eq 0 -or
        $indexItem.Length -gt 1MB) {
        throw "Tauri frontend index must be a bounded regular non-reparse file: $indexPath"
    }

    $indexBytes = [System.IO.File]::ReadAllBytes($indexPath)
    try {
        $indexText = [System.Text.UTF8Encoding]::new($false, $true).GetString($indexBytes)
    } catch [System.Text.DecoderFallbackException] {
        throw "Tauri frontend index is not strict UTF-8: $indexPath"
    }
    $assetMatches = [regex]::Matches(
        $indexText,
        '(?i)(?:src|href)=["''](?<path>/assets/[A-Za-z0-9._/-]+)["'']'
    )
    $assetPaths = @($assetMatches | ForEach-Object { $_.Groups['path'].Value } | Sort-Object -Unique)
    if ($assetPaths.Count -eq 0) {
        throw "Tauri frontend index declares no production assets: $indexPath"
    }

    foreach ($assetPath in $assetPaths) {
        $segments = @($assetPath.TrimStart('/').Split('/'))
        if (@($segments | Where-Object {
            [string]::IsNullOrWhiteSpace($_) -or $_ -ceq '.' -or $_ -ceq '..'
        }).Count -gt 0) {
            throw "Tauri frontend index contains an unsafe asset path: $assetPath"
        }
        $assetFile = [System.IO.Path]::GetFullPath(
            (Join-Path $resolvedDist ($assetPath.TrimStart('/').Replace('/', '\')))
        )
        if (-not $assetFile.StartsWith(
            $resolvedDist + [System.IO.Path]::DirectorySeparatorChar,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Tauri frontend asset resolves outside frontendDist: $assetPath"
        }
        $assetItem = Get-Item -LiteralPath $assetFile -Force
        if ($assetItem.PSIsContainer -or
            ($assetItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
            $assetItem.Length -eq 0 -or
            $assetItem.Length -gt 256MB) {
            throw "Tauri frontend asset must be a bounded non-empty regular file: $assetPath"
        }
    }

    $binaryText = [System.Text.Encoding]::ASCII.GetString(
        [System.IO.File]::ReadAllBytes($resolvedBinary)
    )
    foreach ($marker in @('/index.html') + $assetPaths) {
        if ($binaryText.IndexOf($marker, [System.StringComparison]::Ordinal) -lt 0) {
            throw "Tauri release binary does not embed the current frontend marker '$marker': $BinaryPath"
        }
    }
}

Export-ModuleMember -Function `
    Assert-TauriEmbeddedFrontendBinary, `
    Assert-TauriOfflineFrontendContract
