# GitForge Architecture

## Overview

GitForge is a self-hosted Git platform with CI/CD capabilities, built in Rust. It provides Git hosting, pipeline automation, and job execution through a modular microservice architecture.

## System Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                         GitForge Platform                              │
├──────────────────────────────────────────────────────────────────────┤
│                                                                       │
│  ┌─────────────┐      ┌─────────────┐      ┌─────────────┐         │
│  │   Client     │─────▶│  API Server │─────▶│  Database   │         │
│  │  (curl/CLI) │      │  (Axum)     │      │  (SQLite)   │         │
│  └─────────────┘      └─────────────┘      └─────────────┘         │
│                              │                                       │
│                              ▼                                       │
│  ┌─────────────┐      ┌─────────────┐      ┌─────────────┐         │
│  │ Git Server  │◀────▶│  CI Service │◀────▶│  Scheduler  │         │
│  │  (SSH/HTTP) │      │             │      │             │         │
│  └─────────────┘      └─────────────┘      └─────────────┘         │
│                              │                    │                  │
│                              │                    │                  │
│                              ▼                    ▼                  │
│                       ┌─────────────┐      ┌─────────────┐         │
│                       │   Runner    │◀────▶│   Docker    │         │
│                       │  (Agent)    │      │  (bollard)  │         │
│                       └─────────────┘      └─────────────┘         │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

## Components

### Crates (Library Code)

| Crate | Purpose |
|-------|---------|
| `gitforge-common` | Shared types, UUIDs, errors, time utilities |
| `gitforge-db` | Database models, connection pool, SQLite queries |
| `gitforge-events` | Event bus, event types, event streaming |
| `gitforge-ci` | Pipeline orchestration, DAG execution |
| `gitforge-process` | Process supervision: subreaper, SIGCHLD handling, process pools, cgroup limits |
| `gitforge-build` | Build queue client/daemon with semaphore-based build concurrency |
| `gitforge-scheduler` | Job queue, runner assignment, scheduling policies |
| `gitforge-runner` | Job execution agent, Docker integration |
| `gitforge-sandbox` | Container isolation via Docker |
| `gitforge-storage` | Artifact storage, cache management |
| `gitforge-core` | Git protocol handlers, repository management |
| `gitforge-api` | REST API gateway |
| `gitforge-ai` | AI provider abstraction for code review (Anthropic, OpenAI, Ollama) |
| `gitforge-review` | Review domain: diff parsing, security findings, run-state contract |
| `gitforge-cli` | The `gitforge` command-line client |

### Services (Binaries)

| Service | Port | Purpose |
|---------|------|---------|
| `api` | 42780 | REST API gateway |
| `ci` | 42781 | CI orchestration + Scheduler HTTP API |
| `git-server` | 42782 (HTTP), 42022 (SSH) | Git hosting |
| `runner` | - | Job execution agent |

## Data Flow

### Push to Pipeline Trigger

```
Git Push → Git Server → PushReceived Event → Event Bus
                                              │
                                              ▼
                                    CI Service (Event Consumer)
                                              │
                                              ▼
                                    Pipeline Engine (DAG build)
                                              │
                                              ▼
                                    Scheduler (Job enqueue)
                                              │
                                              ▼
                                    Runner (Job fetch & execute)
                                              │
                                              ▼
                                    Docker Container (Job run)
```

### API Request Flow

```
Client → API Server → Auth Middleware → Route Handler
                    │                      │
                    │                      ▼
                    │              Database (SQLite)
                    │
                    ▼
              Response
```

## Event System

The event system uses an in-memory broadcast channel:

- `RepoCreated` - Repository created
- `RepoDeleted` - Repository deleted
- `PushReceived` - Git push received
- `RefUpdated` - Git ref updated
- `PipelineTriggered` - Pipeline triggered
- `PipelineStarted` - Pipeline started
- `PipelineFinished` - Pipeline completed
- `JobQueued` - Job added to queue
- `JobStarted` - Job execution started
- `JobFinished` - Job execution completed
- `ArtifactCreated` - Job artifact stored
- `RunnerRegistered` - Runner joined cluster
- `RunnerHeartbeat` - Runner health ping
- `RunnerOffline` - Runner disconnected
- `MirrorSyncRequested` - Mirror synchronization requested
- `MirrorSyncCompleted` - Mirror synchronization finished

## Security

### Authentication

All API endpoints require JWT authentication except the public set: `/health`, `/metrics`, `/swagger-ui`, `/api-docs/openapi.json`, `GET /dashboard`, `POST /api/runners` (runner registration), `POST /auth/login`, and `GET /auth/status`.

Token format:
- Algorithm: HS256
- Expiry: 24 hours
- Claims: user_id, username, role

### CORS

Configurable CORS origins. Default allows any origin in development.

## Database Schema

### Core Tables

- `users` - User accounts
- `repositories` - Git repositories
- `ssh_keys` - Public keys registered to accounts for Git-over-SSH auth
- `pipelines` - Pipeline definitions
- `pipeline_runs` - Pipeline execution instances
- `jobs` - Individual job executions
- `runners` - Runner agents
- `artifacts` - Job artifact metadata
- `review_runs` - AI code review runs and their lifecycle state
- `review_findings` - Findings produced by a review run
- `events` - Event log

The runtime SQLite migrations in `gitforge-db` also create `job_log_chunks`,
`job_idempotency_keys`, and `publication_outbox`. `mirror_states` exists only
in the legacy PostgreSQL migration under `migrations/` and is not created by
the runtime SQLite migrations.

## Deployment

GitForge is deployed via Docker Compose:

```yaml
services:
  api:        # REST gateway
  ci:         # Pipeline orchestrator (hosts the scheduler HTTP API)
  runner:     # Job executor (scalable)
  git-server: # Git SSH/HTTP
```

See `docs/DEPLOYMENT.md` for detailed deployment instructions.

## AI Code Review

Code review is split across two crates. `gitforge-ai` defines the provider
abstraction, with implementations for Anthropic (Claude), OpenAI, and local
Ollama behind a shared trait. `gitforge-review` owns the review domain: diff
parsing, a rule-based security scanner, and the typed run-state and finding
fingerprint contract from `docs/architecture/adr-20260905-code-review-contract.md`.

Review runs are persisted in `review_runs` and exposed through the API gateway
at `POST /api/review-runs` (submit, with an idempotency key),
`GET /api/review-runs/{id}` (status), and
`GET /api/review-runs/{id}/findings` (findings). Submission creates a `pending`
run; provider execution and worker dispatch are not wired up yet, so that row
is the seam a future worker claims.
