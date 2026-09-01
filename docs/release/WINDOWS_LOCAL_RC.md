# Windows local release candidate 0.1.0

## Purpose

This runbook is for the engineer producing and testing the two local Windows
application installers. After reading it, they should be able to build one
auditable, unsigned `0.1.0` artifact set without publishing anything.

The release set contains two independent applications:

- **LatentDeck App**, identifier `studio.latentdeck.deck` (window title
  `LatentDeck`);
- **LatentPlayer**, identifier `studio.latentdeck.player`.

Both use current-user NSIS installation, carry version `0.1.0`, set
`allowDowngrades: false`, and enable the release `spout-sdk` feature. This
rejects silent `/S` downgrades; an interactive explicit
uninstall-before-install downgrade remains possible as documented below. They
do not contain a Codec Pack, decoder weights, cartridges, raw latents, private
media, or updater artifacts. The H3 Codec Pack has a separate public
current-user setup and adjacent payload defined in
[H3 Codec Pack packaging and lifecycle](H3_CODEC_PACK.md); it is not a third
application bundle and is never folded into either Tauri installer.

## Prerequisites

Build on Windows x64 with the repository's pinned Rust toolchain and the MSVC
C++ build tools. The build helper acquires the pinned Node/pnpm runtime, runs a
frozen dependency install, prepares the hash-pinned Spout2 source, and invokes
the pinned Tauri CLI. The CLI version is checked only after the frozen install,
so a clean machine does not depend on a previously populated `node_modules`.
Generating the mandatory SBOM also requires `uv 0.11.8` on `PATH`; this is the
validated release-tool version and provides the locked CycloneDX 1.5 export
used here.

Keep the clean source/build root short, for example directly below a drive root.
A deeply nested throwaway clone can exceed MSVC FileTracker path limits before
Rust or application code runs. Repeating the same build from a short path is the
correct diagnostic for that toolchain failure; it is not an application-runtime
regression.

