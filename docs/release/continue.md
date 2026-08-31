# Continue: LatentDeck 0.1 owner-UAT closeout

This is the only current handoff for the repository.

## Last completed state

- The previous owner-UAT application build is clean commit
  `2aa5c304e8d147e42820333f93b42be4181bb7b7` on `main`. Its receipt says
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

The previous owner-UAT build addresses four owner-reported 0.1 release
findings:

- D2 Live Capture now continues across expected automatic source-loop reset
  barriers instead of finishing at the first loop; arbitrary resets still
  abort rather than silently joining unrelated latent state.
- D2 and Q4 can record the intrinsic decoded sequence consumed by their
  presentation path as video-only H.264 MP4 at 24 fps. This output is separate
  from latent resampling, uses a bounded background encoder, never overwrites a
  destination, and exposes no partial final file.
- Library imports and completed captures invalidate active Deck source views
  automatically. Refresh preserves currently playing and next-load draft
  identities; stale asynchronous responses are ignored.
- A completed capture has explicit per-slot `Use capture in …` actions. The
  candidate is fully resolved and checked before a bounded worker replacement,
  while the other source draft, controls, roles, seed, transport intent, and
  decoded-video recorder are retained.
- LatentPlayer now has a `Prepare` workspace for explicit raw-H3 file/folder
  preflight, metadata inspection, no-clobber sequential `.lc` conversion,
  progress and per-item errors, cooperative stop-after-current, and direct
  `Open in Player`. Conversion is bound to the payload SHA-256 approved by
  preflight, so a changed source fails without producing an `.lc`. The console
  converter remains the developer interface.

The current source candidate adds one final UX correction:

- A changed next-load slot now exposes `Load + Play` in both Decks. It applies
  the complete compatible draft and starts the requested slot; ordinary
  `Play`/`Pause` remains transport-only when draft and runtime identities match.

The four primary changes above were built from clean commit `2aa5c30`. That
owner-UAT candidate exposed the final changed-draft Play ambiguity addressed by
the contextual action above. Do not record the affected source-insertion slice
as closed until it passes in a fresh application build.

The source candidate containing this handoff is newer than that binary
baseline. Any source change after `2aa5c30`, including this UAT fix, requires a
fresh clean RC before publication review.

## Next action

1. Build a fresh local application candidate from the clean commit containing
   the contextual `Load + Play` fix; the `2aa5c30` artifacts remain an older
   comparison baseline.
2. Repeat the final affected owner-UAT slice: choose a newly captured cartridge
   in a running D2/Q4 slot, press the contextual `Load + Play`, and verify that
   the selected source loads and starts without reloading the application.
3. If an affected slice regresses, fix it narrowly in the primary `main`
   checkout, rerun targeted tests, audit the staged candidate in a clean local
   clone, and commit locally. Do not push.
4. If no new functional regression is waiting, add the ComfyUI all-nodes gallery
   described below and record its separate visual acceptance.
5. After the owner accepts all corrections, rebuild both unsigned applications
   from the final clean commit.
6. Proceed to clean-machine lifecycle, signing, and publication only after the
   owner explicitly authorizes those separate actions.

## Why this is next

The broad create, play, synthesize, resample, replay, and Spout paths have
already passed locally. The source changes above touch only newly reported
boundaries; repeating the previously accepted CUDA, Spout, fullscreen, or
six-minute suites would not add relevant evidence. The affected owner workflow
and the separate gallery item are the remaining direct acceptance surfaces.

## Open threads

- owner acceptance of the final changed-draft `Load + Play` workflow;
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
