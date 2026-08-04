# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Type

**{{PROJECT_NAME}}** is a {{PROJECT_TYPE}} built with the Dark Factory methodology — opinionated development with 90%+ automated test, code, and documentation coverage.

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

# Coverage check
make coverage

# Lint (vet, fmt, golangci, goimports)
make lint

# Vulnerability check
make vuln
```

## Architecture

```
{{PROJECT_STRUCTURE}}
```

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
