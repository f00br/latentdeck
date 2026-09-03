# Public release checklist

Checked items record durable evidence only. Completing a local release candidate
or this checklist does not authorize publication; the owner must still
explicitly approve every action in the Publication authority section.

Current phase: the owner accepted the completed Protocol 2 modular runtime and
final local `0.1.0` application pass on 2026-09-03 at implementation commit
`3648e7c`. Clean `main` checkpoint `0fd1303` produced the unsigned application
and H3 first-install UAT sets with generated receipts and checksums. The owner
UAT remains open. This later documentation update makes those artifacts an older
UAT snapshot; rebuild from the final accepted commit before publication review.
Every unchecked item below remains open.

## Ownership and legal

- [x] The owner selected Apache-2.0 for original code and documentation; see
      `LICENSE`.
- [ ] Copyright and contributor attribution are accurate.
- [ ] Every distributed documentation asset has recorded origin and
      redistribution permission. Local-only concept PNGs remain ignored.
- [ ] Third-party source and dependencies have a license inventory.
- [ ] Codec/model assets and cartridges are governed separately and are not
      implied to inherit the LatentDeck license.
- [ ] An SBOM and dependency-license review exist for the release build.
- [ ] The H3 Codec Pack setup SBOM/notices cover NSIS, the native lifecycle
      helper, and every shipped helper dependency separately from the runtime
      payload inventory.
- [ ] The application SBOM contains the exact pinned upstream Spout2 component,
      commit, archive hash, native integration provenance, and BSD-2-Clause
      license; the hash-bound `THIRD_PARTY_NOTICES.md` accompanies installers.

## Repository contents

- [ ] `pwsh -NoProfile -File tools/Test-PublicTree.ps1` passes.
- [ ] `git status --short --ignored` has been manually reviewed.
- [ ] The exact staged file list and diff have been manually reviewed.
- [ ] The Git archive intended for upload has been opened and inspected.
- [ ] Git history contains no weights, cartridges, datasets, secrets, signing
      material, private media, generated output, or copied environments.
- [ ] Large files are intentional and have provenance; Git LFS is not treated as
      a substitute for permission.
- [ ] Documentation contains no private absolute paths, tokens, internal URLs,
      or unsupported benchmark claims.
- [x] `comfy/toolkit/workflows/00_ALL_NODES_GALLERY.json` exists as a public,
      data-free presentation canvas, passes strict registry equality, and opens
      without missing or red node cards in the isolated ComfyUI profile.

## Product and security

- [ ] The release matches a documented `.lc`, Codec API, Operator API, and app
      version contract.
- [ ] Untrusted-cartridge validation and memory limits are tested.
- [ ] Cartridges are data-only and cannot execute embedded code.
- [ ] Supported platforms, hardware, codec dependencies, and limitations are
      documented from measured results.
- [ ] The signed `LatentDeck-H3-CodecPack-<version>-setup.exe` is bound to the
      exact adjacent payload filename, byte length, SHA-256, identity, and
      version; the complete pair and non-recursive receipts were inspected.
- [ ] On a clean non-admin Windows 11 NVIDIA account, H3 setup installs offline
      to the fixed current-user root without PowerShell, system Python, ComfyUI,
      model assets, or setup-time network access.
- [ ] Windows Installed Apps removes one exact H3 version while preserving
      other versions, both applications, cartridges, and decoder selection;
      immutable side-by-side installation and the 16-version bound were tested.
- [ ] Application installers, H3 setup, its generated uninstaller, checksums,
      and receipts were finalized and verified through the authenticated
      publisher signing path before hashes intended for publication were
      recorded.
- [ ] A private vulnerability-reporting channel is configured.
- [ ] Telemetry and crash reporting, if any, are opt-in and documented.

## Publication authority

- [ ] The owner explicitly approved creation or use of the GitHub remote.
- [ ] Repository visibility, branch protection, and release permissions were
      reviewed.
- [ ] The owner explicitly approved the release tag and uploaded artifacts.
