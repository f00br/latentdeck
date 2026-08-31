# LatentDeck 0.1.0 acceptance status

This record separates locally verified release-candidate behavior from gates
that require additional private media, hardware time, a clean machine, or
publication credentials. It does not authorize a remote, push, tag, upload, or
public release.

Status recorded on 2026-08-31: the `0.1.0` source candidate contains the planned
product surfaces. The owner-led application and Spout walkthrough is complete.
Application installers must still be generated from the final clean tracked
commit; their ignored local receipt is the authority for exact commit, source
snapshot, file sizes, and hashes. Clean-machine, publisher-trust, signing, and
publication gates remain separate until their exact evidence is recorded.

## Verified locally

- The aggregate workspace run passed at clean source commit `379405ba0d76`.
  Rust, pinned-Node-24 frontend, Python, packaging, diagnostic-bundle, and
  public-tree checks all passed from that source state.
- The current Python workspace reports 375 passed tests plus 53 subtests, with
  the two explicitly opt-in CUDA selections skipped by the default command.
  The separate D2 and Q4 CUDA parity selection reports 32 passed tests plus 40
  subtests on the local RTX 4070.
- The private LD-D2 worker proof passed 3 of 3 tests using two independent real
  H3 cartridges and an explicitly selected external decoder. It covered Linear
  and XS5 output, causal reset and deterministic replay, Snapshot, Live Capture,
  atomic `.lc` packing, validation, Library import, and playback of the result.
  No private cartridge, decoder, or machine-local path is stored here.
- The opt-in LD-Q4 worker proof passed with four independently generated
  synthetic cartridges on the real CUDA worker. It covered TOPK and Sinkhorn,
  carrier reassignment, relative donor influence, deterministic reset/replay,
  Snapshot, Live Capture, bounded spooling, and validated resample packing.
- A separate release-class LD-Q4 CUDA proof passed at clean commit
  `6fdca4305fdd` with four independently sourced, full-validated compatible
  private cartridges. The receipt records four distinct cartridge IDs, archive
  hashes, visual-payload hashes, and pairwise-disjoint lineage anchors. Three
  portrait sources and one landscape source reached the common `448x768` grid
  only through explicit provenance-bearing crop operations. TOPK and Sinkhorn
  produced distinct decoded results; carrier reassignment, deterministic
  restart/replay, atomic Snapshot and Live Capture, validation, reload, and
  cleanup with no remaining partial file all passed.
- The same strict four-independent-source Q4 proof was repeated after the final
  runtime and UI fixes at clean commit `379405ba0d76`. Four distinct archives,
  cartridge IDs, visual payloads, and pairwise-disjoint lineage anchors were
  verified. TOPK and Sinkhorn remained distinct, deterministic restart/replay
  passed, carrier reassignment changed the output, Snapshot and Live Capture
  validated and reloaded, and no `.partial` remained. The private, path-free
  receipt SHA-256 is
  `b2b22294a8081ea03f8179b1f904ef0946a2e809654746a9aa0892b07e21964e`.
- The separate duplicate-source LD-Q4 functional proof passed on the same real
  CUDA worker with three distinct private AV archives assigned across four
  slots, with slot D explicitly reusing slot B. The worker acknowledgement,
  capture genealogy, and UI disclosure retain that duplicate identity. This is
  functional evidence only and is deliberately not counted as independent
  four-source corpus acceptance.
- The native Spout2 sender was observed by the pinned official D3D12 receiver
  at the declared sender name, 448 by 800 frame size, and RGBA8 format. Sender
  shutdown remained responsive. See
  [Spout acceptance](../repository/SPOUT_ACCEPTANCE.md).
- The owner-led final application walkthrough passed after the embedded-output,
  source-identity, and fullscreen fixes. It exercised Library and Collections,
  LatentPlayer, LD-D2, and LD-Q4 with private portrait `448x800` and landscape
  `1344x768` cartridges. The visible source names and immutable hashes remained
  bound to the selected cartridges, Q4 disclosed all four role assignments,
  incompatible geometry remained an explicit refusal, and changing a draft
  selection did not replace the already playing sources. Native output stayed
  inside the owning application window, fullscreen displayed and restored the
  same stream, and both geometries used aspect-fit presentation without hidden
  stretching or cropping. Spout published the matching portrait and landscape
  dimensions to the local receiver and stopped responsively. Private media,
  cartridge names, and machine-local paths are intentionally omitted.
