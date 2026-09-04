Set-StrictMode -Version Latest

function Get-NormalizedPublicPathRoot {
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Context
    )

    if ([string]::IsNullOrWhiteSpace($Path) -or $Path.IndexOf([char]0) -ge 0) {
        throw "$Context contains an empty or invalid path root."
    }
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $pathRoot = [System.IO.Path]::GetPathRoot($fullPath)
    if ([string]::Equals(
        $fullPath,
        $pathRoot,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        return $fullPath
    }
    return $fullPath.TrimEnd(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
}

function Add-UniquePublicPathRoot {
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[string]]$Destination,

        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.HashSet[string]]$Seen,

        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string]$Context
    )

    $normalized = Get-NormalizedPublicPathRoot -Path $Path -Context $Context
    if ($Seen.Add($normalized)) {
        $Destination.Add($normalized)
    }
}

function New-PublicRustBuildPolicy {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$RepositoryRoot,

        [string[]]$AdditionalForbiddenPathRoot = @(),

        [string[]]$AdditionalRemapPathRoot = @(),

        [string[]]$AdditionalRustArgument = @()
    )

    $repository = Get-NormalizedPublicPathRoot `
        -Path $RepositoryRoot `
        -Context 'Public Rust build repository root'
    $userProfile = [Environment]::GetFolderPath(
        [Environment+SpecialFolder]::UserProfile
    )
    if ([string]::IsNullOrWhiteSpace($userProfile)) {
        $userProfile = $env:USERPROFILE
    }
    if ([string]::IsNullOrWhiteSpace($userProfile)) {
        throw 'Public Rust builds require a resolvable user profile path.'
    }
    $userProfile = Get-NormalizedPublicPathRoot `
        -Path $userProfile `
        -Context 'Public Rust build user profile'

    $cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
        Join-Path $userProfile '.cargo'
    } else {
        $env:CARGO_HOME
    }
    $rustupHome = if ([string]::IsNullOrWhiteSpace($env:RUSTUP_HOME)) {
        Join-Path $userProfile '.rustup'
    } else {
        $env:RUSTUP_HOME
    }
    $temporaryRoot = [System.IO.Path]::GetTempPath()

    $remapCandidates = [System.Collections.Generic.List[object]]::new()
    foreach ($candidate in @(
        [pscustomobject]@{ Source = $repository; Destination = 'latentdeck-src' },
        [pscustomobject]@{ Source = $cargoHome; Destination = 'cargo-home' },
        [pscustomobject]@{ Source = $rustupHome; Destination = 'rustup-home' },
        [pscustomobject]@{ Source = $temporaryRoot; Destination = 'build-temp' },
        [pscustomobject]@{ Source = $userProfile; Destination = 'build-user' }
    )) {
        $remapCandidates.Add($candidate)
    }
    $additionalRemapIndex = 0
    foreach ($additionalRemap in $AdditionalRemapPathRoot) {
        $additionalRemapIndex += 1
        $remapCandidates.Add([pscustomobject]@{
            Source = $additionalRemap
            Destination = "build-root-$additionalRemapIndex"
        })
    }
    $seenRemap = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    $remaps = [System.Collections.Generic.List[object]]::new()
    foreach ($candidate in @($remapCandidates)) {
        $source = Get-NormalizedPublicPathRoot `
            -Path ([string]$candidate.Source) `
            -Context 'Public Rust build remap source'
        foreach ($variant in @($source, $source.Replace('\', '/'))) {
            if ($seenRemap.Add($variant)) {
                $remaps.Add([pscustomobject]@{
                    Source = $variant
                    Destination = [string]$candidate.Destination
                })
            }
        }
    }
    $remaps = [System.Collections.Generic.List[object]]@(
        # rustc applies the last matching remap. Broad roots must precede their
        # nested, more-specific Cargo/Rustup/temp roots.
        $remaps | Sort-Object { ([string]$_.Source).Length }
    )

    $arguments = [System.Collections.Generic.List[string]]::new()
    foreach ($argument in @('-C', 'link-arg=/Brepro') + $AdditionalRustArgument) {
        if ([string]::IsNullOrWhiteSpace($argument) -or
            $argument.IndexOf([char]0x1f) -ge 0 -or
            $argument.Contains("`r") -or $argument.Contains("`n")) {
            throw 'Public Rust build arguments must be non-empty, single-line values without the Cargo separator.'
        }
        $arguments.Add($argument)
    }
    foreach ($remap in $remaps) {
        $arguments.Add('--remap-path-prefix')
        $arguments.Add("$($remap.Source)=$($remap.Destination)")
    }

    $forbiddenRoots = [System.Collections.Generic.List[string]]::new()
    $seenForbidden = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::OrdinalIgnoreCase
    )
    foreach ($path in @(
        $repository,
        $userProfile,
        $cargoHome,
        $rustupHome,
        $temporaryRoot
    ) + $AdditionalForbiddenPathRoot + $AdditionalRemapPathRoot) {
        Add-UniquePublicPathRoot `
            -Destination $forbiddenRoots `
            -Seen $seenForbidden `
            -Path $path `
            -Context 'Public native binary forbidden path root'
    }

    return [pscustomobject]@{
        RustFlagArguments = @($arguments)
        CargoEncodedRustFlags = ($arguments -join [char]0x1f)
        ForbiddenPathRoots = @($forbiddenRoots)
        Remaps = @($remaps)
    }
}

