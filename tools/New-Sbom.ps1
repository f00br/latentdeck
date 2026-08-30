[CmdletBinding()]
param(
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'ReleaseSpoutMetadata.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$productVersion = '0.1.0'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $repositoryRoot "artifacts/release/latentdeck-$productVersion-sbom.cdx.json"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputPath)) {
    $OutputPath = Join-Path $repositoryRoot $OutputPath
}
$outputFullPath = [System.IO.Path]::GetFullPath($OutputPath)
$outputDirectory = [System.IO.Path]::GetDirectoryName($outputFullPath)
if ([string]::IsNullOrWhiteSpace($outputDirectory)) {
    throw 'SBOM output must have a parent directory.'
}
if (Test-Path -LiteralPath $outputFullPath) {
    throw "Refusing to overwrite an existing SBOM: $outputFullPath"
}
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
$partialPath = "$outputFullPath.partial-$([guid]::NewGuid().ToString('N'))"
$pythonPath = Join-Path $outputDirectory ".python-sbom-$([guid]::NewGuid().ToString('N')).json"

function Invoke-JsonCommand {
    param(
        [Parameter(Mandatory)]
        [scriptblock]$Command,

        [Parameter(Mandatory)]
        [string]$Label
    )

    $text = & $Command | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
    try {
        return $text | ConvertFrom-Json -Depth 100
    } catch {
        throw "$Label did not return valid JSON: $($_.Exception.Message)"
    }
}

function New-LicenseList {
    param([object]$License)

    if ($null -eq $License -or [string]::IsNullOrWhiteSpace([string]$License)) {
        return @()
    }
    return @([ordered]@{ license = [ordered]@{ name = [string]$License } })
}

function ConvertTo-SafePurlName {
    param([Parameter(Mandatory)][string]$Name)

    return [System.Uri]::EscapeDataString($Name).Replace('%2F', '/').Replace('%2f', '/')
}

$components = [System.Collections.Generic.List[object]]::new()

