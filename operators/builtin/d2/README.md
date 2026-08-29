# LD-D2 built-in operator 0.1

This directory is the trusted, built-in Python operator package for the
LatentDeck LD-D2 dual deck. It processes one H3 visual latent slot from deck A
and one from deck B and returns the post-operator latent slot plus JSON-safe
provenance. It does not load code from cartridges.

## Clean-room provenance

The private laboratory evidence established the names of the XS1 through XS5
families, the H3 runtime tensor contract, the usefulness of deterministic
latent operations, and an affinity-routing direction for XS5. The original
math in this package was written for the public 0.1 contract. It is not a copy
or reconstruction of the earlier prototype.

The deliberate 0.1 definitions are:

- `LINEAR`: exact A/B linear interpolation baseline.
- `XS1`: channel-pair rotation on the routed donor.
- `XS2`: circular full-grid exchange with the four spatial neighbours at a
  bounded radius.
- `XS3`: temporal low/high-pass interaction using the previous slot supplied
  independently for each playhead.
- `XS4`: per-channel donor statistics transfer into the structural carrier's
  statistics.
- `XS5`: per-slot cosine-affinity transport over the complete spatial grid,
  using either bounded top-k routing or bounded Sinkhorn normalization.

`ROUTING=A` makes A the structural carrier and B the donor. `ROUTING=B` swaps
those roles without changing the independent A/B playhead positions.
`HYBRIDIZE` blends routed donor material into the carrier. `INTERACT` applies
the routed donor displacement to the carrier. With `CHAOS=0`,
`INTERACTION=0` is an exact bypass to the linear baseline. `CHAOS=0` is itself
an exact unchanged chaos path; non-zero chaos uses a seed-derived, stateless
channel/spatial permutation.

## Contract

`process_slot(a, b, controls, context)` accepts two equal, finite, F16 tensors
with layout `[1, 24, 1, H, W]`. The context must identify H3 profile `0.1.0`
and the H3 causal timing contract `0.1.0`. The implementation never crops,
resizes, downsamples, changes temporal mapping, or chooses a cheaper hidden
algorithm. Inputs larger than the documented full-grid token bound are
rejected explicitly.

For `XS3`, `context.previous_a` and `context.previous_b` are optional equal
slots from the respective independent playheads. At the first slot, an absent
previous value means "previous equals current". No history is inferred or
shared between decks.

The returned `operation` object matches the LC operation-history shape:
operator ID/version, deterministic seed, and fully normalized controls. The
additional profile, playhead, carrier, and grid fields are runtime provenance
for diagnostics and resample orchestration.

The machine-readable descriptor and its schema live beside the package source
as `descriptor.json` and `descriptor.schema.json`.

The dependency-free registry contract lives in
`codec-host/python/src/latentdeck_codec_host/operator_api.py`. Application code
registers this builtin by an explicit ID, version, callable, and matching
exported entrypoint; descriptor text is never imported dynamically. The H3
binding in `codec-host/codecs/h3/src/latentdeck_codec_h3/d2_engine.py` owns the
two independently indexed F16 sources and feeds each post-operator slot
directly into the causal decoder. It is deliberately separate from Player's
single-cartridge worker state.

Loop and Restart first return a typed reset barrier. The scheduler may resume
only after the decoder reset succeeds with a strictly newer nonzero `u64`
generation. A failed reset preserves the barrier, playheads, sequence, and XS3
history so it can be retried without crossing causal state.

## Local checks

From the repository root, use the workspace's pinned PyTorch runtime and test
extra:

```powershell
uv run --package latentdeck-operator-d2 --extra cu130 --extra test pytest operators/builtin/d2/tests
uv run ruff check operators/builtin/d2
```

The H3 pre-decode binding has a separate synthetic conformance check:

```powershell
uv run --package latentdeck-codec-h3 --extra cu130 pytest codec-host/codecs/h3/tests/test_d2_engine.py
```

## Isolated worker boundary

The Codec Pack installs a second no-argument process entrypoint for the deck:

```powershell
uv run --package latentdeck-codec-h3 --extra cu130 latentdeck-h3-d2-worker
```

Like the Player worker, it reads the one-time bootstrap secret from inherited
stdin and then uses length-prefixed MessagePack over the supervisor-created
Named Pipe. Its closed commands are `deck.d2.load`,
`deck.d2.process_slot`, `deck.d2.reset`, `deck.d2.restart`,
`deck.d2.controls.set`, `deck.d2.transport.set`, `deck.d2.seed.set`, and
`deck.d2.status`. The host scheduler, never the UI, sends `process_slot` and
the generation-changing reset command.

`deck.d2.process_slot` runs the trusted operator over the complete F16 latent
slot before TAEH3 decode and publishes the resulting one-to-four RGBA frames
to the bounded shared-memory ring. Its control acknowledgement contains only
typed counters, playheads, ring sequences, and canonical JSON provenance. It
does **not** place latent bytes or paths on control IPC.

Snapshot and Live Capture use a separate disk sink attached to
`D2ProcessedSlot.output` immediately before decode. The three closed Python
worker commands are:

```text
deck.d2.capture.start  {
  deck_id, deck_revision, capture_id,
  mode: "snapshot" | "live_capture",
  temporary_root, max_latent_slots, max_visual_bytes
}
deck.d2.capture.stop   {deck_id, deck_revision, capture_id}
deck.d2.capture.status {deck_id, deck_revision, capture_id}
```

`capture_id` is a canonical non-nil UUID. `temporary_root` must be an existing
absolute directory selected by the trusted host. Starting either mode first
requests a restart barrier; no slot is written until a strictly newer causal
generation resets both playheads to zero. Snapshot freezes controls, seed, and
transport and automatically finalizes exactly one structural-carrier cycle.
A shorter, playing, loop-enabled non-carrier is rejected before Snapshot
starts because it would force a reset inside that cycle.

Live Capture records at most 32 full control/seed state events. Stop finalizes
immediately when the current length already satisfies `T=2+5n`; otherwise it
arms the first future valid length. Transport remains frozen so playhead/time
mapping cannot change silently. Changing routing remains visible in the event
history and makes carrier audio temporally ineligible.

The bounded receipt contains only the capture ID/mode, partial Safetensors
path, SHA-256, byte length, F16 visual shape/frame count, two parent
identities, audio disposition/descriptor, and either frozen Snapshot state or
the Live control history. Audio disposition values match the LC contract:
`source_absent`, `copied_from_carrier_exact`, or `omitted_timing_mismatch` with
`duration_mismatch`, `temporal_mapping_mismatch`, or
`duration_and_mapping_mismatch`.

The worker owns both capture partials until the host validates and consumes the
finished payload. Active or finished capture-owned partials are deleted on
worker close/unload; active partials are also deleted on reset, process/decode
error, spool failure, or capture replacement. Successful host consumption
must therefore happen before codec worker teardown.

The test corpus is generated synthetically in memory. No cartridges, latent
payloads, model assets, workflows, or generated media are included.
