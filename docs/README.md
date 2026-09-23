# GitForge Documentation

This directory contains both current product documentation and historical planning notes. Use this page as the entry point.

## Canonical Docs

| Need | Document |
|------|----------|
| Master plan (phases, findings ledger) | [planning/MASTER_PLAN_2026-09-20.md](planning/MASTER_PLAN_2026-09-20.md) |
| Current improvement backlog | [planning/IMPROVEMENTS.md](planning/IMPROVEMENTS.md) |
| Current handoff / live state | [planning/HANDOFF.md](planning/HANDOFF.md) |
| Code quality audit | [AUDIT.md](AUDIT.md) |
| Architecture overview | [ARCHITECTURE.md](ARCHITECTURE.md) |
| API reference | [API.md](API.md) |
| Local operations | [RUNBOOK.md](RUNBOOK.md) |
| Deployment | [DEPLOYMENT.md](DEPLOYMENT.md) |
| Testing | [TESTING_STRATEGY.md](TESTING_STRATEGY.md) |
| Contributing | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Security policy | [SECURITY.md](SECURITY.md) |
| Recent changes | [CHANGELOG_RECENT.md](CHANGELOG_RECENT.md) |
| Authentication design | [AUTH_DESIGN.md](AUTH_DESIGN.md) |
| AI code review | [AI_CODE_REVIEW.md](AI_CODE_REVIEW.md) |
| Git hooks | [HOOKS.md](HOOKS.md) |
| macOS build plan | [MACOS_BUILD.md](MACOS_BUILD.md) |
| Branch strategy | [BRANCH_STRATEGY.md](BRANCH_STRATEGY.md) |
| Build queue plan | [BUILD_QUEUE_PLAN.md](BUILD_QUEUE_PLAN.md) |
| Dated audits | [audits/](audits/) |
| Dated handoffs | [handoffs/](handoffs/) |
| Comprehensive execution plan | [planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md](planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md) |

`audits/` holds [CODE_SMELLS_2026-09-20.md](audits/CODE_SMELLS_2026-09-20.md) and
[AEGIS_BASELINE_2026-09-20.md](audits/AEGIS_BASELINE_2026-09-20.md);
`handoffs/` holds [REPO_STATE_AUDIT_2026-09-20.md](handoffs/REPO_STATE_AUDIT_2026-09-20.md).

## Historical Or Supporting Docs

These documents may contain useful design intent, but they are not the source of truth for current implementation status:

- [HANDOFF_PLAN.md](HANDOFF_PLAN.md) — historical; declares itself superseded by the planning documents
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

1. [planning/HANDOFF.md](planning/HANDOFF.md) — live session state
2. [planning/IMPROVEMENTS.md](planning/IMPROVEMENTS.md) — live improvement backlog
3. The source files linked by the current phase
4. The verification commands in [TESTING_STRATEGY.md](TESTING_STRATEGY.md)

Historical context, useful but not authoritative: [HANDOFF_PLAN.md](HANDOFF_PLAN.md),
[planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md](planning/COMPREHENSIVE_EXECUTION_PLAN_2026-08-28.md),
and [AUDIT.md](AUDIT.md).
