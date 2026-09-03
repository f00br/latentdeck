# LatentDeck Worker Protocol 2

Status: normative for the generic LatentDeck worker boundary. Protocol 1 is
retained only as an explicitly selected, Player-only H3 compatibility bridge.

This document is for Codec Pack, Deck runtime, and host implementers. After
reading it, an implementer should be able to build a compatible Protocol 2
peer, choose the correct Player bridge deliberately, and reject incompatible
or ambiguous behavior without relying on the former D2/Q4-specific commands.

## Scope and version policy

Worker Protocol 2 is the control plane between Core and one isolated Codec
Pack worker. It defines bounded commands, acknowledgements, errors, events,
identity receipts, and operating-system handle descriptors. It does not define
the LC container, Codec SDK algorithms, Deck operator behavior, package
installation, the decoded-ring memory layout, process scheduling, or the user
interface.

The protocol marker is `latentdeck.worker`; the exact version is the integer
`2`. There is no version-range negotiation after launch. A Protocol 2 launch
accepts only Protocol 2, and any Protocol 2 failure is returned to the caller.
It must never trigger an implicit retry through Protocol 1.

Protocol 2 replaces every production Deck-specific Protocol 1 command. D2,
Q4, bundled Decks, and external `.ld` Decks all use the same generic
`deck.*` and `capture.*` command families.

## Control data and bulk data

Protocol frames never contain:

- cartridge archive bytes or latent tensor bytes;
- decoded RGBA bytes;
- model weights or external asset bytes;
- Python modules or Deck package files.

Core first performs codec-neutral LC integrity validation and retains the exact
archive through a read-only operating-system handle that disallows share-write
and share-delete. `source.open` carries a handle duplicated into the worker,
the exact cartridge/archive identity, and a bounded integrity-access receipt.
The worker consumes that retained object instead of reopening an arbitrary
path.

Decoded output uses a host-created Ring ABI 2 shared-memory mapping.
`ring.configure` transfers only duplicated mapping/event handle values and
bounded geometry. The current Player and Deck runtimes configure a
`decoded_rgba` ring containing complete CPU `uint8 [N,H,W,4]` batches. The
control enum also reserves `latent_tensor`, but the production Player and Deck
flows do not send latent payloads through control frames.

External assets are explicit `asset_id`, path, lowercase SHA-256, and byte
length bindings. The host selection layer validates and retains the selected
file before worker launch; the Codec Pack may not discover or substitute an
asset implicitly.

## Windows transport and supervision

The release transport is a local byte-mode Named Pipe. Core creates a fresh
random pipe for each worker session with one instance, first-instance-only
creation, remote-client rejection, 64 KiB input/output buffers, and a DACL
containing only the current process user. After connection, Core verifies that
the pipe client PID is the PID of the process it spawned.

The worker executable, arguments, and working directory come only from the
exact enabled `.ldcodec` version after package-tree and trust-receipt
validation. Core launches that executable directly without a shell or PATH
lookup. The inherited environment is cleared. Only `SystemRoot`,
`SystemDrive`, `TEMP`, and `TMP` are copied, and
`PYTHONDONTWRITEBYTECODE=1` is set explicitly. A Codec Pack must therefore be
self-contained and must not depend on inherited Python, CUDA, PATH, profile,
or credential variables.

Before the worker receives any command, Core assigns it to a Job Object with
`KILL_ON_JOB_CLOSE`. Dropping or force-killing a session terminates the worker
and its descendants. An orderly stop requires the matching
`session.shutdown` acknowledgement and process exit before the caller's
deadline.

The worker runs with the current user's authority. Environment clearing,
package validation, the Named Pipe boundary, and the Job Object are lifecycle
and integrity controls; they are not a security sandbox for trusted Codec or
Deck code.

## Bootstrap and authenticated hello

Core generates a separate 32-byte cryptographic token for each session. It
writes one closed named-MessagePack bootstrap record to the child's piped
stdin, then closes stdin. The record is framed with a four-byte little-endian
payload length, is at most 4096 bytes, and contains exactly:

- `bootstrap_version: 2`;
- `protocol_version: 2`;
- canonical non-nil `session_id`;
- the private pipe name;
- the token as exactly 64 lowercase hexadecimal characters.

The token never appears in arguments, environment variables, pipe names, or
normal diagnostics. Its Rust debug representation is redacted.

