# GitForge Per-Job Cancellation Lifecycle Implementation Receipt

**Task:** Operator job cancellation was a durable status flip only — both entrypoints
(API `cancel_job`, `Scheduler::cancel`) wrote `cancelled` and moved on, with no
runner notification path, no lease custody, no acknowledgement, and finalizers
that either ignored the cancelled status or graded it inconsistently.
**Date:** 2026-10-05
**Worker branch:** `codex/gitforge-per-job-cancel-lifecycle-20261005`
**Base:** origin/main `19daeea`; the branch carries three commits
(`7eaf889` → `bd23be6` → `e0db7d6`) and the fixture fixes below are still
uncommitted working-tree changes (the parent session owns the commit/push and
the Fedora GitForge lane)
**Status:** IMPLEMENTED; first lane run (`e0db7d6`) red on 5 unpersisted-runner
test fixtures — all 5 fixture fixes are applied but UNCOMMITTED, and the tests
have not been rerun (locally or on the lane); lane re-validation pending, see
Lane Validation below

---

## Findings (defects fixed, all verified in live source before editing)

| # | Defect | Where |
|---|--------|-------|
| F1 | `JobQueries::cancel` was a two-step check-then-write; a completion racing the cancel could be overwritten, and repeats could clobber a receipt | `crates/gitforge-db/src/queries.rs` |
| F2 | Cancelling an `assigned`/`running` job cleared `lease_token`/`runner_id`, so the workspace checkout was treated as free while the container was still executing — finalizers deleted the checkout out from under it | same |
| F3 | Nothing ever told the runner to stop: the cancellation watch polls the durable row, but nothing released the lease afterwards, so finalization wedged forever on the F2 columns (or, pre-fix, deleted the workspace) | scheduler/runner |
| F4 | Scheduler `finalize_pipeline_if_terminal` graded a run with only `timed_out` rows as **`succeeded`** (only `failed` was checked before `cancelled`) | `crates/gitforge-scheduler/src/assigner.rs` |
| F5 | Scheduler grade placed `cancelled` **above** `failed`; the CI engine and the service reconciler graded `failed` above `cancelled` — the same run recovered to a different verdict depending on which grader saw it first | scheduler vs `gitforge-ci` vs `services/ci` |
| F6 | `fence_actions` only fenced `Running` mirrors; an operator cancelling a **queued** job turned the durable row terminal under a `Queued` mirror with no completion event ever coming — the run wedged non-terminal, engine and workspace leaked until restart | `crates/gitforge-ci/src/engine.rs` |
| F7 | `CiEngine::cancel_job` cancelled the one job and never settled the run: a per-job cancel of the last unfinished job (or of a stage with dependents) left the mirror non-terminal forever | same |
| F8 | Orphan-run reconciliation ignored `infrastructure_failure` rows; a restart recovered such a run to `succeeded` | `services/ci/src/main.rs` |
| F9 | Scheduler `JobCompleted{success:false}` was indistinguishable between a real failure and a synthetic event; the services/ci consumer would have driven a cancelled mirror to `Failed` on an acknowledgement | scheduler ↔ services/ci |
| F10 | Cancellation lease reaping used `COALESCE(heartbeat_at, finished_at)`, so an old pre-cancel heartbeat could consume the grace period immediately and release workspace custody too early | `crates/gitforge-db/src/queries.rs` |

## Design (smallest coherent contract)

1. **One durable transition.** `JobQueries::cancel(pool, id, result_json)` is a
   single conditional UPDATE under `BEGIN IMMEDIATE`:
   `WHERE status IN ('pending','queued','assigned','running')`. First terminal
   writer wins (F24); a completion that lands first is never overwritten; a
   repeat observes `cancelled` and preserves the original receipt
   (`JobCancelOutcome::{Cancelled, AlreadyCancelled, AlreadyTerminal}`);
   unknown non-terminal statuses and DB errors fail closed.
2. **Custody, not deletion.** The same UPDATE retains `lease_token`/`runner_id`
   via SQL CASE only for rows cancelled out of `assigned`/`running` — the
   executing runner keeps the workspace checkout until it relinquishes. Queued/
   pending cancels clear the columns (nothing ever had custody).
3. **Acknowledgement = lease CAS.** `POST /jobs/{id}/cancelled/ack`
   (runner-auth layer; justified in its doc comment — completion is
   lease-gated to `assigned`/`running` and must never accept an outcome for a
   `cancelled` row, so a separate custody-relinquishment route is required).
   `JobQueries::release_cancelled_lease` is an exact-match CAS
   (job + runner + token + status='cancelled'). The runner
   (`agent.rs::acknowledge_cancellation`) presents the proof with 3 bounded
   attempts; 409 is final; past that the reaper owns the deadline.
