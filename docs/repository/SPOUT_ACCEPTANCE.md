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
Windows 11 release gate or the owner-approved six-minute pre-master stability
suite recorded in the acceptance status.

## 2026-08-31 — final application-surface walkthrough

Status: **verified on the development machine**.

- LatentPlayer, LD-D2, and LD-Q4 kept their native output inside the owning
  application window and published the same stream through Spout2.
- A portrait `448x800` cartridge and landscape `1344x768` cartridges retained
  their intrinsic geometry. The receiver reported the matching sender
  dimensions, and presentation used aspect fit without hidden stretch or crop.
- Fullscreen displayed the active stream and returned to the embedded surface.
- Sender sequences advanced during playback and receiver shutdown remained
  responsive.
- Visible cartridge identity remained attached to the selected source through
  Player and Deck playback; private cartridge names and hashes are omitted from
  this public record.

This closes the local application-surface replay requested after the embedded
presentation changes. Clean-machine, signing, and publication gates remain
separate.
