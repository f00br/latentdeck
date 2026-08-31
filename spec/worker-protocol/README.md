# LatentDeck Worker Protocol 1

Status: normative for the local LatentDeck `0.1.0` worker boundary.

This document defines the typed control messages exchanged by Rust Core and an
isolated codec worker. The Rust implementation is `latentdeck-control`.
Operating-system transport, process supervision, decoded-frame shared memory,
codec algorithms, operators, and resampling are separate contracts.

## Stable boundary

Worker Protocol 1 carries commands, acknowledgements, bounded errors, and
events. It never carries latent tensors, decoded RGB frames, model weights, a
Python module, or executable cartridge content. File paths and duplicated
operating-system handle values are control descriptors only.

The protocol marker is `latentdeck.worker`; the exact protocol version is the
integer `1`. Unknown versions are not assumed compatible.

## Framing

Each message is encoded as:

```text
u32 little-endian MessagePack byte length
exactly one MessagePack envelope
```

The payload length is `1..=262144` bytes. A receiver rejects zero and oversized
lengths before allocating a payload buffer, rejects a truncated prefix or
payload, and rejects bytes after the first MessagePack object.

The format uses MessagePack maps with UTF-8 string keys. Duplicate keys,
unknown fields, invalid UTF-8, extension values, non-finite floats, and schema
coercion are invalid. Variable-length protocol arrays use type-specific bounds.
Protocol UUIDs are canonical lowercase hyphenated strings.

The generic Rust `Read`/`Write` framing is transport-independent. The Windows
release uses a local Named Pipe. Named Pipe creation, ACLs, connection handling,
and process supervision are implemented by `latentdeck-core`, not by
`latentdeck-control`.

## Windows transport and supervision

Core creates one random pipe name per worker session before spawning the
worker. The server is byte-mode, first-instance-only, rejects remote clients,
allows exactly one instance, and uses bounded 64 KiB input and output buffers.
Its protected DACL contains one full-access allow ACE for the current process
user. Default or inherited broad pipe permissions are not accepted.

The worker command is derived only from a fully integrity-checked
`ValidatedCodecPack`. Core launches that exact executable directly, without a
shell or PATH lookup, using only the bounded arguments and working directory
from the validated pack manifest. The child environment is cleared before
spawn; only `SystemRoot`, `SystemDrive`, `TEMP`, and `TMP` are copied for the
Windows and temporary-file runtime contract. Core additionally fixes
`PYTHONDONTWRITEBYTECODE=1`; it is not inherited from the parent and prevents a
Python worker from mutating an integrity-checked Codec Pack. In particular,
`PATH`, other Python and CUDA configuration, user-profile variables, and parent
credentials are not inherited. A self-contained Codec Pack must not depend on
them.

Core generates a separate 32-byte cryptographic authentication token. A single
bounded bootstrap record containing protocol version, session UUID, pipe name,
and token is written to the child's piped stdin and stdin is then closed. The
token is never placed in arguments, environment variables, pipe names, or log
messages. The first connected client must have the spawned worker PID and its
first envelope must be `worker.hello` with the same PID, session, and token.

The child is assigned to a Windows Job Object configured with
`KILL_ON_JOB_CLOSE` before bootstrap delivery. Dropping a pending/authenticated
session therefore terminates the worker and its descendants; explicit force
kill terminates the entire Job Object. Graceful shutdown requires the typed
`worker.shutdown` acknowledgement followed by process exit within a caller
deadline. Any crash, authentication failure, malformed frame, pipe failure, or
timeout ends that session. Protocol 1 never auto-resumes playback or reuses its
causal decoder state.

The small Win32 FFI surface for current-user security descriptors, pipe client
PID verification, and Job Objects is isolated in the Windows supervisor module.
Its contract tests inspect the exact one-ACE DACL and Job Object flag, then run
synthetic worker processes through authenticated shutdown, bad-token rejection,
early-exit observation, and explicit force-kill paths.