4. **Missed acks are bounded, not wedged.** `reap_cancelled_leases(grace)`
   clears custody one fence-grace window after the cancellation timestamp
   (`finished_at`); it uses the last heartbeat only as a fallback for legacy
   rows without a finish timestamp. A stale pre-cancel heartbeat must not
   consume the cancellation grace window. The regression test now seeds an
   hour-old heartbeat before cancellation and verifies the reaper waits for
   a full grace period from `finished_at`. It runs in scheduler
   `process_queue` recovery and in the services/ci watchdog sweep, so it
   covers both restart and steady state.
5. **Custody-gated finalization everywhere.** `finalize_terminal` (DB-level,
   BEGIN IMMEDIATE liveness check preserved), `finalize_run_if_terminal`,
   `finalize_pipeline_if_terminal`, and `reconcile_orphaned_runs_filtered`
   all defer while `status='cancelled' AND lease_token IS NOT NULL`, then
   commit the verdict and doomed rows atomically. Unknown status / DB read
   errors defer (fail closed), never finalize.
6. **Aligned durable-job verdict precedence** across the engine
   `settle_if_all_finished`, scheduler `finalize_pipeline_if_terminal`,
   services/ci consumer, and orphan reconciler: a failed, timed-out, or
   infrastructure-failed job grades `failed`; otherwise any cancelled job
   grades `cancelled`; otherwise the run succeeds. The engine additionally
   gives its mirror-only `cancel_requested` flag precedence over those durable
   job outcomes. **Restart parity holds for
   per-job cancellation only:** the `cancelled` row verdict is durable, and
   every grader re-derives the same run verdict from the durable rows.
   Run-level `cancel_requested` is the documented exception — it lives only
   in the engine mirror, so a control-plane restart before finalization
   loses it and the re-derived verdict can differ: remaining jobs run to
   completion and the run grades `succeeded` where the operator intended
   `cancelled` (see Remaining Limitations 1).
7. **Convergence without stranding.** `fence_actions` now fences every
   non-terminal mirror (F6); `CiEngine::cancel_job` cascades doomed
   descendants and settles the run (F7); the services/ci consumer handles a
   `cancelled:true` completion event as custody release → engine cancel
   reconcile → finalization, never adopting assign/start for a cancelled row.
   `requeue_inflight` leaves `cancelled` rows untouched, so custody and
   verdicts survive a restart unchanged.

## Changed Files

| File | Change |
|------|--------|
| `crates/gitforge-db/src/models/job.rs` | `JobCancelOutcome` enum + `is_new_cancellation()`/`is_cancelled()` |
| `crates/gitforge-db/src/queries.rs` | CAS `cancel`; `release_cancelled_lease`; `reap_cancelled_leases`; `has_cancelled_lease`; custody-aware `finalize_terminal`; 4 new tests + seed helper |
| `crates/gitforge-scheduler/src/assigner.rs` | `SchedulerEvent::JobCompleted{cancelled}`; `Scheduler::cancel` durable+in-memory custody; `Scheduler::acknowledge_cancellation`; custody gate + precedence in `finalize_pipeline_if_terminal`; reaper in `process_queue`; `cancelled_leases` map; 3 new tests + helpers |
| `crates/gitforge-scheduler/src/server.rs` | `POST /jobs/{id}/cancelled/ack` route + handler (200/404/409/500) |
| `crates/gitforge-runner/src/agent.rs` | `acknowledge_cancellation` (3 attempts, 409-final); orphaned branch of `execute_job` acks before returning; 3 new tests |
| `crates/gitforge-ci/src/engine.rs` | `settle_if_all_finished` precedence; `fence_actions` all non-terminal mirrors; `cancel_job` cascade+settle; stale fence comment corrected; 2 new tests |
| `crates/gitforge-ci/src/state.rs` | Mirror transitions `(Queued,Succeeded)`, `(Assigned,Failed)`, `(Assigned,Succeeded)` with fence-sweep rationale |
| `services/ci/src/main.rs` | Consumer `cancelled` branch; custody-aware `finalize_run_if_terminal`; custody + infra-parity in `reconcile_orphaned_runs_filtered`; watchdog reaper; `cancel_doomed_rows` outcome handling; 2 new tests |
| `crates/gitforge-api/src/routes/ci.rs` | `cancel_job` maps `JobCancelOutcome`: 200 / 409 `job_already_terminal` / 404 / 500 |
| `docs/CHANGELOG_RECENT.md`, `.github/CHANGELOG.md` | Unreleased entries |

No schema migration was needed: custody reuses the existing
`lease_token`/`runner_id`/`heartbeat_at` columns.

## Tests Authored (15 lifecycle tests plus 6 HTTP contract tests; current diff unvalidated)

