# LatentDeck agent runway

This repository is in active local `0.1.0` implementation. The approved
implementation contract is `docs/main-plan-v01.md`; repository safeguards and
component boundaries remain mandatory throughout development.

## Required reading order

Before changing anything:

1. Read this file completely.
2. Read `docs/main-plan-v01.md` completely. It is the approved implementation
   and release-candidate contract.
3. Read `latentdeck_0.1-plan.md` completely for the underlying product and
   architecture rationale.
4. Read `README.md`.
5. Read `docs/repository/BOOTSTRAP_HANDOFF.md`.
6. Read `docs/repository/REPOSITORY_BOUNDARY.md`.
7. Read the relevant component directory and specification before editing it.
8. Run `git status --short` and `pwsh -NoProfile -File tools/Test-PublicTree.ps1`.

If the current user instruction conflicts with an older document, the current
instruction wins. Record a deliberate decision instead of silently rewriting
the baseline.

## Scope discipline

- Implement only the task explicitly assigned in the current session.
- Do not infer new scope beyond the approved 0.1 implementation contract and
  the current user instruction.
- Do not create a new roadmap, Technical Design Document, dependency graph, or
  stack migration unless requested.
- Do not treat the concept images as pixel-perfect UI requirements. They are
  visual references, not executable specifications.
- Keep the product boundaries from the plan: Cartridge Standard, LatentDeck,
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

- Do not create or change a remote, push, publish, tag a release, enable a
  service, or upload an artifact without explicit owner authorization.
- Do not choose or add a software license on the owner's behalf.
- Before any commit, inspect the exact candidate set with
  `git status --short` and run `tools/Test-PublicTree.ps1`.
- Before any future push, complete
  `docs/repository/PUBLIC_RELEASE_CHECKLIST.md` and inspect the archive that Git
  would publish.
- Never use `git add -f` to bypass a payload safeguard without explicit review.
- Preserve unrelated user work and do not rewrite history as a cleanup method.

## Engineering contracts inherited from the plan

- `.lc` is codec-neutral, data-only, strictly validated, and never executes
  embedded code.
- H3 is the first codec profile, not the definition of the format.
- Runtime controls are independent from UI controls.
- Realtime latent processing happens before decode; resampling records the
  post-operator latent state before decode.
- No hidden conversion, resize, or re-encode is allowed for incompatible
  cartridges in 0.1.
- Audio metadata may exist in 0.1 cartridges, but audio playback and synthesis
  are out of scope for 0.1.
- Model weights are external codec assets and are not vendored.
- Inputs are untrusted media: validate schema, tensor layout, dtype, sizes,
  hashes, compatibility, and memory limits before runtime allocation.

## Change quality

- Keep changes narrow, modular, deterministic where the plan requires it, and
  easy to remove or replace.
- Add tests at the same boundary as changed behavior.
- Distinguish `verified`, `inferred`, and `proposed` in research and benchmark
  documentation.
- Do not invent measurements. Store reproducible raw evidence separately from
  conclusions, and never commit private or heavyweight raw evidence by default.
- Link durable documentation from `README.md`; avoid machine-specific paths in
  public docs.
- Finish by running the relevant targeted tests, the public-tree audit,
  `git diff --check`, and a final `git status --short` review.
