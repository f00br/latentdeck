# Develop with Latent Cartridges

Use this guide to read, validate, create, or derive `.lc` media without
reimplementing the format. The [LC 0.1 specification](../../spec/latent-cartridge/README.md)
and [manifest schema](../../spec/latent-cartridge/manifest.schema.json) are
normative.

## Choose the supported surface

- Use the **Rust crate and CLI** for native tooling and the complete retained
  validation/read/write surface.
- Use **`latentdeck-cartridge` for Python** when integrating recorders,
  converters, or research tools. It binds the same Rust implementation and has
  no separate parser.
- Use **LatentPlayer PREPARE** for the normal interactive raw-import workflow.
- Use **ComfyUI-LatentCartridge** to save a generated H3 latent before decode.

An `.lc` is data-only media. Never place source code, an operator, a model
weight, a URL-fetch instruction, or a second payload in it.

## Inspect, validate, and hash

Structural inspection is bounded but does not grant tensor access:

```powershell
cargo run -p latentdeck-cartridge -- inspect example.lc
```

Full validation verifies archive structure, canonical manifest, hashes,
Safetensors layout, finite values, H3 profile semantics, and resource limits:

```powershell
cargo run -p latentdeck-cartridge -- validate example.lc
cargo run -p latentdeck-cartridge -- hash example.lc
```

Do not label a structural inspection as full validation. Runtime tensor access
must remain attached to the validated file handle rather than reopening an
untrusted path later.

The Python binding exposes the same operations:

```python
import latentdeck_cartridge as cartridge

inspection = cartridge.inspect("example.lc")
validation = cartridge.validate("example.lc")
digest = cartridge.hash("example.lc")
```

## Pack raw H3 data

For an already existing supported H3 Safetensors file:

```powershell
latentdeck-pack input.safetensors --profile h3 -o output.lc
latentdeck-convert input.safetensors --output output.lc
```

The SDK derives tensor layout, cadence, decoded geometry, dtype, audio
disposition, and payload hash. It copies the payload bytes without crop,
resize, cast, or re-encode, writes through a same-directory partial file,
validates the complete result, and refuses an existing destination by default.

From Python:

```python
import latentdeck_cartridge as cartridge

receipt = cartridge.pack_raw_h3(
    "input.safetensors",
    "output.lc",
    provenance={
        "created_by": {"name": "my-authoring-tool", "version": "0.1.0"},
        "source_kind": "raw_h3_safetensors",
        "source_metadata": {"workflow_sha256": "..."},
    },
)
```

Store hashes or intentionally public bounded metadata, not absolute paths,
credentials, private prompts, or complete workflows.

## Derive A.lc into B.lc

A derived cartridge must receive a new cartridge UUID and record exact parent
archive identity plus the transformation. Its operation history includes the
operator ID/version, normalized controls, deterministic seed, and explicit
audio disposition. Those records remain descriptive and never authorize code
execution.

The [cartridge genealogy example](../../examples/cartridge-genealogy/README.md)
generates synthetic input, performs a deterministic transform, and writes a
new validated cartridge. It demonstrates:

- retained validation of the parent;
- a new UUID rather than reusing the source identity;
- parent UUID and complete archive SHA-256;
- bounded operation controls and JSON-safe seed;
- copied, preserved, absent, or explicitly omitted audio according to the
  profile contract;
- no-clobber atomic finalization and post-write validation.

Use the Cartridge SDK's authoring/resample helpers rather than manually editing
`manifest.json` inside a ZIP. A complete-cartridge hash cannot be placed inside
its own manifest because that would be circular.

## Comfy Toolkit LC I/O

The Toolkit package root exports these supported 0.1 helpers:

- `load_lc`
- `import_raw_h3`
- `parent_cartridge_ref`
- `save_resampled_lc`

They carry the bounded metadata ledger used by Load, operators, LC Save, and
research reports. They do not weaken the Rust validation path or accept an
arbitrary workflow-supplied filesystem path.

## Compatibility and failure behavior

The codec profile—not the `.lc` extension alone—defines tensor names, layout,
timing, decoded geometry, and synthesis compatibility. Never infer compatibility
from orientation, filename, approximate aspect ratio, or codec family alone.

Public errors have a stable code and path-safe detail. Preserve those codes in
new language bindings and tools. A failed write must leave no final target or
orphaned partial file.

## Checks

```powershell
cargo test -p latentdeck-cartridge --all-features
uv run pytest sdk/python/tests comfy/latent-cartridge/tests
uv run ruff check sdk/python comfy/latent-cartridge
```

Tests should generate bounded synthetic payloads. Real cartridges and raw
latents remain outside the source repository.
