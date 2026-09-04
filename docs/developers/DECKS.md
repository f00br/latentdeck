# Build a realtime Deck

A Deck is a versioned realtime instrument distributed as one deterministic
`.ld` package. It combines source topology, logical roles, a typed operator,
signal requirements, a declarative faceplate, integrity metadata, and notices.
The host supplies Library, transport, sessions, native output, Spout, MP4,
capture, presets, and package lifecycle.

Read the normative [Deck Package](../../spec/deck-package/README.md), [Deck
Signal](../../spec/deck-api/README.md), [Deck SDK](../../sdk/deck-python/README.md),
and [Worker Protocol](../../spec/worker-protocol/README.md) contracts before
publishing a package.

## Scaffold a package

Use a reverse-DNS ID you control; `org.latentdeck.*` is reserved.

```powershell
cargo run -p latentdeck-extension-manager -- scaffold --kind deck --id org.example.strange-mixer --version 0.1.0 --output strange-mixer
```

The command refuses an existing output and creates a minimal source tree. The
[starter Deck](../../examples/extensions/starter-deck/README.md) is the
executable reference.

## Define the contract

Edit the scaffolded declarations together:

- `deck-pack.json` defines package identity, application/protocol/runtime
  compatibility, source roles, signal geometry, capabilities, entrypoint,
  license, publisher, and integrity binding.
- `operator.json` defines exact operator identity/API, topology, controls,
  deterministic behavior, and bypass.
- `faceplate.json` maps every required source, role, control, transport, seed,
  action, and single monitor anchor to host-rendered widgets.
- the Python module exports the `module:callable` named by the manifest.
- notices describe the package and every redistributed dependency or asset.

The machine-readable schemas are:

- [`deck-pack.schema.json`](../../spec/deck-package/deck-pack.schema.json)
- [`operator.schema.json`](../../spec/deck-package/operator.schema.json)
- [`faceplate.schema.json`](../../spec/deck-package/faceplate.schema.json)
- shared [`integrity.schema.json`](../../spec/extension-package/integrity.schema.json)

Rust parsing remains authoritative for cross-file semantics that JSON Schema
cannot express. A schema-valid package can still be rejected for mismatched
identities, undeclared controls, invalid role bindings, incompatible signal
requirements, or a bad integrity hash.

## Implement the operator

Export:

```python
def process_sources_host(sources, controls, context):
    ...
```

Return `DeckOperatorResult` and test through `process_sources_checked()`. Use
`context.roles` to resolve logical roles to `context.physical_slots`; changing
roles never reorders the source tuple or its playhead/history. Preserve the
input tensor ABI exactly and use `context.seed` for randomness.

Do not import the codec host, open cartridges, decode, capture, create windows,
call Tauri, or access user paths. Those belong to other components.

## Build and inspect

```powershell
cargo run -p latentdeck-extension-manager -- build --source strange-mixer --output strange-mixer.ld
cargo run -p latentdeck-extension-manager -- inspect --archive strange-mixer.ld
```

`build` copies the source into a private staging tree, creates the sorted
`integrity.json`, binds its hash into the manifest, validates the staged Deck
manifest, operator, faceplate, and generated integrity catalog against the
embedded public Draft 2020-12 schemas, then runs the normative Rust package
parser and cross-file semantic validation. It writes a deterministic archive,
reinspects it, and atomically publishes only after success. It refuses an
existing output and does not modify the source directory.

The build-time schema evaluator is offline. The developer onboarding check
also validates the tracked and dynamically scaffolded examples with the
published schema files, then passes the same scaffold through the normative
package parser and application host parser. That independent gate detects
drift between published and embedded schemas.

The low-level `pack` command remains available only for a source tree that
already contains a correct closed integrity catalog and manifest binding.

## Test the lifecycle

Use a disposable `LOCALAPPDATA` root for CLI tests instead of changing a normal
application installation:

```powershell
$testRoot = Join-Path $env:TEMP "latentdeck-extension-test"
$sha = (Get-FileHash -Algorithm SHA256 .\strange-mixer.ld).Hash.ToLowerInvariant()

cargo run -p latentdeck-extension-manager -- --local-app-data $testRoot install --archive .\strange-mixer.ld --expected-sha256 $sha
cargo run -p latentdeck-extension-manager -- --local-app-data $testRoot enable --kind deck --id org.example.strange-mixer --version 0.1.0
cargo run -p latentdeck-extension-manager -- --local-app-data $testRoot verify --kind deck --id org.example.strange-mixer --version 0.1.0
cargo run -p latentdeck-extension-manager -- --local-app-data $testRoot matrix
cargo run -p latentdeck-extension-manager -- --local-app-data $testRoot disable --kind deck --id org.example.strange-mixer --version 0.1.0
cargo run -p latentdeck-extension-manager -- --local-app-data $testRoot remove --kind deck --id org.example.strange-mixer --version 0.1.0
```

Installation starts disabled. Versions are immutable and selected by exact
identity. A compatible Codec Pack must be installed and enabled before the
matrix can produce a loadable pair. Test incompatible combinations as visible
results, not as reasons to add a fallback.

## Author checklist

- Every declared control and role is used and rendered exactly once.
- Bypass returns the documented identity behavior.
- Operator tests cover every role permutation and state/reset boundary.
- Package/schema/parser validation and deterministic rebuild pass.
- Install, verify, enable, matrix, disable, repair, and remove behavior is
  tested without touching another version.
- The package contains no weight, cartridge, raw latent, generated media,
  credential, machine path, network installer, HTML/JavaScript/CSS faceplate,
  or native window code.
- Publisher identity is described as self-declared unless an authenticated
  distribution mechanism exists.
