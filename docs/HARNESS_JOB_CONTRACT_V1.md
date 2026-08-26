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
requeues its assigned jobs through the scheduler state machine.

Pending-job responses include `contract_version`, `runner_id`, and
`lease_token`. Repeated claim/start/complete calls are safe for the same
assignment, while a wrong runner or lease is rejected.

## Current boundary

The crate-level lifecycle, durable heartbeat/cancellation transitions,
database recovery writes, cancellation probe, and runner sandbox cancellation
are covered by scheduler, runner, and database tests. A scheduler restart
requeues durable `assigned` rows and restores persisted command definitions
before scheduling. Durable `running` rows are fenced as failed with a restart
receipt instead of being replayed: without a durable runner-generation lease,
replay could duplicate external side effects if the old runner is still alive.
The lease map remains process-local and is invalidated by recovery; a future
durable lease table may safely replace this conservative failure behavior for
multi-scheduler operation. User-facing API submission, status, ownership, and
cancellation are now implemented against durable state. Streaming log append,
artifact transfer, and an end-to-end service test remain follow-up work before
exposing the endpoints outside a trusted local network.
