# LatentDeck 0.1.0 acceptance status

This is the current release-status record. It separates verified local behavior,
active owner UAT, and publication gates. It does not authorize a remote, push,
tag, upload, signing action, or public release.

Status on 2026-09-01: the complete `0.1.0` product surface is in owner-UAT
closeout. The owner reports that the current experience is generally working
well. The four primary findings are present in a clean local RC; one final
changed-draft `Load + Play` UX correction and its fresh clean RC remain open.

## Previous owner-UAT binary baseline

The previous unsigned local application RC was built from clean commit
`2aa5c304e8d147e42820333f93b42be4181bb7b7` on `main`.

| Artifact | SHA-256 |
| --- | --- |
| `LatentDeck-0.1.0-windows-x64-unsigned-setup.exe` | `b1e6f1a0ad8bc14df2a856dcecae9584b8fd8e9de8b3a7ae445b0754af83f78f` |
| `LatentPlayer-0.1.0-windows-x64-unsigned-setup.exe` | `51b73d6165a3708a159edc958d0d6686ead2d60ebba3051618203c31889241c0` |
| `latentdeck-0.1.0-sbom.cdx.json` | `7c6ab2a1451ed0e0f094e16cb0f55604f6366f1614719d6d711079e02358305c` |
| `THIRD_PARTY_NOTICES.md` | `f99db3adcf79512f4ee8f753b168919a42e475fc46aefe70ef3751db48232991` |

The schema-3 receipt records `git_dirty=false`, 478 files in the public source
snapshot, pinned Node 24.20.0, pnpm 11.24.0, Tauri CLI 2.11.4, Rust 1.93.1,
Spout2 2.007.017 at commit `f49e2f469f8cb25f559a6eaa61a3f5b8173fc100`,
and a CycloneDX 1.5 SBOM with 741 components.

The documentation handoff was written after this binary set. This RC remains
the previous behavior baseline for owner UAT, but any accepted source change
requires a fresh clean RC before publication review.

### Current post-baseline source candidate (owner acceptance open)

The `2aa5c30` binary contains the four primary fixes listed below:

- D2 Live Capture survives expected automatic source-loop reset barriers and
  remains bounded by explicit Stop and the existing validated spool limits.
- D2/Q4 decoded output can be saved as no-clobber, video-only H.264 MP4 at
  intrinsic geometry and 24 fps. Synthetic Windows Media Foundation output was
  finalized and checked as an H.264 MP4 with no audio stream or partial residue.
- Library invalidation refreshes active Deck source options without an Active
  Collection toggle, while preserving loaded and edited draft identities.
- Finished captures can be inserted explicitly into D2 A/B or Q4 A/B/C/D by a
  preflighted bounded worker replacement; unrelated runtime settings and an
  active decoded recorder are retained.
- LatentPlayer exposes a PLAY/PREPARE workflow for bounded raw-H3 preflight and
  `.lc` authoring with explicit recursion, no-clobber collision checks,
  per-item metadata/errors, sequential progress, stop-after-current, and direct
  open of a completed cartridge. Conversion is bound to the preflight payload
  SHA-256; a subsequently changed source fails without writing its destination.

The source commit containing this status adds the final contextual action not
represented by the artifact hashes above: when a next-load slot differs from
its currently playing identity, D2 and Q4 expose `Load + Play`, which applies
the complete compatible draft and starts the requested slot. Matching
identities retain ordinary transport-only `Play`/`Pause` behavior.

Focused Rust, Python, and frontend contract tests cover these boundaries. The
accepted CUDA, Spout, fullscreen, and six-minute stability suites were not
repeated: latent math and the Spout/native-presentation implementation did not
change, while the new post-presentation MP4 handoff has its own focused tests
and affected owner-UAT slice. Live application behavior remains **owner-UAT
pending**, not verified by those synthetic checks.

## Verified locally

### Workspace and contracts

- The final application source passed Rust formatting, strict Clippy across all
  targets, frontend build/lint/tests, Python tests, packaging contracts,
  diagnostic-bundle checks, and the public-tree audit from a clean candidate.
- The final frontend suites reported 93 of 93 LatentDeck tests and 27 of 27
  LatentPlayer tests passing.
- The Python workspace reported 375 passing tests plus 53 subtests. The separate
  real-CUDA D2/Q4 parity selection reported 32 passing tests plus 40 subtests on
  the local RTX 4070.
- LC readers, writers, Python bindings, raw conversion, Recorder, workers,
  Library migrations, resampling, diagnostics, packaging, and public-tree
  safeguards use synthetic public tests; private latents and weights remain
  outside Git.

### Cartridge creation and conversion

- The official Comfy Recorder produced and validated an H3 AV `.lc` while
  retaining its latent passthrough output.
- Standalone `latentdeck-convert` preflighted an external 16:9 H3 AV
  Safetensors payload with visual shape `[1,24,107,48,84]`, audio shape
  `[1,32,2,603]`, decoded geometry `1344x768`, and 362 decoded frames. The
  resulting cartridge passed full validation and preserved the exact payload
  bytes and hash. No private filename or path is stored here.

### Player, Library, Decks, and resampling

- The owner-led application walkthrough covered Library and many-to-many
  Collections, LatentPlayer, LD-D2, and LD-Q4 with private portrait `448x800`
  and landscape `1344x768` cartridges.
- Player and both Decks kept native output embedded in the owning application.
  Fullscreen displayed and restored the same stream. Both aspect ratios used
  aspect fit without hidden stretching or cropping.