| Crate | Test | Pins |
|-------|------|------|
| gitforge-db | `test_cancel_is_conditional_idempotent_and_f24_safe` | terminal not cancellable, receipt preserved, repeat idempotent, 404 kind |
| gitforge-db | `test_cancel_running_job_keeps_lease_until_acknowledged` | lease retained, `complete_with_lease` rejected post-cancel, wrong runner/token ack refused, correct ack clears, F24 both directions |
| gitforge-db | `test_reap_cancelled_leases_expires_after_grace` | within grace → 0 rows; past grace → 1, verdict/receipt preserved |
| gitforge-db | `test_finalize_terminal_defers_while_cancelled_row_holds_lease` | `Ok(None)` deferred, run stays `running`, release → verdict + doomed rows commit, sibling preserved |
| gitforge-scheduler | `test_cancel_running_job_defers_finalization_until_acknowledgement` | real dispatch, cancel retains lease + defers run, wrong ack false, real ack → lease cleared + `JobCompleted{cancelled:true}`, duplicate ack false/no second event |
| gitforge-scheduler | `test_in_memory_cancellation_acknowledgement_matches_mirror_custody` | exact-match CAS on `cancelled_leases`, consumed on ack, event asserted |
| gitforge-scheduler | `test_finalize_pipeline_grades_failed_over_cancelled_and_defers_custody` | failed>cancelled, `timed_out`→failed regression pin (F4), all-cancelled→cancelled, custody defers → release → `cancelled`, sibling preserved |
| gitforge-runner | `test_cancellation_acknowledgement_releases_custody_on_first_accept` | 200 → true, exactly 1 request |
| gitforge-runner | `test_cancellation_acknowledgement_treats_conflict_as_final` | 409 → false, exactly 1 request |
| gitforge-runner | `test_cancellation_acknowledgement_retries_transient_failures_bounded` | 500×3 → false, exactly 3 requests |
| gitforge-ci | `test_fence_actions_converges_queued_mirror_with_cancelled_row` | queued-mirror cancel → `FenceAction::Cancel` (F6), cascade cancels dependent, run `Cancelled` (F7) |
| gitforge-ci | `test_cancel_settlement_precedence_matches_durable_graders` | green+cancelled → `Cancelled`; failed+cancelled → `Failed`; run-level cancel outranks in-flight outcomes |
| gitforge-api | `cancelling_a_leased_job_keeps_runner_custody_until_the_lease_is_released` | real POST route, 200 + lease retained + run in custody, repeat 200, wrong runner/token refused, correct ack releases |
| services/ci | `test_finalize_defers_while_cancelled_job_holds_runner_lease` | durable cancel keeps lease; engine+workspace held; doomed row not terminalized behind custody; ack → verdict+receipts+eviction+workspace release |
| services/ci | `test_reconcile_grades_infrastructure_failure_and_respects_cancel_custody` | `infrastructure_failure` → `failed` (F8); custody run spared, then `cancelled` after release (restart parity) |
| gitforge-scheduler HTTP | `test_cancellation_ack_invalid_job_id_is_bad_request` | actual router returns 400 and the invalid-ID error contract |
| gitforge-scheduler HTTP | `test_cancellation_ack_requires_runner_id` | actual router rejects missing and malformed runner IDs with 400 |
| gitforge-scheduler HTTP | `test_cancellation_ack_requires_lease_token` | actual router rejects missing lease proof with 400 |
| gitforge-scheduler HTTP | `test_cancellation_ack_unknown_job_is_not_found` | actual router returns 404 and identifies the requested job |
| gitforge-scheduler HTTP | `test_cancellation_ack_success_releases_and_consumes_custody` | 200 acknowledgement contract; replay returns 409 |
| gitforge-scheduler HTTP | `test_cancellation_ack_conflict_does_not_consume_custody` | wrong lease/runner return 409 without consuming genuine custody |

Every test asserts the durable row, run status, lease columns, engine
registry, and/or workspace custody it is named for. No existing assertion was
weakened; two pre-existing tests were updated mechanically (outcome enum at
the `JobQueries::cancel` call site; `JobCompleted{cancelled: false}` at two
event-match sites).

## Gates Run / Not Run

- `cargo fmt --all` — **ran, exit 0, stable** (`--check` clean on re-run).
- `git diff --check` — **ran, clean** (no whitespace errors).
- No Cargo build, tests, Clippy, or coverage has run on the current working
  diff. The first GitHub Actions run at `e0db7d6` did execute the original 15
  lifecycle tests but failed five on unpersisted-runner fixtures; those five
  fixture corrections and the six new HTTP contract tests remain unvalidated.
  The required Fedora GitForge lane is `fmt → clippy → test → coverage`.

## Remaining Limitations

1. **Run-level cancel intent is mirror-only** (pre-existing, documented on
   `CiEngine::cancel`): a control-plane restart before finalization loses
   `cancel_requested` and the run resumes. The durable per-job cancel path —
   this work — is the durable alternative; no new schema field was added for
   it.
