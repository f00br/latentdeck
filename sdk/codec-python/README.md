# LatentDeck Codec SDK for Python 0.2.0

`latentdeck-codec-sdk` defines the Python side of Worker Protocol 2 and the
codec-neutral adapter contract used by LatentPlayer and generic Deck runtimes.
It is a typed contract package for CPython 3.13; it is not a codec, worker,
installer, or model downloader.

Use this SDK when implementing the adapter inside a `.ldcodec` Codec Package.
After reading this document, an implementer should be able to expose one
adapter factory, describe and validate a profile, open retained cartridge
access, decode slots, and stage capture output without bypassing Core.

## Boundary and ownership

The Codec SDK owns:

- immutable data contracts for codec identity, profiles, tensor and decoded
  ABIs, external assets, capture, and optional raw import;
- structural protocols for adapters, opened sources, cartridge access, and
  capture writers;
- strict JSON and named-MessagePack validation for Worker Protocol 2;
- stable `CodecSdkError` and `ProtocolError` failures.

The SDK does not:

- build, install, enable, trust, or select a `.ldcodec` package;
- validate the LC archive, expose arbitrary archive paths, or construct the
  final LC container;
- select, download, or license external model assets;
- provide Torch, a decoder, a worker process, shared-memory transport, or a
  security sandbox;
- repair incompatible media through an implicit cast, resize, crop, alignment,
  re-encode, profile substitution, model substitution, or device fallback.

Core owns package validation, trust receipts, compatibility selection,
retained file handles, worker launch, Protocol 2 supervision, final LC
construction, and Library import. The generic worker owns command dispatch and
invokes the adapter through the contracts below.

## Implement one CodecAdapter

`CodecAdapter` is a runtime-checkable structural protocol. Subclassing is not
required; an object with the exact methods is sufficient. A Codec Package
declares a portable `module:callable` entrypoint. Prefer a zero-argument factory
that returns a fresh adapter instance for the worker session.

| Method                                       | Required behavior                                                                                                              |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `descriptor()`                               | Return the exact pack, adapter, host API, capabilities, and supported profile identities without loading media or model state. |
| `inspect(cartridge)`                         | Inspect codec-specific metadata through bounded `CartridgeAccess` and return payload identity plus exact signal geometry.      |
| `validate_profile(cartridge, inspection)`    | Validate profile semantics against the same retained cartridge and return a fully bound `ProfileReceipt`.                      |
| `load(request)`                              | Accept only the selected descriptor, device, ordinal, and explicit hash/length-bound external assets.                          |
| `open_source(cartridge, receipt, source_id)` | Bind the retained cartridge and accepted receipt to a new `SourceHandle`.                                                      |
| `read_slot(source, slot_index)`              | Return one zero-based latent slot in the exact negotiated runtime tensor ABI.                                                  |
| `decode_slot(tensor, maximum_frames)`        | Decode at most the requested number of frames and return a validated `DecodedBatch`.                                           |
| `reset_decoder(stream_generation)`           | Reset causal decoder state at an explicit newer stream generation.                                                             |
| `create_capture_writer(request)`             | Create a bounded writer beneath the host-owned staging root.                                                                   |

Descriptor, inspection, and profile validation run before Core accepts any
codec-owned GPU allocation. `load` and `open_source` must cross-check the exact
accepted identities; they must not rediscover another pack, asset, profile, or
device.

### Minimal descriptor

Every full Codec Pack v2 descriptor declares the five required capabilities.
Declare `Capability.RAW_IMPORT` only when the adapter also implements the
optional raw-import protocol.

```python
from latentdeck_codec_sdk import (
    Capability,
    CodecDescriptor,
    ProfileKey,
    validate_codec_v2_descriptor,
)


PROFILE = ProfileKey(
    codec_family="example_family",
    profile="example_latent",
    profile_version="0.1.0",
)


def make_descriptor() -> CodecDescriptor:
    return validate_codec_v2_descriptor(
        CodecDescriptor(
            pack_id="org.example.codec",
            pack_version="0.2.0",
            adapter_id="org.example.codec.adapter",
            adapter_version="0.2.0",
            host_api_version="2.0",
            capabilities=(
                Capability.PLAYER,
                Capability.REALTIME,
                Capability.RESAMPLE,
                Capability.SNAPSHOT_CAPTURE,
                Capability.LIVE_CAPTURE,
            ),
            profiles=(PROFILE,),
        )
    )
```

