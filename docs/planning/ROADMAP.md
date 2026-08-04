# Dark Factory — Roadmap

## Vision

**Dark Factory** is an AI-first, 90%+ automated coverage, self-enforcing development methodology and GitHub repo template. It creates a "hands-off, dark factory" — AI agents and CI/CD enforce quality, security, and process with minimal human intervention.

The factory metaphor: raw inputs (ideas, requirements) enter one end, and polished, tested, documented code exits the other, with AI filling in every gap.

---

## Features

### Core Principles

1. **Test as you go** — Not deferred to the end. Problems compound when testing is postponed.
2. **Git as the source of truth** — All configuration, process, and policy lives in git.
3. **Automation everywhere** — Human intervention only when necessary (approvals, security exceptions).
4. **Documentation is code** — Inline docs, ADRs, and READMEs kept in sync.
5. **Enforcement without permission** — Pre-commit/push hooks block bad code. CI blocks bad merges.

### What's Built

- ✅ 8 GitHub Actions workflows (CI, security, auto-merge, release, wiki, dev-testing, integration, setup-repo)
- ✅ Pre-commit hooks (format, vet, test, coverage, bundle)
- ✅ Pre-push hook (full suite with race detection)
- ✅ Branch protection (strict, required checks, 1 review)
- ✅ Auto-merge for Dependabot + same-repo PRs
- ✅ Template parts: Go module, E2E testing framework, code library snippets
- ✅ Comprehensive GitHub config (CODEOWNERS, issue/PR templates, dependabot, funding, copilot)
- ✅ Auto-published Wiki from `.github/wiki/`

### What's Next

#### Phase 1: Python + Quality Gates (In Progress)
- Python template part (module structure, cmd/, internal/, pyproject.toml)
- Python CI workflow (uv, pytest, ruff, mypy, coverage)
- Quality gates pre-commit hooks (secrets, console.log, fake data, placeholder code)
- SDLC documentation (hook system, CI/CD, PR/merge, release, testing strategy)

#### Phase 2: E2E + Code Library
- E2E test runner script (CLI with coverage, retry, parallel)
- E2E CI integration (in main CI workflow)
- Code library expansion (20+ patterns: Go, Python, Bash, API patterns)
- Branch strategy documentation
- Scaffolding templates (folder structures, standard files)

#### Phase 3: Multi-Stack + Enforcement
- JS/TS template part (Node.js structure)
- Rust template part (Cargo structure)
- Enforcement hooks (zero-tolerance, behavioral regression detection)
- AI integration hooks (frustration detection, complexity scoring)
- Benchmark tracking workflow
- Slack/Discord notifications

#### Phase 4: Ecosystem
- Template generator CLI (interactively create repos from template parts)
- Dark Factory bootstrap service
- TaskWizer template (Dev task management — powered by dark-factory)
- VibeGenie template (Kids platform — simplified dark-factory)
- Documentation site (MkDocs or Docusaurus)

---

## Stack Coverage Targets

| Stack | Coverage Target | Status |
|-------|---------------|--------|
| Go | 90%+ | ✅ Built-in |
| Python | 85%+ | 🔨 Phase 1 |
| JS/TS | 85%+ | 📋 Planned |
| Bash | 70%+ (lint) | ✅ Partial |
| Rust | 80%+ | 📋 Planned |
| Multi-stack E2E | 80%+ | 🔨 Phase 2 |

---

## Maturity Levels

| Level | Description | Indicators |
|-------|-------------|------------|
| 1 — Template | Basic repo structure | README, Makefile, basic CI |
| 2 — Hooked | Pre-commit hooks | format, vet, test gates |
| 3 — Enforced | Branch protection + required checks | No bypass possible |
| 4 — Automated | Auto-merge, auto-dep, auto-publish | Human intervention minimal |
| 5 — Self-improving | AI-assisted review, regression detection | Factory learns |

Dark Factory targets **Level 5** for all repos.

---

## Inspiration

- **12-factor app** — Configuration from environment
- **conventionalcommits.org** — Semantic commit messages
- **Test-Driven Development** — Test first, not last
- **Enforcement patterns** from `backend/src/enforcement/` — quality gates, tool call validation
- **Scaffolding patterns** from `backend/src/scaffolding/` — deterministic project generation
- **Zero Tolerance enforcement** — Block everything that doesn't meet standards
