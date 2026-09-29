# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Type

**GitForge** is a self-hosted Git platform (repos, smart HTTP + SSH Git serving) with a built-in CI/CD engine, container runner, and AI-powered code review. Rust workspace: 15 libraries under `crates/`, 4 binaries under `services/` (api, ci, git-server, runner).

## Build, Test, and Lint Commands

```bash
# Setup (install git hooks via scripts/install-hooks.sh)
make setup

# Build (debug / release)
make build
make build-release

# Run tests
make test                                    # cargo test
make test-race                               # release, --test-threads=1

# Lint — fmt check, cargo vet, strict clippy, shellcheck, aegis scan
make lint

# Aegis pattern scan against the committed baseline
make aegis                # human findings vs .github/aegis-baseline.json
make aegis-report         # full scan, no baseline
make aegis-baseline       # regenerate baseline AFTER triaging findings

# Coverage report (codecov + html from the last instrumented run)
make coverage
```

The authoritative CI chain is this repo's **own GitForge pipeline** (`.gitforge.yml`, self-hosted): `fmt → clippy → test → coverage` on `dsc-ci-rust:7`, linear chain (one shared workspace per run). Before pushing, mirror it locally:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace -- --test-threads=2   # the 2-thread bound is deliberate (see .gitforge.yml)
```

GitHub Actions is a mirror, not the gate — a red GitHub run on this repo is not authoritative; validate through GitForge.

## Architecture

### Directory Structure

```
crates/gitforge-common    # Shared types (ids, models, errors)
crates/gitforge-db        # sqlx/SQLite pool, migrations, queries
crates/gitforge-core      # Git protocol (smart HTTP, SSH), repo plumbing
crates/gitforge-scheduler # Job queue, leases, durable dispatch
crates/gitforge-runner    # Container job execution
crates/gitforge-ci        # Pipeline definitions, engine
crates/gitforge-api       # Gateway routes + openapi
crates/gitforge-cli       # `gitforge` CLI
crates/gitforge-ai/-review # AI code review
...                       # events, process, build, sandbox, storage
services/api              # Gateway binary (:42780)
services/ci               # CI orchestrator binary (:42781)
services/git-server       # Git HTTP (:42782) + SSH (:42022)
services/runner           # Runner binary
infrastructure/docker/    # CI image recipes (dsc-ci-rust)
docs/                     # RUNBOOK, API, DEPLOYMENT, planning ledger
.github/                  # Workflows + aegis baseline
.githooks/                # Installed git hooks
```

### Active GitHub Actions Workflows

| Workflow | Purpose |
|----------|---------|
| `rust-ci.yml` | Rust CI: fmt, clippy, test, build, supply chain |
| `security.yml` | CodeQL, cargo audit, secrets scan, aegis changed-line scan |
| `integration.yml` | Integration + e2e tests |
| `gitforge-ci.yml` | Enqueue the branch to the self-hosted GitForge pipeline |
| `auto-merge.yml` | Auto-merge Dependabot + same-repo PRs |
| `release.yml` | Tag-push release builds |
| `wiki.yml` | Publish wiki from `.github/wiki/` |

### Git Hooks (installed by `make setup`)

| Hook | Purpose |
|------|---------|
| `pre-commit` | fmt, vet, clippy, selective tests |
| `pre-push` | Full test suite |

## Key Conventions

1. **Conventional Commits** — `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`, `ci:`
2. **Structured Logging** — `tracing`, not `println!`/`eprintln!`
3. **Result Propagation** — `?` over `unwrap()`/`expect()` in library code; `panic!` only where a bug is provably unreachable
4. **Durable Writes** — one-shot DB writes under contention are the F21/F23 failure class; new write paths go through `persist_with_retry` or a conditional/idempotent UPDATE
5. **SQLite Concurrency** — write paths use `BEGIN IMMEDIATE` transactions (`begin_with`); the busy handler is the backlog, not a guarantee
6. **Tests** — unit tests in `#[cfg(test)]` modules, integration in `crates/*/tests/` and `services/*/tests/`; regression-test every defect with a pinned test

## Coverage

The line-coverage gate lives in `.gitforge.yml` (coverage job): **fail < 82%, warn < 84%**, calibrated against the CI sandbox — not the host, where ambient services inflate integration coverage. Raise thresholds only with an in-sandbox measurement.

## Anti-Patterns

- `unwrap()`/`expect()` in library code — return `Result`
- Global mutable state — pass pools/registries explicitly
- Placeholder code: `TODO`/`FIXME`/stubs left in place
- Hardcoded credentials or tokens (JWT_SECRET, trigger tokens live in env/config)
- Editing `docs/planning/MASTER_PLAN_2026-09-20.md` ledger entries without evidence — the F-series ledger is append-with-verdict, citing commit/PR/run ids
