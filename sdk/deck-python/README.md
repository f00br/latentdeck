# LatentDeck Deck SDK for Python 0.2.0

`latentdeck-deck-sdk` is the authoritative Python operator contract for generic
LatentDeck Decks. It lets a `.ld` Deck Package provide latent-space behavior
without adding a product-specific worker command or rebuilding the host.

Use this SDK when implementing the Python operator named by a Deck Package.
After reading this document, an implementer should be able to export one
callable, consume physical sources and logical roles correctly, return an exact
latent tensor plus bounded provenance, and test the operator through the same
gates used by the generic worker.

## Boundary and entrypoint

A Deck Package owns its operator, source/role/control declaration, compatibility
requirements, and declarative faceplate. The host owns package validation and
trust, source selection, transport, sessions, output, capture, presets, and UI
rendering. The compatible Codec Package owns cartridge access and decode.

The Deck SDK owns only the operator-call contract and its pre/post validation.
It does not:

- build, install, enable, trust, or select a `.ld` package;
- open cartridges, decode frames, write captures, or address an output surface;
- expose paths, Tauri commands, HTML, JavaScript, CSS, or a private worker API;
- resolve package compatibility or repair incompatible tensors;
- provide Torch or a security sandbox.

The package declares a portable `module:callable` entrypoint. The generic
worker imports that exact callable from the enabled, hash-bound package and
invokes it as:

```python
def process_sources(
    sources: tuple[object, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult:
    ...
```

The SDK is typed without importing Torch at module import time. At validation
time it uses the exact Torch build supplied by the selected Codec Package.

## Minimal operator

This operator returns the source currently bound to the logical `carrier` role.
It uses `physical_slots` rather than assuming role assignment changes tuple
order.

```python
from latentdeck_deck_sdk import DeckOperatorContext, DeckOperatorResult


def process_sources(
    sources: tuple[object, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult:
    del controls
    carrier_slot = next(
        binding.physical_slot
        for binding in context.roles
        if binding.role == "carrier"
    )
    source_index = context.physical_slots.index(carrier_slot)
    return DeckOperatorResult(
        output=sources[source_index],
        provenance={
            "carrier_slot": carrier_slot,
            "sequence": context.sequence,
        },
    )
```

The package's operator descriptor must declare the `carrier` role expected by
this code. In production, the generic worker wraps the callable with
`process_sources_checked()`. Do not wrap it again inside the entrypoint. Use
that helper directly in isolated unit tests so test and production gates stay
the same.

## Call contract

One call processes one ordered latent tick. Inputs are:

- `sources`: one to 16 current latent tensors;
- `controls`: the complete typed control state for this tick;
- `context`: immutable identity, timing, causal state, role bindings, and
  physical-slot history.

`DeckOperatorContext` contains:

| Field group    | Meaning                                                                                            |
| -------------- | -------------------------------------------------------------------------------------------------- |
| Profile        | `codec_family`, `profile`, and `profile_version`                                                   |
| Timing         | `timing_contract`, `timing_contract_version`, `frame_rate_numerator`, and `frame_rate_denominator` |
| Causality      | positive `generation` and `sequence`, plus deterministic `seed`                                    |
| Physical state | `playheads`, `physical_slots`, and `previous_sources`, aligned by tuple index                      |
| Logical state  | `RoleBinding` values with unique role names that point to valid physical slots                     |

Logical role changes never reorder physical sources or move their playheads and
history. Pair `sources[index]`, `playheads[index]`,
`previous_sources[index]`, and `physical_slots[index]`; resolve a role through
its `physical_slot`. A previous source is either `None` or the prior tensor for
that same physical slot.

Use `context.seed` for any randomness. An operator should produce the same
result for the same sources, controls, and context. Do not use ambient random
state, wall-clock time, process state, or hidden mutable role history.

## Tensor and control invariants

`process_sources_checked()` enforces the full boundary before and after the
operator call:

- `sources` is a tuple containing `1..=16` Torch tensors;
- every current/history tensor is finite and contiguous with shape
  `[1,C,1,H,W]`;
