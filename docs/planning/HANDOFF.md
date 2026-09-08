# GitForge Handoff

**Last Updated:** 2026-09-01
**Evidence boundary (central audit):** branch `main`, HEAD `186e3eb1fdda0fbc4571002a8807c51efbb40822`, 1 dirty status entries. This boundary covers the merged first-admin bootstrap, release packaging, and source promotion receipts; numeric ratings below remain historical context, not release gates.
**Status:** 🔄 Active — merged first-admin bootstrap is built and promoted on Fedora; authenticated BigData registration and production operator provisioning remain pending
**Location:** `/nas/Temp/repos/GitForge`
**Rating:** 7.5/10 (historical context only)

> **Current execution authority:** Use `/nas/Temp/repos/Platform-Architecture/docs/planning/HANDOFF_AUDIT_2026-08-13.md` for verified cross-repository findings and `/nas/Temp/repos/Platform-Architecture/docs/planning/CODEX_CLI_EXECUTION_PACKETS_2026-08-13.md` for bounded implementation sessions. The authority filenames are explicit for provenance checks. The runner lifecycle is evidenced by a completed disposable Control Center canary; continue requiring a release receipt and post-promotion canary for future changes.

---

## Project Overview

GitForge is a self-hosted GitHub alternative providing local Git hosting, CI/CD pipelines, build runners, and artifact management. It sits at Layer 2 (Execution) of the platform architecture.

**Key value:** Full CI/CD control without third-party dependency.

---

## Architecture

```
gitforge-core/       — Core library
gitforge-runner/     — Job execution runner
gitforge-server/     — Web API server
gitforge-worker/     — Background worker
gitforge-cli/        — CLI tool
gitforge-hooks/      — Git hook integrations
```

---

## Test Status

```
cargo test --workspace  ✅ EXIT CODE 0 — substantial unit/integration coverage
```

**Finding:** The workspace has many tests, including API, CI, runner, scheduler, storage, and integration tests. These do not by themselves prove a deployed runner executes a queued job.

---

## Critical Issue: Runner Not Executing Jobs

**Production runner lifecycle issue:** `services/runner/src/main.rs` registers the agent and waits for shutdown without calling `agent.run()`. Unit tests call `run()`, but the service path must be fixed and verified with a real queued job.

This is the #1 priority fix. The runner must be invoked to actually execute CI jobs.

**Debugging approach:**
1. Check `runner.rs` — verify `run()` is called from the server
2. Check job queue — verify jobs are being dequeued
3. Check Docker sandbox — verify isolation is working

---

## Key Features

### CI/CD Pipeline
- YAML-based pipeline definitions
- Multi-step jobs with stage support
- Docker-based job isolation
- Artifact storage and retrieval

### Git Hosting
- Git repository hosting (like GitHub)
- SSH and HTTPS access
- Web UI for repository browsing

### Runner System
- Docker sandbox for job isolation
- Artifact management
- Build caching

---

## Integration Points

| Component | Integration |
|-----------|-------------|
| **Aegis** | Pre-pipeline security gate via gitforge-aegis contract |
| **Oracle** | Post-pipeline verification |
| **Control Center** | Project management + deployment triggers |
| **GitHub Actions** | Migration path / compatibility |

---

## Aegis Integration

Contract schema: `/nas/Temp/repos/Platform-Architecture/contracts/schemas/gitforge-aegis.json`

Aegis scans should run as a **pre-pipeline gate** before job execution:
```json
{
  "scan_id": "<scan-id>",
  "repo_url": "<repo-url>",
  "commit_sha": "<sha>",
  "branch": "<branch>",
  "scan_type": "incremental",
  "severity_threshold": "high",
  "categories": ["secrets", "pii", "security-hardening"]
}
```

---

## Planning Documents

| Document | Purpose |
|----------|---------|
| `AUDIT.md` | Gap audit for dark-factory template (Python, JS/TS missing) |
| `PHASE2_PLAN.md` | Phase 2 expansion plan |
| `ROADMAP.md` | Roadmap |
| `SDLC.md` | Software Development Life Cycle guide |

