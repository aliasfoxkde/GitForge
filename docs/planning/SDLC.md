# Dark Factory — SDLC Documentation

**System Development Life Cycle** — How code flows from idea to production.

---

## 1. Hook System

Dark Factory uses **two git hooks** installed via `make setup` (or `scripts/install-hooks.sh`):

### 1.1 Pre-commit Hook (`.githooks/pre-commit`)

Runs **before every `git commit`**. Blocks the commit if any check fails.

| Check | Command | Blocks On |
|-------|---------|-----------|
| gofmt | `gofmt -l .` | Any file needs formatting |
| goimports | `goimports -l .` | Any file needs import fixes |
| go vet | `go vet ./...` | `go vet` reports errors |
| Selective test | `go test -p 1 ...` | Tests fail OR coverage below threshold |
| Bundle rebuild | `go run ./bundler` | Bundle generation fails (if patterns staged) |

**Selective test behavior:**
- If `core/` files are staged → runs full test suite with coverage gate
- Otherwise → runs tests only for staged packages
- Coverage threshold: `$COVERAGE_THRESHOLD` env var (default: 70%)

**Pattern files staged:** If YAML files in `community/` or `patterns/` are staged, automatically rebuilds `core/patterns.bundle` via `go run ./bundler`.

### 1.2 Pre-push Hook (`.githooks/pre-push`)

Runs **before every `git push`**. Blocks the push if tests fail or coverage is below threshold.

| Check | Command | Blocks On |
|-------|---------|-----------|
| Full test suite | `go test -p 1 -race -timeout 15m -coverprofile=... ./...` | Tests fail OR coverage below threshold |

This is the **last line of defense** before code leaves your machine.

### 1.3 Pre-commit Framework (`.pre-commit-config.yaml`)

The pre-commit framework hooks run **before** the custom hooks on every commit attempt:

| Hook | What It Checks |
|------|---------------|
| trailing-whitespace | Lines end with trailing whitespace |
| end-of-file-fixer | Files don't end with newline |
| check-yaml | YAML files are valid |
| check-added-large-files | No files > 1MB added |
| check-merge-conflict | No merge conflict markers |
| check-case-conflict | No case-insensitive filename conflicts |
| detect-private-key | No SSH private keys committed |
| detect-aws-credentials | No AWS credentials committed |
| golangci-lint | All golangci-lint rules pass |

### 1.4 Customizing Hooks

**To skip hooks temporarily:**
```bash
git commit --no-verify -m "WIP: temporary skip"
git push --no-verify
```
⚠️ Use sparingly — hooks exist for a reason.

**To add a new check to pre-commit:**
1. Edit `.githooks/pre-commit`
2. Add the check function
3. Test: `./.githooks/pre-commit`

**To add a pre-commit framework hook:**
Add to `.pre-commit-config.yaml` under `repos[0].hooks`.

---

## 2. CI/CD Pipeline

### 2.1 Workflow Overview

```
git push / PR opened
    ↓
ci.yml (required checks)
    ├── go-test (Go 1.21-1.24 × Linux/macOS/Windows)
    ├── lint (go vet, gofmt, golangci-lint, goimports)
    ├── build (cross-platform binaries)
    ├── quality-grep (TODO/FIXME without issue refs)
    └── vuln (govulncheck)
    ↓
security.yml (on push + weekly schedule)
    ├── CodeQL (security-extended queries)
    ├── govulncheck
    ├── secrets scan (credential patterns)
    ├── dependency review
    └── security anti-pattern grep
    ↓
auto-merge.yml (on PR merge)
    └── Squash-merge if all checks pass
```

### 2.2 Required Status Checks

Branch protection requires **all three** checks to pass before merge:
- `ci/go-test` — All Go versions, all platforms
- `ci/lint` — Code quality
- `ci/build` — Cross-platform compilation

### 2.3 Dev Testing (`dev-testing.yml`)

Relaxed CI for `dev`, `dev-testing`, `development` branches:
- 50% coverage threshold (vs. 70% default)
- Non-blocking lint (continue-on-error: true)
- Full build matrix still active

