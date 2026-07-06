# AGENTS.md

This file provides agent behavior rules for AI assistants working in this repository.

## Agent Behavior Rules

### Before Acting

- Always verify changes with tests before committing
- Check coverage impact before merging
- Read existing code patterns before introducing new ones
- Use `grep` / `search_graph` before creating new files

### Escalation Triggers

Escalate to human review when:
- Security vulnerabilities detected
- Breaking API changes proposed
- Coverage drops >5%
- Unknown error patterns in logs
- Decisions affecting project architecture

### Commit Generation

Use conventional commit format:
```
<type>(<scope>): <description>

Types: feat, fix, docs, test, refactor, ci, chore, perf
Scope: optional, e.g., api, auth, config

Examples:
  feat(api): add user authentication endpoint
  fix(auth): resolve null pointer in token validation
  docs: update API documentation
  refactor(db): extract connection pooling
```

### Test Requirements

- Write failing test before implementing code
- Core business logic: 95% minimum coverage
- API handlers: 90% minimum coverage
- Configuration: 85% minimum coverage
- Use table-driven tests for multiple input cases

### PR/Branch Interaction

- Create branch for each feature/fix: `feature/description` or `fix/description`
- Never commit directly to `main` or `stable`
- All PRs require CODEOWNERS review
- Use `make lint` and `make test` before requesting review

### Coverage Enforcement

- Block commits that reduce coverage below thresholds
- Run `make coverage` to check before pushing
- Request exemption from coverage rules with justification

### Code Quality Gates

- `go vet ./...` passes
- `golangci-lint run --timeout=5m` passes
- `gofmt -l .` shows no files
- No new `//golint:disable` without justification
- No debug/temporary code committed
