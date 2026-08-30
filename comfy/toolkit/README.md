# LatentDeck Comfy Toolkit 0.1

LatentDeck Comfy Toolkit is the public clean-room laboratory for inspecting H3
latent media, developing latent operators, comparing decoders, and writing
manipulated latent state back to `.lc`. It is separate from the lightweight
`ComfyUI-LatentCartridge` recorder and from the standalone realtime Deck.

The package contains no model weight, decoder asset, cartridge, raw latent,
prompt, generated media, or private laboratory workflow. The six included
[example workflows](workflows/README.md) are original, sanitized, data-free
graphs. InteractionNet, MutantNet, prompt conditioning, audio processing,
MIDI/OSC, Spout, a timeline, and an H3 generation pipeline are not included.

## Node inventory

The canonical Comfy registry exports 32 nodes under `LatentDeck / Toolkit`.

### Cartridge I/O and compatibility

- **LC Load / Inspect** — strict `.lc` validation plus manifest, codec/profile,
  tensor schema, timing, provenance, hash, and compatibility receipt.
- **Raw H3 Latent Import** — reads a validated legacy H3 visual/AV Safetensors
  directly into the lab without requiring a prior `.lc` conversion.
- **LC Save / Resample** — writes post-operator latent state as a validated new
  cartridge with parent cartridges, operation history, and explicit audio
  disposition derived from the bounded metadata ledger carried by the graph.
- **Compatibility Checker** — reports exact codec/profile, geometry, temporal,
  and timing disagreements before synthesis, including whether every H3 visual
  length satisfies the codec-valid `T = 2 + 5n` contract.
- **Explicit H3 Crop** and **Explicit H3 Pair Align** — visible, user-selected
  temporal/spatial policies. A requested output `T` that does not satisfy
  `T = 2 + 5n` fails visibly; it is never rounded or otherwise adjusted. There
  is no implicit resize or re-encode.

All cartridge bytes are read and written through the shared Rust Cartridge SDK
binding. The Toolkit does not implement a second ZIP/Safetensors trust path.
Audio payloads can be preserved as opaque cartridge data when the declared
policy permits it, but Toolkit 0.1 does not play or synthesize audio.

### XS operators and mixer labs

- **XS1 — Channel Mixer** uses explicit per-channel cross-synthesis weights.
- **XS2 — Spatial Latent Graft** applies a visible mask on the latent grid.
- **XS3 — Frequency Cross-Synthesis** performs spatial FFT-band exchange.
- **XS4 — Statistics Transfer** transfers bounded mean/std statistics.
- **XS5 — Affinity / Sinkhorn Transport** provides `HYBRIDIZE` and `INTERACT`
  with TOPK/Sinkhorn routing, bounded parameters, and deterministic seed.
- **Dual Mixer Lab** provides `carrier + donor → operator` for Linear/XS1–XS5.
- **Carrier / Donor Router** fixes one carrier, three donor weights, and a
  deterministic B/C/D processing order.
- **Quad Mixer Lab** provides the 0.1 `carrier + 3 donors` topology, manual or
  triangular donor-weight macro, and Linear/XS5 processing.

Every synthesis path accepts complete finite H3 grids. It never silently
downscales, drops a donor, crops a stream, or changes algorithm for speed.

### Research labs and evaluation

- **Temporal Lab** — explicit offset, reverse, loop, and crop operations. The
  exact post-loop output must satisfy `T = 2 + 5n`; invalid crop/loop
  combinations fail before any temporal transform and are never silently
  changed.
- **Feedback Lab** — bounded safe feedback variants; no unbounded recursive
  graph or hidden persistent state.
- **Channel Lab** — explicit 24×24 rotation/matrix operations.
- **Operator Chain Receipt** — aggregates the receipts produced by a visible
  Comfy node chain; the tensor chain itself remains ordinary node connections.
- **Latent Scopes / Diagnostics** — mean/std, min/max, NaN/Inf, channel energy,
  and temporal energy.
- **Dual Operator Test Hook**, **Operator Benchmark**, **Determinism Test**, and
  **Streaming Compatibility Test** — measure execution time, CUDA memory delta,
  shape, repeatability, and full-clip-versus-chunk agreement.
