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

## 3. Corrected diagnosis

1. **Root cause (harness/bridge, fixed here)**: the `gitforge-ci.yml`
   bridge enqueues `(repo_id, "N/merge", <GitHub merge sha>)`, but no
   mechanism delivered that commit into the instance's repository storage.
   The run was created and failed at `prepare_run_workspace` checkout
   (`unable to read tree`). Worse, `load_pipeline_from_commit` reads the
   committed `.gitforge.yml` from the same storage via `git cat-file`, and a
   missing object reads exactly like an absent file — the run would have
   executed the **default pipeline**, not the committed one.
2. **Observability gap (product, NOT fixed here)**: the trigger settled
   (`status=failed`, `run_id` recorded) at 03:21:14Z, *before* the poll
   job's first re-POST at 03:21:42Z. Settled rows are excluded from the
   `(repo_id, new_hash)` active dedup index, so the re-POST minted a second
   doomed trigger instead of surfacing the original's run id. The workflow's
   `trigger_lost` fast-fail worked exactly as PR #293 designed it; what it
   could not see is the settled row's run id — the blind spot the
   `codex/pr280-trigger-status-20261008` endpoint (unmerged at this base)
   exists to remove.

## 4. Fix

`.github/workflows/gitforge-ci.yml` gains a `Register commit in GitForge
storage` step between configuration validation and the enqueue POST:

1. Fetch `github.ref` (`refs/pull/N/merge` on pull_request events,
   `refs/heads/*` / `refs/tags/*` on push events) from the GitHub mirror —
   full history, never `--depth` (a shallow push would install a shallow
   boundary in the instance's storage repo).
2. Require the fetched head to equal `github.sha` (peel-aware for annotated
   tags), then push to `GITFORGE_GIT_URL` under the per-SHA scratch ref
   `refs/ci/github-bridge/<sha>` — content-addressed, never force-pushed,
   no branch/tag ever moves, race-free under concurrent PRs. Delivery is
   ordinary receive-pack, the same mechanism every developer push uses; the
   ref update fires a native trigger which the trigger endpoint dedups onto
   the same `(repo_id, new_hash)` tuple as the workflow's POST.
3. Deterministically verify `git ls-remote` serves the scratch ref at
   exactly the event's SHA; only then enqueue.

New configuration: `GITFORGE_GIT_URL` (variable, git HTTP URL of the same
repository) and `GITFORGE_GIT_USERNAME` / `GITFORGE_GIT_TOKEN` (secrets,
dedicated push account, validated distinct from both scheduler credentials;
presented via an inline credential helper so they never reach argv, remote
URLs, or logs). No host path is hardcoded; an unreachable repo fails the
step before any run exists, and the scheduler-side storage preflight stays
as the terminal backstop.

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
