# GitForge queue fairness / backpressure handoff — 2026-10-11

Scope: reconcile the 2026-10-11 05:47 UTC fleet snapshot (72 queued, 44
pending, 1 running; PR #296's merge-ref run waiting on its final fuzz jobs)
against the scheduler code, name the metrics that would have explained it,
and lay out safe phased fixes. This is a handoff only: no code changes, no
tests or builds run, no live job or service mutations, nothing merged,
cancelled, or retried.

Method: fresh checkout at HEAD `c4e3816` (merge of PR #290), read-only code
audit plus read-only live observation (GET /health, GET /queue/status, and
`sqlite3 mode=ro` reads of the fleet database). Every code claim below cites
`file:line` at that HEAD. The 05:47 snapshot is treated as an observation,
not a diagnosis; nothing here assumes it proves root cause.

## 1. Observed

Live, read-only, 2026-10-11 05:59–06:02 UTC:

- API `GET /health` → 200 `{"status":"healthy","database":"connected"}`;
  scheduler `GET /health` on :42781 → `OK`.
- Job status counts (fleet DB, read-only): `queued=0, pending=16,
  running=2, assigned=1`. Against the 05:47 snapshot (72/44/1/—): the
  queued backlog drained to zero within ~13 minutes of the snapshot.
- GitHub PR #296 (draft, MERGEABLE, mergeStateStatus CLEAN): its
  "Poll GitForge Build Status" check completed 05:50:35Z and "Report Build
  Status" 05:51:45Z — its final fuzz jobs cleared ~4 minutes after the
  snapshot. The fleet drained; it did not wedge.
- Runners: exactly 2 online — `fedora-docker` (capacity 5, heartbeat
  06:01:55Z) and `t5500-docker` (capacity 2, heartbeat 06:01:38Z). All
  other runner rows are `retired`/`offline`. Aggregate online capacity = 7
  slots.
- Two long-stalled pipeline runs in `running` state:
  - `2d921e50…` created 2026-10-10 10:44Z: 6 jobs all `pending`
    (typecheck, test, build, docs, smells, aegis), zero dispatch for
    ~19.3 h.
  - `3c883f2d…` created 2026-10-10 21:14Z: job `fmt` durably `assigned`
    with `lease_generation=1`, `retry_count=0`, `heartbeat_at` EMPTY,
    `started_at` NULL, for ~8.8 h; its 5 siblings (`clippy, test, frontend,
    coverage, e2e`) are `pending` behind it.
- The CI process (pid 1026279, release `gitforge-4d72efc2-20261010`) has
  been up since 2026-10-10 12:55:04Z. The `fmt` assignment at 21:14Z
  happened in this same process — no restart between assignment and
  observation.
- `GET /queue/status` (no operator token) returns
  `{"error":"scheduler_auth_required"}`: the queue-metrics surface is not
  readable without operator credentials.
- A stale database at `/var/lib/gitforge/gitforge.db` (last job activity
  2026-09-12) still exists beside the live fleet DB at
  `/nas/Temp/repos/GitForge/data/gitforge.db`. Querying the wrong path
  shows a dead fleet. Ops hazard, noted for the runbook.

Code, at HEAD `c4e3816`:

- Fair dispatch order: priority → repo with fewer *running* jobs → FIFO →
  insertion order (`crates/gitforge-scheduler/src/queue.rs:163-182`,
  `peek_fair` at `queue.rs:149-158`, report exposure `fair_order` at
  `queue.rs:190-194`).
- The F27 fix is live: dispatch walks past blocked heads using a per-tick
  skip set (`crates/gitforge-scheduler/src/assigner.rs:1265` and
  `:1366`), so one blocked job no longer stalls the batch.
- Resource class: any job whose commands contain
  `cargo … test … --workspace` gets class `workspace-cargo-test`
  (`assigner.rs:198-209`); a runner with a class job in flight has its
  capacity zeroed for further class jobs (`assigner.rs:211-227`, applied
  at `assigner.rs:1300-1304`). One class job per runner, fleet-wide two
  runners → at most 2 concurrent class jobs.
- Fairness only orders; nothing caps per-repo concurrency (`peek_fair`
  picks the next job; no admission limit exists anywhere in the dispatch
  loop `assigner.rs:1267-1369`).
- `durable_pending` in `/queue/status` is literally
  `list_dispatchable().len()` (`assigner.rs:1530`), and
  `list_dispatchable` is `WHERE status = 'queued'` only
  (`crates/gitforge-db/src/queries.rs:1915-1928`). Rows in `pending`
  (planned, dependency-gated) are invisible to it.
- Durable timeout watchdog: `reconcile_expired` reaps only
  `WHERE status = 'running' AND started_at IS NOT NULL AND
  datetime(started_at, '+timeout_secs seconds') <= now`
  (`queries.rs:2028-2038`).
- Runner-loss fence keys on `job.status == "running"`
  (`assigner.rs:965`), gated on the job's own liveness proof
  `heartbeat_at.or(started_at)` (`assigner.rs:966`).
- Restart re-adoption reads running rows only (`list_running`,
  `queries.rs:1930-1935`).
- Runner claim protocol: a runner polls `GET /jobs/pending?runner_id=…`,
  which drives `process_queue()` and hands back assigned jobs with lease
  tokens (`crates/gitforge-scheduler/src/server.rs:500-527`).
- Runner selection: `SimplePolicy` filters online + capacity > 0 and takes
  max remaining capacity (`crates/gitforge-scheduler/src/policy.rs:29-48`);
  dispatch decrements advertised capacity by the in-memory in-flight count
  (`assigner.rs:1292-1299`).

## 2. Inferred (labeled as inference)

- The 05:47 shape — 72 queued with only 1 running and 6 of 7 slots idle —
  is not explainable by runner-full saturation. `queued` means
  dispatchable-but-unassigned. The most consistent mechanism is class
  serialization: if the running job and most of the queued work were
  `workspace-cargo-test` class (this repo's `test` and `coverage` jobs
  are), then `assigner.rs:1300-1304` zeroed every class-bearing runner's
  capacity except for the one in-flight class job per runner. That is
  F27's mechanism, but benign under F27's fix: with ALL queued work in one
  blocked class, the skip set has nothing classless to walk to, so the
  queue legitimately waits. Inference, not proof — the 05:47 job-level
  detail was not captured (see §3).
- The backlog was a transient burst, not a defect: it drained in ~13
  minutes with no operator action, and PR #296 went green at 05:51:45Z.
- The `fmt` row in `3c883f2d` is a distinct, ongoing defect: an
  assigned-never-started job on an *online* runner sits outside every
  recovery plane —
  1. `reconcile_expired` requires `status='running' AND started_at IS NOT
     NULL` (`queries.rs:2030`) — doubly excluded;
  2. the runner-loss fence requires `status=="running"`
     (`assigner.rs:965`) and a stale runner besides — t5500-docker
     heartbeats online;
  3. restart re-adoption mirrors `running` only (`queries.rs:1930-1935`)
     — and no restart occurred anyway.
  Its runner keeps polling `/jobs/pending` and is re-offered the same job
  with the same lease (`server.rs:519` `ensure_job_lease`), never starts
  it, and nothing escalates. Because CI is the same process that assigned
  it, the in-memory mirror holds it — consuming 1 of t5500-docker's 2
  slots for ~8.8 h (~44% of that runner's capacity) and blocking its 5 DAG
  siblings in `pending` indefinitely. This is the custody-stall shape the
  2026-10-10 L1 handoff's Phase 1 detector targets, with a concrete live
  instance and a named code gap (neither liveness plane in
  `docs/architecture/adr-20261010-runner-heartbeat-plus-durable-job-timeouts.md`
  covers assigned-never-started).
- Run `2d921e50` (19.3 h, 6 pending, zero dispatch) is the same stall
  family but with no assigned head row — its gating job state could not be
  identified before the fleet DB became write-locked (§3).

## 3. Unknown

- What the 05:47 "1 running" job was, and how many of the 72 queued were
  `workspace-cargo-test` class. The DB is write-locked to readers as of
  ~06:04 UTC (a bounded `.timeout 3000` read of the burst window timed
  out), and no per-minute snapshot was captured at 05:47. This is the
  single most valuable datum a future capture should preserve (§5, Phase A).
- Why t5500-docker's runner never starts the assigned `fmt` job (image
  pull failure, executor wedge, silent filter) — runner-side logs not
  inspected in this pass; no runner service was touched.
- Why run `2d921e50`'s DAG gate never opened. Its gating jobs are not in
  the DB's in-flight set; the trigger-side state (PR #293/#296 chain,
  F39's second-run minting) is a candidate but unverified.
- Whether `/queue/status`'s `durable_queued_not_in_memory` was nonzero at
  05:47 (it is the field that would have exposed scheduler/DB divergence);
  it requires the operator token to read live and nothing logged it.
- Whether any of the 16 current `pending` rows are orphaned versus
  legitimately dependency-gated.

## 4. Prior art and novelty determination

This handoff was written only after establishing what already exists; it
deliberately does not re-derive any of it:

- Historic slot-hoarding deadlock: F27
  (`docs/planning/MASTER_PLAN_2026-09-20.md:646-658`), fixed in v0.6.8 via
  the per-tick skip set (PR #228, tip `1a045af`).
- Cross-repo starvation fix: PR #209 (`fa86aa0`), rationale now at
  `queue.rs:142-148`, recorded in `docs/CHANGELOG_RECENT.md:465-466`.
- Lease-write churn (F23), durable enqueue (F21), stranded non-terminal
  runs (F31), custody deadlock of duelling finalizers (F37): all ledgered
  with verdicts in the F-series.
- Two-liveness-plane decision (heartbeats + durable timeout rows):
  `docs/architecture/adr-20261010-runner-heartbeat-plus-durable-job-timeouts.md`.
- Lease token / generation fencing contract:
  `docs/HARNESS_JOB_CONTRACT_V1.md`.
- Queue reliability lane, 2026-10-10: L1 handoff
  `GITFORGE_RUNNER_QUEUE_RELIABILITY_AND_TEMP_ISOLATION_2026-10-10.md`
  (platform-handoffs worktree; Phase 1 = custody-stall detector + queue
  depth/oldest-queued-age surface) and PR #292's phase-0 receipt (open;
  its "Ranked next implementation task #2" is the queue-visibility
  surface).
- Open items that own adjacent ground: #215 (per-repo concurrent-run cap —
  proposed options, no PR), #263 (`durable_pending` counts only `queued`),
  #277 (admission control on disk/I-O headroom; PR #287 explicitly does
  not close it), #284 (lane dedupe + bounded runner concurrency), PR #286
  (stale durable lease re-offer, open).

