Below is a **cohesive system definition package** for GitForce consisting of:

1. Rust workspace crate layout (production-grade modular structure)
2. Postgres schema (normalized, CI-aware, event-driven)
3. Strict event/message schema (versioned, strongly typed, extensible)

Everything is designed to be consistent across all three layers.

---

# 1. RUST WORKSPACE CRATE LAYOUT

## 1.1 Top-level workspace

```text
gitforce/
│
├── Cargo.toml
├── Cargo.lock
├── README.md
│
├── crates/
│   ├── gitforce-core/
│   ├── gitforce-api/
│   ├── gitforce-auth/
│   ├── gitforce-events/
│   ├── gitforce-ci/
│   ├── gitforce-scheduler/
│   ├── gitforce-runner/
│   ├── gitforce-sandbox/
│   ├── gitforce-storage/
│   ├── gitforce-mirror/
│   ├── gitforce-common/
│   ├── gitforce-db/
│   ├── gitforce-observability/
│   └── gitforce-config/
│
├── services/
│   ├── api-gateway/
│   ├── git-server/
│   ├── ci-orchestrator/
│   ├── runner-agent/
│   └── mirror-worker/
│
├── migrations/
├── scripts/
└── infra/
```

---

## 1.2 Crate responsibilities

### gitforce-common

Shared primitives:

* error types
* result wrappers
* ID types (RepoId, JobId, etc.)
* time utilities
* serialization helpers

---

### gitforce-core

Git server implementation:

* SSH server
* HTTP git protocol
* repo management
* refs handling
* hook execution
* filesystem repository storage

---

### gitforce-api

External API layer:

* REST/GraphQL endpoints
* request validation
* DTO mapping
* auth middleware integration

---

### gitforce-auth

Authentication system:

* JWT issuance/validation
* OAuth2 hooks (optional)
* RBAC engine
* permission evaluation

---

### gitforce-events

Event system:

* event definitions
* serialization
* publish/subscribe abstraction
* event bus adapters

---

### gitforce-ci

CI orchestration engine:

* pipeline parsing
* DAG construction
* job state machine
* execution planning

---

### gitforce-scheduler

Job scheduling:

* queue management
* priority handling
* worker assignment
* retry logic

---

### gitforce-runner

Runner agent:

* job execution runtime
* communication with scheduler
* sandbox interface integration
* log streaming

---

### gitforce-sandbox

Isolation layer:

* Docker backend
* Firecracker abstraction (future)
* process isolation fallback
* resource enforcement

---

### gitforce-storage

Artifact + cache system:

* object storage abstraction
* filesystem + MinIO support
* artifact lifecycle management
* cache keys + retrieval

---

### gitforce-mirror

GitHub sync system:

* repo mirroring
* branch mapping
* retry queue
* consistency validation

---

### gitforce-db

Database abstraction layer:

* repository models
* job models
* pipeline models
* migrations interface

---

### gitforce-observability

* logging (structured)
* metrics (Prometheus)
* tracing (OpenTelemetry)

---

### gitforce-config

* configuration loader
* environment parsing
* feature flags

---

## 1.3 Services (binary targets)

### git-server

* wraps gitforce-core
* exposes SSH + HTTP

### api-gateway

* wraps gitforce-api

### ci-orchestrator

* runs gitforce-ci + scheduler

### runner-agent

* executes jobs

### mirror-worker

* handles GitHub sync asynchronously

---

# 2. POSTGRES DATABASE SCHEMA (DDL)

This schema is normalized for:

* CI execution tracking
* Git metadata
* event correlation
* runner management

---

## 2.1 USERS & AUTH

```sql
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
```

---

## 2.2 REPOSITORIES

```sql
CREATE TABLE repositories (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    owner_id UUID REFERENCES users(id),
    visibility TEXT NOT NULL,
    git_path TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_repositories_owner ON repositories(owner_id);
```

---

## 2.3 COMMITS / REFS

```sql
CREATE TABLE repo_refs (
    id UUID PRIMARY KEY,
    repo_id UUID REFERENCES repositories(id),
    ref_name TEXT NOT NULL,
    commit_hash TEXT NOT NULL,
    updated_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_repo_refs_repo ON repo_refs(repo_id);
```

---

## 2.4 PIPELINES

```sql
CREATE TABLE pipelines (
    id UUID PRIMARY KEY,
    repo_id UUID REFERENCES repositories(id),
    name TEXT NOT NULL,
    trigger_type TEXT NOT NULL,
    config JSONB NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_pipelines_repo ON pipelines(repo_id);
```

---

## 2.5 PIPELINE RUNS

