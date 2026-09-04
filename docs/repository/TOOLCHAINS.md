# Pinned development toolchains

LatentDeck 0.1 uses exact toolchain and dependency locks so the local release
candidate can be rebuilt without inheriting whichever runtimes happen to be on
a developer machine.

## Required runtimes

| Runtime | Pin                   | Repository contract                                                             |
| ------- | --------------------- | ------------------------------------------------------------------------------- |
| Node.js | 24.20.0 LTS (Krypton) | `.node-version`, `.nvmrc`, `tools/Get-PinnedNode.ps1`                           |
| pnpm    | 11.24.0               | `package.json`, `pnpm-lock.yaml`                                                |
| Rust    | 1.93.1 MSVC           | `rust-toolchain.toml`, `Cargo.lock`                                             |
| Python  | 3.13.x                | `.python-version`, `pyproject.toml`, `uv.lock`                                  |
| uv      | 0.11.8                | validates/installs `uv.lock`; H3 enforces it and app RC preparation verifies it |
| NSIS    | 3.11                  | `tools/Get-PinnedNsis.ps1`, `tools/Build-H3CodecPackInstaller.ps1`              |

The Node archive bootstrap verifies the official Windows x64 SHA-256 before
extracting into the ignored `.tools/` directory. Node 25 is EOL and is rejected
by the package engine constraint.

TypeScript 7.0.2 is installed as `@typescript/native` and used by
`svelte-check --tsgo`. The current Svelte checker also requires its legacy
TypeScript 6.0.3 language-service package; this compatibility shim is explicit
in the lock file and is not the application compiler target.

The standalone H3 Codec Pack setup uses the same NSIS family as the application
installers but is built directly rather than through a dummy Tauri application.
`tools/Get-PinnedNsis.ps1` verifies the NSIS 3.11 archive, full extracted tree,
and compiler hashes before placing the build-only tool under ignored `.tools/`.
Application builds add the exact pinned `nsis-tauri-utils 0.5.3` payload with
`-IncludeTauriUtils`, validate the resulting 442-file tree, and copy that tree
into the private Cargo target through Tauri's project-local tools directory.
SBOM and license generation receive this verified path explicitly and do not
depend on a previously populated user-profile Tauri cache.
The aggregate `tools/Check-Workspace.ps1` may bootstrap this exact pinned
build tool when the cache is absent; the Codec Pack builder itself remains
offline unless `-AllowNetwork` is explicit. NSIS is not an end-user
prerequisite: the resulting setup is self-contained apart from its exact
required adjacent `.ldcodec` payload.

## Python codec boundary

The default Python workspace is CPU-light and does not install PyTorch. The H3
codec package declares an explicit `cu130` extra locked to
`torch==2.13.0+cu130` from the official PyTorch index. Codec packs install that
extra independently; application installers never contain weights. The public
Codec Pack setup embeds a statically linked native Rust lifecycle helper, so a
clean user machine needs neither a system Python installation nor PowerShell.

## Local checks

`tools/Check-Workspace.ps1` bootstraps the pinned Node runtime when necessary,
then runs Rust formatting, Clippy and tests; Svelte checks, tests and builds;
Python lint and tests; lock verification; the public-tree audit; and Git
whitespace checks.
