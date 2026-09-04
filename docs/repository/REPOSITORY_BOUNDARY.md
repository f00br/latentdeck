# Public repository boundary

## Public by design

The main repository may contain:

- original LatentDeck source code;
- the `.lc` standard and codec-profile specifications;
- application, SDK, operator, codec-adapter, and ComfyUI integration code;
- tests that do not contain private or restricted media;
- documentation and explicitly approved visual assets;
- dependency locks and build metadata for reproducibility;
- small fixtures whose origin, rights, hashes, and safety have been reviewed.

## Outside the repository by default

The following belong in external development storage, model/codec installation,
private research storage, or separately governed media distribution:

- H3 Transformer, text encoder, VAE, TAEHV/taeh3, or other model weights;
- `.lc`, `.h3latent`, Safetensors, PyTorch checkpoints, ONNX/TensorRT artifacts,
  and raw latent payloads;
- user media, prompts, workflows, datasets, generated renders, and benchmark
  captures;
- local ComfyUI installations, virtual environments, dependency caches, build
  products, databases, logs, diagnostics, and crash dumps;
- API keys, tokens, credentials, signing certificates, machine configuration,
  and absolute local paths;
- copied third-party repositories or assets without a recorded redistribution
  basis.

## Exception process

A normally excluded artifact may become a public fixture or release asset only
after an explicit review records:

1. purpose and minimal required size;
2. origin and cryptographic hash;
3. rights holder and redistribution license;
4. absence of private prompts, paths, credentials, and unrelated payloads;
5. parser and memory-safety implications;
6. whether it belongs in Git history, a GitHub release, or separate storage.

Do not bypass `.gitignore` as the exception mechanism. Change the repository
policy deliberately and document why.

## Showcase media exception

Before the repository becomes public, `docs/assets/showcase/` may contain one
hero image and up to four additional optimized still images. Each file must be
no larger than 2 MiB, and the directory must remain at or below 10 MiB total.
Every image requires recorded authorship, redistribution rights, provenance,
and useful alt text. Showcase video belongs in the matching GitHub release,
not in Git history. Until those checks pass, the directory remains absent.

## Third-party boundary

LatentDeck code, third-party runtime dependencies, external codec assets, and
individual cartridges have separate provenance and licensing. A project license
cannot grant rights to upstream weights or cartridge content.

The Apache-2.0 project license covers original LatentDeck code and
documentation only. It does not change the terms of model weights, codec packs,
cartridges, external operators, or third-party assets.

The current Extensions Manager shows self-declared publisher, source, license,
and exact package identity for local `.ld` and `.ldcodec` packages. It performs
no URL installation or external asset download. Required decoder/model assets
remain explicit user selections with their own identity and terms; the source
repository must not silently vendor them.
