# LatentDeck Deck Signal Contract 0.1

## Status and scope

This document defines the shared visual signal and geometry policy used by all
first-party and third-party Deck implementations in LatentDeck 0.1. The Rust
reference API is `latentdeck_core::signal_geometry`.

Cartridge/profile validation remains authoritative. A Deck receives a
`SignalGeometry` only after the selected codec adapter has validated the
cartridge. Future codec adapters may produce the same codec-neutral descriptor;
H3 is merely the first implementation.

The service reports facts and compatibility. It never crops, stretches,
resizes, changes dtype, re-encodes, or substitutes a source.

## Signal geometry

`SignalGeometry` contains the complete visual contract required before a Deck
allocates or processes a source:

- codec family, profile, and profile version;
- runtime dtype, batch, and latent channel count;
- latent `T`, `H`, and `W`;
- decoded frame count, width, and height;
- timing contract, timing-contract version, and exact rational frame rate.

Its derived presentation data contains the intrinsic decoded extent, reduced
aspect ratio, and `portrait`, `landscape`, or `square` orientation. Its workload
data contains checked exact latent sites/values and decoded pixels; arithmetic
overflow is reported as unavailable, never replaced with a guessed or
downscaled workload.

These facts belong to Core rather than to a faceplate. Library cards, slot
pickers, native presentation, diagnostics, and custom Decks therefore describe
the same cartridge consistently.

## Compatibility policies

Every Deck/operator chooses one Core policy before accepting sources:

| Policy | Shared spatial/runtime contract | Clip length |
| --- | --- | --- |
| `playback` | One validated source plays at its intrinsic geometry. | Intrinsic. |
| `spatial_synthesis` | Codec/profile/version, runtime dtype, batch, channels, latent `H/W`, decoded `W/H`, timing contract/version, and frame rate must match. | Independent `T` and decoded frame counts are allowed. |
| `full_tensor_synthesis` | All `spatial_synthesis` fields must match. | Latent `T` and decoded frame count must also match exactly. |

LD-D2 and LD-Q4 use `spatial_synthesis`: each playhead may loop a different
length clip, while every operator slot still sees an exact common spatial grid.
An offline operator that compares complete tensors must declare
`full_tensor_synthesis` instead.

The Core report is deterministic and contains a stable mismatch code plus the
expected and actual values for every candidate. Codes cover codec/profile,
runtime dtype/layout, latent axes, decoded geometry, temporal length, timing,
and frame rate. A faceplate may localize those codes for people, but it must not
implement a second compatibility algorithm.

## Mixed portrait and landscape libraries

In user-facing discussion, “scale” can mean three different facts. LatentDeck
keeps them separate: decoded extent (for example `448×800`), reduced aspect
ratio (for example `14:25`), and latent spatial grid (for example `28×50`).
Matching only the orientation or approximate aspect ratio is never enough for
direct synthesis; the complete Core policy above must pass.

Library and Collections may contain any mixture of validated geometries.
Changing a Collection or Bank never transforms a cartridge and never unloads a
source already retained by a live Deck session.

For the current private H3 development corpus, representative intrinsic
geometries include:

| Orientation | Exact ratio | Latent grid | Decoded extent | Playback | Direct D2/Q4 mixing |
| --- | --- | --- | --- | --- | --- |
| Portrait | `14:25` | `28×50` | `448×800` | Yes | Only with the same shared geometry. |
| Landscape | `7:4` | `84×48` | `1344×768` | Yes | Only with the same shared geometry. |

Those two rows are intentionally incompatible for direct spatial synthesis.
The UI must keep an incompatible cartridge visible, mark it with the exact Core
reason, and prevent only the incompatible slot assignment. It must not hide the
cartridge from Library or `All Cartridges`.

If a researcher deliberately needs a common grid, an explicit Toolkit
Crop/Align node creates a new cartridge and records the selected policy,
parents, and operation history. The original files remain unchanged. There is
no automatic conversion node in a Deck.

## Presentation and output

Native window and fullscreen presentation use aspect-fit against the intrinsic
decoded extent. Unused surface area is cleared to black, producing centered
letterbox or pillarbox bars instead of stretching the image.

Spout2 publishes the raw intrinsic DX12 texture. Bars are a local presentation
decision and are never baked into the shared texture. A receiver therefore sees
the exact decoded width, height, format, sender name, and frame sequence.

The public 0.1 performance target is `448×800` at 24 fps. Each Deck/operator
mode is certified only after its own final receipt passes. Other validated
geometries remain accepted for intrinsic playback, while Deck-specific
functional/performance certification stays pending until a geometry-specific
receipt exists. A Deck must expose that distinction and must not downscale,
drop a donor, or change an algorithm to claim the benchmark.

## Requirements for a custom Deck

A custom Deck targeting the 0.1 contract must:

1. obtain geometry from a validated codec profile;
2. declare the applicable Core compatibility policy for each operator path;
3. use the Core report for source admission and retain its stable mismatch
   codes for UI/diagnostics;
4. keep transport and latent math independent from faceplate state;
5. process the intrinsic latent grid without hidden conversion;
6. use shared aspect-fit presentation when it owns a native output surface;
7. treat explicit conversion as a separate provenance-bearing authoring
   operation.

Input count, carrier/donor order, controls, bypass state, determinism, and
streaming support remain properties of the Deck/operator contract. Geometry
compatibility does not silently infer or change them.
