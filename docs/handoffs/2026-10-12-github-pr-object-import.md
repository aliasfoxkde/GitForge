# Handoff: GitHub PR trigger — merge object missing from GitForge store

**Date:** 2026-10-12
**Status:** Documentation only. Nothing in this document is implemented or
validated. No source, workflow, or test was modified while producing it; the
failure path is established by code reading, and the PR #309 incident is
operator-observed (run logs were not re-fetched in this session).
**Scope of analysis:** `AGENTS.md`, `.github/workflows/gitforge-ci.yml`
(enqueue-build block), `services/ci/src/main.rs` (`prepare_run_workspace` and
its call sites/tests), and the two `gitforge-db` items it depends on.

---

## 1. Confirmed facts (with evidence)

### 1.1 The workflow supplies `github.sha` as the build commit

`.github/workflows/gitforge-ci.yml:72-77` (job `enqueue-build`, step
"Enqueue build job") builds the trigger payload as:

```yaml
PAYLOAD=$(jq -n \
  --arg repo "$GITFORGE_REPO_ID" \
  --arg ref "${{ github.ref_name }}" \
  --arg old "${{ github.event.before }}" \
  --arg new "${{ github.sha }}" \
  '{repo_id:$repo,ref_name:$ref,old_hash:$old,new_hash:$new,working_dir:null}')
```

For a `pull_request` event, `github.sha` is GitHub's **synthetic merge
commit** — the head of the ephemeral `<n>/merge` ref GitHub computes on its
own servers. That object exists only in GitHub's object database; nothing in
this pipeline transfers it to GitForge. (Secondary observations, same lines:
for `pull_request`, `github.event.before` is empty so `old_hash` is the empty
string, and `ref_name` is `309/merge`. The ref name is only recorded on the
trigger event — checkout is by hash alone — so it is not a failure factor.)

### 1.2 `prepare_run_workspace` clones the local registered bare repo and
checks out that exact object

`services/ci/src/main.rs:1733-1831`:

1. Loads the registered repository row via
   `RepoQueries::get` (`main.rs:1739-1741`; query at
   `crates/gitforge-db/src/queries.rs:366-367`; the `git_path` column is the
   on-disk path of the registered bare repository,
   `crates/gitforge-db/src/models/repo.rs:30`). Missing registration or a
   missing/non-directory `git_path` fails immediately (`main.rs:1742-1756`).
2. If a workspace for the run already exists, it adopts it by
   `git checkout --force --detach <commit_hash>` + `git clean -fdx`
   (`main.rs:1771-1793`).
3. Otherwise it runs `git clone --no-local --no-checkout <git_path> <run
   workspace>` (`main.rs:1799-1813`) — i.e. the **source is the local
   registered bare repository**, never a remote; the CI service performs no
   network fetch.
4. It then runs `git checkout --detach <commit_hash>`
   (`main.rs:1815-1828`). If that fails (object absent from the cloned
   store), it returns `Err("checkout commit {commit_hash} failed for run
   {run_id}: …")` (`main.rs:1821-1827`).

### 1.3 An absent object fails before job execution

The only live trigger call site is the push/trigger handler at
`main.rs:2336-2348`: the durable run row is created first
(`activate_pipeline_and_create_run`, `main.rs:2318-2324`), then
`prepare_run_workspace` runs. On error it calls `fail_run` (persists
`status='failed'` with the error string and sweeps unclaimed job rows,
`main.rs:2623-2640`) and returns `TriggerFailure::terminal_after_run`.
`persist_planned_jobs` (`main.rs:2367-2388`) is reached only **after** the
workspace exists — so a failed checkout yields a run row graded `failed`
with **zero job rows**, exactly the shape `fail_run` exists to make legible
(comment at `main.rs:2339-2345`). The restart/rebuild path has the same
dependency (`main.rs:2936-2946`): a rebuild whose checkout fails skips engine
registration and the run is left for the orphan reconciler.

### 1.4 Existing tests pinning these semantics

All in `services/ci/src/main.rs` `#[cfg(test)]`:

