# LatentDeck native output

This crate owns the reusable Tauri raw-window lifecycle around the DX12-only
presentation primitives in `latentdeck-gpu`. The owning application supplies a
stable window label and title, then owns resize, fullscreen, visibility, and
frame sequencing through `NativeOutput`.

It contains no codec, playback, synthesis, WebView, or Spout policy. RGB frames
must already satisfy RGB Ring ABI 1; backend fallback and hidden conversion are
not performed.