- **One-click Research Report** — validates bounded JSON receipts and writes
  deterministic JSON plus Markdown through `.partial` files followed by atomic
  rename. Duplicate keys, NaN/Inf, unsafe embedded paths, unsafe report names,
  oversized sections, and implicit overwrite are rejected.

The report contains versions, cartridges, operator IDs/parameters,
timing/benchmark/VRAM measurements, and outputs. Receipts expose only output
basenames and content hashes, not the selected machine directory.
Load, operator, evaluation, save, and report nodes exchange this information
automatically; parent-cartridge, operator-history, and report-section JSON are
not manual workflow fields.

### FAST/HQ decode and projection

- **Declare H3 VAE Asset** attaches explicit role, source, license, version,
  and SHA-256 identity to a caller-selected Comfy `VAE` object.
- **FAST Decode** uses the explicitly supplied TAEHV/taeh3-compatible VAE.
- **HQ Decode** uses the explicitly supplied native H3 VAE.
- **FAST vs HQ Comparator** decodes the same visual latent through both and
  reports bounded MAE, maximum error, RMSE, and PSNR when defined.
- **Manifold Projector — Native H3** performs the explicit offline research path
  `native H3 decode → native H3 encode → projected latent`.
- **RAW vs PROJECTED Comparator** sends both latent states through the exact
  same FAST/HQ pair.
- **Compare FAST/HQ Hooks** remains the lower-level caller-supplied decoder-hook
  surface.
- **PCA Diagnostic (Offline CPU)** remains a clearly named diagnostic; it is
  not presented as the native H3 manifold projector.

The Toolkit never discovers, downloads, or bundles a VAE. The caller selects
the exact external assets and supplies truthful provenance. H3 audio is ignored
by decode and is never sent into a visual VAE.

## External operator contract

External operators are separately installed trusted Python packages. A
cartridge cannot import one, carry Python source, choose a package, or mutate
the registry. Every descriptor declares one of `single_source`, `dual_source`,
or `carrier_donors`, its exact input count/order, supported codec/timing
profiles, deterministic and streaming/chunk capabilities, closed controls,
resource limit, and exact bypass state.

Start with the
[MyLatentOperator developer template](docs/OPERATOR_DEVELOPER_TEMPLATE.md), the
normative [Operator API 0.1](../../spec/operator-api/README.md), and the public
[Channel Roll example](../../operators/examples/channel-roll/README.md).

## Example workflows

The [workflow guide](workflows/README.md) covers:

- `01_LC_INSPECT.json`;
- `02_FAST_HQ_COMPARE.json`;
- `03_DUAL_SYNTH_LAB.json`;
- `04_QUAD_CARRIER_DONORS.json`;
- `05_PROJECT_RESAMPLE.json`;
- `99_OPERATOR_DEVELOPER_TEMPLATE.json`.

The graphs use relative placeholders only. Replace cartridge inputs and
external VAE identity fields before queueing. No test cartridge or golden H3
payload is distributed; users connect their own compatible `.lc` files.

## Installation

Install the Toolkit wheel and its pinned dependencies into the same Python
environment that launches ComfyUI. Then place this `comfy/toolkit` directory in
ComfyUI's `custom_nodes` directory so the top-level discovery shim is loaded,
and restart ComfyUI.

The workspace resolves the shared `latentdeck-cartridge`,
`latentdeck-operator-d2`, and `latentdeck-operator-q4` distributions. A
standalone installation must provide compatible wheels for all four packages.
No weight or decoder package is installed by this process.

After restart, confirm that **LatentDeck LC Load / Inspect**, **LatentDeck XS5
— Affinity / Sinkhorn Transport**, and **LatentDeck One-click Research Report**
appear. Open `01_LC_INSPECT.json` first to verify the local node installation
before selecting any external VAE.

## Local checks

From the repository root:

```powershell
uv run --no-sync pytest comfy/toolkit/tests
uv run --no-sync ruff check comfy/toolkit
```

Tests create finite synthetic tensors and temporary outputs in memory or the
test temporary directory. They do not use private media, a model asset, or a
real cartridge fixture.
