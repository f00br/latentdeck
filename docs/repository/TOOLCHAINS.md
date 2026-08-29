# Pinned development toolchains

LatentDeck 0.1 uses exact toolchain and dependency locks so the local release
candidate can be rebuilt without inheriting whichever runtimes happen to be on
a developer machine.

## Required runtimes

| Runtime | Pin | Repository contract |
| --- | --- | --- |
| Node.js | 24.20.0 LTS (Krypton) | `.node-version`, `.nvmrc`, `tools/Get-PinnedNode.ps1` |
| pnpm | 11.24.0 | `package.json`, `pnpm-lock.yaml` |
| Rust | 1.93.1 MSVC | `rust-toolchain.toml`, `Cargo.lock` |
| Python | 3.13.x | `.python-version`, `pyproject.toml`, `uv.lock` |
| uv | 0.11.8 or compatible | validates and installs `uv.lock` |

The Node archive bootstrap verifies the official Windows x64 SHA-256 before
extracting into the ignored `.tools/` directory. Node 25 is EOL and is rejected
by the package engine constraint.

TypeScript 7.0.2 is installed as `@typescript/native` and used by
`svelte-check --tsgo`. The current Svelte checker also requires its legacy
TypeScript 6.0.3 language-service package; this compatibility shim is explicit
in the lock file and is not the application compiler target.

## Python codec boundary

The default Python workspace is CPU-light and does not install PyTorch. The H3
codec package declares an explicit `cu130` extra locked to
`torch==2.13.0+cu130` from the official PyTorch index. Codec packs install that
extra independently; application installers never contain weights.

## Local checks

`tools/Check-Workspace.ps1` bootstraps the pinned Node runtime when necessary,
then runs Rust formatting, Clippy and tests; Svelte checks, tests and builds;
Python lint and tests; lock verification; the public-tree audit; and Git
whitespace checks.
