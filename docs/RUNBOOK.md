# GitForge Runbook

**Last Updated**: 2026-08-29

## Overview

GitForge is a self-hosted Git platform with event-driven CI/CD capabilities. This runbook covers running and managing GitForge services.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    GitForge Services                      │
├─────────────┬─────────────┬─────────────┬───────────────┤
│  git-server │     ci     │   runner    │      api      │
│ (42022/42782)│ (42781)   │  (Dynamic)  │  (Port 42780) │
└─────────────┴─────────────┴─────────────┴───────────────┘
```

## Prerequisites

- Rust 1.80+ toolchain
- Docker (for runner sandbox execution)
- 4GB RAM minimum
- 20GB disk space

## Building

```bash
# Build all services
cargo build --workspace

# Build release binaries
cargo build --release --workspace
```

## Running Services

### 1. API Gateway

The API gateway exposes the REST API for frontend integration.

```bash
# Development
cargo run -p api

# Production (configure through environment; the binary does not parse CLI
# host/port flags)
JWT_SECRET=your-secret PORT=42780 ./target/release/api
```

**Environment Variables:**
- `JWT_SECRET` - Secret for JWT token signing (required)
- `PORT` - API listen port (default: `42780`)
- `DATABASE_URL` - SQLite or PostgreSQL URL
- `GITFORGE_CI_TRIGGER_URL` - CI trigger endpoint used by Git-server after a successful push
- `GITFORGE_CI_TRIGGER_TOKEN` - bearer token matching CI's `GITFORGE_TRIGGER_TOKEN`

**Endpoints:**
- `GET /health` - Health check (public)
- `GET /metrics` - Prometheus metrics (public)
- `GET /swagger-ui` - API documentation (public)
- `POST /api/repos` - Create repository (auth required)
- `GET /api/repos` - List repositories (auth required)
- `GET /api/pipelines` - List pipelines (auth required)
- `GET /api/pipeline-runs` - List pipeline runs (auth required)
- `GET /api/jobs/:id` - Get job status (auth required)
- `GET /api/runners` - List runners (auth required)

### 2. Git Server

The Git server handles Git protocol over SSH and HTTP.

```bash
# Development
cargo run -p git-server

# Production
./target/release/git-server
```

**Ports:**
- SSH: 42022
- HTTP: 42782

### 3. CI Orchestrator (includes Scheduler)

The CI orchestrator manages pipeline execution and job scheduling. The scheduler HTTP API runs within this service on port 42781.

```bash
# Development
cargo run -p ci

# Production
./target/release/ci
```

**Responsibilities:**
- Subscribes to push events from event bus
- Triggers pipeline execution
- Hosts scheduler HTTP API on port 42781
- Assigns jobs to runners

### 4. Runner Agent

The runner agent executes jobs in Docker containers.

```bash
# Development — GITFORGE_SCHEDULER_URL is required
GITFORGE_SCHEDULER_URL=http://localhost:42781 cargo run -p runner

# Production
GITFORGE_SCHEDULER_URL=http://ci:42781 \
GITFORGE_RUNNER_NAME=prod-runner-01 \
GITFORGE_RUNNER_CAPACITY=4 \
GITFORGE_SCHEDULER_TOKEN=<token> \
./target/release/runner
```

**Environment Variables:**

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `GITFORGE_SCHEDULER_URL` | **Yes** | — | Scheduler HTTP endpoint (e.g. `http://localhost:42781`). Startup fails without this. |
| `GITFORGE_RUNNER_NAME` | No | `runner` | Display name for this runner instance |
| `GITFORGE_RUNNER_CAPACITY` | No | `2` | Maximum concurrent jobs |
| `GITFORGE_HEARTBEAT_INTERVAL` | No | `30` | Heartbeat interval in seconds |
| `GITFORGE_FETCH_INTERVAL` | No | `5` | Job-poll interval in seconds |
| `GITFORGE_SCHEDULER_TOKEN` | No | _(none)_ | Bearer token for scheduler API authentication |

> **Startup behavior**: If `GITFORGE_SCHEDULER_URL` is missing or empty, the runner exits immediately
> with a clear error message. Invalid values for numeric variables (non-integer) also cause a fast
> failure. Safe defaults apply to all optional variables when they are unset.

## Docker Compose

```bash
# Start all services
docker-compose up -d

# Check health
curl http://localhost:42780/health
curl http://localhost:42781/health  # CI/Scheduler

# Read-only Fedora service and endpoint report (user-level systemd)
./scripts/gitforge-status
./scripts/gitforge-status --json

# View logs
docker-compose logs -f
```