The adapter's `descriptor()` may return this value. The package entrypoint may
then be a factory such as `make_adapter()` that returns the complete structural
adapter. Package metadata, not cartridge data or UI state, selects that
entrypoint.

## Profile receipts and tensor ABI

`inspect()` returns a `ProfileInspection` without accepting arbitrary file
paths. `validate_profile()` returns a unique `ProfileReceipt` that binds:

- cartridge, archive, and payload identity;
- exact pack and adapter identity and versions;
- `ProfileKey` and `SignalGeometry`;
- runtime `TensorAbi` and `DecodedAbi`;
- supported capabilities and host/device memory estimates.

Use `validate_profile_receipt(receipt, descriptor)` before returning a receipt.
The adapter must additionally prove that the receipt describes the supplied
inspection and retained cartridge. Core independently cross-checks the receipt
before allowing the load/open path to proceed.

The current tensor ABI is:

- CPython `3.13` and the exact declared Torch build;
- finite, contiguous `[1,C,1,H,W]` tensors;
- `float16`, `bfloat16`, or `float32`;
- one explicit `cpu` or `cuda` device;
- channel and spatial dimensions exactly matching `SignalGeometry`.

The decoded ABI is contiguous CPU RGBA8 represented as a `memoryview` of
unsigned bytes with shape `[N,H,W,4]`, where `1 <= N <= 24` and the byte length
is exactly `N * H * W * 4`.

Representation work explicitly required by the selected profile may happen
inside the adapter. It must produce the declared ABI exactly. It must never be
used to make an incompatible cartridge look compatible or to silently change
geometry, timing, profile, model, or device.

`CodecLoadRequest` supplies zero to 16 exact `ExternalAsset` bindings. Each
binding contains an asset ID, path, lowercase SHA-256, and byte length; it is
not permission to discover a substitute asset. On Windows, Core has already
hashed the selected file and retains its safe path ancestry for the session, so
the adapter cross-checks the binding without repeating a full hash during
`load()` or `open_source()`. A platform without an equivalent retained-identity
bridge must conservatively remeasure the asset.

## Retained cartridge and source access

`CartridgeAccess` is an already integrity-validated, read-only view retained by
Core. It exposes only:

- `cartridge_id` and `archive_sha256`;
- the validated manifest;
- a named `TensorAccessDescriptor` with exact storage dtype, shape, and byte
  length;
- bounded byte ranges relative to that named tensor.

Tensor descriptors deliberately contain no archive offsets. Do not reopen the
cartridge by path, reparse the archive, or repeat codec-neutral ZIP,
Safetensors, hash, finite-value, and size validation for every slot.

`open_source()` may prepare reusable codec state and returns a `SourceHandle`
with an exact `source_id`, positive `slot_count`, and `close()`. `read_slot()`
must reject a stale/closed handle and an index outside `0..slot_count-1`.
Realtime implementations should retain validated/resident state and avoid
per-slot archive scans, repeated host-to-device copies, or duplicate
synchronizing validation.

## Decode and capture

`decode_slot()` receives one exact runtime tensor and a maximum frame count. It
returns `DecodedBatch`; media bytes travel through the shared ring managed by
the worker, not inside Protocol 2 control messages.

`CaptureWriter` has three operations:

- `append(tensor, reset_event=...)` records one post-operator latent slot and an
  optional bounded reset event;
- `finish()` returns a `CapturePayload` for a staged payload beneath the
  host-owned root;
- `abort()` removes capture-owned partial state.

Snapshot and Live Capture both use this writer. The adapter stages only the
codec payload. Core verifies its identity and limits, builds and reopens the
codec-neutral LC container, and imports it into the Library. The adapter must
not write directly into the Library.