## Envelope

```json
{
  "protocol": "latentdeck.worker",
  "protocol_version": 1,
  "session_id": "00000000-0000-0000-0000-000000000001",
  "sequence": 1,
  "message_id": "00000000-0000-0000-0000-000000000002",
  "sender_uptime_ns": 123,
  "message": {
    "kind": "command",
    "body": {
      "name": "worker.status",
      "payload": {}
    }
  }
}
```

`kind` is `command`, `ack`, `error`, or `event`.

- Core sends commands; the worker sends acknowledgements, errors, and events.
- `sequence` starts at one and increases by exactly one independently for each
  sender.
- `message_id` is non-nil and unique within the bounded session.
- Every command receives exactly one terminal acknowledgement or error.
- An acknowledgement/error body contains `reply_to`. Its typed name must equal
  the referenced command name.
- An event may contain `caused_by`, which must reference a command known in the
  same session.
- A session is recreated before either its 65,536 inbound-envelope budget or
  its 65,536 outbound-command budget is exhausted. At most 256 command replies
  may be pending. A runtime MUST reserve enough budget for an orderly stop and
  MUST surface controlled session rotation before the cap, never discover the
  cap by failing a realtime command.

## Handshake and events

The first worker envelope is `worker.hello`. It carries the fixed 32-byte
session authentication token, worker/runtime identity, supported protocol
range, PID, and a bounded adapter list. The token has a redacted Rust `Debug`
representation.

Core replies with `session.configure`, fixing:

- protocol version `1`;
- heartbeat interval and hard timeout;
- maximum frame size `262144`;
- exactly one in-flight decode batch.

The heartbeat hard timeout applies while a command is pending. Time spent with
no pending command does not consume the next command's heartbeat window; a
client without a background event pump begins that window when it sends the
command and drains any queued authenticated events in order.

Worker events are:

| Name                   | Meaning                                   |
| ---------------------- | ----------------------------------------- |
| `worker.hello`         | first authenticated worker description    |
| `worker.heartbeat`     | lightweight liveness and component states |
| `worker.state_changed` | durable state transition plus reason      |
| `metrics.snapshot`     | bounded cumulative counters               |
| `worker.fault`         | stable error plus diagnostic ID           |

`sender_uptime_ns` is local monotonic uptime. Values from different processes
are not directly compared as timestamps.

## Commands and acknowledgements

| Command                 | Purpose                                                                      |
| ----------------------- | ---------------------------------------------------------------------------- |
| `session.configure`     | finish protocol negotiation                                                  |
| `codec.inspect`         | inspect adapters, CUDA, and devices without loading assets                   |
| `codec.load`            | load one installed pack/adapter and explicit external assets                 |
| `slot.load`             | revalidate and load one exact cartridge into a Player slot                   |
| `slot.reset`            | change generation and clear causal decoder state                             |
| `slot.decode_cycle`     | decode the next codec-owned timing cycle                                     |
| `ring.bind`             | bind a previously created RGB-ring mapping and notification handle           |
| `deck.d2.load`          | bind two exact H3 sources and initialize the trusted LD-D2 operator          |
| `deck.d2.process_slot`  | process one post-operator latent slot, decode it, and publish its RGB frames |
| `deck.d2.reset`         | apply a reported D2 reset barrier with a newer generation                    |
| `deck.d2.restart`       | request a restart barrier without clearing causal state implicitly           |
| `deck.d2.controls.set`  | atomically replace the closed realtime D2 control block                      |
| `deck.d2.transport.set` | atomically replace both A/B play and loop flags                              |
| `deck.d2.seed.set`      | replace the exact deterministic u53 seed                                     |
| `deck.d2.status`        | return the worker-owned D2 scheduler state                                   |
| `deck.q4.load`          | bind four exact H3 sources and initialize trusted LD-Q4                       |
| `deck.q4.process_slot`  | synthesize/decode one carrier-donor slot and publish its RGB frames           |
| `deck.q4.reset`         | apply a reported Q4 causal-reset barrier with a newer generation              |
| `deck.q4.restart`       | request a four-playhead restart barrier                                       |
| `deck.q4.controls.set`  | atomically replace the closed Q4 synthesis controls                           |
| `deck.q4.roles.set`     | atomically replace the explicit carrier/B/C/D role permutation                |
| `deck.q4.transport.set` | atomically replace all four play and loop pairs                               |
| `deck.q4.seed.set`      | replace the exact deterministic Q4 u53 seed                                   |
| `deck.q4.status`        | return the worker-owned Q4 scheduler state                                    |
| `deck.q4.capture.start` | arm bounded Snapshot or Live Capture at the next valid reset boundary         |
| `deck.q4.capture.stop`  | stop or arm stopping of the active Q4 capture                                 |
| `deck.q4.capture.status`| read the active Q4 capture state without mutation                             |
| `worker.status`         | return current worker/codec/slot/ring states                                 |
| `metrics.get`           | return a cumulative metrics snapshot                                         |
| `worker.shutdown`       | acknowledge orderly shutdown before process exit                             |

