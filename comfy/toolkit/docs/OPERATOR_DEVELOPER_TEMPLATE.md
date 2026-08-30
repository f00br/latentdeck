# MyLatentOperator developer template

The canonical beginner surface is the standalone
[`MyLatentOperator.py`](../templates/MyLatentOperator.py) file. Install the
Toolkit first, copy that one file into ComfyUI's `custom_nodes` directory,
change every `AUTHOR EDIT` value plus `process_sources()`, and restart ComfyUI.
The copied file immediately exposes a normal process node and a test-hook node;
no package scaffold, generated shim, or dynamic entrypoint loader is required.

Copying executable Python into `custom_nodes` is the explicit installation and
trust decision. The template registers only its already imported callable with
`TrustedOperatorRegistry`; a cartridge cannot select, import, or install the
file. Give every copied operator a unique stable ID, entrypoint identity, node
mapping keys, and display names. Do not put operator code inside a cartridge.

The template keeps descriptor, PyTorch callable, Comfy wrappers, discovery
mappings, and evaluation hook in one file. Most authors only edit its small
descriptor block and replace the marked two-line blend with roughly 30-50
lines of bounded PyTorch. The separately packaged
[`channel-roll`](../../../operators/examples/channel-roll/README.md) example is
the next reference when an operator needs its own wheel, package tests, and
license metadata.

## Choose one topology

- `single_source` receives `(source,)` and declares `input_count: 1`.
- `dual_source` receives `(carrier, donor)` and declares `input_count: 2`.
- `carrier_donors` receives `(carrier, donor_1, donor_2, ...)` in stable order
  and declares the exact `input_count` from 2 through 16.

`MyLatentOperator.py` starts as `dual_source`. To change topology, update the
descriptor and all visible Comfy inputs together:

- for `single_source`, use `input_count: 1`, unpack `(source,)`, remove the
  donor widgets, and pass `captured_sources=()` to the hook builder;
- for `dual_source`, keep the supplied `(carrier, donor)` order and
  `captured_sources=(donor,)`;
- for `carrier_donors`, declare the exact count, add one fixed Comfy input per
  donor, unpack them in that same order, and pass the identical ordered donor
  tuple to `captured_sources`.

Never accept a comma-separated path, module name, or variable hidden input as
a source list. Source count and order are part of the closed descriptor and
must remain visible in the graph.

The common callable is intentionally small:

```python
@torch.inference_mode()
def process_sources(sources, controls, context):
    carrier, donor = sources
    amount = float(controls["amount"])
    output = torch.lerp(carrier.float(), donor.float(), amount)
    return ToolkitOperatorResult(
        output=output.to(torch.float16).contiguous(),
        provenance={
            "operation": {
                "operator_id": "org.example.my-latent-operator",
                "operator_version": "0.1.0",
                "seed": context.seed,
                "controls": controls,
            }
        },
    )
```

This illustrates the callable shape, not a complete descriptor. A public
operator must also declare and test all of the following:

- exact `supported_profiles`, including codec, profile, timing, layout, and
  runtime dtype;
- exact input topology/count and stable source order;
- `full_clip`, `streaming`, and `chunk` capabilities;
- whether the operation is `deterministic` for identical source/control/context
  inputs;
- a closed control schema with bounded enum, float, or integer values;
- an exact `zero/bypass` state and the source that must be cloned unchanged;
- a finite full-grid F16 `[1,24,1,H,W]` result with unchanged shape/device;
- bounded JSON provenance identifying the operator version, controls, seed,
  processing mode, and any declared approximation.

## Mandatory local loop

1. Validate the descriptor against the closed Operator API schema.
2. Install the already imported callable through `TrustedOperatorRegistry`;
   the registry never imports the descriptor entrypoint itself.
3. Add a unit test for exact bypass before testing the artistic path.
4. Add repeat-run determinism coverage for deterministic operators.
5. Route every explicit source through **LatentDeck Explicit Device Transfer —
   CPU / CUDA** before the hook/evaluation nodes. Select the same target and
   CUDA index for every source. Use `ERROR` for CUDA-only evidence; use the
   visible `FALLBACK_TO_CPU` policy only when a CPU result is acceptable.
6. Use `99_OPERATOR_DEVELOPER_TEMPLATE.json` for timing/VRAM, determinism, and
   full-clip-versus-chunk checks.
7. Use FAST and HQ comparison to distinguish an operator defect from a preview
   decoder artifact.
8. Export a JSON/Markdown research report and keep any private cartridges,
   weights, renders, and machine paths outside the source repository.

The transfer node preserves shape/dtype and moves H3 visual and optional audio
streams together. It does not cast, resize, crop, or re-encode. A bad CUDA
index, failed CUDA query, allocation/copy failure, or oversized transfer is a
stable error; no such failure triggers an implicit CPU retry. The public 99
template wires separate carrier and donor transfer nodes to CUDA device zero
with a visibly selected CPU fallback so the graph also opens on a CPU-only
development host. Change both policies to `ERROR` before recording CUDA-only
benchmark evidence.

Passing the Toolkit tests does not automatically make an operator eligible for
the standalone realtime Deck. Its declared streaming behavior must match the
measured chunk path, and the exact same full-grid algorithm must meet the
release performance gate without hidden downscale or donor removal.

The public `LatentDeckResearchOperatorHook` value is the bridge from a trusted
external Comfy node to those three evaluation nodes. A copied operator file or
package must provide its own hook-builder node and capture all explicit sources
and controls. `build_installed_operator_research_hook()` supplies the standard
full-clip and ordered chunk wrappers around the slot callable. The checked-in
one-file template and packaged Channel Roll example are complete dual-source
implementations.
The same value supports `single_source`, `dual_source`, and `carrier_donors`;
only the external module knows which explicit inputs its trusted callable
requires. The Toolkit deliberately does not dynamically import an entrypoint
string or discover executable code from a cartridge.

Use `build_installed_operator_research_hook()` after your trusted module has called
`TrustedOperatorRegistry.install()` with its already imported callable. Export
both the normal process node and its topology-specific hook builder through the
module's `NODE_CLASS_MAPPINGS`:

| Topology | Normal process node | Hook-builder node |
|---|---|---|
| `single_source` | source + explicit controls | controls only; the evaluation input is source zero |
| `dual_source` | carrier + donor + controls | donor + controls |
| `carrier_donors` | carrier + every ordered donor + controls | the exact same fixed donor order + controls |

The hook builder passes those non-primary inputs as the ordered
`captured_sources` tuple. It never accepts a module name, file path, entrypoint
string, or code from a cartridge. See the exact graph replacement steps beside
`99_OPERATOR_DEVELOPER_TEMPLATE.json` in the
[workflow guide](../workflows/README.md).

See the normative [Operator API 0.1](../../../spec/operator-api/README.md) for
descriptor fields, trusted-install behavior, wrappers, validation limits, and
stable error semantics.
