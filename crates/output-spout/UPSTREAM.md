# Spout2 upstream pin

LatentDeck uses the official Spout2 DX12 source through a local, reproducible
preparation step. The upstream repository is not vendored in Git.

## Approved source

| Field | Value |
|---|---|
| Project | Spout2 |
| Upstream | <https://github.com/leadedge/Spout2> |
| Tag | `2.007.017` |
| Commit | `f49e2f469f8cb25f559a6eaa61a3f5b8173fc100` |
| Archive | <https://github.com/leadedge/Spout2/archive/f49e2f469f8cb25f559a6eaa61a3f5b8173fc100.zip> |
| Archive byte length | `5,099,633` |
| Archive SHA-256 | `cb60c83d4df3c2927cd3c5a505910bb720a8011d505217a71d293968405e4bf4` |
| License | BSD-2-Clause |

The preparation script verifies both the archive and the critical source files
used by the build:

| Prepared source-relative path | SHA-256 |
|---|---|
| `LICENSE` | `7b602b5c652a76ced1c6ff5f3f4c15c37a733230eeb5b8d075f1282b446b10be` |
| `CMakeLists.txt` | `4b78c6930b52e5a013ef3cc40717a4534349d1693fbc3d4bffbdb17b61201dea` |
| `SPOUTSDK/SpoutGL/CMakeLists.txt` | `69dc548b163c01690b7cd23f9b2ad8fea0603ed5b935e3e3718393889d5a408e` |
| `SPOUTSDK/SpoutDirectX/SpoutDX/CMakeLists.txt` | `95e3f52a1ee518773c6d9735edc4e5bf68b4d88005808798a2bdf2384b830a69` |
| `SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/CMakeLists.txt` | `d3c3d823d0e53421be98ceee262d178c7deb8afe5ba7b6a5f90f5273ce26d552` |
| `SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.cpp` | `4c8ded4a561d74dcc95fdc2ab7f76b5a90c940990ef19280843d0e023ee002e3` |
| `SPOUTSDK/SpoutDirectX/SpoutDX/SpoutDX12/SpoutDX12.h` | `5e48a55a0b70a274b303d20ea4c688ba8e100fce2c8eb9df6ad361d341271cb8` |

## Local preparation

Prepare from the approved URL:

```powershell
pwsh -NoProfile -File tools/Prepare-Spout2.ps1
```

Or prepare from an already downloaded copy of the exact archive:

```powershell
pwsh -NoProfile -File tools/Prepare-Spout2.ps1 -ArchivePath path/to/Spout2.zip
```

The verified source is installed below
`vendor-local/spout2/2.007.017-f49e2f469f8cb25f559a6eaa61a3f5b8173fc100/source`.
The whole `vendor-local` tree is ignored. After this one-time preparation, the
native bridge can be built offline from the pinned local source.

The script is idempotent only for an exact prepared install. It refuses to
replace a destination with a missing or mismatched stamp or with modified
critical files.

## Integration boundary

The native bridge links the official static `SpoutDX12` implementation. It
shares an application-owned `ID3D12Resource` through the official D3D11On12
device and `SendDX11Resource`; it does not encode frames and does not use the
CPU `SendImage` path. The upstream `WrapDX12Resource` helper hardcodes
`OutState=PRESENT`, so the original LatentDeck bridge instead calls
`CreateWrappedResource` on the official `ID3D11On12Device` with identical
input/output states. This preserves wgpu's tracked
`PIXEL_SHADER_RESOURCE` state. LatentDeck's C ABI bridge remains separate
original project code.

The complete upstream license notice required for source and binary
redistribution is in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