Every acknowledgement has the same typed name as its command. Long operations
acknowledge only after their state transition is complete. Progress and
liveness use events rather than an early acceptance acknowledgement.

## LD-D2 scheduler contract

The D2 worker is a separate Codec Pack entrypoint. It uses the same framing,
authentication, session negotiation, codec inspection/load, ring binding, and
shutdown rules as the Player worker. A valid Player-only pack may omit the D2
entrypoint; Core then reports D2 as unavailable and does not reuse the Player
command implicitly.

Only the trusted host sends D2 commands. The UI supplies cartridge UUID/hash
identities and realtime controls to the host API; it never supplies a local
path, `deck_id`, revision, generation, operator identity, processing tick, or
ring sequence. The application host resolves each source through the Library
index, performs full LC validation, and places the resulting local path in
`deck.d2.load`. The low-level `WorkerClient` accepts an already constructed
typed command and does not perform Library lookup. The worker reopens and
rehashes each exact archive before allocating its cartridge tensor on the GPU.

`deck.d2.load` contains:

- a bounded host-owned `deck_id`;
- the explicitly installed operator ID and version;
- A and B source bindings, each with local path, canonical cartridge UUID, and
  expected lowercase archive SHA-256;
- the complete closed controls and four-flag transport blocks;
- an integer seed in `0..=9007199254740991`;
- a nonzero initial `u64` stream generation.

The worker accepts compatible H3 sources with independent temporal lengths.
It rejects mismatched codec/profile/timing/runtime layout, frame rate, latent
spatial grid, or decoded geometry. It does not crop, resize, re-encode, select
a cheaper algorithm, or silently change dtype beyond the profile-authorized
F32-storage to F16-runtime cast.

The required single-session lifecycle is:

```text
session.configure -> codec.inspect -> codec.load -> deck.d2.load -> ring.bind
```

`deck.d2.load` is accepted once per worker process and returns the
worker-assigned nonzero `deck_revision`. The host retains that revision and
uses it in every later D2 command. Protocol 1 has no D2 unload command;
changing source cartridges or recovering from a failed deck load recreates the
isolated worker session.

The realtime control block has these exact finite ranges:

| Control                                   | Values or range                                                                           |
| ----------------------------------------- | ----------------------------------------------------------------------------------------- |
| `algorithm`                               | `LINEAR`, `XS1`, `XS2`, `XS3`, `XS4`, `XS5`                                               |
| `mix`, `interaction`, `preserve`, `chaos` | `0..=1`                                                                                   |
| `mode`                                    | `HYBRIDIZE`, `INTERACT`                                                                   |
| `routing`                                 | structural carrier `A` or `B`                                                             |
| `xs1_channel_a`, `xs1_channel_b`          | distinct channels in `0..=23`                                                             |
| `xs1_angle_degrees`                       | `-180..=180`                                                                              |
| `xs2_radius`                              | integer `1..=8`                                                                           |
| `xs3_high_gain`                           | `-2..=2`                                                                                  |
| `xs4_epsilon`                             | `0.00000001..=0.001`                                                                      |
| `xs5_routing`                             | `TOPK` or `SINKHORN`                                                                      |
| `temperature`                             | `0.02..=1`                                                                                |
| `top_k`                                   | integer `1..=64`; the worker additionally rejects values larger than the loaded full grid |
| `sinkhorn_iterations`                     | integer `2..=12`                                                                          |

