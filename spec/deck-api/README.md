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

Decoded MP4 recording is a separate viewing derivative of that same intrinsic
Deck output. The bundled D2/Q4 implementations submit each validated frame
consumed by the authoritative presentation sequence, including when local
window presentation is temporarily skipped, preserve its decoded width and
height without resize or crop, and write video-only H.264 at the 0.1 cadence of
24 fps. The result is not a cartridge and does not contain latent or audio
payloads. Encoding runs behind a bounded queue; encoder failure or overflow
terminates only the recording and must not stall or terminate Deck playback.
The final `.mp4` is no-clobber and becomes visible only after safe finalization
of a sibling partial file. Latent Snapshot/Live Capture and decoded MP4
recording are mutually exclusive in 0.1.

Library changes are invalidation-driven. Import or successful capture must
refresh an active D2/Q4 source list without requiring the user to toggle Active
Collection, while an older asynchronous Library response must not replace a
newer view. Refresh preserves the currently playing identities and an explicit
next-load draft.

Changing a source picker edits only the next-load draft. A newly captured
cartridge enters a running Deck only through an explicit `Use capture in …` or
normal Load action. The host resolves immutable identities and validates the
complete candidate launch configuration before stopping the current worker. A
successful bounded replacement retains the other source choices, controls,
roles, seed, loop/play intent, and any active decoded-video recorder; causal
operator state restarts at the source-replacement boundary.

The public 0.1 performance target is `448×800` at 24 fps. Each Deck/operator
mode is certified only after its own final receipt passes. Other validated
geometries remain accepted for intrinsic playback, while Deck-specific
functional/performance certification stays pending until a geometry-specific
receipt exists. A Deck must expose that distinction and must not downscale,
drop a donor, or change an algorithm to claim the benchmark.

## Embedded output surface ownership

The video area shown inside a Deck is part of that Deck's faceplate layout,
but the native renderer is a capability supplied by the LatentDeck host. This
is an intentional split rather than a D2/Q4 implementation detail:

- the Deck owns an empty layout anchor, its visible/inactive state, and the
  user's fullscreen intent;
- the shared Deck-facing API carries revisioned CSS bounds and visibility to
  the host;
- the host reads the authoritative window scale and client extent, validates
  and converts the request to physical parent-client coordinates, and owns the
  child HWND, wgpu surface, frame sequencing, fullscreen transition, and Spout
  publication;
- the Deck never receives a raw HWND, DX12 device, decoded RGB copy, or an
  unrestricted native-window creation capability.

The reference frontend capability is `DeckEmbeddedOutputHost`; the reference
native implementation is `latentdeck_native_output::NativeOutput::new_embedded`.
LD-D2 and LD-Q4 currently adapt their typed command namespaces to that same
capability. Command prefixes and faceplate CSS are replaceable adapters, not
separate presentation contracts.

Bounds use a host-issued session epoch plus client revisions that are mapped to
host-monotonic applied revisions. Stale epochs or revisions are rejected, and
one client revision may be retried only with exactly the same geometry. A
hidden or zero-area anchor suspends the local surface instead of inventing
geometry, and a visible request is acknowledged only after host validation,
native placement, and a final state check succeed. A Deck must not enable
loading or fullscreen presentation until its first visible viewport request is
acknowledged.

The faceplate layout must also keep that acknowledged video area usable while
the operator controls are being edited. A long scrolling control surface must
not create a deadlock where `Load` is reachable only after the required video
anchor has left the client area. The bundled D2 and Q4 references use a sticky
program monitor; a split-pane custom Deck may satisfy the same requirement.

This 0.1 source tree proves the contract with the two bundled Decks. Loading an
arbitrary third-party Deck package is a separate installation and trust
boundary; when exposed by a future Deck SDK, it must wrap this host capability
rather than ask community Decks to copy Win32/wgpu code or create their own
top-level output windows.

## Requirements for a custom Deck

A custom Deck targeting the 0.1 contract must:

1. obtain geometry from a validated codec profile;
2. declare the applicable Core compatibility policy for each operator path;
3. use the Core report for source admission and retain its stable mismatch
   codes for UI/diagnostics;
4. keep transport and latent math independent from faceplate state;
5. process the intrinsic latent grid without hidden conversion;
6. use shared aspect-fit presentation when it owns a native output surface;
7. bind its faceplate video anchor through the shared embedded-output host
   capability instead of creating a second program window;
8. treat explicit conversion as a separate provenance-bearing authoring
   operation.

Input count, carrier/donor order, controls, bypass state, determinism, and
streaming support remain properties of the Deck/operator contract. Geometry
compatibility does not silently infer or change them.
