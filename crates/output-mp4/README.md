# Decoded MP4 output

`latentdeck-output-mp4` is the replaceable Windows recording boundary for the
decoded video shown by LD-D2 and LD-Q4. It is deliberately separate from
post-operator latent Snapshot/Live Capture: an MP4 is a viewing derivative, not
a Latent Cartridge and not an input to realtime latent synthesis.

## 0.1 contract

- Input is each exact validated, ABI-padded RGBA frame consumed by the Deck's
  authoritative presentation sequence at its intrinsic decoded width and
  height. Recording stays continuous when a local window presentation is
  temporarily skipped because the surface is occluded or being recreated.
- Output is video-only H.264 in an `.mp4` container at the 0.1 cadence of
  24 fps. Audio tensors and cartridge audio metadata are not rendered.
- The first accepted frame fixes geometry. A later geometry change fails only
  the recording; the Deck keeps presenting.
- Frame handoff uses a small bounded queue on a dedicated encoder thread. An
  encoder stall or overflow fails the recorder instead of blocking playback.
- The chosen final path is absolute, local, `.mp4`, and no-clobber. Bytes are
  written to a uniquely named sibling `.partial.mp4`, finalized, and renamed
  only after success. Cancelling before the first frame creates no file.
- Wire status is path-free and contains only lifecycle, dimensions, frame
  counts, and a stable sanitized error code.

The Windows implementation uses the operating-system Media Foundation Sink
Writer and keeps all COM/Media Foundation objects on the dedicated writer
thread. See Microsoft's [Sink Writer encoding
tutorial](https://learn.microsoft.com/windows/win32/medfound/tutorial--using-the-sink-writer-to-encode-video)
and [`IMFSinkWriter::Finalize`](https://learn.microsoft.com/windows/win32/api/mfreadwrite/nf-mfreadwrite-imfsinkwriter-finalize).

## Application ownership

Each bundled Deck state owns one `DecodedRecordingController`. A bounded source
replacement passes the same controller to the replacement runtime, so explicit
`Use capture in …` can restart the worker without silently ending an active MP4
recording. Latent Snapshot/Live Capture and decoded MP4 recording are mutually
exclusive in 0.1.

Run the focused contract tests with:

```powershell
cargo test -p latentdeck-output-mp4
cargo test -p latentdeck-app decoded_recording::tests
```
