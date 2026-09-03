# LatentDeck agent runway

The owner accepted the completed Protocol 2 modular runtime and the full local
`0.1.0` application surface on 2026-09-03. This repository is now in
coordinated release-documentation and publication preparation. The current
operational handoff is
`docs/release/continue.md`, and accepted evidence versus open publication gates
are tracked in `docs/release/ACCEPTANCE_STATUS.md`. Repository safeguards,
specifications, and component boundaries remain mandatory through release.

## Required reading order

Before changing anything:

1. Read this file completely.
2. Read `README.md`.
3. Read `docs/release/continue.md`.
4. Read `docs/release/ACCEPTANCE_STATUS.md`.
5. Read `docs/repository/REPOSITORY_BOUNDARY.md`.
6. Read the relevant component directory and specification before editing it.
7. Run `git status --short`. If unrelated owner changes are present, preserve
   them and plan to audit the exact staged candidate in a clean local clone.

If the current user instruction conflicts with an older document, the current
instruction wins. Record a deliberate decision instead of silently rewriting
the baseline.

## Scope discipline

- Implement only the task explicitly assigned in the current session.
- Do not infer new scope beyond the accepted 0.1 contracts, current
  release-preparation work, and the current user instruction.
- Do not create a new roadmap, Technical Design Document, dependency graph, or
  stack migration unless requested.
- Do not treat the concept images as pixel-perfect UI requirements. They are
  visual references, not executable specifications.
- Keep the product boundaries: Cartridge Standard, LatentDeck,
  LatentPlayer, Comfy Toolkit, Comfy recorder, codec adapters, SDKs, and APIs
  remain separable.
- Preserve the stable-center rule: cartridges and the realtime signal contract
  are stable; UI, deck implementations, codecs, and workers are replaceable.
- Verify time-sensitive dependency and platform facts before pinning versions.

## Public-repository invariant

Assume every tracked byte may eventually become public.

Never commit:

- `.lc` cartridges or raw latent payloads;
- model weights, checkpoints, decoder assets, or H3 generator components;
- private datasets, prompts, workflows, user media, or generated renders;
- credentials, tokens, signing keys, machine-local configuration, or absolute
  user-machine paths;
- virtual environments, dependency caches, build outputs, diagnostics, or
  copied third-party repositories;
- third-party assets whose source and redistribution permission are not
  recorded.

Tiny test fixtures are not an exception by default. A fixture may enter
`tests/fixtures/public/` only after provenance, license, size, and data-only
safety are reviewed and the repository rules are deliberately updated.

H3 weights and distributable H3 cartridges remain outside the main source
repository. Do not copy files from local ComfyUI, H3, or RunPod workspaces into
this tree merely because they are useful for development.

## Git and publication safety

- Do not create or change a GitHub or other external remote, push, publish, tag
  a release, enable a service, or upload an artifact without explicit owner
  authorization. The local build clone described below may have only its
  automatic local `origin` pointing back to the primary checkout.
- Do not choose or add a software license on the owner's behalf.
- Before any commit, inspect `git status --short`, the exact staged file list,
  and `git diff --cached`. Run `git diff --cached --check`. Run
  `tools/Test-PublicTree.ps1` in the primary checkout only when unrelated local
  changes do not affect it; otherwise apply the staged diff to a clean local
  clone and run the audit there without modifying the owner's files.
- Before any future push, complete
  `docs/repository/PUBLIC_RELEASE_CHECKLIST.md` and inspect the archive that Git
  would publish.
- Never use `git add -f` to bypass a payload safeguard without explicit review.
- Preserve unrelated user work and do not rewrite history as a cleanup method.
- `docs/CONCEPT.md` and any owner-authored untracked concept documents may
  contain unrelated work; do not edit, stage, delete, or reformat them unless
  the owner assigns that exact documentation task.

## Clean release-build workflow

- Make and commit every accepted source change in the primary `main` checkout.
  Never develop or fix code inside an earlier release-build clone.
- After the owner accepts a source commit, create a fresh independent local
  clone for the RC. Use a short Windows path such as
  `X:\ldrc-<short-commit>`; deep paths can exceed MSVC FileTracker limits before
  application code runs.
