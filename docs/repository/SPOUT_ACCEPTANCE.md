# Spout2 acceptance evidence

This document records the public-safe acceptance result for the LatentDeck
Spout2 output boundary. Raw captures and private cartridge media are local test
evidence and are intentionally excluded from the repository.

## 2026-08-30 — Windows DX12 receiver proof

Status: **verified on the development machine**.

- Sender: LatentPlayer built with the `spout-sdk` feature.
- Receiver: the official Spout2 `D3D12TextureReceiver` example built from the
  pinned upstream revision recorded in
  [`crates/output-spout/UPSTREAM.md`](../../crates/output-spout/UPSTREAM.md).
- Transport: the native DX12 render texture was shared without CPU readback,
  image encoding, or an RGB fallback path.
- Published sender name: `LatentDeck v0.1 Receiver Proof`.
- Published texture: 448 x 800, `rgba8_unorm`.
- Sequence evidence: the sender advanced monotonically to frame 3,575 while
  the receiver displayed the decoded stream.
- Stop evidence: disabling the sender changed `enabled` and `published` to
  `false`; the receiver stayed responsive and returned to its no-sender
  checkerboard.
- Error state: no Spout error code was reported during publish or shutdown.

This proves interoperability for the tested adapter, GPU, driver, sender, and
official receiver combination. It does not replace the separate clean-machine
Windows 11 release gate or the 30-minute performance soak defined by the 0.1
release plan.
