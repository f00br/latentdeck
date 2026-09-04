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

For the Windows preview, download
`LatentDeck-0.1.0-preview.1-comfy-recorder-windows-x64.zip` and its checksum
from the same official GitHub release as the applications. Verify the archive,
extract it, close ComfyUI, and run:

```powershell
$comfyRoot = (Resolve-Path -LiteralPath (Read-Host 'Path to your ComfyUI root')).Path
powershell -ExecutionPolicy Bypass -File .\Install-ComfyRecorder.ps1 -ComfyUIRoot $comfyRoot
```

The self-contained bundle supports Windows x64 CPython 3.12 and 3.13. It
installs the Recorder, `latentdeck-cartridge==0.1.0`, and
`safetensors==0.8.0` from exact hash-bound wheels into the node's private
`vendor` directory. It does not invoke pip, access the network, compile Rust,
or modify the rest of ComfyUI's Python environment. An unsupported Python ABI
fails before the node directory is written. For a nonstandard ComfyUI layout,
pass both `-PythonPath` and `-CustomNodesPath` explicitly.

An existing ComfyUI Safetensors installation remains the first choice and is
not replaced. The bundled 0.8.0 copy is a fallback under the unique
`latentdeck_recorder_vendor.safetensors` namespace, so adding the Recorder's
private dependency directory cannot take over the global `safetensors` import.

Comfy Registry/Manager installation is not enabled for this preview:
`latentdeck-cartridge` is not yet published to PyPI, and the repository will
not pretend that its declared dependency can resolve there. Use the release
bundle. Registry installation remains a separate publication/configuration
gate for a later release.

For development from a source checkout, build the Recorder and Cartridge SDK
wheels as described below and install their declared dependencies into a
disposable environment. Dropping only this source directory into
`custom_nodes` does not supply the native Cartridge SDK.

The node appears under `LatentDeck / Cartridge` as
`Save Latent Cartridge (.lc)`. Recordings go under the dedicated
`latentdeck/cartridges` subdirectory of ComfyUI's output directory.
`filename_prefix` is a sanitized single-file basename inside that directory;
path traversal and Windows device names are not accepted as output paths. Each
recording receives a cartridge UUID, so the node never needs to scan the output
directory for a counter.

For an old raw H3 Safetensors file that was not produced by this Recorder, open
the Toolkit's `06_RAW_RECORD_INSPECT.json`: **Raw H3 Latent Import** validates
the file, then this same official Recorder writes the `.lc`. Renaming a
`.safetensors` file to `.lc` does not create a cartridge.

For standalone or batch conversion without ComfyUI, use the Python SDK's
[`latentdeck-convert`](../../sdk/python/README.md#raw-h3-command) command. It
preserves the source payload bytes, writes the LC manifest and hashes, validates
the result, and never modifies the source file.

## Development tests

From the repository root:

```text
uv run pytest comfy/latent-cartridge/tests
uv run ruff check comfy/latent-cartridge
pwsh -NoProfile -File tools/Test-ComfyRecorderBundle.ps1
```

The node tests use small data-only stubs and do not import ComfyUI, Torch,
model weights, real latents, or private workflows. The bundle contract builds
the real native ABI3 wheel and performs isolated imports with CPython 3.12 and
3.13; it also proves that an unsupported interpreter and a tampered wheel are
rejected before installation.
