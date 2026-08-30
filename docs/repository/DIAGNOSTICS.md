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

## Create a sanitized bundle

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

## Privacy and sharing boundary

A bundle contains no Library database, `.lc` archive, raw latent tensor,
preview, model weight, decoder asset, user media, prompt, absolute path, raw
exception detail, environment variable, credential, or process identifier.
The manifest states these exclusions explicitly.

Still inspect a bundle before sending it outside the machine. Creating a bundle
does not authorize uploading it, attaching it to an issue, or publishing it.
Those are separate owner decisions.

When applications and workers are stopped, local logs and already-created
bundles may be deleted normally. The exporter never deletes source logs and
never includes the application database backup.

## Contract test

The public test uses synthetic temporary records, injects path-like and secret
content, and verifies bounds, redaction, exact archive layout, no-overwrite
behavior, and temporary-file cleanup:

```powershell
pwsh -NoProfile -File tools/Test-DiagnosticBundle.ps1
```