Tauri produces Windows NSIS installers with `tauri build`. A current-user NSIS
install does not require administrator privileges and installs below the
current user's local application data directory. The configured WebView2 mode
uses Tauri's download bootstrapper when a suitable runtime is absent. See the
[official Tauri Windows installer guide](https://v2.tauri.app/distribute/windows-installer/).

Network access is needed when the pinned Node archive, pnpm packages, Spout2
archive, Rust crates, or Tauri's NSIS tooling are not already cached. A locally
available Spout2 archive may be supplied explicitly; it is still checked
against the repository pin before use.

`Prepare-Spout2.ps1` maintains the normal ignored developer source cache. The
release builder does not compile from that mutable cache: it opens the exact
pinned archive with exclusive sharing, verifies its byte length and SHA-256,
and extracts it into a fresh private build directory for the two Tauri builds.

## Build

From the repository root:

```powershell
pwsh -NoProfile -File tools/Build-ReleaseCandidate.ps1
```

The builder performs the frozen pnpm install and generates the public-safe SBOM
inside its unique private staging directory. A prebuilt `-SbomPath` is rejected
so an older, schema-valid inventory cannot be reused accidentally. The fresh
SBOM is validated as strict UTF-8 CycloneDX 1.5 with the `LatentDeck` component
version `0.1.0`, a bounded non-empty component list, no machine-local paths or
file URIs, and exactly one upstream `Spout2` component with tag, commit, pinned
archive SHA-256, native C++ integration provenance, and the `BSD-2-Clause`
license. Cargo metadata alone is not sufficient because Spout2 is compiled from
separately prepared native source rather than a Cargo package.

To use an already downloaded Spout2 archive:

```powershell
pwsh -NoProfile -File tools/Build-ReleaseCandidate.ps1 `
  -SpoutArchivePath $spoutArchive
```

The helper passes the following release properties to each Tauri build:

- target `x86_64-pc-windows-msvc`;
- bundle type `nsis` only;
- Rust feature `spout-sdk`;
- `--no-sign` and `--ci`;
- Cargo runner argument `--locked`.

The frozen pnpm install is anchored to the repository root even when this
script is invoked by absolute path from another directory. The builder records
Node, pnpm, Tauri CLI, verbose rustc/Cargo facts, hashes of `Cargo.lock`,
`pnpm-lock.yaml`, and `uv.lock`, the Spout2 tag/commit/archive pin, Git
commit/branch/dirty state, and a deterministic hash of the public source
candidate. The SBOM records the same three lock hashes. The builder rechecks
them after SBOM generation, after both Tauri builds, and immediately before
atomic finalization; it also fails if the source snapshot changes.
`Test-PublicTree.ps1` must pass both before the snapshot and after compilation.
A dirty local candidate is recorded honestly; rebuild from the final clean
commit before treating an artifact as a durable publication candidate.

Cargo build products are placed in temporary directories below the ignored
`artifacts` root. The final set is staged as
`artifacts/release-candidate/0.1.0-windows-x64`. The helper refuses to replace
that directory. Move an accepted prior set elsewhere before deliberately
building another candidate.

## Artifact contract

A successful set contains only:

```text
BUILD-COMMANDS.txt
release-candidate.json
SHA256SUMS.txt
installers/
  LatentDeck-0.1.0-windows-x64-unsigned-setup.exe
  LatentPlayer-0.1.0-windows-x64-unsigned-setup.exe
metadata/
  THIRD_PARTY_NOTICES.md
  latentdeck-0.1.0-sbom.cdx.json
```

The builder requires Tauri's exact source bundle names, checks the complete
DOS/PE signature, expected NSIS PE32 bootstrapper fields, and minimum sizes,
assigns canonical unsigned names, records byte lengths and SHA-256 values, and
then measures the staged files again. The mandatory SBOM
is copied under `metadata`, with its byte length, component count, and SHA-256
recorded in `release-candidate.json` together with the three lock hashes from
which it was generated; its digest is also included in `SHA256SUMS.txt`. The
reviewed Spout2 notice and full BSD-2-Clause text are
copied to `metadata/THIRD_PARTY_NOTICES.md`; its byte length and SHA-256 are
bound by the schema-3 RC receipt and the same checksum list. Neither receipt
hashes itself, so there is no recursive hash contract. The builder refuses an
unexpected file, an existing destination, a notice with changed required text,
or an SBOM without the exact pinned upstream component.

`release-candidate.json` explicitly records that the set is local, unsigned,
Spout-enabled, and contains no Codec Pack, model weights, or cartridges.
Keep the `metadata` directory with the installers whenever this binary set is
copied or reviewed. The official D3D12 receiver used for QA is not an
application dependency. A portable ignored local copy may exist under
`vendor-local`, but the receiver is never tracked or committed and is never
included in either installer or the release-candidate set.

## Install, update, and uninstall behavior

LatentDeck App and LatentPlayer retain different product names and identifiers,
so installing or removing one must not install, upgrade, or remove the other.
Both shortcuts may appear in the shared `LatentDeck` Start Menu folder.

The Deck installer product name deliberately includes `App`. With current-user
NSIS this keeps the application tree at `%LOCALAPPDATA%\LatentDeck App`, outside
the independently managed Codec Pack tree at
`%LOCALAPPDATA%\LatentDeck\CodecPacks`. The visible application window remains
`LatentDeck`, and the stable application identifier remains
`studio.latentdeck.deck`. Do not collapse these install roots: uninstalling or
upgrading the Deck application must never remove, replace, or own Codec Packs.

For an interactive upgrade, run the newer installer under the same product
name and identifier and choose Tauri NSIS's uninstall-before-install path. A
same-version installer exposes its maintenance/reinstall flow. With the pinned
Tauri NSIS template and `allowDowngrades: false`, a silent `/S` downgrade is
rejected. An interactive older installer warns and can proceed only through its
explicit uninstall-before-install choice; this is not a hard GUI downgrade
block. Product names and identifiers are release identities and must not be
renamed between patch releases.

The NSIS installers also accept the uppercase `/S` switch for unattended test
installation, as documented by Tauri. Do not use a silent install as the only
release acceptance test because it hides the interactive maintenance choices.

Remove each app independently through Windows Installed Apps or its generated
uninstaller. Application uninstall is not Codec Pack uninstall. The shared H3
Codec Pack has its own version-scoped Windows Installed Apps entry and
uninstaller outside the integrity-closed pack directory.

## Acceptance matrix

Run the following on a disposable, clean Windows 11 x64 machine with NVIDIA
hardware and without ComfyUI:

1. Verify both installer hashes plus the SBOM and
   `THIRD_PARTY_NOTICES.md` hashes against
   `SHA256SUMS.txt`; keep the complete set together during testing.
2. Install LatentPlayer only; verify launch, missing-codec UI, and uninstall.
3. Install LatentDeck only; verify launch, missing-codec UI, and uninstall.
4. Install both in each order; remove each in each order and prove the other
   application remains launchable.
5. Install an older local test build, upgrade to `0.1.0`, and prove that only
   the selected product is replaced.
6. Re-run `0.1.0` interactively and exercise the maintenance/reinstall path.
7. Attempt a silent `/S` downgrade and verify that it is rejected. Then run the
   same older installer interactively and verify that it warns and requires an
   explicit uninstall-before-install choice before the downgrade can proceed.
8. Keep `LatentDeck-H3-CodecPack-0.1.1-setup.exe` beside its exact matching
   `LatentDeck-H3-CodecPack-0.1.1-windows-x64.zip`, then install through setup
   without elevation, PowerShell, system Python, a model, or setup-time network
   access. Select an external decoder weight, then test playback, D2/Q4, native
   output, and Spout receiver shutdown.
9. Remove each application and prove the independently installed Codec Pack is
   unaffected; then remove only Codec Pack `0.1.1` through its Windows Installed
   Apps entry and prove both applications and user data remain.

## Current publication boundary

The local builders and synthetic packaging lifecycle are automatable. A real
clean-machine install, including the H3 setup/adjacent-payload and exact-version
uninstall path, NVIDIA runtime test, Spout receiver proof, and
install/update/uninstall matrix are external acceptance gates and must be
recorded only after they are actually run. These installers are intentionally
unsigned; Windows may show an unknown-publisher warning. Authenticated signing
of the application installers, H3 setup, and its generated uninstaller remains
a separate publication gate.

No script in this path creates a Git tag, remote, release, upload, or updater
feed.
