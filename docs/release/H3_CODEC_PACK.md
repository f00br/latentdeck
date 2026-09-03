# H3 Codec Pack packaging and lifecycle

## Purpose and current contract

This runbook covers the independent H3 playback and synthesis runtime for
LatentDeck `0.1.0`. The current package, adapter, and setup version is `0.2.0`.
It is delivered as one hash-bound `.ldcodec` archive plus a small adjacent
Windows setup executable.

The Codec Pack is trusted executable code. It is not a cartridge, Deck,
decoder-weight distribution, or H3 generator. It uses the generic Protocol 2
worker and can serve Player and any compatible `.ld` Deck. Bundled D2/Q4 are
separate Deck packages at `0.2.1`; they are not embedded as H3-specific worker
entrypoints.

The public-release axes are deliberately independent:

| Contract                                      | Current version                           |
| --------------------------------------------- | ----------------------------------------- |
| LatentDeck and LatentPlayer                   | `0.1.0`                                   |
| H3 Codec Pack, adapter, setup, and `.ldcodec` | `0.2.0`                                   |
| Bundled D2 and Q4 `.ld` Decks                 | `0.2.1`                                   |
| Worker Protocol                               | `2`                                       |
| Codec manifest / integrity catalog            | `2.0.0` / `1.0.0`                         |
| Codec Adapter API / tensor ABI                | `1` / `latentdeck.tensor.v1`              |
| LC spec / H3 profile                          | `0.1.0` / `minimax_h3/h3_av_latent/0.1.0` |

Protocol 1 remains only as an explicit legacy Player bridge. There is no
automatic Protocol 1, codec, device, profile, or Deck fallback.

## Required inputs

The release builder takes one mandatory external build input: the official
CPython `3.13.14` Windows x64 embed archive with its pinned filename
`python-3.13.14-embed-amd64.zip`. Its SHA-256 is checked against
`codec-host/codecs/h3/packaging/windows-x64-cu130.lock.json`.

The build host must also expose the exact pinned
`uv 0.11.8 (0e961dd9a 2026-04-27 x86_64-pc-windows-msvc)` and the repository
`.venv\Scripts\python.exe` must be Python 3.13. The builder checks both before
assembling a package. Prepare that locked environment with `uv sync --locked`
as part of the clean workspace gate; it is not an end-user prerequisite.

The reviewed decoder contract defaults to
`codec-host/codecs/h3/packaging/taeh3.asset.json`. It records accepted upstream
TAEH3 identity and licensing metadata only. The actual `taeh3.safetensors`
bytes must remain outside the repository and package; the user selects that
exact external asset in the application.

The locked repository supplies the remaining inputs: the exact dependency
closure, local wheels, curated CPython runtime, third-party notices,
dependency inventory, and CycloneDX SBOM. The builder never discovers or
copies a ComfyUI installation, generator, model tree, cartridge, latent, or
user media.

The packager verifies wheel records and locked versions, checks PE/CPython
identity, rejects reparse points, caches, repository metadata, unbounded or
uncatalogued trees, and prohibited model/media payloads, and writes a strict
integrity catalog. This is a bounded packaging policy, not a malware scanner or
publisher attestation; the exact inputs, notices, inventory, SBOM, and signing
state still require release review.

## Build the release candidate

Build only from the final clean clone selected for the release candidate. The
normal path is offline and requires the locked wheels to be present in the uv
cache:

```powershell
pwsh -NoProfile -File tools/Build-H3CodecPack.ps1 `
  -PythonEmbedArchive $pythonEmbedArchive `
  -PackVersion 0.2.0 `
  -RequireCuda
