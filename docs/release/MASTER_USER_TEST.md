# LatentDeck 0.1 master-user test

This guide is for the owner performing visual and user-path acceptance. It does
not require knowledge of Rust, Python internals, or latent mathematics. Report
what the product shows and does; the engineer can diagnose the implementation.

The owner-accepted implementation baseline is clean `main` commit
`3648e7c634c4310767165ce8975129323a5c09f2`: application `0.1.0`, H3 Codec
Pack/adapter `0.2.0`, and bundled D2/Q4 Decks `0.2.1`. It is not a published or
signed release. The H3 Codec Pack, TAEH3 decoder weight, test cartridges, and
Spout receiver are separate local test inputs and are not bundled in either
application. The owner completed the full functional pass on 2026-09-03; keep
this guide as the reproducible regression and clean-machine acceptance
sequence.

## Prepare the test set

Use visually distinct inputs so a wrong source is immediately obvious:

- at least two portrait cartridges;
- at least two landscape cartridges;
- one existing raw H3 AV `.safetensors` file for conversion;
- the matching H3 Codec Pack setup and adjacent `.ldcodec`, plus an explicitly
  selected compatible TAEH3 decoder asset;
- the official Spout2 D3D12 receiver for output verification.

It is acceptable to reuse cartridges in more than one Q4 slot for this
functional UI pass. Duplicates do not prove four-source diversity, but that
separate acceptance has already passed.

## Install and start the local RC

1. Start from a clean current-user state. Use the engineer-provided artifact
   sets identified by their generated receipts in the current handoff. Verify
   their checksums, then install LatentDeck App and
   LatentPlayer independently. The full procedure and artifact contract are in
   the [Windows RC runbook](WINDOWS_LOCAL_RC.md).
2. Keep `LatentDeck-H3-CodecPack-0.2.0-setup.exe` and the exact matching
   `LatentDeck-H3-CodecPack-0.2.0-windows-x64.ldcodec` in one folder, then run the
   setup. It must install for the current user without UAC, a network request,
   PowerShell, system Python, or either application already being installed.
   Follow the [Codec Pack runbook](H3_CODEC_PACK.md) for the complete contract.
3. Confirm Windows Installed Apps contains `LatentDeck H3 Codec Pack 0.2.0`.
   Installing or removing either application must not install, replace, or
   remove this entry or its pack.
4. Start both applications, open Extensions, refresh discovery, and confirm H3
   `0.2.0` plus bundled D2/Q4 `0.2.1` are visible. Enable each exact required
   version; do not rely on an automatic newest selection. Choose H3 for Player
   and select the compatible external TAEH3 decoder. Confirm the UI shows ready
   status plus the selected variant, SHA-256 identity, source, and license.
   Repeat the ready check in Player, D2, and Q4.
5. Keep the official D3D12 receiver available for the Spout section. The exact
   tested upstream boundary is recorded in
   [Spout acceptance](../repository/SPOUT_ACCEPTANCE.md).

If a pack or decoder is missing or incompatible, the application must explain
that state and remain usable for Library/diagnostic actions. Do not work around
it by copying a weight into an undocumented application directory.

## H3 Codec Pack setup lifecycle

Perform this short lifecycle once on a disposable current-user test account;
reinstall `0.2.0` afterward for the functional sections below:

1. Temporarily separate the setup from its required `.ldcodec` and run it. It must
   report the exact missing adjacent filename, create no visible pack version,
   and create no Windows Installed Apps entry.
2. Put the exact matching `.ldcodec` back beside setup and install. Confirm the
   fixed path `%LOCALAPPDATA%\LatentDeck\CodecPacks\org.latentdeck.h3\0.2.0`
   exists and Extensions discovers it after refresh/restart.
3. Run the same setup again. It may restore its external maintenance metadata,
   but must not overwrite or silently repair the immutable installed pack.
4. Remove `LatentDeck H3 Codec Pack 0.2.0` through Windows Installed Apps.
   Confirm only that exact version disappears; LatentDeck, LatentPlayer,
   Library/cartridge data, and decoder selection remain.
5. Reinstall from the same setup/payload pair and continue the product test.

