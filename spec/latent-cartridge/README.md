# Latent Cartridge specification 0.1

Status: normative for LC Spec `0.1.0`.

This document defines the interoperable, codec-neutral `.lc` container. A
reader should be able to implement a safe validator, and a writer should be
able to produce byte-for-byte reproducible cartridges, using only this
document and the linked manifest schema. Codec-specific tensor and timing
rules are defined by separate profiles.

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHALL NOT**,
**SHOULD**, **SHOULD NOT**, and **MAY** are to be interpreted as described by
[BCP 14](https://www.rfc-editor.org/info/bcp14) when they appear in capitals.

## 1. Conformance and stable center

LC Spec `0.1.0` defines three conformance roles:

- an **LC reader** parses and validates an untrusted cartridge without
  executing it or extracting it;
- an **LC writer** emits the canonical archive described here;
- a **codec profile** narrows tensor names, shapes, dtypes, timing, and
  compatibility for one codec family.

A conforming implementation MUST implement every requirement for its claimed
role. Merely opening the file with a general ZIP or Safetensors library is not
validation.

The `.lc` file and the realtime signal contract are the stable center of
LatentDeck. Application versions, Deck implementations, codec packs, operator
APIs, and user interfaces are versioned independently.

## 2. Security and data-only model

An `.lc` is untrusted media. It is never a package or plugin.

- Opening a cartridge MUST NOT execute code, import a module, run a command,
  install an operator, resolve a network resource, or load an identifier as a
  plugin.
- Archive entries MUST be read in place. A reader MUST NOT extract entries to
  a filesystem path.
- Operator IDs, creator names, URIs, prompts, workflow metadata, and other
  provenance values are descriptive data only.
- A reader MUST complete bounded archive, manifest, tensor-layout, hash, and
  finite-value validation before any GPU allocation.
- Malformed input MUST produce a stable error, not a process crash or a
  partially trusted cartridge.
- A structural inspection is not full validation and MUST be labelled as such.
  Tensor access MUST be available only from a fully validated cartridge.

Safetensors removes pickle-style object execution, but it does not prove that
tensor values are finite. LC validation therefore includes an explicit
streaming NaN and infinity scan.

## 3. Canonical archive

The physical container is a single-disk ZIP64 archive using the STORE method.
LC `0.1.0` contains exactly two or three file entries in this local-header and
central-directory order:

1. `manifest.json`;
2. exactly one profile-declared path matching
   `payloads/<payload-id>.safetensors`;
3. optional `preview.webp`.

The H3 Profile fixes the second path to `payloads/h3.safetensors`. A different
future profile can select a different lowercase ASCII payload ID without
changing the codec-neutral archive rule.

No directory entry or other entry is valid. In particular, an extra script,
executable, media render, nested archive, sidecar, or second tensor payload
MUST be rejected even when the manifest does not reference it.

### 3.1 Entry names

Entry names are ASCII and case-sensitive. They MUST use `/` as the separator.
They MUST NOT contain a backslash, NUL, control character, empty segment, `.`
segment, `..` segment, leading slash, drive prefix, URI prefix, or trailing
slash. Local-header and central-directory names MUST be identical.

A reader MUST reject both byte-identical duplicate names and ASCII
case-folding collisions before looking up a required entry.

### 3.2 Required ZIP encoding

Canonical LC writers MUST emit these values:

| ZIP field                 | Required value           |
| ------------------------- | ------------------------ |
| Compression method        | `0` (STORE)              |
| General-purpose flags     | `0`                      |
| Version needed            | `45` (ZIP 4.5)           |
| Version made by           | `0x032d` (Unix, ZIP 4.5) |
| DOS timestamp             | `1980-01-01 00:00:00`    |
| Internal attributes       | `0`                      |
| External attributes       | `0`                      |
| Disk number               | `0`                      |
| File and archive comments | empty                    |

Every local header MUST place `0xffffffff` in both 32-bit size fields and MUST
carry one ZIP64 extra field containing the actual uncompressed and compressed
64-bit sizes in that order. Every central header MUST additionally place
`0xffffffff` in its 32-bit local-header-offset field and carry the actual
uncompressed size, compressed size, and local-header offset in one ZIP64 extra
field, in that order. All ZIP64 integers are unsigned little-endian values.

The ZIP64 end-of-central-directory record and locator MUST always be present,
including for a small cartridge. The legacy end record MUST use the ZIP64
sentinel values for entry counts, central-directory size, and offset. Only the
required ZIP64 extra fields are permitted.

Encryption, data descriptors, compression, split disks, directory entries,
symlinks, overlapping entries, archive extra data, digital signatures, and
bytes after the legacy end record are forbidden. CRC-32 values MUST be present
and correct. Compressed and uncompressed entry sizes MUST be equal.

These constraints deliberately make a valid LC archive canonical rather than
accepting every ZIP encoding that could contain the same files.

## 4. Deterministic bytes and atomic writing

An LC writer is deterministic with respect to finalized logical inputs. Given
the same manifest values and the same payload and preview bytes, two writes
MUST produce the same archive bytes and SHA-256 digest.

- `manifest.json` MUST be UTF-8 without a BOM or trailing newline and MUST use
  [RFC 8785 JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785).
- The writer MUST accept a finalized `cartridge_id`; it MUST NOT silently add a
  random UUID, current time, machine path, or host-specific value.
- Imported Safetensors bytes MUST be copied without re-encoding. A newly
  generated Safetensors payload MUST order tensor names lexically and serialize
  its header deterministically.
- Source payload and preview size and SHA-256 MUST be measured before the
  manifest is serialized. They MUST be measured again while copying so a
  source mutation cannot produce a falsely described cartridge.

The normal file-writing sequence is:

1. preflight and hash each source while holding its open file handle;
2. build the canonical manifest;
3. create a unique same-directory file whose name ends in `.partial`;
4. write and flush all bytes, then synchronize the file;
5. reopen that same file and perform full LC and profile validation;
6. atomically rename it to the requested `.lc` name;
7. remove the partial file on any failure.

The default policy is no-clobber. Replacing an existing target requires an
explicit overwrite option. A failure MUST NOT leave a final cartridge or an
orphaned partial file.

## 5. Manifest

The normative machine-readable shape is
[LC manifest schema 0.1](manifest.schema.json). Every object is closed:
unknown fields are invalid. A parser MUST detect duplicate JSON keys at every
depth before ordinary object deserialization. A JSON library that keeps only
the first or last duplicate does not satisfy this rule.

### 5.1 Root fields

| Field               | Requirement                                                           |
| ------------------- | --------------------------------------------------------------------- |
| `spec_version`      | Exactly `0.1.0`.                                                      |
| `cartridge_id`      | Non-nil canonical lowercase UUID. It is identity, not a content hash. |
| `codec`             | Codec family, profile ID, and independently versioned profile.        |
| `payloads`          | Exactly one Safetensors payload descriptor in 0.1.                    |
| `tensors`           | One or two tensor descriptors; profile rules narrow them.             |
| `timing`            | Versioned timing contract plus decoded-video description.             |
| `audio`             | Explicit audio presence or omission policy.                           |
| `preview`           | Optional WebP descriptor.                                             |
| `provenance`        | Creator and bounded source records.                                   |
| `parent_cartridges` | Ordered genealogy inputs. Empty for an original cartridge.            |
| `operation_history` | Ordered transformations. Empty for an unmodified import.              |

All string lengths below are measured as UTF-8 bytes, not Unicode scalar
values or UTF-16 code units.

JSON Schema `maxLength` is a structural preflight measured in Unicode code
points. Implementations MUST additionally enforce the stricter UTF-8 byte
ceilings in section 6.

### 5.2 Codec and payload descriptors

`codec.family`, `codec.profile`, and identifiers elsewhere in the manifest are
lowercase ASCII tokens. `codec.profile_version` selects a complete profile
contract; readers MUST NOT guess compatibility from the family alone.

A payload descriptor records:

- its exact archive `path`;
- media type `application/vnd.safetensors`;
- exact `byte_length` of the raw entry;
- lowercase hexadecimal SHA-256 of the raw entry bytes.

Payload SHA-256 does not include the ZIP header. The ZIP CRC-32 is an
additional transport-integrity check and does not replace SHA-256.

### 5.3 Tensor descriptors

Each tensor descriptor records its logical `stream`, Safetensors `name`,
containing payload, `storage_dtype`, `runtime_dtype`, and complete shape.

In LC 0.1 the stream is `visual` or `audio`, and dtypes are `F16` or `F32`.
The profile decides which combinations are legal. Every descriptor MUST match
the Safetensors header exactly for name, dtype, rank, shape, offset-derived byte
length, and containing payload. Unknown Safetensors tensor keys are invalid.

`storage_dtype` describes bytes in the cartridge. `runtime_dtype` describes an
explicit profile-approved runtime cast. No other crop, resize, re-encode, dtype
conversion, or tensor rewrite is implied by loading a cartridge.

### 5.4 Timing

Timing uses reduced positive rational numbers:

```json
{ "numerator": 24, "denominator": 1 }
```

The denominator MUST be nonzero. The canonical writer MUST divide numerator
and denominator by their greatest common divisor. `decoded_video.duration`
MUST equal `frame_count / frame_rate` exactly as a rational number. A profile
defines the relationship between tensor time axes and decoded frames.

### 5.5 Audio disposition

The `audio` object is always present and has one of four closed forms:

| Policy                      | Audio tensor | Additional meaning                                                               |
| --------------------------- | ------------ | -------------------------------------------------------------------------------- |
| `source_absent`             | forbidden    | The source had no audio latent.                                                  |
| `preserved_source`          | required     | Original audio bytes were retained without processing.                           |
| `copied_from_carrier_exact` | required     | Audio was copied byte-for-byte from the named carrier after exact timing checks. |
| `omitted_timing_mismatch`   | forbidden    | Audio existed upstream but exact duration or temporal mapping did not match.     |

The last two forms require `source_cartridge`, containing the canonical source
cartridge UUID and archive SHA-256. `omitted_timing_mismatch` also requires one
of `duration_mismatch`, `temporal_mapping_mismatch`, or
`duration_and_mapping_mismatch` as its reason.

Audio is metadata and payload preservation only in LatentDeck 0.1. The format
does not imply audio playback or synthesis.

### 5.6 Preview

When present, the preview entry is exactly `preview.webp` with media type
`image/webp`. The descriptor records its raw-byte length and SHA-256 plus
positive pixel dimensions. The reader MUST verify a valid WebP envelope and
the declared dimensions before exposing it. A UI image decoder still treats
the verified bytes as untrusted media.

The preview is never authoritative for decoded dimensions, timing, or content.

### 5.7 Provenance and genealogy

`provenance.created_by` identifies the authoring tool and version.
`provenance.created_at` is optional and, when supplied, is an RFC 3339 UTC
timestamp chosen by the caller. The deterministic writer does not create one.

Provenance `sources` may record an input kind, hash, URI, license label, and a
bounded metadata object. Authoring tools SHOULD avoid recording absolute local
paths, credentials, or private workflow contents by default. A prompt, seed,
model label, LoRA list, or workflow hash MAY be stored as metadata, but none is
required for playback.

Each `parent_cartridges` record contains a UUID, archive SHA-256, and a
descriptive role such as `carrier` or `donor_b`. Each operation record contains
an operator ID and version, a deterministic seed, and bounded controls. The
array order is chronological. These IDs never authorize or trigger operator
installation or execution.

Manifest seeds are JSON-safe non-negative integers no larger than
`9007199254740991` (`2^53 - 1`). Every JSON number, including numbers nested in
metadata or controls, MUST be finite and exactly representable under the JCS
number model.

## 6. Hard validation limits

These are format ceilings, not recommended working sizes. A caller MAY impose
smaller limits but MUST NOT silently raise them while claiming LC 0.1
conformance.

| Resource               |               LC 0.1 ceiling |
| ---------------------- | ---------------------------: |
| Complete archive       | 16 GiB (`17179869184` bytes) |
| File entries           |               exactly 2 or 3 |
| Manifest entry         |      1 MiB (`1048576` bytes) |
| Safetensors header     |      1 MiB (`1048576` bytes) |
| Tensor payload         | 15 GiB (`16106127360` bytes) |
| Preview entry          |    16 MiB (`16777216` bytes) |
| Preview dimension      |         4096 pixels per axis |
| Preview pixels         |                   16,777,216 |
| Tensor descriptors     |                            2 |
| Tensor rank            |                            5 |
| Parent cartridges      |                          256 |
| Operation records      |                         1024 |
| Controls per operation |           128 top-level keys |
| Provenance sources     |                           64 |
| JSON nesting depth     |                           32 |
| ASCII identifier       |                    128 bytes |
| General human string   |                   4096 bytes |
| URI string             |                   8192 bytes |

Arrays, objects, strings, archive offsets, tensor element counts, tensor byte
counts, decoded dimensions, and resource estimates MUST use checked arithmetic.
A declared value that overflows or cannot fit within a ceiling MUST be rejected
before an allocation based on that value.

## 7. Validation procedure

A full validator MUST perform these stages in order, without loading the whole
payload into memory:

1. Open the cartridge once, determine its bounded file length, and retain that
   handle for validation and subsequent tensor access.
2. Preflight raw local headers, central directory, ZIP64 records, names,
   offsets, ranges, canonical fields, entry count, and absence of trailing
   bytes.
3. Read the bounded manifest entry; verify UTF-8, JCS form, duplicate-key
   absence, strict schema, version, and resource counts.
4. Cross-check every archive entry against its descriptor and reject any
   missing or unexpected entry.
5. Parse the bounded Safetensors header with duplicate-key detection. Verify
   tensor names, dtypes, shapes, checked byte sizes, non-overlapping in-range
   offsets, contiguous data coverage, and exact agreement with the manifest.
6. Stream each entry through CRC-32 and SHA-256 verification. Stream every F16
   or F32 tensor in bounded chunks and reject positive infinity, negative
   infinity, quiet NaN, or signaling NaN, including a value split at a chunk
   boundary.
7. Apply the selected codec-profile geometry, cadence, timing, dtype, audio,
   and compatibility rules.
8. Return a validation receipt containing archive hash, payload hashes, storage
   bytes, and an explicit runtime-resource estimate.

Device-specific GPU admission is a later runtime decision. The LC validator
MUST NOT allocate GPU memory.

An inspection command MAY stop after bounded stage 5 and report metadata, but
it MUST return `validation_level = "structure"`. Only completion of stages 1
through 8 can return `validation_level = "full"` or expose a tensor reader.

On systems that support sharing modes, the validated handle SHOULD prevent
write or delete replacement until its consumer releases it. A validated path
MUST NOT be closed and silently reopened for runtime use.

## 8. Errors

Public errors have a stable machine-readable `code`, optional `location`, and
human-readable `message`. The message can improve without a version bump; code
semantics cannot. Locations SHOULD use an archive entry name, tensor name, or
JSON Pointer and MUST NOT expose a private absolute path unnecessarily.

### 8.1 Stable error codes

I/O and output:

`io_open`, `io_read`, `io_write`, `target_exists`,
`atomic_commit_failed`, `postwrite_validation_failed`.

Archive:

`archive_too_large`, `archive_malformed`, `zip64_required`,
`archive_noncanonical`, `entry_count_invalid`, `entry_missing`,
`entry_unexpected`, `entry_duplicate`, `entry_unsafe_path`,
`entry_encrypted`, `entry_compressed`, `entry_too_large`,
`entry_size_mismatch`, `entry_overlap`, `archive_trailing_data`,
`entry_crc_mismatch`.

Manifest:

`manifest_too_large`, `manifest_not_utf8`, `manifest_json_invalid`,
`manifest_duplicate_key`, `manifest_unknown_field`, `manifest_invalid`,
`unsupported_spec_version`.

Payload and tensor:

`payload_hash_mismatch`, `safetensors_header_too_large`,
`safetensors_invalid`, `tensor_missing`, `tensor_unexpected`,
`tensor_descriptor_mismatch`, `tensor_dtype_forbidden`,
`tensor_shape_invalid`, `tensor_size_overflow`, `tensor_non_finite`.

Codec profile and runtime admission:

`unsupported_codec`, `unsupported_profile_version`, `timing_mismatch`,
`decoded_geometry_mismatch`, `runtime_limit_exceeded`.

### 8.2 CLI exit groups

| Exit | Meaning                             |
| ---: | ----------------------------------- |
|  `0` | Success                             |
|  `2` | Command-line usage error            |
|  `3` | Invalid or corrupt cartridge        |
|  `4` | Unsupported spec, codec, or profile |
|  `5` | Environment or I/O failure          |
|  `6` | Internal invariant failure          |

The detailed stable code MUST remain available in structured CLI output and
language bindings; the grouped process exit code is not a replacement.

## 9. Hashes and identity

All LC hashes use SHA-256 and lowercase hexadecimal encoding.

- Payload and preview hashes cover raw uncompressed entry bytes.
- A complete-cartridge hash covers every byte of the canonical `.lc` archive.
- The complete-cartridge hash is returned by `hash` and full validation but
  MUST NOT be placed inside its own manifest, which would be circular.
- `cartridge_id` remains stable across an intentional byte-preserving rewrite
  but is not proof of content identity. Consumers use both UUID and archive
  hash when exact identity matters.

## 10. Versioning

LC Spec, codec profiles, application releases, codec packs, and operator APIs
have separate versions. A reader MUST accept only versions it explicitly
implements. It MUST NOT treat an unknown `0.1.x` or profile version as
compatible merely because the major or minor component looks familiar.

The wire value defined here is exactly `0.1.0`. Adding, removing, renaming, or
changing the meaning of a manifest field, archive rule, or validation ceiling
requires a new advertised spec version. Editorial clarifications that do not
change conforming bytes or behavior do not.

An unsupported LC version produces `unsupported_spec_version`. A recognized LC
version with an unknown codec family produces `unsupported_codec`; a known
family with an unknown profile version produces `unsupported_profile_version`.

## 11. Conformance tests and evidence boundary

Public conformance tests MUST generate synthetic tensor data and malformed
archives in a temporary directory. Binary `.lc` files, real latents, model
weights, workflows, remote manifests, and generated media are private opt-in
E2E inputs and are not normative fixtures.

A minimum LC suite covers:

- deterministic visual-only, visual-plus-audio, F32-import, F16-resample,
  genealogy, and optional-preview round trips;
- truncation at every envelope region; duplicate and case-colliding names;
  traversal, absolute, drive-prefixed, backslash, NUL, directory, and symlink
  names; encryption, DEFLATE, descriptors, comments, trailing data, overlap,
  missing ZIP64, size mismatch, CRC mismatch, and unexpected entries;
- oversized, non-UTF-8, noncanonical, malformed, duplicate-key, missing-field,
  unknown-field, excessive-depth, and unsupported-version manifests;
- invalid Safetensors header lengths, duplicate keys, gaps, overlaps,
  out-of-range offsets, forbidden dtypes, shape and byte-count overflow,
  manifest mismatch, hash mismatch, and every F16/F32 non-finite encoding;
- no extraction, no outside-path modification, bounded allocation for hostile
  lengths, source-mutation detection, no-clobber behavior, and cleanup after an
  injected write failure.

Codec cadence, geometry, and compatibility cases belong to the selected codec
profile. Private E2E observations can validate an implementation, but they do
not silently change this public contract.
