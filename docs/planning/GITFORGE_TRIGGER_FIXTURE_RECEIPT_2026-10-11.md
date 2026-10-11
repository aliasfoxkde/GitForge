# GitForge trigger fixture receipt — merge-commit registration for the GitHub PR bridge (2026-10-11)

- **Branch**: `codex/gitforge-trigger-fixture-registration-20261011`
  (draft PR, stacked on `codex/gitforge-pr293-trigger-contract-20261011`)
- **Base head at diagnosis**: `6228f8680aba417b5bb7c6d8859e647348211211`
- **Scope of fix**: `.github/workflows/gitforge-ci.yml`,
  `docs/GITFORGE_CI_SETUP.md`, this receipt. No Rust sources touched; the
  PR #293/#294 trigger-contract gates are unchanged.
- **Authoritative lane**: the self-hosted GitForge instance on
  `192.168.1.202` (v0.6.16). All instance evidence below was read from that
  instance (GitHub Actions logs + read-only sqlite over ssh), not inferred.

## 1. Reported failure

GitHub run `38106742159`, PR #294, job `114377797491` "Poll GitForge Build
Status" (runner `fedora-gitforge-01`):

- Enqueue: HTTP 202, `trigger_id dc3382f1-d0db-4ef0-8a80-6b88589bccd2`,
  `pipeline_run_id` null.
- Poll re-POST at 03:21:46Z echoed a **different** trigger id with no run id
  → `terminal_status=trigger_lost`, job red, 4 s after poll start.

The tasking premise said the new preflight "rejects missing/unavailable repo
storage before creating a run". The instance record shows that is **not**
what happened:

## 2. Instance evidence (192.168.1.202, read-only queries)

`repositories` row for `6905d6ea-3c2a-4a0a-9434-edabc83837b5`
(`operator/gitforge`):

- `git_path = /home/mkinney/work/gitforge-audit-20260822/runtime-repos/6905d6ea-3c2a-4a0a-9434-edabc83837b5`
- The path exists, is a directory, and is served by the git server
  (`git ls-remote http://192.168.1.202:42782/operator/gitforge.git` lists
  `refs/heads/main` = `c71a87c2`). **The PR #293 preflight
  (`resolve_repository_git_path`) therefore passed.**

`pipeline_trigger_requests` rows around the failure (UTC):

| id | ref | new_hash | status | run_id | error |
|---|---|---|---|---|---|
| `dc3382f1` | `294/merge` | `d79419a7…` | failed | `89d2a80e-2c7b-4ad3-9c33-f1af16d4e105` | `checkout commit d79419a7… failed for run 89d2a80e…: fatal: unable to read tree (d79419a7…)` |
| `dcc5e93e` | `294/merge` | `d79419a7…` | failed | `a681bf27-309b-4080-adcf-f74a8d8b548e` | identical checkout failure (this is the poll's re-POST minting a second trigger 32 s later) |

Corroborating pattern: `295/merge` triggers failed identically at
03:41:57Z and 03:45:58Z the same hour — every PR-bridge trigger for a
GitHub merge commit dies the same way. `git ls-remote` on the instance repo
shows **zero `refs/pull/*`** and no `d79419a7` object: the commit was never
delivered.

### 2b. Second-round evidence (after the register step shipped)

Instance repos named `gitforge` — the split the first round missed:

| repo_id | owner | git_path contents |
|---|---|---|
| `6905d6ea…` | `b4ada040` (codex-audit) | stale audit seed: no `main`, no post-August branches, mtime Oct 10 19:52 CDT — untouched by any push |
| `4509402f…` | `584fccb1` (operator) | live mirror: `refs/heads/main` = `c71a87c2`, this branch at `3e6eac82`, bridge refs, objects for both |

Trigger attribution around the second failure (merge sha `1f0cc779…`,
GitHub run `38111104070`, all times UTC):

| trigger | repo_id | ref | status | run | outcome |
|---|---|---|---|---|---|
| `fc9d4875` | `6905d6ea…` | `296/merge` | failed | `0ed179f6` | checkout failed: commit absent from that repo's storage |
| `7a7eecc7` | `6905d6ea…` | `296/merge` | failed | `09294f94` | identical (poll re-POST duplicate) |
| `b24be7d8` | `4509402f…` | `refs/ci/github-bridge/1f0cc779…` | failed | `ec8b84ca` | commit IS in this repo, still `unable to read tree` — workspace clone does not fetch `refs/ci/*` |
| `db119398` | `4509402f…` | `refs/heads/codex/gitforge-trigger-fixture-registration-20261011` | completed | `00520636` | branch push, `refs/heads/*` namespace: clone + checkout succeeded, run went green into the pipeline |

## 3. Corrected diagnosis

The first fix round (register step + `GITFORGE_GIT_*` credentials) was
necessary but not sufficient: this branch's own PR bridge run (GitHub run
`38111104070`, merge sha `1f0cc779…`) reproduced the failure **with
registration verified green** — the push landed, `ls-remote` confirmed the
scratch ref, and the run still died at checkout 4 s later. The instance
record splits the failure into two stacked harness defects:

