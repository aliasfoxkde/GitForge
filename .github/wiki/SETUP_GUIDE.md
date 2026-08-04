# Setup Guide

Initial configuration steps after creating a repository from the Dark Factory template.

## Prerequisites

- [ ] Git installed (2.40+)
- [ ] Go installed (1.21+) — if using Go templates
- [ ] `gh` CLI authenticated (`gh auth login`)
- [ ] Repository created from template

## Initial Setup

### 1. Clone Your Repository

```bash
git clone https://github.com/YOUR_USERNAME/YOUR_PROJECT.git
cd YOUR_PROJECT
```

### 2. Install Git Hooks

```bash
make setup
```

This installs pre-commit and pre-push hooks.

### 3. Replace Placeholders

Replace `aliasfoxkde` with your GitHub username in:

```bash
# Automatic replacement
find . -type f \( -name "*.yml" -o -name "*.yaml" -o -name "*.md" -o -name "*.json" \) \
    -exec sed -i 's/aliasfoxkde/YOUR_USERNAME/g' {} \;
```

Files to check manually:
- `.github/CODEOWNERS`
- `.github/FUNDING.yml`
- `README.md`

### 4. Set Up Environment

```bash
# Copy environment template
cp .env.example .env

# Edit with your values
vim .env
```

### 5. Verify Installation

```bash
make lint   # Should pass
make test   # Should pass
make build  # Should pass
```

## GitHub Configuration

### Required GitHub Apps

Install these apps from GitHub Marketplace:

| App | Purpose |
|-----|---------|
| Codecov | Coverage tracking |
| Dependabot | Dependency updates |

### Required Permissions

Ensure repository has:
- **Wiki**: Enabled (for documentation)
- **Issues**: Enabled (for issue tracking)
- **Projects**: Enabled (for project boards)

### Secrets Configuration

Add these repository secrets:

| Secret | Description |
|--------|-------------|
| `CODECOV_TOKEN` | For coverage uploads |
| `GITHUB_TOKEN` | Automatically provided |

### Branch Protection

After first push, verify branch protection:

```bash
# Check rulesets
gh api repos/OWNER/REPO/rulesets --jq '.[] | "\(.name): \(.enforcement)"'
```

## CI/CD Verification

### Trigger First CI Run

```bash
git add -A
git commit -m "feat: initial commit"
git push -u origin main
```

### Verify Checks Pass

1. Go to your repository on GitHub
2. Click on the commit
3. Verify all checks pass:
   - `go-test`
   - `lint`
   - `build`

### Common CI Issues

| Issue | Solution |
|-------|----------|
| Coverage below threshold | Add more tests |
| `golangci-lint` failures | Fix lint errors |
| Build fails | Check Go version matches |

## Updating from Template

To pull in latest template changes:

```bash
# Add template as remote
git remote add template https://github.com/aliasfoxkde/dark-factory.git

# Fetch template changes
git fetch template

# Review template changes
git log --oneline template/main..HEAD

# Merge template changes
git merge template/main --allow-unrelated-histories
```

## Getting Help

- [GitHub Discussions](https://github.com/aliasfoxkde/dark-factory/discussions)
- [Troubleshooting Guide](./TROUBLESHOOTING.md)
