# Continue: LatentDeck 0.1 release preparation

This is the only current operational handoff for the repository.

## Last completed state

- On 2026-09-01 the owner explicitly accepted the complete local `0.1.0`
  functional surface with no remaining product defects.
- The owner-accepted source and unsigned artifact baseline is clean `main`
  commit `dbe310a2b8c0a9f78a11ab8217f07c8c91a39db4`.
- LatentDeck App, LatentPlayer, and H3 Codec Pack `0.1.1` were built from that
  commit. Their receipts record `git_dirty=false`, exact hashes, SBOMs, notices,
  and the same 490-file public snapshot.
- The owner confirmed Player, Library/Collections, LD-D2, LD-Q4, 9:16 and 16:9
  presentation, Snapshot, long latent Live Capture, capture hot insertion,
  upright D2/Q4 MP4 recording, presets, Resample, Spout2, and LatentPlayer
  PLAY/PREPARE behavior.
- The corrected H3 Codec Pack setup installed successfully for the current
  user. Player, D2, and Q4 discovered its required CUDA H3 adapter; the local
  install/reinstall/exact-version uninstall lifecycle passed.
- Strict four-source Q4, the owner-approved six-minute D2/Q4 stability runs,
  CUDA parity, and Spout2 evidence remain accepted. Do not repeat those heavy
  tests without a related source change or concrete regression.
- Superseded release-build and evidence clones used during owner UAT were
  removed after the accepted artifact directories had been preserved and
  integrity-checked in the primary checkout.

The accepted `dbe310a` binaries remain the durable functional UAT evidence
snapshot. The release-documentation commit made after them is newer source, so
those binaries are not the final publication candidate. Build the publication
candidate again from the final clean documentation/release commit.

## Next action

The next agent owns release documentation, repository publication preparation,
release presentation, and the eventual release workflow:

1. Read `AGENTS.md`, `README.md`, this handoff,
   `ACCEPTANCE_STATUS.md`, `MASTER_USER_TEST.md`,
   `REPOSITORY_BOUNDARY.md`, and `PUBLIC_RELEASE_CHECKLIST.md` before editing.
2. Create `comfy/toolkit/workflows/00_ALL_NODES_GALLERY.json` under the exact
   contract below. This is an open, required release-presentation task; it was
   not part of the completed functional owner UAT and must not be reported as
   already implemented or tested.
3. Prepare detailed public-facing documentation, onboarding, release notes,
   repository presentation, and any other owner-assigned GitHub/release copy.
   Keep verified facts distinct from proposed text and do not expose local
   paths, private data, or unsupported claims.
4. Complete the Git archive and history review, attribution/license/SBOM review,
   security-contact plan, and every applicable item in the public-release
   checklist. Record only evidence that was actually produced.
5. Commit accepted changes locally on `main`. Create a fresh independent
   short-path clone from the final clean commit, run the aggregate workspace
   gate, and rebuild the complete application and H3 Codec Pack artifact sets.
   Never mix these with the older `dbe310a` binaries.
6. Exercise the final signed artifacts on a clean non-admin Windows 11 NVIDIA
   account: application install/update/uninstall, H3 offline setup plus adjacent
   ZIP, exact-version Codec Pack removal, external decoder selection, Player,
   D2, Q4, recovery, and Spout2.
7. Create or change a remote, push, tag, sign, upload, or publish only after the
   owner gives explicit authority for that exact action.

## Why this is next

There is no known functional product bug left in 0.1. The remaining work turns
the accepted local product into a reviewed, understandable, signed, and
authorized public release. Because documentation and presentation are part of
the public source snapshot, they must be complete before the final clean build.

## ComfyUI all-nodes gallery contract

Add `comfy/toolkit/workflows/00_ALL_NODES_GALLERY.json` as a public, data-free,
non-queueable discovery canvas:

- exactly one instance of every 33 Toolkit node, the one official Recorder
  node, and the two reviewed Channel Roll example nodes: 36 repository-owned
  node types in total;
- strict automated equality between the graph's repository-owned node types
  and the combined three `NODE_CLASS_MAPPINGS` registries; a subset-only test
  is insufficient;
- clear groups for Cartridge/Conversion, Decode/Offline, XS operators, Labs,
  Diagnostics/Evaluation, Developer/Utilities, Recorder, and External Example;
- no cartridge, weight, raw latent, prompt, private workflow, absolute path, or
  machine-specific value;
- opening is the test: Queue is not required and may be intentionally invalid;
- in the isolated CPU profile, Fit View shows the complete readable canvas with
  no missing or red node cards.

After visual verification, update the Toolkit workflow index and the isolated
ComfyUI test runbook, and record the actual evidence in the acceptance status.
Adding the gallery is a new post-acceptance source change, so include it before
building the final clean RC.

## Open release and publication threads

- `00_ALL_NODES_GALLERY.json`, its strict registry test, workflow index, and
  isolated ComfyUI visual proof;
- detailed public documentation, onboarding, release notes, and repository
  presentation;
- exact Git archive/history and legal/attribution/license/SBOM review;
- a private vulnerability-reporting channel and authenticated publisher trust;
- clean-machine signed application and Codec Pack lifecycle evidence;
- explicit owner authority for the remote, push, tag, upload, and release.

## Protected local state

- `docs/CONCEPT.md` has owner-authored tracked modifications.
- `docs/latent_concept.md` is an owner-authored untracked document.
- Local concept PNGs are ignored visual references, not technical
  specifications.

Do not edit, stage, delete, or reformat those owner documents. If they affect a
repository guard, apply the exact staged candidate to a clean local clone and
audit there.

## Do not

- Do not create or alter a remote, push, tag, upload, publish, sign, or enable a
  service without explicit owner authorization.
- Do not start RunPod or obtain new private corpus for release preparation.
- Do not repeat completed CUDA, Spout, fullscreen, or six-minute soak suites
  without a relevant code change or a concrete regression.
- Do not reuse an old build clone, patch generated artifacts, or combine files
  and metadata from different commits.
- Do not add `.lc`, Safetensors, weights, private media, generated outputs,
  diagnostics, environments, or machine-local paths to Git.
- Do not silently align, resize, crop, re-encode, or substitute incompatible
  cartridges.

## Release-preparation definition of done

- The gallery and detailed public documentation are committed and verified.
- The public-release checklist has evidence for every applicable item.
- A fresh clean clone at the final commit passes
  `tools/Check-Workspace.ps1` and `tools/Test-PublicTree.ps1`.
- Fresh application and H3 Codec Pack artifacts, receipts, checksums, SBOMs,
  notices, and signatures all bind that same final commit.
- The clean Windows lifecycle and publisher-trust gates are recorded.
- Publication still waits for a separate explicit owner command.