The first worker-to-host envelope must be an uncaused `worker.hello` event.
It contains the same token, the nonzero worker PID, bounded worker/runtime
identities, and `protocol_min = protocol_max = 2`. Any preceding frame,
repeated hello, wrong PID, wrong token, malformed frame, timeout, or early
process exit fails the session. Authentication failure does not reopen the
session and does not select Protocol 1.

## Framing and envelope

The Windows pipe wire format is:

```text
u32 little-endian named-MessagePack payload length
exactly one named-MessagePack envelope
```

The payload length is `1..=262144` bytes. Receivers reject an invalid length
before allocating the payload buffer and reject truncation, trailing bytes,
duplicate map fields, unknown schema fields, invalid canonical identities,
out-of-bound collections, and non-finite numeric controls. Maps use UTF-8 text
keys. Protocol UUIDs are canonical lowercase hyphenated strings; SHA-256
values are exactly 64 lowercase hexadecimal characters.

The Rust contract also provides bounded JSON encode/decode for conformance
corpora and diagnostics. JSON is not the Windows worker transport.

A representative envelope is:

```json
{
  "protocol": "latentdeck.worker",
  "protocol_version": 2,
  "session_id": "00000000-0000-4000-8000-000000000001",
  "sequence": 1,
  "message_id": "00000000-0000-4000-8000-000000000002",
  "sender_uptime_ns": 123,
  "message": {
    "kind": "command",
    "body": {
      "name": "session.status",
      "payload": {}
    }
  }
}
```

`kind` is `command`, `ack`, `error`, or `event`.

- Host-to-worker and worker-to-host sequences each start at one and increase
  by exactly one.
- Every message ID is non-nil and unique in its direction for the session.
- Every command receives one terminal acknowledgement or error.
- An acknowledgement or error names the same command and contains its exact
  `reply_to` message ID.
- An event may contain `caused_by`, but only for a command already known in
  that session.
- Each direction has a 65,536-message session budget; at most 256 command
  replies may be pending. The current Rust client is deliberately sequential
  and has only one command in flight.
- `sender_uptime_ns` is sender-local monotonic uptime, not a timestamp that can
  be compared between processes.

The host must rotate or stop a session while enough message budget remains for
controlled shutdown. It must not discover exhaustion by failing a realtime
command.

## Capabilities and ABI receipts

The closed capability set is:

| Capability         | Meaning                                        |
| ------------------ | ---------------------------------------------- |
| `player`           | open, step, reset, and query one Player source |
| `realtime`         | run a generic multi-source Deck                |
| `resample`         | serialize post-operator latent output          |
| `snapshot_capture` | capture a bounded Snapshot                     |
| `live_capture`     | capture a bounded live latent stream           |
| `raw_import`       | optional raw-media preflight and staged import |

A full Codec Pack v2 descriptor must advertise `player`, `realtime`,
`resample`, `snapshot_capture`, and `live_capture`. `raw_import` is optional.
The descriptor also binds the exact pack and adapter versions, host API `2.0`,
and up to 64 supported profile keys. Session configuration requests the
capabilities needed for that session; the acknowledgement must return them as
accepted capabilities.

Profile inspection is allocation-safe metadata discovery. Profile validation
returns a `ProfileReceipt` that Core cross-checks before `codec.load`. The
receipt binds:

- a unique receipt and exact cartridge/archive/payload identity;
- exact pack and adapter identity/version;
- `(codec_family, profile, profile_version)`;
- channels, latent and decoded dimensions, rational frame rate, and timing
  contract/version;
- tensor ABI, decoded ABI, capabilities, and host/device memory estimates.

The declared tensor ABI requires CPython 3.13, an exact declared Torch
version, a contiguous `[1,C,1,H,W]` tensor, an explicit CPU or CUDA device, and
one of `float16`, `bfloat16`, or `float32`. Its channel and spatial dimensions
must equal the signal receipt. Runtime tensors governed by that ABI must also
be finite. The decoded ABI is `rgba8` with a maximum batch in `1..=24`. Core
rejects identity, profile, signal, ABI, capability, or memory differences
before permitting GPU allocation.

## Status, events, and errors

Every acknowledgement includes both its command-specific payload and a common
status snapshot. Every error includes a common status snapshot. The snapshot
contains session, codec, Player, Deck, and capture states; the number of open
sessions; the optional foreground-output session; and whether that output
lease is pinned. `open_session_count` is bounded by four. A pinned lease must
name its foreground session.

