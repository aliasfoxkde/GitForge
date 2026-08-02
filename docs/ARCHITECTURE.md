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
| `gitforce-common` | Shared types, UUIDs, errors, time utilities |
| `gitforce-db` | Database models, connection pool, SQLite queries |
| `gitforce-events` | Event bus, event types, event streaming |
| `gitforce-ci` | Pipeline orchestration, DAG execution |
| `gitforce-scheduler` | Job queue, runner assignment, scheduling policies |
| `gitforce-runner` | Job execution agent, Docker integration |
| `gitforce-sandbox` | Container isolation via Docker |
| `gitforce-storage` | Artifact storage, cache management |
| `gitforce-core` | Git protocol handlers, repository management |
| `gitforce-api` | REST API gateway |

### Services (Binaries)

| Service | Port | Purpose |
|---------|------|---------|
| `api` | 42780 | REST API gateway |
| `ci` | 42781 | CI orchestration + Scheduler HTTP API |
| `git-server` | 42782 (HTTP), 42022 (SSH placeholder) | Git hosting |
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

- `PushReceived` - Git push received
- `PipelineTriggered` - Pipeline started
- `PipelineFinished` - Pipeline completed
- `JobQueued` - Job added to queue
- `JobStarted` - Job execution started
- `JobFinished` - Job execution completed
- `RunnerRegistered` - Runner joined cluster
- `RunnerHeartbeat` - Runner health ping
- `RunnerOffline` - Runner disconnected

## Security

### Authentication

All API endpoints (except `/health`, `/metrics`, `/swagger-ui`, `/api-docs`) require JWT authentication.

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
- `pipelines` - Pipeline definitions
- `pipeline_runs` - Pipeline execution instances
- `jobs` - Individual job executions
- `runners` - Runner agents
- `events` - Event log

## Deployment

GitForge is deployed via Docker Compose:

```yaml
services:
  api:        # REST gateway
  ci:         # Pipeline orchestrator
  scheduler:  # Job queue manager
  runner:     # Job executor (scalable)
  git-server: # Git SSH/HTTP
```

See `docs/DEPLOYMENT.md` for detailed deployment instructions.