1. **Wrong repository in the trigger tuple (primary)**: the instance has
   TWO `gitforge` repos — `6905d6ea…` owned by `b4ada040` (codex-audit; a
   stale audit-seeded mirror, git_path has no `main`) and `4509402f…` owned
   by `584fccb1` (operator; the live mirror every push lands in).
   `GITFORGE_REPO_ID` was set to `6905d6ea…` while `GITFORGE_GIT_URL`
   (`…/operator/gitforge.git`) serves `4509402f…`. Every workflow POST
   therefore minted a trigger and run for the stale repo, whose storage
   never receives any push — checkout fails for ANY new commit, merge or
   not. This alone explains the PR #294/#295 failures.
2. **`refs/ci/*` is invisible to run workspaces**: the registration push
   itself fired a native trigger on the LIVE repo (`b24be7d8`, repo
   `4509402f…`, ref `refs/ci/github-bridge/1f0cc779…`). Its run `ec8b84ca`
   had the commit IN storage and still failed `unable to read tree` — the
   run workspace is materialized by a plain `git clone`, which fetches only
   `refs/heads/*` and `refs/tags/*`. The same event delivered under
   `refs/heads/codex/…` (branch push, trigger `db119398`) cloned and
   checked out fine (run `00520636`, still running at the time of writing).
   Fixed here by moving the bridge namespace to
   `refs/tags/github-bridge/<sha>`.
3. **Observability gap (product, NOT fixed here)**: the trigger settled
   (`status=failed`, `run_id` recorded) at 03:21:14Z, *before* the poll
   job's first re-POST at 03:21:42Z. Settled rows are excluded from the
   `(repo_id, new_hash)` active dedup index, so the re-POST minted a second
   doomed trigger instead of surfacing the original's run id. The workflow's
   `trigger_lost` fast-fail worked exactly as PR #293 designed it; what it
   could not see is the settled row's run id — the blind spot the
   `codex/pr280-trigger-status-20261008` endpoint (unmerged at this base)
   exists to remove.

## 4. Fix

Two changes, both harness/config:

1. **`GITFORGE_REPO_ID` repointed** to `4509402f-03c1-402a-be78-eca700e5d66b`
   (instance config, GitHub variable): the repo behind `GITFORGE_GIT_URL`,
   the one the git server actually serves and every push lands in. The
   validate step cannot cross-check a UUID against the URL offline; the
   documented failure signature for a future mismatch is: registration
   verifies the scratch ref via `ls-remote`, the run still fails
   `unable to read tree`.
