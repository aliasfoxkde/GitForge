# Task Ledger — Dark Factory

**Last Updated:** 2026-06-26
**Status:** Phase 2 PR open (#3)

---

## Status Legend

- `[x]` Complete
- `[~]` In Progress
- `[ ]` Open
- `[!]` Blocked

---

## Phase 1: Foundation (PR #1 — Merged)

- [x] Python template part (`template-parts/python/`)
- [x] Python CI workflow (uv, ruff, mypy, pytest, coverage)
- [x] Quality gates pre-commit hook (secrets, console.log, fake data, placeholder code)
- [x] SDLC, Branch Strategy, Testing Strategy, Hooks documentation
- [x] Issue templates (false_positive, pattern_submission)
- [x] Discussions, Template repo, Secret scanning, Push protection
- [x] CI path filters + check-go-files gate
- [x] Code library expansion (9 → 16 snippets)

---

## Phase 2: Expansion (PR #3 — Open)

### Template Parts
- [x] TypeScript template part (`template-parts/typescript/`)
- [x] Rust template part (`template-parts/rust/`)
- [x] Scaffolding templates (`template-parts/scaffolding/` — api-service, cli-tool, worker-service, data-pipeline)
- [x] E2E runner expansion (runner.sh, playwright.config.ts, smoke tests)

### Enforcement Hooks
- [x] `pre-push.d/10-complexity-gate` — function/file size, cyclomatic complexity
- [x] `pre-push.d/20-clean-branch` — advisory warnings
- [x] `pre-push.d/30-coverage-gate` — coverage threshold
- [x] `pre-rebase.d/01-check-clean-state` — dirty branch rebase guard

### CI/CD
- [x] Benchmark tracking workflow (`.github/workflows/benchmark.yml`)
- [x] E2E GitHub Actions workflow (`template-parts/e2e-testing/.github/workflows/e2e.yml`)

### Code Library
- [x] +16 new snippets: circuit breaker, rate limiter, worker pool, OTEL tracing, k8s, Docker, auth patterns, API versioning

---

## Onboarding (per-project, after copying template)

- [ ] Replace `aliasfoxkde` with actual GitHub username in CODEOWNERS, FUNDING.yml, README.md
- [ ] Customize `.github/wiki/` content
- [ ] Configure repository variables: `COVERAGE_THRESHOLD`, `GO_VERSIONS`, `PYTHON_VERSIONS`, `GOLANGCI_LINT_VERSION`
- [ ] Set up Codecov dashboard and add `CODECOV_TOKEN` secret
- [ ] Enable auto-merge manually on PRs (GitHub limitation — see PR #3)
- [ ] Customize `.github/PULL_REQUEST_TEMPLATE.md`

---

## Ideas

- [ ] GitHub App for automated PR review assignments
- [ ] Slack/Discord integration for CI notifications
- [ ] Stale issue/PR automation
- [ ] Auto-close resolved issues after 30 days
- [ ] JS/TS template part expansion (NestJS, Next.js scaffolds)
- [ ] Python template part expansion (FastAPI, Django scaffolds)
- [ ] Terraform/IaC template part
- [ ] Mobile (React Native, Flutter) template part
