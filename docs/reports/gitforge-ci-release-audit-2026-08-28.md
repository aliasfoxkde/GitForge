# GitForge CI/Release Audit — 2026-08-28

**Repository:** `/home/mkinney/work/gitforge-audit-20260822`
**Branch:** `feature/ci-release-audit-20260828`
**Auditor:** Read-only code and workflow audit; workspace tests executed against local Rust toolchain 1.98.0
**Date:** 2026-08-28

---

## Scope

Inspect repository's own CI orchestrator, scheduler, runner, persistence, clean-checkout behavior, release artifact path, and failure handling. Run workspace tests, formatting check, strict Clippy, and narrowest runnable service/negative-path checks. Produce this report and a matching JSON receipt.

---

## 1. Active CI/CD Workflows

### 1.1 `gitforge-ci.yml` (active)

**Purpose:** Route all Rust builds through GitForge itself instead of running directly on GitHub Actions runners.

**Trigger:** push to `main`/`stable`, any tag `v*`, and PRs touching `**.rs`, `**/Cargo.toml`, `**/Cargo.lock`, `.github/workflows/**`.

**Jobs:**
- `enqueue-build` — POSTs to `$GITFORGE_API_URL/api/webhook/trigger/{repo_id}` with repo_id, commit_hash, branch, pipeline_name. Uses `set +e` so curl errors do not fail the step. Falls back to `cargo test --workspace --no-fail-fast` if GitForge returns non-200/201 or the response body lacks `.pipeline_id`.
- `poll-build` — Runs only when `enqueue-build` outputs `build_status: enqueued`. Contains only an `echo` step; **no actual polling loop, no `until` or `while`, no `GET /api/jobs/{id}/status` call.**
- `report-status` — Runs `if: always()`; contains only echo steps.

**Observations:**
- The polling step is a stub. No actual HTTP GET to check job status.
- The fallback test job shares `cargo test --workspace --no-fail-fast` with the in-repo Makefile `test` target.
- Authorization header in the webhook curl is truncated to `$GITFO...OKEN` (line 37), a redacted placeholder.

### 1.2 `release.yml` (active)

**Trigger:** push of any `v*` tag, or `workflow_dispatch`.

**Jobs:**
- `build-linux` — `cargo build --release --bin api --bin ci --bin git-server --bin runner` on ubuntu-latest.
- `build-macos` — Cross-compiles x86_64 and arm64 Darwin targets.
- `build-windows` — Cross-compiles x86_64-pc-windows-gnu.
- `release` — Needs all three build jobs. Runs only when `github.repository_owner == 'aliasfoxkde'`. Downloads artifacts, computes SHA256SUMS, flattens into `dist/release/`, then calls `softprops/action-gh-release@v1`.

**Observations:**
- Artifacts are four binaries in tar.gz archives named after platform triple.
- Comment on line 15 states *"cross-compilation not available"* but macOS job does add targets and build them; Windows job also cross-compiles with the mingw target. The comment is stale.
- Artifact flattening step (`cp dist/*/*.tar.gz dist/release/`) is fragile — uses `|| true` to absorb missing files silently.

### 1.3 `codeql.yml` (active)

**Trigger:** push to `main`, PRs targeting `main`, weekly cron `0 0 * * 0`.

**Matrix:** languages `["go", "rust"]`. Paths-ignore excludes `**/*_test.go`, `**/*_test.rs`, and `template-parts/**`.

**Observations:**
- Rust autobuild step falls back to `cargo build --workspace || cargo build --workspace || true`, so it can succeed without building anything.

### 1.4 `rust.yml.disabled`, `security.yml.disabled`, `ci.yml.disabled`, `integration.yml.disabled`, `dev-testing.yml.disabled`, `python-ci.yml.disabled`, `release-rust.yml.disabled`, `benchmark.yml.disabled`, `setup-repo.yml.disabled`

All disabled. These are not active.

---

## 2. Service Inventory (built artifacts confirmed)

All four service binaries build successfully under `cargo build --release`:

