# Repository layout

The repository uses a component-separated monorepo layout. Specifications and
stable contracts remain authoritative over replaceable implementations.

| Path                             | Intended responsibility                                                                   |
| -------------------------------- | ----------------------------------------------------------------------------------------- |
| `apps/latentdeck/`               | Standalone Library, LD-D2, and LD-Q4 application with embedded native output.             |
| `apps/latentplayer/`             | Standalone cartridge player and raw-H3 preparation workspace with embedded native output. |
| `crates/core/`                   | Realtime coordination and stable signal contracts.                                        |
| `crates/cartridge/`              | Codec-neutral `.lc` reader, writer, validator, hashing, and metadata.                     |
| `crates/cartridge-python/`       | PyO3 adapter over the single Rust Cartridge SDK implementation.                           |
| `crates/gpu/`                    | Native GPU presentation and shared frame transport.                                       |
| `crates/control/`                | UI-independent controls and state commands.                                               |
| `crates/deck-runtime-contracts/` | Package compatibility, session-capacity, and output-lease contracts.                      |
| `crates/extension-manager/`      | Shared `.ld`/`.ldcodec` lifecycle and compatibility resolution.                           |
| `crates/native-output/`          | Embedded native presentation host and shared fullscreen surface.                          |
| `crates/output-mp4/`             | Bounded video-only H.264 MP4 recording of intrinsic decoded Deck frames.                  |
| `crates/output-spout/`           | Required Windows Spout2 native texture output.                                            |
| `codec-host/python/`             | Isolated Python/PyTorch worker host.                                                      |
| `codec-host/codecs/h3/`          | H3 codec adapter code; never H3 weights.                                                  |
| `operators/builtin/`             | Versioned built-in latent operators.                                                      |
| `comfy/toolkit/`                 | Research/development ComfyUI package.                                                     |
| `comfy/latent-cartridge/`        | Small cartridge-recording ComfyUI package.                                                |
| `spec/latent-cartridge/`         | Codec-neutral `.lc` specification.                                                        |
| `spec/codec-h3/`                 | H3 codec-profile contract.                                                                |
| `spec/codec-pack/`               | Installable `.ldcodec` package and lifecycle contract.                                    |
| `spec/deck-package/`             | Installable `.ld` Deck package and declarative faceplate contract.                        |
| `spec/deck-api/`                 | Shared geometry, compatibility, and presentation contract for Decks.                      |
| `spec/worker-protocol/`          | Protocol 2 control plane and explicit legacy Player bridge.                               |
| `sdk/`                           | Cartridge, Codec, and Deck SDK surfaces and bindings.                                     |
| `tests/`                         | Cross-component tests and explicitly approved tiny fixtures.                              |
| `docs/`                          | Durable public documentation; local concept PNGs remain ignored.                          |
| `tools/`                         | Repository maintenance and release-safety tools.                                          |

Cargo, pnpm, uv, Tauri, Svelte, and Python workspaces use pinned manifests and
lock files. Application UI, workers, codec adapters, Deck implementations, and
output integrations remain replaceable behind their documented contracts.
