# GitHub repository settings

This is the target configuration for the single
`f00br/latentdeck` repository. Apply every available setting while the
repository is private. Two controls have platform timing constraints described
below: private-repository rulesets depend on the account plan, and GitHub
documents Private Vulnerability Reporting for public repositories. Complete
both during the controlled public-visibility transition when they cannot be
completed earlier. This document does not authorize any setting or remote
change.

## Repository identity

- Name: `latentdeck`
- Repository account: `f00br`
- Description: `Open tools and formats for realtime synthesis of saved generative latent representations.`
- Website: leave empty for the preview.
- Topics: `generative-art`, `video-synthesis`, `latent-space`, `realtime`,
  `vj`, `comfyui`, `rust`, `tauri`
- Default branch: `main`
- Repository type: one monorepo; do not create an Organization for the preview.

Keep visibility private through source upload, first CI, settings verification,
showcase review, final RC rebuild, and draft prerelease review.

## Features

Enable:

- Issues with the repository forms;
- Discussions with `Announcements`, `Q&A`, `Research / Ideas`, and `Show and
  tell` categories;
- dependency graph;
- Dependabot alerts/security updates and the checked-in weekly version-update
  configuration for Cargo, pnpm, uv, and GitHub Actions;
- secret scanning and push protection where available.

Private Vulnerability Reporting is a public-launch control, not a
private-repository prerequisite. Its enablement sequence is defined under
[Security configuration](#security-configuration).

Create the labels referenced by the Issue forms before opening Issues:
`bug`, `documentation`, `research`, `extension`, and `needs-triage`. Add
`security` only for public, non-sensitive hardening work; private reports stay
inside Security Advisories.

Disable the wiki for the preview so canonical documentation remains versioned
with the source. Do not enable an external app, bot, or deployment service
without a separate review of permissions and data access.

## Pull requests and merge policy

Allow squash merge only. Disable merge commits and rebase merges so `main`
retains a linear sequence of reviewed changes. Automatically delete merged
branches when practical.

All contributors—including collaborators—use a branch and pull request. AI
authorship does not change review or test requirements.

## Main ruleset

[GitHub rulesets](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/about-rulesets)
are available for public repositories on GitHub Free. A private repository
needs GitHub Pro, GitHub Team, or GitHub Enterprise Cloud. If the current plan
supports private-repository rulesets, create and verify the ruleset after the
first CI run. Otherwise, preserve this exact configuration as a pending launch
control and activate it immediately after the visibility change, before
publishing the prerelease.

Create one active ruleset targeting `main`:

- restrict deletion;
- block force pushes;
- require a pull request before merging;
- require linear history;
- require all conversations to be resolved;
- require the exact CI status context observed from the first successful
  private-repository run;
- require the branch to be up to date when that setting does not make the
  single-maintainer flow unusable;
- start with zero required approvals while `@f00br` is the only maintainer;
- do not require signed commits because the retained project history is
  unsigned.

Run CI once before entering a required-status name. Do not guess a workflow or
job label: select the exact context GitHub reports. Raise required approvals to
one when a second active maintainer can review changes independently.

Do not add a broad bypass list. Emergency changes still use a pull request and
record why normal checks could not run.

## Security configuration

Set GitHub Actions token permissions to read repository contents by default.
Grant a narrower write permission only inside a reviewed workflow/job that
needs it. Fork pull requests must not receive signing material or other
repository secrets.

[GitHub currently documents Private Vulnerability
Reporting](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/configure-vulnerability-reporting/configure-for-a-repository)
for public repositories. During private review, the secure-channel fallback in
[SECURITY.md](../../SECURITY.md) is the reporting route. At the approved
public-visibility transition, enable Private Vulnerability Reporting and
confirm the **Security → Report a vulnerability** path before publishing the
prerelease. Keep security reports separate from public Issues and Discussions.

Review Dependabot findings before release. A dependency alert is evidence to
triage, not permission for an automatic unreviewed version change.

## Private-first source publication

Use a dedicated publication clone containing only the selected committed
`main`. Add the GitHub remote only after explicit authorization, inspect refs,
then push exactly:

```powershell
git push <github-remote> HEAD:refs/heads/main
```

Never publish with `--all`, `--mirror`, or a wildcard refspec. Verify on GitHub
that only intended branches/tags exist and that no local `refs/codex/*` or
build-clone refs were transferred.

## Before public visibility

- CI passes on the exact final source commit.
- Community files render and Issue forms open correctly.
- Discussions categories are usable, and the pre-launch private reporting
  fallback in `SECURITY.md` is ready.
- The approved hero/showcase media has documented provenance, license, hash,
  alt text, and repository-size review.
- Artifacts were rebuilt after the final showcase/documentation commit.
- The draft prerelease and downloaded hashes passed [release
  validation](RELEASE_VALIDATION.md).
- Branch/rules/security/merge settings were independently reviewed; any
  plan-blocked ruleset is recorded as a mandatory transition action.
- Release authority explicitly approved both the public visibility change and
  prerelease publication.

## At the public-visibility transition

Treat the transition as a gated sequence:

1. make the repository public after explicit approval;
2. activate and verify the `main` ruleset if the private plan did not support
   it;
3. enable Private Vulnerability Reporting and verify the public **Report a
   vulnerability** path;
4. publish the prerelease only after both controls are active.

The visibility change does not waive either launch control. After publication,
perform an unsigned/signed-out anonymous clone and release download check
instead of relying on the maintainer's authenticated view.
