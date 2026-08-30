# H3 Codec Pack packaging and lifecycle

## Purpose

This runbook is for the engineer assembling the independently installed H3
playback/synthesis runtime. After reading it, they should be able to build an
integrity-catalogued pack from explicitly prepared inputs, install versions
side by side, and remove exactly one version.

The pack is a trusted runtime and adapter distribution. It is not a cartridge
and it is not a model distribution. The release builder is offline by default;
network access is available only through its explicit `-AllowNetwork` switch.

## Required inputs

The reproducible release builder takes two explicit inputs:

1. The official CPython 3.13.14 Windows x64 embed archive, retaining the pinned
   filename `python-3.13.14-embed-amd64.zip`. Its SHA-256 is verified against
   `codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json` before use.
2. A decoder-asset contract JSON file containing metadata only. The reviewed
   default is `codec-host/codecs/h3/packaging/taeh3.asset.json`.

The builder derives everything else from the locked repository: the exact
Windows dependency closure, five local wheels, curated CPython runtime,
third-party notices, dependency inventory, and CycloneDX 1.5 SBOM. It never
discovers a ComfyUI, generator, model directory, or weight. The active
`python313._pth` lines are exactly `python313.zip`, `.`, and
`Lib/site-packages`, in that order. The dot is required for native CPython
stdlib modules such as `_ctypes.pyd`; it does not enable system site packages.

The decoder-asset contract has this shape:

```json
{
  "asset_id": "taeh3",
  "display_name": "TAEH3 decoder weight",
  "kind": "decoder_weight",
  "required": true,
  "selection": "explicit_file",
  "format": "safetensors",
  "accepted_variants": [
    {
      "variant_id": "upstream-variant-id",
      "sha256": "64-lowercase-hex-characters",
      "byte_length": 123,
      "source_url": "https://upstream.example/asset",
      "license_label": "upstream license label",
      "license_url": "https://upstream.example/license"
    }
  ]
}
```

Replace every illustrative value with measured upstream facts. Do not put a
local file path in the contract. The selected decoder weight remains external
and is chosen explicitly in the application after installation.

The curator verifies every original wheel `RECORD`, exact locked distribution
versions and post-curation content digests, bundled license evidence, and file
ownership. It removes installer receipts, build-machine SBOMs, launcher stubs,
caches/tests, and the one pinned non-runtime CPython helper
`ctypes/macholib/fetch_macholib.bat`, then rewrites `RECORD` deterministically.

The packager statically validates PE32+ x86-64 headers and CPython 3.13 version
resources, rejects reparse points, caches, repository metadata, unbounded
trees, uncatalogued files, private-path/credential-like text, environment
files, and known model/latent/media payload types. Nested archives are rejected
except for the exact `python313.zip` location, whose entries are also bounded
and inspected. In particular, the pack may not contain `taeh3` bytes, an H3
Transformer, native HQ VAE, checkpoints, cartridges, raw latents, or user
media.

Known portable-text extensions are decoded as strict UTF-8 and scanned up to a
hard four MiB limit; a larger candidate is rejected instead of skipped. This
is a bounded policy check, not a general malware or semantic-content scanner.

An extension/content-policy scan cannot establish source provenance or prove
the semantics of arbitrary compiled bytes. Review the exact runtime and
package inventory, upstream checksums, notices, and SBOM before distribution.
The local package receipt records this review as required and does not claim
that an unsigned scan is a publisher attestation.

## Build a pack

The command is offline unless `-AllowNetwork` is supplied. All required wheels
must therefore already be present in the uv cache for the normal release path:

```powershell
pwsh -NoProfile -File tools/Build-H3CodecPack.ps1 `
  -PythonEmbedArchive $pythonEmbedArchive `
  -PackVersion 0.1.0 `
  -RequireCuda
