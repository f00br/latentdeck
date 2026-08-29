# Latent Cartridge Rust SDK

This package is the single LC 0.1 implementation used by the Rust API and the
latentdeck-cartridge command-line tool. Future Python bindings call this crate
rather than maintaining a second parser.

Implemented boundaries:

- deterministic, STORE-only canonical ZIP64 writing;
- bounded archive, manifest, WebP, and H3 Safetensors inspection;
- streaming CRC-32, SHA-256, and F16/F32 finite-value validation;
- strict LC manifest and H3 profile validation;
- validated-handle-only tensor readers;
- same-directory partial writing, post-write validation, and atomic commit.

Normative behavior is defined by the
[LC 0.1 specification](../../spec/latent-cartridge/README.md), its
[manifest schema](../../spec/latent-cartridge/manifest.schema.json), and the
[H3 profile](../../spec/codec-h3/README.md).

## CLI

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
