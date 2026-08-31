# Isolated ComfyUI user-test environment

For the owner-facing sequence and all-nodes visual acceptance, start with the
[master-user test guide](MASTER_USER_TEST.md).

This profile runs LatentDeck Comfy Toolkit and `ComfyUI-LatentCartridge`
without installing anything into the source ComfyUI `custom_nodes` or embedded
Python `site-packages` directories. It reuses the reviewed ComfyUI core,
embedded CPython 3.13 runtime, Torch/CUDA dependencies, and explicitly selected
external H3 VAE files read-only.

The generated profile is private and non-distributable. It lives below the
ignored `artifacts/comfy-test` directory and contains no model weight,
cartridge, raw latent, or user media copied from the source laboratory.

## Prepare or update

Stop an older instance of this isolated profile, then run from the repository
root:

```powershell
pwsh -NoProfile -File tools/Initialize-IsolatedComfyEnvironment.ps1
```

The default discovery expects a sibling `h3-pipeline` portable ComfyUI tree.
On another development machine, pass explicit variables instead of editing the
public script:

```powershell
pwsh -NoProfile -File tools/Initialize-IsolatedComfyEnvironment.ps1 `
  -ComfyRoot $comfyRoot `
  -PythonExecutable $embeddedPython `
  -ModelsRoot $modelsRoot `
  -HqVaePath $nativeH3Vae `
  -Port 8192
```

Preparation builds exactly seven repository wheels: the native Cartridge SDK,
Codec Host contract, D2 and Q4 operators, Toolkit, Recorder, and the reviewed
Channel Roll operator/SDK example. They are
installed with `uv pip --target --no-deps` into a fresh overlay. A successful
update replaces only the generated wheel/overlay directories; the isolated
input, output, user, and temp directories remain in place.

The command fails closed if the port is occupied, the embedded dependency
versions differ from the 0.1 contract, the accepted TAEH3 hash does not match,
the native H3 VAE is absent or ambiguous, or the selected Comfy core has an
active source `extra_model_paths.yaml` that could weaken custom-node isolation.

## Check without starting a server

```powershell
pwsh -NoProfile -File tools/Test-IsolatedComfyEnvironment.ps1
```

The smoke check does all of the following without opening a listening socket
or decoding a frame:

- exercises the generated Comfy `--help` parser;
- imports all seven wheels from the isolated overlay;
- verifies embedded Python 3.13, Torch `2.13.0+cu130`, CUDA build 13.0, and
  Safetensors 0.8.0;
- asks ComfyUI to discover only the Toolkit, Recorder, and reviewed external
  example shims under the isolated `custom_nodes` directory, including the
  example's normal node and hook-provider mappings;
- verifies that TAEH3 and the native H3 VAE are visible through external model
  paths;
- hashes TAEH3 and opens only the HQ VAE Safetensors header.

Use `-ContractOnly` when only the public PowerShell syntax/path-safety contract
should be checked and no prepared artifact exists yet.

Before handing the profile to a user, prove a real bounded server start and
the public node-discovery API:

```powershell
pwsh -NoProfile -File tools/Test-IsolatedComfyEnvironment.ps1 -ServerSmoke
```

This starts the isolated profile on its configured loopback port with CPU mode
and `CUDA_VISIBLE_DEVICES=-1`, reads `/system_stats` and `/object_info`, checks
every Toolkit node plus the Recorder and example-operator nodes, then stops
only the process it created and verifies that the port was released. Logs and
a JSON receipt stay inside the ignored environment. This is a server/discovery
test, not evidence that a FAST/HQ decode or a synthesis workflow produced
correct images.

## Start for visual testing

```powershell
pwsh -NoProfile -File tools/Start-IsolatedComfyEnvironment.ps1 -OpenBrowser
```

The default address is `http://127.0.0.1:8192`. This is separate from the H3
laboratory port. The launcher runs the smoke check first, rejects arguments
that could redirect the base/model/user/custom-node roots, and enables only
the Toolkit, Recorder, and reviewed example-operator shims. Press `Ctrl+C` in
that terminal to stop the test server.

For a discovery-only UI launch that must not use the GPU, add `-Cpu`. FAST/HQ
decode and realistic operator timing should be tested with the normal GPU
launch instead.

After source changes, stop this test server and rerun the preparation command.
The normal start command does not silently rebuild an existing environment;
use `-Refresh` when a rebuild immediately before launch is intentional.

## Isolated data locations

All mutable ComfyUI data remains below `artifacts/comfy-test/comfy-base`:

```text
custom_nodes/  generated Toolkit, Recorder, and example discovery shims only
input/         files explicitly supplied for this test profile
output/        previews, reports, and recorded cartridges
user/          workflows, settings, and the profile database
temp/          temporary ComfyUI data
models/        empty local model root
```

ComfyUI may remove its temporary directory during a normal shutdown. The
isolated checker and launcher recreate only that generated mutable directory;
missing install, input, output, user, or custom-node directories remain hard
errors rather than being silently repaired.

The `extra_model_paths.yaml` generated beside that directory exposes only the
external `vae` and `vae_approx` folders. It does not expose source custom nodes
or copy model files. The generated `environment.json` records wheel hashes,
source commit/dirty state, paths, version checks, and the external-model policy
for diagnosis. Absolute machine paths are confined to these ignored generated
files, never the public scripts or documentation.
