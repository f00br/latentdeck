# LatentDeck 0.1.0 acceptance status

This is the current release-status record. It separates verified local behavior,
active owner UAT, and publication gates. It does not authorize a remote, push,
tag, upload, signing action, or public release.

Status on 2026-09-01: the complete `0.1.0` product surface is in owner-UAT
closeout. The owner confirms that the broader product works as stated,
including Library refresh, hot insertion, and LatentPlayer preparation. Final
retest remains open for two reproduced delivery defects: the prior installed D2
worker ended latent Live Capture after one carrier loop, and D2/Q4 decoded MP4
pixels were vertically inverted. Release onboarding also remains open until the
new public H3 Codec Pack setup and exact-version Windows uninstall lifecycle
are built and accepted; engineer-only PowerShell commands are not sufficient
for a public `0.1` release.

## Previous owner-UAT binary baseline

The previous unsigned local application RC was built from clean commit
`9fc7caa02c0edcfcf45bd29207191cc2b7bb16c0` on `main`.

| Artifact | SHA-256 |
| --- | --- |
| `LatentDeck-0.1.0-windows-x64-unsigned-setup.exe` | `5734f73ce68e50c4e1c8e9450867dd379da603bcc85e42279ba6d17a328254a3` |
| `LatentPlayer-0.1.0-windows-x64-unsigned-setup.exe` | `0c4a58fdedc92fc3bd4d3242f9ff22c6350e047e064286a0a2f8810abf3e7458` |
| `latentdeck-0.1.0-sbom.cdx.json` | `492a5645809c6011c8946ae678367eee8268a773d734e439a377f67f79613203` |
| `THIRD_PARTY_NOTICES.md` | `f99db3adcf79512f4ee8f753b168919a42e475fc46aefe70ef3751db48232991` |

The schema-3 receipt records `git_dirty=false`, 478 files in the public source
snapshot, pinned Node 24.20.0, pnpm 11.24.0, Tauri CLI 2.11.4, Rust 1.93.1,
Spout2 2.007.017 at commit `f49e2f469f8cb25f559a6eaa61a3f5b8173fc100`,
and a CycloneDX 1.5 SBOM with 741 components.

The documentation handoff was written after this binary set. This RC remains
the previous behavior baseline for owner UAT, but any accepted source change
requires a fresh clean RC before publication review.

### Current owner-UAT application behavior (Codec Pack retest open)

The `9fc7caa` application artifact set and corresponding source contain the
primary corrections below. The independently installed Codec Pack remained an
older source snapshot, so the D2 worker correction was not delivered by those
application installers:

- Current D2 and Q4 worker source preserves Live Capture across expected
  automatic source-loop reset barriers and remains bounded by explicit Stop and
  the matching validated spool limits. The stale installed D2 `0.1.0` pack did
  not contain this source and is not valid evidence for that behavior.
- D2/Q4 decoded output can be saved as no-clobber, video-only H.264 MP4 at
  intrinsic geometry and 24 fps. The application binary preserves top-down row
  order before Media Foundation encoding; an asymmetric-row regression test
  catches vertical inversion.
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

The same application binary includes the owner-accepted contextual action: when
a next-load slot differs from its currently playing identity, D2 and Q4 expose
`Load + Play`, which applies the complete compatible draft and starts the
requested slot. Matching identities retain ordinary transport-only
`Play`/`Pause` behavior.

Focused Rust, Python, and frontend contract tests cover these boundaries. The
physical Codec Pack runtime smoke now also proves D2/Q4 capture ownership across
one automatic loop reset and resume without a model, decoder, GPU, or payload.
The accepted CUDA, Spout, fullscreen, and six-minute stability suites were not
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
- That immutable `0.1.0` pack predates the D2 loop-preservation source change.
  It contains the reproduced D2 loop defect, is not a valid rollback candidate,
  and must be removed from the owner test machine. It is historical evidence
  only and must not be offered as a public artifact.
- The next owner-UAT delivery must build the corrected immutable `0.1.1` pack
  and its `LatentDeck-H3-CodecPack-0.1.1-setup.exe` from the same clean final
  source commit. The exact matching
  `LatentDeck-H3-CodecPack-0.1.1-windows-x64.zip` must remain adjacent to setup.
  The current-user setup is offline, has a fixed LocalAppData destination, and
  requires no UAC, PowerShell, system Python, network access, decoder, or model.
- Setup and payload hashes, source identity, toolchain, and lifecycle status
  must be bound by path-free receipts and `SHA256SUMS.txt`. Setup helpers,
  uninstallers, and receipts remain outside the integrity-closed pack
  directory. Windows Installed Apps removes one exact version; immutable
  side-by-side installation remains capped at 16 versions and never overwrites
  another version.
- Application packaging binds both independent NSIS installers, all three lock
  files, Spout2 provenance/license, the SBOM, and third-party notices in one
  schema-3 receipt and checksum set.

## Active owner UAT

Use the [master-user test guide](MASTER_USER_TEST.md). At this handoff:

- the owner confirms all other reported workflows work as stated;
- the fresh candidate must select H3 Codec Pack `0.1.1`, keep D2 capture active
  across at least three carrier loops until manual Stop, cross one short Q4 loop,
  and produce upright D2 and Q4 MP4 files;
- the H3 public setup must reject a missing or mismatched adjacent ZIP without
  a discoverable or partial pack, maintenance tree, or Installed Apps entry,
  install `0.1.1` without developer prerequisites or elevation,
  register exact-version removal in Windows Installed Apps, and uninstall only
  that pack version while preserving both applications and user data;
- already accepted heavy CUDA, six-minute stability, embedded-output,
  fullscreen, aspect, source-identity, and Spout paths should be repeated only
  when a relevant change or concrete regression requires it;
- the Comfy all-nodes gallery must still be implemented and visually accepted.

## Open release and publication gates

- **UAT closeout:** resolve or explicitly accept every owner finding and the
  Comfy gallery item, then rebuild the clean RC from the final commit.
- **Clean-machine lifecycle:** verify the signed application installers and H3
  setup plus adjacent payload, maintenance/upgrade/downgrade behavior,
  independent app removal, exact-version Codec Pack removal, recovery, and
  Spout on a separate non-admin Windows 11 x64 NVIDIA account without
  PowerShell 7, system Python, ComfyUI, or setup-time network access.
- **Security contact and trust:** configure a private vulnerability-reporting
  channel and an authenticated publisher trust/signing path.
- **Publication review:** inspect the exact Git archive and history, complete
  SBOM/license/asset review, and obtain explicit owner authorization for the
  remote, push, tag, and release artifacts.

Until those gates are recorded, call this a **local unsigned 0.1.0 release
candidate under owner UAT**, not a published or signed release.
