# ComfyUI-LatentCartridge

`Save Latent Cartridge (.lc)` is the small authoring node for recording an
already generated MiniMax H3 latent before decode. It does not generate,
decode, resize, crop, or reinterpret latent data, and it does not include the
larger LatentDeck Comfy Toolkit.

## v0.1 behavior

- Input and passthrough output use ComfyUI's `LATENT` type. The exact input
  object is returned, including unrelated mapping entries.
- A visual H3 latent is read from `latent["samples"]`.
- An H3 AV latent uses ComfyUI's `NestedTensor` convention: `samples.unbind()`
  yields visual first and optional audio second.
- Tensor names, layouts, dtypes, decoded geometry, frame count, duration, and
  audio cadence are inferred and validated. They are not manual node fields.
- The source tensors are written to a size-bounded temporary Safetensors file.
  The installed `latentdeck-cartridge` Python SDK derives the manifest and
  timing in Rust, performs authoritative finite-data and LC/profile
  validation, and atomically packs with no clobber.
- A failure always removes the Recorder-owned temporary payload. The native
  SDK owns its unique `.partial` output and atomic commit; the Recorder never
  deletes a final path that could belong to another process. Existing
  cartridges are never overwritten.
- Workflow content is not embedded. When ComfyUI supplies its prompt graph,
  only a SHA-256 digest is recorded as optional provenance.

H3 Profile 0.1 accepts visual `[1, 24, T, H, W]` and optional audio
`[1, 32, 2, T_audio]` tensors stored as F16 or F32. Audio remains preservation
metadata/payload only; LatentDeck 0.1 does not play or synthesize audio.

## Installation

Place this directory in ComfyUI's `custom_nodes` directory. In the same Python
environment, install the LatentDeck Cartridge SDK wheel and the pinned
Safetensors dependency declared in `pyproject.toml`, then restart ComfyUI.

The node appears under `LatentDeck / Cartridge` as
`Save Latent Cartridge (.lc)`. Recordings go under the dedicated
`latentdeck/cartridges` subdirectory of ComfyUI's output directory.
`filename_prefix` is a sanitized single-file basename inside that directory;
path traversal and Windows device names are not accepted as output paths. Each
recording receives a cartridge UUID, so the node never needs to scan the output
directory for a counter.

## Development tests

From the repository root:

```text
uv run pytest comfy/latent-cartridge/tests
uv run ruff check comfy/latent-cartridge
```

The tests use small data-only stubs and do not import ComfyUI, Torch, model
weights, real latents, or private workflows.
