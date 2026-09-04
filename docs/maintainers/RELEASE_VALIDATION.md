# Release validation

Use this checklist for the exact source commit and artifact set proposed for a
release. Record evidence in generated receipts or the release review—not by
adding machine paths, private media, or transient status notes to public docs.

## Source and documentation

- [ ] The selected branch is `main`, the full commit is recorded, and the clean
      build clone has no tracked or untracked source changes.
- [ ] `Test-PublicDocumentation.ps1`, `Test-PublicAssetProvenance.ps1`,
      `Test-PublicTree.ps1`, `Test-PublicHistory.ps1`, and
      `Check-Workspace.ps1` pass.
- [ ] The exact `refs/heads/main` history and `git archive HEAD` have been
      inspected; no local `refs/codex/*` will be published.
- [ ] Public prose is canonical English, relative links resolve, and no
      handoff/status chronology, local path, credential, private URL, stale
      checkpoint, or unsupported novelty/performance claim remains.
- [ ] A cold reader can complete the Artist, Operator, Deck, Codec, and agent
      routes from the documentation hub.
- [ ] The changelog, release notes, compatibility page, manifests, and receipts
      agree on every independently versioned surface.

## Ownership, provenance, and security

- [ ] Original code/documentation attribution and Apache-2.0 coverage are
      accurate.
- [ ] Every distributed visual/media asset records author, origin,
      redistribution permission, hash, and intended use.
- [ ] Every third-party dependency or native component has exact source,
      version, license, purpose, and required notice.
- [ ] Artifact-scoped SBOMs separate runtime, build, and development
      dependencies and contain no unresolved critical/high runtime finding or
      unknown redistributed license.
- [ ] Each artifact's actual build-only SBOM set exactly matches its entry in
      `tools/packaging/windows-x64-release-build-only.lock.json`; the license
      evidence records `not_redistributed_no_text_required` and binds the
      reviewed lock's SHA-256.
- [ ] Any build-only scope drift was reviewed before the lock was deliberately
      updated; no runtime, embedded, native, or otherwise redistributed
      dependency was reclassified to suppress its license text.
- [ ] Model/decoder assets and cartridges remain separately licensed and are
      not implied to inherit the project license.
- [ ] The external demo pack is linked at one pinned Hugging Face revision;
      its file hashes and LC/H3 compatibility are independently validated, and
      its creator/source provenance, rights confirmation, and media terms are
      explicit before it is described as reusable or redistributable.
- [ ] Before public visibility, the Discord secure-channel fallback in
      `SECURITY.md` is ready; Private Vulnerability Reporting is a mandatory
      public-transition gate rather than a private-repository prerequisite.
- [ ] CI uses read-only permissions, pinned actions, and no signing secret in a
      pull-request context.
- [ ] Telemetry/crash reporting is absent or explicitly opt-in and documented.

### Dependency-review evidence

Run dependency audits from the clean candidate checkout. These commands use
registry/advisory network data and are release-review evidence, not an offline
or automatic CI guarantee:

```powershell
$nodeRoot = & .\tools\Get-PinnedNode.ps1
& (Join-Path $nodeRoot 'pnpm.cmd') audit --prod --audit-level high

$requirements = Join-Path `
  ([System.IO.Path]::GetTempPath()) `
  ("latentdeck-audit-{0}.txt" -f [guid]::NewGuid().ToString('N'))
try {
  uv export --all-packages --all-extras --locked --no-dev --no-hashes `
    --no-emit-workspace --format requirements-txt --output-file $requirements
  uvx pip-audit --version
  uvx pip-audit --strict --progress-spinner off --no-deps --disable-pip `
    --vulnerability-service osv -r $requirements
} finally {
  Remove-Item -LiteralPath $requirements -Force -ErrorAction SilentlyContinue
}

cargo audit --version
cargo audit
```

Record the audit-tool versions, advisory-data time, commands, exit codes, and
all findings in the release review. `uvx` may fetch `pip-audit`; record the
resolved version. The Python command audits the complete pinned third-party
closure through OSV without asking pip to re-resolve packages; preserve any
unaudited-package or incomplete-hash warning as a finding. A successful `cargo
audit` can still report allowed warnings such as unmaintained or unsound
transitive crates. Triage every warning against the actual release targets
with `cargo tree -i <crate> --target <triple>`; an empty Windows-target tree
does not prove the crate is absent from another supported build target. Do not
turn an advisory result into an unreviewed dependency update.

