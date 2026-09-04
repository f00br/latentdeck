[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

Import-Module (Join-Path $PSScriptRoot 'PublicNativeBuild.psm1') -Force

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = Join-Path $repositoryRoot 'artifacts'
[System.IO.Directory]::CreateDirectory($artifactsRoot) | Out-Null
$testRoot = Join-Path $artifactsRoot ".public-native-build-$([guid]::NewGuid().ToString('N')) with space"
$savedRustFlags = $env:RUSTFLAGS
$savedEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
$savedCargoTarget = $env:CARGO_TARGET_DIR

function Assert-Throws {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$ExpectedText
    )

    try {
        & $Action
    }
    catch {
        if ($_.Exception.Message -notlike "*$ExpectedText*") {
            throw "Unexpected failure: $($_.Exception.Message)"
        }
        return
    }
    throw "Expected failure containing '$ExpectedText'."
}

try {
    [System.IO.Directory]::CreateDirectory($testRoot) | Out-Null
    $policy = New-PublicRustBuildPolicy `
        -RepositoryRoot $repositoryRoot `
        -AdditionalForbiddenPathRoot @($testRoot) `
        -AdditionalRemapPathRoot @($testRoot) `
        -AdditionalRustArgument @('-C', 'target-feature=+crt-static')
    $decodedArguments = @(
        ([string]$policy.CargoEncodedRustFlags).Split([char]0x1f)
    )
    if (($decodedArguments -join "`0") -cne
        ((@($policy.RustFlagArguments) | ForEach-Object { [string]$_ }) -join "`0") -or
        '-C' -cnotin $decodedArguments -or
        'link-arg=/Brepro' -cnotin $decodedArguments -or
        'target-feature=+crt-static' -cnotin $decodedArguments -or
        '--remap-path-prefix' -cnotin $decodedArguments) {
        throw 'Public Rust build policy did not preserve exact encoded arguments.'
    }
    $remapSources = @($policy.Remaps | ForEach-Object { [string]$_.Source })
    if ($repositoryRoot -cnotin $remapSources -or
        $repositoryRoot.Replace('\', '/') -cnotin $remapSources -or
        $testRoot -cnotin $remapSources -or
        $testRoot.Replace('\', '/') -cnotin $remapSources) {
        throw 'Public Rust build policy must remap native and slash-normalized source roots.'
    }
    for ($index = 1; $index -lt $remapSources.Count; $index++) {
        if ($remapSources[$index - 1].Length -gt $remapSources[$index].Length) {
            throw 'Public Rust build remaps must be ordered broad-first so specificity wins.'
        }
    }

    $env:RUSTFLAGS = '--cfg should_not_survive'
    $env:CARGO_ENCODED_RUSTFLAGS = 'should_not_survive'
    Set-PublicRustBuildPolicy -Policy $policy
    if (Test-Path -LiteralPath 'Env:RUSTFLAGS') {
        throw 'Public Rust build policy did not suppress RUSTFLAGS precedence ambiguity.'
    }
    if ($env:CARGO_ENCODED_RUSTFLAGS -cne [string]$policy.CargoEncodedRustFlags) {
        throw 'Public Rust build policy did not install its exact encoded flags.'
    }

    $safeBytes = [System.Text.Encoding]::UTF8.GetBytes(
        'latentdeck-src/crates/cartridge/src/archive.rs cargo-home/registry'
    )
    Assert-PublicBytesPathHygiene `
        -Bytes $safeBytes `
        -ForbiddenPathRoot @($repositoryRoot, $testRoot) `
        -Context 'Safe synthetic native payload'

    $utf8Leak = [System.Text.Encoding]::UTF8.GetBytes(
        "prefix $($repositoryRoot.Replace('\', '/'))/crates suffix"
    )
    Assert-Throws -ExpectedText 'machine-local build path' -Action {
        Assert-PublicBytesPathHygiene `
            -Bytes $utf8Leak `
            -ForbiddenPathRoot @($repositoryRoot) `
            -Context 'UTF-8 synthetic native payload'
    }
    $utf16Leak = [System.Text.Encoding]::Unicode.GetBytes(
        "prefix $repositoryRoot\crates suffix"
    )
    Assert-Throws -ExpectedText 'machine-local build path' -Action {
        Assert-PublicBytesPathHygiene `
            -Bytes $utf16Leak `
            -ForbiddenPathRoot @($repositoryRoot) `
            -Context 'UTF-16LE synthetic native payload'
    }

    $probeHashes = [System.Collections.Generic.List[string]]::new()
    foreach ($probeName in @('probe-a', 'probe-b')) {
        $crateRoot = Join-Path $testRoot $probeName
        $sourceRoot = Join-Path $crateRoot 'src'
        [System.IO.Directory]::CreateDirectory($sourceRoot) | Out-Null
        [System.IO.File]::WriteAllText(
            (Join-Path $crateRoot 'Cargo.toml'),
            "[package]`nname = `"public-native-probe`"`nversion = `"0.0.0`"`nedition = `"2024`"`n`n[workspace]`n",
            [System.Text.UTF8Encoding]::new($false)
        )
        [System.IO.File]::WriteAllText(
            (Join-Path $sourceRoot 'main.rs'),
            'fn main() { println!("{}", file!()); }' + "`n",
            [System.Text.UTF8Encoding]::new($false)
        )
        $probePolicy = New-PublicRustBuildPolicy `
            -RepositoryRoot $crateRoot `
            -AdditionalForbiddenPathRoot @($repositoryRoot, $testRoot)
        Set-PublicRustBuildPolicy -Policy $probePolicy
        $env:CARGO_TARGET_DIR = Join-Path $crateRoot 'target'
        Push-Location $crateRoot
        try {
            & cargo generate-lockfile --offline
            if ($LASTEXITCODE -ne 0) {
                throw "Public native remap probe lock generation failed with exit code $LASTEXITCODE."
            }
            & cargo build --offline --release --locked
            if ($LASTEXITCODE -ne 0) {
                throw "Public native remap probe build failed with exit code $LASTEXITCODE."
            }
        }
        finally {
            Pop-Location
        }
        $probe = Join-Path $env:CARGO_TARGET_DIR 'release/public-native-probe.exe'
        $probeAudit = Assert-PublicNativeBinary `
            -Path $probe `
            -ForbiddenPathRoot $probePolicy.ForbiddenPathRoots `
            -Context "Compiled public native remap $probeName"
        $probeHashes.Add([string]$probeAudit.Sha256)
        $probeOutput = (& $probe).Trim()
        if ($LASTEXITCODE -ne 0 -or
            $probeOutput.IndexOf($repositoryRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
            $probeOutput.IndexOf($testRoot, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            throw 'Compiled public native remap probe exposed a physical build path at runtime.'
        }
    }
    if ($probeHashes.Count -ne 2 -or $probeHashes[0] -cne $probeHashes[1]) {
        throw 'Equivalent native builds from different roots were not byte-for-byte reproducible.'
    }

    Write-Host 'PUBLIC NATIVE BUILD CONTRACT: PASS' -ForegroundColor Green
}
finally {
    if ($null -eq $savedRustFlags) {
        Remove-Item Env:RUSTFLAGS -ErrorAction SilentlyContinue
    } else {
        $env:RUSTFLAGS = $savedRustFlags
    }
    if ($null -eq $savedEncodedRustFlags) {
        Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
    } else {
        $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustFlags
    }
    if ($null -eq $savedCargoTarget) {
        Remove-Item Env:CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    } else {
        $env:CARGO_TARGET_DIR = $savedCargoTarget
    }
    $expectedPrefix = [System.IO.Path]::GetFullPath($artifactsRoot).TrimEnd('\') +
        '\.public-native-build-'
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if (Test-Path -LiteralPath $resolvedTestRoot) {
        if (-not $resolvedTestRoot.StartsWith(
            $expectedPrefix,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            throw "Refusing to remove an unexpected native-build test path: $resolvedTestRoot"
        }
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
    }
}
