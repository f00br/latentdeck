# LatentDeck 0.1.0 acceptance status

This record separates locally verified release-candidate behavior from gates
that require additional private media, hardware time, a clean machine, or
publication credentials. It does not authorize a remote, push, tag, upload, or
public release.

Status recorded on 2026-08-30: the `0.1.0` source candidate contains the planned
product surfaces, while the current installer set predates later source commits
and must be rebuilt. Hardware, current-profile ComfyUI, clean-machine, and
publication gates listed below remain separate until their exact evidence is
recorded.

## Verified locally

- The previous aggregate workspace run passed. After the subsequent source
  changes, targeted Rust, pinned-Node-24 frontend, Python, packaging,
  diagnostic-bundle, and public-tree checks also passed. A final aggregate run
  from the final clean commit is still required before rebuilding installers.
- The current Python workspace reports 345 passed tests plus 53 subtests. The
  optional D2 and Q4 CUDA parity selection reports 32 passed tests plus 40
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
- `tools/Test-ReleasePackaging.ps1` passed the independent-application and H3
  Codec Pack lifecycle contract. An earlier application release set contains
  two unsigned Spout-enabled NSIS installers, a receipt, checksums, build
  commands, and a CycloneDX SBOM, but it is not the current source candidate and
  is not accepted as the final local `0.1.0` installer set.
- The separate H3 Codec Pack distributable was built offline from the pinned
  CPython 3.13.14 embed archive and exact Windows PyTorch 2.13.0+cu130 closure.
  The 1,942,789,598-byte archive SHA-256 is
  `3859aec61a89867b6f7797bd8326b5bdd2eb1764243efc0e5dbb9ae71229839d`.
  Archive and post-install isolated import/CUDA smokes passed on an RTX 4070.
  The physical pack is currently installed below the current-user Codec Pack
  root and passed the same isolated CUDA smoke. It includes generated dependency
  inventory, CycloneDX 1.5 SBOM, and bundled third-party license texts; it
  contains no model weight, generator, ComfyUI, cartridge, or private media.
- `tools/Test-DiagnosticBundle.ps1` passed bounded collection, field allowlists,
  secret/path removal, exact archive layout, atomic finalization, and
  no-overwrite behavior.
- Public screenshots were captured from the running Tauri applications against
  isolated empty data and codec roots, not from concept sketches or private
  cartridges.

## Open external acceptance gates

- **Real four-source Q4 GPU receipt:** four full-validated compatible private
  cartridges now exist at 448 by 768. They have four unique cartridge, archive,
  video-payload, and declared source-lineage identities. Three portrait and one
  landscape source were brought to the common grid only through explicit
  provenance-bearing crop operations. The strict non-GPU preflight passes; the
  current-clean-commit CUDA execution receipt is still pending.
- **Realtime stability:** a legacy 1,800-second D2 Linear receipt records
  23.9335 fps, 52.422 ms control-latency p95, and passing recorded gates. On
  2026-08-30 the owner explicitly replaced the remaining pre-master-test
  30-minute runs with separate 360-second D2 XS5, Q4 TOPK, and Q4 Sinkhorn runs.
  Those shortened runs are still pending and must be labelled as owner-approved
  pre-master-test stability evidence, not as 30-minute performance proof.
- **Current isolated ComfyUI profile:** the existing generated profile and its
  earlier Recorder/API receipts predate the latest Toolkit cadence fix. Rebuild
  it from the final clean commit, then repeat server discovery and Recorder E2E
  before handing it to the master user.
- **Current application artifacts and Spout replay:** rebuild the two unsigned
  installers and SBOM from the final clean commit. The recorded Spout proof is
  valid historical evidence, but the receiver/shutdown check must be repeated
  with the final LatentPlayer binary after later presentation changes.
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
