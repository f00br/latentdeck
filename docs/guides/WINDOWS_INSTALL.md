# Install the Windows preview

This guide is for an artist installing LatentDeck without a source checkout.
The 0.1 preview uses independent current-user installers for LatentDeck,
LatentPlayer, and the H3 Codec Pack, plus an optional self-contained Comfy LC
Recorder bundle.

## Requirements

- 64-bit Windows 11 for the release-validation path;
- an NVIDIA GPU and compatible CUDA driver for the current H3 runtime;
- a compatible external TAEH3 decoder asset selected by the user;
- at least one valid H3 `.lc` cartridge or raw H3 Safetensors file.

For a first run, use the separately hosted [LatentDeck Demo LC
Pack](DEMO_CARTRIDGES.md). Its three pinned H3 cartridges share compatible
1344 × 768 synthesis geometry. The pack is licensed under CC BY 4.0; follow
its attribution terms when sharing or adapting the cartridge media.

The installers do not require administrator elevation. The H3 setup runs
offline and does not require system Python, PowerShell, ComfyUI, either
application, or a network connection.

## Download one complete release set

Download `v0.1.0-preview.1` only from [GitHub
Releases](https://github.com/f00br/latentdeck/releases). It has exactly five
downloadable project assets:

| Purpose | File |
| --- | --- |
| Player, LatentDeck, and H3 Codec Pack | `LatentDeck-0.1.0-preview.1-Artist-Starter-Windows-x64-unsigned.zip` |
| Optional ComfyUI LC Recorder | `LatentDeck-0.1.0-preview.1-comfy-recorder-windows-x64.zip` |
| Optional extension-authoring tools | `LatentDeck-0.1.0-preview.1-developer-kit-windows-x64.zip` |
| Receipts, SBOMs, licenses, notices, and manifests | `LatentDeck-0.1.0-preview.1-Release-Evidence.zip` |
| Outer checksums | `LatentDeck-0.1.0-preview.1-SHA256SUMS.txt` |

GitHub also displays automatically generated source archives; artists do not
need those files. Do not combine assets from different releases or mirror
downloads.

Verify every downloaded ZIP against the release checksum file before opening
it:

```powershell
Get-FileHash -Algorithm SHA256 .\LatentDeck-0.1.0-preview.1-Artist-Starter-Windows-x64-unsigned.zip
```

Repeat for every downloaded ZIP. Compare the complete
lowercase/uppercase-insensitive hexadecimal digest, not a short prefix.
Extract the entire Artist Starter to a new directory and check its internal
`SHA256SUMS.txt` before running anything. Do not launch setup from inside a ZIP
viewer. Full extraction preserves the required adjacency between the H3 setup
and its exact `.ldcodec`; the setup is bound to that payload's filename, byte
length, SHA-256, package ID, and version.

## Unsigned preview warning

`v0.1.0-preview.1` is deliberately published as an unsigned prerelease. Windows
may show an unknown-publisher or SmartScreen warning. Verify that the download
came from the official GitHub release and that its SHA-256 matches before using
Windows' option to inspect and run it. If the source or hash is uncertain,
cancel the installation.

The warning is a property of this preview distribution; it is not proof that a
file is safe. Extension packages remain separate executable-code trust
decisions even after the applications are installed.

## Install the applications

Run the LatentDeck and LatentPlayer installers from the extracted Artist
Starter's `Installers` directory. Each installs for the current Windows user.
Either application can be installed, updated, or removed without installing or
removing the other application, the H3 Codec Pack, cartridges, Library data,
or decoder selection.

Launch each installed application once. It should open without a cartridge or
codec and provide Library/Extensions or diagnostic actions in that state.

## Install H3 Codec Pack 0.2.1

1. Open the extracted Artist Starter's `H3-Codec` directory. Confirm the setup
   and `.ldcodec` filenames both contain `0.2.1` and remain side by side.
2. Run `LatentDeck-H3-CodecPack-0.2.1-setup.exe` from that directory.
3. Confirm **LatentDeck H3 Codec Pack 0.2.1** appears in Windows Installed Apps.
4. Open **Extensions** in LatentPlayer or LatentDeck and refresh discovery.
5. Find the exact H3 `0.2.1` pack and enable it. Installation does not enable a
   package automatically.
6. Select that exact version for Player and for each compatible Deck you intend
   to use. LatentDeck never selects a newer version implicitly.

The pack contains the H3 adapter and its isolated runtime, but no decoder/model
weight, generator, ComfyUI installation, or cartridge.

## Install the Comfy LC Recorder

This optional path is for recording a generated H3 latent before decode. It is
independent from the LatentDeck, LatentPlayer, and H3 Codec Pack installers.

1. Verify
   `LatentDeck-0.1.0-preview.1-comfy-recorder-windows-x64.zip` against the
   release checksum, then extract it to a new directory.
2. Close ComfyUI.
3. Run the extracted installer against the root of the Windows Portable
   distribution:

   ```powershell
   $comfyRoot = (Resolve-Path -LiteralPath (Read-Host 'Path to your ComfyUI root')).Path
   powershell -ExecutionPolicy Bypass -File .\Install-ComfyRecorder.ps1 -ComfyUIRoot $comfyRoot
   ```

4. Restart ComfyUI and find **Save Latent Cartridge (.lc)** under
   `LatentDeck / Cartridge`.

The bundle supports Windows x64 CPython 3.12 and 3.13 and carries exact
prebuilt wheels for the Recorder, `latentdeck-cartridge==0.1.0`, and
`safetensors==0.8.0`. Installation is offline, does not use pip or a Rust
compiler, keeps these packages inside the node directory, and refuses an
unsupported Python ABI before writing the installation. Pass explicit
`-PythonPath` and `-CustomNodesPath` values when the ComfyUI layout is not one
of the detected Windows Portable layouts.

If ComfyUI already supplies Safetensors, the Recorder preserves and uses that
host package. Otherwise it uses the exact bundled 0.8.0 copy from the private
`latentdeck_recorder_vendor.safetensors` namespace; the bundle does not expose
that fallback as a global `safetensors` package.

Comfy Registry/Manager installation is not available for this preview because
the native `latentdeck-cartridge` wheel is not published to PyPI. The bundled
installer is the supported public route; Registry publication/configuration is
a separate future gate.

## Select the external decoder

Choose the decoder asset through the application's H3 extension controls. The
pack declares the accepted byte length, SHA-256, source, and license; the
application accepts only a matching local file. Review the upstream terms and
obtain the asset from the source identified by the pack.

For H3 `0.2.1`, follow the **Source** link shown by the application or download
the declared
[TAEH3 file](https://raw.githubusercontent.com/madebyollin/taehv/62f7591f59dfbb4c3c02b7a621d180a9eeaba26c/safetensors/taeh3.safetensors).
The accepted variant is 22,709,752 bytes with SHA-256
`4fd022bfcab08772fe0536b17ea1a3bbb5625be11e397868d1c5d891863d4c13`.
Use **Select decoder** in LatentPlayer. In LatentDeck, open the Deck's **Codec
assets** section and use **Choose file…**. Select the asset explicitly in each
application you use.

Do not rename an arbitrary model or copy a weight into an undocumented
application directory. A missing or mismatched decoder should produce a visible
not-ready state rather than a fallback.

When ready, the UI shows the exact pack, adapter, profile, device, and decoder
identity. Select CUDA explicitly. The preview does not silently switch to CPU,
another decoder, another Codec Pack, or Protocol 1 after a Protocol 2 failure.

## Verify the first launch

1. Download and checksum one compatible `.lc`, such as a cartridge from the
   [pinned demo pack](DEMO_CARTRIDGES.md).
2. In LatentPlayer, open that `.lc` and test Play, Pause, Restart,
   Loop, window resize, and fullscreen.
3. In LatentDeck, confirm the bundled D2 and Q4 Decks are visible as exact
   enabled packages and that the compatibility matrix shows an H3 pairing.
4. Load a compatible source set. The decoded image should remain at its
   intrinsic aspect ratio; incompatible sources should remain visible but be
   refused with a reason.
5. Save a short Snapshot or Live Capture and open the new `.lc` in Player.
6. Optionally record a short MP4 or verify a Spout sender.

The complete creative sequence is in the [artist workflow](ARTIST_WORKFLOW.md).

## Update, repair, and remove

Installed Deck and Codec versions are immutable and coexist side by side. A new
version installs separately; it does not overwrite the old one. Disable an
exact version before removal. An active Player or Deck session may retain a
usage lease, so close that session if removal reports the package is active.

Use Windows Installed Apps to remove the exact H3 setup version. Removing H3
must preserve the applications, cartridges, Library database, decoder
selection, and other installed extension versions. Use each application's own
uninstaller to remove that application. To remove the Recorder, close ComfyUI
and remove only its `custom_nodes/ComfyUI-LatentCartridge` directory; its
private dependencies are contained there.

For a reproducible failure, save a sanitized bundle using **Save diagnostics**
and read the [support guidance](../../SUPPORT.md) before sharing it.
