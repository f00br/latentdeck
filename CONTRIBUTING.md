# Contributing

LatentDeck 0.1 owner UAT and the Protocol 2 modular-runtime milestone are
complete. The repository is in coordinated release and publication
preparation; uncoordinated feature work is not open yet.

For any explicitly assigned change:

1. Read `AGENTS.md`, `docs/release/continue.md`, and the relevant specification.
2. Keep the change inside the assigned component and scope.
3. Do not add weights, cartridges, generated media, private datasets, secrets,
   copied environments, or unreviewed third-party code/assets.
4. Add focused tests and documentation with the implementation.
5. Run the relevant component checks and
   `pwsh -NoProfile -File tools/Test-PublicTree.ps1`.
6. Review `git status --short` and `git diff --check` before handing work off.

Dependency additions must record source, version, license, purpose, and why the
dependency belongs in the affected component. Approved dependency changes must
include the reviewed lock-file updates; dependency caches and vendored package
trees should not be committed.

No contributor may publish the repository, alter remotes, create releases, or
change the project license without explicit owner authorization.