- dtype is `float16`, `bfloat16`, or `float32` on `cpu` or `cuda`;
- all current sources and non-`None` history have identical shape, dtype, and
  device;
- physical slots are one permutation of `1..N`, and playhead/history lengths
  equal `N`;
- controls contain at most 64 unique bounded names and scalar boolean, integer,
  finite number, or bounded text values.

Treat current and previous source tensors as read-only. The SDK batches the
current/history finite checks into one aggregate synchronization point and
performs one post-operator finite check. Do not repeat those full-tensor gates
inside the operator unless the algorithm itself requires a narrower assertion.

The operator must return a `DeckOperatorResult` containing:

- one finite, contiguous output tensor with exactly the shape, dtype, and
  device of source 1;
- a bounded JSON-object `provenance` value.

Provenance may contain `None`, booleans, integers, finite numbers, bounded text,
objects with bounded identifier keys, and bounded arrays. It must not contain
tensors, bytes, paths, handles, arbitrary objects, NaN, or infinity.
The current Protocol 2 process acknowledgement exposes the bounded scalar
top-level projection, so keep provenance intended for the host at that level.

## No hidden conversion

An operator may perform declared latent-space math, but it must preserve the
negotiated tensor ABI exactly. It must not cast, resize, crop, pad, align,
resample, re-encode, move devices, substitute profiles, decode, or silently
drop a source to make an incompatible set run.

Package and selected-source compatibility is decided before the operator is
called. Unexpected input should fail with `DeckContractError`, not be repaired.
Post-operator output is captured before decode, so Snapshot and Live Capture
record the actual latent result returned by the Deck.

## Validation API

- `validate_process_call()` validates context, sources, history, and controls,
  then returns the normalized control dictionary without running an operator.
- `validate_process_result()` validates a returned `DeckOperatorResult` against
  the input tensor ABI.
- `process_sources_checked()` runs both gates around one callable and is the
  preferred unit-test entrypoint.

Failures raise `DeckContractError` with a stable path-free `code` and `detail`
from the failed contract gate. Do not catch a contract error and retry with
transformed inputs.

## Public exports

The package root exports exactly:

| Name                      | Purpose                                                               |
| ------------------------- | --------------------------------------------------------------------- |
| `DeckOperator`            | Runtime-checkable structural callable protocol                        |
| `DeckOperatorContext`     | Immutable per-tick profile, timing, causal, role, and history context |
| `DeckOperatorResult`      | Exact latent output and bounded provenance                            |
| `RoleBinding`             | Logical role to physical-slot binding                                 |
| `DeckContractError`       | Stable contract failure                                               |
| `validate_process_call`   | Pre-call boundary validation                                          |
| `validate_process_result` | Post-call boundary validation                                         |
| `process_sources_checked` | Combined validated invocation                                         |

## Package and trust caveat

The SDK does not make a Python file into a Deck Package. The `.ld` format binds
the operator callable to exact package/operator identities, compatibility
requirements, a typed control and role descriptor, a host-rendered faceplate,
and a closed integrity catalog. Keep those declarations consistent with the
callable: every runtime control and required role comes from the package
contract, not from hidden code or UI state.

An enabled Deck operator is trusted executable Python running with the current
user's authority. Package validation, exact hashes, retained usage leases,
environment clearing, and the worker process boundary are integrity and
lifecycle controls, not a security sandbox. Do not rely on ambient packages,
credentials, network access, or user files, and enable only Deck code the user
deliberately trusts.

## Author checks

Before packaging an operator:

1. Exercise `process_sources_checked()` with every declared source count,
   control mode, and role permutation.
2. Test exact ABI mismatches: source shape, dtype, device, contiguity, finite
   values, and incompatible history.
3. Test missing/duplicate roles, physical-slot association, playhead history,
   generation changes, and deterministic seed behavior.
4. Test output ABI and provenance bounds, including NaN, infinity, tensors, and
   oversized nested data.
5. Test the package through the generic worker with a compatible non-H3 codec;
   the operator must not depend on a product-specific D2/Q4 command or codec
   implementation.
