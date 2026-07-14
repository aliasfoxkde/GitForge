# Changelog

All notable changes to GitForge will be documented in this file.

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
