# AI Copilot Instructions

This file provides instructions for GitHub Copilot and other AI assistants
working in this repository.

## Project Type

This is a **Dark Factory** powered project. Dark Factory is an opinionated
development methodology emphasizing:
- 90%+ automated test, code, and documentation coverage
- Incremental coverage growth (not deferred testing)
- Strict CI/CD and local pre-commit/push hooks
- Deterministic enforcement via git hooks and GitHub Actions

## Key Conventions

1. **Test First** — Write the failing test before writing code
2. **Document Why** — Doc comments explain WHY, not just WHAT
3. **Sentinel Errors** — Define package-level `var ErrXxx = errors.New("...")`
4. **Context Propagation** — All public APIs accept `context.Context`
5. **Structured Logging** — Use `log/slog` (Go) or equivalent
6. **Conventional Commits** — `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `ci:`
7. **`-p 1`** — Mandatory for `go test` (package-level init state)

## Coverage Requirements

| Component | Minimum Coverage |
|-----------|-----------------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utility functions | 85% |

## File Patterns

- `*.go` — Go source files
- `*_test.go` — Go test files
- `.github/workflows/*.yml` — GitHub Actions workflows
- `.githooks/*` — Git hooks (pre-commit, pre-push)
- `docs/**/*.md` — Documentation

## Anti-Patterns (Do NOT use)

- `fmt.Fprintf(os.Stderr, ...)` — Use structured logging instead
- `//golint:disable` without justification
- Global variables (use struct + dependency injection)
- `panic` in production code (except at top-level main)
- Hardcoded credentials or secrets
