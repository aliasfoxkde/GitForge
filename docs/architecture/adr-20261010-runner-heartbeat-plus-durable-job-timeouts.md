# ADR: Runner Heartbeats + Durable Job Timeouts (Two Liveness Planes)

**Status:** Accepted — 2026-10-10
**Evidence:** `services/ci/src/main.rs` scheduler tick + timeout
watchdog; `gitforge-scheduler` `mark_stale_runners_offline` /
`reconcile_expired`; the 2026-10-10 false-green incident (run
`89f4b2a8`, see CAMPAIGN_PLAN_2026-10-03.md) and the runner_lost /
heartbeat-stall incident class in `docs/planning/MASTER_PLAN_2026-09-20.md`.

## Context

A CI fleet can lose at two granularities and they must not share a
detector:

- **Runner loss** — the runner process (or its host) dies. A heartbeat
  signal stops.
- **Job stall** — the runner is alive and heartbeating, but one job
  inside it never reports: the container's init is wedged, the
  workload deadlocked, or a shell child died silently. A runner-level
  signal stays green for hours while the job holds a lease and blocks
  its pipeline.

An earlier design shipped a dedicated runner-loss detection loop whose
requeue branch was hardcoded to never fire — duplicated state that
looked like defense and did nothing. The orchestrator already ran a
5-second `process_queue` tick; a second loop re-derived the same
decision from the same rows.

## Decision

Two planes, two mechanisms, one scheduler tick:

1. **Runner liveness = heartbeats, evaluated in the existing
   `process_queue` tick.** `mark_stale_runners_offline` marks
   heartbeat-lost runners offline and re-enqueues their jobs for other
   runners. No dedicated detection loop: the tick is the single
   dispatcher, and anything it can decide it should decide.

2. **Job liveness = durable timeout rows, enforced by a watchdog
   sweep.** `reconcile_expired` reaps `running` rows whose
   `started_at + timeout_secs` has elapsed, drives each live engine to
   the same terminal truth, and finalizes exactly like a reported
   completion. The durable row — not any in-memory signal — is the
   expiry authority, so a restart mid-stall cannot lose the expiry.

3. **The sweep skips, never delays.** The watchdog uses
   `tokio::time::interval` `MissedTickBehavior::Skip`, not `Delay`:
   under DB contention a delay-accumulating 60 s interval was observed
   slipping to 43-minute effective enforcement. A late timeout
   decision is strictly worse than a skipped interval — the next tick
   catches everything the skipped one would have, but a delayed one
   holds leases longer each time it slips.

## Consequences

- A healthy heartbeat is *necessary but not sufficient* for a job to
  keep its lease; the timeout row is what actually kills a stalled
  job.
- `timeout_secs` must be set on every job row at enqueue (the
  reconciler grades missing-timeout rows as non-expiring — a silent
  stall class if the column is ever dropped from the write path).
- Runner-level requeue-on-heartbeat-loss and job-level timeout reaping
  can both fire for the same job after a partition; both paths
  converge on the same terminal-grade function, so the job is reaped
  exactly once (lease generation fencing makes the second attempt a
  no-op).

## Alternatives rejected

- **Per-job application-level heartbeats** (job appends liveness
  evidence to the scheduler): heavier write path, and a wedged
  workload can freeze a userspace heartbeat as easily as a process —
  the wall-clock timeout is the only signal a truly hung job cannot
  fake.
- **Global watchdog per runner** (one timeout covers the runner's
  whole job set): one long job would mask the expiry of its siblings.
