# ADR-20261008: Durable CI trigger correlation

- **Status:** Accepted
- **Date:** 2026-10-08
- **Scope:** GitForge CI trigger submission and status reporting

## Context

The CI trigger endpoint publishes an event to an in-process consumer. A
successful HTTP response without durable correlation can leave callers unable
to discover whether a build was planned, especially after a timeout or service
restart. Re-submitting is not a safe status check because it can create a
second build. The service therefore needs a stable request identifier and
durable lifecycle evidence linked to the pipeline run.

The trigger-request table is consumed by the CI service but shares the
platform database with repositories, pipelines, and runs. Schema ownership
must stay in the shared database migration path rather than a second
service-specific DDL path.

## Decision

1. Store each accepted trigger request in `ci_trigger_requests`, keyed by a
   stable `trigger_id` and unique event ID. Track `pending`, `claimed`,
   `processing`, and terminal lifecycle states durably.
2. Create the table and its indexes through
   `gitforge_db::Pool::migrate`; keep request transitions and the HTTP
   contract in the CI service. Run the shared migration before the service
   binds its listener.
3. Fail closed when the durable store cannot be written or the event consumer
   is unavailable. Reserve the intended run ID before run-side writes. On
   recovery, fail a claimed request only when no run row exists; preserve
   run-backed claims and derive their status from the durable run verdict.
4. Return the stable trigger ID from submit responses and expose lifecycle
   status through a separate operator-authenticated read endpoint. Keep submit
   and read credentials distinct; never include credentials in responses.
5. Keep the no-database development mode explicit. It may serve health, but
   it cannot claim durable trigger correlation and returns the documented
   unavailable response for status reads.

## Alternatives considered

- **In-memory correlation only:** rejected because process restart loses the
  request-to-run relationship.
- **Treat submit retries as status reads:** rejected because deduplication is
  intentionally narrow and a terminal request may represent a new build.
- **Let the CI service create its own schema at startup:** rejected because it
  bypasses `gitforge-db::Pool::migrate` and duplicates schema ownership.
- **Return only a run ID:** rejected because the run may not yet exist when
  the event is accepted, and an asynchronous caller needs a stable ID before
  planning finishes.

## Consequences

- Database-backed CI startup now depends on the shared migration containing
  the trigger-request schema; migration failure prevents listener startup.
- A timed-out submit can be polled by its trigger ID without creating another
  build.
- Lifecycle SQL remains in the CI service because it is currently the only
  owner of these transitions. If another service needs to query or mutate this
  state, move those operations behind `gitforge-db` query methods rather than
  duplicating SQL.
- The wire contract and recovery edge cases require integration tests; source
  test definitions alone are not evidence that those tests pass.

## Validation record

The implementation adds migration, recovery, health-gating, and trigger-flow
tests. `cargo fmt -p ci -- --check` and `git diff --check` passed for the
initial implementation commit. Build, tests, Clippy, Aegis, and GitForge
pipeline results remain pending until the current branch gates complete.
