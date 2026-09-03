# Comfy Toolkit Explicit-Install Operator API 0.1

## Status and scope

This document defines the older public Python research-operator boundary used
by LatentDeck Comfy Toolkit 0.1. It does not define the current LatentDeck
application Deck SDK and does not install arbitrary Python operators into the
realtime application worker.

An external operator is separately installed trusted Python code: either a
normal distribution or an explicitly copied ComfyUI module. It is not
cartridge content. `.lc` readers never inspect, import, install, or execute an
operator.

## This API versus the current Deck SDK

The two APIs deliberately share a `process_sources`-shaped callable, but they
are separate contracts and their descriptors and types are not
interchangeable.

| Boundary     | This Comfy Toolkit API                                                      | Current LatentDeck Deck SDK                                                                              |
| ------------ | --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Purpose      | Offline and research operator use inside the Toolkit host.                  | Realtime installed Decks in the LatentDeck application.                                                  |
| Distribution | A separately reviewed Python distribution or explicitly copied module.      | A strict, integrity-catalogued `.ld` Deck Package.                                                       |
| Activation   | Host imports code, then calls `registry.install`.                           | Extensions Manager installs, verifies, and explicitly enables an exact package version.                  |
| Descriptor   | Toolkit operator descriptor/API `0.1.0`.                                    | `deck-pack.json` `1.0.0` plus `operator.json` schema/API `0.2.0`.                                        |
| Call types   | `OperatorContext` and `ToolkitOperatorResult`.                              | `DeckOperatorContext` and `DeckOperatorResult` from `latentdeck-deck-sdk` `0.2.0`.                       |
| Topology     | `topology`, `input_count`, processing modes, and descriptor bypass.         | Manifest slots/roles, role permutation, exact geometry/timing/capabilities, and previous-source context. |
| User surface | No Deck faceplate, session, transport, native output, or capture authority. | Host-rendered faceplate v2 plus generic sessions, transport, output, and capture.                        |

Third-party Decks are supported now through the
[Deck Package contract](../deck-package/README.md) and current Deck SDK. Do not
rename a Toolkit descriptor to `operator.json`, place it in `.ld`, or pass a
Toolkit result/context to the generic Deck worker. Shared mathematical code may
be adapted deliberately, but the target contract must be implemented and
validated explicitly.

## Versioning

- descriptor schema: `0.1.0`
- Python API: `0.1.0`
- this complete descriptor is the first public 0.1 contract; it replaces the
  incomplete local bootstrap draft before publication
- after publication, evolution within 0.1 is additive only
- changing required fields, callable arguments, tensor layout, or result
  semantics requires a new API/schema version

The machine-readable closed schema is
[`operator-descriptor.schema.json`](../../comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json).

## Descriptor

Every external operator declares exactly these fields:

| Field                       | Contract                                                                                                               |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `schema_version`            | Exactly `0.1.0`.                                                                                                       |
| `operator_id`               | Stable bounded lowercase token.                                                                                        |
| `operator_version`          | `MAJOR.MINOR.PATCH`.                                                                                                   |
| `trust`                     | Exactly `explicit_install`.                                                                                            |
| `entrypoint`                | Identity string `module:callable`; never dynamically imported by the registry.                                         |
| `topology`                  | Exactly `single_source`, `dual_source`, or `carrier_donors`.                                                           |
| `input_count`               | Exact number of ordered sources: 1 for single, 2 for dual, and 2–16 for carrier plus donors.                           |
| `capabilities`              | Closed booleans for `full_clip`, `streaming`, `chunk`, and `deterministic`; at least one processing mode is supported. |
| `supported_profiles`        | One to sixteen closed codec/profile/timing/layout declarations.                                                        |
| `controls`                  | Closed, bounded enum/float/integer control declarations.                                                               |
| `bypass`                    | One or more exact control values plus `output_source`; the runtime owns this identity path.                            |
| `limits.max_spatial_tokens` | Positive bound no greater than 4096.                                                                                   |

Unknown fields, non-finite numeric values, invalid ranges, executable source,
download locations, and cartridge trust claims are rejected. The descriptor is
metadata, not a loader.

### Topologies and ordered sources

The callable always receives one immutable tuple. The descriptor fixes its
meaning and length:

| Topology         | `input_count` | `sources` order                    |
| ---------------- | ------------: | ---------------------------------- |
| `single_source`  |             1 | `(source,)`                        |
| `dual_source`    |             2 | `(carrier, donor)`                 |
| `carrier_donors` |          2–16 | `(carrier, donor_1, donor_2, ...)` |

