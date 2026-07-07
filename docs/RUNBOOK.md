# GitForge Runbook

**Last Updated**: 2026-07-06

## Overview

GitForge is a self-hosted Git platform with event-driven CI/CD capabilities. This runbook covers running and managing GitForge services.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    GitForge Services                      │
├─────────────┬─────────────┬─────────────┬───────────────┤
│  git-server │     ci     │   runner    │      api      │
│   (Port 22) │  (Internal) │  (Internal) │   (Port 8080) │
└─────────────┴─────────────┴─────────────┴───────────────┘
```

## Prerequisites

- Rust 1.70+ toolchain
- PostgreSQL 15+ (for future production use)
- Docker (for runner sandbox execution)
- 4GB RAM minimum

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

# Production
PORT=8080 JWT_SECRET=your-secret ./target/release/api
```

**Environment Variables:**
- `PORT` - API server port (default: 8080)
- `JWT_SECRET` - Secret for JWT token signing

**Endpoints:**
- `GET /health` - Health check
- `GET /api/v1/repos` - List repositories
- `POST /api/v1/repos` - Create repository
- `GET /api/v1/repos/:id` - Get repository
- `DELETE /api/v1/repos/:id` - Delete repository
- `GET /api/v1/pipelines` - List pipelines
- `GET /api/v1/pipeline-runs` - List pipeline runs
- `GET /api/v1/pipeline-runs/:id` - Get pipeline run
- `GET /api/v1/jobs/:id` - Get job status
- `GET /api/v1/jobs/:id/logs` - Get job logs
- `GET /api/v1/runners` - List runners
- `POST /api/v1/runners` - Register runner
- `GET /api/v1/artifacts` - List artifacts

### 2. Git Server

The Git server handles Git protocol over SSH and HTTP.

```bash
# Development
cargo run -p git-server

# Production (requires root for port 22)
sudo ./target/release/git-server
```

**Note:** The MVP implementation logs hooks but doesn't actually start Git servers.

### 3. CI Orchestrator

The CI orchestrator manages pipeline execution and job scheduling.

```bash
# Development
cargo run -p ci

# Production
./target/release/ci
```

**Responsibilities:**
- Subscribes to push events
- Triggers pipeline execution
- Manages job queue
- Assigns jobs to runners

### 4. Runner Agent

The runner agent executes jobs in Docker containers.

```bash
# Development
cargo run -p runner

# Production
./target/release/runner
```

**Environment Variables:**
- `RUNNER_NAME` - Runner name (default: runner)
- `SCHEDULER_URL` - Scheduler URL (default: http://localhost:8080)
- `CAPACITY` - Concurrent job capacity (default: 2)

## Docker Compose (Development)

```yaml
version: '3.8'
services:
  api:
    build: .
    ports:
      - "8080:8080"
    environment:
      - JWT_SECRET=dev-secret
    depends_on:
      - git-server
      - ci

  git-server:
    build: .
    ports:
      - "2222:22"
    volumes:
      - git-data:/var/lib/gitforce

  ci:
    build: .
    depends_on:
      - git-server

  runner:
    build: .
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
    environment:
      - SCHEDULER_URL=http://ci:8080

volumes:
  git-data:
```

## Health Checks

All services expose health information via structured logging.

```bash
# Check API health
curl http://localhost:8080/health

# Expected response:
# {"status":"healthy","timestamp":"2026-07-06T12:00:00Z"}
```

## Troubleshooting

### Service Won't Start

1. Check ports aren't already in use:
   ```bash
   lsof -i :8080  # API
   lsof -i :22    # Git SSH
   ```

2. Check logs for errors:
   ```bash
   RUST_LOG=debug cargo run -p <service>
   ```

### Jobs Not Being Scheduled

1. Verify CI orchestrator is running
2. Check scheduler has runners registered:
   ```bash
   curl http://localhost:8080/api/v1/runners
   ```
3. Check CI logs for queue processing

### Runner Not Picking Up Jobs

1. Verify runner is registered:
   ```bash
   curl http://localhost:8080/api/v1/runners
   ```
2. Check runner logs for heartbeat errors
3. Verify runner can reach CI orchestrator

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
cargo clippy -- -D warnings

# Format
cargo fmt
```

### Adding a New Crate

1. Create crate under `crates/`
2. Add to workspace `Cargo.toml`
3. Add dependencies to workspace root
4. Create lib.rs and mod.rs files

## Configuration

### Environment Variables

| Variable | Service | Default | Description |
|----------|---------|---------|-------------|
| `PORT` | api | 8080 | API server port |
| `JWT_SECRET` | api | dev-secret | JWT signing secret |
| `RUNNER_NAME` | runner | runner | Runner identifier |
| `SCHEDULER_URL` | runner | http://localhost:8080 | Scheduler endpoint |
| `CAPACITY` | runner | 2 | Max concurrent jobs |
| `GIT_ROOT` | git-server | /var/lib/gitforce/repos | Git repository storage |

## Logging

Services use `tracing` for structured logging.

```bash
# Set log level
RUST_LOG=debug cargo run -p api

# JSON logging for production
RUST_LOG=json cargo run -p api
```

## Metrics (Future)

Planned Prometheus metrics endpoint at `/metrics`:

- `gitforge_jobs_total` - Total jobs executed
- `gitforge_jobs_running` - Currently running jobs
- `gitforge_queue_length` - Jobs waiting for runner
- `gitforge_runner_heartbeats` - Runner heartbeat count
