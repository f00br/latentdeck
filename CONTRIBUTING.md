# Contributing to LatentDeck

LatentDeck welcomes focused contributions to its formats, tools, applications,
documentation, operators, Decks, and codecs. A useful contribution does not
need to understand the whole system: the project is deliberately divided into
versioned extension surfaces.

By participating, you agree to follow the [Code of
Conduct](CODE_OF_CONDUCT.md).

## Choose the contribution route

- Ask usage questions and share early experiments in GitHub Discussions.
- Open an Issue for a reproducible defect or a proposal that changes a shared
  contract.
- Send a pull request for a scoped, tested change.
- Publish an extension in an independent repository when it does not need a
  core change. It can be proposed for inclusion later.

Start from the [developer hub](docs/developers/README.md). The
[research-to-extension guide](docs/developers/RESEARCH_TO_EXTENSION.md) helps
choose between a Toolkit experiment, installable operator, realtime Deck, and
Codec Pack.

## Forks, collaborators, and branches

External contributors normally fork the repository and open a pull request
from their fork. Trusted collaborators may create a branch in the main
repository. Both routes use the same review and test process; nobody pushes
feature work directly to `main`.

Use a short descriptive branch name, such as:

```text
feature/temporal-feedback-operator
feature/example-codec
fix/h3-timing-validation
docs/deck-authoring
```

Keep unrelated changes in separate pull requests. Do not rewrite another
contributor's branch without agreement.

## Before writing code

1. Read [AGENTS.md](AGENTS.md) and the relevant normative specification.
2. Search existing Issues and Discussions.
3. For a shared contract, large dependency, or cross-component behavior,
   propose the direction before implementation.
4. Run `git status --short` and preserve unrelated local changes.

## Pull request requirements

A pull request should:

- explain the user or developer problem and the chosen boundary;
- include focused tests for success and failure behavior;
- update user/developer documentation when behavior changes;
- preserve `.lc`, Deck, Codec, signal, and protocol compatibility unless the
  contract change is explicit;
- identify new dependency provenance and license terms;
- contain no private media, latent payloads, model assets, secrets, machine
  paths, generated environments, or unreviewed third-party material;
- pass the relevant component checks and the public-tree gate.

Run the full local gate when practical:

```powershell
pwsh -NoProfile -File tools/Check-Workspace.ps1
```

At minimum, run the focused tests plus:

```powershell
pwsh -NoProfile -File tools/Test-PublicDocumentation.ps1
pwsh -NoProfile -File tools/Test-PublicTree.ps1
pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1
git diff --check
```

The pull request template records any check that could not be run. A maintainer
may request a clean Windows, GPU, package-lifecycle, or compatibility test when
the affected boundary requires it.

## Extension contribution expectations

- **Cartridge tooling:** use the canonical SDK and validator; never create a
  second permissive parser.
- **Research operators:** start with synthetic tensors and the Toolkit test
  hooks. State the topology, bypass, determinism, and measurable behavior.
- **Decks:** include package metadata, operator descriptor, declarative
  faceplate, integrity catalog, tests, and license notices. Use your own
  reverse-DNS namespace.
- **Codecs:** keep weights and generator code external; implement the complete
  Codec SDK and Protocol 2 contract, including capture and failure behavior.
- **Research notes:** separate observations from interpretations and proposals.
  Do not present one machine or one visual result as a universal claim.

## AI-assisted contributions

AI-assisted changes are welcome. The person opening the pull request remains
responsible for understanding the diff, verifying its tests, disclosing any
material third-party source, confirming redistribution rights, and removing
private data or credentials. Generated output is not evidence by itself.

## Licensing

LatentDeck does not require a CLA or DCO for the preview release. By submitting
a contribution, you agree to license it under the repository's [Apache License
2.0](LICENSE) using an inbound-equals-outbound model. Do not submit code or
assets that you do not have the right to contribute.

External model assets, codecs, cartridges, and media retain their own licenses;
placing them near this project does not relicense them.
