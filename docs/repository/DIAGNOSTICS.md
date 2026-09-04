# Diagnostics and support bundles

LatentDeck diagnostics are deliberately small, structured, and path-free. They
exist to identify lifecycle and stable error-code transitions without copying
cartridges, model assets, user media, the Library database, or arbitrary
exception text.

## Local logs

LatentDeck App and LatentPlayer each write JSON Lines records under their Tauri
local application-data directory:

```text
%LOCALAPPDATA%\studio.latentdeck.deck\logs
%LOCALAPPDATA%\studio.latentdeck.player\logs
```

Each process log is capped at 8 MiB. A product retains at most 16 matching log
files. A record contains only:

- schema version;
- Unix timestamp;
- `info`, `warn`, or `error` level;
- a bounded event token;
- an optional bounded stable error-code token.

The isolated H3 worker writes the same kind of closed diagnostic evidence to:

```text
%TEMP%\LatentDeck\worker-diagnostics
```

Each worker file is capped at 1 MiB and the directory retains at most 16 worker
files. Worker exception messages and backend diagnostic details are not
recorded because they can contain machine paths or payload names.

Logs are best-effort evidence. Failure to initialize or append a log never
changes playback or synthesis behavior.

## Create a lifecycle-only bundle from a checkout

The checkout script exports bounded lifecycle events only. It does not inspect
an active Player or Deck runtime and it does not create `realtime.json`.

From a source checkout, run:

```powershell
pwsh -NoProfile -File tools/New-DiagnosticBundle.ps1
```

The default output is a new timestamped ZIP under `artifacts/diagnostics/`.
The command refuses to overwrite an existing file and publishes the result by
an atomic rename only after verifying its exact two-entry layout:

```text
manifest.json
events.jsonl
```

The exporter parses each source record and writes a new allowlisted record. It
never copies a raw log file. Unknown schemas, malformed JSON, invalid tokens,
oversized files, oversized records, reparse-point files, and records beyond the
configured limits are dropped or skipped and counted in the manifest.

Default export limits are:

- 48 input files;
- 8 MiB per input file;
- 24 MiB total input;
- 65,536 accepted events;
- 4 KiB per source record.

Missing log directories are valid and produce an empty, still-valid bundle.
For tests or recovery work, the three roots and all limits can be overridden
through named parameters. Overrides do not weaken field allowlisting or archive
layout verification.

## Create a native realtime support bundle

LatentPlayer and LatentDeck App each expose a **Save diagnostics** button. Use
this path when the application is running and the support bundle should include
the current realtime state. The application opens a native save dialog; the
webview never supplies or receives the destination path.

The native command refuses to overwrite an existing file. It validates the
bounded snapshot and events, writes and syncs a temporary archive beside the
chosen destination, then publishes it through a same-directory hard link that
cannot replace an existing destination. The archive has exactly three entries:

```text
manifest.json
events.jsonl
realtime.json
```

`manifest.json` describes the bounded evidence and its exclusions.
`events.jsonl` contains only re-parsed allowlisted application and worker
lifecycle records. LatentPlayer reads only Player and worker roots; LatentDeck
reads only Deck and worker roots.

`realtime.json` uses the shared versioned realtime diagnostic schema:

- an active LatentPlayer session identifies the exact cartridge archive, GPU,
  Codec Pack, selected decoder, worker counters, and measured native
  presentation state;
- an active LatentDeck session contains an LD-D2 section, an LD-Q4 section, or
  both, with exact cartridge archive hashes and the Q4 carrier slot;
- D2 and Q4 may share one snapshot only when their GPU and codec identities are
  exactly equal; otherwise export fails with
  `diagnostics.session_identity_conflict` instead of combining unlike sessions;
- an application without an active realtime actor emits an explicit
  `no_active_session` form and may retain only the last stable lifecycle error
  code. It never invents realtime counters for an ended session.

Native presentation counts and frame intervals include only successful frame
presentations. Retryable surface skips are not reported as dropped frames.
Control latency remains a zero-sample distribution until the complete
control-to-visible-effect path is measured. Spout evidence is a bounded history
of stable error-code transitions, without SDK or driver exception text.

After save or cancellation, the UI receives only a closed result containing
status and, for a successful save, archive byte count, event count, and schema
version. The result contains no path or destination field. The save operation
is owned by the native command and does not require broad webview filesystem or
save-dialog permission.

## Privacy and sharing boundary

A bundle contains no Library database, `.lc` archive, raw latent tensor,
preview, model weight, decoder asset, user media, prompt, absolute path, raw
exception detail, environment variable, credential, or process identifier.
The manifest states these exclusions explicitly.

Still inspect a bundle before sending it outside the machine. Creating a bundle
does not authorize uploading it, attaching it to an issue, or publishing it.
Those are separate publication and disclosure decisions.

When applications and workers are stopped, local logs and already-created
bundles may be deleted normally. The exporter never deletes source logs and
never includes the application database backup.

## Contract tests

The checkout CLI test uses synthetic temporary records, injects path-like and
secret content, and verifies bounds, redaction, its exact two-entry layout,
no-overwrite behavior, and temporary-file cleanup:

```powershell
pwsh -NoProfile -File tools/Test-DiagnosticBundle.ps1
```

The native application tests separately verify the three-entry realtime
contract, lifecycle-only and active-session forms, exact D2/Q4 identity checks,
bounded Spout history, path-free save receipts, no-overwrite behavior, and the
least-privilege capability boundary:

```powershell
cargo test -p latentplayer-app
cargo test -p latentdeck-app
```
