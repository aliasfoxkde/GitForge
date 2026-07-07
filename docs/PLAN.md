# GitForge Project Plan

**Version**: 1.0.0
**Last Updated**: 2026-07-06
**Status**: APPROVED

---

## Project Overview

**GitForge** is a self-hosted Git platform with event-driven CI/CD capabilities, similar to GitHub Actions but fully self-hosted. The system provides:

- Git server (SSH + HTTP) for repository hosting
- Event-driven CI/CD orchestration
- Sandbox-based job execution runners
- Artifact and cache storage
- GitHub mirror synchronization
- REST API for frontend integration

**Target Users**: Development teams requiring self-hosted Git repository management with integrated CI/CD pipelines.

---

## Architecture Overview

### System Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              GitForge Platform                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  ┌─────────────┐     ┌─────────────┐     ┌─────────────┐                  │
│  │ Git Server  │     │  CI Engine  │     │   Runner    │                  │
│  │   (SSH)     │     │             │     │   Agent     │                  │
│  └──────┬──────┘     └──────┬──────┘     └──────┬──────┘                  │
│         │                   │                    │                          │
│  ┌──────▼──────┐     ┌──────▼──────┐     ┌──────▼──────┐                  │
│  │ Git Server  │     │  Scheduler  │     │  Sandbox    │                  │
│  │   (HTTP)    │     │             │     │  (Docker)   │                  │
│  └──────┬──────┘     └──────┬──────┘     └──────┬──────┘                  │
│         │                   │                    │                          │
│         └───────────────────┼────────────────────┘                          │
│                             │                                               │
│                    ┌────────▼────────┐                                     │
│                    │  Event System   │                                     │
│                    │   (NATS/Async)  │                                     │
│                    └────────┬────────┘                                     │
│                             │                                               │
│         ┌───────────────────┼───────────────────┐                          │
│         │                   │                   │                          │
│  ┌──────▼──────┐     ┌──────▼──────┐     ┌──────▼──────┐                │
│  │   Storage   │     │    API      │     │   Mirror    │                │
│  │ (Artifacts) │     │  Gateway    │     │  (GitHub)   │                │
│  └─────────────┘     └─────────────┘     └─────────────┘                │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Technology Stack

- **Language**: Rust (primary), Go (infrastructure templates)
- **Database**: PostgreSQL 15+
- **Event Bus**: In-memory (MVP), NATS (production)
- **Container Runtime**: Docker (MVP), Firecracker (production)
- **API Framework**: Axum
- **Async Runtime**: Tokio

### Module Architecture

| Crate | Responsibility |
|-------|----------------|
| `gitforce-common` | Shared types, UUIDs, errors |
| `gitforce-db` | Database models and migrations |
| `gitforce-events` | Event bus and type definitions |
| `gitforce-core` | Git protocol handlers (SSH/HTTP) |
| `gitforce-ci` | Pipeline orchestration and DAG execution |
| `gitforce-scheduler` | Job queue and runner assignment |
| `gitforce-runner` | Job execution agent |
| `gitforce-sandbox` | Container/VM isolation |
| `gitforce-storage` | Artifact and cache storage |
| `gitforce-api` | REST API gateway |

---

## Implementation Phases

### Phase 1: Foundation (Common + DB + Events)

**Goals**: Establish core shared infrastructure

**Tasks**:
- [x] Set up Rust workspace structure
- [ ] Implement `gitforce-common` crate
  - UUID types (RepoId, JobId, PipelineId, RunnerId, StepId)
  - Unified error enum
  - Result<T> alias
  - Time utilities
- [ ] Implement `gitforce-db` crate
  - Postgres connection pool
  - Repository model
  - Pipeline/PipelineRun models
  - Job model
  - Runner model
  - Event log model
  - Migration system
- [ ] Implement `gitforce-events` crate
  - EventEnvelope structure
  - EventType enum
  - Event bus trait
  - In-memory pub/sub implementation
  - JSON serialization

**Deliverables**:
- Working Rust workspace
- Database schema with migrations
- Event system ready for CI integration

---

### Phase 2: Git Server Core

**Goals**: Implement bare git server with SSH and HTTP support

**Tasks**:
- [ ] Implement `gitforce-core` crate
  - Bare repository filesystem storage
  - Repository CRUD operations
  - Git protocol handlers
  - SSH handler for git push
  - HTTP handler for git clone/push
  - Hook execution system (pre-receive, post-receive)
  - Authentication middleware

**Deliverables**:
- Git push/pull working over SSH and HTTP
- Repository management API
- Post-receive hook triggers events

---

### Phase 3: CI Orchestrator

**Goals**: Build pipeline parsing and job orchestration

**Tasks**:
- [ ] Implement `gitforce-ci` crate
  - Pipeline definition loader (from .gitforce.yml)
  - DAG builder for job dependencies
  - Job state machine (pending → queued → running → succeeded/failed)
  - Execution engine
  - Retry logic