This local lifecycle proof covers the user flow but not publisher trust. Repeat
the signed setup lifecycle offline on a clean Windows 11 NVIDIA machine without
PowerShell 7, system Python, or ComfyUI before publication.

## Convert existing raw H3 AV data

Changing the filename extension is not conversion. An `.lc` is a validated
ZIP64 cartridge containing `manifest.json`, the Safetensors payload, and bound
hashes.

For the normal user path, select and enable an exact Codec Pack that advertises
`raw_import`, choose **Use in Player**, and use the PREPARE workspace described
below. The adapter performs bounded CPU preflight and stages profile-valid
payload bytes; Core creates and reopens the final `.lc` before it becomes a
Library item.

The repository CLI remains an engineering surface. From the repository root,
prepare the locked Python workspace once:

```powershell
uv sync --locked
```

Convert one file:

```powershell
uv run latentdeck-convert "<raw-file>.safetensors" --output "<new-cartridge>.lc"
```

Convert every supported file in one folder, or include subfolders:

```powershell
uv run latentdeck-convert "<raw-folder>" --output-directory "<output-folder>"
uv run latentdeck-convert "<raw-folder>" --recursive --output-directory "<output-folder>"
```

Expected result:

- the source Safetensors file remains unchanged;
- each output ends in `.lc` and the command returns a successful JSON receipt;
- visual and optional audio tensor identity, dtype, shape, timing, and payload
  hash are derived and validated without crop, resize, cast, or re-encode;
- the selected adapter never writes directly into Library;
- importing the new `.lc` into Library and opening it in LatentPlayer works like
  a Recorder-produced cartridge.

If conversion fails, keep the complete error text and do not rename, edit, or
delete the source file.

## Geometry and aspect-ratio rule in 0.1

LatentDeck does not classify media only as “9:16” or “16:9.” The shared core
uses the exact codec/profile, latent grid, decoded dimensions, timing contract,
and runtime dtype.

- **Playback:** any valid supported portrait or landscape cartridge may play by
  itself. Embedded and fullscreen output must preserve its intrinsic aspect
  ratio using aspect fit: no stretch and no hidden crop.
- **Direct D2/Q4 synthesis:** sources must have compatible codec/profile,
  spatial latent grid, timing contract, and runtime dtype. Portrait and
  landscape sources with different grids therefore normally cannot be mixed
  directly. The Deck must explain the mismatch instead of changing the data.
- **Different clip lengths:** standalone Deck slots use independent cyclic
  playheads, so compatible sources may have different valid durations. Toolkit
  full-clip operators require equal temporal tensor length unless the graph
  contains an explicit temporal crop.
- **Intentional conversion:** the Toolkit's visible Align/Crop nodes can derive
  compatible data with recorded policy and provenance. There is no hidden
  resize or re-encode in 0.1.

Test portrait and landscape playback independently. Test D2 and Q4 once with a
compatible portrait set and once with a compatible landscape set. Also attempt
one intentionally incompatible portrait/landscape combination and confirm a
clear refusal.

Audio tensors and metadata may be preserved in a cartridge, but 0.1 has no
audio playback or synthesis controls.

## LatentPlayer

Begin with **Extensions → H3 0.2.0 → Player device: CUDA → Use in Player**.
Return to PLAY and select the accepted decoder if the profile has no binding.
The exact enabled Codec identity must remain visible; a different installed
version must never replace it automatically.

Repeat the sequence with one portrait and one landscape cartridge:

1. Open the cartridge and confirm the visible name/identity belongs to that
   exact source.
2. Press Play, Pause, Restart, and toggle Loop.
3. Confirm video appears inside the main Player window, not in a separate
   output window.
4. Resize the app window. The image must remain proportionate, centered, and
   fully visible without stretching.
5. Enter fullscreen and return. The same active stream must remain visible.
6. Restart or loop and confirm playback begins cleanly rather than showing a
   stale causal frame from the previous cycle.
7. Do not simulate a worker crash. If a worker error occurs naturally, record
   the exact status and confirm the app remains responsive; then re-open the
   cartridge or relaunch the app as the visible status directs. The Player's
   `Restart` control resets playback and causal state, not the worker process.

