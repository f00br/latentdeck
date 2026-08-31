# LatentPlayer

LatentPlayer 0.1 has two local, single-window workspaces:

- **PLAY** validates and presents an LC cartridge through an installed codec;
- **PREPARE** validates raw H3 Safetensors and packages them as LC cartridges.

## PREPARE workflow

1. Add one or more `.safetensors` files, or add a folder. Folder recursion is
   off until the user explicitly enables **Include nested folders**.
2. Choose an existing local output folder.
3. Select **Validate batch** to inspect every raw payload, derive decoded
   geometry/frame count/dtype/audio presence, and reject unsafe input,
   duplicate output names, or any existing `.lc` target before writing.
4. Select **Convert to .lc**. LatentPlayer converts at most 4096 inputs in a
   deterministic sequence, one file at a time, with same-directory atomic
   commit and post-write LC validation. Each conversion is bound to the raw
   payload SHA-256 approved by **Validate batch**; a source changed afterward
   fails that queue item without creating its `.lc` destination.
5. A completed queue item can be opened directly in **PLAY**. Conversion itself
   does not need a codec pack, decoder model, CUDA device, or GPU.

**Stop after current** is cooperative, not an immediate mid-file cancel. The
active file finishes its atomic write and validation; remaining ready items are
marked cancelled and never started. Application exit uses the same boundary.

## Safety and privacy

- Existing outputs are never overwritten by PREPARE.
- Source Safetensors remain unchanged; no crop, resize, cast, or re-encode is
  performed.
- Frontend snapshots and public errors omit absolute source/output paths.
- The generated manifest records the producer and source payload hash, not the
  source path, prompt, workflow, or model weights.
- LC and raw latent payloads are user data and must remain outside the public
  source tree.

The Python converter and Rust CLI remain available as developer/automation
surfaces. Their validation and authoring behavior shares the same
`latentdeck-cartridge` Rust implementation used by PREPARE.
