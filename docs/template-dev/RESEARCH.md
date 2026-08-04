# Research: Dark Factory Enhancement

## Requirements

1. **E2E Testing Framework Completion**
   - `session.go` — Session lifecycle, parallel coordination
   - `reporter.go` — JSON/HTML/SARIF output formats, coverage trends
   - `debugger.go` — Screenshots, console logs, network recording

2. **Go Template Enhancement**
   - Add `cmd/app/main.go` with graceful shutdown
   - Add `internal/api/server.go`, `middleware.go`
   - Add `internal/logger/logger.go` with slog
   - Add `db/migrations/001_initial.sql`
   - Add `_fixtures/example_test.go`

3. **code-library/snippets/common/**
   - `git_hooks.sh` — Hook installation helpers
   - `ci_validation.sh` — CI environment detection

## Constraints

- Must maintain conventional commit format
- Go code must pass `go vet`, `golangci-lint`
- Template files should be parameterized with `{{PLACEHOLDER}}` patterns
- All new Go files require corresponding test files

## Existing Patterns

- `template-parts/e2e-testing/framework/harness.go` — Reference implementation
- `template-parts/e2e-testing/framework/ai_coverage.go` — Reference implementation
- `template-parts/go/` — Sparse, needs expansion

## Implementation Order

1. Create required documentation files
2. Complete E2E framework files
3. Enhance Go template
4. Create snippets/common
