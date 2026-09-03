# LatentDeck Q4 Deck Package 0.2.1

Q4 is the bundled four-source reference Deck for LatentDeck `0.1.x`. It is a
real `.ld` package that runs through the same validator, trust lifecycle,
generic Protocol 2 runtime, Deck SDK, and declarative faceplate contract as an
external Deck. It has no private Q4 worker or application-only operator API.

The package source in this directory is authoritative. The Python wheel uses
the same operator module, but a wheel by itself is not an installable Deck and
does not supply the package manifest, signal contract, or faceplate.

## Contract identity

| Axis                | Value                                  |
| ------------------- | -------------------------------------- |
| Deck package        | `org.latentdeck.deck.q4` `0.2.1`       |
| Package manifest    | `deck-pack.json` `1.0.0`               |
| Operator            | `org.latentdeck.builtin.ld_q4` `0.2.0` |
| Operator schema/API | `0.2.0` / `0.2.0`                      |
| Python package      | `latentdeck-operator-q4` `0.2.0`       |
| Deck SDK            | `latentdeck-deck-sdk` `0.2.0`          |
| Faceplate schema    | `2`                                    |
| Worker protocol     | `2`                                    |
| Runtime kind        | `python_operator_stream_v1`            |

The Deck package and operator have independent versions. Changing package
metadata or its faceplate can require a new `deck_version` without changing
the mathematical `operator_version`.

## Signal and roles

Q4 declares four physical source slots and four logical roles:

- `carrier` is the unchanged structural reference for routing;
- `donor_b`, `donor_c`, and `donor_d` provide the three influence sources.

The default permutation maps physical slots A through D to those roles in
order. The faceplate role editor may assign any exact permutation. Role changes
do not reorder the source tuple or move independent playheads; the operator
uses `DeckOperatorContext` to map each logical role back to its physical slot.

The bundled manifest accepts these exact CUDA F16 shapes:

```text
[1, 24, 1, 50, 28]
[1, 24, 1, 48, 28]
[1, 24, 1, 48, 84]
[1, 24, 1, 30, 45]
```

Timing is exactly `24/1` frames per second with 24 samples per latent slot.
The profile allowlist is `null`, so Q4 does not hard-code H3 identity. This
does not relax compatibility: the enabled Codec Package, selected cartridges,
Torch/tensor ABI, geometry, timing, and five required capabilities must still
intersect exactly. Q4 never casts, resizes, crops, aligns, re-encodes, or moves
a source to another device.

## Synthesis controls

`LINEAR` mixes the three donors by normalized influence weight and interpolates
from the logical carrier by `interaction`:

```text
(1 - interaction) * carrier + interaction * donor_mix
```

`XS5` computes full-grid cosine affinity from the same unchanged carrier to all
three donors as one donor batch. It then applies bounded TOPK softmax transport
or bounded log-space Sinkhorn normalization. Routed donor states accumulate in
the fixed logical order B, C, D. `mode` chooses hybridization or
displacement-style interaction, and `preserve` controls retained carrier
structure.

The donor distribution has two presentations over the same three weights:

- `manual` exposes independent B, C, and D values and normalizes them to one;
- `triangle` maps a point in a barycentric field to B, C, and D.

The triangle vertices are B `(0, 0)`, C `(1, 0)`, and D `(0.5, 1)`:

```text
B = 1 - x - 0.5y
C = x - 0.5y
D = y
```

Points outside the triangle and an all-zero manual distribution are rejected.
Seeded `chaos` applies a deterministic channel/spatial permutation
perturbation; `chaos = 0` is unchanged. With `interaction = 0` and
`chaos = 0`, the output is an exact carrier clone.

All 15 public controls are declared in `operator.json`. The operator rejects
unknown or ill-typed controls, TOPK larger than the current grid, invalid
Sinkhorn bounds, and grids larger than 4096 spatial tokens. It records exact
operator identity, normalized controls, seed, profile, role/slot mapping,
playheads, resolved donor weights, routing, accumulation order, and grid in
bounded JSON provenance.

## Generic Deck SDK and runtime

The package manifest selects the
`latentdeck_operator_q4.operator:process_sources_host` entrypoint. Protocol 2
loads it through the enabled Deck package and compatible Codec Package. The
generic worker invokes it through the Deck SDK gate:

```python
process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult
```

The SDK requires four finite contiguous tensors with identical shape, dtype,
and device, plus bounded scalar controls and an exact role permutation. The
result must be finite and contiguous, preserve the input shape, dtype, and
device exactly, and contain bounded data-only provenance.

The standalone `process_sources` export wraps the same implementation with SDK
validation for tests and direct SDK use. The host entrypoint avoids duplicating
that gate because the generic worker already applies it. Neither entrypoint
imports the codec host or owns transport, decode, output, capture, or package
lifecycle.

## Declarative faceplate

The schema-v2 faceplate declares six host-rendered sections:

- four source pickers;
- independent transport and seed;
- the carrier/three-donor role editor;
- the synthesis controls and barycentric influence field;
- Snapshot and Live Capture actions;
- one output monitor anchor.

Every operator control is bound exactly once. `visible_when` swaps manual donor
numbers for the triangle and reveals only the active XS5/TOPK/Sinkhorn
parameters. The host renders the layout and owns accessibility, realtime
dispatch, native video, fullscreen, Spout, MP4, and capture orchestration; the
package contains no HTML, JavaScript, CSS, native window code, or private Tauri
commands.

Snapshot and Live Capture receive the post-operator latent state before decode.
Core validates and imports the finished `.lc`; Q4 never writes a cartridge
directly or embeds code in one.

## Trust and provenance

The deterministic `.ld` archive is a closed integrity-catalogued tree. It must
be installed, verified, and explicitly enabled through the Extensions Manager.
Active sessions retain the exact validated package and Codec Package versions;
there is no newest-version or protocol fallback.

The carrier/three-donor topology and broad XS5 research direction came from
private laboratory evidence. The public implementation and control mapping
were written for this contract and do not copy a private workflow, model,
cartridge, or latent payload. Tests generate their tensors synthetically.

## Focused checks

From the repository root:

```powershell
uv run --package latentdeck-operator-q4 --extra cu130 --extra test pytest operators/builtin/q4/tests
uv run ruff check operators/builtin/q4
```

The package tests check the manifest/operator/faceplate cross-contract,
integrity catalog, deterministic `.ld` packing, isolated imports, generic SDK
behavior, and the operator's deterministic golden traces.