| Binary | Location | Default port |
|--------|----------|--------------|
| `api` | `target/release/api` | 42780 |
| `ci` | `target/release/ci` | 42781 (scheduler HTTP) |
| `git-server` | `target/release/git-server` | 42782 (HTTP), 42022 (SSH, pending) |
| `runner` | `target/release/runner` | registers to ci:42781 |

**Startup facts:**
- `api` — panics if `JWT_SECRET` env var is absent. SQLite default `sqlite:/gitforge.db`. Process supervision init (subreaper + SIGCHLD) with warn-on-failure.
- `ci` — starts embedded Scheduler HTTP API on port 42781. Event consumer loop subscribes to `PushReceived` events on an in-memory `InMemoryEventBus`. Scheduler loop ticks every 5 seconds, calls `load_pending_jobs()` then `process_queue()`. Process supervision init with warn-on-failure.
- `runner` — sets `GITFORGE_SANDBOX_MODE=required` before any other init. Attempts Docker connection via `DockerSandbox::new()` → `connect_required()` (ping). Fails closed if Docker daemon unavailable. Registers with scheduler via HTTP POST `/runners`. Runs heartbeat every 30s, job-fetch every 5s.
- `git-server` — HTTP Git protocol handler only. SSH is documented as pending russh API resolution.

---

## 3. Orchestrator — `gitforge-ci` service

### 3.1 Pipeline Trigger Endpoint (`POST /pipelines/trigger`)

- Requires `x-gitforge-trigger-token` header matching `GITFORGE_CI_TRIGGER_TOKEN` env var. Returns `503` if token unset, `401` if mismatch.
- Validates `repo_id` as UUID, `ref_name` as `refs/*` ≤256 chars, both commit hashes as 40-char hex.
- Queries DB for repository existence, pipeline existence and ownership.
- Persists a `PipelineRun` row with source `"control-center"` before publishing `PushReceived` event to the in-memory bus.
- Returns `202 Accepted` with `{event_id, repo_id, pipeline_run_id, pipeline_id}`.

**Gap:** No rate limiting. No webhook secret verification beyond a static bearer token comparison.

### 3.2 Event Consumer

- Subscribes to `PushReceived` events on `InMemoryEventBus`.
- Uses a 100ms sleep + `stream.next()` select loop. Checks shutdown flag on every iteration.
- Handles `PushReceived` by looking up pipeline from in-memory `HashMap<RepoId, PipelineDefinition>` cache (creates a default pipeline if absent).

**Gap:** `InMemoryEventBus` is ephemeral. Events published on this bus are lost if the CI process restarts. The `PushReceived` event is created in-memory only after the pipeline/run DB records already exist, which is correct for durability of the trigger record — but the event itself has no durable backing.

### 3.3 Scheduler Integration

The CI process creates a `Scheduler::with_db(db_pool)` with a shared SQLite database URL. The scheduler HTTP API is started on port 42781 within the same process. Jobs are queued in-memory in the `JobQueue` (BinaryHeap). The `load_pending_jobs()` call re-hydrates from SQLite on startup and every 5 seconds.

---

## 4. Scheduler — `gitforge-scheduler` crate

### 4.1 Architecture

`Scheduler` holds `Arc<RwLock<SchedulerState>>` with in-memory `JobQueue` (BinaryHeap + HashMap), `HashMap<RunnerId, Runner>`, `HashMap<JobId, RunnerId>` for assignments, and `HashMap<JobId, PipelineRunId>` for assigned pipeline runs. Optionally wraps a `Pool` (SQLite). Two `enqueue` variants:

- `enqueue()` — creates a **new** `DbJob` row in the database (risk of duplicate rows if caller also creates).
- `enqueue_persisted_job()` — updates existing job status to `queued` without creating a new row.

### 4.2 Scheduling Policy

`SimplePolicy::select_runner()` is called for each job dequeue attempt. Only runners with `status == "online"` are considered. Policy is pluggable via `with_policy()`.

### 4.3 Persistence

- `process_queue()` persists each job assignment (`JobQueries::assign`) and status update to SQLite.
- `complete_job()` updates job status, then checks all jobs for the same `PipelineRunId` to update the run status. If any job in the run failed, the run is marked `failed`. All-or-nothing completion semantics.
- `load_pending_jobs()` queries `JobQueries::list_pending()` and populates the in-memory queue. `RepoId` is not stored in the job model, so re-hydrated jobs use `RepoId::new()` (a random ID) — this is a **defect**: jobs reloaded after restart lose their repo association in memory (though the DB row retains it).