- `test_prepare_run_workspace_clones_and_checks_out_exact_sha`
  (`main.rs:3159-3234`) — seeds a bare repo, asserts the workspace `HEAD`
  equals the requested commit.
- `test_prepare_run_workspace_adopts_existing_workspace`
  (`main.rs:3242-3313`) — adoption on restart; also asserts a workspace that
  cannot be checked out at the requested commit is an error, not a silent
  adoption (`main.rs:3306-3311`).
- `test_prepare_run_workspace_rejects_unknown_repository`
  (`main.rs:3316-3332`), `…_rejects_unavailable_repository_path`
  (`main.rs:3335-3360`), `…_rejects_non_directory_repository_path`
  (`main.rs:4858-…`), `…_rejects_invalid_commit` (`main.rs:4880-4910`) —
  fail-closed error paths, including a bogus hash.

---

## 2. Observed failure — PR #309

Operator-observed on the live GitForge instance (2026-10-12 window): the
GitHub `pull_request` event for PR #309 enqueued a run with
`new_hash = github.sha` — GitHub's merge-ref commit. That commit object does
not exist in the GitForge bare repository for the repo (the store only holds
what has been pushed into it), so:

- the run row was created and then graded `failed` by `fail_run`,
- zero jobs were dispatched,
- the persisted reason was the `checkout commit <sha> failed for run …`
  error from `main.rs:1821-1827`.

This is a structural gap, not a flake: **GitForge's CI can only test commits
that exist in its own object store, and the GitHub PR merge commit never
enters that store.** Every future `pull_request`-triggered enqueue will fail
identically while `vars.GITFORGE_ENABLED == 'true'`.

---

## 3. Options

### Option A — trusted import of the exact GitHub merge commit

Have the runner (or a new importer step) transfer the exact merge object
graph into the GitForge bare repository before/at enqueue — e.g.
`git push` the merge SHA to a staging ref such as
`refs/ci/github/<pr>/merge`.

- **Semantics:** exact. CI tests the identical commit GitHub reports, so
  commit-status reporting against `github.sha` lines up, and the tested tree
  is precisely GitHub's merge result.
- **Security/credential requirements:**
  - The import needs **git-server write authorization** (smart HTTP or SSH)
    scoped to the repo. The workflow explicitly declares only
    `GITFORGE_API_TOKEN` for scheduler/API calls (`gitforge-ci.yml:52,80,124`);
    whether the self-hosted runner has ambient SSH-agent or credential-helper
    access is not established by the workflow and must be verified before
    choosing the transport. Any newly provisioned credential belongs in a
    service-managed secret store per `AGENTS.md:32-34`, never in YAML or logs.
  - The import must be ref-scoped (staging ref namespace) so a GitHub-side
    actor cannot inject objects into `refs/heads/*` via the CI path; the
    server-side ref-policy work (e.g. the negotiated-sideband rejections
    shipped 8431f117 lineage) is the natural enforcement point.
  - Importing the object is trusting GitHub's merge computation. That is a
    policy decision: CI would validate content GitHub authored by merge,
    which is standard for cross-forge CI but should be recorded explicitly.

### Option B — build from branch head + GitForge-side merge

Enqueue with `new_hash` = the PR **head branch commit** (an object GitForge
already holds once the branch is pushed/mirror-synced), and have the
pipeline's first stage perform the merge against the base in the prepared
workspace.

- **Semantics:** different. GitForge computes the merge, so the tested SHA is
  GitForge's, not GitHub's; conflicts surface as CI-internal merge failures;
  and commit-status reporting must target the head SHA (GitHub has no status
  context on a SHA it doesn't hold — the merge SHA would no longer be the
  test subject at all).
- **Precondition that is not met today:** if the PR head branch has never
  been pushed to GitForge (mirror lag or GitHub-only fork branch), the head
  object is *also* absent and Option B fails the same way. `services/ci` has
  no remote-fetch capability (verified: no fetch/remote logic in
  `services/ci/src/main.rs`); branch arrival into the store relies on the
  operator-driven mirror reconciliation flow, not an automated per-PR
  contract.

### Recommendation

