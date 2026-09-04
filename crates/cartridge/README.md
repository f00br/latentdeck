# Latent Cartridge Rust SDK

This package is the single LC 0.1 implementation used by the Rust API, Python
bindings, LatentPlayer preparation workflow, and the latentdeck-cartridge
command-line tool. Every surface delegates to this crate rather than
maintaining a second parser or raw-H3 packer.

New Rust integrations should begin with the deliberately small
`latentdeck_cartridge::sdk` facade. It re-exports the supported manifest,
inspection, validation, hashing, atomic authoring, and genealogy-aware
resampling types without requiring callers to navigate internal module
boundaries. Specialized low-level modules remain available, but are not the
recommended discovery surface.

Implemented boundaries:

- deterministic, STORE-only canonical ZIP64 writing;
- bounded archive, manifest, WebP, and H3 Safetensors inspection;
- streaming CRC-32, SHA-256, and F16/F32 finite-value validation;
- strict LC manifest and H3 profile validation;
- validated-handle-only tensor readers;
- shared bounded raw-H3 inspection and authoring through
  `inspect_raw_h3` / `pack_raw_h3_atomic`;
- same-directory partial writing, post-write validation, and atomic commit.

`RawH3AuthoringOptions` supplies the producer identity and optional bounded
provenance or WebP preview. Raw H3 authoring derives the H3 manifest, cadence,
geometry, dtype, payload hash, and deterministic cartridge id in Rust. It does
not crop, cast, resize, re-encode, or require a playback codec/GPU. Replacement
is forbidden unless the caller opts in explicitly. Interactive planners can
also supply `expected_payload_sha256`; authoring then refuses a raw source that
no longer matches the payload identity approved during preflight.

Normative behavior is defined by the
[LC 0.1 specification](../../spec/latent-cartridge/README.md), its
[manifest schema](../../spec/latent-cartridge/manifest.schema.json), and the
[H3 profile](../../spec/codec-h3/README.md).

## CLI

The CLI is the developer and automation surface. End users preparing local H3
performance files should use LatentPlayer's **PREPARE** workspace.

Every successful command writes structured JSON to standard output. Validation
failures write a stable error code and location as JSON to standard error.

    latentdeck-cartridge pack --manifest manifest.json --payload input.safetensors --output output.lc
    latentdeck-cartridge inspect output.lc
    latentdeck-cartridge validate output.lc
    latentdeck-cartridge hash output.lc

The inspect command validates bounded structure and metadata but never grants
tensor access. The validate command additionally streams every entry and
tensor value, verifies hashes, and returns full validation evidence.

All public tests synthesize temporary data at runtime. No cartridge, latent,
weight, media, workflow, or private fixture belongs in this crate.