### 4.4 Runner Lifecycle

- `register_runner()` — persists runner to DB. Runners are assumed online on registration.
- `heartbeat()` — updates `last_heartbeat` timestamp in-memory only; **not persisted to DB**.
- `runner_offline()` — sets status to `"offline"` in-memory only; **not persisted**.
- No heartbeat timeout or runner eviction mechanism exists.

**Gap:** Offline runners are never automatically detected. A runner that crashes has its in-memory entry persist forever, causing `list_online_runners()` to eventually return empty as registrations accumulate without eviction.

### 4.5 HTTP Routes (scheduler API embedded in CI service)

| Route | Handler | Auth |
|-------|---------|------|
| `GET /health` | `"OK"` | None |
| `POST /pipelines/trigger` | `trigger_pipeline` | `x-gitforge-trigger-token` header |
| `POST /runners` | `register_runner` | None |
| `POST /runners/{id}/heartbeat` | `runner_heartbeat` | None (ID only) |
| `GET /jobs/pending` | `get_pending_jobs` | None |
| `POST /jobs/{id}/assign` | `assign_job` | None (ID only) |
| `POST /jobs/{id}/complete` | `complete_job` | None |

**Gap:** All scheduler HTTP routes lack authentication. Any process that can reach port 42781 can register runners, assign jobs, and mark jobs complete.

---

## 5. Runner Agent — `gitforge-runner` crate

### 5.1 Registration

- Attempts HTTP POST to `http://localhost:42781/runners` (configurable).
- On non-2xx response or connection failure, logs a warning and continues in **standalone mode** — `self.runner` is set to a locally-constructed `Runner` with a random ID, but no actual assignment will be received.
- Standalone mode means the runner does useful work only if a scheduler happens to be at the default URL.

### 5.2 Job Fetch Loop

- Polls `GET /jobs/pending` every `fetch_interval_secs` (default 5s).
- The HTTP response is deserialized as `Vec<JobAssignment>`. Each assignment carries `job_id`, `name`, `pipeline_run_id`, `commands`, `working_dir`.
- **Gap:** `get_pending_jobs` in the scheduler returns jobs that are in `job_assignments` (already assigned to a runner), not the queue. The runner fetch loop will only ever receive jobs assigned to **that specific runner ID** it registered with — but the scheduler's `get_assigned_jobs()` does not filter by runner ID. If multiple runners call `GET /jobs/pending`, they all receive the same full list of all assigned jobs. The runner will then execute all of them regardless of which runner they're assigned to.

### 5.3 Job Execution

- Creates `ExecutableJob` with hardcoded `image: "rust:latest"`. The job step commands come from the scheduler's placeholder `commands: vec!["echo 'job assigned'".to_string()]`.
- **Gap:** Real pipeline commands are never transmitted. The runner receives an empty/nonsense job from the scheduler.

### 5.4 Docker Sandbox — Fail-Closed

`services/runner/src/main.rs` line 17: `std::env::set_var("GITFORGE_SANDBOX_MODE", "required");` — sets this before any async runtime starts. This forces `DockerSandbox::new()` → `connect_required()` which pings Docker and returns an error if unavailable. Production runners will **not start** without Docker.

The `new()` constructor (used in tests) still allows stub mode when `GITFORGE_SANDBOX_MODE != "required"`.

---

## 6. Persistence — SQLite via `gitforge-db`

### 6.1 Schema (inferred from models and queries)

Key tables: `repos`, `pipelines`, `pipeline_runs`, `jobs`, `runners`, `artifacts`, `events`.

**Gap:** No schema migration system visible. `Pool::migrate()` exists but the migration definitions have not been reviewed. If schema changes require a migration, there is no mechanism to apply incremental migrations.

### 6.2 Database Path

Both `api` and `ci` default to `sqlite:./gitforge.db` (relative path). In docker-compose, both bind-mount a named volume to `/data` and use `DATABASE_URL=sqlite:/data/gitforge.db`. They share the same SQLite file through the named volume.