---

## Known Issues

1. **Runner registration fail-open** — scheduler registration failure still falls back to standalone mode
2. **Runner service authentication** — registration, heartbeat, job fetch, assignment, and completion need a service credential and ownership proof
3. **Docker sandbox** — needs verification it works for job isolation
4. **Python template missing** — dark-factory needs Python support
5. **JS/TS template missing** — web app template not yet created

---

## Next Steps

1. **P0:** Make production runner registration fail closed and add service authentication
2. **P0:** Verify Docker sandbox works for isolation
3. **P0:** Run a disposable queued-job smoke with durable completion and restart recovery
4. **P1:** Define VPS exposure policy (Tailscale/private network)
5. **P1:** Add Aegis as pre-pipeline security gate
6. **P1:** Document runner deployment runbook (`PLATFORM_SERVICE.md`)

---

## What a New Developer Needs to Know

1. **Server entry:** `gitforge-server/src/main.rs` — HTTP API
2. **Runner entry:** `gitforge-runner/src/runner.rs` — job execution
3. **Pipeline definition:** `.forge.yml` in repository root
4. **Job isolation:** Docker containers per job
5. **Artifacts:** stored in `{data_dir}/artifacts/{job_id}/`

## Platform-Architecture audit addendum (2026-08-13)

Control Center is the orchestration/audit plane; GitForge remains the Git and CI execution provider. The runner entrypoint must actually invoke the agent loop before pipeline integration can be called live. Every CI result used by Control Center must be bound to the exact workspace/commit/PR head SHA, and stale approvals or deployment records must be rejected. Aegis is the active pre-pipeline security successor to Atheon-Enhanced; Oracle remains a separate post-pipeline verification concern.

Fresh graph evidence: `services/runner/src/main.rs::main` currently registers the runner, constructs an executor, installs shutdown handling, and waits for shutdown without invoking `RunnerAgent::run()`. `RunnerAgent::register()` stores a local runner and returns success even after scheduler registration failure, labeling the process standalone. Treat the runner as non-operational until a bounded smoke submits one job, observes assignment/execution, and records a durable result. Unit tests of `RunnerAgent::run()` and result models are insufficient.

The bounded implementation authority is `GIT-W1-01` in the Platform-Architecture execution packets. It is intentionally a follow-on from the first authenticated Control Center project/task slice. The packet requires: (A) lifecycle/payload/fail-closed boundary evidence; (B) production entrypoint invocation plus success/failure result persistence; and (C) SHA/workspace integrity and a regression that catches removal of the run-loop call.

## Current Platform-Architecture evidence (2026-08-15)

The historical audit above is retained as a warning boundary. The current
working tree now has focused scheduler/API/storage slices passing: storage
receipt tests 63/63, scheduler tests 74/74, and API library tests 212/212.
The Control Center adapter has also completed a disposable authenticated
trigger → isolated workspace → two-job Docker DAG → terminal-success smoke,
including an idempotent duplicate trigger. The API exposes persisted
`JobReceipt` metadata through pipeline job responses and the logs route.

This does not close the full handoff. Runtime population of log/artifact
receipts, large-payload external storage, negative-path coverage, and the
Control Center owner-scoped artifact retrieval contract remain open. Update
this section when those gates receive reproducible evidence; do not erase the
older audit conclusions without a replacement receipt.

## Current audit reconciliation (2026-08-21, updated 2026-09-08)

The historical runner-entrypoint finding is superseded on the current branch:
`services/runner/src/main.rs` now starts `RunnerAgent::run()` in a task,
performs graceful stop, and awaits the task result. Package and integration
tests pass for the API (212 unit, 39 integration), runner (38), and scheduler
(74).

Resolution status of the previously confirmed production gaps:

