Set-StrictMode -Version Latest

$script:Spout2 = [ordered]@{
    Tag = '2.007.017'
    Commit = 'f49e2f469f8cb25f559a6eaa61a3f5b8173fc100'
    ArchiveSha256 = 'cb60c83d4df3c2927cd3c5a505910bb720a8011d505217a71d293968405e4bf4'
    ArchiveBytes = [int64]5099633
    ArchiveUrl = 'https://github.com/leadedge/Spout2/archive/f49e2f469f8cb25f559a6eaa61a3f5b8173fc100.zip'
    VcsUrl = 'https://github.com/leadedge/Spout2/tree/f49e2f469f8cb25f559a6eaa61a3f5b8173fc100'
    WebsiteUrl = 'https://github.com/leadedge/Spout2'
    LicenseId = 'BSD-2-Clause'
    Copyright = 'Copyright (c) 2020-2024, Lynn Jarvis'
    BomRef = 'native:spout2@2.007.017+f49e2f469f8cb25f559a6eaa61a3f5b8173fc100'
}

function Get-RequiredObjectProperty {
    param(
        [Parameter(Mandatory)]
        [object]$Object,

        [Parameter(Mandatory)]
        [string]$Name,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ($Object -is [System.Collections.IDictionary] -and $Object.Contains($Name)) {
        return $Object[$Name]
    }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        throw "$Context is missing required property '$Name'."
    }
    return $property.Value
}

function Get-Spout2ReleaseMetadata {
    [CmdletBinding()]
    param()

    return [pscustomobject]$script:Spout2
}

function New-Spout2CycloneDxComponent {
    [CmdletBinding()]
    param()

    $metadata = Get-Spout2ReleaseMetadata
    return [pscustomobject][ordered]@{
        type = 'library'
        'bom-ref' = $metadata.BomRef
        name = 'Spout2'
        version = $metadata.Tag
        copyright = $metadata.Copyright
        hashes = @(
            [ordered]@{
                alg = 'SHA-256'
                content = $metadata.ArchiveSha256
            }
        )
        licenses = @(
            [ordered]@{
                license = [ordered]@{ id = $metadata.LicenseId }
            }
        )
        externalReferences = @(
            [ordered]@{ type = 'website'; url = $metadata.WebsiteUrl }
            [ordered]@{ type = 'vcs'; url = $metadata.VcsUrl }
            [ordered]@{ type = 'distribution'; url = $metadata.ArchiveUrl }
        )
        properties = @(
            [ordered]@{ name = 'latentdeck:ecosystem'; value = 'native-cpp' }
            [ordered]@{ name = 'latentdeck:source-kind'; value = 'pinned-upstream' }
            [ordered]@{ name = 'latentdeck:upstream-commit'; value = $metadata.Commit }
            [ordered]@{ name = 'latentdeck:archive-bytes'; value = [string]$metadata.ArchiveBytes }
            [ordered]@{ name = 'latentdeck:integration'; value = 'statically-linked-spoutdx12' }
        )
    }
}

