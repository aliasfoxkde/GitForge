# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Type

**Dark Factory** is an opinionated GitHub repository template for bootstrapping production-grade software projects. It enforces:
- 90%+ automated test, code, and documentation coverage
- Strict pre-commit/push hooks and CI/CD pipelines
- Conventional commit conventions

This is a **template repository** — actual project code lives in `template-parts/scaffolding/` for various project types (api-service, cli-tool, worker-service, data-pipeline).

## Build, Test, and Lint Commands

```bash
# Setup (install git hooks)
make setup

# Build binaries
make build

# Run tests
make test

# Run tests with race detector
make test-race

# Coverage check (70% threshold)
make coverage

# Lint (vet, fmt, golangci, goimports)
make lint

# Vulnerability check
make vuln
```

## Architecture

### Directory Structure

```
.github/           # GitHub configuration (workflows, templates)
.githooks/         # Installed git hooks (pre-commit, pre-push)
docs/              # Architecture, testing strategy, hooks docs
scripts/           # Setup and installation scripts
template-parts/    # Modular language-specific starter templates
```

### GitHub Actions Workflows

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | push, PR | Test + lint + build pipeline |
| `security.yml` | push, schedule | CodeQL, govulncheck, secrets scan |
| `auto-merge.yml` | PR | Auto-merge Dependabot + same-repo PRs |
| `release.yml` | tag push | GoReleaser cross-platform builds |
| `wiki.yml` | push to wiki/ | Publish wiki from `.github/wiki/` |

### Git Hooks

| Hook | Purpose |
|------|---------|
| `pre-commit` | gofmt, goimports, go vet, selective tests, coverage gate |
| `pre-push` | Full test suite with coverage gate |

### Template Parts

| Template | Purpose |
|----------|---------|
| `template-parts/go/` | Go module structure (cmd/, internal/, api/, db/) |
| `template-parts/e2e-testing/` | E2E test harness with AI coverage analysis |
| `template-parts/code-library/` | Reusable snippets and documentation |
| `template-parts/scaffolding/` | Pre-built project templates |

## Key Conventions

1. **Test First** — Write failing test before code
2. **Sentinel Errors** — `var ErrXxx = errors.New("...")`
3. **Context Propagation** — All public APIs accept `context.Context`
4. **Structured Logging** — Use `log/slog` not `fmt.Fprintf`
5. **Conventional Commits** — `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `ci:`
6. **`-p 1`** — Mandatory for `go test` (package-level init state)

## Coverage Targets

| Layer | Minimum |
|-------|---------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utilities | 85% |

## Anti-Patterns

- `fmt.Fprintf(os.Stderr, ...)` — Use structured logging instead
- Global variables — use struct + dependency injection
- `panic` in production code (except top-level main)
- Hardcoded credentials or secrets
- `//golint:disable` without justification
