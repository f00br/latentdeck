# Repository layout

The scaffold follows the monorepo shape accepted by the 0.1 plan.

| Path | Intended responsibility |
| --- | --- |
| `apps/latentdeck/` | Standalone LatentDeck application shell and UI. |
| `apps/latentplayer/` | Standalone lightweight cartridge player. |
| `crates/core/` | Realtime coordination and stable signal contracts. |
| `crates/cartridge/` | Codec-neutral `.lc` reader, writer, validator, hashing, and metadata. |
| `crates/gpu/` | Native GPU presentation and shared frame transport. |
| `crates/control/` | UI-independent controls and state commands. |
| `crates/output-spout/` | Required Windows Spout2 native texture output. |
| `codec-host/python/` | Isolated Python/PyTorch worker host. |
| `codec-host/codecs/h3/` | H3 codec adapter code; never H3 weights. |
| `operators/builtin/` | Versioned built-in latent operators. |
| `comfy/toolkit/` | Research/development ComfyUI package. |
| `comfy/latent-cartridge/` | Small cartridge-recording ComfyUI package. |
| `spec/latent-cartridge/` | Codec-neutral `.lc` specification. |
| `spec/codec-h3/` | H3 codec-profile contract. |
| `sdk/` | Public SDK surfaces and bindings. |
| `tests/` | Cross-component tests and explicitly approved tiny fixtures. |
| `docs/` | Durable public documentation; local concept PNGs remain ignored. |
| `tools/` | Repository maintenance and release-safety tools. |

The scaffold intentionally does not create Cargo, pnpm, uv, Tauri, Svelte, or
Python manifests. Those files establish real dependency and workspace contracts
and should be generated only by an implementation task that can validate them.
