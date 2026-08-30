# LatentDeck Comfy Toolkit 0.1

LatentDeck Comfy Toolkit is the public clean-room research surface for latent
operators inside ComfyUI. It is intentionally separate from the lightweight
`ComfyUI-LatentCartridge` recorder and from the standalone realtime worker.

The package contains no model weights, decoder assets, cartridges, workflows,
prompts, generated media, or private laboratory code. It does not include
InteractionNet or any checkpoint.

## Nodes

The Toolkit exports seven ComfyUI nodes:

- `LatentDeck XS1 — Channel Rotation`
- `LatentDeck XS2 — Grid Exchange`
- `LatentDeck XS3 — Temporal Interaction`
- `LatentDeck XS4 — Statistics Transfer`
- `LatentDeck XS5 — Affinity Transport`
- `LatentDeck Compare FAST / HQ Hooks`
- `LatentDeck Projector (Offline CPU)`

The five XS nodes are a stable sequence adapter over the already reviewed
`latentdeck_operator_d2.process_slot` implementation. The Toolkit does not
copy, fork, or reinterpret XS math. It only validates a complete
`[1,24,T,H,W]` sequence, supplies independent per-slot history, invokes the
builtin implementation, and aggregates bounded JSON provenance.

Inputs remain full-grid F16 H3 0.1 tensors. There is no hidden crop, resize,
downscale, temporal conversion, or runtime dtype cast. The public bounds are:

- at most 512 temporal slots;
- at most 4096 spatial tokens per slot;
- at most 50,331,648 values per sequence;
- the exact H3 profile and causal timing contract from LC Profile 0.1.

## FAST / HQ comparison hooks

The comparison API accepts two explicit `DecoderHook` objects. Each object
contains a caller-supplied callable and an optional opaque, caller-owned asset.
The Toolkit never searches for, downloads, imports, or bundles a decoder. If an
asset is provided, a lowercase SHA-256 identity is required for provenance.

External decoder packages can expose Comfy nodes that return the custom
`LATENTDECK_DECODER_HOOK` type. The comparison node invokes both hooks over the
same latent and reports bounded finite MAE, maximum error, RMSE, and PSNR when
defined. FAST and HQ outputs must have identical shape and device; the Comfy
node additionally requires standard `[N,H,W,C]` IMAGE layout with one, three,
or four channels. Each hook receives an independent contiguous clone, so an
in-place implementation cannot mutate the workflow latent or affect the other
hook.

Each decoded output is bounded to 402,653,184 F16, BF16, or F32 values. This
covers the release profile's 243-frame 448×800 case with up to four channels.
Metrics are accumulated in F64 chunks of at most 1,048,576 values rather than
materializing full-size F64 copies of both decoded sequences.

## Offline Projector

`project_offline` is a deterministic centered full-SVD reconstruction. It is
CPU-only, accepts F16 or F32 `[1,24,T,H,W]`, preserves shape and storage dtype,
and is bounded to 262,144 latent tokens. The Comfy node makes CPU staging
explicit in its name and provenance and returns a CPU latent.

The Projector is an offline research processor. It is deliberately absent from
the standalone realtime codec worker and its provenance always records
`realtime_eligible: false`.

## Explicit external operators

The Toolkit includes a versioned explicit-install registry for separately
distributed research operators. Installation requires application code to pass
both a closed descriptor and an already imported callable:

```python
from latentdeck_comfy_toolkit import TrustedOperatorRegistry
from latentdeck_example_channel_roll import install_into

registry = TrustedOperatorRegistry()
install_into(registry)  # explicit owner/host action
operator = registry.load("org.latentdeck.example.channel_roll", "0.1.0")
```

The registry never imports the descriptor's entrypoint string, discovers
packages, fetches URLs, or reads code from a cartridge. Importing the example
package alone does not install it. An explicitly installed operator is trusted
native Python code, not a sandboxed plugin, so users must review its source and
distribution before making that call.

The normative contract is documented in the
[Operator API 0.1](../../spec/operator-api/README.md). The separate example is
under [`operators/examples/channel-roll`](../../operators/examples/channel-roll/README.md).

## Installation

The wheel installs the Python library and its pinned dependencies; it does not
modify a ComfyUI installation or auto-register custom nodes. Install the
Toolkit wheel into the same Python environment that starts ComfyUI. Then place
this `comfy/toolkit` directory in ComfyUI's `custom_nodes` directory so its
top-level discovery shim is loaded, and restart ComfyUI.

The separately packaged `latentdeck-operator-d2` dependency must be installed
in that same environment. A repository workspace install resolves it from the
uv workspace; a standalone install must provide both compatible wheels. No
weight or decoder package is installed by this process.

After restart, verify that the seven nodes listed above appear under
`LatentDeck / Toolkit`. Installing the optional Channel Roll example wheel does
not register it automatically; a trusted host still has to call its explicit
`install_into(registry)` function.

## Local checks

From the repository root:

```powershell
uv run --no-sync pytest comfy/toolkit/tests
uv run --no-sync ruff check comfy/toolkit
```

All tests use finite synthetic tensors created in memory. No media fixture or
decoder asset is part of the test corpus.
