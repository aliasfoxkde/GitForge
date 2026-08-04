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
- **Sandbox Execution** — Job isolation via Docker containers
- **Artifact Storage** — Build artifact and cache management
- **REST API** — Full API for integration with other tools
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
# Clone and build
cargo build --release

# Start with Docker Compose
docker-compose up -d

# Or run services individually
./target/release/api &
./target/release/ci &
./target/release/git-server &
./target/release/runner &
```

See [docs/RUNBOOK.md](docs/RUNBOOK.md) for detailed instructions.

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
│   ├── gitforce-api/        # REST API
│   ├── gitforce-ci/         # CI orchestration
│   ├── gitforce-core/       # Git protocol handlers
│   ├── gitforce-runner/     # Job execution
│   └── ...
├── services/         # Binary services
│   ├── api/          # API gateway
│   ├── ci/          # CI orchestrator
│   ├── git-server/  # Git SSH/HTTP server
│   └── runner/       # Job runner agent
├── docs/             # Documentation
└── .github/          # GitHub workflows
```

## License

MIT License
