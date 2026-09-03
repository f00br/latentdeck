# LatentDeck 0.1.0 acceptance status

This is the current release-status record. It separates accepted local product
behavior from fresh-install, publisher-trust, and publication gates. It does
not authorize a remote, push, tag, upload, signing action, or public release.

Status on 2026-09-03: the owner accepted the completed Protocol 2 modular
runtime and final manual application pass with no known product defect
remaining. The accepted implementation checkpoint is clean `main` commit
`3648e7c634c4310767165ce8975129323a5c09f2`.

On 2026-09-04 clean `main` documentation checkpoint `0fd1303` repeated the
aggregate workspace and public-tree gates and generated the unsigned
first-install UAT sets for both applications and H3 `0.2.0`. Generated receipts
and `SHA256SUMS.txt` identify the exact bytes. Fresh-install UAT remains open.

## Version and format baseline

| Surface                         | Accepted identity |
| ------------------------------- | ----------------- |
| LatentDeck App and LatentPlayer | `0.1.0`           |
| LC specification and H3 profile | `0.1.0`           |
| H3 Codec Pack and adapter       | `0.2.0`           |
| Bundled D2 and Q4 Deck packages | `0.2.1`           |
| Worker control protocol         | `2`               |
| Codec manifest                  | `2.0.0`           |
| Deck manifest                   | `1.0.0`           |
| Codec and Deck Python SDKs      | `0.2.0`           |

The installable Deck extension is `.ld`. The installable Codec extension is
`.ldcodec`. `.lddeck` is not an alias, and the Protocol 2 H3 setup no longer
uses a separately named ZIP payload.

## Current clean-clone engineering evidence

The exact clean `0fd1303` clone, which contains the accepted implementation and
the coordinated handoff documentation, passed the aggregate workspace and
public-tree gates:

- LatentDeck frontend: 172 passed;
- LatentPlayer frontend: 49 passed;
- Rust: 694 passed, 0 failed, 21 expected ignored;
- Python: 422 passed;
- Codec Pack curator: 7 passed;
- Rust formatting and Clippy: passed;
- frontend checks/builds, package lifecycle, H3 setup tooling, Protocol 2
  conformance, diagnostics, and linked development-pack contracts: passed.

Private media/GPU tests and child-process fixtures remain intentionally outside
the aggregate test count. The aggregate gate does not claim a signed artifact
or clean-machine install.

## Accepted package and runtime architecture

- One common Extensions lifecycle handles `.ld` and `.ldcodec`: inspect,
  explicit expected SHA-256, install disabled, verify, enable, disable, repair,
  remove, list, and matrix.
- Installation remeasures the local archive, extracts into a bounded sibling,
  validates a closed integrity tree, atomically publishes one immutable exact
  version, and atomically records trust. Publisher metadata remains
  self-declared.
- Exact versions coexist side by side and are selected explicitly. There is no
  overwrite, URL install, inherited trust, or automatic newest selection.
- The compatibility resolver reports stable reasons including trust, asset,
  package, protocol, host API, tensor ABI, profile, signal, timing, and
  capability incompatibility. It performs no hidden conversion or fallback.
- Core retains an integrity-validated cartridge handle. Codec-specific profile
  validation and memory estimates come from the selected adapter and are
  cross-checked before GPU allocation.
- Worker Protocol 2 carries authenticated bounded control messages only.
  Latent/RGBA bytes do not travel through control IPC. Protocol 1 remains only
  as an explicit Player bridge and is never a fallback for a Protocol 2 fault.
- D2, Q4, and external Decks share the same Deck SDK, declarative host-rendered
  faceplate, generic scheduler, capture finalizer, and session broker.
- A faceplate can change layout and controls without owning Tauri, Win32, DX12,
  worker launch, package trust, or cartridge validation.

## Owner-accepted functional behavior

### Player, Library, and Extensions

- LatentPlayer opens and presents a selected `.lc` through an exact enabled
  Codec version and explicit decoder binding.
- Raw import is an optional selected-codec capability; Core builds and reopens
  the final `.lc` rather than trusting an adapter-written Library file.
- Library and Collections preserve search, tags, favorites, ordering, virtual
  banks, active filters, and exact cartridge identities.
- Installed compatible and incompatible external Decks become visible without
  an application rebuild or restart. Incompatible packages remain inspectable
  and display the stable reason without a selectable fallback.

