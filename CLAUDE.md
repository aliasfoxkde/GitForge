# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Type

**Dark Factory** is an opinionated GitHub repository template for bootstrapping production-grade software projects. It enforces:
- 90%+ automated test, code, and documentation coverage
- Strict pre-commit/push hooks and CI/CD pipelines
- Conventional commit conventions
- **Language-agnostic pipeline** — auto-detects Go, Python, Rust, TypeScript and runs appropriate checks

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

# Coverage check (70% threshold — unified across all languages)
make coverage

# Lint (vet, fmt, clippy — language-aware)
make lint

# Vulnerability check (language-specific)
make vuln
```

## Architecture

### Directory Structure

```
.github/
├── actions/
│   └── detect-languages/   # Reusable language detection action
├── workflows/
│   ├── ci.yml              # Entry point orchestrator (no path filtering)
│   ├── lint.yml            # workflow_call — dispatches to language linters
│   ├── test.yml            # workflow_call — dispatches to language tests
│   ├── build.yml           # workflow_call — dispatches to language builds
│   ├── security.yml        # workflow_call — Atheon + CodeQL + vuln scans
│   ├── smoke-test.yml      # workflow_call — sanity checks
│   ├── quality-audit.yml   # workflow_call — complexity + hot-spot analysis
│   ├── e2e.yml             # Playwright E2E tests
│   ├── ai-review.yml       # AI code review (Claude Code CLI today, TaskWizer CLI later)
│   └── atheon.yml          # Atheon-Enhanced security scanning
.githooks/
├── pre-commit              # Language-aware dispatcher
├── pre-commit.d/           # Language hooks + quality gates
├── pre-push               # Language-aware dispatcher
├── pre-push.d/            # Language push hooks
└── lib/                   # Shared utilities (common.sh, language-detect.sh, staged-files.sh)
docs/              # Architecture, testing strategy, hooks docs
scripts/           # Setup and installation scripts
template-parts/     # Modular language-specific starter templates
```

### GitHub Actions Pipeline

The pipeline is **language-agnostic and modular**. `ci.yml` is the entry point that:
1. Detects languages via `.github/actions/detect-languages/`
2. Calls reusable `workflow_call` workflows for lint, test, build, security

| Workflow | Type | Purpose |
|----------|------|---------|
| `ci.yml` | Entry point | Orchestrates all jobs — no path filtering |
| `lint.yml` | `workflow_call` | Dispatches to language-specific linters |
| `test.yml` | `workflow_call` | Dispatches to language-specific tests |
| `build.yml` | `workflow_call` | Dispatches to language-specific builds |
| `security.yml` | `workflow_call` | Atheon + CodeQL + vuln scans per language |
| `smoke-test.yml` | `workflow_call` | File structure, README, license checks |
| `quality-audit.yml` | `workflow_call` | Complexity, cognitive load, hot-spot analysis |
| `e2e.yml` | Standalone | Playwright E2E tests |
| `ai-review.yml` | Standalone | AI PR review (Claude Code CLI today, TaskWizer later) |
| `atheon.yml` | Standalone | Atheon-Enhanced SARIF scanning |
| `rust.yml` | Standalone | Rust-specific pipeline (test, lint, coverage, audit) |
| `release-rust.yml` | Standalone | Cross-platform Rust release builds |

**Branch protection required checks:** `ci/check`, `ci/lint`, `ci/test`, `ci/security`

### Git Hooks

All hooks are **language-aware** — they detect which language files are staged and run only relevant checks.

| Hook | Purpose |
|------|---------|
| `pre-commit` | Quality gates (Atheon) → language-specific format/vet/clippy |
| `pre-push` | Full test suite per language + complexity gate |
| `pre-commit.d/quality-gates.sh` | Atheon-enhanced secrets/PII scan + bash fallback patterns |
| `pre-commit.d/{go,python,rust,ts}-hooks.sh` | Language-specific format + lint + tests |
| `pre-push.d/{go,python,rust,ts}-push.sh` | Full test suite per language |

### Template Parts

| Template | Purpose |
|----------|---------|
| `template-parts/go/` | Go module structure (cmd/, internal/, api/, db/) |
| `template-parts/python/` | Python package (pyproject.toml, ruff, pytest) |
| `template-parts/rust/` | Rust workspace crate |
| `template-parts/typescript/` | TypeScript project (eslint, prettier, vitest) |
| `template-parts/e2e-testing/` | E2E test harness with Playwright + AI coverage analysis |
| `template-parts/atheon-enhanced/` | Atheon-Enhanced scanner + MCP server |
| `template-parts/code-library/` | Reusable snippets and documentation |
| `template-parts/scaffolding/` | Pre-built project templates |

## Key Conventions

1. **Test First** — Write failing test before code
2. **Sentinel Errors** — `var ErrXxx = errors.New("...")`
3. **Context Propagation** — All public APIs accept `context.Context`
4. **Structured Logging** — Use `log/slog` not `fmt.Fprintf`
5. **Conventional Commits** — `feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `ci:`
6. **`-p 1`** — Mandatory for `go test` (package-level init state)

## Coverage

**Unified threshold:** `COVERAGE_THRESHOLD` repo variable (default 70%) applied to all languages. Each language uses its own coverage tool:

| Language | Tool |
|----------|------|
| Go | `go tool cover` |
| Python | `coverage.py` |
| Rust | `cargo tarpaulin` |
| TypeScript | `vitest --coverage` |

## Security Scanning

**Atheon-Enhanced** (384+ patterns, 28 categories) is the primary security scanner:
- **Pre-commit:** `atheon --categories=secrets,pii` on staged files
- **CI:** `atheon --categories=secrets,pii,security,ai-detection,quality --sarif` → GitHub Security tab
- **MCP:** `atheon-mcp` server for real-time Claude Code scanning

Supplementary scanners per language: CodeQL, govulncheck, pip-audit, cargo-audit, npm audit.

## AI Code Review

`ai-review.yml` reviews PRs using Claude Code CLI today. Architecture is designed so **TaskWizer CLI** (GitForge's own deterministic enforcement CLI) replaces it later — same workflow structure, only the binary changes.

Enable by setting `ENABLE_AI_REVIEW=true` in repo variables.

## Anti-Patterns

- `fmt.Fprintf(os.Stderr, ...)` — Use structured logging instead
- Global variables — use struct + dependency injection
- `panic` in production code (except top-level main)
- Hardcoded credentials or secrets
- Bare `console.log` / `print(` / `fmt.Print` in production code
- `TODO` / `FIXME` without an issue reference
