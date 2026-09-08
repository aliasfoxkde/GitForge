# Changelog

All notable changes to GitForge will be documented in this file.

## [0.4.0] - 2026-09-08

### Security

- Git over SSH now works and authenticates: the transport was rewritten
  from ssh2/libssh2 (client-only library — the old listener could never
  complete a handshake, so the port served nothing) to a russh server that
  pipes authenticated channels to real `git upload-pack`/`git receive-pack`
  child processes. Public-key auth is required; accepted fingerprints are
  logged. The ed25519 host key is generated on first boot, persisted under
  the ssh volume (override path with `GITFORGE_SSH_HOST_KEY`), and
  published as `.pub` for `known_hosts` pinning
- Scheduler job completion requires lease proof: anonymous completion of a
  known job is rejected (409), unknown jobs return 404
- Removed the `POST /jobs/{id}/assign` no-op stub that acknowledged
  assignments without performing any
- Runner registration is fail-closed by default
  (`GITFORGE_RUNNER_STANDALONE=deny`); standalone fallback requires explicit
  opt-in
- docker-compose requires `GITFORGE_SCHEDULER_TOKEN` for CI and runners,
  matching the fail-closed scheduler auth boundary

### Added

- Git Smart HTTP protocol integration tests: real `git push`, `clone`,
  `fetch`, and `ls-remote` against the spawned git-server binary
- Git over SSH protocol integration tests: real `ssh-keygen` client
  keypairs and host-key pinning; `push`, `clone`, `fetch`, and `ls-remote`
  over the `ssh://` transport, plus rejection of key-less clients and
  unknown repositories
- Periodic orphaned-run reconciliation in CI (60s loop, 120s run-age grace,
  live-engine guard); startup still sweeps once
- Restart recovery stops orphaned executions: the scheduler's cancellation
  probe reports every terminal durable status, the runner skips log,
  artifact, and completion reporting once its outcome was decided
  mid-execution, and credentialed completions for unassigned jobs are
  rejected 409 with an explicit orphaned-outcome message
- Runner registration retries an unreachable or not-ready scheduler with
  bounded exponential backoff (`GITFORGE_REGISTER_ATTEMPTS`, default 6;
  `GITFORGE_REGISTER_BACKOFF_SECS`, default 1s, doubling to a 30s cap) so a
  runner started beside a restarting control plane survives the compose race
  instead of exiting; auth rejections (401/403) are never retried
- ShellCheck and actionlint gates in Rust CI and `make lint`
- cargo-vet supply chain (`supply-chain/`) behind `make lint`
- cargo-vet enforcement in Rust CI: a `supply-chain` job runs `cargo vet`
  so dependency changes that lose audit coverage fail CI. Five public
  audit registries (isrg, google, mozilla, bytecode-alliance,
  embark-studios) are registered and pinned in `imports.lock`, and the
  dependencies introduced by the SSH transport rewrite are recorded as
  tracked exemptions so the gate is green without pretending they were
  audited

### Changed

- The git-server image no longer ships `openssh-server`: SSH is served
  in-process, so the container carries no sshd

### Fixed

- API list endpoints return 500 `database_error` instead of masking
  storage failures as empty 200 responses
- ai-review.yml passed review outputs as action inputs instead of step
  env vars, so the PR comment always used its fallback text
- ShellCheck SC2012/SC2034/SC2155 findings in scripts/
- Compose stack could not run a pipeline end to end (found by a live
  queued-job smoke, all fixed and validated):
  - api, ci, and git-server used separate SQLite volumes and git-server
    had no `DATABASE_URL`, so pushes and pipeline triggers could not
    resolve repositories; they now share one `gitforge-data` volume
  - `sqlite:` database URLs open an existing file only; compose now uses
    `?mode=rwc` so first boot creates the database
  - git-server port mappings pointed at ports the server does not bind
    (in-container ports are 42022/42782)
  - api provisioned repositories outside the shared git volume (no
    `GIT_ROOT`) and ci could not read them (no `/git` mount)
  - images lacked the volume mountpoints, so Docker seeded fresh volumes
    root-owned and the non-root services could not write; the Dockerfile
    now creates `/data` and `/git` with `gitforge` ownership
  - the ci image had no `git` binary, so loading `.gitforce.yml` from a
    pushed revision failed with ENOENT
  - the non-root runner could not open the mounted Docker socket; compose
    now maps the host docker group via `DOCKER_GID`
  - run workspaces are bind-mounted at a host-identical absolute path
    (`GITFORGE_WORKSPACE_HOST_DIR`): the runner passes workspace paths to
    the host Docker daemon as bind sources, so a container-only path made
    every job see an empty auto-created directory

## [0.3.2] - 2026-08-28

### Added

- MockAiProvider for AI testing with configurable behavior (success, failure, rate limiting)
- Executor unit tests (7 new tests for JobResult, ExecutableJob, compute_output_sha)
- Clone derive on AiError for mock support

### Changed

- Coverage improved from 79.80% to 80.12%
- 46 total tests in gitforge-runner (up from 39)

## [0.3.0] - 2026-08-28

### Fixed