Events are closed to:

| Event              | Purpose                                             |
| ------------------ | --------------------------------------------------- |
| `worker.hello`     | first authenticated worker description              |
| `status.changed`   | durable state transition                            |
| `worker.heartbeat` | liveness and current common status                  |
| `worker.fault`     | asynchronous stable failure and diagnostic identity |

The heartbeat hard timeout applies while a command is pending. Idle time before
the next command does not consume that command's heartbeat window.

An error reply contains `reply_to`, the command name, and:

- stable code;
- bounded message;
- `retryable` and `fatal` flags;
- common status snapshot;
- non-nil diagnostic UUID;
- up to 16 unique bounded key/value details.

Stable Protocol 2 error codes are:

```text
protocol.invalid_message       protocol.unsupported_version
protocol.bound_exceeded        session.not_configured
session.capacity_exceeded      session.output_lease_busy
session.output_lease_pinned    codec.not_loaded
codec.untrusted                codec.capability_unsupported
profile.invalid                profile.incompatible
source.invalid                 source.not_loaded
deck.invalid                   deck.incompatible
capture.invalid_state          capture.not_ready
capture.limit_exceeded         state.busy
worker.internal
```

## Closed command set

Every acknowledgement uses the same name as its command.

| Command                | Purpose / acknowledgement                                                                                                            |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| `session.configure`    | Select version 2, heartbeat/frame/in-flight bounds, and requested capabilities; returns negotiated bounds and accepted capabilities. |
| `session.status`       | Return the common status snapshot.                                                                                                   |
| `session.shutdown`     | Request orderly shutdown for `user_request`, `host_exit`, or `protocol_fault`; echoes the reason before exit.                        |
| `codec.descriptor`     | Describe one exact pack/adapter, host API, capabilities, and profiles without loading model assets.                                  |
| `codec.load`           | Load the exact adapter/device and explicit external assets after receipt validation; returns the exact loaded identity.              |
| `codec.unload`         | Unload the exact pack version; returns that identity.                                                                                |
| `source.open`          | Register a retained validated LC handle and integrity receipt; returns exact source/cartridge/archive identity.                      |
| `source.close`         | Close one exact source ID.                                                                                                           |
| `ring.configure`       | Bind duplicated mapping and notification handles; returns exact ring kind and geometry.                                              |
| `ring.release`         | Release one exact ring ID.                                                                                                           |
| `profile.inspect`      | Return payload hash, profile key, and signal geometry for an opened source.                                                          |
| `profile.validate`     | Validate expected profile/capabilities and return the exact `ProfileReceipt`.                                                        |
| `raw_import.preflight` | Inspect one bounded absolute raw source and return a typed source/manifest receipt.                                                  |
| `raw_import.stage`     | Stage the payload for an exact preflight receipt below a host-owned root.                                                            |
| `raw_import.abort`     | Abort and clean one exact preflight/stage identity.                                                                                  |
| `player.open`          | Open one source/receipt as a Player session and return Player status.                                                                |
| `player.step`          | Decode at most 24 frames and return Player status plus optional output-ring publication.                                             |
| `player.reset`         | Move to a strictly newer stream generation and return reset Player status.                                                           |
| `player.status`        | Return current Player status.                                                                                                        |
| `deck.load`            | Load one exact generic Deck runtime, sources, roles, controls, seed, and generation; returns full Deck status.                       |
| `deck.process`         | Process one latent slot, capture before decode when active, publish decoded output, and return full status/ring/provenance.          |
| `deck.controls.set`    | Atomically replace the closed control set; returns full Deck status.                                                                 |
| `deck.roles.set`       | Atomically replace logical role bindings; returns full Deck status.                                                                  |
| `deck.transport.set`   | Atomically replace play/pause and loop state for every physical source; returns full Deck status.                                    |
| `deck.seed.set`        | Replace the deterministic `u64` seed; returns full Deck status.                                                                      |
| `deck.reset`           | Apply a host-chosen newer generation and explicit playhead-preservation policy; returns full Deck status.                            |
| `deck.restart`         | Restart an exact Deck revision; the worker advances revision/generation and clears playheads/history.                                |
| `deck.status`          | Return full Deck status.                                                                                                             |
| `capture.start`        | Start bounded Snapshot or Live Capture below a host-owned staging root; returns capture status.                                      |
| `capture.stop`         | Stop/finalize the exact Live Capture identity; returns capture status.                                                               |
| `capture.status`       | Return the exact active/terminal capture status.                                                                                     |
| `metrics.get`          | Return cumulative bounded worker counters.                                                                                           |

