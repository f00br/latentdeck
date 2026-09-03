# LatentDeck D2 Deck Package 0.2.1

D2 is the bundled two-source reference Deck for LatentDeck `0.1.x`. It is a
real `.ld` package that uses the same package validator, trust lifecycle,
generic Protocol 2 runtime, Deck SDK, and declarative faceplate contract as an
external Deck. It has no private D2 worker or application-only operator API.

The package source in this directory is authoritative. The Python wheel uses
the same operator module, but a wheel by itself is not an installable Deck and
does not supply the package manifest, signal contract, or faceplate.

## Contract identity

| Axis                | Value                                  |
| ------------------- | -------------------------------------- |
| Deck package        | `org.latentdeck.deck.d2` `0.2.1`       |
| Package manifest    | `deck-pack.json` `1.0.0`               |
| Operator            | `org.latentdeck.builtin.ld_d2` `0.2.0` |
| Operator schema/API | `0.2.0` / `0.2.0`                      |
| Python package      | `latentdeck-operator-d2` `0.2.0`       |
| Deck SDK            | `latentdeck-deck-sdk` `0.2.0`          |
| Faceplate schema    | `2`                                    |
| Worker protocol     | `2`                                    |
| Runtime kind        | `python_operator_stream_v1`            |

The Deck package and operator have independent versions. Changing package
metadata or its faceplate can require a new `deck_version` without changing
the mathematical `operator_version`.

## Signal and roles

D2 declares two physical source slots and two logical roles:

- `carrier` is the structural reference;
- `donor` supplies material to the selected synthesis algorithm.

The default permutation maps physical slots A and B to `carrier` and `donor`.
The faceplate role editor may swap that mapping without moving the sources or
their independent playheads. The role binding in `DeckOperatorContext` is
authoritative. The old `routing` control is not part of `operator.json`; a
direct SDK caller that supplies it must agree with the role binding.

The bundled manifest accepts these exact CUDA F16 shapes:

```text
[1, 24, 1, 50, 28]
[1, 24, 1, 48, 28]
[1, 24, 1, 48, 84]
[1, 24, 1, 30, 45]
```

Timing is exactly `24/1` frames per second with 24 samples per latent slot.
The profile allowlist is `null`, so D2 does not hard-code H3 identity. This
does not relax compatibility: the enabled Codec Package, selected cartridges,
Torch/tensor ABI, geometry, timing, and five required capabilities must still
intersect exactly. D2 never casts, resizes, crops, aligns, re-encodes, or moves
a source to another device.

## Synthesis controls

`LINEAR` is the A/B interpolation baseline controlled by `mix`. The remaining
algorithm choices operate on the logical carrier and donor:

- `XS1` rotates a selected pair of donor channels;
- `XS2` exchanges material with four spatial neighbours at a bounded radius;
- `XS3` combines current and previous source slots for temporal low/high-pass
  interaction;
- `XS4` transfers donor statistics into the carrier's per-channel statistics;
- `XS5` performs full-grid cosine-affinity transport with bounded TOPK or
  Sinkhorn routing.

`interaction` blends an XS target with the linear baseline. `mode` selects
hybridization or displacement-style interaction, `preserve` controls retained
carrier structure, and seeded `chaos` adds a deterministic channel/spatial
permutation perturbation. With `interaction = 0`, the XS path returns to the
linear baseline; with `chaos = 0`, the chaos path is unchanged.

All 16 public controls are declared in `operator.json`. The operator rejects
unknown or ill-typed controls, equal XS1 channel indices, TOPK larger than the
current grid, and grids larger than 4096 spatial tokens. It records the exact
operator identity, normalized controls, seed, profile, playheads, carrier,
history use, and grid in bounded JSON provenance.

## Generic Deck SDK and runtime

The package manifest selects the
`latentdeck_operator_d2.operator:process_sources_host` entrypoint. Protocol 2
loads it through the enabled Deck package and compatible Codec Package. The
generic worker invokes it through the Deck SDK gate:

```python
process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult
```

The SDK requires two finite contiguous tensors with identical shape, dtype,
and device, plus bounded scalar controls and an exact role permutation.
Previous-source history stays attached to its physical slot when roles move.
The result must be finite and contiguous, preserve the input shape, dtype, and
device exactly, and contain bounded data-only provenance.

The standalone `process_sources` export wraps the same implementation with SDK
validation for tests and direct SDK use. The host entrypoint avoids duplicating
that gate because the generic worker already applies it. Neither entrypoint
imports the codec host or owns transport, decode, output, capture, or package
lifecycle.

## Declarative faceplate

The schema-v2 faceplate declares six host-rendered sections:

- two source pickers;
- independent transport and seed;
- the carrier/donor role editor;
- the synthesis controls;
- Snapshot and Live Capture actions;
- one output monitor anchor.

Every operator control is bound exactly once. Algorithm-specific controls use
`visible_when`, so XS1 through XS5 show only their relevant parameters. The
host renders the layout and owns accessibility, realtime dispatch, native
video, fullscreen, Spout, MP4, and capture orchestration; the package contains
no HTML, JavaScript, CSS, native window code, or private Tauri commands.

Snapshot and Live Capture receive the post-operator latent state before decode.
Core validates and imports the finished `.lc`; D2 never writes a cartridge
directly or embeds code in one.

## Trust and provenance

The deterministic `.ld` archive is a closed integrity-catalogued tree. It must
be installed, verified, and explicitly enabled through the Extensions Manager.
Active sessions retain the exact validated package and Codec Package versions;
there is no newest-version or protocol fallback.

The XS family names and broad research direction came from private laboratory
evidence. The public implementation and control mapping were written for this
contract and do not copy a private workflow, model, cartridge, or latent
payload. Tests generate their tensors synthetically.

## Focused checks

From the repository root:

```powershell
uv run --package latentdeck-operator-d2 --extra cu130 --extra test pytest operators/builtin/d2/tests
uv run ruff check operators/builtin/d2
```

The package tests check the manifest/operator/faceplate cross-contract,
integrity catalog, deterministic `.ld` packing, isolated imports, generic SDK
behavior, and the operator's deterministic golden traces.
