# Continue: LatentDeck 0.1 owner-UAT closeout

This is the only current handoff for the repository.

## Last completed state

- The application behavior used by the current owner test is clean commit
  `2f00a4bc220c9274027513ce898794f597794f61` on `main`.
- Its ignored local artifact set is
  `artifacts/release-candidate-final-2f00a4b/0.1.0-windows-x64`. The receipt says
  `git_dirty=false`; installer and SBOM hashes are recorded in
  [ACCEPTANCE_STATUS.md](ACCEPTANCE_STATUS.md).
- Owner UAT is active. The owner reports that the product is generally working
  well; no open P0 or P1 defect has been reported at this handoff.
- Player, Library/Collections, LD-D2, LD-Q4, embedded native output, fullscreen,
  portrait and landscape aspect-fit presentation, source identity, Snapshot,
  Live Capture, and Spout2 were already exercised successfully.
- Strict four-source Q4 and duplicate-source functional Q4 were both covered.
  Duplicate inputs remain acceptable for the owner's UI regression pass.
- The owner accepted the measured six-minute D2 XS5, Q4 TOPK, and Q4 Sinkhorn
  stability runs for 0.1. Do not reintroduce the superseded 30-minute gate.
- Standalone conversion of an existing 16:9 H3 AV Safetensors source into a
  validated `.lc` passed while preserving payload bytes and geometry.

The documentation cleanup containing this handoff is newer than the binary
baseline. Any source change after `2f00a4b`, including an accepted UAT fix,
requires a fresh clean RC before publication review.

## Next action

1. Read the newest owner UAT findings and reproduce only the reported surface.
2. If no new functional regression is waiting, add the ComfyUI all-nodes gallery
   described below. It is the one known UAT presentation item not present in
   the `2f00a4b` baseline.
3. Fix minor or cosmetic findings narrowly. Do not alter latent math, geometry,
   worker contracts, source identity, or embedded output unless the report
   demonstrates a regression at that boundary.
4. Run targeted tests, review the exact staged diff, validate the staged public
   candidate in a clean clone, and commit locally on `main` after each coherent
   fix. Do not push.
5. After the owner accepts the corrections, rebuild both unsigned applications
   from the final clean commit and repeat only the affected UAT slices.
6. Proceed to clean-machine lifecycle, signing, and publication only after the
   owner explicitly authorizes those separate actions.

## Why this is next

The functional create, play, synthesize, resample, replay, and Spout paths have
already passed locally. The gallery is the only known presentation surface not
yet available to the owner, while new owner findings take priority because they
are direct evidence from the release-candidate workflow.

## Open threads

- minor or cosmetic findings from the active owner UAT;
- the ComfyUI all-nodes gallery and visual proof;
- a fresh clean RC after the final accepted source commit;
- clean-machine lifecycle, private security contact, signing, and explicit
  owner-authorized publication.

## ComfyUI all-nodes gallery contract

Add `comfy/toolkit/workflows/00_ALL_NODES_GALLERY.json` as a public, data-free,
non-queueable discovery canvas:

- exactly one instance of every 33 Toolkit node, the one official Recorder
  node, and the two reviewed Channel Roll example nodes: 36 repository-owned
  node types in total;
- strict automated equality between the graph's repository-owned node types
  and the combined three `NODE_CLASS_MAPPINGS` registries; no subset-only test;
- clear groups for Cartridge/Conversion, Decode/Offline, XS operators, Labs,
  Diagnostics/Evaluation, Developer/Utilities, Recorder, and External Example;
- no cartridge, weight, raw latent, prompt, private workflow, absolute path, or
  machine-specific value;
- opening is the test: Queue is not required and may be intentionally invalid;
- in the isolated CPU profile, Fit View must show the complete readable canvas
  with no missing or red node cards.

After visual verification, update the Toolkit workflow index and the isolated
Comfy test runbook, and record the evidence in the acceptance status.

## Protected local state

- `docs/CONCEPT.md` has owner-authored tracked modifications.
- `docs/latent_concept.md` is an owner-authored untracked document.
- Local concept PNGs are ignored visual references, not technical
  specifications.
- A LatentDeck application process may be the owner's active UAT session. Do
  not stop it unless the owner asks or a specific reproduction requires it.

Do not edit, stage, delete, or reformat the two owner documents merely to make a
repository-wide check pass. If their unrelated working-tree state affects a
guard, apply the exact staged candidate to a clean local clone and audit there.

## Do not

- Do not create or alter a remote, push, tag, upload, publish, sign, or enable a
  service without explicit owner authorization.
- Do not start RunPod or download more private corpus for this closeout; the
  present local data and duplicates are sufficient for the requested UAT.
- Do not repeat completed CUDA, Spout, fullscreen, or six-minute soak suites
  without a relevant code change or a concrete regression.
- Do not add `.lc`, Safetensors, weights, private media, generated outputs,
  diagnostics, environments, or absolute local paths to Git.
- Do not silently align, resize, crop, re-encode, or substitute incompatible
  cartridges.

## Release-closeout definition of done

- Every owner-reported finding is either fixed and locally committed or
  explicitly accepted by the owner.
- The all-nodes gallery opens cleanly and its strict registry test passes.
- Targeted checks and the aggregate local-equivalent check pass at the final
  source commit.
- A fresh clean, unsigned RC and its receipt bind that exact commit.
- Clean Windows lifecycle, release archive/history, SBOM/license, security
  contact, and signing gates are recorded in the
  [public release checklist](../repository/PUBLIC_RELEASE_CHECKLIST.md).
- Publication still waits for a separate explicit owner command.
