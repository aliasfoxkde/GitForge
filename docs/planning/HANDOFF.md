# GitForge Handoff

**Last Updated:** 2026-08-13
**Evidence boundary (central audit):** branch `main`, HEAD `b413aee190b2d0d411d55729bb70855c386823b5`, 17 dirty status entries. Refresh this boundary before relying on any test or rating below; numeric ratings are historical context, not release gates.
**Status:** 🔄 Active — tests present, production runner lifecycle unproven
**Location:** `/nas/Temp/repos/GitForge`
**Rating:** 7.5/10

> **Current execution authority:** Use `/nas/Temp/repos/Platform-Architecture/docs/planning/HANDOFF_AUDIT_2026-08-13.md` for verified cross-repository findings and `/nas/Temp/repos/Platform-Architecture/docs/planning/CODEX_CLI_EXECUTION_PACKETS_2026-08-13.md` for bounded implementation sessions. The runner lifecycle remains a blocker until the registered agent actually enters its run loop and executes a disposable job.

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

1. **Runner not executing jobs** — `Runner::run()` not being called
2. **Docker sandbox** — needs verification it works for job isolation
3. **Python template missing** — dark-factory needs Python support
4. **JS/TS template missing** — web app template not yet created

---

## Next Steps

1. **P0:** Fix `Runner::run()` — jobs must execute
2. **P0:** Verify Docker sandbox works for isolation
3. **P1:** Define VPS exposure policy (Tailscale/private network)
4. **P1:** Add Aegis as pre-pipeline security gate
5. **P1:** Document runner deployment runbook (`PLATFORM_SERVICE.md`)

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

---

## Validation Commands

```bash
cd /nas/Temp/repos/GitForge
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```
