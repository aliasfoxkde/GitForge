# CI trigger status audit follow-up receipt — 2026-10-05

## Scope

Focused follow-ups to the confirmed audit findings against commit `6f17a8a`
("fix(ci): durable trigger status correlation") on branch
`codex/gitforge-ci-status-hardening-20261005`. No live service, systemd unit,
or GitHub secret was touched; the work is committed locally only.

## Fixes applied

1. **Workflow comment correction** (`.github/workflows/gitforge-ci.yml`): the
   comment claimed the CI service's scheduler-operator token is NOT accepted
   at the trigger endpoint. That was false: `configured_trigger_token`
   (`services/ci/src/main.rs`) still accepts
   `GITFORGE_SCHEDULER_OPERATOR_TOKEN` and `GITFORGE_SCHEDULER_TOKEN` as a
   migration fallback. The comment now states the fallback precisely and that
   the operator token must be treated as trigger-capable. The status endpoint
   genuinely has no fallback (`configured_status_token`), which the comment
   says.
2. **Transient status-read faults are retryable, not terminal**
   (`services/ci/src/main.rs`): a correlated event whose
   `PipelineRunQueries::get` fails was graded through `map_run_status(None)`
   into HTTP 200 `failed` — a poller ended the job on a database fault. The
   handler now answers read errors with retryable 503 (`status_read_failed` /
   `unavailable`), while a genuinely missing run row stays terminal 200
   `failed` and an unknown event id stays 404 `missing`. The correlation-read
   503 body was aligned to `unavailable` so no retryable answer carries a
   terminal verdict word.
3. **Setup doc matched to the workflow** (`.github/GITFORGE_CI_SETUP.md`):
   removed the stale `GITFORGE_API_URL`/`GITFORGE_API_TOKEN` rows, documented
   `GITFORGE_TRIGGER_TOKEN`/`GITFORGE_STATUS_TOKEN` with the
   distinctness/UUID validation the workflow enforces, and noted the trigger
   fallback above.
4. **Event id shell-interpolation hardening**
   (`.github/workflows/gitforge-ci.yml`): `event_id` was interpolated inline
   (`EVENT_ID='${{ needs...outputs.event_id }}'`), so a crafted trigger
   response could break out of the quotes in the poll and report steps. The
   value now reaches both steps through `env:`, and both the trigger
   response's `event_id` and `pipeline_run_id` are UUID-validated before they
   are written to `$GITHUB_OUTPUT` at all. Poll-step validation is retained as
   defense in depth.
5. **Compile repair (precondition discovered during verification)**
   (`crates/gitforge-db/src/queries.rs`): branch HEAD did not compile —
   `hydrate_ci_trigger_event` used `?` inside a closure returning
   `PipelineRunId` (E0277 at the former line 1026). The optional run-id parse
   is now a `match` in the function's `Result`-returning body; behavior is
   unchanged. Consequence worth recording: no test in this workspace could
   have produced a passing result from this tree as committed in `6f17a8a`,
   including that commit's own db and e2e tests. `cargo clippy
   -D warnings` also failed at HEAD on pre-existing code (a redundant
   closure in the status-token unit test), fixed in passing.
6. **E2E fixture repair** (`services/ci/tests/ci_trigger_flow.rs`): `spawn_ci`
   created the push target with `create_dir_all` only, never `git init
   --bare`, so every service spawn failed at the seed push ("does not appear
   to be a git repository") — the e2e suite was deterministically broken at
   HEAD, independent of any code under test. The bare repository is now
   initialized before the seed push.

## Tests added

- `services/ci/src/main.rs` unit tests (handler invoked directly against a
  real file-backed SQLite pool):
  - `trigger_status_correlated_event_grades_from_the_run_row` — positive
    anchor: a succeeded run answers 200 `succeeded`.
  - `trigger_status_missing_run_row_is_terminal_failed` — genuinely missing
    run row: terminal 200 `failed`.
  - `trigger_status_correlated_row_without_run_id_is_terminal_failed` —
    corrupted row: terminal 200 `failed`, no run lookup.
  - `trigger_status_unreadable_run_is_retryable_503_not_terminal_failed` —
    forced run-read error: 503 `status_read_failed`/`unavailable`, never the
    terminal 200 contract.
- `services/ci/tests/ci_trigger_flow.rs`:
  - `test_correlated_event_with_missing_run_row_is_terminal_failed` — e2e
    through the real service: terminal 200 `failed` with the correlation
    reported, contrasted with 404 `missing` for an unknown event id.
- `services/ci/Cargo.toml`: `sqlx` added as a dev-dependency (workspace
  pinned) so tests can force database states the typed queries cannot
  express; same pattern as `crates/gitforge-scheduler` and the git-server
  integration tests.

## F2 — UNRESOLVED: pending trigger events have no crash/recovery path

Status: **open reliability issue**, deliberately not redesigned in this
follow-up.

`trigger_pipeline` (`services/ci/src/main.rs`) writes the correlation row as
`pending` before publishing the push event to the **in-process**
`InMemoryEventBus`; the consumer is what later calls `correlate` or
`mark_failed`. If the ci process dies between `insert_pending` and the
consumer's terminal write — or the consumer dies after a successful publish —
the row stays `pending` forever: the status endpoint answers `queued`
indefinitely and pollers run out their full `GITFORGE_POLL_TIMEOUT_SECONDS`.
The startup recovery path (`rebuild_live_engines` plus the reconciliation
sweep) covers pipeline runs only; nothing sweeps orphaned `ci_trigger_events`
rows. The publish-error branch does mark the row failed, so only the crash /
consumer-death case is exposed.

Consequently the commit's "correlation survives restarts" claim holds for the
correlation **row** (durable read-back), not for in-flight pending events:
restart coverage of pending events is **not** claimed and does not exist.

Candidate follow-ups (not implemented, listed for triage): a startup sweep
that fails `pending` rows older than a grace threshold, or a durable
publication path (the existing `publication_outbox` pattern) so an accepted
trigger is always either consumed or explicitly failed.

## Validation

Run on this Fedora host (worktree
`/home/mkinney/Temp/work/gitforge-ci-status-hardening-20261005`), after
confirming no concurrent `cargo`/`rustc` process was active:

- `cargo fmt --all -- --check` — clean.
- `cargo test -p ci --bin ci -- trigger_status` — 4 passed (the four new
  handler-level tests above).
- `cargo test -p ci --test ci_trigger_flow` — 6 passed (all, including the
  five that were deterministically broken at HEAD by the fixture and the one
  added here).
- `cargo test -p gitforge-db --lib -- trigger` — 5 passed (first time
  runnable: the crate did not compile at HEAD).
- `cargo clippy -p ci -p gitforge-db --all-targets -- -D warnings` — clean.
- Workflow YAML parses (`python3 -c yaml.safe_load`); every `run:` block is
  shellcheck-clean with `${{ }}` expressions substituted the way the runner
  does. `actionlint` itself is not installed on this host, so its
  expression/schema pass did not run locally.
