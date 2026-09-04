# Original application marks

`apps/latentdeck/src-tauri/icons/source.svg` and
`apps/latentplayer/src-tauri/icons/source.svg` are original geometric marks
created by project owner `@f00br` for this repository. The checked-in ICO
variants are deterministically generated from those SVG sources with the pinned
Tauri CLI:

```powershell
pnpm exec tauri icon apps/latentdeck/src-tauri/icons/source.svg --output apps/latentdeck/src-tauri/icons
pnpm exec tauri icon apps/latentplayer/src-tauri/icons/source.svg --output apps/latentplayer/src-tauri/icons
```

The generator also emits mobile, macOS, and raster variants. Those outputs are
ignored for the Windows-only 0.1 source tree; only each original SVG and the
required Windows ICO resource are tracked.

## File provenance and disposition

| File | SHA-256 | Origin and intended use |
| --- | --- | --- |
| [`apps/latentdeck/src-tauri/icons/source.svg`](../../../apps/latentdeck/src-tauri/icons/source.svg) | `6a215c05222f77866729ea686974d6e0425754576cba55419136747c0ffd6e3e` | Original `@f00br` LatentDeck application mark and canonical icon source. |
| [`apps/latentdeck/src-tauri/icons/icon.ico`](../../../apps/latentdeck/src-tauri/icons/icon.ico) | `4ce64accba31b689246794deb095c4df8b7b79bfb868f4083d96ae4debfc227b` | Windows application resource generated from the LatentDeck SVG. |
| [`apps/latentplayer/src-tauri/icons/source.svg`](../../../apps/latentplayer/src-tauri/icons/source.svg) | `6c251651f9f17aad76d197693216fed667729345b54e5e61e9ae2dcfb390252b` | Original `@f00br` LatentPlayer application mark and canonical icon source. |
| [`apps/latentplayer/src-tauri/icons/icon.ico`](../../../apps/latentplayer/src-tauri/icons/icon.ico) | `38e84ce999e27b205cb24e01a84e9e087e80637c6f2dffcc7d4f0307dd9b0287` | Windows application resource generated from the LatentPlayer SVG. |

Author and rights holder for both source marks: `@f00br`. The source marks and
generated ICO resources are original project assets redistributed under the
repository's Apache-2.0 license. Regeneration or replacement must update this
record and its hashes.
