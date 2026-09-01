# LatentDeck 0.1.0 acceptance status

This is the current release-status record. It separates owner-accepted local
behavior from release-presentation and publication gates. It does not authorize
a remote, push, tag, upload, signing action, or public release.

Status on 2026-09-01: the owner explicitly accepted the complete local `0.1.0`
functional surface with no remaining product defects. Player, Library,
Collections, LD-D2, LD-Q4, Resample, Snapshot, long Live Capture, hot insertion,
upright MP4 output, Spout2, 9:16 and 16:9 presentation, LatentPlayer PREPARE,
and H3 Codec Pack setup/runtime behavior work as intended on the owner test
machine.

The repository is now in release-documentation and publication preparation.
The ComfyUI all-nodes gallery is a required task for that next phase, not a
missing functional fix. Clean-machine, signing, security-contact, archive,
legal, and owner-authorized publication gates remain open.

## Owner-accepted functional baseline

The accepted unsigned local artifact set was built from clean `main` commit
`dbe310a2b8c0a9f78a11ab8217f07c8c91a39db4`. Both application installers and
H3 Codec Pack `0.1.1` receipts record `git_dirty=false` and the same public
source snapshot:

- file count: `490`;
- SHA-256:
  `1c7ff1d524b2451d313f24a36bf6b8dc9910ee88aeada8fb943e07ae6db8456a`.

### Application artifacts

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `LatentDeck-0.1.0-windows-x64-unsigned-setup.exe` | 6,564,636 | `9bbdc405b7f49345c64941b00b65133855e0894e01797ebd2366d2c5edced25f` |
| `LatentPlayer-0.1.0-windows-x64-unsigned-setup.exe` | 5,200,446 | `2edfcc304563bfff06b60f956036a7263bbd54d149baebcc7a82ead4de545a86` |
| `latentdeck-0.1.0-sbom.cdx.json` | 385,134 | `0d6122e7af26d01478bc9bd2615ed2b548dc80ff8eb1b612580cc9e4a3093d27` |
| `THIRD_PARTY_NOTICES.md` | 3,621 | `f99db3adcf79512f4ee8f753b168919a42e475fc46aefe70ef3751db48232991` |

The schema-3 application receipt records pinned Node 24.20.0, pnpm 11.24.0,
Tauri CLI 2.11.4, Rust 1.93.1, Spout2 2.007.017 at commit
`f49e2f469f8cb25f559a6eaa61a3f5b8173fc100`, and a CycloneDX 1.5 SBOM with
745 components. Neither application installer contains a Codec Pack, decoder,
model weight, or cartridge.

### H3 Codec Pack artifacts

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| `LatentDeck-H3-CodecPack-0.1.1-setup.exe` | 1,102,404 | `7f8af08cfbb4ad2a66091bcd510d4ba11a360921c2c37fff3199a02618300dbb` |
| `LatentDeck-H3-CodecPack-0.1.1-windows-x64.zip` | 1,942,790,075 | `b2a04a4d42caaa4ac148c3ce30ab9a90ec3e83adcafb446d9b5609dcba491742` |

The ZIP expands to 2,911,144,233 bytes. The immutable pack version is `0.1.1`;
the H3 adapter and Player/D2/Q4 worker contract version remains `0.1.0`. Setup
is current-user, offline, fixed-path, and bound to the exact adjacent ZIP name,
length, SHA-256, pack identity, and version. It requires no elevation,
PowerShell, system Python, network access, decoder, or model.

The accepted local physical lifecycle covered removal of the defective
engineering pack, fixed setup install, same-version setup retry without
overwriting immutable files, exact-version removal behavior, and reinstall.
Installed-runtime smoke passed `codec.inspect` for Player, D2, and Q4 with CUDA
on the local RTX 4070 and Torch `2.13.0+cu130`. The pack contains no model
weight, generator, ComfyUI, cartridge, or private media; the TAEH3 decoder is a
separate explicit user selection.

The `dbe310a` artifact directories remain preserved as functional UAT evidence.
Any later accepted source change, including release documentation or the
all-nodes gallery, makes them an older source snapshot. Do not publish or mix
them with new metadata; rebuild the complete artifact set from the final clean
commit.

## Owner-accepted functional behavior

### Cartridge creation, conversion, and Player

- The official Comfy Recorder produced and validated H3 AV `.lc` cartridges
  while retaining latent passthrough.
- `latentdeck-convert` and LatentPlayer PREPARE preflight raw H3 AV payloads,
  preserve source bytes and geometry, enforce no-clobber destinations, report
  per-item progress/errors, support stop-after-current, and open completed
  cartridges directly in Player.
- Conversion is bound to the preflight payload SHA-256. A changed source fails
  without producing a destination cartridge.
- LatentPlayer playback, embedded output, fullscreen, aspect fit, diagnostics,
  and Spout2 passed with portrait and landscape cartridges.

### Library, Collections, and Deck source lifecycle

- Library imports and completed captures invalidate active Deck source views
  automatically; no Active Collection toggle is required to reveal new
  cartridges.
- Refresh preserves currently playing and edited next-load identities and
  ignores stale asynchronous responses.
- A finished capture can be inserted immediately into D2 A/B or Q4 A/B/C/D
  through the explicit `Use capture in ...` action. The bounded worker
  replacement retains the other draft sources, controls, roles, seed,
  transport intent, and an active decoded-video recorder.
- When a next-load draft differs from the runtime identity, `Load + Play`
  applies the compatible draft and starts that slot. Matching identities keep
  ordinary transport-only `Play`/`Pause` behavior.
