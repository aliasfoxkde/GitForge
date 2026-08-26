# Harness Job Contract v1

GitForge's scheduler and runner use the `harness.job.v1` lifecycle for jobs
submitted by the provider-neutral harness.

## Lifecycle

```text
queued -> assigned -> running -> succeeded|failed|cancelled
```

The scheduler assigns a runner and creates a lease token. The runner must use
that token for the following transitions:

- `POST /jobs/{id}/claim` confirms the runner assignment and returns the lease.
- `POST /jobs/{id}/started` changes the job to `running`.
- `POST /jobs/{id}/complete` records the terminal receipt.

Pending-job responses include `contract_version`, `runner_id`, and
`lease_token`. Repeated claim/start/complete calls are safe for the same
assignment, while a wrong runner or lease is rejected.

## Current boundary

The crate-level lifecycle and restart-safe database transitions are covered by
the scheduler, runner, and database tests. HTTP authentication, heartbeat
expiry/requeue, streaming log append, artifact upload, and an end-to-end
service test remain follow-up work before exposing the endpoints outside a
trusted local network. The scheduler must not be treated as a public API until
those controls are complete.
