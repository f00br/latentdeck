# LatentDeck Deck Package (`.ld`) v1

Status: normative for the LatentDeck `0.1.x` implementation.

A Deck Package defines a complete latent instrument: its source topology,
logical roles, exact signal requirements, typed controls, Python operator, and
host-rendered faceplate. Installing a new compatible `.ld` adds that Deck
without rebuilding LatentDeck.

The stable boundary is deliberately split:

- `.lc` cartridges carry codec-neutral media;
- `.ldcodec` packages supply codec adapters and isolated worker runtimes;
- `.ld` packages supply replaceable operators and declarative UI;
- the host owns trust, compatibility, sessions, output, capture, and Library
  integration.

The current contract uses `deck-pack.json` manifest version `1.0.0`,
`operator.json` schema/API version `0.2.0`, faceplate schema version `2`, and
Worker Protocol 2.

## Package tree

A Deck archive is a deterministic ZIP with the canonical lowercase `.ld`
extension. A typical root is:

```text
deck-pack.json
operator.json
faceplate.json
integrity.json
NOTICE.txt
python/<operator package>/__init__.py
python/<operator package>/operator.py
```

`deck-pack.json` and `integrity.json` are control files. Every other file,
including `operator.json`, `faceplate.json`, the license notice, and all Python
modules, MUST be listed in `integrity.json`. The catalog excludes the two
control files and closes the complete tree: extra files and empty directories
are invalid.

Only `.py`, `.json`, `.txt`, `.md`, and `.png` files are allowed. JSON is
strict; text and Python files must be UTF-8 without NUL; PNG files must have a
PNG signature. Native binaries, bytecode, wheels, HTML, JavaScript, CSS, and
nested archives are structurally forbidden.

This format restriction is not a Python security sandbox. An enabled Deck
operator is executable trusted code and receives only the bounded runtime
surface described here. Installation, verification, and explicit activation
are therefore mandatory trust boundaries.

## `deck-pack.json`

The manifest is a closed JSON object. Duplicate or unknown fields are rejected.

| Field                     | Contract                                                            |
| ------------------------- | ------------------------------------------------------------------- |
| `manifest_version`        | Exactly `1.0.0`.                                                    |
| `kind`                    | Exactly `deck_pack`.                                                |
| `deck_id`                 | Lowercase reverse-DNS identifier. `org.latentdeck.*` is reserved.   |
| `deck_version`            | Canonical lowercase SemVer and immutable storage identity.          |
| `display_name`, `summary` | Bounded user-facing text.                                           |
| `publisher`               | Name, optional HTTPS URL, and `identity_claim: self_declared`.      |
| `license`                 | License label and catalogued notice path.                           |
| `compatibility`           | Application, host API, protocol, and exact runtime requirements.    |
| `runtime`                 | Python operator runtime and entrypoint.                             |
| `signal`                  | Slots, roles, exact geometries, timing, profiles, and capabilities. |
| `faceplate_path`          | Exactly `faceplate.json` at the archive root.                       |
| `integrity`               | `catalog_path: integrity.json` and the catalog's SHA-256.           |

`compatibility` declares an inclusive minimum and exclusive maximum app
SemVer, nonzero numeric `deck_host_api`, `worker_protocol`, and
`deck_operator_api`, plus an explicit tensor ABI, CPython implementation and
version, platform tag, and exact Torch build. Wildcards and `any` runtime
identifiers are forbidden.

For the current host, bundled packages use Deck Host API `1`, Worker Protocol
`2`, Deck Operator API `1`, `latentdeck.tensor.v1`, CPython `3.13` on
`win_amd64`, and an exact Torch build supplied by a compatible Codec Package.

### Runtime declaration

The only runtime kind is `python_operator_stream_v1`.
`runtime.operator_descriptor_path` MUST be root `operator.json`.
`runtime.python_root` is a package-relative normal directory, and
`runtime.entrypoint` uses portable `module:callable` syntax. The declared module
must exist beneath `python_root` as a module file or package in the integrity
catalog.

Package paths, entrypoints, identities, and hashes are derived from the enabled
validated package. They cannot be supplied by a preset, cartridge, faceplate,
or dynamic load request.

### Signal declaration

`signal` defines:

- `slots`: one to 16 physical sources;
- one unique logical role per slot;
- `default_permutation`: every role exactly once, in physical-slot order;
- `structural_carrier_role`: one declared role that anchors structural output;
- one to 64 exact tensor geometries;
- one exact frame timing and `samples_per_slot` contract;
- a nonempty unique set of required codec capabilities;
- an optional allowlist of one to 64 exact profile keys.

Each geometry is an exact `[1,C,1,H,W]` contract with explicit dtype and device.
Batch and temporal extent are `1`; no dimension may be zero. Supported manifest
dtypes are `fp16` and `fp32`, and devices are `cpu` and `cuda`. A compatible
pair still has to be supported by the current host and Codec Package.

A profile key contains exact `codec_family`, `profile`, and canonical
`profile_version` values. `profile_allowlist: null` means the Deck adds no
profile-specific filter; it does not bypass runtime, geometry, timing,
capability, LC, or selected-source checks.