1. **Runner registration fail-open — RESOLVED (2026-09-08).**
   `RunnerAgent::register()` is fail-closed by default: a scheduler connection
   failure or unexpected status aborts runner startup with an error. Legacy
   standalone fallback remains available behind an explicit
   `GITFORGE_RUNNER_STANDALONE=allow` policy. Auth rejections (401/503) were
   already fail-closed.
2. **Unauthenticated scheduler routes — RESOLVED (verified 2026-09-08).**
   `scheduler_routes_with_tokens` enforces fail-closed bearer credentials on
   every runner and operator route (unset credential returns 503
   `scheduler_auth_not_configured`). Runner and operator tokens are scoped
   independently, and completion/log/artifact routes are fenced by per-job
   lease tokens. The `/jobs/{id}/assign` no-op stub was removed: assignment is
   scheduler-owned and a client-selectable assignment route would bypass
   scheduling policy. `POST /jobs/{id}/complete` now requires lease proof for
   any existing job and returns 404 for unknown jobs; the anonymous completion
   path is gone.
3. **Compose DATABASE_URL mismatch — RESOLVED (2026-09-08, validated live).**
   api, ci, and git-server now share one `gitforge-data` volume with
   `sqlite:/data/gitforge.db?mode=rwc` (bare `sqlite:` URLs open an existing
   file only — `?mode=rwc` is what lets first boot create it), and git-server
   gained `DATABASE_URL` plus the CI trigger URL/token.
4. **Scheduler listener on `0.0.0.0` — OPEN (deployment concern).**
   Deployment must keep the listener on the private GitForge network or add an
   explicit service boundary before external exposure. `docker-compose.yml`
   now requires `GITFORGE_SCHEDULER_TOKEN` (via `${VAR:?}` interpolation) for
   the CI and runner services so the fail-closed credential cannot be silently
   absent.

### Compose-stack queued-job smoke — COMPLETE (2026-09-08)

Disposable project `gitforge-smoke` (api + ci + runner + git-server, shared
SQLite/git volumes, host ports shifted to avoid native dev processes).
Validated end to end:

- Real `git push` through the compose git-server resolved owner/repo from the
  shared database, found the API-provisioned bare repo in the shared `/git`
  volume, and enqueued a durable `ci.trigger.pending` event; the delivery
  loop posted it to CI with the shared bearer token.
- CI loaded `.gitforce.yml` committed at the pushed revision, started the
  pipeline, enqueued the job, and the runner claimed it with a lease, ran it
  in a busybox container bind-mounted from the CI checkout, and completed it
  with lease proof; the run finalized `succeeded` and the persisted log
  chunk (with the step's marker) was retrievable through the authenticated
  API.
- Cross-process durable queue: `POST /api/jobs` returned 201 `queued`, the
  CI scheduler reloaded the row on its next tick and executed it
  (`rust:latest`), and resubmitting the same idempotency key returned 200
  `already_queued` with the same job id.
- Restart recovery: a mid-flight scheduler restart requeued the two in-flight
  jobs (`requeued jobs left in-flight by scheduler restart count=2`);
  stale-lease completions were rejected fail-closed (409
  `completion_persistence_failed`); the next startup reconciliation finalized
  the orphaned run as `failed`.

Compose/image defects found and fixed by this smoke (see CHANGELOG 0.4.0
"Fixed"): split control-plane databases, missing git-server `DATABASE_URL`,
wrong git-server port mappings, missing `GIT_ROOT` on api, missing `/git`
mount on ci, root-owned volume mountpoints in all four images, missing git
binary in the ci image, root Docker socket inaccessible to the non-root
runner (`DOCKER_GID` group mapping), and workspace bind paths that must be
host-identical (`GITFORGE_WORKSPACE_HOST_DIR`).

Follow-up product gaps recorded in IMPROVEMENTS.md: orphaned-run
reconciliation runs only at startup, and post-restart requeued jobs complete
without lease proof and are marked failed despite successful execution.

---

## Validation Commands

```bash
cd /nas/Temp/repos/GitForge
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```