```

Pass `-DecoderAssetContractPath $decoderContract` only to use another reviewed
metadata contract. The builder runs locked export, exact target installation,
wheel build, curation, archive validation, isolated CUDA/import smoke, isolated
install, post-install smoke, and exact-version uninstall before atomically
publishing below `artifacts/codec-pack/0.1.0`. It refuses overwrite.

The version directory contains the ZIP, `SHA256SUMS.txt`, path-free package and
distributable proof receipts, plus separate archive and installed-runtime smoke
receipts. The adjacent checksum is transport evidence, not a publisher trust
anchor.

Inside the archive, the version-scoped pack contains:

```text
codec-pack.json
integrity.json
THIRD_PARTY_NOTICES.md
DEPENDENCY_INVENTORY.json
SBOM.cdx.json
runtime/
  python.exe
  python313.dll
  python313._pth
  python313.zip
  Lib/site-packages/...
```

Every payload file is bound by byte length and SHA-256 in `integrity.json`.
`codec-pack.json` binds that catalog, the exact Player/D2/Q4 module entrypoints,
Windows x64 compatibility, app range `>=0.1.0,<0.2.0`, Worker Protocol 1, LC
Spec 0.1, H3 Profile 0.1, and external decoder metadata.

All three Python entrypoints use `-B`, and Core independently fixes
`PYTHONDONTWRITEBYTECODE=1` for every worker process. A successful playback or
synthesis session therefore cannot create `__pycache__` files inside the
integrity-checked installed pack.

`tools/New-H3CodecPack.ps1` remains the low-level packager for already curated
inputs. It requires runtime, site-packages, notices, dependency inventory,
SBOM, and decoder contract paths. Use `Build-H3CodecPack.ps1` for a release
candidate so curation and the install/smoke proof cannot be skipped.

## Private linked runtime testing

Before assembling or installing a distributable pack, a known local Python/CUDA
laboratory can be linked read-only into an isolated app-data root:

```powershell
pwsh -NoProfile -File tools/Start-PrivateH3TestEnvironment.ps1 `
  -PythonRuntimeRoot $pythonRuntimeRoot `
  -PythonSitePackages $pythonSitePackages `
  -Mode PrepareOnly
```

The helper creates a fresh ignored directory below `artifacts/`, builds a
non-distributable linked pack, and imports the Player, D2, and Q4 modules with
the linked runtime. `-Mode LatentDeck` or `-Mode LatentPlayer` starts the chosen
Tauri development application with isolated `LOCALAPPDATA` and `PROGRAMDATA`
values, so discovery never depends on or replaces an installed Codec Pack.
Input runtime/package trees are read only. The helper refuses an existing or
out-of-`artifacts` environment root and never installs, downloads, or modifies
the source laboratory. Its import probe and every worker entrypoint use
Python's `-B` switch so linked package trees never receive bytecode caches.

`tools/Test-LinkedDevCodecPack.ps1` is the public synthetic contract test. It
requires all three entrypoint files, exact manifest argument arrays, and worker
package precedence ahead of the linked laboratory packages.

## Install

Obtain the archive SHA-256 from an independently authenticated owner-approved
release record, then install for the current user:

```powershell
pwsh -NoProfile -File tools/Install-H3CodecPack.ps1 `
  -ArchivePath $packArchive `
  -TrustedArchiveSha256 $trustedPackSha256
```

`SHA256SUMS.txt` and `package-receipt.json` beside the ZIP are useful for
transport-error detection, but an attacker can replace an unsigned ZIP and its
adjacent hashes together. They are not a publisher trust anchor. Do not install
from an adjacent checksum alone. Publisher signing/key verification remains a
mandatory distribution gate beyond this unsigned local RC path.

Current-user versions are installed below
`%LOCALAPPDATA%\LatentDeck\CodecPacks\org.latentdeck.h3\<version>`.
Use `-Scope AllUsers` from an elevated PowerShell process to use the
corresponding `%PROGRAMDATA%` root.

The installer opens the archive once with exclusive sharing, validates the
independently trusted hash, and extracts from that same immutable handle. This
closes the checksum-to-extraction replacement race. It rejects duplicate or
unsafe ZIP entries and reparse points,
verifies exact JSON value kinds plus the strict manifest and every catalogued
file, refuses uncatalogued bytes, and atomically moves the validated version
into discovery. It never overwrites an installed version.