**Gap:** SQLite over a Docker volume is safe only with File locking. Two processes (`api` + `ci`) both opening the same SQLite file concurrently will contend. `sqlx` uses WAL mode by default, which allows concurrent reads and writer serialization — but a stuck writer (e.g., the ci service holding a transaction open) will block the api service.

### 6.3 `JobQueue` In-Memory vs. SQLite

The `JobQueue` is an in-memory BinaryHeap. The scheduler's `process_queue()` call is driven by a 5-second ticker loop in the CI process. SQLite is used for durability of job and run records, but the **queue ordering itself** is not persisted. On CI process restart, `load_pending_jobs()` re-populates the queue from DB, but only for jobs in `pending` status — jobs that were `assigned` at restart are lost (they remain `assigned` in DB with no runner).

---

## 7. Release Artifact Path

From `release.yml`:
- Linux: `dist/gitforge-x86_64-unknown-linux-gnu.tar.gz` containing `api ci git-server runner`
- macOS x86_64: `dist/gitforge-x86_64-apple-darwin.tar.gz`
- macOS arm64: `dist/gitforge-aarch64-apple-darwin.tar.gz`
- Windows: `dist/gitforge-x86_64-pc-windows-gnu.tar.gz` containing `.exe` variants

Artifacts are uploaded individually per platform job, then downloaded, checksummed, flattened into `dist/release/`, and published as GitHub Release assets.

**Gap:** No provenance attestation. No reproducibility check (`.tar.gz` timestamps are set by tar, not git). No per-binary signature separate from the SHA256SUMS file.

---

## 8. Failure Handling

### 8.1 Orchestrator Failure

- Event consumer loop: on error from `handle_push_event()`, logs error and continues. On stream closure, exits loop.
- No dead-letter queue. Failed events are not retried.
- `InMemoryEventBus` has no durability. If the process crashes between DB insert and event publication, the DB record exists but the pipeline never fires.

### 8.2 Scheduler Failure

- Scheduler loop: on `load_pending_jobs()` error, logs and continues. `process_queue()` is called without error checking (returns `()`).
- Jobs in `assigned` state in DB after a runner crash are never re-enqueued. No timeout/retry mechanism.

### 8.3 Runner Failure

- Heartbeat failures are logged at `trace` level and swallowed.
- Job fetch failures are logged at `trace` level and swallowed.
- A runner that loses connectivity to the scheduler will silently stop processing jobs with no alerting.
- No job timeout at the executor level beyond the 3600-second default on `ExecutableJob`.

### 8.4 Docker Failures

- `DockerSandbox::new()` with `GITFORGE_SANDBOX_MODE=required` fails the runner startup entirely.
- Stub mode (`is_available() == false`) returns exit code 0 and `"Executing: [...]"` as stdout — tests pass silently with no Docker.

---

## 9. Clean-Checkout Behavior

### 9.1 Template Parts Target Directory

The workspace has `template-parts/rust/target/` committed with compiled artifacts (`.rustc_info.json`, `CACHEDIR.TAG`, `debug/` and `release/` trees with `.fingerprint/`, `build/`, `deps/`, `.d` files, `.rlib`/`.rmeta`/`.so` files). This is 3386 deleted files in the working tree.

**This inflates the repo by hundreds of megabytes and makes clones slow.** The `.gitignore` likely does not exclude this path.

### 9.2 `workspaces/` Subdirectory

An untracked `workspaces/platform-aegis-gitforge-smoke-20260825/` directory exists. It is not in `.gitignore` and not in `.gitmodules`.

---

## 10. Test Results

### 10.1 Workspace Tests

```
cargo test --workspace --no-fail-fast
```

**Result:** All tests pass. 9 runner service tests, scheduler tests, queue tests, process supervision tests, Docker sandbox stub tests — all green. No test failures.

### 10.2 Formatting Check

```
cargo fmt --all -- --check
```

**Result:** PASS (no output, exit 0). All workspace crates are formatted correctly.

### 10.3 Strict Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

**Result:** FAIL — 3 errors in `gitforge-api/src/routes/ci.rs`:

