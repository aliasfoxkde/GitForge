# GitForge Documentation

This directory contains both current product documentation and historical planning notes. Use this page as the entry point.

## Canonical Docs

| Need | Document |
|------|----------|
| Current improvement backlog | [HANDOFF_PLAN.md](HANDOFF_PLAN.md) |
| Code quality audit | [AUDIT.md](AUDIT.md) |
| Architecture overview | [ARCHITECTURE.md](ARCHITECTURE.md) |
| API reference | [API.md](API.md) |
| Local operations | [RUNBOOK.md](RUNBOOK.md) |
| Deployment | [DEPLOYMENT.md](DEPLOYMENT.md) |
| Testing | [TESTING_STRATEGY.md](TESTING_STRATEGY.md) |
| Contributing | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Security policy | [SECURITY.md](SECURITY.md) |

## Historical Or Supporting Docs

These documents may contain useful design intent, but they are not the source of truth for current implementation status:

- [PLAN.md](PLAN.md)
- [PLAN_NEXT_PHASE.md](PLAN_NEXT_PHASE.md)
- [MASTER_PLAN.md](MASTER_PLAN.md)
- [TASKS.md](TASKS.md)
- [PROGRESS.md](PROGRESS.md)
- [planning/](planning/)
- [project_notes/](project_notes/)
- [template-dev/](template-dev/)

When there is disagreement, prefer the canonical docs above and verify against code.

## Handoff Rule

Follow-on AI agents should start with:

1. [AUDIT.md](AUDIT.md)
2. [HANDOFF_PLAN.md](HANDOFF_PLAN.md)
3. The source files linked by the current phase
4. The verification commands in [TESTING_STRATEGY.md](TESTING_STRATEGY.md)
