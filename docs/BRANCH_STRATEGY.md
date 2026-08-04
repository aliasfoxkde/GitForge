# Branch Strategy

## Overview

Dark Factory uses a simplified trunk-based development model with short-lived feature branches.

## Branch Types

```
main (production)
  └── [short-lived feature/fix/docs/test/refactor branches]
       ↓
       PR → review → squash-merge → main
```

### Main Branch

- **Name:** `main`
- **Protection:** Strict branch protection, required CI checks, 1 review
- **State:** Always production-ready
- **History:** Preserved via merge commits

### Feature Branches

| Prefix | Purpose | Example |
|--------|---------|---------|
| `feature/` | New functionality | `feature/oauth-authentication` |
| `fix/` | Bug fixes | `fix/jwt-token-detection` |
| `docs/` | Documentation only | `docs/api-reference` |
| `test/` | Test improvements | `test/increase-pattern-coverage` |
| `refactor/` | Code restructuring | `refactor/scanner-interface` |
| `ci/` | CI/CD improvements | `ci/add-codecov` |
| `perf/` | Performance work | `perf/regex-compilation` |

### Dev Branches

| Prefix | Purpose | Gates |
|--------|---------|-------|
| `dev/` | Experimental work | Dev-testing workflow (50% coverage, non-blocking lint) |
| `dev-testing/` | Integration testing | Dev-testing workflow |
| `development/` | Staging integration | Dev-testing workflow |

## Workflow

### Creating a Feature Branch

```bash
git checkout main
git pull
git checkout -b feature/my-feature
# ... work ...
git push -u origin feature/my-feature
```

### Merging

1. Ensure all CI checks pass
2. Open PR against `main`
3. CODEOWNERS review
4. Squash and merge
5. Branch auto-deleted

### Hotfix Process

```bash
git checkout main
git pull
git checkout -b fix/critical-issue
# Fix...
git push -u origin fix/critical-issue
# PR → expedite review → squash-merge
# Then tag and release
```

## Release Branches

We do **not** maintain long-lived release branches. Every tag on `main` is a release candidate.

```bash
git tag v1.2.3
git push origin v1.2.3
# → GoReleaser builds and publishes
```

## Best Practices

- **Short-lived branches** — PRs should be merged within 1-2 days
- **Atomic commits** — One logical change per commit
- **Conventional commits** — `feat:`, `fix:`, `docs:`, etc.
- **Reference issues** — Link PRs to issues: `Closes #123`
- **No force-pushes** to `main` or protected branches
- **Delete old branches** — Auto-deleted on merge (if configured)