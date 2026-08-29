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
Windows and temporary-file runtime contract. In particular, `PATH`, Python and
CUDA configuration, user-profile variables, and parent credentials are not
inherited. A self-contained Codec Pack must not depend on them.

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
- A session is recreated before exceeding 65,536 inbound messages. At most 256
  command replies may be pending.

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

Worker events are:

| Name | Meaning |
| --- | --- |
| `worker.hello` | first authenticated worker description |
| `worker.heartbeat` | lightweight liveness and component states |
| `worker.state_changed` | durable state transition plus reason |
| `metrics.snapshot` | bounded cumulative counters |
| `worker.fault` | stable error plus diagnostic ID |

`sender_uptime_ns` is local monotonic uptime. Values from different processes
are not directly compared as timestamps.

## Commands and acknowledgements

| Command | Purpose |
| --- | --- |
| `session.configure` | finish protocol negotiation |
| `codec.inspect` | inspect adapters, CUDA, and devices without loading assets |
| `codec.load` | load one installed pack/adapter and explicit external assets |
| `slot.load` | revalidate and load one exact cartridge into a Player slot |
| `slot.reset` | change generation and clear causal decoder state |
| `slot.decode_cycle` | decode the next codec-owned timing cycle |
| `ring.bind` | bind a previously created RGB-ring mapping and notification handle |
| `worker.status` | return current worker/codec/slot/ring states |
| `metrics.get` | return a cumulative metrics snapshot |
| `worker.shutdown` | acknowledge orderly shutdown before process exit |

Every acknowledgement has the same typed name as its command. Long operations
acknowledge only after their state transition is complete. Progress and
liveness use events rather than an early acceptance acknowledgement.

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
`decode.*`, `ring.*`, and `worker.*`. Program logic switches on the code, never
on the human message. Stack traces, authentication tokens, tensor bytes, and
RGB bytes are not error details.

## Conformance tests

Public tests use only synthetic messages and in-memory byte streams. They cover
round trips, clean EOF, zero/oversized/truncated frames, trailing objects,
unknown envelope and nested payload fields, unsupported versions, wrong
sessions, sequence gaps, duplicate command IDs, unmatched replies, mismatched
reply names, directionality, fixed authentication-token length, and bounded
arrays. No cartridge, latent, weight, or local machine path is a fixture.
