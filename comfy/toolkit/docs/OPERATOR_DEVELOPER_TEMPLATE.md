# MyLatentOperator developer template

Start from the public
[`channel-roll`](../../../operators/examples/channel-roll/README.md) example.
Copy the package into a new, separately installed Python distribution; change
its stable ID, descriptor, implementation, tests, and license metadata. Do not
place operator code inside a cartridge.

## Choose one topology

- `single_source` receives `(source,)` and declares `input_count: 1`.
- `dual_source` receives `(carrier, donor)` and declares `input_count: 2`.
- `carrier_donors` receives `(carrier, donor_1, donor_2, ...)` in stable order
  and declares the exact `input_count` from 2 through 16.

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
5. Use `99_OPERATOR_DEVELOPER_TEMPLATE.json` for timing/VRAM, determinism, and
   full-clip-versus-chunk checks.
6. Use FAST and HQ comparison to distinguish an operator defect from a preview
   decoder artifact.
7. Export a JSON/Markdown research report and keep any private cartridges,
   weights, renders, and machine paths outside the source repository.

Passing the Toolkit tests does not automatically make an operator eligible for
the standalone realtime Deck. Its declared streaming behavior must match the
measured chunk path, and the exact same full-grid algorithm must meet the
release performance gate without hidden downscale or donor removal.

The public `LatentDeckResearchOperatorHook` value is the bridge from a trusted
external Comfy node to those three evaluation nodes. A copied operator package
must provide its own hook-builder node, capture all explicit sources and
controls, and implement both a full-clip callable and an ordered chunk callable.
The checked-in Channel Roll example is a complete dual-source implementation.
The same value supports `single_source`, `dual_source`, and `carrier_donors`;
only the external package knows which explicit inputs its trusted callable
requires. The Toolkit deliberately does not dynamically import an entrypoint
string or discover executable code from a cartridge.

Use `build_installed_operator_research_hook()` after your package has called
`TrustedOperatorRegistry.install()` with its already imported callable. Export
both the normal process node and its topology-specific hook builder through the
package's `NODE_CLASS_MAPPINGS`:

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
