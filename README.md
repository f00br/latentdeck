# LatentDeck

> LatentDeck treats saved generative latents as playable and synthesizable media
> signals.

LatentDeck is a planned open ecosystem around the codec-neutral **Latent
Cartridge** (`.lc`) format: record a latent representation, play it, synthesize
it with other cartridges in real time, and resample the post-operator latent
state into a new cartridge.

## Repository status

This repository currently contains **project groundwork only**:

- the accepted 0.1 product and architecture plan;
- public-repository boundaries and agent instructions;
- the planned monorepo directory scaffold;
- a policy for local-only, non-binding interface references.

There is no runnable application, package, model, codec pack, cartridge, or
supported release yet. The scaffold must not be presented as an implementation.

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
- [Agent runway](AGENTS.md)
- [Bootstrap handoff](docs/repository/BOOTSTRAP_HANDOFF.md)
- [Repository layout](docs/repository/REPOSITORY_LAYOUT.md)
- [Public repository boundary](docs/repository/REPOSITORY_BOUNDARY.md)
- [Public release checklist](docs/repository/PUBLIC_RELEASE_CHECKLIST.md)
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

## License

Original LatentDeck code and documentation are licensed under the
[Apache License 2.0](LICENSE). External codec assets, model weights,
cartridges, third-party code, and media retain their own terms and are not
covered merely by appearing near this project.
