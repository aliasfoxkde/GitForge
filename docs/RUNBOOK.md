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
- `DATABASE_URL` - SQLite database URL (e.g. `sqlite:/data/gitforge.db?mode=rwc`)
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

**Git over SSH:** the server runs an in-process SSH transport (russh) that
requires public-key authentication against a per-user key registry. On
first boot it generates an ed25519 host key at `GITFORGE_SSH_HOST_KEY`
(default `$HOME/.ssh/gitforge_host_ed25519`) and publishes the public half
as `<path>.pub` for `known_hosts` pinning. In compose the key lives on the
`ssh-data` volume, so it survives restarts.

A client key must be registered to an account before it can connect
(`POST /api/ssh-keys` with `name` and the OpenSSH `public_key` line; the
transport matches the presented key's `SHA256:` fingerprint against the
registry and rejects everything else). List your keys with
`GET /api/ssh-keys` and remove one with `DELETE /api/ssh-keys/{id}`.

```bash
# Register a key, then clone over SSH after pinning the host key
curl -X POST http://localhost:42780/api/ssh-keys \
  -H "Authorization: Bearer $JWT" -H 'Content-Type: application/json' \
  -d '{"name":"laptop","public_key":"ssh-ed25519 AAAA... you@host"}'
git clone "ssh://gitforge@localhost:42022/<owner>/<repo>.git"
```

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

