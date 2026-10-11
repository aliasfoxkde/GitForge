# PR #294 trigger correlation receipt — 2026-10-11

Branch `codex/gitforge-trigger-correlation-fix-20261011`, stacked on
PR #294 head `6228f868` (`codex/gitforge-pr293-trigger-contract-20261011`).

## Observed failure (exact-head evidence)

At PR #294 head `6228f868`, GitForge pipeline run `38106742159` for commit
`6228f868` **passed** its source, test, coverage, integration, and e2e
checks. The GitHub `gitforge-ci.yml` gate for the same commit failed:

- Enqueue job `114373763121`: HTTP `202`, `status=queued`, durable
  `trigger_id`, `event_id`, `pipeline_run_id: null` — accepted and
  correlation-pending, not a failure.
- Poll job `114377797491` (`GITFORGE_CORRELATE_TIMEOUT_SECONDS=300`): failed
  `trigger_lost` after ~4 seconds — its first correlation re-POST returned a
  fresh-minted trigger id, not the addressed one.
- Report job `114377885492`: failed faithfully on that result.

Live scheduler state on `fedora-gitforge-01` could not be inspected (SSH
refused from the orchestration host per tasking), so the mechanism below is
established from source at this head plus the run evidence.

## Root cause

The workflow's only correlation channel was the tuple dedup, and the dedup
exists only inside a request's active window:

1. `POST /pipelines/trigger` records the request durably, publishes the push
   event, and waits `CI_TRIGGER_CORRELATION_WINDOW` (15s) for the run
   (`services/ci/src/main.rs`, `trigger_pipeline`). Under load the window
   elapses and the enqueue answers `queued` — a pending request, not a
   failure.
2. The consumer marks the request `completed` **with its run id** the moment
   the run materializes (`process_trigger_event` →
   `TriggerRequestQueries::mark_completed`), typically seconds after enqueue.
3. `TriggerRequestQueries::create_or_existing` dedups only `pending`/
   `processing` rows (`crates/gitforge-db/src/queries.rs`), so the poll
   job's re-POST after settlement matched nothing, **minted a fresh trigger
   request and published a duplicate event**, and echoed the new id.
4. The poll job's fail-fast (echo ≠ original → `trigger_lost`) then reported
   a red gate for a build that had already passed — and the minted duplicate
   event produced a second run for the same commit.

The same blind spot hides terminal trigger failures (e.g. the ghost
`git path is unavailable` class fixed in `b7e0af3d`): after settlement no
endpoint exposes the recorded cause. The scheduler serves only
`GET /pipelines/runs/{id}` and `POST /pipelines/trigger`
(`crates/gitforge-scheduler/src/server.rs`); the gateway run route needs a
user JWT the workflow deliberately never holds, and the trigger-status
endpoint from `codex/pr280-trigger-status-20261008` was never merged.

## Fix

Smallest end-to-end change that keeps enqueue durable, retries idempotent,
credentials separated, and the gate fail-closed:

- **`services/ci/src/main.rs`** — `POST /pipelines/trigger` accepts an
  optional `trigger_id`. When present the call is a pure state read of that
  durable request (`trigger_request_state`): nothing is recorded, published,
  or deduped. `404` unknown id, `409` on a (repo_id, new_hash) tuple
  mismatch, `503` in non-durable mode. The durable lifecycle maps onto the
  response vocabulary clients already parse: `completed` → `accepted` + run
  id; `failed` → `failed` + recorded cause (plus the run row the failure
  produced, when one exists); `pending`/`processing` → `queued` + null run
  id. Old callers (git-server webhook path, api `CiTriggerClient`) are
  unaffected: the field is optional and serde ignores nothing new.
- **`.github/workflows/gitforge-ci.yml`** — the correlation loop addresses
  its original trigger id on every re-POST and additionally parses `status`:
  `failed` fails the gate immediately with the recorded cause
  (`terminal_status=trigger_failed`); `accepted` without a run id is a
  protocol violation and fails closed; only a fresh-mint echo (scheduler
  predating the contract, or no durable trace) still reports
  `trigger_lost`. No timeout was raised, nothing is re-POSTed beyond the
  existing loop, and a queued response is never treated as success.
- Credential separation unchanged: trigger-request state is read with the
  trigger credential; the scheduler operator token remains reserved for
  `GET /pipelines/runs/{id}`.

## Regression coverage (`services/ci/tests/ci_trigger_flow.rs`)

- `test_completed_trigger_state_read_returns_its_run` — 202 queued →
  consumer settles `completed` → addressed read returns that exact run id;
  still exactly one run (no false run), settled request stays invisible to
  the tuple dedup (no lost/duplicate trigger).
- `test_failed_trigger_state_read_names_the_cause_without_minting` —
  settled `failed` request reports `failed` + the `git path is unavailable`
  cause + null run id; mints no fresh request and no run.
- `test_pending_trigger_state_read_is_inert_and_tuple_bound` — pending
  request reads `queued` + null run id; the read publishes nothing (no run
  appears); tuple mismatch → `409 trigger_tuple_mismatch`; unknown id →
  `404 trigger_not_found`.

Existing suites (dedup race, ghost terminal failure, committed-pipeline
flow) are unchanged and still pin the old contract for unaddressed POSTs.

## Known limits

- The deployed `v0.6.16` scheduler predates the addressed read: until a
  release containing this change is promoted, the correlation loop against
  that instance still hits the fresh-mint echo and reports `trigger_lost`
  (immediately, with the cause named in the log) whenever the consumer
  settles a request before the first correlation re-POST. This branch's
  GitForge pipeline gates (source/test/coverage/integration/e2e) validate
  the code; the GitHub `Poll GitForge Build Status` gate can only go green
  on a scheduler that serves the addressed read.
- One transient artifact of the old contract may exist on the live
  instance: the fresh-minted duplicate request the failed poll job created.
  It ran the same commit through the same pipeline and required no data
  repair here.