- Create the clone from committed local source with
  `git clone --no-hardlinks <primary-checkout> <short-build-path>`. The clone's
  local `origin` is not authorization to add or use a GitHub remote. Refuse an
  existing destination instead of mixing it with an older build tree.
- The build clone must resolve to the exact full commit selected from `main`.
  Before building, verify that `git branch --show-current` is exactly `main`,
  `git rev-parse HEAD` is the selected full commit, `git status --short` is
  empty, and `tools/Test-PublicTree.ps1` passes in that clone.
- Run the aggregate final gate with
  `pwsh -NoProfile -File tools/Check-Workspace.ps1` in the exact clean build
  clone, then verify again that `git status --short` is empty. Targeted tests
  alone are not sufficient for the source commit selected for an RC.
- Build the applications from the clean short-path clone with
  `tools/Build-ReleaseCandidate.ps1`. Do not build a release candidate from the
  primary checkout when it contains unrelated or owner-authored working-tree
  changes.
- Accept an artifact set only when its receipt records the expected full commit,
  branch `main`, `git_dirty=false`, the public snapshot identity, installer
  hashes, SBOM, and third-party notices. Keep the complete artifact directory
  together; never combine installers or metadata from different builds.
- The builder writes below the build clone's ignored
  `artifacts/release-candidate/<version>-windows-x64`. Preserve an accepted copy
  in the primary checkout under the new ignored destination
  `artifacts/release-candidate-final-<short-commit>/<version>-windows-x64`.
  Copy the complete version directory only; refuse an existing destination and
  never merge or overwrite a previous candidate.
- If the accepted artifact directory is copied or moved out of the build clone,
  remeasure every file listed in `SHA256SUMS.txt` at the destination and compare
  it with the receipt. A successful filesystem copy is not integrity evidence.
- Any accepted commit after an RC, including release-documentation changes,
  makes that RC an older source snapshot. It may remain useful for comparison or
  ongoing behavior UAT, but rebuild from a new clean clone before calling the
  artifacts current or beginning publication review.
- When a build exposes a real source defect, return to the primary checkout,
  fix and commit it there, then create a new clean build clone. Do not patch the
  generated artifact set or silently reuse the old clone.

## Engineering contracts

- `.lc` is codec-neutral, data-only, strictly validated, and never executes
  embedded code.
- H3 is the first codec profile, not the definition of the format.
- Runtime controls are independent from UI controls.
- Realtime latent processing happens before decode; resampling records the
  post-operator latent state before decode.
- No hidden conversion, resize, crop, alignment, or re-encode is allowed for
  incompatible cartridges in 0.1.
- Audio metadata may exist in 0.1 cartridges, but audio playback and synthesis
  are out of scope for 0.1.
- Model weights are external codec assets and are not vendored.
- Inputs are untrusted media: validate schema, tensor layout, dtype, sizes,
  hashes, finite values, compatibility, and memory limits before runtime
  allocation.
- Deck extensions use only `.ld`; Codec Pack extensions use only `.ldcodec`.
  Do not reintroduce the retired `.lddeck` alias or the legacy adjacent ZIP
  payload name.
- Worker Protocol 2 is the authoritative Player and generic Deck runtime.
  Protocol 1 remains only as an explicit Player-compatible bridge; a Protocol
  2 error must never trigger a hidden fallback.
- Installed Deck and Codec versions are immutable, explicitly enabled, and
  selected by exact identity. Never auto-select the newest version.

## Change quality

- Keep changes narrow, modular, deterministic where the contract requires it,
  and easy to remove or replace.
- Add tests at the same boundary as changed behavior.
- Distinguish `verified`, `inferred`, and `proposed` in research and benchmark
  documentation.
- Do not invent measurements. Store reproducible raw evidence separately from
  conclusions, and never commit private or heavyweight raw evidence by default.
- Link durable documentation from `README.md`; avoid machine-specific paths in
  public docs.
- Finish by running the relevant targeted tests, the staged-candidate
  public-tree audit, `git diff --cached --check`, exact staged review, and a
  final `git status --short` review.
