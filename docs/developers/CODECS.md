# Build a Codec Pack

A Codec Pack connects one latent family/profile to LatentDeck through the typed
Codec SDK and Worker Protocol 2. It is distributed as a deterministic
`.ldcodec` package and may contain an isolated runtime, so it is trusted
executable code rather than data-only cartridge media.

Read the normative [Codec Package](../../spec/codec-pack/README.md), [Worker
Protocol](../../spec/worker-protocol/README.md), [Codec SDK](../../sdk/codec-python/README.md),
and profile contract before implementation.

## Scaffold a package

Use a reverse-DNS namespace you control:

```powershell
cargo run -p latentdeck-extension-manager -- scaffold --kind codec --id org.example.synthetic-codec --version 0.1.0 --output synthetic-codec
```

The [synthetic Codec example](../../examples/extensions/synthetic-codec/README.md)
implements the complete CPU-first contract without weights, private media, or
H3. Use it to prove that a new adapter depends on public contracts rather than
product-specific H3 behavior.

A Codec scaffold is not immediately packageable: the author must provide the
declared isolated CPython 3.13 runtime, Codec Host/SDK and adapter dependencies,
runtime lock, and license material. The scaffold never copies an ambient Python
environment or downloads those inputs on the author's behalf.

## Declare package and profile identity

`codec-pack.json` declares:

- package and adapter identities/versions;
- supported application, Protocol 2, host API, Python, Torch, and platform
  ranges;
- one or more exact codec/profile/version keys;
- all five mandatory capabilities: Player, realtime, resample, Snapshot, and
  Live Capture;
- optional `raw_import` only when the adapter implements it;
- runtime entrypoint and launch contract;
- external assets by exact SHA-256, byte length, source, and license;
- integrity-catalog identity.

Validate structure with [`codec-pack.schema.json`](../../spec/codec-pack/codec-pack.schema.json)
and [`integrity.schema.json`](../../spec/extension-package/integrity.schema.json).
The Rust parser additionally enforces cross-field identity, path, version,
capability, and lifecycle rules.

## Implement the adapter

Provide the complete `CodecAdapter` structural protocol:

- `descriptor()` returns identities, capabilities, and profiles without
  loading media or a model;
- `inspect()` and `validate_profile()` produce a receipt bound to the retained
  cartridge and exact signal/tensor/decoded ABI;
- `load()` accepts only the selected device and hash-bound external assets;
- `open_source()` and `read_slot()` use retained bounded cartridge access;
- `decode_slot()` returns bounded decoded batches;
- `reset_decoder()` obeys explicit newer stream generations;
- `create_capture_writer()` stages bounded post-operator latent output below a
  host-owned directory;
- optional raw-import methods preflight and stage source data without creating
  or importing the final `.lc` themselves.

Core revalidates profile receipts, owns worker launch and shared-memory
transport, constructs the final cartridge, and imports it into Library. An
adapter must not reopen arbitrary paths, download assets, weaken LC validation,
write directly to Library, or choose a different profile/device/model after
admission.

## Protocol 2 behavior

Worker Protocol 2 carries authenticated bounded control messages. Latent and
RGBA data use retained files or shared memory rather than the control stream.
Implement bootstrap, capability/ABI receipts, load/open, transport, reset,
decode, capture, abort, and deterministic replay exactly as specified.

A Protocol 2 failure is terminal for that session. Do not retry through
Protocol 1, another installed pack, CPU, another asset, or hidden conversion.

## Build and inspect

First run the synthetic adapter's CPU contract test. Assemble the declared
isolated runtime only when package lifecycle testing is needed; keep generated
runtime bytes untracked. On Windows, copying `python.exe` alone is not a valid
portable CPython runtime: include its adjacent versioned runtime DLL
(`python313.dll` for the supported CPython 3.13 runtime) and the standard
library/runtime files declared by your lock.

```powershell
uv run --no-sync pytest codec-host/python/tests/test_public_synthetic_codec.py
cargo run -p latentdeck-extension-manager -- build --source synthetic-codec --output synthetic-codec.ldcodec
cargo run -p latentdeck-extension-manager -- inspect --archive synthetic-codec.ldcodec
```

`build` stages the source, generates and binds a sorted integrity catalog, runs
the embedded public Draft 2020-12 Codec and integrity schemas, then applies the
normative Rust package parser and semantic validation. It writes a
deterministic archive, reinspects the result, and refuses overwrite. The
source tree is not modified.

The build-time schema evaluator is offline. The developer onboarding check
independently validates the public examples with the published schema files,
then runs the normative package-parser checks. This detects drift between the
published and embedded schemas.

Use the lifecycle sequence in [Deck authoring](DECKS.md#test-the-lifecycle) with
`--kind codec` and the Codec Pack identity. Test the compatibility matrix
against both a compatible starter Deck and deliberate mismatches.

## External assets and licenses

A package declaration identifies accepted asset bytes; it does not bundle,
download, copy, license, or silently select them. The user selects a local file
and the host checks its exact identity. Never store a machine path in a package,
cartridge, preset, diagnostic, or public document.

If a runtime dependency or native library is redistributed, include its source,
exact version, license, purpose, and notices in the package SBOM/notices. Model
weights and generator-side components remain external even when their license
would permit redistribution.

## Conformance checklist

- Descriptor and package identities match exactly.
- Profile inspection/validation occurs before codec-owned GPU allocation.
- Every tensor and decoded frame satisfies the declared ABI and bounds.
- Capture is atomic, abortable, no-clobber, and cannot escape the host staging
  root.
- Reset, loop, end-of-file, crash, timeout, and cancellation behavior is
  deterministic and observable.
- Runtime environment clearing and imports do not depend on ambient packages,
  credentials, network access, or user files.
- The full package lifecycle and compatible/incompatible matrix pass.
- Tests use synthetic data and run without private payloads; hardware-specific
  evidence is recorded separately and never generalized.