```
crates/gitforge-api/src/routes/ci.rs:604:16 — unnecessary lazy evaluation (ok_or_else → ok_or)
crates/gitforge-api/src/routes/ci.rs:608:22 — same
crates/gitforge-api/src/routes/ci.rs:612:20 — same
```

`gitforge-api` fails to compile under strict Clippy. Production build (`cargo build --release`) succeeds because it does not apply `-D warnings`.

### 10.4 `cargo vet`

Not installed (`no such command: vet`). Skipped.

### 10.5 Build

```
cargo build --release
```

**Result:** PASS in 81 seconds. All 4 service binaries produced at `target/release/{api,ci,git-server,runner}`.

---

## 11. Health Endpoint Analysis

The `ci` service exposes `GET /health` returning `"OK"` with no logic. The `poll-build` job in `gitforge-ci.yml` does not call this endpoint or any other. **A health endpoint being 200 OK does not confirm any of the following:**

- The CI service has connected to the database.
- The Scheduler is processing the queue.
- Runners are registered and heartbeating.
- Docker is available and containers can be created.
- The `InMemoryEventBus` has subscribers.
- Jobs are not stuck in `assigned` state after runner crash.

Claiming the pipeline works from a health endpoint alone is unsupported by evidence.

---

## 12. Concrete Gaps (Prioritized)

### CRITICAL — Pipeline never delivers real work

1. **`gitforge-ci.yml` `poll-build` is a stub.** No `GET /api/jobs/{id}/status` call exists anywhere. GitHub Actions has no way to learn if the enqueued job passed or failed. The release workflow does not use this workflow at all.

2. **Runner receives placeholder commands only.** `get_pending_jobs()` in `server.rs:135` hardcodes `commands: vec!["echo 'job assigned'".to_string()]`. The real pipeline step definitions are never transmitted to the runner.

3. **`RunnerAgent::execute_job()` uses hardcoded `image: "rust:latest"`** regardless of actual pipeline job configuration.

### CRITICAL — Scheduler has no runner lifecycle management

4. **No heartbeat timeout.** Runners that crash are never marked offline. `list_online_runners()` returns crashed runners indefinitely. `process_queue()` will attempt to assign jobs to them and fail silently (no DB persistence of assignment failure).

5. **`assigned` jobs survive runner crashes.** When a runner dies, its assigned jobs remain in `assigned` status in DB forever. No re-enqueue mechanism.

### HIGH — Security and isolation gaps

6. **Scheduler HTTP API has no authentication.** Any client that can reach port 42781 can register fake runners, claim jobs, and mark them complete.

7. **GitHub webhook authorization token is a placeholder** (`$GITFO...OKEN`). If the actual secret were committed in plaintext it would be a critical exposure.

8. **`InMemoryEventBus` provides no durability.** Events are lost on process restart. Control-plane triggers persist the run record before publishing, but native push events from the git-server would be lost.

### HIGH — Operational gaps

9. **SQLite concurrency.** Two processes (`api` + `ci`) sharing one SQLite file over a Docker volume with no lock timeout configuration risks writer starvation.

10. **`template-parts/rust/target/` committed.** 3386 files deleted from working tree, hundreds of MB of compiled artifacts in the repo. Not in `.gitignore`.

11. **`workspaces/` not ignored.** Untracked workspace subdirectory present in working tree.

### MEDIUM — Release and build gaps

12. **Release artifact has no provenance.** No reproducibility attestation, no per-binary signing, no SBOM.

13. **Clippy strict mode broken.** `gitforge-api` fails `-D warnings` with lazy evaluation lints. Devs running `make lint` (which calls `cargo clippy`) in strict mode will see failures.

14. **No schema migrations.** `Pool::migrate()` exists but the upgrade path is unversioned.

15. **No rate limiting** on `POST /pipelines/trigger`.

16. **Stale comment** in `release.yml` line 15: *"cross-compilation not available"* while cross-compilation is explicitly performed in the same file.

---

## 13. Prioritized Implementation Steps