2. **Ack authentication is the runner-auth layer**, not per-job proof of
   identity beyond the (runner_id, lease_token) CAS — a forged pair fails the
   CAS, so the security property is custody correctness, not request origin.
3. **Reaper granularity is the fence grace window**, so a runner that misses
   its ack delays its run's finalization by up to one window (bounded, never
   indefinite).
4. The stale comment fix in `test_fence_actions_action_and_noise_mapping`
   re-documents the old case (terminal mirror excluded); the new queued-mirror
   test pins the changed behavior next to it.

---

## Lane Validation (2026-10-05 — first lane run, commit `e0db7d6`, PR #264)

**Run:** GitHub Actions mirror, Rust CI
[`37379916852`](https://github.com/aliasfoxkde/GitForge/actions/runs/37379916852).
`Test`, `Test (Serialized)`, and `Coverage` all FAILED on the identical test
set; `Format`, `Clippy`, `Build` green. (The GitForge enqueue jobs skipped —
known lane wiring, not a code signal.)

**Result:** 5 failing tests across 3 crates, all one failure class — a fixture
`RunnerId::new()` never persisted to `runners`, tripping the `jobs.runner_id`
foreign key (SQLite code 787) at the first lease write. No custody assertion
failed anywhere: every panic is fixture setup, so the production custody
behavior under test was never reached.

| Failing test | Panic site | Failing call |
|---|---|---|
| `scheduler::test_finalize_pipeline_grades_failed_over_cancelled_and_defers_custody` | `assigner.rs:4092` | `assign_with_lease` — "failed to assign job lease" |
| `cancelling_a_leased_job_keeps_runner_custody_until_the_lease_is_released` | `crates/gitforge-api/tests/ci_routes.rs:635` | `JobQueries::create` with `runner_id = Some(RunnerId::new())` |
| `test_finalize_cancelled_run_waits_for_live_runner_then_settles` | `services/ci/src/main.rs:4107` | `sync_lease` after `engine.assign_job(RunnerId::new())` |
| `test_finalize_defers_while_cancelled_job_holds_runner_lease` | `services/ci/src/main.rs:4333` | `sync_lease`, same unpersisted-runner shape |
| `test_reconcile_grades_infrastructure_failure_and_respects_cancel_custody` | `services/ci/src/main.rs:4464` | `sync_lease`, same unpersisted-runner shape |

### Fixes applied in this update (all 5 fixtures — code only, validation pending)

All five failures share one root cause: a fixture `RunnerId::new()` never
persisted to `runners`, tripping the `jobs.runner_id` foreign key (SQLite code
787) at the first durable lease write. Each fix registers the runner through
the existing persistence helper before that write, mirroring production
(scheduler register-before-dispatch). Every custody assertion is retained
verbatim; no production behavior changed.

1. `crates/gitforge-scheduler/src/assigner.rs`
   (`test_finalize_pipeline_grades_failed_over_cancelled_and_defers_custody`):
   the runner is registered through the DB-backed scheduler before the lease
   is assigned — `scheduler.register_runner(make_runner(runner, "custody-runner",
   "online", 1))` — which persists the row via `RunnerQueries::register_or_refresh`
   and satisfies the FK for `assign_with_lease`.
2. `crates/gitforge-api/tests/ci_routes.rs`
   (`cancelling_a_leased_job_keeps_runner_custody_until_the_lease_is_released`,
   panic site `ci_routes.rs:635`): the job's runner is now seeded with the
   file's existing pattern (`Runner::new` + `RunnerQueries::create`, as the
   logs test already does) and `job.runner_id` references that persisted
   `runner.id` instead of a bare `RunnerId::new()`. `RunnerId` stays imported
   for the wrong-runner refusal assertion.
3. `services/ci/src/main.rs`
   (`test_finalize_cancelled_run_waits_for_live_runner_then_settles`, site
   `main.rs:4107`): `seed_runner(&pool, runner)` persists the runner before
   `sync_lease`.
4. `services/ci/src/main.rs`
   (`test_finalize_defers_while_cancelled_job_holds_runner_lease`, site
   `main.rs:4333`): same `seed_runner` call before `sync_lease`.
5. `services/ci/src/main.rs`
   (`test_reconcile_grades_infrastructure_failure_and_respects_cancel_custody`,
   site `main.rs:4464`): same `seed_runner` call before `sync_lease`.

The three services/ci sites share one new test helper, `seed_runner`
(`gitforge_db::models::Runner` + `RunnerQueries::create`, the same shape the
gitforge-db unit tests use), placed beside `seed_job`; the API test uses the
inline form its neighboring test already established. **None of these edits
have been executed locally per task constraints — `Test`, `Test (Serialized)`,
and `Coverage` must be re-run on the lane before this branch is called green.**
