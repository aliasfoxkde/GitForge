# Changelog

All notable GitForge changes are recorded here by release or merged change.

## Unreleased

- Make runner registration restart-safe by refreshing the durable identity for
  a stable `GITFORGE_RUNNER_NAME` instead of inserting a new UUID on every
  process restart. Existing stale rows remain available for audited retirement.
- Reject duplicate job names and unresolved `needs` references while building
  pipeline DAGs, preventing invalid definitions from becoming runnable.
- Preserve the `gitforge-current` symlink pathname during release promotion so
  an existing valid pointer passes the safety check.
- Per-job cancellation is now race-safe and custody-aware end to end: the
  durable cancel is one conditional transaction (completion wins over a
  later cancel, repeats are idempotent), a cancelled runner-owned job keeps
  its lease until the runner acknowledges the new
  `/jobs/{id}/cancelled/ack` endpoint or the abandoned-lease reaper expires
  it, and every run finalizer defers the verdict while that custody is
  outstanding — with aligned verdict precedence across engine, scheduler,
  and restart reconciliation. The reaper grace is measured from the durable
  cancellation timestamp, not a potentially stale pre-cancel heartbeat.

## 2026-08-31

- Propagated bounded pipeline timeouts through CI configuration, database
  persistence, scheduler assignments, runner execution, and stale-job
  reconciliation.
- Added timeout cleanup and durable timeout receipts for sandbox execution.
- Added coverage diagnostics and deterministic serialized coverage execution.
- Promoted the verified timeout implementation to the Fedora GitForge runtime;
  the previous release remains available for rollback.
