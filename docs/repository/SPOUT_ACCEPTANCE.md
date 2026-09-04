# Spout2 output and validation

LatentPlayer, D2, and Q4 can publish the intrinsic decoded output through
Spout2 on Windows. This page defines what the integration guarantees and how a
release reviewer can reproduce the interoperability check without committing
private media.

## Output boundary

- The native DX12 render texture is shared without CPU readback, image
  encoding, or an RGB fallback transport.
- The sender publishes the decoded stream's intrinsic width, height, format,
  chosen sender name, and advancing sequence.
- Window letterbox or pillarbox bars are a local aspect-fit decision and are
  not baked into the shared texture.
- Disabling the sender or closing the application releases publication without
  terminating or hanging a conforming receiver.
- LatentPlayer and the generic Deck runtime use the same native output
  integration; an external Deck does not implement or receive its own DX12
  sharing surface.

The pinned upstream Spout2 source, license, and archive identity are recorded in
[`crates/output-spout/UPSTREAM.md`](../../crates/output-spout/UPSTREAM.md).
Receiver executables are validation tools and are not release artifacts.

## Reproduce the receiver check

Use the official upstream `D3D12TextureReceiver` built from the pinned revision.
For LatentPlayer, D2, and Q4:

1. Load and play one compatible source or source set.
2. Start the official receiver.
3. Enable Spout and choose a recognizable sender name.
4. Confirm the receiver lists that exact sender and reports the source's
   intrinsic decoded dimensions.
5. Confirm frames advance without squeeze, crop, baked bars, or a CPU-copy
   presentation path.
6. Enter/leave fullscreen and resize the application; receiver dimensions and
   source aspect remain intrinsic.
7. Disable Spout, then close the application in a second pass. The receiver
   remains responsive and returns to its no-sender state.

Record application, Deck, Codec Pack, adapter, decoder, GPU, driver, receiver
revision, dimensions, frame count/interval, and stable error transitions in the
release evidence. A successful check applies only to that declared
configuration and does not certify every GPU, driver, geometry, or receiver.

Public source evidence must omit cartridge names/hashes, captured frames, and
private media. Store those test inputs separately. The complete clean-release
gate is in [Release validation](../maintainers/RELEASE_VALIDATION.md).
