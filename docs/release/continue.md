# Continue: LatentDeck 0.1 release preparation

This is the only current operational handoff for the repository. It describes
the state a new agent should act on, not the history of the milestone.

## Last completed state

- On 2026-09-03 the owner completed the final manual user pass and accepted the
  Protocol 2 modular-runtime milestone with no known product defect remaining.
- The accepted implementation checkpoint is clean `main` commit
  `3648e7c634c4310767165ce8975129323a5c09f2`.
- Application version remains `0.1.0`. H3 Codec Pack and adapter are `0.2.0`.
  Bundled D2 and Q4 Deck packages are `0.2.1`.
- `.lc` remains the codec-neutral data format. Installable Decks use only `.ld`;
  installable codecs use only `.ldcodec`. The retired `.lddeck` spelling and
  legacy adjacent Codec Pack ZIP must not return.
- Worker Protocol 2 is the authoritative Player and generic Deck path. Protocol
  1 remains only as an explicit Player bridge; Protocol 2 failures never fall
  back silently.
- The common Extensions lifecycle covers inspect, exact expected SHA-256,
  install, verify, enable, disable, repair, remove, list, and compatibility
  matrix operations. Versions are immutable and explicitly selected; there is
  no newest-version auto-selection.
- D2, Q4, and external Decks run through the same Deck SDK, declarative
  faceplate renderer, compatibility resolver, session broker, and generic
  runtime. A third-party Deck can define its own faceplate without changing the
  host application.
- The Codec SDK owns profile validation, source reads, decode, raw import, and
  capture writing. Core retains an integrity-validated cartridge handle and
  cross-checks the adapter's profile receipt before GPU allocation.
- Up to four Deck sessions may stay warm. A fifth is refused without LRU
  eviction. One foreground output lease exists; Live Capture and MP4 pin it
  independently until stopped.
- The ComfyUI gallery is complete: exactly 36 repository-owned nodes, strict
  combined-registry equality, no private payloads, and isolated CPU Fit View
  acceptance with no missing or red cards.

## Accepted evidence

The exact `3648e7c` clean clone passed `tools/Check-Workspace.ps1` and the
public-tree audit:

- LatentDeck frontend: 172 passed;
- LatentPlayer frontend: 49 passed;
- Rust: 694 passed, 0 failed, 21 expected ignored;
- Python: 422 passed;
- Codec Pack curator: 7 passed;
- formatting, Clippy, packaging, H3 setup tooling, Protocol 2 contracts,
  diagnostics, and linked development-pack checks: passed.

The owner then accepted the real CUDA/H3 user paths in Player, D2, and Q4:

- smooth 24 fps presentation and immediate realtime controls;
- role changes, play/pause/restart/loop, and natural non-loop EOF without losing
  the warm session or output;
- Snapshot, Live Capture, immediate capture reuse, replay, and MP4;
- fullscreen, compact sticky monitor, aspect handling, and Spout;
- compatible and incompatible external `.ld` lifecycle with explicit reasons
  and no fallback;
- four warm sessions, fifth-session refusal, Close, and separate Live
  Capture/MP4 output-lease pinning.

The manual pass is owner acceptance, not a signed clean-machine receipt. Exact
artifact hashes belong in generated receipts and `SHA256SUMS.txt`, not in this
source handoff.

## Current local delivery step

The development installations and stale test profiles were removed before the
new first-install pass. After this documentation commit, build one fresh
unsigned set from a new clean short-path clone of the exact final `main` HEAD:

1. Run `tools/Test-PublicTree.ps1` and `tools/Check-Workspace.ps1` in that clone.
2. Run `tools/Build-ReleaseCandidate.ps1` for the two application installers.
3. Run `tools/Build-H3CodecPack.ps1` with the pinned CPython archive,
   `-PackVersion 0.2.0`, and `-RequireCuda` for the H3 setup plus adjacent
   `.ldcodec`.
4. Keep each complete artifact directory with its receipt, checksums, SBOM, and
   notices. Never combine output from different commits.
5. Preserve that exact clean clone as the sole current build clone; remove
   superseded development clones only after exact path and process checks.

The owner will perform the short first-install UAT using those generated
installers. If it passes, the next agent owns final public documentation,
repository presentation, release notes, legal/security review, signing
preparation, and the publication checklist.

## Next action for a fresh agent

1. Read `AGENTS.md`, `README.md`, this file, `ACCEPTANCE_STATUS.md`,
   `MASTER_USER_TEST.md`, `REPOSITORY_BOUNDARY.md`, and
   `PUBLIC_RELEASE_CHECKLIST.md`.
2. Confirm the owner's first-install result and inspect the exact generated
   receipts before calling any artifact current.
3. If the install pass is clean, update only the final public-facing
   documentation and release evidence the owner assigns. Do not reopen the
   completed Protocol 2 architecture without a concrete defect.
4. If the owner reports a defect, reproduce it against the exact receipt,
   repair source only in the primary checkout, add a test at the failed
   boundary, commit locally on `main`, and rebuild from another fresh clone.
5. Complete the open archive/history, attribution/license/SBOM,
   security-contact, signing, and clean-machine publication gates with real
   evidence.
6. Create or change a remote, push, tag, sign, upload, or publish only after the
   owner explicitly authorizes that exact action.

## First-install expectations

- LatentDeck App, LatentPlayer, and H3 Codec Pack are independent current-user
  installations.
- H3 setup requires the exact matching `.ldcodec` beside it and installs no
  decoder, model, cartridge, Deck package, or application.
- On first launch, Extensions provisions trusted bundled D2/Q4, discovers H3,
  and requires explicit enable/selection. The user then selects the accepted
  external TAEH3 file.
- Player opens the selected cartridge through H3 on CUDA. D2/Q4 require a
  compatible complete source set and show exact incompatibility reasons rather
  than converting or substituting data.
- Removing one application must not remove the other application, extension
  packages, decoder selection, or user cartridges. Exact-version H3 removal is
  a separate lifecycle.

## Open release and publication gates

- owner confirmation of the fresh unsigned first-install path;
- final public onboarding, architecture/SDK documentation, release notes, and
  repository presentation review;
- exact Git archive and history inspection;
- attribution, third-party license, artifact SBOM, and distributed-asset review;
- private vulnerability-reporting channel;
- authenticated signing for both application installers, H3 setup, and its
  generated uninstaller;
- signed offline lifecycle on a clean non-admin Windows 11 NVIDIA account;
- explicit owner authority for any remote, push, tag, upload, or release.

## Protected local state

`docs/CONCEPT.md` has owner-authored tracked changes and
`docs/latent_concept.md` is owner-authored and untracked. Do not edit, stage,
delete, or reformat either document. Audit the exact staged candidate in a
clean clone when those files make the primary checkout dirty.

## Do not

- Do not add cartridges, raw latents, decoder/model weights, private media,
  generated output, diagnostics, environments, or local absolute paths to Git.
- Do not restore hardcoded H3/D2/Q4 production paths or duplicate package,
  compatibility, presentation, or session logic in a faceplate.
- Do not hide incompatibility through cast, resize, crop, alignment, re-encode,
  source substitution, or Protocol 1 fallback.
- Do not treat exact SHA-256 or self-declared publisher text as authenticated
  publisher identity.
- Do not patch an old clone or generated artifact set.
- Do not publish anything without separate explicit owner authorization.

Current classification: **owner-accepted local unsigned 0.1 application and
Protocol 2 modular-runtime milestone; fresh installer UAT and publication gates
remain open**.
