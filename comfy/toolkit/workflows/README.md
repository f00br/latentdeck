# LatentDeck Toolkit example workflows

This directory contains one node gallery and eight public, data-free ComfyUI
workflow examples. They contain no cartridge, latent payload, model weight,
prompt, generated output, private path, or credential. Open a graph with
ComfyUI's **Workflow → Open** command or drag its JSON file onto the canvas.

Before queueing a graph:

1. use the LC Load or Raw Import **Upload** button to copy a selected file into
   Comfy's input area, or select a file already present there;
2. select the external FAST and/or native HQ H3 VAE where the graph asks for
   one;
3. replace `REPLACE_WITH_LOWERCASE_SHA256` and the source/license placeholders
   with the identity of that exact external asset;
4. keep output names relative; LC Save resolves below
   `output/latentdeck/cartridges`, and Research Report resolves its relative
   session directory below `output/latentdeck/reports`;
5. run the compatibility branch before judging an operator result.
6. inspect every **Explicit Device Transfer** node: choose one device for all
   synthesis inputs, and change `FALLBACK_TO_CPU` to `ERROR` when a CUDA-only
   benchmark or acceptance result is required.

`06_RAW_RECORD_INSPECT.json` also uses the separately packaged Comfy LC
Recorder. Install its self-contained Windows bundle by following the
[Recorder section of the Windows guide](../../../docs/guides/WINDOWS_INSTALL.md#install-the-comfy-lc-recorder),
then restart ComfyUI before opening that graph. Installing the Toolkit alone
does not provide **Save Latent Cartridge (.lc)**.

The examples never download assets and never hide crop, resize, dtype
conversion, or re-encoding. The native H3 VAE and taeh3 weights are external
inputs and are not part of this repository.

H3 visual temporal lengths use the exact codec contract `T = 2 + 5n`. The
Compatibility Checker reports that state per input. Explicit Crop and Temporal
Lab reject an invalid requested/post-loop `T` instead of rounding, padding, or
truncating it; choose a valid value directly in the visible node controls.

The standalone realtime Deck and this full-clip laboratory deliberately differ
on clip length. D2/Q4 Deck slots have independent cyclic playheads, so source
cartridges may have different valid `T`/durations when the shared spatial,
runtime, codec, and timing contracts match. Toolkit operators receive complete
tensors in one graph execution and therefore require the same `T`; use the
visible Explicit Pair Align/Crop node to make that full-clip choice explicit.

## Included graphs

- [`00_ALL_NODES_GALLERY.json`](00_ALL_NODES_GALLERY.json) is the inventory
  entry point. It lays out every public Toolkit node so a developer can inspect
  names, inputs, outputs, defaults, and visible control boundaries before
  opening a task-specific workflow.
- [`01_LC_INSPECT.json`](01_LC_INSPECT.json) loads and validates one `.lc`,
  previews its manifest/compatibility receipt, and runs latent scopes for
  mean/std, extrema, finite checks, and channel/temporal energy.
- [`02_FAST_HQ_COMPARE.json`](02_FAST_HQ_COMPARE.json) decodes the exact same
  visual latent through explicitly declared FAST and HQ VAE assets, displays
  both images, and previews numeric comparison metrics.
- [`03_DUAL_SYNTH_LAB.json`](03_DUAL_SYNTH_LAB.json) loads a carrier and donor,
  moves both through visible CUDA:0 transfer nodes (with explicit CPU fallback),
  executes the compatibility checker, runs the Dual Mixer Lab with XS5 in
  `HYBRIDIZE` mode, and performs FAST decode.
- [`04_QUAD_CARRIER_DONORS.json`](04_QUAD_CARRIER_DONORS.json) loads one carrier
  plus three donors, moves all four through visible device-transfer nodes,
  previews four-way compatibility, fixes donor order with the Carrier / Donor
  Router, feeds its reordered normalized B/C/D weights into the Quad Mixer Lab,
  runs deterministic XS5, and FAST decodes the result. The four files may be
  duplicate development inputs, but
  duplicate content is not evidence of independent four-source diversity.
- [`05_PROJECT_RESAMPLE.json`](05_PROJECT_RESAMPLE.json) performs the explicit
  offline native-H3 decode→encode projection, compares RAW and PROJECTED through
  the same FAST/HQ pair, then writes the projected latent as a new `.lc`.
  Source cartridges, the projector identity/controls, and the audio policy are
  collected automatically from the latent flowing through the graph.
- [`06_RAW_RECORD_INSPECT.json`](06_RAW_RECORD_INSPECT.json) selects/uploads an
  old H3 Safetensors file, passes it through the official lightweight
  `Save Latent Cartridge (.lc)` Recorder, and shows both import inspection and
  post-Recorder latent scopes. The Recorder writes below
  `output/latentdeck/cartridges`.
- [`07_EXPLICIT_ALIGN_CROP.json`](07_EXPLICIT_ALIGN_CROP.json) is the master
  mixed-geometry path: two cartridges enter an explicitly labelled center/end
  crop, compatibility is rechecked, a Linear mix is produced, and the
  provenance-bearing result is saved. The graph visibly uses
  `DROP_EXPLICIT` for audio; it performs no hidden resize or re-encode.
- [`99_OPERATOR_DEVELOPER_TEMPLATE.json`](99_OPERATOR_DEVELOPER_TEMPLATE.json)
  supplies the smallest developer harness: one carrier and one donor each pass
  through an explicit CUDA:0/CPU-policy transfer node before the operator hook,
  benchmark, determinism test, full-vs-chunk streaming test, and JSON/Markdown
  report exporter that reads the accumulated graph ledger.

## Explicit device staging contract

LC Load and Raw H3 Import produce bounded CPU tensors. Toolkit operators never
move them implicitly. **LatentDeck Explicit Device Transfer — CPU / CUDA** is a
visible graph decision that transfers both H3 visual and optional audio streams,
preserves shape/dtype, normalizes dense contiguity, and returns a JSON receipt.

`target=CUDA` takes a zero-based `cuda_index`. `ERROR` is the safe default when
CUDA is absent. `FALLBACK_TO_CPU` is a visible opt-in used only when no CUDA
device exists; an invalid index, CUDA query failure, transfer/allocation error,
or input above the 512 MiB node bound remains an error. The supplied 03/04/99
graphs select CUDA:0 plus explicit CPU fallback so they remain runnable for
functional inspection on CPU-only hosts. A fallback run is not CUDA benchmark
evidence: confirm `device: cuda` and non-null VRAM fields in the Benchmark
receipt, or set every transfer policy to `ERROR` before queueing.

### Replace the built-in hook with an installed external operator

`99_OPERATOR_DEVELOPER_TEMPLATE.json` deliberately ships with
`LatentDeckToolkitDualOperatorHook`, so the graph remains loadable when only the
Toolkit is installed. To evaluate separately installed trusted code:

1. explicitly install the external operator package or copy
   [`MyLatentOperator.py`](../templates/MyLatentOperator.py) into ComfyUI's
   `custom_nodes` directory, then restart ComfyUI;
2. delete `LatentDeckToolkitDualOperatorHook` from the graph;
3. add that module's topology-specific hook-builder node. For the one-file
   template, add `MyLatentOperatorTestHook`; for the packaged Channel Roll
   example, add `LatentDeckExampleChannelRollHook`. Connect the donor plus its
   visible controls;
4. connect the hook builder's `LATENTDECK_OPERATOR_HOOK` output to **Operator
   Benchmark**, **Determinism Test**, and **Streaming Compatibility Test**;
5. keep the carrier connected directly to those three evaluation nodes. A
   `single_source` hook captures no additional latent, a `dual_source` hook
   captures one donor, and a `carrier_donors` hook exposes every donor as a
   fixed, ordered Comfy input.

Do not paste an entrypoint string into the graph or add a dynamic loader. The
external module imports its own trusted implementation during explicit
installation and gives the Toolkit an already constructed hook value. Loading
a cartridge never installs or imports operator code.

## Expected execution behavior

`PreviewAny` is a ComfyUI core utility used only to expose JSON receipts on the
canvas. `PreviewImage` is used for decoded images. A graph can open without the
external VAE assets, but decode/projector execution correctly stops until the
user selects them and supplies truthful provenance.

LC inputs are treated as untrusted media by the shared Rust Cartridge SDK.
Toolkit operators receive already validated H3 tensors; a workflow does not
grant a cartridge permission to import or install Python code.

The graph ledger is path-free and bounded. LC loaders add content identity,
operators append their declared version/seed/controls, diagnostics append their
measurements, and LC Save appends the new cartridge hash. Users do not retype
parent or operation JSON in the supplied workflows.

For a custom implementation, continue with the
[operator developer template](../docs/OPERATOR_DEVELOPER_TEMPLATE.md).
