# LatentDeck native output

This crate owns the reusable Tauri raw-window lifecycle around the DX12-only
presentation primitives in `latentdeck-gpu`. The owning application supplies a
stable window label and title, then owns placement, visibility, and frame
sequencing through `NativeOutput`.

LatentPlayer, LD-D2, and LD-Q4 use the same embedded presentation contract in
0.1. Each runtime creates a borderless, non-focusable native child attached to
the main Tauri `WebviewWindow`; decoded output does not open a second program
window. The WebView contains only controls and an empty layout anchor. Decoded
RGB remains on the native wgpu/DX12 path and is never copied into browser media
elements.

## Embedded geometry

The application converts its revisioned CSS anchor measurement with the
authoritative Tauri display scale, validates it against the current main-window
client extent, and passes `NativeOutputBounds` in physical parent-client
coordinates. `NativeOutput::new_embedded` creates a real `WS_CHILD` through the
parent raw HWND and applies the initial bounds before creating the wgpu surface,
so a detached or incorrectly sized output window is never shown.

Every subsequent placement is synchronous. The Windows implementation calls
`SetWindowPos` and then verifies all of the following before reporting success:

- the native output still has the expected parent and `WS_CHILD` style;
- its client origin matches the requested parent-client `x` and `y`;
- its client width and height match the requested physical extent; and
- it is the top child in the parent sibling order.

Failure of any placement or postcondition is returned as a stable native-output
error. Zero-sized or hidden anchors suspend local surface acquisition and hide
the child; they are never converted into invented non-zero geometry.

The child rectangle controls only the local swapchain. The renderer performs a
centered aspect-fit inside that rectangle without crop, stretch, resize, or
re-encode of the decoded frame. The intrinsic frame texture remains at the
cartridge dimensions, and Spout publishes those exact intrinsic dimensions
regardless of the main-window size, display scale, or fullscreen state.

## Hidden and occluded presentation

Every successful `present_padded_rgba` call consumes the supplied frame. A
`Skipped*` outcome means only that the local swapchain did not display it; the
caller must still advance the realtime stream.

When the embedded child is hidden or zero-sized, or surface acquisition is
timed out, occluded, outdated, or lost, the renderer draws the uploaded frame
through a persistent 1×1 offscreen target. This bounded render pass flushes the
upload and normalizes the frame texture to the same combined shader-resource
state used by normal presentation. Spout can therefore submit the intrinsic
texture exactly once on these paths without depending on swapchain visibility.
Outdated and lost local surfaces are reconfigured or recreated after this
normalization; there is no RGB fallback or unbounded hidden-frame queue.

It contains no codec, playback, synthesis, WebView media rendering, or Spout
product policy. RGB frames must already satisfy RGB Ring ABI 1; backend
fallback and hidden conversion are not performed.