All sources are independently validated before trusted operator code runs.
They must have identical shape and device. Donor order is stable and is part of
the deterministic input contract.

`capabilities.full_clip`, `.streaming`, and `.chunk` declare where a host may
dispatch the operator. `OperatorContext.processing_mode` is one of those three
names and is rejected when the corresponding capability is false. Slot tensors
remain `[1,24,1,H,W]`; `full_clip` means the host applies the operator over the
complete ordered clip, while `streaming` and `chunk` describe realtime or
bounded-chunk dispatch. The compatibility test in the Toolkit verifies whether
those declared paths actually agree.

`capabilities.deterministic` declares whether identical ordered sources,
controls, profile, seed, slot index, and processing mode are expected to return
the same output. The Toolkit determinism test verifies the declaration; the
registry does not silently rewrite a non-deterministic operator.

The `bypass.controls` object names an explicit zero/bypass state using valid
descriptor control values. Once sources, controls, and context pass validation,
the registry returns a contiguous clone of `sources[output_source]` without
executing operator code. This guarantees an exact, inspectable identity path.

## Installation

After the trusted module has been explicitly installed and imported by its
host, registration is one explicit host call:

```python
registry.install(
    descriptor,
    already_imported_process_sources,
    exported_entrypoint="my_distribution:process_sources",
)
```

The registry verifies descriptor schema, callable presence, and exact
entrypoint identity. It performs no import, filesystem discovery, package
installation, network access, or URL fetch. Duplicate operator IDs are
rejected. Loading requires the exact installed ID and version.

This boundary establishes explicit consent and predictable identity; it is not
a Python sandbox. After installation the operator has the permissions of the
hosting Python process. Only separately reviewed distributions or module files
should be installed. Copying a standalone `.py` operator into ComfyUI's
`custom_nodes` directory is therefore an executable-code trust decision, not a
cartridge-loading feature.

## Callable contract

```python
process_sources(
    sources: tuple[torch.Tensor, ...],
    controls: dict[str, object],
    context: OperatorContext,
) -> ToolkitOperatorResult
```

The canonical callable exposes exactly three positional parameters. The
Toolkit retains one narrow compatibility path for a pre-0.1, exactly
four-parameter `process_slot(carrier, donor, controls, context)` callable with
`dual_source` descriptors. New operators must use `process_sources`.

Sources are finite dense F16 tensors with exact layout `[1,24,1,H,W]`, identical
shape and device, and a spatial grid within the descriptor limit. The context
contains codec/profile/timing identity, deterministic seed, slot index, and
processing mode. Controls are defaulted and validated against the closed
descriptor before the call.

Installed operators expose checked convenience wrappers:

- `process_single(source, ...)` for `single_source`;
- `process_dual(carrier, donor, ...)` for `dual_source`;
- `process_carrier_donors(carrier, donors_tuple, ...)` for
  `carrier_donors`;
- `process_slot(carrier, donor, ...)` as the dual compatibility spelling;
- `process_sources(sources_tuple, ...)` as the common primitive.

Calling a wrapper for the wrong topology or supplying a list/wrong-length tuple
is a stable contract error. No wrapper performs conversion, resize, dtype cast,
source reordering, or hidden alignment.

The result must preserve shape, dtype, device, dense layout, and contiguity. It
must be finite. Provenance must be JSON-safe, no larger than 65,536 UTF-8 bytes,
and retain the installed operator ID and version under `operation`.

Unexpected implementation failures are translated into the stable
`operator.execution_failed` error without exposing implementation paths or
exception text.

## Cartridge security invariant

There is intentionally no `install_from_cartridge`, descriptor auto-discovery,
or dynamic entrypoint import. A cartridge cannot select a package or module,
provide Python source, supply a download URL, or trigger registry mutation.
Operator installation and cartridge loading are separate actions and separate
trust domains.

## Moving a research operator into a Deck

To expose Toolkit research as a realtime Deck, create a new Deck integration:

1. implement the current Deck SDK callable using `DeckOperatorContext` and
   `DeckOperatorResult`;
2. declare its typed controls and role order in `operator.json` `0.2.0`;
3. declare exact runtime, signal, timing, geometry, and capability requirements
   in `deck-pack.json`;
4. bind every source, role, control, transport, seed, and output anchor in a
   schema-v2 declarative faceplate;
5. build, inspect, install, verify, and explicitly enable the resulting `.ld`
   package.

There is no automatic descriptor conversion or inherited Toolkit trust. The
Deck package receives application runtime authority only after its own package
and compatibility lifecycle succeeds.

The public reference implementation is the
[50-line Channel Roll / MyLatentOperator example](../../operators/examples/channel-roll/README.md).