Novel here (and the reason this document exists):

1. The 05:47→06:00 drain reconciliation — no prior artifact records this
   incident or shows the fleet drains rather than wedges.
2. The assigned-never-started custody gap on an online runner, with a
   triple recovery-plane miss cited at code level and a live 8.8 h
   instance holding real slot capacity. PR #286 covers *stale* leases;
   the ADR's durable timeouts cover *running* jobs; this state is covered
   by neither.
3. Metric semantics during saturation: at 05:47, `durable_pending` would
   have read 72 while ignoring the 44 `pending` rows, and nothing exposes
   oldest-queued-age or per-runner slot utilization. Issue #263 names the
   misnomer; this is its first incident reconciliation.

If any of these turns out to be covered elsewhere, this handoff should be
closed in favor of that artifact.

## 5. Metrics: what exists, what is missing

Exists today (`/queue/status`, contract `scheduler.queue.v2`,
`assigner.rs:1476-1604`): per-job `waited_secs` for the head 64 in-memory
queued jobs, `per_repo_running`, `recent_dispatch` (last 20 queued→started
latencies), `queued_total`, `in_memory_queued`, `assigned_jobs`,
`online_runners`, `durable_queued_not_in_memory`, `next_up` +
`next_up_reason`, `durable_pending` (== queued-row count only).