Then test the PREPARE workspace. The selected enabled codec must advertise
`raw_import`; preflight itself does not require the decoder or GPU:

8. Add two explicit raw H3 `.safetensors` files and one folder. Confirm nested
   folders are excluded by default and included only after enabling the visible
   recursion option.
9. Choose a new output folder and press `Validate batch`. Inspect each planned
   item: source name, relative `.lc` destination, payload size/hash, storage
   dtype, latent `T/H/W`, decoded dimensions/frame count, and audio presence
   must be visible without writing a cartridge.
10. Deliberately create one existing destination and one duplicate
    case-insensitive destination. Preflight must reject the batch before any
    new output is written and must not overwrite either file.
11. Validate one source, replace it with a different valid H3 Safetensors file,
    then start conversion. That item must fail with `raw_import.source_changed`,
    preserve the preflight identity shown in the queue, and create no `.lc`.
12. Convert a clean multi-file plan. Progress must be sequential and every
    successful item must be atomically complete, fully validated, and free of
    temporary residue. One invalid item must show its own sanitized error
    without hiding the other item results.
13. During another multi-file plan press `Stop after current`. The active item
    must finish and validate; queued items must become cancelled and must not
    start writing.
14. Press `Open in Player` on a completed item. The workspace must return to
    PLAY and load that exact new cartridge. The developer console converter
    remains available but is not required for this workflow.

## Library and Collections

1. Import one `.lc` file, then import a folder explicitly. The app must not scan
   unrelated disk locations.
2. Confirm `All Cartridges` contains the complete imported index.
3. Search by a filename fragment, then edit a cartridge's comma-separated tags
   and confirm both the tag display and search result update correctly.
4. Toggle Favorite and confirm the star/state persists after changing banks and
   reopening the app.
5. Use `Add to Recent` on two cartridges and confirm the Recent strip shows the
   expected identities and ordering.
6. Create two collections and place one cartridge in both.
7. Reorder cartridges manually and confirm the order persists after changing
   collections and reopening the app.
8. Confirm `Unassigned` contains only cartridges with no collection membership.
9. Rename and delete a collection. The cartridge file and Library entry must
   remain, and a cartridge already playing in a Deck slot must not unload.
10. Open D2 or Q4 from a chosen collection. The slot picker should start in that
    bank while still allowing `All Cartridges` when deliberately selected.
11. With `All Cartridges` or `Unassigned` already active, import another
    compatible cartridge in Library, then return to the active D2 or Q4 page.
    It must appear without toggling Active Collection or reselecting the same
    virtual collection. Currently playing identities and an intentionally
    edited next-load draft must remain unchanged. Repeat with a real Collection
    active: its membership filter must remain selected and must not silently
    admit the new unassigned cartridge.

## Extensions and modular Deck lifecycle

1. Open Extensions and confirm the exact enabled H3 `0.2.0` plus bundled D2
   and Q4 `0.2.1` identities. The matrix must show the compatible pairs and no
   implicit newest-version selection.
2. Inspect a local test `.ld`, independently compare its SHA-256, enter that
   exact value, and install it. Installation must finish disabled; enable is a
   separate explicit action.
3. Enable a compatible external Deck. It must become selectable and use the
   same declarative renderer and generic runtime without rebuilding or
   restarting the application.
4. Install an intentionally incompatible external Deck. It must remain visible
   with the stable matrix reason, must not offer a loadable H3 combination, and
   must not fall back to another profile, protocol, or Deck.
5. Verify, disable, re-enable, close any warm session using the package, then
   disable and remove its exact version. Other packages and sessions must remain
   healthy.

## LD-D2

1. Select two visually distinct compatible cartridges and confirm both source
   names and identities are shown.
2. Play both slots and verify independent Pause, Loop, and Restart behavior.
   With Loop off, natural EOF must pause only the exhausted source; it must not
   destroy the warm session or replace the output with `No active output`.
