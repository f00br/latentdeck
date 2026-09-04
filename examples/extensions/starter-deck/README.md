# Starter Deck source

This is a complete, data-free, one-source identity Deck. It demonstrates the
smallest external `.ld` source tree that can use the generic Protocol 2 Deck
runtime, host-rendered faceplate, capture controls, and extension lifecycle. Its
CPU `fp32` signal contract deliberately admits the companion synthetic Codec
example by exact profile, shape, dtype, and device; real H3 Decks retain their
separate CUDA and profile constraints.

The committed `deck-pack.json` contains a zero placeholder only for
`integrity.catalog_sha256`; do not edit it by hand. `build` copies the source
to an isolated staging directory, generates the sorted catalog, binds the real
hash, validates the manifest/operator/faceplate schemas and cross-file bindings,
packs deterministically, and reinspects the archive. The source directory
remains unchanged.

Build exactly once into a unique disposable directory, retain the emitted hash,
then use that same archive for inspection and the lifecycle:

```powershell
$work = Join-Path ([IO.Path]::GetTempPath()) ('latentdeck-starter-deck-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
$archive = Join-Path $work 'starter-deck.ld'
$localAppData = Join-Path $work 'LocalAppData'
$receipt = cargo run -q -p latentdeck-extension-manager -- build --source examples/extensions/starter-deck --output $archive | ConvertFrom-Json
$hash = [string]$receipt.inspection.archive_sha256
cargo run -q -p latentdeck-extension-manager -- inspect --archive $archive --expected-sha256 $hash
cargo run -q -p latentdeck-extension-manager -- --local-app-data $localAppData install --archive $archive --expected-sha256 $hash
cargo run -q -p latentdeck-extension-manager -- --local-app-data $localAppData enable --kind deck --id org.example.latentdeck.identity --version 0.1.0
cargo run -q -p latentdeck-extension-manager -- --local-app-data $localAppData matrix
```

The example is Apache-2.0 project code. For a new extension, run `scaffold`
with an identity you control, choose your own license, update the exact signal
contract, then implement and test the operator. Never use `org.latentdeck.*`
for an external package.
