# Continue: LatentDeck 0.1 owner-UAT closeout

This is the only current handoff for the repository.

## Last completed state

- The previous owner-UAT application build is clean commit
  `9fc7caa02c0edcfcf45bd29207191cc2b7bb16c0` on `main`. Its receipt says
  `git_dirty=false`; installer and SBOM hashes are recorded in
  [ACCEPTANCE_STATUS.md](ACCEPTANCE_STATUS.md).
- Owner UAT is active. The owner confirms that the product otherwise works as
  stated. The prior test set reproduced D2 latent Live Capture stopping after
  one carrier cycle and vertically inverted decoded MP4; their corrections
  still require the affected owner retest in the next complete artifact set.
  The owner also identified the missing public H3 Codec Pack setup as a release
  blocker: engineer-only PowerShell installation is not acceptable onboarding.
- Player, Library/Collections, LD-D2, LD-Q4, embedded native output, fullscreen,
  portrait and landscape aspect-fit presentation, source identity, Snapshot,
  capture hot insertion, decoded recording lifecycle, and Spout2 were already
  exercised successfully apart from the two defects above.
- Strict four-source Q4 and duplicate-source functional Q4 were both covered.
  Duplicate inputs remain acceptable for the owner's UI regression pass.
- The owner accepted the measured six-minute D2 XS5, Q4 TOPK, and Q4 Sinkhorn
  stability runs for 0.1. Do not reintroduce the superseded 30-minute gate.
- Standalone conversion of an existing 16:9 H3 AV Safetensors source into a
  validated `.lc` passed while preserving payload bytes and geometry.

The previous owner-UAT application build addresses four owner-reported 0.1
release findings:

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

The owner accepted the changed-draft UX correction in the prior application
build:

- A changed next-load slot now exposes `Load + Play` in both Decks. It applies
  the complete compatible draft and starts the requested slot; ordinary
  `Play`/`Pause` remains transport-only when draft and runtime identities match.

The affected corrections and the public-onboarding gap have exact local
diagnoses:

- Both reported D2 cartridges fully validate but contain the same 107-slot,
  362-frame payload. The application was loading an independently installed H3
  Codec Pack `0.1.0` built before the D2 multi-loop correction. That pack still
  stops capture at the first carrier reset. Current D2 worker source continues
  Live Capture across expected automatic source-loop reset barriers while
  arbitrary resets still abort rather than silently joining unrelated latent
  state; those corrected worker bytes remain pending delivery and owner retest
  in Codec Pack `0.1.1`; application installers do not and must not replace
  Codec Packs.
- The shared Media Foundation path declared positive top-down RGB32 stride but
  also reversed RGBA rows during channel packing. The current correction keeps
  top-down row order for both D2 and Q4. An asymmetric-row unit test covers the
  regression.
- The physical Codec Pack runtime smoke now exercises D2 and Q4 loop reset and
  resume transitions without a decoder, GPU, or payload. It fails on the stale
  installed D2 pack and must pass on the new pack before delivery.
- The public H3 delivery must add a small
  `LatentDeck-H3-CodecPack-0.1.1-setup.exe` beside the exact matching
  `LatentDeck-H3-CodecPack-0.1.1-windows-x64.zip`. The user runs setup; no
  PowerShell, system Python, network access, model download, UAC, custom path,
  or preinstalled application is allowed. Setup installs to the fixed
  current-user Codec Pack root, keeps all maintenance bytes outside the
  integrity-closed pack directory, and registers exact-version removal in
  Windows Installed Apps.
- Pack versions remain immutable and may coexist side by side up to the Core
  bound of 16 versions. The stale `0.1.0` pack contains the reproduced D2 loop
  defect, is not a rollback, and must be removed rather than offered as a
  recovery version.

The source candidate containing this handoff is newer than that binary
baseline. Any source change after `9fc7caa`, including this setup work,
requires a fresh clean RC before publication review.

## Next action

1. Finish the public H3 Codec Pack setup, native lifecycle helper, setup
   receipts/checksums, focused tests, and this release documentation in the
   primary checkout. Audit and commit the accepted candidate locally on `main`.
2. Delete the superseded RC clone and create one fresh short-path clone at that
   exact commit. From it, build both unsigned applications, corrected H3 Codec
   Pack `0.1.1`, and `LatentDeck-H3-CodecPack-0.1.1-setup.exe`.
3. Remove the known-defective local `0.1.0` pack. Keep the exact `0.1.1` payload
   ZIP beside setup, install through setup, restart the applications, and
   confirm Codec Manager selected `0.1.1`.
4. Repeat only the affected owner-UAT slices: keep D2 Live Capture active across
   at least three carrier loops, manually stop it, and confirm visual `T` is
   greater than the carrier's 107 slots; repeat one short Q4 loop crossing; then
   record D2 and Q4 MP4 and confirm both are upright.
5. Exercise the short public setup lifecycle: missing adjacent ZIP refusal,
   successful current-user install, Installed Apps entry, exact-version
   uninstall preserving both applications and user data, then reinstall for
   continued UAT.
6. If an affected slice regresses, fix it narrowly in the primary `main`
   checkout, rerun targeted tests, audit the staged candidate in a clean local
   clone, and commit locally. Do not push.
7. If no new functional regression is waiting, add the ComfyUI all-nodes gallery
   described below and record its separate visual acceptance.
8. After the owner accepts all corrections, rebuild the full unsigned
   application and Codec Pack artifact sets from the final clean commit.
9. Proceed to clean-machine lifecycle, signing, and publication only after the
   owner explicitly authorizes those separate actions.

## Why this is next

The broad create, play, synthesize, resample, replay, and Spout paths have
already passed locally. The source changes above touch only newly reported
boundaries; repeating the previously accepted CUDA, Spout, fullscreen, or
six-minute suites would not add relevant evidence. The affected owner workflow
and the separate gallery item are the remaining direct acceptance surfaces.

## Open threads

- owner acceptance of D2/Q4 multi-loop capture from H3 Codec Pack `0.1.1` and
  upright D2/Q4 decoded MP4;
- owner acceptance of the public H3 setup plus adjacent payload and
  exact-version Windows Installed Apps removal;
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
- A fresh clean, unsigned RC plus H3 Codec Pack setup/payload set and their
  receipts bind that exact commit. Setup requires its exact adjacent ZIP and
  needs no PowerShell, system Python, network, model, or elevation.
- Clean Windows lifecycle, release archive/history, SBOM/license, security
  contact, authenticated setup/uninstaller signing, and publication gates are
  recorded in the
  [public release checklist](../repository/PUBLIC_RELEASE_CHECKLIST.md).
- Publication still waits for a separate explicit owner command.