## Artifact identity and integrity

- [ ] Application, H3, Comfy Recorder, and Developer Kit receipts name the same
      full commit and tree, branch `main`, `git_dirty=false`, and exact public
      snapshot hash/file count.
- [ ] `release_label=0.1.0-preview.1` and
      `release_channel=unsigned_preview` are consistent across all sets.
- [ ] Application API `0.1.0`, installer build `0.1.0+1`, H3 pack `0.2.1`, H3
      adapter `0.2.0`, and D2/Q4 packages `0.2.1` match their manifests.
- [ ] Every preview receipt records the unsigned state: application, Comfy
      Recorder, and Developer Kit receipts use `signed=false` and
      `unsigned=true`, while H3 records
      `signing.mode=unsigned_local_rc` and its Authenticode evidence fields.
      Filenames and release prose say `unsigned` where required.
- [ ] Every nested checksum and copied-destination hash was recomputed.
- [ ] Each GitHub asset is below the [GitHub Releases 2 GiB per-file
      limit](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases)
      and appears exactly once in the staging allowlist with a unique flat
      name.
- [ ] Unified `RELEASE-MANIFEST.json` and `SHA256SUMS.txt` describe every
      uploaded asset and no extra file.
- [ ] SBOMs, notices, setup/uninstall licenses, and build receipts are
      hash-bound to the appropriate artifact.

## Application installation and user workflow

Perform on a clean non-admin Windows 11 account with the documented NVIDIA/CUDA
configuration:

- [ ] LatentDeck and LatentPlayer install independently as current-user apps,
      launch without existing data, and do not require H3 to show
      Library/Extensions/diagnostics.
- [ ] Removing or updating one application preserves the other application,
      Codec/Deck packages, Library/cartridges, and decoder selection.
- [ ] Player PREPARE validates a raw H3 batch, refuses collision/source-change
      cases without partial output, converts sequentially, supports Stop after
      current, and opens the exact result.
- [ ] Player opens portrait and landscape cartridges; Play/Pause/Restart/Loop,
      natural non-loop EOF, resize, fullscreen, and exact decoder/pack/device
      selection behave as documented.
- [ ] Library import, folder import, search, tags, favorite, Recent,
      Collections, ordering, All/Unassigned, rename/delete, and live
      invalidation preserve exact cartridge identities.
- [ ] D2 and Q4 accept compatible source sets, visibly refuse incompatible
      sets, update valid controls in realtime, preserve roles/playheads, and
      replay deterministically with the same seed/context.
- [ ] Snapshot and Live Capture write valid post-operator cartridges, survive
      valid loop boundaries, leave no partial file, enter Library, and can be
      inserted explicitly without reloading the application.
- [ ] D2/Q4 MP4 is upright video-only H.264 at intrinsic dimensions and 24 fps;
      capture/MP4 exclusion and no-clobber/cancel behavior hold.
- [ ] Player, D2, and Q4 publish and stop intrinsic Spout2 output without
      stretching, baked presentation bars, or receiver failure.
- [ ] Four sessions remain warm, a fifth is refused without eviction, Close
      releases capacity, and capture/MP4 pin the foreground output lease.
- [ ] Presets restore exact identities/roles/controls/seed/loop as a draft and
      never substitute a missing cartridge.
- [ ] Diagnostics save/cancel works in both applications and the reviewed
      bundle contains only the documented allowlist.

## H3 Codec Pack lifecycle

- [ ] Setup and exact adjacent `.ldcodec` pass archive and installed-runtime
      smokes before packaging acceptance.
- [ ] Setup fails closed when the payload is missing, renamed, truncated,
      changed, or from another version and leaves no visible installation.
- [ ] Correct setup installs offline for the current user without elevation,
      PowerShell, system Python, ComfyUI, applications, decoder, model, or
      setup-time network access.
- [ ] Installed Apps registers H3 `0.2.1`; Extensions discovers it disabled;
      verify, enable, explicit selection, matrix, disable, repair, and remove
      use the exact identity.