There are no `deck.d2.*` or `deck.q4.*` Protocol 2 commands.

## Common startup order

Core validates and selects exact enabled Codec and Deck package versions before
launch. A production startup then follows this order:

```text
worker.hello
session.configure
codec.descriptor
for each source:
  source.open
  profile.inspect
  profile.validate
codec.load
ring.configure
player.open OR deck.load
```

Generic Deck startup follows `deck.load` with `deck.transport.set` so every
physical source receives the exact host transport intent. The worker cannot
change package identity, source identity, profile, ABI, or external assets
during that session. Source replacement creates a bounded replacement session;
it is not an in-place path swap.

Teardown may explicitly close sources, release the ring, and unload the codec,
but process ownership remains the final cleanup boundary.

## Player lifecycle

`player.open` binds one physical source and its profile receipt to a nonzero
stream generation. `player.step` carries only the Player session identity,
current generation, and a requested maximum decoded batch in `1..=24`.
Its acknowledgement reports authoritative state, generation, sequence,
playhead, EOS, decoded ring, output sequence, and decoded frame count. A
positive decoded count requires a ring ID.

The worker never wraps a causal Player stream invisibly. At EOS it reports
`end_of_stream`. If host transport requests looping, Core performs an explicit
generation-increasing `player.reset` before decoding again. Decoder state and
ring generation therefore change at one visible boundary.

## Generic Deck lifecycle

`deck.load` supports one to 16 physical sources. Production loads include an
exact `DeckRuntimeBinding` derived from an enabled `.ld` usage lease: Deck and
operator identities, canonical Python root, `module:callable` entrypoint, and
the package-manifest and integrity-catalog hashes. The optional wire form is
retained for injected conformance runtimes; production installed Decks require
the hash-bound binding.

Core validates source count, physical slots, logical roles, and typed controls
against the exact Deck operator descriptor. Role reassignment never moves
physical playheads or previous-source history. Control values are closed to
boolean, signed integer, finite number, or bounded text.

`deck.process` is one ordered processing tick:

1. read the current slot for every physical source through its opened handle;
2. call the selected Deck operator with controls, roles, playheads, generation,
   sequence, seed, and physical-slot history;
3. validate the finite, shape/dtype/device-preserving post-operator tensor;
4. append that tensor to an active capture writer before decode;
5. decode and publish one complete RGBA batch through Ring ABI 2;
6. return the full authoritative `DeckStatusSnapshot` and bounded typed
   provenance.

The acknowledgement preserves Deck identity/revision/generation and advances
the stream sequence exactly once. Controls, roles, and seed cannot change as a
side effect. The host validates the returned snapshot rather than trusting a
partial delta.

Playheads and transport are physical-slot scoped. A looping source may wrap
from its exact final slot to zero; Core then performs a strictly newer
`deck.reset` with `preserve_playheads=true` so decoder/ring/previous-source
state crosses an explicit causal boundary. A non-looping source reaching its
exact final slot changes only its own `playing` flag to false, retains its last
playhead, and reports EOS. When all sources settle, the Deck becomes paused but
the warm session remains valid. Stopping before exact EOS or changing unrelated
state in a process acknowledgement is a protocol fault.

`deck.controls.set`, `deck.roles.set`, `deck.transport.set`, and
`deck.seed.set` are sequential atomic replacements. Each returns a complete
snapshot; the host accepts only the requested mutation. Resuming or pausing a
source after EOS does not recreate the session. The host scheduler stops
recurring `deck.process` calls when every source is paused and must not
busy-spin.

`deck.reset` requires a strictly newer nonzero generation, resets decoder and
ring generation, clears stream sequence and previous-source history, and
either preserves or zeroes playheads exactly as requested. `deck.restart` is
not a synonym: outside active capture it advances the Deck revision and
generation, clears all playheads/history, and returns the restarted full
snapshot.

## Capture and resample lifecycle

`capture.start` carries the exact Deck revision and capture UUID, mode,
absolute host-owned staging root, and explicit limits. Protocol maxima are
1,048,576 latent slots, 15 GiB of visual tensor data, and 32 reset events. A
product may choose smaller values.

