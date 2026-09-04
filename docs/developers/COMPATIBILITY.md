# Compatibility reference

This page is the reader-facing version map for the public preview.
Package manifests, lock files, schemas, and parsers remain authoritative. A
release's generated compatibility manifest records the exact artifacts it
contains.

## Public identities

| Surface | Identity |
| --- | --- |
| Release label | `0.1.0-preview.1` |
| Release channel | `unsigned_preview` |
| LatentDeck App / LatentPlayer API | `0.1.0` |
| Windows installer build identity | `0.1.0+1` |
| LC specification | `0.1.0` |
| H3 codec profile | `minimax_h3 / h3_av_latent / 0.1.0` |
| H3 Codec Pack | `org.latentdeck.h3 / 0.2.1` |
| H3 codec adapter package | `0.2.0` |
| Deck Package host/operator ABI (`compatibility.deck_operator_api`) | integer `1` |
| Deck operator descriptor API (`operator.json.deck_operator_api`) | `0.2.0` |
| Codec Package host/adapter ABI (`compatibility.codec_adapter_api`) | integer `1` |
| Comfy Toolkit operator descriptor schema | `0.1.0` |
| Bundled D2 Deck Package | `org.latentdeck.deck.d2 / 0.2.1` |
| Bundled Q4 Deck Package | `org.latentdeck.deck.q4 / 0.2.1` |
| D2 and Q4 operator Python packages | `0.2.0` |
| Channel Roll example Python package | `0.1.0` |
| Deck manifest | `1.0.0` |
| Codec manifest | `2.0.0` |
| Declarative faceplate schema | `2` |
| Cartridge SDK | `0.1.0` |
| Comfy LC Recorder bundle | Windows x64 CPython `3.12` / `3.13`; Cartridge SDK `cp312-abi3` |
| Deck SDK | `0.2.0` |
| Codec SDK | `0.2.0` |
| Worker Protocol | `2` |

These versions are independent. A Deck faceplate/package change can increment
the Deck Package without changing its operator math. The H3 Codec Pack `0.2.1`
contains adapter package version `0.2.0`; those identities are intentionally
distinct. The integer Deck/Codec ABI selectors in package compatibility are
host protocol gates; they are not versions of the Python SDKs or the Deck
operator descriptor schema/API.

## Developer Kit compatibility manifest

The Developer Kit generates `COMPATIBILITY.json` from authoritative project
metadata; it is not maintained by copying this page. For
`0.1.0-preview.1`, its contract is:

| Manifest field | Preview value |
| --- | --- |
| `schema_version` | `1` |
| `release_label` / `release_channel` | `0.1.0-preview.1` / `unsigned_preview` |
| `platform` | `windows-x86_64` |
| `application_api_version` | `0.1.0` |
| `windows_installer_version` | `0.1.0+1` |
| `python.implementation` / `python.supported_series` | `cpython` / `3.13` |
| `python.h3_runtime_version` / `python.platform_tag` | `3.13.14` / `win_amd64` |
| `torch.h3_runtime_exact_build` | `2.13.0+cu130` |
| `torch.bundled_in_developer_kit` | `false` |
| `lc_spec_versions` | `["0.1.0"]` |
| `worker_protocol_versions` | `[2]` |
| `deck_manifest_version` / `codec_manifest_version` | `1.0.0` / `2.0.0` |
| `deck_package_operator_host_api_version` / `codec_adapter_api_version` | host/package ABI integer `1` / integer `1` |
| `operator_descriptor_schema_version` | Comfy Toolkit schema `0.1.0` |
| `h3_codec.pack_version` / `h3_codec.adapter_version` | `0.2.1` / `0.2.0` |
| `sdks.cartridge` / `sdks.deck` / `sdks.codec` | `0.1.0` / `0.2.0` / `0.2.0` |
| `decks.d2` | `org.latentdeck.deck.d2` / Deck Package `0.2.1` |
| `decks.q4` | `org.latentdeck.deck.q4` / Deck Package `0.2.1` |
| `python_operator_packages.d2` | `latentdeck-operator-d2` / `0.2.0` |
| `python_operator_packages.q4` | `latentdeck-operator-q4` / `0.2.0` |
| `python_operator_packages.channel_roll` | `latentdeck-example-channel-roll` / `0.1.0` |