- [ ] Implement `gitforce-scheduler` crate
  - Priority queue management
  - Runner selection policy
  - Job assignment
  - Heartbeat monitoring
  - Dead runner detection

**Deliverables**:
- Pipeline triggers from push events
- Job queue with priority handling
- Runner assignment logic

---

### Phase 4: Runner System

**Goals**: Implement job execution agents

**Tasks**:
- [ ] Implement `gitforce-runner` crate
  - Runner registration
  - Job polling/fetching
  - Log streaming
  - Artifact upload
  - Heartbeat loop
- [ ] Implement `gitforce-sandbox` crate
  - Docker backend
  - Resource limits (CPU, memory, disk, time)
  - Workspace mounting
  - Cleanup guarantees

**Deliverables**:
- Runner agent binary
- Job execution in Docker containers
- Log streaming to storage

---

### Phase 5: Storage System

**Goals**: Implement artifact and cache storage

**Tasks**:
- [ ] Implement `gitforce-storage` crate
  - Filesystem artifact store
  - Cache key/retrieval system
  - Retention policy engine
  - Deduplication

**Deliverables**:
- Artifact upload/download
- Build cache persistence
- TTL-based cleanup

---

### Phase 6: API Gateway

**Goals**: Expose system state via REST API

**Tasks**:
- [ ] Implement `gitforce-api` crate
  - Axum server setup
  - Repository endpoints
  - Pipeline endpoints
  - Job status endpoints
  - Runner status endpoints
  - Artifact endpoints
  - Log retrieval endpoints
  - JWT authentication middleware

**Deliverables**:
- REST API for frontend integration
- Job status visibility
- Artifact retrieval

---

### Phase 7: Integration & Hardening

**Goals**: Wire all components and harden for production

**Tasks**:
- [ ] Create service binaries
  - `git-server` - Git SSH/HTTP server
  - `ci-orchestrator` - CI engine + scheduler
  - `runner-agent` - Job execution agent
  - `api-gateway` - HTTP API server
- [ ] Implement graceful shutdown
- [ ] Add structured logging
- [ ] Add Prometheus metrics
- [ ] Implement health checks
- [ ] Add GitHub mirror service (future)

**Deliverables**:
- Full end-to-end integration
- Production-ready services
- Observability stack

---

## Database Schema

### Core Tables

```sql
-- Users & Auth
CREATE TABLE users (
    id UUID PRIMARY KEY,
    username TEXT UNIQUE NOT NULL,
    email TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE roles (
    id UUID PRIMARY KEY,
    name TEXT UNIQUE NOT NULL
);

CREATE TABLE user_roles (
    user_id UUID REFERENCES users(id),
    role_id UUID REFERENCES roles(id),
    PRIMARY KEY (user_id, role_id)
);

-- Repositories
CREATE TABLE repositories (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    owner_id UUID REFERENCES users(id),
    visibility TEXT NOT NULL CHECK (visibility IN ('public', 'private')),
    git_path TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Pipeline Runs
CREATE TABLE pipeline_runs (
    id UUID PRIMARY KEY,
    pipeline_id UUID REFERENCES pipelines(id),
    repo_id UUID REFERENCES repositories(id),
    status TEXT NOT NULL,
    triggered_by TEXT NOT NULL,
    commit_hash TEXT NOT NULL,
    started_at TIMESTAMP,
    finished_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Jobs
CREATE TABLE jobs (
    id UUID PRIMARY KEY,
    pipeline_run_id UUID REFERENCES pipeline_runs(id),
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    runner_id UUID,
    started_at TIMESTAMP,
    finished_at TIMESTAMP,
    retry_count INT DEFAULT 0,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Runners
CREATE TABLE runners (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    last_heartbeat TIMESTAMP,
    capacity INT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Artifacts
CREATE TABLE artifacts (
    id UUID PRIMARY KEY,
    job_id UUID REFERENCES jobs(id),
    path TEXT NOT NULL,
    checksum TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- Events (Append-only log)
CREATE TABLE events (
    id UUID PRIMARY KEY,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_events_type ON events(event_type);
CREATE INDEX idx_events_created ON events(created_at);

-- Mirror State
CREATE TABLE mirror_states (
    id UUID PRIMARY KEY,
    repo_id UUID REFERENCES repositories(id),
    github_repo TEXT NOT NULL,
    last_synced_commit TEXT,
    status TEXT NOT NULL,
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);
```

---

## Event Schema

### Event Envelope

```rust
struct EventEnvelope {
    event_id: Uuid,
    event_type: EventType,
    event_version: u8,
    timestamp: i64,
    repo_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    correlation_id: Option<Uuid>,
    payload: EventPayload,
}
```

### Event Types

