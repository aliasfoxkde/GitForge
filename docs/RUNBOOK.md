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

## Service Environment and Credential Isolation

### Who owns service lifecycle

The live Fedora services are supervised by four **user-systemd** units:
`gitforge-api.service`, `gitforge-ci.service`,
`gitforge-git-server.service`, and `gitforge-runner.service`. The active
release-bundle paths and operational drop-ins are under
`~/.config/systemd/user/`; inspect names/paths with
`systemctl --user show <unit> -p FragmentPath -p DropInPaths` (do not print
`Environment=` values). `systemd/gitforge@.service` is a separate system-scope
template and is **not** the current live deployment. `scripts/gitforge-status`
reports the scope that owns each process.

### Credential scrub policy

Provider keys and unrelated host secrets are present in the imported user
manager environment and, in the 2026-10-05 names-only audit, were present in
all four live GitForge service process environments. No GitForge service
binary consumes those provider variables, and children can inherit them.
The candidate policy adds an explicit `UnsetEnvironment=` scrub to the
user-service drop-in — systemd applies it
as the **final step** when compiling the executed environment, so it
overrides `EnvironmentFile=` files, drop-in `Environment=` lines, manager
globals, and PAM:

| Scope | File | Status |
|-------|------|--------|
| User-systemd (live deployment) | `systemd/user/gitforge-env-isolation.conf` — canonical drop-in to install per service | **not yet installed**; current service processes still inherit provider/host variables |
| System template (not live) | `systemd/gitforge@.service` — mirrored policy for a future system-scope deployment | candidate only |

