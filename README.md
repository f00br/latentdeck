# LatentDeck

> LatentDeck treats saved generative latents as playable and synthesizable media
> signals.

LatentDeck is an open ecosystem around the codec-neutral **Latent Cartridge**
(`.lc`) format: record or convert a latent representation, play it, synthesize
it with other cartridges in real time, and resample the post-operator latent
state into a new cartridge.

## Repository status

The `0.1.0` implementation is in owner-UAT closeout on `main`. The owner has
confirmed the broader walkthrough. The latest correction candidate preserves
D2/Q4 Live Capture across automatic source loops through a newly versioned H3
Codec Pack and fixes top-down MP4 orientation; affected owner retest, the
ComfyUI all-nodes presentation canvas, and final release gates remain. The
repository contains:

- public-repository boundaries and agent instructions;
- pinned Cargo, pnpm, uv, Tauri, Svelte, and Python workspaces;
- complete LatentDeck application and a LatentPlayer that can both play `.lc`
  cartridges and prepare raw H3 latent files for them;
- normative LC 0.1 and H3 0.1 specifications;
- a deterministic Rust cartridge SDK and command-line tool;
- the native Python SDK binding, raw H3 `latentdeck-pack` authoring command,
  and `latentdeck-convert` batch converter for existing H3 AV Safetensors;
- the independent `Save Latent Cartridge (.lc)` ComfyUI recorder;
- the clean-room LatentDeck Comfy Toolkit research nodes and explicit-install
  external Operator API;
- isolated H3 Player, LD-D2, and LD-Q4 workers with native DX12 presentation;
- a shared SQLite Library with many-to-many Collections and virtual `All` and
  `Unassigned` banks;
- deterministic Snapshot and bounded Live Capture resampling back into `.lc`,
  including D2/Q4 capture across automatic source-loop boundaries and matching
  large application safety limits;
- upright video-only H.264 MP4 recording of D2/Q4 decoded output at intrinsic
  geometry;
- automatic Library-to-Deck invalidation and explicit hot insertion of newly
  captured cartridges through a bounded worker replacement;
- Spout2 output, structured diagnostics, database backup/migration, Windows
  application packaging, an independent current-user H3 Codec Pack setup and
  exact-version uninstall lifecycle, and SBOM generation;
- a policy for local-only, non-binding interface references.

The owner-UAT binary baseline is a clean, unsigned local RC built from commit
`9fc7caa`. It does not bundle a Codec Pack, decoder, model weight, or cartridge.
Strict four-source Q4 acceptance, the owner-approved six-minute D2/Q4 stability
suite, application playback, portrait and landscape presentation, and Spout2
were verified locally. Clean-machine installer/Codec Pack lifecycle, signing,
and owner-authorized publication remain external gates. See the
[0.1.0 acceptance status](docs/release/ACCEPTANCE_STATUS.md). There is no
published release or supported bundled model in this repository.

## Implemented interfaces

The public screenshots come from the running applications with isolated empty
data and codec roots. They do not use the local concept sketches or private
cartridges.

![LatentDeck Library with virtual collection banks](docs/assets/screenshots/latentdeck-library-empty.png)

- [LD-D2 faceplate and missing-codec state](docs/assets/screenshots/latentdeck-d2-missing-codec.png)
- [LD-Q4 carrier/donor faceplate and missing-codec state](docs/assets/screenshots/latentdeck-q4-missing-codec.png)
- [LatentPlayer empty playback surface](docs/assets/screenshots/latentplayer-empty.png)
- [Screenshot provenance and capture boundary](docs/assets/screenshots/README.md)

## Ecosystem components

- **LatentDeck App** — standalone real-time latent synthesis instrument.
- **LatentPlayer App** — `.lc` playback plus guided raw-H3 preflight and batch
  cartridge preparation; the console converter remains the developer surface.
- **Latent Cartridge Standard** — codec-neutral, data-only media container.
- **Cartridge SDK and APIs** — read, write, validate, inspect, hash, resample,
  codec adaptation, and operator integration.
- **ComfyUI-LatentCartridge** — small authoring package for recording generated
  latents into `.lc` files.
- **LatentDeck Comfy Toolkit** — separate research environment for operators,
  codecs, benchmarking, and offline experiments.

MiniMax H3 is the first intended codec profile. It is not the definition of
`.lc`, and H3 weights or cartridges are not distributed from this source tree.

## Windows installation boundary

LatentDeck App, LatentPlayer, and the H3 Codec Pack are three independent
Windows installations. Installing or removing one does not install, replace,
or remove either of the others.

The public H3 path is user-facing: keep
`LatentDeck-H3-CodecPack-<version>-setup.exe` beside the exact matching
`LatentDeck-H3-CodecPack-<version>-windows-x64.zip`, then run the setup. The
large ZIP remains adjacent rather than being embedded in the small setup, and
the setup accepts only the filename, byte length, SHA-256, pack identity, and
version bound into that executable. It installs for the current user at the
fixed `%LOCALAPPDATA%\LatentDeck\CodecPacks` root and registers exact-version
removal in Windows Installed Apps. It needs no administrator elevation,
network access, system Python, or PowerShell.

The Codec Pack does not contain a decoder weight or model. After installation,
select an accepted external TAEH3 decoder explicitly in Codec Manager. The
PowerShell lifecycle scripts remain engineering and recovery tools; they are
not part of normal public onboarding. See the
[H3 Codec Pack runbook](docs/release/H3_CODEC_PACK.md).

## Start here

- [Current UAT and release handoff](docs/release/continue.md)
- [Master-user test guide](docs/release/MASTER_USER_TEST.md)
- [0.1.0 acceptance status](docs/release/ACCEPTANCE_STATUS.md)
- [Project concept](docs/CONCEPT.md)
- [Latent Cartridge Specification 0.1](spec/latent-cartridge/README.md)
- [LC Manifest JSON Schema 0.1](spec/latent-cartridge/manifest.schema.json)
- [MiniMax H3 Codec Profile 0.1](spec/codec-h3/README.md)
- [Codec Pack installation contract 0.1](spec/codec-pack/README.md)
- [Worker Protocol 1](spec/worker-protocol/README.md)
- [Deck Signal Contract 0.1](spec/deck-api/README.md)
- [Decoded MP4 output boundary](crates/output-mp4/README.md)
- [LatentPlayer PLAY/PREPARE workflow](apps/latentplayer/README.md)
- [Python Cartridge SDK, raw H3 packer, and converter](sdk/python/README.md)
- [ComfyUI-LatentCartridge recorder](comfy/latent-cartridge/README.md)
- [LatentDeck Comfy Toolkit](comfy/toolkit/README.md)
- [Explicit-install Operator API 0.1](spec/operator-api/README.md)
- [Agent runway](AGENTS.md)
- [Repository layout](docs/repository/REPOSITORY_LAYOUT.md)
- [Public repository boundary](docs/repository/REPOSITORY_BOUNDARY.md)
- [Public release checklist](docs/repository/PUBLIC_RELEASE_CHECKLIST.md)
- [Local Windows release engineering](docs/release/README.md)
- [Diagnostics and sanitized support bundles](docs/repository/DIAGNOSTICS.md)
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
