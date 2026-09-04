[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$temporaryRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
    'latentdeck-developer-onboarding-' + [guid]::NewGuid().ToString('N')
)
$previousPythonNoBytecode = $env:PYTHONDONTWRITEBYTECODE
$env:PYTHONDONTWRITEBYTECODE = '1'

function Assert-Condition {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$Condition,
        [Parameter(Mandatory = $true)]
        [string]$Message
    )
    if (-not $Condition) {
        throw $Message
    }
}

function Invoke-JsonCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Executable,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )
    $output = & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Executable failed with exit code $LASTEXITCODE."
    }
    $text = ($output -join [Environment]::NewLine)
    if ([string]::IsNullOrWhiteSpace($text)) {
        throw "$Executable did not emit JSON."
    }
    return $text | ConvertFrom-Json -Depth 100
}

function Invoke-CargoExtensionManager {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )
    $commandArguments = @(
        'run', '--quiet', '-p', 'latentdeck-extension-manager', '--'
    ) + $Arguments
    return Invoke-JsonCommand -Executable 'cargo' -Arguments $commandArguments
}

Push-Location $repositoryRoot
try {
    New-Item -ItemType Directory -Path $temporaryRoot | Out-Null

    $schemaCases = @(
        @('spec/deck-package/deck-pack.schema.json', 'operators/builtin/d2/package/deck-pack.json'),
        @('spec/deck-package/deck-pack.schema.json', 'operators/builtin/q4/package/deck-pack.json'),
        @('spec/deck-package/deck-pack.schema.json', 'examples/extensions/starter-deck/deck-pack.json'),
        @('spec/deck-package/operator.schema.json', 'operators/builtin/d2/package/operator.json'),
        @('spec/deck-package/operator.schema.json', 'operators/builtin/q4/package/operator.json'),
        @('spec/deck-package/operator.schema.json', 'examples/extensions/starter-deck/operator.json'),
        @('spec/deck-package/faceplate.schema.json', 'operators/builtin/d2/package/faceplate.json'),
        @('spec/deck-package/faceplate.schema.json', 'operators/builtin/q4/package/faceplate.json'),
        @('spec/deck-package/faceplate.schema.json', 'examples/extensions/starter-deck/faceplate.json'),
        @('spec/codec-pack/codec-pack.schema.json', 'examples/extensions/synthetic-codec/codec-pack.json'),
        @('spec/extension-package/integrity.schema.json', 'operators/builtin/d2/package/integrity.json'),
        @('spec/extension-package/integrity.schema.json', 'operators/builtin/q4/package/integrity.json'),
        @(
            'comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json',
            'operators/examples/channel-roll/src/latentdeck_example_channel_roll/descriptor.json'
        )
    )
    foreach ($case in $schemaCases) {
        $schemaPath = Join-Path $repositoryRoot $case[0]
        $documentPath = Join-Path $repositoryRoot $case[1]
        Assert-Condition -Condition (Test-Path -LiteralPath $schemaPath -PathType Leaf) `
            -Message "Missing schema: $($case[0])"
        Assert-Condition -Condition (Test-Path -LiteralPath $documentPath -PathType Leaf) `
            -Message "Missing schema example: $($case[1])"
        $valid = Test-Json -Json (Get-Content -Raw -LiteralPath $documentPath) `
            -SchemaFile $schemaPath -ErrorAction Stop
        Assert-Condition -Condition $valid `
            -Message "$($case[1]) does not match $($case[0])."
    }
    $schemaFiles = @(
        'spec/deck-package/deck-pack.schema.json',
        'spec/deck-package/operator.schema.json',
        'spec/deck-package/faceplate.schema.json',
        'spec/codec-pack/codec-pack.schema.json',
        'spec/extension-package/integrity.schema.json',
        'comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json'
    )
    foreach ($relative in $schemaFiles) {
        $schema = Get-Content -Raw -LiteralPath (Join-Path $repositoryRoot $relative) |
            ConvertFrom-Json -Depth 100
        Assert-Condition -Condition (
            [string]$schema.'$id' -like 'https://raw.githubusercontent.com/f00br/latentdeck/*'
        ) -Message "$relative must use the canonical GitHub schema namespace."
    }
    $operatorSchema = Get-Content -Raw -LiteralPath (
        Join-Path $repositoryRoot 'spec/deck-package/operator.schema.json'
    ) | ConvertFrom-Json -Depth 100
    $generalSemVer = [string]$operatorSchema.'$defs'.semver.pattern
    $packageSemVer = [string]$operatorSchema.'$defs'.packageVersion.pattern
    Assert-Condition -Condition ([regex]::IsMatch('1.2.3-RC.1+Build.A', $generalSemVer)) `
        -Message 'General schema SemVer must accept the canonical parser grammar, including uppercase identifiers.'
    Assert-Condition -Condition (-not [regex]::IsMatch('1.2.3-01', $generalSemVer)) `
        -Message 'General schema SemVer must reject a leading-zero numeric prerelease identifier.'
    Assert-Condition -Condition ([regex]::IsMatch('1.2.3-preview.1+build.2', $packageSemVer)) `
        -Message 'Package schema SemVer must accept canonical lowercase storage identities.'
    Assert-Condition -Condition (-not [regex]::IsMatch('1.2.3-RC.1', $packageSemVer)) `
        -Message 'Package schema SemVer must preserve the normative lowercase storage-key restriction.'

    & cargo test --quiet -p latentdeck-extension-manager authoring --lib
    if ($LASTEXITCODE -ne 0) {
        throw 'Extension authoring unit tests failed.'
    }
    & cargo test --quiet -p latentdeck-extension-manager --test public_example_admission
    if ($LASTEXITCODE -ne 0) {
        throw 'Exact selected-source admission test failed.'
    }
    & cargo test --quiet -p latentdeck-extension-manager --test cli_journey `
        cli_scaffolds_and_builds_a_no_clobber_external_deck
    if ($LASTEXITCODE -ne 0) {
        throw 'Extension authoring CLI journey failed.'
    }
    # These parser-only tests do not need Tauri's production custom protocol.
    # Disabling default features keeps this standalone gate independent from a
    # pre-existing frontend dist directory in a clean source checkout.
    & cargo test --quiet -p latentdeck-app --no-default-features `
        public_starter_deck_is_accepted_by_the_host_parsers
    if ($LASTEXITCODE -ne 0) {
        throw 'Starter Deck host-parser validation failed.'
    }

    & uv run --no-sync pytest --quiet `
        comfy/toolkit/tests/test_public_genealogy_example.py `
        operators/examples/channel-roll/tests/test_example_operator.py `
        sdk/deck-python/tests/test_public_starter_deck.py `
        codec-host/python/tests/test_public_synthetic_codec.py
    if ($LASTEXITCODE -ne 0) {
        throw 'CPU-only public developer examples failed.'
    }

    $deckId = 'org.example.latentdeck.onboarding'
    $deckSource = Join-Path $temporaryRoot 'scaffolded-deck-source'
    $deckScaffold = Invoke-CargoExtensionManager -Arguments @(
        'scaffold', '--kind', 'deck', '--id', $deckId, '--version', '0.1.0',
        '--output', $deckSource
    )
    Assert-Condition -Condition ([bool]$deckScaffold.ready_to_build) `
        -Message 'The lifecycle Deck scaffold must be immediately buildable.'
    Assert-Condition -Condition ($deckScaffold.package.package_id -ceq $deckId) `
        -Message 'The lifecycle Deck scaffold identity changed.'
    foreach ($dynamicCase in @(
        @('spec/deck-package/deck-pack.schema.json', 'deck-pack.json'),
        @('spec/deck-package/operator.schema.json', 'operator.json'),
        @('spec/deck-package/faceplate.schema.json', 'faceplate.json')
    )) {
        $dynamicSchema = Join-Path $repositoryRoot $dynamicCase[0]
        $dynamicDocument = Join-Path $deckSource $dynamicCase[1]
        $valid = Test-Json -Json (Get-Content -Raw -LiteralPath $dynamicDocument) `
            -SchemaFile $dynamicSchema -ErrorAction Stop
        Assert-Condition -Condition $valid `
            -Message "Scaffolded $($dynamicCase[1]) does not match $($dynamicCase[0])."
    }
    $previousScaffoldPath = $env:LATENTDECK_SCAFFOLDED_DECK_PATH
    try {
        $env:LATENTDECK_SCAFFOLDED_DECK_PATH = $deckSource
        & cargo test --quiet -p latentdeck-app --no-default-features `
            scaffolded_starter_deck_is_accepted_by_the_host_parsers
        if ($LASTEXITCODE -ne 0) {
            throw 'The exact lifecycle Deck scaffold failed the application host parser.'
        }
    }
    finally {
        if ($null -eq $previousScaffoldPath) {
            Remove-Item Env:LATENTDECK_SCAFFOLDED_DECK_PATH -ErrorAction SilentlyContinue
        }
        else {
            $env:LATENTDECK_SCAFFOLDED_DECK_PATH = $previousScaffoldPath
        }
    }
    $codecSource = Join-Path $temporaryRoot 'synthetic-codec-source'
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'examples/extensions/synthetic-codec') `
        -Destination $codecSource -Recurse
    $pythonRuntime = (& uv run --no-sync python -c `
        'import json, sys; print(json.dumps({"path": sys._base_executable, "version": [sys.version_info.major, sys.version_info.minor], "bits": 64 if sys.maxsize > 2**32 else 32}))'
    ) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not resolve the current base Python executable.'
    }
    Assert-Condition -Condition (
        @($pythonRuntime.version).Count -eq 2 -and
        [int]$pythonRuntime.version[0] -eq 3 -and
        [int]$pythonRuntime.version[1] -eq 13 -and
        [int]$pythonRuntime.bits -eq 64
    ) -Message 'Synthetic Codec onboarding requires the declared CPython 3.13 x64 runtime.'
    Assert-Condition -Condition (Test-Path -LiteralPath $pythonRuntime.path -PathType Leaf) `
        -Message 'The current base Python executable does not exist.'
    $packagedPython = Join-Path $codecSource 'runtime/python.exe'
    Copy-Item -LiteralPath $pythonRuntime.path -Destination $packagedPython
    & $packagedPython -c `
        'import sys; assert sys.version_info[:2] == (3, 13); assert sys.maxsize > 2**32'
    if ($LASTEXITCODE -ne 0) {
        throw 'The temporary Codec runtime Python executable is not runnable.'
    }
    $deckArchive = Join-Path $temporaryRoot 'starter-deck.ld'
    $codecArchive = Join-Path $temporaryRoot 'synthetic-codec.ldcodec'
    $localAppData = Join-Path $temporaryRoot 'LocalAppData'

    $deckBuild = Invoke-CargoExtensionManager -Arguments @(
        'build', '--source', $deckSource, '--output', $deckArchive
    )
    $codecBuild = Invoke-CargoExtensionManager -Arguments @(
        'build', '--source', $codecSource, '--output', $codecArchive
    )
    Assert-Condition -Condition ($deckBuild.inspection.package.package_id -ceq $deckId) `
        -Message 'Scaffolded Deck build identity changed.'
    Assert-Condition -Condition ($codecBuild.inspection.package.package_id -ceq `
        'org.example.latentdeck.synthetic') -Message 'Synthetic Codec build identity changed.'
    Assert-Condition -Condition (
        @($deckBuild.included_files) -contains 'operator.json' -and
        @($deckBuild.included_files) -contains 'faceplate.json' -and
        @($codecBuild.included_files) -contains 'runtime/python.exe'
    ) -Message 'Build receipts do not expose the expected reviewable included-file catalogs.'
    $deckInspection = Invoke-CargoExtensionManager -Arguments @(
        'inspect', '--archive', $deckArchive, '--expected-sha256',
        [string]$deckBuild.inspection.archive_sha256
    )
    $codecInspection = Invoke-CargoExtensionManager -Arguments @(
        'inspect', '--archive', $codecArchive, '--expected-sha256',
        [string]$codecBuild.inspection.archive_sha256
    )
    Assert-Condition -Condition ($deckInspection.package.package_id -ceq $deckId) `
        -Message 'Scaffolded Deck inspect identity changed.'
    Assert-Condition -Condition ($codecInspection.package.package_id -ceq `
        'org.example.latentdeck.synthetic') -Message 'Synthetic Codec inspect identity changed.'

    Invoke-CargoExtensionManager -Arguments @(
        '--local-app-data', $localAppData, 'install', '--archive', $deckArchive,
        '--expected-sha256', [string]$deckBuild.inspection.archive_sha256
    ) | Out-Null
    Invoke-CargoExtensionManager -Arguments @(
        '--local-app-data', $localAppData, 'install', '--archive', $codecArchive,
        '--expected-sha256', [string]$codecBuild.inspection.archive_sha256
    ) | Out-Null
    foreach ($package in @(
        @('deck', $deckId),
        @('codec', 'org.example.latentdeck.synthetic')
    )) {
        Invoke-CargoExtensionManager -Arguments @(
            '--local-app-data', $localAppData, 'verify', '--kind', $package[0],
            '--id', $package[1], '--version', '0.1.0'
        ) | Out-Null
        Invoke-CargoExtensionManager -Arguments @(
            '--local-app-data', $localAppData, 'enable', '--kind', $package[0],
            '--id', $package[1], '--version', '0.1.0'
        ) | Out-Null
    }
    $matrix = Invoke-CargoExtensionManager -Arguments @(
        '--local-app-data', $localAppData, 'matrix'
    )
    Assert-Condition -Condition (@($matrix).Count -eq 1) `
        -Message 'Starter Deck and Synthetic Codec must produce one compatibility pair.'
    Assert-Condition -Condition ($matrix.reason -ceq 'compatible') `
        -Message "Expected compatible matrix pair, received $($matrix.reason)."

    foreach ($package in @(
        @('deck', $deckId),
        @('codec', 'org.example.latentdeck.synthetic')
    )) {
        Invoke-CargoExtensionManager -Arguments @(
            '--local-app-data', $localAppData, 'disable', '--kind', $package[0],
            '--id', $package[1], '--version', '0.1.0'
        ) | Out-Null
        Invoke-CargoExtensionManager -Arguments @(
            '--local-app-data', $localAppData, 'remove', '--kind', $package[0],
            '--id', $package[1], '--version', '0.1.0'
        ) | Out-Null
    }
    $remaining = Invoke-CargoExtensionManager -Arguments @(
        '--local-app-data', $localAppData, 'list'
    )
    Assert-Condition -Condition (@($remaining).Count -eq 0) `
        -Message 'Temporary extension lifecycle did not finish empty.'

    Write-Host 'Developer onboarding checks passed.'
}
finally {
    Pop-Location
    if ($null -eq $previousPythonNoBytecode) {
        Remove-Item Env:PYTHONDONTWRITEBYTECODE -ErrorAction SilentlyContinue
    }
    else {
        $env:PYTHONDONTWRITEBYTECODE = $previousPythonNoBytecode
    }
    $resolvedTemporary = [System.IO.Path]::GetFullPath($temporaryRoot)
    $resolvedSystemTemporary = [System.IO.Path]::GetFullPath(
        [System.IO.Path]::GetTempPath()
    )
    if ($resolvedTemporary.StartsWith($resolvedSystemTemporary, [StringComparison]::OrdinalIgnoreCase) `
        -and (Split-Path -Leaf $resolvedTemporary).StartsWith(
            'latentdeck-developer-onboarding-',
            [StringComparison]::Ordinal
        )) {
        Remove-Item -LiteralPath $resolvedTemporary -Recurse -Force -ErrorAction SilentlyContinue
    }
}