The manifest also contains a sorted `project_wheels` array with the exact name
and version of all nine wheels:

| Project wheel | Version |
| --- | --- |
| `latentdeck-cartridge` | `0.1.0` |
| `latentdeck-codec-host` | `0.1.0` |
| `latentdeck-codec-sdk` | `0.2.0` |
| `latentdeck-comfy-cartridge` | `0.1.0` |
| `latentdeck-comfy-toolkit` | `0.1.0` |
| `latentdeck-deck-sdk` | `0.2.0` |
| `latentdeck-example-channel-roll` | `0.1.0` |
| `latentdeck-operator-d2` | `0.2.0` |
| `latentdeck-operator-q4` | `0.2.0` |

`distributable` is `true` only for a normal Developer Kit build from clean
`main`; local contract builds explicitly mark it `false`.

The generated host/adapter API fields above mirror the integer compatibility
selectors in package manifests. They do not replace the `0.2.0` Deck operator
descriptor/SDK API or the independently versioned `0.2.0` Codec SDK and H3
adapter package. The generated operator-descriptor schema identity belongs to
the Comfy Toolkit research-operator format; it is not `operator.json` in a
Deck Package.

The Kit carries seven machine-readable schemas: LC manifest, Comfy Toolkit
operator descriptor, Deck manifest, Deck operator, Deck faceplate, Codec
manifest, and the shared extension integrity catalog.

## Development/runtime pins

| Runtime | Preview contract |
| --- | --- |
| Operating system | Windows x64; Windows 11 is the clean release-validation path |
| Python | CPython `3.13.x` |
| Torch for H3 | `2.13.0+cu130` |
| Safetensors in the Recorder | Existing host package is preserved; bundled private fallback is `0.8.0` |
| Rust | `1.93.1` MSVC |
| Node.js | `24.20.0` |
| pnpm | `11.24.0` |
| uv | `0.11.8` |
| NSIS | `3.11` for release builds |

The default Python development workspace is CPU-light and does not install
Torch. H3 declares its CUDA stack through an explicit extra and ships it only
inside the separately curated Codec Pack runtime.

The `3.13.x` row is the source-workspace and H3 runtime contract. The separate
Comfy LC Recorder release bundle supports Windows x64 CPython 3.12 and 3.13 by
shipping the Cartridge SDK as one `cp312-abi3` wheel; it does not widen the
Python ABI of Codec, Deck, operator, or Toolkit packages.

## Extension and media types

| Type | Meaning | Trust |
| --- | --- | --- |
| `.lc` | Codec-neutral Latent Cartridge | Untrusted data-only media |
| `.ld` | Deck Package | Explicitly installed executable code |
| `.ldcodec` | Codec Package and isolated runtime | Explicitly installed executable code |

`.lddeck` is retired and is not an alias. An H3 setup requires its exact
adjacent `.ldcodec`; a generic adjacent ZIP is not a Protocol 2 package.

## Compatibility is an intersection

A loadable Deck/Codec/source combination must agree on application range,
Worker Protocol, host and adapter APIs, Python/Torch/tensor ABI, LC version,
profile key, required capabilities, selected device/assets, signal geometry,
timing, and source count. Installed or newer does not mean compatible.

The Extensions matrix reports every exact installed Deck-version/Codec-version
pair. Source admission adds the validated cartridge facts. A mismatch remains
visible and never triggers a hidden fallback or conversion.

For the complete rules, use the [Deck Package](../../spec/deck-package/README.md),
[Codec Package](../../spec/codec-pack/README.md), and [Deck Signal
Contract](../../spec/deck-api/README.md).