Each workspace-prep git command (adopt checkout, clean, clone, checkout)
runs under a wall-clock budget; a hung child is killed and the run is
graded `failed` with the cause recorded on the run row (visible as
`error` in the run's API/CLI output).

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `GITFORGE_WORKSPACE_PREP_TIMEOUT_SECS` | No | `300` | Per-command budget for workspace prep. Raise it on hosts that legitimately clone multi-gigabyte repositories; `0`/invalid values fall back to the default. |

Failed runs carry their cause in the `error` field of
`GET /api/v1/ci/pipeline-runs/{id}`, and a run that ends `failed` or
`cancelled` sweeps its never-dispatched (`pending`/`queued`) job rows to
`cancelled`, so no zombie rows outlive a terminal run.

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
| `GITFORGE_RUNNER_NAME` | Recommended | `runner` | Stable unique identity; restarts refresh this row instead of creating another one |
| `GITFORGE_RUNNER_CAPACITY` | No | `2` | Maximum concurrent jobs |
| `GITFORGE_HEARTBEAT_INTERVAL` | No | `30` | Heartbeat interval in seconds |
| `GITFORGE_JOB_HEARTBEAT_INTERVAL` | No | `15` | Per-job lease-heartbeat interval in seconds (#243): sent while a job runs to renew its fence grace |
| `GITFORGE_FETCH_INTERVAL` | No | `5` | Job-poll interval in seconds |
| `GITFORGE_SCHEDULER_TOKEN` | No | _(none)_ | Bearer token for scheduler API authentication |
| `GITFORGE_REGISTER_ATTEMPTS` | No | `6` | Registration attempts before giving up when the scheduler is unreachable |
| `GITFORGE_REGISTER_BACKOFF_SECS` | No | `1` | Initial registration retry delay; doubles per attempt up to 30s |

> **Startup behavior**: If `GITFORGE_SCHEDULER_URL` is missing or empty, the runner exits immediately
> with a clear error message. Invalid values for numeric variables (non-integer) also cause a fast
> failure. Safe defaults apply to all optional variables when they are unset. When the scheduler is
> merely unreachable or answering 503, registration retries up to `GITFORGE_REGISTER_ATTEMPTS`
> times with exponential backoff before the fail-closed exit; credential rejections (401/403) are
> never retried.

Runner names are durable identities. Set a distinct name for every concurrently
running runner (for example, `remote-podman-runner-01`); leaving the default
`runner` name on multiple instances makes them contend for one registry row.
Historical stale rows are retained for audit and can be retired through the
authenticated runner-retirement operation after confirming that they own no
active jobs.

## Release Cycling

Releases are bundle-cut, not tag-cut: a release is a directory under
`releases/` plus the `releases/gitforge-current` symlink the systemd drop-in
pins. The full cycle is mechanical:

```bash
# 1. Gate: refuse to cut unless the live instance graded the exact
#    source commit with a full, honest pipeline run (job rows must cover
#    the persisted definition, name for name).
scripts/gitforge-release-gate <40-hex-source-commit>

# 2. Bundle: preflight + build release binaries + MANIFEST.sha256 + READY.
GITFORGE_SOURCE_COMMIT=<sha> \
  scripts/gitforge-release-bundle /nas/Temp/repos/GitForge \
  /nas/Temp/repos/GitForge/releases gitforge-<shortsha>-<YYYYMMDD>

# 3. Promote + restart, drain-gated and rollback-ready:
./scripts/gitforge-release-auto            # dry-run: prints the plan
./scripts/gitforge-release-auto --apply    # or AUTO_UPDATE=1
```

`gitforge-release-auto` refuses to run twice concurrently (flock on
`releases/.auto-update.lock`), defers while any `gitforge-job-*` container is
still running (drain gate — pending-only runs are just queued and do not
block), flips `gitforge-current` atomically, restarts the `gitforge@*` units,
and leaves the previous bundle in place for instant rollback (re-point the
symlink and restart). Verify after cutover: `systemctl` unit states, `/health`
on :42780/:42781/:42782, and `scripts/gitforge-status` with
`GITFORGE_RELEASE_ROOT=/nas/Temp/repos/GitForge/releases/gitforge-current`
(the default path reports false drift on this instance).

Known boot behavior: the ci unit runs migrations, then the startup workspace
sweep, then spawns the event consumer. Triggers published while the consumer
is down or busy are durable and land once it drains — a `202` with a null
`pipeline_run_id` means queued, not lost; re-query before re-triggering.

## Docker Compose

Before `docker compose up`, set the required deployment variables in `.env`
(see `.env.example`):

| Variable | Why it is required |
|----------|--------------------|
| `GITFORGE_SCHEDULER_TOKEN` | Shared scheduler/trigger credential; scheduler auth is fail-closed without it |
| `DOCKER_GID` | Host group id of `/var/run/docker.sock` (`stat -c %g /var/run/docker.sock`) so the non-root runner can use the mounted socket |
| `GITFORGE_WORKSPACE_HOST_DIR` | Host directory (create it first) where CI checks out run workspaces; bind-mounted at the same absolute path in ci and runner because the runner passes workspace paths to the host Docker daemon as bind sources |

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

1. Verify runner is registered. The runner list is served by the API gateway;
   the scheduler on 42781 only exposes `POST /runners` (registration and
   heartbeat), so query the gateway:
   ```bash
   curl -H "Authorization: Bearer $TOKEN" http://localhost:42780/api/runners
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

The database is SQLite (`DATABASE_URL`, e.g.
`sqlite:/data/gitforge.db?mode=rwc`); there is no PostgreSQL backend. SQLite
serializes writers, so bursts of concurrent writes surface as
`database is locked`. Keep the write path short and add runners gradually —
the scheduler queue absorbs bursts better than extra direct writers.

## Development

### Running Tests

```bash
# Run all tests
cargo test --workspace

# Run specific crate tests
cargo test -p gitforge-ci

# Run with output
RUST_LOG=debug cargo test -p gitforge-events
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

Services are configured exclusively through environment variables; there is no
config file. The `gitforge` CLI keeps its own client configuration (server URL
and auth token), which it writes after `gitforge auth --login`.

### Environment Variables

| Variable | Service | Default | Description |
|----------|---------|---------|-------------|
| `JWT_SECRET` | api | - | JWT signing secret (required) |
| `DATABASE_URL` | api | sqlite:/data/gitforge.db | Database URL |
| `GITFORGE_RUNNER_NAME` | runner | runner | Stable unique runner identifier; distinct for concurrent instances |
| `GITFORGE_SCHEDULER_URL` | runner | — | Scheduler endpoint (required) |
| `GITFORGE_RUNNER_CAPACITY` | runner | 2 | Max concurrent jobs |
| `GITFORGE_HEARTBEAT_INTERVAL` | runner | 30 | Heartbeat interval in seconds |
| `GITFORGE_JOB_HEARTBEAT_INTERVAL` | runner | 15 | Per-job lease-heartbeat interval in seconds (#243) |
| `GITFORGE_FETCH_INTERVAL` | runner | 5 | Job-poll interval in seconds |
| `GITFORGE_SCHEDULER_TOKEN` | runner | _(none)_ | Bearer token for scheduler API. Required in compose: the scheduler rejects runner/operator requests when unset |
| `GITFORGE_RUNNER_STANDALONE` | runner | `deny` | `deny` exits the runner when scheduler registration fails; `allow` falls back to standalone execution |
| `GITFORGE_REGISTER_ATTEMPTS` | runner | `6` | Registration attempts against an unreachable scheduler before giving up |
| `GITFORGE_REGISTER_BACKOFF_SECS` | runner | `1` | Initial registration backoff; doubles per failed attempt up to a 30s cap. Auth rejections (401/403) are never retried |
| `SSH_PORT` | git-server | 42022 | SSH port |
| `HTTP_PORT` | git-server | 42782 | HTTP port |
| `GITFORGE_SSH_HOST_KEY` | git-server | `$HOME/.ssh/gitforge_host_ed25519` | ed25519 host key path; generated on first boot, persisted on the `ssh-data` volume, published as `.pub` for `known_hosts` pinning |

**SSH key registry:** git-over-SSH accepts only public keys registered to
an account through `POST /api/ssh-keys` (JWT required). Authentication
matches the presented key's OpenSSH fingerprint; unregistered keys are
rejected and the connection fails with `Permission denied (publickey)`.
If the registry is unreachable, connections are refused rather than
allowed through.

## Logging

Services use `tracing` for structured logging.

```bash
# Set log level
RUST_LOG=debug cargo run -p api
```

`RUST_LOG` is a `tracing` `EnvFilter` directive, not a format switch: it
selects which targets and levels are emitted (`info`, `debug`,
`gitforge_ci=trace`, and so on). Services default to `info` and always emit
structured `tracing` events.

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
- [ ] Restrict access to the SQLite database file (there is no network database)
- [ ] Set up TLS reverse proxy
- [ ] Configure firewall rules
- [ ] Front the API with a rate-limiting proxy (no rate limiter is built into the API)