## Health Checks

```bash
# Check API health
curl http://localhost:42780/health

# Expected response:
# {"status":"healthy","timestamp":"2026-07-14T12:00:00Z","database":"connected"}
```

## Troubleshooting

### Service Won't Start

1. Check ports aren't already in use:
   ```bash
   lsof -i :42780  # API
   lsof -i :42022  # Git SSH
   lsof -i :42782  # Git HTTP
   ```

2. Check logs for errors:
   ```bash
   RUST_LOG=debug cargo run -p api
   ```

### Jobs Not Being Scheduled

1. Verify CI orchestrator is running
2. Check scheduler has runners registered:
   ```bash
   curl -H "Authorization: Bearer $TOKEN" http://localhost:42780/api/runners
   ```
3. Check CI logs for queue processing

### Runner Not Picking Up Jobs

1. Verify runner is registered:
   ```bash
   curl -H "Authorization: Bearer $GITFORGE_RUNNER_TOKEN" \
     http://localhost:42781/runners  # Scheduler API
   ```
2. Check runner logs for heartbeat errors
3. Verify runner can reach scheduler

### Safely Submit or Cancel a Job

Use the operator credential for control-plane actions. Always supply a stable
idempotency key when submitting so retries cannot duplicate work:

```bash
curl -X POST http://localhost:42781/jobs \
  -H "Authorization: Bearer $GITFORGE_SCHEDULER_OPERATOR_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"pipeline_run_id":"<run-id>","repo_id":"<repo-id>","commands":["cargo test"],"working_dir":null,"idempotency_key":"<attempt-id>"}'

curl -X POST http://localhost:42781/jobs/<job-id>/cancel \
  -H "Authorization: Bearer $GITFORGE_SCHEDULER_OPERATOR_TOKEN"
```

Do not use the operator credential in runners. The shared token remains only
as a backward-compatible migration fallback.

### Database Locked

SQLite doesn't support concurrent writes. For multi-runner setups, use PostgreSQL:

```toml
[database]
url = "postgres://gitforge:password@localhost:5432/gitforge"
```

## Development

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p gitforce-ci

# Run with output
RUST_LOG=debug cargo test -p gitforce-events
```

### Code Quality

```bash
# Lint
cargo fmt --check
cargo clippy --workspace -- -D warnings

# Format
cargo fmt
```

## Configuration

### config.toml

```toml
[server]
host = "0.0.0.0"
port = 42780

[database]
url = "sqlite:/data/gitforge.db"

[auth]
jwt_secret = "your-secret-here"

[runner]
scheduler_url = "http://ci:42781"
capacity = 4
```

### Environment Variables

| Variable | Service | Default | Description |
|----------|---------|---------|-------------|
| `JWT_SECRET` | api | - | JWT signing secret (required) |
| `DATABASE_URL` | api | sqlite:/data/gitforge.db | Database URL |
| `RUNNER_NAME` | runner | runner | Runner identifier |
| `SCHEDULER_URL` | runner | http://localhost:42781 | Scheduler endpoint |
| `RUNNER_CAPACITY` | runner | 2 | Max concurrent jobs |
| `SSH_PORT` | git-server | 42022 | SSH port |
| `HTTP_PORT` | git-server | 42782 | HTTP port |

## Logging

Services use `tracing` for structured logging.

```bash
# Set log level
RUST_LOG=debug cargo run -p api

# JSON logging for production
RUST_LOG=json cargo run -p api
```

## Metrics

Prometheus metrics available at `/metrics`:

- `gitforge_http_requests_total` - HTTP request count by method and path
- `gitforge_job_duration_seconds` - Job execution duration
- `gitforge_runners_online` - Number of online runners
- `gitforge_pipeline_runs_total` - Pipeline runs by status
- `gitforge_artifact_size_bytes` - Artifact sizes

## Backup

```bash
# Backup database
docker-compose cp api:/data/gitforge.db ./backup/

# Backup artifacts
docker-compose cp api:/data/artifacts ./backup/
```

## Scaling Runners

```bash
# Scale horizontally
docker-compose up -d --scale runner=3
```

## Security Checklist

- [ ] Change JWT secret from default
- [ ] Configure CORS origins
- [ ] Use PostgreSQL for production
- [ ] Set up TLS reverse proxy
- [ ] Configure firewall rules
- [ ] Enable rate limiting