- Storage race condition: Added `sync_all()` calls to ensure artifact and cache data is flushed to disk before returning
- Scheduler stale snapshots: Reject runner-loss snapshots that are older than current state
- Scheduler persistence errors: Fail closed on database errors to prevent state corruption
- Build daemon: Proper shutdown coordination and cargo flag forwarding
- Multi-repository recovery: Preserve repository context during job recovery

### Improved

- Workspace rustfmt applied consistently
- CI pipeline streaming with lease-fenced logs
- SBOM and artifact provenance attestations added
- LLVM coverage ratcheting in CI

### Documentation

- Git at Scale research: Analysis of Cursor's distributed Git architecture
- Comprehensive execution plan: 8-phase roadmap with evidence-based tracking

## [0.2.0] - 2026-07-14

### Added

#### Phase A - Security Hardening (COMPLETED)
- JWT authentication enforced on all API routes except /health and /metrics
- Auth middleware with token validation
- AuthenticatedUser extractor for route handlers
- Public paths: /health, /metrics, /swagger-ui, /api-docs
- Protected routes: /api/repos, /api/pipelines, /api/pipeline-runs, /api/jobs, /api/runners, /api/artifacts

#### Phase B - Runner-Scheduler Communication (COMPLETED)
- Real HTTP client implementation in runner agent using reqwest
- Runner registration via HTTP POST to scheduler
- Heartbeat loop sending POST to scheduler
- Job fetch loop polling GET /jobs/pending
- Scheduler HTTP server with routes for runners and jobs
- Graceful fallback when scheduler is unavailable

#### Phase C - Event Pipeline Triggering (COMPLETED)
- CI service event consumer subscribed to push events
- Pipeline triggered automatically on push received events
- Default pipeline definition generated per repository
- Jobs enqueued to scheduler on pipeline start

#### Phase D - Artifact Storage (COMPLETED)
- Routes wired with FileStorage integration
- Get artifact metadata from storage
- Delete artifact from storage
- Auth enforced on all artifact routes

#### Phase E - Docker Deployment (COMPLETED)
- Multi-stage Dockerfile for minimal production images
- Separate images for api, ci, runner, and git-server
- docker-compose.yml with all services configured
- config.toml.example with all configuration options
- Docker-in-Docker support for runner

#### Scheduler HTTP Endpoints (COMPLETED)
- POST /runners - Register new runner
- POST /runners/{id}/heartbeat - Runner heartbeat
- GET /jobs/pending - Get pending jobs for runner
- POST /jobs/{id}/assign - Assign job to runner
- POST /jobs/{id}/complete - Mark job complete

### Fixed

- Fixed unused imports across multiple crates
- Fixed serde_json missing dependencies
- Fixed EventFilter export from gitforce-events
- Fixed JobDefinition/StepDefinition exports from gitforce-ci
- Fixed auth middleware compatibility with Axum 0.7
- Fixed ArtifactId private field issue with From<Uuid> implementation
- Resolved all clippy warnings for strict linting (-D warnings)
- Fixed needless borrows, unnecessary map_or, if_same_then_else
- Fixed derivable_impls with #[derive(Default)]
- Fixed never_loop by replacing while with if in scheduler
- Fixed Priority enum ordering with proper Default

### Linting & Quality

- Zero clippy warnings with strict -D warnings
- All tests passing (200+ tests across workspace)
- Full workspace builds successfully
- Git hooks installed via make setup
- GitHub Actions Rust workflow with test, lint, build, coverage, security

## [0.1.0] - 2026-07-06

### Added

#### Core Infrastructure
- `gitforce-common` - Shared types, UUIDs, errors, time utilities
- `gitforce-db` - Database models and connection pool
- `gitforce-events` - Event bus and event type definitions

#### Git Server
- `gitforce-core` - Git protocol handlers and repository management
- Repository storage backend (FileStorageBackend)
- Git SSH and HTTP protocol handlers
- Hook execution system (pre-receive, post-receive)

#### CI/CD
- `gitforce-ci` - Pipeline orchestration and DAG execution
- Pipeline definition loader (YAML-based)
- Job state machine (Pending → Queued → Assigned → Running → Completed)
- `gitforce-scheduler` - Job queue and runner assignment
- Priority-based job scheduling
- Runner selection policies

#### Execution
- `gitforce-runner` - Job execution agent
- Runner registration and heartbeat
- `gitforce-sandbox` - Container isolation
- Docker sandbox implementation with bollard (real Docker integration)
- Resource limits support

#### Storage
- `gitforce-storage` - Artifact and cache storage
- Filesystem-based artifact store
- Cache store with key/retrieval

#### API
- `gitforce-api` - REST API gateway
- Repository endpoints (wired to SQLite)
- CI/CD endpoints (pipelines, jobs, logs - wired to SQLite)
- Runner management endpoints (wired to SQLite)
- Artifact endpoints
- JWT authentication
- OpenAPI 3.0 / Swagger UI documentation

#### Services
- `git-server` - Git SSH/HTTP server binary
- `ci` - CI orchestrator binary
- `runner` - Runner agent binary
- `api` - HTTP API server binary

### Technical Details

- **Language**: Rust (10 crates, 4 service binaries)
- **Async Runtime**: Tokio
- **Web Framework**: Axum 0.7
- **Database**: SQLite (MVP) via sqlx
- **Git Library**: git2
- **Container Runtime**: bollard (Docker client)

### Next Steps

- CLI tool for GitForge
- Cloud sync protocol
