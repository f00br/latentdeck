# LatentDeck documentation

Choose the route that matches what you want to do. Normative specifications
define compatibility; guides explain how to use those contracts without
duplicating them.

## Artists and performers

1. [Install the Windows preview](guides/WINDOWS_INSTALL.md).
2. Follow the complete [artist workflow](guides/ARTIST_WORKFLOW.md) from raw H3
   latent or ComfyUI recording to playback, synthesis, resampling, MP4, and
   Spout.
3. Use the [diagnostics guide](repository/DIAGNOSTICS.md) when a reproducible
   failure needs a public-safe support bundle.

## Developers and agents

- [Developer hub](developers/README.md) — select a supported extension point.
- [Quickstart](developers/QUICKSTART.md) — prepare and verify a source checkout.
- [Cartridges](developers/CARTRIDGES.md) — read, validate, write, and derive
  `.lc` media.
- [Operators](developers/OPERATORS.md) — prototype latent transforms in the
  Comfy Toolkit.
- [Decks](developers/DECKS.md) — package an operator and declarative faceplate
  as a realtime `.ld` extension.
- [Codecs](developers/CODECS.md) — support a new latent family through a
  `.ldcodec` package and Worker Protocol 2.
- [Research to extension](developers/RESEARCH_TO_EXTENSION.md) — move from an
  open question to a measured, reviewable contribution.
- [Compatibility matrix](developers/COMPATIBILITY.md) — current public version
  and toolchain identities.

Automated coding agents must also follow the public [contributor and agent
guide](../AGENTS.md).

## Concepts and research

- [Concept overview](concepts/OVERVIEW.md) — what is implemented, what is a
  design principle, and what remains research.
- [Latent as a medium](concepts/LATENT_AS_MEDIUM.md) — the project's artistic
  perspective, explicitly separated from normative claims.
- [Research directions](research/RESEARCH_DIRECTIONS.md) — bounded starting
  points for experiments rather than a product roadmap.

## Normative specifications

- [Latent Cartridge 0.1](../spec/latent-cartridge/README.md) and its
  [manifest schema](../spec/latent-cartridge/manifest.schema.json)
- [H3 codec profile 0.1](../spec/codec-h3/README.md)
- [Deck Package v1](../spec/deck-package/README.md)
- [Deck Signal Contract 0.1](../spec/deck-api/README.md)
- [Codec Package v2](../spec/codec-pack/README.md)
- [Worker Protocol 2](../spec/worker-protocol/README.md)
- [Comfy Toolkit Operator API 0.1](../spec/operator-api/README.md)

## Repository and maintenance

- [Repository layout](repository/REPOSITORY_LAYOUT.md)
- [Public repository boundary](repository/REPOSITORY_BOUNDARY.md)
- [Pinned toolchains](repository/TOOLCHAINS.md)
- [Spout acceptance evidence](repository/SPOUT_ACCEPTANCE.md)
- [Release process](maintainers/RELEASE_PROCESS.md)
- [Release validation](maintainers/RELEASE_VALIDATION.md)
- [GitHub settings](maintainers/GITHUB_SETTINGS.md)
- [Preview release notes](releases/0.1.0-preview.1.md)
- [Changelog](../CHANGELOG.md)

## Community

- [Contributing](../CONTRIBUTING.md)
- [Governance](../GOVERNANCE.md)
- [Support](../SUPPORT.md)
- [Security](../SECURITY.md)
- [Code of Conduct](../CODE_OF_CONDUCT.md)
