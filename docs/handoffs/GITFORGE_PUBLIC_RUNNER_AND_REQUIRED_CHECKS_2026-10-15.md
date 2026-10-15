# GitForge public PR runner and required-check remediation

**Status:** handoff prepared; no live runner or ruleset setting changed by this handoff.  
**Evidence checked:** 2026-10-15.  
**Tracker:** [GitForge issue #318](https://github.com/aliasfoxkde/GitForge/issues/318).

## Verified state

- `aliasfoxkde` is a personal GitHub account (`GET /users/aliasfoxkde` reports `User`), not an organization. GitHub organization runner-group controls are therefore not available at this owner scope.
- GitForge is public and allows forking. The repository runner API reports repository-level runners: ID 21 `fedora-gitforge-01` (online/busy during this audit) and ID 22 `fedora-gitforge-02` (offline). Their labels include `self-hosted`, `linux`, `x64`, and `gitforge-github`.
- The active `Safeguards` ruleset (ID `20319553`) applies to the default branch, is strict, has no bypass actors, and requires these 20 contexts: `check`, `integration`, `e2e`, `Test`, `Test (Serialized)`, `Build`, `Clippy`, `Format`, `Coverage`, `Lint Workflows`, `ShellCheck`, `Security Audit`, `Cargo Audit`, `Rust Security`, `secrets-scan`, `Aegis Changed-Line Scan`, `Supply Chain`, `Analyze (rust)`, `Analyze (go)`, `Report Build Status`.
- `rust-ci.yml`, `security.yml`, and `gitforge-ci.yml` have workflow-level PR path filters, but jobs from those workflows are included in the required-context set. A PR that changes only paths outside those filters can leave required contexts pending. GitHub documents that path-filtered required workflows can remain pending.
- Several PR workflows check out or build PR-controlled source on the persistent Fedora runner. GitHub explicitly warns that public fork PRs can compromise a persistent self-hosted runner and its reachable environment.
- During an active CodeQL/Cargo job, the C compiler used a transient assembly file under `/tmp`. On the Fedora host, `/tmp` is `tmpfs`, while `/nas/Temp` is on `/dev/sda1` (`fuseblk`). No scratch-path change was made mid-job; any `TMPDIR` relocation should be evaluated separately for memory pressure versus JBOD I/O latency.
- The attempt to create an in-workflow `runs-on` conditional is not an adequate trust boundary by itself: a fork PR can propose modifications to workflow YAML. No such placeholder workflow is retained in this branch.
- The latest trigger-fix PR (#316, head `2e5c5b9e`) has a successful `Enqueue Build to GitForge` job, but its authenticated receipt poll is still queued. End-to-end completion has not been proven.

## Required design decisions

1. **Remove persistent repository-level runners from public PR execution.** The current owner is a personal account, so organization runner-group restrictions cannot be applied here. The defensible choices are:
   - remove/disable the repository-level Fedora runners for this public repository and run PR validation on GitHub-hosted ephemeral runners; or
   - move the repository to an organization, then configure a runner group restricted to private/selected repositories and explicitly allow only trusted workflow references. Validate the exact group policy before attaching Fedora runners.
2. **Do not use `pull_request_target` to execute PR code.** It may be used only for trusted-base operations that treat PR data as data; do not checkout and execute the fork head, build its artifacts, or restore/write untrusted caches in that privileged context.
3. **Keep actual validation.** PR unit, lint, security, and integration gates must still run on an isolated GitHub-hosted environment or on a separately audited, ephemeral, sandboxed GitForge runner with no host/internal-network/credential escape. A skipped check is not evidence that a test passed.
4. **Make required statuses unconditional at workflow level.** Remove workflow-level `paths` filters from workflows whose job contexts are required. If expensive jobs should not run for irrelevant changes, keep an always-triggered workflow and conditionally skip individual jobs after a deterministic path-detection job; skipped jobs report success, while applicable jobs must execute and pass.
5. **Treat the live ruleset as production configuration.** Version the intended check list in a runbook and verify it against GitHub’s live ruleset API after change. Do not make direct ruleset mutations as part of an unreviewed code change.

## Implementation sequence

### Phase 0 — prevent new exposure

1. Schedule a short runner maintenance window. Do not kill the active runner job mid-build.
2. Disable/remove the repository-level Fedora runner association for this public repo before merging workflow changes that assume an isolated boundary. This is an external GitHub setting change; the code branch cannot enforce it against a malicious fork that edits the workflow.
3. Confirm pending jobs fail closed or wait without reaching the Fedora host. Record runner IDs and final status; do not delete runner workspaces as part of this change.

### Phase 1 — restore PR validation on isolated workers

1. Route every `pull_request` source-code job to GitHub-hosted `ubuntu-latest` runners; install native dependencies explicitly where needed.
2. Keep GitForge-specific dispatch only if the GitForge execution path is shown to isolate untrusted code. Otherwise, validate PR code on GitHub-hosted workers and reserve the Fedora/GitForge runner for trusted default-branch pushes or maintainer-triggered operations.
3. Remove workflow-level path filters for workflows that contribute required contexts. If path-aware jobs are used, ensure the workflow itself always runs and the decision job emits a stable result for every PR.
4. Audit all `pull_request` workflows, not only Rust CI: `always.yml`, `integration.yml`, `codeql.yml`, `security.yml`, `gitforge-ci.yml`, `ai-review.yml`, `auto-merge.yml`, and any new workflow. Check checkout refs, permissions, secrets, caches, artifacts, and runner labels.

### Phase 2 — reconcile branch rules

1. Define a small, explicit list of stable required check names and their authoritative producing jobs.
2. Ensure each required context is emitted for docs-only changes, Rust changes, and fork PRs. Do not require path-filtered workflow contexts.
3. Preserve strict freshness and zero bypass actors. Keep auto-merge disabled until required checks pass on the current head and mergeability is `CLEAN`/approved according to the repository policy.
4. Store the intended ruleset configuration and a drift-check command in repository docs; compare the live ruleset after rollout.

### Phase 3 — qualify and promote

Run three controlled PR validations, with no Fedora persistent runner eligible:

| Scenario | Required evidence |
|---|---|
| Docs-only PR | Every required context exists and terminates successfully; no pending path-filtered checks. |
| Rust-code PR | Format, Clippy, unit/serialized tests, build, coverage, CodeQL, security, supply-chain, and applicable GitForge validation actually execute and pass. |
| Fork PR | Job runner assignment is GitHub-hosted/ephemeral or a proven isolated sandbox; no repository secrets or Fedora/internal access; required checks complete. |

Only after all three pass should the Fedora runner be reattached, and only to a trusted, explicitly restricted workflow boundary. If the account remains personal and repository-level runners cannot be isolated sufficiently, keep them detached from this public repository.

## Acceptance criteria

- Fork PR code cannot execute on either persistent Fedora repository-level runner.
- No PR can merge because a required workflow was silently skipped by path filters.
- Applicable tests and security checks execute for real; skipped jobs are only used when the path detector proves they do not apply.
- GitForge’s PR build path has a verified isolation boundary and a completed authenticated trigger → receipt → pipeline success trace.
- The ruleset is strict, has no bypass actors, requires only stable always-emitted contexts, and matches the versioned repository policy.
- Rollback does not re-enable an unsafe persistent runner: revert code if necessary, but leave the runner detached until the trust boundary is requalified.

## External documentation

- [GitHub: secure use of self-hosted runners](https://docs.github.com/en/actions/reference/security/secure-use)
- [GitHub: managing access to self-hosted runners using groups](https://docs.github.com/en/actions/how-tos/manage-runners/self-hosted-runners/manage-access)
- [GitHub: securely using `pull_request_target`](https://docs.github.com/en/actions/reference/security/securely-using-pull_request_target)
- [GitHub: troubleshooting required status checks](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/troubleshooting-required-status-checks)
- [GitHub REST: self-hosted runner groups](https://docs.github.com/en/rest/actions/self-hosted-runner-groups)