- Many-to-many Collections and virtual `All` and `Unassigned` banks behaved as
  documented.

### LD-D2, LD-Q4, Live Capture, and MP4

- D2 and Q4 preserve latent Live Capture across expected automatic source-loop
  reset barriers until explicit Stop or a validated large spool safety limit.
  Arbitrary resets still abort instead of joining unrelated latent state.
- The owner reproduced the previous one-loop D2 defect, installed the corrected
  H3 Codec Pack `0.1.1`, and then confirmed the long-capture behavior works as
  intended.
- D2 and Q4 write no-clobber, video-only H.264 MP4 at intrinsic geometry and
  24 fps. The corrected top-down RGBA path produces upright video.
- D2 Linear/XS behavior, deterministic replay, Snapshot, Live Capture,
  validation, Library import, and replay passed.
- Strict Q4 proof used four distinct compatible cartridges and covered TOPK,
  Sinkhorn, carrier reassignment, donor influence, deterministic replay,
  Snapshot, Live Capture, validation, reload, and partial-file cleanup. The
  path-free receipt SHA-256 is
  `b2b22294a8081ea03f8179b1f904ef0946a2e809654746a9aa0892b07e21964e`.
- Incompatible geometry produces an explicit refusal. Neither Deck performs a
  hidden resize, crop, alignment, re-encode, or source substitution.

### Stability, native output, and Spout2

- The owner-approved stability suite ran D2 XS5, Q4 TOPK, and Q4 Sinkhorn for
  360 seconds each. Measured output rates were 23.8279, 23.8862, and 23.9577
  fps; control-to-processed-frame p95 was 61.714, 55.7192, and 52.0285 ms;
  intervals over two frames were 0.0979%, 0.1535%, and 0.0278%.
- Each mode ended with zero ring backpressure, an empty queue, no progressive
  host/worker/CUDA allocator growth, and no `.partial` residue. Receipt SHA-256:
  `e2aff6b59939772f395f75de62b97252f31cb35ec01adb9809e3411dd29b64ca`.
- The owner accepted those six-minute results for 0.1. A 30-minute run is not
  an open gate.
- The pinned official D3D12 receiver observed the native Spout2 sender with the
  declared name, RGBA8 format, correct portrait and landscape dimensions,
  advancing frames, and responsive shutdown in Player, D2, and Q4. See
  [Spout acceptance](../repository/SPOUT_ACCEPTANCE.md).

## Automated and local engineering evidence

- The final clean `dbe310a` clone passed
  `pwsh -NoProfile -File tools/Check-Workspace.ps1` and the public-tree audit.
- Frontend suites reported 108 LatentDeck and 32 LatentPlayer tests passing.
- Rust application suites reported 111 LatentDeck and 36 LatentPlayer tests
  passing.
- The Python workspace reported 382 passing tests with two optional CUDA tests
  skipped in that aggregate run. Separate accepted CUDA parity evidence remains
  recorded for the local RTX 4070.
- Focused tests cover long D2/Q4 capture ownership across source loops, MP4 row
  orientation, Library invalidation, capture hot insertion, PLAY/PREPARE,
  Codec Pack version coherence, setup lifecycle, packaging, diagnostics, and
  public-tree safeguards.
- Private cartridges, weights, raw latent data, test renders, and machine-local
  diagnostics remain outside Git.

## ComfyUI surfaces and open gallery task

Verified local evidence:

- a fresh isolated profile discovered all 33 Toolkit nodes, the official
  Recorder node, and both reviewed Channel Roll example nodes;
- Recorder E2E preserved H3 visual/audio cadence, passed full Rust cartridge
  validation, and left no temporary residue;
- eight public, data-free workflows (`01` through `07`, plus `99`) exist and
  load under the isolated workflow contract.

`comfy/toolkit/workflows/00_ALL_NODES_GALLERY.json` does not yet exist and has
not been visually accepted. The owner assigned it to the next agent as a
required release-documentation and presentation task. It must contain exactly
the 36 repository-owned node types, pass strict equality against the combined
`NODE_CLASS_MAPPINGS` registries, and open in the isolated CPU profile as a
readable Fit View canvas with no missing or red cards. The complete contract is
in the [current handoff](continue.md). Do not describe this item as completed
until the file, automated test, and visual evidence exist.

## Open release and publication gates

- **Release presentation and documentation:** create and verify the all-nodes
  gallery; prepare detailed public onboarding, release notes, repository copy,
  and any other owner-assigned release documentation.
- **Final clean build:** after all accepted documentation and gallery changes,
  create a fresh short-path clone, run the aggregate gate, and rebuild both
  applications and H3 Codec Pack from the same final clean commit.
- **Clean-machine lifecycle:** verify the final signed application installers
  and H3 setup plus adjacent payload, maintenance/upgrade/downgrade behavior,
  independent application removal, exact-version Codec Pack removal, recovery,
  external decoder selection, and Spout2 on a clean non-admin Windows 11 NVIDIA
  account without PowerShell 7, system Python, ComfyUI, or setup-time network
  access.
- **Security and publisher trust:** configure a private vulnerability-reporting
  channel and an authenticated signing path for both application installers,
  H3 setup, and its generated uninstaller.
- **Publication review:** inspect the exact Git archive and history, finish the
  attribution/license/SBOM/asset review, and complete the
  [public release checklist](../repository/PUBLIC_RELEASE_CHECKLIST.md).
- **Publication authority:** obtain explicit owner authorization before
  creating or changing a remote, pushing, tagging, uploading, or releasing.

Current classification: **owner-accepted local unsigned 0.1.0 functional
baseline; not signed or published**.