```sql
CREATE TABLE pipeline_runs (
    id UUID PRIMARY KEY,
    pipeline_id UUID REFERENCES pipelines(id),
    repo_id UUID REFERENCES repositories(id),
    status TEXT NOT NULL,
    triggered_by TEXT NOT NULL,
    commit_hash TEXT NOT NULL,
    started_at TIMESTAMP,
    finished_at TIMESTAMP
);

CREATE INDEX idx_pipeline_runs_pipeline ON pipeline_runs(pipeline_id);
```

---

## 2.6 JOBS

```sql
CREATE TABLE jobs (
    id UUID PRIMARY KEY,
    pipeline_run_id UUID REFERENCES pipeline_runs(id),
    name TEXT NOT NULL,
    status TEXT NOT NULL,
    runner_id UUID,
    started_at TIMESTAMP,
    finished_at TIMESTAMP,
    retry_count INT DEFAULT 0
);

CREATE INDEX idx_jobs_run ON jobs(pipeline_run_id);
```

---

## 2.7 RUNNERS

```sql
CREATE TABLE runners (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    last_heartbeat TIMESTAMP,
    capacity INT NOT NULL
);
```

---

## 2.8 ARTIFACTS

```sql
CREATE TABLE artifacts (
    id UUID PRIMARY KEY,
    job_id UUID REFERENCES jobs(id),
    path TEXT NOT NULL,
    checksum TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
```

---

## 2.9 EVENTS (EVENT LOG STORE)

```sql
CREATE TABLE events (
    id UUID PRIMARY KEY,
    event_type TEXT NOT NULL,
    payload JSONB NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_events_type ON events(event_type);
```

---

## 2.10 MIRROR STATE

```sql
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

# 3. MESSAGE / EVENT SCHEMA (STRICT TYPING)

All events are:

* versioned
* immutable
* JSON-serializable
* strongly typed
* append-only

---

## 3.1 Base Event Envelope

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

---

## 3.2 Event Types

```rust
enum EventType {
    RepoCreated,
    RepoDeleted,
    PushReceived,
    RefUpdated,
    PipelineTriggered,
    PipelineStarted,
    PipelineFinished,
    JobQueued,
    JobStarted,
    JobFinished,
    ArtifactCreated,
    RunnerRegistered,
    RunnerHeartbeat,
    MirrorSyncRequested,
    MirrorSyncCompleted,
}
```

---

## 3.3 Payload Definitions

### RepoCreated

```rust
struct RepoCreatedPayload {
    repo_id: Uuid,
    name: String,
    owner_id: Uuid,
    visibility: String,
}
```

---

### PushReceived

```rust
struct PushReceivedPayload {
    repo_id: Uuid,
    ref_name: String,
    old_hash: String,
    new_hash: String,
    pusher_id: Option<Uuid>,
}
```

---

### PipelineTriggered

```rust
struct PipelineTriggeredPayload {
    pipeline_id: Uuid,
    repo_id: Uuid,
    commit_hash: String,
    trigger_source: String,
}
```

---

### JobStarted

```rust
struct JobStartedPayload {
    job_id: Uuid,
    runner_id: Uuid,
    started_at: i64,
}
```

---

### JobFinished

```rust
struct JobFinishedPayload {
    job_id: Uuid,
    status: String,
    exit_code: i32,
    duration_ms: u64,
}
```

---

### ArtifactCreated

```rust
struct ArtifactCreatedPayload {
    artifact_id: Uuid,
    job_id: Uuid,
    path: String,
    checksum: String,
    size_bytes: u64,
}
```

---

### RunnerHeartbeat

```rust
struct RunnerHeartbeatPayload {
    runner_id: Uuid,
    capacity_used: u32,
    active_jobs: u32,
}
```

---

### MirrorSyncCompleted

```rust
struct MirrorSyncCompletedPayload {
    repo_id: Uuid,
    github_repo: String,
    commit_hash: String,
    success: bool,
}
```

---

## 3.4 Serialization Rule

All events MUST serialize as:

```json
{
  "event_id": "...",
  "event_type": "...",
  "event_version": 1,
  "timestamp": 0,
  "repo_id": "...",
  "actor_id": "...",
  "correlation_id": "...",
  "payload": {}
}
```

---

## 3.5 Event Guarantees

* Append-only log
* Idempotent processing required
* At-least-once delivery
* Deduplication by `event_id`
* Ordered only per repository scope (not global)

---

# FINAL SYSTEM INTEGRATION BOUNDARY

All three layers connect as:

```text
Git Server (Rust core)
        ↓
Postgres (state)
        ↓
Event Log (append-only)
        ↓
CI Orchestrator
        ↓
Scheduler
        ↓
Runner Agents (sandbox)
        ↓
Artifacts / Cache
        ↓
Mirror Worker (GitHub sync)
```

---

If you want the next level of implementation detail, the natural next step is:

* full Rust trait definitions per crate
* async execution model (Tokio task graph)
* or a minimal MVP build sequence that reduces this entire system to a 1–2 month buildable core

Just specify direction.
