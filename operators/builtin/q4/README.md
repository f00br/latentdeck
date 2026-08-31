# LD-Q4 built-in operator 0.1

This directory contains the trusted built-in Python operator package for the
LatentDeck LD-Q4 quad deck. The operator accepts one explicitly selected H3
carrier slot and three logical donor roles, B, C, and D. It returns one
post-operator latent slot plus JSON-safe provenance. It never loads code from a
cartridge.

## Clean-room provenance

The documented LatentDeck 0.1 operator contract fixes the
carrier-plus-three-donors topology,
the names `LINEAR` and `XS5`, the H3 runtime tensor contract, and the requirement
to offer bounded top-k and Sinkhorn routing. The math and control mapping in
this package were written for that public contract. No private workflow,
checkpoint, latent payload, generated output, or earlier implementation was
copied or reconstructed.

The deliberate Q4 0.1 definitions are:

- `LINEAR`: `(1 - INTERACTION) * carrier + INTERACTION * donor_mix`.
- `XS5 TOPK`: cosine affinity from the unchanged carrier to all three donors,
  evaluated as one donor batch over the complete spatial grid, followed by
  bounded top-k softmax transport.
- `XS5 SINKHORN`: the same batched full-grid affinity followed by a bounded
  number of alternating log-space row and column normalizations.
- `HYBRIDIZE`: accumulates each routed donor toward the carrier while
  `PRESERVE` retains structural carrier material.
- `INTERACT`: accumulates each routed displacement relative to its original
  donor.

Each donor is routed relative to the same unchanged carrier. Routed states are
then accumulated in the fixed logical order B, C, D. `INTERACTION` is the total
donor strength; the three donor weights determine only its relative
distribution and are normalized to sum to one.

## Influence field

Manual B/C/D weights remain independent controls. The optional triangular
field is only a macro over those same three weights. Its normalized vertices
are B `(0, 0)`, C `(1, 0)`, and D `(0.5, 1)`. A point maps to barycentric
weights:

```text
B = 1 - x - 0.5y
C = x - 0.5y
D = y
```

Points outside the triangle are rejected. The operator never silently clamps
or substitutes a different distribution.

## Runtime contract

`process_slot(carrier, donor_b, donor_c, donor_d, controls, context)` accepts
four equal, finite, F16 tensors with layout `[1, 24, 1, H, W]`. The caller
selects the physical carrier and supplies it as the first argument; the
remaining physical slots are assigned to the logical B/C/D donor roles.
`Q4Context` records the physical slot, cartridge identity, and independent
playhead for every role.

The context must identify H3 profile `0.1.0` and the H3 causal timing contract
`0.1.0`. Inputs are immutable. Processing never crops, resizes, downsamples,
changes temporal mapping, or chooses a cheaper algorithm. A grid beyond the
documented token limit, top-k larger than the grid, or an out-of-range
Sinkhorn iteration count is rejected explicitly.

`CHAOS` is a seeded, stateless permutation perturbation. `CHAOS=0` is an exact
unchanged chaos path. With both `INTERACTION=0` and `CHAOS=0`, the output is an
exact clone of the carrier.

The `operation` provenance object records the operator ID and version, seed,
fully resolved controls, identities, playheads, normalized donor weights,
routing method, fixed accumulation order, and full-grid dimensions. The
machine-readable descriptor and its schema live beside the package source as
`descriptor.json` and `descriptor.schema.json`.

The descriptor intentionally uses only the closed generic Operator Descriptor
0.1 fields. Q4 topology is executable behavior documented by this contract and
recorded in per-operation provenance; it is not an operator-specific top-level
descriptor extension.

## Local checks

With a Python 3.13 environment containing the pinned PyTorch runtime:

```powershell
$env:PYTHONPATH = "operators/builtin/q4/src"
python -m unittest discover -s operators/builtin/q4/tests -v
python -m ruff check operators/builtin/q4
uv build --project operators/builtin/q4
```

Set `LATENTDECK_RUN_CUDA_TESTS=1` to include the optional TOPK and Sinkhorn
CPU/CUDA parity test. All test tensors are generated synthetically in memory.
No cartridges, raw latents, model assets, workflows, or generated media are
included.
