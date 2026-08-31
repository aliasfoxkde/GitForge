# GitForge Systematic Improvement Handoff Plan

Date: 2026-08-07

> Superseded as the execution ledger by
> [planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md](planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md).
> Retain this document as historical task context; verify its status claims
> against the dated audit before acting.

This plan is written for a follow-on AI agent. Work top to bottom. Do not start broad refactors without first adding or confirming tests around the behavior being changed.

## Operating Rules For The Next AI

1. Preserve unrelated working-tree changes. Inspect `git status --short` before edits.
2. Use codebase-memory graph tools for code discovery when available. Scope product-code queries to `crates/`, `services/`, and top-level `tests/`; `template-parts/` is historical/template material.
3. Keep each task small enough to verify independently.
4. Prefer existing crate boundaries and patterns over new abstractions.
5. Run the smallest relevant test first, then the full verification set before handoff.
6. Update docs when behavior changes.

## Current Baseline

The following checks passed during the audit:

```bash
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

Known current risks:

- Docker sandbox has a silent stub path that reports success.
- Route authentication logic is duplicated across API modules.
- Git server maps repository paths to deterministic IDs instead of DB-backed repository records.
- Scheduler and runner HTTP flow uses placeholder job commands and incomplete persistence.
- DB migrations are inline SQL in code.
- Historical docs contain stale `gitforce-*` names and older template-system instructions.

## Phase 0: Repo And Documentation Hygiene

Goal: make the repo easy to reason about before changing product behavior.

### Task 0.1 Confirm Ignored Runtime Artifacts

Files:

- `.gitignore`
- local `repos/`
- local `test.db`
- any nested `target/` directories outside the root target

Steps:

1. Run `git status --short`.
2. Confirm runtime artifacts are ignored.
3. Ask the owner before deleting local runtime data.

Acceptance:

- Runtime repositories and scratch DB files are not shown as untracked work.

Verification:

```bash
git status --short
```

### Task 0.2 Keep Canonical Docs Current

Files:

- `docs/README.md`
- `docs/AUDIT.md`
- `docs/HANDOFF_PLAN.md`
- `README.md`

Steps:

1. Update the docs index when adding or deprecating docs.
2. Keep current status statements dated and tied to commands.
3. Avoid "service is running" claims unless they are command output from the current environment.

Acceptance:

- A new contributor can start from `docs/README.md` and find the current plan.

## Phase 1: Authentication Refactor

Goal: one authentication path for protected API endpoints.

Files to inspect first:

- `crates/gitforge-api/src/auth.rs`
- `crates/gitforge-api/src/middleware.rs`
- `crates/gitforge-api/src/server.rs`
- `crates/gitforge-api/src/routes/artifacts.rs`
- `crates/gitforge-api/src/routes/ci.rs`
- `crates/gitforge-api/src/routes/repo.rs`
- `crates/gitforge-api/src/routes/runners.rs`
- `crates/gitforge-api/src/routes/webhook.rs`

Problem:

- Each route module has a local `extract_user` helper.
- Tests duplicate bearer-token parsing cases across modules.
- `register_runner` is intentionally public but not clearly isolated from protected runner routes.

Implementation plan:

1. Create a reusable authenticated-user extractor or tower middleware.
2. Store validated claims/user ID in request extensions.
3. Apply it once to protected `/api` routes.
4. Leave public routes explicit:
   - `/health`
   - `/metrics`
   - `/swagger-ui`
   - `/api-docs/openapi.json`
   - `/auth/login`
   - `/auth/status`
   - runner registration if that remains intentionally unauthenticated
5. Remove route-local `extract_user` functions.
6. Replace duplicated auth tests with:
   - focused tests for token extraction/claims validation
   - middleware/extractor tests for missing, malformed, invalid, and valid tokens
   - route smoke tests that prove protected routes reject unauthenticated requests

Acceptance:

- `rg "fn extract_user" crates/gitforge-api/src/routes` returns no route-local helpers.
- Protected routes behave consistently.
- Public exceptions are documented in `docs/API.md`.

Verification:

```bash
cargo test -p gitforge-api auth middleware routes
cargo clippy -p gitforge-api --all-targets -- -D warnings
```

## Phase 2: Docker Sandbox Explicit Modes

Goal: production runner must not report success when Docker is unavailable.

Files to inspect first:

- `crates/gitforge-sandbox/src/docker.rs`
- `crates/gitforge-sandbox/src/lib.rs`
- `crates/gitforge-runner/src/agent.rs`
- `crates/gitforge-runner/src/executor.rs`
- `services/runner/src/main.rs`

Problem:

- `DockerSandbox::new()` falls back to stub mode.
- Stub execution returns exit code 0 for every command.
- Real Docker tests are ignored.

Implementation plan:

1. Add explicit constructors, for example:
   - `DockerSandbox::connect_required() -> Result<Self>`
   - `DockerSandbox::stub_for_tests() -> Self`
   - keep `with_limits` only if its mode is unambiguous
2. Update production runner startup to use required Docker mode by default.
3. Add an environment/config escape hatch only if the product needs non-Docker local dry runs.
4. Update unit tests to call `stub_for_tests()`.
5. Add Docker-capable tests that verify:
   - `sh -c "exit 7"` returns exit code 7
   - stdout and stderr are captured
   - network-disabled limits are applied or explicitly documented as best effort

Acceptance:

- Runner startup fails clearly when Docker is required and unavailable.
- Stub mode cannot be entered accidentally by production code.

Verification:

```bash
cargo test -p gitforge-sandbox
cargo test -p gitforge-runner
cargo test -p gitforge-sandbox -- --ignored
```

## Phase 3: Git Repository Lookup And Authorization

Goal: Git protocol endpoints operate on the same repositories as the API and DB.

Files to inspect first:

- `services/git-server/src/main.rs`
- `crates/gitforge-core/src/repo.rs`
- `crates/gitforge-core/src/storage.rs`
- `crates/gitforge-db/src/models/repo.rs`
- `crates/gitforge-db/src/queries.rs`
- `crates/gitforge-api/src/routes/repo.rs`

Problem:

- `git-server` hashes `owner/repo` to derive a `RepoId`.
- API-created repositories use generated UUIDs and DB `git_path`.
- `RepoService` is initialized but not used for path lookup.

Implementation plan:

1. Add DB query methods for repository lookup by owner/name or canonical git path.
2. Decide the canonical Git URL path format and document it:
   - `/{owner}/{repo}.git`
   - `/{owner}/{repo}`
   - org/group nesting if supported
3. Replace `derive_repo_id` in `git-server` with DB-backed lookup.
4. Use the stored repo ID and git path to open storage.
5. Add repository visibility/access checks before serving upload-pack/receive-pack.
6. Add integration tests that:
   - create a user and repo in DB
   - create/open the storage repo
   - request upload-pack through git-server handler
   - verify missing repos return 404

Acceptance:

- API-created repositories are reachable by Git HTTP.
- Git server no longer relies on path-hashed IDs.

Verification:

```bash
cargo test -p gitforge-core
cargo test -p git-server
cargo test --workspace --no-fail-fast
```

## Phase 4: Scheduler, Runner, And Job Persistence

Goal: replace synthetic job flow with durable scheduler/runner state transitions.

Files to inspect first:

- `crates/gitforge-scheduler/src/server.rs`
- `crates/gitforge-scheduler/src/assigner.rs`
- `crates/gitforge-scheduler/src/queue.rs`
- `crates/gitforge-runner/src/agent.rs`
- `crates/gitforge-runner/src/executor.rs`
- `crates/gitforge-db/src/models/job.rs`
- `crates/gitforge-db/src/queries.rs`
- `crates/gitforge-api/src/routes/ci.rs`
- `crates/gitforge-storage/src/job_logs.rs`

Problem:

- `/jobs/pending` returns placeholder commands.
- Assignment/completion handlers mostly log and return success.
- Runner completion does not fully update durable job status/logs.

Implementation plan:

1. Define a scheduler API contract:
   - runner registration
   - runner heartbeat
   - fetch one or more jobs
   - claim/assign job
   - job started
   - append logs
   - job completed
2. Extend DB model/query support as needed:
   - status transition method
   - runner assignment
   - started/finished timestamps
   - exit code if not currently modeled
3. Update `/jobs/pending` to return real job definitions from DB/pipeline config.
4. Update runner to report started/completed status.
5. Persist job logs through `JobLogStore`.
6. Add end-to-end tests for success and failure.

Acceptance:

- A queued job becomes `running`, then `succeeded` or `failed`.
- API job and log endpoints reflect runner output.
- Placeholder command string is removed.

Verification:

```bash
cargo test -p gitforge-scheduler
cargo test -p gitforge-runner
cargo test -p gitforge-api routes::ci
cargo test --workspace --no-fail-fast
```

## Phase 5: Git Protocol Core Deduplication

Goal: one implementation of shared Git pack/ref behavior.

Files to inspect first:

- `crates/gitforge-core/src/git_protocol/http.rs`
- `crates/gitforge-core/src/git_protocol/ssh.rs`
- `crates/gitforge-core/src/git_protocol/mod.rs`

Problem:

- HTTP and SSH handlers duplicate pkt-line formatting, ref walking, pack scanning, and object DB writes.

Implementation plan:

1. Create a shared module, for example `git_protocol/core.rs`.
2. Move common helpers:
   - `format_pkt_line`
   - ref advertisement builder
   - pack start detection
   - pack write to ODB
   - receive-pack result builder
3. Keep HTTP/SSH wrappers focused on transport differences.
4. Add shared unit tests.
5. Confirm response framing still satisfies existing tests.

Acceptance:

- `write_pack_to_odb` exists once.
- Shared helpers have direct tests.

Verification:

```bash
cargo test -p gitforge-core
cargo clippy -p gitforge-core --all-targets -- -D warnings
```

## Phase 6: Database Migrations And Row Decoding

Goal: make schema evolution auditable and prevent panics on malformed data.

Files to inspect first:

- `crates/gitforge-db/src/connection.rs`
- `crates/gitforge-db/src/queries.rs`
- `crates/gitforge-db/src/models/*.rs`
- `crates/gitforge-db/tests/integration.rs`

Problem:

- Migrations are inline SQL.
- Row decoding repeatedly uses `unwrap()` for UUID and timestamp parsing.

Implementation plan:

1. Create `crates/gitforge-db/migrations/`.
2. Move schema into versioned SQL migration files.
3. Update `Pool::migrate` to run migration files.
4. Add row decoder helpers per model.
5. Replace parse `unwrap()` calls with mapped `Error::database(...)`.
6. Add malformed-row tests via direct SQL inserts.

Acceptance:

- Migration history is visible in files.
- Query methods return errors instead of panicking on bad stored data.

Verification:

```bash
cargo test -p gitforge-db
cargo clippy -p gitforge-db --all-targets -- -D warnings
```

## Phase 7: API Consistency And CLI Wiring

Goal: API behavior is predictable and CLI commands use real endpoints.

Files to inspect first:

- `crates/gitforge-api/src/routes/*.rs`
- `crates/gitforge-api/src/server.rs`
- `crates/gitforge-cli/src/client.rs`
- `crates/gitforge-cli/src/main.rs`
- `crates/gitforge-cli/src/config.rs`

Problem:

- Some route errors return empty arrays instead of 5xx.
- CLI appears partly parser-focused rather than fully wired.
- API docs have historically drifted from router paths.

Implementation plan:

1. Standardize error responses.
2. Ensure all protected API docs include `/api` prefix.
3. Add missing endpoint tests for actual route paths.
4. Wire CLI commands one group at a time:
   - auth
   - repos
   - pipelines
   - runners
   - artifacts/logs
5. Add CLI integration tests with a local test server.

Acceptance:

- CLI commands perform real HTTP operations.
- API docs match tests.

Verification:

```bash
cargo test -p gitforge-api
cargo test -p gitforge-cli
```

## Phase 8: CI And Release Hardening

Goal: CI reflects the actual Rust workspace and blocks broken releases.

Files to inspect first:

- `.github/workflows/*.yml`
- `.github/dependabot.yml`
- `Dockerfile`
- `docker-compose.yml`

Problem:

- Workflow history includes disabled/replaced workflows.
- Some docs mention older workflow names.
- Release and security gates need confirmation against current files.

Implementation plan:

1. Inventory active workflows.
2. Ensure Rust CI runs:
   - fmt check
   - clippy with warnings denied
   - workspace tests
   - cargo audit or equivalent
3. Ensure release workflow depends on tests.
4. Add Docker build validation if Dockerfile is production-supported.
5. Document CI checks in `docs/TESTING_STRATEGY.md`.

Acceptance:

- Active workflow names and docs agree.
- Release cannot run from an untested build.

Verification:

```bash
gh workflow list
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

## Phase 9: Historical Documentation Cleanup

Goal: remove confusion from old plans without losing useful design notes.

Files/directories:

- `docs/PLAN.md`
- `docs/PLAN_NEXT_PHASE.md`
- `docs/MASTER_PLAN.md`
- `docs/TASKS.md`
- `docs/PROGRESS.md`
- `docs/planning/`
- `docs/project_notes/`
- `docs/template-dev/`

Implementation plan:

1. Add a historical banner to old planning docs or move them under `docs/archive/`.
2. Preserve any still-relevant tasks by copying them into `docs/HANDOFF_PLAN.md`.
3. Remove or update stale `gitforce-*` paths in canonical docs only. Historical docs may keep original wording if clearly archived.

Acceptance:

- `docs/README.md` clearly separates current docs from historical notes.
- A search for `gitforce-*` in canonical docs returns no stale crate paths.

Verification for operational docs:

```bash
rg "gitforce-|GitForce|Dark Factory|golangci|pytest|make test" README.md docs/README.md docs/ARCHITECTURE.md docs/API.md docs/RUNBOOK.md docs/TESTING_STRATEGY.md docs/CONTRIBUTING.md docs/SECURITY.md
```

## Final Full Verification

Run before handing back:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```
