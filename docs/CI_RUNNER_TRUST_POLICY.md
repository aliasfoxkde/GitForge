# CI Runner Trust & Required-Check Policy

Resolves #318 (evidence 2026-10-15; implemented 2026-11-02).

## Execution boundary for untrusted PR code

This repository is **public with fork-allow enabled**. GitHub's guidance is
unambiguous: persistent self-hosted runners must never execute code from
untrusted `pull_request` events, because PR checkout runs with no secrets but
full access to the runner host's filesystem, Docker socket, and any ambient
credentials.

### Policy

`pull_request` jobs route by origin:

- **Same-repo PRs and all `push`/`tag`/`schedule` events** → persistent
  `[self-hosted, linux, x64, gitforge-github]` runner (`fedora-gitforge-01`).
- **Fork PRs** → ephemeral GitHub-hosted `ubuntu-latest` runners. Same checks,
  same steps, disposable host.

Implementation: every job that can fire on a `pull_request` uses

```yaml
runs-on: ${{ !github.event.pull_request.head.repo.fork && fromJSON('["self-hosted", "linux", "x64", "gitforge-github"]') || 'ubuntu-latest' }}
```

Fork-reachability audit (2026-11-02): rust-ci (7 jobs), security (5), codeql
(1), integration (2), gitforge-ci (3), ai-review (1) — all guarded.
`always.yml` runs its trivial echo on `ubuntu-latest` unconditionally.
`auto-merge.yml` is fork-gated by its `user.login == repository.owner.login`
condition. `release.yml` and `wiki.yml` have no `pull_request` trigger.

### Secrets on fork PRs

Fork PRs receive no `secrets.*` (except `GITHUB_TOKEN`, read-scoped). This is
load-bearing: `gitforge-ci.yml` additionally gates its `enqueue-build` and
`report-status` jobs on `!github.event.pull_request.head.repo.fork`, because
fork PR head commits do not exist on the 202 mirror and the trigger token
would 401 anyway. A fork PR therefore skips the GitForge pipeline leg rather
than failing against it.

### Residual exposure

`pull_request` forks still get a *fresh* GitHub-hosted VM, so no persistent
state (Fedora services, NAS mounts, runner workspaces, Docker images) is
reachable from fork code. There is no `pull_request_target` usage in this
repo; adding one requires the same review bar as a credential-bearing change.

## Required checks

The live `Safeguards` ruleset (as of 2026-11-02) contains **no
`required_status_checks` rule**: enforcement is via `code_coverage`
(min 70%, max drop 10%) and `code_scanning` (CodeQL, all alerts). Branch
protection is not separately configured. Consequently the historical
concern — path-filtered workflows leaving 20 required contexts pending on a
docs-only PR — does not apply to the current ruleset.

If required contexts are re-added, follow the issue's rule: gate workflows
must trigger unconditionally (event filters on `types`, never `paths`) with
per-job `paths` scoping inside, so every required context is emitted for
every applicable PR.

## Promotion of changes to this policy

Workflow or ruleset changes follow the standard review path: PR into `main`,
validated by the checks this repo runs (including the GitForge pipeline on
the 202 instance), merged under the Safeguards ruleset. The ruleset itself
changes via the API (`PUT /repos/{owner}/{repo}/rulesets/{id}`) and must be
verified afterwards by reading it back.
