# GitForge asynchronous trigger acknowledgement audit — 2026-10-15

## Observed failure

The GitForge CI GitHub Actions job posted a pipeline trigger and received HTTP
202 with `status: queued`, a durable trigger ID, and a null `pipeline_run_id`.
The workflow treated the missing run ID as enqueue failure even though GitForge
had durably accepted the work. GitForge's trigger consumer intentionally
materializes runs asynchronously and documents this response as success.

## Root cause

The client assumed enqueue was synchronous. The service already persisted a
durable trigger request and exposes `TriggerRequestQueries::get`, but the
HTTP API had no status endpoint for clients to correlate that request with the
eventual pipeline run. The workflow also treated transport acceptance as
equivalent to run creation.

## Change in this branch

- Added authenticated `GET /api/pipeline-trigger-requests/{id}`. It returns
  the durable trigger state and eventual run ID, and authorizes access against
  the trigger's repository before disclosing receipt details.
- Added the canonical `trigger_request_id` field to both new and deduplicated
  trigger responses while preserving existing response fields.
- Updated GitForge CI to accept HTTP 202 only when GitForge reports an
  accepted/queued state, then resolve the durable request before polling the
  run. For older deployed servers without the receipt endpoint, a bounded
  compatibility path resolves the latest run by exact commit and the
  repository's pipeline IDs; it never treats enqueue alone as a passing build.
- Added API tests for pending/completed states, run correlation, invalid/missing
  IDs, and repository authorization.

## Validation evidence

- Live GitHub failure log for PR #315 showed HTTP 202 and a queued event with
  no immediate run ID; GitForge health endpoints were HTTP 200.
- Codebase graph inspection confirmed the service's durable queue semantics,
  `TriggerRequestQueries::get`, and newest-first pipeline run listing.
- Local NVMe checkout verified at `/home/mkinney/worktrees/...` on
  `/dev/nvme0n1p2`, distinct from `/nas`; only CPU validation was run.
- `actionlint .github/workflows/gitforge-ci.yml`: pass.
- `cargo fmt --all -- --check`: pass.
- Focused receipt integration test: pass.
- `cargo test -p gitforge-api --test ci_routes`: 28 passed, 0 failed.
- `git diff --check`: pass.
- No Fedora runner job or GPU workload was started. A GitForge Actions run
  remains necessary to verify the remote service interaction after this branch
  is pushed; that evidence is intentionally not claimed yet.
