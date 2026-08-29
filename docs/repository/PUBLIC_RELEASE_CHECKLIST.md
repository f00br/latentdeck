# Public release checklist

Checked items record durable evidence only. Completing repository bootstrap or
this checklist does not authorize publication; the owner must still explicitly
approve every action in the Publication authority section.

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

## Product and security

- [ ] The release matches a documented `.lc`, Codec API, Operator API, and app
      version contract.
- [ ] Untrusted-cartridge validation and memory limits are tested.
- [ ] Cartridges are data-only and cannot execute embedded code.
- [ ] Supported platforms, hardware, codec dependencies, and limitations are
      documented from measured results.
- [ ] A private vulnerability-reporting channel is configured.
- [ ] Telemetry and crash reporting, if any, are opt-in and documented.

## Publication authority

- [ ] The owner explicitly approved creation or use of the GitHub remote.
- [ ] Repository visibility, branch protection, and release permissions were
      reviewed.
- [ ] The owner explicitly approved the release tag and uploaded artifacts.