Capture receives the post-operator tensor before decode. Snapshot asks the
Codec writer to finish at the first profile-valid boundary. Live Capture keeps
appending until `capture.stop`, then remains `finalizing` until the writer
reaches a profile-valid boundary. Normal source-loop resets may be recorded as
bounded reset events while capture continues. A manual restart is rejected
during active capture.

A capture acknowledgement never carries tensor bytes. Before completion it
contains identity, mode, state, latent-slot count, and reset-event count. Only
`completed` may contain an artifact, and `completed` must contain one. The
artifact is a staged payload path, lowercase payload SHA-256, byte length,
latent-slot count, and decoded-frame count.

The staged path remains untrusted control data. Core binds it to the expected
capture-owned staging root/name, checks the receipt and limits, constructs the
codec-neutral LC manifest and genealogy, writes with no-clobber semantics,
reopens and fully validates the resulting `.lc`, then imports it into the
Library. The adapter never writes directly into the Library. Abort, worker
fault, or finalization failure removes capture-owned partial state.

## Optional raw import lifecycle

Raw import is available only when the descriptor declares `raw_import`.
`raw_import.preflight` accepts a non-nil import UUID, absolute source path, and
a source-size limit no larger than 64 GiB. It returns an exact source hash and
length plus bounded typed metadata: profile, safe payload entry, one visual
tensor, at most one matching audio tensor, storage/runtime dtypes, shapes,
timing, decoded geometry, duration, and audio policy.

`raw_import.stage` is valid only for the exact preflight receipt and a
host-owned absolute staging root. It returns a staged payload path/hash/length;
`raw_import.abort` cleans that identity. As with capture, Core constructs and
revalidates the final LC container. A Codec Pack may not return a ready Library
cartridge or silently replace the source.

## Protocol 1 Player compatibility bridge

Protocol 1 remains at the crate root for the accepted legacy H3 Player path.
Its closed command names are:

```text
session.configure    codec.inspect      codec.load
slot.load            slot.reset         slot.decode_cycle
ring.bind            worker.status      metrics.get
worker.shutdown
```

Protocol 1 no longer accepts any Deck command. In particular, legacy
`deck.d2.*` and `deck.q4.*` names fail schema decoding. It has no generic Deck,
capture, `.ld` runtime, retained-handle source, or raw-import contract.

The Player runtime has two explicit selections only: the Protocol 1 H3 bridge
or Protocol 2. There is no `auto` selection. Selecting Protocol 1 invokes only
the legacy bridge. Selecting Protocol 2 invokes only Protocol 2; handshake,
capability, profile, runtime, or decode failure is surfaced and never falls
back to Protocol 1.

Protocol 1 and Protocol 2 have distinct typed command, acknowledgement, event,
error, and bootstrap schemas even where their framing and Windows supervision
principles are similar. Implementers must not mix their payloads or infer one
version from the other's command names.

## Conformance requirements

A conforming implementation must exercise both JSON and named-MessagePack
round trips for the typed Protocol 2 corpus, and the actual worker transport
must use length-prefixed named MessagePack. At minimum, tests must cover:

- all 32 command names and matching acknowledgement names;
- cross-language Rust/Python fixtures;
- unknown/duplicate fields, trailing bytes, bad UUID/hash text, non-finite
  controls, and every collection/frame bound;
- first-frame hello, token/PID/session checks, contiguous sequences, duplicate
  IDs, reply correlation, heartbeat timeout, process crash, and Job cleanup;
- retained-handle source transfer with no cartridge/tensor/RGBA bytes in a
  control frame;
- exact descriptor and ProfileReceipt cross-check before `codec.load`;
- Player step/EOS/reset and explicit no-fallback bridge selection;
- one-, two-, four-, and sixteen-source Deck scheduling, role changes,
  independent loops/EOS, generation resets, and exact mutation snapshots;
- Snapshot/Live Capture bounds, loop reset events, staged artifact validation,
  abort cleanup, and replay of the finalized cartridge;
- a synthetic non-H3 Codec Pack and external `.ld` Deck, so the contract is
  proved independently of H3, D2, and Q4.

The authoritative wire types are the Protocol 2 module of
`latentdeck-control`; the Python Codec SDK and generic worker runtime must stay
byte-for-byte compatible with its closed representations.
