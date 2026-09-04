# Changelog

All notable public changes to LatentDeck are documented here. Format,
application, Deck, Codec, SDK, and protocol versions are independent; each
release entry lists the identities it contains.

## Unreleased

- Added an owner-approved README hero screenshot and a checksum-pinned route to
  the separately hosted H3 demo cartridges.
- Added a first-run Windows quick start covering application choice, H3 Codec
  Pack setup, explicit CUDA/decoder selection, demo playback, and D2 startup.

## 0.1.0-preview.1

Channel: unsigned Windows prerelease.

See [the complete preview release notes](docs/releases/0.1.0-preview.1.md).

### Added

- Codec-neutral LC 0.1 cartridge format and strict Rust/Python tooling.
- LatentPlayer playback and raw-import PREPARE workspace.
- LatentDeck Library, Collections, installable Decks, and declarative
  faceplates.
- Bundled D2 and Q4 Deck packages with realtime pre-decode synthesis.
- Snapshot and bounded Live Capture back into `.lc`.
- Native window/fullscreen output, Spout2, and video-only H.264 MP4 recording.
- Installable `.ld` and `.ldcodec` lifecycle with exact-version compatibility.
- Worker Protocol 2, Cartridge/Deck/Codec SDKs, ComfyUI recorder, and Comfy
  Toolkit research nodes.
- Public documentation, community workflow, extension authoring kit, and
  reproducible GitHub prerelease staging path.
- Machine-readable schemas and CPU-first cartridge, Deck, and Codec examples.
- Pinned documentation for the separately hosted three-cartridge H3 demo pack.

### Known preview limitations

- Windows x64 and the H3 NVIDIA/CUDA path are the initial tested runtime.
- Preview installers are not Authenticode-signed.
- H3 decoder/model assets and demo cartridges are not bundled. The current
  external demo-pack revision is evaluation-only pending explicit media terms.
- Audio playback and synthesis are not implemented.
- Incompatible latent signals are refused rather than converted implicitly.
