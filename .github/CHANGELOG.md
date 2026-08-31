# Changelog

All notable GitForge changes are recorded here by release or merged change.

## Unreleased

- Reject duplicate job names and unresolved `needs` references while building
  pipeline DAGs, preventing invalid definitions from becoming runnable.
- Preserve the `gitforge-current` symlink pathname during release promotion so
  an existing valid pointer passes the safety check.

## 2026-08-31

- Propagated bounded pipeline timeouts through CI configuration, database
  persistence, scheduler assignments, runner execution, and stale-job
  reconciliation.
- Added timeout cleanup and durable timeout receipts for sandbox execution.
- Added coverage diagnostics and deterministic serialized coverage execution.
- Promoted the verified timeout implementation to the Fedora GitForge runtime;
  the previous release remains available for rollback.
