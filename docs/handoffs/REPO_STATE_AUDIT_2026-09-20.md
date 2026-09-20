# Repository State Audit — 2026-09-20

Auditor: Claude (resuming after a one-week session gap). Scope: this
checkout (`/nas/Temp/repos/GitForge` on
`codex/push-pipeline-version-retire-20260911`), the branch/worktree
landscape, and the quality gates. No other agent's worktree was
modified; `/nas/Temp/work/gitforge-http-500-audit-20260920` is an
active session (see its `docs/handoffs/GIT_HTTP_CLONE_500_AUDIT_2026-09-20.md`).

## Actions taken this session

1. **Recovered week-old uncommitted WIP** that sat in this checkout
   since ~2026-09-13, verified unique against every branch
   (`git log --all -S`), validated (clippy, 139 cli tests, 53 ci
   tests), and committed:
   - `a62ba0bf` fix(cli): stop double-prefixing the api url — four
     command groups passed `config.api_url()` into a client that adds
     `/api` itself, so repo/pipeline/runner/auth commands hit
     `/api/api/...`; plus `#[serde(default)]` on the `enabled` field
     the API list endpoint omits.
   - `bb1c021d` fix(ci): widen reconcile grace for large clones —
     120s run-age grace cancelled runs whose `git clone --no-local` of
     multi-GB repositories was still in flight; now 600s with the
     rationale documented at the constant.
2. **Committed the settled auth design** (`435ab6a5`) —
   `docs/AUTH_DESIGN.md` had sat untracked since 2026-09-10.
3. **Ignored runtime dirs** (`f856dbf1`) — `repos/` (2.1G) and
   `workspaces/` (18G) are smoke-test runtime data inside the repo
   tree; now gitignored, still on disk.
4. **cargo vet incremental pass** (`e417231e`) — re-imported all six
   peer registries, recorded the one new tool-derived trust
   (tokio-test via Darksonn, grounded in mozilla + bytecode-alliance
   imports): 87 fully audited / 354 exempted (was 86/355).
5. All commits pushed to `origin/codex/push-pipeline-version-retire-20260911`.

## Gate status (2026-09-20)

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo vet` | green (87 audited / 354 exempted) |
| `cargo test --workspace` | via the llvm-cov instrumented run, recorded in IMPROVEMENTS.md |

## Branch landscape (local: ~160)

- **30 branches are ancestry-merged into `origin/main`** — deletable
  with plain `git branch -d` (git refuses otherwise). List: all
  `feature/coverage-*` (90/91/92/93/99/more/more-2),
  `feat/add-critical-tests`, `feat/publication-runtime-wiring`,
  `docs/improved-readme`, `fix/harden-legacy-setup`,
  `fix/propagate-job-timeout-20260831`,
  `fix/requeue-clears-stale-runner`,
  `fix/restart-recovery-resource-receipts-20260913`,
  `fix/durable-workspace-cleanup-20260831`,
  `worktree-cursor-git-article-review`, and 15 `codex/*` branches from
  the August–September receipts campaigns.
- **~130 unmerged branches.** Most date from the Aug-21 swarm, the
  Aug-29/31 release sprint, and the Sep-05→16 live-ops campaigns.
  Ancestry is a weak signal here: this platform promotes revisions to
  the Fedora deployment without a GitHub merge, so a branch can be
  fully deployed yet appear unmerged. Each needs a content decision
  (`git diff` of its touched paths against main) before deletion.
- Current branch carries the CI pipeline-version/push-retire fixes and
  is fully pushed; it is 47+ commits ahead of `origin/main` (main is
  on a dependabot merge and has 61 commits this branch lacks — the
  two histories need an explicit reconciliation decision, likely a PR
  or a deliberate fast-forward campaign).

## Worktree landscape (30)

Dirty worktrees (uncommitted state at risk — resolve before cleanup):

| Worktree | Branch | State |
|---|---|---|
| `work/gitforge-http-500-audit-20260920` | `codex/git-http-500-audit-20260920` | **ACTIVE today** — untracked handoff doc; another agent's live audit |
| `work/gitforge-sha-exec-env-20260913` | `codex/sha-evidence-exec-env-20260913` | **damaged**: 3962 staged deletions (every tracked file), worktree locked — decide restore vs. abandon |
| `work/gitforge-restart-repair-20260913` | `fix/restart-recovery-resource-receipts-20260913` | 8 modified source files (db queries, runner executor, sandbox docker, scheduler assigner) — real uncommitted work |
| `repos/GitForge-artifact-retention-readback` | `codex/artifact-retention-readback-20260916` | 9 dirty |
| `work/gitforge-pipeline-persistence-impl-20260916` | `codex/fix-pipeline-persistence-20260916` | 4 dirty |
| `work/gitforge-fedora-jobs-idempotency-20260906` | `codex/fedora-jobs-idempotency-20260906` | 3 dirty |
| `work/gitforge-stale-requeue-20260909` | `fix/requeue-clears-stale-runner` | 1 dirty |

The remaining 23 worktrees are clean and mostly check out merged
branches; they are safe to `git worktree remove` once the merged
branches are deleted.

## Cleanup recommendation (awaiting operator decision)

Deletion is deliberately not performed autonomously:

1. `git branch -d` the 30 merged branches (safe; git verifies).
2. Remove the 23 clean worktrees of merged branches.
3. For the 7 dirty worktrees: commit-or-abandon per row above; the
   sha-evidence worktree likely wants `git restore .` or worktree
   removal plus branch deletion (its branch tip is pushed).
4. Reconcile `origin/main` with the promoted branch history via PR so
   "merged" becomes meaningful again.

## Runtime disk note

`repos/` (2.1G) and `workspaces/` (18G) under the repo root are smoke
artifacts now excluded by `.gitignore` (`f856dbf1`). The 18G is mostly
`dsc-*` smoke workspaces from 2026-09-14 plus uuid run dirs; reclaiming
it needs an operator-approved `rm -r` of the named directories.
