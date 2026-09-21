# Tasks: Dark Factory Enhancement

> **Abandoned.** This task list belonged to the Dark Factory Go-template effort, not GitForge. Current plans live in [planning/](planning/).

## Task Checklist

### Phase 1: Documentation
- [x] RESEARCH.md created
- [x] PLAN.md created
- [ ] TASKS.md created (this file)
- [ ] PROGRESS.md created

### Phase 2: E2E Testing Framework
- [ ] `session.go` — Session management with parallel coordination
- [ ] `reporter.go` — Multi-format reporting (JSON, HTML, SARIF)
- [ ] `debugger.go` — Debug utilities (screenshots, logs, state)

### Phase 3: Go Template
- [ ] `cmd/app/main.go` — Entry point with graceful shutdown
- [ ] `internal/api/server.go` — HTTP server
- [ ] `internal/api/middleware.go` — CORS, auth, logging middleware
- [ ] `internal/logger/logger.go` — slog-based structured logging
- [ ] `db/migrations/001_initial.sql` — Example migration
- [ ] `_fixtures/example_test.go` — Test patterns

### Phase 4: Code Library
- [ ] `snippets/common/git_hooks.sh` — Hook installation helpers
- [ ] `snippets/common/ci_validation.sh` — CI environment detection