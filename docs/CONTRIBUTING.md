# Contributing to Dark Factory

Thank you for your interest in contributing!

## Branch Strategy

```
main (production)
  └── stable (integration)
       ├── feature/description
       ├── fix/description
       ├── docs/description
       ├── test/description
       └── refactor/description
```

## Commit Convention

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add new feature
fix: fix a bug
docs: documentation only
test: adding or updating tests
refactor: code refactoring (no behavior change)
ci: CI/CD changes
chore: maintenance tasks
perf: performance improvements
```

**Fix format:** `fix: correct X — was doing Y instead of Z`

## Pull Request Checklist

- [ ] Conventional commit format in title
- [ ] Root cause analysis in PR description (for `fix:` prefix)
- [ ] `go vet ./...` passes
- [ ] `go test -p 1 ./...` passes
- [ ] `go build ./...` passes
- [ ] `golangci-lint run --timeout=5m` passes
- [ ] `gofmt -l .` shows no files
- [ ] Coverage maintained or improved
- [ ] CODEOWNERS review required
- [ ] CHANGELOG updated (if applicable)
- [ ] ADR added/updated (for architectural decisions)

## Coding Standards

- **Sentinel errors** — `var ErrXxx = errors.New("...")`
- **Context propagation** — All public APIs accept `context.Context`
- **Structured logging** — `log/slog` (Go), not `fmt.Fprintf`
- **Error wrapping** — `fmt.Errorf("context: %w", err)`
- **Tests** — Write tests alongside code, not deferred

## Coverage Requirements

| Component | Minimum |
|-----------|---------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utilities | 85% |

## Documentation

Every exported function must have a doc comment explaining:
1. What it does
2. What inputs it accepts
3. What outputs it produces
4. Any side effects or error conditions

## Security

- Never commit secrets, credentials, or API keys
- Use environment variables for all configuration
- Report security issues via GitHub Advisories, not public issues