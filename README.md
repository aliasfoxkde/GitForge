# GitForge

> **Self-hosted Git platform with event-driven CI/CD capabilities.**

GitForge is a production-ready, self-hosted Git service that provides Git hosting, CI/CD pipeline orchestration, and job execution — similar to GitHub Actions but fully self-hosted.

## Quick Links

| I want to... | Go to... |
|--------------|----------|
| Get started quickly | [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) |
| Deploy with Docker | [docs/DEPLOYMENT.md#docker-compose](docs/DEPLOYMENT.md#docker-compose) |
| Understand the architecture | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| Run locally | [docs/RUNBOOK.md](docs/RUNBOOK.md) |
| API reference | [docs/API.md](docs/API.md) |
| Contribute | [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) |

## Features

- **Git Hosting** — Git server with SSH and HTTP protocol support
- **Event-Driven CI/CD** — Pipeline automation triggered by Git events
- **AI Code Review** — Review runs with per-commit findings over the diff
- **Sandbox Execution** — Job isolation via Docker containers
- **Artifact Storage** — Build artifact and cache management
- **REST API** — Full API for integration with other tools and the `gitforge` CLI
- **Prometheus Metrics** — Built-in observability
- **Cross-Platform** — Linux, macOS, and Windows binaries

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    GitForge                         │
├─────────────┬─────────────────┬─────────────────────┤
│ Git Server  │   CI Engine     │    Runner Agent     │
│  (SSH/HTTP) │   (Scheduler)   │   (Sandbox/Docker)  │
└─────────────┴─────────────────┴─────────────────────┘
```

## Services

| Service | Port | Description |
|---------|------|-------------|
| API Gateway | 42780 | REST API server |
| CI Orchestrator | 42781 | Pipeline execution + Scheduler API |
| Git HTTP | 42782 | Git over HTTP |
| Git SSH | 42022 | Git over SSH |
| Runner Agent | Dynamic | Executes CI jobs in Docker |

## Quick Start

```bash
# Build everything
make build            # or: cargo build --release

# Run the test suite and quality gates
make test
make lint

# Start the stack with Docker Compose (requires .env — see below)
cp .env.example .env  # then set JWT_SECRET and the scheduler token
docker-compose up -d

# Bootstrap the first administrator, then log in
./target/release/gitforge admin --bootstrap
./target/release/gitforge auth --login
```

> The compose stack requires environment variables from `.env`
> (`DOCKER_GID`, `GITFORGE_WORKSPACE_HOST_DIR`, `GITFORGE_SCHEDULER_TOKEN`,
> `JWT_SECRET`). Services are configured exclusively through the
> environment — there is no `config.toml`. Running binaries directly
> needs at least `JWT_SECRET` (API) and `GITFORGE_SCHEDULER_URL`
> (runner); see the [Runbook](docs/RUNBOOK.md) for the full table.

## Documentation

| Category | Documents |
|----------|-----------|
| **User Guides** | [Deployment](docs/DEPLOYMENT.md) · [Runbook](docs/RUNBOOK.md) · [API Reference](docs/API.md) |
| **Architecture** | [Architecture Overview](docs/ARCHITECTURE.md) · [Hooks](docs/HOOKS.md) · [Testing Strategy](docs/TESTING_STRATEGY.md) |
| **Development** | [Contributing](docs/CONTRIBUTING.md) · [Branch Strategy](docs/BRANCH_STRATEGY.md) |
| **Project** | [Changelog](docs/CHANGELOG_RECENT.md) · [Security](docs/SECURITY.md) |

## Project Structure

```
GitForge/
├── crates/           # Core libraries
│   ├── gitforge-api/        # REST API (axum router + routes)
│   ├── gitforge-ci/         # CI pipeline engine + DAG
│   ├── gitforge-scheduler/  # Durable job scheduler
│   ├── gitforge-core/       # Git protocol handlers
│   ├── gitforge-review/     # AI review domain (diff scan, findings)
│   └── ...
├── services/         # Binary services
│   ├── api/          # API gateway
│   ├── ci/           # CI orchestrator (+ scheduler API)
│   ├── git-server/   # Git SSH/HTTP server
│   └── runner/       # Job runner agent
├── docs/             # Documentation
└── .github/          # CI workflows
```

## License

MIT License
