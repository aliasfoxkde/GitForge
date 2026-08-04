# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Type

**GitForge** is a local-first Git platform client with AI-powered code review capabilities. It provides:
- Git repository management with local-first architecture
- AI-powered code review (Anthropic Claude, OpenAI GPT, Ollama local)
- CI/CD pipeline integration
- Security vulnerability scanning

## Build, Test, and Lint Commands

```bash
# Build binaries
cargo build --release

# Run tests
cargo test --workspace

# Run tests with race detector
cargo test --workspace -- --test-threads=1

# Lint
cargo clippy --workspace --all-targets -- -D warnings

# Format
cargo fmt --all
```

## Architecture

### Directory Structure

```
.github/           # GitHub configuration (workflows)
.githooks/         # Installed git hooks
crates/            # Core Rust libraries
services/          # Microservices (api, git-server, ci, runner)
docs/              # Documentation
```

### Crates

| Crate | Purpose |
|-------|---------|
| gitforce-ai | AI provider interface (Claude, GPT, Ollama) |
| gitforce-review | Diff parsing, security scanning, fix suggestions |
| gitforce-cli | CLI with `gitforge review` command |
| gitforce-core | Core Git operations |
| gitforce-api | API gateway |

### GitHub Actions Workflows

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `gitforge-ci.yml` | push, PR | Build queue via GitForge |
| `ai-review.yml` | PR | AI code review |
| `release.yml` | tag push | Cross-platform releases |

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