function Set-PublicRustBuildPolicy {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [psobject]$Policy
    )

    if ([string]::IsNullOrWhiteSpace([string]$Policy.CargoEncodedRustFlags)) {
        throw 'Public Rust build policy has no encoded Rust flags.'
    }
    Remove-Item -LiteralPath 'Env:RUSTFLAGS' -ErrorAction SilentlyContinue
    $env:CARGO_ENCODED_RUSTFLAGS = [string]$Policy.CargoEncodedRustFlags
}

function Initialize-PublicPathScanner {
    if ($null -ne ('LatentDeck.PublicPathScanner' -as [type])) {
        return
    }

    Add-Type -TypeDefinition @'
using System;

namespace LatentDeck
{
    public static class PublicPathScanner
    {
        private static byte FoldAscii(byte value)
        {
            if (value >= (byte)'A' && value <= (byte)'Z')
            {
                return (byte)(value + 32);
            }
            return value;
        }

        public static int IndexOfAsciiIgnoreCase(byte[] haystack, byte[] needle)
        {
            if (haystack == null || needle == null || needle.Length == 0 ||
                needle.Length > haystack.Length)
            {
                return -1;
            }
            int limit = haystack.Length - needle.Length;
            for (int start = 0; start <= limit; start++)
            {
                int offset = 0;
                while (offset < needle.Length &&
                    FoldAscii(haystack[start + offset]) == FoldAscii(needle[offset]))
                {
                    offset++;
                }
                if (offset == needle.Length)
                {
                    return start;
                }
            }
            return -1;
        }
    }
}
'@
}

function Assert-PublicBytesPathHygiene {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [AllowEmptyCollection()]
        [byte[]]$Bytes,

        [Parameter(Mandatory)]
        [string[]]$ForbiddenPathRoot,

        [Parameter(Mandatory)]
        [string]$Context
    )

    Initialize-PublicPathScanner
    $seenPatterns = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($rootInput in $ForbiddenPathRoot) {
        $root = Get-NormalizedPublicPathRoot `
            -Path $rootInput `
            -Context "$Context forbidden path root"
        foreach ($variant in @($root, $root.Replace('\', '/'))) {
            foreach ($encoding in @(
                [System.Text.UTF8Encoding]::new($false),
                [System.Text.UnicodeEncoding]::new($false, $false, $true)
            )) {
                $pattern = $encoding.GetBytes($variant)
                $identity = [Convert]::ToBase64String($pattern)
                if (-not $seenPatterns.Add($identity)) {
                    continue
                }
                if ([LatentDeck.PublicPathScanner]::IndexOfAsciiIgnoreCase(
                    $Bytes,
                    $pattern
                ) -ge 0) {
                    throw "$Context contains a machine-local build path: $root"
                }
            }
        }
    }
}

function Assert-PublicNativeBinary {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path,

        [Parameter(Mandatory)]
        [string[]]$ForbiddenPathRoot,

        [string]$Context = 'Public native binary',

        [long]$MaximumBytes = 512MB
    )

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $item = Get-Item -LiteralPath $resolved -Force
    if ($item.PSIsContainer -or
        ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0 -or
        $item.Length -le 0 -or $item.Length -gt $MaximumBytes) {
        throw "$Context is not a bounded regular file: $Path"
    }
    $bytes = [System.IO.File]::ReadAllBytes($resolved)
    Assert-PublicBytesPathHygiene `
        -Bytes $bytes `
        -ForbiddenPathRoot $ForbiddenPathRoot `
        -Context $Context

    return [pscustomobject]@{
        Path = $resolved
        ByteLength = [int64]$item.Length
        Sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

Export-ModuleMember -Function @(
    'New-PublicRustBuildPolicy',
    'Set-PublicRustBuildPolicy',
    'Assert-PublicBytesPathHygiene',
    'Assert-PublicNativeBinary'
)
