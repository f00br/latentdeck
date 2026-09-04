## What changes and why?

Describe the user or developer problem, the owning boundary, and the proposed
behavior.

## Contract and compatibility

- [ ] I identified every affected format, API, protocol, package, or runtime
      version.
- [ ] The change is backward-compatible, or the intentional compatibility
      change is documented and versioned.
- [ ] No hidden conversion, fallback, package selection, or trust decision was
      introduced.

## Verification

- [ ] Focused tests cover success and failure behavior.
- [ ] Documentation is updated and public prose is in English.
- [ ] `tools/Test-PublicDocumentation.ps1` passes.
- [ ] `tools/Test-PublicTree.ps1` passes.
- [ ] `tools/Test-DeveloperOnboarding.ps1` passes, or the change cannot affect
      the public authoring path.
- [ ] `tools/Check-Workspace.ps1` passes, or I explain why it was not run below.
- [ ] `git diff --check` passes.

Checks not run and reason:

## Public and legal boundary

- [ ] This change contains no `.lc`, raw latent, weight, decoder, private media,
      secret, machine-local path, generated environment, or build output.
- [ ] New third-party code/assets/dependencies include source, version, license,
      purpose, and redistribution basis.
- [ ] I reviewed AI-assisted output and take responsibility for this diff.