```

Use `-AllowNetwork` only when deliberately preparing a new dependency cache.
Use `-DecoderAssetContractPath` only for another reviewed metadata contract.
Use `-SigningCommand` only during the separately authorized signing phase. It
signs and verifies the outer setup and the generated uninstaller; the
`.ldcodec` remains an exact hash-bound package rather than an Authenticode
binary.

The builder performs locked installation and curation, package validation,
isolated CUDA/import smoke, Protocol 2 runtime probes, isolated native
install/uninstall, setup assembly, and receipt generation before atomically
publishing `artifacts/codec-pack/0.2.0`. It refuses an existing destination.

Keep the complete output directory together. Its user-facing pair is:

```text
LatentDeck-H3-CodecPack-0.2.0-setup.exe
LatentDeck-H3-CodecPack-0.2.0-windows-x64.ldcodec
```

The complete directory contains exactly these 12 top-level files:

```text
LatentDeck-H3-CodecPack-0.2.0-windows-x64.ldcodec
LatentDeck-H3-CodecPack-0.2.0-setup.exe
SHA256SUMS.txt
package-receipt.json
setup-receipt.json
distributable-proof.json
archive-runtime-smoke.json
installed-runtime-smoke.json
installer-SBOM.cdx.json
INSTALLER_THIRD_PARTY_NOTICES.md
INSTALLER_NSIS_COPYING.txt
INSTALLER_RUST_LICENSES.txt
```

`SHA256SUMS.txt` binds seven transported setup objects: the `.ldcodec`, setup,
installer SBOM, three installer notice/license files, and `setup-receipt.json`.
The package, distributable, and two runtime-smoke receipts report evidence but
are not recursively included in that checksum list. Do not mix any file
between builds or imply that the checksum file covers an item it does not name.

`tools/New-H3CodecPack.ps1` is the low-level deterministic packager for already
curated inputs. Release candidates use `Build-H3CodecPack.ps1` so curation,
runtime checks, and native lifecycle checks cannot be skipped.

## Package contents and identity

The `.ldcodec` contains one version-scoped, integrity-closed tree:

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

`codec-pack.json` manifest `2.0.0` declares:

- pack `org.latentdeck.h3@0.2.0`, adapter `org.latentdeck.h3@0.2.0`, and the
  separate entrypoint `latentdeck_codec_h3.adapter:make_adapter`;
- Windows x86-64, CPython 3.13 `win_amd64`, and Torch `2.13.0+cu130`;
- app range `>=0.1.0,<1.0.0`, Worker Protocol 2, Codec Adapter API 1,
  and tensor ABI `latentdeck.tensor.v1`;
- H3 LC/profile compatibility;
- the single generic `latentdeck_codec_host` worker;
- `player`, `realtime`, `resample`, `snapshot_capture`, `live_capture`, and
  `raw_import` capabilities;
- one exact external TAEH3 asset contract;
- the dependency inventory and integrity catalog identities.

Every payload file is bound by path, byte length, and SHA-256. The runtime uses
isolated Python flags and disables bytecode writes so normal use cannot mutate
the installed pack.

## Public Windows installation

Keep the exact matching setup and `.ldcodec` in the same folder and run the
setup. Renaming, moving, omitting, truncating, or replacing the adjacent
package makes installation fail without exposing a partial pack to discovery.
`SHA256SUMS.txt` is transport evidence; publisher signing remains a separate
mandatory public-release gate.

Setup is offline, current-user only, requires no elevation, and does not need a
system Python or PowerShell. It installs the immutable package at:

```text
%LOCALAPPDATA%\LatentDeck\CodecPacks\org.latentdeck.h3\0.2.0
```

Version-scoped Windows maintenance data lives separately at:

```text
%LOCALAPPDATA%\LatentDeck\CodecPackMaintenance\org.latentdeck.h3\0.2.0
```

The package directory itself must remain integrity-closed. Setup validates the
archive from one exclusive handle, extracts into same-volume staging, verifies
the complete manifest/catalog, and publishes atomically. Re-running setup for
identical installed bytes may restore maintenance metadata but never replaces
or repairs the immutable package.

After installation:

1. Start LatentDeck or LatentPlayer and open **Extensions**.
2. Refresh discovery, enable exact H3 `0.2.0`, and choose **Use in Player** when
   configuring LatentPlayer.
3. Select **CUDA** and the accepted external TAEH3 decoder.
4. Enable bundled D2/Q4 `0.2.1` or another compatible `.ld` Deck separately.

Installing, updating, or removing the Codec Pack does not modify the
applications, Deck packages, Library, cartridges, presets, or external decoder
asset.

## Update, rollback, and removal

There is no in-place payload repair or overwrite. Install a new canonical
SemVer beside an older accepted version, enable the intended exact version,
and smoke-test it. Rollback means disabling or uninstalling only the newer
version and explicitly selecting a retained compatible version. Rebuilding
different bytes under the same version is forbidden.

For normal removal, stop applications and workers, then use **Settings > Apps >
Installed apps > LatentDeck H3 Codec Pack 0.2.0 > Uninstall**. Locked files are
handled interactively with Retry or Cancel; the uninstaller must not terminate
applications itself. Removal targets only this exact pack and maintenance
version.

Engineer-only recovery remains available:

```powershell
pwsh -NoProfile -File tools/Uninstall-H3CodecPack.ps1 `
  -PackVersion 0.2.0 `
  -LifecycleHelperPath $lifecycleHelper
```

`$lifecycleHelper` must be the current
`latentdeck-codec-pack-installer.exe` built from this source tree. The optional
`-RemoveCorrupt` switch is an explicitly authorized recovery operation, not the
normal user path. `-CleanupQuarantine` is retired because the shared extension
lifecycle owns its exact staging and quarantine entries. Silent corrupt-pack
removal is deliberately fail-closed.

## Validation

Run the repository contract gates:

```powershell
pwsh -NoProfile -File tools/Test-H3CodecPackSetup.ps1
pwsh -NoProfile -File tools/Test-ReleasePackaging.ps1
```

Repeat the runtime proof for an expanded or installed exact pack with:

```powershell
pwsh -NoProfile -File tools/Test-H3CodecPackRuntime.ps1 `
  -PackRoot $installedPackRoot `
  -RequireCuda
```

The builder's isolated native-helper install/runtime/uninstall smoke does not
execute the public setup experience; generated setup metadata deliberately
records `windows_setup_lifecycle=not_run_clean_machine_gate`.

These tests do not replace clean-machine acceptance. Before publication, test
the signed pair on a clean Windows 11 NVIDIA current-user account without
PowerShell 7, system Python, ComfyUI, or network access. Verify first install,
Installed Apps registration, Extensions discovery/enabling, TAEH3 selection,
Player, D2, Q4, capture, Spout, notices/SBOM, exact-version uninstall, and
preservation of applications and user data. Record that result separately.