function Assert-Spout2CycloneDxComponent {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [object[]]$Components
    )

    $metadata = Get-Spout2ReleaseMetadata
    $matches = @(
        $Components |
            Where-Object {
                $name = $_.PSObject.Properties['name']
                $null -ne $name -and [string]$name.Value -ceq 'Spout2'
            }
    )
    if ($matches.Count -ne 1) {
        throw 'Release SBOM must contain exactly one upstream Spout2 component.'
    }
    $component = $matches[0]
    $expectedScalars = [ordered]@{
        type = 'library'
        'bom-ref' = $metadata.BomRef
        name = 'Spout2'
        version = $metadata.Tag
        copyright = $metadata.Copyright
    }
    foreach ($entry in $expectedScalars.GetEnumerator()) {
        $actual = Get-RequiredObjectProperty `
            -Object $component `
            -Name $entry.Key `
            -Context 'Spout2 SBOM component'
        if ([string]$actual -cne [string]$entry.Value) {
            throw "Spout2 SBOM component has an invalid '$($entry.Key)' value."
        }
    }

    $hashes = @(Get-RequiredObjectProperty -Object $component -Name 'hashes' -Context 'Spout2 SBOM component')
    if ($hashes.Count -ne 1) {
        throw 'Spout2 SBOM component must declare exactly one pinned archive hash.'
    }
    $archiveHashes = @(
        $hashes |
            Where-Object {
                [string](Get-RequiredObjectProperty -Object $_ -Name 'alg' -Context 'Spout2 hash') -ceq 'SHA-256' -and
                [string](Get-RequiredObjectProperty -Object $_ -Name 'content' -Context 'Spout2 hash') -ceq $metadata.ArchiveSha256
            }
    )
    if ($archiveHashes.Count -ne 1) {
        throw 'Spout2 SBOM component is missing the exact pinned archive SHA-256.'
    }

    $licenses = @(Get-RequiredObjectProperty -Object $component -Name 'licenses' -Context 'Spout2 SBOM component')
    if ($licenses.Count -ne 1) {
        throw 'Spout2 SBOM component must declare exactly one license entry.'
    }
    $licenseMatches = @(
        foreach ($entry in $licenses) {
            $license = Get-RequiredObjectProperty -Object $entry -Name 'license' -Context 'Spout2 license entry'
            $id = Get-RequiredObjectProperty -Object $license -Name 'id' -Context 'Spout2 license'
            if ([string]$id -ceq $metadata.LicenseId) {
                $entry
            }
        }
    )
    if ($licenseMatches.Count -ne 1) {
        throw 'Spout2 SBOM component must declare BSD-2-Clause exactly once.'
    }

    $references = @(Get-RequiredObjectProperty -Object $component -Name 'externalReferences' -Context 'Spout2 SBOM component')
    if ($references.Count -ne 3) {
        throw 'Spout2 SBOM component must declare exactly three provenance references.'
    }
    foreach ($expected in @(
        @{ Type = 'website'; Url = $metadata.WebsiteUrl },
        @{ Type = 'vcs'; Url = $metadata.VcsUrl },
        @{ Type = 'distribution'; Url = $metadata.ArchiveUrl }
    )) {
        $referenceMatches = @(
            $references |
                Where-Object {
                    [string](Get-RequiredObjectProperty -Object $_ -Name 'type' -Context 'Spout2 external reference') -ceq $expected.Type -and
                    [string](Get-RequiredObjectProperty -Object $_ -Name 'url' -Context 'Spout2 external reference') -ceq $expected.Url
                }
        )
        if ($referenceMatches.Count -ne 1) {
            throw "Spout2 SBOM component is missing its exact $($expected.Type) provenance."
        }
    }

    $properties = @(Get-RequiredObjectProperty -Object $component -Name 'properties' -Context 'Spout2 SBOM component')
    if ($properties.Count -ne 5) {
        throw 'Spout2 SBOM component must declare exactly five integration properties.'
    }
    foreach ($expected in @(
        @{ Name = 'latentdeck:ecosystem'; Value = 'native-cpp' },
        @{ Name = 'latentdeck:source-kind'; Value = 'pinned-upstream' },
        @{ Name = 'latentdeck:upstream-commit'; Value = $metadata.Commit },
        @{ Name = 'latentdeck:archive-bytes'; Value = [string]$metadata.ArchiveBytes },
        @{ Name = 'latentdeck:integration'; Value = 'statically-linked-spoutdx12' }
    )) {
        $propertyMatches = @(
            $properties |
                Where-Object {
                    [string](Get-RequiredObjectProperty -Object $_ -Name 'name' -Context 'Spout2 property') -ceq $expected.Name -and
                    [string](Get-RequiredObjectProperty -Object $_ -Name 'value' -Context 'Spout2 property') -ceq $expected.Value
                }
        )
        if ($propertyMatches.Count -ne 1) {
            throw "Spout2 SBOM component is missing exact property '$($expected.Name)'."
        }
    }
    return $component
}

function Test-Spout2ThirdPartyNotice {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -eq 0 -or
        $item.Length -gt 1MB) {
        throw 'Spout2 third-party notice must be a bounded regular non-reparse file.'
    }
    $bytes = [System.IO.File]::ReadAllBytes($resolved)
    try {
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
    } catch [System.Text.DecoderFallbackException] {
        throw 'Spout2 third-party notice is not strict UTF-8.'
    }
    $metadata = Get-Spout2ReleaseMetadata
    $spoutHeadings = [regex]::Matches($text, '(?m)^## Spout2\r?$')
    if ($spoutHeadings.Count -ne 1) {
        throw 'Spout2 third-party notice must contain exactly one Spout2 section.'
    }
    $sectionStart = $spoutHeadings[0].Index
    $nextSectionPattern = [regex]::new('(?m)^## (?!Spout2\r?$).+$')
    $nextSection = $nextSectionPattern.Match(
        $text,
        $sectionStart + $spoutHeadings[0].Length
    )
    $sectionEnd = if ($nextSection.Success) { $nextSection.Index } else { $text.Length }
    $spoutSection = $text.Substring($sectionStart, $sectionEnd - $sectionStart)
    if (-not $text.Contains(
            '# LatentDeck third-party notices',
            [System.StringComparison]::Ordinal
        )) {
        throw 'Spout2 third-party notice is missing the document title.'
    }
    foreach ($required in @(
        '## Spout2',
        "- Source: <$($metadata.WebsiteUrl)>",
        "- Tag: ``$($metadata.Tag)``",
        "- Pinned commit: ``$($metadata.Commit)``",
        "- License: $($metadata.LicenseId)",
        $metadata.Copyright,
        '2. Redistributions in binary form must reproduce the above copyright notice,',
        'THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"'
    )) {
        if (-not $spoutSection.Contains($required, [System.StringComparison]::Ordinal)) {
            throw "Spout2 third-party notice is missing required text: $required"
        }
    }
    foreach ($exactLine in @(
        "- Source: <$($metadata.WebsiteUrl)>",
        "- Tag: ``$($metadata.Tag)``",
        "- Pinned commit: ``$($metadata.Commit)``",
        "- Copyright: 2020-2024 Lynn Jarvis",
        "- License: $($metadata.LicenseId)"
    )) {
        $lineMatches = [regex]::Matches(
            $spoutSection,
            '(?m)^' + [regex]::Escape($exactLine) + '\r?$'
        )
        if ($lineMatches.Count -ne 1) {
            throw "Spout2 third-party notice must contain exact metadata line once: $exactLine"
        }
    }
    foreach ($prefix in @('Source', 'Tag', 'Pinned commit', 'Copyright', 'License')) {
        $metadataLines = [regex]::Matches(
            $spoutSection,
            "(?m)^- $([regex]::Escape($prefix)):.+\r?$"
        )
        if ($metadataLines.Count -ne 1) {
            throw "Spout2 third-party notice contains conflicting '$prefix' metadata."
        }
    }
    if ($text -match '(?i)(?<![A-Za-z])[A-Z]:[\\/]' -or
        $text -match '(?i)file:/{2,3}' -or
        $text -match '(?i)/(?:Users|home)/[^/\s]+/') {
        throw 'Spout2 third-party notice contains a machine-local path.'
    }
    return [pscustomobject]@{
        Path = $resolved
        ByteLength = [int64]$item.Length
        Sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

function Copy-Spout2ThirdPartyNotice {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$SourcePath,

        [Parameter(Mandatory)]
        [string]$DestinationDirectory
    )

    $source = Test-Spout2ThirdPartyNotice -Path $SourcePath
    $directory = (Resolve-Path -LiteralPath $DestinationDirectory).Path
    $directoryItem = Get-Item -LiteralPath $directory -Force
    if (-not $directoryItem.PSIsContainer -or
        ($directoryItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Third-party notice destination must be an existing regular directory.'
    }
    $destination = Join-Path $directory 'THIRD_PARTY_NOTICES.md'
    if (Test-Path -LiteralPath $destination) {
        throw 'Refusing to overwrite staged third-party notices.'
    }
    [System.IO.File]::Copy($source.Path, $destination, $false)
    $staged = Test-Spout2ThirdPartyNotice -Path $destination
    if ($staged.Sha256 -cne $source.Sha256 -or
        $staged.ByteLength -ne $source.ByteLength) {
        throw 'Staged third-party notices do not match the reviewed source notice.'
    }
    return $staged
}

Export-ModuleMember -Function @(
    'Get-Spout2ReleaseMetadata',
    'New-Spout2CycloneDxComponent',
    'Assert-Spout2CycloneDxComponent',
    'Test-Spout2ThirdPartyNotice',
    'Copy-Spout2ThirdPartyNotice'
)
