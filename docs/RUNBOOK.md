# GitForge Runbook

**Last Updated**: 2026-07-14

## Overview

GitForge is a self-hosted Git platform with event-driven CI/CD capabilities. This runbook covers running and managing GitForge services.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    GitForge Services                      │
├─────────────┬─────────────┬─────────────┬───────────────┤
│  git-server │     ci     │   runner    │      api      │
│  (2222/8082)│  (Internal)│  (Internal) │   (Port 8080) │
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

# Production
export JWT_SECRET
./target/release/api --host 0.0.0.0 --port 8080
```

**Environment Variables:**
- `JWT_SECRET` - Secret for JWT token signing (required)
- `DATABASE_URL` - SQLite or PostgreSQL URL

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
sudo ./target/release/git-server
```

**Ports:**
- SSH: 2222
- HTTP: 8082

### 3. CI Orchestrator

The CI orchestrator manages pipeline execution and job scheduling.

```bash
# Development
cargo run -p ci

# Production
./target/release/ci
```

**Responsibilities:**
- Subscribes to push events from event bus
- Triggers pipeline execution
- Manages job queue via scheduler
- Assigns jobs to runners

### 4. Runner Agent

The runner agent executes jobs in Docker containers.

```bash
# Development
cargo run -p runner

# Production
SCHEDULER_URL=http://localhost:8081 ./target/release/runner
```

**Environment Variables:**
- `RUNNER_NAME` - Runner name (default: runner)
- `SCHEDULER_URL` - Scheduler URL (default: http://localhost:8081)
- `RUNNER_CAPACITY` - Concurrent job capacity (default: 2)
- `HEARTBEAT_INTERVAL_SECS` - Heartbeat interval (default: 30)
- `FETCH_INTERVAL_SECS` - Job fetch interval (default: 5)

## Docker Compose

```bash
# Start all services
docker-compose up -d

# Check health
curl http://localhost:8080/health

# View logs
docker-compose logs -f
```

## Health Checks

```bash
# Check API health
curl http://localhost:8080/health

# Expected response:
# {"status":"healthy","timestamp":"2026-07-14T12:00:00Z","database":"connected"}
```

## Troubleshooting

### Service Won't Start

1. Check ports aren't already in use:
   ```bash
   lsof -i :8080  # API
   lsof -i :2222  # Git SSH
   lsof -i :8082  # Git HTTP
   ```

2. Check logs for errors:
   ```bash
   RUST_LOG=debug cargo run -p api
   ```

### Jobs Not Being Scheduled

1. Verify CI orchestrator is running
2. Check scheduler has runners registered:
   ```bash
   curl -H "Authorization: Bearer $TOKEN" http://localhost:8080/api/runners
   ```
3. Check CI logs for queue processing

### Runner Not Picking Up Jobs

1. Verify runner is registered:
   ```bash
   curl http://localhost:8081/runners  # Scheduler API
   ```
2. Check runner logs for heartbeat errors
3. Verify runner can reach scheduler

### Database Locked

SQLite doesn't support concurrent writes. For multi-runner setups, use PostgreSQL:

```toml
[database]
url = "postgres://gitforge:${POSTGRES_PASSWORD}@localhost:5432/gitforge"
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
port = 8080

[database]
url = "sqlite:/data/gitforge.db"

[auth]
jwt_secret = ""

[runner]
scheduler_url = "http://scheduler:8081"
capacity = 4
```

### Environment Variables

| Variable | Service | Default | Description |
|----------|---------|---------|-------------|
| `JWT_SECRET` | api | - | JWT signing secret (required) |
| `DATABASE_URL` | api | sqlite:/data/gitforge.db | Database URL |
| `RUNNER_NAME` | runner | runner | Runner identifier |
| `SCHEDULER_URL` | runner | http://localhost:8081 | Scheduler endpoint |
| `RUNNER_CAPACITY` | runner | 2 | Max concurrent jobs |
| `SSH_PORT` | git-server | 2222 | SSH port |
| `HTTP_PORT` | git-server | 8082 | HTTP port |

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
