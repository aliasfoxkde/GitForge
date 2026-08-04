# Phase 2 Plan — Dark Factory

**Date:** 2026-06-26
**Status:** In Progress

## Context

Phase 1 (PR #1) delivered: Python template part, Python CI workflow, SDLC/HOOKS/TESTING docs, quality-gates pre-commit hook, CI path filters, auto-merge workflow, discussions/sponsorship enabled, issue templates.

This phase delivers: JS/TS template, Rust template, E2E runner expansion, code library expansion, scaffolding templates, benchmark tracking, enforcement hooks.

---

## Items

### 1. JS/TS Template Part

**Files:** `template-parts/typescript/`

Structure:
- `src/project-name/` — main package
- `src/project-name/api/` — HTTP handlers
- `src/project-name/models/` — types/interfaces
- `src/project-name/services/` — business logic
- `src/project-name/utils/` — utilities
- `tests/unit/`, `tests/integration/`, `tests/e2e/`
- `package.json` — with vitest, typescript, eslint, prettier
- `tsconfig.json`, `vitest.config.ts`
- `.eslintrc.json`, `.prettierrc`
- `Makefile` — test, lint, typecheck, build

**Implementation:** Agent

---

### 2. Rust Template Part

**Files:** `template-parts/rust/`

Structure:
- `src/` — main lib
- `src/api/`, `src/models/`, `src/services/`, `src/utils/`
- `tests/unit/`, `tests/integration/`
- `Cargo.toml` — with serde, tokio, axum, tracing
- `rust-toolchain.toml` — pinned toolchain
- `.clippy.toml`, `rustfmt.toml`
- `Makefile` — test, clippy, fmt, build

**Implementation:** Agent

---

### 3. E2E Test Runner Expansion

**Files:** `template-parts/e2e-testing/`

Current: basic harness + README
Missing:
- `runner.sh` — CLI to run E2E tests with browser selection, parallelization, coverage
- `ci.yml` — GitHub Actions workflow for E2E on schedule + workflow_dispatch
- `playwright.config.ts` — full config with trace viewing, screenshot on failure

**Implementation:** Agent

---

### 4. Code Library Expansion (20+ Snippets)

Current: 9 snippets (Go, Python, Bash)
Target: 20+ covering: error handling, retry patterns, circuit breakers, rate limiting, graceful shutdown, context patterns, concurrent patterns, observability (OTEL), Kubernetes probes, Docker best practices, API versioning, auth patterns, config management, testing patterns, fuzzing, benchmarking, profiling

**Implementation:** Agent

---

### 5. Scaffolding Templates

**Files:** `template-parts/scaffolding/`

Templates for:
- `api-service/` — REST API with auth, OpenAPI spec
- `cli-tool/` — CLI with cobra, config file, completions
- `worker-service/` — background processor with queue
- `data-pipeline/` — ETL with streaming

Each with: Dockerfile, docker-compose, k8s manifests, Helm chart, Makefile, CI workflow.

**Implementation:** Agent

---

### 6. Enforcement Hooks

**Files:** `.githooks/`

Additional hooks:
- `pre-push.d/10-zero-tolerance` — blocks secrets, hardcoded creds, console.log in Go/Python
- `pre-push.d/20-complexity-gate` — blocks functions >50 lines, files >500 lines
- `pre-rebase.d/01-check-clean-branch` — warns on dirty branch rebase

**Implementation:** Agent

---

### 7. Benchmark Tracking Workflow

**Files:** `.github/workflows/benchmark.yml`

- Runs pprof-based benchmarks on every PR
- Uploads comparison to artifacts
- Posts comment with delta vs main branch
- Tracks over time in CSV artifact

**Implementation:** Agent

---

## Verification

### Pipeline Test Checklist
- [ ] Push to `test/auto-merge-validation` branch
- [ ] CI workflow triggers
- [ ] ci/check job runs (always-pass)
- [ ] python-ci/check (skipped — no pyproject.toml at root)
- [ ] auto-merge workflow runs → posts comment explaining limitation
- [ ] wiki.yml triggered on `.github/wiki/` push
- [ ] security.yml runs on weekly schedule (not on every push)
- [ ] release.yml triggers on tag push

### Known Limitation
`gh pr merge --auto` via GITHUB_TOKEN fails with `enablePullRequestAutoMerge` permission error. This is a GitHub Actions security restriction — third-party workflows cannot enable auto-merge without a GitHub App token. Workaround: require manual merge or use repository admin to enable auto-merge on individual PRs.

---

## Rollout Order

1. JS/TS template part
2. Rust template part
3. E2E runner expansion
4. Code library expansion (20+)
5. Scaffolding templates
6. Enforcement hooks
7. Benchmark tracking workflow
