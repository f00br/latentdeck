Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-SafetensorsLowerSha256Bytes {
    param([Parameter(Mandatory)][byte[]]$Bytes)

    return [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($Bytes)
    ).ToLowerInvariant()
}

function Get-SafetensorsLowerSha256File {
    param([Parameter(Mandatory)][string]$Path)

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Assert-SafetensorsRegularFile {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Context,
        [Parameter(Mandatory)][int64]$MaximumBytes
    )

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $cursor = Get-Item -LiteralPath $resolved -Force
    if ($cursor.PSIsContainer -or
        ($cursor.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $cursor.Length -le 0 -or $cursor.Length -gt $MaximumBytes) {
        throw "$Context must be a bounded regular non-reparse file."
    }
    while ($null -ne $cursor) {
        if (($cursor.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Context has a reparse-point ancestor."
        }
        $parent = [System.IO.Directory]::GetParent($cursor.FullName)
        $cursor = if ($null -eq $parent) { $null } else { Get-Item -LiteralPath $parent.FullName -Force }
    }
    return Get-Item -LiteralPath $resolved -Force
}

function Get-SafetensorsExactProperties {
    param(
        [Parameter(Mandatory)][object]$Object,
        [Parameter(Mandatory)][string[]]$Required,
        [string[]]$Optional = @(),
        [Parameter(Mandatory)][string]$Context
    )

    $actual = @($Object.PSObject.Properties.Name | Sort-Object -CaseSensitive)
    $expected = @($Required + $Optional | Sort-Object -CaseSensitive -Unique)
    foreach ($name in $Required) {
        if ($null -eq $Object.PSObject.Properties[$name]) {
            throw "$Context is missing required property '$name'."
        }
    }
    foreach ($name in $actual) {
        if ($name -cnotin $expected) {
            throw "$Context contains unexpected property '$name'."
        }
    }
}

function Read-SafetensorsZipEntry {
    param(
        [Parameter(Mandatory)][System.IO.Compression.ZipArchive]$Archive,
        [Parameter(Mandatory)][string]$EntryName,
        [Parameter(Mandatory)][int64]$ExpectedLength,
        [Parameter(Mandatory)][string]$ExpectedSha256,
        [Parameter(Mandatory)][string]$Context
    )

    $matches = @($Archive.Entries | Where-Object { $_.FullName -ceq $EntryName })
    if ($matches.Count -ne 1 -or $matches[0].Length -ne $ExpectedLength) {
        throw "$Context entry identity or length drifted."
    }
    $stream = $matches[0].Open()
    $memory = [System.IO.MemoryStream]::new()
    try {
        $buffer = [byte[]]::new(65536)
        $actualLength = 0L
        while (($read = $stream.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $actualLength += $read
            if ($actualLength -gt $ExpectedLength) {
                throw "$Context entry exceeds its locked length."
            }
            $memory.Write($buffer, 0, $read)
        }
        if ($actualLength -ne $ExpectedLength) {
            throw "$Context entry ended before its locked length."
        }
        $bytes = $memory.ToArray()
    } finally {
        $memory.Dispose()
        $stream.Dispose()
    }
    if ((Get-SafetensorsLowerSha256Bytes -Bytes $bytes) -cne $ExpectedSha256) {
        throw "$Context entry SHA-256 drifted."
    }
    return ,$bytes
}

function Read-SafetensorsTreeEntry {
    param(
        [Parameter(Mandatory)][string]$Root,
        [Parameter(Mandatory)][string]$EntryName,
        [Parameter(Mandatory)][int64]$ExpectedLength,
        [Parameter(Mandatory)][string]$ExpectedSha256,
        [Parameter(Mandatory)][string]$Context
    )

    if ($EntryName -match '(^|/)(?:\.|\.\.)(?:/|$)' -or
        [System.IO.Path]::IsPathRooted($EntryName)) {
        throw "$Context entry path is not portable."
    }
    $rootPath = (Resolve-Path -LiteralPath $Root).Path
    $relative = $EntryName.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
    $candidate = [System.IO.Path]::GetFullPath((Join-Path $rootPath $relative))
    $prefix = $rootPath.TrimEnd([char[]]@('/', '\')) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $candidate.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Context entry escapes its distribution root."
    }
    $item = Assert-SafetensorsRegularFile -Path $candidate -Context $Context -MaximumBytes 4MB
    if ($item.Length -ne $ExpectedLength -or
        (Get-SafetensorsLowerSha256File -Path $item.FullName) -cne $ExpectedSha256) {
        throw "$Context entry length or SHA-256 drifted."
    }
    return ,[System.IO.File]::ReadAllBytes($item.FullName)
}

function Get-SafetensorsLicenseExpression {
    param([Parameter(Mandatory)][object]$Component)

    $licenses = @($Component.licenses)
    if ($licenses.Count -ne 1 -or
        $null -eq $licenses[0].PSObject.Properties['expression'] -or
        [string]::IsNullOrWhiteSpace([string]$licenses[0].expression)) {
        throw "Safetensors embedded SBOM component has no exact license expression: $($Component.name)"
    }
    return [string]$licenses[0].expression
}

function Read-SafetensorsNativeClosure {
    [CmdletBinding(DefaultParameterSetName = 'Wheel')]
    param(
        [Parameter(Mandatory, ParameterSetName = 'Wheel')][string]$WheelPath,
        [Parameter(Mandatory, ParameterSetName = 'Tree')][string]$DistributionRoot
    )

    $repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
    $lockPath = Join-Path `
        $repositoryRoot `
        'comfy/latent-cartridge/packaging/safetensors-native-0.8.0.lock.json'
    $lockItem = Assert-SafetensorsRegularFile `
        -Path $lockPath `
        -Context 'Safetensors native closure lock' `
        -MaximumBytes 1MB
    $lock = Get-Content -LiteralPath $lockItem.FullName -Raw | ConvertFrom-Json -Depth 100
    Get-SafetensorsExactProperties `
        -Object $lock `
        -Required @(
            'schema_version', 'closure_id', 'wheel', 'license', 'embedded_sbom',
            'native_binary', 'components'
        ) `
        -Context 'Safetensors native closure lock'
    if ([int]$lock.schema_version -ne 1 -or
        [string]$lock.closure_id -cne 'safetensors-python@0.8.0/cp310-abi3-win_amd64' -or
        @($lock.components).Count -ne 32) {
        throw 'Safetensors native closure lock identity is invalid.'
    }

    if ($PSCmdlet.ParameterSetName -ceq 'Wheel') {
        $wheel = Assert-SafetensorsRegularFile `
            -Path $WheelPath `
            -Context 'Safetensors wheel' `
            -MaximumBytes 16MB
        if ($wheel.Name -cne [string]$lock.wheel.file_name -or
            $wheel.Length -ne [int64]$lock.wheel.byte_length -or
            (Get-SafetensorsLowerSha256File -Path $wheel.FullName) -cne [string]$lock.wheel.sha256) {
            throw 'Safetensors wheel does not match its native closure lock.'
        }
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        $archive = [System.IO.Compression.ZipFile]::OpenRead($wheel.FullName)
        try {
            $licenseBytes = Read-SafetensorsZipEntry `
                -Archive $archive `
                -EntryName ([string]$lock.license.entry) `
                -ExpectedLength ([int64]$lock.license.byte_length) `
                -ExpectedSha256 ([string]$lock.license.sha256) `
                -Context 'Safetensors license'
            $sbomBytes = Read-SafetensorsZipEntry `
                -Archive $archive `
                -EntryName ([string]$lock.embedded_sbom.entry) `
                -ExpectedLength ([int64]$lock.embedded_sbom.byte_length) `
                -ExpectedSha256 ([string]$lock.embedded_sbom.sha256) `
                -Context 'Safetensors embedded SBOM'
            $nativeBytes = Read-SafetensorsZipEntry `
                -Archive $archive `
                -EntryName ([string]$lock.native_binary.entry) `
                -ExpectedLength ([int64]$lock.native_binary.byte_length) `
                -ExpectedSha256 ([string]$lock.native_binary.sha256) `
                -Context 'Safetensors native binary'
        } finally {
            $archive.Dispose()
        }
    } else {
        $distribution = (Resolve-Path -LiteralPath $DistributionRoot).Path
        $rootItem = Get-Item -LiteralPath $distribution -Force
        if (-not $rootItem.PSIsContainer -or
            ($rootItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw 'Safetensors distribution root must be a non-reparse directory.'
        }
        $licenseBytes = Read-SafetensorsTreeEntry `
            -Root $distribution `
            -EntryName ([string]$lock.license.entry) `
            -ExpectedLength ([int64]$lock.license.byte_length) `
            -ExpectedSha256 ([string]$lock.license.sha256) `
            -Context 'Safetensors installed license'
        $sbomBytes = Read-SafetensorsTreeEntry `
            -Root $distribution `
            -EntryName ([string]$lock.embedded_sbom.entry) `
            -ExpectedLength ([int64]$lock.embedded_sbom.byte_length) `
            -ExpectedSha256 ([string]$lock.embedded_sbom.sha256) `
            -Context 'Safetensors installed embedded SBOM'
        $nativeBytes = Read-SafetensorsTreeEntry `
            -Root $distribution `
            -EntryName ([string]$lock.native_binary.entry) `
            -ExpectedLength ([int64]$lock.native_binary.byte_length) `
            -ExpectedSha256 ([string]$lock.native_binary.sha256) `
            -Context 'Safetensors installed native binary'
    }

    try {
        $embeddedText = [System.Text.UTF8Encoding]::new($false, $true).GetString($sbomBytes)
        $licenseTextValue = [System.Text.UTF8Encoding]::new($false, $true).GetString($licenseBytes)
    } catch [System.Text.DecoderFallbackException] {
        throw 'Safetensors embedded metadata is not strict UTF-8.'
    }
    if ($embeddedText.IndexOf([char]0) -ge 0 -or $licenseTextValue.IndexOf([char]0) -ge 0) {
        throw 'Safetensors embedded metadata contains a NUL byte.'
    }
    $embedded = $embeddedText | ConvertFrom-Json -Depth 100
    if ([string]$embedded.bomFormat -cne 'CycloneDX' -or
        [string]$embedded.specVersion -cne '1.5' -or
        [int]$embedded.version -ne 1 -or
        [string]$embedded.serialNumber -cne [string]$lock.embedded_sbom.serial_number -or
        [string]$embedded.metadata.component.name -cne 'safetensors-python' -or
        [string]$embedded.metadata.component.version -cne '0.8.0' -or
        @($embedded.components).Count -ne [int]$lock.embedded_sbom.component_count) {
        throw 'Safetensors embedded SBOM identity is invalid.'
    }

    $embeddedByIdentity = [System.Collections.Generic.Dictionary[string, object]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($component in @($embedded.components)) {
        $identity = "$($component.name)@$($component.version)"
        if ($embeddedByIdentity.ContainsKey($identity)) {
            throw "Safetensors embedded SBOM duplicates component $identity."
        }
        $embeddedByIdentity[$identity] = $component
    }
    $transformed = [System.Collections.Generic.List[object]]::new()
    $lockedIdentities = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($locked in @($lock.components)) {
        $identity = "$($locked.name)@$($locked.version)"
        if (-not $lockedIdentities.Add($identity) -or
            -not $embeddedByIdentity.ContainsKey($identity)) {
            throw "Safetensors native closure component set is invalid: $identity"
        }
        $component = $embeddedByIdentity[$identity]
        if ((Get-SafetensorsLicenseExpression -Component $component) -cne
                [string]$locked.license_expression) {
            throw "Safetensors native closure license expression drifted: $identity"
        }
        if ([string]$locked.source_kind -ceq 'crates.io-archive') {
            $hashes = @($component.hashes)
            if ([string]$component.'bom-ref' -cne
                    "registry+https://github.com/rust-lang/crates.io-index#$identity" -or
                [string]$component.purl -cne [string]$locked.purl -or
                $hashes.Count -ne 1 -or
                [string]$hashes[0].alg -cne 'SHA-256' -or
                [string]$hashes[0].content -cne [string]$locked.archive_sha256) {
                throw "Safetensors native closure registry provenance drifted: $identity"
            }
            $contentHash = [string]$locked.archive_sha256
        } elseif ([string]$locked.source_kind -ceq 'pinned-wheel-project') {
            if ($identity -cne 'safetensors@0.8.0' -or
                [string]$locked.purl -cne 'pkg:cargo/safetensors@0.8.0' -or
                -not ([string]$component.'bom-ref').EndsWith(
                    '/safetensors#0.8.0',
                    [System.StringComparison]::Ordinal
                )) {
                throw 'Safetensors native project component provenance drifted.'
            }
            $contentHash = [string]$lock.native_binary.sha256
        } else {
            throw "Safetensors native closure source kind is unsupported: $identity"
        }
        $transformed.Add([pscustomobject][ordered]@{
            type = 'library'
            'bom-ref' = "rust:safetensors-native:$identity"
            name = [string]$locked.name
            version = [string]$locked.version
            hashes = @([ordered]@{ alg = 'SHA-256'; content = $contentHash })
            licenses = @([ordered]@{ expression = [string]$locked.license_expression })
            purl = [string]$locked.purl
            externalReferences = @([ordered]@{
                type = 'distribution'
                url = [string]$lock.wheel.url
            })
            properties = @(
                [ordered]@{ name = 'latentdeck:ecosystem'; value = 'rust' }
                [ordered]@{ name = 'latentdeck:dependency-scope'; value = 'runtime' }
                [ordered]@{
                    name = 'latentdeck:source-kind'
                    value = 'pinned-safetensors-wheel-native-closure'
                }
                [ordered]@{
                    name = 'latentdeck:source-sbom-sha256'
                    value = [string]$lock.embedded_sbom.sha256
                }
                [ordered]@{
                    name = 'latentdeck:wheel-sha256'
                    value = [string]$lock.wheel.sha256
                }
                [ordered]@{
                    name = 'latentdeck:native-binary-sha256'
                    value = [string]$lock.native_binary.sha256
                }
            )
        })
    }
    if ($lockedIdentities.Count -ne $embeddedByIdentity.Count -or
        $transformed.Count -ne [int]$lock.embedded_sbom.component_count) {
        throw 'Safetensors native closure component coverage is incomplete.'
    }

    $canonicalLicense = $licenseTextValue.TrimStart([char]0xFEFF).
        Replace("`r`n", "`n").Replace("`r", "`n").TrimEnd("`n") + "`n"
    $canonicalLicenseBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($canonicalLicense)
    return [pscustomobject]@{
        LockPath = $lockItem.FullName
        Lock = $lock
        Components = $transformed.ToArray()
        LicenseText = [pscustomobject]@{
            Text = $canonicalLicense
            ByteLength = [int64]$canonicalLicenseBytes.Length
            Sha256 = Get-SafetensorsLowerSha256Bytes -Bytes $canonicalLicenseBytes
            RawSha256 = Get-SafetensorsLowerSha256Bytes -Bytes $licenseBytes
        }
        Evidence = [pscustomobject][ordered]@{
            closure_id = [string]$lock.closure_id
            wheel_file_name = [string]$lock.wheel.file_name
            wheel_byte_length = [int64]$lock.wheel.byte_length
            wheel_sha256 = [string]$lock.wheel.sha256
            embedded_sbom_entry = [string]$lock.embedded_sbom.entry
            embedded_sbom_byte_length = [int64]$lock.embedded_sbom.byte_length
            embedded_sbom_sha256 = [string]$lock.embedded_sbom.sha256
            native_binary_entry = [string]$lock.native_binary.entry
            native_binary_byte_length = [int64]$lock.native_binary.byte_length
            native_binary_sha256 = [string]$lock.native_binary.sha256
            component_count = [int]$lock.embedded_sbom.component_count
        }
    }
}

function Test-SafetensorsNativeClosureEvidence {
    [CmdletBinding(DefaultParameterSetName = 'Path')]
    param(
        [Parameter(Mandatory)][object]$Evidence,
        [Parameter(Mandatory, ParameterSetName = 'Path')][string]$SbomPath,
        [Parameter(Mandatory, ParameterSetName = 'Object')][object]$Sbom
    )

    $repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
    $lockPath = Join-Path `
        $repositoryRoot `
        'comfy/latent-cartridge/packaging/safetensors-native-0.8.0.lock.json'
    $lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json -Depth 100
    Get-SafetensorsExactProperties `
        -Object $Evidence `
        -Required @(
            'closure_id', 'wheel_file_name', 'wheel_byte_length', 'wheel_sha256',
            'embedded_sbom_entry', 'embedded_sbom_byte_length', 'embedded_sbom_sha256',
            'native_binary_entry', 'native_binary_byte_length', 'native_binary_sha256',
            'component_count'
        ) `
        -Context 'Safetensors native closure evidence'
    if ([string]$Evidence.closure_id -cne [string]$lock.closure_id -or
        [string]$Evidence.wheel_file_name -cne [string]$lock.wheel.file_name -or
        [int64]$Evidence.wheel_byte_length -ne [int64]$lock.wheel.byte_length -or
        [string]$Evidence.wheel_sha256 -cne [string]$lock.wheel.sha256 -or
        [string]$Evidence.embedded_sbom_entry -cne [string]$lock.embedded_sbom.entry -or
        [int64]$Evidence.embedded_sbom_byte_length -ne [int64]$lock.embedded_sbom.byte_length -or
        [string]$Evidence.embedded_sbom_sha256 -cne [string]$lock.embedded_sbom.sha256 -or
        [string]$Evidence.native_binary_entry -cne [string]$lock.native_binary.entry -or
        [int64]$Evidence.native_binary_byte_length -ne [int64]$lock.native_binary.byte_length -or
        [string]$Evidence.native_binary_sha256 -cne [string]$lock.native_binary.sha256 -or
        [int]$Evidence.component_count -ne [int]$lock.embedded_sbom.component_count) {
        throw 'Safetensors native closure evidence drifted from its reviewed lock.'
    }

    if ($PSCmdlet.ParameterSetName -ceq 'Path') {
        $resolvedSbom = (Resolve-Path -LiteralPath $SbomPath).Path
        $sbomItem = Assert-SafetensorsRegularFile `
            -Path $resolvedSbom `
            -Context 'Safetensors-bound SBOM' `
            -MaximumBytes 32MB
        $Sbom = Get-Content -LiteralPath $sbomItem.FullName -Raw | ConvertFrom-Json -Depth 100
    }
    $nativeComponents = @($sbom.components | Where-Object {
        ([string]$_.'bom-ref').StartsWith(
            'rust:safetensors-native:',
            [System.StringComparison]::Ordinal
        )
    })
    if ($nativeComponents.Count -ne [int]$lock.embedded_sbom.component_count) {
        throw 'Safetensors-bound SBOM does not contain the exact native component count.'
    }
    $seen = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($locked in @($lock.components)) {
        $identity = "$($locked.name)@$($locked.version)"
        $reference = "rust:safetensors-native:$identity"
        $matches = @($nativeComponents | Where-Object { [string]$_.'bom-ref' -ceq $reference })
        if ($matches.Count -ne 1 -or -not $seen.Add($reference)) {
            throw "Safetensors-bound SBOM component coverage is invalid: $identity"
        }
        $component = $matches[0]
        $hashes = @($component.hashes)
        $licenses = @($component.licenses)
        $references = @($component.externalReferences)
        $expectedHash = if ([string]$locked.source_kind -ceq 'crates.io-archive') {
            [string]$locked.archive_sha256
        } else {
            [string]$lock.native_binary.sha256
        }
        $propertyNames = @($component.properties | ForEach-Object { [string]$_.name })
        $expectedProperties = [ordered]@{
            'latentdeck:ecosystem' = 'rust'
            'latentdeck:dependency-scope' = 'runtime'
            'latentdeck:source-kind' = 'pinned-safetensors-wheel-native-closure'
            'latentdeck:source-sbom-sha256' = [string]$lock.embedded_sbom.sha256
            'latentdeck:wheel-sha256' = [string]$lock.wheel.sha256
            'latentdeck:native-binary-sha256' = [string]$lock.native_binary.sha256
        }
        if ([string]$component.type -cne 'library' -or
            [string]$component.name -cne [string]$locked.name -or
            [string]$component.version -cne [string]$locked.version -or
            [string]$component.purl -cne [string]$locked.purl -or
            $hashes.Count -ne 1 -or [string]$hashes[0].alg -cne 'SHA-256' -or
            [string]$hashes[0].content -cne $expectedHash -or
            $licenses.Count -ne 1 -or
            [string]$licenses[0].expression -cne [string]$locked.license_expression -or
            $references.Count -ne 1 -or [string]$references[0].type -cne 'distribution' -or
            [string]$references[0].url -cne [string]$lock.wheel.url -or
            ($propertyNames -join "`0") -cne (@($expectedProperties.Keys) -join "`0")) {
            throw "Safetensors-bound SBOM component drifted: $identity"
        }
        foreach ($propertyName in $expectedProperties.Keys) {
            $matchesForProperty = @($component.properties | Where-Object {
                [string]$_.name -ceq $propertyName
            })
            if ($matchesForProperty.Count -ne 1 -or
                [string]$matchesForProperty[0].value -cne [string]$expectedProperties[$propertyName]) {
                throw "Safetensors-bound SBOM component property drifted: $identity/$propertyName"
            }
        }
    }
    if ($seen.Count -ne $nativeComponents.Count) {
        throw 'Safetensors-bound SBOM native closure contains an unexpected component.'
    }
    return [pscustomobject]@{
        ComponentCount = $nativeComponents.Count
        EmbeddedSbomSha256 = [string]$lock.embedded_sbom.sha256
        NativeBinarySha256 = [string]$lock.native_binary.sha256
        WheelSha256 = [string]$lock.wheel.sha256
    }
}

function Merge-SafetensorsNativeClosureIntoSbom {
    [CmdletBinding(DefaultParameterSetName = 'Wheel')]
    param(
        [Parameter(Mandatory)][string]$SbomPath,
        [Parameter(Mandatory, ParameterSetName = 'Wheel')][string]$WheelPath,
        [Parameter(Mandatory, ParameterSetName = 'Tree')][string]$DistributionRoot
    )

    $resolvedSbom = (Resolve-Path -LiteralPath $SbomPath).Path
    $sbomItem = Assert-SafetensorsRegularFile `
        -Path $resolvedSbom `
        -Context 'target CycloneDX SBOM' `
        -MaximumBytes 32MB
    $parameters = if ($PSCmdlet.ParameterSetName -ceq 'Wheel') {
        @{ WheelPath = $WheelPath }
    } else {
        @{ DistributionRoot = $DistributionRoot }
    }
    $closure = Read-SafetensorsNativeClosure @parameters
    $sbom = Get-Content -LiteralPath $sbomItem.FullName -Raw | ConvertFrom-Json -Depth 100
    if ([string]$sbom.bomFormat -cne 'CycloneDX' -or
        [string]$sbom.specVersion -cne '1.5' -or
        [int]$sbom.version -ne 1 -or
        $null -eq $sbom.metadata.component) {
        throw 'Target CycloneDX SBOM identity is invalid.'
    }
    $components = @($sbom.components)
    if (@($components | Where-Object {
        ([string]$_.'bom-ref').StartsWith(
            'rust:safetensors-native:',
            [System.StringComparison]::Ordinal
        )
    }).Count -ne 0) {
        throw 'Target CycloneDX SBOM already contains a Safetensors native closure.'
    }
    $all = @($components + @($closure.Components) | Sort-Object {
        [string]$_.'bom-ref'
    } -CaseSensitive)
    $references = @($all | ForEach-Object { [string]$_.'bom-ref' })
    if ($references.Count -ne @($references | Sort-Object -CaseSensitive -Unique).Count) {
        throw 'Safetensors native closure merge would create duplicate SBOM references.'
    }
    $sbom.components = $all
    $identityBytes = [System.Text.UTF8Encoding]::new($false).GetBytes(
        ((@(
            [string]$sbom.metadata.component.'bom-ref'
            $references
        ) -join "`n") + "`n")
    )
    $identityHash = [System.Security.Cryptography.SHA256]::HashData($identityBytes)
    $guidBytes = [byte[]]::new(16)
    [System.Array]::Copy($identityHash, $guidBytes, 16)
    $sbom.serialNumber = "urn:uuid:$([guid]::new($guidBytes).ToString())"
    $json = (($sbom | ConvertTo-Json -Depth 100).Replace("`r`n", "`n")) + "`n"
    foreach ($forbidden in @('D:/a/', 'D:\\a\\')) {
        if ($json.IndexOf($forbidden, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw 'Merged Safetensors native SBOM contains an upstream machine-local path.'
        }
    }
    $partial = "$resolvedSbom.partial-$([guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllText($partial, $json, [System.Text.UTF8Encoding]::new($false))
        [System.IO.File]::Move($partial, $resolvedSbom, $true)
    } finally {
        if (Test-Path -LiteralPath $partial -PathType Leaf) {
            [System.IO.File]::Delete($partial)
        }
    }
    return $closure.Evidence
}

Export-ModuleMember -Function @(
    'Read-SafetensorsNativeClosure',
    'Merge-SafetensorsNativeClosureIntoSbom',
    'Test-SafetensorsNativeClosureEvidence'
)