3. Compare Linear with XS5. The visual result should be observably different.
4. Exercise `HYBRIDIZE`, `INTERACT`, swap the `carrier`/`donor` role mapping,
   select TOPK and Sinkhorn where applicable, and change a few bounded controls.
   Each valid synthesis control must affect the running output in real time
   without an Apply button or a transient layout-changing wait message. Only
   controls relevant to the selected algorithm/mode should be visible. The UI
   must stay responsive and the physical sources/playheads must not swap when
   their logical roles change.
5. Create a Snapshot resample. It must validate, appear in Library immediately,
   and play in LatentPlayer.
6. Create a Live Capture while changing controls and let a short looping source
   cross at least three automatic loop boundaries. It must remain active until
   manual Stop, then validate and appear in Library without `.partial` residue.
   The result must be longer than one carrier cycle; for a 107-slot carrier its
   visual `T` must be greater than 107.
7. Select the finished capture as A's next-load draft. The A button must become
   `Load + Play A`; press it and confirm the complete compatible A+B draft loads
   and A starts without reloading the application. Repeat in B with either the
   contextual action or `Use capture in B`. Each explicit action may perform one
   bounded worker restart; the other source draft, controls, seed, loop/play
   intent, and compatible geometry must remain.
8. Repeat the main playback and resample path with the other aspect-ratio set.

## LD-Q4

1. Select one Carrier and three Donors. All A/B/C/D roles must show the actual
   cartridge information; a blank or stale role card is a defect.
2. Start with four compatible sources. Reusing a source in two slots is allowed
   for this UI pass and must be disclosed rather than hidden.
3. Change the Carrier and confirm the visible role and output change together.
4. Move each donor influence independently, then use the triangular influence
   macro. The three donor weights must remain understandable and predictable.
5. Compare TOPK and Sinkhorn and repeat the same seed/settings after Restart.
   Valid controls must update the running output immediately, and the panel must
   render only controls relevant to the chosen algorithm/mode. The replay
   should be deterministic.
6. Create both Snapshot and Live Capture results. Select a finished capture as
   a next-load draft and confirm that slot becomes `Load + Play`; press it and
   verify that the complete compatible Q4 draft loads and the requested slot
   starts without reloading the application. Repeat the insertion path in each
   required A/B/C/D slot, using either the contextual action or `Use capture in
…`; other sources, controls, roles, seed, and transport intent must remain.
7. Repeat once with a compatible portrait set and once with a compatible
   landscape set.

## Decoded MP4 recording in D2 and Q4

Repeat in each Deck with a loaded compatible source set:

1. Choose `Record MP4`, select a new destination, change a few normal realtime
   controls, let several seconds play, and choose `Stop MP4`.
2. Open the result in a normal video player. It must be video-only H.264 at
   24 fps, upright with the source top still at the top, and at the exact
   intrinsic decoded dimensions; no letterbox/pillarbox, hidden resize/crop,
   audio stream, or sibling `.partial.mp4` may be present.
3. Start another MP4 and use a newly captured cartridge of the same compatible
   geometry in one slot. The bounded source replacement must not silently end
   the decoded recording; Stop must still finalize one playable MP4.
4. While MP4 recording is active, latent Snapshot/Live Capture controls must be
   unavailable. While latent capture is active, MP4 start must be unavailable.
   Neither conflict may stop Deck playback.
5. Cancel the destination dialog and stop once before the first decoded frame.
   Cancellation must remain non-destructive and must not claim a saved file.

## D2 and Q4 presets

Repeat once in each Deck while no capture is active:

1. Load known sources, choose a collection, set controls/seed/loops, and save a
   preset through the native dialog.
2. Change the next-load source draft and controls, then load the saved preset.
3. Confirm the preset restores collection, exact cartridge identities, logical
   role bindings, controls, seed, and loop choices as a **draft**. The currently
   playing Deck must not change yet.
4. Press `Load exact Deck draft`. The contextual `Load + Play` shown on a
   changed slot is an equivalent explicit source-apply action that also starts
   that slot. Only an explicit source-load action should replace the playing
   sources and activate the loaded preset; synthesis controls remain realtime.
5. If a saved cartridge is unavailable, the UI must warn about the missing
   identity and must never substitute another cartridge silently.

## Warm sessions and foreground output lease

