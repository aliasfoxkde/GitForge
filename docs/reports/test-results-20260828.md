# GitForge P0 Command Wiring Retry — Test Results

**Date:** 2026-08-28
**Branch:** `feature/p0-runner-command-wiring-retry-20260828`

## Summary

All tests pass. The implementation correctly wires job metadata (image + steps) from the API layer through the scheduler to the runner.

## What Was Fixed

### Critical Gap: Steps Not Persisted
The `parse_jobs_from_config` function only extracted `name` and `image`, but **not steps**. Steps were never persisted to the `job_steps` table, so `get_pending_jobs` would return empty commands even when the scheduler had a DB pool.

**Fix:** Updated `parse_jobs_from_config` to also extract steps from the pipeline config and return a `JobSpec` struct containing `name`, `image`, and `steps: Vec<(String, String)>`. The `trigger_pipeline` handler now persists each step via `JobStepQueries::create`.

### Pipeline Config Structure Required
Jobs in the pipeline config must now have this shape:

```json
{
  "jobs": [
    {
      "name": "build",
      "image": "rust:1.75",
      "steps": [
        {"name": "compile", "run": "cargo build"},
        {"name": "test", "run": "cargo test"}
      ]
    }
  ]
}
```

Validation rules enforced:
- `name` — required, non-empty string
- `image` — required, non-empty string (container image)
- `steps` — required, non-empty array
- Each step: `name` required, `run` required and non-empty

### Inline Test Migrations Updated
`gitforge_db::connection::Pool::migrate()` now includes:
- `image TEXT NOT NULL DEFAULT ''` column in `jobs` table
- Full `job_steps` table with all required columns

This ensures the test database schema matches production.

## Test Results

### Clippy
```
cargo clippy --workspace --all-targets -- -D warnings
```
**Result:** PASS — zero warnings or errors.

### Targeted Tests
```
cargo test --package gitforge-api --package gitforge-scheduler \
           --package gitforge-db --package gitforge-runner
```
**Result:** ALL PASS

| Package | Passed | Failed |
|---------|--------|--------|
| gitforge-api | 231 | 0 |
| gitforge-db | 81 | 0 |
| gitforge-runner | 60 | 0 |
| gitforge-scheduler | 37 | 0 |

### Full Workspace Tests
```
cargo test --workspace
```
**Result:** ALL PASS — all packages, all test suites.

## Files Changed

| File | Change |
|------|--------|
| `crates/gitforge-api/src/routes/ci.rs` | Parse steps from pipeline config; persist via `JobStepQueries::create` |
| `crates/gitforge-db/src/connection.rs` | Add `image` column + `job_steps` table to inline migrations |
| `crates/gitforge-scheduler/src/assigner.rs` | Remove unused `DbJob` import |
| `crates/gitforge-scheduler/src/server.rs` | Remove unused `Pool` import |

## Fail-Closed Behavior

The execution path now fails closed on malformed metadata:

1. **Empty image** → `get_pending_jobs` **skips** the job (logs warning), runner never receives it
2. **Missing job in DB** → `get_pending_jobs` **skips** the job (logs warning)
3. **Runner receives empty image** → `execute_job` **refuses** execution, reports failure to scheduler

This ensures no job can reach sandbox execution without validated, persisted metadata.

## Pipeline: API → DB → Scheduler → Runner

```
trigger_pipeline (API)
  └─ parse_jobs_from_config → JobSpec { name, image, steps }
  └─ JobQueries::create (with image)
  └─ JobStepQueries::create (for each step)
  └─ scheduler.enqueue_persisted_job()

get_pending_jobs (Scheduler)
  └─ scheduler.get_assigned_jobs()
  └─ JobQueries::get (fetch image)
  └─ JobStepQueries::list_by_job (fetch commands)
  └─ PendingJobInfo { image, commands }
  └─ Runner polls and receives job

execute_job (Runner)
  └─ verify image non-empty (fail-closed)
  └─ build ExecutableJob from { image, commands }
  └─ executor.execute()
  └─ POST /jobs/{id}/complete
```