`deck.d2.process_slot` carries only the current deck identity/revision and
generation. Its acknowledgement is exactly one of:

- `decoded_slot`: playheads, the authoritative four-flag transport, stream
  sequence, decoded frame range, the exact half-open RGB-ring sequence range,
  and bounded JSON-object provenance;
- `reset_barrier`: current/minimum-new generations and one or both loop or
  restart reasons;
- `paused`: current generation, playheads, and the authoritative four-flag
  transport, with no decode or publication.

For `decoded_slot`, the half-open ring range length must equal the reported
decoded frame count, which is `1..=4`. Provenance is limited to 32 KiB and
must parse as a JSON object; malformed JSON, scalar, array, null, and
non-finite numeric forms are rejected. The F16 latent itself is not returned;
it reaches decode inside the worker. When an explicit capture is armed, the
same post-operator slot also reaches the separate bounded resample sink before
decode. Tensor and RGB bytes never enter capture control replies.

`D2Status.stream_sequence` is the next slot sequence expected by the host. The
first decoded slot in a generation therefore acknowledges sequence `0`; after
accepting it the host advances its expected sequence to `1`. While the worker
publishes a decoded batch, the RGB consumer sequence must remain unchanged and
occupancy/capacity must change by exactly the acknowledged frame count.

The transport returned by `decoded_slot` and `paused` is authoritative. In
particular, a non-looping source reaching EOS clears only its own `playing_*`
flag; when both sources settle, `paused` carries both flags cleared. The host
adopts these values rather than inferring EOS from stale UI state.

Loop and Restart are two-step state transitions. A barrier never clears state
by itself. Core chooses a strictly newer nonzero generation and sends
`deck.d2.reset`; only an acknowledgement with `causal_state_cleared=true` may
resume processing. Before that acknowledgement, a successful reset clears
decoder history and operator history, applies the barrier's required playhead
changes, resets decoded-frame progress, and changes ring generation. A failed
reset leaves the barrier active for explicit retry or worker termination.

Controls, transport, and seed updates are atomic and report
`requires_causal_reset=false`. The application may enqueue updates while a
slot is in flight, but the sequential worker client can apply them only between
complete commands. The host scheduler MUST stop recurring process requests
when both playheads are paused and MUST NOT busy-spin.

### D2 post-operator capture

Protocol 1 defines three closed capture commands:

- `deck.d2.capture.start`: deck identity/revision, canonical non-nil capture
  UUID, `snapshot` or `live_capture`, an existing absolute host-owned temporary
  root, and explicit latent-slot/visual-byte ceilings;
- `deck.d2.capture.stop`: deck identity/revision and capture UUID;
- `deck.d2.capture.status`: the same exact identity block, without mutation.

The protocol ceilings are 1,048,576 latent slots and 15 GiB of visual tensor
data. An application SHOULD choose smaller product limits. The worker creates
only capture-ID-derived files below the supplied root. The host treats every
returned path as untrusted control data and MUST bind it to the exact expected
root and capture-owned filename before packaging.

Start does not write immediately. It requests a normal restart barrier and
returns `awaiting_reset`; capture becomes `capturing` only after the existing
two-step reset handshake reaches a newer generation with both playheads at
zero. Transport is locked through this boundary and throughout capture.
Snapshot also freezes controls and seed. Live Capture permits bounded
between-slot controls/seed changes and records at most 32 ordered events.

