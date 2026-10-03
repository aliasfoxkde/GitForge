# Harness Job Contract v1

GitForge's scheduler and runner use the `harness.job.v1` lifecycle for jobs
submitted by the provider-neutral harness.

The scheduler accepts separate runtime credentials: `GITFORGE_RUNNER_TOKEN`
for runner lifecycle routes and `GITFORGE_SCHEDULER_OPERATOR_TOKEN` for
operator inspection, submission, and cancellation. The older
`GITFORGE_SCHEDULER_TOKEN` remains a temporary compatibility fallback for both
roles. Credentials are runtime configuration only and must not be committed.
Missing or invalid credentials produce a fail-closed response.

## Lifecycle

```text
queued -> assigned -> running -> succeeded|failed|cancelled
```

The scheduler assigns a runner and creates a lease token. The runner must use
that token for the following transitions:

- `POST /jobs/{id}/claim` confirms the runner assignment and returns the lease.
- `POST /jobs/{id}/started` changes the job to `running`.
- `POST /jobs/{id}/heartbeat` proves the execution is still driven. Sent
  periodically while the job runs, it refreshes the job's liveness proof and
  the owning runner's heartbeat. Scheduler fencing waits for THIS channel to
  go quiet for the fence grace (`GITFORGE_JOB_FENCE_GRACE_SECS`, default
  300 s) before failing a running job, so a runner whose global heartbeat
  starves under host load no longer loses healthy builds (issue #243). A
  `409` means the lease was rotated or revoked and the outcome is already
  decided.
- `POST /jobs/{id}/cancel` records an operator cancellation as terminal in
  the scheduler. A running runner polls `GET /jobs/{id}/cancelled` and
  destroys its active sandbox when the probe becomes true. The scheduler’s
  terminal state is authoritative; runner cleanup failures remain observable
  in runner logs.
- `POST /jobs/{id}/complete` records the terminal receipt.

Operators submit work with `POST /jobs` and must provide nonempty commands,
valid pipeline/repository UUIDs, and an idempotency key (maximum 128 bytes).
The durable SQLite idempotency record makes a retry return the original job
ID; reusing a key for a different request returns `409 Conflict`. A durable
scheduler database is required for this endpoint.

Runner heartbeats are rejected for unknown runner IDs and are persisted to the
runner record. Stale-runner reconciliation marks the runner offline and
requeues its assigned jobs through the scheduler state machine, except that a
running job whose own heartbeat proof is fresh inside the fence grace stays
assigned — the runner is starving, not lost (issue #243).

Pending-job responses include `contract_version`, `runner_id`, and
`lease_token`. Repeated claim/start/complete calls are safe for the same
assignment, while a wrong runner or lease is rejected.

## Current boundary

The crate-level lifecycle, durable heartbeat/cancellation transitions,
database recovery writes, cancellation probe, and runner sandbox cancellation
are covered by scheduler, runner, and database tests. A scheduler restart
requeues durable `assigned` rows and restores persisted command definitions
before scheduling. Durable `running` rows whose own liveness proof went quiet
past the fence grace are fenced as failed with a restart receipt instead of
being replayed: without a durable runner-generation lease, replay could
duplicate external side effects if the old runner is still alive. Rows that
are still reporting liveness survive the restart and the replacement
scheduler re-adopts them into its lease mirror so their runners complete
them against the original durable lease (issue #243).
The scheduler keeps an in-process lease mirror for fast checks, while the
database persists a lease token and monotonic generation. Assignment, start,
and completion use conditional updates so a competing scheduler or stale
runner cannot transition a job it no longer owns. User-facing API submission,
status, ownership, and cancellation are now implemented against durable state.
Runner output can be appended through authenticated `POST /jobs/{id}/logs`
requests containing `runner_id`, `lease_token`, and a bounded `chunk`. Chunks
are persisted in SQLite in sequence order and are exposed with the user-facing
job logs response. A runner can upload bounded artifact bytes through
`POST /jobs/{id}/artifacts` using `x-runner-id`, `x-lease-token`,
`x-artifact-name`, and an optional checksum; the scheduler writes server-owned
metadata into the shared artifact store. These endpoints remain lease-fenced
and require the runner credential. The sandbox contract now exposes bounded
`OutputSink` delivery while commands run. The current integration test covers
the scheduler/API HTTP boundary in one process; OS-process restart/recovery
testing and durable live-stream retry semantics remain follow-up work.