`make unit-policy` (part of `make lint`; `scripts/verify-unit-env-policy`)
fails on: drift between the canonical list and the user drop-in, a scrubbed
name that the codebase consumes for the CLI (grep'd from `gitforge-ai`), or
a scrub of any variable in the required-environment table below. Extend the
list by editing **both** files together — never one.

**Scope limit:** this policy removes observed provider keys and provider
endpoints; it is not yet a per-service secret allowlist. GitForge-specific
secrets such as JWT/database/scheduler/trigger credentials may still be
inherited by services that do not need them. Follow-up hardening should build
per-unit `UnsetEnvironment=` policy (or move secrets to narrowly scoped
`LoadCredential=` files where supported) from a source-verified environment
read inventory, without removing a variable a service actually consumes.

Two credential paths are intentionally outside every service unit:

- **Interactive CLI** — `gitforge` code review reads `ANTHROPIC_API_KEY` /
  `OPENAI_API_KEY` from the invoking shell (`crates/gitforge-ai`). The CLI
  is not a service; the scrub never applies to it.
- **Job payloads** — credentials a CI job needs travel in the job
  specification through the scheduler/runner API and are injected as
  explicit `docker exec` env pairs (`crates/gitforge-runner/src/executor.rs`,
  `crates/gitforge-sandbox/src/docker.rs`). The runner's own process
  environment is never forwarded into job containers, so the scrub cannot
  break an explicitly configured payload.

### Required environment, per service

The complete set of variables each binary reads. Anything else (and anything
on the scrub list) has no effect on the service; prefer `JWT_SECRET_FILE`-
style credential files over inline secrets where supported.

**`gitforge@api`** (`services/api`)

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `JWT_SECRET_FILE` / `JWT_SECRET` (`_FILE` wins) | one of the two | — | JWT signing secret; `_FILE` points at a 0600 file (LoadCredential pattern) |
| `PORT` | no | `42780` | Listen port |
| `DATABASE_URL` | no | `sqlite:/gitforge.db` | SQLite location |
| `GITFORGE_CI_TRIGGER_URL` | no | — | CI trigger endpoint used by Git-server bridge |
| `GITFORGE_CI_TRIGGER_TOKEN` | no | — | Bearer token for the trigger |

**`gitforge@git-server`** (`services/git-server`)

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `GIT_ROOT` | yes | — | Repository tree root (unset ⇒ lookups 503) |
| `DATABASE_URL` | yes | — | **Git-server's own** database URL (not `GITFORGE_DATABASE_URL`) |
| `GITFORGE_SSH_HOST_KEY` | no | `$HOME/.ssh/gitforge_host_ed25519` | SSH host key path |
| `HTTP_PORT` / `SSH_PORT` | no | `42782` / `42022` | Listen ports |
| `GITFORGE_MAX_GIT_BODY_BYTES` | no | — | Smart-HTTP body cap |
| `GITFORGE_CI_TRIGGER_URL` / `GITFORGE_CI_TRIGGER_TOKEN` | no | — | Outbox trigger bridge to CI |

**`gitforge@ci`** (scheduler included; `services/ci`)

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `GITFORGE_DATABASE_URL` | yes | — | Scheduler database (not `DATABASE_URL`) |
| `SCHEDULER_PORT` | no | `42781` | Scheduler HTTP port |
| `GITFORGE_TRIGGER_TOKEN` | see note | — | Trigger auth; falls back to `GITFORGE_CI_TRIGGER_TOKEN`, `GITFORGE_SCHEDULER_OPERATOR_TOKEN`, `GITFORGE_SCHEDULER_TOKEN` |
| `GITFORGE_SCHEDULER_TOKEN` / `GITFORGE_SCHEDULER_OPERATOR_TOKEN` / `GITFORGE_RUNNER_TOKEN` | no | — | Scheduler API + runner registration auth (fail-closed when unset) |
| `GITFORGE_ARTIFACT_ROOT` | no | — | Artifact storage root |
| `GITFORGE_WORKSPACE_ROOT` / `GITFORGE_WORKSPACE_ROOTS` | no | — | Checkout workspace roots |
| `GITFORGE_CONTAINER_BACKEND` | no | error if invalid | Backend selection |
| `GITFORGE_JOB_FENCE_GRACE_SECS` | no | built-in | Job lease fence grace |
| `GITFORGE_BUILD_SOCKET` | no | — | Build-queue socket (`gitforge-build`) |

**`gitforge@runner`** (`services/runner`, `crates/gitforge-runner`)

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `GITFORGE_SCHEDULER_URL` | **yes** | — | Scheduler endpoint; startup fails without it |
| `GITFORGE_SCHEDULER_TOKEN` | no | — | Bearer token for scheduler API |
| `GITFORGE_RUNNER_NAME` / `GITFORGE_RUNNER_CAPACITY` | no | `runner` / `2` | Identity and concurrency |
| `GITFORGE_HEARTBEAT_INTERVAL` / `GITFORGE_JOB_HEARTBEAT_INTERVAL` / `GITFORGE_FETCH_INTERVAL` | no | `30` / `15` / `5` | Timing knobs (seconds) |
| `GITFORGE_REGISTER_ATTEMPTS` / `GITFORGE_REGISTER_BACKOFF_SECS` / `GITFORGE_RUNNER_STANDALONE` | no | `6` / `1` / `deny` | Registration retry policy |
| `GITFORGE_ARTIFACT_ROOT` | no | — | Artifact storage root |
| `GITFORGE_SANDBOX_ACQUIRE_SECS` / `GITFORGE_SANDBOX_MEMORY_MB` | no | built-in | Sandbox acquisition/memory caps |
| `GITFORGE_RECONCILE_DELETE` / `_GRACE_SECS` / `_INTERVAL_SECS` / `_RECEIPT` | no | `off` / `3600` / `300` | Container reconciler |
| `DOCKER_HOST` | no | well-known socket | Container daemon endpoint |

Shared: `RUST_LOG` (tracing filter) on every service.

### Rollout / rollback (live host)

The live services use per-unit files under `~/.config/systemd/user/`. Install
the same drop-in under each active unit's `.service.d/` directory; do not
install the system-scope template as a substitute. This rollout restarts
services, so drain the queue and confirm no active job/container custody first.

```bash
# 1. Back up each unit's drop-ins (rollback anchor).
backup="$HOME/.config/systemd/user/gitforge-env-isolation-backup-$(date +%Y%m%d%H%M%S)"
mkdir -m 700 "$backup"
for unit in api ci git-server runner; do
  dir="$HOME/.config/systemd/user/gitforge-$unit.service.d"
  if [ -d "$dir" ]; then cp -a "$dir" "$backup/"; fi
done

# 2. Install the provider scrub into each live user-service drop-in directory.
for unit in api ci git-server runner; do
  dir="$HOME/.config/systemd/user/gitforge-$unit.service.d"
  mkdir -p "$dir"
  install -m 0644 systemd/user/gitforge-env-isolation.conf \
    "$dir/10-env-isolation.conf"
done
systemctl --user daemon-reload

# 3. Restart after drain; never interrupt active jobs or runner-owned workspaces.
systemctl --user restart gitforge-api.service gitforge-git-server.service
systemctl --user restart gitforge-ci.service gitforge-runner.service

# 4. Verify: services healthy, and no provider credential/endpoint name in
#    environment. Both commands print NAMES ONLY — never pipe the raw
#    environ or `systemctl show -p Environment` anywhere, they carry values.
./scripts/gitforge-status
pid=$(systemctl --user show -p MainPID --value gitforge-api.service)
tr '\0' '\n' <"/proc/$pid/environ" | cut -d= -f1 | sort
```

Rollback: remove `10-env-isolation.conf` from each of the four unit drop-in
directories (or restore the backed-up directories), run
`systemctl --user daemon-reload`, and restart the same four user units after
confirming service/queue health. The drop-in changes no `ExecStart` or
service-specific environment configuration.

> **2026-10-05 audit correction.** A names-only `/proc/<pid>/environ`
> inspection of the active user-systemd API, CI, Git-server, and runner
> processes confirmed ambient provider keys and provider endpoint variables
> in all four environments. The user manager's effective imported
> environment is the source. The system-template policy alone does not fix
> this live deployment; install the user drop-in in each unit's `.service.d`
> directory and restart only after the GitForge queue is drained and a
> rollback path is ready. No values were read or recorded.

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

# Read-only Fedora service and endpoint report (reports the scope that owns
# each live process)
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
