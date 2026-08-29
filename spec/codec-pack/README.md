# LatentDeck Codec Pack contract 0.1

Status: runtime installation contract for LatentDeck `0.1.x`.

A Codec Pack is an independently installed, integrity-checked runtime adapter.
It is not a cartridge and is never discovered by scanning arbitrary folders.
The application checks only these exact roots:

- `%LOCALAPPDATA%\LatentDeck\CodecPacks`;
- `%PROGRAMDATA%\LatentDeck\CodecPacks`.

Each installed version has the layout
`<root>/<pack_id>/<pack_version>/codec-pack.json`. Missing roots mean “not
installed.” A malformed, conflicting, linked, or corrupt candidate is an
explicit error; it is not silently skipped in favour of another copy.

## Stable boundaries

The pack contains the isolated worker runtime and trusted codec adapter. It
MUST NOT contain an H3 Transformer, native HQ VAE, user cartridge, or selected
`taeh3` weight. Model assets remain external and require an explicit user file
selection.

The application launches only the manifest's integrity-checked executable,
without a command shell. The worker receives a one-use authentication token
and private Named Pipe address through inherited standard input. Tensor and RGB
bytes never travel through that control channel.

A cartridge cannot install or select a Codec Pack, adapter, model asset, or
operator.

## Installation manifest

`codec-pack.json` is a strict JSON object. Unknown fields are rejected. It
binds:

- manifest, pack, and adapter versions;
- publisher and license notice metadata;
- Windows x86-64 platform compatibility;
- supported application, Worker Protocol, LC, and codec-profile versions;
- one direct worker executable, bounded Player argument list, optional bounded
  D2 argument list, working directory, and probe timeout;
- a SHA-256-bound integrity catalog;
- required external assets and their accepted byte length/SHA-256 variants.

Pack and adapter identifiers are lowercase dotted identifiers. Versions are
canonical SemVer. All pack-internal paths use `/`, contain only relative normal
components, and resolve within the version directory.

The executable is shared only as an integrity-checked process boundary. Its
entrypoints are explicit and are never inferred from one another:

```json
{
  "worker": {
    "executable": "runtime/python.exe",
    "arguments": ["-s", "-m", "latentdeck_codec_h3.worker"],
    "d2_arguments": ["-s", "-m", "latentdeck_codec_h3.d2_worker"],
    "working_directory": "runtime",
    "probe_timeout_ms": 120000
  }
}
```

`d2_arguments` is optional so a validated Player-only pack remains usable.
When it is absent, LatentDeck reports the D2 worker as unavailable; it never
silently starts the Player entrypoint as a Deck worker. When present, the list
must be non-empty and obey the same count and bounded-text rules as
`arguments`. Neither entrypoint is accepted from UI state or a cartridge.

The integrity catalog is a strict object:

```json
{
  "manifest_version": "1.0.0",
  "files": [
    {
      "path": "bin/latentdeck-h3-worker.exe",
      "byte_length": 123456,
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ]
}
```

Every catalogued file is measured before launch. Required worker and license
notice files must be present in the catalog. Duplicate catalog paths, size or
hash mismatches, oversized JSON, and more than the bounded pack/file counts are
errors.

## Filesystem safety

Discovery rejects symlinks, junctions, and other Windows reparse points at the
root, pack, version, and every intermediate component of a referenced path.
Canonical containment remains a second check. Existing paths are never treated
as shell fragments.

The worker independently rechecks the exact `.lc` archive identity and selected
external asset immediately before use. This closes the trust boundary between
the UI-side validation decision and GPU allocation.

## External decoder assets

An external asset declaration identifies accepted bytes; it does not download,
copy, license, or select them. For example, an H3 pack may declare one required
`taeh3` Safetensors asset with one or more accepted variants. The Codec Manager
must show the selected variant's source URL, license label/URL, SHA-256, size,
and compatibility before enabling it.

LatentDeck validates the explicitly selected regular file against an accepted
size and SHA-256 pair. Reparse points, missing files, and unknown bytes fail
closed. The selected local path is machine state and does not belong in Git,
presets, cartridges, logs, or diagnostics.

No manifest field authorizes a silent network request or acceptance of a
different checkpoint.

## Versioning

Codec Pack manifest and integrity-catalog versions are independently versioned
from the app, Worker Protocol, LC Spec, and H3 profile. LatentDeck `0.1.x`
implements manifest/catalog version `1.0.0` and Worker Protocol `1`.

An unsupported version or compatibility range is a visible incompatibility,
not an upgrade instruction. Installation, update, and removal remain separate
from the LatentDeck application lifecycle.
