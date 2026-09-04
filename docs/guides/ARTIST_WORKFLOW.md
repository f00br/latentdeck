# Artist workflow

This guide follows one piece of material through the complete LatentDeck 0.1
cycle:

```text
raw H3 latent → .lc → play → D2/Q4 synthesis → resample or decode → reuse
```

Start with the [Windows installation guide](WINDOWS_INSTALL.md) if the
applications and H3 Codec Pack are not ready.

## 1. Create a cartridge

Choose one of three explicit authoring paths. Renaming a `.safetensors` file to
`.lc` is never conversion.

### Record during generation in ComfyUI

Install the self-contained Windows Recorder bundle by following the [Recorder
section of the Windows guide](WINDOWS_INSTALL.md#install-the-comfy-lc-recorder).
It supplies the node, the prebuilt Cartridge SDK, and Safetensors for ComfyUI's
Windows x64 CPython 3.12 or 3.13 without a compiler or network install. Connect
**Save Latent Cartridge (.lc)** to the generated H3 latent before decode. The
node reads the visual stream and optional H3 audio stream, derives the
profile/timing facts, writes a canonical cartridge atomically, validates it,
and returns the input latent unchanged to the rest of the graph.

The bundle preserves Safetensors already provided by ComfyUI and uses its
uniquely namespaced bundled 0.8.0 copy only as a fallback.

Recordings appear below ComfyUI's `output/latentdeck/cartridges` directory. The
node does not embed the prompt or workflow; it may record only their hashes as
provenance. See the [recorder guide](../../comfy/latent-cartridge/README.md).

### Prepare an existing raw H3 latent in LatentPlayer

1. In **Extensions**, enable H3 `0.2.1`, select it for Player, and confirm it
   advertises `raw_import`. Raw preflight itself is CPU-only and does not need
   the decoder or GPU.
2. Open **PREPARE** and add one or more `.safetensors` files or a folder.
   Nested folders remain off unless **Include nested folders** is enabled.
3. Choose a new output directory and select **Validate batch**.
4. Review each source's hash, size, storage dtype, latent geometry, decoded
   geometry/frame count, and audio presence before writing.
5. Select **Convert to .lc**. Conversion runs sequentially, never overwrites an
   existing destination, preserves the source bytes, and validates every final
   cartridge.
6. Use **Stop after current** to finish the active atomic item and cancel the
   queued items. Use **Open in Player** on a completed item.

If a source changes after validation, that item fails without producing a final
`.lc`. No source file is edited or deleted.

### Use the developer CLI

For automation from a source checkout or Developer Kit environment:

```powershell
latentdeck-convert input.safetensors --output output.lc
latentdeck-convert input-folder --output-directory output-folder
latentdeck-convert input-folder --recursive --output-directory output-folder
```

The [Cartridge developer guide](../developers/CARTRIDGES.md) covers validation,
inspection, and genealogy.

## 2. Play the cartridge

In LatentPlayer, select the exact enabled H3 Codec Pack, CUDA device, and
accepted external decoder. Open the `.lc`, then use Play, Pause, Restart, Loop,
and Fullscreen.

Playback preserves the cartridge's intrinsic decoded geometry. Resizing the
window uses aspect fit rather than stretching or cropping. With Loop off,
natural end-of-file pauses the source without inventing another frame or
switching codec/runtime.

The cartridge is validated before runtime allocation. A failure should leave
the application responsive and show a stable error rather than partially load
the source.

## 3. Organize material

Import individual `.lc` files or an explicitly chosen folder into the
LatentDeck Library. The application does not scan unrelated locations.

Use Collections, tags, favorites, search, Recent, and manual ordering to
prepare a performance. One cartridge may belong to several Collections.
Deleting a Collection does not delete its cartridges. `All Cartridges` and
`Unassigned` are virtual views, not folders on disk.

A Collection selects the initial bank shown to a Deck; it does not transform a
cartridge or unload a source already retained by a running session.

## 4. Check compatibility before synthesis

Direct D2/Q4 synthesis requires compatible codec family/profile/version,
runtime dtype, batch/channels, latent spatial grid, decoded dimensions, timing
contract, and frame rate. D2/Q4 use independent cyclic playheads, so compatible
sources may have different clip lengths.

Portrait and landscape cartridges can coexist in the Library and play
individually. They normally have different spatial grids and cannot be mixed
directly. The Deck reports the exact mismatch; it never hides a resize, crop,
cast, alignment, re-encode, decoder substitution, or source substitution.

When a common shape is artistically intentional, use the Comfy Toolkit's
visible Align/Crop operation to create a new provenance-bearing cartridge.

## 5. Perform with D2

D2 has two physical source slots and two logical roles: `carrier` and `donor`.
Role changes do not move the physical sources or their independent playheads.

1. Select a compatible A/B pair and load the complete draft.
2. Start each source and choose independent Loop/Pause/Restart behavior.
3. Use `LINEAR` as the interpolation baseline.
4. Compare XS1–XS5 and reveal only the controls relevant to the selected
   algorithm/mode.
5. Change the carrier/donor assignment, seed, interaction, preserve, routing,
   and other visible controls while the stream is running.

The exact algorithms and bounds are documented by the [D2 package](../../operators/builtin/d2/README.md).

## 6. Perform with Q4

Q4 has four physical source slots and four logical roles: one `carrier` and
three donors. Sources remain independently transportable while the role editor
changes how they enter the operator.

1. Select four compatible sources. Reusing one source in several slots is
   allowed but is not evidence of four-source diversity.
2. Compare `LINEAR` and XS5.
3. Adjust donor influence manually or with the triangular macro.
4. Change carrier/donor roles and compare TOPK/Sinkhorn where the current mode
   exposes them.
5. Restart with the same sources, controls, roles, and seed to check
   deterministic replay.

See the [Q4 package guide](../../operators/builtin/q4/README.md) for the exact
operator contract.

## 7. Resample into a new cartridge

Snapshot and Live Capture receive the Deck's post-operator latent state before
decode.

- **Snapshot** saves a bounded current latent result.
- **Live Capture** records a bounded sequence while sources and controls run.
  It may cross valid source-loop boundaries and stops only through its explicit
  capture lifecycle.

The host finalizes and validates the new `.lc`, records parent identities,
operator/version, controls, seed, and audio disposition, then imports it into
Library. Use **Use capture in ...**, **Load + Play**, or a normal explicit Load
to insert it into a slot. Merely selecting it as a draft does not silently
replace a running source.

The result can be opened in LatentPlayer or resynthesized in another generation
of the workflow:

```text
A.lc + B.lc → C.lc
C.lc + D.lc → E.lc
```

Genealogy is descriptive data only. A cartridge never installs the operator
named in its history.

## 8. Record decoded output

Use **Record MP4** in D2 or Q4 to write the decoded program output. The result
is video-only H.264 at the intrinsic decoded dimensions and 24 fps. It contains
no latent or audio stream and is not a cartridge. Normal playback and realtime
controls continue while encoding runs behind a bounded queue.

MP4 recording and latent Snapshot/Live Capture are mutually exclusive in 0.1.
Starting one should make the conflicting action unavailable without stopping
the Deck.

Enable Spout to share the same intrinsic decoded texture with another Windows
visual application. The receiver sees the decoded extent without the local
window's letterbox/pillarbox bars. Give the sender an intentional name, verify
that its frame sequence advances, and disable it cleanly before exit.

## 9. Manage sessions and save state

LatentDeck keeps up to four explicit warm Deck sessions and never evicts one to
make room for a fifth. Close a session to release its capacity. Only one session
owns foreground output at a time; Live Capture and MP4 independently pin that
lease until stopped.

A Deck preset stores collection, exact cartridge identities, role bindings,
controls, seed, and loops as a draft. Loading a preset does not substitute a
missing cartridge and does not change the running sources until an explicit
load action is used.

## Audio boundary

An H3 cartridge may preserve an audio latent and its disposition metadata.
LatentDeck 0.1 does not decode, play, synthesize, mix, or export that audio.
Decoded MP4 is intentionally video-only. An operation that cannot preserve
audio under the profile's exact timing rules records an explicit omission
policy instead of silently claiming preservation.