- [ ] Re-running setup does not overwrite or silently repair immutable package
      bytes.
- [ ] Removal affects only H3 `0.2.1` and preserves other pack versions,
      applications, cartridges, Library data, and decoder selection.
- [ ] The external decoder declaration has a verified anonymous source when a
      URL is advertised, exact length/SHA-256, license label, and license URL;
      the asset itself is not distributed.
- [ ] Active usage leases block repair/removal of the running exact version.

## Developer onboarding

Run `pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1`, then confirm:

- [ ] The source quickstart succeeds from a clean clone.
- [ ] The Developer Kit checksum validates and its bootstrap succeeds in a new
      Python 3.13 environment.
- [ ] Every shipped wheel imports expected modules and includes typing markers
      where declared.
- [ ] The generated compatibility manifest matches package metadata.
- [ ] The cartridge genealogy example produces a new validated UUID, parent
      hash, operation history, seed, audio disposition, and no-clobber result.
- [ ] The research operator passes synthetic bypass/determinism/streaming
      checks.
- [ ] The starter Deck passes scaffold/build/inspect/install/enable/matrix/
      disable/remove against a disposable root.
- [ ] The synthetic non-H3 Codec's direct SDK and in-process Protocol 2 command
      tests pass load/open/decode/reset/capture/abort/replay without GPU,
      weights, or private media.
- [ ] Its temporary-runtime test launches a copy of the current CPython
      executable and completes the authenticated Protocol 2 service bootstrap
      over an in-memory framed connector. Treat this as service/bootstrap
      evidence, not Windows named-pipe or installed-process-supervisor evidence.
- [ ] The synthetic Codec package lifecycle passes against a disposable root.
- [ ] JSON examples pass both machine schemas and authoritative Rust semantic
      parsers.

## Comfy LC Recorder bundle

- [ ] The standalone Recorder artifact set has exactly its ZIP, receipt,
      checksum manifest, artifact-scoped SBOM, notices, full-text license
      mapping, and license-review sidecar.
- [ ] The receipt binds the `cp312-abi3` Cartridge wheel, pure Recorder wheel,
      pinned Safetensors wheel, CPython 3.12/3.13 support, and the same clean
      source snapshot used by the other artifact sets.
- [ ] A clean extracted install succeeds without Rust or network access on
      CPython 3.12 and 3.13 and rejects an unsupported ABI before writing.
- [ ] Altered wheels, sidecars, receipt bindings, or checksum coverage are
      refused; the installer and artifact builders refuse overwrite.
- [ ] Importing the Recorder preserves any ambient ComfyUI `safetensors`
      module; the bundle's fallback remains under its private namespace.
- [ ] Removing only `custom_nodes/ComfyUI-LatentCartridge` after ComfyUI is
      closed removes the Recorder and its private dependencies without
      changing the host Python environment or other custom nodes.
- [ ] The Developer Kit contains the byte-identical Recorder ZIP and binds the
      standalone Recorder receipt identity without duplicating its sidecars.

## GitHub and publication

- [ ] Repository metadata, branch rules, merge policy, Issues/Discussions, and
      security settings match [GitHub settings](GITHUB_SETTINGS.md).
- [ ] CI has passed once in the private repository before its exact observed
      status context becomes required.
- [ ] If the private account plan does not expose repository rulesets, the
      exact `main` ruleset is prepared as a mandatory public-transition action.
- [ ] Public-safe showcase media and provenance are committed before the public
      switch; the final RC was rebuilt after that commit.
- [ ] The annotated release tag resolves to the exact validated commit.
- [ ] The draft prerelease contains only staged allowlisted assets.
- [ ] Every downloaded draft asset re-hashes against the unified checksum file.
- [ ] Release notes disclose the unsigned state, system/runtime boundary,
      external decoder, separate demo-pack terms, executable-extension trust,
      and known limitations.
- [ ] Explicit release-authority approval exists separately for the remote
      push, tag, asset upload, repository visibility change, and prerelease
      publication.
- [ ] After the repository becomes public and before the prerelease is
      published, the `main` ruleset is active and GitHub Private Vulnerability
      Reporting is enabled and verified without submitting a report.
- [ ] After launch, anonymous clone/download/checksum/install/launch/playback
      smoke passes.
