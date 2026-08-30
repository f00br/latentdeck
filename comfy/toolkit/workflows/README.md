# LatentDeck Toolkit example workflows

These six graphs are public, data-free ComfyUI workflow examples. They contain
no cartridge, latent payload, model weight, prompt, generated output, private
path, or credential. Open a graph with ComfyUI's **Workflow → Open** command or
drag its JSON file onto the canvas.

Before queueing a graph:

1. replace every relative `.lc` placeholder with a cartridge you selected;
2. select the external FAST and/or native HQ H3 VAE where the graph asks for
   one;
3. replace `REPLACE_WITH_LOWERCASE_SHA256` and the source/license placeholders
   with the identity of that exact external asset;
4. keep output names relative, or deliberately select your own output
   directory in the corresponding node;
5. run the compatibility branch before judging an operator result.

The examples never download assets and never hide crop, resize, dtype
conversion, or re-encoding. The native H3 VAE and taeh3 weights are external
inputs and are not part of this repository.

H3 visual temporal lengths use the exact codec contract `T = 2 + 5n`. The
Compatibility Checker reports that state per input. Explicit Crop and Temporal
Lab reject an invalid requested/post-loop `T` instead of rounding, padding, or
truncating it; choose a valid value directly in the visible node controls.

## Included graphs

- [`01_LC_INSPECT.json`](01_LC_INSPECT.json) loads and validates one `.lc`,
  previews its manifest/compatibility receipt, and runs latent scopes for
  mean/std, extrema, finite checks, and channel/temporal energy.
- [`02_FAST_HQ_COMPARE.json`](02_FAST_HQ_COMPARE.json) decodes the exact same
  visual latent through explicitly declared FAST and HQ VAE assets, displays
  both images, and previews numeric comparison metrics.
- [`03_DUAL_SYNTH_LAB.json`](03_DUAL_SYNTH_LAB.json) loads a carrier and donor,
  executes the compatibility checker, runs the Dual Mixer Lab with XS5 in
  `HYBRIDIZE` mode, and performs FAST decode.
- [`04_QUAD_CARRIER_DONORS.json`](04_QUAD_CARRIER_DONORS.json) loads one carrier
  plus three donors, previews four-way compatibility, fixes donor order with
  the Carrier / Donor Router, feeds its reordered normalized B/C/D weights into
  the Quad Mixer Lab, runs deterministic XS5, and FAST decodes the result. The
  four files may be duplicate development inputs, but
  duplicate content is not evidence of independent four-source diversity.
- [`05_PROJECT_RESAMPLE.json`](05_PROJECT_RESAMPLE.json) performs the explicit
  offline native-H3 decode→encode projection, compares RAW and PROJECTED through
  the same FAST/HQ pair, then writes the projected latent as a new `.lc`.
  Source cartridges, the projector identity/controls, and the audio policy are
  collected automatically from the latent flowing through the graph.
- [`99_OPERATOR_DEVELOPER_TEMPLATE.json`](99_OPERATOR_DEVELOPER_TEMPLATE.json)
  supplies the smallest developer harness: one carrier, one donor, an operator
  hook, benchmark, determinism test, full-vs-chunk streaming test, and a
  JSON/Markdown report exporter that reads the accumulated graph ledger.

### Replace the built-in hook with an installed external operator

`99_OPERATOR_DEVELOPER_TEMPLATE.json` deliberately ships with
`LatentDeckToolkitDualOperatorHook`, so the graph remains loadable when only the
Toolkit is installed. To evaluate separately installed trusted code:

1. explicitly install the external operator package as a ComfyUI custom node
   and restart ComfyUI;
2. delete `LatentDeckToolkitDualOperatorHook` from the graph;
3. add that package's topology-specific hook-builder node. For the checked-in
   Channel Roll example, add `LatentDeckExampleChannelRollHook` and connect the
   donor plus its visible controls;
4. connect the hook builder's `LATENTDECK_OPERATOR_HOOK` output to **Operator
   Benchmark**, **Determinism Test**, and **Streaming Compatibility Test**;
5. keep the carrier connected directly to those three evaluation nodes. A
   `single_source` hook captures no additional latent, a `dual_source` hook
   captures one donor, and a `carrier_donors` hook exposes every donor as a
   fixed, ordered Comfy input.

Do not paste an entrypoint string into the graph or add a dynamic loader. The
external package imports its own trusted implementation during explicit
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