- `tools/Test-ReleasePackaging.ps1` passed the independent-application and H3
  Codec Pack lifecycle contract. The current source contract additionally
  generates and rejects drift from the pinned upstream Spout2 CycloneDX
  component, BSD-2-Clause identity, archive/commit provenance, and hash-bound
  third-party notice staging. An earlier application release set contains two
  unsigned Spout-enabled NSIS installers, a receipt, checksums, build commands,
  and a CycloneDX SBOM, but it predates that metadata contract, is not the
  current source candidate, and is not accepted as the final local `0.1.0`
  installer set.
- The separate H3 Codec Pack distributable was built offline from the pinned
  CPython 3.13.14 embed archive and exact Windows PyTorch 2.13.0+cu130 closure.
  The 1,942,789,596-byte archive SHA-256 is
  `0ee8a0c1293526334dc832f8f8a48527e7e0f2d6e3acd8f8a40a375a6135acfb`.
  Archive and post-install isolated import/CUDA smokes passed on an RTX 4070.
  The physical pack is currently installed below the current-user Codec Pack
  root and passed the same isolated CUDA smoke. Its installed manifest,
  integrity catalog, dependency inventory, SBOM, and notices match the archive
  entries byte for byte. It includes generated dependency inventory, CycloneDX
  1.5 SBOM, and bundled third-party license texts; it contains no model weight,
  generator, ComfyUI, cartridge, or private media.
- `tools/Test-DiagnosticBundle.ps1` passed bounded collection, field allowlists,
  secret/path removal, exact archive layout, atomic finalization, and
  no-overwrite behavior.
- Public screenshots were captured from the running Tauri applications against
  isolated empty data and codec roots, not from concept sketches or private
  cartridges.
- A fresh isolated ComfyUI profile was built from clean commit
  `379405ba0d76` with `dirty_at_build=false`. CPU-only server discovery found
  all 33 Toolkit nodes. Recorder E2E imported an external 16:9 H3 AV
  Safetensors payload, preserved its exact `1x24x107x48x84` visual geometry and
  `1x32x2x603` audio cadence, explicitly cast visual storage to FP16, passed
  full Rust cartridge validation, and left no temporary residue. All eight
  queue-ready master workflows were generated with no placeholder or absolute
  machine path.
- The owner-approved pre-master stability suite ran D2 XS5, Q4 TOPK, and Q4
  Sinkhorn for 360 seconds each at clean commit `379405ba0d76`. The measured
  output rates were respectively 23.8279, 23.8862, and 23.9577 fps; control to
  processed-frame p95 was 61.714, 55.7192, and 52.0285 ms; intervals over two
  frames were 0.0979%, 0.1535%, and 0.0278%. Every mode reported zero ring
  backpressure, an empty final queue, no progressive host/worker/CUDA allocator
  growth, and no `.partial` residue. On 2026-08-31 the owner accepted the small
  D2 XS5 and Q4 TOPK frame-rate deviations for `0.1` user-test readiness and
  deferred tighter frame-pacing work to a future version. This is explicitly a
  6-minute stability acceptance, not a claim that the original 30-minute
  performance gate was exercised. The path-free suite receipt SHA-256 is
  `e2aff6b59939772f395f75de62b97252f31cb35ec01adb9809e3411dd29b64ca`.

## Local artifact handoff

- Generate the two unsigned application installers and fresh lock-bound SBOM
  only after the final tracked acceptance commit. The ignored schema-3 receipt
  must name that exact clean commit and bind both installers, all three lock
  hashes, the SBOM, and third-party notices. Do not treat an older installer
  directory as the current candidate merely because its binaries launch.

## Open external acceptance gates
- **Clean-machine lifecycle:** the two installers and a real Codec Pack have not
  completed the full install, upgrade, downgrade, uninstall, recovery, and
  Spout matrix on a separate clean Windows 11 x64 NVIDIA machine without
  ComfyUI.
- **Codec Pack trust and clean-machine proof:** the portable runtime bytes,
  dependency inventory, notices, SBOM, local CUDA smoke, isolated install, and
  uninstall proof are now present. An authenticated publisher trust anchor and
  the separate clean-Windows runtime matrix are still open; the adjacent local
  checksum is transport evidence only.
- **Signing and publication:** the installers are intentionally unsigned. A
  code-signing certificate, owner-approved publication review, remote push,
  Git tag, and release upload remain separate explicit-owner gates.

Until the current artifacts and every applicable external gate are recorded,
call the source a **local 0.1.0 candidate**. Do not call the stale installer set
the final local RC, and do not call any unsigned local artifact a published or
signed release.