## Optional raw import

An adapter declaring `Capability.RAW_IMPORT` also implements
`RawImportAdapter`:

1. `preflight_raw_import()` performs bounded CPU-only inspection and returns a
   receipt bound to the exact source hash, length, profile, timing, and tensor
   metadata.
2. `stage_raw_import()` stages the exact receipted Safetensors payload beneath
   the host-owned root.
3. `abort_raw_import()` removes state and partial output for that import ID.

Preflight metadata contains exactly one visual tensor and at most one audio
tensor. Audio metadata may be preserved, but playback and synthesis are not
part of the current runtime. Raw import never returns a ready Library
cartridge; Core constructs and fully validates the final LC container.

## Worker Protocol 2 helpers

Most adapter authors should use the generic worker and need not construct wire
envelopes. Worker implementers and conformance tests can use:

- `validate_envelope()` for one closed Protocol 2 envelope;
- `encode_json()` and `decode_json()` for diagnostics and cross-language
  fixtures;
- `encode_messagepack()` and `decode_messagepack()` for the named-MessagePack
  payload used by the worker transport;
- `WorkerStreamValidator` for first-frame authenticated hello, session,
  sequence, and worker message-ID validation.

These helpers encode or decode the envelope payload only. The operating-system
transport adds its own bounded length framing. Unknown fields, duplicate map
keys, invalid identities, non-finite controls, unknown commands, and trailing
data fail closed. This SDK is Protocol 2 only and never falls back to Protocol 1.

## Public exports

The package root exports the following supported names:

| Area                        | Names                                                                                                                                                                                              |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Adapter protocols           | `CodecAdapter`, `CartridgeAccess`, `SourceHandle`, `CaptureWriter`, `RawImportAdapter`                                                                                                             |
| Codec and profile data      | `CodecDescriptor`, `CodecLoadRequest`, `ExternalAsset`, `ProfileKey`, `ProfileInspection`, `ProfileReceipt`, `SignalGeometry`, `TensorAbi`, `TensorAccessDescriptor`, `DecodedAbi`, `DecodedBatch` |
| Capture data                | `CaptureRequest`, `CapturePayload`                                                                                                                                                                 |
| Raw-import data             | `RawImportPreflightRequest`, `RawImportPreflight`, `RawImportStageRequest`, `RawImportArtifact`, `RawImportMetadata`, `RawImportTensor`                                                            |
| Contract validation         | `CodecSdkError`, `validate_codec_v2_descriptor`, `validate_profile_receipt`                                                                                                                        |
| Protocol identity and enums | `PROTOCOL`, `PROTOCOL_VERSION`, `Capability`, `ErrorCode`, `SessionState`, `CodecState`, `PlayerState`, `DeckState`, `CaptureState`                                                                |
| Protocol validation         | `ProtocolError`, `WorkerStreamValidator`, `validate_envelope`, `encode_json`, `decode_json`, `encode_messagepack`, `decode_messagepack`                                                            |

## Author checks

Before packaging an adapter:

1. Validate the descriptor and every profile receipt, including deliberate
   identity, geometry, dtype, device, and capability mismatches.
2. Test retained access and source close/reset behavior without reopening the
   cartridge path.
3. Test zero, maximum, and out-of-range slot/decode/capture limits.
4. Test capture finish and abort cleanup, then let Core construct and reopen the
   resulting LC cartridge.
5. If raw import is declared, test changed-source detection between preflight
   and staging plus abort cleanup.
6. Run JSON and named-MessagePack conformance against the Rust Protocol 2
   implementation.
7. Test with the exact CPython, Torch, device, external assets, and package
   identities declared by the Codec Package.

## Trust and runtime caveat

Package hashes and trust receipts bind exact bytes; they do not authenticate a
publisher or make Python safe. Codec code and its model runtime execute with
the current user's authority. Environment clearing, retained handles, worker
authentication, and process supervision are integrity and lifecycle controls,
not a security sandbox. Install and enable only code and external assets the
user deliberately trusts.