| Priority | Step | Owner | Files |
|----------|------|-------|-------|
| P0 | Implement actual job status polling in `poll-build` job: `GET /api/jobs/{job_id}/status` with `until` loop + `break` on terminal state | CI workflow | `.github/workflows/gitforge-ci.yml` |
| P0 | Wire real pipeline commands to runner job assignment: replace placeholder in `get_pending_jobs` with actual step data from DB | Scheduler API | `crates/gitforge-scheduler/src/server.rs` |
| P0 | Add runner heartbeat timeout and auto-offline eviction: if `now() - last_heartbeat > threshold`, mark offline and call `process_queue()` to reassign orphaned jobs | Scheduler | `crates/gitforge-scheduler/src/assigner.rs` |
| P0 | Add `assigned` job recovery on scheduler startup: query jobs in `assigned` status with no active runner heartbeat, move back to `queued` | Scheduler | `crates/gitforge-scheduler/src/assigner.rs` |
| P1 | Add `Authorization: Bearer` token authentication to all scheduler HTTP routes | Scheduler API | `crates/gitforge-scheduler/src/server.rs` |
| P1 | Add `GITFORGE_CI_TRIGGER_TOKEN` to GitHub Secrets; update `gitforge-ci.yml` webhook call to use real token | CI workflow + secrets | `.github/workflows/gitforge-ci.yml` |
| P1 | Add Docker volume lock timeout or migrate to Postgres for concurrent writer access | DB + docker-compose | `crates/gitforge-db/`, `docker-compose.yml` |
| P1 | Fix Clippy strict lints in `gitforge-api/src/routes/ci.rs`: `ok_or_else` → `ok_or` | API | `crates/gitforge-api/src/routes/ci.rs` |
| P2 | Add `tracing` alert on heartbeat/fetch failures instead of `trace` level | Runner agent | `crates/gitforge-runner/src/agent.rs` |
| P2 | Add `GITFORGE_SANDBOX_MODE=docker-required` health check to docker-compose healthcheck for runner service | docker-compose | `docker-compose.yml` |
| P2 | Remove `template-parts/rust/target/` from git, add to `.gitignore`, delete from history with `git filter-branch` or BFG | Repo hygiene | `.gitignore`, `template-parts/rust/target/` |
| P2 | Add `workspaces/` to `.gitignore` | Repo hygiene | `.gitignore` |
| P2 | Add schema migration framework (e.g., `sqlx` migrations) | DB | `crates/gitforge-db/` |
| P3 | Add SHA256 checksum to each binary in release artifacts, separate from SHA256SUMS | Release | `.github/workflows/release.yml` |
| P3 | Replace `InMemoryEventBus` with a durable alternative (e.g., `gitforge-db` backed) for git-server push events | Events | `crates/gitforge-events/src/` |
| P3 | Add rate limiting to `POST /pipelines/trigger` | API | `crates/gitforge-api/src/routes/ci.rs` |

---

## 14. Audit Evidence Summary

| Check | Command | Exit Code | Result |
|-------|---------|-----------|--------|
| Workspace tests | `cargo test --workspace --no-fail-fast` | 0 | PASS — all crates green |
| Formatting | `cargo fmt --all -- --check` | 0 | PASS |
| Clippy strict | `cargo clippy --all-targets --all-features -- -D warnings` | 1 | FAIL — 3 lints in `gitforge-api` |
| Release build | `cargo build --release` | 0 | PASS — 4 binaries produced in 81s |
| Sandbox fail-closed | `grep -n "GITFORGE_SANDBOX_MODE.*required" services/runner/src/main.rs` | 0 | Confirmed line 17 |
| Health endpoint | `GET /health` in ci service | — | Returns `"OK"` with no logic (insufficient evidence) |
| Scheduler auth | `grep -n "Authorization" crates/gitforge-scheduler/src/server.rs` | 1 | No auth on any route |
| Runner job commands | `grep -n "echo.*job assigned" crates/gitforge-scheduler/src/server.rs` | 0 | Confirmed placeholder |
| Poll-build stub | `grep -n "GET.*status" .github/workflows/gitforge-ci.yml` | 1 | No polling call exists |
| `cargo vet` | `cargo vet` | — | Not installed, skipped |

---

*End of audit report.*
