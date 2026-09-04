# Governance

LatentDeck begins as a maintainer-led open-source project. Governance is kept
small enough for the preview while making contribution and release decisions
visible.

## Roles

- **Users and contributors** may participate through Discussions, Issues,
  forks, and pull requests.
- **Collaborators** may create branches in the main repository, but use the
  same pull-request checks as external contributors.
- **Maintainers** triage work, review changes, protect contracts, moderate
  community spaces, and prepare releases.
- **Release authority:** `@f00br` is the initial maintainer and final release
  authority for the preview line.

Repository write access does not grant authority to bypass `main` protection,
publish artifacts, change licenses, alter security settings, or speak for the
project outside an assigned role.

## Decisions

Small implementation decisions are made in the pull request that contains the
change. Shared format, API, compatibility, dependency, security, or governance
changes should begin as an Issue or Discussion and record:

- the problem and affected readers or users;
- the stable contract involved;
- compatibility and migration effects;
- alternatives considered;
- evidence and tests needed for acceptance.

Maintainers seek practical consensus, but the release authority makes the
final decision when consensus is not possible. Rejected proposals may remain
as independent extensions when they can use a public contract without changing
the core project.

## Reviews and releases

All changes to `main`, including collaborator changes, use a pull request and
required checks. The initial single-maintainer ruleset may require no separate
approval because a person cannot approve their own pull request; this should be
tightened when another active maintainer joins.

A release is built from a clean, committed `main` revision and follows the
[release process](docs/maintainers/RELEASE_PROCESS.md). Passing tests or having
write access does not itself authorize a tag, artifact upload, visibility
change, or release publication.

## Becoming a maintainer

Maintainer access may be offered after a contributor has shown sustained,
constructive work across review, testing, documentation, and community support.
The initial maintainer decides appointments and records them in this file.
Maintainers may step down at any time; their contributions remain under the
project license.

## Conduct and security

Community moderation follows the [Code of Conduct](CODE_OF_CONDUCT.md).
Security reports follow [SECURITY.md](SECURITY.md) and are not debated in a
public issue before coordinated disclosure.