Use these branches for experimental work. Merge to `main` only when ready for full gates.

### 2.4 Integration Tests (`integration.yml`)

Runs integration and E2E tests:
```bash
go test -v -tags=integration -p 1 ./...
go test -v -tags=e2e -p 1 ./...
```

Add build tags to integration/E2E tests:
```go
//go:build integration
// +build integration

func TestDatabaseMigration(t *testing.T) { ... }
```

### 2.5 Release Pipeline (`release.yml`)

**Trigger:** Push of a `v*` tag (e.g., `v1.2.3`)

**Steps:**
1. Runs GoReleaser with:
   - Cross-platform binaries (`atheon`, `atheon-mcp`)
   - amd64 + arm64 for Linux/macOS/Windows
   - `-trimpath` for reproducible builds
   - SPDX-JSON SBOM generation
2. Publishes to GitHub Releases
3. **Only runs on canonical repo** (`aliasfoxkde/Atheon-Enhanced`)

**Manual release (snapshot):**
```bash
goreleaser build --clean --snapshot --output dist/
```

---

## 3. Branch Strategy

### Branch Types

| Prefix | Purpose | Protection |
|--------|---------|------------|
| `main` | Production-ready code | Branch protection: strict + required checks + 1 review |
| `stable/` | Integration branch | Requires PR + review |
| `feature/` | New features | Standard PR |
| `fix/` | Bug fixes | Standard PR |
| `docs/` | Documentation only | Standard PR |
| `test/` | Test improvements | Standard PR |
| `refactor/` | Code refactoring | Standard PR |
| `dev` | Experimental work | Dev-testing workflow (relaxed gates) |

### Workflow

```
feature/description → PR → review → squash-merge → main
                              ↓
                    auto-delete branch (if configured)
```

### Creating a Feature Branch

```bash
git checkout main
git pull
git checkout -b feature/my-new-feature
# ... work ...
git add -A && git commit -m "feat: add my new feature"
git push -u origin feature/my-new-feature
# Open PR on GitHub
# CI runs → if green, 1 review required → squash-merge
```

---

## 4. Pull Request Process

### 4.1 Before Opening a PR

- [ ] `go vet ./...` passes
- [ ] `go test -p 1 ./...` passes
- [ ] `go build ./...` passes
- [ ] `golangci-lint run --timeout=5m` passes
- [ ] `gofmt -l .` shows no files
- [ ] Coverage maintained or improved
- [ ] CHANGELOG updated (if applicable)
- [ ] Conventional commit format in title

### 4.2 PR Title Format

```
<type>: <description>

Types: feat, fix, docs, test, refactor, ci, chore, perf
```

Examples:
- `feat: add user authentication via OAuth`
- `fix: correct pattern matching for JWT tokens`
- `docs: update installation instructions`
- `test: add coverage for error handling paths`

### 4.3 PR Description

Use the PR template (`.github/PULL_REQUEST_TEMPLATE.md`):
- Root cause analysis (required for `fix:` prefix)
- Summary of changes
- Testing checklist
- Breaking changes

### 4.4 Review Process

1. CODEOWNERS automatically requested for review
2. Reviewer approves or requests changes
3. All CI checks must pass
4. 1 approving review required
5. Squash and merge → branch auto-deleted

### 4.5 Auto-merge

Dependabot PRs and same-repo PRs auto-merge if:
- Not a draft
- All CI checks pass
- No conflicts

---

## 5. Release Process

### 5.1 Release Checklist

1. All tests passing on `main`
2. Coverage above threshold
3. No open critical issues
4. CHANGELOG updated
5. Version bumped (if applicable)

### 5.2 Cutting a Release

```bash
# Ensure main is up to date
git checkout main && git pull

# Create release tag
git tag v1.2.3
git push origin v1.2.3

# → release.yml workflow triggers automatically
# → GoReleaser builds + publishes binaries
# → GitHub Release created
```

### 5.3 Hotfix Process

```bash
git checkout main
git pull
git checkout -b fix/critical-security-issue
# ... fix ...
git push -u origin fix/critical-security-issue
# Open PR → expedite review → squash-merge → main
# Then tag + release
```

