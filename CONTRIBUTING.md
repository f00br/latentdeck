# Contributing

LatentDeck 0.1 is under active local implementation. Contributions are not open
for uncoordinated feature work yet.

For any explicitly assigned change:

1. Read `AGENTS.md` and the complete 0.1 plan.
2. Keep the change inside the assigned component and scope.
3. Do not add weights, cartridges, generated media, private datasets, secrets,
   copied environments, or unreviewed third-party code/assets.
4. Add focused tests and documentation with the implementation.
5. Run the relevant component checks and
   `pwsh -NoProfile -File tools/Test-PublicTree.ps1`.
6. Review `git status --short` and `git diff --check` before handing work off.

Dependency additions must record source, version, license, purpose, and why the
dependency belongs in the affected component. Generated lock files should be
committed once a real workspace exists; dependency caches and vendored package
trees should not.

No contributor may publish the repository, alter remotes, create releases, or
change the project license without explicit owner authorization.
