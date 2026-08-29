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
- owner-supplied rough interface sketches kept under
  `docs/assets/concepts/` in a local working copy;
- API keys, tokens, credentials, signing certificates, machine configuration,
  and absolute local paths;
- copied third-party repositories or assets without a recorded redistribution
  basis.

## Exception process

A normally excluded artifact may become a public fixture or release asset only
after an explicit review records:

1. purpose and minimal required size;
2. origin and cryptographic hash;
3. owner and redistribution license;
4. absence of private prompts, paths, credentials, and unrelated payloads;
5. parser and memory-safety implications;
6. whether it belongs in Git history, a GitHub release, or separate storage.

Do not bypass `.gitignore` as the exception mechanism. Change the repository
policy deliberately and document why.

## Third-party boundary

LatentDeck code, third-party runtime dependencies, external codec assets, and
individual cartridges have separate provenance and licensing. A project license
cannot grant rights to upstream weights or cartridge content.

The Apache-2.0 project license covers original LatentDeck code and
documentation only. It does not change the terms of model weights, codec packs,
cartridges, external operators, or third-party assets.

The future Codec Manager must show source and license information before any
external download. The source repository must not silently vendor those assets.