---

## 6. Testing Strategy

### 6.1 Coverage Targets

| Component | Minimum Coverage |
|-----------|-----------------|
| Core business logic | 95% |
| API handlers | 90% |
| Configuration | 85% |
| Utilities | 85% |
| E2E tests | 80% |

### 6.2 Incremental Coverage

The key principle: **build coverage as you work, not at the end.**

Every PR should increase or maintain coverage. If a PR decreases coverage:
- CI blocks the merge
- Pre-push hook blocks the push

### 6.3 How to Write Tests

**1. Write the failing test first:**
```go
func TestFindPattern_MatchesJWT(t *testing.T) {
    scanner := NewScanner()
    results := scanner.Scan("Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9")
    require.Len(t, results, 1, "should detect JWT token")
    require.Equal(t, "jwt-token", results[0].Pattern.Name)
}
```

**2. Run it to see it fail:**
```bash
go test -p 1 -v ./core/... -run TestFindPattern_MatchesJWT
```

**3. Write the code to make it pass:**
```go
func (s *Scanner) Scan(input string) []Match {
    for _, pattern := range s.patterns {
        if pattern.Match.MatchString(input) {
            s.results = append(s.results, Match{Pattern: pattern})
        }
    }
    return s.results
}
```

**4. Verify coverage:**
```bash
go test -p 1 -coverprofile=coverage.out ./...
go tool cover -func=coverage.out | grep total
```

### 6.4 E2E Testing

E2E tests live in `e2e-tests/` or tagged with `//go:build e2e`:

```go
//go:build e2e
// +build e2e

package e2e

import "testing"

func TestE2E_FullScan(t *testing.T) {
    // Full end-to-end test
}
```

Run E2E tests:
```bash
# Via integration workflow
go test -v -tags=e2e -p 1 ./...

# Or via runner script
./scripts/run-e2e.sh
```

---

## 7. Environment Variables

Dark Factory uses environment variables for all configuration (12-factor app):

| Variable | Default | Description |
|----------|---------|-------------|
| `COVERAGE_THRESHOLD` | 70 | Minimum test coverage % |
| `GO_VERSIONS` | 1.21-1.24 | Go versions to test |
| `GOLANGCI_LINT_VERSION` | v2.1.6 | golangci-lint version |
| `DEFAULT_BRANCH` | main | Default branch name |

Set these in GitHub Settings → Variables, or in your shell:
```bash
export COVERAGE_THRESHOLD=80
export GO_VERSIONS='["1.21","1.22","1.23","1.24"]'
```

---

## 8. Monitoring & Observability

### 8.1 CI/CD Monitoring

- **GitHub Actions** — Workflow runs at `https://github.com/owner/repo/actions`
- **Codecov** — Coverage reports at `https://codecov.io`
- **GitHub Security** — Advisories, vulnerability alerts

### 8.2 What to Monitor

| Metric | Target | Alert If |
|--------|--------|----------|
| Test coverage | ≥ 70% | Below threshold |
| Lint errors | 0 | Any failures |
| Build failures | 0 | Any failures |
| Security vulnerabilities | 0 critical | Any critical |
| Stale branches | < 5 | More than 5 |

### 8.3 Alerting

Enable GitHub email notifications for:
- Failed workflow runs
- Security vulnerability alerts
- Dependabot PRs (security updates)

---

## 9. Security

### 9.1 Secrets Management

- **Never commit secrets** — Use environment variables or a secrets manager
- **Pre-commit hook** detects some secrets — not a substitute for a vault
- **GitGuardian / TruffleHog** recommended for production repos

### 9.2 Dependency Security

- **Dependabot** — Automatically opens PRs for vulnerable dependencies
- **govulncheck** — Runs weekly in `security.yml`
- **CodeQL** — Runs on every push, detects vulnerable patterns

### 9.3 Reporting Vulnerabilities

See `SECURITY.md` for the full security policy. Summary:
1. Report privately via GitHub Advisories
2. 72-hour acknowledgement
3. 7-day initial assessment
