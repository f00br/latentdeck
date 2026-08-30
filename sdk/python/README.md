# LatentDeck Cartridge Python SDK

This package is the Python surface for the single Rust LC 0.1 implementation.
It does not contain a second cartridge parser, codec runtime, model weights, or
PyTorch dependency.

## Build and install

From the repository root, install the locked development environment and build
a release wheel:

```text
uv sync --locked
uv build --wheel sdk/python
```

Install the wheel emitted under the ignored `dist/` directory with
`uv pip install path/to/latentdeck_cartridge-0.1.0-*.whl`.

## Python API

```python
import latentdeck_cartridge as cartridge

receipt = cartridge.pack_raw_h3(
    "input.safetensors",
    "clip.lc",
    provenance={
        "created_by": {"name": "my-recorder", "version": "0.1.0"},
        "created_at": "2026-08-30T08:00:00Z",
        "source_kind": "raw_h3_safetensors",
        "source_metadata": {"workflow_sha256": "..."},
    },
)
inspection = cartridge.inspect("clip.lc")
validation = cartridge.validate("clip.lc")
digest = cartridge.hash("clip.lc")
```

`pack_raw_h3` derives the H3 tensor, cadence, geometry, dtype, payload hash,
and manifest fields in Rust. With no explicit `cartridge_id`, it derives a
canonical deterministic UUIDv8 from the payload SHA-256. The optional
provenance object has a strict schema; its metadata remains subject to LC
manifest size, depth, string, and number limits.

The lower-level `pack(manifest, payload_path, output_path, preview_path=None)`
surface is available for callers that already have a complete LC 0.1 manifest.
All pack operations forbid replacement by default; pass `overwrite=True`
explicitly to replace an existing `.lc` atomically.

## Raw H3 command

```text
latentdeck-pack input.safetensors --profile h3 -o clip.lc
```

The packer preserves the Safetensors payload bytes exactly and validates the
final archive before commit. It does not crop, cast, resize, or re-encode the
payload, and it never adds raw prompts or model weights. Recorders should store
only hashes such as `prompt_sha256` and `workflow_sha256` in provenance.

For existing raw H3 AV collections, use the dedicated converter. Renaming a
`.safetensors` file to `.lc` does **not** create a cartridge: `.lc` is a
validated deterministic ZIP64 container with a manifest and payload hashes.

```text
latentdeck-convert old-av-latent.safetensors --output converted.lc
latentdeck-convert folder-with-latents --output-directory converted
latentdeck-convert folder-with-latents --recursive --output-directory converted
```

`--output` names one exact `.lc` file and is valid only for one explicitly
named `.safetensors` input. `-o` / `--output-directory` always names a
directory and preserves each source basename (and relative subdirectories for
`--recursive`).

The converter recognises the H3 visual/optional-audio tensor schema through the
same Rust validator, preserves the original Safetensors payload bytes exactly,
preflights output collisions, writes atomically, and validates every resulting
cartridge. It never edits or deletes the source files.
