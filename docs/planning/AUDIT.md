# Dark Factory — Gap Audit

**Date:** 2026-06-26
**Status:** In Progress

## Current State

### What Exists
- 8 GitHub Actions workflows (ci, security, auto-merge, release, wiki, dev-testing, integration, setup-repo)
- 3 wiki files (Getting Started, Pattern Development, Troubleshooting)
- Pre-commit hooks: gofmt, go vet, selective test, coverage gate, bundle rebuild
- Pre-push hook: full test suite with race detection
- Makefile: build, test, lint, coverage, vuln, bundle, setup, clean
- Template parts: go module, e2e-testing (basic harness), code-library (4 snippets)
- Standard docs: README, CONTRIBUTING, SECURITY, ARCHITECTURE, TASKS, PROGRESS
- GitHub config: CODEOWNERS, dependabot, funding, issue/PR templates

### What's Missing

#### Template Parts
| Gap | Priority | Notes |
|-----|----------|-------|
| Python template part | CRITICAL | Python is core to TaskWizer/VibeGenie stacks |
| JS/TS template part | HIGH | Standard web app structure |
| Rust template part | MEDIUM | Future-proofing |
| Scaffolding templates | HIGH | Folder structures, standard files |
| E2E test runner script | HIGH | CLI to run E2E tests with coverage |
| E2E CI integration | HIGH | Integration with CI pipeline |

#### Quality Gates (from backend enforcement)
| Gap | Priority | Notes |
|-----|----------|-------|
| Secrets detector hook | CRITICAL | Block hardcoded API keys, tokens, credentials |
| Console log detector hook | HIGH | Block console.log/error/warn in production code |
| Fake data detector hook | MEDIUM | Block placeholder/fabricated data patterns |
| Placeholder code hook | MEDIUM | Block TODO/FIXME without issue refs |
| Pre-request frustration detection | LOW | For AI-assisted branches |
| Regression detector | LOW | Block behavioral regression in AI output |

#### Documentation
| Gap | Priority | Notes |
|-----|----------|-------|
| SDLC Guide | CRITICAL | Complete hook system, CI/CD, PR, merge, release docs |
| Testing Strategy | CRITICAL | How to write 90%+ coverage incrementally |
| Branch Strategy | HIGH | feature/ fix/ docs/ test/ refactor/ prefixes |
| Hook System Docs | HIGH | What each hook does, how to customize |
| CI/CD Pipeline Docs | MEDIUM | What each workflow does, how to modify |
| Release Process | MEDIUM | How to cut a release |
| Dependabot Guide | MEDIUM | How dependency updates work |

#### CI/CD
| Gap | Priority | Notes |
|-----|----------|-------|
| Python CI workflow | CRITICAL | uv-based, coverage, lint (ruff, mypy) |
| Test matrix expansion | HIGH | Coverage reports, benchmark tracking |
| Codecov integration fix | MEDIUM | Token scope issue |

## Gap Closure Priority

### Phase 1 — Critical (foundation)
1. Python template part (module structure, cmd/, internal/)
2. Python CI workflow (uv, pytest, ruff, mypy, coverage)
3. SDLC documentation
4. Quality gates pre-commit hooks

### Phase 2 — High
5. E2E test runner + CI integration
6. Code library expansion (20+ snippets)
7. Branch strategy documentation
8. Scaffolding templates

### Phase 3 — Medium
9. JS/TS template part
10. Rust template part
11. Enforcement hooks (zero-tolerance, regression detection)
12. Benchmark tracking