## `operator.json`

The operator descriptor is a closed JSON object with these fields:

- `schema_version`: exactly `0.2.0`;
- `deck_operator_api`: exactly `0.2.0`;
- `deck_id` and `deck_version`;
- independent `operator_id` and canonical `operator_version`;
- Python `entrypoint`;
- `source_count` in `1..=16`;
- ordered `role_ids`, one per source;
- zero to 64 typed `controls`.

The host cross-checks `deck_id`, `deck_version`, `entrypoint`, source count, and
role order against `deck-pack.json`. A mismatch invalidates the runtime even if
both files are independently well formed.

Each control has a unique `control_id`, `value_type`, and type-correct
`default`:

| Type      | Additional contract                                                        |
| --------- | -------------------------------------------------------------------------- |
| `boolean` | Boolean default; no options or numeric bounds.                             |
| `integer` | Exact JSON integer; optional integral minimum, maximum, and positive step. |
| `number`  | Finite JSON number; optional finite minimum, maximum, and positive step.   |
| `enum`    | One to 256 unique bounded options; default must be one option.             |
| `text`    | Bounded non-NUL default; no options or numeric bounds.                     |

The current faceplate schema has no text-input widget and requires every
operator control to be exposed exactly once. New UI-bearing Decks therefore
must not declare `text` controls until a corresponding host widget is added.

## Operator execution contract

The authoritative Deck SDK call is:

```python
process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: DeckOperatorContext,
) -> DeckOperatorResult
```

The source tuple contains one tensor per physical slot. All current and retained
previous-source tensors are finite, contiguous, and exactly equal in shape,
dtype, and device. Their shape is `[1,C,1,H,W]`. Controls are bounded scalar
JSON values validated against `operator.json`.

The context supplies exact codec/profile/timing identity, generation and
sequence, deterministic seed, playheads, physical slots, logical role bindings,
and previous-source history. Role assignments are a permutation over physical
slots; history remains associated with physical slots when roles move.

The operator MUST return `DeckOperatorResult` with:

- one finite contiguous tensor preserving source shape, dtype, and device
  exactly;
- a bounded JSON-object provenance value.

The SDK validates before and after every call. Operators cannot request a cast,
copy, resize, device move, decode, file path, output surface, or Tauri command.
Post-operator latent state reaches Snapshot or Live Capture before decode; Core,
not the Deck, finalizes and imports the resulting `.lc`.

## Declarative faceplate v2

`faceplate.json` describes layout and control bindings. LatentDeck renders it
with host components; a package cannot supply HTML, JavaScript, CSS, or direct
application access. Authors can arrange labeled sections, choose bounded
columns, expose sources and roles, and conditionally reveal controls while the
host retains consistent transport, capture, output, and accessibility behavior.

The root has exactly `schema_version`, `title`, and `sections`.
`schema_version` is `2`. A faceplate contains one to 16 sections and at most 128
widgets total. Section and widget IDs are unique canonical identifiers; labels
and titles are bounded safe text.

Every v2 section declares:

- `section_id`, `title`, and `widgets`;
- `region`: `output`, `actions`, or `controls`;
- `columns`: an integer from one to four.

### Widget set

| Widget          | Required binding                                                                                |
| --------------- | ----------------------------------------------------------------------------------------------- |
| `source_picker` | One zero-based `slot_index`.                                                                    |
| `slider`        | One numeric `control_id` and bounds matching `operator.json`.                                   |
| `number`        | One numeric or integer control and exact matching bounds. Integer controls require this widget. |
| `toggle`        | One boolean control.                                                                            |
| `select`        | One enum control and the exact option-value set.                                                |
| `role_editor`   | The complete declared role-ID set.                                                              |
| `barycentric3`  | Two distinct normalized `[0,1]` number controls and three distinct vertex roles.                |
| `transport`     | The complete physical-slot set.                                                                 |
| `seed`          | Deterministic session seed control.                                                             |
| `capture`       | One or both of `snapshot` and `live_capture`.                                                   |
| `monitor`       | Host-owned decoded output surface.                                                              |

The host requires:

- every physical source slot exactly once;
- every operator control exactly once;
- exactly one role editor, transport, seed, and monitor;
- at most one capture widget;
- exactly one `output` region;
- monitors only in `output`, capture only in `actions`, and all other widgets
  in `controls`;
- every capture mode to be present in the Deck's required capabilities.

`visible_when` is an optional v2-only list of one to eight predicates. Each
predicate names an enum or boolean control and one to 16 exact accepted values.
All predicates must match for the widget to be visible. Visibility affects only
presentation: the control remains part of the typed operator contract.

Numeric widgets must repeat the exact minimum, maximum, and step from
`operator.json`. Select values must equal the complete enum option set. These
cross-checks prevent a faceplate from presenting a looser or different runtime
contract.

## Integrity, archive, and path rules

