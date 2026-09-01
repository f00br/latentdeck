# H3 Codec Pack packaging and lifecycle

## Purpose

This runbook covers both assembly of the independently installed H3
playback/synthesis runtime and its public Windows lifecycle. An engineer should
be able to build an integrity-catalogued pack plus its setup, while a user needs
only the matching setup and adjacent payload to install or remove one exact
version.

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
  -PackVersion 0.1.1 `
  -RequireCuda
```

Pass `-DecoderAssetContractPath $decoderContract` only to use another reviewed
metadata contract. The builder runs locked export, exact target installation,
wheel build, curation, archive validation, isolated CUDA/import smoke, a
no-payload D2/Q4 Live Capture loop-preservation probe, isolated install,
post-install smoke, public-setup assembly, and exact-version uninstall before
atomically publishing below `artifacts/codec-pack/0.1.1`. It refuses
overwrite.

The version directory contains the small
`LatentDeck-H3-CodecPack-0.1.1-setup.exe`, its required adjacent
`LatentDeck-H3-CodecPack-0.1.1-windows-x64.zip`, `SHA256SUMS.txt`, path-free
package, setup, and distributable proof receipts, plus separate archive and
installed-runtime smoke receipts. It also carries the installer-specific
`installer-SBOM.cdx.json`, `INSTALLER_THIRD_PARTY_NOTICES.md`,
`INSTALLER_NSIS_COPYING.txt`, and `INSTALLER_RUST_LICENSES.txt`; keep the
complete directory together. `SHA256SUMS.txt` binds setup, payload, setup
receipt, and all four installer sidecars. The adjacent checksum is transport
evidence, not a publisher trust anchor.

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

The public `0.1` release keeps these version axes explicit:

| Axis | Version |
| --- | --- |
| Codec Pack, setup, and distribution artifact | `0.1.1` |
| LatentDeck apps, H3 adapter/worker, Python/Rust packages, and D2/Q4 operators | `0.1.0` |
| LC Spec and H3 profile | `0.1.0` |
| Worker Protocol | `1` |
| Codec Pack manifest and integrity catalog | `1.0.0` |

The pack version identifies the immutable distribution payload; it is not an
alias for the bundled adapter version. In particular, the `0.1.1` pack must
declare the adapter version actually exposed by Player, D2, and Q4: `0.1.0`.

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

## Install with the public Windows setup

Keep these exact matching files in one folder:

```text
LatentDeck-H3-CodecPack-0.1.1-setup.exe
LatentDeck-H3-CodecPack-0.1.1-windows-x64.zip
```

Run `LatentDeck-H3-CodecPack-0.1.1-setup.exe`. The large ZIP is deliberately
adjacent rather than embedded; renaming, moving, omitting, truncating, or
replacing it makes setup fail without exposing a partial pack to application
discovery. The setup is bound to that exact filename, byte length, SHA-256,
pack identity, and version. `SHA256SUMS.txt` and the receipts are useful
transport evidence, but the authenticated setup is the publisher trust anchor
for its adjacent payload. Publisher signing remains a mandatory release gate;
an unsigned local RC is not a public trust claim.

The setup is offline and current-user only. It exposes no install-directory
choice, requests no administrator elevation, performs no download, and needs
neither PowerShell nor a system Python installation. It does not install a
model or decoder weight. The fixed destination is:

```text
%LOCALAPPDATA%\LatentDeck\CodecPacks\org.latentdeck.h3\0.1.1
```

Setup opens the adjacent ZIP once with exclusive sharing, verifies its bound
identity, and validates and extracts from that same handle. It rejects
duplicate or unsafe ZIP entries and reparse points, verifies the strict
manifest and every catalogued file, refuses uncatalogued bytes, and atomically
moves only a complete version from same-volume staging into discovery. A
concurrent app scan never sees an `.install-*` directory as a malformed pack.

The pack directory is an integrity-closed tree. The native helper is embedded
in setup and in the generated uninstaller and is extracted only to Windows'
temporary plugin directory while a lifecycle operation runs; no mutable loose
helper is installed. The uninstaller, installation metadata, SBOM, and notices
used by Windows maintenance are stored separately below the version-scoped
sibling root
`%LOCALAPPDATA%\LatentDeck\CodecPackMaintenance\org.latentdeck.h3\0.1.1`.
None of these bytes may be copied into the pack directory because any
uncatalogued file there invalidates discovery.

