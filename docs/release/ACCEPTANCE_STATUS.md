# LatentDeck 0.1.0 acceptance status

This record separates locally verified release-candidate behavior from gates
that require additional private media, hardware time, a clean machine, or
publication credentials. It does not authorize a remote, push, tag, upload, or
public release.

Status recorded on 2026-08-30: the source tree and unsigned local Windows
release candidate are code-complete for `0.1.0`; the external acceptance gates
listed below are still open.

## Verified locally

- `tools/Check-Workspace.ps1` passed with Rust formatting, Clippy, Rust tests,
  frontend checks/tests/builds, Python lint, 238 Python tests, diagnostic-bundle
  validation, the public-tree audit, and whitespace checks. Two optional CUDA
  parity tests are excluded from that aggregate command by default and were run
  separately.
- The optional D2 and Q4 CUDA parity cases passed: 2 tests and 2 subtests.
- The private LD-D2 worker proof passed 3 of 3 tests using two independent real
  H3 cartridges and an explicitly selected external decoder. It covered Linear
  and XS5 output, causal reset and deterministic replay, Snapshot, Live Capture,
  atomic `.lc` packing, validation, Library import, and playback of the result.
  No private cartridge, decoder, or machine-local path is stored here.
- The opt-in LD-Q4 worker proof passed with four independently generated
  synthetic cartridges on the real CUDA worker. It covered TOPK and Sinkhorn,
  carrier reassignment, relative donor influence, deterministic reset/replay,
  Snapshot, Live Capture, bounded spooling, and validated resample packing.
- The native Spout2 sender was observed by the pinned official D3D12 receiver
  at the declared sender name, 448 by 800 frame size, and RGBA8 format. Sender
  shutdown remained responsive. See
  [Spout acceptance](../repository/SPOUT_ACCEPTANCE.md).
- `tools/Test-ReleasePackaging.ps1` passed the independent-application and H3
  Codec Pack lifecycle contract. The local release set contains two unsigned,
  Spout-enabled NSIS installers, a receipt, checksums, build commands, and a
  CycloneDX SBOM; it contains no Codec Pack, weights, cartridges, raw latents,
  private media, updater, or signing material.
- `tools/Test-DiagnosticBundle.ps1` passed bounded collection, field allowlists,
  secret/path removal, exact archive layout, atomic finalization, and
  no-overwrite behavior.
- Public screenshots were captured from the running Tauri applications against
  isolated empty data and codec roots, not from concept sketches or private
  cartridges.

## Open external acceptance gates

- **Real four-source Q4 corpus:** only three independent compatible private H3
  cartridges were locally available. A duplicate is deliberately not counted
  as a fourth source. The real four-cartridge Carrier plus three Donors cycle
  therefore remains unverified even though the full worker path passed with
  four independent synthetic cartridges.
- **Realtime soak:** the four separate 30-minute 448 by 800 at 24 fps D2
  Linear, D2 XS5, Q4 TOPK, and Q4 Sinkhorn performance runs have not been
  recorded. No FPS, latency, queue, RAM, or VRAM acceptance claim is inferred
  from short functional tests.
- **Clean-machine lifecycle:** the two installers and a real Codec Pack have not
  completed the full install, upgrade, downgrade, uninstall, recovery, and
  Spout matrix on a separate clean Windows 11 x64 NVIDIA machine without
  ComfyUI.
- **Distributable H3 Codec Pack:** the packaging and lifecycle tooling is
  verified with synthetic fixtures, but provenance-reviewed portable runtime
  bytes, dependency inventory, complete notices, an authenticated archive
  trust anchor, and clean-machine runtime proof have not been supplied.
- **Signing and publication:** the installers are intentionally unsigned. A
  code-signing certificate, owner-approved publication review, remote push,
  Git tag, and release upload remain separate explicit-owner gates.

Until every applicable external gate is recorded, call this artifact set the
**local unsigned 0.1.0 release candidate**, not a published or signed release.
