# Developer quickstart

This guide gets a clean source checkout to a verified state and points to the
smallest command for each development surface. It does not install model
weights. The complete workspace and all-package Python routes do install the
locked H3 development dependencies, including the pinned CUDA Torch build.

## Prerequisites

LatentDeck 0.1 development is pinned to:

- Windows x64;
- Rust `1.93.1` with MSVC, rustfmt, and Clippy;
- Node.js `24.20.0` and pnpm `11.24.0`;
- Python `3.13.x` and uv `0.11.8`;
- NSIS `3.11` only for Windows installer work.

Exact pins and bootstrap behavior are documented in [Pinned
toolchains](../repository/TOOLCHAINS.md). Do not substitute a newer runtime and
then update lock files as a side effect of unrelated work.

## Clone and verify

```powershell
git clone https://github.com/f00br/latentdeck.git
Set-Location latentdeck
git status --short
pwsh -NoProfile -File tools/Check-Workspace.ps1
```

The aggregate check bootstraps the pinned Node runtime when needed and verifies
Rust format/Clippy/tests, frontend checks/tests/builds, Python lint/tests, lock
files, package contracts, public documentation, public-tree policy, and Git
whitespace. It may create ignored caches and build output, but a successful run
must not change tracked files.

For Python-only iteration, create the locked environment once:

```powershell
uv sync --all-packages --all-extras --no-editable --locked
uv run --no-sync pytest
uv run --no-sync ruff check .
```

The root project has no runtime dependencies and is not itself a package, so a
plain `uv sync --locked` does not install the workspace members needed by the
repository-wide test suite.

For Rust-only iteration:

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Run focused package/application commands from the component's README before the
aggregate gate.

## Open an application during development

Use the workspace scripts after the pinned Node dependencies are available:

```powershell
pnpm install --frozen-lockfile
pnpm dev:player
```

or:

```powershell
pnpm dev:deck
```

Development launch is not release evidence. Final artifacts are built only by
the maintainer process from a clean committed clone.

## Start an extension

The source checkout exposes the authoring CLI through Cargo:

```powershell
cargo run -p latentdeck-extension-manager -- scaffold --kind deck --id org.example.my-deck --version 0.1.0 --output my-deck
cargo run -p latentdeck-extension-manager -- scaffold --kind codec --id org.example.my-codec --version 0.1.0 --output my-codec
```

Continue with [Deck authoring](DECKS.md) or [Codec authoring](CODECS.md). For a
smaller latent-math experiment, start with [Operators](OPERATORS.md) instead.

Run the complete CPU-first public authoring journey at any time:

```powershell
pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1
pwsh -NoProfile -File tools/Test-ComfyRecorderBundle.ps1
```

It validates tracked and dynamically scaffolded examples against the published
schemas, exercises build-time embedded-schema validation, authoritative Rust
parsers, and the application Deck host parser, creates runtime-generated
cartridge genealogy, tests the research operator and synthetic Codec, and
completes a disposable Deck/Codec
build→inspect→install→verify→enable→matrix→disable→remove lifecycle.
The Recorder bundle check separately builds the Windows `cp312-abi3`
Cartridge SDK, installs the offline bundle into disposable ComfyUI roots, and
imports it with real CPython 3.12 and 3.13 interpreters. It proves that an
unsupported ABI, an altered wheel, and an existing destination fail closed.

## Developer Kit route

The Windows preview release also provides
`LatentDeck-0.1.0-preview.1-developer-kit-windows-x64.zip`. It contains the
project wheels, Cartridge and extension-manager CLIs, schemas, examples, a
Python 3.13 bootstrap script, the exact standalone Comfy Recorder bundle,
compatibility manifest, hashes, SBOM, and notices.

Extract it into a new directory, verify its top-level `SHA256SUMS.txt`, and run
`bootstrap/Install-ProjectWheels.ps1` from PowerShell. The bootstrap creates a
local `.latentdeck-dev` environment and installs only the nine hash-bound
project wheels; it does not download model weights, install third-party runtime
dependencies, or grant trust to an extension. Use the source repository's
locked environment or a documented Codec runtime profile for the external
dependencies required by a particular workflow. Consult
`DEVELOPER-KIT-MANIFEST.json` for the exact included package versions.

The source checkout remains required for changing core applications and running
the complete workspace gate.

## Before opening a pull request

Read [CONTRIBUTING.md](../../CONTRIBUTING.md), run the relevant focused tests,
then run:

```powershell
pwsh -NoProfile -File tools/Test-PublicDocumentation.ps1
pwsh -NoProfile -File tools/Test-PublicTree.ps1
pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1
git diff --check
git status --short
```

Review every changed and untracked file. Build output, environments, media,
weights, cartridges, raw latents, and diagnostics must remain untracked.
