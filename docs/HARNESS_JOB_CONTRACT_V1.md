# Harness Job Contract v1

GitForge's scheduler and runner use the `harness.job.v1` lifecycle for jobs
submitted by the provider-neutral harness.

The scheduler requires the `GITFORGE_SCHEDULER_TOKEN` environment variable at
startup. Runner requests send it as a bearer token from the same environment
variable. The token is runtime configuration only and must not be committed.
Missing or invalid credentials produce a fail-closed scheduler response.

## Lifecycle

```text
queued -> assigned -> running -> succeeded|failed|cancelled
```

The scheduler assigns a runner and creates a lease token. The runner must use
that token for the following transitions:

- `POST /jobs/{id}/claim` confirms the runner assignment and returns the lease.
- `POST /jobs/{id}/started` changes the job to `running`.
- `POST /jobs/{id}/cancel` records an operator cancellation as terminal in
  the scheduler. It does not yet signal a running Docker executor, so a
  cancelled running job must not be treated as physically stopped.
- `POST /jobs/{id}/complete` records the terminal receipt.

Runner heartbeats are rejected for unknown runner IDs and are persisted to the
runner record. Stale-runner reconciliation marks the runner offline and
requeues its assigned jobs through the scheduler state machine.

Pending-job responses include `contract_version`, `runner_id`, and
`lease_token`. Repeated claim/start/complete calls are safe for the same
assignment, while a wrong runner or lease is rejected.

## Current boundary

The crate-level lifecycle, durable heartbeat/cancellation transitions, and
database writes are covered by scheduler, runner, and database tests. The
in-memory assignment/lease map is not restart-safe yet: a scheduler restart
must reconcile assigned/running database rows before accepting completions.
Running-job cancellation, streaming log append, artifact upload, granular
authorization, and an end-to-end service test remain follow-up work before
exposing the endpoints outside a trusted local network. The scheduler must
not be treated as a public API until those controls are complete.