The post-operator F16 slot is appended before causal decode. A slot is kept
only if decode and RGB publication also succeed; a failure aborts and removes
capture-owned partials. Snapshot ends automatically after one complete
structural-carrier cycle. Live Capture stops immediately when the accumulated
temporal length is already codec-valid (`T = 2 + 5n`), otherwise it enters
`stop_armed` and finishes at the first later valid boundary. There is no hidden
crop, stretch, padding, or RGB fallback.

For Live Capture the worker derives the last codec-valid boundary that fits
both host ceilings. If the user has not stopped capture by that boundary, the
worker finishes automatically after that slot decodes successfully; it never
turns an ordinary bounded-spool limit into a fatal Deck failure. A limit set
that cannot contain at least `T=2` is rejected before capture starts.

An input cartridge cannot cross a causal decoder generation, but an active Live
Capture may retain its post-operator spool across a normal automatic loop
barrier whose reasons are exclusively `slot_*.loop`. The ordinary two-step Deck
reset must return a strictly newer generation and a valid structural-carrier
playhead before capture resumes. Restart, recovery, manual or non-loop reset,
and an invalid reset mapping still abort Live Capture explicitly; Snapshot does
not gain this loop-crossing exception.

Capture status is one of `awaiting_reset`, `capturing`, `stop_armed`,
`finished`, or `aborted`, with state-specific generation, target, stop-boundary,
reason, and receipt fields. A finished receipt is bounded to 32 KiB and binds:

- capture ID/mode, exact payload path, SHA-256, byte length, F16 dtype,
  `[1,24,T,H,W]` shape, and decoded frame count;
- structural carrier and exact A/B cartridge UUID/hash parents;
- Snapshot frozen controls/seed or Live Capture control-event history;
- one explicit audio policy: `source_absent`,
  `copied_from_carrier_exact`, or `omitted_timing_mismatch` with
  `duration_mismatch`, `temporal_mapping_mismatch`, or
  `duration_and_mapping_mismatch`.

Copied audio is allowed only for an exact structural-carrier duration and
temporal mapping. The application finalizer revalidates the spool, constructs
genealogy, writes and validates a same-directory `.lc.partial`, atomically
commits the `.lc`, and imports it into Library. A successful finalizer consumes
the exact spool. Active partials and unconsumed finished spools are removed on
orderly worker close or replacement; after forced process termination, cleanup
of the trusted temporary root belongs to the application.

## LD-Q4 carrier-donor contract

Q4 uses a separate explicitly declared Codec Pack entrypoint and the same
authenticated framing, session, ring, reset, recovery, and shutdown invariants
as D2. It never substitutes the Player or D2 entrypoint. The UI sends four
Library identities/hashes; the trusted host resolves and fully validates the
paths before constructing the low-level `deck.q4.load` command.

Physical slots are always `A`, `B`, `C`, and `D`. A closed `roles` object maps
one physical slot to `carrier` and the remaining slots to logical `donor_b`,
`donor_c`, and `donor_d`. The four values must be an exact permutation. This
assignment is explicit in load/status/process acknowledgements, may be replaced
atomically with `deck.q4.roles.set`, and is recorded in resample provenance.
No positional or UI-order inference is allowed.

All four sources must match codec/profile/timing, frame rate, latent spatial
grid, runtime layout, and decoded geometry. Their temporal lengths may differ
and retain four independent playheads. The eight transport flags control each
physical slot independently. Any looping slot or Restart produces a reported
barrier; only `deck.q4.reset` with a newer generation and
`causal_state_cleared=true` may resume decode. A non-looping EOS clears only
that physical slot's `playing_*` flag.

The Q4 control block is closed and finite:

