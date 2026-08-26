# Control Center MVP provider handoff

This document records the GitForge boundary used by the LAN-only Control
Center MVP.

## Durable CI contract

Run the CI scheduler with `GITFORGE_DATABASE_URL` set to a durable SQLite
database. The CI event consumer loads the newest valid `pipelines.config` row
for the repository and caches that `PipelineDefinition` for subsequent push
events. If no valid persisted definition exists, it uses the documented Rust
demo pipeline fallback.

The scheduler exposes:

- `POST /pipelines/trigger` — validated LAN trigger with repository, ref,
  commit, and workspace path.
- `GET /pipeline-runs/:id` — durable run, job, status, and bounded result
  receipt readback.
- `POST /jobs/:id/complete` — runner completion receipt persistence.

The runner currently defaults to `http://localhost:42781` and requires Docker.
Scheduler routes require `Authorization: Bearer $GITFORGE_SCHEDULER_TOKEN`;
the legacy `/pipelines/trigger` service route is LAN-scoped and still needs
the same explicit authentication middleware before public deployment.
For Control Center workspaces, set `GITFORGE_WORKSPACE_ROOT` and pass a
workspace below that canonical root.

## Validation status — 2026-08-15

- `cargo test -p gitforge-ci --lib`: 56 passed.
- `cargo test -p gitforge-runner --lib`: 38 passed.
- `cargo test -p gitforge-db --lib`: 63 passed.
- `cargo build --bin ci`: passed.
- `cargo build --bin runner`: passed.
- Disposable persisted-command run loaded `false`, `true` instead of the
  hard-coded `cargo fetch` pipeline.
- Disposable runner execution produced a real Docker-backed failed run and a
  bounded failure receipt; a separate run completed both `true` steps before
  its downstream job hit a Docker acquisition timeout.
- Terminal pipeline status updates now persist `finished_at`.
- Jobs publish artifacts by writing files under `<workspace>/artifacts/`.
  The runner stores them under the shared `GITFORGE_ARTIFACT_ROOT` and emits
  bounded `gitforge://artifact/<id>` URI, SHA-256, byte-count, and name metadata
  in the completion receipt. The API service mounts the same storage root.
- Control Center can proxy an artifact through
  `/pipeline/runs/:id/artifacts/:artifact_id` after validating run ownership
  and receipt membership; it reads only the UUID-named shared-storage object.
- Disposable multi-job artifact run `b0d8293f-f5d5-46d3-8c39-785df292f0dc`
  succeeded and preserved two artifact receipts through scheduler readback.
- Authenticated API endpoints now support metadata and byte download at
  `/api/artifacts/:id` and `/api/artifacts/:id/content`. The integration test
  proves missing bearer tokens are rejected and a valid token returns exact
  artifact bytes.

The runner-executed timeout/cancellation matrix now has the scheduler probe
and sandbox-destroy path, but still needs a Docker-backed service E2E test.
Authenticated Control Center API/UI artifact readback is still required before
production
promotion. GitForge API readback is proven. Intermittent Docker
acquisition/cleanup stalls remain an operational risk and must be observed
through bounded failure receipts rather than hidden.
