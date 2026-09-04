# LatentDeck contributor and agent guide

This file is the operational entry point for human contributors and automated
coding agents. Read it before changing the repository. The same boundaries,
tests, and review requirements apply regardless of who authored a change.

## Start with the right contract

1. Read [README.md](README.md) and the [documentation hub](docs/README.md).
2. Read the specification and developer guide for the surface being changed.
3. Read the component's own README and tests before editing it.
4. Run `git status --short` and preserve unrelated work in the checkout.
5. Keep one contribution focused on one reviewable problem.

| Change | Guide | Normative contract |
| --- | --- | --- |
| Cartridge format or tooling | [Cartridges](docs/developers/CARTRIDGES.md) | [LC 0.1](spec/latent-cartridge/README.md) |
| Research operator | [Operators](docs/developers/OPERATORS.md) | [Operator API](spec/operator-api/README.md) |
| Realtime Deck | [Decks](docs/developers/DECKS.md) | [Deck Package](spec/deck-package/README.md) and [Deck Signal](spec/deck-api/README.md) |
| Codec adapter or package | [Codecs](docs/developers/CODECS.md) | [Codec Package](spec/codec-pack/README.md) and [Worker Protocol](spec/worker-protocol/README.md) |
| User-facing behavior | [Artist workflow](docs/guides/ARTIST_WORKFLOW.md) | Relevant app/component README |
| Release tooling | [Release process](docs/maintainers/RELEASE_PROCESS.md) | [Release validation](docs/maintainers/RELEASE_VALIDATION.md) |

If prose conflicts with a normative specification, the specification wins.
Update the contract and its boundary tests deliberately rather than allowing
implementations to drift.

## Stable center and replaceable parts

The `.lc` cartridge and realtime signal contract are the stable center.
Applications, user interfaces, Deck implementations, codec adapters, workers,
and output integrations are replaceable components with independent versions.

Keep these boundaries intact:

- `.lc` is codec-neutral, data-only, strictly validated untrusted media. It
  never installs or executes embedded code.
- H3 is the first codec profile, not the definition of `.lc`.
- Realtime latent processing occurs before decode. Snapshot and Live Capture
  record the post-operator latent state before decode.
- Runtime controls are independent from their current UI presentation.
- Deck extensions use `.ld`; Codec Pack extensions use `.ldcodec`. Do not
  restore the retired `.lddeck` alias or legacy adjacent ZIP payload.
- Worker Protocol 2 is authoritative for Player and generic Deck runtimes.
  Protocol 1 is an explicit Player bridge, never a hidden fallback.
- Installed extension versions are immutable, explicitly enabled, and selected
  by exact identity. Never auto-select the newest version.
- Incompatible cartridges are refused. Do not hide a conversion, resize, crop,
  alignment, cast, re-encode, profile substitution, device fallback, or source
  substitution inside a loader or Deck.
- Audio metadata may be preserved, but audio playback and synthesis are outside
  the 0.1 product contract.

Inputs are untrusted media. Validate archives, strict JSON, tensor layout,
dtype, dimensions, sizes, hashes, finite values, compatibility, and checked
memory bounds before runtime allocation. Decks, Codec Packs, and installed
operators may execute code with the user's authority; process isolation is not
a security sandbox.

## Public repository boundary

Assume every tracked byte can become public. Follow the [repository
boundary](docs/repository/REPOSITORY_BOUNDARY.md).

Do not add:

- `.lc` cartridges, raw latent payloads, model weights, checkpoints, decoder
  assets, or generator components;
- private datasets, prompts, workflows, user media, or generated renders;
- credentials, tokens, signing material, machine-local configuration, or
  absolute user-machine paths;
- virtual environments, dependency caches, build output, logs, diagnostics,
  databases, or copied third-party repositories;
- third-party assets without recorded origin and redistribution permission.

Tests should generate bounded synthetic data in temporary directories. A tiny
fixture is not automatically safe to publish; use the documented exception
review when a real fixture is essential.

## Change workflow

- Make the smallest coherent change at the owning boundary.
- Preserve backward compatibility unless the relevant versioned contract is
  intentionally revised.
- Add tests at the same boundary as the behavior.
- Keep failures stable, bounded, and path-safe.
- Keep documentation in canonical English and link durable contracts instead
  of duplicating them.
- Mark research statements as `Implemented in 0.1`, `Design principle`, or
  `Research direction`. Do not invent novelty, performance, or compatibility
  claims.
- Record source, version, license, purpose, and lock-file effects for every
  dependency change.
- Do not weaken an allowlist, hash check, no-clobber rule, atomic publication
  rule, or package-trust boundary merely to make a test pass.

For code changes, run the focused component checks first. Before requesting
review, run:

```powershell
pwsh -NoProfile -File tools/Test-PublicDocumentation.ps1
pwsh -NoProfile -File tools/Test-PublicTree.ps1
pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1
pwsh -NoProfile -File tools/Check-Workspace.ps1
git diff --check
git status --short
```

Review the exact diff and ensure unrelated files remain untouched. The complete
workspace gate is required for a release candidate, even when focused tests
passed earlier.

## Git and release safety

Use a branch and pull request for shared development. Do not rewrite public
history or bypass repository protection. Do not change remotes, push, tag,
publish a release, upload artifacts, enable an external service, or use signing
material unless a maintainer has explicitly assigned that action.

Release candidates are built only from a clean, committed `main` checkout in a
fresh short-path clone. Never patch an old build clone or combine artifacts
from different commits. A documentation change after a build makes that binary
set an older source snapshot; rebuild before publication review.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the public collaboration workflow and
[GOVERNANCE.md](GOVERNANCE.md) for decision authority.
