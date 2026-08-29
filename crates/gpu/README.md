# LatentDeck GPU boundary

This crate owns two replaceable boundaries: RGB Ring ABI 1 and the native
`wgpu` presentation core. It does not depend on Tauri, a codec, or Python.

## RGB Ring ABI 1

ABI 1 is little-endian, single-producer/single-consumer, and fixed to RGBA8
with top-left origin. Production uses an unnamed, pagefile-backed Windows
file-mapping object: it creates no disk file and has no filesystem path.

Core owns the mapping and an unnamed auto-reset frames-ready event. After the
worker process exists, `WindowsRgbRingOwner::duplicate_into` duplicates both
handles into that exact process. `ring.bind` carries only those target-valid
handle values, the exact mapping byte length, layout version, and ring ID.
`WindowsRgbRingProducer::open_from_owned_handles` consumes the worker's handles
and closes them on all success/error paths. Cartridges and control messages
never provide mapped bytes or names.

The mapping is exactly:

```text
4096-byte mapping header
24 * align_up(128-byte slot header + row_stride * height, 4096)
```

`row_stride = align_up(width * 4, 256)` and every `slot_stride` is 4096-byte
aligned. The complete mapping may not exceed 256 MiB.

Mapping header fields:

| Offset | Type | Meaning |
|---:|---|---|
| 0 | `[u8; 8]` | `LDRGBR01` |
| 8 | `u32` | ABI version `1` |
| 12 | `u32` | header bytes `4096` |
| 16 | atomic `u64` | non-zero active generation; zero only during reset |
| 24 | `u32` | slot count `24` |
| 28 | `u32` | slot header bytes `128` |
| 32 | `u32` | pixel format `1` = RGBA8 |
| 36 | `u32` | origin `1` = top-left |
| 40, 44 | `u32` | width, height |
| 48 | `u32` | 256-byte-aligned row stride |
| 52 | `u32` | padded payload bytes |
| 56 | `u64` | slot stride |
| 64 | `u64` | exact mapping bytes |
| 72 | atomic `u64` | latest published sequence |
| 80 | atomic `u64` | latest released sequence |
| 88 | atomic `u64` | consumer claim (`0`/`1`) |
| 96 | atomic `u64` | producer claim (`0`/`1`) |
| 104..4096 | zero | reserved |

Slot header fields:

| Offset | Type | Meaning |
|---:|---|---|
| 0 | atomic `u64` | committed sequence; stored last with release ordering |
| 8 | `u64` | generation |
| 16 | `u64` | producer monotonic timestamp, nanoseconds |
| 24 | `u32` | padded payload bytes |
| 28, 32 | `u32` | width, height |
| 36 | `u32` | row stride |
| 40 | `u32` | pixel format |
| 44 | `u32` | origin |
| 48..128 | zero | reserved |

Each slot then contains exactly `row_stride * height` payload bytes followed by
zeroes up to `slot_stride`. Every per-row byte after `width * 4`, every reserved
header byte, and all slot-tail alignment bytes must be zero. Readers validate
them before returning a frame.

Sequences start at one. The producer may publish only while
`producer_sequence - consumer_sequence < 24`; otherwise `try_write` returns
explicit backpressure and never overwrites an unread slot. The consumer copies
a committed slot before advancing `consumer_sequence`. Release/acquire atomics
publish payload bytes across processes. A complete 5/17-frame H3 cycle is
preflighted through one consistent `RingState`; partial causal-cycle publish is
not required to discover capacity failure.

After committing both slot and producer sequences, the Windows producer calls
`SetEvent`. Notifications may coalesce; counters are authoritative and the
consumer drains until empty after a wake.

Reset accepts only a strictly newer generation. The producer temporarily
stores generation zero, clears all 24 atomic slot commits plus both sequence
counters, then release-stores the new generation. It does not rewrite payload
bytes during reset. The consumer checks generation both before and after its
copy, discards any crossing stale frame, and adopts only the exact generation
from `slot.reset_ack` after confirming every commit/counter is zero. Decode for
the new generation starts only after that acknowledgement/adoption boundary.

Production unsafe code is isolated in private modules: Win32 mapping/event/
handle ownership, mapped-slice construction, and aligned `AtomicU64` views.
All public Windows APIs accept `OwnedHandle` or `BorrowedHandle`, validate the
mapping before use, and preserve workspace-wide `unsafe_code = deny` elsewhere.
`TestFileRgbRingProducer` and `TestFileRgbRingConsumer` retain file mappings
only for deterministic malformed-memory tests; they are not a production
transport and never appear in `ring.bind`.

## Native presentation

`renderer::dx12_instance_descriptor` selects only DX12. The application creates
a `wgpu::Surface` from an owned raw-window-handle provider and passes it to
`Dx12Device::request`; no window toolkit type crosses into this crate.

Decoded frames retain the ring's 256-byte row padding and are uploaded directly
with `Queue::write_texture`. `RgbaFrameRenderer` owns a fixed RGBA8 program
texture, independent of swapchain resize, and records a single fullscreen
triangle into a caller-provided target view. The caller remains responsible for
surface configure/acquire/present, resize, fullscreen, and device-loss policy.