**No recommended solution is declared, because neither option is supported by
existing contracts end-to-end.** The concrete blocker:

> There is no inbound path from GitHub into the GitForge object store on the
> PR trigger path. The workflow has no explicit Git transport step or
> credential; ambient authentication available to the self-hosted runner was
> not audited. The other observed path is operator-run mirror reconciliation,
> which is not part of the trigger contract. Option A additionally needs a
> ref-policy decision to sandbox
> imported objects outside `refs/heads/*`; Option B additionally needs either
> guaranteed branch presence in the store or new remote-fetch capability in
> `services/ci`.

Short-term containment is already available without code change: leave
`vars.GITFORGE_ENABLED` gating the PR-triggered path off, or restrict the
workflow's `pull_request` trigger, until an import path exists. (Not done in
this session.)

---

## 4. Staged implementation plan (proposed, not started)

1. **Decide semantics** — exact GitHub merge (Option A) vs. GitForge merge
   (Option B). This is an owner-level policy call; everything downstream
   depends on it.
2. **Option A skeleton** — runner-side export step (`git bundle` or push of
   the merge SHA to `refs/ci/github/<n>/merge`), a service-managed git
   credential for the git-server, ref-policy allow-list for the staging
   namespace, and enqueue passing a resolvable SHA.
3. **Option B skeleton** — pre-trigger branch-presence check (fail fast with
   a distinct reason instead of the generic checkout error), pipeline merge
   stage, status reporting keyed to the head SHA.
4. **Hardening either way** — distinct error surface in the trigger response
   for "commit not in store" (today it surfaces only as the checkout failure
   string after the run row exists); keep `fail_run`'s zero-jobs audit shape.
5. **Rollout** — flag-gated on one repo; verify against the live instance's
   own pipeline before widening; then `GITFORGE_ENABLED` re-enable for PRs.

## 5. Regression tests (proposed)

- **Unit (`services/ci`):** trigger payload with a `new_hash` absent from the
  registered repo's store → run row `failed`, zero job rows, reason contains
  the checkout failure (pins §1.3; today only the direct
  `prepare_run_workspace` error is pinned at `main.rs:3306-3311` and
  `main.rs:4880-4910`, not the trigger-handler outcome).
- **Integration:** full PR-shape enqueue (empty `old_hash`, `<n>/merge` ref
  name, GitHub-only merge SHA) against a store lacking the object → explicit
  "commit not in store" error class, not a generic clone/checkout failure.
- **Option A, once implemented:** imported merge SHA resolves end-to-end
  (workspace HEAD == `github.sha`); rejected ref outside the staging
  namespace; missing credential fails closed before enqueue.
- **Option B, once implemented:** head-present/absent matrix; status reports
  target the head SHA.

## 6. Rollback

- Any code path added for import should sit behind a config flag
  (`GITFORGE_*` env, consistent with `GITFORGE_WORKSPACE_PREP_TIMEOUT_SECS`,
  `main.rs:1842-1855`): off ⇒ current fail-closed checkout behavior, which
  is itself safe (run graded failed, no jobs dispatched).
- Re-enabling the old behavior is a flag revert + service restart; no data
  migration is involved. Imported staging refs are removable without
  touching `refs/heads/*`.

## 7. Success criteria

1. A `pull_request`-triggered enqueue produces a run that dispatches ≥ 1 job
   and reaches a terminal state from real execution — no zero-jobs
   `failed`-with-checkout-error rows.
2. The tested tree equals the semantics chosen in §4 step 1 (GitHub merge
   SHA for Option A; GitForge merge result for Option B) and that identity is
   assertable from the run record.
3. The trigger response distinguishes "commit not in store" from other
   workspace failures.
4. No new credential appears in workflow YAML or logs; the git credential
   (if Option A) lives in a service-managed env file per `AGENTS.md:32-34`.
5. Regression tests from §5 are green in this repo's own GitForge pipeline
   (the authoritative gate), including the `--test-threads=2` CI invocation.

---

*End of handoff. Implementation status: **not implemented, not validated** —
no code, workflow, or test changes were made.*
