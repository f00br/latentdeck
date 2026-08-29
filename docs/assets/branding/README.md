# Original application marks

`apps/latentdeck/src-tauri/icons/source.svg` and
`apps/latentplayer/src-tauri/icons/source.svg` are original geometric marks
created for this repository. The checked-in raster and ICO variants are
deterministically generated from those SVG sources with the pinned Tauri CLI:

```powershell
pnpm exec tauri icon apps/latentdeck/src-tauri/icons/source.svg --output apps/latentdeck/src-tauri/icons
pnpm exec tauri icon apps/latentplayer/src-tauri/icons/source.svg --output apps/latentplayer/src-tauri/icons
```

The generator also emits mobile, macOS, and raster variants. Those outputs are
ignored for the Windows-only 0.1 source tree; only each original SVG and the
required Windows ICO resource are tracked. These marks do not derive from the
ignored local interface concepts.
