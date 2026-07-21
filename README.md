# GitForge

> **Self-hosted Git platform with event-driven CI/CD capabilities.**

GitForge is a production-ready, self-hosted Git service similar to GitHub Actions but fully self-hosted. It provides Git server functionality (SSH + HTTP), event-driven CI/CD orchestration, sandbox-based job execution runners, artifact storage, and a REST API.

## Features

- ✅ **Git Server** - SSH and HTTP Git protocol support
- ✅ **Event-Driven CI/CD** - Pipeline orchestration with DAG-based job execution
- ✅ **Sandbox Execution** - Docker-based job isolation with resource limits
- ✅ **Artifact Storage** - Build artifact and cache management
- ✅ **REST API** - Full API for integration with other tools
- ✅ **Prometheus Metrics** - Built-in observability
- ✅ **Cross-Platform Builds** - Linux and Windows binaries; macOS planned
- ✅ **High Coverage** - 88%+ test coverage with 823+ tests

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                    GitForge                         │
├─────────────┬─────────────────┬─────────────────────┤
│ Git Server  │   CI Engine     │    Runner Agent     │
│  (SSH/HTTP) │   (Scheduler)   │   (Sandbox/Docker)  │
└─────────────┴─────────────────┴─────────────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.70+
- PostgreSQL 15+ (or use SQLite for development)
- Docker (for sandbox execution)

### Build

```bash
cargo build --release --workspace
```

### Test

```bash
cargo test --workspace
```

### Run

```bash
# API server
cargo run --bin gitforge -- api

# CI engine
cargo run --bin gitforge -- ci

# Git server
cargo run --bin gitforge -- git-server
```

## Documentation

| Document | Description |
|----------|-------------|
| [docs/PLAN.md](docs/PLAN.md) | Project plan and implementation phases |
| [docs/PROGRESS.md](docs/PROGRESS.md) | Implementation progress and status |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | System architecture |
| [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) | Deployment strategies |
| [docs/MACOS_BUILD.md](docs/MACOS_BUILD.md) | macOS build and release plan |
| [docs/API.md](docs/API.md) | REST API documentation |
| [docs/TESTING_STRATEGY.md](docs/TESTING_STRATEGY.md) | Testing approach |

## Crates

| Crate | Purpose |
|-------|---------|
| `gitforce-common` | Shared types, UUIDs, errors |
| `gitforce-db` | Database models and migrations |
| `gitforce-events` | Event bus and type definitions |
| `gitforce-core` | Git protocol handlers (SSH/HTTP) |
| `gitforce-ci` | Pipeline orchestration and DAG execution |
| `gitforce-scheduler` | Job queue and runner assignment |
| `gitforce-runner` | Job execution agent |
| `gitforce-sandbox` | Container/VM isolation |
| `gitforce-storage` | Artifact and cache storage |
| `gitforce-api` | REST API gateway |

## Services

| Service | Binary | Description |
|---------|--------|-------------|
| API | `gitforge api` | REST API server (Axum) |
| CI | `gitforge ci` | CI engine and scheduler |
| Git Server | `gitforge git-server` | Git SSH/HTTP server |
| Runner | `gitforge runner` | Job execution agent |

## Cross-Platform Builds

| Platform | Status |
|----------|--------|
| Linux (x86_64, ARM64) | ✅ Ready |
| Windows (x86_64, ARM64) | ✅ Ready |
| macOS (x86_64, ARM64) | 🔲 Planned |

For macOS builds, see [docs/MACOS_BUILD.md](docs/MACOS_BUILD.md).

## License

MIT License
