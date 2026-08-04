# Git Hooks Documentation

Dark Factory uses two custom git hooks plus the pre-commit framework.

## Overview

| Hook | Trigger | Purpose |
|------|---------|---------|
| `.githooks/pre-commit` | Every `git commit` | Format, vet, quality gates, test, coverage |
| `.githooks/pre-push` | Every `git push` | Full test suite with race detection |
| `.pre-commit-config.yaml` | Via pre-commit framework | Shell safety, secrets, YAML, etc. |

## Installation

```bash
make setup
# or
./scripts/install-hooks.sh
```

This sets `git config core.hooksPath .githooks`.

## Pre-commit Hook (`.githooks/pre-commit`)

Runs **before every commit**. Blocks if any check fails.

### Checks (in order)

1. **gofmt** — All Go files must be formatted
2. **goimports** — All Go files must have correct imports
3. **go vet** — `go vet ./...` must pass
4. **Quality Gates** — Secrets, console.log, fake data, placeholder code
5. **Selective Test** — Full suite if `core/` changed; staged package tests otherwise
6. **Coverage Gate** — Coverage must be above threshold
7. **Bundle Rebuild** — Auto-rebuilds pattern bundle if pattern files staged

### Quality Gates (`.githooks/quality-gates`)

Blocks commits containing:
- Hardcoded secrets (API keys, tokens, passwords, private keys)
- Bare console logging (print, console.log, fmt.Print — not structured loggers)
- Fake/placeholder data (placeholder_, fake_, example.com)
- Placeholder code (TODO/FIXME/HACK without issue references)

### Skipping Hooks

```bash
# Skip all hooks
git commit --no-verify -m "WIP: temporary"

# Skip only tests (not format/lint)
git commit -m "WIP" --no-verify
```

⚠️ Use sparingly. Hooks exist to protect the repo.

## Pre-push Hook (`.githooks/pre-push`)

Runs **before every push**. Blocks if the full test suite fails or coverage drops below threshold.

### Checks

1. Full test suite: `go test -p 1 -race -timeout 15m -coverprofile=... ./...`
2. Coverage gate: Must meet `COVERAGE_THRESHOLD` (default: 70%)

This is the **last line of defense** before code reaches CI.

## Pre-commit Framework (`.pre-commit-config.yaml`)

Runs via the pre-commit framework. Installed separately: `pre-commit install`.

### What It Checks

| Hook | Purpose |
|------|---------|
| trailing-whitespace | No trailing whitespace |
| end-of-file-fixer | Files end with newline |
| check-yaml | YAML files are valid |
| check-added-large-files | No files > 1MB |
| check-merge-conflict | No `<<<<<<<` markers |
| check-case-conflict | No case conflicts (e.g. `File.ts` vs `file.ts`) |
| detect-private-key | No SSH private keys |
| detect-aws-credentials | No AWS credentials |
| golangci-lint | All golangci-lint rules |

## Customizing

### Add a New Quality Gate

Edit `.githooks/quality-gates`:

```bash
# Add after the existing checks:
fake_data_check() {
    local file="$1"
    # ... check logic ...
}
```

### Add a New Pre-commit Check

Edit `.githooks/pre-commit`, add after `go vet`:

```bash
INFO "Running my new check..."
if ! my_new_check; then
    ERROR "My new check failed"
    exit 1
fi
```

### Change Coverage Threshold

```bash
export COVERAGE_THRESHOLD=80
git commit ...
# Or set in GitHub Settings → Variables
```

## Troubleshooting

### Hook Not Running

```bash
# Check hooks path
git config core.hooksPath

# Re-install
make setup
```

### Hook Blocks a Legitimate Commit

If a quality gate is too strict for a valid use case:
1. Fix the underlying issue
2. Or use `--no-verify` with a comment explaining why
3. Or open an issue to discuss the gate rule

### Pre-commit Framework Not Installed

```bash
pip install pre-commit
pre-commit install
```