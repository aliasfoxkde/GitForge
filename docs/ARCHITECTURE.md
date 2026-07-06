# Architecture

## Overview

Dark Factory is a template repository for bootstrapping production-grade
software projects with built-in CI/CD, testing, and documentation coverage.

## Core Principles

1. **Test coverage as you go** — Not deferred to the end
2. **Git as the source of truth** — All configuration in git
3. **Automation everywhere** — Human intervention only when necessary
4. **Documentation is code** — Inline docs, ADRs, and READMEs kept in sync

## Components

### GitHub Actions Workflows

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | push, PR | Main test + lint + build pipeline |
| `security.yml` | push, schedule | CodeQL, govulncheck, secrets scan |
| `auto-merge.yml` | PR | Auto-merge Dependabot + same-repo PRs |
| `release.yml` | tag push | GoReleaser cross-platform builds |
| `wiki.yml` | push to wiki/ | Publish wiki from `.github/wiki/` |
| `setup-repo.yml` | manual | Configure non-templatable GitHub settings |

### Git Hooks

| Hook | Purpose |
|------|---------|
| `pre-commit` | Format, vet, selective tests, coverage, bundle rebuild |
| `pre-push` | Full test suite with coverage gate |

### Template Parts

Modular, language-specific starter kits:

- `template-parts/go/` — Go module structure
- `template-parts/e2e-testing/` — E2E test harness
- `template-parts/code-library/` — Reusable snippets

## Data Flow

```
Developer → git commit → pre-commit hook
                          ├── gofmt / goimports
                          ├── go vet
                          ├── selective tests (if core/ changed)
                          └── coverage gate

git push → GitHub Actions
              ├── ci.yml (required checks)
              ├── security.yml
              └── auto-merge.yml (on PR merge)

Tag push → release.yml → GoReleaser → GitHub Releases
```

## Configuration

All configuration is environment-driven:

- `vars.COVERAGE_THRESHOLD` — Minimum coverage % (default: 70)
- `vars.GO_VERSIONS` — JSON array of Go versions to test
- `vars.GOLANGCI_LINT_VERSION` — golangci-lint version
- `vars.DEFAULT_BRANCH` — Default branch name