# Repository bootstrap handoff

## Current state

This repository was prepared as a clean local foundation for future LatentDeck
development. At bootstrap time it contains documentation, empty component
boundaries, and repository-safety tooling only. The owner may keep ignored,
project-local generated interface sketches as visual references, but they are
not technical repository inputs.

No product source code, build manifest, dependency lock, model, codec pack,
cartridge, dataset, test payload, executable, installer, CI workflow, remote, or
release was created as part of this bootstrap.

The local Git repository was initialized on branch `main` without a remote.
The owner subsequently authorized the public-safe scaffold as the first local
commit. Publication remains a separate explicit gate.

The approved `docs/main-plan-v01.md` is the authoritative implementation and
release-candidate contract. `latentdeck_0.1-plan.md` retains the underlying
product and architecture rationale. This handoff deliberately does not restate
either document as a second roadmap.

## What the scaffold means

The directory tree mirrors the monorepo shape already selected in the plan. It
reserves boundaries for the applications, Rust crates, isolated Python codec
host, builtin operators, ComfyUI packages, specifications, SDKs, and tests.

Empty directories contain `.gitkeep` only so their intended boundaries survive
version control. They do not imply that APIs, package names, dependencies, or
implementation details have been finalized beyond the plan.

## Inputs present at bootstrap

- The complete 0.1 plan at the repository root.
- Repository policy and public-release safety documents.
- A tracked policy for using optional local interface sketches as broad visual
  references only. The sketches themselves are ignored and are not expected in
  a clone.

## First actions for a future agent

1. Follow the reading order in `AGENTS.md`.
2. Confirm the current user task before creating code or a new plan.
3. Inspect the narrow component in scope.
4. Verify time-sensitive upstream facts before selecting concrete versions.
5. Preserve the public boundary and run the public-tree audit before handoff.

Do not import the previous ComfyUI laboratory, model files, latents, benchmark
outputs, or Python environment wholesale. Reuse knowledge and explicitly
approved source code only; keep heavyweight and private material outside this
repository.
