# LatentDeck

> LatentDeck treats saved generative latents as playable and synthesizable media
> signals.

LatentDeck is an open ecosystem under active local development around the codec-neutral **Latent
Cartridge** (`.lc`) format: record a latent representation, play it, synthesize
it with other cartridges in real time, and resample the post-operator latent
state into a new cartridge.

## Repository status

The public-safe bootstrap is complete and the reproducible 0.1 workspace is
active. It currently contains:

- the accepted 0.1 product and architecture plan;
- public-repository boundaries and agent instructions;
- pinned Cargo, pnpm, uv, Tauri, Svelte, and Python workspaces;
- buildable LatentDeck and LatentPlayer smoke shells;
- normative LC 0.1 and H3 0.1 specifications;
- a deterministic Rust cartridge SDK and command-line tool;
- the native Python SDK binding and raw H3 `latentdeck-pack` authoring command;
- the independent `Save Latent Cartridge (.lc)` ComfyUI recorder;
- a policy for local-only, non-binding interface references.

The product behavior, codec runtime, native output, and release packaging are
still being implemented. There is no supported release, bundled model, codec
pack, or cartridge in this repository yet.

## Planned ecosystem

- **LatentDeck App** — standalone real-time latent synthesis instrument.
- **LatentPlayer App** — lightweight `.lc` playback application.
- **Latent Cartridge Standard** — codec-neutral, data-only media container.
- **Cartridge SDK and APIs** — read, write, validate, inspect, hash, resample,
  codec adaptation, and operator integration.
- **ComfyUI-LatentCartridge** — small authoring package for recording generated
  latents into `.lc` files.
- **LatentDeck Comfy Toolkit** — separate research environment for operators,
  codecs, benchmarking, and offline experiments.

MiniMax H3 is the first intended codec profile. It is not the definition of
`.lc`, and H3 weights or cartridges are not distributed from this source tree.

## Start here

- [Approved 0.1 implementation plan](docs/main-plan-v01.md)
- [Product and architecture rationale](latentdeck_0.1-plan.md)
- [Latent Cartridge Specification 0.1](spec/latent-cartridge/README.md)
- [LC Manifest JSON Schema 0.1](spec/latent-cartridge/manifest.schema.json)
- [MiniMax H3 Codec Profile 0.1](spec/codec-h3/README.md)
- [Python Cartridge SDK and raw H3 packer](sdk/python/README.md)
- [ComfyUI-LatentCartridge recorder](comfy/latent-cartridge/README.md)
- [Agent runway](AGENTS.md)
- [Bootstrap handoff](docs/repository/BOOTSTRAP_HANDOFF.md)
- [Repository layout](docs/repository/REPOSITORY_LAYOUT.md)
- [Public repository boundary](docs/repository/REPOSITORY_BOUNDARY.md)
- [Public release checklist](docs/repository/PUBLIC_RELEASE_CHECKLIST.md)
- [Pinned toolchains](docs/repository/TOOLCHAINS.md)
- [Interface reference policy](docs/assets/concepts/README.md)
- [Contributing](CONTRIBUTING.md)
- [Security posture](SECURITY.md)

## Public-source boundary

The source repository is intended for code, specifications, documentation, and
explicitly reviewed public fixtures. Model weights, `.lc` cartridges, raw
latents, private datasets, generated output, local environments, and unreviewed
third-party assets are excluded by default.

Run the local guard before committing:

```powershell
pwsh -NoProfile -File tools/Test-PublicTree.ps1
```

Run the complete local workspace check with the pinned Node runtime:

```powershell
pwsh -NoProfile -File tools/Check-Workspace.ps1
```

## License

Original LatentDeck code and documentation are licensed under the
[Apache License 2.0](LICENSE). External codec assets, model weights,
cartridges, third-party code, and media retain their own terms and are not
covered merely by appearing near this project.
