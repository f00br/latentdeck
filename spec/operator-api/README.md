# LatentDeck Explicit-Install Operator API 0.1

## Status and scope

This document defines the public Python research-operator boundary used by
LatentDeck Comfy Toolkit 0.1. It complements the standalone application's
closed builtin registry; it does not make arbitrary Python operators part of
the realtime release worker.

An external operator is a separately installed, trusted Python distribution.
It is not cartridge content. `.lc` readers never inspect, import, install, or
execute an operator.

## Versioning

- descriptor schema: `0.1.0`
- Python API: `0.1.0`
- evolution within 0.1 is additive only
- changing required fields, callable arguments, tensor layout, or result
  semantics requires a new API/schema version

The machine-readable closed schema is
[`operator-descriptor.schema.json`](../../comfy/toolkit/src/latentdeck_comfy_toolkit/operator-descriptor.schema.json).

## Descriptor

Every external operator declares exactly these fields:

| Field | Contract |
|---|---|
| `schema_version` | Exactly `0.1.0`. |
| `operator_id` | Stable bounded lowercase token. |
| `operator_version` | `MAJOR.MINOR.PATCH`. |
| `trust` | Exactly `explicit_install`. |
| `entrypoint` | Identity string `module:callable`; never dynamically imported by the registry. |
| `supported_profiles` | One to sixteen closed codec/profile/timing/layout declarations. |
| `controls` | Closed, bounded enum/float/integer control declarations. |
| `limits.max_spatial_tokens` | Positive bound no greater than 4096. |

Unknown fields, non-finite numeric values, invalid ranges, executable source,
download locations, and cartridge trust claims are rejected. The descriptor is
metadata, not a loader.

## Installation

Installation is one explicit host call:

```python
registry.install(
    descriptor,
    already_imported_process_slot,
    exported_entrypoint="my_distribution:process_slot",
)
```

The registry verifies descriptor schema, callable presence, and exact
entrypoint identity. It performs no import, filesystem discovery, package
installation, network access, or URL fetch. Duplicate operator IDs are
rejected. Loading requires the exact installed ID and version.

This boundary establishes explicit consent and predictable identity; it is not
a Python sandbox. After installation the operator has the permissions of the
hosting Python process. Only separately reviewed distributions should be
installed.

## Callable contract

```python
process_slot(
    carrier: torch.Tensor,
    donor: torch.Tensor,
    controls: dict[str, object],
    context: OperatorContext,
) -> ToolkitOperatorResult
```

Inputs are finite dense F16 tensors with exact layout `[1,24,1,H,W]`, identical
shape and device, and a spatial grid within the descriptor limit. The context
contains codec/profile/timing identity, deterministic seed, and slot index.
Controls are defaulted and validated against the closed descriptor before the
call.

The result must preserve shape, dtype, device, dense layout, and contiguity. It
must be finite. Provenance must be JSON-safe, no larger than 65,536 UTF-8 bytes,
and retain the installed operator ID and version under `operation`.

Unexpected implementation failures are translated into the stable
`operator.execution_failed` error without exposing implementation paths or
exception text.

## Cartridge security invariant

There is intentionally no `install_from_cartridge`, descriptor auto-discovery,
or dynamic entrypoint import. A cartridge cannot select a package, provide
Python source, supply a download URL, or trigger registry mutation. Operator
installation and cartridge loading are separate actions and separate trust
domains.

The public reference implementation is the
[Channel Roll example](../../operators/examples/channel-roll/README.md).