2. **`.github/workflows/gitforge-ci.yml` gains a `Register commit in
   GitForge storage` step** between configuration validation and the
   enqueue POST:

   1. Fetch `github.ref` (`refs/pull/N/merge` on pull_request events,
      `refs/heads/*` / `refs/tags/*` on push events) from the GitHub
      mirror — full history, never `--depth` (a shallow push would install
      a shallow boundary in the instance's storage repo).
   2. Require the fetched head to equal `github.sha` (peel-aware for
      annotated tags), then push to `GITFORGE_GIT_URL` under the per-SHA
      tag `refs/tags/github-bridge/<sha>` — content-addressed, never
      force-pushed, no branch/tag ever moves, race-free under concurrent
      PRs, and reachable from the run-workspace clone (which fetches only
      `refs/heads/*` + `refs/tags/*`; see diagnosis #2). Delivery is
      ordinary receive-pack; the ref update fires a native trigger which
      the trigger endpoint dedups onto the same `(repo_id, new_hash)` tuple
      as the workflow's POST. A lightweight tag keeps `new_hash` = the
      commit sha, so the dedup tuples match exactly.
   3. Deterministically verify `git ls-remote` serves the scratch ref at
      exactly the event's SHA; only then enqueue.

   New configuration: `GITFORGE_GIT_URL` (variable, git HTTP URL of the
   same repository) and `GITFORGE_GIT_USERNAME` / `GITFORGE_GIT_TOKEN`
   (secrets, dedicated push account, validated distinct from both scheduler
   credentials; presented via an inline credential helper so they never
   reach argv, remote URLs, or logs). No host path is hardcoded; an
   unreachable repo fails the step before any run exists, and the
   scheduler-side storage preflight stays as the terminal backstop.

## 5. Bounded local validation (NVMe worktree)

Run in `/home/mkinney/worktrees/gitforge-trigger-fixture-registration-20261011`
(ext4, not NAS), `TMPDIR=/tmp`:

- `actionlint` (v1.7.12 at `/var/tmp/actionlint-bin/actionlint`, the
  pre-push gate for workflow edits): 0 findings on the edited workflow.
- `shellcheck -s bash -S warning` over the four shell scripts this change
  touches or sits beside (validate, register, enqueue, poll): 0 findings.
  The fifth block (`report-status`, untouched by this change) interpolates
  GitHub `${{ }}` expressions directly and is not shellcheck-clean as a
  result — GitHub expands those before bash sees them; it is pre-existing
  and out of scope.
- PyYAML structural pass (`jobs`/`steps`/`env` walk): 3 jobs, every step has
  exactly one of `uses`/`run`, step order validate → register → enqueue.
- `git diff --check` and conflict-marker grep: clean.
- No local cargo runs; no manual instance writes — the scratch-ref push and
  its native trigger fire only from the PR's own bridge run, which is the
  acceptance exercise.

## 6. Acceptance mapping

- **"same PR merge-ref integration path correlates a pipeline_run_id and
  passes the GitForge build"**: with the merge object present, the
  committed-config loader reads `.gitforge.yml` at the merge SHA, the
  workspace checkout succeeds, and the run starts (measured ~5 s on this
  instance for the clone phase) — inside the 15 s in-request correlation
  window in the normal case (`pipeline_run_id` returned by the enqueue POST
  itself); past the window, the documented dedup-echo/substitution contract
  correlates the poll to the run. Authoritative proof lands with this
  branch's own PR run.
- **"intentionally unavailable repo still fails terminal before run"**: the
  PR #293 preflight is untouched (`test_handle_push_event_fails_terminal_
  without_run_for_missing_git_path` still pins it), and the registration
  step now fails the job even earlier — the enqueue POST is never sent for
  a repo the bridge cannot reach.

## 7. Residual risks

1. **Post-settle correlation still mints one duplicate run** when the
   original trigger settles before the poll's first re-POST (slow start,
   or a red build): the re-POST cannot see the settled row, mints a fresh
   trigger, and the poll follows the substituted run. Correct terminal
   outcome either way, at the cost of a second full build. The product fix
   is the PR #280 trigger-status endpoint (or echo-on-settle dedup); out of
   scope here by tasking constraint.
2. **Scratch-ref accumulation**: `refs/ci/github-bridge/<sha>` grows one
   ref per built SHA (objects are per-PR merge commits, mostly unique). GC
   candidate for the instance, like any CI by-product; never referenced by
   runs (checkout is by SHA), so dropping old ones is safe.
3. **Secret provisioning**: `GITFORGE_GIT_USERNAME`/`GITFORGE_GIT_TOKEN`
   must exist before the workflow runs; the validate step fails fast and
   loudly when they are absent (deterministic refusal, not a silent skip).
4. **Annotated-tag pushes**: `github.sha` for annotated tags is reported
   inconsistently by GitHub; the step accepts either the tag object or its
   peel, but `prepare_run_workspace` checks out `new_hash` as a commit, so
   annotated-tag builds remain a pre-existing bridge gap (untouched here).
5. **`GITFORGE_REPO_ID` ↔ `GITFORGE_GIT_URL` invariant is unchecked**: the
   validate step cannot resolve which repository a git URL serves without
   an authenticated repository read the workflow deliberately does not
   hold. A future mismatch reproduces the exact failure this receipt
   documents (registration green, run dies `unable to read tree`); the
   signature is documented in `docs/GITFORGE_CI_SETUP.md` as the
   diagnostic.
6. **Bridge-tag visibility**: `refs/tags/github-bridge/<sha>` is fetched by
   every clone of the mirror (that is the point — run workspaces need it),
   so developer clones accumulate one namespaced tag per built SHA. Tags
   are a GC candidate on the instance like any CI by-product; they never
   move once created, so pruning old ones cannot race an in-flight build
   of the SAME sha.
