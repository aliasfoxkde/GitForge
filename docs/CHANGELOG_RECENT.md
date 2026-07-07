# Changelog

All notable changes to GitForge will be documented in this file.

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
- Docker sandbox implementation (MVP)
- Resource limits support

#### Storage
- `gitforce-storage` - Artifact and cache storage
- Filesystem-based artifact store
- Cache store with key/retrieval

#### API
- `gitforce-api` - REST API gateway
- Repository endpoints
- CI/CD endpoints (pipelines, jobs, logs)
- Runner management endpoints
- Artifact endpoints
- JWT authentication

#### Services
- `git-server` - Git SSH/HTTP server binary
- `ci` - CI orchestrator binary
- `runner` - Runner agent binary
- `api` - HTTP API server binary

### Technical Details

- **Language**: Rust (10 crates, 4 service binaries)
- **Async Runtime**: Tokio
- **Web Framework**: Axum 0.7
- **Database**: PostgreSQL (via sqlx, future)
- **Git Library**: git2

### Known Issues

- Some test failures in gitforce-ci due to evolving API
- gitforce-storage tests need dependency fixes
- Docker sandbox is MVP stub (no actual container execution)

### Next Steps

- Fix test failures
- Add integration tests
- Implement health checks
- Add Prometheus metrics
- Database migration system
