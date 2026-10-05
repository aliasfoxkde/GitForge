# GitForge Deployment Guide

> **Partially superseded (2026-09).** Configuration is environment-only (no config.toml), the database is SQLite-only, and SSH is implemented. Where this document conflicts with [RUNBOOK.md](RUNBOOK.md), prefer the Runbook.

This guide covers deploying GitForge using Docker Compose for self-hosted Git with CI/CD.

## Prerequisites

- Docker Engine 24.0+
- Docker Compose 2.20+
- 4GB RAM minimum (8GB recommended)
- 20GB disk space

## Quick Start

1. Clone the repository:
```bash
git clone https://github.com/your-org/gitforge.git
cd gitforge
```

2. Configure the environment. Services read configuration from environment
variables only — there is no config file. Copy `.env.example` to `.env.local`
and fill in the deployment variables:
```bash
cp .env.example .env.local
# Edit .env.local: JWT_SECRET, DATABASE_URL, GITFORGE_SCHEDULER_TOKEN,
# DOCKER_GID, GITFORGE_WORKSPACE_HOST_DIR
```

3. Set a secure JWT secret:
```bash
export JWT_SECRET=$(openssl rand -base64 32)
```

4. Start the services:
```bash
docker-compose up -d
```

5. Verify health:
```bash
curl http://localhost:42780/health
```

## First administrator bootstrap

GitForge does not expose public user registration. On a fresh database, run
the local CLI once to create the first administrator:

```bash
export DATABASE_URL='sqlite:/path/to/gitforge.db?mode=rwc'
gitforge admin --bootstrap \
  --username operator \
  --email operator@example.com \
  --confirm
```

The command reads the password without echoing it, requires at least 12
characters, hashes it with bcrypt, and refuses to run if an administrator
already exists. It is a local database bootstrap operation; it does not grant
privileges through the HTTP API or accept a password as a command-line
argument. Obtain the API session token through the normal login flow:

```bash
gitforge auth --login operator
```

Keep `DATABASE_URL` and the database file restricted to the GitForge service
account. After login, use the authenticated session for repository
registration and the trigger canary.

## Services

### API Gateway (port 42780)
REST API for GitForge. All client interaction goes through this service.

### CI Orchestrator (port 42781)
Processes pipeline events and orchestrates job execution. The scheduler HTTP API runs within this service.

### Runner
Executes CI jobs in Docker containers. Multiple runners can be deployed horizontally. Connects to CI service at `http://ci:42781`.

### Git Server (ports 42782 HTTP, 42022 SSH)
Handles Git SSH and HTTP protocols. SSH is served by an in-process russh
transport that authenticates against the SSH key registry — a client key must
be registered to an account (`POST /api/ssh-keys`) before it can connect.

## Configuration

Services are configured exclusively through environment variables; there is no
config file. See `.env.example` for the deployment variables and
[RUNBOOK.md](RUNBOOK.md#configuration) for the full per-service table.

### GitHub Actions trigger credentials (issue #259)

When the repository's `gitforge-ci.yml` workflow is enabled
(`GITFORGE_ENABLED == 'true'`), it uses two distinct credentials:

| Credential | Where it lives | What it may do |
|------------|----------------|----------------|
| `GITFORGE_TRIGGER_TOKEN` | GitHub secret + CI service env | Start runs: `POST /pipelines/trigger` on the CI service |
| `GITFORGE_STATUS_TOKEN` | GitHub secret + CI service env | Read the status of one trigger event it can name: `GET /pipelines/trigger/status/{event_id}` |

Both tokens must be high-entropy and distinct from each other and from the
scheduler operator/runner tokens. The status token is scoped to status reads
only — it cannot start runs, and the trigger or operator token is not accepted
by the status endpoint. Token values are never logged by the service.

Deployment migration (the table is created automatically at the next `ci`
service start; no manual SQL):

1. Add `GITFORGE_STATUS_TOKEN=<new high-entropy secret>` to the CI service
   environment file (the same unit that already sets `GITFORGE_TRIGGER_TOKEN`
   and `GITFORGE_DATABASE_URL`), then restart the service once.
2. In the GitHub repository settings, set the secrets `GITFORGE_TRIGGER_TOKEN`
   and `GITFORGE_STATUS_TOKEN` to their matching values. The workflow fails
   fast with an actionable message if either secret is missing or if both
   hold the same value.
3. `GITFORGE_API_URL` and `GITFORGE_API_TOKEN` are no longer used by the
   workflow: run status is answered by the CI service itself, so the workflow
   no longer holds an API gateway JWT.

The endpoint refuses to answer when the CI service runs without
`GITFORGE_DATABASE_URL` — durable correlation is required, so deployments that
enable the GitHub workflow must keep the CI service's database configured.

## Scaling Runners

Add more runners by scaling the service:
```bash
docker-compose up -d --scale runner=3
```

Give each instance a distinct `GITFORGE_RUNNER_NAME` so they do not contend
for a single registry row.

## Database

The database is SQLite, pointed at by `DATABASE_URL` (for example
`sqlite:/data/gitforge.db?mode=rwc`). There is no PostgreSQL backend; persist
the database file on a volume and back it up (see the checklist below).

## Monitoring

Prometheus metrics available at `http://localhost:42780/metrics`.

Key metrics:
- `gitforge_http_requests_total` - HTTP request counts
- `gitforge_job_duration_seconds` - Job execution time
- `gitforge_runners_online` - Active runners

## Troubleshooting

### Runner can't connect to scheduler
```bash
docker-compose logs runner
# Check GITFORGE_SCHEDULER_URL environment variable
```

### Jobs stuck in queue
```bash
docker-compose logs ci
```

### Database locked
The database is SQLite, which serializes writers. Stagger write-heavy work and
scale by adding runners rather than extra direct database writers.

## Production Checklist

- [ ] Change JWT secret
- [ ] Restrict access to the SQLite database file and back up its volume
- [ ] Configure CORS origins
- [ ] Set up TLS reverse proxy
- [ ] Front the API with a rate-limiting proxy (no rate limiter is built into the API)
- [ ] Configure backup for database volume