`integrity.json` uses catalog version `1.0.0` and sorted entries containing
`path`, `byte_length`, and lowercase SHA-256. It follows the same closed-tree
rules as Codec Packages.

| Limit                                     | `.ld` value |
| ----------------------------------------- | ----------: |
| Archive size                              |       8 MiB |
| Extracted size                            |      16 MiB |
| Total files, including both control files |         256 |
| Individual file size                      |       1 MiB |
| Control JSON size                         |  1 MiB each |

ZIP entries may use Stored or Deflate compression. Encrypted entries, links,
special files, non-empty directory records, empty directories, duplicate or
case-colliding paths, and inconsistent-case hierarchies are rejected.

Paths are bounded ASCII, forward-slash relative paths made only from normal
components. Absolute, drive, device, ADS, backslash, traversal, reserved
Windows name, trailing-dot, and trailing-space forms are rejected. Installed
roots and descendants must also be free of symlinks, junctions, and other
reparse points.

The common packer sorts files, writes fixed ZIP metadata with Stored entries,
refuses to overwrite an output, detects a source changing during the copy, and
reinspects the result before atomic publication.

## Installation, trust, and version lifecycle

External packages are installed only from an explicit local `.ld` file and an
expected archive SHA-256. The lifecycle performs archive preflight, exact hash
measurement, bounded extraction into sibling staging, closed-tree validation,
atomic publication, and then an atomic trust receipt.

Current-user storage is fixed:

```text
%LOCALAPPDATA%\LatentDeck\Decks\<deck_id>\<deck_version>
%LOCALAPPDATA%\LatentDeck\PackageTrust\decks\<deck_id>\<deck_version>.json
```

Install publishes an immutable, initially disabled exact version. Up to 16
versions of one Deck ID may coexist, but at most one may be enabled. The user
must select a version explicitly when alternatives exist; there is no automatic
newest selection, overwrite, inherited trust, URL install, or update.

Enabled runtime resolution revalidates the exact package and holds a shared
usage lease plus retained handles for its closed tree. Disabling changes future
launch authority but does not kill an existing session. Repair and removal are
blocked while the version is in use. Removal requires the exact version to be
disabled; removing a corrupt tree requires an explicit override.

Package IDs under `org.latentdeck.*` are reserved. Bundled Decks are authorized
only by a build-generated index binding exact kind, ID, version, and archive
SHA-256. An external publisher must use its own reverse-DNS namespace. Package
hashes identify approved bytes; publisher metadata remains explicitly
self-declared.

## Compatibility resolution

The Extensions Manager computes every installed Deck-version by Codec-version
pair. Package-level checks compare trust and health, Worker Protocol, host APIs,
tensor/Python/Torch ABI, LC/profile intersection, and capabilities. A selected
source set then adds required external assets, selected profile and device,
source count, exact tensor geometry, decoded dimensions, frame timing, and
timing-contract identity.

Stable result reasons are:

- `compatible`;
- `untrusted`;
- `missing_asset`;
- `package_invalid`;
- `unsupported_protocol`;
- `unsupported_host_api`;
- `unsupported_tensor_abi`;
- `unsupported_profile`;
- `unsupported_signal`;
- `unsupported_timing`;
- `unsupported_capability`.

Package-stage failures are resolved before selected-source facts, so a missing
asset cannot mask an invalid package pair. Compatibility never performs or
authorizes cast, resize, crop, alignment, equivalent-rate normalization,
re-encode, profile substitution, protocol downgrade, or fallback to another
package.

## Bundled conformance examples

The bundled D2 and Q4 packages use the same public package, lifecycle, SDK, and
faceplate path as an external Deck:

- D2 `0.2.1` declares two physical slots and `carrier`/`donor` roles;
- Q4 `0.2.1` declares four slots and one carrier plus three donor roles;
- both use operator schema/API `0.2.0`, faceplate schema `2`, exact CUDA fp16
  geometry allowlists, 24 fps with 24 samples per slot, and all five mandatory
  codec lifecycle capabilities;
- both leave `profile_allowlist` null, so compatibility comes from the exact
  Codec Package profile intersection and selected signals rather than an H3
  name hard-coded into the Deck.

Q4 also demonstrates `barycentric3` and conditional control visibility. D2
demonstrates a two-role permutation and multiple algorithm-specific control
groups. Neither receives a private Tauri namespace or a hard-coded worker
entrypoint.

## Author checklist

Before distributing a Deck Package:

1. Use a non-reserved reverse-DNS ID and a new immutable SemVer for changed
   bytes.
2. Keep the archive within the `.ld` file, type, and size bounds.
3. Make `deck-pack.json`, `operator.json`, and `faceplate.json` agree exactly.
4. Catalog every payload and notice, then bind the exact catalog hash in the
   manifest.
5. Pack with the deterministic package manager and record the resulting
   archive SHA-256.
6. Inspect, install, verify, and explicitly enable the exact version.
7. Confirm the compatibility matrix and selected-source reason without a
   fallback.
8. Exercise realtime controls, roles, transport, deterministic replay,
   Snapshot, Live Capture, and output behavior declared by the faceplate.