Setup registers `LatentDeck H3 Codec Pack 0.1.1` in Windows Installed Apps.
The registry entry is maintenance UI only; the filesystem remains the sole
Codec Pack discovery authority. Re-running setup for already installed,
matching immutable bytes may restore maintenance metadata, but it never
overwrites or repairs the pack payload.

Installing or updating a Codec Pack does not modify LatentDeck App,
LatentPlayer, their installers, user cartridges, or the explicitly selected
decoder weight. The current-user Deck application is installed below
`%LOCALAPPDATA%\LatentDeck App`; this distinct root is a lifecycle invariant,
not a display-name change. The application window remains `LatentDeck`.

After setup finishes, restart LatentDeck and LatentPlayer, refresh Codec Manager
if needed, confirm the expected pack version is selected, and explicitly select
an accepted external TAEH3 decoder asset.

### Developer lifecycle command

`tools/Install-H3CodecPack.ps1` remains an engineer-only packaging, isolated
test, and recovery surface. It requires `pwsh` plus an independently trusted
archive hash and does not create the normal Windows Installed Apps experience.
Public users should run the setup executable above.

## Update and rollback

There is no in-place Codec Pack repair or overwrite. Build a new canonical
SemVer and install it beside previously accepted versions, then restart the
applications so the highest compatible SemVer is selected automatically and
smoke-test it. At most 16 versions of one pack identifier may exist in a
discovery root; setup refuses a seventeenth before changing discovery.

Rollback means uninstalling only the newer version and returning to a retained,
previously accepted compatible version. The earlier local H3 Codec Pack
`0.1.0` predates the D2 multi-loop correction and is known defective; it is not
a rollback candidate for `0.1.1` and must be removed from the owner test
machine. Rebuilding different bytes under an already installed version remains
forbidden. Decoder-weight selection is a separate explicit choice.

## Uninstall exactly one version

Use Windows **Settings > Apps > Installed apps**, choose
`LatentDeck H3 Codec Pack 0.1.1`, and select Uninstall. This entry removes only
that exact pack version and its version-scoped maintenance data. It does not
remove another pack version, LatentDeck App, LatentPlayer, cartridges, or the
external decoder selection.

Stop LatentDeck, LatentPlayer, and their Codec Pack workers before uninstalling.
If a file is locked, the interactive uninstaller asks the user to close the
process and Retry or Cancel; it never terminates an application automatically.
If strict validation reports a corrupt exact version, interactive removal
requires explicit confirmation and retains the same containment and
reparse-point protections.

Silent `Uninstall.exe /S` is deliberately fail-closed for an integrity-corrupt
pack and returns an error rather than forcing removal. Corrupt removal requires
the interactive confirmation above or the explicitly authorized developer
recovery command below.

The developer recovery command remains available:

```powershell
pwsh -NoProfile -File tools/Uninstall-H3CodecPack.ps1 `
  -PackVersion 0.1.1
```

Its `-CleanupQuarantine` and `-RemoveCorrupt` switches remain engineer-only
recovery operations. They retain exact root, identifier, version, containment,
and reparse-point checks and must not be used as the normal public uninstall
path.

## Automated contract test

Run the synthetic lifecycle test from the repository root:

```powershell
pwsh -NoProfile -File tools/Test-H3CodecPackSetup.ps1
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

To repeat the isolated import/CUDA and D2/Q4 Live Capture loop-preservation
proof for an already expanded or installed pack:

```powershell
pwsh -NoProfile -File tools/Test-H3CodecPackRuntime.ps1 `
  -PackRoot $installedPackRoot `
  -RequireCuda
```

These local contract tests do not prove the public lifecycle or supplied
Python/PyTorch/CUDA runtime on another machine. Before publishing signed public
artifacts, place only the signed setup and matching adjacent ZIP on a clean
Windows 11 NVIDIA current-user account without PowerShell 7, system Python,
ComfyUI, or a network connection. Install through setup without elevation,
verify its Installed Apps entry, validate notices and SBOM, explicitly select a measured
`taeh3` asset, and run Player, D2, Q4, recovery, and Spout tests. Uninstall the
exact pack version through Installed Apps and prove both applications and user
data remain. Record that result separately; do not infer it from packaging
success.