| Event | Trigger |
|-------|---------|
| `RepoCreated` | New repository created |
| `RepoDeleted` | Repository deleted |
| `PushReceived` | Git push received |
| `RefUpdated` | Branch/tag updated |
| `PipelineTriggered` | CI pipeline triggered |
| `PipelineStarted` | Pipeline execution started |
| `PipelineFinished` | Pipeline execution completed |
| `JobQueued` | Job added to queue |
| `JobStarted` | Job execution started |
| `JobFinished` | Job execution completed |
| `ArtifactCreated` | Artifact stored |
| `RunnerRegistered` | New runner joined |
| `RunnerHeartbeat` | Runner health ping |
| `MirrorSyncRequested` | GitHub sync requested |
| `MirrorSyncCompleted` | GitHub sync finished |

---

## Acceptance Criteria

### MVP Definition

The system is complete when:

- [ ] Git push triggers CI pipeline automatically
- [ ] CI executes jobs in Docker sandbox
- [ ] Artifacts are stored and retrievable
- [ ] Job status is visible via REST API
- [ ] System survives runner failure without state corruption
- [ ] Pipelines are reproducible

### Quality Gates

| Metric | Target |
|--------|--------|
| Code Coverage (core logic) | 90%+ |
| Code Coverage (API handlers) | 85%+ |
| Linting | Zero warnings |
| Security | No CVEs |

---

## File Structure

```
gitforce/
├── Cargo.toml                    # Workspace definition
├── crates/
│   ├── gitforce-common/         # Shared primitives
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ids.rs
│   │       ├── error.rs
│   │       └── time.rs
│   ├── gitforce-db/            # Database layer
│   │   ├── Cargo.toml
│   │   └── src/
│   ├── gitforce-events/         # Event system
│   ├── gitforce-core/           # Git server
│   ├── gitforce-ci/             # CI orchestrator
│   ├── gitforce-scheduler/       # Job scheduler
│   ├── gitforce-runner/          # Runner agent
│   ├── gitforce-sandbox/         # Isolation layer
│   ├── gitforce-storage/          # Artifacts/cache
│   └── gitforce-api/             # REST API
├── services/                     # Binary targets
│   ├── git-server/
│   ├── ci/
│   ├── runner/
│   └── api/
├── migrations/                   # SQL migrations
└── docs/                         # Architecture docs
```

---

## Timeline

### Week 1-2: Foundation
- Workspace setup
- gitforce-common
- gitforce-db with migrations
- gitforce-events

### Week 3-4: Git Core
- gitforce-core (SSH + HTTP handlers)
- Hook system
- Integration with event system

### Week 5-6: CI/CD
- gitforce-ci (pipeline + DAG)
- gitforce-scheduler (queue + assignment)
- Runner heartbeat and monitoring

### Week 7-8: Execution
- gitforce-runner
- gitforce-sandbox (Docker)
- Log streaming

### Week 9-10: API & Integration
- gitforce-api
- gitforce-storage
- Service wiring
- End-to-end testing

### Week 11-12: Hardening
- Observability (metrics, logging)
- Health checks
- Graceful shutdown
- Documentation

---

## Dependencies

### External Dependencies
| Dependency | Version | Purpose |
|------------|---------|---------|
| tokio | 1.x | Async runtime |
| axum | 0.7.x | HTTP framework |
| sqlx | 0.8.x | Database access |
| serde | 1.x | Serialization |
| uuid | 1.x | ID generation |
| chrono | 0.4.x | Time handling |

### System Dependencies
| Dependency | Purpose |
|------------|---------|
| PostgreSQL 15+ | Primary database |
| Docker | Sandbox execution (MVP) |
| git | Git protocol handling |

---

## Risk Management

| Risk | Impact | Mitigation |
|------|--------|------------|
| Git protocol complexity | HIGH | Use git2-rs library for protocol handling |
| Sandbox isolation gaps | HIGH | Start with Docker, add Firecracker later |
| Database scalability | MED | Design for read replicas from start |
| Event ordering guarantees | MED | Use append-only log, idempotent consumers |

---

## Open Questions

1. **GitHub Mirror Auth**: How to authenticate with GitHub? (OAuth app, PAT, GitHub App)
2. **Multi-tenancy**: Is multi-tenant isolation required for MVP?
3. **Storage Backend**: Filesystem sufficient for MVP, or need S3-compatible from start?
4. **Runner Communication**: Push (runner connects to scheduler) or pull (scheduler pushes to runner)?

---

## Appendix

### Reference Documents
- `docs/project_notes/BRAINSTORM.md` - Initial brainstorming
- `docs/project_notes/DESIGN.md` - Detailed design specs
- `docs/project_notes/ARCHITECTURE.md` - Architecture decisions
- `docs/project_notes/EXECUTION_PLAN.md` - Implementation order
- `docs/project_notes/HARDENING.md` - Production hardening
- `docs/project_notes/EXPANDED_PLAN.md` - Extended planning

### Conventions
- Follow Rust API guidelines
- Use `anyhow` for error handling
- Use `thiserror` for library errors
- Async traits via `async-trait`
- Structured logging via `tracing`
