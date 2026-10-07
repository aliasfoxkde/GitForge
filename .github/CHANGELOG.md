# Changelog

All notable GitForge changes are recorded here by release or merged change.

## Unreleased

- Align the GitHub Actions GitForge bridge with the configured CI-trigger and
  scheduler-operator credentials, and poll durable run status through the
  scheduler endpoint rather than an unconfigured API token.
- Preserve the explicitly requested active pipeline ID from manual and
  webhook triggers through the API-to-CI event path, execute that stored
  definition, and retain commit-bound configuration resolution for ordinary
  Git pushes. Retired and cross-repository selections fail closed.
- Make runner registration restart-safe by refreshing the durable identity for
  a stable `GITFORGE_RUNNER_NAME` instead of inserting a new UUID on every
  process restart. Existing stale rows remain available for audited retirement.
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