### D2, Q4, and declarative controls

- Bundled D2/Q4 are exact trusted `.ld` packages, not hardcoded production
  runtimes. Their `0.2.1` faceplates use the same renderer as an external Deck.
- Realtime synthesis controls update the running output immediately. Controls
  that do not apply to the current algorithm/mode are not rendered.
- Role permutations, independent transport, seed, restart, loop, and natural
  non-loop EOF remain authoritative worker state. EOF pauses only the exhausted
  source and does not destroy the warm session or output surface.
- Compatible sources play smoothly on the accepted CUDA/H3 system. Per-frame
  ZIP/Safetensors revalidation, CPU staging, and synchronous finite checks are
  not repeated in the realtime slot loop.
- Exact geometry/timing/profile differences produce a visible refusal. No
  source is resized, cropped, aligned, cast, re-encoded, or substituted.

### Capture, MP4, output, and sessions

- Snapshot and Live Capture record the post-operator latent before decode,
  finalize atomically, import into Library, and can be loaded into a slot
  immediately without reloading the Deck.
- Live Capture remains active across valid automatic source loops and stops only
  through the explicit bounded capture lifecycle.
- D2/Q4 MP4 is upright video-only H.264 at intrinsic geometry. Capture and MP4
  remain mutually exclusive while normal playback continues.
- Embedded output uses a compact sticky program monitor; scrolling controls no
  longer make the native output disappear. Fullscreen hides the surrounding
  faceplate and restores the same session on exit.
- Spout uses the intrinsic shared output and retains its explicit sender
  lifecycle.
- Four sessions may remain warm. A fifth receives
  `session.capacity_exceeded`; no session is evicted. Close releases a session.
- Live Capture and MP4 independently pin the single foreground output lease.
  Switching is refused with `session.output_lease_pinned` until the owning job
  stops.

## ComfyUI acceptance

- The isolated environment discovers all repository-owned nodes.
- `00_ALL_NODES_GALLERY.json` contains exactly one instance of each of the 33
  Toolkit nodes, one Recorder node, and two reviewed Channel Roll nodes.
- Automated strict equality against the combined three `NODE_CLASS_MAPPINGS`
  registries passes; the graph is public, data-free, and non-queueable.
- The owner accepted the isolated CPU Fit View with no missing or red cards.
- Recorder and the eight executable public workflows retain their independent
  tests; the gallery is a discovery canvas, not an execution graph.

## Evidence classification

The earlier unsigned `dbe310a` application/H3 `0.1.1` artifacts and the private
Protocol 2 GPU run at `bf1e189` are historical evidence only. Later source
changes superseded those artifact identities, and the private receipt was not
retained. They must not be mixed with current receipts or presented as the
final release candidate.

The final manual pass used the current application implementation and the real
H3/CUDA path. It is accepted product UAT, but it is not a signed clean-machine
receipt. Fresh unsigned installer identities were then generated from clean
`main` checkpoint `0fd1303`; their receipts and `SHA256SUMS.txt` are the
authority for exact bytes. This post-build documentation update makes that set
an older source snapshot. It remains valid for the pending owner first-install
UAT, but a new clean-clone RC must be built from the final accepted commit
before publication review.

## Open release and publication gates

- **Fresh first-install UAT:** install the new two application setups and H3
  setup/adjacent `.ldcodec` from a clean local state; configure Extensions and
  the external decoder; spot-check Player, D2, and Q4.
- **Final public documentation:** complete onboarding, SDK/architecture prose,
  release notes, limitations, and repository presentation assigned by the
  owner.
- **Clean-machine lifecycle:** repeat the final signed install/update/remove
  matrix offline on a clean non-admin Windows 11 NVIDIA account.
- **Security and publisher trust:** configure a private vulnerability channel
  and authenticated signing for both application installers, H3 setup, and its
  generated uninstaller.
- **Publication review:** inspect the exact Git archive and history and finish
  attribution, license, SBOM, and distributed-asset review.
- **Publication authority:** obtain explicit owner authorization before any
  remote change, push, tag, upload, or release.

Current classification: **owner-accepted local unsigned 0.1 application and
Protocol 2 modular-runtime milestone; fresh installer UAT and publication gates
remain open**.
