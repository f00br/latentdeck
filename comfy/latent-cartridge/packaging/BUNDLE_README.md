# LatentDeck Comfy LC Recorder 0.1.0 preview

This unsigned Windows x64 bundle installs the **Save Latent Cartridge (.lc)**
node and its exact, prebuilt dependencies without Rust, Cargo, pip, or a network
connection. It supports the standard ComfyUI Windows Portable CPython 3.13
build and the alternate CPython 3.12 build.

Extract the ZIP, close ComfyUI, and run:

```powershell
$comfyRoot = (Resolve-Path -LiteralPath (Read-Host 'Path to your ComfyUI root')).Path
powershell -ExecutionPolicy Bypass -File .\Install-ComfyRecorder.ps1 -ComfyUIRoot $comfyRoot
```

The installer detects the portable Python and `custom_nodes` paths. For a
different layout, pass both `-PythonPath` and `-CustomNodesPath` explicitly.
It verifies all wheel hashes, checks CPython/ABI/architecture before writing,
extracts dependencies into the node's private `vendor` directory, imports the
native modules with the target interpreter, and refuses to overwrite an
existing installation.

The Recorder preserves an existing ComfyUI Safetensors package. Its bundled
0.8.0 fallback is relocated to
`latentdeck_recorder_vendor.safetensors`, not exposed as a top-level
`safetensors` package, and is used only when the host package is unavailable.

Restart ComfyUI. The node appears under `LatentDeck / Cartridge` as
**Save Latent Cartridge (.lc)**.

Python other than CPython 3.12 or 3.13 x64 is intentionally unsupported. The
installer fails before writing anything and never falls back to a source build.

To uninstall, close ComfyUI and remove only the installed
`custom_nodes/ComfyUI-LatentCartridge` directory. The Recorder and its private
dependencies are contained there; uninstalling it does not require changing
the rest of the ComfyUI Python environment.