Missing — each maps to a question the 05:47 snapshot posed and nothing
could answer:

| Missing metric | Question it answers | Owner today |
|---|---|---|
| Oldest-queued-age aggregate (fleet + per repo/runner) | "How long has the worst wait been?" | named in L1 Phase 1 and PR #292 task #2, not landed |
| `pending`-row count (planned, dependency-gated) | "Why does the DB say 44 more rows exist?" | issue #263 |
| Per-runner capacity vs in-flight (incl. class-zeroed capacity) | "Why are 6 of 7 slots idle with 72 queued?" | nothing |
| Class-blocked count (`workspace-cargo-test` skipped per tick) | "How much queued work is class-serialized right now?" | nothing |
| Assigned-never-started age | "Which assignments are custodied but never started?" | nothing (this gap, §2) |
| Dispatch-tick no-op reason counters | "Why didn't the scheduler dispatch this tick?" | nothing |

Backpressure / admission: none exists. No per-repo concurrent-run cap
(issue #215), no enqueue-time deferral with a recorded reason, no
queue-depth admission bound. `docs/project_notes/ARCHITECTURE.md:475-486`
and `docs/project_notes/PLAN_ORDER.md:548-549` remain aspirational.

## 6. Root-cause hypotheses (ranked; none proven)

H1 (most consistent with evidence) — Class serialization of the burst.
The queued fleet was dominated by `workspace-cargo-test`-class jobs; with
≤2 class jobs runnable fleet-wide (one per runner) and one already
running, 6 idle slots could not admit class work. Explains 72 queued + 1
running + idle slots. Falsified if the queued jobs at 05:47 were mostly
classless. Needs the Phase A capture to settle.

H2 — Enqueue burst from the PR chain. The #293/#296 stack plus nightly
fuzz churn minted many runs at once (run `234b05ff` 05:10, run
`b8dfb2d7` 05:53). Contributes volume; cannot alone explain idle slots
with queued work (dispatch would drain classless work into free slots
within a tick). A contributor, not the mechanism.

H3 — Custody stalls compound backlog. The two stalled runs (19.3 h and
8.8 h) block their own DAGs indefinitely and — for the assigned row —
hold a real slot. Not the cause of the 72-queued burst, but the same
fleet's standing defect and the one that does not self-heal.

H4 (weakest, listed for completeness) — Scheduler/DB divergence
(`durable_queued_not_in_memory > 0` at 05:47). CI uptime since 12:55Z
argues against a restart-gap; no evidence for it; unmeasurable
retroactively without the operator token.

## 7. Safe phased fixes (code-side only; no live mutations proposed)

Phase A — Metrics (smallest risk, highest diagnostic value; unblocks H1).
Extend `queue_status` (`assigner.rs:1476-1604`) with: fleet + per-repo
oldest-queued age; `pending`-row count from a new
`list_planned(pool)`-style query (`WHERE status = 'pending'`), renaming or
re-documenting `durable_pending` per issue #263; per-runner
`advertised_capacity / in_flight / class_blocked`; an
`assigned_never_started` list (rows durably `assigned` with NULL
`started_at` older than a threshold). Pure reads; additive JSON keys under
the existing `scheduler.queue.v2` contract (bump to v3 additively).

Phase B — Custody redrive for assigned-never-started (closes the §2 gap).
Extend `reconcile_expired` (`queries.rs:2030`) with a second guarded
predicate: rows `status='assigned' AND started_at IS NULL AND
lease_generation` unchanged AND `datetime(created_at, '+assignment_grace
seconds') <= now` → transition to `timed_out` with a distinct
`reason` (e.g. `assigned_never_started_reconciled_by_watchdog`), keeping
terminal-verdict immutability and the F37 finalizer rules intact. Grace
must exceed the fence grace (`assigner.rs:966`) and the runner poll
interval; the lease-generation guard prevents reaping an assignment a
restart legitimately re-offered. This is the durable-row plane of
ADR-20261010 extended to the third liveness state — coordinate with open
PR #286 to avoid overlapping redrives.

Phase C — Admission / backpressure (implements, not re-proposes, issue
#215). Add a per-repo concurrent-run cap (config key, default generous,
e.g. `max_running_per_repo = 4`): in the dispatch loop
(`assigner.rs:1267-1369`), jobs from repos at cap join the per-tick skip
set exactly like class-blocked jobs — reusing the F27 mechanism rather
than adding a second deferral path. Optionally add a queue-depth
admission bound that defers enqueue with a recorded reason. Config-gated,
off-by-default for rollout.

Phase D — Aging bound (starvation-freedom invariant, §10). Extend
`fair_cmp` (`queue.rs:163-182`) with a terminal aging term so a job's
effective priority rises with `waited_secs`, guaranteeing bounded wait
even against a continuously higher-priority or lower-load adversary.
Keep priority dominant below the aging threshold so existing tests'
ordering holds.

## 8. Test plan (for whoever implements; nothing run in this pass)

- Phase A: unit tests asserting `pending` rows appear in the new field and
  not in `durable_pending` (pin issue #263's semantics); golden JSON for
  `oldest_queued_age` and per-runner utilization; a test that the class-
  blocked counter increments while a class job is in flight.
- Phase B: pin with named tests mirroring the F37 style —
  (i) assigned + NULL started_at + grace exceeded → `timed_out` with the
  distinct reason; (ii) same but inside grace → untouched;
  (iii) lease_generation changed after assignment → untouched;
  (iv) running job still reaped by the original predicate (no regression);
  (v) DAG siblings of a reaped assignment become dispatchable on the next
  tick.
- Phase C: per-repo cap blocks at cap and unblocks on completion; cap
  never applies when config disabled; cross-repo dispatch still proceeds
  under one repo at cap (fairness interaction).
- Phase D: a continuously-arriving higher-priority stream cannot starve a
  Normal job beyond the aging bound; FIFO tie-break preserved below the
  threshold; `fair_order` head == `peek_fair` head still holds
  (existing test `queue.rs:586-617` must keep passing).
- Integration: restart mid-assignment with the Phase B grace → after
  restart the re-offered lease (same generation) is honored, not reaped.

## 9. Rollout / canary / rollback

Per AGENTS.md and the existing release separation:

- Land each phase as its own focused branch + PR; hosted CI, Aegis, and a
  clean merge state gate promotion. Docs-only today; no build artifacts
  produced in this pass.
- Phase A is additive observability — canary by merging and watching
  `/queue/status` shape for one full burst cycle; rollback = revert the
  single commit (no durable state touched).
- Phase B changes one SQL predicate with new guards — canary on the
  Fedora runner with `assignment_grace` set high (hours), then tighten.
  Rollback: revert; already-reaped rows are terminal by design and the
  redrive is idempotent. The release switch keeps
  `gitforge-4d72efc2-20261010` as the rollback pointer until the next
  promotion cycle; verify `/health` + the four managed services after any
  switch, once.
- Phase C ships config-off; canary = enable the cap for one repo class in
  config, observe skip-set counters from Phase A, then widen. Rollback =
  disable the key (no schema change).
- Phase D is the only ordering change — canary with the aging threshold
  at an extreme value (no behavior change) before bringing it to the
  intended bound; rollback = revert commit.
- Never merge while a required job is queued, missing, or red (the
  queue-lane rule); a green transport response alone is not semantic
  validation.

## 10. Non-starvation guarantees

What is guaranteed today:

- Per-tick progress: dispatch walks past blocked heads (F27 skip set,
  `assigner.rs:1265,1366`), so a blocked job cannot idle the scheduler.
- Cross-repo bounded unfairness: among equal priorities, the repo with
  fewer running jobs dispatches first (`queue.rs:163-182`), so a backlog
  repo cannot monopolize dispatch (PR #209).
- FIFO within (priority, load) class, so same-class queues are
  first-come-first-served.

What is NOT guaranteed today (the gap this handoff asks Phase D to close):

- No bounded wait: a continuously higher-priority (or lower-load) stream
  can postpone a Normal-priority job indefinitely; the skip set prevents
  *scheduler* starvation, not *job* starvation.
- No liveness for assigned-never-started rows (§2/Phase B): without it,
  "queued behind a custody-stalled head" is unbounded in practice — the
  8.8 h instance is the existence proof.
- No admission bound, so a single repo's enqueue burst converts directly
  into everyone's queue age under class serialization (H1) — fairness
  orders the wait, backpressure would shorten it.

Target invariant to pin with tests in Phase D: any admitted job starts
within `aging_bound + one dispatch tick` of its enqueue, regardless of
adversarial priority/load, unless a custody defect exists — and Phase B
plus its watchdog makes custody defects terminal and observable rather
than silent.

## 11. Execution boundary statement

This pass performed only reads: fresh checkout at
`/home/mkinney/worktrees/gitforge-queue-fairness-handoff-20261011`,
code inspection, `gh` metadata queries, `GET /health` + one
`GET /queue/status` (401 without token), and `sqlite3` read-only queries
of the fleet DB (the last burst-window scan was abandoned when the DB
became write-locked to readers; the bounded retry used `.timeout 3000`
only). No job was cancelled, retried, reaped, or reassigned; no service
was restarted or reconfigured; no test or build was run; nothing was
merged. The stalled runs (`2d921e50`, `3c883f2d`) were observed and left
exactly as found — Phase B is the code-side remedy proposed for them.

— Prepared 2026-10-11, checkout HEAD `c4e3816` (merge of PR #290).
