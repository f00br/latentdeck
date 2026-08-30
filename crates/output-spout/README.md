# LatentDeck Spout2 output

This crate is the narrow Windows texture-sharing boundary for LatentDeck. Its
real path uses the application's `ID3D12Device`, direct command queue, and
`ID3D12Resource`. The bridge creates a D3D11On12 wrapped resource through the
official `SpoutDX12` device and sends it with official `SendDX11Resource`.
There is no encoded, readback, `SendImage`, or CPU-pixel fallback.

The upstream `WrapDX12Resource` helper always releases to `PRESENT`. That is not
safe for LatentDeck's reused wgpu frame texture, whose tracker ends in
`PIXEL_SHADER_RESOURCE`. The bridge instead calls `CreateWrappedResource` on
the official `ID3D11On12Device` with identical input and output states. A frame
submitted as `PixelShaderResource` is returned in `PixelShaderResource`, so the
next wgpu use does not inherit an untracked native state change.

The normal workspace build is network-free and does not require third-party
source. It compiles the safe state machine and deterministic mock tests. To
build the real backend:

```powershell
pwsh -NoProfile -File tools/Prepare-Spout2.ps1
cargo check -p latentdeck-output-spout --features spout-sdk
```

Preparation downloads the exact approved upstream archive into ignored
`vendor-local/`, verifies its archive and critical-file hashes, and writes a
strict pin stamp. After that preparation step, the Cargo/CMake build is fully
offline. `LATENTDECK_SPOUT2_SOURCE_ROOT` may point to another prepared root with
the same exact stamp and `source/` layout.

`SpoutSender` is `Send` but not `Sync`: a single-owner async actor may migrate
between runtime threads, while concurrent calls are rejected by Rust's mutable
borrow rules. The bridge enables `ID3D11Multithread` protection on the official
D3D11On12 device before opening the sender. Call `enable`, submit monotonically
increasing application frame IDs with the exact D3D12 state, and call `stop`.
Spout publication begins only after the first successful frame. Name changes
unregister the old sender and publish the new collision-resolved name on the
next successful frame.

Receiver proof is an integration/release gate: an external Spout receiver must
observe the active name, exact dimensions and format, frame progression, and
clean sender disappearance. Mock tests and a successful static SDK build do not
claim that proof. The current public-safe result is recorded in
[`docs/repository/SPOUT_ACCEPTANCE.md`](../../docs/repository/SPOUT_ACCEPTANCE.md).
