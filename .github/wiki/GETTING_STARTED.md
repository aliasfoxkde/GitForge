# Getting Started

GitForge is a self-hosted Git platform: repositories, smart HTTP + SSH Git
serving, a built-in CI/CD engine with a container runner, and AI-powered code
review. Rust workspace, 15 crates under `crates/`, 4 service binaries under
`services/`.

## Prerequisites

- Rust (stable) via [rustup](https://rustup.rs)
- Git
- SQLite (the sqlx CLI is used for query checks during development)
- Docker/podman only if you want to exercise the container runner locally

## Quick Start

1. **Clone the repository**
   ```bash
   git clone https://github.com/aliasfoxkde/GitForge.git
   cd GitForge
   ```

2. **Install git hooks**
   ```bash
   make setup
   ```

3. **Build**
   ```bash
   make build          # debug
   make build-release  # release
   ```

4. **Run tests**
   ```bash
   make test
   ```

5. **Lint**
   ```bash
   make lint   # fmt, cargo vet, strict clippy, shellcheck, aegis scan
   ```

## Services

| Binary | Port | Role |
|---|---|---|
| `api` | 42780 | Gateway routes + OpenAPI |
| `ci` | 42781 | CI orchestrator / scheduler |
| `git-server` | 42782 (HTTP) / 42022 (SSH) | Git serving |
| `runner` | — | Container job execution |

## Next Steps

- Read the [Setup Guide](SETUP_GUIDE) for deployment configuration
- Check [Troubleshooting](TROUBLESHOOTING) when something misbehaves
- See `docs/RUNBOOK.md` in the repo for live-instance operations
