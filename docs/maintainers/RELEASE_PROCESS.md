# Release process

This procedure creates a reproducible LatentDeck release without mixing source
states or artifact sets. It documents the work; only the release authority in
[Governance](../../GOVERNANCE.md) may authorize remote changes, tags, uploads,
visibility changes, or publication.

## Release model

The preview channel uses these independent identities:

- release label and tag: `0.1.0-preview.1` / `v0.1.0-preview.1`;
- release channel: `unsigned_preview`;
- application compatibility/API version: `0.1.0`;
- Windows installer build identity: `0.1.0+1`;
- H3 Codec Pack: `0.2.1` with adapter `0.2.0`.

The preview intentionally omits a signing command, and every artifact receipt
must record its unsigned state. The application, Comfy Recorder, and Developer
Kit receipts use `signed=false` and `unsigned=true`; the H3 receipt records
`signing.mode=unsigned_local_rc` together with its Authenticode evidence fields.
A future stable channel must provide an
authenticated signing command and validate the installer, generated
uninstaller, and installed executables before hashes are finalized. Follow the
official [Tauri Windows signing
guide](https://v2.tauri.app/distribute/sign/windows/) when that stable gate is
implemented.

## 1. Finalize the source candidate

1. Complete code, canonical English documentation, release notes, changelog,
   notices, schemas, and compatibility metadata in the primary `main` checkout.
2. Ensure every accepted change is committed. Preserve unrelated local work;
   do not build a release from a dirty primary checkout.
3. Run focused tests for every changed boundary.
4. Inspect `git status --short`, the exact staged candidate before each commit,
   `git diff --cached`, and `git diff --cached --check`.

Any accepted source or documentation commit after an artifact build makes that
artifact set an older source snapshot. Rebuild; never patch generated output or
silently reuse an old build clone.

## 2. Create a clean short-path clone

Use a new, empty, short Windows destination. Long paths can exceed MSVC
FileTracker limits before application code runs.

```powershell
git clone --no-hardlinks <primary-checkout> <new-short-build-path>
Set-Location <new-short-build-path>
git branch --show-current
git rev-parse HEAD
git status --short
```

The branch must be exactly `main`, `HEAD` must be the selected full commit, and
status must be empty. The clone's automatic local `origin` points to the primary
checkout and is not permission to use a GitHub remote.

## 3. Run source and publication gates

```powershell
pwsh -NoProfile -File tools/Test-PublicDocumentation.ps1
pwsh -NoProfile -File tools/Test-PublicTree.ps1
pwsh -NoProfile -File tools/Test-PublicHistory.ps1
pwsh -NoProfile -File tools/Test-DeveloperOnboarding.ps1
pwsh -NoProfile -File tools/Check-Workspace.ps1
git status --short
```

`Test-PublicHistory.ps1` audits exactly `refs/heads/main` for forbidden
historical payloads, oversized blobs, and high-confidence credentials. It also
warns when local `refs/codex/*` exist. CI checks out complete history and runs a
SHA-pinned Gitleaks action with the pinned default-rule configuration in
addition to this repository-specific audit. Review the intended
`git archive HEAD` manually in addition to the automated gates.

The full workspace gate may create ignored caches/artifacts, but it must not
modify tracked files. Stop if the final status is not empty.

## 4. Build independent artifact sets

Build all sets from the same clean commit and keep each generated directory
intact with its receipt, checksums, SBOM, and notices.

Applications:

```powershell
pwsh -NoProfile -File tools/Build-ReleaseCandidate.ps1 `
  -ReleaseChannel unsigned_preview `
  -ReleaseLabel 0.1.0-preview.1
```

H3 Codec Pack, using the exact pinned CPython embed archive:

```powershell
pwsh -NoProfile -File tools/Build-H3CodecPack.ps1 `
  -PythonEmbedArchive <pinned-python-embed-archive> `
  -PackVersion 0.2.1
```

Comfy LC Recorder, using the exact pinned Safetensors wheel named and hashed in
`comfy/latent-cartridge/packaging/windows-x64.lock.json`:

```powershell
pwsh -NoProfile -File tools/Build-ComfyRecorderBundle.ps1 `
  -SafetensorsWheelPath <pinned-safetensors-wheel>
```

Developer Kit, embedding the exact Recorder ZIP from that complete artifact
set:

```powershell
pwsh -NoProfile -File tools/Build-DeveloperKit.ps1 `
  -ComfyRecorderArtifactDirectory <comfy-recorder-artifact-directory> `
  -ReleaseChannel unsigned_preview `
  -ReleaseLabel 0.1.0-preview.1
```

Pass the optional `-SpoutArchivePath` to the application builder when an exact
pinned upstream archive is already available locally. Otherwise the builder
retrieves only the pinned archive and verifies its recorded identity. Release
output paths, when overridden, must remain below the checkout's ignored
`artifacts` root and must not already exist.

Do not combine files from an earlier build or copy individual artifacts between
sets. When preserving a candidate outside the build clone, copy the complete
version directory into a new non-existing destination and recompute every
listed SHA-256 at the destination.

### Build-only license scope

`tools/packaging/windows-x64-release-build-only.lock.json` is the reviewed,
exact per-artifact allowlist of SBOM components that participate in a build but
whose bytes are not redistributed in that artifact. License evidence keeps
those components visible with the disposition
`not_redistributed_no_text_required`, while omitting their full license texts
from the user-facing artifact. The evidence also binds the SHA-256 of the scope
lock, and generation fails unless each artifact's actual build-only component
set exactly matches its locked set.

Treat any drift as a review event: establish from the produced artifact that a
component is still build-only before deliberately updating the lock. Never
reclassify a runtime, embedded, native, or otherwise redistributed dependency
as build-only to avoid carrying its license text.

## 5. Validate artifacts

Follow [Release validation](RELEASE_VALIDATION.md). At minimum:

- receipts identify the same full source commit and clean `main` state;
- release label/channel and all independent package/API versions match;
- the preview records every executable as unsigned;
- every SBOM/notices file is artifact-scoped and hash-bound;
- H3 setup is bound to the exact adjacent `.ldcodec` identity;
- archive and installed-runtime smokes pass;
- the Developer Kit bootstraps in a new Python 3.13 environment;
- the complete user install/play/synthesize/capture/output/remove matrix passes
  on the documented clean Windows account.

Keep behavior/artistic acceptance distinct from final packaging evidence. A
previous functional pass can remain valid when runtime code is unchanged, but
the final installers still require source-identity, install, and smoke checks.

## 6. Stage the five-file GitHub release

Use the staging tool with the four complete artifact directories and a new
output directory:

```powershell
pwsh -NoProfile -File tools/Stage-GitHubRelease.ps1 `
  -ApplicationArtifactDirectory <application-artifact-directory> `
  -CodecArtifactDirectory <h3-artifact-directory> `
  -DeveloperKitArtifactDirectory <developer-kit-artifact-directory> `
  -ComfyRecorderArtifactDirectory <comfy-recorder-artifact-directory> `
  -OutputDirectory <new-release-staging-directory>
```

The tool requires one source commit and release channel, verifies every nested
checksum and receipt, refuses overwrite, and rejects any resulting file at or
above the [GitHub Releases 2 GiB per-file
limit](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases).
It emits exactly these five assets for the preview:

| Asset | GitHub display label |
| --- | --- |
| `LatentDeck-0.1.0-preview.1-Artist-Starter-Windows-x64-unsigned.zip` | `For artists - Player, LatentDeck, and H3 Codec Pack` |
| `LatentDeck-0.1.0-preview.1-comfy-recorder-windows-x64.zip` | `For ComfyUI - LC Recorder for Windows x64` |
| `LatentDeck-0.1.0-preview.1-developer-kit-windows-x64.zip` | `For developers - SDKs, examples, and tools` |
| `LatentDeck-0.1.0-preview.1-Release-Evidence.zip` | `Verification - receipts, SBOMs, licenses, and manifests` |
| `LatentDeck-0.1.0-preview.1-SHA256SUMS.txt` | `Verification - SHA-256 checksums` |

The Artist Starter contains both application installers and an `H3-Codec`
directory where setup remains adjacent to its exact hash-bound `.ldcodec`. It
also contains `README-FIRST.txt`, the project and redistributed third-party
license material, and an internal checksum manifest. The Recorder and
Developer Kit archives remain byte-identical to their validated artifact-set
outputs. The Release Evidence archive contains the source receipts, input
checksum manifests, artifact-scoped SBOMs, licenses, notices, smoke records,
and `RELEASE-MANIFEST.json`; its internal checksum verifies that inventory.
The outer checksum covers the other four release assets and conventionally
excludes itself.

Apply the exact display labels recorded in `RELEASE-MANIFEST.json` when
uploading the assets. Review the complete five-file staged directory and both
archive inventories. No installer, archive, receipt, checksum, SBOM, license,
notice, or manifest may be replaced without restaging and repeating
validation.

## 7. Prepare the private repository

Follow [GitHub settings](GITHUB_SETTINGS.md). Publication uses a dedicated
single-branch clone and an exact refspec:

```powershell
git push <github-remote> HEAD:refs/heads/main
```

Never use `--all`, `--mirror`, or a wildcard refspec. Local working refs are not
part of the public project.

Run private CI once before making its observed status context mandatory. Add
the approved showcase media and provenance while the repository is still
private. Because that is another source commit, rebuild the final artifacts
from a new clean clone afterward.

## 8. Draft, verify, and publish

1. Create the annotated tag `v0.1.0-preview.1` only after release-authority
   approval, pointing to the exact validated source commit.
2. Create a draft GitHub prerelease and upload only the staged allowlist.
3. Download every asset anonymously where possible. Compare each of the four
   non-checksum assets with the outer checksum file, and separately review the
   checksum file as canonical UTF-8/LF text from the official release page.
4. Verify release notes, installer warning, SBOM/notices, source archive, and
   links while the repository remains private.
5. Obtain explicit approval for the visibility change and prerelease
   publication.
6. Make the repository public in the coordinated launch window. If the private
   account plan did not expose rulesets, immediately activate and verify the
   prepared `main` ruleset. Enable and verify GitHub Private Vulnerability
   Reporting.
7. Publish the prerelease only after both public-transition controls are
   active.
8. Perform an anonymous clone, release download, checksum, install, launch, and
   minimal playback smoke.

Passing any local or private gate does not imply authority for the next
external action.