| Control | Values or range |
| --- | --- |
| `algorithm` | `LINEAR` or `XS5` |
| `interaction`, `preserve`, `chaos` | `0..=1`; chaos `0` is the exact unperturbed path |
| `mode` | `HYBRIDIZE` or `INTERACT` |
| `influence_mode` | `MANUAL` or `TRIANGLE` |
| `donor_weight_b/c/d` | each `0..=1`; at least one positive in manual mode |
| `triangle_x/y` | `0..=1` and inside the B/C/D barycentric field |
| `xs5_routing` | `TOPK` or `SINKHORN` |
| `temperature` | `0.02..=1` |
| `top_k` | integer `1..=64`, additionally bounded by the full loaded grid |
| `sinkhorn_iterations` | integer `2..=12` |

The global `interaction` value is total donor strength. Manual or triangular
weights are normalized only to distribute that total among logical donors.
Each donor affinity is computed against the same unchanged carrier state, then
the routed states accumulate in fixed logical B/C/D order. Implementations may
batch or reuse carrier affinity, but may not downscale the grid, omit a donor,
drop to RGB, or silently change the algorithm to meet a frame-rate target.
Seeded chaos and identical input/control/event sequences must be deterministic.

`deck.q4.process_slot` returns `decoded_slot`, `reset_barrier`, or `paused` with
the same ring-range and bounded provenance rules as D2, extended to four
playheads and explicit roles. Controls, roles, transport, and seed updates are
applied only between complete slot commands and acknowledge
`requires_causal_reset=false`.

Q4 capture uses the same bounded post-operator-before-decode spool and valid H3
stop boundaries as D2. Snapshot freezes roles, controls, seed, and one complete
structural-carrier cycle. Live Capture records at most 32 ordered events, each
binding a latent-slot offset to roles, controls, and seed. The receipt binds all
four UUID/hash parents and the structural carrier. Audio is copied only from
that carrier when duration and temporal mapping match exactly; otherwise the
receipt declares the explicit omission policy and reason. The host revalidates
the worker path, Safetensors payload, hash, dtype/shape, genealogy, and final
`.lc` before atomic commit and Library import.

## H3 slot timing

The worker owns H3 cadence. Core requests only a sequential `cycle_index`; it
does not send latent/frame range math back to the adapter.

For a complete H3 clip `T = 2 + 5n`:

```text
cycle 0: 2 latent slots -> 5 decoded frames
cycles 1..n: 5 latent slots -> 17 decoded frames each
total cycles: 1 + n
total frames: 5 + 17n
```

The timing acknowledgement reports the exact latent and decoded range plus the
half-open RGB ring sequence range. Pause is represented by Core sending no new
cycle. Loop, Restart, and recovery use a strictly newer stream generation and
`slot.reset`; arbitrary seek has no command in version 1.

## Ring binding

`ring.bind` carries only:

- layout version `1`;
- a nonzero file-mapping handle valid in the worker process;
- mapping size from 4096 bytes through 256 MiB;
- a nonzero frames-ready event handle;
- a non-nil ring UUID.

It carries no mapped bytes. Header/slot layout, atomic publication, pixel
format, and backpressure are defined by the separate RGB ring contract and are
validated by the runtime before acknowledgement.

## Error shape

An error reply contains:

```text
reply_to
command name
stable error code
bounded human message
retryable / fatal flags
worker state
diagnostic UUID
up to 16 bounded key/value details
```

Stable code namespaces are `protocol.*`, `state.*`, `codec.*`, `slot.*`,
`decode.*`, `ring.*`, `capture.*`, and `worker.*`. Program logic switches on
the code, never on the human message. Stack traces, authentication tokens,
tensor bytes, and RGB bytes are not error details.

## Conformance tests

Public tests use only synthetic messages and in-memory byte streams. They cover
round trips, clean EOF, zero/oversized/truncated frames, trailing objects,
unknown envelope and nested payload fields, unsupported versions, wrong
sessions, sequence gaps, duplicate command IDs, unmatched replies, mismatched
reply names, directionality, fixed authentication-token length, and bounded
arrays. No cartridge, latent, weight, or local machine path is a fixture.