Extraction is staged on the same volume outside discovery, below
`%LOCALAPPDATA%\LatentDeck\CodecPackStaging` for current-user installs (or the
corresponding ProgramData sibling). Therefore a concurrent app scan never sees
an `.install-*` directory as a malformed pack version.

Different versions may coexist in the same scope. Lifecycle commands for the
invoking user are serialized with a named mutex so current-user and elevated
all-users operations from that session cannot race their check-and-move steps.
The installer rejects the same pack/version across the invoking user's
`LOCALAPPDATA` root and ProgramData. It cannot inventory another Windows
account's private `LOCALAPPDATA`; if that user later sees the same version in
both roots, application discovery fails closed instead of choosing one.

Installing or updating a Codec Pack does not modify LatentDeck App,
LatentPlayer, their installers, user cartridges, or the explicitly selected
decoder weight. The current-user Deck application is installed below
`%LOCALAPPDATA%\LatentDeck App`; this distinct root is a lifecycle invariant,
not a display-name change. The application window remains `LatentDeck`.

## Update and rollback

There is no in-place Codec Pack repair or overwrite. Build a new canonical
SemVer, install it beside the old version, then stop/restart the applications so
the highest compatible SemVer is selected automatically and smoke-test it.
Retain the old version until acceptance. Roll back by uninstalling only the new
version, restart the applications, and verify that the retained old version is
selected. Rebuilding different bytes under an already installed version is
deliberately refused. Decoder-weight selection remains a separate explicit
choice.

## Uninstall exactly one version

```powershell
pwsh -NoProfile -File tools/Uninstall-H3CodecPack.ps1 `
  -PackVersion 0.1.0
```

Use the same `-Scope` used for installation. The uninstaller validates the
pack before moving and deleting only the named version. Other versions and
both applications remain untouched.

Stop LatentDeck/LatentPlayer and their Codec Pack workers before uninstalling.
The selected version is first atomically moved out of discovery to the
same-volume `CodecPackTrash` sibling. Once deletion begins, the script never
restores partial remnants into discovery. If a locked DLL prevents cleanup,
the command reports the exact quarantined path; after stopping the worker,
retry safe cleanup with:

```powershell
pwsh -NoProfile -File tools/Uninstall-H3CodecPack.ps1 `
  -PackVersion 0.1.0 `
  -CleanupQuarantine
```

`-CleanupQuarantine` is cleanup-only and always returns without touching an
installed version, including when the same version has since been reinstalled.
Run a separate command without that switch when the live version itself must
be uninstalled.

For recovery from a deliberately confirmed corrupt pack, `-RemoveCorrupt`
skips manifest validation but retains exact root, identifier, version,
containment, and reparse-point checks. Do not use it for a healthy pack.

## Automated contract test

Run the synthetic lifecycle test from the repository root:

```powershell
pwsh -NoProfile -File tools/Test-ReleasePackaging.ps1
```

The test creates only temporary fixtures below the ignored `artifacts` root
and uses a locally installed CPython 3.13 executable/DLL for static identity
checks. It proves independent application installer configuration, the Spout
release feature, strict JSON typing, pack integrity checks, overwrite refusal,
out-of-discovery staging, two-version side-by-side install, exact-version
uninstall, prohibited-payload rejection, dependency metadata validation, and
deterministic archive output for identical inputs. Curator unit tests run
separately:

```powershell
.\.venv\Scripts\python.exe -m pytest -q tools/tests/test_codec_pack_curator.py
```

To repeat the isolated import/CUDA proof for an already expanded or installed
pack:

```powershell
pwsh -NoProfile -File tools/Test-H3CodecPackRuntime.ps1 `
  -PackRoot $installedPackRoot `
  -RequireCuda
```

This local contract test does not execute or prove that a supplied
Python/PyTorch/CUDA runtime starts on another machine. Before release
acceptance, verify upstream provenance, install the real signed/trust-anchored
pack on a clean Windows 11 NVIDIA system, validate its notices and SBOM,
explicitly select a measured `taeh3` asset, and run Player, D2, Q4, recovery,
and Spout tests. Record that result separately; do not infer it from packaging
success.