- Visible source names and immutable identities remained attached to selected
  cartridges. Q4 exposed all four role assignments, and changing a draft
  selection did not replace already playing sources.
- Incompatible latent geometry produced an explicit refusal. No Deck performed
  a hidden resize, crop, re-encode, or source substitution.
- LD-D2 real-worker proof covered Linear and XS5, causal reset, deterministic
  replay, Snapshot, Live Capture, atomic packing, Library import, validation,
  and replay of each result.
- Strict LD-Q4 proof used four distinct compatible cartridges with distinct
  archive, cartridge, visual-payload, and lineage identities. TOPK and Sinkhorn
  remained distinct; carrier reassignment, donor influence, deterministic
  replay, Snapshot, Live Capture, validation, reload, and partial-file cleanup
  passed. The path-free receipt SHA-256 is
  `b2b22294a8081ea03f8179b1f904ef0946a2e809654746a9aa0892b07e21964e`.
- A second Q4 functional pass deliberately reused one cartridge across slots.
  The worker and UI disclosed the duplicate identity; this is useful for local
  functional UAT but is not counted as independent-source evidence.

### Stability and output

- The owner-approved stability suite ran D2 XS5, Q4 TOPK, and Q4 Sinkhorn for
  360 seconds each. Measured output rates were 23.8279, 23.8862, and 23.9577 fps;
  control-to-processed-frame p95 was 61.714, 55.7192, and 52.0285 ms; intervals
  over two frames were 0.0979%, 0.1535%, and 0.0278%.
- Each mode ended with zero ring backpressure, an empty queue, no progressive
  host/worker/CUDA allocator growth, and no `.partial` residue. Receipt SHA-256:
  `e2aff6b59939772f395f75de62b97252f31cb35ec01adb9809e3411dd29b64ca`.
- The owner accepted these six-minute results for 0.1 and deferred tighter
  frame pacing and longer soak work to a future version. A 30-minute run is not
  an open 0.1 gate.
- The pinned official D3D12 receiver observed the native Spout2 sender with the
  declared name, RGBA8 format, correct portrait and landscape dimensions,
  advancing frames, and responsive sender shutdown in Player, D2, and Q4. See
  [Spout acceptance](../repository/SPOUT_ACCEPTANCE.md).

### ComfyUI surfaces

- A fresh isolated ComfyUI profile built from a clean source state discovered
  all 33 Toolkit nodes, the official Recorder, and both reviewed Channel Roll
  example nodes.
- Recorder E2E imported external 16:9 H3 AV data, preserved its visual/audio
  cadence, explicitly cast visual storage only when requested, passed full Rust
  cartridge validation, and left no temporary residue.
- Eight public, data-free master workflows exist and generate private
  queue-ready copies without unresolved placeholders or public absolute paths.
- One requested visual acceptance item remains: a single non-queueable canvas
  containing all 36 repository-owned Comfy nodes. Its exact contract is in the
  [current handoff](continue.md). Do not record this item as passed until the
  gallery exists, its strict registry test passes, and it opens without missing
  node cards in the isolated UI.

### Codec Pack and release engineering

- The separate H3 Codec Pack was built offline from the pinned CPython 3.13.14
  embed archive and Windows PyTorch 2.13.0+cu130 closure. The
  1,942,789,596-byte archive SHA-256 is
  `0ee8a0c1293526334dc832f8f8a48527e7e0f2d6e3acd8f8a40a375a6135acfb`.
- Archive validation, isolated install, import/CUDA smoke, integrity catalog,
  dependency inventory, SBOM, notices, and exact-version removal passed. The
  pack contains no model weight, generator, ComfyUI, cartridge, or private
  media; the decoder asset remains an explicit external selection.
- Application packaging binds both independent NSIS installers, all three lock
  files, Spout2 provenance/license, the SBOM, and third-party notices in one
  schema-3 receipt and checksum set.

## Active owner UAT

Use the [master-user test guide](MASTER_USER_TEST.md). At this handoff:

- the owner is testing the applications and reports the overall result as good;
- minor and cosmetic findings will be fixed as they are reported;
- the post-baseline candidate must pass D2 multi-loop capture, D2/Q4 decoded
  MP4, Library auto-refresh, captured-source insertion, and LatentPlayer Prepare
  tests before the four findings are closed;
- already accepted heavy CUDA, six-minute stability, embedded-output,
  fullscreen, aspect, source-identity, and Spout paths should be repeated only
  when a relevant change or concrete regression requires it;
- the Comfy all-nodes gallery must still be implemented and visually accepted.

## Open release and publication gates

- **UAT closeout:** resolve or explicitly accept every owner finding and the
  Comfy gallery item, then rebuild the clean RC from the final commit.
- **Clean-machine lifecycle:** verify install, maintenance/upgrade, downgrade
  behavior, independent app removal, Codec Pack lifecycle, recovery, and Spout
  on a separate clean Windows 11 x64 NVIDIA system without ComfyUI.
- **Security contact and trust:** configure a private vulnerability-reporting
  channel and an authenticated publisher trust/signing path.
- **Publication review:** inspect the exact Git archive and history, complete
  SBOM/license/asset review, and obtain explicit owner authorization for the
  remote, push, tag, and release artifacts.

Until those gates are recorded, call this a **local unsigned 0.1.0 release
candidate under owner UAT**, not a published or signed release.