try {
    Push-Location $repositoryRoot
    try {
        $cargo = Invoke-JsonCommand -Label 'cargo metadata' -Command {
            cargo metadata --locked --format-version 1
        }
        foreach ($package in $cargo.packages) {
            $name = [string]$package.name
            $version = [string]$package.version
            $component = [ordered]@{
                type = 'library'
                'bom-ref' = "rust:$name@$version"
                name = $name
                version = $version
                purl = "pkg:cargo/$(ConvertTo-SafePurlName $name)@$version"
                properties = @(
                    [ordered]@{ name = 'latentdeck:ecosystem'; value = 'rust' }
                    [ordered]@{
                        name = 'latentdeck:source-kind'
                        value = if ($null -eq $package.source) { 'workspace' } else { 'registry' }
                    }
                )
            }
            $licenses = @(New-LicenseList $package.license)
            if ($licenses.Count -gt 0) {
                $component.licenses = $licenses
            }
            $components.Add([pscustomobject]$component)
        }

        $nodeRoot = & (Join-Path $PSScriptRoot 'Get-PinnedNode.ps1')
        $pnpm = Join-Path $nodeRoot 'pnpm.cmd'
        $pnpmLicenses = Invoke-JsonCommand -Label 'pnpm license inventory' -Command {
            & $pnpm licenses list --json --long
        }
        foreach ($licenseGroup in $pnpmLicenses.PSObject.Properties) {
            foreach ($package in @($licenseGroup.Value)) {
                foreach ($versionValue in @($package.versions)) {
                    $name = [string]$package.name
                    $version = [string]$versionValue
                    $component = [ordered]@{
                        type = 'library'
                        'bom-ref' = "node:$name@$version"
                        name = $name
                        version = $version
                        purl = "pkg:npm/$(ConvertTo-SafePurlName $name)@$version"
                        licenses = @(New-LicenseList $package.license)
                        properties = @(
                            [ordered]@{ name = 'latentdeck:ecosystem'; value = 'node' }
                        )
                    }
                    $components.Add([pscustomobject]$component)
                }
            }
        }

        uv export --format cyclonedx1.5 --all-packages --all-extras --locked `
            --preview-features sbom-export --output-file $pythonPath | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw "uv CycloneDX export failed with exit code $LASTEXITCODE"
        }
        $pythonBom = Get-Content -Raw -LiteralPath $pythonPath | ConvertFrom-Json -Depth 100
        foreach ($package in @($pythonBom.components)) {
            $originalReference = [string]$package.'bom-ref'
            $package.'bom-ref' = "python:$originalReference"
            $propertyField = $package.PSObject.Properties['properties']
            $properties = [System.Collections.Generic.List[object]]::new()
            if ($null -ne $propertyField) {
                foreach ($property in @($propertyField.Value)) {
                    $properties.Add($property)
                }
            }
            $properties.Add([pscustomobject]@{
                name = 'latentdeck:ecosystem'
                value = 'python'
            })
            $propertyArray = $properties.ToArray()
            if ($null -eq $propertyField) {
                $package | Add-Member -NotePropertyName properties -NotePropertyValue $propertyArray
            } else {
                $propertyField.Value = $propertyArray
            }
            $components.Add($package)
        }
    }
    finally {
        Pop-Location
    }

    $components.Add((New-Spout2CycloneDxComponent))
    $sortedComponents = @($components | Sort-Object -Property @{ Expression = { $_.'bom-ref' } })
    $duplicateReferences = @(
        $sortedComponents |
            Group-Object -Property 'bom-ref' |
            Where-Object Count -gt 1
    )
    if ($duplicateReferences.Count -gt 0) {
        throw 'Generated SBOM contains duplicate component references.'
    }
    $bom = [ordered]@{
        bomFormat = 'CycloneDX'
        specVersion = '1.5'
        serialNumber = "urn:uuid:$([guid]::NewGuid())"
        version = 1
        metadata = [ordered]@{
            timestamp = [DateTimeOffset]::UtcNow.ToString('o')
            tools = @(
                [ordered]@{
                    vendor = 'LatentDeck'
                    name = 'tools/New-Sbom.ps1'
                    version = $productVersion
                }
            )
            component = [ordered]@{
                type = 'application'
                'bom-ref' = "pkg:generic/latentdeck@$productVersion"
                name = 'LatentDeck'
                version = $productVersion
            }
        }
        components = $sortedComponents
    }
    $json = $bom | ConvertTo-Json -Depth 100
    foreach ($forbidden in @(
        $repositoryRoot,
        $repositoryRoot.Replace('\', '\\'),
        $env:USERPROFILE,
        $env:USERPROFILE.Replace('\', '\\')
    )) {
        if (-not [string]::IsNullOrWhiteSpace($forbidden) -and
            $json.IndexOf($forbidden, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw 'Generated SBOM contains a machine-local absolute path.'
        }
    }
    if ($json -match '(?i)(?<![A-Za-z])[A-Z]:[\\/]' -or
        $json -match '(?m)(?<![\\])\\\\[^\\\s]+\\' -or
        $json -match '(?i)file:/{2,3}' -or
        $json -match '(?i)/(?:Users|home)/[^/\s]+/' -or
        $json -match '(?i)node_modules') {
        throw 'Generated SBOM contains a machine-local package or filesystem reference.'
    }
    [System.IO.File]::WriteAllText(
        $partialPath,
        $json + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $roundTrip = Get-Content -Raw -LiteralPath $partialPath | ConvertFrom-Json -Depth 100
    if ($roundTrip.bomFormat -cne 'CycloneDX' -or
        $roundTrip.specVersion -cne '1.5' -or
        @($roundTrip.components).Count -lt 1) {
        throw 'Generated SBOM failed its structural self-check.'
    }
    Assert-Spout2CycloneDxComponent -Components @($roundTrip.components) | Out-Null
    [System.IO.File]::Move($partialPath, $outputFullPath, $false)
    $hash = (Get-FileHash -LiteralPath $outputFullPath -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "SBOM: $outputFullPath"
    Write-Host "Components: $(@($roundTrip.components).Count)"
    Write-Host "SHA-256: $hash"
    [pscustomobject]@{
        Path = $outputFullPath
        Components = @($roundTrip.components).Count
        Sha256 = $hash
    }
}
finally {
    foreach ($temporaryPath in @($partialPath, $pythonPath)) {
        if (Test-Path -LiteralPath $temporaryPath) {
            Remove-Item -LiteralPath $temporaryPath -Force
        }
    }
}