1. Begin with `Warm sessions 0/4`. Load four distinct exact Deck drafts and
   confirm all four remain listed and reusable.
2. Attempt a fifth load. It must fail with `session.capacity_exceeded`; no
   existing session may be evicted or silently reused.
3. Close one listed session and confirm capacity returns. Close must release
   the worker and must not leave a recurring `session.not_found` status.
4. Start Live Capture in the foreground session and try to switch output to
   another warm session. The switch must fail with
   `session.output_lease_pinned`. Stop capture and confirm switching works.
5. Repeat the same pin test during MP4 recording, independently of the latent
   capture path.
6. Stop every output job and close every session. Finish at
   `Warm sessions 0/4` with no worker process left behind.

## Spout2 in all applications

Spout sharing is built into the release applications; the separate official
receiver is needed only to observe it.

For LatentPlayer, LD-D2, and LD-Q4:

1. Start the official D3D12 receiver.
2. Enable the sender, give it a recognizable name, and confirm the receiver
   sees that exact sender.
3. Confirm the receiver dimensions match the active portrait or landscape
   stream and the image is not squeezed.
4. Confirm the sequence advances while playback runs.
5. Disable the sender or close the application. The receiver must stay
   responsive and return to its no-sender state.

## Support diagnostics

1. In LatentPlayer choose `Save diagnostics`, select a new `.zip` destination,
   and confirm a success status without requiring a loaded cartridge.
2. In LatentDeck choose `Save diagnostics` from the Library surface and confirm
   the same bounded save flow succeeds.
3. Repeat once and cancel the native dialog. Cancellation must be reported as a
   normal non-error result and both applications must remain responsive.
4. Keep the bundle local when it contains session evidence. Do not commit it;
   send it to the engineer only when it is needed for a specific finding.

## ComfyUI visual inventory

The isolated profile keeps this test separate from the H3 generation lab. Build
or refresh it from the repository root:

```powershell
pwsh -NoProfile -File tools/Initialize-IsolatedComfyEnvironment.ps1
pwsh -NoProfile -File tools/Test-IsolatedComfyEnvironment.ps1 -ServerSmoke
pwsh -NoProfile -File tools/Start-IsolatedComfyEnvironment.ps1 -Cpu -OpenBrowser
```

The accepted baseline contains eight executable public workflows and the
separate data-free `00_ALL_NODES_GALLERY.json`. Automated strict equality and
the isolated CPU visual pass are complete; retain these steps as regression
coverage:

1. Open `00_ALL_NODES_GALLERY.json` and use Fit View.
2. Confirm one readable canvas shows all 33 Toolkit nodes, the one official
   Recorder node, and the two reviewed external example nodes: 36 in total.
3. Confirm there are no red or missing node cards and no absolute paths,
   selected private files, or model weights. Do not press Queue; the gallery is
   a discovery and rendering test.
4. Open workflows `01`, `02`, `03`, `04`, `05`, `06`, `07`, and `99` one at a
   time and confirm each graph loads completely.
5. In workflow `06_RAW_RECORD_INSPECT`, select a test raw H3 AV source only when
   deliberately testing the official Recorder path. A visual-only gallery test
   does not need to execute any workflow.

## How to report a finding

Send one finding per item with:

```text
Surface: Player / Library / Extensions / D2 / Q4 / Sessions / Preset / Spout / Converter / Comfy / Diagnostics
Build: installer receipt commit or current source commit
Input geometry: decoded width x height and portrait/landscape
Exact steps:
Expected:
Actual:
Screenshot or exact error text:
Does Restart fix it: yes/no
Severity: blocks test / functional minor / cosmetic
```

Do not place private cartridge payloads, model weights, credentials, or private
absolute paths into a public issue or committed document.

## Owner acceptance and publication follow-up

The owner completed the Protocol 2 and final local functional pass on
2026-09-03 and confirmed there are no remaining product defects. The all-nodes
gallery is also accepted. That acceptance does not claim that the fresh
first-install lifecycle, authenticated signing, clean-machine signed lifecycle,
or publication review has passed. After this documentation is committed, build
fresh artifacts from the exact final commit before those remaining gates.
